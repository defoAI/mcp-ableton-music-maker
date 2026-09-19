//! The Mac companion app. It never runs the server Claude talks to: clients
//! spawn the bundled `ableton-music-maker` themselves. This app installs the
//! Remote Script, writes the client config, and reads what the server writes
//! under the state directory. It opens no socket except the one to Live, for
//! the Check step.

mod activity;
mod clients;
mod settings;
mod status;
mod tray;

use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub use settings::Settings;

/// Where the bundled server binary is: next to this executable, which is
/// `Contents/MacOS/` in the app bundle and `target/debug/` in development.
pub fn sidecar_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("ableton-music-maker")))
        .unwrap_or_else(|| PathBuf::from("ableton-music-maker"))
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ── commands ────────────────────────────────────────────────────────────────

#[tauri::command]
async fn get_status(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || status::status(&app))
        .await
        .map_err(err)
}

#[tauri::command]
async fn check_live(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || status::check_live(&app))
        .await
        .map_err(err)
}

#[tauri::command]
fn script_state(app: AppHandle) -> Result<Value, String> {
    Ok(status::script_state(&app))
}

#[tauri::command]
async fn install_script(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || status::install_script(&app))
        .await
        .map_err(err)?
}

#[tauri::command]
fn pick_library(app: AppHandle) -> Result<Value, String> {
    // macOS only: the native folder chooser without a plugin.
    let out = std::process::Command::new("osascript")
        .args([
            "-e",
            "POSIX path of (choose folder with prompt \"Choose your Ableton User Library\")",
        ])
        .output()
        .map_err(err)?;
    if !out.status.success() {
        return Ok(json!({"cancelled": true}));
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().trim_end_matches('/').to_string();
    let mut s = settings::load(&app);
    s.library = Some(path.clone());
    settings::save(&app, &s)?;
    Ok(json!({"library": path}))
}

#[tauri::command]
fn client_config(app: AppHandle, kind: String) -> Result<Value, String> {
    clients::describe(&app, &kind)
}

#[tauri::command]
fn configure_client(app: AppHandle, kind: String) -> Result<Value, String> {
    clients::configure(&app, &kind)
}

#[tauri::command]
async fn list_captures(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        settings::apply_env(&settings::load(&app));
        mcp_ableton_music_maker::app::captures()
    })
    .await
    .map_err(err)?
}

/// Play an audio file Live recorded, in place, with the system player.
#[tauri::command]
fn play_file(path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).is_file() {
        return Err(format!("{path} is not there any more"));
    }
    std::process::Command::new("afplay")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(err)
}

#[tauri::command]
fn reveal_file(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .args(["-R", &path])
        .spawn()
        .map(|_| ())
        .map_err(err)
}

#[tauri::command]
fn remove_legacy_client(app: AppHandle) -> Result<Value, String> {
    clients::remove_legacy(&app)
}

#[tauri::command]
fn activity_sessions(app: AppHandle) -> Result<Value, String> {
    Ok(activity::sessions(&app))
}

#[tauri::command]
fn activity_lines(session: String) -> Result<Value, String> {
    activity::lines(&session)
}

#[tauri::command]
fn clear_session(session: String) -> Result<Value, String> {
    activity::clear(&session)
}

#[tauri::command]
fn delete_local_data(app: AppHandle) -> Result<Value, String> {
    activity::delete_all(&app)
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    Ok(settings::load(&app))
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: Settings) -> Result<Value, String> {
    settings::save(&app, &settings)?;
    settings::apply_env(&settings);
    // The client-started server reads its switches from the client config's
    // env block, so keep Claude Desktop's entry in step when it exists.
    let synced = clients::resync_desktop_env(&app).unwrap_or(false);
    Ok(json!({"saved": true, "desktop_config_updated": synced}))
}

#[tauri::command]
fn open_external(target: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&target)
        .spawn()
        .map(|_| ())
        .map_err(err)
}

#[tauri::command]
fn show_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_status,
            check_live,
            script_state,
            install_script,
            pick_library,
            client_config,
            configure_client,
            remove_legacy_client,
            list_captures,
            play_file,
            reveal_file,
            activity_sessions,
            activity_lines,
            clear_session,
            delete_local_data,
            get_settings,
            set_settings,
            open_external,
            show_window,
        ])
        .setup(|app| {
            let s = settings::load(app.handle());
            settings::apply_env(&s);
            activity::prune(app.handle(), s.retention_days);
            tray::install(app.handle())?;
            tray::start_refresh(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the menu bar item; Quit lives there.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
