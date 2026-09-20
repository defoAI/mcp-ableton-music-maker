//! The MCP server: every tool Claude can call, plus the wrapper that writes
//! one local activity line around each call.
//!
//! Tool bodies are plain functions `fn(&LiveState, &Params) -> ToolResult`
//! so tests can drive them with a fake bridge; the `#[tool]` methods only
//! bind a body to its [`ToolSpec`] and hand both to [`Server::run`].

use crate::connection::{self, LiveError, LiveState, Performance, Take};
use crate::notes::NotesInput;
use crate::performance::{self as perf, CueParams, PerfState};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, InitializeRequestParams, InitializeResult};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
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
    "set_track_mixer",
    "set_send",
    "get_returns",
    "set_track_color",
    "set_clip_color",
    "get_drum_rack_pads",
    "delete_arrangement_clip",
    "delete_locator",
    "search_browser",
    "get_clip_info",
    "set_clip_loop",
    "set_clip_launch",
    "get_track_meters",
    "set_clip_automation",
    "get_clip_automation",
    "get_library_status",
    "ensure_capture_track",
    "start_capture",
    "capture_status",
    "stop_capture",
    "list_captures",
    "play_from",
    "delete_track",
    "reset_set",
    "back_to_arrangement",
    "set_arrangement_loop",
    "get_performance_state",
    "set_launch_quantization",
    "create_scene",
    "fire_scene",
    "stop_all_clips",
    "set_crossfader",
    "record_clip",
    "schedule_cue",
    "cancel_cue",
    "set_scale",
    "set_slot_stop_buttons",
    "get_context",
    "set_performance_mode",
    "set_scene",
    "start_live_capture",
    "snapshot_mix",
    "restore_mix",
    "get_browser_index",
    "capture_scene",
    "duplicate_scene",
    "get_grooves",
    "set_clip_groove",
    "set_device_parameters",
    "place_clips",
    "delete_arrangement_clips",
    "duplicate_arrangement_clip",
    "create_return_track",
    "create_tracks",
    "write_clips",
    "arrangement_summary",
    "start_arrangement_record",
    "stop_arrangement_record",
    "place_sample",
    "list_sample_folders",
    "delete_device",
    "move_device",
    "get_meter_scale",
    "subscribe",
    "unsubscribe",
    "describe",
    "run",
];

pub type ToolResult = Result<String, String>;

/// The standard MCP hints for a tool, from its name: reads are read-only,
/// deletions are destructive, everything stays inside the machine.
fn annotations_for(name: &str) -> rmcp::model::ToolAnnotations {
    let read_only = name.starts_with("get_")
        || name.starts_with("list_")
        || name.starts_with("search_")
        || name.starts_with("measure_")
        || name == "listen";
    let destructive = name.starts_with("delete_")
        || name.starts_with("clear_")
        || name.starts_with("remove_")
        || matches!(
            name,
            "reset_set"
                | "import_set"
                | "end_performance"
                | "panic"
                | "stop_playback"
                | "stop_clip"
                | "undo_vary"
        );
    let mut a = rmcp::model::ToolAnnotations::new().read_only(read_only);
    a.destructive_hint = Some(destructive);
    a.idempotent_hint = Some(read_only);
    a.open_world_hint = Some(false);
    a
}

/// The raw layer's tools are served under this prefix (`adv_fire_scene`).
pub const ADVANCED_PREFIX: &str = "adv_";

/// The artist's set, in five groups — look, build, shape, arrange, play —
/// plus the housekeeping a session needs. Every other tool is the raw
/// layer: still served, as `adv_<name>`.
pub const CORE_TOOLS: &[&str] = &[
    // look
    "get_context",
    // build
    "build_song",
    "make_section",
    "create_clip",
    "add_notes_to_clip",
    "load_instrument_or_effect",
    "search_browser",
    "add_sample",
    "set_key",
    "set_tempo",
    // shape
    "shape_sound",
    "feel",
    "set_track_mixer",
    "set_send",
    "create_return",
    // arrange
    "set_song",
    "add_to_song",
    "remove_from_song",
    "arrange",
    "create_locator",
    // play
    "play_song",
    "go",
    "jump_to",
    "back",
    "hold_section",
    "next_section",
    "previous_section",
    "record_clip",
    "capture_mix",
    "clear_captures",
    "end_performance",
    // remember
    "remember",
    "stash",
    // housekeeping
    "delete_track",
    "delete_clip",
    "export_set",
    "import_set",
    "batch",
];

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

/// A beat position without a pointless ".0".
fn beat(v: Option<&Value>) -> String {
    match v.and_then(Value::as_f64) {
        Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => "?".to_string(),
    }
}

pub(crate) fn get_display(v: &Value, key: &str, default: &str) -> String {
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
fn track_kind() -> String {
    "track".to_string()
}
fn quarter_beat() -> f64 {
    0.25
}
fn eight() -> i64 {
    8
}
fn capture_name() -> String {
    "capture".to_string()
}
fn all_categories() -> String {
    "all".to_string()
}
fn thirty() -> i64 {
    30
}
fn linear() -> String {
    "linear".to_string()
}
fn one() -> f64 {
    1.0
}
fn hundred_ms() -> i64 {
    100
}

/// What to automate: a device parameter, or a mixer control.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct AutomationTarget {
    /// Index into the track's device chain (with parameter_index)
    #[serde(default)]
    pub device_index: Option<i64>,
    /// Index into device.parameters — see get_device_parameters
    #[serde(default)]
    pub parameter_index: Option<i64>,
    /// Or a mixer control: "volume", "pan" or "send"
    #[serde(default)]
    pub mixer: Option<String>,
    /// With mixer "send": which send
    #[serde(default)]
    pub send_index: Option<i64>,
}

/// One automation breakpoint: the parameter reaches `value` at `time` beats.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct AutomationPoint {
    pub time: f64,
    pub value: f64,
}

/// A ramp in one line: from `from` at `start` to `to` at `start + over`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct Ramp {
    pub from: f64,
    pub to: f64,
    /// Beats the ramp takes
    pub over: f64,
    /// Beat the ramp starts at (default 0)
    #[serde(default)]
    pub start: f64,
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
    /// The track: a name or an index
    track: Option<Value>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
});
params!(ResetSetParams {
    /// How many tracks the fresh set has (default 4: two MIDI, two audio)
    tracks: Option<i64>,
    /// How many scenes (default 8)
    scenes: Option<i64>,
    /// The tempo to come back to (default 120)
    tempo: Option<f64>,
    /// Delete the return tracks too (default false: they are kept)
    returns: Option<bool>,
});
params!(TrackInfoParams {
    /// The track: a name, an index, "master", or a return's name or letter
    track: Option<Value>,
    /// "track" (default), "return" or "master" — with a numeric track
    kind: Option<String>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
});
params!(ClipParams {
    /// The track holding the clip: a name or an index
    track: Option<Value>,
    /// The clip: its name, or the Session slot it sits in
    clip: Option<Value>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The clip by slot index, when `clip` is not given
    clip_index: Option<i64>,
});
params!(DeviceParams {
    /// The track that owns the device: a name, an index, "master", or a
    /// return's name or letter ("A")
    track: Option<Value>,
    /// "track" (default), "return" or "master" — with a numeric track
    kind: Option<String>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The device by name (a substring is enough) or index
    device: Option<Value>,
    /// Index into the track's device chain (default: the first device)
    device_index: Option<i64>,
});
params!(SetDeviceParameterParams {
    /// The track that owns the device: a name, an index, "master", or a
    /// return's name or letter ("A")
    track: Option<Value>,
    /// "track" (default), "return" or "master" — with a numeric track
    kind: Option<String>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The device by name (a substring is enough) or index
    device: Option<Value>,
    /// Index into the track's device chain (default: the first device)
    device_index: Option<i64>,
    /// Index into device.parameters (or give `parameter` by name)
    parameter_index: Option<i64>,
    /// The parameter by name: an exact name, or a substring of one ("Cutoff", "atk")
    parameter: Option<String>,
    /// The new value: what Live shows ("3 dB", "200 Hz", "Low Cut 48 dB") or
    /// the raw number between the parameter's min and max
    value: Value = "Value::default",
});
params!(EditDevicesParams {
    /// The track whose chain changes: a name, an index, "master", or a
    /// return's name or letter ("A")
    track: Option<Value>,
    /// "track" (default), "return" or "master" — with a numeric track
    kind: Option<String>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The device by name (a substring is enough) or index
    device: Option<Value>,
    /// The device by index, when `device` is not given
    device_index: Option<i64>,
    /// "remove", "move" (with to_index), "bypass" or "enable"
    action: String = "String::new",
    /// With "move": where in the chain it should end up (0 is first)
    to_index: Option<i64>,
});
params!(ShapeSoundParams {
    /// The track: a name, an index, "master", or a return's name or letter
    track: Value,
    /// "track" (default), "return" or "master" — with a numeric track
    kind: Option<String>,
    /// The device, by name (a substring) or index; default: the first instrument, rack or drum rack on the track
    device: Option<Value>,
    /// Any parameter by its own name (or a substring of it): {"LP Mod Amt 2": 0.5, "Glide Time": "+10%"}
    #[serde(default)]
    set: BTreeMap<String, Value>,
    /// Each word: a fraction 0–1 of the parameter's range, or "±N%" relative to where it sits
    cutoff: Option<Value>,
    resonance: Option<Value>,
    attack: Option<Value>,
    decay: Option<Value>,
    sustain: Option<Value>,
    release: Option<Value>,
    drive: Option<Value>,
    detune: Option<Value>,
    width: Option<Value>,
    lfo_rate: Option<Value>,
    reverb: Option<Value>,
    delay: Option<Value>,
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
    /// The track to create the clip in: a name or an index
    track: Option<Value>,
    /// The slot to create it in: a slot index, or the name of a clip to
    /// replace on this track
    clip: Option<Value>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The slot by index, when `clip` is not given
    clip_index: Option<i64>,
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
#[schemars(inline)]
pub struct Note {
    /// MIDI pitch 0-127
    #[serde(deserialize_with = "int_or_float")]
    pub pitch: i64,
    /// Start position in beats
    pub start_time: f64,
    /// Length in beats
    pub duration: f64,
    /// Velocity 1-127
    #[serde(deserialize_with = "int_or_float")]
    pub velocity: i64,
    /// Optional; defaults to false. Leave it out unless the note is muted.
    #[serde(default)]
    pub mute: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Live reports velocities (and, on some versions, pitches) as floats;
/// notes read with get_clip_notes must write back unchanged.
fn int_or_float<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let v = Value::deserialize(d)?;
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.round() as i64))
            .ok_or_else(|| serde::de::Error::custom("not a whole number")),
        other => Err(serde::de::Error::custom(format!(
            "expected a number, got {other}"
        ))),
    }
}

params!(AddNotesParams {
    /// The track holding the clip: a name or an index
    track: Option<Value>,
    /// The clip: its name, or the Session slot it sits in
    clip: Option<Value>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The clip by slot index, when `clip` is not given
    clip_index: Option<i64>,
    /// Remove the clip's existing notes first, so this call replaces
    /// instead of appending (default false)
    clear: bool = "bool::default",
    /// Also refresh the Arrangement copies of this clip (same name, same
    /// track): they are deleted and placed again at their start times
    propagate_to_arrangement: bool = "bool::default",
    /// The notes, in any mix of: notes (objects), notes_csv, steps, patterns;
    /// plus loop_every/until to tile them
    #[serde(flatten)]
    input: NotesInput,
});
params!(SetClipNameParams {
    /// The track holding the clip: a name or an index
    track: Option<Value>,
    /// The clip: its current name, or the Session slot it sits in
    clip: Option<Value>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The clip by index, when `clip` is not given (Session slot, or
    /// position in track.arrangement_clips)
    clip_index: Option<i64>,
    /// The new name for the clip
    name: String,
});
params!(SetTempoParams {
    /// The new tempo in BPM
    tempo: f64,
});
params!(LoadInstrumentParams {
    /// The track: a name, an index, "master", or a return's name or letter
    track: Option<Value>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// The instrument or effect: a browser URI, or plain words ("reverb", "analog bass") searched in the library
    uri: String,
    /// "track" (default), "return" (an effect on a return track) or "master" (an effect on the master)
    kind: String = "track_kind",
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
    /// The bar to move the playhead to (Live's 1-based bars; 3.5 is halfway through bar 3)
    bar: Option<f64>,
    /// Or a position in beats from the start of the arrangement (8.0 = bar 3 in 4/4)
    time: Option<f64>,
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
    /// Or in bars (Live's 1-based bars): the bar to place at
    at_bar: Option<f64>,
    /// With `until_bar`: place a copy every `every_bars` (default the clip's length) from at_bar up to until_bar (exclusive)
    until_bar: Option<f64>,
    every_bars: Option<f64>,
});
params!(SetTrackMixerParams {
    /// The track: a name, an index, "master", or a return's name or letter
    track: Option<Value>,
    /// The track by index, when `track` is not given (among the song's
    /// tracks, or among the return tracks when kind is "return")
    track_index: Option<i64>,
    /// "track" (default), "return" or "master"
    kind: String = "track_kind",
    /// Fader in dB, e.g. -6 (0 is unity, +6 the top); the reply reads it back in dB
    volume: Option<f64>,
    /// The same, for callers that spell it out
    volume_db: Option<f64>,
    /// Or Live's raw fader parameter 0.0-1.0 (0.85 is 0 dB)
    fader: Option<f64>,
    /// Pan, -1.0 (left) to 1.0 (right)
    pan: Option<f64>,
    mute: Option<bool>,
    solo: Option<bool>,
    /// Record arm (MIDI and audio tracks only)
    arm: Option<bool>,
});
params!(SetSendParams {
    /// The track whose send changes: a name or an index
    track: Option<Value>,
    /// The track by index, when `track` is not given
    track_index: Option<i64>,
    /// "track" (default) or "return"
    kind: String = "track_kind",
    /// The return track's name ("Reverb") or letter ("A"); see get_returns
    send_name: String = "String::new",
    /// The send's index, if you would rather not name it
    send_index: Option<i64>,
    /// Send level 0.0-1.0
    value: f64,
});
params!(SetColorParams {
    /// Index of the track (or of the clip's track)
    track_index: i64,
    /// Live colour index, 0-69 (the palette in the colour chooser, left to right, top to bottom)
    color_index: i64,
    /// Colour a clip instead of the track: its slot index (Session) or
    /// position in the Arrangement when `arrangement` is true
    clip_index: Option<i64>,
    /// With clip_index: the clip is an Arrangement clip
    arrangement: bool = "bool::default",
    /// "track" (default), "return" or "master" — for track colours only
    kind: String = "track_kind",
});
params!(DrumRackPadsParams {
    /// The track that holds the Drum Rack: a name, an index, "master", or a
    /// return's name or letter
    track: Option<Value>,
    /// "track" (default), "return" or "master" — with a numeric track
    kind: Option<String>,
    /// Track that holds the Drum Rack, by index
    track_index: i64,
    /// Which device, if the track has more than one Drum Rack (default: the first)
    device_index: Option<i64>,
});
params!(DeleteArrangementClipParams {
    /// Track that owns the Arrangement clip(s)
    track_index: i64,
    /// Position in the track's Arrangement clips, ordered by start time —
    /// the index get_arrangement_clips lists (ignored with `all` or `clip_indices`)
    clip_index: i64 = "minus_one",
    /// Several positions at once
    #[serde(default)]
    clip_indices: Vec<i64>,
    /// Remove every Arrangement clip on the track
    all: bool = "bool::default",
});
params!(DeleteLocatorParams {
    /// The locator's exact name
    name: String = "String::new",
    /// Or its beat position
    time: Option<f64>,
});
params!(SearchBrowserParams {
    /// Words that must all appear in the item's name or folder path, e.g. "analog bass" or "techno kit"
    query: String = "String::new",
    /// Several searches in one call, e.g. ["analog bass", "techno kit", "evolving pad"]
    queries: Vec<String> = "no_queries",
    /// "all" (instruments, sounds, drums, audio and MIDI effects) or one of: instruments, sounds, drums, audio_effects, midi_effects, samples, packs, user_library
    category: String = "all_categories",
    /// Most hits to return per query (default 30, max 200)
    limit: i64 = "thirty",
    /// One best hit per query, ready to load
    best: bool = "bool::default",
    /// Drop the server's library index and walk the browser again (after installing packs)
    refresh: bool = "bool::default",
});
params!(ClipRefParams {
    /// Track that owns the clip
    track_index: i64,
    /// Session slot index, or position in the Arrangement when `arrangement` is true
    clip_index: i64,
    /// The clip is an Arrangement clip
    arrangement: bool = "bool::default",
});
params!(SetClipLoopParams {
    /// Track that owns the clip
    track_index: i64,
    /// Session slot index, or Arrangement position when `arrangement` is true
    clip_index: i64,
    /// The clip is an Arrangement clip
    arrangement: bool = "bool::default",
    /// Loop on or off
    looping: Option<bool>,
    /// Loop start, in beats from the clip's start
    loop_start: Option<f64>,
    /// Loop end, in beats
    loop_end: Option<f64>,
    /// Where playback starts, in beats
    start_marker: Option<f64>,
    /// Where the clip ends, in beats
    end_marker: Option<f64>,
});
params!(SetClipLaunchParams {
    /// Track that owns the Session clip
    track_index: i64,
    /// Session slot index
    clip_index: i64,
    /// "trigger", "gate", "toggle" or "repeat"
    launch_mode: Option<String>,
    /// "global", "none", "8_bars", "4_bars", "2_bars", "1_bar", "1/2", "1/4", "1/8", "1/16", "1/32" (add "t" for triplets: "1/8t")
    launch_quantization: Option<String>,
    /// Legato launch
    legato: Option<bool>,
    /// How much velocity affects volume, 0.0-1.0
    velocity_amount: Option<f64>,
});
params!(PlayAndMeasureParams {
    /// Beat to start playing from (default: where the playhead is)
    start_time: Option<f64>,
    /// How long to listen, in seconds (default 4, max 10)
    seconds: f64 = "four",
    /// Milliseconds between meter readings (default 100)
    interval_ms: i64 = "hundred_ms",
    /// Stop playback afterwards (default true)
    stop_after: bool = "yes",
});
params!(SetClipAutomationParams {
    /// Track that owns the clip
    track_index: i64,
    /// Session slot index, or Arrangement position when `arrangement` is true
    clip_index: i64,
    /// The clip is an Arrangement clip (how Live 11/12 stores arrangement automation)
    arrangement: bool = "bool::default",
    /// What to automate
    target: AutomationTarget,
    /// Breakpoints, in clip beats; a value is held until the next point (step) or ramped to it (linear)
    #[serde(default)]
    points: Vec<AutomationPoint>,
    /// Or one ramp: {"from": 0.2, "to": 0.9, "over": 32}
    ramp: Option<Ramp>,
    /// "linear" (default) or "step"
    mode: String = "linear",
    /// Step size in beats used to draw ramps (default 0.25)
    resolution: f64 = "quarter_beat",
    /// Remove the parameter's existing envelope first (default true)
    clear: bool = "yes",
});
params!(GetClipAutomationParams {
    /// Track that owns the clip
    track_index: i64,
    /// Session slot index, or Arrangement position when `arrangement` is true
    clip_index: i64,
    /// The clip is an Arrangement clip
    arrangement: bool = "bool::default",
    /// Which envelope to read
    target: AutomationTarget,
    /// Beats between samples (default 1.0)
    resolution: f64 = "one",
});
/// One step of a batch: a tool name and its arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct BatchStep {
    /// Any Live-facing tool name, e.g. "create_clip"
    pub tool: String,
    /// That tool's arguments. Inside integer fields the string "$last_track"
    /// becomes the index of the most recent track created in this batch and
    /// "$last_clip" the slot of the most recent create_clip (also inside cue steps).
    #[serde(default)]
    pub args: Value,
}
params!(BatchParams {
    /// Steps, run in order on one connection to Live
    steps: Vec<BatchStep>,
    /// Stop at the first failing step (default true); with false, later steps still run
    stop_on_error: bool = "yes",
    /// Print every successful step's own text. Off by default: the reply is a
    /// grouped summary, plus every failure and everything a step skipped.
    /// A batch of ten steps or fewer prints in full either way.
    verbose: bool = "bool::default",
});

/// A track in a build_song document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongTrack {
    /// Track name; clips and placements refer to it
    pub name: String,
    /// "midi" (default) or "audio"
    #[serde(default = "midi_kind")]
    pub kind: String,
    /// Browser URI of an instrument or device to load
    #[serde(default)]
    pub instrument: Option<String>,
    /// Or words to search for; the first browser hit is loaded
    #[serde(default)]
    pub instrument_query: Option<String>,
    /// Fader in dB (0 is unity)
    #[serde(default)]
    pub volume: Option<f64>,
    /// The same, spelled out
    #[serde(default)]
    pub volume_db: Option<f64>,
    /// Or Live's raw fader 0.0-1.0 (0.85 = 0 dB)
    #[serde(default)]
    pub fader: Option<f64>,
    #[serde(default)]
    pub pan: Option<f64>,
    /// Live palette index 0-69
    #[serde(default)]
    pub color_index: Option<i64>,
    /// Sends by return name or letter: {"Reverb": 0.3, "B": 0.1}
    #[serde(default)]
    pub sends: BTreeMap<String, f64>,
}
fn midi_kind() -> String {
    "midi".to_string()
}

/// A Session clip in a build_song document, with its notes in any compact form.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongClip {
    /// A track name from `tracks`, or an existing track's index as a string ("2")
    pub track: String,
    /// Session slot (default 0; with `slots` given and this omitted, the clip goes only into `slots`)
    #[serde(default)]
    pub slot: Option<i64>,
    /// Clip name
    #[serde(default)]
    pub name: String,
    /// Length in beats (default 4)
    #[serde(default = "four")]
    pub length: f64,
    /// Extra slots that get a copy of this clip (e.g. [1, 2]), so every scene
    /// row plays it and a scene launch never stops the track
    #[serde(default)]
    pub slots: Vec<i64>,
    #[serde(flatten)]
    pub notes: NotesInput,
}

/// Where a clip goes in the Arrangement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongPlacement {
    pub track: String,
    #[serde(default)]
    pub slot: i64,
    /// Beat positions
    #[serde(default)]
    pub times: Vec<f64>,
    /// Or a range: start, end (exclusive), step
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongLocator {
    pub name: String,
    pub time: f64,
}

/// A scene row in a build_song document: name, tempo, phrase length.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongScene {
    pub name: String,
    #[serde(default)]
    pub tempo: Option<f64>,
    /// Bars per phrase for cues ("next_phrase"); default 16
    #[serde(default)]
    pub phrase_bars: Option<i64>,
}

params!(BuildSongParams {
    /// Tempo in BPM (optional)
    tempo: Option<f64>,
    /// The key, e.g. "F minor": set in Live's scale settings before anything is written
    key: Option<String>,
    /// Scenes (rows) to name in order: row 0 gets scenes[0]; rows that do not exist yet are created
    #[serde(default)]
    scenes: Vec<SongScene>,
    /// Tracks to create, in order
    #[serde(default)]
    tracks: Vec<SongTrack>,
    /// Session clips to create and fill
    #[serde(default)]
    clips: Vec<SongClip>,
    /// Arrangement placements of those clips
    #[serde(default)]
    placements: Vec<SongPlacement>,
    /// Locators to set
    #[serde(default)]
    locators: Vec<SongLocator>,
    /// What to do about tracks, clips and placements the set already has:
    /// "converge" (default — reuse them, so re-running the same document
    /// after a failure finishes it instead of duplicating it), "add" (build
    /// another copy) or "fail"
    on_existing: String = "String::new",
    /// Validate and describe the plan without touching Live (default false)
    dry_run: bool = "bool::default",
    /// Write a rebuildable copy of the finished set under the server's state
    /// folder, the way export_set does (default false). The Live set itself
    /// is still only saved by you, in Live.
    snapshot: bool = "bool::default",
});
params!(SetArrangementLoopParams {
    /// The bar the loop starts on (Live's 1-based bars)
    start_bar: Option<f64>,
    /// How many bars it loops
    bars: Option<f64>,
    /// Or the loop start in beats
    start: Option<f64>,
    /// Or the loop length in beats
    length: Option<f64>,
    /// Loop on or off
    enabled: Option<bool>,
});
params!(CaptureMixParams {
    /// The bar to start capturing from: a number (Live's 1-based bars) or a
    /// locator's name (default: bar 1)
    start_bar: Option<Value>,
    /// Or a beat position (a bar boundary, e.g. 128 for bar 33 in 4/4)
    start: Option<f64>,
    /// How many bars to capture (default 8, max 64)
    bars: i64 = "eight",
    /// A name for the capture, e.g. "drop" — the clip is called "<name> @ <start>"
    name: String = "capture_name",
});
params!(DeviceVocabularyParams {
    /// "show" (default) what this Live's devices have answered to, or
    /// "forget" to delete the file
    action: String = "String::new",
    /// show: one device by name, instead of the summary
    device: Option<String>,
});
params!(MeasureCaptureParams {
    /// Slot index on the Capture track, as list_captures shows
    slot: i64,
});
params!(CreateLocatorParams {
    /// The locator label (e.g. "Chorus", "Verse 1", "Drop")
    name: String,
    /// The bar it marks (Live's 1-based bars)
    bar: Option<f64>,
    /// Or a beat position from the start of the arrangement
    time: Option<f64>,
});

// ── Tool specs ──────────────────────────────────────────────────────────────

pub const GET_SESSION_INFO: ToolSpec = ToolSpec::new("get_session_info");
pub const GET_REMOTE_SCRIPT_INFO: ToolSpec = ToolSpec::new("get_remote_script_info");
pub const GET_TRACK_INFO: ToolSpec = ToolSpec::new("get_track_info");
pub const GET_CLIP_NOTES: ToolSpec = ToolSpec::new("get_clip_notes");
pub const GET_DEVICE_PARAMETERS: ToolSpec = ToolSpec::new("get_device_parameters");
pub const SET_DEVICE_PARAMETER: ToolSpec = ToolSpec::new("set_device_parameter");
pub const EDIT_DEVICES: ToolSpec = ToolSpec::new("edit_devices");
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
pub const SET_TRACK_MIXER: ToolSpec = ToolSpec::new("set_track_mixer");
pub const SET_SEND: ToolSpec = ToolSpec::new("set_send");
pub const GET_RETURNS: ToolSpec = ToolSpec::new("get_returns");
pub const DESCRIBE_LIVE: ToolSpec = ToolSpec::new("describe_live");
pub const RUN_OPS: ToolSpec = ToolSpec::new("run_ops");
pub const SET_COLOR: ToolSpec = ToolSpec::new("set_color");
pub const GET_DRUM_RACK_PADS: ToolSpec = ToolSpec::new("get_drum_rack_pads");
pub const DELETE_ARRANGEMENT_CLIP: ToolSpec = ToolSpec::new("delete_arrangement_clip");
pub const DELETE_LOCATOR: ToolSpec = ToolSpec::new("delete_locator");
pub const SEARCH_BROWSER: ToolSpec = ToolSpec::new("search_browser");
pub const GET_CLIP_INFO: ToolSpec = ToolSpec::new("get_clip_info");
pub const SET_CLIP_LOOP: ToolSpec = ToolSpec::new("set_clip_loop");
pub const SET_CLIP_LAUNCH: ToolSpec = ToolSpec::new("set_clip_launch");
pub const GET_TRACK_METERS: ToolSpec = ToolSpec::new("get_track_meters");
pub const PLAY_AND_MEASURE: ToolSpec = ToolSpec::new("play_and_measure");
pub const SET_CLIP_AUTOMATION: ToolSpec = ToolSpec::new("set_clip_automation");
pub const GET_CLIP_AUTOMATION: ToolSpec = ToolSpec::new("get_clip_automation");
pub const BATCH: ToolSpec = ToolSpec::new("batch");
pub const BUILD_SONG: ToolSpec = ToolSpec::new("build_song");
pub const GET_LIBRARY_STATUS: ToolSpec = ToolSpec::new("get_library_status");
pub const DEVICE_VOCABULARY: ToolSpec = ToolSpec::new("device_vocabulary");
pub const REMEMBER: ToolSpec = ToolSpec::new("remember");
pub const STASH: ToolSpec = ToolSpec::new("stash");
pub const SONG_MEMORY: ToolSpec = ToolSpec::new("song_memory");
pub const CAPTURE_MIX: ToolSpec = ToolSpec::new("capture_mix");
pub const LIST_CAPTURES: ToolSpec = ToolSpec::new("list_captures");
pub const MEASURE_CAPTURE: ToolSpec = ToolSpec::new("measure_capture");
pub const DELETE_TRACK: ToolSpec = ToolSpec::new("delete_track");
pub const RESET_SET: ToolSpec = ToolSpec::new("reset_set");
pub const BACK_TO_ARRANGEMENT: ToolSpec = ToolSpec::new("back_to_arrangement");
pub const SET_ARRANGEMENT_LOOP: ToolSpec = ToolSpec::new("set_arrangement_loop");
pub const START_PERFORMANCE: ToolSpec = ToolSpec::new("start_performance");
pub const GET_PERFORMANCE_STATE: ToolSpec = ToolSpec::new("get_performance_state");
pub const CUE: ToolSpec = ToolSpec::new("cue");
pub const CANCEL_CUE: ToolSpec = ToolSpec::new("cancel_cue");
pub const FIRE_SCENE: ToolSpec = ToolSpec::new("fire_scene");
pub const CREATE_SCENE: ToolSpec = ToolSpec::new("create_scene");
pub const RECORD_CLIP: ToolSpec = ToolSpec::new("record_clip");
pub const SET_LAUNCH_QUANTIZATION: ToolSpec = ToolSpec::new("set_launch_quantization");
pub const SET_CROSSFADER: ToolSpec = ToolSpec::new("set_crossfader");
pub const END_PERFORMANCE: ToolSpec = ToolSpec::new("end_performance");
pub const KEEP_TRACK_PLAYING: ToolSpec = ToolSpec::new("keep_track_playing");
pub const GET_CONTEXT: ToolSpec = ToolSpec::new("get_context");
pub const SET_SCENE: ToolSpec = ToolSpec::new("set_scene");
pub const LISTEN: ToolSpec = ToolSpec::new("listen");
pub const VARY_CLIP: ToolSpec = ToolSpec::new("vary_clip");
pub const UNDO_VARY: ToolSpec = ToolSpec::new("undo_vary");
pub const FOLLOW_KEY: ToolSpec = ToolSpec::new("follow_key");
pub const SNAPSHOT_MIX: ToolSpec = ToolSpec::new("snapshot_mix");
pub const RESTORE_MIX: ToolSpec = ToolSpec::new("restore_mix");
pub const PANIC: ToolSpec = ToolSpec::new("panic");
pub const RETIME_CLIP: ToolSpec = ToolSpec::new("retime_clip");
pub const SHAPE_SOUND: ToolSpec = ToolSpec::new("shape_sound");
pub const EXPORT_SET: ToolSpec = ToolSpec::new("export_set");
pub const SET_KEY: ToolSpec = ToolSpec::new("set_key");
pub const CREATE_RETURN: ToolSpec = ToolSpec::new("create_return");
pub const CLEAR_CAPTURES: ToolSpec = ToolSpec::new("clear_captures");
pub const FEEL: ToolSpec = ToolSpec::new("feel");
pub const ARRANGE: ToolSpec = ToolSpec::new("arrange");
pub const IMPORT_SET: ToolSpec = ToolSpec::new("import_set");
pub const GROOVE_CLIP: ToolSpec = ToolSpec::new("groove_clip");
pub const GROOVE_AMOUNT: ToolSpec = ToolSpec::new("groove_amount");
pub const HUMANIZE: ToolSpec = ToolSpec::new("humanize");
pub const SWING_NOTES: ToolSpec = ToolSpec::new("swing_notes");
pub const MAKE_SECTION: ToolSpec = ToolSpec::new("make_section");
pub const SET_SONG: ToolSpec = ToolSpec::new("set_song");
pub const ADD_TO_SONG: ToolSpec = ToolSpec::new("add_to_song");
pub const REMOVE_FROM_SONG: ToolSpec = ToolSpec::new("remove_from_song");
pub const PLAY_SONG: ToolSpec = ToolSpec::new("play_song");
pub const HOLD_SECTION: ToolSpec = ToolSpec::new("hold_section");
pub const GO: ToolSpec = ToolSpec::new("go");
pub const NEXT_SECTION: ToolSpec = ToolSpec::new("next_section");
pub const PREVIOUS_SECTION: ToolSpec = ToolSpec::new("previous_section");
pub const BACK: ToolSpec = ToolSpec::new("back");
pub const JUMP_TO: ToolSpec = ToolSpec::new("jump_to");
pub const ADD_SAMPLE: ToolSpec = ToolSpec::new("add_sample");
pub const SAMPLE_FOLDERS: ToolSpec = ToolSpec::new("sample_folders");

// ── Tool bodies ─────────────────────────────────────────────────────────────

pub(crate) fn live_err(what: &str, e: LiveError) -> String {
    format!("Could not {what}: {e}")
}

/// Every tool checks that the loaded Remote Script serves the command it is
/// about to send; a stale or missing script gets a reinstall message rather
/// than a half-working session.
pub(crate) fn require(live: &LiveState, capability: &str) -> Result<(), String> {
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

pub fn get_track_info_body(live: &LiveState, p: &TrackInfoParams) -> ToolResult {
    require(live, "get_track_info")?;
    let target = resolve_track_target(live, p.track.as_ref(), p.kind.as_deref(), p.track_index)?;
    live.send_command(
        "get_track_info",
        Some(json!({"track_index": target.index, "kind": target.kind})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("get track info", e))
}

pub fn get_clip_notes_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "get_clip_notes")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    let slot = resolve_clip_slot(live, &target, p.clip.as_ref(), p.clip_index)?;
    live.send_command(
        "get_clip_notes",
        Some(json!({"track_index": target.index, "clip_index": slot})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("get clip notes", e))
}

/// A track, a return or the master, addressed the way `load_instrument_or_effect`
/// has always taken it. Every device-facing tool resolves through this, so
/// anything the loader can reach can be read and corrected afterwards.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrackTarget {
    pub index: i64,
    pub kind: String,
    pub name: String,
}

impl TrackTarget {
    pub(crate) fn at(kind: &str, index: i64) -> Self {
        Self {
            index,
            kind: kind.to_string(),
            name: String::new(),
        }
    }

    /// What the reply calls it, preferring the name Live sent back.
    fn label(&self, from_live: &str) -> String {
        if !from_live.is_empty() && from_live != "?" {
            return from_live.to_string();
        }
        if !self.name.is_empty() {
            return self.name.clone();
        }
        match self.kind.as_str() {
            "master" => "Master".into(),
            "return" => format!("return {}", self.index),
            _ => format!("track {}", self.index),
        }
    }

    /// How the tool names it when it has to ask again.
    pub(crate) fn as_argument(&self) -> String {
        match self.kind.as_str() {
            "master" => "\"master\"".into(),
            _ if !self.name.is_empty() => format!("\"{}\"", self.name),
            _ => self.index.to_string(),
        }
    }
}

/// "master" → the master; a return's name or letter → that return; a track's
/// name or index → that track. `kind` disambiguates a numeric index.
pub(crate) fn resolve_track_target(
    live: &LiveState,
    which: Option<&Value>,
    kind: Option<&str>,
    track_index: Option<i64>,
) -> Result<TrackTarget, String> {
    let given = kind.map(|k| k.trim().to_lowercase()).unwrap_or_default();
    if !given.is_empty() && given != "track" {
        if !matches!(given.as_str(), "return" | "master") {
            return Err(format!(
                "kind must be track, return or master, not '{given}'"
            ));
        }
        let index = which
            .and_then(Value::as_i64)
            .or(track_index)
            .unwrap_or(0)
            .max(0);
        return Ok(TrackTarget::at(
            &given,
            if given == "master" { 0 } else { index },
        ));
    }
    match which {
        None | Some(Value::Null) => match track_index {
            Some(i) => Ok(TrackTarget::at("track", i)),
            None => Err(
                "give track (a name, an index, \"master\", or a return's name or letter)".into(),
            ),
        },
        Some(Value::Number(n)) => Ok(TrackTarget::at("track", n.as_i64().unwrap_or(0))),
        Some(Value::String(s)) => resolve_track_name(live, s),
        Some(other) => Err(format!("a track is a name or an index, not {other}")),
    }
}

/// A name from the producer: a track, a return (by name or by Live's letter),
/// or the master.
fn resolve_track_name(live: &LiveState, given: &str) -> Result<TrackTarget, String> {
    let want = given.trim().to_lowercase();
    if want.is_empty() {
        return Err("give a track name, an index, \"master\", or a return".into());
    }
    if let Ok(i) = want.parse::<i64>() {
        return Ok(TrackTarget::at("track", i));
    }
    if matches!(
        want.as_str(),
        "master" | "main" | "master track" | "main track"
    ) {
        return Ok(TrackTarget {
            index: 0,
            kind: "master".into(),
            name: "Master".into(),
        });
    }
    let state = read_perf_state(live)?;
    if let Some(t) = state.tracks.iter().find(|t| t.name.to_lowercase() == want) {
        return Ok(TrackTarget {
            index: t.index,
            kind: "track".into(),
            name: t.name.clone(),
        });
    }
    // A role is a suffix on the track's name (`Sitar [lead]`), so "lead"
    // addresses the track — and the base name still does, whatever role it
    // was given since.
    for by in [
        |n: &str| crate::memory::split_role(n).1.unwrap_or_default(),
        |n: &str| crate::memory::split_role(n).0,
    ] {
        if let Some(t) = state
            .tracks
            .iter()
            .find(|t| by(&t.name).to_lowercase() == want)
        {
            return Ok(TrackTarget {
                index: t.index,
                kind: "track".into(),
                name: t.name.clone(),
            });
        }
    }
    let mut returns: Vec<(i64, String, String)> = Vec::new();
    if live.script.has_capability("get_returns") {
        if let Ok(r) = live.send_command("get_returns", None) {
            for (i, ret) in r
                .get("returns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let index = ret.get("index").and_then(Value::as_i64).unwrap_or(i as i64);
                let name = get_display(ret, "name", "");
                let letter = get_display(ret, "letter", "").to_lowercase();
                returns.push((index, name, letter));
            }
        }
    }
    for (index, name, letter) in &returns {
        let lower = name.to_lowercase();
        if lower == want
            || (!letter.is_empty() && (want == *letter || want == format!("return {letter}")))
        {
            return Ok(TrackTarget {
                index: *index,
                kind: "return".into(),
                name: name.clone(),
            });
        }
    }
    if let Some(t) = state
        .tracks
        .iter()
        .find(|t| t.name.to_lowercase().contains(&want))
    {
        return Ok(TrackTarget {
            index: t.index,
            kind: "track".into(),
            name: t.name.clone(),
        });
    }
    for (index, name, _) in &returns {
        if name.to_lowercase().contains(&want) {
            return Ok(TrackTarget {
                index: *index,
                kind: "return".into(),
                name: name.clone(),
            });
        }
    }
    Err(format!(
        "no track named '{given}'; tracks: {}{}{}. The master is \"master\".",
        state
            .tracks
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        if returns.is_empty() {
            ""
        } else {
            "; returns: "
        },
        returns
            .iter()
            .map(|(_, n, l)| format!("{n} ({})", l.to_uppercase()))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The Session clips on a track, as `(slot, name)` — only the slots that
/// hold one.
fn track_clips(
    live: &LiveState,
    target: &TrackTarget,
) -> Result<(String, Vec<(i64, String)>), String> {
    require(live, "get_track_info")?;
    let info = live
        .send_command(
            "get_track_info",
            Some(json!({"track_index": target.index, "kind": target.kind})),
        )
        .map_err(|e| live_err("read the track's clips", e))?;
    let clips = info
        .get("clip_slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(i, slot)| {
            let index = slot
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or(i as i64);
            let clip = slot.get("clip").filter(|c| c.is_object())?;
            Some((index, get_display(clip, "name", "")))
        })
        .collect();
    Ok((get_display(&info, "name", ""), clips))
}

/// A clip on a track: a Session slot by index, or a clip by its name.
///
/// A name that matches nothing is an error listing what the track holds. It
/// never falls back to an index: silently reading "Drop" as slot 0 is how a
/// producer overwrites the wrong clip.
pub(crate) fn resolve_clip_slot(
    live: &LiveState,
    target: &TrackTarget,
    which: Option<&Value>,
    clip_index: Option<i64>,
) -> Result<i64, String> {
    match which {
        None | Some(Value::Null) => clip_index
            .ok_or_else(|| "give clip (a clip's name, or a Session slot index)".to_string()),
        Some(Value::Number(n)) => Ok(n.as_i64().unwrap_or(0)),
        Some(Value::String(s)) => {
            let given = s.trim();
            if let Ok(i) = given.parse::<i64>() {
                return Ok(i);
            }
            resolve_clip_name(live, target, given)
        }
        Some(other) => Err(format!("a clip is a name or a slot index, not {other}")),
    }
}

/// A clip name against one track's slots: exact first, then a substring.
/// Two matches name both and change nothing.
fn resolve_clip_name(live: &LiveState, target: &TrackTarget, given: &str) -> Result<i64, String> {
    let want = given.to_lowercase();
    if want.is_empty() {
        return Err("give a clip name or a Session slot index".into());
    }
    let (track_name, clips) = track_clips(live, target)?;
    let label = target.label(&track_name);
    let listing = || -> String {
        if clips.is_empty() {
            format!("{label} holds no clips")
        } else {
            format!(
                "{label} holds: {}",
                clips
                    .iter()
                    .map(|(slot, name)| format!("'{name}' (slot {slot})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    for exact in [true, false] {
        let hits: Vec<&(i64, String)> = clips
            .iter()
            .filter(|(_, name)| {
                let lower = name.to_lowercase();
                if exact {
                    lower == want
                } else {
                    lower.contains(&want)
                }
            })
            .collect();
        match hits.len() {
            0 => continue,
            1 => return Ok(hits[0].0),
            _ => {
                return Err(format!(
                    "'{given}' matches {} clips on {label}: {}. Say the slot index, or rename one.",
                    hits.len(),
                    hits.iter()
                        .map(|(slot, name)| format!("'{name}' (slot {slot})"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
    Err(format!("no clip named '{given}'; {}.", listing()))
}

/// Live's locators, as `(name, beat)`, read through the generic ops layer:
/// one batch for the count, one for the names and times. No new command.
pub(crate) fn locators(live: &LiveState) -> Result<Vec<(String, f64)>, String> {
    use crate::lom::{Batch, Op, Path};
    crate::lom::require(live).map_err(|e| live_err("read the locators", e))?;
    let cues = Path::song().attr("cue_points");
    // The script renders a Live vector as a list of strings, so the first
    // batch is only ever asked for how many there are.
    let count = Batch::new()
        .push(Op::get(&cues, "cues"))
        .run(live)?
        .get("cues")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut batch = Batch::new();
    for i in 0..count as i64 {
        let cue = cues.clone().at(i);
        batch = batch
            .push(Op::get(&cue.clone().attr("name"), &format!("n{i}")))
            .push(Op::get(&cue.attr("time"), &format!("t{i}")));
    }
    let out = batch.run(live)?;
    Ok((0..count as i64)
        .filter_map(|i| {
            let name = out.get(&format!("n{i}"))?.as_str()?.to_string();
            let time = out.get(&format!("t{i}")).and_then(Value::as_f64)?;
            Some((name, time))
        })
        .collect())
}

/// A bar the caller named: a number is Live's 1-based bar, and a word is a
/// locator — `create_locator` already makes them and Live's Save keeps them.
pub(crate) fn resolve_bar(
    live: &LiveState,
    beats_per_bar: f64,
    which: Option<&Value>,
) -> Result<Option<f64>, String> {
    let given = match which {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(n)) => return Ok(n.as_f64()),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(other) => return Err(format!("a bar is a number or a locator name, not {other}")),
    };
    if let Ok(n) = given.parse::<f64>() {
        return Ok(Some(n));
    }
    if given.is_empty() {
        return Err("give a bar number or a locator name".into());
    }
    let marks = locators(live)?;
    let want = given.to_lowercase();
    for exact in [true, false] {
        let hits: Vec<&(String, f64)> = marks
            .iter()
            .filter(|(name, _)| {
                let lower = name.to_lowercase();
                if exact {
                    lower == want
                } else {
                    lower.contains(&want)
                }
            })
            .collect();
        match hits.len() {
            0 => continue,
            1 => return Ok(Some(hits[0].1 / beats_per_bar + 1.0)),
            _ => {
                return Err(format!(
                    "'{given}' matches {} locators: {}. Say the bar, or rename one.",
                    hits.len(),
                    hits.iter()
                        .map(|(name, beat)| format!(
                            "'{name}' (bar {})",
                            crate::arrange::fmt_bar(beat / beats_per_bar + 1.0)
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
    Err(format!(
        "no locator named '{given}'; {}. create_locator makes one.",
        if marks.is_empty() {
            "the set has none".to_string()
        } else {
            format!(
                "the set has: {}",
                marks
                    .iter()
                    .map(|(name, beat)| format!(
                        "'{name}' (bar {})",
                        crate::arrange::fmt_bar(beat / beats_per_bar + 1.0)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    ))
}

/// The devices on a track, a return or the master.
fn track_devices(live: &LiveState, target: &TrackTarget) -> Result<(String, Vec<Value>), String> {
    require(live, "get_track_info")?;
    let info = live
        .send_command(
            "get_track_info",
            Some(json!({"track_index": target.index, "kind": target.kind})),
        )
        .map_err(|e| live_err("read the track's devices", e))?;
    let devices = info
        .get("devices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok((get_display(&info, "name", ""), devices))
}

/// A device by name (a substring is enough) or by index.
fn resolve_device_index(
    live: &LiveState,
    target: &TrackTarget,
    which: Option<&Value>,
    device_index: Option<i64>,
) -> Result<i64, String> {
    let name = match which {
        None | Some(Value::Null) => return Ok(device_index.unwrap_or(0)),
        Some(Value::Number(n)) => return Ok(n.as_i64().unwrap_or(0)),
        Some(Value::String(s)) => match s.trim().parse::<i64>() {
            Ok(i) => return Ok(i),
            Err(_) => s.trim().to_lowercase(),
        },
        Some(other) => return Err(format!("a device is a name or an index, not {other}")),
    };
    let (track_name, devices) = track_devices(live, target)?;
    let named = |d: &Value| get_display(d, "name", "").to_lowercase();
    let found = devices
        .iter()
        .find(|d| named(d) == name)
        .or_else(|| devices.iter().find(|d| named(d).contains(&name)));
    match found {
        Some(d) => Ok(d.get("index").and_then(Value::as_i64).unwrap_or(0)),
        None if devices.is_empty() => Err(format!("{} has no devices", target.label(&track_name))),
        None => Err(format!(
            "no device matching '{name}' on {}; its devices: {}",
            target.label(&track_name),
            devices
                .iter()
                .map(|d| get_display(d, "name", "?"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Both halves of a device address, resolved in as few round trips as the
/// caller's words allow.
fn resolve_device(
    live: &LiveState,
    track: Option<&Value>,
    kind: Option<&str>,
    track_index: Option<i64>,
    device: Option<&Value>,
    device_index: Option<i64>,
) -> Result<(TrackTarget, i64), String> {
    let target = resolve_track_target(live, track, kind, track_index)?;
    let di = resolve_device_index(live, &target, device, device_index)?;
    Ok((target, di))
}

pub fn get_device_parameters_body(live: &LiveState, p: &DeviceParams) -> ToolResult {
    require(live, "get_device_parameters")?;
    let (target, device_index) = resolve_device(
        live,
        p.track.as_ref(),
        p.kind.as_deref(),
        p.track_index,
        p.device.as_ref(),
        p.device_index,
    )?;
    let r = live
        .send_command(
            "get_device_parameters",
            Some(json!({"track_index": target.index, "kind": target.kind, "device_index": device_index})),
        )
        .map_err(|e| live_err("get device parameters", e))?;
    note_device_read(live, &r);
    Ok(render_device(&target, &r))
}

/// Feed the device vocabulary from a `get_device_parameters` reply (#67).
/// Nothing extra is asked of Live: this is the read that already happened.
fn note_device_read(live: &LiveState, r: &Value) {
    let d = r.get("device").cloned().unwrap_or(Value::Null);
    let params: Vec<crate::sound::Param> = d
        .get("parameters")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(crate::sound::Param::from_value)
                .collect()
        })
        .unwrap_or_default();
    live.devices.note_device(
        crate::devices::DeviceRef {
            live_version: &live.live_version(),
            device: &get_display(&d, "name", ""),
            class_name: &get_display(&d, "class_name", ""),
        },
        &params,
    );
}

/// The readout: an aligned table of what Live shows, not the raw JSON. A
/// model that reads "Low Cut 48 dB" does not have to guess what 1 means.
fn render_device(target: &TrackTarget, r: &Value) -> String {
    let d = r.get("device").cloned().unwrap_or(Value::Null);
    let params: Vec<crate::sound::Param> = d
        .get("parameters")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(crate::sound::Param::from_value)
                .collect()
        })
        .unwrap_or_default();
    let where_ = target.label(&get_display(r, "track_name", ""));
    let class = get_display(&d, "class_name", "");
    let mut out = format!(
        "{where_} · device {} '{}'{} — {} parameter{}\n",
        get_display(&d, "index", "?"),
        get_display(&d, "name", "device"),
        if class.is_empty() {
            String::new()
        } else {
            format!(" ({class})")
        },
        params.len(),
        if params.len() == 1 { "" } else { "s" }
    );
    if params.is_empty() {
        out.push_str("This device exposes no parameters.\n");
        return out;
    }
    let name_w = params
        .iter()
        .map(|q| q.name.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 28);
    let now_w = params
        .iter()
        .map(|q| q.display().chars().count())
        .max()
        .unwrap_or(5)
        .clamp(5, 22);
    out.push_str(&format!(
        "  idx  {:<name_w$}  {:<now_w$}  range\n",
        "name", "now"
    ));
    for q in &params {
        out.push_str(&format!(
            "  {:>3}  {:<name_w$}  {:<now_w$}  {}{}\n",
            q.index,
            ellipsis(&q.name, name_w),
            ellipsis(&q.display(), now_w),
            q.range_text(),
            if q.enabled {
                ""
            } else {
                "  (not directly settable)"
            }
        ));
    }
    if let Some(chains) = d.get("chains").and_then(Value::as_array) {
        let inner: Vec<String> = chains
            .iter()
            .map(|c| {
                format!(
                    "{} ({})",
                    get_display(c, "name", "chain"),
                    c.get("devices")
                        .and_then(Value::as_array)
                        .map(|ds| ds
                            .iter()
                            .map(|x| get_display(x, "name", "?"))
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                )
            })
            .collect();
        if !inner.is_empty() {
            out.push_str(&format!("Chains: {}.\n", inner.join(" · ")));
        }
    }
    out.push_str(&format!(
        "Set one by what it reads: adv_set_device_parameter {{\"track\": {}, \"device\": \"{}\", \"parameter\": \"{}\", \"value\": \"{}\"}} — the raw number works too.",
        target.as_argument(),
        get_display(&d, "name", "device"),
        params.first().map(|q| q.name.clone()).unwrap_or_default(),
        params.first().map(|q| q.display()).unwrap_or_default()
    ));
    out
}

fn ellipsis(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One device's parameters, as [`crate::sound::Param`]s, with its name and class.
fn device_params(
    live: &LiveState,
    target: &TrackTarget,
    device_index: i64,
) -> Result<(String, String, String, Vec<crate::sound::Param>), String> {
    require(live, "get_device_parameters")?;
    let r = live
        .send_command(
            "get_device_parameters",
            Some(json!({"track_index": target.index, "kind": target.kind, "device_index": device_index})),
        )
        .map_err(|e| live_err("read the device's parameters", e))?;
    let d = r.get("device").cloned().unwrap_or(Value::Null);
    let params: Vec<crate::sound::Param> = d
        .get("parameters")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(crate::sound::Param::from_value)
                .collect()
        })
        .unwrap_or_default();
    let name = get_display(&d, "name", "device");
    let class = get_display(&d, "class_name", "");
    // #67: the read already happened. What this device is called and what
    // its parameters are is a fact about Ableton's content, not about this
    // song, so it is kept keyed on the device and the Live version.
    live.devices.note_device(
        crate::devices::DeviceRef {
            live_version: &live.live_version(),
            device: &name,
            class_name: &class,
        },
        &params,
    );
    Ok((get_display(&r, "track_name", ""), name, class, params))
}

/// The device `shape_sound` and a sound ramp mean: by name or index, else
/// the first instrument, rack or drum rack on the track.
fn pick_device(
    live: &LiveState,
    target: &TrackTarget,
    which: Option<&Value>,
) -> Result<(i64, String), String> {
    let (track_name, devices) = track_devices(live, target)?;
    if devices.is_empty() {
        return Err(format!(
            "{} has no devices to shape",
            target.label(&track_name)
        ));
    }
    let names: Vec<String> = devices
        .iter()
        .map(|d| get_display(d, "name", "?"))
        .collect();
    let found = match which {
        Some(Value::Number(n)) => devices
            .iter()
            .find(|d| d.get("index").and_then(Value::as_i64) == n.as_i64()),
        Some(Value::String(s)) => {
            let want = s.trim().to_lowercase();
            devices
                .iter()
                .find(|d| get_display(d, "name", "").to_lowercase() == want)
                .or_else(|| {
                    devices
                        .iter()
                        .find(|d| get_display(d, "name", "").to_lowercase().contains(&want))
                })
        }
        Some(other) => return Err(format!("device must be a name or an index, not {other}")),
        None => devices
            .iter()
            .find(|d| {
                matches!(
                    d.get("type").and_then(Value::as_str),
                    Some("instrument" | "rack" | "drum_machine")
                )
            })
            .or_else(|| devices.first()),
    };
    let d = found.ok_or_else(|| {
        format!(
            "no device {} on this track; its devices: {}",
            which.map(display).unwrap_or_default(),
            names.join(", ")
        )
    })?;
    Ok((
        d.get("index").and_then(Value::as_i64).unwrap_or(0),
        get_display(d, "name", "device"),
    ))
}

/// The parameter names a reply can carry: enough to steer by, then a pointer
/// to the full list. An EQ Eight has 84 of them.
fn name_list(params: &[crate::sound::Param]) -> String {
    let shown: Vec<String> = params.iter().take(24).map(|q| q.name.clone()).collect();
    if params.len() > shown.len() {
        format!(
            "{}, … ({} in all — adv_get_device_parameters lists them)",
            shown.join(", "),
            params.len()
        )
    } else {
        shown.join(", ")
    }
}

pub fn shape_sound_body(live: &LiveState, p: &ShapeSoundParams) -> ToolResult {
    require(live, "set_device_parameters")?;
    let given: Vec<(&str, &Value)> = [
        ("cutoff", &p.cutoff),
        ("resonance", &p.resonance),
        ("attack", &p.attack),
        ("decay", &p.decay),
        ("sustain", &p.sustain),
        ("release", &p.release),
        ("drive", &p.drive),
        ("detune", &p.detune),
        ("width", &p.width),
        ("lfo_rate", &p.lfo_rate),
        ("reverb", &p.reverb),
        ("delay", &p.delay),
    ]
    .into_iter()
    .filter_map(|(w, v)| v.as_ref().filter(|v| !v.is_null()).map(|v| (w, v)))
    .collect();
    if given.is_empty() && p.set.is_empty() {
        return Err(format!(
            "Give at least one word: {} (a fraction 0–1 of the range, or \"±N%\"), or set: {{\"<parameter name>\": value}}.",
            crate::sound::WORDS.join(", ")
        ));
    }
    let target = resolve_track_target(live, Some(&p.track), p.kind.as_deref(), None)?;
    let (di, dname) = pick_device(live, &target, p.device.as_ref())?;
    let (track_name, _, class, params) = device_params(live, &target, di)?;
    let track_name = target.label(&track_name);
    let rack = crate::sound::is_rack(&class);
    let mut values: Vec<Value> = Vec::new();
    let mut resolved: Vec<(String, crate::sound::Param, crate::sound::Via, f64)> = Vec::new();
    let mut unresolved: Vec<&str> = Vec::new();
    for (word, v) in &given {
        match crate::sound::resolve(word, &class, &params) {
            Some((param, via)) => {
                let target = crate::sound::target_value(param, v)?;
                values.push(json!({"index": param.index, "value": target}));
                resolved.push((word.to_string(), param.clone(), via, target));
            }
            None => unresolved.push(word),
        }
    }
    for (name, v) in &p.set {
        match crate::sound::by_name(name, &params) {
            Some(param) => {
                let target = crate::sound::target_value(param, v)?;
                values.push(json!({"index": param.index, "value": target}));
                resolved.push((name.clone(), param.clone(), crate::sound::Via::Name, target));
            }
            None => {
                return Err(format!(
                    "no parameter named '{name}' on '{dname}'; its parameters: {}",
                    name_list(&params)
                ))
            }
        }
    }
    let param_names = || -> String { name_list(&params) };
    // #67: this device has been asked before, possibly in another song and
    // another session. What it refused then is worth saying before refusing
    // it again, and what was learned now is worth keeping.
    let version = live.live_version();
    let this_device = crate::devices::DeviceRef {
        live_version: &version,
        device: &dname,
        class_name: &class,
    };
    let known = live.devices.recall(this_device);
    let recalled: Vec<String> = known
        .as_ref()
        .map(|facts| {
            unresolved
                .iter()
                .filter_map(|w| crate::devices::advice(facts, w))
                .collect()
        })
        .unwrap_or_default();
    for word in &unresolved {
        live.devices.note_unknown_word(this_device, word);
    }
    if resolved.is_empty() {
        let mut text = format!(
            "{track_name} ('{dname}', {class}) is not in the vocabulary for {}; its parameters by name are: {}. adv_set_device_parameter takes a name substring: {{\"track\": {}, \"device\": \"{dname}\", \"parameter\": \"<name>\", \"value\": …}}.",
            unresolved.join(", "),
            param_names(),
            target.as_argument(),
        );
        if !recalled.is_empty() {
            text.push_str(&format!(" {}", recalled.join(" ")));
        }
        return Err(text);
    }
    let r = live
        .send_command(
            "set_device_parameters",
            Some(json!({"track_index": target.index, "kind": target.kind, "device_index": di, "values": values})),
        )
        .map_err(|e| live_err("set the parameters", e))?;
    let written: Vec<Value> = r
        .get("parameters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Every word asked for gets a line, applied or skipped: a reply that
    // reads as success while a word was dropped is how a partial change is
    // reported as a whole one.
    let mut lines: Vec<String> = Vec::new();
    let mut applied = 0usize;
    for (word, param, via, _) in &resolved {
        let after = written
            .iter()
            .find(|w| w.get("index").and_then(Value::as_i64) == Some(param.index));
        let new_display = after
            .and_then(crate::sound::Param::from_value)
            .map(|q| q.display())
            .unwrap_or_else(|| "?".into());
        // A word Live ignored is skipped, not applied: the same rule as
        // adv_set_device_parameter, one layer up.
        match after.map(|a| landing(a, &param.name, &new_display)) {
            Some(Err(reason)) => lines.push(format!("  skipped  {word} — {reason}")),
            landed => {
                applied += 1;
                lines.push(format!(
                    "  applied  {}'{}' {} → {new_display}  ({word}){}",
                    match via {
                        crate::sound::Via::Macro => "macro ",
                        _ => "",
                    },
                    param.name,
                    param.display(),
                    match landed {
                        Some(Ok(Some(note))) => note,
                        _ => String::new(),
                    }
                ));
            }
        }
    }
    for word in &unresolved {
        lines.push(format!(
            "  skipped  {word} — no {} on this device says it",
            if rack { "macro" } else { "parameter" }
        ));
    }
    // The read-back already happened; what came back is what is kept.
    for (_, param, _, _) in &resolved {
        if let Some(after) = written
            .iter()
            .find(|w| w.get("index").and_then(Value::as_i64) == Some(param.index))
        {
            live.devices.note_write(
                this_device,
                param.index,
                &param.name,
                after
                    .get("value")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                &crate::sound::Param::from_value(after)
                    .map(|q| q.display())
                    .unwrap_or_default(),
            );
        }
    }
    // A macro that runs the other way is reported, never silently corrected:
    // a server that quietly inverts a value is one the producer cannot
    // reconcile with what Live shows them.
    let mut backwards: Vec<String> = Vec::new();
    if let Some(facts) = live.devices.recall(this_device) {
        for (_, param, _, _) in &resolved {
            if let Some(said) = facts
                .parameter(&param.name)
                .and_then(crate::devices::backwards)
            {
                backwards.push(said);
            }
        }
    }
    let vocab = crate::sound::vocabulary(&class, &params);
    let mut text = format!(
        "{track_name} ({}'{dname}'{}){}:\n{}",
        if rack { "rack " } else { "" },
        if class.is_empty() {
            String::new()
        } else {
            format!(", {class}")
        },
        if applied == resolved.len() + unresolved.len() {
            String::new()
        } else {
            format!(
                " — {applied} of {} applied",
                resolved.len() + unresolved.len()
            )
        },
        lines.join("\n")
    );
    for said in &backwards {
        text.push_str(&format!("\n{said}"));
    }
    for said in &recalled {
        text.push_str(&format!("\n{said}"));
    }
    if !unresolved.is_empty() {
        text.push_str(&format!(
            "\nFor {}: {}adv_set_device_parameter takes a name substring; the parameters are: {}.",
            unresolved.join(", "),
            if rack {
                "a rack's own chain devices are reached through their macros, or "
            } else {
                ""
            },
            param_names()
        ));
    }
    if rack {
        text.push_str("\nRacks are asked first: their macros are how the preset's maker meant it to be shaped.");
    }
    text.push_str(&format!(
        "\nWords this device answers to: {}. Sweep one in a cue: {{\"ramp\": {{\"sound\": \"{}\", \"track\": \"{}\", \"to\": 0.8}}, \"bars\": 8}}.",
        vocab
            .iter()
            .map(|(w, n)| format!("{w} ({n})"))
            .collect::<Vec<_>>()
            .join(", "),
        vocab.first().map(|(w, _)| w.as_str()).unwrap_or("cutoff"),
        track_name
    ));
    Ok(text)
}

/// A cue step's `ramp {"sound": word, "track": …, "device"?: …, "to": 0–1}`
/// resolved to the device parameter it means; other steps pass through.
fn resolve_sound_ramps(
    live: &LiveState,
    state: &PerfState,
    p: &CueParams,
) -> Result<CueParams, String> {
    let mut out = p.clone();
    for (i, step) in out.steps.iter_mut().enumerate() {
        let Some(ramp) = step.ramp.as_ref().and_then(Value::as_object).cloned() else {
            continue;
        };
        let Some(word) = ramp.get("sound").and_then(Value::as_str) else {
            continue;
        };
        let n = i + 1;
        let which = ramp
            .get("track")
            .ok_or_else(|| format!("step {n}: a sound ramp needs \"track\""))?;
        let track = state
            .track_by(which)
            .map_err(|e| format!("step {n}: {e}"))?;
        let to = ramp.get("to").and_then(Value::as_f64).ok_or_else(|| {
            format!("step {n}: a sound ramp needs \"to\" (0–1 of the parameter's range)")
        })?;
        if !(0.0..=1.0).contains(&to) {
            return Err(format!("step {n}: to must be between 0 and 1, got {to}"));
        }
        let on_track = TrackTarget::at("track", track.index);
        let (di, dname) = pick_device(live, &on_track, ramp.get("device"))
            .map_err(|e| format!("step {n}: {e}"))?;
        let (_, _, class, params) =
            device_params(live, &on_track, di).map_err(|e| format!("step {n}: {e}"))?;
        let (param, _) = crate::sound::resolve(word, &class, &params).ok_or_else(|| {
            format!(
                "step {n}: '{word}' is not in the vocabulary for '{dname}' ({class}); its parameters: {}",
                params.iter().map(|q| q.name.clone()).collect::<Vec<_>>().join(", ")
            )
        })?;
        step.ramp = Some(
            json!({"target": "device", "track": track.index, "device_index": di,
                                "parameter_index": param.index, "to": param.at_fraction(to)}),
        );
    }
    Ok(out)
}

/// What Live did with a parameter write, read off the reply the script sends
/// back: `asked`, `landed`, `is_enabled`, `is_quantized`, `automation_state`.
///
/// `Err` is a write that did nothing — the step counted as done and was not.
/// `Ok(Some(note))` is a write that moved but not to the number asked for:
/// a quantized parameter snapping to its nearest step, which is a success
/// the producer should see rather than discover by ear. `Ok(None)` is a
/// clean landing, and also what an older script gets, since it sends no
/// `landed` and nothing may be assumed about what it did.
fn landing(r: &Value, name: &str, shown_after: &str) -> Result<Option<String>, String> {
    if r.get("landed").and_then(Value::as_bool) != Some(false) {
        return Ok(None);
    }
    let asked = short_num(r.get("asked"));
    let now = if shown_after.is_empty() {
        short_num(r.get("value"))
    } else {
        shown_after.to_string()
    };
    if r.get("is_quantized").and_then(Value::as_bool) == Some(true) {
        let moved =
            r.get("old_value").and_then(Value::as_f64) != r.get("value").and_then(Value::as_f64);
        return Ok(Some(format!(
            " {name} takes steps: {asked} is nearest {now}{}.",
            if moved {
                ""
            } else {
                ", the step it was already on"
            }
        )));
    }
    if r.get("is_enabled").and_then(Value::as_bool) == Some(false) {
        return Err(format!(
            "'{name}' did not move; it is still {now}. Live has it switched off (is_enabled false): a rack macro owns it, or its device has that parameter disabled. Move the macro that maps to it, or enable it in Live."
        ));
    }
    if let Some(state) = r.get("automation_state").and_then(Value::as_i64) {
        if state != 0 {
            return Err(format!(
                "'{name}' did not move; it is still {now}. It is automated (automation_state {state}), and the envelope overwrites a manual write on the next playback tick. Clear or bypass the envelope in Live, or write the envelope itself with adv_set_clip_automation."
            ));
        }
    }
    Err(format!(
        "'{name}' did not move; Live left it at {now} after being asked for {asked}. Nothing in the reply says why: check in Live whether the parameter is mapped, frozen or owned by a chain."
    ))
}

pub fn set_device_parameter_body(live: &LiveState, p: &SetDeviceParameterParams) -> ToolResult {
    require(live, "set_device_parameter")?;
    let (target, device_index) = resolve_device(
        live,
        p.track.as_ref(),
        p.kind.as_deref(),
        p.track_index,
        p.device.as_ref(),
        p.device_index,
    )?;
    let parameter_index = match (p.parameter_index, p.parameter.as_deref()) {
        (Some(i), _) => i,
        (None, Some(name)) => {
            let (_, dname, _, params) = device_params(live, &target, device_index)?;
            crate::sound::by_name(name, &params)
                .map(|q| q.index)
                .ok_or_else(|| {
                    format!(
                        "no parameter named '{name}' on '{dname}'; its parameters: {}",
                        name_list(&params)
                    )
                })?
        }
        (None, None) => {
            return Err("give parameter_index, or parameter (a name or a substring of one)".into())
        }
    };
    // A value may be what Live shows ("3 dB", "Low Cut 48 dB") or the raw
    // number; the script resolves a display string through Live's own strings.
    let mut args = json!({
        "track_index": target.index,
        "kind": target.kind,
        "device_index": device_index,
        "parameter_index": parameter_index,
    });
    match &p.value {
        Value::String(text) if text.trim().is_empty() => {
            return Err("give value: what Live shows (\"200 Hz\") or the raw number".into())
        }
        Value::String(text) => {
            args["value_display"] = json!(text.trim());
        }
        Value::Number(n) => {
            args["value"] = json!(n.as_f64().unwrap_or_default());
        }
        Value::Null => {
            return Err("give value: what Live shows (\"200 Hz\") or the raw number".into())
        }
        other => return Err(format!("value must be a number or a string, not {other}")),
    }
    let r = live
        .send_command("set_device_parameter", Some(args))
        .map_err(|e| live_err("set device parameter", e))?;
    let before = get_display(&r, "old_display", "");
    let after = get_display(&r, "display", "");
    let raw = format!(
        "raw {} → {} of {} … {}",
        short_num(r.get("old_value")),
        short_num(r.get("value")),
        short_num(r.get("min")),
        short_num(r.get("max"))
    );
    let name = get_display(&r, "name", "parameter");
    // #67: an inverted or non-monotonic macro is not discoverable from the
    // parameter list; it is only learnable by writing a value and reading
    // the display back, which just happened.
    live.devices.note_write(
        crate::devices::DeviceRef {
            live_version: &live.live_version(),
            device: &get_display(&r, "device", ""),
            class_name: &get_display(&r, "class_name", ""),
        },
        parameter_index,
        &name,
        r.get("value").and_then(Value::as_f64).unwrap_or_default(),
        &after,
    );
    // A write Live ignored is an error, not a success line with the same
    // number on both sides of the arrow.
    let note = landing(&r, &name, &after)?;
    Ok(format!(
        "{} · {} · {}: {} → {} ({raw}).{}",
        target.label(&get_display(&r, "track_name", "")),
        get_display(&r, "device", "device"),
        name,
        if before.is_empty() {
            short_num(r.get("old_value"))
        } else {
            before
        },
        if after.is_empty() {
            short_num(r.get("value"))
        } else {
            after
        },
        note.unwrap_or_default()
    ))
}

/// `0.542`, `-36`: a number a person reads, from whatever Live sent.
fn short_num(v: Option<&Value>) -> String {
    match v.and_then(Value::as_f64) {
        Some(n) => {
            let s = format!("{n:.3}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        None => "?".into(),
    }
}

/// Remove, move, bypass or re-enable a device — on a track, a return or the
/// master. Without this a wrong load is permanent for the rest of the session.
pub fn edit_devices_body(live: &LiveState, p: &EditDevicesParams) -> ToolResult {
    let action = p.action.trim().to_lowercase();
    let (target, device_index) = resolve_device(
        live,
        p.track.as_ref(),
        p.kind.as_deref(),
        p.track_index,
        p.device.as_ref(),
        p.device_index,
    )?;
    match action.as_str() {
        "remove" | "delete" => {
            require(live, "delete_device")?;
            let r = live
                .send_command(
                    "delete_device",
                    Some(json!({"track_index": target.index, "kind": target.kind, "device_index": device_index})),
                )
                .map_err(|e| live_err("remove the device", e))?;
            Ok(format!(
                "Removed '{}' (was device {device_index}) from {}. {} Cmd-Z in Live puts it back.",
                get_display(&r, "deleted", "the device"),
                target.label(&get_display(&r, "track_name", "")),
                chain_line(&r)
            ))
        }
        "move" => {
            require(live, "move_device")?;
            let to = p.to_index.ok_or(
                "move needs to_index: where in the chain the device should end up (0 is first)",
            )?;
            let r = live
                .send_command(
                    "move_device",
                    Some(json!({"track_index": target.index, "kind": target.kind, "device_index": device_index, "to_index": to})),
                )
                .map_err(|e| live_err("move the device", e))?;
            Ok(format!(
                "Moved '{}' on {} from {device_index} to {}. {}",
                get_display(&r, "moved", "the device"),
                target.label(&get_display(&r, "track_name", "")),
                get_display(&r, "to_index", "?"),
                chain_line(&r)
            ))
        }
        "bypass" | "off" | "disable" | "enable" | "on" => {
            let off = matches!(action.as_str(), "bypass" | "off" | "disable");
            require(live, "set_device_parameter")?;
            let (track_name, dname, _, params) = device_params(live, &target, device_index)?;
            let switch = params
                .iter()
                .find(|q| q.name.eq_ignore_ascii_case("Device On"))
                .ok_or_else(|| {
                    format!("'{dname}' has no Device On switch, so it cannot be bypassed")
                })?;
            let value = if off { switch.min } else { switch.max };
            live.send_command(
                "set_device_parameter",
                Some(json!({
                    "track_index": target.index, "kind": target.kind,
                    "device_index": device_index, "parameter_index": switch.index,
                    "value": value,
                })),
            )
            .map_err(|e| live_err("switch the device", e))?;
            Ok(format!(
                "{} '{dname}' on {} (device {device_index}) — {}.{}",
                if off { "Bypassed" } else { "Enabled" },
                target.label(&track_name),
                if off {
                    "still in the chain, not processing"
                } else {
                    "processing again"
                },
                if off {
                    format!(
                        " Turn it back on with {{\"track\": {}, \"device\": \"{dname}\", \"action\": \"enable\"}}.",
                        target.as_argument()
                    )
                } else {
                    String::new()
                }
            ))
        }
        "" => Err("action must be remove, move (with to_index), bypass or enable".into()),
        other => Err(format!(
            "action must be remove, move (with to_index), bypass or enable, not '{other}'"
        )),
    }
}

/// "Chain: EQ Eight, Glue Compressor, Limiter." from a device reply.
fn chain_line(r: &Value) -> String {
    match r.get("devices").and_then(Value::as_array) {
        Some(d) if !d.is_empty() => format!(
            "Chain: {}.",
            d.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(_) => "The chain is empty now.".into(),
        None => String::new(),
    }
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

/// Create a track and return its index and name; the index is what every
/// following call needs and what `create_*_track` reports.
pub fn create_track(
    live: &LiveState,
    kind: &str,
    index: i64,
) -> Result<(Option<i64>, String), String> {
    let command = if kind == "audio" {
        "create_audio_track"
    } else {
        "create_midi_track"
    };
    require(live, command)?;
    let r = live
        .send_command(command, Some(json!({"index": index})))
        .map_err(|e| live_err(&format!("create {kind} track"), e))?;
    Ok((
        r.get("index").and_then(Value::as_i64),
        get_display(&r, "name", "unnamed"),
    ))
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
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    // A slot by number, or the name of a clip already on this track. A name
    // that matches nothing is an error, not slot 0.
    let slot = resolve_clip_slot(live, &target, p.clip.as_ref(), p.clip_index)?;
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
        Some(json!({"track_index": target.index, "clip_index": slot, "length": p.length})),
    )
    .map_err(|e| live_err("create clip", e))?;
    let mut text = format!(
        "Created clip at track {}, slot {slot} with length {} beats",
        target.index, p.length
    );
    if !p.name.is_empty() {
        live.send_command(
            "set_clip_name",
            Some(json!({"track_index": target.index, "clip_index": slot, "name": p.name})),
        )
        .map_err(|e| live_err("name the new clip", e))?;
        text.push_str(&format!(", named '{}'", p.name));
    }
    if !notes.is_empty() {
        live.send_command(
            "add_notes_to_clip",
            Some(json!({"track_index": target.index, "clip_index": slot, "notes": notes})),
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
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    let slot = resolve_clip_slot(live, &target, p.clip.as_ref(), p.clip_index)?;
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
                Some(json!({"track_index": target.index, "clip_index": slot})),
            )
            .map_err(|e| live_err("clear notes from clip", e))?;
        cleared = format!(
            " (cleared {} first)",
            get_display(&r, "cleared_count", "the old notes")
        );
    }
    if notes.is_empty() {
        return Ok(format!(
            "Cleared clip at track {}, slot {slot}{cleared}",
            target.index
        ));
    }
    live.send_command(
        "add_notes_to_clip",
        Some(json!({"track_index": target.index, "clip_index": slot, "notes": notes})),
    )
    .map_err(|e| live_err("add notes to clip", e))?;
    let last = notes
        .iter()
        .map(|n| n.start_time + n.duration)
        .fold(0.0_f64, f64::max);
    let mut text = format!(
        "Added {} notes to clip at track {}, slot {slot}{cleared} — last note ends at beat {last}",
        notes.len(),
        target.index,
    );
    if p.propagate_to_arrangement {
        text.push_str(&propagate_to_arrangement(live, target.index, slot)?);
    }
    Ok(text)
}

/// Arrangement copies of a Session clip do not follow edits to it (Live's
/// rule). Find the copies by name, remove them, and place the clip again at
/// the same beats.
fn propagate_to_arrangement(
    live: &LiveState,
    track_index: i64,
    clip_index: i64,
) -> Result<String, String> {
    for cmd in [
        "get_clip_info",
        "get_arrangement_clips",
        "delete_arrangement_clip",
        "duplicate_session_clip_to_arrangement",
    ] {
        require(live, cmd)?;
    }
    let info = live
        .send_command(
            "get_clip_info",
            Some(
                json!({"track_index": track_index, "clip_index": clip_index, "arrangement": false}),
            ),
        )
        .map_err(|e| live_err("read the clip", e))?;
    let name = get_display(&info, "name", "");
    let clips = live
        .send_command(
            "get_arrangement_clips",
            Some(json!({"track_index": track_index})),
        )
        .map_err(|e| live_err("list the arrangement clips", e))?;
    let copies: Vec<(usize, f64)> = clips
        .get("clips")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .filter(|(_, c)| get_display(c, "name", "") == name)
                .filter_map(|(i, c)| Some((i, c.get("start_time")?.as_f64()?)))
                .collect()
        })
        .unwrap_or_default();
    if copies.is_empty() {
        return Ok(format!(
            ". No Arrangement copies named '{name}' on this track to refresh"
        ));
    }
    for (i, _) in copies.iter().rev() {
        live.send_command(
            "delete_arrangement_clip",
            Some(json!({"track_index": track_index, "clip_index": i})),
        )
        .map_err(|e| live_err("remove an arrangement copy", e))?;
    }
    for (_, t) in &copies {
        live.send_command(
            "duplicate_session_clip_to_arrangement",
            Some(json!({"track_index": track_index, "clip_index": clip_index, "destination_time": t})),
        )
        .map_err(|e| live_err("place the refreshed clip", e))?;
    }
    let times: Vec<f64> = copies.iter().map(|(_, t)| *t).collect();
    Ok(format!(
        ". Refreshed {} Arrangement cop{} of '{name}' at beat(s) {}",
        copies.len(),
        if copies.len() == 1 { "y" } else { "ies" },
        list_beats(&times)
    ))
}

pub fn clear_notes_from_clip_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "clear_notes_from_clip")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    let slot = resolve_clip_slot(live, &target, p.clip.as_ref(), p.clip_index)?;
    let r = live
        .send_command(
            "clear_notes_from_clip",
            Some(json!({"track_index": target.index, "clip_index": slot})),
        )
        .map_err(|e| live_err("clear notes from clip", e))?;
    Ok(format!(
        "Cleared {} note(s) from clip '{}' (track {}, slot {})",
        get_display(&r, "cleared_count", "?"),
        get_display(&r, "clip_name", "clip"),
        target.index,
        slot
    ))
}

pub fn set_clip_name_body(live: &LiveState, p: &SetClipNameParams) -> ToolResult {
    require(live, "set_clip_name")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    let slot = resolve_clip_slot(live, &target, p.clip.as_ref(), p.clip_index)?;
    live.send_command(
        "set_clip_name",
        Some(json!({"track_index": target.index, "clip_index": slot, "name": p.name})),
    )
    .map_err(|e| live_err("set clip name", e))?;
    Ok(format!(
        "Renamed clip at track {}, slot {slot} to '{}'",
        target.index, p.name
    ))
}

/// The Arrangement's clips are addressed by their position in
/// `track.arrangement_clips`, which is what `arrange list` prints — a name
/// there is ambiguous by design (the copies share one), so this one keeps
/// the index and only the track takes a name.
pub fn set_arrangement_clip_name_body(live: &LiveState, p: &SetClipNameParams) -> ToolResult {
    require(live, "set_arrangement_clip_name")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    let index = match p.clip.as_ref() {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) if s.trim().parse::<i64>().is_ok() => {
            s.trim().parse::<i64>().unwrap_or(0)
        }
        Some(other) => {
            return Err(format!(
                "an Arrangement clip is its position as `arrange list` prints it, not {other}"
            ))
        }
        None => p
            .clip_index
            .ok_or("give clip (the Arrangement clip's position, as `arrange list` prints it)")?,
    };
    live.send_command(
        "set_arrangement_clip_name",
        Some(json!({"track_index": target.index, "clip_index": index, "name": p.name})),
    )
    .map_err(|e| live_err("set arrangement clip name", e))?;
    Ok(format!(
        "Renamed arrangement clip at track {}, index {index} to '{}'",
        target.index, p.name
    ))
}

pub fn set_tempo_body(live: &LiveState, p: &SetTempoParams) -> ToolResult {
    guard_performance(live, "set_tempo", "would jump the tempo mid-bar; ramp it with cue {\"steps\": [{\"from\": \"next_bar\", \"bars\": 8, \"ramp\": {\"tempo\": N}}]} instead")?;
    require(live, "set_tempo")?;
    live.send_command("set_tempo", Some(json!({"tempo": p.tempo})))
        .map_err(|e| live_err("set tempo", e))?;
    Ok(format!("Set tempo to {} BPM", p.tempo))
}

pub fn load_instrument_or_effect_body(live: &LiveState, p: &LoadInstrumentParams) -> ToolResult {
    require(live, "load_browser_item")?;
    let kind = p.kind.trim().to_lowercase();
    if !matches!(kind.as_str(), "track" | "return" | "master") {
        return Err(format!(
            "kind must be track, return or master, not '{}'",
            p.kind
        ));
    }
    let target = resolve_track_target(live, p.track.as_ref(), Some(&p.kind), p.track_index)?;
    // Plain words are a search; a URI carries a ':'.
    let uri = if p.uri.contains(':') {
        p.uri.clone()
    } else {
        let hits = find_items(live, &p.uri, "all", 1)?;
        match hits.first() {
            Some(h) => h.uri.clone(),
            None => return Err(format!("nothing in the browser matches \"{}\"", p.uri)),
        }
    };
    let r = live
        .send_command(
            "load_browser_item",
            Some(json!({"track_index": target.index, "item_uri": uri, "kind": target.kind})),
        )
        .map_err(|e| live_err("load instrument by URI", e))?;
    if !r.get("loaded").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("Failed to load instrument with URI '{}'", p.uri));
    }
    match r.get("loaded_device").filter(|d| d.is_object()) {
        Some(dev) => Ok(format!(
            "Loaded '{}' as device {} on track {} ('{}'). Devices on the track now: {}",
            get_display(dev, "name", &get_display(&r, "item_name", "device")),
            get_display(dev, "index", "?"),
            target.index,
            target.label(&get_display(&r, "track_name", "")),
            join_names(r.get("devices_after"))
        )),
        None => Ok(format!(
            "Loaded '{}' on track {} ('{}'). Devices on the track now: {}",
            get_display(&r, "item_name", &p.uri),
            target.index,
            target.label(&get_display(&r, "track_name", "")),
            join_names(r.get("devices_after"))
        )),
    }
}

pub fn fire_clip_body(live: &LiveState, p: &FireClipParams) -> ToolResult {
    require(live, "fire_clip")?;
    if let Some(text) = defer_if_too_close(
        live,
        p.no_later_than,
        json!({"action": "fire_clip", "track_index": p.track_index, "clip_index": p.clip_index}),
        &format!("fire track {}/slot {}", p.track_index, p.clip_index),
    )? {
        return Ok(text);
    }
    let r = live
        .send_command(
            "fire_clip",
            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
        )
        .map_err(|e| live_err("fire clip", e))?;
    Ok(format!(
        "Fired track {}, slot {}{}",
        p.track_index,
        p.clip_index,
        landing_text(live, &r)
    ))
}

/// " — lands on bar 40 (issued at 39.3)" from the Remote Script's own reading
/// after the fire; says so when the bar line passed while the call was in flight.
fn landing_text(live: &LiveState, r: &Value) -> String {
    let Some(lands) = r.get("lands_on_bar").and_then(Value::as_i64) else {
        return String::new();
    };
    let issued = format!(
        "{}.{}",
        r.get("issued_at_bar").and_then(Value::as_i64).unwrap_or(0),
        r.get("issued_at_beat_in_bar")
            .and_then(Value::as_i64)
            .unwrap_or(1)
    );
    let expected = live
        .last_clock()
        .filter(|(age, _)| *age < 5.0)
        .and_then(|(_, c)| c.get("bar").and_then(Value::as_i64))
        .map(|b| b + 1);
    match expected {
        Some(e) if lands > e => format!(
            " — lands on bar {lands}, not {e}: the call arrived at {issued}, after the bar line (round trip {:.2} s). For certainty, cue it.",
            live.round_trip_s()
        ),
        _ => format!(" — lands on bar {lands} (issued at {issued})."),
    }
}

/// `no_later_than`: when the bar is closer than the measured round trip plus
/// a margin, schedule a one-step cue for the next certain bar instead of
/// gambling; returns the text to reply with in that case.
fn defer_if_too_close(
    live: &LiveState,
    no_later_than: Option<i64>,
    mut step: Value,
    what: &str,
) -> Result<Option<String>, String> {
    let Some(target) = no_later_than else {
        return Ok(None);
    };
    let Some((age, clock)) = live.last_clock() else {
        return Ok(None);
    };
    let bar = clock.get("bar").and_then(Value::as_i64).unwrap_or(0);
    let bib = clock
        .get("beat_in_bar")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let next_bar = bar + 1;
    if next_bar < target {
        return Ok(None);
    }
    if next_bar > target {
        return Err(format!(
            "bar {target} has passed (it is bar {bar}.{bib}); fire without no_later_than, or cue the next bar."
        ));
    }
    let secs = clock
        .get("seconds_to_next_bar")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        - age;
    let rt = live.round_trip_s();
    if secs > rt + 0.15 {
        return Ok(None);
    }
    require(live, "schedule_cue")?;
    let bpb = clock
        .get("beats_per_bar")
        .and_then(Value::as_f64)
        .unwrap_or(4.0);
    step["beat"] = json!(target as f64 * bpb);
    step["bar"] = json!(target + 1);
    step["label"] = json!(what);
    let sent = live
        .send_command(
            "schedule_cue",
            Some(json!({"cue": {"name": format!("{what} (no later than)"), "steps": [step]}})),
        )
        .map_err(|e| live_err("schedule the launch", e))?;
    let id = sent.get("id").and_then(Value::as_i64).unwrap_or(0);
    Ok(Some(format!(
        "Not fired: bar {target} is {:.1} s away and the round trip is {rt:.2} s, so the launch might land on {}. Scheduled as cue {id} for bar {} instead (the earliest certain bar); cancel_cue {id} to drop it.",
        secs.max(0.0),
        target + 1,
        target + 1
    )))
}

pub fn stop_clip_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "stop_clip")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    let slot = resolve_clip_slot(live, &target, p.clip.as_ref(), p.clip_index)?;
    live.send_command(
        "stop_clip",
        Some(json!({"track_index": target.index, "clip_index": slot})),
    )
    .map_err(|e| live_err("stop clip", e))?;
    Ok(format!(
        "Stopped clip at track {}, slot {slot}",
        target.index
    ))
}

pub fn delete_clip_body(live: &LiveState, p: &ClipParams) -> ToolResult {
    require(live, "delete_clip")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    let slot = resolve_clip_slot(live, &target, p.clip.as_ref(), p.clip_index)?;
    guard_delete(live, target.index, Some(slot))?;
    live.send_command(
        "delete_clip",
        Some(json!({"track_index": target.index, "clip_index": slot})),
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
    guard_performance(live, "stop_playback", "would cut the audio mid-bar")?;
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
    guard_performance(
        live,
        "switch_to_arrangement_view",
        "would leave the Session view the set is played from",
    )?;
    live.send_command("switch_to_arrangement_view", None)
        .map_err(|e| live_err("switch to arrangement view", e))?;
    Ok("Switched to Arrangement view".to_string())
}

/// A position given as a bar (Live's 1-based bars) or in beats, as beats
/// plus the beats per bar it was converted with.
fn beats_from(
    live: &LiveState,
    bar: Option<f64>,
    time: Option<f64>,
    tool: &str,
) -> Result<(f64, f64), String> {
    // The signature is read only when a bar has to be converted.
    let bpb = if bar.is_some() {
        tempo_and_meter(live).1
    } else {
        4.0
    };
    match (bar, time) {
        (Some(b), _) if b >= 1.0 => Ok(((b - 1.0) * bpb, bpb)),
        (Some(b), _) => Err(format!(
            "{tool}: bar {b} is before bar 1 (Live's bars start at 1)"
        )),
        (None, Some(t)) if t >= 0.0 => Ok((t, bpb)),
        (None, Some(t)) => Err(format!("{tool}: beat {t} is before the start")),
        (None, None) => Err(format!("{tool}: give `bar` (Live's 1-based bar number)")),
    }
}

/// `33` or `33.5`: a beat position as a bar.
fn bar_text(beat: f64, bpb: f64) -> String {
    let bar = beat / bpb.max(1.0) + 1.0;
    if (bar - bar.round()).abs() < 1e-6 {
        format!("{}", bar.round() as i64)
    } else {
        format!("{bar:.2}")
    }
}

pub fn set_arrangement_time_body(live: &LiveState, p: &ArrangementTimeParams) -> ToolResult {
    guard_performance(
        live,
        "set_arrangement_time",
        "would move the playhead under the playing clips",
    )?;
    require(live, "set_current_song_time")?;
    let (time, bpb) = beats_from(live, p.bar, p.time, "set_arrangement_time")?;
    let r = live
        .send_command("set_current_song_time", Some(json!({"time": time})))
        .map_err(|e| live_err("set arrangement time", e))?;
    let was = r
        .get("previous_song_time")
        .and_then(Value::as_f64)
        .map(|v| format!(" (was at bar {})", bar_text(v, bpb)))
        .unwrap_or_default();
    Ok(format!(
        "Playhead moved to bar {}{was}",
        r.get("current_song_time")
            .and_then(Value::as_f64)
            .map(|v| bar_text(v, bpb))
            .unwrap_or_else(|| bar_text(time, bpb))
    ))
}

pub fn get_arrangement_clips_body(live: &LiveState, p: &TrackParams) -> ToolResult {
    require(live, "get_arrangement_clips")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    live.send_command(
        "get_arrangement_clips",
        Some(json!({"track_index": target.index})),
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
    let times = if p.at_bar.is_some() {
        let (_, bpb) = tempo_and_meter(live);
        let at = p.at_bar.unwrap_or(1.0);
        if at < 1.0 {
            return Err(format!("at_bar {at} is before bar 1"));
        }
        match p.until_bar {
            Some(until) => {
                let step = match p.every_bars {
                    Some(b) if b > 0.0 => b,
                    Some(b) => return Err(format!("every_bars must be positive, got {b}")),
                    None => {
                        require(live, "get_clip_info")?;
                        live.send_command(
                            "get_clip_info",
                            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "arrangement": false})),
                        )
                        .ok()
                        .and_then(|i| i.get("length").and_then(Value::as_f64))
                        .filter(|l| *l > 0.0)
                        .unwrap_or(4.0)
                            / bpb
                    }
                };
                let mut t = Vec::new();
                let mut b = at;
                while b < until - 1e-9 && t.len() < 512 {
                    t.push((b - 1.0) * bpb);
                    b += step;
                }
                if t.is_empty() {
                    return Err("until_bar is not after at_bar".into());
                }
                t
            }
            None => vec![(at - 1.0) * bpb],
        }
    } else {
        placement_times(p)?
    };
    // Existing clips on the track, to warn when a placement lands inside one
    // (Live trims or splits the earlier clip).
    let existing: Vec<(f64, f64)> = live
        .send_command(
            "get_arrangement_clips",
            Some(json!({"track_index": p.track_index})),
        )
        .ok()
        .and_then(|r| r.get("clips").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| Some((c.get("start_time")?.as_f64()?, c.get("end_time")?.as_f64()?)))
        .collect();
    let overlaps: Vec<f64> = times
        .iter()
        .copied()
        .filter(|t| existing.iter().any(|(s, e)| *t > s - 1e-6 && *t < e - 1e-6))
        .collect();
    // Every placement in one round trip (the script loops on Live's main thread).
    require(live, "place_clips")?;
    let r = live
        .send_command(
            "place_clips",
            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "times": times})),
        )
        .map_err(|e| live_err("place the clip in the arrangement", e))?;
    // A reply without `placed` (an older script's shape) means every time landed.
    let placed: Vec<f64> = r
        .get("placed")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_else(|| times.clone());
    let clip_name = get_display(&r, "clip", "clip");
    let track_name = get_display(&r, "track", &format!("track {}", p.track_index));
    if let Some(failed) = r
        .get("failed")
        .and_then(Value::as_array)
        .filter(|f| !f.is_empty())
    {
        return Err(format!(
            "Placed {} of {}, but {} could not be placed: {}",
            placed.len(),
            times.len(),
            failed.len(),
            failed
                .iter()
                .map(|f| format!(
                    "beat {}: {}",
                    get_display(f, "time", "?"),
                    get_display(f, "error", "?")
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if placed.is_empty() {
        return Err("nothing was placed".into());
    }
    let warn = if overlaps.is_empty() {
        String::new()
    } else {
        format!(
            ". Note: the placement(s) at beat(s) {} start inside clips that were already there; Live trims the earlier clip",
            list_beats(&overlaps)
        )
    };
    if placed.len() == 1 {
        Ok(format!(
            "Duplicated '{clip_name}' from Session slot {} on '{track_name}' to arrangement at beat {}{warn}",
            p.clip_index, placed[0]
        ))
    } else {
        Ok(format!(
            "Placed '{clip_name}' from Session slot {} on '{track_name}' {} times in the arrangement, at beats {}{warn}",
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
    let (time, bpb) = beats_from(live, p.bar, p.time, "create_locator")?;
    let r = live
        .send_command(
            "create_locator",
            Some(json!({"name": p.name, "time": time})),
        )
        .map_err(|e| live_err("create locator", e))?;
    Ok(format!(
        "Locator '{}' set at bar {}",
        get_display(&r, "name", &p.name),
        r.get("time")
            .and_then(Value::as_f64)
            .map(|v| bar_text(v, bpb))
            .unwrap_or_else(|| bar_text(time, bpb))
    ))
}

// ── Mixer, colours, drum pads, deletion ─────────────────────────────────────

/// `-6.0 dB`, from the script's reading of the fader.
pub(crate) fn db_text(v: Option<f64>) -> Option<String> {
    v.map(|db| {
        if db <= -70.0 {
            "-inf dB".to_string()
        } else {
            format!("{db:.1} dB")
        }
    })
}

fn mixer_summary(r: &Value) -> String {
    let mut parts = vec![
        match db_text(r.get("volume_db").and_then(Value::as_f64)) {
            Some(db) => format!("volume {db}"),
            None => format!("volume {}", get_display(r, "volume", "?")),
        },
        format!("pan {}", get_display(r, "panning", "?")),
    ];
    if let Some(m) = r.get("mute").and_then(Value::as_bool) {
        parts.push(if m { "muted".into() } else { "unmuted".into() });
    }
    if let Some(s) = r.get("solo").and_then(Value::as_bool) {
        if s {
            parts.push("solo".into());
        }
    }
    if let Some(a) = r.get("arm").and_then(Value::as_bool) {
        if a {
            parts.push("armed".into());
        }
    }
    if let Some(sends) = r.get("sends").and_then(Value::as_array) {
        let s: Vec<String> = sends
            .iter()
            .map(|x| {
                format!(
                    "{}={}",
                    get_display(x, "name", "send"),
                    get_display(x, "value", "?")
                )
            })
            .collect();
        if !s.is_empty() {
            parts.push(format!("sends {}", s.join(" ")));
        }
    }
    parts.join(", ")
}

pub fn set_track_mixer_body(live: &LiveState, p: &SetTrackMixerParams) -> ToolResult {
    require(live, "set_track_mixer")?;
    let target = resolve_track_target(live, p.track.as_ref(), Some(&p.kind), p.track_index)?;
    let volume_db = p.volume_db.or(p.volume);
    if volume_db.is_none()
        && p.fader.is_none()
        && p.pan.is_none()
        && p.mute.is_none()
        && p.solo.is_none()
        && p.arm.is_none()
    {
        return Err("Nothing to set: give volume (dB), pan, mute, solo or arm.".into());
    }
    if let Some(db) = volume_db {
        if !(-80.0..=6.0).contains(&db) {
            return Err(format!(
                "volume is in dB: between -80 and +6 (0 is unity), got {db}"
            ));
        }
    }
    if let Some(f) = p.fader {
        if !(0.0..=1.0).contains(&f) {
            return Err(format!("fader is Live's raw 0–1 parameter, got {f}"));
        }
    }
    let r = live
        .send_command(
            "set_track_mixer",
            Some(json!({
                "track_index": target.index, "kind": target.kind,
                "volume": p.fader, "volume_db": volume_db, "pan": p.pan, "mute": p.mute, "solo": p.solo, "arm": p.arm,
            })),
        )
        .map_err(|e| live_err("set the track mixer", e))?;
    Ok(format!(
        "'{}' now: {}",
        target.label(&get_display(&r, "name", "")),
        mixer_summary(&r)
    ))
}

pub fn set_send_body(live: &LiveState, p: &SetSendParams) -> ToolResult {
    require(live, "set_send")?;
    let target = resolve_track_target(live, p.track.as_ref(), Some(&p.kind), p.track_index)?;
    if p.send_name.is_empty() && p.send_index.is_none() {
        return Err("Say which send: send_name (the return track's name or letter) or send_index. get_returns lists them.".into());
    }
    let r = live
        .send_command(
            "set_send",
            Some(json!({
                "track_index": target.index, "kind": target.kind,
                "send_name": if p.send_name.is_empty() { Value::Null } else { json!(p.send_name) },
                "send_index": p.send_index, "value": p.value,
            })),
        )
        .map_err(|e| live_err("set the send", e))?;
    Ok(format!(
        "'{}' send {} ({}) set to {}",
        target.label(&get_display(&r, "track", "")),
        get_display(&r, "send_index", "?"),
        get_display(&r, "return_name", "return"),
        get_display(&r, "value", &p.value.to_string())
    ))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DescribeLiveParams {
    /// A path into Live's object model, rooted at `song`, `application` or
    /// `browser`: `song`, `song.tracks[0]`, `song.tracks[0].mixer_device.volume`.
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RunOpsParams {
    /// The operations, in order. Each is `{"op": "get"|"set"|"call"|"wait_tick",
    /// "path": "song...", "value": …, "args": […], "ticks": n, "as": "name"}`.
    pub ops: Vec<Value>,
}

/// What this Live actually has at a path: its class, every attribute with
/// its type and whether it can be written, and the methods that can be
/// called. Cached per Live version.
pub fn describe_live_body(live: &LiveState, p: &DescribeLiveParams) -> ToolResult {
    require(live, "describe")?;
    let path = crate::lom::Path::from_str_checked(&p.path)?;
    let d = live.lom.describe(live, &path)?;
    let mut names: Vec<_> = d.attrs.iter().collect();
    names.sort_by_key(|(k, _)| k.as_str());
    let mut out = format!(
        "{} at {}{}\n\nLive {}\n",
        d.class,
        path,
        d.name
            .as_deref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default(),
        d.live_version.as_deref().unwrap_or("?")
    );
    out.push_str("\nAttributes (· = read-only):\n");
    for (name, a) in names {
        out.push_str(&format!(
            "  {}{} {}\n",
            if a.readonly { "· " } else { "  " },
            name,
            a.r#type
        ));
    }
    if !d.methods.is_empty() {
        out.push_str(&format!("\nMethods: {}\n", d.methods.join(", ")));
    }
    Ok(out)
}

/// A batch of operations against Live's object model, in one round trip.
pub fn run_ops_body(live: &LiveState, p: &RunOpsParams) -> ToolResult {
    require(live, "run")?;
    let batch = crate::lom::Batch::from_values(&p.ops)?;
    let out = batch.run(live)?;
    let ran = out.get("ops").and_then(Value::as_u64).unwrap_or(0);
    let mut named: Vec<_> = out.iter().filter(|(k, _)| k.as_str() != "ops").collect();
    named.sort_by_key(|(k, _)| k.as_str());
    if named.is_empty() {
        return Ok(format!("{ran} ops ran; nothing was asked back."));
    }
    let mut text = format!("{ran} ops ran:\n");
    for (name, value) in named {
        text.push_str(&format!("  {name} = {value}\n"));
    }
    Ok(text)
}

pub fn get_returns_body(live: &LiveState, _p: &Empty) -> ToolResult {
    require(live, "get_returns")?;
    live.send_command("get_returns", None)
        .map(|r| pretty(&r))
        .map_err(|e| live_err("list the return tracks", e))
}

pub fn set_color_body(live: &LiveState, p: &SetColorParams) -> ToolResult {
    if !(0..=69).contains(&p.color_index) {
        return Err(format!(
            "color_index {} is outside Live's palette (0-69)",
            p.color_index
        ));
    }
    match p.clip_index {
        Some(clip_index) => {
            require(live, "set_clip_color")?;
            let r = live
                .send_command(
                    "set_clip_color",
                    Some(json!({
                        "track_index": p.track_index, "clip_index": clip_index,
                        "arrangement": p.arrangement, "color_index": p.color_index,
                    })),
                )
                .map_err(|e| live_err("set the clip colour", e))?;
            Ok(format!(
                "Clip '{}' coloured {} (index {})",
                get_display(&r, "name", "clip"),
                if p.arrangement {
                    "in the Arrangement"
                } else {
                    "in the Session"
                },
                get_display(&r, "color_index", &p.color_index.to_string())
            ))
        }
        None => {
            require(live, "set_track_color")?;
            let r = live
                .send_command(
                    "set_track_color",
                    Some(json!({"track_index": p.track_index, "kind": p.kind, "color_index": p.color_index})),
                )
                .map_err(|e| live_err("set the track colour", e))?;
            Ok(format!(
                "Track '{}' coloured (index {})",
                get_display(&r, "name", &format!("track {}", p.track_index)),
                get_display(&r, "color_index", &p.color_index.to_string())
            ))
        }
    }
}

pub fn get_drum_rack_pads_body(live: &LiveState, p: &DrumRackPadsParams) -> ToolResult {
    require(live, "get_drum_rack_pads")?;
    let target = resolve_track_target(
        live,
        p.track.as_ref(),
        p.kind.as_deref(),
        Some(p.track_index),
    )?;
    let r = live
        .send_command(
            "get_drum_rack_pads",
            Some(
                json!({"track_index": target.index, "kind": target.kind, "device_index": p.device_index.unwrap_or(-1)}),
            ),
        )
        .map_err(|e| live_err("read the drum rack pads", e))?;
    let mut out = format!(
        "{} on track {} ('{}'), device {} — {} pads with a sound:\n",
        get_display(&r, "device_name", "Drum Rack"),
        p.track_index,
        get_display(&r, "track_name", "track"),
        get_display(&r, "device_index", "?"),
        get_display(&r, "pad_count", "0")
    );
    if let Some(pads) = r.get("pads").and_then(Value::as_array) {
        for pad in pads {
            let flags = match (
                pad.get("mute").and_then(Value::as_bool).unwrap_or(false),
                pad.get("solo").and_then(Value::as_bool).unwrap_or(false),
            ) {
                (true, _) => " [muted]",
                (_, true) => " [solo]",
                _ => "",
            };
            out.push_str(&format!(
                "  {} ({}): {}{}\n",
                get_display(pad, "pitch", "?"),
                get_display(pad, "note_name", "?"),
                get_display(pad, "pad_name", "?"),
                flags
            ));
        }
    }
    out.push_str("Use the pitch numbers in steps, patterns or notes. Names follow Live (C1 = 36).");
    Ok(out)
}

pub fn delete_arrangement_clip_body(
    live: &LiveState,
    p: &DeleteArrangementClipParams,
) -> ToolResult {
    require(live, "delete_arrangement_clip")?;
    if p.all || !p.clip_indices.is_empty() {
        // Every clip in one round trip (the script loops on Live's main thread).
        require(live, "delete_arrangement_clips")?;
        let params = if p.all {
            json!({"track_index": p.track_index, "all": true})
        } else {
            json!({"track_index": p.track_index, "indices": p.clip_indices})
        };
        let r = live
            .send_command("delete_arrangement_clips", Some(params))
            .map_err(|e| live_err("delete the arrangement clips", e))?;
        let removed: Vec<String> = r
            .get("removed")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|c| {
                        format!(
                            "'{}' ({}–{})",
                            get_display(c, "name", "clip"),
                            beat(c.get("start_time")),
                            beat(c.get("end_time"))
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if removed.is_empty() {
            return Ok(format!("Track {} has no Arrangement clips.", p.track_index));
        }
        return Ok(format!(
            "Removed {} Arrangement clip(s) from track {} in one round trip: {}",
            removed.len(),
            p.track_index,
            if removed.len() > 12 {
                format!(
                    "{} … {}",
                    removed[..6].join(", "),
                    removed[removed.len() - 3..].join(", ")
                )
            } else {
                removed.join(", ")
            }
        ));
    }
    if p.clip_index < 0 {
        return Err("Give clip_index, clip_indices, or all: true.".into());
    }
    let r = live
        .send_command(
            "delete_arrangement_clip",
            Some(json!({"track_index": p.track_index, "clip_index": p.clip_index})),
        )
        .map_err(|e| live_err("delete the arrangement clip", e))?;
    Ok(format!(
        "Removed '{}' (beats {}–{}) from the Arrangement on track {}; {} clips remain there. Indices of later clips shifted down by one.",
        get_display(&r, "name", "clip"),
        beat(r.get("start_time")),
        beat(r.get("end_time")),
        p.track_index,
        get_display(&r, "remaining", "?")
    ))
}

pub fn delete_locator_body(live: &LiveState, p: &DeleteLocatorParams) -> ToolResult {
    require(live, "delete_locator")?;
    if p.name.is_empty() && p.time.is_none() {
        return Err("Say which locator: its name or its beat position.".into());
    }
    let r = live
        .send_command(
            "delete_locator",
            Some(json!({
                "name": if p.name.is_empty() { Value::Null } else { json!(p.name) },
                "time": p.time,
            })),
        )
        .map_err(|e| live_err("delete the locator", e))?;
    Ok(format!(
        "Deleted locator '{}' at beat {}; {} remain",
        get_display(&r, "deleted", &p.name),
        get_display(&r, "time", "?"),
        get_display(&r, "remaining", "?")
    ))
}

// ── Search, clip settings, meters, automation ───────────────────────────────

/// Search hits from the server's library index when it is complete, else
/// from the Remote Script (merged with whatever the index has so far).
/// The best sample Live's browser knows by these words, for the case where
/// no file on disk matched (a Place Live will not locate, a pack whose files
/// this does not walk). None when the browser has nothing either.
pub(crate) fn browser_sample(
    live: &LiveState,
    query: &str,
) -> Result<Option<crate::library::Item>, String> {
    if require(live, "search_browser").is_err() {
        return Ok(None);
    }
    Ok(find_items(live, query, "samples", 1)?.into_iter().next())
}

fn find_items(
    live: &LiveState,
    query: &str,
    category: &str,
    limit: usize,
) -> Result<Vec<crate::library::Item>, String> {
    let ix = live.library.snapshot();
    let walked = crate::library::WALKED_CATEGORIES.contains(&category);
    if let Some(ix) = ix.as_ref().filter(|ix| ix.complete && walked) {
        return Ok(crate::library::search(ix, query, category, limit)
            .into_iter()
            .cloned()
            .collect());
    }
    require(live, "search_browser")?;
    let r = live
        .send_command(
            "search_browser",
            Some(json!({"query": query, "category": category, "limit": limit})),
        )
        .map_err(|e| browser_error("search the browser", e))?;
    let mut items: Vec<crate::library::Item> = r
        .get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    if let Some(ix) = ix.as_ref().filter(|_| walked) {
        for hit in crate::library::search(ix, query, category, limit) {
            if !items.iter().any(|i| i.uri == hit.uri) {
                items.push(hit.clone());
            }
        }
        items.truncate(limit);
    }
    Ok(items)
}

pub fn search_browser_body(live: &LiveState, p: &SearchBrowserParams) -> ToolResult {
    if p.refresh {
        live.library.clear();
        if let Ok(path) = std::env::var("ABLETON_MCP_STATE_DIR").map(std::path::PathBuf::from) {
            let _ = std::fs::remove_dir_all(path.join("library"));
        } else {
            let _ = std::fs::remove_dir_all(crate::state::library_dir());
        }
    }
    let mut queries: Vec<String> = p
        .queries
        .iter()
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty())
        .collect();
    if !p.query.trim().is_empty() {
        queries.insert(0, p.query.trim().to_string());
    }
    if queries.is_empty() {
        if p.refresh {
            return Ok(format!(
                "Library index dropped; it is walked again in the background. {}",
                live.library.status_line()
            ));
        }
        return Err(
            "Give a query, e.g. \"analog bass\" or \"techno kit\", or queries: [\"…\", \"…\"]."
                .into(),
        );
    }
    let limit = if p.best {
        1
    } else {
        p.limit.clamp(1, 200) as usize
    };
    // Samples are files: the sample index answers with paths, which is what
    // add_sample needs. Live's browser still answers when the index cannot.
    if p.category.trim().eq_ignore_ascii_case("samples") {
        if let Some(text) = crate::samples::search_text(live, &queries, limit, p.best)? {
            return Ok(text);
        }
    }
    let ix = live.library.snapshot();
    let from_index = ix.as_ref().is_some_and(|ix| ix.complete)
        && crate::library::WALKED_CATEGORIES.contains(&p.category.as_str());
    let mut out = if from_index {
        format!("From the {}:\n", live.library.status_line())
    } else if let Some(ix) = ix.as_ref() {
        format!(
            "From Live plus the library index ({} items so far, walking):\n",
            ix.items.len()
        )
    } else {
        String::new()
    };
    if queries.len() == 1 && !p.best {
        let q = &queries[0];
        let items = find_items(live, q, &p.category, limit)?;
        if items.is_empty() {
            return Ok(format!(
                "{out}Nothing in the {} browser matches \"{q}\". Try fewer or different words, or another category.",
                p.category
            ));
        }
        out.push_str(&format!(
            "{} match{} for \"{q}\" ({}):\n",
            items.len(),
            if items.len() == 1 { "" } else { "es" },
            p.category
        ));
        for item in &items {
            out.push_str(&format!(
                "  {} — {}\n    uri: {}\n",
                item.name, item.path, item.uri
            ));
        }
        out.push_str("Load one with load_instrument_or_effect(track_index, uri), or give build_song the words.");
        return Ok(out);
    }
    let width = queries.iter().map(String::len).max().unwrap_or(8).min(40);
    for q in &queries {
        let items = find_items(live, q, &p.category, limit)?;
        match items.first() {
            Some(item) if p.best => out.push_str(&format!(
                "  {q:<width$} → '{}'  {}  ({})\n",
                item.name, item.uri, item.path
            )),
            Some(_) => {
                out.push_str(&format!("  {q}:\n"));
                for item in &items {
                    out.push_str(&format!(
                        "    {} — {}  uri: {}\n",
                        item.name, item.path, item.uri
                    ));
                }
            }
            None => out.push_str(&format!(
                "  {q:<width$} → nothing{}\n",
                if from_index {
                    ""
                } else {
                    " yet; more may appear when the walk completes"
                }
            )),
        }
    }
    out.push_str("Use the URIs in build_song `instrument` or load_instrument_or_effect; or give build_song the words and it resolves them the same way.");
    Ok(out)
}

pub fn search_browser_body_legacy(live: &LiveState, p: &SearchBrowserParams) -> ToolResult {
    require(live, "search_browser")?;
    if p.query.trim().is_empty() {
        return Err("Give a query, e.g. \"analog bass\" or \"techno kit\".".into());
    }
    let r = live
        .send_command(
            "search_browser",
            Some(json!({"query": p.query, "category": p.category, "limit": p.limit})),
        )
        .map_err(|e| browser_error("search the browser", e))?;
    let items = r
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return Ok(format!(
            "Nothing in the {} browser matches \"{}\". Try fewer or different words, or another category.",
            get_display(&r, "category", "all"),
            p.query
        ));
    }
    let mut out = format!(
        "{} of {} matches for \"{}\" ({}):\n",
        get_display(&r, "returned", "?"),
        get_display(&r, "total_matches", "?"),
        p.query,
        get_display(&r, "category", "all")
    );
    for item in &items {
        out.push_str(&format!(
            "  {} — {}\n    uri: {}\n",
            get_display(item, "name", "?"),
            get_display(item, "path", ""),
            get_display(item, "uri", "?")
        ));
    }
    out.push_str("Load one with load_instrument_or_effect(track_index, uri).");
    if r.get("truncated_walk").and_then(Value::as_bool) == Some(true) {
        out.push_str(&format!(
            " Searched for {} s and stopped early; narrow the category (e.g. \"instruments\") if the sound you want is missing.",
            get_display(&r, "seconds", "?")
        ));
    }
    Ok(out)
}

/// Live 12 Suite's instruments. An instrument missing from the browser is
/// either not in this Live edition or not installed.
const SUITE_INSTRUMENTS: &[&str] = &[
    "Analog",
    "Collision",
    "Drift",
    "Drum Rack",
    "Drum Sampler",
    "Electric",
    "External Instrument",
    "Instrument Rack",
    "Meld",
    "Operator",
    "Sampler",
    "Simpler",
    "Tension",
    "Wavetable",
];

/// What this Live's devices have answered to — and what is **not** kept.
///
/// A local cache the producer cannot see or delete is not one this server
/// ships; this is the one call for both. It asks Live nothing.
pub fn device_vocabulary_body(live: &LiveState, p: &DeviceVocabularyParams) -> ToolResult {
    let version = live.live_version();
    let action = p.action.trim().to_lowercase();
    if action == "forget" {
        return Ok(format!(
            "{} Nothing in Live changed — a device's own parameters are Live's, not the server's.",
            live.devices.forget(&version)
        ));
    }
    if !action.is_empty() && action != "show" {
        return Err(format!("action must be show or forget, not '{action}'"));
    }
    let vocab = live.devices.snapshot(&version);
    if let Some(want) = p.device.as_deref().filter(|d| !d.trim().is_empty()) {
        let want = want.trim().to_lowercase();
        let Some(facts) = vocab
            .devices
            .values()
            .find(|d| d.device.to_lowercase() == want)
            .or_else(|| {
                vocab
                    .devices
                    .values()
                    .find(|d| d.device.to_lowercase().contains(&want))
            })
        else {
            return Ok(format!(
                "Nothing learned about a device called '{want}' on Live {version}. {}",
                crate::devices::status_line(&vocab)
            ));
        };
        let mut out = format!(
            "'{}' ({}), Live {}, last read {}.\nParameters: {}\n",
            facts.device,
            facts.class_name,
            facts.live_version,
            facts.observed_at,
            facts.names().join(", ")
        );
        for fact in &facts.parameters {
            if fact.seen.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "  {} — {}\n",
                fact.name,
                fact.seen
                    .iter()
                    .map(|s| format!("{:.2} → {} ({})", s.value, s.display, s.at))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if let Some(said) = crate::devices::backwards(fact) {
                out.push_str(&format!("  {said}\n"));
            }
        }
        if !facts.unknown_words.is_empty() {
            out.push_str(&format!(
                "Words it does not answer to: {}\n",
                facts
                    .unknown_words
                    .iter()
                    .map(|(w, at)| format!("{w} ({at})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        return Ok(out);
    }
    let mut out = format!("{}\n", crate::devices::status_line(&vocab));
    for facts in vocab.devices.values().take(40) {
        out.push_str(&format!(
            "  {} ({}) — {} parameters, read {}{}\n",
            facts.device,
            facts.class_name,
            facts.parameters.len(),
            facts.observed_at,
            if facts.unknown_words.is_empty() {
                String::new()
            } else {
                format!(
                    "; no {}",
                    facts
                        .unknown_words
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        ));
    }
    if vocab.devices.len() > 40 {
        out.push_str(&format!("  … and {} more\n", vocab.devices.len() - 40));
    }
    out.push_str(&format!(
        "It holds device and parameter names as Live reports them, and the values that were written with what Live displayed for each — stamped with the Live version and the day. It holds no note, no audio and no path, and nothing about any song: these are facts about Ableton's content, true in every set. {}. ABLETON_MCP_LIBRARY_INDEX=false keeps it in memory only, and action: \"forget\" deletes it.",
        if crate::devices::disk_enabled() && version != crate::devices::UNKNOWN_VERSION {
            format!("It is one file, {}", crate::state::devices_dir().join(format!("{version}.json")).display())
        } else {
            "It is in memory only".to_string()
        }
    ));
    Ok(out)
}

pub fn get_library_status_body(live: &LiveState, _p: &Empty) -> ToolResult {
    require(live, "get_library_status")?;
    let r = live
        .send_command("get_library_status", None)
        .map_err(|e| live_err("read the library status", e))?;
    let names = |key: &str| -> Vec<String> {
        r.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| get_display(v, "name", "?")).collect())
            .unwrap_or_default()
    };
    let instruments = names("instruments");
    let missing: Vec<&str> = SUITE_INSTRUMENTS
        .iter()
        .copied()
        .filter(|s| !instruments.iter().any(|i| i.eq_ignore_ascii_case(s)))
        .collect();
    let packs = names("packs");
    let mut out = format!(
        "Live {}{}.\n",
        get_display(&r, "live_version", "?"),
        r.get("edition_hint")
            .and_then(Value::as_str)
            .map(|e| format!(" ({e})"))
            .unwrap_or_default()
    );
    out.push_str(&format!(
        "Instruments available ({}): {}\n",
        instruments.len(),
        instruments.join(", ")
    ));
    if missing.is_empty() {
        out.push_str("Every Live 12 Suite instrument is present.\n");
    } else {
        out.push_str(&format!(
            "Not available here ({}): {} — not in this edition or not installed; pick from the list above instead.\n",
            missing.len(),
            missing.join(", ")
        ));
    }
    for (label, key) in [
        ("Audio effects", "audio_effects"),
        ("MIDI effects", "midi_effects"),
    ] {
        let list = names(key);
        out.push_str(&format!("{label} ({}): {}\n", list.len(), list.join(", ")));
    }
    out.push_str(&format!(
        "Packs installed ({}): {}\n",
        packs.len(),
        if packs.is_empty() {
            "none".to_string()
        } else {
            packs.join(", ")
        }
    ));
    for (label, key) in [("Drums folders", "drums"), ("Sounds folders", "sounds")] {
        let list = names(key);
        if !list.is_empty() {
            out.push_str(&format!("{label}: {}\n", list.join(", ")));
        }
    }
    out.push_str("Packs that are not downloaded do not appear in Live's browser API, so they cannot be listed from here: anything you expect but do not see above must be installed from Live's Packs tab (Browser › Packs) or ableton.com/packs first.");
    Ok(out)
}

pub fn get_clip_info_body(live: &LiveState, p: &ClipRefParams) -> ToolResult {
    require(live, "get_clip_info")?;
    live.send_command(
        "get_clip_info",
        Some(json!({"track_index": p.track_index, "clip_index": p.clip_index, "arrangement": p.arrangement})),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("read the clip", e))
}

fn clip_settings_summary(r: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(l) = r.get("looping").and_then(Value::as_bool) {
        parts.push(if l {
            format!(
                "loop {}–{}",
                beat(r.get("loop_start")),
                beat(r.get("loop_end"))
            )
        } else {
            "loop off".to_string()
        });
    }
    if r.get("start_marker").is_some() {
        parts.push(format!(
            "plays {}–{}",
            beat(r.get("start_marker")),
            beat(r.get("end_marker"))
        ));
    }
    if let Some(m) = r.get("launch_mode_name").and_then(Value::as_str) {
        parts.push(format!("launch {m}"));
    }
    if let Some(q) = r.get("launch_quantization_name").and_then(Value::as_str) {
        parts.push(format!("quantize {q}"));
    }
    if r.get("legato").and_then(Value::as_bool) == Some(true) {
        parts.push("legato".into());
    }
    parts.join(", ")
}

pub fn set_clip_loop_body(live: &LiveState, p: &SetClipLoopParams) -> ToolResult {
    require(live, "set_clip_loop")?;
    if p.looping.is_none()
        && p.loop_start.is_none()
        && p.loop_end.is_none()
        && p.start_marker.is_none()
        && p.end_marker.is_none()
    {
        return Err(
            "Nothing to set: give looping, loop_start, loop_end, start_marker or end_marker."
                .into(),
        );
    }
    let r = live
        .send_command(
            "set_clip_loop",
            Some(json!({
                "track_index": p.track_index, "clip_index": p.clip_index, "arrangement": p.arrangement,
                "looping": p.looping, "loop_start": p.loop_start, "loop_end": p.loop_end,
                "start_marker": p.start_marker, "end_marker": p.end_marker,
            })),
        )
        .map_err(|e| live_err("set the clip loop", e))?;
    Ok(format!(
        "Clip '{}' now: {}",
        get_display(&r, "name", "clip"),
        clip_settings_summary(&r)
    ))
}

pub fn set_clip_launch_body(live: &LiveState, p: &SetClipLaunchParams) -> ToolResult {
    require(live, "set_clip_launch")?;
    if p.launch_mode.is_none()
        && p.launch_quantization.is_none()
        && p.legato.is_none()
        && p.velocity_amount.is_none()
    {
        return Err(
            "Nothing to set: give launch_mode, launch_quantization, legato or velocity_amount."
                .into(),
        );
    }
    let r = live
        .send_command(
            "set_clip_launch",
            Some(json!({
                "track_index": p.track_index, "clip_index": p.clip_index,
                "launch_mode": p.launch_mode, "launch_quantization": p.launch_quantization,
                "legato": p.legato, "velocity_amount": p.velocity_amount,
            })),
        )
        .map_err(|e| live_err("set the clip launch settings", e))?;
    Ok(format!(
        "Clip '{}' now: {}",
        get_display(&r, "name", "clip"),
        clip_settings_summary(&r)
    ))
}

fn meter_peak(t: &Value) -> f64 {
    ["left", "right", "level"]
        .iter()
        .filter_map(|k| t.get(*k).and_then(Value::as_f64))
        .fold(0.0, f64::max)
}

/// Live's own meter curve, asked for once per session. An older script (or a
/// Live that will not answer) leaves it empty and the readouts say "meter
/// units" instead of pretending the number is dB.
pub(crate) fn meter_scale(live: &LiveState) -> crate::song::MeterScale {
    {
        let cached = live.meter_scale.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(scale) = cached.as_ref() {
            return scale.clone();
        }
    }
    let scale = if live.script.has_capability("get_meter_scale") {
        live.send_command("get_meter_scale", None)
            .map(|r| crate::song::MeterScale::from_value(&r))
            .unwrap_or_default()
    } else {
        crate::song::MeterScale::default()
    };
    *live.meter_scale.lock().unwrap_or_else(|e| e.into_inner()) = Some(scale.clone());
    scale
}

/// One meter reading as dB on Live's meter scale.
fn meter_text(scale: &crate::song::MeterScale, v: f64) -> String {
    format!("{} dB", crate::song::fmt_db(scale.db(v), 1))
}

/// The line every meter readout ends with, so nobody has to guess what the
/// number is or where it was measured.
fn meter_footer(_scale: &crate::song::MeterScale) -> &'static str {
    "Peaks are post-fader, in dB on Live's meter scale (−70 dB at the bottom, 0 dB is full scale, +6 dB the top)."
}

fn meters_text(r: &Value, scale: &crate::song::MeterScale) -> String {
    let mut out = String::new();
    for (label, key) in [("tracks", "tracks"), ("returns", "returns")] {
        if let Some(list) = r.get(key).and_then(Value::as_array) {
            if list.is_empty() {
                continue;
            }
            out.push_str(&format!("{label}:\n"));
            for t in list {
                out.push_str(&format!(
                    "  {} '{}': {}{}\n",
                    get_display(t, "index", "?"),
                    get_display(t, "name", "?"),
                    meter_text(scale, meter_peak(t)),
                    if t.get("mute").and_then(Value::as_bool) == Some(true) {
                        " (muted)"
                    } else {
                        ""
                    }
                ));
            }
        }
    }
    if let Some(m) = r.get("master") {
        out.push_str(&format!("master: {}\n", meter_text(scale, meter_peak(m))));
    }
    out.push_str(meter_footer(scale));
    out.push('\n');
    out
}

pub fn get_track_meters_body(live: &LiveState, _p: &Empty) -> ToolResult {
    require(live, "get_track_meters")?;
    let r = live
        .send_command("get_track_meters", None)
        .map_err(|e| live_err("read the meters", e))?;
    let playing = r
        .get("is_playing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let scale = meter_scale(live);
    Ok(format!(
        "Levels right now ({}):\n{}",
        if playing {
            "playing"
        } else {
            "stopped — levels are only meaningful while playing; use play_and_measure"
        },
        meters_text(&r, &scale)
    ))
}

/// Play a stretch of the set, sample the meters while it plays, and report
/// each track's peak. Nothing in the set changes.
pub fn play_and_measure_body(live: &LiveState, p: &PlayAndMeasureParams) -> ToolResult {
    // Live's meter curve first, while nothing is playing: it is read once per
    // session and must not interrupt the measurement.
    let scale = meter_scale(live);
    guard_performance(
        live,
        "play_and_measure",
        "would stop and move the transport; read get_track_meters while it plays instead",
    )?;
    require(live, "get_track_meters")?;
    require(live, "start_playback")?;
    require(live, "stop_playback")?;
    let seconds = p.seconds.clamp(0.5, 10.0);
    let interval = std::time::Duration::from_millis(p.interval_ms.clamp(50, 1000) as u64);
    // start_playing would jump to the start marker; play_from continues from
    // the position, so the section asked for is the section measured.
    match p.start_time {
        Some(t) if live.script.has_capability("play_from") => {
            live.send_command("play_from", Some(json!({"time": t})))
                .map_err(|e| live_err("play from the position", e))?;
        }
        Some(t) => {
            require(live, "set_current_song_time")?;
            live.send_command("set_current_song_time", Some(json!({"time": t})))
                .map_err(|e| live_err("move the playhead", e))?;
            live.send_command("start_playback", None)
                .map_err(|e| live_err("start playback", e))?;
        }
        None => {
            live.send_command("start_playback", None)
                .map_err(|e| live_err("start playback", e))?;
        }
    }
    let deadline = Instant::now() + std::time::Duration::from_secs_f64(seconds);
    let mut peaks: std::collections::BTreeMap<String, (String, f64)> =
        std::collections::BTreeMap::new();
    let mut readings = 0usize;
    let mut last_err = None;
    while Instant::now() < deadline {
        std::thread::sleep(interval);
        match live.send_command("get_track_meters", None) {
            Ok(r) => {
                readings += 1;
                for (key, prefix) in [("tracks", "track"), ("returns", "return")] {
                    for t in r.get(key).and_then(Value::as_array).into_iter().flatten() {
                        let id = format!("{prefix} {}", get_display(t, "index", "?"));
                        let e = peaks
                            .entry(id)
                            .or_insert((get_display(t, "name", "?"), 0.0));
                        e.1 = e.1.max(meter_peak(t));
                    }
                }
                if let Some(m) = r.get("master") {
                    let e = peaks
                        .entry("zz master".into())
                        .or_insert(("Master".into(), 0.0));
                    e.1 = e.1.max(meter_peak(m));
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    if p.stop_after {
        let _ = live.send_command("stop_playback", None);
    }
    if readings == 0 {
        return Err(format!(
            "Could not read the meters while playing: {}",
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ));
    }
    let mut out = format!(
        "Played {seconds:.1} s{} and took {readings} readings. Peak per track:\n",
        match p.start_time {
            Some(t) if live.script.has_capability("play_from") => format!(" from beat {t}"),
            Some(t) => format!(" from the start marker (this Remote Script cannot play from beat {t}; reinstall it)"),
            None => " from the start marker".to_string(),
        }
    );
    let mut silent = Vec::new();
    for (id, (name, peak)) in &peaks {
        let id = id.trim_start_matches("zz ");
        out.push_str(&format!("  {id} '{name}': {}\n", meter_text(&scale, *peak)));
        if *peak < 0.01 && id.starts_with("track") {
            silent.push(name.clone());
        }
    }
    out.push_str(meter_footer(&scale));
    if !silent.is_empty() {
        out.push_str(&format!(
            "\nSilent during this stretch: {}.",
            silent.join(", ")
        ));
    }
    Ok(out)
}

fn automation_target_json(t: &AutomationTarget) -> Result<Value, String> {
    if t.mixer.is_none() && (t.device_index.is_none() || t.parameter_index.is_none()) {
        return Err(
            "target needs device_index and parameter_index, or mixer (volume, pan, send).".into(),
        );
    }
    Ok(json!({
        "device_index": t.device_index, "parameter_index": t.parameter_index,
        "mixer": t.mixer, "send_index": t.send_index,
    }))
}

pub fn set_clip_automation_body(live: &LiveState, p: &SetClipAutomationParams) -> ToolResult {
    require(live, "set_clip_automation")?;
    if p.arrangement {
        return Err("Live's API writes automation into Session clips only (an Arrangement clip answers \"Not a session clip\"). Automate the Session clip, then place it with arrange or duplicate_to_arrangement: the envelope travels with the clip.".into());
    }
    let target = automation_target_json(&p.target)?;
    let mut points: Vec<Value> = p
        .points
        .iter()
        .map(|pt| json!({"time": pt.time, "value": pt.value}))
        .collect();
    if let Some(r) = &p.ramp {
        if r.over <= 0.0 {
            return Err("ramp.over must be greater than 0".into());
        }
        points.push(json!({"time": r.start, "value": r.from}));
        points.push(json!({"time": r.start + r.over, "value": r.to}));
    }
    if points.is_empty() {
        return Err("Give points [{time, value}] or a ramp {from, to, over}.".into());
    }
    let r = live
        .send_command(
            "set_clip_automation",
            Some(json!({
                "track_index": p.track_index, "clip_index": p.clip_index, "arrangement": p.arrangement,
                "target": target, "points": points, "mode": p.mode, "resolution": p.resolution, "clear": p.clear,
            })),
        )
        .map_err(|e| live_err("write the automation", e))?;
    Ok(format!(
        "Automated {} in clip '{}': {} points as {} steps ({}); range {}",
        get_display(&r, "target", "the parameter"),
        get_display(&r, "clip", "clip"),
        get_display(&r, "points", "?"),
        get_display(&r, "steps_written", "?"),
        get_display(&r, "mode", &p.mode),
        get_display(&r, "range", "?")
    ))
}

pub fn get_clip_automation_body(live: &LiveState, p: &GetClipAutomationParams) -> ToolResult {
    require(live, "get_clip_automation")?;
    let target = automation_target_json(&p.target)?;
    live.send_command(
        "get_clip_automation",
        Some(json!({
            "track_index": p.track_index, "clip_index": p.clip_index, "arrangement": p.arrangement,
            "target": target, "resolution": p.resolution,
        })),
    )
    .map(|r| pretty(&r))
    .map_err(|e| live_err("read the automation", e))
}

// ── Capture: record the master and measure it ───────────────────────────────

/// Tempo and beats per bar from the set, with Live's defaults as fallback.
fn tempo_and_meter(live: &LiveState) -> (f64, f64) {
    match live.send_command("get_session_info", None) {
        Ok(s) => (
            s.get("tempo").and_then(Value::as_f64).unwrap_or(120.0),
            s.get("signature_numerator")
                .and_then(Value::as_f64)
                .unwrap_or(4.0),
        ),
        Err(_) => (120.0, 4.0),
    }
}

/// Stops the transport and the recording slot if the body leaves early.
struct CaptureGuard<'a> {
    live: &'a LiveState,
    slot: i64,
    armed: bool,
}

impl Drop for CaptureGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self
                .live
                .send_command("stop_capture", Some(json!({"slot": self.slot})));
        }
    }
}

fn capture_text(
    name: &str,
    slot: i64,
    path: &str,
    m: &crate::audio::Measurements,
    bars: i64,
    tempo: f64,
) -> String {
    let bars_txt: Vec<String> = m.rms_per_bar.iter().map(|v| format!("{v:.1}")).collect();
    let mut out = format!(
        "Captured '{name}' (Capture track, slot {slot}) — {bars} bars at {tempo:.0} BPM, {:.1} s, {}\n{path}\n{}RMS per bar: {}\nReading: {}.\n",
        m.duration_s,
        match m.channels { 1 => "mono".to_string(), 2 => "stereo".to_string(), n => format!("{n} channels") },
        analysis_text(m),
        bars_txt.join(" "),
        crate::audio::reading(m)
    );
    if path.contains("Live Recordings") || path.contains("Untitled") {
        out.push_str("This set is unsaved, so Live recorded into a temporary project folder; the file moves when you save the set.\n");
    }
    out.push_str(&format!(
        "Play it in Live by firing Capture slot {slot}; the folder may need its own access grant before the file can be opened. Nothing else in the set changed."
    ));
    out
}

/// Peak, RMS, crest, the octave balance and the low-to-high ratio: the
/// numbers a mix decision is actually made on.
fn analysis_text(m: &crate::audio::Measurements) -> String {
    let labels = crate::audio::octave_labels();
    let width = 6usize;
    let head: String = labels
        .iter()
        .map(|l| format!("{l:>width$}"))
        .collect::<Vec<_>>()
        .join("");
    let shares: String = m
        .octaves
        .iter()
        .map(|v| format!("{:>width$}", format!("{:.0}%", v * 100.0)))
        .collect::<Vec<_>>()
        .join("");
    format!(
        "peak {:.1} dBFS · RMS {:.1} dBFS · crest {:.1} dB · LF/HF {}{:.1} dB{}\nbands {}\n      {}\n",
        m.peak_dbfs,
        m.rms_dbfs,
        m.crest_db,
        if m.lf_hf_db > 0.0 { "+" } else { "" },
        m.lf_hf_db,
        m.stereo_correlation
            .map(|c| format!(" · stereo correlation {c:.2}"))
            .unwrap_or_default(),
        head,
        shares
    )
}

/// One take, recorded and measured. The slot is left holding it; the caller
/// decides whether it is a take worth keeping.
fn record_take(
    live: &LiveState,
    p: &CaptureMixParams,
    start: f64,
    tempo: f64,
    seconds_per_bar: f64,
) -> Result<(i64, String, f64), String> {
    let started = live
        .send_command(
            "start_capture",
            Some(json!({"start": start, "bars": p.bars, "name": p.name})),
        )
        .map_err(|e| live_err("start the capture", e))?;
    let slot = started.get("slot").and_then(Value::as_i64).unwrap_or(0);
    let confirmed = started
        .get("confirmed_at")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN);
    let mut guard = CaptureGuard {
        live,
        slot,
        armed: true,
    };
    let preroll_beats = started
        .get("preroll_beats")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    let budget = (p.bars as f64 * seconds_per_bar + preroll_beats * 60.0 / tempo + 5.0).min(90.0);
    let deadline = Instant::now() + std::time::Duration::from_secs_f64(budget);
    let status = loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let s = live
            .send_command("capture_status", Some(json!({"slot": slot})))
            .map_err(|e| live_err("read the capture status", e))?;
        let has_clip = s.get("has_clip").and_then(Value::as_bool).unwrap_or(false);
        let recording = s
            .get("is_recording")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let file = s.get("file_path").and_then(Value::as_str).unwrap_or("");
        if has_clip && !recording && !file.is_empty() {
            break s;
        }
        if Instant::now() > deadline {
            return Err(format!(
                "The capture did not finish within {budget:.0} s (recording: {recording}, clip: {has_clip}); it was stopped and nothing else changed."
            ));
        }
    };
    // Recording is done: stop the transport ourselves and disarm the guard.
    let _ = live.send_command("stop_capture", Some(json!({"slot": slot})));
    guard.armed = false;
    Ok((
        slot,
        status
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        confirmed,
    ))
}

pub fn capture_mix_body(live: &LiveState, p: &CaptureMixParams) -> ToolResult {
    guard_performance(live, "capture_mix", "would stop and move the transport")?;
    for cmd in [
        "ensure_capture_track",
        "start_capture",
        "capture_status",
        "stop_capture",
    ] {
        require(live, cmd)?;
    }
    if !(1..=64).contains(&p.bars) {
        return Err(format!("bars must be between 1 and 64, got {}", p.bars));
    }
    let (tempo, beats_per_bar) = tempo_and_meter(live);
    let start_bar = resolve_bar(live, beats_per_bar, p.start_bar.as_ref())?;
    let start = match (start_bar, p.start) {
        (Some(b), _) if b >= 1.0 => (b - 1.0) * beats_per_bar,
        (Some(b), _) => return Err(format!("start_bar {b} is before bar 1")),
        (None, Some(s)) if s >= 0.0 => s,
        (None, Some(_)) => return Err("start must be 0 or later".into()),
        (None, None) => 0.0,
    };
    let seconds_per_bar = 60.0 / tempo * beats_per_bar;
    let ensured = live
        .send_command("ensure_capture_track", None)
        .map_err(|e| live_err("prepare the Capture track", e))?;
    let track_index = ensured.get("index").and_then(Value::as_i64).unwrap_or(-1);
    let created = ensured
        .get("created")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // A take whose head is silent means the playhead had not arrived: it says
    // nothing about the music, so it is discarded and recorded again rather
    // than measured. Anything under an eighth of a bar is Live's own latency.
    let tolerance = seconds_per_bar / 8.0;
    let mut discarded: Vec<String> = Vec::new();
    let mut retried = String::new();
    let (slot, path, m) = loop {
        let (slot, path, confirmed) = record_take(live, p, start, tempo, seconds_per_bar)?;
        let audio = crate::audio::read_file(std::path::Path::new(&path))?;
        let m = crate::audio::measure(&audio, seconds_per_bar);
        // Silent from end to end is the set, not a failed take: nothing plays
        // there. Say so instead of blaming the playhead, and do not re-record.
        if m.peak_dbfs <= -60.0 {
            // Live plays Session clips over the Arrangement while the tracks
            // are "back to arranger": the timeline is silent and that is not
            // the capture's doing.
            let session = live
                .send_command("get_session_info", None)
                .ok()
                .and_then(|r| r.get("back_to_arranger").and_then(Value::as_bool))
                .unwrap_or(false);
            retried.push_str(&format!(
                "Nothing plays from {} for {} bar{}: the take is silent end to end. That is the set, not a failed capture.{}\n",
                bar_text(start, beats_per_bar),
                p.bars,
                if p.bars == 1 { "" } else { "s" },
                if session {
                    " The tracks are following Session clips, so the Arrangement is not what you hear — adv_back_to_arrangement puts them back on the timeline."
                } else {
                    " Check that something is placed there."
                }
            ));
            break (slot, path, m);
        }
        if m.leading_silence_s <= tolerance {
            if !discarded.is_empty() {
                retried = format!(
                    "The first take's opening {} was silent — the playhead had not reached {}. Discarded it and recorded again.\n",
                    discarded.join(" and "),
                    bar_text(start, beats_per_bar)
                );
            }
            if confirmed.is_finite() {
                retried.push_str(&format!(
                    "Playhead confirmed at beat {confirmed:.2} before recording began.\n"
                ));
            }
            break (slot, path, m);
        }
        discarded.push(format!("{:.2} s", m.leading_silence_s));
        let _ = live.send_command(
            "delete_clip",
            Some(json!({"track_index": track_index, "clip_index": slot})),
        );
        if discarded.len() >= 2 {
            return Err(format!(
                "Could not capture {} bar(s) from {}: both takes began before the playhead reached it ({} of silence at the head). Nothing was measured and no reading is offered — a capture that starts early is not a mix observation. Capture slot {slot} was cleared. Is Live's transport being driven from somewhere else?",
                p.bars,
                bar_text(start, beats_per_bar),
                discarded.join(" and ")
            ));
        }
    };
    let clip_name = format!(
        "{} @ {} | {:.1} dBFS",
        p.name,
        bar_text(start, beats_per_bar),
        m.peak_dbfs
    );
    if live.script.has_capability("set_clip_name") && track_index >= 0 {
        let _ = live.send_command(
            "set_clip_name",
            Some(json!({"track_index": track_index, "clip_index": slot, "name": clip_name})),
        );
    }
    let mut text = String::new();
    if created {
        text.push_str(&format!(
            "Created the Capture track at index {track_index} (audio, input Resampling, monitoring off, muted, armed).\n"
        ));
    }
    text.push_str(&retried);
    text.push_str(&capture_text(
        &format!("{} @ bar {}", p.name, bar_text(start, beats_per_bar)),
        slot,
        &path,
        &m,
        p.bars,
        tempo,
    ));
    // #63: the drift line travels, beside the reading this call already
    // returns — the set may have moved since the overview was written.
    if let Some(said) = drift_note_now(live) {
        text.push_str(&format!("\n{said}"));
    }
    Ok(text)
}

pub fn list_captures_body(live: &LiveState, _p: &Empty) -> ToolResult {
    require(live, "list_captures")?;
    let r = live
        .send_command("list_captures", None)
        .map_err(|e| live_err("list the captures", e))?;
    let caps = r
        .get("captures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if caps.is_empty() {
        return Ok(
            "No captures yet. capture_mix records a stretch of the arrangement and measures it."
                .into(),
        );
    }
    let mut out = format!("Captures on the Capture track ({}):\n", caps.len());
    for c in &caps {
        out.push_str(&format!(
            "  slot {}  {}  {} beats{}  {}\n",
            get_display(c, "slot", "?"),
            get_display(c, "name", "?"),
            beat(c.get("length")),
            if c.get("is_recording").and_then(Value::as_bool) == Some(true) {
                " (recording)"
            } else {
                ""
            },
            get_display(c, "file_path", "no file yet")
        ));
    }
    out.push_str("measure_capture(slot) re-reads one; fire the slot in Live to hear it.");
    Ok(out)
}

pub fn measure_capture_body(live: &LiveState, p: &MeasureCaptureParams) -> ToolResult {
    require(live, "capture_status")?;
    let status = live
        .send_command("capture_status", Some(json!({"slot": p.slot})))
        .map_err(|e| live_err("read the capture", e))?;
    if status.get("has_clip").and_then(Value::as_bool) != Some(true) {
        return Err(format!(
            "Slot {} of the Capture track holds no capture.",
            p.slot
        ));
    }
    if status.get("is_recording").and_then(Value::as_bool) == Some(true) {
        return Err(format!("Slot {} is still recording.", p.slot));
    }
    let path = status
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("");
    if path.is_empty() {
        return Err(format!(
            "Slot {} has no audio file (a MIDI clip, or Live has not written it yet).",
            p.slot
        ));
    }
    let (tempo, beats_per_bar) = tempo_and_meter(live);
    let seconds_per_bar = 60.0 / tempo * beats_per_bar;
    let audio = crate::audio::read_file(std::path::Path::new(path))?;
    let m = crate::audio::measure(&audio, seconds_per_bar);
    let length = status.get("length").and_then(Value::as_f64).unwrap_or(0.0);
    let bars = (length / beats_per_bar).round().max(1.0) as i64;
    Ok(capture_text(
        &get_display(&status, "name", "capture"),
        p.slot,
        path,
        &m,
        bars,
        tempo,
    ))
}

// ── Tracks and transport ────────────────────────────────────────────────────

/// Empty the open set back to what Live starts with.
///
/// The Live API has no File > New, so nothing here opens a document: this
/// clears the one that is open — every clip, every track, the locators, the
/// scene names — and puts the tempo, the tracks and the scene count back to
/// a new set's. It is how a test, a demo or a differential run starts from
/// the same place twice.
pub fn reset_set_body(live: &LiveState, p: &ResetSetParams) -> ToolResult {
    require(live, "reset_set")?;
    let mut params = serde_json::Map::new();
    if let Some(v) = p.tracks {
        if !(0..=64).contains(&v) {
            return Err(format!(
                "tracks: {v} — a fresh set has between 0 and 64 tracks"
            ));
        }
        params.insert("tracks".into(), json!(v));
    }
    if let Some(v) = p.scenes {
        if !(1..=128).contains(&v) {
            return Err(format!(
                "scenes: {v} — Live keeps at least one scene, and this stops at 128"
            ));
        }
        params.insert("scenes".into(), json!(v));
    }
    if let Some(v) = p.tempo {
        if !(20.0..=999.0).contains(&v) {
            return Err(format!("tempo: {v} BPM is outside Live's range (20-999)"));
        }
        params.insert("tempo".into(), json!(v));
    }
    if let Some(v) = p.returns {
        params.insert("returns".into(), json!(v));
    }
    // The memory is about the song that is about to stop existing: its
    // plan, its roles and its parked ideas all described tracks and clips
    // this call is deleting. Carrying it into the empty set would have the
    // next agent read a plan for music that is not there.
    let forgotten = crate::memory::open_for_set(live);
    let had = !forgotten.is_empty();
    let r = live
        .send_command("reset_set", Some(Value::Object(params)))
        .map_err(|e| live_err("reset the set", e))?;
    if had {
        live.songs.forget();
    }
    live.songs.start_over();
    let removed = r.get("removed").cloned().unwrap_or_else(|| json!({}));
    let count = |k: &str| removed.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    Ok(format!(
        "The set is back to a new one: {} tracks ({}), {} scenes, {} BPM. \
         Cleared {} session clip(s), {} Arrangement clip(s), {} track(s), {} locator(s). \
         The Live API has no File > New, so this emptied the open set rather than opening one — \
         and nothing is saved until you save it in Live (Cmd+S).\n\
         {}This set now remembers nothing: no overview, no notes, no roles (they were in the \
         track names that just went) and nothing parked. Start the new one the way you would \
         start any song — set_key and set_tempo first, then build_song — and write the overview \
         as you go with remember(overview: {{\"what_it_is\": …, \"plan\": …, \"tracks\": …, \
         \"next\": …}}), so the next session picks this up instead of guessing.",
        get_display(&r, "track_count", "?"),
        join_names(r.get("tracks")),
        get_display(&r, "scene_count", "?"),
        get_display(&r, "tempo", "?"),
        count("session_clips"),
        count("arrangement_clips"),
        count("tracks"),
        count("locators"),
        if had {
            format!(
                "The memory of '{}' went with it ({} overview key(s), {} note(s)) — it described \
                 tracks and clips that no longer exist. ",
                if forgotten.set_name.is_empty() {
                    "this set"
                } else {
                    &forgotten.set_name
                },
                forgotten.overview.len(),
                forgotten.notes.len()
            )
        } else {
            String::new()
        },
    ))
}

pub fn delete_track_body(live: &LiveState, p: &TrackParams) -> ToolResult {
    require(live, "delete_track")?;
    let target = resolve_track_target(live, p.track.as_ref(), None, p.track_index)?;
    if target.kind != "track" {
        return Err(format!(
            "delete_track deletes the song's tracks; {} is the {}. Live keeps the master, and a return goes from Live's own mixer.",
            target.as_argument(),
            target.kind
        ));
    }
    guard_delete(live, target.index, None)?;
    let r = live
        .send_command("delete_track", Some(json!({"track_index": target.index})))
        .map_err(|e| {
            let text = live_err("delete the track", e);
            // An older script passes Live's own bare refusal through; say
            // which rule it is rather than making the caller guess.
            if text.to_lowercase().contains("couldn't delete track")
                || text.to_lowercase().contains("could not delete track")
            {
                format!("{text} — Live keeps at least one track in a set, and will not delete a frozen track. Create the track that replaces it first, then delete this one.")
            } else {
                text
            }
        })?;
    Ok(format!(
        "Deleted track {} ('{}'); {} tracks remain and later indices moved down by one.",
        target.index,
        get_display(&r, "deleted", "track"),
        get_display(&r, "track_count", "?")
    ))
}

pub fn back_to_arrangement_body(live: &LiveState, _p: &Empty) -> ToolResult {
    guard_performance(
        live,
        "back_to_arrangement",
        "would stop every Session clip in favour of the timeline",
    )?;
    require(live, "back_to_arrangement")?;
    live.send_command("back_to_arrangement", None)
        .map_err(|e| live_err("return to the arrangement", e))?;
    Ok("Back to Arrangement: every track follows the timeline again.".into())
}

pub fn set_arrangement_loop_body(live: &LiveState, p: &SetArrangementLoopParams) -> ToolResult {
    require(live, "set_arrangement_loop")?;
    if p.start_bar.is_none()
        && p.bars.is_none()
        && p.start.is_none()
        && p.length.is_none()
        && p.enabled.is_none()
    {
        return Err("Give start_bar and bars (or start and length in beats), or enabled.".into());
    }
    let (_, bpb) = tempo_and_meter(live);
    let start = p.start.or_else(|| p.start_bar.map(|b| (b - 1.0) * bpb));
    let length = p.length.or_else(|| p.bars.map(|b| b * bpb));
    let r = live
        .send_command(
            "set_arrangement_loop",
            Some(json!({"start": start, "length": length, "enabled": p.enabled})),
        )
        .map_err(|e| live_err("set the arrangement loop", e))?;
    let start = r.get("loop_start").and_then(Value::as_f64).unwrap_or(0.0);
    let length = r.get("loop_length").and_then(Value::as_f64).unwrap_or(0.0);
    Ok(format!(
        "Arrangement loop {}: beats {} to {} ({} beats)",
        if r.get("loop").and_then(Value::as_bool) == Some(true) {
            "on"
        } else {
            "off"
        },
        beat(Some(&json!(start))),
        beat(Some(&json!(start + length))),
        beat(Some(&json!(length)))
    ))
}

// ── Batch and build_song ─────────────────────────────────────────────────────

macro_rules! named_tools {
    ($live:expr, $name:expr, $args:expr; $( $n:literal => ($P:ty, $body:path) ),* $(,)?) => {
        match $name {
            $( $n => {
                let p: $P = serde_json::from_value($args)
                    .map_err(|e| format!("{}: bad arguments: {e}", $n))?;
                $body($live, &p)
            } )*
            other => Err(format!("unknown tool `{other}`")),
        }
    };
}

// ── Performance ─────────────────────────────────────────────────────────────
//
// Claude plans, Live executes. start_performance turns on the guards below;
// cue hands a resolved timeline to the Remote Script, which runs it on its
// own clock; get_performance_state is how Claude sees where the set is.

fn one_bar_q() -> String {
    "1_bar".to_string()
}
fn ask_record() -> String {
    "ask".to_string()
}

params!(StartPerformanceParams {
    /// Scene to fire first, by name or index (optional: keeps whatever plays)
    scene: Option<Value>,
    /// Global launch quantization: "1_bar" (default), "2_bars", "4_bars", "8_bars", "1/2", "1/4" … or "none"
    quantization: String = "one_bar_q",
    /// The key, e.g. "F minor", when the set does not carry one (Live 12's scale setting is read automatically)
    key: Option<String>,
    /// Tempo to set before starting; only applied while the transport is stopped (ramp it in a cue otherwise)
    tempo: Option<f64>,
    /// Disarm every armed track first (default true): Live arms new MIDI tracks by itself, and an armed track with an empty slot records on a scene launch
    disarm: bool = "yes",
    /// Load Live's Limiter on the master (ceiling −0.3 dB) as a safety net
    limiter: bool = "bool::default",
    /// After every record_clip, estimate the key of what was played and re-key the other MIDI clips to it
    follow_key: bool = "bool::default",
    /// What to do with the Arrangement while this plays: "ask" (default — records from bar 1 when the Arrangement is empty, otherwise returns what is there and the choices), "after" (record after everything already there), "replace" (delete it and record from bar 1), "off" (do not record)
    record: String = "ask_record",
});
params!(GetContextParams {
    /// Also read the browser inventory (instruments, effects, packs); slower, so off by default
    include_library: bool = "bool::default",
    /// Return the raw JSON instead of the readout
    json: bool = "bool::default",
});
params!(KeepTrackPlayingParams {
    /// Track name or index
    track: Value,
    /// true (default) removes the stop buttons from the track's empty slots so scene launches leave it playing; false puts them back
    keep: bool = "yes",
});
params!(CancelCueParams {
    /// The cue id from the cue result or get_performance_state
    id: i64,
});
params!(FireSceneParams {
    /// Scene name or index
    scene: Value,
    /// The bar the launch must not miss: when that bar is closer than the round trip, a cue is scheduled for the next certain bar instead
    no_later_than: Option<i64>,
});
params!(FireClipParams {
    /// The index of the track containing the clip
    track_index: i64,
    /// The index of the clip slot containing the clip
    clip_index: i64,
    /// The bar the launch must not miss: when that bar is closer than the round trip, a cue is scheduled for the next certain bar instead
    no_later_than: Option<i64>,
});
params!(GetPerformanceStateParams {
    /// Add the next 32 bars: every cue step and phrase boundary
    bar_map: bool = "bool::default",
});
params!(SetSceneParams {
    /// Scene name or index
    scene: Value,
    /// New name
    name: Option<String>,
    /// Scene tempo (Live 11+)
    tempo: Option<f64>,
    /// Bars per phrase for "next_phrase" cues (default 16)
    phrase_bars: Option<i64>,
});
params!(ListenParams {
    /// Bars to listen for (default 1, max 16), from the next bar line
    bars: f64 = "one",
    /// Record the bars through the Capture track (never touches the transport) and add RMS and low/mid/high balance
    capture: bool = "bool::default",
});
params!(VaryClipParams {
    /// Track name or index
    track: Value,
    /// Session slot of the clip to vary
    clip: i64,
    /// "fill_last_bar", "ghost_notes", "invert_chords", "thin", "half_time" or "double_time"
    variation: String,
    /// Seed: the same seed gives the same variation (default 1)
    seed: u64 = "one_u64",
    /// Write the variation into this slot instead of in place (a new clip)
    to_slot: Option<i64>,
});
params!(RetimeClipParams {
    /// Track name or index
    track: Value,
    /// Session slot of the clip
    clip: i64,
    /// "half_time" or "double_time"
    to: String,
    /// Seed (default 1)
    seed: u64 = "one_u64",
    /// Write the retimed clip into this slot instead of in place
    to_slot: Option<i64>,
});
params!(GrooveClipParams {
    /// Track name or index
    track: Value,
    /// Session slot of the clip
    clip: i64,
    /// A groove from this set's Groove Pool, by name (a substring will do) or index; "none" removes the clip's groove
    groove: Value,
    /// The groove's timing amount 0–1 (how much of the groove's timing applies)
    amount: Option<f64>,
    /// The groove's random amount 0–1
    random: Option<f64>,
    /// The groove's velocity amount 0–1
    velocity: Option<f64>,
});
params!(GrooveAmountParams {
    /// The set's global groove amount, 0–1 (Live's Groove Pool "Amount")
    value: f64,
});
params!(HumanizeParams {
    /// Track name or index
    track: Value,
    /// Session slot of the clip
    clip: i64,
    /// How far a hit may land early or late, in milliseconds at the current tempo (default 12)
    timing_ms: f64 = "twelve",
    /// How much a velocity may vary, up or down (default 15); off-beat notes vary more
    velocity: i64 = "fifteen",
    /// Seed: the same seed gives the same feel (default 1)
    seed: u64 = "one_u64",
});
params!(SwingNotesParams {
    /// Track name or index
    track: Value,
    /// Session slot of the clip
    clip: i64,
    /// How far the off-beat steps are delayed, as a fraction of the grid step (0.5 = halfway to the next step; 0.33 is a classic swing)
    amount: f64,
    /// The grid: "1/16" (default) or "1/8"
    grid: String = "sixteenth_grid",
    /// Seed (unused by a plain swing; kept so the call matches humanize)
    seed: u64 = "one_u64",
});
fn twelve() -> f64 {
    12.0
}
fn fifteen() -> i64 {
    15
}
fn sixteenth_grid() -> String {
    "1/16".to_string()
}
params!(UndoVaryParams {
    /// Track name or index
    track: Value,
    /// Session slot varied in place
    clip: i64,
});
params!(FollowKeyParams {
    /// Track name or index holding the recording (record_clip's track)
    track: Value,
    /// Its slot
    clip: i64,
});
params!(RestoreMixParams {
    /// Snapshot id from snapshot_mix
    id: i64,
});
params!(PanicParams {
    /// Tracks to keep, by name or index
    keep: Vec<Value> = "no_keep",
    /// Bars to fade over (default 1)
    bars: f64 = "one",
});
params!(CreateSceneParams {
    /// Where to insert the scene (-1 = at the end)
    index: i64 = "minus_one",
    /// Scene name
    name: Option<String>,
    /// A scene tempo (Live 11+); the scene sets it when fired
    tempo: Option<f64>,
    /// Bars per phrase for "next_phrase" cues (default 16)
    phrase_bars: Option<i64>,
});
params!(RecordClipParams {
    /// Track name or index (a MIDI or audio track that can be armed)
    track: Value,
    /// Bars to record (default 4, max 64); the clip loops when done
    bars: i64 = "four_i",
    /// Clip name (default "take")
    name: Option<String>,
    /// The bar the recording must start on at the latest; a cue is scheduled when the bar is too close
    no_later_than: Option<i64>,
});
fn one_u64() -> u64 {
    1
}
fn no_queries() -> Vec<String> {
    Vec::new()
}
fn no_keep() -> Vec<Value> {
    Vec::new()
}
params!(SetLaunchQuantizationParams {
    /// "none", "8_bars", "4_bars", "2_bars", "1_bar", "1/2", "1/2t", "1/4", "1/4t", "1/8", "1/8t", "1/16", "1/16t" or "1/32"
    quantization: String,
});

/// One track's crossfader side.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct CrossfadeAssign {
    /// Track name or index
    pub track: Value,
    /// "A", "B" or "none"
    pub side: String,
}
params!(SetCrossfaderParams {
    /// Crossfader position: 0 = A, 1 = B
    value: Option<f64>,
    /// Tracks to assign to a side
    assign: Vec<CrossfadeAssign> = "no_assign",
});
params!(EndPerformanceParams {
    /// Stop on a bar: "next_bar", {"bar": N} or {"bars_after": k} (the default is next_bar)
    at: Option<perf::CueTime>,
    /// Fade the master to silence over this many bars from the next bar, then stop
    fade_bars: Option<f64>,
    /// Stop immediately, mid-bar
    now: bool = "bool::default",
});

fn four_i() -> i64 {
    4
}
fn no_assign() -> Vec<CrossfadeAssign> {
    Vec::new()
}

pub(crate) fn read_perf_state(live: &LiveState) -> Result<PerfState, String> {
    require(live, "get_performance_state")?;
    let v = live
        .send_command("get_performance_state", None)
        .map_err(|e| live_err("read the performance state", e))?;
    PerfState::from_value(&v)
}

pub(crate) fn performance_running(live: &LiveState) -> Option<Performance> {
    live.performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

const END_OPTIONS: &str = "Use end_performance {\"at\": \"next_bar\"} to stop on the bar, end_performance {\"fade_bars\": 8} to fade the master and stop, or end_performance {\"now\": true} if you really mean now.";

/// The guard every transport-touching tool calls first: while a performance
/// runs, the call is refused with the on-the-bar alternative in the message.
/// A performance nobody is playing is reconciled away first — the server
/// outlives a conversation, and state from this morning must not block work
/// tonight.
fn guard_performance(live: &LiveState, tool: &str, harm: &str) -> Result<(), String> {
    let Some(p) = performance_running(live) else {
        return Ok(());
    };
    if let Some(note) = reconcile_performance(live, &p) {
        add_note(live, note);
        return Ok(());
    }
    Err(format!(
        "A performance is running (since bar {}, started {}, transport playing). {tool} {harm}. {END_OPTIONS}",
        p.start_bar,
        p.started_at.format("%H:%M:%S")
    ))
}

/// How long a performance may sit with the transport stopped before the
/// server stops believing in it.
const STALE_PERFORMANCE_S: i64 = 60;

/// Ends a performance that is not being played — the transport stopped, no cue
/// pending, and more than a minute old — and returns the line the reply
/// carries. `None` leaves the performance alone.
fn reconcile_performance(live: &LiveState, p: &Performance) -> Option<String> {
    let age = (chrono::Local::now() - p.started_at).num_seconds();
    if age < STALE_PERFORMANCE_S {
        return None;
    }
    let state = read_perf_state(live).ok()?;
    if state.is_playing || !state.cues.is_empty() {
        return None;
    }
    *live.performance.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Some(format!(
        "A performance was left running from {} ago and Live's transport has been stopped since — I ended it.",
        crate::performance::fmt_secs(age as f64)
    ))
}

/// A line a guard wants on the reply of whatever tool it let through.
pub(crate) fn add_note(live: &LiveState, note: String) {
    live.notes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(note);
}

fn take_notes(live: &LiveState) -> Vec<String> {
    std::mem::take(&mut *live.notes.lock().unwrap_or_else(|e| e.into_inner()))
}

/// delete_track / delete_clip: allowed during a performance only when the
/// target is neither playing nor queued.
fn guard_delete(live: &LiveState, track_index: i64, clip_index: Option<i64>) -> Result<(), String> {
    if performance_running(live).is_none() {
        return Ok(());
    }
    let state = read_perf_state(live)?;
    if let Some(t) = state.tracks.iter().find(|t| t.index == track_index) {
        let hits = |i: i64| i >= 0 && clip_index.is_none_or(|c| c == i);
        if hits(t.playing_slot_index) || hits(t.fired_slot_index) || t.is_recording {
            let what = match clip_index {
                Some(c) => format!("slot {c} on '{}'", t.name),
                None => format!("track '{}'", t.name),
            };
            return Err(format!(
                "A performance is running and {what} is playing or queued. Stop it on the bar first (a cue with stop_clip, or fire another scene), then delete."
            ));
        }
    }
    Ok(())
}

fn key_line(state: &PerfState, given: Option<&String>) -> (Option<String>, String) {
    match (given.filter(|k| !k.trim().is_empty()), state.key_from_set()) {
        (Some(k), _) => (Some(k.trim().to_string()), format!("key {}", k.trim())),
        (None, Some(k)) => (Some(k.clone()), format!("key {k} (from your set's scale)")),
        _ => (
            None,
            "key not set (tell me the key, or set the scale in Live 12)".into(),
        ),
    }
}

pub fn get_context_body(live: &LiveState, p: &GetContextParams) -> ToolResult {
    require(live, "get_context")?;
    let ctx = live
        .send_command(
            "get_context",
            Some(json!({"include_library": p.include_library})),
        )
        .map_err(|e| live_err("read the set", e))?;
    // Every learned device fact is stamped with the Live it was measured on.
    live.note_live_version(&get_display(&ctx, "live_version", ""));
    if p.json {
        return Ok(pretty(&ctx));
    }
    let since = performance_running(live).map(|r| {
        (
            r.start_bar,
            (chrono::Local::now() - r.started_at).num_milliseconds() as f64 / 1000.0,
        )
    });
    let text = crate::context::context_text(&ctx, since);
    let text = text.replace(
        crate::context::FOOTER,
        &format!(
            "{} · round trip {:.2} s\n{}",
            live.library.status_line(),
            live.round_trip_s(),
            crate::context::FOOTER
        ),
    );
    // #63: the whole memory rides back on the call the agent makes first
    // anyway. A "load my memory" call is one an agent can fail to make, and
    // the turn it skips it on is the first turn of a session.
    Ok(format!("{}{text}", song_memory_header(live, &ctx)))
}

/// The one drift line, for the replies that are not `get_context`.
///
/// `get_context` is called once per session — which is why the overview
/// rides there, and exactly why the staleness check cannot ride there alone.
/// A set the producer nudges mid-session is noticed without a second call.
pub(crate) fn drift_note(live: &LiveState, state: &PerfState) -> Option<String> {
    let memory = live.songs.snapshot()?;
    if memory.overview.is_empty() {
        return None;
    }
    crate::memory::drift_line(&memory.as_of, &crate::memory::as_of_from(state))
}

/// The same, for a caller with no state in hand. The overview check comes
/// first and is free, so a set with nothing remembered pays no round trip
/// and a tool that refuses before Live still refuses before Live.
pub(crate) fn drift_note_now(live: &LiveState) -> Option<String> {
    let memory = live.songs.snapshot()?;
    if memory.overview.is_empty() {
        return None;
    }
    drift_note(live, &read_perf_state(live).ok()?)
}

/// The song-memory header `get_context` leads with — empty when there is
/// nothing to say.
///
/// **Everything but the set's identity comes out of the `get_context`
/// payload already in hand.** `get_context` is one round trip for the set
/// and that is the point of it; a header that read the state again would
/// have cost three. The one op it does add is `song.file_path`, which is
/// what says *which song* this is and has nowhere else to come from.
fn song_memory_header(live: &LiveState, ctx: &Value) -> String {
    let memory = crate::memory::open_for_set(live);
    // The digest is written whether or not anything was remembered on
    // purpose, so a session that never called `remember` still leaves a trace.
    live.songs.flush_digest();
    let session = ctx.get("session").cloned().unwrap_or(Value::Null);
    let scenes = ctx
        .get("scenes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let now = crate::memory::AsOf {
        tempo: session.get("tempo").and_then(Value::as_f64).unwrap_or(0.0),
        key: match (
            session.get("root_note_name").and_then(Value::as_str),
            session.get("scale_name").and_then(Value::as_str),
        ) {
            (Some(r), Some(k)) if !r.is_empty() && !k.is_empty() => format!("{r} {k}"),
            _ => String::new(),
        },
        sections: scenes
            .iter()
            .filter(|sc| !crate::song::is_reserved_scene(&get_display(sc, "name", "")))
            .count(),
        tracks: ctx
            .get("tracks")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        at: crate::memory::now(),
    };
    let names: Vec<String> = ctx
        .get("tracks")
        .and_then(Value::as_array)
        .map(|ts| ts.iter().map(|t| get_display(t, "name", "")).collect())
        .unwrap_or_default();
    // The parked ideas are the clips in the Stash: rows, which the payload
    // already counted.
    let stashed: usize = scenes
        .iter()
        .filter(|sc| crate::song::is_stash_scene(&get_display(sc, "name", "")))
        .map(|sc| {
            sc.get("clip_count")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| {
                    sc.get("clip_tracks")
                        .and_then(Value::as_array)
                        .map_or(0, |t| t.len() as u64)
                }) as usize
        })
        .sum();
    let renamed = live
        .songs
        .renamed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let mut text = crate::memory::header_text(&crate::memory::Header {
        memory: Some(&memory),
        full: live.songs.take_full(),
        now: &now,
        roles: crate::memory::roles(&names),
        stashed,
        renamed,
    });
    // A note whose track was renamed is reported, never repointed by guess.
    let mut subjects: Vec<String> = memory
        .notes
        .iter()
        .map(|n| n.about.clone())
        .filter(|a| a != "song" && !a.starts_with("section:"))
        .collect();
    subjects.sort();
    subjects.dedup();
    for moved in crate::memory::reconcile(&subjects, &names) {
        text.push_str(&format!("{}\n", moved.line()));
    }
    text
}

pub fn set_scene_body(live: &LiveState, p: &SetSceneParams) -> ToolResult {
    require(live, "set_scene")?;
    if p.name.is_none() && p.tempo.is_none() && p.phrase_bars.is_none() {
        return Err("Give name, tempo and/or phrase_bars.".into());
    }
    let state = read_perf_state(live)?;
    let scene = state.scene_by(&p.scene)?;
    let r = live
        .send_command(
            "set_scene",
            Some(json!({"index": scene.index, "name": p.name, "tempo": p.tempo, "phrase_bars": p.phrase_bars})),
        )
        .map_err(|e| live_err("set the scene", e))?;
    Ok(format!(
        "Scene {} '{}': phrase {} bars{}{}.",
        scene.index,
        get_display(&r, "name", ""),
        get_display(&r, "phrase_bars", "16"),
        if r.get("phrase_default") == Some(&json!(true)) {
            " (default)"
        } else {
            ""
        },
        r.get("tempo")
            .and_then(Value::as_f64)
            .filter(|t| *t > 0.0)
            .map(|t| format!(", tempo {t}"))
            .unwrap_or_default()
    ))
}

/// The loudest of a track's meters as the script reports them (`left`,
/// `right`, `level`, the `output_meter_` prefix stripped).
fn meter_level(v: &Value) -> f64 {
    meter_peak(v)
}

pub fn listen_body(live: &LiveState, p: &ListenParams) -> ToolResult {
    require(live, "get_track_meters")?;
    if !(0.25..=16.0).contains(&p.bars) {
        return Err(format!("bars must be between 0.25 and 16, got {}", p.bars));
    }
    let state = read_perf_state(live)?;
    if !state.is_playing {
        return Err("Nothing is playing; listen needs the transport running.".into());
    }
    let bar_s = state.seconds_per_beat() * state.beats_per_bar();
    let start_bar = state.next_bar();
    if p.capture {
        for c in ["start_live_capture", "capture_status"] {
            require(live, c)?;
        }
        let started = live
            .send_command(
                "start_live_capture",
                Some(json!({"bars": p.bars.ceil() as i64, "name": "listen"})),
            )
            .map_err(|e| live_err("start the live capture", e))?;
        let slot = started.get("slot").and_then(Value::as_i64).unwrap_or(0);
        let budget = state.seconds_to_next_bar() + p.bars.ceil() * bar_s + 6.0;
        let deadline = Instant::now() + std::time::Duration::from_secs_f64(budget.min(120.0));
        let status = loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let s = live
                .send_command("capture_status", Some(json!({"slot": slot})))
                .map_err(|e| live_err("read the capture status", e))?;
            let has_clip = s.get("has_clip").and_then(Value::as_bool).unwrap_or(false);
            let recording = s
                .get("is_recording")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let file = s.get("file_path").and_then(Value::as_str).unwrap_or("");
            if has_clip && !recording && !file.is_empty() {
                break s;
            }
            if Instant::now() > deadline {
                return Err(format!("The live capture did not finish within {budget:.0} s; the transport was never touched."));
            }
        };
        let path = status
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let audio = crate::audio::read_file(std::path::Path::new(&path))?;
        let m = crate::audio::measure(&audio, bar_s);
        let bands = crate::audio::band_balance(&audio);
        let lands = started
            .get("lands_on_bar")
            .and_then(Value::as_i64)
            .unwrap_or(start_bar);
        return Ok(format!(
            "Listened by recording {} bar{} from bar {lands} (Capture slot {slot}, {:.1} s):\n{}  low {:.0}% · mid {:.0}% · high {:.0}%{}{}\nRMS per bar: {}\nReading: {}.\nThe clip stays on the Capture track; the transport was never touched.",
            p.bars.ceil() as i64,
            if p.bars.ceil() as i64 == 1 { "" } else { "s" },
            m.duration_s,
            analysis_text(&m),
            bands.low * 100.0,
            bands.mid * 100.0,
            bands.high * 100.0,
            if m.clipped_samples > 0 { format!(" · {} clipped samples", m.clipped_samples) } else { String::new() },
            match m.stereo_correlation { Some(c) if c < 0.1 => " · very wide/mono-unsafe".to_string(), _ => String::new() },
            m.rms_per_bar.iter().map(|x| format!("{x:.1}")).collect::<Vec<_>>().join(" "),
            crate::audio::reading(&m)
        ));
    }
    let scale = meter_scale(live);
    let wait = state.seconds_to_next_bar().clamp(0.0, 10.0);
    std::thread::sleep(std::time::Duration::from_secs_f64(wait));
    let deadline = Instant::now() + std::time::Duration::from_secs_f64(p.bars * bar_s);
    let mut peaks: BTreeMap<String, (f64, f64, usize)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut readings = 0usize;
    loop {
        let r = live
            .send_command("get_track_meters", None)
            .map_err(|e| live_err("read the meters", e))?;
        readings += 1;
        let mut entries: Vec<(String, f64)> = Vec::new();
        for key in ["tracks", "returns"] {
            if let Some(a) = r.get(key).and_then(Value::as_array) {
                for t in a {
                    entries.push((get_display(t, "name", "?"), meter_level(t)));
                }
            }
        }
        if let Some(m) = r.get("master") {
            entries.push(("Master".into(), meter_level(m)));
        }
        for (name, level) in entries {
            if !order.contains(&name) {
                order.push(name.clone());
            }
            let e = peaks.entry(name).or_insert((0.0, 0.0, 0));
            e.0 = e.0.max(level);
            e.1 += level;
            e.2 += 1;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let width = order
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(6)
        .clamp(6, 24);
    let mut out = format!(
        "Listened for {} bar{} from bar {start_bar} ({:.1} s, {readings} meter readings):\n",
        p.bars,
        if p.bars == 1.0 { "" } else { "s" },
        p.bars * bar_s
    );
    for name in &order {
        let (peak, sum, n) = peaks[name];
        let avg = if n > 0 { sum / n as f64 } else { 0.0 };
        if peak <= 0.0001 {
            continue;
        }
        let flag = if name == "Master" {
            match scale.db(peak) {
                db if db >= 0.0 => "   ← over 0 dB: clipping",
                db if db > -0.5 => "   ← within 0.5 dB of 0 dB",
                _ => "",
            }
        } else {
            ""
        };
        out.push_str(&format!(
            "  {name:<width$} peak {}  avg {}{flag}\n",
            meter_text(&scale, peak),
            meter_text(&scale, avg)
        ));
    }
    let silent: Vec<&String> = order.iter().filter(|n| peaks[*n].0 <= 0.0001).collect();
    if !silent.is_empty() {
        out.push_str(&format!(
            "  silent: {}\n",
            silent
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out.push_str(meter_footer(&scale));
    out.push_str(" listen {\"capture\": true} records the bars instead and adds RMS, crest and the octave balance.");
    Ok(out)
}

pub(crate) fn clip_notes(
    live: &LiveState,
    track_index: i64,
    clip_index: i64,
) -> Result<Vec<Value>, String> {
    require(live, "get_clip_notes")?;
    let r = live
        .send_command(
            "get_clip_notes",
            Some(json!({"track_index": track_index, "clip_index": clip_index})),
        )
        .map_err(|e| live_err("read the clip's notes", e))?;
    Ok(r.get("notes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

pub(crate) fn write_notes(
    live: &LiveState,
    track_index: i64,
    clip_index: i64,
    notes: &[Value],
) -> Result<(), String> {
    run_named(
        live,
        "add_notes_to_clip",
        json!({"track_index": track_index, "clip_index": clip_index, "notes": notes, "clear": true}),
    )
    .map(|_| ())
}

pub fn vary_clip_body(live: &LiveState, p: &VaryClipParams) -> ToolResult {
    if !crate::variation::VARIATIONS.contains(&p.variation.as_str()) {
        return Err(format!(
            "variation must be one of {}",
            crate::variation::VARIATIONS.join(", ")
        ));
    }
    let state = read_perf_state(live)?;
    let track = state.track_by(&p.track)?;
    if !track.slots_with_clips.contains(&p.clip) {
        return Err(format!("slot {} on '{}' holds no clip", p.clip, track.name));
    }
    let raw = clip_notes(live, track.index, p.clip)?;
    let notes: Vec<crate::variation::VNote> = raw
        .iter()
        .filter_map(crate::variation::VNote::from_value)
        .collect();
    if notes.is_empty() {
        return Err(format!(
            "'{}' slot {} has no notes to vary",
            track.name, p.clip
        ));
    }
    require(live, "get_clip_info")?;
    let info = live
        .send_command(
            "get_clip_info",
            Some(json!({"track_index": track.index, "clip_index": p.clip, "arrangement": false})),
        )
        .map_err(|e| live_err("read the clip", e))?;
    let length = info
        .get("length")
        .and_then(Value::as_f64)
        .filter(|l| *l > 0.0)
        .unwrap_or_else(|| {
            notes
                .iter()
                .map(|n| n.start + n.duration)
                .fold(0.0, f64::max)
                .ceil()
                .max(1.0)
        });
    let varied =
        crate::variation::vary(&notes, &p.variation, p.seed, length, state.beats_per_bar())?;
    let values: Vec<Value> = varied.iter().map(|n| n.to_value()).collect();
    match p.to_slot {
        Some(slot) => {
            if track.slots_with_clips.contains(&slot) {
                return Err(format!(
                    "slot {slot} on '{}' already holds a clip; pick an empty slot or vary in place",
                    track.name
                ));
            }
            let input: NotesInput =
                serde_json::from_value(json!({"notes": values})).map_err(|e| e.to_string())?;
            create_clip_body(
                live,
                &CreateClipParams {
                    track_index: Some(track.index),
                    clip_index: Some(slot),
                    length,
                    name: format!("{} ({})", get_display(&info, "name", "clip"), p.variation),
                    input,
                    ..Default::default()
                },
            )?;
            Ok(format!(
                "Varied '{}' slot {} → slot {slot} ({}, seed {}): {} notes → {}. Fire slot {slot} to hear it; the original is untouched.",
                track.name, p.clip, p.variation, p.seed, notes.len(), varied.len()
            ))
        }
        None => {
            live.vary_undo
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((track.index, p.clip), raw.clone());
            write_notes(live, track.index, p.clip, &values)?;
            Ok(format!(
                "Varied '{}' slot {} in place ({}, seed {}): {} notes → {}. undo_vary restores the previous notes.",
                track.name, p.clip, p.variation, p.seed, notes.len(), varied.len()
            ))
        }
    }
}

/// The clip's notes as [`crate::variation::VNote`]s, refusing an empty clip.
fn clip_vnotes(
    live: &LiveState,
    track: &perf::TrackState,
    clip: i64,
) -> Result<(Vec<Value>, Vec<crate::variation::VNote>), String> {
    if !track.slots_with_clips.contains(&clip) {
        return Err(format!("slot {clip} on '{}' holds no clip", track.name));
    }
    let raw = clip_notes(live, track.index, clip)?;
    let notes: Vec<crate::variation::VNote> = raw
        .iter()
        .filter_map(crate::variation::VNote::from_value)
        .collect();
    if notes.is_empty() {
        return Err(format!(
            "'{}' slot {clip} has no notes to work on",
            track.name
        ));
    }
    Ok((raw, notes))
}

/// Write varied notes in place, keeping the previous ones for undo_vary.
fn write_varied(
    live: &LiveState,
    track_index: i64,
    clip: i64,
    previous: Vec<Value>,
    notes: &[crate::variation::VNote],
) -> Result<(), String> {
    live.vary_undo
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((track_index, clip), previous);
    let values: Vec<Value> = notes.iter().map(|n| n.to_value()).collect();
    write_notes(live, track_index, clip, &values)
}

pub fn humanize_body(live: &LiveState, p: &HumanizeParams) -> ToolResult {
    if !(0.0..=200.0).contains(&p.timing_ms) {
        return Err(format!(
            "timing_ms must be between 0 and 200, got {}",
            p.timing_ms
        ));
    }
    if !(0..=127).contains(&p.velocity) {
        return Err(format!(
            "velocity must be between 0 and 127, got {}",
            p.velocity
        ));
    }
    let state = read_perf_state(live)?;
    let track = state.track_by(&p.track)?;
    let (raw, notes) = clip_vnotes(live, track, p.clip)?;
    let timing_beats = p.timing_ms / 1000.0 * state.tempo / 60.0;
    let varied = crate::variation::humanize(
        &notes,
        timing_beats,
        p.velocity,
        p.seed,
        state.beats_per_bar(),
    );
    write_varied(live, track.index, p.clip, raw, &varied)?;
    let off_beat = notes
        .iter()
        .filter(|n| (n.start - n.start.round()).abs() > 0.01)
        .count();
    Ok(format!(
        "{} slot {} humanized (seed {}): hits now land up to {} ms early or late — {} at {} BPM — so they drift around the grid instead of sitting on it; velocities vary ±{}{}. undo_vary puts them back.",
        track.name,
        p.clip,
        p.seed,
        perf_num(p.timing_ms),
        crate::variation::note_value_words(timing_beats),
        perf_num(state.tempo),
        p.velocity,
        if off_beat > 0 { ", the off-beat notes most" } else { "" }
    ))
}

pub fn swing_notes_body(live: &LiveState, p: &SwingNotesParams) -> ToolResult {
    if !(0.0..=1.0).contains(&p.amount) {
        return Err(format!("amount must be between 0 and 1, got {}", p.amount));
    }
    let step = match p.grid.trim() {
        "1/16" | "16" | "16th" | "16ths" => 0.25,
        "1/8" | "8" | "8th" | "8ths" => 0.5,
        other => return Err(format!("grid must be \"1/16\" or \"1/8\", not '{other}'")),
    };
    let state = read_perf_state(live)?;
    let track = state.track_by(&p.track)?;
    let (raw, notes) = clip_vnotes(live, track, p.clip)?;
    let swung = crate::variation::swing(&notes, p.amount, step);
    let moved = swung
        .iter()
        .zip(notes.iter())
        .filter(|(a, b)| (a.start - b.start).abs() > 1e-9)
        .count();
    if moved == 0 {
        return Err(format!(
            "'{}' slot {} has no notes on the off-beat {} steps, so a swing changes nothing",
            track.name,
            p.clip,
            if step == 0.25 { "16th" } else { "8th" }
        ));
    }
    write_varied(live, track.index, p.clip, raw, &swung)?;
    Ok(format!(
        "{} slot {} swung: the off-beat {}s now lag by {} ({}% of the step); the on-beat notes stay where they were. undo_vary straightens them.",
        track.name,
        p.clip,
        if step == 0.25 { "16th" } else { "8th" },
        crate::variation::note_value_words(p.amount * step),
        (p.amount * 100.0).round() as i64
    ))
}

const NO_GROOVE_HELP: &str = "the API cannot add one; drag one in from the browser (Grooves) so it appears in the pool, or use humanize / swing_notes (a note rewrite, undoable)";

pub fn groove_clip_body(live: &LiveState, p: &GrooveClipParams) -> ToolResult {
    for c in ["get_grooves", "set_clip_groove"] {
        require(live, c)?;
    }
    for (name, v) in [
        ("amount", p.amount),
        ("random", p.random),
        ("velocity", p.velocity),
    ] {
        if let Some(v) = v {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("{name} must be between 0 and 1, got {v}"));
            }
        }
    }
    let state = read_perf_state(live)?;
    let track = state.track_by(&p.track)?;
    if !track.slots_with_clips.contains(&p.clip) {
        return Err(format!("slot {} on '{}' holds no clip", p.clip, track.name));
    }
    let pool = live
        .send_command("get_grooves", None)
        .map_err(|e| live_err("read the Groove Pool", e))?;
    if let Some(e) = pool.get("error").and_then(Value::as_str) {
        return Err(format!("{e}; use humanize / swing_notes instead."));
    }
    let grooves = pool
        .get("grooves")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names: Vec<String> = grooves
        .iter()
        .map(|g| get_display(g, "name", "?"))
        .collect();
    let remove = matches!(&p.groove, Value::String(s) if s.trim().eq_ignore_ascii_case("none") || s.trim().is_empty());
    let index: Option<i64> = if remove {
        None
    } else {
        match &p.groove {
            Value::Number(n) => {
                let i = n.as_i64().unwrap_or(-1);
                if i < 0 || i >= grooves.len() as i64 {
                    return Err(format!(
                        "no groove {i} in the pool ({} groove{}: {})",
                        grooves.len(),
                        if grooves.len() == 1 { "" } else { "s" },
                        names.join(", ")
                    ));
                }
                Some(i)
            }
            Value::String(s) => {
                let want = s.trim().to_lowercase();
                if grooves.is_empty() {
                    return Err(format!(
                        "No groove named '{}' in this set's Groove Pool (it is empty) and {NO_GROOVE_HELP}.",
                        s.trim()
                    ));
                }
                let hit = names
                    .iter()
                    .position(|n| n.to_lowercase() == want)
                    .or_else(|| names.iter().position(|n| n.to_lowercase().contains(&want)));
                match hit {
                    Some(i) => Some(i as i64),
                    None => {
                        return Err(format!(
                            "No groove named '{}' in this set's Groove Pool and {NO_GROOVE_HELP}. In the pool: {}.",
                            s.trim(),
                            names.join(", ")
                        ))
                    }
                }
            }
            other => {
                return Err(format!(
                    "groove must be a name, an index or \"none\", not {other}"
                ))
            }
        }
    };
    let r = live
        .send_command(
            "set_clip_groove",
            Some(json!({"track_index": track.index, "clip_index": p.clip, "groove_index": index.unwrap_or(-1),
                        "timing": p.amount, "random": p.random, "velocity": p.velocity})),
        )
        .map_err(|e| live_err("assign the groove", e))?;
    match r.get("groove").filter(|g| g.is_object()) {
        Some(g) => {
            let mut amounts = Vec::new();
            for (k, label) in [
                ("timing_amount", "timing"),
                ("random_amount", "random"),
                ("velocity_amount", "velocity"),
            ] {
                if let Some(v) = g.get(k).and_then(Value::as_f64) {
                    amounts.push(format!("{label} {}%", (v * 100.0).round() as i64));
                }
            }
            Ok(format!(
                "{} slot {}: groove '{}' from the Groove Pool{} (non-destructive; Live's own groove, shared by every clip that uses it). Set the pool's global amount with groove_amount{}.",
                track.name,
                p.clip,
                get_display(g, "name", "?"),
                if amounts.is_empty() { String::new() } else { format!(" at {}", amounts.join(", ")) },
                pool.get("groove_amount").and_then(Value::as_f64).map(|a| format!(" (now {}%)", (a * 100.0).round() as i64)).unwrap_or_default()
            ))
        }
        None => Ok(format!(
            "{} slot {}: groove removed; the clip plays straight again.",
            track.name, p.clip
        )),
    }
}

pub fn groove_amount_body(live: &LiveState, p: &GrooveAmountParams) -> ToolResult {
    require(live, "set_clip_groove")?;
    if !(0.0..=1.0).contains(&p.value) {
        return Err(format!("value must be between 0 and 1, got {}", p.value));
    }
    let r = live
        .send_command("set_clip_groove", Some(json!({"global_amount": p.value})))
        .map_err(|e| live_err("set the groove amount", e))?;
    Ok(format!(
        "Groove Pool amount {}%: every clip with a groove follows it that much.",
        (r.get("groove_amount")
            .and_then(Value::as_f64)
            .unwrap_or(p.value)
            * 100.0)
            .round() as i64
    ))
}

/// `retime_clip`: the half- or double-time rewrite a transition writes,
/// on its own.
pub fn retime_clip_body(live: &LiveState, p: &RetimeClipParams) -> ToolResult {
    let to = p.to.trim().to_lowercase().replace([' ', '-'], "_");
    if to != "half_time" && to != "double_time" {
        return Err(format!(
            "to must be half_time or double_time, not '{}'",
            p.to
        ));
    }
    vary_clip_body(
        live,
        &VaryClipParams {
            track: p.track.clone(),
            clip: p.clip,
            variation: to,
            seed: p.seed,
            to_slot: p.to_slot,
        },
    )
}

pub fn undo_vary_body(live: &LiveState, p: &UndoVaryParams) -> ToolResult {
    let state = read_perf_state(live)?;
    let track = state.track_by(&p.track)?;
    let previous = live
        .vary_undo
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(track.index, p.clip))
        .ok_or_else(|| {
            format!(
                "nothing to undo on '{}' slot {} (one level, in-place variations only)",
                track.name, p.clip
            )
        })?;
    write_notes(live, track.index, p.clip, &previous)?;
    Ok(format!(
        "Restored '{}' slot {} to its {} notes from before vary_clip.",
        track.name,
        p.clip,
        previous.len()
    ))
}

pub fn follow_key_body(live: &LiveState, p: &FollowKeyParams) -> ToolResult {
    let state = read_perf_state(live)?;
    let source = state.track_by(&p.track)?;
    let raw = clip_notes(live, source.index, p.clip)?;
    let notes: Vec<crate::variation::VNote> = raw
        .iter()
        .filter_map(crate::variation::VNote::from_value)
        .collect();
    let Some((root, mode, confidence)) = crate::variation::estimate_key(&notes) else {
        return Err(format!(
            "'{}' slot {} has no notes to read a key from",
            source.name, p.clip
        ));
    };
    let key_name = format!("{} {mode}", crate::variation::PITCH_CLASSES[root as usize]);
    let mut lines = vec![format!(
        "Recording on '{}' slot {}: {} notes, {key_name} likely ({:.0}% sure).",
        source.name,
        p.clip,
        notes.len(),
        confidence * 100.0
    )];
    let from_root = state
        .root_note
        .or_else(|| {
            performance_running(live)
                .and_then(|r| r.key)
                .and_then(|k| perf::parse_key(&k))
                .map(|k| k.0)
        })
        .unwrap_or(root);
    let interval = crate::variation::transpose_interval(from_root, root);
    if live.script.has_capability("set_scale") {
        let _ = live.send_command(
            "set_scale",
            Some(json!({"root_note": root, "scale_name": mode})),
        );
        lines.push(format!("Live's scale set to {key_name}."));
    }
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.key = Some(key_name.clone());
    }
    if interval == 0 {
        lines.push("The other clips are already in that key; nothing transposed.".into());
        return Ok(lines.join("\n"));
    }
    let mut moved = 0;
    for t in &state.tracks {
        for slot in &t.slots_with_clips {
            if t.index == source.index && *slot == p.clip {
                continue;
            }
            let notes = match clip_notes(live, t.index, *slot) {
                Ok(n) if !n.is_empty() => n,
                _ => continue,
            };
            let shifted: Vec<Value> = notes
                .iter()
                .map(|n| {
                    let mut m = n.clone();
                    if let Some(pch) = n.get("pitch").and_then(Value::as_i64) {
                        m["pitch"] = json!((pch + interval).clamp(0, 127));
                    }
                    m
                })
                .collect();
            if write_notes(live, t.index, *slot, &shifted).is_ok() {
                moved += 1;
            }
        }
    }
    lines.push(format!(
        "Transposed {moved} MIDI clip{} by {interval:+} semitone{} to {key_name}.",
        if moved == 1 { "" } else { "s" },
        if interval.abs() == 1 { "" } else { "s" }
    ));
    Ok(lines.join("\n"))
}

pub fn snapshot_mix_body(live: &LiveState, _p: &Empty) -> ToolResult {
    require(live, "snapshot_mix")?;
    let r = live
        .send_command("snapshot_mix", None)
        .map_err(|e| live_err("take the mix snapshot", e))?;
    Ok(format!(
        "Mix snapshot {} taken at beat {} ({} tracks, {} returns, master: volume, pan, sends, mute). restore_mix {{\"id\": {}}} or a cue step {{\"gesture\": {{\"restore_mix\": {{\"snapshot\": {}}}}}}} brings it back on the bar.",
        get_display(&r, "id", "?"),
        beat(r.get("beat")),
        get_display(&r, "tracks", "?"),
        get_display(&r, "returns", "?"),
        get_display(&r, "id", "?"),
        get_display(&r, "id", "?")
    ))
}

pub fn restore_mix_body(live: &LiveState, p: &RestoreMixParams) -> ToolResult {
    require(live, "restore_mix")?;
    let r = live
        .send_command("restore_mix", Some(json!({"id": p.id})))
        .map_err(|e| live_err("restore the mix", e))?;
    Ok(format!(
        "Mix snapshot {} restored on {} tracks, returns and master.",
        p.id,
        get_display(&r, "restored", "?")
    ))
}

pub fn panic_body(live: &LiveState, p: &PanicParams) -> ToolResult {
    require(live, "schedule_cue")?;
    let state = read_perf_state(live)?;
    let params = CueParams {
        name: Some("panic".into()),
        allow_silence: true,
        steps: vec![perf::CueStep {
            at: Some(perf::CueTime::Named("now".into())),
            gesture: Some(json!({"panic": {"keep": p.keep, "bars": p.bars}})),
            ..Default::default()
        }],
    };
    let resolved = perf::resolve_cue(&state, &params)?;
    let sent = live
        .send_command(
            "schedule_cue",
            Some(json!({"cue": {"name": "panic", "steps": resolved.steps}})),
        )
        .map_err(|e| live_err("schedule the panic", e))?;
    Ok(format!(
        "Panic (cue {}): {}",
        sent.get("id").and_then(Value::as_i64).unwrap_or(0),
        resolved.lines.join("; ")
    ))
}

pub fn keep_track_playing_body(live: &LiveState, p: &KeepTrackPlayingParams) -> ToolResult {
    require(live, "set_slot_stop_buttons")?;
    let state = read_perf_state(live)?;
    let track = state.track_by(&p.track)?;
    let r = live
        .send_command(
            "set_slot_stop_buttons",
            Some(json!({"track_index": track.index, "has_stop_button": !p.keep})),
        )
        .map_err(|e| live_err("change the slot stop buttons", e))?;
    let n = r.get("slots").and_then(Value::as_array).map_or(0, Vec::len);
    Ok(if p.keep {
        format!(
            "{} keeps playing through scene launches: stop buttons removed from its {n} empty slot{}. A clip in a row still replaces it; keep: false restores the stop buttons.",
            track.name,
            if n == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{}: stop buttons back on its {n} empty slot{}; scene launches stop it again.",
            track.name,
            if n == 1 { "" } else { "s" }
        )
    })
}

/// The take's start, or the question. Runs before anything is changed in
/// Live, because the producer may answer "off" — and because a question that
/// arrives after the launch quantization has been rewritten is not a question,
/// it is an apology.
///
/// `Ok(None)` means nothing will be recorded and `notes` says why.
fn prepare_take(
    live: &LiveState,
    choice: perf::RecordChoice,
    bpb: f64,
    notes: &mut Vec<String>,
) -> Result<Option<Take>, String> {
    if choice == perf::RecordChoice::Off {
        return Ok(None);
    }
    if !live.script.has_capability("arrangement_summary")
        || !live.script.has_capability("start_arrangement_record")
    {
        notes.push(
            "Not recording: this Remote Script cannot keep a take (reinstall it to record performances)."
                .into(),
        );
        return Ok(None);
    }
    let summary = match live.send_command("arrangement_summary", None) {
        Ok(v) => v,
        Err(e) => {
            notes.push(format!(
                "Not recording: I could not read what the Arrangement holds ({e}), and I will not record over it blind."
            ));
            return Ok(None);
        }
    };
    if !summary
        .get("supported")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        notes.push(
            "Not recording: this Live does not report Arrangement clips, so I cannot tell what is already there, and I will not record over it blind. Live 11 or newer records the take."
                .into(),
        );
        return Ok(None);
    }
    let end_beat = summary
        .get("end_beat")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let bars = summary.get("bars").and_then(Value::as_i64).unwrap_or(0);
    let tracks = summary.get("tracks").and_then(Value::as_i64).unwrap_or(0);
    if bars <= 0 {
        return Ok(Some(Take {
            start_bar: 1,
            start_beat: 0.0,
            replaced_bars: 0,
        }));
    }
    match choice {
        perf::RecordChoice::Off => Ok(None),
        perf::RecordChoice::After => {
            let beat = perf::take_start_beat(end_beat, bpb);
            Ok(Some(Take {
                start_bar: perf::bar_of_beat(beat, bpb),
                start_beat: beat,
                replaced_bars: 0,
            }))
        }
        perf::RecordChoice::Replace => Ok(Some(Take {
            start_bar: 1,
            start_beat: 0.0,
            replaced_bars: bars,
        })),
        perf::RecordChoice::Ask => {
            let after_bar = perf::bar_of_beat(perf::take_start_beat(end_beat, bpb), bpb);
            Err(format!(
                "Not started — your Arrangement already has {bars} bars on {tracks} track{}, and I keep performances as a take in the Arrangement. What should I do with what is there?\n\n  after    record the take from bar {after_bar}, leaving those {bars} bars alone\n  replace  delete those {bars} bars and record the take from bar 1\n  off      play without recording; the Arrangement is untouched\n\nCall again with record set to one of those. I will remember after or off for the rest of this session.",
                if tracks == 1 { "" } else { "s" }
            ))
        }
    }
}

/// Arm Live for the take and say what that did.
fn arm_take(live: &LiveState, take: &Take) -> Result<String, String> {
    let r = live
        .send_command(
            "start_arrangement_record",
            Some(json!({"from_beat": take.start_beat, "replace": take.replaced_bars > 0})),
        )
        .map_err(|e| live_err("arm the Arrangement take", e))?;
    if take.replaced_bars > 0 {
        let clips = r.get("replaced_clips").and_then(Value::as_i64).unwrap_or(0);
        let tracks = r
            .get("replaced_tracks")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        Ok(format!(
            "Deleted {} bars ({clips} clip{} on {tracks} track{}) and recording this take from bar 1, as you asked. There is no undo in this server — Cmd+Z in Live is the way back, and only before you save.",
            take.replaced_bars,
            if clips == 1 { "" } else { "s" },
            if tracks == 1 { "" } else { "s" }
        ))
    } else if take.start_bar == 1 {
        Ok("Recording this take from bar 1 — the Arrangement was empty, so there was nothing to ask about. Say record \"off\" if you would rather I did not.".into())
    } else {
        Ok(format!(
            "Recording this take from bar {}. Everything before it is untouched. Live still has to be saved by hand when you like it.",
            take.start_bar
        ))
    }
}

/// A take armed and then abandoned would leave Live recording into a
/// performance the server has already given up on. Every fallible step
/// between arming and storing the performance goes through here.
fn disarm_on_err<T>(
    live: &LiveState,
    armed: Option<&Take>,
    r: Result<T, String>,
) -> Result<T, String> {
    if let (Err(_), Some(t)) = (&r, armed) {
        let _ = live.send_command(
            "stop_arrangement_record",
            Some(json!({"from_beat": t.start_beat, "back_to_arrangement": false})),
        );
    }
    r
}

/// Remember an answer worth reusing. `replace` never is: the material it
/// would delete is different every time.
fn remember_record_answer(live: &LiveState, choice: perf::RecordChoice) {
    if matches!(choice, perf::RecordChoice::After | perf::RecordChoice::Off) {
        *live.record_answer.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(choice.as_str().to_string());
    }
}

/// The parameter, unless the producer already answered this session.
pub(crate) fn record_choice_for(
    live: &LiveState,
    given: &str,
) -> Result<perf::RecordChoice, String> {
    let choice = perf::parse_record_choice(given)?;
    if choice != perf::RecordChoice::Ask {
        return Ok(choice);
    }
    let remembered = live
        .record_answer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match remembered.as_deref() {
        Some(a) => perf::parse_record_choice(a),
        None => Ok(perf::RecordChoice::Ask),
    }
}

pub fn start_performance_body(live: &LiveState, p: &StartPerformanceParams) -> ToolResult {
    for c in [
        "get_performance_state",
        "set_launch_quantization",
        "fire_scene",
        "start_playback",
        "schedule_cue",
    ] {
        require(live, c)?;
    }
    if let Some(running) = performance_running(live) {
        return Err(format!(
            "A performance is already running (since bar {}, started {}). cue and get_performance_state work now; end_performance ends it.",
            running.start_bar,
            running.started_at.format("%H:%M:%S")
        ));
    }
    let mut q = p.quantization.trim().to_lowercase();
    if q == "bar" || q == "1 bar" {
        q = "1_bar".into();
    }
    if !perf::GLOBAL_QUANTIZATIONS.contains(&q.as_str()) {
        return Err(format!(
            "quantization must be one of {}",
            perf::GLOBAL_QUANTIZATIONS.join(", ")
        ));
    }
    // A word we do not understand costs nothing: checked before the first read.
    let choice = record_choice_for(live, &p.record)?;
    let before = read_perf_state(live)?;
    let mut notes: Vec<String> = Vec::new();
    // The take is settled next: prepare_take may return the question, and a
    // question must arrive before anything in Live has moved.
    let take = prepare_take(live, choice, before.beats_per_bar(), &mut notes)?;
    live.send_command("set_launch_quantization", Some(json!({"name": q})))
        .map_err(|e| live_err("set the launch quantization", e))?;
    if live.script.has_capability("set_performance_mode") {
        live.send_command("set_performance_mode", Some(json!({"on": true})))
            .map_err(|e| live_err("turn on performance mode", e))?;
    }
    if p.limiter {
        match find_items(live, "limiter", "audio_effects", 1) {
            Ok(hits) if !hits.is_empty() => {
                match live.send_command(
                    "load_browser_item",
                    Some(json!({"track_index": 0, "kind": "master", "item_uri": hits[0].uri})),
                ) {
                    Ok(_) => notes.push(format!("Loaded '{}' on the master as a safety net (set its ceiling to −0.3 dB if it is not).", hits[0].name)),
                    Err(e) => notes.push(format!("Could not load a limiter on the master: {e}")),
                }
            }
            _ => notes
                .push("No Limiter in this Live's browser; the master has no safety net.".into()),
        }
    }
    if p.disarm {
        let armed: Vec<&perf::TrackState> = before
            .tracks
            .iter()
            .filter(|t| t.arm && !t.is_recording)
            .collect();
        if !armed.is_empty() && live.script.has_capability("set_track_mixer") {
            let mut names = Vec::new();
            for t in armed {
                if live
                    .send_command(
                        "set_track_mixer",
                        Some(json!({"track_index": t.index, "kind": "track", "arm": false})),
                    )
                    .is_ok()
                {
                    names.push(t.name.clone());
                }
            }
            if !names.is_empty() {
                notes.push(format!(
                    "Disarmed {} (an armed track records on a scene launch; record_clip arms one on purpose).",
                    names.join(", ")
                ));
            }
        }
    }
    let mut key_set_in_live: Option<String> = None;
    if let Some(k) = p.key.as_ref().filter(|k| !k.trim().is_empty()) {
        match perf::parse_key(k) {
            Some((root, scale)) if live.script.has_capability("set_scale") => {
                match live.send_command(
                    "set_scale",
                    Some(json!({"root_note": root, "scale_name": scale})),
                ) {
                    Ok(r) => {
                        key_set_in_live = Some(format!(
                            "{} {}",
                            get_display(&r, "root_note_name", perf::PITCH_CLASSES[root as usize]),
                            get_display(&r, "scale_name", &scale)
                        ));
                    }
                    Err(e) => notes.push(format!(
                        "Key noted as '{}' but not set in Live: {e}",
                        k.trim()
                    )),
                }
            }
            Some(_) => {}
            None => notes.push(format!(
                "Key '{}' not understood (say e.g. \"D minor\"); kept as given.",
                k.trim()
            )),
        }
    }
    if let Some(t) = p.tempo {
        if !before.is_playing {
            require(live, "set_tempo")?;
            live.send_command("set_tempo", Some(json!({"tempo": t})))
                .map_err(|e| live_err("set the tempo", e))?;
        } else if (t - before.tempo).abs() > 0.01 {
            notes.push(format!(
                "Tempo left at {} BPM: the set is already playing, so ramp it in a cue rather than jump.",
                perf_num(before.tempo)
            ));
        }
    }
    let mut take_note: Option<String> = None;
    if let Some(t) = take.as_ref() {
        match arm_take(live, t) {
            Ok(line) => take_note = Some(line),
            Err(e) => notes.push(format!("Not recording: {e}")),
        }
    }
    let armed_take = if take_note.is_some() {
        take.clone()
    } else {
        None
    };
    let mut fired: Option<(String, usize)> = None;
    if let Some(which) = &p.scene {
        let scene = disarm_on_err(live, armed_take.as_ref(), before.scene_by(which))?;
        let r = disarm_on_err(
            live,
            armed_take.as_ref(),
            live.send_command("fire_scene", Some(json!({"scene_index": scene.index})))
                .map_err(|e| live_err("fire the scene", e)),
        )?;
        let clips = r.get("clips").and_then(Value::as_array).map_or(0, Vec::len);
        let would_record = join_names(r.get("would_record"));
        if !would_record.is_empty() {
            notes.push(format!(
                "{would_record}: armed with an empty slot in '{}' — Live records into it if Start Recording on Scene Launch is on.",
                scene.name
            ));
        }
        fired = Some((scene.name.clone(), clips));
    } else if !before.is_playing {
        disarm_on_err(
            live,
            armed_take.as_ref(),
            live.send_command("start_playback", None)
                .map_err(|e| live_err("start playback", e)),
        )?;
    }
    let after = disarm_on_err(live, armed_take.as_ref(), read_perf_state(live))?;
    let (key, key_text) = match key_set_in_live {
        Some(k) => (Some(k.clone()), format!("key {k} (set in Live)")),
        None => key_line(&after, p.key.as_ref()),
    };
    let started = Performance {
        started_at: chrono::Local::now(),
        start_bar: after.bar,
        start_beat: after.beat,
        key: key.clone(),
        quantization: q.clone(),
        cues_scheduled: 0,
        cues_cancelled: 0,
        follow_key: p.follow_key,
        song: None,
        take: armed_take,
    };
    *live.performance.lock().unwrap_or_else(|e| e.into_inner()) = Some(started.clone());
    remember_record_answer(live, choice);
    let mut text = format!(
        "Performance started at {}.\n",
        started.started_at.format("%H:%M:%S")
    );
    match fired {
        Some((name, clips)) => {
            text.push_str(&format!(
            "Playing scene '{name}' ({clips} clip{}) from bar {} · {} BPM · {}/{} · {key_text}\n",
            if clips == 1 { "" } else { "s" },
            if before.is_playing { after.next_bar() } else { after.bar },
            perf_num(after.tempo),
            after.signature_numerator,
            after.signature_denominator
        ))
        }
        None => {
            let playing = after
                .tracks
                .iter()
                .filter(|t| t.playing_slot_index >= 0)
                .count();
            text.push_str(&format!(
                "{} from bar {} · {} BPM · {}/{} · {key_text}\n",
                if playing > 0 {
                    format!(
                        "Playing ({playing} clip{} already running)",
                        if playing == 1 { "" } else { "s" }
                    )
                } else {
                    "Transport running, nothing playing yet (fire_scene or cue to begin)"
                        .to_string()
                },
                after.bar,
                perf_num(after.tempo),
                after.signature_numerator,
                after.signature_denominator
            ));
        }
    }
    if let Some(line) = take_note.as_ref() {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&format!(
        "Launch quantization is {}: everything you or I fire lands on the next {}.\n",
        quant_words(&q),
        if q == "1_bar" {
            "bar".to_string()
        } else {
            quant_words(&q)
        }
    ));
    text.push_str("While the performance runs I will not stop the transport, move the playhead, jump the tempo, or delete anything that is playing; those calls are refused until end_performance.\n");
    for n in notes {
        text.push_str(&n);
        text.push('\n');
    }
    if p.follow_key {
        text.push_str("follow_key is on: after every record_clip the other MIDI clips are re-keyed to what you played.\n");
    }
    text.push_str("Every result now ends with the clock line (bar, next bar, phrase, next cue). State: get_performance_state. Timed moves: cue.");
    Ok(text)
}

fn perf_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

fn quant_words(q: &str) -> String {
    match q {
        "1_bar" => "1 bar".into(),
        "2_bars" => "2 bars".into(),
        "4_bars" => "4 bars".into(),
        "8_bars" => "8 bars".into(),
        other => other.to_string(),
    }
}

pub fn get_performance_state_body(live: &LiveState, p: &GetPerformanceStateParams) -> ToolResult {
    let state = read_perf_state(live)?;
    let running = performance_running(live);
    let auto_follow = running.as_ref().is_some_and(|r| r.follow_key);
    let mut followed = String::new();
    if auto_follow {
        followed = follow_after_recordings(live, &state.events);
    }
    let since = running.as_ref().map(|r| {
        (
            r.start_bar,
            (chrono::Local::now() - r.started_at).num_milliseconds() as f64 / 1000.0,
        )
    });
    let key = running
        .as_ref()
        .and_then(|r| r.key.clone())
        .or_else(|| state.key_from_set());
    let song_lines = crate::sections::song_lines(live, &state);
    let mut text = perf::state_text_with(&state, since, key.as_deref(), &song_lines);
    if p.bar_map {
        text.push('\n');
        text.push_str(&perf::bar_map_text(&state, state.bar, 32));
    }
    text.push_str(&format!("\nround trip {:.2} s", live.round_trip_s()));
    if !followed.is_empty() {
        text.push('\n');
        text.push_str(&followed);
    }
    if let Some(take) = running.as_ref().and_then(|r| r.take.as_ref()) {
        text.push_str(&format!(
            "\nRecording this performance into the Arrangement from bar {}; end_performance stops the take and says what it covered.",
            take.start_bar
        ));
    }
    if let Some(r) = running.as_ref().filter(|_| !state.is_playing) {
        let age = (chrono::Local::now() - r.started_at).num_seconds();
        if age >= STALE_PERFORMANCE_S && state.cues.is_empty() {
            text.push_str(&format!(
                "\nThe transport is stopped and nothing is cued, and this performance has been running {} — the next tool that needs the transport will end it and carry on.",
                crate::performance::fmt_secs(age as f64)
            ));
        } else {
            text.push_str("\nThe transport is stopped (outside this server, or by end_performance's cue). The guards are still on: fire_scene or start a cue to resume, or end_performance to lift them.");
        }
    }
    Ok(text)
}

/// With follow_key on: a `recording_done` event re-keys the other clips.
fn follow_after_recordings(live: &LiveState, events: &[Value]) -> String {
    let mut out = Vec::new();
    for e in events {
        if e.get("type").and_then(Value::as_str) != Some("recording_done") {
            continue;
        }
        let (Some(t), Some(s)) = (
            e.get("track_index").and_then(Value::as_i64),
            e.get("slot").and_then(Value::as_i64),
        ) else {
            continue;
        };
        match follow_key_body(
            live,
            &FollowKeyParams {
                track: json!(t),
                clip: s,
            },
        ) {
            Ok(text) => out.push(text),
            Err(err) => out.push(format!("follow_key after the recording failed: {err}")),
        }
    }
    out.join("\n")
}

pub fn cue_body(live: &LiveState, p: &CueParams) -> ToolResult {
    require(live, "schedule_cue")?;
    let state = read_perf_state(live)?;
    let p = &resolve_sound_ramps(live, &state, p)?;
    let resolved = perf::resolve_cue(&state, p)?;
    let sent = live
        .send_command(
            "schedule_cue",
            Some(json!({"cue": {"name": p.name, "steps": resolved.steps}})),
        )
        .map_err(|e| live_err("schedule the cue", e))?;
    let id = sent.get("id").and_then(Value::as_i64).unwrap_or(0);
    let name = get_display(&sent, "name", "cue");
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.cues_scheduled += 1;
    }
    let mut text = format!(
        "Cue '{name}' (id {id}) scheduled — it is bar {} now.\n",
        state.position()
    );
    for line in &resolved.lines {
        text.push_str("  ");
        text.push_str(line);
        text.push('\n');
    }
    let launches = resolved.steps.iter().any(|s| {
        matches!(
            s.get("action").and_then(Value::as_str),
            Some("fire_scene" | "fire_clip" | "stop_clip" | "stop_all_clips")
        )
    });
    if launches && !p.allow_silence {
        text.push_str("Check: every bar from the first step has at least one clip playing.\n");
    }
    for w in &resolved.warnings {
        text.push_str("Warning: ");
        text.push_str(w);
        text.push('\n');
    }
    text.push_str(&format!(
        "The Remote Script runs this on its own clock; it happens even if I go quiet. Cancel with cancel_cue {{\"id\": {id}}}."
    ));
    Ok(text)
}

pub fn cancel_cue_body(live: &LiveState, p: &CancelCueParams) -> ToolResult {
    require(live, "cancel_cue")?;
    let r = live
        .send_command("cancel_cue", Some(json!({"id": p.id})))
        .map_err(|e| live_err("cancel the cue", e))?;
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.cues_cancelled += 1;
    }
    Ok(format!(
        "Cancelled cue {}; steps already fired stay fired.",
        r.get("cancelled")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .map(display)
            .unwrap_or_else(|| p.id.to_string())
    ))
}

pub fn fire_scene_body(live: &LiveState, p: &FireSceneParams) -> ToolResult {
    require(live, "fire_scene")?;
    let state = read_perf_state(live)?;
    let scene = state.scene_by(&p.scene)?;
    if let Some(text) = defer_if_too_close(
        live,
        p.no_later_than,
        json!({"action": "fire_scene", "scene_index": scene.index}),
        &format!("fire scene '{}'", scene.name),
    )? {
        return Ok(text);
    }
    let r = live
        .send_command("fire_scene", Some(json!({"scene_index": scene.index})))
        .map_err(|e| live_err("fire the scene", e))?;
    let clips = r.get("clips").and_then(Value::as_array).map_or(0, Vec::len);
    let landing = landing_text(live, &r);
    let mut text = format!(
        "Fired scene '{}' ({clips} clip{}){}",
        scene.name,
        if clips == 1 { "" } else { "s" },
        if landing.is_empty() {
            format!(" — starts {}.", state.launch_lands_on())
        } else {
            landing
        }
    );
    if let Some(off) = r.get("off_grid_clips").and_then(Value::as_array) {
        if !off.is_empty() {
            let names: Vec<String> = off
                .iter()
                .map(|c| {
                    format!(
                        "{}/{} ({})",
                        get_display(c, "track", "?"),
                        get_display(c, "name", "?"),
                        get_display(c, "launch_quantization", "?")
                    )
                })
                .collect();
            text.push_str(&format!(
                "\nNote: {} launch on their own quantization, not the bar.",
                names.join(", ")
            ));
        }
    }
    let would_record = join_names(r.get("would_record"));
    if !would_record.is_empty() {
        text.push_str(&format!(
            "\nNote: {would_record} armed with an empty slot in this scene — Live records into it if Start Recording on Scene Launch is on."
        ));
    }
    Ok(text)
}

pub fn create_scene_body(live: &LiveState, p: &CreateSceneParams) -> ToolResult {
    require(live, "create_scene")?;
    let r = live
        .send_command(
            "create_scene",
            Some(json!({"index": p.index, "name": p.name, "tempo": p.tempo, "phrase_bars": p.phrase_bars})),
        )
        .map_err(|e| live_err("create the scene", e))?;
    let index = get_display(&r, "index", "?");
    Ok(format!(
        "Created scene {index} '{}' ({} scenes now). create_clip with clip_index {index} fills its row; fire_scene or a cue plays it.",
        get_display(&r, "name", ""),
        get_display(&r, "scene_count", "?")
    ))
}

pub fn record_clip_body(live: &LiveState, p: &RecordClipParams) -> ToolResult {
    require(live, "record_clip")?;
    if !(1..=64).contains(&p.bars) {
        return Err(format!("bars must be between 1 and 64, got {}", p.bars));
    }
    let state = read_perf_state(live)?;
    let track = state.track_by(&p.track)?;
    if p.no_later_than.is_some() {
        return Err("record_clip cannot be cued yet: arm and fire it two bars ahead, or drop no_later_than.".into());
    }
    let r = live
        .send_command(
            "record_clip",
            Some(json!({"track_index": track.index, "bars": p.bars, "name": p.name})),
        )
        .map_err(|e| live_err("start the recording", e))?;
    let landing = landing_text(live, &r);
    Ok(format!(
        "Recording {} slot {} '{}'{}, {} bar{}, then loops. The track is armed until the recording ends; the clip is named when it appears.",
        track.name,
        get_display(&r, "slot", "?"),
        p.name.clone().unwrap_or_else(|| "take".into()),
        if landing.is_empty() {
            format!(" — starts {}", state.launch_lands_on())
        } else {
            landing.trim_end_matches('.').to_string()
        },
        p.bars,
        if p.bars == 1 { "" } else { "s" }
    ))
}

pub fn set_launch_quantization_body(
    live: &LiveState,
    p: &SetLaunchQuantizationParams,
) -> ToolResult {
    require(live, "set_launch_quantization")?;
    let mut q = p.quantization.trim().to_lowercase();
    if q == "bar" || q == "1 bar" {
        q = "1_bar".into();
    }
    if !perf::GLOBAL_QUANTIZATIONS.contains(&q.as_str()) {
        return Err(format!(
            "quantization must be one of {}",
            perf::GLOBAL_QUANTIZATIONS.join(", ")
        ));
    }
    let r = live
        .send_command("set_launch_quantization", Some(json!({"name": q})))
        .map_err(|e| live_err("set the launch quantization", e))?;
    let name = get_display(&r, "name", &q);
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.quantization = name.clone();
    }
    Ok(format!(
        "Launch quantization is now {}: fire_clip, fire_scene, record_clip and cue launches land on the next {}.",
        quant_words(&name),
        if name == "none" { "— no quantization: launches happen immediately".to_string() } else { quant_words(&name) }
    ))
}

pub fn set_crossfader_body(live: &LiveState, p: &SetCrossfaderParams) -> ToolResult {
    require(live, "set_crossfader")?;
    if p.value.is_none() && p.assign.is_empty() {
        return Err("Give value (0 = A, 1 = B) and/or assign.".into());
    }
    if let Some(v) = p.value {
        if !(0.0..=1.0).contains(&v) {
            return Err(format!("value must be between 0 (A) and 1 (B), got {v}"));
        }
    }
    let mut assign = Vec::new();
    if !p.assign.is_empty() {
        let state = read_perf_state(live)?;
        for a in &p.assign {
            let side = a.side.trim().to_uppercase();
            if !matches!(side.as_str(), "A" | "B" | "NONE") {
                return Err(format!("side must be A, B or none, got '{}'", a.side));
            }
            let t = state.track_by(&a.track)?;
            assign.push(json!({"track_index": t.index, "side": side.to_lowercase()}));
        }
    }
    let r = live
        .send_command(
            "set_crossfader",
            Some(json!({"value": p.value, "assign": assign})),
        )
        .map_err(|e| live_err("set the crossfader", e))?;
    let mut parts = Vec::new();
    if let Some(v) = r.get("crossfader").and_then(Value::as_f64) {
        parts.push(format!(
            "crossfader at {} ({})",
            perf_num(v),
            if v <= 0.0 {
                "A"
            } else if v >= 1.0 {
                "B"
            } else {
                "between"
            }
        ));
    }
    if let Some(a) = r.get("assigned").and_then(Value::as_array) {
        for x in a {
            parts.push(format!(
                "{} → {}",
                get_display(x, "track", "?"),
                get_display(x, "side", "?")
            ));
        }
    }
    Ok(format!(
        "Crossfader: {}. For a timed blend, ramp it in a cue.",
        parts.join(", ")
    ))
}

/// Disarm Live's Arrangement Record and say what the take came to.
fn stop_take(live: &LiveState, take: &Take, scheduled_stop: bool) -> String {
    if !live.script.has_capability("stop_arrangement_record") {
        return "The take is still armed: this Remote Script cannot stop it. Turn Live's Arrangement Record off by hand.".into();
    }
    let r = match live.send_command(
        "stop_arrangement_record",
        Some(json!({"from_beat": take.start_beat, "back_to_arrangement": true})),
    ) {
        Ok(r) => r,
        Err(e) => {
            return format!(
                "The take could not be stopped ({e}). Turn Live's Arrangement Record off by hand."
            )
        }
    };
    let end_bar = r.get("end_bar").and_then(Value::as_i64).unwrap_or(0);
    let tracks = r.get("tracks").and_then(Value::as_i64).unwrap_or(0);
    if end_bar < take.start_bar || tracks == 0 {
        return "Nothing reached the Arrangement — the take is empty.".into();
    }
    let bars = end_bar - take.start_bar + 1;
    let mut line = format!(
        "Take recorded: bars {}–{end_bar} of the Arrangement, {bars} bar{} on {tracks} track{}. Back to Arrangement is on, so the tracks follow the timeline again. Press Cmd+S in Live to keep it.",
        take.start_bar,
        if bars == 1 { "" } else { "s" },
        if tracks == 1 { "" } else { "s" }
    );
    if scheduled_stop {
        line.push_str(
            " The take stops here; what plays until the transport actually stops is not in it.",
        );
    }
    line
}

pub fn end_performance_body(live: &LiveState, p: &EndPerformanceParams) -> ToolResult {
    let Some(running) = performance_running(live) else {
        return Err("No performance is running.".into());
    };
    for c in [
        "cancel_cue",
        "stop_playback",
        "stop_all_clips",
        "schedule_cue",
    ] {
        require(live, c)?;
    }
    let state = read_perf_state(live)?;
    let _ = live.send_command(
        "cancel_cue",
        Some(json!({"id": null, "reason": "end_performance"})),
    );
    let duration = (chrono::Local::now() - running.started_at)
        .num_seconds()
        .max(0);
    let summary = format!(
        "{} min {} s, {} cue{} scheduled, {} cancelled.",
        duration / 60,
        duration % 60,
        running.cues_scheduled,
        if running.cues_scheduled == 1 { "" } else { "s" },
        running.cues_cancelled
    );
    let how = if p.now {
        let _ = live.send_command("stop_all_clips", None);
        live.send_command("stop_playback", None)
            .map_err(|e| live_err("stop playback", e))?;
        format!(
            "Performance ended now, at bar {}: all clips and the transport stopped.",
            state.position()
        )
    } else if !state.is_playing {
        format!(
            "Performance ended (the transport was already stopped at bar {}).",
            state.position()
        )
    } else if let Some(bars) = p.fade_bars {
        if !(1.0..=64.0).contains(&bars) {
            return Err(format!("fade_bars must be between 1 and 64, got {bars}"));
        }
        let (bar, beat) = perf::resolve_time(&state, &perf::CueTime::Named("next_bar".into()))?;
        let end_beat = beat + bars * state.beats_per_bar();
        let master_volume = live
            .send_command("get_session_info", None)
            .ok()
            .and_then(|s| s.pointer("/master_track/volume").and_then(Value::as_f64))
            .unwrap_or(0.85);
        let steps = vec![
            json!({"action": "ramp", "beat": beat, "end_beat": end_beat, "bar": bar, "target": "volume", "kind": "master", "to": 0.0, "label": "fade master to silence"}),
            json!({"action": "stop_playback", "beat": end_beat, "bar": bar + bars, "label": "stop"}),
            json!({"action": "set", "beat": end_beat + 0.01, "bar": bar + bars, "target": "volume", "kind": "master", "value": master_volume, "label": "restore master volume"}),
        ];
        live.send_command(
            "schedule_cue",
            Some(json!({"cue": {"name": "end", "steps": steps}})),
        )
        .map_err(|e| live_err("schedule the ending", e))?;
        format!(
            "Performance ends: the master fades over {} bars from bar {} and the transport stops at bar {}; the master volume is restored after the stop.",
            perf_num(bars),
            perf_num(bar),
            perf_num(bar + bars)
        )
    } else {
        let at =
            p.at.clone()
                .unwrap_or(perf::CueTime::Named("next_bar".into()));
        let (bar, beat) = perf::resolve_time(&state, &at)?;
        if beat <= state.beat {
            return Err(format!(
                "bar {} has passed (it is bar {}); use \"next_bar\" or now: true.",
                perf_num(bar),
                state.position()
            ));
        }
        let steps =
            vec![json!({"action": "stop_playback", "beat": beat, "bar": bar, "label": "stop"})];
        live.send_command(
            "schedule_cue",
            Some(json!({"cue": {"name": "end", "steps": steps}})),
        )
        .map_err(|e| live_err("schedule the ending", e))?;
        format!(
            "Performance ends at bar {}: the transport stops on the bar.",
            perf_num(bar)
        )
    };
    // The take stops here, whatever the transport does next: Live keeps
    // recording only while record_mode is on, and leaving it on after the
    // server has forgotten the performance would arm the next thing played.
    let scheduled_stop = !p.now && state.is_playing;
    let take_line = running
        .take
        .as_ref()
        .map(|t| stop_take(live, t, scheduled_stop));
    if live.script.has_capability("set_performance_mode") {
        let _ = live.send_command("set_performance_mode", Some(json!({"on": false})));
    }
    *live.performance.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let mut text = format!("{how} {summary} Guards lifted.");
    if let Some(line) = take_line {
        text.push('\n');
        text.push_str(&line);
    }
    if state.pending_record.is_some() {
        text.push_str(" A recording was pending; the script disarms the track when it ends or the transport stops.");
    }
    Ok(text)
}

/// Run one tool by name with JSON arguments — the batch and build_song
/// dispatcher. Every Live-facing tool is here; batch itself is not.
pub fn run_named(live: &LiveState, name: &str, args: Value) -> ToolResult {
    let args = if args.is_null() { json!({}) } else { args };
    // The raw layer is served as adv_<name>; batch steps may use either.
    let name = name.strip_prefix(ADVANCED_PREFIX).unwrap_or(name);
    named_tools!(live, name, args;
        "get_session_info" => (Empty, get_session_info_body),
        "get_remote_script_info" => (Empty, get_remote_script_info_body),
        "get_track_info" => (TrackInfoParams, get_track_info_body),
        "get_clip_notes" => (ClipParams, get_clip_notes_body),
        "get_device_parameters" => (DeviceParams, get_device_parameters_body),
        "set_device_parameter" => (SetDeviceParameterParams, set_device_parameter_body),
        "edit_devices" => (EditDevicesParams, edit_devices_body),
        "get_session_snapshot" => (SnapshotParams, get_session_snapshot_body),
        "create_midi_track" => (CreateTrackParams, create_midi_track_body),
        "create_audio_track" => (CreateTrackParams, create_audio_track_body),
        "set_track_name" => (SetTrackNameParams, set_track_name_body),
        "create_clip" => (CreateClipParams, create_clip_body),
        "create_audio_clip" => (CreateAudioClipParams, create_audio_clip_body),
        "add_notes_to_clip" => (AddNotesParams, add_notes_to_clip_body),
        "clear_notes_from_clip" => (ClipParams, clear_notes_from_clip_body),
        "set_clip_name" => (SetClipNameParams, set_clip_name_body),
        "set_arrangement_clip_name" => (SetClipNameParams, set_arrangement_clip_name_body),
        "set_tempo" => (SetTempoParams, set_tempo_body),
        "load_instrument_or_effect" => (LoadInstrumentParams, load_instrument_or_effect_body),
        "fire_clip" => (FireClipParams, fire_clip_body),
        "stop_clip" => (ClipParams, stop_clip_body),
        "delete_clip" => (ClipParams, delete_clip_body),
        "start_playback" => (Empty, start_playback_body),
        "stop_playback" => (Empty, stop_playback_body),
        "get_browser_tree" => (BrowserTreeParams, get_browser_tree_body),
        "get_browser_items_at_path" => (BrowserPathParams, get_browser_items_at_path_body),
        "load_drum_kit" => (LoadDrumKitParams, load_drum_kit_body),
        "switch_to_arrangement_view" => (Empty, switch_to_arrangement_view_body),
        "set_arrangement_time" => (ArrangementTimeParams, set_arrangement_time_body),
        "get_arrangement_clips" => (TrackParams, get_arrangement_clips_body),
        "duplicate_to_arrangement" => (DuplicateToArrangementParams, duplicate_to_arrangement_body),
        "create_locator" => (CreateLocatorParams, create_locator_body),
        "set_track_mixer" => (SetTrackMixerParams, set_track_mixer_body),
        "set_send" => (SetSendParams, set_send_body),
        "get_returns" => (Empty, get_returns_body),
        "set_color" => (SetColorParams, set_color_body),
        "get_drum_rack_pads" => (DrumRackPadsParams, get_drum_rack_pads_body),
        "delete_arrangement_clip" => (DeleteArrangementClipParams, delete_arrangement_clip_body),
        "delete_locator" => (DeleteLocatorParams, delete_locator_body),
        "search_browser" => (SearchBrowserParams, search_browser_body),
        "get_clip_info" => (ClipRefParams, get_clip_info_body),
        "set_clip_loop" => (SetClipLoopParams, set_clip_loop_body),
        "set_clip_launch" => (SetClipLaunchParams, set_clip_launch_body),
        "get_track_meters" => (Empty, get_track_meters_body),
        "play_and_measure" => (PlayAndMeasureParams, play_and_measure_body),
        "set_clip_automation" => (SetClipAutomationParams, set_clip_automation_body),
        "get_clip_automation" => (GetClipAutomationParams, get_clip_automation_body),
        "get_library_status" => (Empty, get_library_status_body),
        "device_vocabulary" => (DeviceVocabularyParams, device_vocabulary_body),
        "capture_mix" => (CaptureMixParams, capture_mix_body),
        "list_captures" => (Empty, list_captures_body),
        "measure_capture" => (MeasureCaptureParams, measure_capture_body),
        "delete_track" => (TrackParams, delete_track_body),
        "reset_set" => (ResetSetParams, reset_set_body),
        "back_to_arrangement" => (Empty, back_to_arrangement_body),
        "set_arrangement_loop" => (SetArrangementLoopParams, set_arrangement_loop_body),
        "start_performance" => (StartPerformanceParams, start_performance_body),
        "get_performance_state" => (GetPerformanceStateParams, get_performance_state_body),
        "cue" => (CueParams, cue_body),
        "cancel_cue" => (CancelCueParams, cancel_cue_body),
        "fire_scene" => (FireSceneParams, fire_scene_body),
        "create_scene" => (CreateSceneParams, create_scene_body),
        "record_clip" => (RecordClipParams, record_clip_body),
        "set_launch_quantization" => (SetLaunchQuantizationParams, set_launch_quantization_body),
        "set_crossfader" => (SetCrossfaderParams, set_crossfader_body),
        "end_performance" => (EndPerformanceParams, end_performance_body),
        "keep_track_playing" => (KeepTrackPlayingParams, keep_track_playing_body),
        "get_context" => (GetContextParams, get_context_body),
        "set_scene" => (SetSceneParams, set_scene_body),
        "listen" => (ListenParams, listen_body),
        "vary_clip" => (VaryClipParams, vary_clip_body),
        "undo_vary" => (UndoVaryParams, undo_vary_body),
        "follow_key" => (FollowKeyParams, follow_key_body),
        "snapshot_mix" => (Empty, snapshot_mix_body),
        "restore_mix" => (RestoreMixParams, restore_mix_body),
        "panic" => (PanicParams, panic_body),
        "retime_clip" => (RetimeClipParams, retime_clip_body),
        "shape_sound" => (ShapeSoundParams, shape_sound_body),
        "export_set" => (crate::sets::ExportSetParams, crate::sets::export_set_body),
        "set_key" => (crate::arrange::SetKeyParams, crate::arrange::set_key_body),
        "create_return" => (crate::arrange::CreateReturnParams, crate::arrange::create_return_body),
        "clear_captures" => (crate::arrange::ClearCapturesParams, crate::arrange::clear_captures_body),
        "remember" => (crate::memory::RememberParams, crate::memory::remember_body),
        "stash" => (crate::memory::StashParams, crate::memory::stash_body),
        "song_memory" => (
            crate::memory::SongMemoryParams,
            crate::memory::song_memory_body
        ),
        "feel" => (crate::arrange::FeelParams, crate::arrange::feel_body),
        "arrange" => (crate::arrange::ArrangeParams, crate::arrange::arrange_body),
        "import_set" => (crate::sets::ImportSetParams, crate::sets::import_set_body),
        "groove_clip" => (GrooveClipParams, groove_clip_body),
        "groove_amount" => (GrooveAmountParams, groove_amount_body),
        "humanize" => (HumanizeParams, humanize_body),
        "swing_notes" => (SwingNotesParams, swing_notes_body),
        "make_section" => (crate::sections::MakeSectionParams, crate::sections::make_section_body),
        "set_song" => (crate::sections::SetSongParams, crate::sections::set_song_body),
        "add_to_song" => (crate::sections::AddToSongParams, crate::sections::add_to_song_body),
        "remove_from_song" => (crate::sections::RemoveFromSongParams, crate::sections::remove_from_song_body),
        "play_song" => (crate::sections::PlaySongParams, crate::sections::play_song_body),
        "hold_section" => (Empty, crate::sections::hold_section_body),
        "go" => (crate::sections::SteerParams, crate::sections::go_body),
        "next_section" => (crate::sections::SteerParams, crate::sections::next_section_body),
        "previous_section" => (crate::sections::SteerParams, crate::sections::previous_section_body),
        "back" => (crate::sections::SteerParams, crate::sections::back_body),
        "jump_to" => (crate::sections::JumpToParams, crate::sections::jump_to_body),
        "add_sample" => (crate::samples::AddSampleParams, crate::samples::add_sample_body),
        "sample_folders" => (crate::samples::SampleFoldersParams, crate::samples::sample_folders_body),
    )
}

/// "Created MIDI track 6 (…)" → 6.
fn track_index_from_text(text: &str) -> Option<i64> {
    let rest = text.strip_prefix("Created ")?;
    if rest.starts_with("clip ") {
        return None;
    }
    let after = rest.split(" track ").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// "Created clip at track 2, slot 3 …" → (2, 3).
fn clip_from_text(text: &str) -> Option<(i64, i64)> {
    let rest = text.strip_prefix("Created clip at track ")?;
    let (t, after) = rest.split_once(", slot ")?;
    let s: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some((t.trim().parse().ok()?, s.parse().ok()?))
}

fn substitute(
    v: &mut Value,
    last_track: Option<i64>,
    last_clip: Option<(i64, i64)>,
) -> Result<(), String> {
    match v {
        Value::String(s) if s == "$last_track" => match last_track {
            Some(i) => *v = json!(i),
            None => {
                return Err("$last_track used before any track was created in this batch".into())
            }
        },
        Value::String(s) if s == "$last_clip" => match last_clip {
            Some((_, c)) => *v = json!(c),
            None => return Err("$last_clip used before any clip was created in this batch".into()),
        },
        Value::Array(items) => {
            for item in items {
                substitute(item, last_track, last_clip)?;
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                substitute(item, last_track, last_clip)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// What one batch step did, kept so the summary can be written before the
/// detail is rendered.
struct StepOutcome {
    number: usize,
    tool: String,
    ok: bool,
    /// The tool's own text, or the failure message.
    text: String,
    /// The lines in that text saying a part of the step did not land.
    skipped: Vec<String>,
}

/// The lines a tool writes to say part of what was asked did not happen —
/// `shape_sound` writes one per word no parameter answered to. A step that
/// half-applied is the case the summary exists for, so those lines are
/// lifted out of the step's text and listed under it.
fn skipped_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.trim_start().starts_with("skipped "))
        .map(|l| l.trim().to_string())
        .collect()
}

/// "Removed 71 clips …" → ("removed", 71). Tools say in their first line how
/// much they did; the summary adds that up per tool, so a group line carries
/// the work and not only the number of steps.
fn reported_count(text: &str) -> Option<(String, i64)> {
    const VERBS: [&str; 8] = [
        "removed", "deleted", "added", "placed", "created", "wrote", "moved", "cleared",
    ];
    let first = text.lines().next()?.trim();
    let (verb, rest) = first.split_once(' ')?;
    let verb = verb
        .trim_matches(|c: char| !c.is_alphabetic())
        .to_lowercase();
    if !VERBS.contains(&verb.as_str()) {
        return None;
    }
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some((verb, digits.parse().ok()?))
}

/// One tool, one outcome, however many steps.
struct StepGroup {
    tool: String,
    ok: bool,
    steps: usize,
    partial: usize,
    counts: Vec<(String, i64)>,
}

/// The summary, then everything that did not go as asked, then the detail.
/// Successful steps print in full only when the caller asked for it or the
/// batch is short enough to be the debugging case.
fn render_batch(outcomes: &[StepOutcome], not_run: usize, verbose: bool) -> String {
    let total = outcomes.len();
    let ok = outcomes.iter().filter(|o| o.ok).count();
    let failed = total - ok;
    let partial = outcomes.iter().filter(|o| !o.skipped.is_empty()).count();

    let mut out = format!("{total} step{}, {ok} ok", if total == 1 { "" } else { "s" });
    if failed > 0 {
        out.push_str(&format!(", {failed} failed"));
    }
    if partial > 0 {
        out.push_str(&format!(" — {partial} with something skipped"));
    }
    out.push('\n');

    let mut groups: Vec<StepGroup> = Vec::new();
    for o in outcomes {
        let g = match groups
            .iter_mut()
            .position(|g| g.tool == o.tool && g.ok == o.ok)
        {
            Some(i) => &mut groups[i],
            None => {
                groups.push(StepGroup {
                    tool: o.tool.clone(),
                    ok: o.ok,
                    steps: 0,
                    partial: 0,
                    counts: Vec::new(),
                });
                groups.last_mut().expect("just pushed")
            }
        };
        g.steps += 1;
        if !o.skipped.is_empty() {
            g.partial += 1;
        }
        if let Some((verb, n)) = reported_count(&o.text) {
            match g.counts.iter_mut().find(|(v, _)| *v == verb) {
                Some((_, total)) => *total += n,
                None => g.counts.push((verb, n)),
            }
        }
    }
    for g in &groups {
        out.push_str(&format!("  {}", g.tool));
        if g.steps > 1 {
            out.push_str(&format!(" ×{}", g.steps));
        }
        out.push_str(if g.ok { " ✓" } else { " ✗" });
        if !g.counts.is_empty() {
            out.push_str(&format!(
                " — {}",
                g.counts
                    .iter()
                    .map(|(v, n)| format!("{v} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if g.partial > 0 {
            out.push_str(&format!(" ({} with something skipped)", g.partial));
        }
        out.push('\n');
    }

    if partial > 0 {
        out.push_str("Skipped:\n");
        for o in outcomes.iter().filter(|o| !o.skipped.is_empty()) {
            for line in &o.skipped {
                out.push_str(&format!("  {}. {}: {}\n", o.number, o.tool, line));
            }
        }
    }
    if failed > 0 {
        out.push_str("Failed:\n");
        for o in outcomes.iter().filter(|o| !o.ok) {
            out.push_str(&format!("  {}. {} ✗ {}\n", o.number, o.tool, o.text));
        }
    }
    if not_run > 0 {
        out.push_str(&format!("Stopped; {not_run} step(s) not run.\n"));
    }
    if verbose || total <= 10 {
        for o in outcomes.iter().filter(|o| o.ok) {
            let body = if o.text.contains('\n') {
                format!("\n   {}", o.text.replace('\n', "\n   "))
            } else {
                o.text.clone()
            };
            out.push_str(&format!("{}. {} ✓ {}\n", o.number, o.tool, body));
        }
    }
    out
}

pub fn batch_body(live: &LiveState, p: &BatchParams) -> ToolResult {
    if p.steps.is_empty() {
        return Err("Give at least one step {tool, args}.".into());
    }
    if p.steps.len() > 200 {
        return Err(format!(
            "{} steps is too many for one batch (limit 200)",
            p.steps.len()
        ));
    }
    if let Some(bad) = p
        .steps
        .iter()
        .find(|s| s.tool == "batch" || s.tool == "build_song")
    {
        return Err(format!("`{}` cannot run inside a batch", bad.tool));
    }
    let mut outcomes: Vec<StepOutcome> = Vec::new();
    let mut last_track: Option<i64> = None;
    let mut last_clip: Option<(i64, i64)> = None;
    let mut not_run = 0;
    for (i, step) in p.steps.iter().enumerate() {
        let mut args = step.args.clone();
        let outcome = substitute(&mut args, last_track, last_clip)
            .and_then(|_| run_named(live, &step.tool, args));
        match outcome {
            Ok(text) => {
                if let Some(idx) = track_index_from_text(&text) {
                    last_track = Some(idx);
                }
                if let Some(c) = clip_from_text(&text) {
                    last_clip = Some(c);
                }
                outcomes.push(StepOutcome {
                    number: i + 1,
                    tool: step.tool.clone(),
                    ok: true,
                    skipped: skipped_lines(&text),
                    text,
                });
            }
            Err(e) => {
                outcomes.push(StepOutcome {
                    number: i + 1,
                    tool: step.tool.clone(),
                    ok: false,
                    text: e,
                    skipped: Vec::new(),
                });
                if p.stop_on_error {
                    not_run = p.steps.len() - i - 1;
                    break;
                }
            }
        }
    }
    let failed = outcomes.iter().filter(|o| !o.ok).count();
    let text = render_batch(&outcomes, not_run, p.verbose);
    if failed > 0 {
        return Err(text);
    }
    Ok(text)
}

/// Resolve a build_song track reference: a name defined in the document,
/// or an existing track's index written as a string.
fn song_track_index(name: &str, created: &BTreeMap<String, i64>) -> Result<i64, String> {
    if let Some(i) = created.get(name) {
        return Ok(*i);
    }
    name.trim().parse::<i64>().map_err(|_| {
        format!("track '{name}' is not defined in `tracks` and is not an existing track index")
    })
}

/// How many tracks go to Live in one `create_tracks`. Each track
/// reinitialises Live's audio graph and most load a device from disk; ten in
/// one command took Live down twice on a producer's machine (#45, Live
/// 12.4.6: main-thread slices of 2.1 s, then the socket closed mid-command).
/// Three keeps each command short enough to answer and costs one group, not
/// the document, when Live does go away.
const TRACKS_PER_GROUP: usize = 3;

/// What `build_song` ends on once it has changed the set. A crash costs
/// whatever exists only in Live's memory, and the Live API has no save to
/// call — that is Cmd+S, the producer's. The one thing this server can do is
/// write the document back out, and the moment nobody reaches for it is the
/// moment it is worth saying (#46).
const SNAPSHOT_OFFER: &str = "\nWhat is in the set exists only in Live's memory until you save it there (Cmd+S — the Live API has no save of its own). export_set {\"name\": \"…\"} writes a rebuildable copy under the server's state folder; build_song takes snapshot: true to write one as part of the build.";

pub fn build_song_body(live: &LiveState, p: &BuildSongParams) -> ToolResult {
    // ── validate everything before the first command ──
    let mode = match p.on_existing.trim().to_lowercase().as_str() {
        "" | "converge" => "converge".to_string(),
        "add" => "add".to_string(),
        "fail" => "fail".to_string(),
        other => {
            return Err(format!(
                "on_existing must be converge, add or fail, not '{other}'"
            ))
        }
    };
    let mode = mode.as_str();
    let mut names: Vec<&str> = Vec::new();
    for t in &p.tracks {
        if t.name.trim().is_empty() {
            return Err("every track needs a name".into());
        }
        if names.contains(&t.name.as_str()) {
            return Err(format!("track name '{}' is used twice", t.name));
        }
        if t.kind != "midi" && t.kind != "audio" {
            return Err(format!("track '{}': kind must be midi or audio", t.name));
        }
        if t.instrument.is_some() && t.instrument_query.is_some() {
            return Err(format!(
                "track '{}': give instrument or instrument_query, not both",
                t.name
            ));
        }
        names.push(&t.name);
    }
    let mut planned_notes: Vec<Vec<Note>> = Vec::new();
    let mut defined_clips: Vec<(String, i64)> = Vec::new();
    for c in &p.clips {
        let known = names.contains(&c.track.as_str()) || c.track.trim().parse::<i64>().is_ok();
        if !known {
            return Err(format!(
                "clip '{}': track '{}' is neither defined in tracks nor an index",
                c.name, c.track
            ));
        }
        if c.length <= 0.0 {
            return Err(format!("clip '{}': length must be greater than 0", c.name));
        }
        let notes =
            crate::notes::expand(&c.notes).map_err(|e| format!("clip '{}': {e}", c.name))?;
        let main_slot = c.slot.or_else(|| c.slots.first().copied()).unwrap_or(0);
        if defined_clips.contains(&(c.track.clone(), main_slot)) {
            return Err(format!(
                "two clips are defined for track '{}' slot {}",
                c.track, main_slot
            ));
        }
        defined_clips.push((c.track.clone(), main_slot));
        planned_notes.push(notes);
    }
    let mut placements: Vec<(usize, Vec<f64>)> = Vec::new();
    for (i, pl) in p.placements.iter().enumerate() {
        let existing_track = pl.track.trim().parse::<i64>().is_ok();
        if !defined_clips.contains(&(pl.track.clone(), pl.slot)) && !existing_track {
            return Err(format!(
                "placement {}: no clip is defined for track '{}' slot {} — define it in `clips`, or give an existing track's index (\"3\") to place a clip that is already in the set",
                i + 1,
                pl.track,
                pl.slot
            ));
        }
        let times = placement_times(&DuplicateToArrangementParams {
            track_index: 0,
            clip_index: pl.slot,
            destination_time: None,
            destination_times: pl.times.clone(),
            start: pl.start,
            end: pl.end,
            step: pl.step,
            at_bar: None,
            until_bar: None,
            every_bars: None,
        })
        .map_err(|e| format!("placement {}: {e}", i + 1))?;
        placements.push((i, times));
    }
    let total_placements: usize = placements.iter().map(|(_, t)| t.len()).sum();
    let mut plan = format!(
        "Plan: {} track(s), {} clip(s) with {} notes, {} placement(s), {} locator(s){}.\n",
        p.tracks.len(),
        p.clips.len(),
        planned_notes.iter().map(Vec::len).sum::<usize>(),
        total_placements,
        p.locators.len(),
        p.tempo.map(|t| format!(", tempo {t}")).unwrap_or_default()
    );
    if p.dry_run {
        plan.push_str("Dry run: nothing was sent to Live.");
        return Ok(plan);
    }

    // ── execute, stopping at the first failure ──
    let mut done = plan;
    let fail = |done: &str, what: String| -> String {
        format!(
            "{done}Stopped: {what}\nWhat is above is in the set; the rest is not. Re-run the same document — build_song converges: a track whose name already exists is reused, and a slot that already holds the named clip is left alone.{SNAPSHOT_OFFER}"
        )
    };
    if let Some(k) = p.key.as_ref().filter(|k| !k.trim().is_empty()) {
        let text =
            crate::arrange::set_key_body(live, &crate::arrange::SetKeyParams { key: k.clone() })
                .map_err(|e| fail(&done, e))?;
        done.push_str(text.split(':').next().unwrap_or("Key set"));
        done.push_str(".\n");
    }
    if let Some(t) = p.tempo {
        set_tempo_body(live, &SetTempoParams { tempo: t }).map_err(|e| fail(&done, e))?;
        done.push_str(&format!("Tempo {t}.\n"));
    }
    if !p.scenes.is_empty() {
        require(live, "create_scene").map_err(|e| fail(&done, e))?;
        require(live, "set_scene").map_err(|e| fail(&done, e))?;
        let existing = read_perf_state(live)
            .map(|s| s.scenes.len() as i64)
            .unwrap_or(0);
        for (i, sc) in p.scenes.iter().enumerate() {
            let i = i as i64;
            let cmd = if i < existing {
                "set_scene"
            } else {
                "create_scene"
            };
            let args = if i < existing {
                json!({"index": i, "name": sc.name, "tempo": sc.tempo, "phrase_bars": sc.phrase_bars})
            } else {
                json!({"index": -1, "name": sc.name, "tempo": sc.tempo, "phrase_bars": sc.phrase_bars})
            };
            live.send_command(cmd, Some(args))
                .map_err(|e| fail(&done, live_err("set up the scenes", e)))?;
        }
        done.push_str(&format!(
            "Scenes: {}.\n",
            p.scenes
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{i} {}", s.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // Every track in one round trip: the instruments are resolved to URIs
    // here (the library index), the script creates, names, loads and mixes.
    let mut created: BTreeMap<String, i64> = BTreeMap::new();
    let mut reused: Vec<i64> = Vec::new();
    let mut skipped_clips = 0usize;
    let mut skipped_places = 0usize;
    if !p.tracks.is_empty() {
        require(live, "create_tracks").map_err(|e| fail(&done, e))?;
        let mut specs: Vec<Value> = Vec::new();
        let mut found: Vec<String> = Vec::new();
        for t in &p.tracks {
            // A browser URI always carries a ':' (query:Synths#…); plain words in
            // `instrument` are what the model meant by instrument_query.
            let query = match (&t.instrument, &t.instrument_query) {
                (Some(u), None) if !u.contains(':') => Some(u),
                (_, q) => q.as_ref(),
            };
            let uri = match (&t.instrument, query) {
                (Some(u), _) if u.contains(':') => Some(u.clone()),
                (_, Some(q)) => {
                    let hits = find_items(live, q, "all", 1).map_err(|e| fail(&done, e))?;
                    match hits.first() {
                        Some(h) => {
                            found.push(format!("'{}' for \"{q}\"", h.name));
                            Some(h.uri.clone())
                        }
                        None => {
                            return Err(fail(
                                &done,
                                format!(
                                    "track '{}': nothing in the browser matches \"{q}\"",
                                    t.name
                                ),
                            ))
                        }
                    }
                }
                _ => None,
            };
            let sends: Vec<Value> = t
                .sends
                .iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect();
            specs.push(json!({
                "name": t.name, "kind": t.kind, "instrument_uri": uri,
                "volume": t.fader, "volume_db": t.volume_db.or(t.volume), "pan": t.pan,
                "color_index": t.color_index, "sends": sends,
            }));
        }
        // Stopping part-way through the tracks is the failure this is shaped
        // around, so say what exists and how to carry on from it.
        let stopped = |done: &str, made: usize, what: String| -> String {
            let total = p.tracks.len();
            let mut out = format!("{done}Stopped after {made} of {total} tracks: {what}\n");
            out.push_str("The tracks listed above are in the set; nothing after them is. ");
            out.push_str("The document is unchanged, so run build_song again with it: ");
            out.push_str("on_existing \"converge\" (the default) reuses the tracks that ");
            out.push_str("exist and carries on from there.");
            out.push_str(SNAPSHOT_OFFER);
            out
        };

        // One look before the first group: it proves Live is answering, and
        // it says whether the transport is running. Adding a track or a
        // device reinitialises Live's audio graph, which pops by hand too,
        // so the reply says it rather than pretending the build was silent.
        let session = live
            .send_command("get_session_info", None)
            .map_err(|e| fail(&done, live_err("reach Live before building", e)))?;
        if session.get("is_playing").and_then(Value::as_bool) == Some(true) {
            done.push_str(
                "Built while playing: Live pops when a track or device is added — build before the performance.\n",
            );
        }

        // Live decides what already exists, inside the round trip: a re-run
        // of the document converges instead of building a second copy.
        //
        // The tracks go in groups rather than all at once. Every track
        // reinitialises the audio graph and most load a device from disk,
        // and ten of those in one task is what took Live down (#45): the
        // socket died mid-command and the set was left half built with
        // nothing said about it. A group is short enough to answer, and a
        // death costs one group instead of the document.
        for (group_no, group) in specs.chunks(TRACKS_PER_GROUP).enumerate() {
            let first = group_no * TRACKS_PER_GROUP;
            if group_no > 0 {
                // Ask something small between groups. If Live went away
                // while it was loading the last one, this says so now,
                // naming what exists, instead of blocking on a command that
                // will never be answered.
                live.send_command("get_session_info", None).map_err(|e| {
                    stopped(&done, first, live_err("reach Live between track groups", e))
                })?;
            }
            let r = live
                .send_command(
                    "create_tracks",
                    Some(json!({"tracks": group, "on_existing": mode})),
                )
                .map_err(|e| stopped(&done, first, live_err("create the tracks", e)))?;
            let entries = r
                .get("created")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if entries.len() != group.len() {
                return Err(stopped(
                    &done,
                    first,
                    format!(
                        "Live reported {} of {} tracks in this group",
                        entries.len(),
                        group.len()
                    ),
                ));
            }
            // Written as each group lands, so a failure later leaves the
            // reply naming every track that is really in the set.
            for (t, e) in p.tracks[first..].iter().zip(entries.iter()) {
                let index = e.get("index").and_then(Value::as_i64).unwrap_or(-1);
                created.insert(t.name.clone(), index);
                if e.get("reused").and_then(Value::as_bool) == Some(true) {
                    reused.push(index);
                    done.push_str(&format!("Track {index} '{}' — reused\n", t.name));
                    continue;
                }
                done.push_str(&format!(
                    "Track {index} '{}'{}\n",
                    t.name,
                    e.get("device")
                        .and_then(Value::as_str)
                        .map(|d| format!(" with {d}"))
                        .unwrap_or_default()
                ));
            }
        }
        if !found.is_empty() {
            done.push_str(&format!("Found in the library: {}.\n", found.join(", ")));
        }
    }
    // What the reused tracks already hold, so a re-run writes only what is
    // missing. Fresh tracks are empty by construction and cost no read.
    let mut slots_taken: BTreeMap<(i64, i64), String> = BTreeMap::new();
    if mode == "converge" && !reused.is_empty() {
        let mut seen: Vec<i64> = Vec::new();
        for c in &p.clips {
            let Ok(index) = song_track_index(&c.track, &created) else {
                continue;
            };
            if seen.contains(&index) || !reused.contains(&index) {
                continue;
            }
            seen.push(index);
            if let Ok(info) = live.send_command(
                "get_track_info",
                Some(json!({"track_index": index, "kind": "track"})),
            ) {
                for slot in info
                    .get("clip_slots")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let slot_index = slot.get("index").and_then(Value::as_i64).unwrap_or(-1);
                    if let Some(clip) = slot.get("clip").filter(|c| !c.is_null()) {
                        slots_taken.insert(
                            (index, slot_index),
                            get_display(clip, "name", "").to_lowercase(),
                        );
                    }
                }
            }
        }
    }
    // Every clip in one round trip; copies in other rows are made inside
    // Live from the first one, so the notes travel once.
    if !p.clips.is_empty() {
        require(live, "write_clips").map_err(|e| fail(&done, e))?;
        let mut specs: Vec<Value> = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        for (c, notes) in p.clips.iter().zip(planned_notes.iter()) {
            let track_index = song_track_index(&c.track, &created).map_err(|e| fail(&done, e))?;
            let mut slots: Vec<i64> = match c.slot {
                Some(s) => vec![s],
                None if !c.slots.is_empty() => Vec::new(),
                None => vec![0],
            };
            for s in &c.slots {
                if !slots.contains(s) {
                    slots.push(*s);
                }
            }
            let first = slots[0];
            let mut written_here = 0usize;
            for (i, slot) in slots.iter().enumerate() {
                if slots_taken.get(&(track_index, *slot)) == Some(&c.name.to_lowercase()) {
                    skipped_clips += 1;
                    continue;
                }
                written_here += 1;
                if i == 0 {
                    specs.push(json!({"track_index": track_index, "clip_index": slot, "name": c.name, "length": c.length, "notes": notes}));
                } else {
                    specs.push(json!({"track_index": track_index, "clip_index": slot, "name": c.name, "copy_of": first}));
                }
            }
            if written_here == 0 {
                continue;
            }
            lines.push(format!(
                "Clip '{}' on track {track_index} slot{} {} ({} notes).",
                c.name,
                if slots.len() == 1 { "" } else { "s" },
                slots
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                notes.len()
            ));
        }
        if specs.is_empty() {
            // Everything this document asks for is already in those slots.
            done.push_str("Every clip was already in its slot; nothing was written.\n");
        } else {
            let r = live
                .send_command("write_clips", Some(json!({"clips": specs})))
                .map_err(|e| fail(&done, live_err("write the clips", e)))?;
            let written = r
                .get("written")
                .and_then(Value::as_array)
                .map_or(specs.len(), Vec::len);
            for l in &lines {
                done.push_str(l);
                done.push('\n');
            }
            done.push_str(&format!(
                "{written} clip{} written in one round trip.\n",
                if written == 1 { "" } else { "s" }
            ));
        }
    }
    for (i, times) in &placements {
        let pl = &p.placements[*i];
        let track_index = song_track_index(&pl.track, &created).map_err(|e| fail(&done, e))?;
        // On a reused track, a beat that already holds this clip is left alone.
        let mut times = times.clone();
        if mode == "converge" && reused.contains(&track_index) {
            if let Ok(r) = live.send_command(
                "get_arrangement_clips",
                Some(json!({"track_index": track_index})),
            ) {
                let there: Vec<f64> = r
                    .get("clips")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.get("start_time").and_then(Value::as_f64))
                            .collect()
                    })
                    .unwrap_or_default();
                let before = times.len();
                times.retain(|t| !there.iter().any(|x| (x - t).abs() < 0.01));
                skipped_places += before - times.len();
            }
        }
        if times.is_empty() {
            continue;
        }
        let params = DuplicateToArrangementParams {
            track_index,
            clip_index: pl.slot,
            destination_time: None,
            destination_times: times.clone(),
            start: None,
            end: None,
            step: None,
            at_bar: None,
            until_bar: None,
            every_bars: None,
        };
        duplicate_to_arrangement_body(live, &params).map_err(|e| fail(&done, e))?;
        done.push_str(&format!(
            "Placed track {track_index} slot {} at {} position(s).\n",
            pl.slot,
            times.len()
        ));
    }
    for l in &p.locators {
        create_locator_body(
            live,
            &CreateLocatorParams {
                name: l.name.clone(),
                bar: None,
                time: Some(l.time),
            },
        )
        .map_err(|e| fail(&done, e))?;
        done.push_str(&format!("Locator '{}' at beat {}.\n", l.name, l.time));
    }
    if skipped_clips > 0 || skipped_places > 0 || !reused.is_empty() {
        done.push_str(&format!(
            "Converged: {} track(s) reused, {} clip(s) and {} placement(s) were already there — nothing was duplicated.\n",
            reused.len(),
            skipped_clips,
            skipped_places
        ));
    }
    if p.snapshot {
        // The flag is the consent: a set export is written only when it was
        // asked for (CLAUDE.md, tests/sets.rs), so this cannot become a
        // default without the story and the TERMS.md change that go with it.
        let name = format!("build-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        match crate::sets::export_set_body(live, &crate::sets::ExportSetParams { name }) {
            Ok(text) => done.push_str(&format!("Snapshot: {text}\n")),
            Err(e) => done.push_str(&format!(
                "Snapshot asked for but not written: {e}\nThe set itself is untouched; export_set can be run again.\n"
            )),
        }
    }
    done.push_str(
        "Done. switch_to_arrangement_view to see it; play_and_measure to hear the balance.",
    );
    if !p.snapshot {
        done.push_str(SNAPSHOT_OFFER);
    }
    Ok(done)
}

// ── Server ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Server {
    live: Arc<LiveState>,
    tool_router: ToolRouter<Self>,
}

/// A single main-thread slice longer than this, while the transport runs,
/// is audible on a small buffer: the reply says so.
pub const HELD_MS_THRESHOLD: f64 = 50.0;

/// The line under a reply when the call held Live's main thread for longer
/// than [`HELD_MS_THRESHOLD`] while the music played; None otherwise.
pub fn held_line(trace: &connection::CallTrace) -> Option<String> {
    let playing = trace
        .clock
        .as_ref()
        .and_then(|c| c.get("is_playing"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !playing || trace.main_ms < HELD_MS_THRESHOLD {
        return None;
    }
    let mut seen: Vec<&str> = Vec::new();
    for c in &trace.commands {
        if !seen.contains(&c.as_str()) {
            seen.push(c);
        }
    }
    Some(format!(
        "Held Live for {} ms while the music played ({}). Tracks, devices and scenes are Live's own work and cannot be sliced: make those edits before the show, or between sections.",
        trace.main_ms.round() as i64,
        seen.join(", ")
    ))
}

fn run_blocking<P: Serialize>(
    live: &LiveState,
    spec: &ToolSpec,
    params: &P,
    body: fn(&LiveState, &P) -> ToolResult,
) -> ToolResult {
    let start = Instant::now();
    connection::begin_trace();
    let _ = take_notes(live);
    // The per-session digest (#63 AC2): counted here, so a session that
    // never calls `remember` still leaves a trace of what it did.
    live.songs.note_call(spec.name);
    let result = body(live, params);
    // A guard may have ended a stale performance on the way in; the tool that
    // was being called says so.
    let result = match (take_notes(live), result) {
        (notes, Ok(text)) if !notes.is_empty() => Ok(format!("{} {text}", notes.join(" "))),
        (notes, Err(text)) if !notes.is_empty() => Err(format!("{} {text}", notes.join(" "))),
        (_, other) => other,
    };
    let trace = connection::end_trace();
    // While a performance runs the Remote Script attaches its clock to every
    // response; it ends every result here, success or error, at no cost. The
    // level line rides under it, and the song's plan cue reads as "next jump".
    let plan_cue = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|p| p.song.as_ref())
        .and_then(|s| s.plan_cue_id);
    let result = match trace.clock.as_ref().map(|c| perf::clock_lines(c, plan_cue)) {
        Some(line) => match result {
            Ok(text) => Ok(format!("{text}\n{line}")),
            Err(text) => Err(format!("{text}\n{line}")),
        },
        None => result,
    };
    // The cost line: what this call held Live's main thread for while the
    // music played, measured by the script (decision 0007). Under the
    // threshold nothing is said; over it the artist learns which edits to
    // make before the show.
    let result = match held_line(&trace) {
        Some(line) => match result {
            Ok(text) => Ok(format!("{text}\n{line}")),
            Err(text) => Err(format!("{text}\n{line}")),
        },
        None => result,
    };
    live.activity
        .record(spec.name, params, &result, start.elapsed(), trace);
    crate::app::refresh_heartbeat(live);
    result
}

impl Server {
    /// Every tool is always served: the artist's set ([`CORE_TOOLS`]) under
    /// its own names, the raw layer as `adv_<name>` so the two sets do not
    /// compete in a client's list and a search for the artist's words finds
    /// the artist's tools first. Nothing is hidden by the server.
    pub fn new(live: Arc<LiveState>) -> Self {
        Self {
            live,
            tool_router: Self::tool_router(),
        }
    }

    /// Every tool in one router: the main table plus the section and song
    /// tools; the raw layer renamed `adv_<name>`, every tool annotated.
    pub fn tool_router() -> ToolRouter<Self> {
        let mut router = Self::main_tool_router() + Self::section_tool_router();
        let names: Vec<String> = router.map.keys().map(|k| k.to_string()).collect();
        for name in names {
            let Some(mut route) = router.map.remove(name.as_str()) else {
                continue;
            };
            route.attr.annotations = Some(annotations_for(&name));
            let served = if CORE_TOOLS.contains(&name.as_str()) {
                name.clone()
            } else {
                format!("{ADVANCED_PREFIX}{name}")
            };
            route.attr.name = served.clone().into();
            router.map.insert(served.into(), route);
        }
        router
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

#[tool_router(router = main_tool_router)]
impl Server {
    /// Start here. One call: the set (tempo, key, signature, where the
    /// transport is), every track with its instrument and effects, fader in
    /// dB, pan, clips by slot with lengths, what plays and what is queued;
    /// the returns; the sections (scene rows with their phrase lengths, the
    /// playing one in [brackets]) and the song from the Setlist scene; the
    /// performance clock and what fired since the last read; and a one-line
    /// workflow reminder. include_library adds the browser inventory (slower);
    /// json: true returns the raw payload.
    #[tool(name = "get_context")]
    async fn get_context(&self, Parameters(p): Parameters<GetContextParams>) -> CallToolResult {
        self.run(&GET_CONTEXT, p, get_context_body).await
    }

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
    async fn get_track_info(&self, Parameters(p): Parameters<TrackInfoParams>) -> CallToolResult {
        self.run(&GET_TRACK_INFO, p, get_track_info_body).await
    }

    /// Read all MIDI notes from a Session-view clip. Returns pitch,
    /// start_time, duration, velocity, mute (and extended fields when
    /// available). The track and the clip are a name or an index.
    #[tool(name = "get_clip_notes")]
    async fn get_clip_notes(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&GET_CLIP_NOTES, p, get_clip_notes_body).await
    }

    /// Every parameter of one device, as Live shows it: the display string
    /// ("200 Hz", "Low Cut 48 dB"), the range, and for a switch or a chooser
    /// the list of values it accepts. `track` is a name, an index, "master",
    /// or a return's name or letter; `device` is a name (a substring is
    /// enough) or an index.
    #[tool(name = "get_device_parameters")]
    async fn get_device_parameters(
        &self,
        Parameters(p): Parameters<DeviceParams>,
    ) -> CallToolResult {
        self.run(&GET_DEVICE_PARAMETERS, p, get_device_parameters_body)
            .await
    }

    /// Set one device parameter. The value may be what Live displays ("3 dB",
    /// "200 Hz", "Low Cut 48 dB") or the raw number between min and max; the
    /// reply gives both, before and after. `track` is a name, an index,
    /// "master", or a return's name or letter. A chooser refuses a label it
    /// does not have, lists the ones it has, and changes nothing.
    #[tool(name = "set_device_parameter")]
    async fn set_device_parameter(
        &self,
        Parameters(p): Parameters<SetDeviceParameterParams>,
    ) -> CallToolResult {
        self.run(&SET_DEVICE_PARAMETER, p, set_device_parameter_body)
            .await
    }

    /// Remove, move, bypass or re-enable a device in a chain — on a track, a
    /// return or the master. action: "remove" takes it out (Cmd-Z in Live puts
    /// it back), "move" needs to_index, "bypass" leaves it in the chain and
    /// stops it processing, "enable" turns it back on. `device` is a name (a
    /// substring is enough) or an index; the reply lists the chain afterwards.
    #[tool(name = "edit_devices")]
    async fn edit_devices(&self, Parameters(p): Parameters<EditDevicesParams>) -> CallToolResult {
        self.run(&EDIT_DEVICES, p, edit_devices_body).await
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
    /// add_notes_to_clip afterwards. The clip length is `length` beats. The
    /// track is a name or an index, and `clip` a slot or the name of a clip
    /// on that track.
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
    /// `clear: true` to replace the clip's notes instead of appending. The
    /// track and the clip are a name or an index.
    #[tool(name = "add_notes_to_clip")]
    async fn add_notes_to_clip(&self, Parameters(p): Parameters<AddNotesParams>) -> CallToolResult {
        self.run(&ADD_NOTES_TO_CLIP, p, add_notes_to_clip_body)
            .await
    }

    /// Remove all MIDI notes from a Session clip. Writes are additive
    /// (add_notes_to_clip only appends), so to truly modify a clip: read the
    /// notes with get_clip_notes, edit the list, clear_notes_from_clip, then
    /// add_notes_to_clip the edited notes. The track and the clip are a name
    /// or an index.
    #[tool(name = "clear_notes_from_clip")]
    async fn clear_notes_from_clip(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&CLEAR_NOTES_FROM_CLIP, p, clear_notes_from_clip_body)
            .await
    }

    /// Set the name of a Session clip. The track and the clip are a name or
    /// an index.
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

    /// Load an instrument or effect: by plain words searched in the library
    /// ("reverb", "analog bass") or by browser URI. An instrument replaces
    /// the track's instrument; an effect goes at the end of the chain, so
    /// "put an Echo on the pad" is one call. The track is a name, an index,
    /// "master", or a return's name or letter; kind "return" puts an effect
    /// on a return track, "master" on the master. The result names the
    /// device.
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

    /// Start playing a clip on the global launch quantization. The reply
    /// says which bar it lands on, read from the transport after the fire;
    /// no_later_than: N refuses to gamble when bar N is closer than the round
    /// trip and schedules a cue for the next certain bar instead.
    #[tool(name = "fire_clip")]
    async fn fire_clip(&self, Parameters(p): Parameters<FireClipParams>) -> CallToolResult {
        self.run(&FIRE_CLIP, p, fire_clip_body).await
    }

    /// Stop playing a clip. The track and the clip are a name or an index.
    #[tool(name = "stop_clip")]
    async fn stop_clip(&self, Parameters(p): Parameters<ClipParams>) -> CallToolResult {
        self.run(&STOP_CLIP, p, stop_clip_body).await
    }

    /// Delete the clip in the given clip slot, freeing it for reuse. The
    /// track and the clip are a name or an index. Use this before create_clip
    /// when you want to overwrite an existing clip (create_clip refuses to
    /// write into an occupied slot).
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

    /// List all clips placed in the Arrangement timeline for a track (a name
    /// or an index): name, start_time, end_time, length, and type.
    #[tool(name = "get_arrangement_clips")]
    async fn get_arrangement_clips(
        &self,
        Parameters(p): Parameters<TrackParams>,
    ) -> CallToolResult {
        self.run(&GET_ARRANGEMENT_CLIPS, p, get_arrangement_clips_body)
            .await
    }

    /// Copy a Session clip into the Arrangement on the same track: at a bar
    /// (`at_bar`), or every `every_bars` from `at_bar` up to `until_bar`;
    /// beat positions (`destination_time`, `destination_times`, or `start`,
    /// `end`, `step`) also work. Every copy goes in one round trip. `arrange`
    /// is the bar-based tool for placing, repeating, moving and deleting.
    #[tool(name = "duplicate_to_arrangement")]
    async fn duplicate_to_arrangement(
        &self,
        Parameters(p): Parameters<DuplicateToArrangementParams>,
    ) -> CallToolResult {
        self.run(&DUPLICATE_TO_ARRANGEMENT, p, duplicate_to_arrangement_body)
            .await
    }

    /// Create a named locator (cue point) in the Arrangement at a bar. A
    /// locator already there is renamed instead.
    #[tool(name = "create_locator")]
    async fn create_locator(
        &self,
        Parameters(p): Parameters<CreateLocatorParams>,
    ) -> CallToolResult {
        self.run(&CREATE_LOCATOR, p, create_locator_body).await
    }

    /// Set a track's fader (volume_db, e.g. -6), pan, mute, solo or arm in
    /// one call. The track is a name, an index, "master", or a return's name
    /// or letter; returns (kind "return") and the master (kind "master")
    /// work too. The reply reads the fader back in dB.
    #[tool(name = "set_track_mixer")]
    async fn set_track_mixer(
        &self,
        Parameters(p): Parameters<SetTrackMixerParams>,
    ) -> CallToolResult {
        self.run(&SET_TRACK_MIXER, p, set_track_mixer_body).await
    }

    /// Route a track to a return track: set one send level (0.0-1.0) by
    /// the return's name ("Reverb"), its letter ("A") or its index. The
    /// track is a name or an index. Call get_returns first to see what
    /// returns exist.
    #[tool(name = "set_send")]
    async fn set_send(&self, Parameters(p): Parameters<SetSendParams>) -> CallToolResult {
        self.run(&SET_SEND, p, set_send_body).await
    }

    /// (advanced) What this Live has at a path in its object model: the
    /// class, every attribute with its type and whether it can be written,
    /// and the methods. Paths start at `song`, `application` or `browser` —
    /// `song.tracks[0].mixer_device.volume`. Use it when the artist's tools
    /// have no word for what you need, then reach it with run_ops.
    #[tool(name = "describe_live")]
    async fn describe_live(&self, Parameters(p): Parameters<DescribeLiveParams>) -> CallToolResult {
        self.run(&DESCRIBE_LIVE, p, describe_live_body).await
    }

    /// (advanced) Run operations against Live's object model in one round
    /// trip: get, set, call and wait_tick, in order, each with a path rooted
    /// at `song`, `application` or `browser`. `as` names a result to return.
    /// The first failure stops the batch and says which op failed; what ran
    /// before it has happened. Nothing outside Live is reachable.
    #[tool(name = "run_ops")]
    async fn run_ops(&self, Parameters(p): Parameters<RunOpsParams>) -> CallToolResult {
        self.run(&RUN_OPS, p, run_ops_body).await
    }

    /// List the return tracks (index, letter, name, level, devices) so sends
    /// can be addressed by name.
    #[tool(name = "get_returns")]
    async fn get_returns(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&GET_RETURNS, p, get_returns_body).await
    }

    /// Colour a track, or a clip when clip_index is given (Session slot, or
    /// Arrangement position with arrangement: true). color_index is Live's
    /// palette index 0-69.
    #[tool(name = "set_color")]
    async fn set_color(&self, Parameters(p): Parameters<SetColorParams>) -> CallToolResult {
        self.run(&SET_COLOR, p, set_color_body).await
    }

    /// Which sound sits on which pad of a track's Drum Rack: pitch, Live's
    /// note name (C1 = 36) and the pad's sample or chain name. Call this
    /// before writing drums so the pattern hits the intended sounds.
    #[tool(name = "get_drum_rack_pads")]
    async fn get_drum_rack_pads(
        &self,
        Parameters(p): Parameters<DrumRackPadsParams>,
    ) -> CallToolResult {
        self.run(&GET_DRUM_RACK_PADS, p, get_drum_rack_pads_body)
            .await
    }

    /// Remove clips from the Arrangement timeline: one `clip_index`, several
    /// `clip_indices`, or `all: true` for the whole track. Indices are the
    /// positions get_arrangement_clips lists (ordered by start time); later
    /// clips shift down by one. Live 11 or newer.
    #[tool(name = "delete_arrangement_clip")]
    async fn delete_arrangement_clip(
        &self,
        Parameters(p): Parameters<DeleteArrangementClipParams>,
    ) -> CallToolResult {
        self.run(&DELETE_ARRANGEMENT_CLIP, p, delete_arrangement_clip_body)
            .await
    }

    /// Delete a locator (cue point) by its exact name or its beat position.
    #[tool(name = "delete_locator")]
    async fn delete_locator(
        &self,
        Parameters(p): Parameters<DeleteLocatorParams>,
    ) -> CallToolResult {
        self.run(&DELETE_LOCATOR, p, delete_locator_body).await
    }

    /// Find a sound or device by words in its name or folder path — "analog
    /// bass", "techno kit", "reverb hall" — instead of walking folders.
    /// Returns names, paths and URIs ready for load_instrument_or_effect.
    /// Search one category (instruments, sounds, drums, audio_effects,
    /// midi_effects, samples, packs, user_library) or "all" of the first five.
    #[tool(name = "search_browser")]
    async fn search_browser(
        &self,
        Parameters(p): Parameters<SearchBrowserParams>,
    ) -> CallToolResult {
        self.run(&SEARCH_BROWSER, p, search_browser_body).await
    }

    /// Read a clip's settings: length, loop points, start and end markers,
    /// launch mode and quantization, legato, warp and gain — for a Session
    /// slot or an Arrangement clip.
    #[tool(name = "get_clip_info")]
    async fn get_clip_info(&self, Parameters(p): Parameters<ClipRefParams>) -> CallToolResult {
        self.run(&GET_CLIP_INFO, p, get_clip_info_body).await
    }

    /// Set a clip's loop (on/off, loop_start, loop_end) and its start and end
    /// markers, in beats from the clip's start. Session or Arrangement clip.
    #[tool(name = "set_clip_loop")]
    async fn set_clip_loop(&self, Parameters(p): Parameters<SetClipLoopParams>) -> CallToolResult {
        self.run(&SET_CLIP_LOOP, p, set_clip_loop_body).await
    }

    /// Set how a Session clip launches: launch_mode (trigger, gate, toggle,
    /// repeat), launch_quantization ("1_bar", "1/4", "none", "global", …),
    /// legato and velocity_amount.
    #[tool(name = "set_clip_launch")]
    async fn set_clip_launch(
        &self,
        Parameters(p): Parameters<SetClipLaunchParams>,
    ) -> CallToolResult {
        self.run(&SET_CLIP_LAUNCH, p, set_clip_launch_body).await
    }

    /// One reading of every track's output meter in dB, plus returns and
    /// master. Only meaningful while Live plays; for a reading over time use
    /// listen or play_and_measure.
    #[tool(name = "get_track_meters")]
    async fn get_track_meters(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&GET_TRACK_METERS, p, get_track_meters_body).await
    }

    /// Hear the balance without ears: play from start_time for a few
    /// seconds, sample the meters, stop, and report each track's peak level
    /// and which tracks stayed silent. Nothing in the set changes.
    #[tool(name = "play_and_measure")]
    async fn play_and_measure(
        &self,
        Parameters(p): Parameters<PlayAndMeasureParams>,
    ) -> CallToolResult {
        self.run(&PLAY_AND_MEASURE, p, play_and_measure_body).await
    }

    /// Write automation into a Session clip: a filter sweep, a volume fade,
    /// a send ride. Target a device parameter (device_index +
    /// parameter_index from get_device_parameters) or a mixer control
    /// (volume, pan, send). Give points [{time, value}] in clip beats, or one
    /// ramp {from, to, over}. Values are in the parameter's own range. Live's
    /// API cannot write into Arrangement clips: automate the Session clip,
    /// then place it (the envelope travels with it).
    #[tool(name = "set_clip_automation")]
    async fn set_clip_automation(
        &self,
        Parameters(p): Parameters<SetClipAutomationParams>,
    ) -> CallToolResult {
        self.run(&SET_CLIP_AUTOMATION, p, set_clip_automation_body)
            .await
    }

    /// Read a clip's automation for one target, sampled every `resolution`
    /// beats (Live exposes no point list).
    #[tool(name = "get_clip_automation")]
    async fn get_clip_automation(
        &self,
        Parameters(p): Parameters<GetClipAutomationParams>,
    ) -> CallToolResult {
        self.run(&GET_CLIP_AUTOMATION, p, get_clip_automation_body)
            .await
    }

    /// What this Live can actually use: its version, the instruments and
    /// effects present in the browser (and which Suite instruments are not),
    /// the packs installed, and the drum and sound folders. Call it before
    /// planning sounds, so nothing is written for a device that is not here.
    /// Packs that are not downloaded are invisible to Live's API; the result
    /// says where to install them.
    #[tool(name = "get_library_status")]
    async fn get_library_status(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&GET_LIBRARY_STATUS, p, get_library_status_body)
            .await
    }

    /// What the devices on this Live have answered to, learned as you work
    /// and kept between sessions and songs: each device's parameter names,
    /// the values that were written with what Live displayed for them, and
    /// the words a device does **not** answer to — so the same failed call
    /// is not paid for twice. A macro that ran backwards is reported here
    /// with its measurements, never silently corrected. `action: "forget"`
    /// deletes the file; nothing in Live changes. Asks Live nothing.
    #[tool(name = "device_vocabulary")]
    async fn device_vocabulary(
        &self,
        Parameters(p): Parameters<DeviceVocabularyParams>,
    ) -> CallToolResult {
        self.run(&DEVICE_VOCABULARY, p, device_vocabulary_body)
            .await
    }

    /// Hear the result as numbers: record `bars` bars of the arrangement
    /// from `start_bar` through a Capture track (made once, input
    /// Resampling, muted), then report peak dBFS, RMS, crest factor, ten
    /// octave bands, the low-to-high ratio and RMS per bar, with a reading
    /// you can act on. The playhead is confirmed at the starting bar before
    /// recording begins; a take that starts early is discarded and recorded
    /// again rather than measured. `start_bar` is a bar number or a
    /// locator's name. The clip stays on the Capture track so you
    /// can play it; clear_captures removes the track when you are done.
    /// Nothing else in the set changes. One capture at a time; 1–64 bars.
    #[tool(name = "capture_mix")]
    async fn capture_mix(&self, Parameters(p): Parameters<CaptureMixParams>) -> CallToolResult {
        self.run(&CAPTURE_MIX, p, capture_mix_body).await
    }

    /// The captures on the Capture track: slot, name, length, file path.
    #[tool(name = "list_captures")]
    async fn list_captures(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&LIST_CAPTURES, p, list_captures_body).await
    }

    /// Re-read the levels of an existing capture by slot, without playing.
    #[tool(name = "measure_capture")]
    async fn measure_capture(
        &self,
        Parameters(p): Parameters<MeasureCaptureParams>,
    ) -> CallToolResult {
        self.run(&MEASURE_CAPTURE, p, measure_capture_body).await
    }

    /// Empty the open Live set back to what a new one is: the tracks, the
    /// scenes, the tempo, and every clip and locator gone. Live's API has
    /// no File > New, so this clears the set that is open rather than
    /// opening a fresh one, and nothing is saved until you save it in Live.
    #[tool(name = "adv_reset_set")]
    async fn reset_set(&self, Parameters(p): Parameters<ResetSetParams>) -> CallToolResult {
        self.run(&RESET_SET, p, reset_set_body).await
    }

    /// Delete a track, by name or by index. Later tracks move up by one.
    /// Refused while a performance runs if the track is playing.
    #[tool(name = "delete_track")]
    async fn delete_track(&self, Parameters(p): Parameters<TrackParams>) -> CallToolResult {
        self.run(&DELETE_TRACK, p, delete_track_body).await
    }

    /// After launching Session clips, make every track follow the
    /// Arrangement timeline again (Live's "Back to Arrangement" button).
    #[tool(name = "back_to_arrangement")]
    async fn back_to_arrangement(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&BACK_TO_ARRANGEMENT, p, back_to_arrangement_body)
            .await
    }

    /// Set the Arrangement loop brace (start and length in beats) and switch
    /// it on or off — the way to audition one section repeatedly.
    #[tool(name = "set_arrangement_loop")]
    async fn set_arrangement_loop(
        &self,
        Parameters(p): Parameters<SetArrangementLoopParams>,
    ) -> CallToolResult {
        self.run(&SET_ARRANGEMENT_LOOP, p, set_arrangement_loop_body)
            .await
    }

    /// Go live. Sets the global launch quantization (default 1 bar), reads
    /// the key (from the set on Live 12, else from `key`), fires the named
    /// scene if given and starts the transport if it is stopped, and turns on
    /// the performance guards: until end_performance, stop_playback,
    /// set_arrangement_time, set_tempo, capture_mix, play_and_measure,
    /// switch_to_arrangement_view, back_to_arrangement, and
    /// delete_track/delete_clip on anything playing or queued are refused
    /// with a message that names the on-the-bar alternative. Keeps the
    /// performance as a take in the Arrangement (`record`, default "ask"):
    /// from bar 1 when the Arrangement is empty, and when it is not, nothing
    /// is started — the reply says what is there and offers "after" (record
    /// after it), "replace" (delete it and record from bar 1) and "off". The
    /// answer is remembered for the session, except "replace", which is asked
    /// every time. There is no undo in this server, so "replace" says how many
    /// bars it deleted and that Cmd+Z in Live is the way back. Live's API
    /// cannot save the set: the producer presses Cmd+S. Recording needs Live
    /// 11 or newer; on Live 10 nothing is recorded and the reply says why.
    /// Apart from the take, nothing in the set changes except the
    /// quantization. Refused if a performance is already running. Then: get_performance_state to see where the set is,
    /// cue for timed moves, fire_scene for the next section, record_clip to
    /// record the producer playing.
    #[tool(name = "start_performance")]
    async fn start_performance(
        &self,
        Parameters(p): Parameters<StartPerformanceParams>,
    ) -> CallToolResult {
        self.run(&START_PERFORMANCE, p, start_performance_body)
            .await
    }

    /// Where the set is: bar.beat, tempo, signature, seconds since the
    /// performance started, launch quantization, key; per track the playing
    /// clip and the queued clip (● marks an armed track); the scenes with the
    /// playing one in [brackets]; pending cues with their next step; seconds
    /// to the next bar; and everything the Remote Script's clock did since
    /// the last call (cue steps fired, cues cancelled, recordings finished,
    /// the transport stopped in Live). Read-only. Bar numbers are Live's own.
    /// bar_map: true adds the next 32 bars with every cue step and phrase
    /// boundary. The result also reports the measured round trip to Live.
    #[tool(name = "get_performance_state")]
    async fn get_performance_state(
        &self,
        Parameters(p): Parameters<GetPerformanceStateParams>,
    ) -> CallToolResult {
        self.run(&GET_PERFORMANCE_STATE, p, get_performance_state_body)
            .await
    }

    /// Schedule steps on Live's clock. Each step has a time — "next_bar",
    /// {"bar": N} in Live's bar numbers, or {"bars_after": k} counted from
    /// the next bar — and exactly one action: fire_scene (name or index),
    /// fire_clip / stop_clip ({"track": name or index, "clip": slot}),
    /// stop_all_clips: true, set ({"target": "tempo"|"crossfader"|"volume"|
    /// "mute"|"send"|"device", "track": …, "value": …}), or a ramp over
    /// `bars` from `from`: {"tempo": 134}, {"crossfader": 1.0}, {"volume":
    /// 0.5, "track": "Bass"}, {"send": 0.3, "track": "Pad", "send_index": 0}
    /// or {"target": "device", "track": …, "device_index": …,
    /// "parameter_index": …, "to": …}. The Remote Script executes the cue on
    /// its own clock, so it happens even if this server is slow or gone.
    /// Launches are placed by Live's quantization and land exactly on the
    /// bar; sets and ramp steps land within one script tick of the beat (a
    /// filter sweep belongs in set_clip_automation, which is sample-accurate;
    /// cue ramps are for tempo and the crossfader). Refuses a step in the
    /// past, an unknown scene or clip, a ramp longer than 64 bars, and a plan
    /// that leaves a bar with nothing playing unless allow_silence is true.
    /// Returns the cue id and the plan as it will run, with warnings when a
    /// step is less than a bar ahead or the quantization is not 1 bar.
    #[tool(name = "cue")]
    async fn cue(&self, Parameters(p): Parameters<CueParams>) -> CallToolResult {
        self.run(&CUE, p, cue_body).await
    }

    /// Cancel a pending cue by id; steps already fired stay fired.
    #[tool(name = "cancel_cue")]
    async fn cancel_cue(&self, Parameters(p): Parameters<CancelCueParams>) -> CallToolResult {
        self.run(&CANCEL_CUE, p, cancel_cue_body).await
    }

    /// Fire a scene by name or index, quantized by Live to the next bar (or
    /// the current launch quantization). Says which bar it starts on, names
    /// any clip in the row whose own launch quantization is finer than a bar,
    /// and warns when an armed track has an empty slot in the scene (Live
    /// records into it if Start Recording on Scene Launch is on). For timed
    /// moves use cue.
    #[tool(name = "fire_scene")]
    async fn fire_scene(&self, Parameters(p): Parameters<FireSceneParams>) -> CallToolResult {
        self.run(&FIRE_SCENE, p, fire_scene_body).await
    }

    /// Add a scene (a row of slots) at an index or the end, optionally named
    /// and with a scene tempo (Live 11+). Returns its index so create_clip can
    /// fill its row before a cue fires it.
    #[tool(name = "create_scene")]
    async fn create_scene(&self, Parameters(p): Parameters<CreateSceneParams>) -> CallToolResult {
        self.run(&CREATE_SCENE, p, create_scene_body).await
    }

    /// Record the producer playing into a Session clip: arms the track, fires
    /// its first empty slot on the next bar (the global launch quantization)
    /// for `bars` bars, and lets the clip loop when the recording ends; the
    /// Remote Script names the clip and disarms the track by itself. Live 11+.
    /// Refused while the track is already recording.
    #[tool(name = "record_clip")]
    async fn record_clip(&self, Parameters(p): Parameters<RecordClipParams>) -> CallToolResult {
        self.run(&RECORD_CLIP, p, record_clip_body).await
    }

    /// The global launch quantization (Live's transport-bar setting): none,
    /// 8_bars, 4_bars, 2_bars, 1_bar, 1/2, 1/2t, 1/4, 1/4t, 1/8, 1/8t, 1/16,
    /// 1/16t, 1/32. This is what quantizes fire_clip, fire_scene, record_clip
    /// and cue launches. start_performance sets it to 1_bar.
    #[tool(name = "set_launch_quantization")]
    async fn set_launch_quantization(
        &self,
        Parameters(p): Parameters<SetLaunchQuantizationParams>,
    ) -> CallToolResult {
        self.run(&SET_LAUNCH_QUANTIZATION, p, set_launch_quantization_body)
            .await
    }

    /// Move the master crossfader (0 = A, 1 = B) and/or assign tracks to A, B
    /// or neither. For a timed blend, ramp the crossfader in a cue.
    #[tool(name = "set_crossfader")]
    async fn set_crossfader(
        &self,
        Parameters(p): Parameters<SetCrossfaderParams>,
    ) -> CallToolResult {
        self.run(&SET_CROSSFADER, p, set_crossfader_body).await
    }

    /// End the performance: stop on a bar ({"at": "next_bar"} or {"at":
    /// {"bar": N}}; the default), fade the master over `fade_bars` bars and
    /// stop (the master volume is restored after the stop), or stop now.
    /// Cancels pending cues, lifts the guards, and reports the set: duration,
    /// cues scheduled and cancelled. The launch quantization stays as it is.
    /// Stops the Arrangement take if one was recording, reports the bars it
    /// covers and how many tracks it touched, and puts the tracks back on the
    /// timeline with Back to Arrangement. A stop scheduled on a bar or after a
    /// fade ends the take now, so what plays until the transport stops is not
    /// in it.
    #[tool(name = "end_performance")]
    async fn end_performance(
        &self,
        Parameters(p): Parameters<EndPerformanceParams>,
    ) -> CallToolResult {
        self.run(&END_PERFORMANCE, p, end_performance_body).await
    }

    /// Name a scene, give it a tempo, or set its phrase length (bars per
    /// phrase, default 16) — what "next_phrase" and {"phrases_after": n} count
    /// in cues, from the bar the scene was fired on.
    #[tool(name = "set_scene")]
    async fn set_scene(&self, Parameters(p): Parameters<SetSceneParams>) -> CallToolResult {
        self.run(&SET_SCENE, p, set_scene_body).await
    }

    /// Listen without stopping the set: the peak and average level of every
    /// track, return and the master in dB over the next `bars` bars, from
    /// the next bar line (the master is flagged within 0.5 dB of clipping).
    /// capture: true records the bars through the Capture track instead and
    /// adds RMS per bar and low/mid/high balance. Allowed during a performance.
    #[tool(name = "listen")]
    async fn listen(&self, Parameters(p): Parameters<ListenParams>) -> CallToolResult {
        self.run(&LISTEN, p, listen_body).await
    }

    /// A variation of an existing clip's notes: fill_last_bar (16th repeats of
    /// its own pitches into the downbeat), ghost_notes (quiet notes on empty
    /// 16ths), invert_chords, thin (every other off-beat note dropped),
    /// half_time, double_time. Seeded: the same seed gives the same result.
    /// to_slot writes a new clip; without it the clip is changed in place and
    /// undo_vary restores it (one level).
    #[tool(name = "vary_clip")]
    async fn vary_clip(&self, Parameters(p): Parameters<VaryClipParams>) -> CallToolResult {
        self.run(&VARY_CLIP, p, vary_clip_body).await
    }

    /// Restore the notes a clip had before the last in-place vary_clip.
    #[tool(name = "undo_vary")]
    async fn undo_vary(&self, Parameters(p): Parameters<UndoVaryParams>) -> CallToolResult {
        self.run(&UNDO_VARY, p, undo_vary_body).await
    }

    /// Read the key of what the producer just played (a record_clip clip),
    /// set Live's scale to it, and transpose every other MIDI clip to match.
    /// start_performance {"follow_key": true} does this after every recording.
    #[tool(name = "follow_key")]
    async fn follow_key(&self, Parameters(p): Parameters<FollowKeyParams>) -> CallToolResult {
        self.run(&FOLLOW_KEY, p, follow_key_body).await
    }

    /// Remember every track's, return's and master's volume, pan, sends and
    /// mute in one round trip, so an experiment can be reverted on the bar:
    /// restore_mix, or a cue step {"gesture": {"restore_mix": {"snapshot": id}}}.
    #[tool(name = "snapshot_mix")]
    async fn snapshot_mix(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&SNAPSHOT_MIX, p, snapshot_mix_body).await
    }

    /// Put the mix back as a snapshot had it, now. For "on the bar", cue it.
    #[tool(name = "restore_mix")]
    async fn restore_mix(&self, Parameters(p): Parameters<RestoreMixParams>) -> CallToolResult {
        self.run(&RESTORE_MIX, p, restore_mix_body).await
    }

    /// Fade every playing track except `keep` to silence over `bars` bars
    /// from now, then stop them (faders stay down: snapshot_mix first if you
    /// want to restore them). Runs on the Remote Script's clock.
    #[tool(name = "panic")]
    async fn panic(&self, Parameters(p): Parameters<PanicParams>) -> CallToolResult {
        self.run(&PANIC, p, panic_body).await
    }

    /// Let a track play through scene launches: removes the stop buttons
    /// from its empty Session slots, so a layer added mid-set (a lead, a
    /// pad) survives every section change without copying the clip into
    /// every row. A clip in a row still replaces it. keep: false puts the
    /// stop buttons back. cue's silence check knows about it.
    #[tool(name = "keep_track_playing")]
    async fn keep_track_playing(
        &self,
        Parameters(p): Parameters<KeepTrackPlayingParams>,
    ) -> CallToolResult {
        self.run(&KEEP_TRACK_PLAYING, p, keep_track_playing_body)
            .await
    }

    /// Run several tool calls in one round-trip: an ordered list of
    /// {tool, args}. Stops at the first failure (unless stop_on_error is
    /// false). Answers with a grouped summary, every failure in full and
    /// every line a step skipped; pass verbose: true for each successful
    /// step's own text (a batch of ten or fewer prints in full anyway).
    /// Inside args, "$last_track" stands for the index of the most recently
    /// created track, so "create a track, name it, load a sound, fill a
    /// clip" is one call.
    #[tool(name = "batch")]
    async fn batch(&self, Parameters(p): Parameters<BatchParams>) -> CallToolResult {
        self.run(&BATCH, p, batch_body).await
    }

    /// Build a whole arrangement from one document: tracks (with an
    /// instrument by URI or search words, level, pan, sends, colour), Session
    /// clips with their notes in any compact form, Arrangement placements
    /// (times or start/end/step) and locators. Everything is validated before
    /// the first command reaches Live; dry_run: true shows the plan only.
    /// Stops at the first failure and says what was built — re-run the same
    /// document and it converges: a track whose name already exists is reused
    /// and a slot that already holds the named clip is left alone, so nothing
    /// is duplicated (on_existing: "add" or "fail" changes that). What it
    /// builds lives only in Live's memory until you save the set in Live —
    /// the Live API has no save — so snapshot: true writes a rebuildable copy
    /// under the server's state folder, the way export_set does.
    #[tool(name = "build_song")]
    async fn build_song(&self, Parameters(p): Parameters<BuildSongParams>) -> CallToolResult {
        self.run(&BUILD_SONG, p, build_song_body).await
    }
}

#[tool_router(router = section_tool_router)]
impl Server {
    /// A new section: a scene row named "<name> · <bars>" (its phrase
    /// length, kept by Live's Save). Three sources: from: "playing" copies
    /// what plays into a new row below the playing one (Live's
    /// capture-and-insert-scene; the set keeps playing); from: {"section":
    /// "Groove"} copies that row and applies `changes` per track (a
    /// vary_clip variation name, {"transpose": -12}, "empty", or replacement
    /// notes); `clips` writes it from notes per track in any compact form
    /// (tracks not named stay empty and stop when it fires). At the end or
    /// after a named section. A duplicate name is refused; replace: true
    /// rewrites a `clips` section in place so the setlist keeps its name.
    #[tool(name = "make_section")]
    async fn make_section(
        &self,
        Parameters(p): Parameters<crate::sections::MakeSectionParams>,
    ) -> CallToolResult {
        self.run(&MAKE_SECTION, p, crate::sections::make_section_body)
            .await
    }

    /// The song: an ordered setlist of sections. An entry without repeats
    /// loops until you say go; with repeats the song moves on by itself.
    /// Validates every section, refuses one that would leave the set silent,
    /// and writes the 'Setlist:' scene (an empty row at the bottom, kept by
    /// Live's Save; edit its name in Live to change the order). A running
    /// song is re-planned from where it is.
    #[tool(name = "set_song")]
    async fn set_song(
        &self,
        Parameters(p): Parameters<crate::sections::SetSongParams>,
    ) -> CallToolResult {
        self.run(&SET_SONG, p, crate::sections::set_song_body).await
    }

    /// Put a section into the song: after or before a named entry, or at
    /// the end; with repeats to pre-plan it. Re-plans a running song.
    #[tool(name = "add_to_song")]
    async fn add_to_song(
        &self,
        Parameters(p): Parameters<crate::sections::AddToSongParams>,
    ) -> CallToolResult {
        self.run(&ADD_TO_SONG, p, crate::sections::add_to_song_body)
            .await
    }

    /// Take every entry of a section out of the song. Re-plans a running song.
    #[tool(name = "remove_from_song")]
    async fn remove_from_song(
        &self,
        Parameters(p): Parameters<crate::sections::RemoveFromSongParams>,
    ) -> CallToolResult {
        self.run(&REMOVE_FROM_SONG, p, crate::sections::remove_from_song_body)
            .await
    }

    /// Play the song: starts a performance if none runs (start_performance's
    /// defaults), fires `from` (default the first entry) now, and schedules
    /// every counted jump as one cue at phrase boundaries, waiting at the
    /// first entry without a count. Then go, next_section, previous_section,
    /// back and jump_to steer it; hold_section stops a count. Every reply
    /// carries the plan, the clock line and the level line. Keeps the
    /// performance as a take in the Arrangement (`record`, default "ask" —
    /// see start_performance): with something already in the Arrangement the
    /// song does not start until the producer says "after", "replace" or
    /// "off".
    #[tool(name = "play_song")]
    async fn play_song(
        &self,
        Parameters(p): Parameters<crate::sections::PlaySongParams>,
    ) -> CallToolResult {
        self.run(&PLAY_SONG, p, crate::sections::play_song_body)
            .await
    }

    /// Stop a running count: the playing section loops from here until go.
    #[tool(name = "hold_section")]
    async fn hold_section(&self, Parameters(p): Parameters<Empty>) -> CallToolResult {
        self.run(&HOLD_SECTION, p, crate::sections::hold_section_body)
            .await
    }

    /// Continue the song: the next setlist entry at the end of the playing
    /// section's phrase (the next multiple of its phrase length from the
    /// bar it started on, however many passes it looped), or at: "next_bar"
    /// to cut the phrase short (the reply says by how many bars). With a
    /// count still running it says so and changes nothing.
    #[tool(name = "go")]
    async fn go(&self, Parameters(p): Parameters<crate::sections::SteerParams>) -> CallToolResult {
        self.run(&GO, p, crate::sections::go_body).await
    }

    /// The next setlist entry, at the end of this phrase or on the next bar;
    /// the song continues after it. Warns when the target last ran more
    /// than 3 dB hotter than this section (force: true skips the warning).
    #[tool(name = "next_section")]
    async fn next_section(
        &self,
        Parameters(p): Parameters<crate::sections::SteerParams>,
    ) -> CallToolResult {
        self.run(&NEXT_SECTION, p, crate::sections::next_section_body)
            .await
    }

    /// The previous setlist entry, at the end of this phrase or on the next bar.
    #[tool(name = "previous_section")]
    async fn previous_section(
        &self,
        Parameters(p): Parameters<crate::sections::SteerParams>,
    ) -> CallToolResult {
        self.run(&PREVIOUS_SECTION, p, crate::sections::previous_section_body)
            .await
    }

    /// Return to the section before the last jump (the jump history, so a
    /// mistaken jump_to goes back to where you were, not to the entry
    /// before it), at the end of this phrase or on the next bar.
    #[tool(name = "back")]
    async fn back(
        &self,
        Parameters(p): Parameters<crate::sections::SteerParams>,
    ) -> CallToolResult {
        self.run(&BACK, p, crate::sections::back_body).await
    }

    /// Set the key: "F minor", "D dorian", "G". Written into Live 12's scale
    /// settings, so the clip editor shows it and everything written follows
    /// it. A fresh set sits in C Major until you do.
    #[tool(name = "set_key")]
    async fn set_key(
        &self,
        Parameters(p): Parameters<crate::arrange::SetKeyParams>,
    ) -> CallToolResult {
        self.run(&SET_KEY, p, crate::arrange::set_key_body).await
    }

    /// A new return track, optionally with an effect on it ("reverb",
    /// "ping pong delay", or a URI). Feed it with set_send.
    #[tool(name = "create_return")]
    async fn create_return(
        &self,
        Parameters(p): Parameters<crate::arrange::CreateReturnParams>,
    ) -> CallToolResult {
        self.run(&CREATE_RETURN, p, crate::arrange::create_return_body)
            .await
    }

    /// Remove the Capture track that capture_mix and listen record into
    /// (keep_track: true removes only its clips). The audio files stay in
    /// the project's Samples/Recorded folder.
    #[tool(name = "clear_captures")]
    async fn clear_captures(
        &self,
        Parameters(p): Parameters<crate::arrange::ClearCapturesParams>,
    ) -> CallToolResult {
        self.run(&CLEAR_CAPTURES, p, crate::arrange::clear_captures_body)
            .await
    }

    /// Keep what Live cannot hold. `overview` is your model of the song as
    /// an object — what it is trying to be, the plan section by section,
    /// what each track is for, what was decided and why, what is next — and
    /// the whole of it comes back in the get_context header next session,
    /// with no call to fetch it. Named keys merge, so a patch of one leaves
    /// the rest alone; `replace: true` rewrites it. `about` + `note` keeps
    /// one thing about the song, a track or a section ("section:Drop");
    /// `about` alone reads them back. `about` + `role` writes a role into
    /// the track's name (`Sitar [lead]`), so Live's Save keeps it, it
    /// travels in the .als, and the role then addresses that track in every
    /// tool. Local, capped at 8 KB, and off with ABLETON_MCP_SONG_MEMORY=false.
    #[tool(name = "remember")]
    async fn remember(
        &self,
        Parameters(p): Parameters<crate::memory::RememberParams>,
    ) -> CallToolResult {
        self.run(&REMEMBER, p, crate::memory::remember_body).await
    }

    /// Park an idea where it can still be heard. `save` puts a Session clip
    /// (`track` + `clip`), or audio (`sample`: words, a path or a browser
    /// URI, placed the way add_sample does), into a `Stash:` scene row —
    /// so three candidates can be fired against the song before one is
    /// committed. `list` shows what is parked with its track, bars, notes
    /// and tags; `place` copies one into a section and leaves the parked
    /// copy; `drop` removes one. A `Stash:` row is never a section: it is
    /// not listed, launched, counted or played. All of it lives in your Live
    /// set, kept by Live's own Save — not in any file of the server's.
    #[tool(name = "stash")]
    async fn stash(&self, Parameters(p): Parameters<crate::memory::StashParams>) -> CallToolResult {
        self.run(&STASH, p, crate::memory::stash_body).await
    }

    /// What is remembered about this song, and what is not: `show` names the
    /// file, its size, the overview, the notes, the sessions, and says
    /// plainly what never enters it (no MIDI note, no audio, no path outside
    /// the set's own). `list` every song on this machine, `attach` notes
    /// whose track was renamed, `forget` deletes this song's file and
    /// touches nothing in Live — the roles are in your track names and the
    /// stash is a scene row in your set.
    #[tool(name = "song_memory")]
    async fn song_memory(
        &self,
        Parameters(p): Parameters<crate::memory::SongMemoryParams>,
    ) -> CallToolResult {
        self.run(&SONG_MEMORY, p, crate::memory::song_memory_body)
            .await
    }

    /// Make a clip feel played: swing (delay the off-beats by a fraction of
    /// the step), humanize_ms (hits drift early or late, velocities vary),
    /// groove (one from the set's Groove Pool, Live's own, non-destructive),
    /// groove_amount, retime (half_time / double_time), or a variation
    /// (fill_last_bar, ghost_notes, invert_chords, thin). Any mix in one
    /// call, seeded; undo: true puts the clip back as it was before.
    #[tool(name = "feel")]
    async fn feel(&self, Parameters(p): Parameters<crate::arrange::FeelParams>) -> CallToolResult {
        self.run(&FEEL, p, crate::arrange::feel_body).await
    }

    /// The Arrangement in bars: place a Session clip at a bar (or every N
    /// bars up to a bar), repeat an Arrangement clip after itself, move one
    /// to a bar, delete the clips that start in a bar range, shorten the
    /// whole arrangement to end at a bar, or list what is there. Tracks and
    /// clips are named or indexed, and every bar is a number or a locator's
    /// name. One round trip per track, however many clips.
    #[tool(name = "arrange")]
    async fn arrange(
        &self,
        Parameters(p): Parameters<crate::arrange::ArrangeParams>,
    ) -> CallToolResult {
        self.run(&ARRANGE, p, crate::arrange::arrange_body).await
    }

    /// Write the whole set as a rebuildable document under the server's
    /// sets folder (tracks with every device by name and instruments by
    /// browser URI, Session clips with their notes, mixer and sends,
    /// sections with phrase lengths, the setlist, tempo, signature and key).
    /// Only on request: nothing exports on its own. The file holds your
    /// notes and names; delete it from the folder or with "Delete all local
    /// data". The Live set stays the memory (save it in Live).
    #[tool(name = "export_set")]
    async fn export_set(
        &self,
        Parameters(p): Parameters<crate::sets::ExportSetParams>,
    ) -> CallToolResult {
        self.run(&EXPORT_SET, p, crate::sets::export_set_body).await
    }

    /// Rebuild an exported set through build_song and set_song: validates
    /// the document, refuses a set that already has tracks unless merge:
    /// true, sets the key, writes the setlist. Audio clips are named, not
    /// rebuilt. dry_run describes the document without touching Live.
    #[tool(name = "import_set")]
    async fn import_set(
        &self,
        Parameters(p): Parameters<crate::sets::ImportSetParams>,
    ) -> CallToolResult {
        self.run(&IMPORT_SET, p, crate::sets::import_set_body).await
    }

    /// Shape a sound in words: cutoff, resonance, attack, decay, sustain,
    /// release, drive, detune, width, lfo_rate, reverb, delay, each a
    /// fraction 0–1 of the parameter's range or "±N%" of where it sits. The
    /// words are resolved against the device's rack macros first (most Live
    /// presets are racks), then a table per Live instrument, then parameter
    /// names; several words are one round trip. The reply names each
    /// parameter with its before and after in Live's display units and the
    /// words this device answers to; an unknown device lists its parameters
    /// for set_device_parameter by name.
    #[tool(name = "shape_sound")]
    async fn shape_sound(&self, Parameters(p): Parameters<ShapeSoundParams>) -> CallToolResult {
        self.run(&SHAPE_SOUND, p, shape_sound_body).await
    }

    /// Give a Session clip a groove from this set's Groove Pool (Live's own,
    /// non-destructive, Live 11+), by name or index, with the groove's
    /// timing/random/velocity amounts; "none" removes it. The API cannot add
    /// a groove to the pool: an unknown name lists the pool and says what to
    /// drag in, or use humanize / swing_notes (note rewrites, undoable).
    #[tool(name = "groove_clip")]
    async fn groove_clip(&self, Parameters(p): Parameters<GrooveClipParams>) -> CallToolResult {
        self.run(&GROOVE_CLIP, p, groove_clip_body).await
    }

    /// The set's global groove amount (Live's Groove Pool "Amount"), 0–1.
    #[tool(name = "groove_amount")]
    async fn groove_amount(&self, Parameters(p): Parameters<GrooveAmountParams>) -> CallToolResult {
        self.run(&GROOVE_AMOUNT, p, groove_amount_body).await
    }

    /// Loosen a clip: every hit lands up to timing_ms early or late (a
    /// seeded, bell-shaped drift, never across a bar line) and velocities
    /// vary by up to `velocity`, off-beat notes most. A note rewrite; the
    /// reply says what to listen for in note values, and undo_vary restores.
    #[tool(name = "humanize")]
    async fn humanize(&self, Parameters(p): Parameters<HumanizeParams>) -> CallToolResult {
        self.run(&HUMANIZE, p, humanize_body).await
    }

    /// Swing a clip: the off-beat 16ths (or 8ths) are delayed by `amount`
    /// of the grid step; on-beat notes stay. A note rewrite; undo_vary
    /// restores. For Live's own groove use groove_clip.
    #[tool(name = "swing_notes")]
    async fn swing_notes(&self, Parameters(p): Parameters<SwingNotesParams>) -> CallToolResult {
        self.run(&SWING_NOTES, p, swing_notes_body).await
    }

    /// Half- or double-time a clip's notes (the rewrite a jump's `retime`
    /// transition makes), in place with one undo_vary, or into a free slot.
    #[tool(name = "retime_clip")]
    async fn retime_clip(&self, Parameters(p): Parameters<RetimeClipParams>) -> CallToolResult {
        self.run(&RETIME_CLIP, p, retime_clip_body).await
    }

    /// Go to any section by name at the end of this phrase (default) or on
    /// the next bar, optionally for a number of passes, after which the song
    /// continues from the entry after it. `transition` composes tempo ramps,
    /// half/double-time rewrites into the target row, crossfades between
    /// track groups, a fill, a drop or a sweep into the same cue; the
    /// default is a straight cut. One state read and one cue; the old plan
    /// is replaced in the same call. Warns when the target last ran more
    /// than 3 dB hotter (force: true skips it).
    #[tool(name = "jump_to")]
    async fn jump_to(
        &self,
        Parameters(p): Parameters<crate::sections::JumpToParams>,
    ) -> CallToolResult {
        self.run(&JUMP_TO, p, crate::sections::jump_to_body).await
    }

    /// Put audio into the song: a sample from your folders or Live's browser
    /// becomes a clip in a section, or lands in the Arrangement at a bar.
    /// `sample` is plain words to search for ("break 90", "vinyl crackle"), an
    /// absolute file path, or a browser URI. Give `section` (a Session row, so
    /// it plays with the song) or `at_bar` (Live's 1-based bar), not both.
    /// `track` is an audio track by name or index; leave it out and an audio
    /// track is made, named after the sample. By default the clip is fitted to
    /// the song: warping on so it follows the tempo, the loop set to a whole
    /// number of bars when Live heard one, named after the file — one undo step
    /// in Live. `bars` forces a loop length, `transpose` shifts it in
    /// semitones, `fit: false` places it exactly as dragging the file in would.
    /// The file is referenced where it is: nothing is copied, moved or
    /// uploaded. Needs Live 12 (Live's own API for placing audio from a file).
    /// Refuses a MIDI track, both targets at once, a section that does not
    /// exist, and a browser sample asked for at a bar — Live only reveals a
    /// browser item's file once it is a clip, so put that one in a section
    /// first. Find samples with search_browser category "samples"; add a
    /// folder with adv_sample_folders.
    #[tool(name = "add_sample")]
    async fn add_sample(
        &self,
        Parameters(p): Parameters<crate::samples::AddSampleParams>,
    ) -> CallToolResult {
        self.run(&ADD_SAMPLE, p, crate::samples::add_sample_body)
            .await
    }

    /// The folders Claude looks in for samples. action "list" (default) shows
    /// them with their file counts; "add" with a path adds one and indexes it;
    /// "remove" takes one out; "refresh" walks them again. Live's own folders —
    /// the Core Library, your Packs, the User Library, this project — are
    /// always included and need no adding. The list is kept in
    /// ~/.ableton-music-maker/sample_folders.json and holds paths only, never
    /// audio.
    #[tool(name = "sample_folders")]
    async fn sample_folders(
        &self,
        Parameters(p): Parameters<crate::samples::SampleFoldersParams>,
    ) -> CallToolResult {
        self.run(&SAMPLE_FOLDERS, p, crate::samples::sample_folders_body)
            .await
    }
}

#[tool_handler(router = self.tool_router, name = "AbletonMusicMaker")]
impl ServerHandler for Server {
    /// What every client receives at `initialize`: the tools, and the
    /// instructions that teach the model the workflow before its first call.
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "AbletonMusicMaker",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(crate::context::INSTRUCTIONS.to_string())
    }

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

    /// Every command name the script serves, read from its dispatch the same
    /// way the script reads it back from itself.
    fn commands_the_embedded_script_dispatches() -> std::collections::BTreeSet<String> {
        let src = crate::REMOTE_SCRIPT_SOURCE;
        let mut found = std::collections::BTreeSet::new();
        // `command_type == "name"` — the dispatch chain and the two handlers
        // that answer ahead of it.
        for rest in src.split("command_type == \"").skip(1) {
            if let Some(name) = rest.split('"').next() {
                found.insert(name.to_string());
            }
        }
        // `command_type in ("a", "b")` — subscribe and unsubscribe.
        for rest in src.split("command_type in (").skip(1) {
            let Some(group) = rest.split(')').next() else {
                continue;
            };
            for name in group.split('"').skip(1).step_by(2) {
                found.insert(name.to_string());
            }
        }
        // The script's own derivation is written with escaped regexes, so the
        // literal forms above never match it; if that ever changed this set
        // would gain junk and the equality below would catch it.
        found
    }

    /// The command list has one home: `ALL_REMOTE_COMMANDS`. The Remote
    /// Script declares nothing by hand — it reads its own dispatch back at
    /// import — so the only way the two can disagree is a handler added
    /// without a name here, or a name here without a handler. Both fail the
    /// build, in both directions, against the script the binary embeds.
    #[test]
    fn the_servers_command_list_and_the_scripts_dispatch_are_the_same_set() {
        let dispatched = commands_the_embedded_script_dispatches();
        let served: std::collections::BTreeSet<String> = ALL_REMOTE_COMMANDS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let missing_handler: Vec<_> = served.difference(&dispatched).collect();
        let unreachable: Vec<_> = dispatched.difference(&served).collect();
        assert!(
            missing_handler.is_empty(),
            "the server asks for commands the Remote Script has no handler for: {missing_handler:?}"
        );
        assert!(
            unreachable.is_empty(),
            "the Remote Script handles commands the server never asks for; delete them or add them to ALL_REMOTE_COMMANDS: {unreachable:?}"
        );
    }

    /// The script derives its capability list; it must never go back to a
    /// typed one, which is what drifted before.
    #[test]
    fn the_remote_script_declares_no_hand_written_capability_list() {
        let src = crate::REMOTE_SCRIPT_SOURCE;
        assert!(
            !src.contains("SCRIPT_CAPABILITIES = ["),
            "the Remote Script has a hand-typed capability list again; derive it from the dispatch"
        );
        assert!(
            src.contains("SCRIPT_CAPABILITIES = _served_commands()"),
            "the Remote Script no longer derives its capability list"
        );
    }

    /// Clients show the model the input schema as one document; a `$ref`
    /// into `$defs` hides the nested type (notes, cue steps, song tracks)
    /// from it, so every nested type is `#[schemars(inline)]`.
    #[test]
    fn no_tool_schema_hides_a_type_behind_a_ref() {
        for tool in Server::tool_router().list_all() {
            let schema = serde_json::to_string(&tool.input_schema).unwrap();
            assert!(
                !schema.contains("\"$ref\""),
                "{} has a $ref: {schema}",
                tool.name
            );
        }
    }

    #[test]
    fn tool_count_and_schema_defaults() {
        let router = Server::tool_router();
        let tools = router.list_all();
        assert_eq!(tools.len(), 109);
        let create_clip = tools.iter().find(|t| t.name == "create_clip").unwrap();
        let schema = serde_json::to_value(&create_clip.input_schema).unwrap();
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        // The track and the clip are a name or an index, so neither index is
        // required any more; `length` has always had a default.
        let properties = schema["properties"].as_object().unwrap();
        for key in ["track", "track_index", "clip", "clip_index"] {
            assert!(properties.contains_key(key), "create_clip has no {key}");
        }
        for key in ["length", "user_prompt", "track_index", "clip_index"] {
            assert!(!required.iter().any(|r| r == key), "{key} is required");
        }
    }

    #[test]
    fn recording_a_take_is_a_parameter_not_a_tool() {
        // Decision 0006: a new capability joins the artist tool that owns the
        // intent. Keeping a performance is part of playing one.
        let router = Server::tool_router();
        let tools = router.list_all();
        assert_eq!(tools.len(), 109, "the take must not add a tool");
        // start_performance is served as adv_start_performance (decision 0006).
        for name in ["adv_start_performance", "play_song"] {
            let tool = tools.iter().find(|t| t.name == name).unwrap();
            let schema = serde_json::to_value(&tool.input_schema).unwrap();
            assert!(
                schema["properties"].get("record").is_some(),
                "{name} has no record parameter"
            );
            assert!(
                !schema["required"]
                    .as_array()
                    .map(|r| r.iter().any(|x| x == "record"))
                    .unwrap_or(false),
                "{name}: record has a default"
            );
        }
    }
}
