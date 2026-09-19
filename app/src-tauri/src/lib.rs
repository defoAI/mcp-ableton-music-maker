//! The Mac companion app. It never runs the server Claude talks to: clients
//! spawn the bundled `ableton-music-maker` themselves. This app installs the
//! Remote Script, writes the client config, and reads what the server writes
//! under the state directory. It opens no socket except the one to Live, for
//! the Check step.

mod activity;
mod clients;
pub mod listen;
mod settings;
mod status;
mod tray;

use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

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

// ── listening to Live ───────────────────────────────────────────────────────

#[tauri::command]
async fn listen_start(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || listen::start(&app))
        .await
        .map_err(err)?
}

#[tauri::command]
fn listen_stop(app: AppHandle) -> Value {
    listen::stop(&app)
}

#[tauri::command]
fn listen_status(app: AppHandle) -> Result<Value, String> {
    listen::status(&app)
}

/// The small always-on-top spectrum that sits beside Live's own window.
#[tauri::command]
fn listen_float(app: AppHandle, open: bool) -> Result<Value, String> {
    if !open {
        if let Some(w) = app.get_webview_window(FLOAT_LABEL) {
            save_float_geometry(&app, &w);
            let _ = w.close();
        }
        return Ok(json!({"open": false}));
    }
    if let Some(w) = app.get_webview_window(FLOAT_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(json!({"open": true}));
    }
    let s = settings::load(&app);
    let mut b = WebviewWindowBuilder::new(&app, FLOAT_LABEL, WebviewUrl::App("float.html".into()))
        .title("Live · master")
        .inner_size(s.float_w.unwrap_or(360.0), s.float_h.unwrap_or(180.0))
        .min_inner_size(240.0, 120.0)
        .always_on_top(true)
        .skip_taskbar(false)
        .resizable(true);
    if let (Some(x), Some(y)) = (s.float_x, s.float_y) {
        b = b.position(x, y);
    }
    b.build().map_err(err)?;
    Ok(json!({"open": true}))
}

const FLOAT_LABEL: &str = "listen-float";
const VISUAL_LABEL: &str = "listen-visual";

/// The visual: a window of its own that can go full screen, drawn from the
/// same frames the Listen screen receives. Listening keeps running while it
/// is open, even with the main window hidden.
#[tauri::command]
fn listen_visual(app: AppHandle, open: bool, fullscreen: Option<bool>) -> Result<Value, String> {
    if !open {
        if let Some(w) = app.get_webview_window(VISUAL_LABEL) {
            let _ = w.close();
        }
        return Ok(json!({"open": false}));
    }
    if let Some(w) = app.get_webview_window(VISUAL_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(full) = fullscreen {
            let _ = w.set_fullscreen(full);
        }
        return Ok(json!({"open": true}));
    }
    WebviewWindowBuilder::new(&app, VISUAL_LABEL, WebviewUrl::App("visual.html".into()))
        .title("Live · visual")
        .inner_size(960.0, 600.0)
        .min_inner_size(320.0, 200.0)
        .resizable(true)
        .fullscreen(fullscreen.unwrap_or(false))
        .build()
        .map_err(err)?;
    Ok(json!({"open": true}))
}

/// Where the float window was left, so it comes back in the same place.
fn save_float_geometry(app: &AppHandle, w: &tauri::WebviewWindow) {
    let mut s = settings::load(app);
    if let Ok(p) = w.outer_position() {
        s.float_x = Some(p.x as f64);
        s.float_y = Some(p.y as f64);
    }
    if let Ok(size) = w.inner_size() {
        let scale = w.scale_factor().unwrap_or(1.0);
        s.float_w = Some(size.width as f64 / scale);
        s.float_h = Some(size.height as f64 / scale);
    }
    let _ = settings::save(app, &s);
}

/// The app's Tauri context: `tauri.conf.json`, the icons, the front end and,
/// on macOS, the Info.plist with the audio-capture usage string merged in.
/// One expansion for the crate, shared with the tests.
pub fn context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
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
            listen_start,
            listen_stop,
            listen_status,
            listen_float,
            listen_visual,
        ])
        .setup(|app| {
            app.manage(listen::Listen::default());
            let s = settings::load(app.handle());
            settings::apply_env(&s);
            activity::prune(app.handle(), s.retention_days);
            tray::install(app.handle())?;
            tray::start_refresh(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            let app = window.app_handle().clone();
            match event {
                // Closing the main window keeps the menu bar item; Quit lives
                // there. Listening is something the producer can see, so it
                // stops as soon as no window is showing it.
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    let label = window.label().to_string();
                    if label == FLOAT_LABEL || label == VISUAL_LABEL {
                        if label == FLOAT_LABEL {
                            if let Some(w) = app.get_webview_window(FLOAT_LABEL) {
                                save_float_geometry(&app, &w);
                            }
                        }
                        listen::stop_if_unwatched(&app, &label);
                        return;
                    }
                    api.prevent_close();
                    let _ = window.hide();
                    listen::stop_if_unwatched(&app, window.label());
                }
                tauri::WindowEvent::Destroyed => {
                    // The Listen screen keeps its Float / Visual buttons honest.
                    let _ = app.emit("listen:window-closed", json!({"label": window.label()}));
                    listen::stop_if_unwatched(&app, window.label());
                }
                _ => {}
            }
        })
        .run(context())
        .expect("error while running the app");
}
