//! The MCP server: every tool Claude can call, plus the wrapper that writes
//! one local activity line around each call.
//!
//! Tool bodies are plain functions `fn(&LiveState, &Params) -> ToolResult`
//! so tests can drive them with a fake bridge; the `#[tool]` methods only
//! bind a body to its [`ToolSpec`] and hand both to [`Server::run`].

use crate::connection::{self, LiveError, LiveState};
use crate::notes::NotesInput;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, InitializeRequestParams, InitializeResult};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Instant;

/// Every command this server sends to the Remote Script. A test checks each
/// one against the script's own capability list.
pub const ALL_REMOTE_COMMANDS: &[&str] = &[
    "get_session_info",
    "get_track_info",
    "get_script_info",
    "get_clip_notes",
    "get_device_parameters",
    "get_session_snapshot",
    "set_device_parameter",
    "drain_passive_events",
    "create_midi_track",
    "create_audio_track",
    "set_track_name",
    "create_clip",
    "create_audio_clip",
    "add_notes_to_clip",
    "clear_notes_from_clip",
    "set_clip_name",
    "set_arrangement_clip_name",
    "delete_clip",
    "set_tempo",
    "load_browser_item",
    "fire_clip",
    "stop_clip",
    "start_playback",
    "stop_playback",
    "get_browser_tree",
    "get_browser_items_at_path",
    "switch_to_arrangement_view",
    "set_current_song_time",
    "get_arrangement_clips",
    "duplicate_session_clip_to_arrangement",
    "create_locator",
];

pub type ToolResult = Result<String, String>;

/// The one thing the wrapper knows about a tool: its name, for the activity
/// line. Everything else (which commands it sent, how long Live took) is
/// observed while the body runs.
pub struct ToolSpec {
    pub name: &'static str,
}

impl ToolSpec {
    const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn get_display(v: &Value, key: &str, default: &str) -> String {
    v.get(key)
        .map(display)
        .unwrap_or_else(|| default.to_string())
}

fn join_names(v: Option<&Value>) -> String {
    v.and_then(Value::as_array)
        .map(|items| items.iter().map(display).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

// ── Parameter types ─────────────────────────────────────────────────────────

fn minus_one() -> i64 {
    -1
}
fn four() -> f64 {
    4.0
}
fn yes() -> bool {
    true
}
fn all() -> String {
    "all".to_string()
}

macro_rules! params {
    ($(#[$meta:meta])* $name:ident { $($(#[$fmeta:meta])* $field:ident : $ty:ty $(= $default:expr)?),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
        pub struct $name {
            $(
                $(#[$fmeta])*
                $(#[serde(default = $default)])?
                pub $field: $ty,
            )*
        }
    };
}

params!(Empty {});
params!(TrackParams {
    /// The index of the track
    track_index: i64,
});
params!(ClipParams {
    /// The index of the track containing the clip
    track_index: i64,
    /// The index of the clip slot containing the clip
    clip_index: i64,
});
params!(DeviceParams {
    /// Track that owns the device
    track_index: i64,
    /// Index into the track's device chain
    device_index: i64,
});
params!(SetDeviceParameterParams {
    /// Track that owns the device
    track_index: i64,
    /// Index into the track's device chain
    device_index: i64,
    /// Index into device.parameters
    parameter_index: i64,
    /// New parameter value (Live parameter units)
    value: f64,
});
params!(SnapshotParams {
    /// Include MIDI note arrays in clips (default true)
    include_notes: bool = "yes",
    /// Include device parameter values (default true)
    include_params: bool = "yes",
    /// Leave out empty clip slots and the scene list (default true). Each
    /// track still reports slot_count. Set false for the raw dump.
    compact: bool = "yes",
    /// Include the scene list even when compact (default false)
    include_scenes: bool = "bool::default",
});
params!(CreateTrackParams {
    /// The index to insert the track at (-1 = end of list)
    index: i64 = "minus_one",
});
params!(SetTrackNameParams {
    /// The index of the track to rename
    track_index: i64,
    /// The new name for the track
    name: String,
});
params!(CreateClipParams {
    /// The index of the track to create the clip in
    track_index: i64,
    /// The index of the clip slot to create the clip in
    clip_index: i64,
    /// The length of the clip in beats (default: 4.0)
    length: f64 = "four",
    /// Name for the new clip (optional; saves a set_clip_name call)
    name: String = "String::new",
    /// Notes to put in the clip right away, in any of the compact forms
    /// (optional; saves an add_notes_to_clip call)
    #[serde(flatten)]
    input: NotesInput,
});
params!(CreateAudioClipParams {
    /// The index of the audio track to create the clip in
    track_index: i64,
    /// The index of the clip slot to create the clip in
    clip_index: i64,
    /// Absolute path to a supported audio file (e.g. a .wav). The target
    /// track must be an audio track and the clip slot must be empty.
    path: String,
});

/// One MIDI note. Extra fields (note_id, probability, ...) are forwarded to
/// Live untouched, so notes read with get_clip_notes round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    /// MIDI pitch 0-127
    pub pitch: i64,
    /// Start position in beats
    pub start_time: f64,
    /// Length in beats
    pub duration: f64,
    /// Velocity 1-127
    pub velocity: i64,
    /// Optional; defaults to false. Leave it out unless the note is muted.
    #[serde(default)]
    pub mute: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

params!(AddNotesParams {
    /// The index of the track containing the clip
    track_index: i64,
    /// The index of the clip slot containing the clip
    clip_index: i64,
    /// Remove the clip's existing notes first, so this call replaces
    /// instead of appending (default false)
    clear: bool = "bool::default",
    /// The notes, in any mix of: notes (objects), notes_csv, steps, patterns;
    /// plus loop_every/until to tile them
    #[serde(flatten)]
    input: NotesInput,
});
params!(SetClipNameParams {
    /// The index of the track containing the clip
    track_index: i64,
    /// The index of the clip (Session slot, or position in track.arrangement_clips)
    clip_index: i64,
    /// The new name for the clip
    name: String,
});
params!(SetTempoParams {
    /// The new tempo in BPM
    tempo: f64,
});
params!(LoadInstrumentParams {
    /// The index of the track to load the instrument on
    track_index: i64,
    /// The URI of the instrument or effect to load (e.g. 'query:Synths#Instrument%20Rack:Bass:FileId_5116')
    uri: String,
});
params!(BrowserTreeParams {
    /// Type of categories to get ('all', 'instruments', 'sounds', 'drums', 'audio_effects', 'midi_effects')
    category_type: String = "all",
});
params!(BrowserPathParams {
    /// Path in the format "category/folder/subfolder" where category is one
    /// of the available browser categories in Ableton
    path: String,
});
params!(LoadDrumKitParams {
    /// The index of the track to load on
    track_index: i64,
    /// The URI of the drum rack to load (e.g. 'Drums/Drum Rack')
    rack_uri: String,
    /// Path to the drum kit inside the browser (e.g. 'drums/acoustic/kit1')
    kit_path: String,
});
params!(ArrangementTimeParams {
    /// Position in beats from the start of the arrangement (e.g. 8.0 = bar 3 in 4/4)
    time: f64,
});
params!(DuplicateToArrangementParams {
    /// Index of the track that owns the Session clip
    track_index: i64,
    /// Index of the clip slot in that track (Session view)
    clip_index: i64,
    /// One beat position to place the clip at (0.0 = start, 8.0 = bar 3 in 4/4)
    destination_time: Option<f64>,
    /// Several beat positions, placed in order: [32, 36, 40, 44]
    #[serde(default)]
    destination_times: Vec<f64>,
    /// With `end` and `step`: place at start, start+step, ... while below end
    /// (e.g. start 32, end 96, step 4 = 16 placements, one per bar)
    start: Option<f64>,
    /// End of the range (exclusive), in beats
    end: Option<f64>,
    /// Beats between placements in the range (usually the clip length)
    step: Option<f64>,
});
params!(CreateLocatorParams {
    /// The locator label (e.g. "Chorus", "Verse 1", "Drop")
    name: String,
    /// Beat position where the locator should sit
    time: f64,
});

// ── Tool specs ──────────────────────────────────────────────────────────────

pub const GET_SESSION_INFO: ToolSpec = ToolSpec::new("get_session_info");
pub const GET_REMOTE_SCRIPT_INFO: ToolSpec = ToolSpec::new("get_remote_script_info");
pub const GET_TRACK_INFO: ToolSpec = ToolSpec::new("get_track_info");
pub const GET_CLIP_NOTES: ToolSpec = ToolSpec::new("get_clip_notes");
pub const GET_DEVICE_PARAMETERS: ToolSpec = ToolSpec::new("get_device_parameters");
pub const SET_DEVICE_PARAMETER: ToolSpec = ToolSpec::new("set_device_parameter");
pub const GET_SESSION_SNAPSHOT: ToolSpec = ToolSpec::new("get_session_snapshot");
pub const CREATE_MIDI_TRACK: ToolSpec = ToolSpec::new("create_midi_track");
pub const CREATE_AUDIO_TRACK: ToolSpec = ToolSpec::new("create_audio_track");
pub const SET_TRACK_NAME: ToolSpec = ToolSpec::new("set_track_name");
pub const CREATE_CLIP: ToolSpec = ToolSpec::new("create_clip");
pub const CREATE_AUDIO_CLIP: ToolSpec = ToolSpec::new("create_audio_clip");
pub const ADD_NOTES_TO_CLIP: ToolSpec = ToolSpec::new("add_notes_to_clip");
pub const CLEAR_NOTES_FROM_CLIP: ToolSpec = ToolSpec::new("clear_notes_from_clip");
pub const SET_CLIP_NAME: ToolSpec = ToolSpec::new("set_clip_name");
pub const SET_ARRANGEMENT_CLIP_NAME: ToolSpec = ToolSpec::new("set_arrangement_clip_name");
pub const SET_TEMPO: ToolSpec = ToolSpec::new("set_tempo");
pub const LOAD_INSTRUMENT_OR_EFFECT: ToolSpec = ToolSpec::new("load_instrument_or_effect");
pub const FIRE_CLIP: ToolSpec = ToolSpec::new("fire_clip");
pub const STOP_CLIP: ToolSpec = ToolSpec::new("stop_clip");
pub const DELETE_CLIP: ToolSpec = ToolSpec::new("delete_clip");
pub const START_PLAYBACK: ToolSpec = ToolSpec::new("start_playback");
pub const STOP_PLAYBACK: ToolSpec = ToolSpec::new("stop_playback");
pub const GET_BROWSER_TREE: ToolSpec = ToolSpec::new("get_browser_tree");
pub const GET_BROWSER_ITEMS_AT_PATH: ToolSpec = ToolSpec::new("get_browser_items_at_path");
pub const LOAD_DRUM_KIT: ToolSpec = ToolSpec::new("load_drum_kit");
pub const SWITCH_TO_ARRANGEMENT_VIEW: ToolSpec = ToolSpec::new("switch_to_arrangement_view");
pub const SET_ARRANGEMENT_TIME: ToolSpec = ToolSpec::new("set_arrangement_time");
pub const GET_ARRANGEMENT_CLIPS: ToolSpec = ToolSpec::new("get_arrangement_clips");
pub const DUPLICATE_TO_ARRANGEMENT: ToolSpec = ToolSpec::new("duplicate_to_arrangement");
pub const CREATE_LOCATOR: ToolSpec = ToolSpec::new("create_locator");

// ── Tool bodies ─────────────────────────────────────────────────────────────

fn live_err(what: &str, e: LiveError) -> String {
    format!("Could not {what}: {e}")
}

/// Every tool checks that the loaded Remote Script serves the command it is
/// about to send; a stale or missing script gets a reinstall message rather
/// than a half-working session.
fn require(live: &LiveState, capability: &str) -> Result<(), String> {
    match live
        .script
        .require_capability(live.bridge.as_ref(), capability)
    {
        None => Ok(()),
        Some(msg) => Err(msg),
    }
}

pub fn get_session_info_body(live: &LiveState, _p: &Empty) -> ToolResult {
    live.send_command("get_session_info", None)
        .map(|r| pretty(&r))
        .map_err(|e| live_err("get session info", e))
}

pub fn get_remote_script_info_body(live: &LiveState, _p: &Empty) -> ToolResult {
    let info = live.script.handshake(live.bridge.as_ref());
    serde_json::to_string_pretty(&info).map_err(|e| format!("Could not serialise script info: {e}"))
}

pub fn get_track_info_body(live: &LiveState, p: &TrackParams) -> ToolResult {
    require(live, "get_track_info")?;
    live.send_command(
        "get_track_info",
        Some(json!({"track_index": p.track_index})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("get track info", e))
}

pub fn get_clip_notes_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "get_clip_notes")?;
    live.send_command(
        "get_clip_notes",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("get clip notes", e))
}

pub fn get_device_parameters_body(live: &LiveState, p: &DeviceParams) -> ToolResult {
    require(live, "get_device_parameters")?;
    live.send_command(
        "get_device_parameters",
        Some(json!({"track_index": p.track_index, "device_index": p.device_index})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("get device parameters", e))
}

pub fn set_device_parameter_body(live: &LiveState, p: &SetDeviceParameterParams) -> ToolResult {
    require(live, "set_device_parameter")?;
    let r = live
        .send_command(
            "set_device_parameter",
            Some(json!({
                "track_index": p.track_index,
                "device_index": p.device_index,
                "parameter_index": p.parameter_index,
                "value": p.value,
            })),
        )
        .map_err(|e| live_err("set device parameter", e))?;
    Ok(format!(
        "Set {} {} → {}",
        get_display(&r, "name", "parameter"),
        get_display(&r, "old_value", "?"),
        get_display(&r, "value", "?")
    ))
}

pub fn get_session_snapshot_body(live: &LiveState, p: &SnapshotParams) -> ToolResult {
    require(live, "get_session_snapshot")?;
    let mut r = live
        .send_command(
            "get_session_snapshot",
            Some(json!({"include_notes": p.include_notes, "include_params": p.include_params})),
        )
        .map_err(|e| live_err("get session snapshot", e))?;
    if p.compact {
        compact_snapshot(&mut r, p.include_scenes);
    }
    Ok(pretty(&r))
}

/// Drop what carries no information: empty clip slots (the count stays)
/// and, unless asked, the scene list. The rest of the dump is untouched.
pub fn compact_snapshot(snapshot: &mut Value, include_scenes: bool) {
    let Some(obj) = snapshot.as_object_mut() else {
        return;
    };
    if let Some(tracks) = obj.get_mut("tracks").and_then(Value::as_array_mut) {
        for track in tracks.iter_mut().filter_map(Value::as_object_mut) {
            if let Some(Value::Array(slots)) = track.get("clip_slots").cloned() {
                let kept: Vec<Value> = slots
                    .iter()
                    .filter(|s| s.get("has_clip").and_then(Value::as_bool).unwrap_or(true))
                    .cloned()
                    .collect();
                track.insert("slot_count".into(), json!(slots.len()));
                track.insert("clip_slots".into(), Value::Array(kept));
            }
            if let Some(Value::Array(clips)) = track.get("arrangement_clips") {
                if clips.is_empty() {
                    track.remove("arrangement_clips");
                }
            }
        }
    }
    if !include_scenes {
        if let Some(Value::Array(scenes)) = obj.get("scenes") {
            let n = scenes.len();
            obj.remove("scenes");
            obj.insert("scene_count".into(), json!(n));
        }
    }
    obj.insert("compact".into(), json!(true));
}

pub fn create_midi_track_body(live: &LiveState, p: &CreateTrackParams) -> ToolResult {
    require(live, "create_midi_track")?;
    let r = live
        .send_command("create_midi_track", Some(json!({"index": p.index})))
        .map_err(|e| live_err("create MIDI track", e))?;
    Ok(created_track_text("MIDI", &r))
}

/// "Created MIDI track 6 ('6-MIDI')" — the index is what the next call needs.
fn created_track_text(kind: &str, r: &Value) -> String {
    match r.get("index").and_then(Value::as_i64) {
        Some(index) => format!(
            "Created {kind} track {index} ('{}'). Use track_index {index} for it; tracks that were at {index} or later moved up by one.",
            get_display(r, "name", "unnamed")
        ),
        None => format!("Created {kind} track '{}'", get_display(r, "name", "unnamed")),
    }
}

pub fn create_audio_track_body(live: &LiveState, p: &CreateTrackParams) -> ToolResult {
    require(live, "create_audio_track")?;
    let r = live
        .send_command("create_audio_track", Some(json!({"index": p.index})))
        .map_err(|e| live_err("create audio track", e))?;
    Ok(created_track_text("audio", &r))
}

pub fn set_track_name_body(live: &LiveState, p: &SetTrackNameParams) -> ToolResult {
    require(live, "set_track_name")?;
    let r = live
        .send_command(
            "set_track_name",
            Some(json!({"track_index": p.track_index, "name": p.name})),
        )
        .map_err(|e| live_err("set track name", e))?;
    Ok(format!(
        "Renamed track to: {}",
        get_display(&r, "name", &p.name)
    ))
}

pub fn create_clip_body(live: &LiveState, p: &CreateClipParams) -> ToolResult {
    require(live, "create_clip")?;
    // Expand the notes before touching Live, so a bad pattern creates nothing.
    let notes = if p.input.is_empty() {
        Vec::new()
    } else {
        require(live, "add_notes_to_clip")?;
        crate::notes::expand(&p.input)?
    };
    if !p.name.is_empty() {
        require(live, "set_clip_name")?;
    }
    live.send_command(
        "create_clip",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "length": p.length})),
    )
    .map_err(|e| live_err("create clip", e))?;
    let mut text = format!(
        "Created clip at track {}, slot {} with length {} beats",
        p.track_index, p.clip_index, p.length
    );
    if !p.name.is_empty() {
        live.send_command(
            "set_clip_name",
            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "name": p.name})),
        )
        .map_err(|e| live_err("name the new clip", e))?;
        text.push_str(&format!(", named '{}'", p.name));
    }
    if !notes.is_empty() {
        live.send_command(
            "add_notes_to_clip",
            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "notes": notes})),
        )
        .map_err(|e| live_err("add notes to the new clip", e))?;
        text.push_str(&format!(", with {} notes", notes.len()));
    }
    Ok(text)
}

pub fn create_audio_clip_body(live: &LiveState, p: &CreateAudioClipParams) -> ToolResult {
    require(live, "create_audio_clip")?;
    let r = live
        .send_command(
            "create_audio_clip",
            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "path": p.path})),
        )
        .map_err(|e| live_err("create audio clip", e))?;
    Ok(format!(
        "Created audio clip '{}' at track {}, slot {} (length {} beats)",
        get_display(&r, "name", "clip"),
        p.track_index,
        p.clip_index,
        get_display(&r, "length", "?")
    ))
}

pub fn add_notes_to_clip_body(live: &LiveState, p: &AddNotesParams) -> ToolResult {
    require(live, "add_notes_to_clip")?;
    let notes = crate::notes::expand(&p.input)?;
    if notes.is_empty() && !p.clear {
        return Err("No notes given. Use notes, notes_csv, steps or patterns (or clear: true to empty the clip).".into());
    }
    let mut cleared = String::new();
    if p.clear {
        require(live, "clear_notes_from_clip")?;
        let r = live
            .send_command(
                "clear_notes_from_clip",
                Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
            )
            .map_err(|e| live_err("clear notes from clip", e))?;
        cleared = format!(
            " (cleared {} first)",
            get_display(&r, "cleared_count", "the old notes")
        );
    }
    if notes.is_empty() {
        return Ok(format!(
            "Cleared clip at track {}, slot {}{}",
            p.track_index, p.clip_index, cleared
        ));
    }
    live.send_command(
        "add_notes_to_clip",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "notes": notes})),
    )
    .map_err(|e| live_err("add notes to clip", e))?;
    let last = notes
        .iter()
        .map(|n| n.start_time + n.duration)
        .fold(0.0_f64, f64::max);
    Ok(format!(
        "Added {} notes to clip at track {}, slot {}{} — last note ends at beat {}",
        notes.len(),
        p.track_index,
        p.clip_index,
        cleared,
        last
    ))
}

pub fn clear_notes_from_clip_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "clear_notes_from_clip")?;
    let r = live
        .send_command(
            "clear_notes_from_clip",
            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
        )
        .map_err(|e| live_err("clear notes from clip", e))?;
    Ok(format!(
        "Cleared {} note(s) from clip '{}' (track {}, slot {})",
        get_display(&r, "cleared_count", "?"),
        get_display(&r, "clip_name", "clip"),
        p.track_index,
        p.clip_index
    ))
}

pub fn set_clip_name_body(live: &LiveState, p: &SetClipNameParams) -> ToolResult {
    require(live, "set_clip_name")?;
    live.send_command(
        "set_clip_name",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "name": p.name})),
    )
    .map_err(|e| live_err("set clip name", e))?;
    Ok(format!(
        "Renamed clip at track {}, slot {} to '{}'",
        p.track_index, p.clip_index, p.name
    ))
}

pub fn set_arrangement_clip_name_body(live: &LiveState, p: &SetClipNameParams) -> ToolResult {
    require(live, "set_arrangement_clip_name")?;
    live.send_command(
        "set_arrangement_clip_name",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "name": p.name})),
    )
    .map_err(|e| live_err("set arrangement clip name", e))?;
    Ok(format!(
        "Renamed arrangement clip at track {}, index {} to '{}'",
        p.track_index, p.clip_index, p.name
    ))
}

pub fn set_tempo_body(live: &LiveState, p: &SetTempoParams) -> ToolResult {
    require(live, "set_tempo")?;
    live.send_command("set_tempo", Some(json!({"tempo": p.tempo})))
        .map_err(|e| live_err("set tempo", e))?;
    Ok(format!("Set tempo to {} BPM", p.tempo))
}

pub fn load_instrument_or_effect_body(live: &LiveState, p: &LoadInstrumentParams) -> ToolResult {
    require(live, "load_browser_item")?;
    let r = live
        .send_command(
            "load_browser_item",
            Some(json!({"track_index": p.track_index, "item_uri": p.uri})),
        )
        .map_err(|e| live_err("load instrument by URI", e))?;
    if !r.get("loaded").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("Failed to load instrument with URI '{}'", p.uri));
    }
    let new_devices = r.get("new_devices").and_then(Value::as_array);
    if new_devices.is_some_and(|d| !d.is_empty()) {
        Ok(format!(
            "Loaded instrument with URI '{}' on track {}. New devices: {}",
            p.uri,
            p.track_index,
            join_names(r.get("new_devices"))
        ))
    } else {
        Ok(format!(
            "Loaded instrument with URI '{}' on track {}. Devices on track: {}",
            p.uri,
            p.track_index,
            join_names(r.get("devices_after"))
        ))
    }
}

pub fn fire_clip_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "fire_clip")?;
    live.send_command(
        "fire_clip",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
    )
    .map_err(|e| live_err("fire clip", e))?;
    Ok(format!(
        "Started playing clip at track {}, slot {}",
        p.track_index, p.clip_index
    ))
}

pub fn stop_clip_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "stop_clip")?;
    live.send_command(
        "stop_clip",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
    )
    .map_err(|e| live_err("stop clip", e))?;
    Ok(format!(
        "Stopped clip at track {}, slot {}",
        p.track_index, p.clip_index
    ))
}

pub fn delete_clip_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "delete_clip")?;
    live.send_command(
        "delete_clip",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("delete clip", e))
}

pub fn start_playback_body(live: &LiveState, _p: &Empty) -> ToolResult {
    live.send_command("start_playback", None)
        .map_err(|e| live_err("start playback", e))?;
    Ok("Started playback".to_string())
}

pub fn stop_playback_body(live: &LiveState, _p: &Empty) -> ToolResult {
    live.send_command("stop_playback", None)
        .map_err(|e| live_err("stop playback", e))?;
    Ok("Stopped playback".to_string())
}

fn browser_error(what: &str, e: LiveError) -> String {
    let msg = e.to_string();
    if msg.contains("Browser is not available") {
        return "The Ableton browser is not available. Make sure Ableton Live is fully loaded and try again.".to_string();
    }
    if msg.contains("Could not access Live application") {
        return "Could not access the Ableton Live application. Make sure Ableton Live is running and the Remote Script is loaded.".to_string();
    }
    if msg.contains("Unknown or unavailable category") {
        return format!("{msg}. Check the available categories with get_browser_tree.");
    }
    if msg.contains("Path part") && msg.contains("not found") {
        return format!("{msg}. Check the path and try again.");
    }
    format!("Could not {what}: {msg}")
}

fn format_tree(item: &Value, indent: usize, out: &mut String) {
    let Some(obj) = item.as_object() else { return };
    let prefix = "  ".repeat(indent);
    out.push_str(&prefix);
    out.push_str("• ");
    out.push_str(
        &obj.get("name")
            .map(display)
            .unwrap_or_else(|| "Unknown".to_string()),
    );
    if let Some(path) = obj
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
    {
        out.push_str(&format!(" (path: {path})"));
    }
    if obj
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        out.push_str(" [...]");
    }
    out.push('\n');
    if let Some(children) = obj.get("children").and_then(Value::as_array) {
        for child in children {
            format_tree(child, indent + 1, out);
        }
    }
}

pub fn get_browser_tree_body(live: &LiveState, p: &BrowserTreeParams) -> ToolResult {
    require(live, "get_browser_tree")?;
    let r = live
        .send_command(
            "get_browser_tree",
            Some(json!({"category_type": p.category_type})),
        )
        .map_err(|e| browser_error("get browser tree", e))?;
    let categories = r
        .get("categories")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if r.get("available_categories").is_some() && categories.is_empty() {
        return Ok(format!(
            "No categories found for '{}'. Available browser categories: {}",
            p.category_type,
            join_names(r.get("available_categories"))
        ));
    }
    let mut out = format!(
        "Browser tree for '{}' (showing {} folders):\n\n",
        p.category_type,
        get_display(&r, "total_folders", "0")
    );
    for category in &categories {
        format_tree(category, 0, &mut out);
        out.push('\n');
    }
    Ok(out)
}

pub fn get_browser_items_at_path_body(live: &LiveState, p: &BrowserPathParams) -> ToolResult {
    require(live, "get_browser_items_at_path")?;
    let r = live
        .send_command("get_browser_items_at_path", Some(json!({"path": p.path})))
        .map_err(|e| browser_error("get browser items at path", e))?;
    if let (Some(error), Some(available)) = (r.get("error"), r.get("available_categories")) {
        return Err(format!(
            "{}\nAvailable browser categories: {}",
            display(error),
            join_names(Some(available))
        ));
    }
    Ok(pretty(&r))
}

pub fn load_drum_kit_body(live: &LiveState, p: &LoadDrumKitParams) -> ToolResult {
    require(live, "load_browser_item")?;
    require(live, "get_browser_items_at_path")?;
    let rack = live
        .send_command(
            "load_browser_item",
            Some(json!({"track_index": p.track_index, "item_uri": p.rack_uri})),
        )
        .map_err(|e| live_err("load drum kit", e))?;
    if !rack.get("loaded").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!(
            "Failed to load drum rack with URI '{}'",
            p.rack_uri
        ));
    }
    let kit_result = live
        .send_command(
            "get_browser_items_at_path",
            Some(json!({"path": p.kit_path})),
        )
        .map_err(|e| live_err("load drum kit", e))?;
    if let Some(error) = kit_result.get("error") {
        return Err(format!(
            "Loaded drum rack but failed to find drum kit: {}",
            display(error)
        ));
    }
    let items = kit_result
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(kit) = items.iter().find(|item| {
        item.get("is_loadable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }) else {
        return Err(format!(
            "Loaded drum rack but no loadable drum kits found at '{}'",
            p.kit_path
        ));
    };
    let kit_uri = kit.get("uri").cloned().unwrap_or(Value::Null);
    live.send_command(
        "load_browser_item",
        Some(json!({"track_index": p.track_index, "item_uri": kit_uri})),
    )
    .map_err(|e| live_err("load drum kit", e))?;
    Ok(format!(
        "Loaded drum rack and kit '{}' on track {}",
        get_display(kit, "name", "kit"),
        p.track_index
    ))
}

pub fn switch_to_arrangement_view_body(live: &LiveState, _p: &Empty) -> ToolResult {
    live.send_command("switch_to_arrangement_view", None)
        .map_err(|e| live_err("switch to arrangement view", e))?;
    Ok("Switched to Arrangement view".to_string())
}

pub fn set_arrangement_time_body(live: &LiveState, p: &ArrangementTimeParams) -> ToolResult {
    require(live, "set_current_song_time")?;
    let r = live
        .send_command("set_current_song_time", Some(json!({"time": p.time})))
        .map_err(|e| live_err("set arrangement time", e))?;
    Ok(format!(
        "Playhead moved to beat {}",
        r.get("current_song_time")
            .map(display)
            .unwrap_or_else(|| p.time.to_string())
    ))
}

pub fn get_arrangement_clips_body(live: &LiveState, p: &TrackParams) -> ToolResult {
    require(live, "get_arrangement_clips")?;
    live.send_command(
        "get_arrangement_clips",
        Some(json!({"track_index": p.track_index})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("get arrangement clips", e))
}

/// Every placement the caller asked for, in order.
pub fn placement_times(p: &DuplicateToArrangementParams) -> Result<Vec<f64>, String> {
    let mut times: Vec<f64> = Vec::new();
    times.extend(p.destination_time);
    times.extend(p.destination_times.iter().copied());
    match (p.start, p.end, p.step) {
        (None, None, None) => {}
        (Some(start), Some(end), Some(step)) => {
            if step <= 0.0 {
                return Err("step must be greater than 0".into());
            }
            if end <= start {
                return Err("end must be after start".into());
            }
            let mut t = start;
            while t < end - 1e-9 {
                times.push(t);
                t += step;
            }
        }
        _ => return Err("start, end and step go together; give all three".into()),
    }
    if times.is_empty() {
        return Err(
            "Give destination_time, destination_times, or start/end/step to say where the clip goes."
                .into(),
        );
    }
    if times.len() > 512 {
        return Err(format!(
            "{} placements is too many for one call (limit 512)",
            times.len()
        ));
    }
    Ok(times)
}

pub fn duplicate_to_arrangement_body(
    live: &LiveState,
    p: &DuplicateToArrangementParams,
) -> ToolResult {
    require(live, "duplicate_session_clip_to_arrangement")?;
    let times = placement_times(p)?;
    let mut placed: Vec<f64> = Vec::new();
    let mut clip_name = String::from("clip");
    let mut track_name = format!("track {}", p.track_index);
    for t in &times {
        let r = live
            .send_command(
                "duplicate_session_clip_to_arrangement",
                Some(json!({
                    "track_index": p.track_index,
                    "clip_index": p.clip_index,
                    "destination_time": t,
                })),
            )
            .map_err(|e| {
                if placed.is_empty() {
                    live_err("duplicate clip to arrangement", e)
                } else {
                    format!(
                        "Placed {} of {} (at beats {}), then could not place at beat {t}: {e}",
                        placed.len(),
                        times.len(),
                        list_beats(&placed),
                    )
                }
            })?;
        clip_name = get_display(&r, "clip_name", &clip_name);
        track_name = get_display(&r, "track_name", &track_name);
        placed.push(*t);
    }
    if placed.len() == 1 {
        Ok(format!(
            "Duplicated '{clip_name}' from Session slot {} on '{track_name}' to arrangement at beat {}",
            p.clip_index, placed[0]
        ))
    } else {
        Ok(format!(
            "Placed '{clip_name}' from Session slot {} on '{track_name}' {} times in the arrangement, at beats {}",
            p.clip_index,
            placed.len(),
            list_beats(&placed)
        ))
    }
}

fn list_beats(times: &[f64]) -> String {
    if times.len() <= 8 {
        times
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        format!(
            "{}, {}, {} … {} (every {} beats)",
            times[0],
            times[1],
            times[2],
            times[times.len() - 1],
            times[1] - times[0]
        )
    }
}

pub fn create_locator_body(live: &LiveState, p: &CreateLocatorParams) -> ToolResult {
    require(live, "create_locator")?;
    let r = live
        .send_command(
            "create_locator",
            Some(json!({"name": p.name, "time": p.time})),
        )
        .map_err(|e| live_err("create locator", e))?;
    Ok(format!(
        "Locator '{}' set at beat {}",
        get_display(&r, "name", &p.name),
        r.get("time")
            .map(display)
            .unwrap_or_else(|| p.time.to_string())
    ))
}

// ── Server ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Server {
    live: Arc<LiveState>,
    tool_router: ToolRouter<Self>,
}

fn run_blocking<P: Serialize>(
    live: &LiveState,
    spec: &ToolSpec,
    params: &P,
    body: fn(&LiveState, &P) -> ToolResult,
) -> ToolResult {
    let start = Instant::now();
    connection::begin_trace();
    let result = body(live, params);
    let trace = connection::end_trace();
    live.activity
        .record(spec.name, params, &result, start.elapsed(), trace);
    crate::app::refresh_heartbeat(live);
    result
}

impl Server {
    pub fn new(live: Arc<LiveState>) -> Self {
        Self {
            live,
            tool_router: Self::tool_router(),
        }
    }

    pub fn live(&self) -> &Arc<LiveState> {
        &self.live
    }

    /// Every tool this server serves, as the client will list them.
    pub fn tool_list(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    /// Execute a tool body and write its activity line. The body runs on the
    /// blocking pool because the Live socket is synchronous. A body's `Err`
    /// is an error *result*, never a protocol error.
    pub async fn run<P>(
        &self,
        spec: &'static ToolSpec,
        params: P,
        body: fn(&LiveState, &P) -> ToolResult,
    ) -> CallToolResult
    where
        P: Serialize + Send + 'static,
    {
        let live = self.live.clone();
        let result = tokio::task::spawn_blocking(move || run_blocking(&live, spec, &params, body))
            .await
            .unwrap_or_else(|e| Err(format!("{} failed unexpectedly: {e}", spec.name)));
        match result {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
    }
}

#[tool_router]
impl Server {
    /// Get detailed information about the current Ableton session
    #[tool(name = "get_session_info")]
    async fn get_session_info(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&GET_SESSION_INFO, p, get_session_info_body).await
    }

    /// Report Ableton Remote Script version and capabilities (handshake).
    /// Use this to verify the Live-side bridge matches this server.
    #[tool(name = "get_remote_script_info")]
    async fn get_remote_script_info(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&GET_REMOTE_SCRIPT_INFO, p, get_remote_script_info_body)
            .await
    }

    /// Get detailed information about a specific track in Ableton.
    #[tool(name = "get_track_info")]
    async fn get_track_info(&self, Parameters(p): Parameters<TrackParams>) -> CallToolResult {
        self.run(&GET_TRACK_INFO, p, get_track_info_body).await
    }

    /// Read all MIDI notes from a Session-view clip. Returns pitch,
    /// start_time, duration, velocity, mute (and extended fields when
    /// available).
    #[tool(name = "get_clip_notes")]
    async fn get_clip_notes(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&GET_CLIP_NOTES, p, get_clip_notes_body).await
    }

    /// Read all parameters for a device on a track (name, value, min, max).
    #[tool(name = "get_device_parameters")]
    async fn get_device_parameters(
        &self,
        Parameters(p): Parameters<DeviceParams>,
    ) -> CallToolResult {
        self.run(&GET_DEVICE_PARAMETERS, p, get_device_parameters_body)
            .await
    }

    /// Set a device parameter to a specific value. Use get_device_parameters
    /// first to discover parameter indices and ranges.
    #[tool(name = "set_device_parameter")]
    async fn set_device_parameter(
        &self,
        Parameters(p): Parameters<SetDeviceParameterParams>,
    ) -> CallToolResult {
        self.run(&SET_DEVICE_PARAMETER, p, set_device_parameter_body)
            .await
    }

    /// Capture a project state snapshot: session metadata, every track
    /// (mixer, devices, session clips, arrangement clips), optional MIDI
    /// notes, and optional device parameters. Compact by default: empty clip
    /// slots and the scene list are left out (counts stay); pass
    /// `compact: false` for the raw dump, `include_notes: false` and
    /// `include_params: false` to shrink it further.
    #[tool(name = "get_session_snapshot")]
    async fn get_session_snapshot(
        &self,
        Parameters(p): Parameters<SnapshotParams>,
    ) -> CallToolResult {
        self.run(&GET_SESSION_SNAPSHOT, p, get_session_snapshot_body)
            .await
    }

    /// Create a new MIDI track in the Ableton session.
    #[tool(name = "create_midi_track")]
    async fn create_midi_track(
        &self,
        Parameters(p): Parameters<CreateTrackParams>,
    ) -> CallToolResult {
        self.run(&CREATE_MIDI_TRACK, p, create_midi_track_body)
            .await
    }

    /// Create a new audio track in the Ableton session. Use this for
    /// recorded or imported audio (samples, stems, vocals). For MIDI
    /// instruments use create_midi_track instead.
    #[tool(name = "create_audio_track")]
    async fn create_audio_track(
        &self,
        Parameters(p): Parameters<CreateTrackParams>,
    ) -> CallToolResult {
        self.run(&CREATE_AUDIO_TRACK, p, create_audio_track_body)
            .await
    }

    /// Set the name of a track.
    #[tool(name = "set_track_name")]
    async fn set_track_name(
        &self,
        Parameters(p): Parameters<SetTrackNameParams>,
    ) -> CallToolResult {
        self.run(&SET_TRACK_NAME, p, set_track_name_body).await
    }

    /// Create a MIDI clip in a track's clip slot — and, in the same call,
    /// name it and fill it with notes. Give `name` and any of the note forms
    /// (`steps`, `notes_csv`, `patterns`, `notes`, with `loop_every`/`until`
    /// to tile a bar across the clip) instead of calling set_clip_name and
    /// add_notes_to_clip afterwards. The clip length is `length` beats.
    #[tool(name = "create_clip")]
    async fn create_clip(&self, Parameters(p): Parameters<CreateClipParams>) -> CallToolResult {
        self.run(&CREATE_CLIP, p, create_clip_body).await
    }

    /// Create a new audio clip in an audio track's clip slot by importing a
    /// file. Requires Ableton Live 12.0.5 or newer.
    #[tool(name = "create_audio_clip")]
    async fn create_audio_clip(
        &self,
        Parameters(p): Parameters<CreateAudioClipParams>,
    ) -> CallToolResult {
        self.run(&CREATE_AUDIO_CLIP, p, create_audio_clip_body)
            .await
    }

    /// Add MIDI notes to a Session clip. Write them compactly: `steps` is a
    /// step-sequencer string per pitch (`{"36": "x...x...x...x...", "42":
    /// "..x...x...x...x."}`, one character per `step` beat, X = accent, 1-9 =
    /// velocity level, _ = hold), `patterns` repeats a note (`{"pitch": 42,
    /// "every": 0.5, "offset": 0.25, "count": 32}`), `notes_csv` is
    /// `pitch,start,duration,velocity` per line, and `loop_every` + `until`
    /// tile one bar across a long clip (e.g. loop_every 4, until 64). Full
    /// `notes` objects still work. Pitches may be names (C1, F#2). Set
    /// `clear: true` to replace the clip's notes instead of appending.
    #[tool(name = "add_notes_to_clip")]
    async fn add_notes_to_clip(&self, Parameters(p): Parameters<AddNotesParams>) -> CallToolResult {
        self.run(&ADD_NOTES_TO_CLIP, p, add_notes_to_clip_body)
            .await
    }

    /// Remove all MIDI notes from a Session clip. Writes are additive
    /// (add_notes_to_clip only appends), so to truly modify a clip: read the
    /// notes with get_clip_notes, edit the list, clear_notes_from_clip, then
    /// add_notes_to_clip the edited notes.
    #[tool(name = "clear_notes_from_clip")]
    async fn clear_notes_from_clip(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&CLEAR_NOTES_FROM_CLIP, p, clear_notes_from_clip_body)
            .await
    }

    /// Set the name of a Session clip.
    #[tool(name = "set_clip_name")]
    async fn set_clip_name(&self, Parameters(p): Parameters<SetClipNameParams>) -> CallToolResult {
        self.run(&SET_CLIP_NAME, p, set_clip_name_body).await
    }

    /// Set the name of a clip placed in the Arrangement timeline. clip_index
    /// is the position in track.arrangement_clips, in the same order
    /// returned by get_arrangement_clips (ordered by start_time).
    #[tool(name = "set_arrangement_clip_name")]
    async fn set_arrangement_clip_name(
        &self,
        Parameters(p): Parameters<SetClipNameParams>,
    ) -> CallToolResult {
        self.run(
            &SET_ARRANGEMENT_CLIP_NAME,
            p,
            set_arrangement_clip_name_body,
        )
        .await
    }

    /// Set the tempo of the Ableton session.
    #[tool(name = "set_tempo")]
    async fn set_tempo(&self, Parameters(p): Parameters<SetTempoParams>) -> CallToolResult {
        self.run(&SET_TEMPO, p, set_tempo_body).await
    }

    /// Load an instrument or effect onto a track using its browser URI.
    #[tool(name = "load_instrument_or_effect")]
    async fn load_instrument_or_effect(
        &self,
        Parameters(p): Parameters<LoadInstrumentParams>,
    ) -> CallToolResult {
        self.run(
            &LOAD_INSTRUMENT_OR_EFFECT,
            p,
            load_instrument_or_effect_body,
        )
        .await
    }

    /// Start playing a clip.
    #[tool(name = "fire_clip")]
    async fn fire_clip(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&FIRE_CLIP, p, fire_clip_body).await
    }

    /// Stop playing a clip.
    #[tool(name = "stop_clip")]
    async fn stop_clip(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&STOP_CLIP, p, stop_clip_body).await
    }

    /// Delete the clip in the given clip slot, freeing it for reuse. Use
    /// this before create_clip when you want to overwrite an existing clip
    /// (create_clip refuses to write into an occupied slot).
    #[tool(name = "delete_clip")]
    async fn delete_clip(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&DELETE_CLIP, p, delete_clip_body).await
    }

    /// Start playing the Ableton session.
    #[tool(name = "start_playback")]
    async fn start_playback(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&START_PLAYBACK, p, start_playback_body).await
    }

    /// Stop playing the Ableton session.
    #[tool(name = "stop_playback")]
    async fn stop_playback(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&STOP_PLAYBACK, p, stop_playback_body).await
    }

    /// Get a hierarchical tree of browser categories from Ableton.
    #[tool(name = "get_browser_tree")]
    async fn get_browser_tree(
        &self,
        Parameters(p): Parameters<BrowserTreeParams>,
    ) -> CallToolResult {
        self.run(&GET_BROWSER_TREE, p, get_browser_tree_body).await
    }

    /// Get browser items at a specific path in Ableton's browser.
    #[tool(name = "get_browser_items_at_path")]
    async fn get_browser_items_at_path(
        &self,
        Parameters(p): Parameters<BrowserPathParams>,
    ) -> CallToolResult {
        self.run(
            &GET_BROWSER_ITEMS_AT_PATH,
            p,
            get_browser_items_at_path_body,
        )
        .await
    }

    /// Load a drum rack and then load a specific drum kit into it.
    #[tool(name = "load_drum_kit")]
    async fn load_drum_kit(&self, Parameters(p): Parameters<LoadDrumKitParams>) -> CallToolResult {
        self.run(&LOAD_DRUM_KIT, p, load_drum_kit_body).await
    }

    /// Switch Ableton's main window to the Arrangement view.
    #[tool(name = "switch_to_arrangement_view")]
    async fn switch_to_arrangement_view(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(
            &SWITCH_TO_ARRANGEMENT_VIEW,
            p,
            switch_to_arrangement_view_body,
        )
        .await
    }

    /// Move the arrangement playhead to a specific position.
    #[tool(name = "set_arrangement_time")]
    async fn set_arrangement_time(
        &self,
        Parameters(p): Parameters<ArrangementTimeParams>,
    ) -> CallToolResult {
        self.run(&SET_ARRANGEMENT_TIME, p, set_arrangement_time_body)
            .await
    }

    /// List all clips placed in the Arrangement timeline for a track:
    /// name, start_time, end_time, length, and type.
    #[tool(name = "get_arrangement_clips")]
    async fn get_arrangement_clips(
        &self,
        Parameters(p): Parameters<TrackParams>,
    ) -> CallToolResult {
        self.run(&GET_ARRANGEMENT_CLIPS, p, get_arrangement_clips_body)
            .await
    }

    /// Copy a Session-view clip into the Arrangement timeline on the same
    /// track — at one beat position (`destination_time`), at several
    /// (`destination_times`), or across a range (`start`, `end`, `step`:
    /// start 32, end 96, step 4 places it on every bar from bar 9 to bar
    /// 24). One call per section, not one per bar. Placements happen in
    /// order and stop at the first failure, which the result reports.
    #[tool(name = "duplicate_to_arrangement")]
    async fn duplicate_to_arrangement(
        &self,
        Parameters(p): Parameters<DuplicateToArrangementParams>,
    ) -> CallToolResult {
        self.run(&DUPLICATE_TO_ARRANGEMENT, p, duplicate_to_arrangement_body)
            .await
    }

    /// Create a named locator (cue point) in the Arrangement at a beat
    /// position. A locator already at that beat is renamed instead.
    #[tool(name = "create_locator")]
    async fn create_locator(
        &self,
        Parameters(p): Parameters<CreateLocatorParams>,
    ) -> CallToolResult {
        self.run(&CREATE_LOCATOR, p, create_locator_body).await
    }
}

#[tool_handler(router = self.tool_router, name = "AbletonMusicMaker")]
impl ServerHandler for Server {
    /// The one place the server learns who started it: the client's own
    /// name and version from `initialize`. Recorded in the heartbeat file the
    /// Mac app reads; never sent anywhere.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        context.peer.set_peer_info(request.clone());
        let client = (
            request.client_info.name.clone(),
            request.client_info.version.clone(),
        );
        crate::app::write_heartbeat(&self.live, Some(client));
        self.negotiate_initialize(&request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_the_server_sends_exists_in_the_remote_script() {
        let src = crate::REMOTE_SCRIPT_SOURCE;
        let start = src
            .find("SCRIPT_CAPABILITIES = [")
            .expect("capability list in script");
        let end = src[start..].find(']').expect("list end") + start;
        let listed: Vec<String> = src[start..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        for cmd in ALL_REMOTE_COMMANDS {
            assert!(
                listed.contains(&cmd.to_string()),
                "{cmd} is not advertised by the Remote Script"
            );
        }
    }

    #[test]
    fn tool_count_and_schema_defaults() {
        let router = Server::tool_router();
        let tools = router.list_all();
        assert_eq!(tools.len(), 31);
        let create_clip = tools.iter().find(|t| t.name == "create_clip").unwrap();
        let schema = serde_json::to_value(&create_clip.input_schema).unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r == "track_index"));
        assert!(
            !required.iter().any(|r| r == "length"),
            "length has a default"
        );
        assert!(!required.iter().any(|r| r == "user_prompt"));
    }
}
