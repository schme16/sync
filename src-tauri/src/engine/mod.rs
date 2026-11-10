//! Sync engine: one long-lived worker task per sync pair, orchestrated by the [`Manager`].

pub mod plan;
mod worker;

use crate::config::{AppPaths, Config, PairConfig};
use crate::drive::auth::Auth;
use crate::drive::Drive;
use crate::hub::{Hub, PairState};
use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

pub struct Shared {
    pub hub: Arc<Hub>,
    pub paths: Arc<AppPaths>,
    pub config: Arc<RwLock<Config>>,
    pub http: reqwest::Client,
    /// Caps concurrent transfers across all pairs.
    pub transfer_slots: Arc<Semaphore>,
    drives: Mutex<HashMap<String, Drive>>,
}

impl Shared {
    pub fn new(hub: Arc<Hub>, paths: Arc<AppPaths>, config: Arc<RwLock<Config>>, http: reqwest::Client) -> Arc<Shared> {
        let slots = config.read().max_concurrent_transfers;
        Arc::new(Shared { hub, paths, config, http, transfer_slots: Arc::new(Semaphore::new(slots * 2)), drives: Mutex::new(HashMap::new()) })
    }

    pub fn drive(&self, account_id: &str) -> Result<Drive> {
        if let Some(d) = self.drives.lock().get(account_id) {
            return Ok(d.clone());
        }
        let (id, secret) = self
            .config
            .read()
            .client_credentials()
            .ok_or_else(|| anyhow!("Google API credentials are not configured (Settings → Google API)"))?;
        let refresh = crate::secrets::load(&self.paths.config_dir, account_id)
            .ok_or_else(|| anyhow!("no saved sign-in for this account; remove it and sign in again"))?;
        let d = Drive::new(self.http.clone(), Arc::new(Auth::new(self.http.clone(), id, secret, refresh)));
        self.drives.lock().insert(account_id.to_string(), d.clone());
        Ok(d)
    }

    pub fn register_drive(&self, account_id: &str, d: Drive) {
        self.drives.lock().insert(account_id.to_string(), d);
    }

    pub fn forget_drive(&self, account_id: &str) {
        self.drives.lock().remove(account_id);
    }

    pub fn forget_all_drives(&self) {
        self.drives.lock().clear();
    }
}

#[derive(Debug)]
pub enum Cmd {
    SyncNow,
    SetPaused(bool),
    ConfirmDeletes(bool),
    Shutdown,
}

struct WorkerHandle {
    tx: mpsc::UnboundedSender<Cmd>,
    paused: Arc<AtomicBool>,
    task: tauri::async_runtime::JoinHandle<()>,
}

pub struct Manager {
    pub shared: Arc<Shared>,
    workers: Mutex<HashMap<String, WorkerHandle>>,
}

impl Manager {
    pub fn new(shared: Arc<Shared>) -> Arc<Manager> {
        Arc::new(Manager { shared, workers: Mutex::new(HashMap::new()) })
    }

    pub fn start_all(&self) {
        let pairs = self.shared.config.read().pairs.clone();
        for p in pairs {
            self.start_pair(p);
        }
    }

    pub fn start_pair(&self, cfg: PairConfig) {
        let id = cfg.id.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let paused = Arc::new(AtomicBool::new(cfg.paused || self.shared.hub.paused_all.load(Ordering::Relaxed)));
        self.shared.hub.update(&id, |s| {
            *s = Default::default();
            s.state = PairState::Starting;
        });
        let task = tauri::async_runtime::spawn(worker::run(self.shared.clone(), cfg, rx, paused.clone()));
        if let Some(old) = self.workers.lock().insert(id, WorkerHandle { tx, paused, task }) {
            let _ = old.tx.send(Cmd::Shutdown);
            old.task.abort();
        }
    }

    pub async fn stop_pair(&self, id: &str) {
        let h = self.workers.lock().remove(id);
        if let Some(h) = h {
            h.paused.store(true, Ordering::Relaxed);
            let _ = h.tx.send(Cmd::Shutdown);
            // Give it a moment to flush its database, then make sure it is gone.
            let abort = h.task.inner().abort_handle();
            if tokio::time::timeout(std::time::Duration::from_secs(5), h.task).await.is_err() {
                abort.abort();
            }
        }
        self.shared.hub.remove_pair(id);
    }

    pub async fn restart_pair(&self, cfg: PairConfig) {
        self.stop_pair(&cfg.id).await;
        self.start_pair(cfg);
    }

    pub fn send(&self, id: &str, cmd: Cmd) {
        if let Some(h) = self.workers.lock().get(id) {
            if let Cmd::SetPaused(p) = cmd {
                h.paused.store(p, Ordering::Relaxed);
            }
            let _ = h.tx.send(cmd);
        }
    }

    pub fn ids(&self) -> Vec<String> {
        self.workers.lock().keys().cloned().collect()
    }

    pub fn set_paused_all(&self, paused: bool) {
        self.shared.hub.paused_all.store(paused, Ordering::Relaxed);
        self.shared.hub.mark_dirty();
        let cfg = self.shared.config.read().clone();
        for id in self.ids() {
            let pair_paused = cfg.pair(&id).map(|p| p.paused).unwrap_or(false);
            self.send(&id, Cmd::SetPaused(paused || pair_paused));
        }
    }

    pub fn sync_all_now(&self) {
        for id in self.ids() {
            self.send(&id, Cmd::SyncNow);
        }
    }

    pub async fn shutdown(&self) {
        for id in self.ids() {
            self.stop_pair(&id).await;
        }
    }
}
