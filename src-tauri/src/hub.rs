//! Central status and activity hub. Workers write into it cheaply; a ticker batches updates to
//! the UI (at most ~4 per second) and only when a window actually exists.

use crate::ignore_rules::RuleError;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

const MAX_ACTIVITY: usize = 5000;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityKind {
    Upload,
    Download,
    DeleteLocal,
    DeleteRemote,
    MoveLocal,
    MoveRemote,
    Mkdir,
    Conflict,
    Error,
    Warning,
    Info,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    pub id: u64,
    pub ts: i64,
    pub pair_id: String,
    pub kind: ActivityKind,
    pub path: String,
    pub detail: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum PairState {
    #[default]
    Starting,
    Scanning,
    Syncing,
    Idle,
    Paused,
    Offline,
    Error,
    NeedsConfirmation,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PendingDeletes {
    pub local: usize,
    pub remote: usize,
    pub sample: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PairStatus {
    pub state: PairState,
    pub message: String,
    pub last_sync_ms: Option<i64>,
    pub local_items: u64,
    pub remote_items: u64,
    pub queued: u64,
    pub error: Option<String>,
    pub recent_errors: u64,
    pub pending_deletes: Option<PendingDeletes>,
    pub warnings: Vec<String>,
    pub rule_errors: Vec<RuleError>,
}

#[derive(Clone, Debug)]
struct Transfer {
    pair_id: String,
    path: String,
    upload: bool,
    total: u64,
    done: Arc<AtomicU64>,
    started_ms: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    pub id: u64,
    pub pair_id: String,
    pub path: String,
    pub upload: bool,
    pub total: u64,
    pub done: u64,
    pub started_ms: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub pairs: BTreeMap<String, PairStatus>,
    pub transfers: Vec<TransferView>,
    pub paused_all: bool,
}

pub struct Hub {
    app: AppHandle,
    next_id: AtomicU64,
    log: Mutex<VecDeque<ActivityItem>>,
    pending: Mutex<Vec<ActivityItem>>,
    pairs: Mutex<BTreeMap<String, PairStatus>>,
    transfers: Mutex<BTreeMap<u64, Transfer>>,
    dirty: AtomicBool,
    pub paused_all: AtomicBool,
    tooltip: Mutex<String>,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Hub {
    pub fn new(app: AppHandle) -> Arc<Hub> {
        Arc::new(Hub {
            app,
            next_id: AtomicU64::new(1),
            log: Mutex::new(VecDeque::new()),
            pending: Mutex::new(Vec::new()),
            pairs: Mutex::new(BTreeMap::new()),
            transfers: Mutex::new(BTreeMap::new()),
            dirty: AtomicBool::new(true),
            paused_all: AtomicBool::new(false),
            tooltip: Mutex::new(String::new()),
        })
    }

    pub fn activity(&self, pair_id: &str, kind: ActivityKind, path: &str, detail: impl Into<String>) {
        let item = ActivityItem {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            ts: now_ms(),
            pair_id: pair_id.to_string(),
            kind,
            path: path.to_string(),
            detail: detail.into(),
        };
        match kind {
            ActivityKind::Error => tracing::error!(pair = pair_id, path, "{}", item.detail),
            ActivityKind::Warning => tracing::warn!(pair = pair_id, path, "{}", item.detail),
            _ => tracing::info!(pair = pair_id, path, kind = ?kind, "{}", item.detail),
        }
        {
            let mut log = self.log.lock();
            if log.len() >= MAX_ACTIVITY {
                log.pop_front();
            }
            log.push_back(item.clone());
        }
        self.pending.lock().push(item);
    }

    pub fn activity_list(&self, limit: usize) -> Vec<ActivityItem> {
        let log = self.log.lock();
        log.iter().rev().take(limit).cloned().collect()
    }

    pub fn clear_activity(&self) {
        self.log.lock().clear();
    }

    pub fn update(&self, pair_id: &str, f: impl FnOnce(&mut PairStatus)) {
        let mut p = self.pairs.lock();
        f(p.entry(pair_id.to_string()).or_default());
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn status(&self, pair_id: &str) -> PairStatus {
        self.pairs.lock().get(pair_id).cloned().unwrap_or_default()
    }

    pub fn remove_pair(&self, pair_id: &str) {
        self.pairs.lock().remove(pair_id);
        self.transfers.lock().retain(|_, t| t.pair_id != pair_id);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn start_transfer(&self, pair_id: &str, path: &str, upload: bool, total: u64, done: Arc<AtomicU64>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.transfers.lock().insert(id, Transfer { pair_id: pair_id.into(), path: path.into(), upload, total, done, started_ms: now_ms() });
        self.dirty.store(true, Ordering::Relaxed);
        id
    }

    pub fn end_transfer(&self, id: u64) {
        self.transfers.lock().remove(&id);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            pairs: self.pairs.lock().clone(),
            transfers: self
                .transfers
                .lock()
                .iter()
                .map(|(id, t)| TransferView {
                    id: *id,
                    pair_id: t.pair_id.clone(),
                    path: t.path.clone(),
                    upload: t.upload,
                    total: t.total,
                    done: t.done.load(Ordering::Relaxed).min(t.total),
                    started_ms: t.started_ms,
                })
                .collect(),
            paused_all: self.paused_all.load(Ordering::Relaxed),
        }
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn summary(&self) -> String {
        let pairs = self.pairs.lock();
        let n_transfers = self.transfers.lock().len();
        if self.paused_all.load(Ordering::Relaxed) {
            return "Sync — paused".into();
        }
        if pairs.values().any(|p| p.state == PairState::Error) {
            return "Sync — attention needed".into();
        }
        if pairs.values().any(|p| p.state == PairState::NeedsConfirmation) {
            return "Sync — waiting for confirmation".into();
        }
        if n_transfers > 0 || pairs.values().any(|p| matches!(p.state, PairState::Syncing | PairState::Scanning | PairState::Starting)) {
            return format!("Sync — syncing ({n_transfers} active)");
        }
        if pairs.values().any(|p| p.state == PairState::Offline) {
            return "Sync — offline".into();
        }
        "Sync — up to date".into()
    }

    /// Emits batched status/activity updates to the UI and keeps the tray tooltip current.
    pub fn spawn_ticker(self: &Arc<Self>) {
        let hub = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut iv = tokio::time::interval(std::time::Duration::from_millis(250));
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                iv.tick().await;
                let has_window = hub.app.get_webview_window("main").is_some();
                let transfers = !hub.transfers.lock().is_empty();
                if hub.dirty.swap(false, Ordering::Relaxed) || transfers {
                    if has_window {
                        let _ = hub.app.emit("status", hub.snapshot());
                    }
                    let tip = hub.summary();
                    let mut cur = hub.tooltip.lock();
                    if *cur != tip {
                        if let Some(tray) = hub.app.tray_by_id("main") {
                            let _ = tray.set_tooltip(Some(&tip));
                        }
                        *cur = tip;
                    }
                }
                let batch: Vec<ActivityItem> = std::mem::take(&mut *hub.pending.lock());
                if !batch.is_empty() && has_window {
                    let _ = hub.app.emit("activity", batch);
                }
            }
        });
    }
}
