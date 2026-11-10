//! Persistent application configuration (settings, accounts and sync pairs).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_GLOBAL_IGNORE: &str = r#"# Global exclusions use .gitignore syntax and apply to every sync pair
# that has "Use global rules" switched on. Prefix a line with ! to re-include.

# Dependency and build folders: thousands of tiny files you can regenerate
node_modules/
bower_components/
__pycache__/
.venv/
venv/
.tox/
.gradle/
.next/
.nuxt/
.parcel-cache/
.turbo/
target/debug/
target/release/

# OS and editor junk
.DS_Store
Thumbs.db
desktop.ini
ehthumbs.db
$RECYCLE.BIN/
System Volume Information/
~$*
.~lock.*#
*.swp
*.swo
*~

# Git writes these lock files for a moment and then deletes them
**/.git/**/*.lock
"#;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SyncMode {
    #[default]
    TwoWay,
    LocalToRemote,
    RemoteToLocal,
}

impl SyncMode {
    pub fn allows_push(self) -> bool {
        self != SyncMode::RemoteToLocal
    }
    pub fn allows_pull(self) -> bool {
        self != SyncMode::LocalToRemote
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub photo_link: String,
}

/// An external local folder that appears inside a sync pair at `rel_path`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Mount {
    pub local_path: String,
    pub rel_path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PairConfig {
    pub id: String,
    pub name: String,
    pub account_id: String,
    pub local_path: String,
    pub remote_folder_id: String,
    pub remote_path_display: String,
    pub drive_id: Option<String>,
    pub mode: SyncMode,
    pub propagate_deletes: bool,
    pub paused: bool,
    pub use_global_ignore: bool,
    pub ignore: String,
    pub mounts: Vec<Mount>,
}

impl Default for PairConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            account_id: String::new(),
            local_path: String::new(),
            remote_folder_id: String::new(),
            remote_path_display: String::new(),
            drive_id: None,
            mode: SyncMode::TwoWay,
            propagate_deletes: true,
            paused: false,
            use_global_ignore: true,
            ignore: String::new(),
            mounts: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub version: u32,
    pub theme: Theme,
    pub launch_at_startup: bool,
    pub start_minimized: bool,
    pub max_concurrent_transfers: usize,
    pub remote_poll_secs: u64,
    pub full_rescan_mins: u64,
    pub debounce_ms: u64,
    pub use_trash: bool,
    pub delete_safety_threshold: usize,
    pub low_priority_io: bool,
    pub global_ignore: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub accounts: Vec<Account>,
    pub pairs: Vec<PairConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            theme: Theme::Auto,
            launch_at_startup: false,
            start_minimized: false,
            max_concurrent_transfers: 6,
            remote_poll_secs: 20,
            full_rescan_mins: 60,
            debounce_ms: 1500,
            use_trash: true,
            delete_safety_threshold: 500,
            low_priority_io: true,
            global_ignore: DEFAULT_GLOBAL_IGNORE.to_string(),
            google_client_id: String::new(),
            google_client_secret: String::new(),
            accounts: Vec::new(),
            pairs: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Config>(&bytes) {
                Ok(c) => c.sanitized(),
                Err(e) => {
                    tracing::error!("config at {} is unreadable ({e}); keeping a backup and using defaults", path.display());
                    let _ = std::fs::copy(path, path.with_extension("json.bad"));
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    fn sanitized(mut self) -> Self {
        self.max_concurrent_transfers = self.max_concurrent_transfers.clamp(1, 32);
        self.remote_poll_secs = self.remote_poll_secs.clamp(5, 3600);
        self.full_rescan_mins = self.full_rescan_mins.clamp(5, 24 * 60);
        self.debounce_ms = self.debounce_ms.clamp(200, 30_000);
        self
    }

    /// Writes the config atomically (temp file + rename) so a crash never leaves a half-written file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn pair(&self, id: &str) -> Option<&PairConfig> {
        self.pairs.iter().find(|p| p.id == id)
    }

    pub fn client_credentials(&self) -> Option<(String, String)> {
        let id = if self.google_client_id.trim().is_empty() {
            option_env!("SYNC_GOOGLE_CLIENT_ID").unwrap_or("").to_string()
        } else {
            self.google_client_id.trim().to_string()
        };
        let secret = if self.google_client_secret.trim().is_empty() {
            option_env!("SYNC_GOOGLE_CLIENT_SECRET").unwrap_or("").to_string()
        } else {
            self.google_client_secret.trim().to_string()
        };
        if id.is_empty() {
            None
        } else {
            Some((id, secret))
        }
    }
}

pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Result<AppPaths> {
        let config_dir = dirs::config_dir().context("no config directory")?.join("schme16-sync");
        let data_dir = dirs::data_local_dir().context("no data directory")?.join("schme16-sync");
        let log_dir = data_dir.join("logs");
        for d in [&config_dir, &data_dir, &log_dir, &data_dir.join("pairs")] {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        Ok(AppPaths { config_dir, data_dir, log_dir })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    pub fn pair_db(&self, pair_id: &str) -> PathBuf {
        self.data_dir.join("pairs").join(format!("{pair_id}.db"))
    }
}
