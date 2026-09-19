//! Reading the activity files the server writes. Sessions are files; the
//! client that produced one is known while its heartbeat is alive.

use crate::status::heartbeats;
use mcp_ableton_music_maker::state;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tauri::AppHandle;

fn session_file(id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return Err("bad session id".into());
    }
    Ok(state::activity_dir().join(format!("{id}.jsonl")))
}

pub fn sessions(_app: &AppHandle) -> Value {
    let beats = heartbeats();
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(state::activity_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let meta = entry.metadata().ok();
            let modified = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339());
            let beat = beats
                .iter()
                .find(|b| b["activity_file"].as_str() == Some(path.to_string_lossy().as_ref()));
            let lines = std::fs::read_to_string(&path)
                .map(|t| t.lines().count())
                .unwrap_or(0);
            rows.push(json!({
                "id": id,
                "file": path,
                "started": started_from_id(&id),
                "modified": modified,
                "bytes": meta.map(|m| m.len()).unwrap_or(0),
                "calls": lines,
                "client": beat.map(|b| b["client"].clone()).unwrap_or(Value::Null),
                "alive": beat.is_some(),
            }));
        }
    }
    rows.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    json!(rows)
}

/// `20260919-143102-4123` → a local RFC 3339 timestamp.
fn started_from_id(id: &str) -> Option<String> {
    let stamp = id.get(0..15)?;
    let naive = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S").ok()?;
    Some(
        naive
            .and_local_timezone(chrono::Local)
            .single()?
            .to_rfc3339(),
    )
}

pub fn lines(id: &str) -> Result<Value, String> {
    let path = session_file(id)?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rows: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if rows.len() > 2000 {
        rows = rows.split_off(rows.len() - 2000);
    }
    Ok(json!(rows))
}

pub fn clear(id: &str) -> Result<Value, String> {
    let path = session_file(id)?;
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({"deleted": path}))
}

/// Removes the activity log and heartbeats; leaves the Remote Script and
/// every client config alone.
pub fn delete_all(_app: &AppHandle) -> Result<Value, String> {
    let mut removed = 0;
    for dir in [
        state::activity_dir(),
        state::sessions_dir(),
        state::library_dir(),
        state::sets_dir(),
    ] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if std::fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    // The set exports and the library index are whole folders the server
    // recreates on demand; remove them so nothing of yours is left behind.
    for dir in [state::library_dir(), state::sets_dir()] {
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(json!({"removed": removed}))
}

/// Delete activity files older than the retention window.
pub fn prune(_app: &AppHandle, retention_days: u32) {
    let cutoff = SystemTime::now() - Duration::from_secs(u64::from(retention_days) * 86_400);
    if let Ok(entries) = std::fs::read_dir(state::activity_dir()) {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t < cutoff)
                .unwrap_or(false);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
