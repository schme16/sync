//! Update checks against GitHub releases, and in-place installation.
//!
//! The latest published release (drafts and pre-releases are ignored by GitHub's `/releases/latest`)
//! is compared with this build's version. Installing downloads the asset that matches how this copy
//! was installed (NSIS setup, MSI or AppImage), verifies it against GitHub's SHA-256 digest, stops
//! the sync engine cleanly and hands over to the new version. `.deb` installs and development builds
//! can check but not self-install: they get a link to the release page instead.

use crate::commands::AppState;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

pub const REPO: &str = "schme16/sync";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// First automatic check after start, then every few hours while running.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Deserialize, Clone, Debug)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize, Clone, Debug)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    /// "sha256:<hex>", provided by GitHub for uploaded assets.
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub published_at: String,
    /// The file that would be installed, when this copy can update itself.
    pub asset_name: Option<String>,
    pub asset_size: u64,
    /// Why it can't self-install (shown instead of the install button).
    pub manual_reason: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub current: String,
    pub checking: bool,
    pub last_checked_ms: Option<i64>,
    pub available: Option<UpdateInfo>,
    /// Bytes downloaded / total, while downloading.
    pub progress: Option<(u64, u64)>,
    pub installing: bool,
    pub error: Option<String>,
}

pub struct Updater {
    app: AppHandle,
    http: reqwest::Client,
    state: Mutex<UpdateState>,
    asset: Mutex<Option<Asset>>,
    busy: tokio::sync::Mutex<()>,
}

/// How this copy was installed, which decides what to download and how to apply it.
enum InstallKind {
    NsisSetup,
    Msi,
    #[cfg_attr(windows, allow(dead_code))]
    AppImage(PathBuf),
    Manual(String),
}

fn install_kind() -> InstallKind {
    if cfg!(debug_assertions) {
        return InstallKind::Manual("This is a development build; updates are installed by rebuilding.".into());
    }
    #[cfg(windows)]
    {
        // The NSIS installer is per-user (%LOCALAPPDATA%); the MSI installs under Program Files.
        let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
        let local = dirs::data_local_dir().and_then(|p| p.canonicalize().ok());
        return match (exe, local) {
            (Some(e), Some(l)) if e.starts_with(&l) => InstallKind::NsisSetup,
            _ => InstallKind::Msi,
        };
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(p) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file()) {
            return InstallKind::AppImage(p);
        }
        return InstallKind::Manual("Installed from a package: update it with your package manager, or download the new version from the release page.".into());
    }
    #[allow(unreachable_code)]
    InstallKind::Manual("Automatic installation isn't supported on this system.".into())
}

fn arch_tags() -> &'static [&'static str] {
    match std::env::consts::ARCH {
        "x86_64" => &["x64", "amd64", "x86_64"],
        "aarch64" => &["arm64", "aarch64"],
        _ => &[],
    }
}

fn pick_asset(assets: &[Asset], kind: &InstallKind) -> Option<Asset> {
    let suffix = match kind {
        InstallKind::NsisSetup => "-setup.exe",
        InstallKind::Msi => ".msi",
        InstallKind::AppImage(_) => ".appimage",
        InstallKind::Manual(_) => return None,
    };
    let tags = arch_tags();
    assets
        .iter()
        .find(|a| {
            let n = a.name.to_ascii_lowercase();
            n.ends_with(suffix) && tags.iter().any(|t| n.contains(t))
        })
        .cloned()
}

/// "v1.2.3" / "1.2.3-beta" → (1, 2, 3). Missing parts count as 0.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let a = it.next()?.ok()?;
    let b = it.next().unwrap_or(Ok(0)).ok()?;
    let c = it.next().unwrap_or(Ok(0)).ok()?;
    Some((a, b, c))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Updater {
    pub fn new(app: AppHandle, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            app,
            http,
            state: Mutex::new(UpdateState { current: CURRENT.into(), ..Default::default() }),
            asset: Mutex::new(None),
            busy: tokio::sync::Mutex::new(()),
        })
    }

    pub fn state(&self) -> UpdateState {
        self.state.lock().clone()
    }

    fn set(&self, f: impl FnOnce(&mut UpdateState)) {
        let snap = {
            let mut s = self.state.lock();
            f(&mut s);
            s.clone()
        };
        let _ = self.app.emit("update", &snap);
        crate::refresh_update_menu(&self.app, snap.available.as_ref().map(|a| a.version.as_str()));
    }

    async fn fetch_latest(&self) -> Result<Option<Release>> {
        let resp = self
            .http
            .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("could not reach GitHub")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None); // no published releases yet
        }
        if !resp.status().is_success() {
            bail!("GitHub returned {}", resp.status());
        }
        Ok(Some(resp.json::<Release>().await.context("unexpected reply from GitHub")?))
    }

    /// Checks GitHub. `skipped` hides that version (automatic checks only).
    pub async fn check(&self, skipped: Option<String>) -> UpdateState {
        let Ok(_guard) = self.busy.try_lock() else {
            return self.state(); // a check or install is already running
        };
        self.set(|s| {
            s.checking = true;
            s.error = None;
        });
        match self.fetch_latest().await {
            Ok(release) => {
                let found = release.filter(|r| is_newer(&r.tag_name, CURRENT)).filter(|r| {
                    let v = r.tag_name.trim_start_matches(['v', 'V']);
                    skipped.as_deref().map_or(true, |s| s.trim_start_matches(['v', 'V']) != v)
                });
                let info = found.map(|r| {
                    let kind = install_kind();
                    let asset = pick_asset(&r.assets, &kind);
                    let manual_reason = match (&kind, &asset) {
                        (InstallKind::Manual(why), _) => Some(why.clone()),
                        (_, None) => Some("This release has no download for your system yet.".into()),
                        _ => None,
                    };
                    let info = UpdateInfo {
                        version: r.tag_name.trim_start_matches(['v', 'V']).to_string(),
                        notes: r.body.clone().unwrap_or_default(),
                        url: r.html_url.clone(),
                        published_at: r.published_at.clone().unwrap_or_default(),
                        asset_name: asset.as_ref().map(|a| a.name.clone()),
                        asset_size: asset.as_ref().map_or(0, |a| a.size),
                        manual_reason,
                    };
                    *self.asset.lock() = asset;
                    info
                });
                if let Some(i) = &info {
                    tracing::info!("update available: {} (current {CURRENT})", i.version);
                }
                self.set(|s| {
                    s.checking = false;
                    s.last_checked_ms = Some(now_ms());
                    s.available = info;
                });
            }
            Err(e) => {
                tracing::warn!("update check failed: {e:#}");
                self.set(|s| {
                    s.checking = false;
                    s.error = Some(format!("{e:#}"));
                });
            }
        }
        self.state()
    }

    /// Runs automatic checks in the background while the setting allows it.
    pub fn spawn_auto_check(self: &Arc<Self>, state: Arc<crate::engine::Manager>) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_CHECK).await;
            loop {
                let (enabled, skipped) = {
                    let c = state.shared.config.read();
                    (c.check_for_updates, Some(c.skipped_update_version.clone()).filter(|s| !s.is_empty()))
                };
                if enabled {
                    me.check(skipped).await;
                }
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    /// Downloads and verifies the update, returning the file and how to apply it.
    async fn download(&self) -> Result<(PathBuf, InstallKind)> {
        let asset = self.asset.lock().clone().ok_or_else(|| anyhow!("no installable update; check for updates first"))?;
        let kind = install_kind();
        if let InstallKind::Manual(why) = &kind {
            bail!(why.clone());
        }
        let dest = match &kind {
            // Same directory as the AppImage, so the final rename is atomic.
            InstallKind::AppImage(p) => p.with_file_name(format!(".{}.download", asset.name)),
            _ => {
                let dir = std::env::temp_dir().join("schme16-sync-update");
                std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
                dir.join(&asset.name)
            }
        };

        self.set(|s| {
            s.error = None;
            s.progress = Some((0, asset.size));
        });
        let resp = self
            .http
            .get(&asset.browser_download_url)
            .send()
            .await
            .context("download failed")?
            .error_for_status()
            .context("download failed")?;
        let mut file = tokio::fs::File::create(&dest).await.with_context(|| format!("cannot write {}", dest.display()))?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        let mut last_emit = Instant::now();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("download interrupted")?;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            done += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(200) {
                last_emit = Instant::now();
                self.set(|s| s.progress = Some((done, asset.size)));
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        let fail = |msg: String| {
            let _ = std::fs::remove_file(&dest);
            anyhow!(msg)
        };
        if done != asset.size {
            return Err(fail(format!("download was incomplete ({done} of {} bytes)", asset.size)));
        }
        let got = hex::encode(hasher.finalize());
        match asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
            Some(want) if !want.eq_ignore_ascii_case(&got) => return Err(fail("the download did not match GitHub's checksum".into())),
            Some(_) => tracing::info!("update {} verified (sha256 {got})", asset.name),
            None => tracing::warn!("update {} has no published checksum; size verified only", asset.name),
        }
        self.set(|s| s.progress = Some((done, done)));
        Ok((dest, kind))
    }
}

/// Starts the new version after this process has exited.
fn hand_over(file: &PathBuf, kind: &InstallKind) -> Result<()> {
    let exe = std::env::current_exe().context("cannot locate the running program")?;
    match kind {
        #[cfg(windows)]
        InstallKind::NsisSetup | InstallKind::Msi => {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let q = |p: &std::path::Path| format!("\"{}\"", p.display());
            let install = match kind {
                InstallKind::NsisSetup => format!("start \"\" /wait {} /S", q(file)),
                _ => format!("start \"\" /wait msiexec /i {} /passive /norestart", q(file)),
            };
            // Give this process a moment to exit, install, then start the new version.
            let script = format!("ping -n 3 127.0.0.1 >nul & {install} & start \"\" {}", q(&exe));
            std::process::Command::new("cmd")
                .raw_arg("/S /C \"")
                .raw_arg(&script)
                .raw_arg("\"")
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .context("could not start the installer")?;
        }
        #[cfg(unix)]
        InstallKind::AppImage(target) => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))?;
            std::fs::rename(file, target).with_context(|| format!("cannot replace {}", target.display()))?;
            // Delay the relaunch so the single-instance check doesn't find this process still running.
            std::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 2; exec \"$0\"")
                .arg(target)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .context("could not restart Sync")?;
        }
        _ => bail!("automatic installation isn't supported here"),
    }
    let _ = exe;
    Ok(())
}

// ---- commands ---------------------------------------------------------------------------------

#[tauri::command]
pub fn get_update_state(state: State<'_, AppState>) -> UpdateState {
    state.updater.state()
}

/// Manual check: shows the newest version even if it was skipped before.
#[tauri::command]
pub async fn check_for_updates(state: State<'_, AppState>) -> Result<UpdateState, String> {
    let s = state.updater.check(None).await;
    match &s.error {
        Some(e) => Err(e.clone()),
        None => Ok(s),
    }
}

#[tauri::command]
pub async fn skip_update(state: State<'_, AppState>, version: String) -> Result<(), String> {
    {
        let sh = &state.manager.shared;
        let mut c = sh.config.write();
        c.skipped_update_version = version;
        c.save(&sh.paths.config_file()).map_err(|e| format!("{e:#}"))?;
    }
    state.updater.set(|s| s.available = None);
    Ok(())
}

#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let updater = state.updater.clone();
    let Ok(_guard) = updater.busy.try_lock() else {
        return Err("an update check or install is already running".into());
    };
    let (file, kind) = match updater.download().await {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("{e:#}");
            updater.set(|s| {
                s.progress = None;
                s.error = Some(msg.clone());
            });
            return Err(msg);
        }
    };
    updater.set(|s| s.installing = true);
    tracing::info!("installing update from {}", file.display());
    // Stop syncing cleanly first so nothing is cut off mid-write, then hand over and exit.
    let manager = state.manager.clone();
    let _ = tokio::time::timeout(Duration::from_secs(20), manager.shutdown()).await;
    if let Err(e) = hand_over(&file, &kind) {
        let msg = format!("{e:#}");
        tracing::error!("update hand-over failed: {msg}");
        manager.start_all();
        updater.set(|s| {
            s.installing = false;
            s.progress = None;
            s.error = Some(msg.clone());
        });
        return Err(msg);
    }
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert_eq!(parse_version("v1.2.3-beta.1"), Some((1, 2, 3)));
    }

    #[test]
    fn picks_matching_asset() {
        let a = |n: &str| Asset { name: n.into(), size: 1, browser_download_url: String::new(), digest: None };
        let assets = vec![a("Sync_0.2.0_amd64.AppImage"), a("Sync_0.2.0_amd64.deb"), a("Sync_0.2.0_x64-setup.exe"), a("Sync_0.2.0_x64_en-US.msi")];
        if std::env::consts::ARCH == "x86_64" {
            assert_eq!(pick_asset(&assets, &InstallKind::NsisSetup).unwrap().name, "Sync_0.2.0_x64-setup.exe");
            assert_eq!(pick_asset(&assets, &InstallKind::Msi).unwrap().name, "Sync_0.2.0_x64_en-US.msi");
            assert_eq!(pick_asset(&assets, &InstallKind::AppImage(PathBuf::new())).unwrap().name, "Sync_0.2.0_amd64.AppImage");
        }
        assert!(pick_asset(&assets, &InstallKind::Manual(String::new())).is_none());
    }
}
