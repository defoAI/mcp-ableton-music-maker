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

/// A beat position without a pointless ".0".
fn beat(v: Option<&Value>) -> String {
    match v.and_then(Value::as_f64) {
        Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => "?".to_string(),
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
fn track_kind() -> String {
    "track".to_string()
}
fn quarter_beat() -> f64 {
    0.25
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
pub struct AutomationPoint {
    pub time: f64,
    pub value: f64,
}

/// A ramp in one line: from `from` at `start` to `to` at `start + over`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
params!(SetTrackMixerParams {
    /// Index of the track (among the song's tracks, or among the return
    /// tracks when kind is "return"; ignored for "master")
    track_index: i64,
    /// "track" (default), "return" or "master"
    kind: String = "track_kind",
    /// Fader level as Live's mixer parameter, 0.0-1.0; 0.85 is 0 dB
    volume: Option<f64>,
    /// Pan, -1.0 (left) to 1.0 (right)
    pan: Option<f64>,
    mute: Option<bool>,
    solo: Option<bool>,
    /// Record arm (MIDI and audio tracks only)
    arm: Option<bool>,
});
params!(SetSendParams {
    /// Index of the track whose send changes
    track_index: i64,
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
    /// Track that holds the Drum Rack
    track_index: i64,
    /// Which device, if the track has more than one Drum Rack (default: the first)
    device_index: Option<i64>,
});
params!(DeleteArrangementClipParams {
    /// Track that owns the Arrangement clip
    track_index: i64,
    /// Position in the track's Arrangement clips, ordered by start time —
    /// the index get_arrangement_clips lists
    clip_index: i64,
});
params!(DeleteLocatorParams {
    /// The locator's exact name
    name: String = "String::new",
    /// Or its beat position
    time: Option<f64>,
});
params!(SearchBrowserParams {
    /// Words that must all appear in the item's name or folder path, e.g. "analog bass" or "techno kit"
    query: String,
    /// "all" (instruments, sounds, drums, audio and MIDI effects) or one of: instruments, sounds, drums, audio_effects, midi_effects, samples, packs, user_library
    category: String = "all_categories",
    /// Most hits to return (default 30, max 200)
    limit: i64 = "thirty",
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
pub struct BatchStep {
    /// Any Live-facing tool name, e.g. "create_clip"
    pub tool: String,
    /// That tool's arguments. Inside integer fields the string "$last_track"
    /// becomes the index of the most recent track created in this batch.
    #[serde(default)]
    pub args: Value,
}
params!(BatchParams {
    /// Steps, run in order on one connection to Live
    steps: Vec<BatchStep>,
    /// Stop at the first failing step (default true); with false, later steps still run
    stop_on_error: bool = "yes",
});

/// A track in a build_song document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Fader 0.0-1.0 (0.85 = 0 dB)
    #[serde(default)]
    pub volume: Option<f64>,
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
pub struct SongClip {
    /// A track name from `tracks`, or an existing track's index as a string ("2")
    pub track: String,
    /// Session slot (default 0)
    #[serde(default)]
    pub slot: i64,
    /// Clip name
    #[serde(default)]
    pub name: String,
    /// Length in beats (default 4)
    #[serde(default = "four")]
    pub length: f64,
    #[serde(flatten)]
    pub notes: NotesInput,
}

/// Where a clip goes in the Arrangement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
pub struct SongLocator {
    pub name: String,
    pub time: f64,
}

params!(BuildSongParams {
    /// Tempo in BPM (optional)
    tempo: Option<f64>,
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
    /// Validate and describe the plan without touching Live (default false)
    dry_run: bool = "bool::default",
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
pub const SET_TRACK_MIXER: ToolSpec = ToolSpec::new("set_track_mixer");
pub const SET_SEND: ToolSpec = ToolSpec::new("set_send");
pub const GET_RETURNS: ToolSpec = ToolSpec::new("get_returns");
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
    match r.get("loaded_device").filter(|d| d.is_object()) {
        Some(dev) => Ok(format!(
            "Loaded '{}' as device {} on track {} ('{}'). Devices on the track now: {}",
            get_display(dev, "name", &get_display(&r, "item_name", "device")),
            get_display(dev, "index", "?"),
            p.track_index,
            get_display(&r, "track_name", "track"),
            join_names(r.get("devices_after"))
        )),
        None => Ok(format!(
            "Loaded '{}' on track {} ('{}'). Devices on the track now: {}",
            get_display(&r, "item_name", &p.uri),
            p.track_index,
            get_display(&r, "track_name", "track"),
            join_names(r.get("devices_after"))
        )),
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
    let was = r
        .get("previous_song_time")
        .map(|v| format!(" (was at beat {})", display(v)))
        .unwrap_or_default();
    Ok(format!(
        "Playhead moved to beat {}{was}",
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

// ── Mixer, colours, drum pads, deletion ─────────────────────────────────────

fn mixer_summary(r: &Value) -> String {
    let mut parts = vec![
        format!("volume {}", get_display(r, "volume", "?")),
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
    if p.volume.is_none()
        && p.pan.is_none()
        && p.mute.is_none()
        && p.solo.is_none()
        && p.arm.is_none()
    {
        return Err("Nothing to set: give volume, pan, mute, solo or arm.".into());
    }
    let r = live
        .send_command(
            "set_track_mixer",
            Some(json!({
                "track_index": p.track_index, "kind": p.kind,
                "volume": p.volume, "pan": p.pan, "mute": p.mute, "solo": p.solo, "arm": p.arm,
            })),
        )
        .map_err(|e| live_err("set the track mixer", e))?;
    Ok(format!(
        "'{}' now: {}",
        get_display(&r, "name", &format!("track {}", p.track_index)),
        mixer_summary(&r)
    ))
}

pub fn set_send_body(live: &LiveState, p: &SetSendParams) -> ToolResult {
    require(live, "set_send")?;
    if p.send_name.is_empty() && p.send_index.is_none() {
        return Err("Say which send: send_name (the return track's name or letter) or send_index. get_returns lists them.".into());
    }
    let r = live
        .send_command(
            "set_send",
            Some(json!({
                "track_index": p.track_index, "kind": p.kind,
                "send_name": if p.send_name.is_empty() { Value::Null } else { json!(p.send_name) },
                "send_index": p.send_index, "value": p.value,
            })),
        )
        .map_err(|e| live_err("set the send", e))?;
    Ok(format!(
        "'{}' send {} ({}) set to {}",
        get_display(&r, "track", &format!("track {}", p.track_index)),
        get_display(&r, "send_index", "?"),
        get_display(&r, "return_name", "return"),
        get_display(&r, "value", &p.value.to_string())
    ))
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
    let r = live
        .send_command(
            "get_drum_rack_pads",
            Some(
                json!({"track_index": p.track_index, "device_index": p.device_index.unwrap_or(-1)}),
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

pub fn search_browser_body(live: &LiveState, p: &SearchBrowserParams) -> ToolResult {
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

fn meters_text(r: &Value) -> String {
    let mut out = String::new();
    for (label, key) in [("tracks", "tracks"), ("returns", "returns")] {
        if let Some(list) = r.get(key).and_then(Value::as_array) {
            if list.is_empty() {
                continue;
            }
            out.push_str(&format!("{label}:\n"));
            for t in list {
                out.push_str(&format!(
                    "  {} '{}': {:.2}{}\n",
                    get_display(t, "index", "?"),
                    get_display(t, "name", "?"),
                    meter_peak(t),
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
        out.push_str(&format!("master: {:.2}\n", meter_peak(m)));
    }
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
    Ok(format!(
        "Levels right now ({}; 0.0 silent, 1.0 clipping):\n{}",
        if playing {
            "playing"
        } else {
            "stopped — levels are only meaningful while playing; use play_and_measure"
        },
        meters_text(&r)
    ))
}

/// Play a stretch of the set, sample the meters while it plays, and report
/// each track's peak. Nothing in the set changes.
pub fn play_and_measure_body(live: &LiveState, p: &PlayAndMeasureParams) -> ToolResult {
    require(live, "get_track_meters")?;
    require(live, "start_playback")?;
    require(live, "stop_playback")?;
    let seconds = p.seconds.clamp(0.5, 10.0);
    let interval = std::time::Duration::from_millis(p.interval_ms.clamp(50, 1000) as u64);
    if let Some(t) = p.start_time {
        require(live, "set_current_song_time")?;
        live.send_command("set_current_song_time", Some(json!({"time": t})))
            .map_err(|e| live_err("move the playhead", e))?;
    }
    live.send_command("start_playback", None)
        .map_err(|e| live_err("start playback", e))?;
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
        "Played {seconds:.1} s{} and took {readings} readings. Peak per track (0.0 silent, 1.0 clipping):\n",
        p.start_time.map(|t| format!(" from beat {t}")).unwrap_or_default()
    );
    let mut silent = Vec::new();
    for (id, (name, peak)) in &peaks {
        let id = id.trim_start_matches("zz ");
        out.push_str(&format!("  {id} '{name}': {peak:.2}\n"));
        if *peak < 0.01 && id.starts_with("track") {
            silent.push(name.clone());
        }
    }
    if !silent.is_empty() {
        out.push_str(&format!(
            "Silent during this stretch: {}.",
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

/// Run one tool by name with JSON arguments — the batch and build_song
/// dispatcher. Every Live-facing tool is here; batch itself is not.
pub fn run_named(live: &LiveState, name: &str, args: Value) -> ToolResult {
    let args = if args.is_null() { json!({}) } else { args };
    named_tools!(live, name, args;
        "get_session_info" => (Empty, get_session_info_body),
        "get_remote_script_info" => (Empty, get_remote_script_info_body),
        "get_track_info" => (TrackParams, get_track_info_body),
        "get_clip_notes" => (ClipParams, get_clip_notes_body),
        "get_device_parameters" => (DeviceParams, get_device_parameters_body),
        "set_device_parameter" => (SetDeviceParameterParams, set_device_parameter_body),
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
        "fire_clip" => (ClipParams, fire_clip_body),
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
    )
}

/// "Created MIDI track 6 (…)" → 6.
fn track_index_from_text(text: &str) -> Option<i64> {
    let rest = text.strip_prefix("Created ")?;
    let after = rest.split(" track ").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn substitute(v: &mut Value, last_track: Option<i64>) -> Result<(), String> {
    match v {
        Value::String(s) if s == "$last_track" => match last_track {
            Some(i) => *v = json!(i),
            None => {
                return Err("$last_track used before any track was created in this batch".into())
            }
        },
        Value::Array(items) => {
            for item in items {
                substitute(item, last_track)?;
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                substitute(item, last_track)?;
            }
        }
        _ => {}
    }
    Ok(())
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
    let mut out = String::new();
    let mut last_track: Option<i64> = None;
    let mut failed = 0;
    for (i, step) in p.steps.iter().enumerate() {
        let mut args = step.args.clone();
        let outcome =
            substitute(&mut args, last_track).and_then(|_| run_named(live, &step.tool, args));
        match outcome {
            Ok(text) => {
                if let Some(idx) = track_index_from_text(&text) {
                    last_track = Some(idx);
                }
                out.push_str(&format!(
                    "{}. {} ✓ {}\n",
                    i + 1,
                    step.tool,
                    text.lines().next().unwrap_or("")
                ));
            }
            Err(e) => {
                failed += 1;
                out.push_str(&format!("{}. {} ✗ {}\n", i + 1, step.tool, e));
                if p.stop_on_error {
                    let left = p.steps.len() - i - 1;
                    if left > 0 {
                        out.push_str(&format!("Stopped; {left} step(s) not run.\n"));
                    }
                    return Err(out);
                }
            }
        }
    }
    if failed > 0 {
        out.push_str(&format!("{failed} step(s) failed."));
        return Err(out);
    }
    Ok(out)
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

pub fn build_song_body(live: &LiveState, p: &BuildSongParams) -> ToolResult {
    // ── validate everything before the first command ──
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
        if defined_clips.contains(&(c.track.clone(), c.slot)) {
            return Err(format!(
                "two clips are defined for track '{}' slot {}",
                c.track, c.slot
            ));
        }
        defined_clips.push((c.track.clone(), c.slot));
        planned_notes.push(notes);
    }
    let mut placements: Vec<(usize, Vec<f64>)> = Vec::new();
    for (i, pl) in p.placements.iter().enumerate() {
        if !defined_clips.contains(&(pl.track.clone(), pl.slot)) {
            return Err(format!(
                "placement {}: no clip is defined for track '{}' slot {}",
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
    let fail = |done: &str, what: String| -> String { format!("{done}Stopped: {what}") };
    if let Some(t) = p.tempo {
        set_tempo_body(live, &SetTempoParams { tempo: t }).map_err(|e| fail(&done, e))?;
        done.push_str(&format!("Tempo {t}.\n"));
    }
    let mut created: BTreeMap<String, i64> = BTreeMap::new();
    for t in &p.tracks {
        let (index, _) = create_track(live, &t.kind, -1).map_err(|e| fail(&done, e))?;
        let index = index.ok_or_else(|| {
            fail(
                &done,
                format!(
                    "Live did not report the index of the new track '{}'",
                    t.name
                ),
            )
        })?;
        set_track_name_body(
            live,
            &SetTrackNameParams {
                track_index: index,
                name: t.name.clone(),
            },
        )
        .map_err(|e| fail(&done, e))?;
        created.insert(t.name.clone(), index);
        let mut line = format!("Track {index} '{}'", t.name);
        let uri = match (&t.instrument, &t.instrument_query) {
            (Some(u), _) => Some(u.clone()),
            (None, Some(q)) => {
                require(live, "search_browser").map_err(|e| fail(&done, e))?;
                let r = live
                    .send_command(
                        "search_browser",
                        Some(json!({"query": q, "category": "all", "limit": 1})),
                    )
                    .map_err(|e| fail(&done, live_err("search the browser", e)))?;
                let hit = r["items"]
                    .get(0)
                    .and_then(|i| i.get("uri"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                match hit {
                    Some(u) => {
                        line.push_str(&format!(
                            ", found '{}' for \"{q}\"",
                            get_display(&r["items"][0], "name", "?")
                        ));
                        Some(u)
                    }
                    None => {
                        return Err(fail(
                            &done,
                            format!("track '{}': nothing in the browser matches \"{q}\"", t.name),
                        ))
                    }
                }
            }
            (None, None) => None,
        };
        if let Some(uri) = uri {
            let text = load_instrument_or_effect_body(
                live,
                &LoadInstrumentParams {
                    track_index: index,
                    uri,
                },
            )
            .map_err(|e| fail(&done, e))?;
            line.push_str(&format!(
                ", {}",
                text.split('.').next().unwrap_or("").to_lowercase()
            ));
        }
        if t.volume.is_some() || t.pan.is_some() {
            set_track_mixer_body(
                live,
                &SetTrackMixerParams {
                    track_index: index,
                    kind: "track".into(),
                    volume: t.volume,
                    pan: t.pan,
                    mute: None,
                    solo: None,
                    arm: None,
                },
            )
            .map_err(|e| fail(&done, e))?;
        }
        for (send, value) in &t.sends {
            set_send_body(
                live,
                &SetSendParams {
                    track_index: index,
                    kind: "track".into(),
                    send_name: send.clone(),
                    send_index: None,
                    value: *value,
                },
            )
            .map_err(|e| fail(&done, e))?;
        }
        if let Some(c) = t.color_index {
            set_color_body(
                live,
                &SetColorParams {
                    track_index: index,
                    color_index: c,
                    clip_index: None,
                    arrangement: false,
                    kind: "track".into(),
                },
            )
            .map_err(|e| fail(&done, e))?;
        }
        done.push_str(&line);
        done.push('\n');
    }
    for (c, notes) in p.clips.iter().zip(planned_notes.iter()) {
        let track_index = song_track_index(&c.track, &created).map_err(|e| fail(&done, e))?;
        let params = CreateClipParams {
            track_index,
            clip_index: c.slot,
            length: c.length,
            name: c.name.clone(),
            input: NotesInput {
                notes: notes.clone(),
                ..Default::default()
            },
        };
        create_clip_body(live, &params).map_err(|e| fail(&done, e))?;
        done.push_str(&format!(
            "Clip '{}' on track {track_index} slot {} ({} notes).\n",
            c.name,
            c.slot,
            notes.len()
        ));
    }
    for (i, times) in &placements {
        let pl = &p.placements[*i];
        let track_index = song_track_index(&pl.track, &created).map_err(|e| fail(&done, e))?;
        let params = DuplicateToArrangementParams {
            track_index,
            clip_index: pl.slot,
            destination_time: None,
            destination_times: times.clone(),
            start: None,
            end: None,
            step: None,
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
                time: l.time,
            },
        )
        .map_err(|e| fail(&done, e))?;
        done.push_str(&format!("Locator '{}' at beat {}.\n", l.name, l.time));
    }
    done.push_str(
        "Done. switch_to_arrangement_view to see it; play_and_measure to hear the balance.",
    );
    Ok(done)
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

    /// Set a track's volume, pan, mute, solo or arm — any mix, in one call.
    /// Volume is Live's mixer parameter 0.0-1.0 where 0.85 is 0 dB. Works
    /// on return tracks (kind "return") and the master (kind "master",
    /// volume and pan only). The result reports the track's state after.
    #[tool(name = "set_track_mixer")]
    async fn set_track_mixer(
        &self,
        Parameters(p): Parameters<SetTrackMixerParams>,
    ) -> CallToolResult {
        self.run(&SET_TRACK_MIXER, p, set_track_mixer_body).await
    }

    /// Route a track to a return track: set one send level (0.0-1.0) by
    /// the return's name ("Reverb"), its letter ("A") or its index. Call
    /// get_returns first to see what returns exist.
    #[tool(name = "set_send")]
    async fn set_send(&self, Parameters(p): Parameters<SetSendParams>) -> CallToolResult {
        self.run(&SET_SEND, p, set_send_body).await
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

    /// Remove a clip from the Arrangement timeline. clip_index is the
    /// position get_arrangement_clips lists (ordered by start time); later
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

    /// One reading of every track's output meter (0.0 silent to 1.0
    /// clipping), plus returns and master. Only meaningful while Live plays;
    /// for a proper reading use play_and_measure.
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

    /// Write automation into a clip: a filter sweep, a volume fade, a send
    /// ride. Target a device parameter (device_index + parameter_index from
    /// get_device_parameters) or a mixer control (volume, pan, send). Give
    /// points [{time, value}] in clip beats, or one ramp {from, to, over}.
    /// Values are in the parameter's own range. Works on Session clips and on
    /// Arrangement clips, which is how Live stores arrangement automation.
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

    /// Run several tool calls in one round-trip: an ordered list of
    /// {tool, args}. Stops at the first failure (unless stop_on_error is
    /// false) and reports each step. Inside args, "$last_track" stands for
    /// the index of the most recently created track, so "create a track,
    /// name it, load a sound, fill a clip" is one call.
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

    #[tool(name = "batch")]
    async fn batch(&self, Parameters(p): Parameters<BatchParams>) -> CallToolResult {
        self.run(&BATCH, p, batch_body).await
    }

    /// Build a whole arrangement from one document: tracks (with an
    /// instrument by URI or search words, level, pan, sends, colour), Session
    /// clips with their notes in any compact form, Arrangement placements
    /// (times or start/end/step) and locators. Everything is validated before
    /// the first command reaches Live; dry_run: true shows the plan only.
    /// Stops at the first failure and says what was built.
    #[tool(name = "build_song")]
    async fn build_song(&self, Parameters(p): Parameters<BuildSongParams>) -> CallToolResult {
        self.run(&BUILD_SONG, p, build_song_body).await
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
        assert_eq!(tools.len(), 49);
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
