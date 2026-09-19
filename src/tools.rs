//! The MCP server: every tool Claude can call, plus the wrapper that adds
//! telemetry and trajectory recording around each call.
//!
//! Tool bodies are plain functions `fn(&LiveState, &Params) -> ToolResult`
//! so tests can drive them with a fake bridge; the `#[tool]` methods only
//! bind a body to its [`ToolSpec`] and hand both to [`Server::run`].

use crate::connection::{LiveError, LiveState};
use crate::dataset::recorder::get_recorder;
use crate::dataset::schema::PreferenceSpec;
use crate::dataset::trajectory;
use crate::telemetry::{get_telemetry, EventDraft, EventType};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_handler, tool_router, Peer, RoleServer, ServerHandler};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Telemetry {
    None,
    /// Tool name, success, duration; the prompt with consent.
    Basic,
    /// Also the tool's parameters (and MIDI notes if asked) with consent.
    Rich {
        capture_notes: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trajectory {
    None,
    /// Record the action; snapshot state if the tool is in MODIFYING_TOOLS.
    Auto,
    /// Record the action; never snapshot.
    ReadOnly,
}

pub struct ToolSpec {
    pub name: &'static str,
    pub telemetry: Telemetry,
    pub trajectory: Trajectory,
}

impl ToolSpec {
    const fn basic(name: &'static str) -> Self {
        Self {
            name,
            telemetry: Telemetry::Basic,
            trajectory: Trajectory::Auto,
        }
    }
    const fn rich(name: &'static str) -> Self {
        Self {
            name,
            telemetry: Telemetry::Rich {
                capture_notes: false,
            },
            trajectory: Trajectory::Auto,
        }
    }
    const fn read_only(name: &'static str, telemetry: Telemetry) -> Self {
        Self {
            name,
            telemetry,
            trajectory: Trajectory::ReadOnly,
        }
    }
    const fn no_trajectory(name: &'static str, telemetry: Telemetry) -> Self {
        Self {
            name,
            telemetry,
            trajectory: Trajectory::None,
        }
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
fn five() -> i64 {
    5
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
            /// The original user prompt that led to this tool call (recorded
            /// only with telemetry consent)
            #[serde(default)]
            pub user_prompt: String,
        }
    };
}

params!(Empty {});
params!(SetDatasetConsentParams {
    /// True if the user agreed to contribute, False if they declined
    consent: bool,
    /// The user's reply, quoted as closely as possible
    #[serde(default)]
    user_said: String,
});
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
    /// Whether the note is muted
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
    /// Notes to add, each with pitch, start_time, duration, velocity and mute
    notes: Vec<Note>,
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
    /// Beat position in the arrangement to place the clip (e.g. 0.0 = start, 8.0 = bar 3 in 4/4)
    destination_time: f64,
});
params!(CreateLocatorParams {
    /// The locator label (e.g. "Chorus", "Verse 1", "Drop")
    name: String,
    /// Beat position where the locator should sit
    time: f64,
});
params!(SubmitIntentParams {
    /// Natural-language intent (e.g. "make the chorus feel bigger")
    text: String,
    /// Hierarchical intent level 1-5 (default 5)
    level: i64 = "five",
});
params!(RateLastActionParams {
    /// Preference label: better | same | worse | keep | reject | thumbs_up | thumbs_down
    rating: String,
    /// Comma-separated aspect tags: groove, harmony, melody, sound, arrangement, energy, mix, emotion
    #[serde(default)]
    tags: String,
    /// Optional free-text reason
    #[serde(default)]
    note: String,
});
params!(PreferCandidateParams {
    /// Id / URI / label for option A
    candidate_a: String,
    /// Id / URI / label for option B
    candidate_b: String,
    /// 'a', 'b', or the winning id
    winner: String,
    /// Optional reason ("C has the right attack")
    #[serde(default)]
    reason: String,
});
params!(RejectLastActionParams {
    /// Optional why it was rejected
    #[serde(default)]
    reason: String,
});
params!(RecordAuditionParams {
    /// Browser item URI (or stable preset/sample id)
    uri: String,
    /// Whether this candidate was kept
    #[serde(default)]
    kept: bool,
    /// Optional search text that led here (e.g. "analog bass")
    #[serde(default)]
    search_query: String,
    /// Optional time spent auditioning
    #[serde(default)]
    dwell_ms: f64,
});

// ── Tool specs ──────────────────────────────────────────────────────────────

pub const SET_DATASET_CONSENT: ToolSpec =
    ToolSpec::no_trajectory("set_dataset_consent", Telemetry::None);
pub const GET_SESSION_INFO: ToolSpec = ToolSpec::basic("get_session_info");
pub const GET_REMOTE_SCRIPT_INFO: ToolSpec =
    ToolSpec::read_only("get_remote_script_info", Telemetry::Basic);
pub const GET_TRACK_INFO: ToolSpec = ToolSpec::basic("get_track_info");
pub const GET_CLIP_NOTES: ToolSpec = ToolSpec::basic("get_clip_notes");
pub const GET_DEVICE_PARAMETERS: ToolSpec = ToolSpec::basic("get_device_parameters");
pub const SET_DEVICE_PARAMETER: ToolSpec = ToolSpec::rich("set_device_parameter");
pub const GET_SESSION_SNAPSHOT: ToolSpec = ToolSpec::basic("get_session_snapshot");
pub const CREATE_MIDI_TRACK: ToolSpec = ToolSpec::basic("create_midi_track");
pub const CREATE_AUDIO_TRACK: ToolSpec = ToolSpec::basic("create_audio_track");
pub const SET_TRACK_NAME: ToolSpec = ToolSpec::rich("set_track_name");
pub const CREATE_CLIP: ToolSpec = ToolSpec::rich("create_clip");
pub const CREATE_AUDIO_CLIP: ToolSpec = ToolSpec::rich("create_audio_clip");
pub const ADD_NOTES_TO_CLIP: ToolSpec = ToolSpec {
    name: "add_notes_to_clip",
    telemetry: Telemetry::Rich {
        capture_notes: true,
    },
    trajectory: Trajectory::Auto,
};
pub const CLEAR_NOTES_FROM_CLIP: ToolSpec = ToolSpec::no_trajectory(
    "clear_notes_from_clip",
    Telemetry::Rich {
        capture_notes: false,
    },
);
pub const SET_CLIP_NAME: ToolSpec = ToolSpec::rich("set_clip_name");
pub const SET_ARRANGEMENT_CLIP_NAME: ToolSpec = ToolSpec::rich("set_arrangement_clip_name");
pub const SET_TEMPO: ToolSpec = ToolSpec::rich("set_tempo");
pub const LOAD_INSTRUMENT_OR_EFFECT: ToolSpec = ToolSpec::rich("load_instrument_or_effect");
pub const FIRE_CLIP: ToolSpec = ToolSpec::basic("fire_clip");
pub const STOP_CLIP: ToolSpec = ToolSpec::basic("stop_clip");
pub const DELETE_CLIP: ToolSpec = ToolSpec::no_trajectory("delete_clip", Telemetry::Basic);
pub const START_PLAYBACK: ToolSpec = ToolSpec::basic("start_playback");
pub const STOP_PLAYBACK: ToolSpec = ToolSpec::basic("stop_playback");
pub const GET_BROWSER_TREE: ToolSpec = ToolSpec::rich("get_browser_tree");
pub const GET_BROWSER_ITEMS_AT_PATH: ToolSpec = ToolSpec::rich("get_browser_items_at_path");
pub const LOAD_DRUM_KIT: ToolSpec = ToolSpec::rich("load_drum_kit");
pub const SWITCH_TO_ARRANGEMENT_VIEW: ToolSpec = ToolSpec::basic("switch_to_arrangement_view");
pub const SET_ARRANGEMENT_TIME: ToolSpec = ToolSpec::rich("set_arrangement_time");
pub const GET_ARRANGEMENT_CLIPS: ToolSpec = ToolSpec::basic("get_arrangement_clips");
pub const DUPLICATE_TO_ARRANGEMENT: ToolSpec = ToolSpec::rich("duplicate_to_arrangement");
pub const CREATE_LOCATOR: ToolSpec = ToolSpec::no_trajectory(
    "create_locator",
    Telemetry::Rich {
        capture_notes: false,
    },
);
pub const SUBMIT_INTENT: ToolSpec = ToolSpec::read_only("submit_intent", Telemetry::Basic);
pub const RATE_LAST_ACTION: ToolSpec = ToolSpec::read_only("rate_last_action", Telemetry::Basic);
pub const PREFER_CANDIDATE: ToolSpec = ToolSpec::read_only("prefer_candidate", Telemetry::Basic);
pub const REJECT_LAST_ACTION: ToolSpec =
    ToolSpec::read_only("reject_last_action", Telemetry::Basic);
pub const RECORD_AUDITION: ToolSpec = ToolSpec::read_only(
    "record_audition",
    Telemetry::Rich {
        capture_notes: false,
    },
);

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

pub fn set_dataset_consent_body(_live: &LiveState, p: &SetDatasetConsentParams) -> ToolResult {
    let quote = if p.user_said.trim().is_empty() {
        None
    } else {
        Some(p.user_said.as_str())
    };
    let state = crate::dataset::consent::record_consent(p.consent, quote);
    if !p.consent {
        return Ok("Recorded: dataset contribution declined. Nothing from this session is uploaded, and you will not be asked again.".to_string());
    }
    crate::telemetry::refresh_consent_from_dataset();
    match get_recorder() {
        None => Ok(format!(
            "Consent saved, but recording is unavailable (telemetry may be disabled). State: {state}."
        )),
        Some(_) => Ok(
            "Thank you — this session is now being contributed to the open dataset. Say so at any time to stop, or set ABLETON_MCP_DISABLE_DATASET=1."
                .to_string(),
        ),
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
    live.send_command(
        "get_session_snapshot",
        Some(json!({"include_notes": p.include_notes, "include_params": p.include_params})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("get session snapshot", e))
}

pub fn create_midi_track_body(live: &LiveState, p: &CreateTrackParams) -> ToolResult {
    require(live, "create_midi_track")?;
    let r = live
        .send_command("create_midi_track", Some(json!({"index": p.index})))
        .map_err(|e| live_err("create MIDI track", e))?;
    Ok(format!(
        "Created new MIDI track: {}",
        get_display(&r, "name", "unknown")
    ))
}

pub fn create_audio_track_body(live: &LiveState, p: &CreateTrackParams) -> ToolResult {
    require(live, "create_audio_track")?;
    let r = live
        .send_command("create_audio_track", Some(json!({"index": p.index})))
        .map_err(|e| live_err("create audio track", e))?;
    Ok(format!(
        "Created new audio track: {}",
        get_display(&r, "name", "unknown")
    ))
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
    live.send_command(
        "create_clip",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "length": p.length})),
    )
    .map_err(|e| live_err("create clip", e))?;
    Ok(format!(
        "Created new clip at track {}, slot {} with length {} beats",
        p.track_index, p.clip_index, p.length
    ))
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
    live.send_command(
        "add_notes_to_clip",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "notes": p.notes})),
    )
    .map_err(|e| live_err("add notes to clip", e))?;
    Ok(format!(
        "Added {} notes to clip at track {}, slot {}",
        p.notes.len(),
        p.track_index,
        p.clip_index
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

pub fn duplicate_to_arrangement_body(
    live: &LiveState,
    p: &DuplicateToArrangementParams,
) -> ToolResult {
    require(live, "duplicate_session_clip_to_arrangement")?;
    let r = live
        .send_command(
            "duplicate_session_clip_to_arrangement",
            Some(json!({
                "track_index": p.track_index,
                "clip_index": p.clip_index,
                "destination_time": p.destination_time,
            })),
        )
        .map_err(|e| live_err("duplicate clip to arrangement", e))?;
    Ok(format!(
        "Duplicated '{}' from Session slot {} on '{}' to arrangement at beat {}",
        get_display(&r, "clip_name", "clip"),
        p.clip_index,
        get_display(&r, "track_name", &format!("track {}", p.track_index)),
        p.destination_time
    ))
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

const DATASET_OFF: &str =
    "Dataset recording is off (telemetry disabled or user has not consented).";

pub fn submit_intent_body(_live: &LiveState, p: &SubmitIntentParams) -> ToolResult {
    let recorder = get_recorder().ok_or_else(|| DATASET_OFF.to_string())?;
    let event = recorder.set_intent(&p.text, Some(p.level), "explicit");
    Ok(format!(
        "Recorded intent {} (level {}): {:?}. Session: {}",
        event.intent_id.unwrap_or_default(),
        p.level,
        p.text,
        recorder.session_id
    ))
}

pub fn rate_last_action_body(_live: &LiveState, p: &RateLastActionParams) -> ToolResult {
    let recorder = get_recorder().ok_or_else(|| DATASET_OFF.to_string())?;
    let tags: Vec<String> = p
        .tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let rating = p.rating.trim().to_lowercase();
    let event = recorder.record_preference(PreferenceSpec {
        rating: &rating,
        tags: if tags.is_empty() { None } else { Some(tags) },
        note: if p.note.is_empty() {
            None
        } else {
            Some(p.note.clone())
        },
        ..Default::default()
    });
    Ok(format!(
        "Recorded preference {:?} for action {}",
        p.rating,
        event.target_action_id.unwrap_or_else(|| "none".to_string())
    ))
}

pub fn prefer_candidate_body(_live: &LiveState, p: &PreferCandidateParams) -> ToolResult {
    let recorder = get_recorder().ok_or_else(|| DATASET_OFF.to_string())?;
    let winner = match p.winner.trim().to_lowercase().as_str() {
        "a" => p.candidate_a.clone(),
        "b" => p.candidate_b.clone(),
        other => other.to_string(),
    };
    let event = recorder.record_preference(PreferenceSpec {
        rating: "pairwise",
        winner: Some(winner.clone()),
        candidate_a: Some(p.candidate_a.clone()),
        candidate_b: Some(p.candidate_b.clone()),
        note: if p.reason.is_empty() {
            None
        } else {
            Some(p.reason.clone())
        },
        ..Default::default()
    });
    Ok(format!(
        "Recorded pairwise preference: winner={:?} (event {})",
        winner, event.event_id
    ))
}

pub fn reject_last_action_body(_live: &LiveState, p: &RejectLastActionParams) -> ToolResult {
    let recorder = get_recorder().ok_or_else(|| DATASET_OFF.to_string())?;
    let event = recorder.record_preference(PreferenceSpec {
        rating: "reject",
        note: if p.reason.is_empty() {
            None
        } else {
            Some(p.reason.clone())
        },
        ..Default::default()
    });
    Ok(format!(
        "Recorded rejection for action {}",
        event.target_action_id.unwrap_or_else(|| "none".to_string())
    ))
}

pub fn record_audition_body(_live: &LiveState, p: &RecordAuditionParams) -> ToolResult {
    let recorder = get_recorder().ok_or_else(|| DATASET_OFF.to_string())?;
    let event = recorder.record_audition(
        &p.uri,
        Some(p.kept),
        if p.search_query.is_empty() {
            None
        } else {
            Some(p.search_query.clone())
        },
        if p.dwell_ms > 0.0 {
            Some(p.dwell_ms)
        } else {
            None
        },
    );
    Ok(format!(
        "Recorded audition ({}) uri={:?} event={}",
        if p.kept { "kept" } else { "rejected" },
        p.uri,
        event.event_id
    ))
}

// ── Telemetry metadata ──────────────────────────────────────────────────────

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

fn pitch_to_name(pitch: i64) -> String {
    let octave = pitch.div_euclid(12) - 1;
    format!("{}{}", NOTE_NAMES[pitch.rem_euclid(12) as usize], octave)
}

/// Parameters worth keeping in the rich telemetry tier. Structural values
/// only; `path` is reduced to its extension because it embeds the OS user.
pub fn extract_tool_params(args: &Map<String, Value>, capture_notes: bool) -> Map<String, Value> {
    const CAPTURE_KEYS: &[&str] = &[
        "track_index",
        "clip_index",
        "index",
        "length",
        "time",
        "destination_time",
        "tempo",
        "uri",
        "rack_uri",
        "kit_path",
        "category_type",
        "name",
    ];
    let mut params = Map::new();
    for key in CAPTURE_KEYS {
        if let Some(value) = args.get(*key).filter(|v| !v.is_null()) {
            let value = match value {
                Value::String(s) if s.chars().count() > 500 => {
                    json!(format!("{}...", s.chars().take(500).collect::<String>()))
                }
                other => other.clone(),
            };
            params.insert((*key).to_string(), value);
        }
    }
    if let Some(Value::String(path)) = args.get("path") {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()));
        params.insert(
            "file_extension".into(),
            json!(ext.unwrap_or_else(|| "unknown".into())),
        );
        params.insert("has_path".into(), json!(true));
    }
    if capture_notes {
        if let Some(Value::Array(notes)) = args.get("notes") {
            params.insert("notes_count".into(), json!(notes.len()));
            if !notes.is_empty() {
                let compact: Vec<(i64, Value, Value, Value)> = notes
                    .iter()
                    .filter_map(Value::as_object)
                    .map(|n| {
                        (
                            n.get("pitch").and_then(Value::as_i64).unwrap_or(0),
                            n.get("start_time").cloned().unwrap_or(json!(0)),
                            n.get("duration").cloned().unwrap_or(json!(0)),
                            n.get("velocity").cloned().unwrap_or(json!(100)),
                        )
                    })
                    .collect();
                params.insert(
                    "notes".into(),
                    Value::Array(
                        compact
                            .iter()
                            .map(|(p, s, d, v)| json!([p, s, d, v]))
                            .collect(),
                    ),
                );
                params.insert(
                    "notes_readable".into(),
                    Value::Array(
                        compact
                            .iter()
                            .take(100)
                            .map(|(p, s, d, v)| json!([pitch_to_name(*p), s, d, v]))
                            .collect(),
                    ),
                );
                if compact.len() > 100 {
                    params.insert("notes_truncated".into(), json!(true));
                }
            }
        }
    }
    params
}

// ── Server ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Server {
    live: Arc<LiveState>,
    tool_router: ToolRouter<Self>,
}

struct Outcome {
    result: ToolResult,
    /// The trajectory layer was active but no recorder exists: ask for
    /// consent after the call.
    ask_consent: bool,
}

fn run_blocking<P: Serialize>(
    live: &LiveState,
    spec: &ToolSpec,
    params: &P,
    body: fn(&LiveState, &P) -> ToolResult,
) -> Outcome {
    let start = Instant::now();
    let args: Map<String, Value> = serde_json::to_value(params)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let user_prompt = args
        .get("user_prompt")
        .and_then(Value::as_str)
        .map(str::to_string);

    let (result, ask_consent) = match spec.trajectory {
        Trajectory::None => (body(live, params), false),
        traj => match trajectory::safe_get_recorder() {
            None => (body(live, params), true),
            Some(recorder) => {
                let modifying =
                    traj == Trajectory::Auto && trajectory::MODIFYING_TOOLS.contains(&spec.name);
                let ctx = trajectory::begin(live, &recorder, spec.name, modifying, &args);
                let result = body(live, params);
                trajectory::finish(
                    live,
                    &recorder,
                    ctx,
                    spec.name,
                    &args,
                    result.is_ok(),
                    result.as_ref().err().map(String::as_str),
                );
                (result, false)
            }
        },
    };

    if spec.telemetry != Telemetry::None {
        let metadata = match spec.telemetry {
            Telemetry::Rich { capture_notes } => {
                Some(json!({"params": extract_tool_params(&args, capture_notes)}))
            }
            _ => None,
        };
        get_telemetry().record_event(
            EventType::ToolExecution,
            EventDraft {
                tool_name: Some(spec.name.to_string()),
                prompt_text: user_prompt,
                success: result.is_ok(),
                duration_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
                error_message: result.as_ref().err().cloned(),
                ableton_version: None,
                metadata,
            },
        );
    }
    Outcome {
        result,
        ask_consent,
    }
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

    /// Execute a tool body with its telemetry and trajectory wrappers. The
    /// body runs on the blocking pool because the Live socket is synchronous.
    pub async fn run<P>(
        &self,
        peer: Option<Peer<RoleServer>>,
        spec: &'static ToolSpec,
        params: P,
        body: fn(&LiveState, &P) -> ToolResult,
    ) -> CallToolResult
    where
        P: Serialize + Send + 'static,
    {
        let live = self.live.clone();
        let outcome = tokio::task::spawn_blocking(move || run_blocking(&live, spec, &params, body))
            .await
            .unwrap_or_else(|e| Outcome {
                result: Err(format!("{} failed unexpectedly: {e}", spec.name)),
                ask_consent: false,
            });

        let mut text = match outcome.result {
            Ok(t) => t,
            Err(e) => return CallToolResult::error(vec![ContentBlock::text(e)]),
        };
        if outcome.ask_consent {
            // Prefer a real client dialog; fall back to the text prompt.
            if let Some(peer) = peer {
                if trajectory::try_elicit(&peer).await && get_recorder().is_some() {
                    return CallToolResult::success(vec![ContentBlock::text(text)]);
                }
            }
            text = trajectory::with_consent_notice(text);
        }
        CallToolResult::success(vec![ContentBlock::text(text)])
    }
}

#[tool_router]
impl Server {
    /// Record the user's answer to the dataset consent question.
    ///
    /// Call this ONLY after the user has answered in their own words. Never
    /// infer the answer, never call it on their behalf, and never call it
    /// because contributing seems helpful. If they have not been asked yet,
    /// ask first and wait for their reply.
    #[tool(name = "set_dataset_consent")]
    async fn set_dataset_consent(
        &self,
        Parameters(p): Parameters<SetDatasetConsentParams>,
    ) -> CallToolResult {
        self.run(None, &SET_DATASET_CONSENT, p, set_dataset_consent_body)
            .await
    }

    /// Get detailed information about the current Ableton session
    #[tool(name = "get_session_info")]
    async fn get_session_info(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<Empty>,
    ) -> CallToolResult {
        self.run(Some(peer), &GET_SESSION_INFO, p, get_session_info_body)
            .await
    }

    /// Report Ableton Remote Script version and capabilities (handshake).
    /// Use this to verify the Live-side bridge matches this server.
    #[tool(name = "get_remote_script_info")]
    async fn get_remote_script_info(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<Empty>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &GET_REMOTE_SCRIPT_INFO,
            p,
            get_remote_script_info_body,
        )
        .await
    }

    /// Get detailed information about a specific track in Ableton.
    #[tool(name = "get_track_info")]
    async fn get_track_info(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<TrackParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &GET_TRACK_INFO, p, get_track_info_body)
            .await
    }

    /// Read all MIDI notes from a Session-view clip. Returns pitch,
    /// start_time, duration, velocity, mute (and extended fields when
    /// available).
    #[tool(name = "get_clip_notes")]
    async fn get_clip_notes(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<ClipParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &GET_CLIP_NOTES, p, get_clip_notes_body)
            .await
    }

    /// Read all parameters for a device on a track (name, value, min, max).
    #[tool(name = "get_device_parameters")]
    async fn get_device_parameters(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<DeviceParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &GET_DEVICE_PARAMETERS,
            p,
            get_device_parameters_body,
        )
        .await
    }

    /// Set a device parameter to a specific value. Use get_device_parameters
    /// first to discover parameter indices and ranges.
    #[tool(name = "set_device_parameter")]
    async fn set_device_parameter(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<SetDeviceParameterParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &SET_DEVICE_PARAMETER,
            p,
            set_device_parameter_body,
        )
        .await
    }

    /// Capture a full project state snapshot: session metadata, every track
    /// (mixer, devices, session clips, arrangement clips), optional MIDI
    /// notes, and optional device parameters.
    #[tool(name = "get_session_snapshot")]
    async fn get_session_snapshot(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<SnapshotParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &GET_SESSION_SNAPSHOT,
            p,
            get_session_snapshot_body,
        )
        .await
    }

    /// Create a new MIDI track in the Ableton session.
    #[tool(name = "create_midi_track")]
    async fn create_midi_track(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<CreateTrackParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &CREATE_MIDI_TRACK, p, create_midi_track_body)
            .await
    }

    /// Create a new audio track in the Ableton session. Use this for
    /// recorded or imported audio (samples, stems, vocals). For MIDI
    /// instruments use create_midi_track instead.
    #[tool(name = "create_audio_track")]
    async fn create_audio_track(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<CreateTrackParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &CREATE_AUDIO_TRACK, p, create_audio_track_body)
            .await
    }

    /// Set the name of a track.
    #[tool(name = "set_track_name")]
    async fn set_track_name(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<SetTrackNameParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &SET_TRACK_NAME, p, set_track_name_body)
            .await
    }

    /// Create a new MIDI clip in the specified track and clip slot.
    #[tool(name = "create_clip")]
    async fn create_clip(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<CreateClipParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &CREATE_CLIP, p, create_clip_body)
            .await
    }

    /// Create a new audio clip in an audio track's clip slot by importing a
    /// file. Requires Ableton Live 12.0.5 or newer.
    #[tool(name = "create_audio_clip")]
    async fn create_audio_clip(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<CreateAudioClipParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &CREATE_AUDIO_CLIP, p, create_audio_clip_body)
            .await
    }

    /// Add MIDI notes to a clip.
    #[tool(name = "add_notes_to_clip")]
    async fn add_notes_to_clip(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<AddNotesParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &ADD_NOTES_TO_CLIP, p, add_notes_to_clip_body)
            .await
    }

    /// Remove all MIDI notes from a Session clip. Writes are additive
    /// (add_notes_to_clip only appends), so to truly modify a clip: read the
    /// notes with get_clip_notes, edit the list, clear_notes_from_clip, then
    /// add_notes_to_clip the edited notes.
    #[tool(name = "clear_notes_from_clip")]
    async fn clear_notes_from_clip(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<ClipParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &CLEAR_NOTES_FROM_CLIP,
            p,
            clear_notes_from_clip_body,
        )
        .await
    }

    /// Set the name of a Session clip.
    #[tool(name = "set_clip_name")]
    async fn set_clip_name(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<SetClipNameParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &SET_CLIP_NAME, p, set_clip_name_body)
            .await
    }

    /// Set the name of a clip placed in the Arrangement timeline. clip_index
    /// is the position in track.arrangement_clips, in the same order
    /// returned by get_arrangement_clips (ordered by start_time).
    #[tool(name = "set_arrangement_clip_name")]
    async fn set_arrangement_clip_name(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<SetClipNameParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &SET_ARRANGEMENT_CLIP_NAME,
            p,
            set_arrangement_clip_name_body,
        )
        .await
    }

    /// Set the tempo of the Ableton session.
    #[tool(name = "set_tempo")]
    async fn set_tempo(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<SetTempoParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &SET_TEMPO, p, set_tempo_body).await
    }

    /// Load an instrument or effect onto a track using its browser URI.
    #[tool(name = "load_instrument_or_effect")]
    async fn load_instrument_or_effect(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<LoadInstrumentParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &LOAD_INSTRUMENT_OR_EFFECT,
            p,
            load_instrument_or_effect_body,
        )
        .await
    }

    /// Start playing a clip.
    #[tool(name = "fire_clip")]
    async fn fire_clip(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<ClipParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &FIRE_CLIP, p, fire_clip_body).await
    }

    /// Stop playing a clip.
    #[tool(name = "stop_clip")]
    async fn stop_clip(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<ClipParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &STOP_CLIP, p, stop_clip_body).await
    }

    /// Delete the clip in the given clip slot, freeing it for reuse. Use
    /// this before create_clip when you want to overwrite an existing clip
    /// (create_clip refuses to write into an occupied slot).
    #[tool(name = "delete_clip")]
    async fn delete_clip(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<ClipParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &DELETE_CLIP, p, delete_clip_body)
            .await
    }

    /// Start playing the Ableton session.
    #[tool(name = "start_playback")]
    async fn start_playback(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<Empty>,
    ) -> CallToolResult {
        self.run(Some(peer), &START_PLAYBACK, p, start_playback_body)
            .await
    }

    /// Stop playing the Ableton session.
    #[tool(name = "stop_playback")]
    async fn stop_playback(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<Empty>,
    ) -> CallToolResult {
        self.run(Some(peer), &STOP_PLAYBACK, p, stop_playback_body)
            .await
    }

    /// Get a hierarchical tree of browser categories from Ableton.
    #[tool(name = "get_browser_tree")]
    async fn get_browser_tree(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<BrowserTreeParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &GET_BROWSER_TREE, p, get_browser_tree_body)
            .await
    }

    /// Get browser items at a specific path in Ableton's browser.
    #[tool(name = "get_browser_items_at_path")]
    async fn get_browser_items_at_path(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<BrowserPathParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &GET_BROWSER_ITEMS_AT_PATH,
            p,
            get_browser_items_at_path_body,
        )
        .await
    }

    /// Load a drum rack and then load a specific drum kit into it.
    #[tool(name = "load_drum_kit")]
    async fn load_drum_kit(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<LoadDrumKitParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &LOAD_DRUM_KIT, p, load_drum_kit_body)
            .await
    }

    /// Switch Ableton's main window to the Arrangement view.
    #[tool(name = "switch_to_arrangement_view")]
    async fn switch_to_arrangement_view(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<Empty>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
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
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<ArrangementTimeParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &SET_ARRANGEMENT_TIME,
            p,
            set_arrangement_time_body,
        )
        .await
    }

    /// List all clips placed in the Arrangement timeline for a track:
    /// name, start_time, end_time, length, and type.
    #[tool(name = "get_arrangement_clips")]
    async fn get_arrangement_clips(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<TrackParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &GET_ARRANGEMENT_CLIPS,
            p,
            get_arrangement_clips_body,
        )
        .await
    }

    /// Copy a Session-view clip into the Arrangement timeline at
    /// destination_time beats, on the same track. Typical workflow:
    /// create_clip / add_notes_to_clip, then duplicate_to_arrangement once
    /// per bar or section, then switch_to_arrangement_view to confirm.
    #[tool(name = "duplicate_to_arrangement")]
    async fn duplicate_to_arrangement(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<DuplicateToArrangementParams>,
    ) -> CallToolResult {
        self.run(
            Some(peer),
            &DUPLICATE_TO_ARRANGEMENT,
            p,
            duplicate_to_arrangement_body,
        )
        .await
    }

    /// Create a named locator (cue point) in the Arrangement at a beat
    /// position. A locator already at that beat is renamed instead.
    #[tool(name = "create_locator")]
    async fn create_locator(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<CreateLocatorParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &CREATE_LOCATOR, p, create_locator_body)
            .await
    }

    /// Record the human's creative intent for subsequent actions in this
    /// session, so trajectory steps are conditioned on it. Levels:
    /// 1=atomic, 2=musical op, 3=section, 4=song, 5=creative goal.
    /// Requires dataset consent.
    #[tool(name = "submit_intent")]
    async fn submit_intent(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<SubmitIntentParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &SUBMIT_INTENT, p, submit_intent_body)
            .await
    }

    /// Rate the most recent recorded action (or the track after an edit).
    /// Ratings: better | same | worse | keep | reject | thumbs_up |
    /// thumbs_down. Requires dataset consent.
    #[tool(name = "rate_last_action")]
    async fn rate_last_action(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<RateLastActionParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &RATE_LAST_ACTION, p, rate_last_action_body)
            .await
    }

    /// Record a pairwise preference between two candidate actions or
    /// auditions. Requires dataset consent.
    #[tool(name = "prefer_candidate")]
    async fn prefer_candidate(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<PreferCandidateParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &PREFER_CANDIDATE, p, prefer_candidate_body)
            .await
    }

    /// Mark the last action as rejected, e.g. when the human undoes or
    /// discards an agent edit. Requires dataset consent.
    #[tool(name = "reject_last_action")]
    async fn reject_last_action(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<RejectLastActionParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &REJECT_LAST_ACTION, p, reject_last_action_body)
            .await
    }

    /// Log a browser/preset/sample audition (keep or reject) for preference
    /// learning. Does not load the device. Requires dataset consent.
    #[tool(name = "record_audition")]
    async fn record_audition(
        &self,
        peer: Peer<RoleServer>,
        Parameters(p): Parameters<RecordAuditionParams>,
    ) -> CallToolResult {
        self.run(Some(peer), &RECORD_AUDITION, p, record_audition_body)
            .await
    }
}

#[tool_handler(router = self.tool_router, name = "AbletonMusicMaker")]
impl ServerHandler for Server {}

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
        assert_eq!(tools.len(), 37);
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

    #[test]
    fn note_names() {
        assert_eq!(pitch_to_name(60), "C4");
        assert_eq!(pitch_to_name(61), "C#4");
        assert_eq!(pitch_to_name(0), "C-1");
    }
}
