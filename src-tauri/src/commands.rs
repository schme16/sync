//! Commands invoked from the UI.

use crate::config::{Account, Config, PairConfig, Theme};
use crate::drive::auth::{self, Auth};
use crate::drive::Drive;
use crate::engine::{Cmd, Manager};
use crate::fsutil::{self, child_key, fold};
use crate::hub::{ActivityItem, Snapshot};
use crate::ignore_rules::{escape_pattern, MatchResult, RuleError, Rules};
use crate::scanner::Layout;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    pub manager: Arc<Manager>,
    pub auth_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn save(state: &AppState) -> Res<()> {
    let sh = &state.manager.shared;
    sh.config.read().save(&sh.paths.config_file()).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.manager.shared.config.read().clone()
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatch {
    theme: Option<Theme>,
    launch_at_startup: Option<bool>,
    start_minimized: Option<bool>,
    max_concurrent_transfers: Option<usize>,
    remote_poll_secs: Option<u64>,
    full_rescan_mins: Option<u64>,
    debounce_ms: Option<u64>,
    use_trash: Option<bool>,
    delete_safety_threshold: Option<usize>,
    global_ignore: Option<String>,
    google_client_id: Option<String>,
    google_client_secret: Option<String>,
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, patch: SettingsPatch) -> Res<Config> {
    let sh = state.manager.shared.clone();
    let (rules_changed, creds_changed) = {
        let mut c = sh.config.write();
        if let Some(v) = patch.theme {
            c.theme = v;
        }
        if let Some(v) = patch.launch_at_startup {
            fsutil::set_autostart(v).map_err(|e| format!("could not change launch at startup: {e:#}"))?;
            c.launch_at_startup = v;
        }
        if let Some(v) = patch.start_minimized {
            c.start_minimized = v;
        }
        if let Some(v) = patch.max_concurrent_transfers {
            c.max_concurrent_transfers = v.clamp(1, 32);
        }
        if let Some(v) = patch.remote_poll_secs {
            c.remote_poll_secs = v.clamp(5, 3600);
        }
        if let Some(v) = patch.full_rescan_mins {
            c.full_rescan_mins = v.clamp(5, 1440);
        }
        if let Some(v) = patch.debounce_ms {
            c.debounce_ms = v.clamp(200, 30_000);
        }
        if let Some(v) = patch.use_trash {
            c.use_trash = v;
        }
        if let Some(v) = patch.delete_safety_threshold {
            c.delete_safety_threshold = v;
        }
        let rules_changed = patch.global_ignore.as_ref().is_some_and(|g| *g != c.global_ignore);
        if let Some(v) = patch.global_ignore {
            c.global_ignore = v;
        }
        let mut creds_changed = false;
        if let Some(v) = patch.google_client_id {
            creds_changed |= v.trim() != c.google_client_id;
            c.google_client_id = v.trim().to_string();
        }
        if let Some(v) = patch.google_client_secret {
            creds_changed |= v.trim() != c.google_client_secret;
            c.google_client_secret = v.trim().to_string();
        }
        (rules_changed, creds_changed)
    };
    save(&state)?;
    if creds_changed {
        sh.forget_all_drives();
        let pairs = sh.config.read().pairs.clone();
        for p in pairs {
            state.manager.restart_pair(p).await;
        }
    } else if rules_changed {
        state.manager.sync_all_now();
    }
    let c = sh.config.read().clone();
    Ok(c)
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> Snapshot {
    state.manager.shared.hub.snapshot()
}

#[tauri::command]
pub fn get_activity(state: State<'_, AppState>, limit: Option<usize>) -> Vec<ActivityItem> {
    state.manager.shared.hub.activity_list(limit.unwrap_or(1000))
}

#[tauri::command]
pub fn clear_activity(state: State<'_, AppState>) {
    state.manager.shared.hub.clear_activity()
}

// ---------------------------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn add_account(app: AppHandle, state: State<'_, AppState>) -> Res<Account> {
    let sh = state.manager.shared.clone();
    let (client_id, secret) = sh
        .config
        .read()
        .client_credentials()
        .ok_or("Add your Google API client ID and secret in Settings first (see the setup guide).")?;
    let http = sh.http.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let app2 = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let r = auth::authorize(&http, &client_id, &secret, |url| {
            if let Err(e) = app2.opener().open_url(url, None::<&str>) {
                tracing::error!("could not open browser: {e}");
            }
        })
        .await
        .map(|t| (t, client_id, secret));
        let _ = tx.send(r);
    });
    if let Some(old) = state.auth_task.lock().replace(task) {
        old.abort();
    }
    let (tokens, client_id, secret) = rx.await.map_err(|_| "Sign-in was cancelled".to_string())?.map_err(|e| format!("{e:#}"))?;
    let auth = Auth::new(sh.http.clone(), client_id, secret, tokens.refresh_token.clone()).with_access(tokens.access_token, tokens.expires_in);
    let drive = Drive::new(sh.http.clone(), Arc::new(auth));
    let user = drive.about().await.map_err(|e| format!("{e:#}"))?;
    let account = Account {
        id: if user.permission_id.is_empty() { user.email_address.clone() } else { user.permission_id.clone() },
        email: user.email_address,
        display_name: user.display_name,
        photo_link: user.photo_link,
    };
    crate::secrets::store(&sh.paths.config_dir, &account.id, &tokens.refresh_token).map_err(err)?;
    sh.register_drive(&account.id, drive);
    {
        let mut c = sh.config.write();
        c.accounts.retain(|a| a.id != account.id);
        c.accounts.push(account.clone());
    }
    save(&state)?;
    // pairs of a re-authorised account may have been stuck on an expired token
    let pairs: Vec<PairConfig> = sh.config.read().pairs.iter().filter(|p| p.account_id == account.id).cloned().collect();
    for p in pairs {
        state.manager.restart_pair(p).await;
    }
    Ok(account)
}

#[tauri::command]
pub fn cancel_add_account(state: State<'_, AppState>) {
    if let Some(t) = state.auth_task.lock().take() {
        t.abort();
    }
}

#[tauri::command]
pub fn remove_account(state: State<'_, AppState>, id: String) -> Res<()> {
    let sh = &state.manager.shared;
    if sh.config.read().pairs.iter().any(|p| p.account_id == id) {
        return Err("This account is used by a sync pair. Remove those pairs first.".into());
    }
    sh.config.write().accounts.retain(|a| a.id != id);
    crate::secrets::delete(&sh.paths.config_dir, &id);
    sh.forget_drive(&id);
    save(&state)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderItem {
    id: String,
    name: String,
    drive_id: Option<String>,
}

#[tauri::command]
pub async fn drive_root(state: State<'_, AppState>, account_id: String) -> Res<FolderItem> {
    let d = state.manager.shared.drive(&account_id).map_err(|e| format!("{e:#}"))?;
    let f = d.get("root").await.map_err(|e| format!("{e:#}"))?;
    Ok(FolderItem { id: f.id, name: "My Drive".into(), drive_id: None })
}

#[tauri::command]
pub async fn list_drive_folders(state: State<'_, AppState>, account_id: String, parent_id: String) -> Res<Vec<FolderItem>> {
    let d = state.manager.shared.drive(&account_id).map_err(|e| format!("{e:#}"))?;
    let v = d.list_folders(&parent_id).await.map_err(|e| format!("{e:#}"))?;
    Ok(v.into_iter().map(|f| FolderItem { id: f.id, name: f.name, drive_id: f.drive_id }).collect())
}

#[tauri::command]
pub async fn list_shared_drives(state: State<'_, AppState>, account_id: String) -> Res<Vec<FolderItem>> {
    let d = state.manager.shared.drive(&account_id).map_err(|e| format!("{e:#}"))?;
    let v = d.shared_drives().await.map_err(|e| format!("{e:#}"))?;
    Ok(v.into_iter().map(|s| FolderItem { id: s.id.clone(), name: s.name, drive_id: Some(s.id) }).collect())
}

#[tauri::command]
pub async fn create_drive_folder(state: State<'_, AppState>, account_id: String, parent_id: String, name: String) -> Res<FolderItem> {
    let d = state.manager.shared.drive(&account_id).map_err(|e| format!("{e:#}"))?;
    let f = d.create_folder(name.trim(), &parent_id).await.map_err(|e| format!("{e:#}"))?;
    Ok(FolderItem { id: f.id, name: f.name, drive_id: f.drive_id })
}

// ---------------------------------------------------------------------------------------------
// Pairs
// ---------------------------------------------------------------------------------------------

fn norm(p: &str) -> String {
    let s = std::path::absolute(p).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string());
    let s = s.replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn overlaps(a: &str, b: &str) -> bool {
    let (a, b) = (norm(a), norm(b));
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

fn validate(cfg: &Config, p: &mut PairConfig) -> Res<()> {
    p.name = p.name.trim().to_string();
    if p.local_path.trim().is_empty() {
        return Err("Choose a local folder.".into());
    }
    if p.remote_folder_id.is_empty() {
        return Err("Choose a Google Drive folder.".into());
    }
    if !cfg.accounts.iter().any(|a| a.id == p.account_id) {
        return Err("Choose an account.".into());
    }
    let root = std::path::Path::new(&p.local_path);
    if !root.exists() {
        std::fs::create_dir_all(root).map_err(|e| format!("Cannot create {}: {e}", root.display()))?;
    } else if !root.is_dir() {
        return Err("The local path is not a folder.".into());
    }
    for other in cfg.pairs.iter().filter(|o| o.id != p.id) {
        if overlaps(&other.local_path, &p.local_path) {
            return Err(format!("This folder overlaps the sync pair \"{}\".", other.name));
        }
        if other.remote_folder_id == p.remote_folder_id && other.account_id == p.account_id {
            return Err(format!("That Drive folder is already synced by \"{}\".", other.name));
        }
    }
    let mut seen = Vec::new();
    for m in p.mounts.iter_mut() {
        m.rel_path = m.rel_path.replace('\\', "/").trim_matches('/').to_string();
        if m.rel_path.is_empty() || m.rel_path.split('/').any(|c| c.is_empty() || c == "." || c == ".." || !fsutil::valid_local_name(c)) {
            return Err(format!("\"{}\" is not a valid location inside the sync folder.", m.rel_path));
        }
        if !std::path::Path::new(&m.local_path).is_dir() {
            return Err(format!("External folder {} does not exist.", m.local_path));
        }
        if overlaps(&m.local_path, &p.local_path) {
            return Err(format!("External folder {} overlaps the sync folder itself.", m.local_path));
        }
        let k = fold(&m.rel_path).into_owned();
        if seen.iter().any(|s: &String| fsutil::is_within(&k, s) || fsutil::is_within(s, &k)) {
            return Err("External folders cannot be nested inside each other.".into());
        }
        seen.push(k);
    }
    if p.name.is_empty() {
        p.name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Sync".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn add_pair(state: State<'_, AppState>, mut pair: PairConfig) -> Res<PairConfig> {
    let sh = state.manager.shared.clone();
    pair.id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    {
        let cfg = sh.config.read();
        validate(&cfg, &mut pair)?;
    }
    sh.config.write().pairs.push(pair.clone());
    save(&state)?;
    state.manager.start_pair(pair.clone());
    Ok(pair)
}

#[tauri::command]
pub async fn update_pair(state: State<'_, AppState>, mut pair: PairConfig) -> Res<PairConfig> {
    let sh = state.manager.shared.clone();
    {
        let cfg = sh.config.read();
        if cfg.pair(&pair.id).is_none() {
            return Err("Unknown sync pair".into());
        }
        validate(&cfg, &mut pair)?;
    }
    {
        let mut c = sh.config.write();
        if let Some(p) = c.pairs.iter_mut().find(|p| p.id == pair.id) {
            *p = pair.clone();
        }
    }
    save(&state)?;
    state.manager.restart_pair(pair.clone()).await;
    Ok(pair)
}

#[tauri::command]
pub async fn remove_pair(state: State<'_, AppState>, id: String) -> Res<()> {
    let sh = state.manager.shared.clone();
    state.manager.stop_pair(&id).await;
    sh.config.write().pairs.retain(|p| p.id != id);
    save(&state)?;
    let db = sh.paths.pair_db(&id);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    Ok(())
}

#[tauri::command]
pub fn set_pair_paused(state: State<'_, AppState>, id: String, paused: bool) -> Res<()> {
    let sh = &state.manager.shared;
    if let Some(p) = sh.config.write().pairs.iter_mut().find(|p| p.id == id) {
        p.paused = paused;
    }
    save(&state)?;
    let all = sh.hub.paused_all.load(std::sync::atomic::Ordering::Relaxed);
    state.manager.send(&id, Cmd::SetPaused(paused || all));
    Ok(())
}

#[tauri::command]
pub fn set_paused_all(state: State<'_, AppState>, paused: bool) {
    state.manager.set_paused_all(paused);
}

#[tauri::command]
pub fn sync_now(state: State<'_, AppState>, id: Option<String>) {
    match id {
        Some(id) => state.manager.send(&id, Cmd::SyncNow),
        None => state.manager.sync_all_now(),
    }
}

#[tauri::command]
pub fn confirm_deletes(state: State<'_, AppState>, id: String, apply: bool) {
    state.manager.send(&id, Cmd::ConfirmDeletes(apply));
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnoreTest {
    result: MatchResult,
    errors: Vec<RuleError>,
}

#[tauri::command]
pub fn test_ignore(state: State<'_, AppState>, rules: String, use_global: bool, path: String, is_dir: bool) -> IgnoreTest {
    let global = use_global.then(|| state.manager.shared.config.read().global_ignore.clone());
    let (r, errors) = Rules::new(global.as_deref(), &rules);
    let path = path.replace('\\', "/");
    IgnoreTest { result: r.explain(&path, is_dir), errors }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseEntry {
    name: String,
    path: String,
    is_dir: bool,
    local: bool,
    remote_id: Option<String>,
    ignored: bool,
    rule: Option<String>,
}

/// Lists one folder level of a pair, merging what is on disk with what is in Drive, and
/// showing which entries are excluded. Used by the selective-sync tree.
#[tauri::command]
pub async fn browse_pair(state: State<'_, AppState>, pair_id: String, path: String, folder_id: Option<String>) -> Res<Vec<BrowseEntry>> {
    let sh = state.manager.shared.clone();
    let (pair, global) = {
        let c = sh.config.read();
        let p = c.pair(&pair_id).cloned().ok_or("Unknown sync pair")?;
        let g = p.use_global_ignore.then(|| c.global_ignore.clone());
        (p, g)
    };
    let (rules, _) = Rules::new(global.as_deref(), &pair.ignore);
    let layout = Layout::new(&pair.local_path, &pair.mounts);
    let path = path.trim_matches('/').to_string();
    let key = fold(&path).into_owned();
    let mut map: BTreeMap<String, BrowseEntry> = BTreeMap::new();
    if let Ok(list) = fsutil::list_dir(&layout.resolve(&key)) {
        for e in list.into_iter().filter(|e| e.is_dir && !fsutil::is_temp_name(&e.name)) {
            map.insert(fold(&e.name).into_owned(), BrowseEntry { path: child_key(&path, &e.name), name: e.name, is_dir: true, local: true, remote_id: None, ignored: false, rule: None });
        }
    }
    for m in layout.mounts.iter().filter(|m| fsutil::parent_key(&m.key) == key) {
        let name = fsutil::last_component(&m.real).to_string();
        map.insert(fold(&name).into_owned(), BrowseEntry { path: m.real.clone(), name, is_dir: true, local: true, remote_id: None, ignored: false, rule: None });
    }
    let fid = folder_id.or_else(|| path.is_empty().then(|| pair.remote_folder_id.clone()));
    if let Some(fid) = fid {
        if let Ok(d) = sh.drive(&pair.account_id) {
            if let Ok(list) = d.list_folders(&fid).await {
                for f in list {
                    let e = map.entry(fold(&f.name).into_owned()).or_insert_with(|| BrowseEntry {
                        path: child_key(&path, &f.name),
                        name: f.name.clone(),
                        is_dir: true,
                        local: false,
                        remote_id: None,
                        ignored: false,
                        rule: None,
                    });
                    e.remote_id = Some(f.id);
                }
            }
        }
    }
    let mut out: Vec<BrowseEntry> = map.into_values().collect();
    for e in out.iter_mut() {
        let m = rules.explain(&e.path, true);
        e.ignored = m.ignored;
        e.rule = m.rule;
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

const SELECTIVE_HEADER: &str = "# Selective sync (managed by the folder tree)";

/// Adds or removes an anchored exclusion for one folder in the pair's rules.
#[tauri::command]
pub async fn set_folder_excluded(state: State<'_, AppState>, pair_id: String, path: String, excluded: bool) -> Res<PairConfig> {
    let sh = state.manager.shared.clone();
    let mut pair = sh.config.read().pair(&pair_id).cloned().ok_or("Unknown sync pair")?;
    let line = format!("/{}/", escape_pattern(path.trim_matches('/')));
    let mut lines: Vec<String> = pair.ignore.lines().map(|s| s.to_string()).collect();
    lines.retain(|l| l.trim() != line);
    if excluded {
        if !lines.iter().any(|l| l.trim() == SELECTIVE_HEADER) {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(SELECTIVE_HEADER.into());
        }
        lines.push(line);
    } else if lines.last().is_some_and(|l| l.trim() == SELECTIVE_HEADER) {
        lines.pop();
    }
    pair.ignore = lines.join("\n");
    if !pair.ignore.is_empty() {
        pair.ignore.push('\n');
    }
    update_pair(state, pair).await
}

// ---------------------------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub fn open_local(app: AppHandle, state: State<'_, AppState>, pair_id: String, path: Option<String>) -> Res<()> {
    let pair = state.manager.shared.config.read().pair(&pair_id).cloned().ok_or("Unknown sync pair")?;
    let layout = Layout::new(&pair.local_path, &pair.mounts);
    let mut target = layout.resolve(&fold(path.as_deref().unwrap_or("")));
    if target.is_file() {
        target = target.parent().map(|p| p.to_path_buf()).unwrap_or(target);
    }
    app.opener().open_path(target.to_string_lossy(), None::<&str>).map_err(err)
}

#[tauri::command]
pub fn open_in_drive(app: AppHandle, state: State<'_, AppState>, pair_id: String) -> Res<()> {
    let pair = state.manager.shared.config.read().pair(&pair_id).cloned().ok_or("Unknown sync pair")?;
    app.opener().open_url(format!("https://drive.google.com/drive/folders/{}", pair.remote_folder_id), None::<&str>).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    os: String,
    config_dir: String,
    data_dir: String,
    log_dir: String,
    has_builtin_credentials: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let p = &state.manager.shared.paths;
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        os: std::env::consts::OS.into(),
        config_dir: p.config_dir.display().to_string(),
        data_dir: p.data_dir.display().to_string(),
        log_dir: p.log_dir.display().to_string(),
        has_builtin_credentials: option_env!("SYNC_GOOGLE_CLIENT_ID").is_some(),
    }
}

#[tauri::command]
pub fn open_log_dir(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    let dir = state.manager.shared.paths.log_dir.clone();
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(err)
}
