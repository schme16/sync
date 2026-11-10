//! Sync — a fast, low-resource Google Drive sync client.

mod commands;
mod config;
mod db;
mod drive;
mod engine;
mod fsutil;
mod hash;
mod hub;
mod ignore_rules;
mod model;
mod remote_tree;
mod scanner;
mod secrets;
mod watcher;

use commands::AppState;
use config::{AppPaths, Config};
use engine::{Manager, Shared};
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager as _, RunEvent, WebviewUrl, WebviewWindowBuilder};

/// Shows the main window, creating it if needed. The window (and its web view) is destroyed
/// when closed, so a backgrounded app costs only the few MB of the sync engine itself.
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let res = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Sync")
        .inner_size(1120.0, 740.0)
        .min_inner_size(860.0, 560.0)
        .center()
        .visible(true)
        .build();
    if let Err(e) = res {
        tracing::error!("could not open window: {e}");
    }
}

fn init_logging(paths: &AppPaths) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("sync")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&paths.log_dir)
        .ok()?;
    let (nb, guard) = tracing_appender::non_blocking(appender);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sync_lib=info,hyper=warn,reqwest=warn".into());
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(nb).with_ansi(false))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init();
    Some(guard)
}

fn build_tray(app: &tauri::App, manager: Arc<Manager>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Sync", true, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause syncing", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &sync, &pause, &PredefinedMenuItem::separator(app)?, &quit])?;
    let pause_item = pause.clone();
    let mut builder = TrayIconBuilder::with_id("main").tooltip("Sync").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "sync" => manager.sync_all_now(),
            "pause" => {
                let now = !manager.shared.hub.paused_all.load(std::sync::atomic::Ordering::Relaxed);
                manager.set_paused_all(now);
                let _ = pause_item.set_text(if now { "Resume syncing" } else { "Pause syncing" });
            }
            "quit" => {
                let m = manager.clone();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    m.shutdown().await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = AppPaths::resolve().expect("cannot create application directories");
    let guard = init_logging(&paths);
    let config = Config::load(&paths.config_file());
    let minimized = std::env::args().any(|a| a == "--minimized") || config.start_minimized;
    tracing::info!("Sync {} starting", env!("CARGO_PKG_VERSION"));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let hub = hub::Hub::new(app.handle().clone());
            hub.spawn_ticker();
            let shared = Shared::new(hub, Arc::new(paths), Arc::new(RwLock::new(config)), drive::client::http_client());
            let manager = Manager::new(shared);
            app.manage(AppState { manager: manager.clone(), auth_task: Mutex::new(None) });
            build_tray(app, manager.clone())?;
            if !minimized {
                show_main(app.handle());
            }
            manager.start_all();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_settings,
            commands::get_snapshot,
            commands::get_activity,
            commands::clear_activity,
            commands::add_account,
            commands::cancel_add_account,
            commands::remove_account,
            commands::drive_root,
            commands::list_drive_folders,
            commands::list_shared_drives,
            commands::create_drive_folder,
            commands::add_pair,
            commands::update_pair,
            commands::remove_pair,
            commands::set_pair_paused,
            commands::set_paused_all,
            commands::sync_now,
            commands::confirm_deletes,
            commands::test_ignore,
            commands::browse_pair,
            commands::set_folder_excluded,
            commands::open_local,
            commands::open_in_drive,
            commands::app_info,
            commands::open_log_dir,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the application");

    app.run(move |_app, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            // Closing the window keeps syncing in the tray; only an explicit Quit exits.
            if code.is_none() {
                api.prevent_exit();
            }
        }
        let _ = &guard;
    });
}
