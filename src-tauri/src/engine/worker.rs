//! The per-pair sync worker.
//!
//! Lifecycle: initialise (load state, list Drive if needed, scan local, start watcher) → full
//! reconcile → then react to native file events (debounced, re-listing only affected folders),
//! Drive change-feed polls, retry timers and periodic safety rescans.

use super::plan::{self, Decision};
use super::{Cmd, Shared};
use crate::config::PairConfig;
use crate::db::{Batch, StateDb};
use crate::drive::client::{is_network, is_not_found};
use crate::drive::{Drive, DriveFile};
use crate::fsutil::{self, is_within, last_component, parent_key};
use crate::hub::{now_ms, ActivityKind as AK, PairState, PendingDeletes};
use crate::ignore_rules::Rules;
use crate::model::{self, BaseEntry, BaseMap, LocalEntry, LocalMap, RemoteView};
use crate::remote_tree::RemoteTree;
use crate::scanner::{self, Layout};
use crate::watcher::{Dirty, PairWatcher};
use anyhow::{anyhow, bail, Context, Result};
use futures_util::stream::{self, StreamExt};
use parking_lot::Mutex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

const MAX_EVENT_LATENCY: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(60);
const FLUSH_EVERY: usize = 200;

enum Scope {
    All,
    Keys(BTreeSet<String>),
}

struct Worker {
    sh: Arc<Shared>,
    cfg: PairConfig,
    paused: Arc<AtomicBool>,
    dirty: Arc<Mutex<Dirty>>,
    wake: Arc<Notify>,
    db: Option<StateDb>,
    batch: Batch,
    layout: Layout,
    rules: Rules,
    l: LocalMap,
    b: BaseMap,
    tree: RemoteTree,
    page_token: Option<String>,
    drive: Option<Drive>,
    watcher: Option<PairWatcher>,
    unavailable: Vec<String>,
    mount_warnings: Vec<String>,
    retry: BTreeSet<String>,
    retry_at: Option<Instant>,
    pending_deletes: Vec<String>,
    allow_deletes_once: bool,
    next_poll: Instant,
    next_full: Instant,
    debounce_at: Option<Instant>,
    first_event: Option<Instant>,
    recent_errors: u64,
    fatal: Option<String>,
}

pub async fn run(sh: Arc<Shared>, cfg: PairConfig, mut rx: mpsc::UnboundedReceiver<Cmd>, paused: Arc<AtomicBool>) {
    let now = Instant::now();
    let mut w = Worker {
        layout: Layout::new(&cfg.local_path, &cfg.mounts),
        tree: RemoteTree::new(&cfg.remote_folder_id),
        sh,
        cfg,
        paused,
        dirty: Arc::new(Mutex::new(Dirty::default())),
        wake: Arc::new(Notify::new()),
        db: None,
        batch: Batch::default(),
        rules: Rules::empty(),
        l: LocalMap::new(),
        b: BaseMap::new(),
        page_token: None,
        drive: None,
        watcher: None,
        unavailable: Vec::new(),
        mount_warnings: Vec::new(),
        retry: BTreeSet::new(),
        retry_at: None,
        pending_deletes: Vec::new(),
        allow_deletes_once: false,
        next_poll: now,
        next_full: now,
        debounce_at: None,
        first_event: None,
        recent_errors: 0,
        fatal: None,
    };

    // ---- initialisation (retried until it works) ----
    let mut backoff = Duration::from_secs(15);
    loop {
        if w.is_paused() {
            w.set_state(PairState::Paused, "Paused");
            match rx.recv().await {
                None | Some(Cmd::Shutdown) => return,
                _ => continue,
            }
        }
        match w.init().await {
            Ok(()) => break,
            Err(e) => {
                let offline = is_network(&e);
                w.set_error(&e, offline);
                let deadline = Instant::now() + backoff;
                backoff = (backoff * 2).min(Duration::from_secs(300));
                loop {
                    tokio::select! {
                        c = rx.recv() => match c {
                            None | Some(Cmd::Shutdown) => return,
                            Some(Cmd::SyncNow) | Some(Cmd::SetPaused(false)) => break,
                            _ => {}
                        },
                        _ = tokio::time::sleep_until(deadline) => break,
                    }
                }
            }
        }
    }

    w.full_cycle().await;

    // ---- main event loop ----
    let far = || Instant::now() + Duration::from_secs(365 * 24 * 3600);
    loop {
        let paused = w.is_paused();
        let wake = w.wake.clone();
        let debounce = if paused { far() } else { w.debounce_at.unwrap_or_else(far) };
        let poll = if paused { far() } else { w.next_poll };
        let full = if paused { far() } else { w.next_full };
        let retry = if paused { far() } else { w.retry_at.unwrap_or_else(far) };
        enum Ev {
            Cmd(Option<Cmd>),
            Wake,
            Debounce,
            Poll,
            Full,
            Retry,
        }
        let ev = tokio::select! {
            c = rx.recv() => Ev::Cmd(c),
            _ = wake.notified() => Ev::Wake,
            _ = tokio::time::sleep_until(debounce) => Ev::Debounce,
            _ = tokio::time::sleep_until(poll) => Ev::Poll,
            _ = tokio::time::sleep_until(full) => Ev::Full,
            _ = tokio::time::sleep_until(retry) => Ev::Retry,
        };
        match ev {
            Ev::Cmd(None) | Ev::Cmd(Some(Cmd::Shutdown)) => {
                w.flush();
                return;
            }
            Ev::Cmd(Some(Cmd::SyncNow)) => {
                w.fatal = None;
                w.full_cycle().await;
            }
            Ev::Cmd(Some(Cmd::SetPaused(p))) => {
                if p {
                    w.set_state(PairState::Paused, "Paused");
                } else {
                    w.full_cycle().await;
                }
            }
            Ev::Cmd(Some(Cmd::ConfirmDeletes(apply))) => {
                if apply {
                    w.allow_deletes_once = true;
                } else {
                    // Restore: forget the base for withheld deletions so the surviving copies are
                    // treated as new and propagate back to the side where they were deleted.
                    for k in std::mem::take(&mut w.pending_deletes) {
                        for gone in model::remove_subtree(&mut w.b, &k) {
                            w.batch.base_del.push(gone);
                        }
                    }
                    w.flush();
                }
                w.full_cycle().await;
                w.allow_deletes_once = false;
            }
            Ev::Wake => {
                let now = Instant::now();
                let first = *w.first_event.get_or_insert(now);
                let ms = w.sh.config.read().debounce_ms;
                w.debounce_at = Some((now + Duration::from_millis(ms)).min(first + MAX_EVENT_LATENCY));
            }
            Ev::Debounce => {
                w.debounce_at = None;
                w.first_event = None;
                w.local_cycle().await;
            }
            Ev::Poll => w.remote_cycle().await,
            Ev::Full => w.full_cycle().await,
            Ev::Retry => {
                w.retry_at = None;
                let keys = std::mem::take(&mut w.retry);
                if !keys.is_empty() {
                    w.guarded(Scope::Keys(keys)).await;
                }
            }
        }
    }
}

struct UploadJob {
    key: String,
    real: String,
    abs: PathBuf,
    name: String,
    parent_id: String,
    existing: Option<String>,
    expect: LocalEntry,
}

struct DownloadJob {
    key: String,
    abs: PathBuf,
    remote: RemoteView,
    expect: Option<LocalEntry>,
}

enum Job {
    Up(UploadJob),
    Down(DownloadJob),
}

enum JobResult {
    Uploaded { key: String, real: String, file: DriveFile, parent_id: String, local: LocalEntry, stable: bool, replaced: bool },
    Downloaded { key: String, local: LocalEntry, remote: RemoteView },
    Skipped { key: String, why: String },
    Failed { key: String, path: String, err: anyhow::Error },
}

impl Worker {
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    fn drive(&self) -> Result<Drive> {
        self.drive.clone().ok_or_else(|| anyhow!("not connected"))
    }

    fn set_state(&self, state: PairState, msg: &str) {
        let (l, r) = (self.l.len() as u64, self.tree.len() as u64);
        self.sh.hub.update(&self.cfg.id, |s| {
            s.state = state;
            s.message = msg.to_string();
            s.local_items = l;
            s.remote_items = r;
            if !matches!(state, PairState::Error | PairState::Offline) {
                s.error = None;
            }
        });
    }

    fn set_error(&self, e: &anyhow::Error, offline: bool) {
        let msg = format!("{e:#}");
        self.sh.hub.update(&self.cfg.id, |s| {
            s.state = if offline { PairState::Offline } else { PairState::Error };
            s.message = if offline { "Offline — waiting for network".into() } else { "Needs attention".into() };
            s.error = Some(msg.clone());
        });
        if !offline {
            self.sh.hub.activity(&self.cfg.id, AK::Error, "", msg);
        }
    }

    fn act(&self, kind: AK, path: &str, detail: impl Into<String>) {
        self.sh.hub.activity(&self.cfg.id, kind, path, detail);
    }

    fn flush(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        let b = std::mem::take(&mut self.batch);
        if let Some(db) = self.db.as_mut() {
            if let Err(e) = db.apply(&b) {
                tracing::error!("state db write failed: {e:#}");
            }
        }
    }

    // ------------------------------------------------------------------------------------
    // Initialisation
    // ------------------------------------------------------------------------------------

    fn check_local_root(&self) -> Result<()> {
        match std::fs::metadata(&self.layout.root) {
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => bail!("the local folder {} is not a directory", self.layout.root.display()),
            Err(e) => bail!(
                "the local folder {} is not available ({e}). Syncing is on hold so nothing gets deleted; reconnect the drive or fix the path",
                self.layout.root.display()
            ),
        }
    }

    fn rebuild_rules(&mut self) {
        let cfg = self.sh.config.read();
        let global = self.cfg.use_global_ignore.then(|| cfg.global_ignore.clone());
        drop(cfg);
        let (rules, errors) = Rules::new(global.as_deref(), &self.cfg.ignore);
        self.rules = rules;
        self.sh.hub.update(&self.cfg.id, |s| s.rule_errors = errors);
    }

    async fn init(&mut self) -> Result<()> {
        self.set_state(PairState::Starting, "Starting");
        self.check_local_root()?;
        self.rebuild_rules();

        // External folders: make sure the parent of each graft point exists inside the root.
        self.mount_warnings.clear();
        for m in self.layout.mounts.clone() {
            let parent = parent_key(&m.real);
            if !parent.is_empty() && self.layout.mount_for(parent).is_none() {
                let _ = std::fs::create_dir_all(fsutil::join_rel(&self.layout.root, parent));
            }
            if !m.target.is_dir() {
                self.mount_warnings.push(format!("External folder {} is not available; \"{}\" is on hold", m.target.display(), m.real));
            }
        }

        let db_path = self.sh.paths.pair_db(&self.cfg.id);
        let mut db = StateDb::open(&db_path).with_context(|| format!("opening {}", db_path.display()))?;
        let fingerprint = format!("{}|{}|{}", self.cfg.remote_folder_id, self.layout.root.display(), self.cfg.account_id);
        if db.get_meta("fingerprint")?.as_deref() != Some(fingerprint.as_str()) {
            db.reset()?;
            let mut b = Batch::default();
            b.meta.push(("fingerprint".into(), fingerprint));
            db.apply(&b)?;
        }
        self.b = db.load_base()?;
        self.tree = RemoteTree::new(&self.cfg.remote_folder_id);
        for n in db.load_remote()? {
            self.tree.upsert(n);
        }
        self.page_token = db.get_meta("page_token")?;
        self.db = Some(db);

        let drive = self.sh.drive(&self.cfg.account_id)?;
        self.drive = Some(drive.clone());
        let root = drive.get(&self.cfg.remote_folder_id).await.context("checking the Drive folder")?;
        if root.trashed {
            bail!("the Drive folder \"{}\" is in the trash; restore it or edit this sync pair", self.cfg.remote_path_display);
        }

        if self.page_token.is_none() || self.tree.len() == 0 {
            self.initial_listing(&drive).await?;
        }

        self.set_state(PairState::Scanning, "Scanning local files");
        self.start_watcher();
        self.act(AK::Info, "", format!("Connected to \"{}\"", self.cfg.remote_path_display));
        Ok(())
    }

    async fn initial_listing(&mut self, drive: &Drive) -> Result<()> {
        self.set_state(PairState::Scanning, "Listing Drive folder");
        // Take the token *before* listing so changes made meanwhile are replayed afterwards.
        let token = drive.start_page_token(self.cfg.drive_id.as_deref()).await?;
        let mut tree = RemoteTree::new(&self.cfg.remote_folder_id);
        let is_drive_root = match &self.cfg.drive_id {
            Some(d) => d == &self.cfg.remote_folder_id,
            None => drive.get("root").await.map(|r| r.id == self.cfg.remote_folder_id).unwrap_or(false),
        };
        if is_drive_root {
            for f in drive.list_all(self.cfg.drive_id.as_deref()).await? {
                if let Some(p) = f.parents.first() {
                    tree.upsert(f.to_node(p));
                }
            }
            tree.prune_detached();
        } else {
            list_subtree(drive, &mut tree, &self.cfg.remote_folder_id).await?;
        }
        self.tree = tree;
        self.batch.remote_put.extend(self.tree.all_nodes().cloned());
        self.batch.meta.push(("page_token".into(), token.clone()));
        self.page_token = Some(token);
        self.flush();
        Ok(())
    }

    fn start_watcher(&mut self) {
        let dirs: Vec<PathBuf> = Vec::new();
        match PairWatcher::start(&self.layout.watch_roots().into_iter().filter(|p| p.is_dir()).collect::<Vec<_>>(), &dirs, self.dirty.clone(), self.wake.clone()) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => {
                self.act(AK::Warning, "", format!("File watching unavailable ({e}); relying on periodic rescans"));
                self.watcher = None;
            }
        }
    }

    // ------------------------------------------------------------------------------------
    // Cycles
    // ------------------------------------------------------------------------------------

    async fn full_cycle(&mut self) {
        if self.is_paused() {
            self.set_state(PairState::Paused, "Paused");
            return;
        }
        let rescan_mins = self.sh.config.read().full_rescan_mins;
        let limited = self.watcher.as_ref().map(|w| w.limited).unwrap_or(true);
        self.next_full = Instant::now() + Duration::from_secs(if limited { rescan_mins.min(10) } else { rescan_mins } * 60);
        if let Err(e) = self.check_local_root() {
            self.set_error(&e, false);
            return;
        }
        if self.watcher.is_none() {
            self.start_watcher();
        }
        self.rebuild_rules();
        // drain any queued events: a full scan supersedes them
        *self.dirty.lock() = Dirty::default();
        self.debounce_at = None;
        self.first_event = None;

        if !self.poll_remote().await {
            return;
        }
        self.set_state(PairState::Scanning, "Scanning local files");
        let layout = self.layout.clone();
        let rules = std::mem::replace(&mut self.rules, Rules::empty());
        let (res, rules) = tokio::task::spawn_blocking(move || {
            let r = scanner::scan_tree(&layout, &rules, "", "");
            (r, rules)
        })
        .await
        .expect("scan task");
        self.rules = rules;
        self.l = res.entries.into_iter().collect();
        self.unavailable = res.unreadable;
        self.add_missing_mounts_to_unavailable();
        if let Some(w) = self.watcher.as_mut() {
            w.add_dirs(&res.dirs);
        }
        self.guarded(Scope::All).await;
    }

    fn add_missing_mounts_to_unavailable(&mut self) {
        for m in &self.layout.mounts {
            if !m.target.is_dir() {
                self.unavailable.push(m.key.clone());
            }
        }
    }

    /// Polls the Drive change feed. Returns false when the remote is unreachable.
    async fn poll_remote(&mut self) -> bool {
        let poll_secs = self.sh.config.read().remote_poll_secs;
        self.next_poll = Instant::now() + Duration::from_secs(poll_secs);
        match self.apply_remote_changes().await {
            Ok(_) => true,
            Err(e) => {
                let offline = is_network(&e);
                self.set_error(&e, offline);
                if offline {
                    self.next_poll = Instant::now() + Duration::from_secs(30);
                }
                false
            }
        }
    }

    async fn remote_cycle(&mut self) {
        let poll_secs = self.sh.config.read().remote_poll_secs;
        self.next_poll = Instant::now() + Duration::from_secs(poll_secs);
        match self.apply_remote_changes().await {
            Ok(keys) if !keys.is_empty() => self.guarded(Scope::Keys(keys)).await,
            Ok(_) => {
                // recover from a previous offline/error state
                let st = self.sh.hub.status(&self.cfg.id).state;
                if matches!(st, PairState::Offline) || (st == PairState::Error && self.fatal.is_none()) {
                    self.full_cycle().await;
                }
            }
            Err(e) => {
                let offline = is_network(&e);
                self.set_error(&e, offline);
            }
        }
    }

    async fn apply_remote_changes(&mut self) -> Result<BTreeSet<String>> {
        let drive = self.drive()?;
        let Some(token) = self.page_token.clone() else {
            self.initial_listing(&drive).await?;
            return Ok(BTreeSet::from([String::new()]));
        };
        let (changes, new_token) = drive.changes(&token, self.cfg.drive_id.as_deref()).await?;
        let mut dirty = BTreeSet::new();
        let mut to_list = Vec::new();
        for ch in changes {
            let Some(id) = ch.file_id.clone().or_else(|| ch.file.as_ref().map(|f| f.id.clone())) else { continue };
            if id == self.cfg.remote_folder_id {
                if ch.removed || ch.file.as_ref().is_some_and(|f| f.trashed) {
                    self.fatal = Some("Drive folder removed".into());
                    bail!("the Drive folder \"{}\" was deleted or trashed; syncing is on hold", self.cfg.remote_path_display);
                }
                continue;
            }
            let old_key = self.tree.path_of(&id).map(|(k, _)| k);
            let live = ch.file.filter(|f| !ch.removed && !f.trashed);
            let parent = live.as_ref().and_then(|f| f.parents.iter().find(|p| self.tree.is_known_folder(p)).cloned());
            match (live, parent) {
                (Some(f), Some(p)) => {
                    let existed = self.tree.contains(&id);
                    let node = f.to_node(&p);
                    if self.tree.get(&id) == Some(&node) {
                        continue; // echo of our own change
                    }
                    self.tree.upsert(node.clone());
                    self.batch.remote_put.push(node);
                    if f.is_folder() && !existed {
                        to_list.push(id.clone());
                    }
                    if let Some((k, _)) = self.tree.path_of(&id) {
                        dirty.insert(k);
                    }
                }
                _ => {
                    if self.tree.contains(&id) {
                        let removed = self.tree.remove_subtree(&id);
                        self.batch.remote_del.extend(removed);
                    }
                }
            }
            if let Some(k) = old_key {
                dirty.insert(k);
            }
        }
        for id in to_list {
            let mut sub = RemoteTree::new(&id);
            list_subtree(&drive, &mut sub, &id).await?;
            for n in sub.all_nodes() {
                self.tree.upsert(n.clone());
                self.batch.remote_put.push(n.clone());
            }
            if let Some((k, _)) = self.tree.path_of(&id) {
                dirty.insert(k);
            }
        }
        if Some(&new_token) != self.page_token.as_ref() {
            self.batch.meta.push(("page_token".into(), new_token.clone()));
            self.page_token = Some(new_token);
        }
        self.flush();
        Ok(dirty)
    }

    async fn local_cycle(&mut self) {
        let d = std::mem::take(&mut *self.dirty.lock());
        if d.overflow {
            return self.full_cycle().await;
        }
        if let Err(e) = self.check_local_root() {
            self.set_error(&e, false);
            self.watcher = None; // the watched handle is dead; recreate once the root is back
            return;
        }
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        for p in d.paths {
            if let Some(k) = self.layout.key_of(&p) {
                // Events inside excluded trees (node_modules during `npm install`…) cost nothing.
                // Keys are folded, which matches because rules are case-insensitive on Windows.
                let parent = parent_key(&k);
                if !parent.is_empty() && !self.l.contains_key(parent) && self.rules.is_ignored(parent, true) {
                    continue;
                }
                if k.is_empty() {
                    dirs.insert(String::new());
                } else {
                    dirs.insert(parent_key(&k).to_string());
                    if self.l.get(&k).is_some_and(|e| e.is_dir) {
                        dirs.insert(k);
                    }
                }
            }
        }
        let changed = self.refresh_dirs(dirs).await;
        if !changed.is_empty() {
            self.guarded(Scope::Keys(changed)).await;
        }
    }

    /// Re-lists the given directories and updates the local index. Returns changed keys.
    async fn refresh_dirs(&mut self, dirs: BTreeSet<String>) -> BTreeSet<String> {
        let mut changed = BTreeSet::new();
        let mut queue: Vec<String> = dirs.into_iter().collect();
        let mut seen = BTreeSet::new();
        while let Some(key) = queue.pop() {
            if !seen.insert(key.clone()) {
                continue;
            }
            if !key.is_empty() && !self.l.get(&key).is_some_and(|e| e.is_dir) {
                // unknown or not a dir any more: let its parent work it out
                queue.push(parent_key(&key).to_string());
                continue;
            }
            let real = if key.is_empty() { String::new() } else { self.l[&key].path.clone() };
            let listing = scanner::list_children(&self.layout, &self.rules, &key, &real);
            let children = match listing {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !key.is_empty() => {
                    queue.push(parent_key(&key).to_string());
                    continue;
                }
                Err(e) => {
                    tracing::warn!("cannot list {key}: {e}");
                    continue;
                }
            };
            let new: BTreeMap<String, LocalEntry> = children.into_iter().collect();
            for old in model::direct_children(&self.l, &key) {
                if !new.contains_key(&old) {
                    for k in model::remove_subtree(&mut self.l, &old) {
                        changed.insert(k);
                    }
                }
            }
            for (k, e) in new {
                let prev = self.l.get(&k).cloned();
                if prev.as_ref() == Some(&e) {
                    continue;
                }
                let became_dir = e.is_dir && !prev.as_ref().is_some_and(|p| p.is_dir);
                if prev.as_ref().is_some_and(|p| p.is_dir && !e.is_dir) {
                    for gone in model::remove_subtree(&mut self.l, &k) {
                        changed.insert(gone);
                    }
                }
                let real = e.path.clone();
                self.l.insert(k.clone(), e);
                changed.insert(k.clone());
                if became_dir {
                    let layout = self.layout.clone();
                    let rules = std::mem::replace(&mut self.rules, Rules::empty());
                    let (k2, r2) = (k.clone(), real);
                    let (res, rules) = tokio::task::spawn_blocking(move || {
                        let r = scanner::scan_tree(&layout, &rules, &k2, &r2);
                        (r, rules)
                    })
                    .await
                    .expect("scan task");
                    self.rules = rules;
                    for (ck, ce) in res.entries {
                        changed.insert(ck.clone());
                        self.l.insert(ck, ce);
                    }
                    if let Some(w) = self.watcher.as_mut() {
                        w.add_dirs(&res.dirs);
                    }
                }
            }
        }
        changed
    }

    // ------------------------------------------------------------------------------------
    // Reconciliation
    // ------------------------------------------------------------------------------------

    async fn guarded(&mut self, scope: Scope) {
        if self.is_paused() {
            self.set_state(PairState::Paused, "Paused");
            return;
        }
        if let Err(e) = self.reconcile(scope).await {
            let offline = is_network(&e);
            self.set_error(&e, offline);
        }
        self.flush();
    }

    fn is_unavailable(&self, key: &str) -> bool {
        self.unavailable.iter().any(|u| is_within(key, u))
    }

    async fn reconcile(&mut self, mut scope: Scope) -> Result<()> {
        self.set_state(PairState::Syncing, "Comparing");
        for _round in 0..4 {
            // -------- candidate keys and the remote view for them --------
            let mut rv: HashMap<String, RemoteView> = HashMap::new();
            let mut keys: BTreeSet<String> = BTreeSet::new();
            match &scope {
                Scope::All => {
                    keys.extend(self.l.keys().cloned());
                    keys.extend(self.b.keys().cloned());
                    for (k, v) in self.tree.subtree_views("") {
                        keys.insert(k.clone());
                        rv.insert(k, v);
                    }
                }
                Scope::Keys(set) => {
                    for k in set {
                        let mut p = k.as_str();
                        while !p.is_empty() {
                            keys.insert(p.to_string());
                            p = parent_key(p);
                        }
                        keys.extend(model::range_keys(&self.l, k));
                        keys.extend(model::range_keys(&self.b, k));
                        for (ck, v) in self.tree.subtree_views(k) {
                            keys.insert(ck.clone());
                            rv.insert(ck, v);
                        }
                    }
                    let missing: Vec<String> = keys.iter().filter(|k| !rv.contains_key(*k)).cloned().collect();
                    for k in missing {
                        if let Some(v) = self.tree.view(&k) {
                            rv.insert(k, v);
                        }
                    }
                }
            }
            keys.remove("");

            // -------- filter: unavailable areas and ignore rules --------
            let mut ignored_dirs: Vec<String> = Vec::new();
            let mut forget: Vec<String> = Vec::new();
            let mut list: Vec<String> = Vec::with_capacity(keys.len());
            for k in keys {
                if self.is_unavailable(&k) || ignored_dirs.iter().any(|d| is_within(&k, d)) {
                    continue;
                }
                let (real, is_dir) = match (self.l.get(&k), rv.get(&k), self.b.get(&k)) {
                    (Some(l), _, _) => (l.path.as_str(), l.is_dir),
                    (_, Some(r), _) => (r.path.as_str(), r.is_dir),
                    (_, _, Some(b)) => (b.path.as_str(), b.is_dir),
                    _ => continue,
                };
                if self.rules.is_ignored(real, is_dir) {
                    if is_dir {
                        ignored_dirs.push(k.clone());
                    }
                    if self.b.contains_key(&k) {
                        forget.push(k);
                    }
                    continue;
                }
                list.push(k);
            }
            for k in forget {
                for gone in model::remove_subtree(&mut self.b, &k) {
                    self.batch.base_del.push(gone);
                }
            }

            // -------- exact move/rename detection --------
            if self.apply_moves(&list, &rv).await? {
                continue; // state changed: recompute this round from scratch
            }

            // -------- hash only where stat data is inconclusive --------
            let needs = plan::hash_needs(&list, &self.l, &self.b, &rv);
            let hashes = if needs.is_empty() {
                HashMap::new()
            } else {
                self.set_state(PairState::Syncing, &format!("Checking {} file(s)", needs.len()));
                let jobs: Vec<(String, PathBuf)> = needs.into_iter().map(|k| (self.layout.resolve(&k), k)).map(|(p, k)| (k, p)).collect();
                tokio::task::spawn_blocking(move || {
                    use rayon::prelude::*;
                    crate::hash::pool().install(|| {
                        jobs.par_iter().filter_map(|(k, p)| crate::hash::md5_file(p).ok().map(|h| (k.clone(), h))).collect::<HashMap<_, _>>()
                    })
                })
                .await
                .expect("hash task")
            };

            let mode = self.cfg.mode;
            let mut decisions = plan::plan(&plan::PlanInput {
                keys: &list,
                l: &self.l,
                b: &self.b,
                r: &rv,
                hashes: &hashes,
                mode,
                propagate_deletes: self.cfg.propagate_deletes,
            });

            self.apply_safety_threshold(&mut decisions);

            let followups = self.execute(decisions, &rv).await?;
            self.flush();
            if followups.is_empty() {
                break;
            }
            scope = Scope::Keys(followups);
        }

        let warnings: Vec<String> = self.mount_warnings.iter().cloned().chain(self.tree.issues(20)).collect();
        let (l, r, errs) = (self.l.len() as u64, self.tree.len() as u64, self.recent_errors);
        let needs_confirm = !self.pending_deletes.is_empty();
        let has_fatal = self.fatal.is_some();
        self.sh.hub.update(&self.cfg.id, |s| {
            s.state = if needs_confirm {
                PairState::NeedsConfirmation
            } else if has_fatal {
                PairState::Error
            } else {
                PairState::Idle
            };
            s.message = if needs_confirm { "Large deletion waiting for confirmation".into() } else { "Up to date".into() };
            if !has_fatal {
                s.error = None;
            }
            s.last_sync_ms = Some(now_ms());
            s.local_items = l;
            s.remote_items = r;
            s.queued = 0;
            s.recent_errors = errs;
            s.warnings = warnings;
        });
        Ok(())
    }

    /// Withholds deletions when an unusually large number would happen at once (for example
    /// when a disk was unmounted or someone emptied the Drive folder by accident).
    fn apply_safety_threshold(&mut self, decisions: &mut BTreeMap<String, Decision>) {
        let threshold = self.sh.config.read().delete_safety_threshold;
        let mut local = 0usize;
        let mut remote = 0usize;
        let mut keys = Vec::new();
        for (k, d) in decisions.iter() {
            let n = 1 + model::range_keys(&self.b, k).len();
            match d {
                Decision::DeleteLocal => local += n,
                Decision::DeleteRemote => remote += n,
                _ => continue,
            }
            keys.push(k.clone());
        }
        if self.allow_deletes_once || threshold == 0 || local + remote <= threshold {
            self.pending_deletes.clear();
            self.sh.hub.update(&self.cfg.id, |s| s.pending_deletes = None);
            return;
        }
        for k in &keys {
            decisions.remove(k);
        }
        let sample: Vec<String> = keys.iter().take(12).map(|k| self.b.get(k).map(|b| b.path.clone()).unwrap_or_else(|| k.clone())).collect();
        if self.pending_deletes != keys {
            self.act(AK::Warning, "", format!("Holding {} deletion(s) for confirmation ({local} local, {remote} in Drive)", local + remote));
        }
        self.pending_deletes = keys;
        self.sh.hub.update(&self.cfg.id, |s| s.pending_deletes = Some(PendingDeletes { local, remote, sample }));
    }

    /// Detects renames/moves on either side using stable identities (NTFS file id / inode
    /// locally, Drive file id remotely) and replays them as a cheap rename on the other side
    /// instead of delete + re-transfer. Returns true if anything was moved.
    async fn apply_moves(&mut self, keys: &[String], rv: &HashMap<String, RemoteView>) -> Result<bool> {
        let push = self.cfg.mode.allows_push();
        let pull = self.cfg.mode.allows_pull();
        let local_new: Vec<&String> = keys.iter().filter(|k| push && self.l.contains_key(*k) && !self.b.contains_key(*k) && !rv.contains_key(*k)).collect();
        let remote_new: Vec<&String> = keys.iter().filter(|k| pull && rv.contains_key(*k) && !self.b.contains_key(*k) && !self.l.contains_key(*k)).collect();
        if local_new.is_empty() && remote_new.is_empty() {
            return Ok(false);
        }
        let mut by_fid: HashMap<u64, String> = HashMap::new();
        let mut by_rid: HashMap<String, String> = HashMap::new();
        for (k, b) in &self.b {
            if b.file_id != 0 {
                by_fid.insert(b.file_id, k.clone());
            }
            by_rid.insert(b.remote_id.clone(), k.clone());
        }
        let drive = self.drive()?;
        let mut moved = false;

        for k in local_new {
            if self.b.contains_key(k) {
                continue; // became tracked through an earlier (parent) move
            }
            let Some(le) = self.l.get(k).cloned() else { continue };
            if le.file_id == 0 {
                continue;
            }
            let Some(q) = by_fid.get(&le.file_id).cloned() else { continue };
            if &q == k {
                continue;
            }
            let Some(bq) = self.b.get(&q).cloned() else { continue };
            let same = bq.is_dir == le.is_dir && (le.is_dir || (bq.size == le.size && bq.mtime_ns == le.mtime_ns));
            let still_there = self.l.get(&q).is_some_and(|x| x.file_id == le.file_id);
            if !same || still_there || self.is_unavailable(&q) {
                continue;
            }
            let Some(rq) = self.tree.lookup(&q).cloned() else { continue };
            if rq.id != bq.remote_id || (!bq.is_dir && rq.md5 != bq.remote_md5) {
                continue;
            }
            let Some(new_parent) = self.tree.dir_id(parent_key(k)) else { continue };
            let name = last_component(&le.path).to_string();
            match drive.move_rename(&rq.id, &name, &new_parent, &rq.parent).await {
                Ok(f) => {
                    let node = f.to_node(&new_parent);
                    self.tree.upsert(node.clone());
                    self.batch.remote_put.push(node);
                    self.rekey_base(&q, k, &le.path);
                    self.act(AK::MoveRemote, &le.path, format!("Moved in Drive from {}", bq.path));
                    moved = true;
                }
                Err(e) => tracing::warn!("remote move of {} failed, falling back to re-upload: {e:#}", bq.path),
            }
        }

        for k in remote_new {
            if self.b.contains_key(k) || self.l.contains_key(k) {
                continue;
            }
            let re = &rv[k];
            let Some(q) = by_rid.get(&re.id).cloned() else { continue };
            if &q == k {
                continue;
            }
            let Some(bq) = self.b.get(&q).cloned() else { continue };
            let Some(lq) = self.l.get(&q).cloned() else { continue };
            let unchanged = lq.is_dir == bq.is_dir && (lq.is_dir || (lq.size == bq.size && lq.mtime_ns == bq.mtime_ns));
            if !unchanged || re.is_dir != bq.is_dir || (!re.is_dir && re.md5 != bq.remote_md5) || self.is_unavailable(&q) || self.is_unavailable(k) {
                continue;
            }
            let parent = parent_key(k);
            if !parent.is_empty() && !self.l.get(parent).is_some_and(|e| e.is_dir) {
                continue;
            }
            let from = self.layout.resolve(&q);
            let to = self.layout.resolve(parent).join(last_component(&re.path));
            let (f2, t2) = (from.clone(), to.clone());
            let res = tokio::task::spawn_blocking(move || {
                fsutil::ensure_writable(&f2);
                std::fs::rename(&f2, &t2)
            })
            .await
            .expect("rename task");
            match res {
                Ok(()) => {
                    let ncomp = lq.path.split('/').count();
                    let new_real = re.path.clone();
                    model::move_subtree(&mut self.l, &q, k, |e, _| e.path = model::rebase_real_path(&e.path, ncomp, &new_real));
                    self.rekey_base(&q, k, &re.path);
                    self.act(AK::MoveLocal, &re.path, format!("Moved locally from {}", lq.path));
                    moved = true;
                }
                Err(e) => tracing::warn!("local move {} → {} failed, falling back to re-download: {e}", from.display(), to.display()),
            }
        }
        Ok(moved)
    }

    fn rekey_base(&mut self, from: &str, to: &str, new_real: &str) {
        let ncomp = self.b.get(from).map(|b| b.path.split('/').count()).unwrap_or(1);
        let moved = model::move_subtree(&mut self.b, from, to, |e, _| e.path = model::rebase_real_path(&e.path, ncomp, new_real));
        for (old, new) in moved {
            self.batch.base_del.push(old);
            if let Some(e) = self.b.get(&new) {
                self.batch.base_put.push((new.clone(), e.clone()));
            }
        }
    }

    fn put_base(&mut self, key: &str, e: BaseEntry) {
        self.batch.base_put.push((key.to_string(), e.clone()));
        self.b.insert(key.to_string(), e);
    }

    fn forget_base(&mut self, key: &str) {
        for k in model::remove_subtree(&mut self.b, key) {
            self.batch.base_del.push(k);
        }
    }

    fn record(&mut self, key: &str, l: &LocalEntry, r: &RemoteView) {
        let e = BaseEntry {
            path: l.path.clone(),
            is_dir: l.is_dir,
            size: l.size,
            mtime_ns: l.mtime_ns,
            file_id: l.file_id,
            md5: r.md5.clone(),
            remote_id: r.id.clone(),
            remote_md5: r.md5.clone(),
            remote_mtime_ms: r.mtime_ms,
        };
        self.put_base(key, e);
    }

    fn fail(&mut self, key: &str, path: &str, err: &anyhow::Error) {
        self.recent_errors += 1;
        self.retry.insert(key.to_string());
        self.retry_at.get_or_insert(Instant::now() + RETRY_DELAY);
        self.act(AK::Error, path, format!("{err:#}"));
    }

    // ------------------------------------------------------------------------------------
    // Execution
    // ------------------------------------------------------------------------------------

    async fn execute(&mut self, decisions: BTreeMap<String, Decision>, rv: &HashMap<String, RemoteView>) -> Result<BTreeSet<String>> {
        let mut followups = BTreeSet::new();
        if decisions.is_empty() {
            return Ok(followups);
        }
        let drive = self.drive()?;
        let use_trash = self.sh.config.read().use_trash;

        // Phase 0: pure bookkeeping.
        let mut work: Vec<(String, Decision)> = Vec::new();
        for (k, d) in decisions {
            match d {
                Decision::Record => {
                    if let (Some(l), Some(r)) = (self.l.get(&k).cloned(), rv.get(&k)) {
                        self.record(&k, &l, r);
                    }
                }
                Decision::TouchBase => {
                    if let (Some(l), Some(mut b)) = (self.l.get(&k).cloned(), self.b.get(&k).cloned()) {
                        b.mtime_ns = l.mtime_ns;
                        b.size = l.size;
                        b.file_id = l.file_id;
                        b.path = l.path.clone();
                        self.put_base(&k, b);
                    }
                }
                Decision::Forget => self.forget_base(&k),
                other => work.push((k, other)),
            }
        }
        if work.is_empty() {
            return Ok(followups);
        }
        self.set_state(PairState::Syncing, &format!("Syncing {} item(s)", work.len()));

        // Phase 1: conflicts — move the local copy aside, then treat as a plain pull.
        let mut conflict_parents = BTreeSet::new();
        for (k, d) in work.iter_mut() {
            if *d != Decision::Conflict {
                continue;
            }
            let Some(l) = self.l.get(k.as_str()).cloned() else {
                *d = Decision::Pull { replace_local: false };
                continue;
            };
            let from = self.layout.resolve(k);
            let new_name = fsutil::conflict_name(last_component(&l.path));
            let to = from.with_file_name(&new_name);
            match std::fs::rename(&from, &to) {
                Ok(()) => {
                    model::remove_subtree(&mut self.l, k);
                    self.act(AK::Conflict, &l.path, format!("Changed on both sides; local copy kept as \"{new_name}\""));
                    conflict_parents.insert(parent_key(k).to_string());
                    *d = Decision::Pull { replace_local: false };
                }
                Err(e) => {
                    let err = anyhow!("could not set aside conflicting local copy: {e}");
                    self.fail(k, &l.path, &err);
                    *d = Decision::Forget; // sentinel: skip
                }
            }
        }
        work.retain(|(_, d)| *d != Decision::Forget);

        // Phase 2: deletions and kind replacements (folder ↔ file), before anything is created.
        let mut remote_dels: Vec<(String, String, String, bool)> = Vec::new(); // key, id, path, forget_base
        let mut local_dels: Vec<(String, String, bool)> = Vec::new(); // key, path, forget_base
        for (k, d) in &work {
            match d {
                Decision::DeleteRemote => {
                    let id = rv.get(k).map(|r| r.id.clone()).or_else(|| self.b.get(k).map(|b| b.remote_id.clone()));
                    let path = self.b.get(k).map(|b| b.path.clone()).unwrap_or_else(|| k.clone());
                    if let Some(id) = id {
                        remote_dels.push((k.clone(), id, path, true));
                    }
                }
                Decision::Push { replace_remote: true } => {
                    if let Some(r) = rv.get(k) {
                        remote_dels.push((k.clone(), r.id.clone(), r.path.clone(), false));
                    }
                }
                Decision::DeleteLocal => {
                    let path = self.b.get(k).map(|b| b.path.clone()).unwrap_or_else(|| k.clone());
                    local_dels.push((k.clone(), path, true));
                }
                Decision::Pull { replace_local: true } => {
                    if let Some(l) = self.l.get(k) {
                        local_dels.push((k.clone(), l.path.clone(), false));
                    }
                }
                _ => {}
            }
        }
        let mut failed: BTreeSet<String> = BTreeSet::new();
        if !remote_dels.is_empty() {
            let results: Vec<_> = stream::iter(remote_dels.into_iter().map(|(k, id, path, fb)| {
                let d = drive.clone();
                async move {
                    let r = d.trash(&id).await;
                    (k, id, path, fb, r)
                }
            }))
            .buffer_unordered(8)
            .collect()
            .await;
            for (k, id, path, fb, r) in results {
                match r {
                    Ok(()) => {
                        let removed = self.tree.remove_subtree(&id);
                        self.batch.remote_del.extend(removed);
                        if fb {
                            self.forget_base(&k);
                            self.act(AK::DeleteRemote, &path, "Moved to Drive trash");
                        } else {
                            self.forget_base(&k);
                        }
                    }
                    Err(e) => {
                        self.fail(&k, &path, &e);
                        failed.insert(k);
                    }
                }
            }
        }
        for (k, path, fb) in local_dels {
            if self.is_paused() {
                break;
            }
            let abs = self.layout.resolve(&k);
            // Never delete a file that changed since we looked at it.
            if let Some(expect) = self.l.get(&k).cloned() {
                if !expect.is_dir {
                    match fsutil::stat(&abs) {
                        Ok(Some(now)) if now.size != expect.size || now.mtime_ns != expect.mtime_ns => {
                            followups.insert(k.clone());
                            failed.insert(k);
                            continue;
                        }
                        _ => {}
                    }
                }
            }
            if self.layout.mounts.iter().any(|m| m.key == k) {
                // never delete the target of an external folder link, just stop tracking it
                self.forget_base(&k);
                continue;
            }
            let a2 = abs.clone();
            let res = tokio::task::spawn_blocking(move || fsutil::remove_path(&a2, use_trash)).await.expect("delete task");
            match res {
                Ok(()) => {
                    model::remove_subtree(&mut self.l, &k);
                    if fb {
                        self.forget_base(&k);
                        self.act(AK::DeleteLocal, &path, if use_trash { "Moved to recycle bin" } else { "Deleted" });
                    } else {
                        self.forget_base(&k);
                    }
                }
                Err(e) => {
                    self.fail(&k, &path, &anyhow!("delete failed: {e}"));
                    failed.insert(k);
                }
            }
        }

        // Phase 3: folders, shallowest first. Remote siblings are created concurrently.
        let mut remote_dirs: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        let mut local_dirs: Vec<String> = Vec::new();
        for (k, d) in &work {
            if failed.iter().any(|f| is_within(k, f)) {
                continue;
            }
            match d {
                Decision::Push { .. } if self.l.get(k).is_some_and(|e| e.is_dir) => {
                    remote_dirs.entry(k.matches('/').count()).or_default().push(k.clone());
                }
                Decision::Pull { .. } if rv.get(k).is_some_and(|r| r.is_dir) => local_dirs.push(k.clone()),
                _ => {}
            }
        }
        for (_, level) in remote_dirs {
            if self.is_paused() {
                break;
            }
            let mut jobs = Vec::new();
            for k in level {
                let Some(l) = self.l.get(&k).cloned() else { continue };
                match self.tree.dir_id(parent_key(&k)) {
                    Some(pid) => jobs.push((k, l, pid)),
                    None => {
                        followups.insert(k);
                    }
                }
            }
            let results: Vec<_> = stream::iter(jobs.into_iter().map(|(k, l, pid)| {
                let d = drive.clone();
                async move {
                    let name = last_component(&l.path).to_string();
                    let r = d.create_folder(&name, &pid).await;
                    (k, l, pid, r)
                }
            }))
            .buffer_unordered(8)
            .collect()
            .await;
            for (k, l, pid, r) in results {
                match r {
                    Ok(f) => {
                        let node = f.to_node(&pid);
                        self.tree.upsert(node.clone());
                        self.batch.remote_put.push(node);
                        let view = RemoteView { id: f.id.clone(), path: l.path.clone(), is_dir: true, size: 0, md5: None, mtime_ms: f.mtime_ms() };
                        self.record(&k, &l, &view);
                        self.act(AK::Mkdir, &l.path, "Folder created in Drive");
                    }
                    Err(e) => {
                        self.fail(&k, &l.path, &e);
                        failed.insert(k);
                    }
                }
            }
        }
        for k in local_dirs {
            let r = rv[&k].clone();
            let parent = parent_key(&k);
            if !parent.is_empty() && !self.l.get(parent).is_some_and(|e| e.is_dir) {
                followups.insert(k);
                continue;
            }
            let abs = self.layout.resolve(parent).join(last_component(&r.path));
            match std::fs::create_dir(&abs).or_else(|e| if abs.is_dir() { Ok(()) } else { Err(e) }) {
                Ok(()) => match fsutil::stat(&abs) {
                    Ok(Some(st)) => {
                        let le = LocalEntry { path: r.path.clone(), is_dir: true, size: 0, mtime_ns: st.mtime_ns, file_id: st.file_id };
                        self.l.insert(k.clone(), le.clone());
                        self.record(&k, &le, &r);
                        self.act(AK::Mkdir, &r.path, "Folder created locally");
                    }
                    _ => {
                        followups.insert(k);
                    }
                },
                Err(e) => {
                    self.fail(&k, &r.path, &anyhow!("could not create folder: {e}"));
                    failed.insert(k);
                }
            }
        }

        // Phase 4: file transfers, concurrently.
        let mut jobs: Vec<Job> = Vec::new();
        for (k, d) in &work {
            if failed.iter().any(|f| is_within(k, f)) {
                continue;
            }
            match d {
                Decision::Push { replace_remote } => {
                    let Some(l) = self.l.get(k).cloned() else { continue };
                    if l.is_dir {
                        continue;
                    }
                    let Some(pid) = self.tree.dir_id(parent_key(k)) else {
                        followups.insert(k.clone());
                        continue;
                    };
                    let existing = if *replace_remote { None } else { rv.get(k).filter(|r| !r.is_dir).map(|r| r.id.clone()) };
                    jobs.push(Job::Up(UploadJob {
                        key: k.clone(),
                        real: l.path.clone(),
                        abs: self.layout.resolve(k),
                        name: last_component(&l.path).to_string(),
                        parent_id: pid,
                        existing,
                        expect: l,
                    }));
                }
                Decision::Pull { replace_local } => {
                    let Some(r) = rv.get(k).cloned() else { continue };
                    if r.is_dir {
                        continue;
                    }
                    let parent = parent_key(k);
                    if !parent.is_empty() && !self.l.get(parent).is_some_and(|e| e.is_dir) {
                        followups.insert(k.clone());
                        continue;
                    }
                    let expect = if *replace_local { None } else { self.l.get(k).cloned() };
                    jobs.push(Job::Down(DownloadJob { key: k.clone(), abs: self.layout.resolve(parent).join(last_component(&r.path)), remote: r, expect }));
                }
                _ => {}
            }
        }

        if !jobs.is_empty() {
            let total = jobs.len() as u64;
            let n = self.sh.config.read().max_concurrent_transfers;
            let ctx = JobCtx { drive: drive.clone(), sh: self.sh.clone(), pair_id: self.cfg.id.clone(), paused: self.paused.clone() };
            let mut results = stream::iter(jobs.into_iter().map(|j| run_job(ctx.clone(), j))).buffer_unordered(n);
            let mut done = 0u64;
            while let Some(res) = results.next().await {
                done += 1;
                self.apply_job_result(res, &mut followups);
                if done % FLUSH_EVERY as u64 == 0 {
                    self.flush();
                }
                let q = total - done;
                self.sh.hub.update(&self.cfg.id, |s| {
                    s.queued = q;
                    s.message = format!("Syncing — {q} remaining");
                });
            }
        }

        for p in conflict_parents {
            followups.extend(self.refresh_dirs(BTreeSet::from([p])).await);
        }
        // anything that failed is retried by the retry timer, not immediately
        for f in &failed {
            followups.remove(f);
        }
        Ok(followups)
    }

    fn apply_job_result(&mut self, res: JobResult, followups: &mut BTreeSet<String>) {
        match res {
            JobResult::Uploaded { key, real, file, parent_id, local, stable, replaced } => {
                let node = file.to_node(&parent_id);
                self.tree.upsert(node.clone());
                self.batch.remote_put.push(node);
                if stable {
                    let view = RemoteView { id: file.id.clone(), path: real.clone(), is_dir: false, size: file.size_u64(), md5: file.md5_checksum.clone(), mtime_ms: file.mtime_ms() };
                    self.record(&key, &local, &view);
                } else {
                    followups.insert(key);
                }
                self.act(AK::Upload, &real, if replaced { "Updated in Drive" } else { "Uploaded" });
            }
            JobResult::Downloaded { key, local, remote } => {
                self.l.insert(key.clone(), local.clone());
                self.record(&key, &local, &remote);
                self.act(AK::Download, &remote.path, "Downloaded");
            }
            JobResult::Skipped { key, why } => {
                tracing::debug!("skipped {key}: {why}");
                followups.insert(key);
            }
            JobResult::Failed { key, path, err } => self.fail(&key, &path, &err),
        }
    }
}

#[derive(Clone)]
struct JobCtx {
    drive: Drive,
    sh: Arc<Shared>,
    pair_id: String,
    paused: Arc<AtomicBool>,
}

async fn run_job(ctx: JobCtx, job: Job) -> JobResult {
    if ctx.paused.load(Ordering::Relaxed) {
        let key = match &job {
            Job::Up(u) => u.key.clone(),
            Job::Down(d) => d.key.clone(),
        };
        return JobResult::Skipped { key, why: "paused".into() };
    }
    let _permit = ctx.sh.transfer_slots.clone().acquire_owned().await;
    match job {
        Job::Up(u) => upload(&ctx, u).await,
        Job::Down(d) => download(&ctx, d).await,
    }
}

async fn upload(ctx: &JobCtx, u: UploadJob) -> JobResult {
    let before = match fsutil::stat(&u.abs) {
        Ok(Some(s)) if !s.is_dir => s,
        _ => return JobResult::Skipped { key: u.key, why: "vanished".into() },
    };
    if before.size != u.expect.size || before.mtime_ns != u.expect.mtime_ns {
        return JobResult::Skipped { key: u.key, why: "changed while queued".into() };
    }
    let progress = Arc::new(AtomicU64::new(0));
    let tid = ctx.sh.hub.start_transfer(&ctx.pair_id, &u.real, true, before.size, progress.clone());
    let mtime_ms = before.mtime_ns.div_euclid(1_000_000);
    let mut res = ctx.drive.upload(&u.abs, &u.name, &u.parent_id, u.existing.as_deref(), mtime_ms, before.size, &progress).await;
    let mut replaced = u.existing.is_some();
    if u.existing.is_some() && res.as_ref().err().is_some_and(is_not_found) {
        // the remote file vanished meanwhile: create it fresh
        replaced = false;
        res = ctx.drive.upload(&u.abs, &u.name, &u.parent_id, None, mtime_ms, before.size, &progress).await;
    }
    ctx.sh.hub.end_transfer(tid);
    match res {
        Ok(file) => {
            let after = fsutil::stat(&u.abs).ok().flatten();
            let stable = after.as_ref().is_some_and(|a| a.size == before.size && a.mtime_ns == before.mtime_ns);
            let local = LocalEntry { path: u.real.clone(), is_dir: false, size: before.size, mtime_ns: before.mtime_ns, file_id: before.file_id };
            JobResult::Uploaded { key: u.key, real: u.real, file, parent_id: u.parent_id, local, stable, replaced }
        }
        Err(err) => JobResult::Failed { key: u.key, path: u.real, err },
    }
}

async fn download(ctx: &JobCtx, d: DownloadJob) -> JobResult {
    let tmp = fsutil::temp_path_for(&d.abs);
    let progress = Arc::new(AtomicU64::new(0));
    let tid = ctx.sh.hub.start_transfer(&ctx.pair_id, &d.remote.path, false, d.remote.size, progress.clone());
    let res = ctx.drive.download(&d.remote.id, &tmp, &progress).await;
    ctx.sh.hub.end_transfer(tid);
    let fail = |err: anyhow::Error| {
        let _ = std::fs::remove_file(&tmp);
        JobResult::Failed { key: d.key.clone(), path: d.remote.path.clone(), err }
    };
    let md5 = match res {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    if let Some(expect) = &d.remote.md5 {
        if !expect.eq_ignore_ascii_case(&md5) {
            return fail(anyhow!("checksum mismatch after download (expected {expect}, got {md5})"));
        }
    }
    let ms = d.remote.mtime_ms;
    if ms > 0 {
        let ft = filetime::FileTime::from_unix_time(ms.div_euclid(1000), (ms.rem_euclid(1000) * 1_000_000) as u32);
        let _ = filetime::set_file_mtime(&tmp, ft);
    }
    // Last-moment race check: the local file must still be what we planned against.
    let current = fsutil::stat(&d.abs).ok().flatten();
    let ok = match (&d.expect, &current) {
        (None, None) => true,
        (Some(e), Some(c)) => !c.is_dir && c.size == e.size && c.mtime_ns == e.mtime_ns,
        _ => false,
    };
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return JobResult::Skipped { key: d.key, why: "local file changed during download".into() };
    }
    if let Err(e) = fsutil::replace_file(&tmp, &d.abs) {
        return fail(anyhow!("could not write file: {e}"));
    }
    match fsutil::stat(&d.abs) {
        Ok(Some(st)) => JobResult::Downloaded {
            key: d.key,
            local: LocalEntry { path: d.remote.path.clone(), is_dir: false, size: st.size, mtime_ns: st.mtime_ns, file_id: st.file_id },
            remote: d.remote,
        },
        _ => JobResult::Skipped { key: d.key, why: "vanished after write".into() },
    }
}

/// Breadth-first listing of a folder subtree with several folders fetched in parallel.
async fn list_subtree(drive: &Drive, tree: &mut RemoteTree, root: &str) -> Result<()> {
    let mut frontier = vec![root.to_string()];
    while !frontier.is_empty() {
        let results: Vec<(String, Result<Vec<DriveFile>>)> = stream::iter(frontier.drain(..).map(|id| {
            let d = drive.clone();
            async move {
                let r = d.list_children(&id).await;
                (id, r)
            }
        }))
        .buffer_unordered(8)
        .collect()
        .await;
        let mut next = Vec::new();
        for (pid, r) in results {
            for f in r? {
                if f.is_folder() {
                    next.push(f.id.clone());
                }
                tree.upsert(f.to_node(&pid));
            }
        }
        frontier = next;
    }
    Ok(())
}
