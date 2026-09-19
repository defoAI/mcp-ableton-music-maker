//! Records (S_t → action → S_{t+1}) trajectory steps around tool calls.
//! Active only when dataset recording is on.

use crate::connection::LiveState;
use crate::dataset::passive_poller::poller_is_running;
use crate::dataset::recorder::{get_recorder, SessionRecorder};
use crate::dataset::snapshot::{fetch_snapshot, scrub_name};
use crate::env_flag;
use regex::Regex;
use rmcp::{Peer, RoleServer};
use serde_json::{json, Map, Value};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Tools that change musical content: snapshot before and after. Transport
/// and view tools are recorded without pre/post hashes.
pub const MODIFYING_TOOLS: &[&str] = &[
    "create_midi_track",
    "create_audio_track",
    "set_track_name",
    "create_clip",
    "create_audio_clip",
    "add_notes_to_clip",
    "set_clip_name",
    "set_arrangement_clip_name",
    "set_tempo",
    "set_device_parameter",
    "load_instrument_or_effect",
    "load_drum_kit",
    "duplicate_to_arrangement",
];

const PARAM_KEYS: &[&str] = &[
    "track_index",
    "clip_index",
    "device_index",
    "index",
    "length",
    "time",
    "destination_time",
    "tempo",
    "uri",
    "rack_uri",
    "kit_path",
    "name",
    "path",
    "include_notes",
    "include_params",
    "rating",
    "tags",
    "note",
    "text",
    "level",
    "winner",
    "candidate_a",
    "candidate_b",
    "search_query",
    "kept",
    "category_type",
];

/// A `path` ending in one of these is a file on disk, not a browser category.
const AUDIO_EXTS: &[&str] = &[
    ".wav", ".aiff", ".aif", ".flac", ".mp3", ".ogg", ".m4a", ".wma", ".alc", ".asd",
];

fn light_snapshots() -> bool {
    env_flag("ABLETON_MCP_DATASET_LIGHT")
}

/// Kill switch: record actions but never snapshot state.
fn snapshots_disabled() -> bool {
    env_flag("ABLETON_MCP_DATASET_NO_SNAPSHOTS")
}

fn pre_cache_disabled() -> bool {
    env_flag("ABLETON_MCP_DATASET_NO_PRE_CACHE")
}

// Snapshots are the only dataset work on the tool's critical path — a
// synchronous round-trip to Live. Stop taking them if they turn slow or fail.
fn slow_snapshot_sec() -> f64 {
    crate::env_f64("ABLETON_MCP_SNAPSHOT_SLOW_SEC", 2.0)
}
const BREAKER_THRESHOLD: u32 = 3;
const BREAKER_COOLDOWN_SEC: f64 = 120.0;

struct Breaker {
    strikes: u32,
    open_until: f64,
}
static BREAKER: Mutex<Breaker> = Mutex::new(Breaker {
    strikes: 0,
    open_until: 0.0,
});

fn breaker_is_open() -> bool {
    crate::telemetry::now_secs() < BREAKER.lock().unwrap_or_else(|e| e.into_inner()).open_until
}

fn breaker_record(ok: bool, elapsed: f64) {
    let mut b = BREAKER.lock().unwrap_or_else(|e| e.into_inner());
    if ok && elapsed < slow_snapshot_sec() {
        b.strikes = 0;
        return;
    }
    b.strikes += 1;
    if b.strikes >= BREAKER_THRESHOLD {
        b.open_until = crate::telemetry::now_secs() + BREAKER_COOLDOWN_SEC;
        b.strikes = 0;
        tracing::warn!(
            "Trajectory snapshots paused for {:.0}s — snapshots were slow or failing. Tool calls are unaffected; state capture resumes after the cooldown.",
            BREAKER_COOLDOWN_SEC
        );
    }
}

/// True for a Live browser location, false for anything filesystem-shaped.
/// Every ambiguous case is treated as a filesystem path.
pub fn is_browser_path(value: &str) -> bool {
    static DRIVE: OnceLock<Regex> = OnceLock::new();
    if value.is_empty() || value.len() > 500 {
        return false;
    }
    if value.starts_with('/') || value.starts_with('~') || value.starts_with("\\\\") {
        return false;
    }
    if DRIVE
        .get_or_init(|| Regex::new(r"^[A-Za-z]:[\\/]").unwrap())
        .is_match(value)
    {
        return false;
    }
    if value.contains('\\') {
        return false;
    }
    let lower = value.to_lowercase();
    !AUDIO_EXTS.iter().any(|ext| lower.ends_with(ext))
}

fn file_extension(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn truncate_string_value(value: &Value) -> Value {
    match value {
        Value::String(s) if s.chars().count() > 500 => {
            json!(format!("{}...", s.chars().take(500).collect::<String>()))
        }
        other => other.clone(),
    }
}

/// The subset of a tool's arguments that goes into the action row.
pub fn extract_params(args: &Map<String, Value>) -> Map<String, Value> {
    let mut params = Map::new();
    for key in PARAM_KEYS {
        let Some(value) = args.get(*key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        if *key == "path" {
            if let Value::String(s) = value {
                if is_browser_path(s) {
                    params.insert(
                        "browser_path".into(),
                        json!(s.chars().take(500).collect::<String>()),
                    );
                } else {
                    params.insert("file_extension".into(), json!(file_extension(s)));
                    params.insert("has_path".into(), json!(true));
                }
            }
            continue;
        }
        if *key == "name" {
            params.insert("name".into(), scrub_name(value));
            continue;
        }
        params.insert((*key).to_string(), truncate_string_value(value));
    }
    if let Some(Value::Array(notes)) = args.get("notes") {
        params.insert("notes_count".into(), json!(notes.len()));
        let compact: Vec<Value> = notes
            .iter()
            .take(256)
            .filter_map(Value::as_object)
            .map(|n| {
                json!([
                    n.get("pitch").cloned().unwrap_or(json!(0)),
                    n.get("start_time").cloned().unwrap_or(json!(0)),
                    n.get("duration").cloned().unwrap_or(json!(0)),
                    n.get("velocity").cloned().unwrap_or(json!(100)),
                ])
            })
            .collect();
        params.insert("notes".into(), Value::Array(compact));
        if notes.len() > 256 {
            params.insert("notes_truncated".into(), json!(true));
        }
    }
    params
}

/// `get_recorder()` touches config/telemetry; never let that break a tool.
pub fn safe_get_recorder() -> Option<std::sync::Arc<SessionRecorder>> {
    get_recorder()
}

/// Returns (hash, snapshot) or (None, None) on failure. This is the only
/// dataset work that runs synchronously inside a tool call; it never fails
/// the call and is skipped while the breaker is open.
fn take_snapshot(live: &LiveState, recorder: &SessionRecorder) -> Option<String> {
    if snapshots_disabled() || breaker_is_open() {
        return None;
    }
    if !live.script.has_capability("get_session_snapshot") {
        tracing::debug!("Skipping trajectory snapshot — Remote Script lacks get_session_snapshot");
        return None;
    }
    let started = Instant::now();
    let light = light_snapshots();
    let snapshot = fetch_snapshot(live.bridge.as_ref(), !light, !light);
    let elapsed = started.elapsed().as_secs_f64();
    let Some(snapshot) = snapshot else {
        breaker_record(false, elapsed);
        return None;
    };
    breaker_record(true, elapsed);
    if elapsed >= slow_snapshot_sec() {
        tracing::debug!("Trajectory snapshot took {:.2}s", elapsed);
    }
    let (hash, _) = recorder.record_state(&snapshot);
    Some(hash)
}

/// True when human edits in Live would be noticed, which is what makes the
/// pre-hash cache sound.
fn passive_watch_active(live: &LiveState) -> bool {
    poller_is_running() && live.script.has_capability("drain_passive_events")
}

/// Pre-action state hash plus its provenance ("fresh" or "cached"). The
/// previous action's post-state is this action's pre-state unless Live
/// changed in between.
fn pre_snapshot(
    live: &LiveState,
    recorder: &SessionRecorder,
) -> (Option<String>, Option<&'static str>) {
    if !pre_cache_disabled() && passive_watch_active(live) {
        if let Some(cached) = recorder.take_cached_state_hash() {
            tracing::debug!("Reusing cached pre-snapshot hash={}", cached);
            return (Some(cached), Some("cached"));
        }
    }
    let hash = take_snapshot(live, recorder);
    let source = hash.as_ref().map(|_| "fresh");
    (hash, source)
}

pub struct StepContext {
    pre_hash: Option<String>,
    pre_hash_source: Option<&'static str>,
    start: Instant,
    modifying: bool,
}

/// Shared pre-call work: adopt the prompt as intent, take the pre-snapshot.
pub fn begin(
    live: &LiveState,
    recorder: &SessionRecorder,
    tool_name: &str,
    modifying: bool,
    args: &Map<String, Value>,
) -> StepContext {
    // submit_intent sets the intent itself, with an explicit level.
    if tool_name != "submit_intent" {
        if let Some(Value::String(prompt)) = args.get("user_prompt") {
            if !prompt.trim().is_empty() {
                recorder.observe_prompt(prompt);
            }
        }
    }
    let (pre_hash, pre_hash_source) = if modifying {
        pre_snapshot(live, recorder)
    } else {
        (None, None)
    };
    StepContext {
        pre_hash,
        pre_hash_source,
        start: Instant::now(),
        modifying,
    }
}

/// Shared post-call work. Never fails: recording must not break a tool.
pub fn finish(
    live: &LiveState,
    recorder: &SessionRecorder,
    ctx: StepContext,
    tool_name: &str,
    args: &Map<String, Value>,
    success: bool,
    error: Option<&str>,
) {
    let duration_ms = ctx.start.elapsed().as_secs_f64() * 1000.0;
    let post_hash = if ctx.modifying {
        take_snapshot(live, recorder)
    } else {
        None
    };
    recorder.record_action(
        tool_name,
        Some(extract_params(args)),
        ctx.pre_hash,
        post_hash,
        success,
        Some(duration_ms),
        error,
        None,
        ctx.pre_hash_source,
    );
}

/// Ask for consent through the client. True if the user actually answered
/// (or dismissed the dialog); false means fall back to the text notice.
pub async fn try_elicit(peer: &Peer<RoleServer>) -> bool {
    use crate::dataset::consent::{try_elicit_consent, UNKNOWN};
    match try_elicit_consent(peer).await {
        None => false,
        Some(UNKNOWN) => true,
        Some(_) => {
            crate::telemetry::refresh_consent_from_dataset();
            true
        }
    }
}

/// Append the one-time consent question to a tool result.
pub fn with_consent_notice(mut text: String) -> String {
    let notice = crate::dataset::consent::maybe_consent_notice();
    if !notice.is_empty() {
        text.push_str(notice);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_paths_vs_files() {
        assert!(is_browser_path("Drums/Kits/808 Core Kit"));
        assert!(!is_browser_path("/Users/nick/kick.wav"));
        assert!(!is_browser_path("C:\\samples\\kick.wav"));
        assert!(!is_browser_path("kick.wav"));
        assert!(!is_browser_path(""));
    }

    #[test]
    fn extracts_and_scrubs_params() {
        let args = json!({
            "track_index": 1, "name": "Secret title", "path": "/Users/x/a.wav",
            "notes": [{"pitch": 60, "start_time": 0.0, "duration": 1.0, "velocity": 100}],
            "user_prompt": "ignored", "ctx": null
        });
        let p = extract_params(args.as_object().unwrap());
        assert_eq!(p["track_index"], 1);
        assert_eq!(p["name"], "<name:12>");
        assert_eq!(p["file_extension"], ".wav");
        assert_eq!(p["has_path"], true);
        assert_eq!(p["notes_count"], 1);
        assert_eq!(p["notes"], json!([[60, 0.0, 1.0, 100]]));
        assert!(p.get("user_prompt").is_none());
        let browser = extract_params(json!({"path": "Drums/Kits"}).as_object().unwrap());
        assert_eq!(browser["browser_path"], "Drums/Kits");
    }
}
