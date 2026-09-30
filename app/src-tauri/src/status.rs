//! What the Overview shows: the chain of client → server → Live, derived
//! from heartbeats the servers write and one `check` against Live.

use crate::{settings, sidecar_path};
use mcp_ableton_music_maker::connection;
use mcp_ableton_music_maker::install::{
    discover_remote_script_dirs, install_and_reload, InstallStatus, ReloadOutcome,
    REMOTE_SCRIPT_FOLDER_NAME,
};
use mcp_ableton_music_maker::{app as server, handshake, state, MCP_VERSION};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Every heartbeat file, with stale ones (dead pid) deleted on sight.
pub fn heartbeats() -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(state::sessions_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(mut doc) = serde_json::from_slice::<Value>(&bytes) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let pid = doc["pid"].as_u64().unwrap_or(0) as u32;
        if pid == 0 || !pid_alive(pid) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        doc["alive"] = json!(true);
        out.push(doc);
    }
    out.sort_by(|a, b| b["started"].as_str().cmp(&a["started"].as_str()));
    out
}

/// The Remote Script targets the installer would use, plus the override.
fn targets(app: &AppHandle) -> Vec<PathBuf> {
    let s = settings::load(app);
    let mut dirs = Vec::new();
    if let Some(lib) = s.library {
        dirs.push(PathBuf::from(lib).join("Remote Scripts"));
    }
    for d in discover_remote_script_dirs() {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// Read `NAME = "x.y.z"` out of an installed Python file.
fn declared_version(file: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix(name)?;
        let rest = rest.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
    })
}

/// Step 1 of Setup: where the script would go, and what is there now.
///
/// The script is two files. `body.py` carries `SCRIPT_VERSION` and can be
/// replaced in a running Live; `__init__.py` carries `LOADER_VERSION` and is
/// the one Live only reads when it starts, so the Setup step's "Restart Live
/// to finish" is true of that file alone.
pub fn script_state(app: &AppHandle) -> Value {
    let expected = handshake::expected_remote_script_version();
    let expected_loader = handshake::expected_loader_version();
    let targets: Vec<Value> = targets(app)
        .into_iter()
        .map(|dir| {
            let folder = dir.join(REMOTE_SCRIPT_FOLDER_NAME);
            let loader = folder.join("__init__.py");
            let body = folder.join("body.py");
            let installed = declared_version(&body, "SCRIPT_VERSION");
            let installed_loader = declared_version(&loader, "LOADER_VERSION");
            let loader_ok = installed_loader.as_deref() == Some(expected_loader);
            json!({
                "dir": dir,
                "script": loader,
                "body": body,
                "installed": installed,
                "installed_loader": installed_loader,
                "loader_up_to_date": loader_ok,
                "up_to_date": installed.as_deref() == Some(expected) && loader_ok,
                // True when the only thing behind is the body: the app can
                // say "nothing to restart" rather than "restart Live".
                "reloadable": loader_ok && installed.as_deref() != Some(expected),
                "library_exists": dir.parent().map(|p| p.is_dir()).unwrap_or(false),
            })
        })
        .collect();
    json!({"expected": expected, "expected_loader": expected_loader, "targets": targets})
}

/// Install both halves and, when only the handlers changed, hand them to a
/// running Live in place. Then the Setup step's "Restart Live to finish" is
/// true only when the loader changed, which is the one file Live reads once
/// (decision 0011).
pub fn install_script(app: &AppHandle) -> Result<Value, String> {
    let s = settings::load(app);
    let target = s.library.map(|l| PathBuf::from(l).join("Remote Scripts"));
    let (host, port) = connection::live_address();
    let bridge = connection::AbletonConnection::new(host, port);
    let (results, outcome) = install_and_reload(target.as_deref(), true, Some(&bridge));
    bridge.disconnect();
    if results.is_empty() {
        return Err("No Ableton User Library found. Choose it with “Change library…”.".into());
    }
    let ok = results.iter().any(|r| {
        matches!(
            r.status,
            InstallStatus::Installed | InstallStatus::Updated | InstallStatus::Unchanged
        )
    });
    let rows: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "path": r.path,
                "status": r.status.to_string(),
                "detail": r.detail,
                "backup": r.backup,
            })
        })
        .collect();
    if ok {
        // What the producer is told to do next, decided here rather than in
        // the UI: `reloaded` means nothing restarts.
        let (reloaded, note) = match &outcome {
            ReloadOutcome::Reloaded(r) => (
                true,
                format!(
                    "Live picked up the new handlers in place (v{} \u{2192} v{}). Nothing to restart.",
                    r["was"].as_str().unwrap_or("?"),
                    r["now"].as_str().unwrap_or("?")
                ),
            ),
            ReloadOutcome::NothingToDo => (true, "Already installed.".to_string()),
            ReloadOutcome::LoaderChanged => (
                false,
                "Restart Live to load it, or re-select AbletonMusicMaker under Settings \u{203a} Link, Tempo & MIDI."
                    .to_string(),
            ),
            _ => (false, "Installed. Restart Live to load it.".to_string()),
        };
        Ok(json!({
            "ok": true,
            "results": rows,
            "reloaded": reloaded,
            "note": note,
            "reload": match &outcome {
                ReloadOutcome::Reloaded(r) => r.clone(),
                _ => Value::Null,
            },
        }))
    } else {
        Err(results
            .iter()
            .map(|r| format!("{}: {}", r.status, r.detail))
            .collect::<Vec<_>>()
            .join("; "))
    }
}

/// One `--check` against Live, in process: handshake plus one read.
pub fn check_live(app: &AppHandle) -> Value {
    settings::apply_env(&settings::load(app));
    let (report, _ok) = server::check();
    report
}

/// The newest line of the newest activity file, for the menu bar.
fn last_call() -> Value {
    let Ok(entries) = std::fs::read_dir(state::activity_dir()) else {
        return Value::Null;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    let Some(newest) = files.pop() else { return Value::Null };
    std::fs::read_to_string(newest)
        .ok()
        .and_then(|t| t.lines().last().and_then(|l| serde_json::from_str(l).ok()))
        .unwrap_or(Value::Null)
}

pub fn status(app: &AppHandle) -> Value {
    let beats = heartbeats();
    let live = check_live(app);
    json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "server_version": MCP_VERSION,
        "expected_script_version": handshake::expected_remote_script_version(),
        "sidecar": sidecar_path(),
        "sidecar_present": sidecar_path().is_file(),
        "state_dir": state::state_dir(),
        "sessions": beats,
        "live": live,
        "last_call": last_call(),
        "clients": crate::clients::summary(app),
        "script": script_state(app),
    })
}
