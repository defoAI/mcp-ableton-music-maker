//! The menu bar item: the chain at a glance, the last call, and the doors
//! into the window. Text lines are refreshed every ten seconds from the same
//! status the Overview uses.

use crate::status;
use serde_json::Value;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

pub struct TrayLines {
    client: MenuItem<tauri::Wry>,
    server: MenuItem<tauri::Wry>,
    live: MenuItem<tauri::Wry>,
    listen: MenuItem<tauri::Wry>,
    last: MenuItem<tauri::Wry>,
    fix: MenuItem<tauri::Wry>,
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let client = MenuItem::with_id(app, "st_client", "○ No client connected", false, None::<&str>)?;
    let server = MenuItem::with_id(app, "st_server", "○ Server not running", false, None::<&str>)?;
    let live = MenuItem::with_id(app, "st_live", "○ Live: checking…", false, None::<&str>)?;
    let last = MenuItem::with_id(app, "st_last", "No calls yet", false, None::<&str>)?;
    let listen = MenuItem::with_id(app, "listen", "Listen to Live…", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Ableton Music Maker", true, None::<&str>)?;
    let activity = MenuItem::with_id(app, "activity", "Show Activity", true, None::<&str>)?;
    let fix = MenuItem::with_id(app, "fix", "Fix the connection…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &client,
            &server,
            &live,
            &PredefinedMenuItem::separator(app)?,
            &last,
            &PredefinedMenuItem::separator(app)?,
            &listen,
            &open,
            &activity,
            &fix,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    app.manage(TrayLines {
        client,
        server,
        live,
        listen,
        last,
        fix,
    });
    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Ableton Music Maker")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show(app, None),
            "activity" => show(app, Some("activity")),
            "listen" => show(app, Some("listen")),
            "fix" => show(app, Some("setup")),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn show(app: &AppHandle, screen: Option<&str>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(s) = screen {
            let _ = w.eval(&format!("window.__goto && window.__goto('{s}')"));
        }
    }
}

/// Refresh the menu lines from a status document; the window's JS calls
/// this through `get_status`, and a background thread does when it is hidden.
pub fn update(app: &AppHandle, st: &Value) {
    let Some(lines) = app.try_state::<TrayLines>() else { return };
    let sessions = st["sessions"].as_array().cloned().unwrap_or_default();
    let live = &st["live"];
    let client_txt = match sessions.first() {
        Some(b) => format!(
            "● {} connected",
            b["client"]["name"].as_str().unwrap_or("A client")
        ),
        None => "○ No client connected".to_string(),
    };
    let server_txt = if sessions.is_empty() {
        "○ Server starts when a client does".to_string()
    } else if sessions.len() > 1 {
        format!("▲ {} servers running — only one should", sessions.len())
    } else {
        format!("● Server {} running", st["server_version"].as_str().unwrap_or(""))
    };
    let live_txt = if live["live_reachable"].as_bool() == Some(true) {
        if live["up_to_date"].as_bool() == Some(true) {
            format!(
                "● Live · AbletonMusicMaker {}",
                live["script_version"].as_str().unwrap_or("?")
            )
        } else {
            format!(
                "▲ Live · Remote Script {} — expected {}",
                live["script_version"].as_str().unwrap_or("?"),
                live["expected_version"].as_str().unwrap_or("?")
            )
        }
    } else {
        "✕ Live not reachable".to_string()
    };
    // Listening is visible wherever the app is: the menu bar says so too.
    let _ = lines.listen.set_text(if crate::listen::is_listening(app) {
        "● Listening to Live"
    } else {
        "Listen to Live…"
    });
    let _ = lines.client.set_text(client_txt);
    let _ = lines.server.set_text(server_txt);
    let _ = lines.live.set_text(live_txt);
    let needs_fix = live["up_to_date"].as_bool() != Some(true) || sessions.is_empty();
    let _ = lines.fix.set_enabled(needs_fix);
    if let Some(last) = st["last_call"].as_object() {
        let _ = lines.last.set_text(format!(
            "Last call: {} · {} ms · {}",
            last["tool"].as_str().unwrap_or("?"),
            last["duration_ms"].as_f64().map(|d| d.round() as i64).unwrap_or(0),
            if last["ok"].as_bool() == Some(true) { "ok" } else { "error" }
        ));
    }
}

/// Keep the menu bar honest while the window is closed.
pub fn start_refresh(app: AppHandle) {
    std::thread::spawn(move || loop {
        let visible = app
            .get_webview_window("main")
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false);
        if !visible {
            let st = status::status(&app);
            update(&app, &st);
        }
        std::thread::sleep(Duration::from_secs(10));
    });
}
