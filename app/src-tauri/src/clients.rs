//! Connecting a client to the bundled server. Claude Desktop is configured by
//! editing its JSON file (shown first, backed up first); Claude Code and
//! Cursor get the exact command or path to paste.

use crate::{settings, sidecar_path};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use tauri::AppHandle;

const ENTRY: &str = "AbletonMusicMaker";

fn desktop_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join("Library/Application Support/Claude/claude_desktop_config.json")
}

fn read_json(path: &PathBuf) -> Result<Map<String, Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?
            .as_object()
            .cloned()
            .ok_or_else(|| format!("{} is not a JSON object", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn desktop_entry(app: &AppHandle) -> Value {
    let s = settings::load(app);
    json!({
        "command": sidecar_path(),
        "env": s.server_env(),
    })
}

fn desktop_configured(_app: &AppHandle) -> bool {
    let Ok(doc) = read_json(&desktop_config_path()) else {
        return false;
    };
    doc.get("mcpServers")
        .and_then(|m| m.get(ENTRY))
        .and_then(|e| e.get("command"))
        .and_then(Value::as_str)
        .map(|c| PathBuf::from(c) == sidecar_path())
        .unwrap_or(false)
}

const LEGACY_ENTRY: &str = "AbletonMCP";

/// The original project's entry (`uvx ableton-mcp`) talks to the same Remote
/// Script; with both present Claude sees two overlapping tool sets.
fn legacy_present() -> bool {
    read_json(&desktop_config_path())
        .ok()
        .and_then(|d| d.get("mcpServers").and_then(|m| m.get(LEGACY_ENTRY)).cloned())
        .is_some()
}

pub fn summary(app: &AppHandle) -> Value {
    json!({
        "desktop": {
            "configured": desktop_configured(app),
            "path": desktop_config_path(),
            "legacy_entry": legacy_present(),
        },
    })
}

/// Remove the legacy `AbletonMCP` entry, backing the file up first.
pub fn remove_legacy(_app: &AppHandle) -> Result<Value, String> {
    let path = desktop_config_path();
    let mut doc = read_json(&path)?;
    let Some(servers) = doc.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(json!({"removed": false}));
    };
    if servers.remove(LEGACY_ENTRY).is_none() {
        return Ok(json!({"removed": false}));
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = path.with_file_name(format!("claude_desktop_config.json.bak-{stamp}"));
    std::fs::copy(&path, &backup).map_err(|e| format!("backup failed: {e}"))?;
    let text = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({"removed": true, "backup": backup}))
}

pub fn describe(app: &AppHandle, kind: &str) -> Result<Value, String> {
    let sidecar = sidecar_path();
    let sidecar_str = sidecar.to_string_lossy().to_string();
    Ok(match kind {
        "desktop" => json!({
            "kind": "desktop",
            "path": desktop_config_path(),
            "entry": {ENTRY: desktop_entry(app)},
            "configured": desktop_configured(app),
        }),
        "code" => json!({
            "kind": "code",
            "command": format!("claude mcp add {ENTRY} \"{sidecar_str}\""),
        }),
        "cursor" => json!({
            "kind": "cursor",
            "command": sidecar_str,
        }),
        other => return Err(format!("unknown client {other}")),
    })
}

/// Merge our entry into Claude Desktop's config, keeping a timestamped
/// backup beside it and every other key intact.
pub fn configure(app: &AppHandle, kind: &str) -> Result<Value, String> {
    if kind != "desktop" {
        return Err("only Claude Desktop is configured by the app; copy the command for the others".into());
    }
    let path = desktop_config_path();
    let mut doc = read_json(&path)?;
    let backup = if path.exists() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup = path.with_file_name(format!("claude_desktop_config.json.bak-{stamp}"));
        std::fs::copy(&path, &backup).map_err(|e| format!("backup failed: {e}"))?;
        Some(backup)
    } else {
        None
    };
    let servers = doc
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;
    servers.insert(ENTRY.into(), desktop_entry(app));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({"written": path, "backup": backup}))
}

/// Settings changed: if Claude Desktop already points at us, update its env
/// block so the next server it starts uses the new switches.
pub fn resync_desktop_env(app: &AppHandle) -> Result<bool, String> {
    if !desktop_configured(app) {
        return Ok(false);
    }
    configure(app, "desktop").map(|_| true)
}
