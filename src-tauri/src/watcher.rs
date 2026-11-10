//! Native change notifications.
//!
//! * Windows: one recursive `ReadDirectoryChangesW` handle per root covers the whole tree.
//! * Linux: inotify is per-directory, so instead of letting a recursive watcher descend into
//!   `node_modules` & co. we place non-recursive watches only on directories that are actually
//!   synced. That keeps us far away from `max_user_watches` limits.
//!
//! Events are coalesced into a set of dirty paths; the worker re-lists only the affected folders.

use notify::event::{AccessKind, EventKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Notify;

const MAX_PENDING: usize = 100_000;

#[derive(Default)]
pub struct Dirty {
    pub paths: HashSet<PathBuf>,
    pub overflow: bool,
}

pub struct PairWatcher {
    watcher: RecommendedWatcher,
    #[allow(dead_code)]
    watched: HashSet<PathBuf>,
    pub limited: bool,
}

impl PairWatcher {
    pub fn start(roots: &[PathBuf], dirs: &[PathBuf], sink: Arc<Mutex<Dirty>>, wake: Arc<Notify>) -> notify::Result<PairWatcher> {
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let mut d = sink.lock();
            match res {
                Ok(ev) => {
                    if ev.need_rescan() {
                        d.overflow = true;
                    }
                    if matches!(ev.kind, EventKind::Access(AccessKind::Open(_)) | EventKind::Access(AccessKind::Read) | EventKind::Access(AccessKind::Any)) {
                        return;
                    }
                    for p in ev.paths {
                        if p.file_name().and_then(|n| n.to_str()).is_some_and(crate::fsutil::is_temp_name) {
                            continue;
                        }
                        d.paths.insert(p);
                    }
                    if d.paths.len() > MAX_PENDING {
                        d.paths.clear();
                        d.overflow = true;
                    }
                }
                Err(e) => {
                    tracing::warn!("watcher error: {e}; scheduling a rescan");
                    d.overflow = true;
                }
            }
            drop(d);
            wake.notify_one();
        })?;
        let mut w = PairWatcher { watcher, watched: HashSet::new(), limited: false };
        if cfg!(target_os = "linux") {
            for r in roots {
                w.add_dir(r);
            }
            w.add_dirs(dirs);
        } else {
            for r in roots {
                w.watcher.watch(r, RecursiveMode::Recursive)?;
            }
        }
        Ok(w)
    }

    fn add_dir(&mut self, d: &PathBuf) {
        if self.limited || self.watched.contains(d) {
            return;
        }
        match self.watcher.watch(d, RecursiveMode::NonRecursive) {
            Ok(()) => {
                self.watched.insert(d.clone());
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("max") || msg.contains("No space") || msg.contains("ENOSPC") || matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                    tracing::warn!("inotify watch limit reached; falling back to periodic rescans for the rest");
                    self.limited = true;
                }
            }
        }
    }

    /// Linux only: adds non-recursive watches for newly discovered directories.
    pub fn add_dirs(&mut self, dirs: &[PathBuf]) {
        if !cfg!(target_os = "linux") {
            return;
        }
        for d in dirs {
            self.add_dir(d);
        }
    }
}
