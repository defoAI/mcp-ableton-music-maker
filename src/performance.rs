//! Live performance: the state the Remote Script reports, bar arithmetic,
//! cue resolution (bars → beats, names → indices, the silence check) and the
//! text the producer reads. Pure logic: nothing here talks to Live, so every
//! rule is unit-tested on synthetic states. The tool bodies in `tools.rs`
//! send the commands.
//!
//! The one principle: Claude plans, Live executes. A cue is resolved here to
//! absolute beats and handed to the script, which runs it on its own clock.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

fn minus_one() -> i64 {
    -1
}
fn four_i() -> i64 {
    4
}
fn one_i() -> i64 {
    1
}
fn hundred_twenty() -> f64 {
    120.0
}

/// One track as `get_performance_state` reports it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TrackState {
    #[serde(default)]
    pub index: i64,
    #[serde(default)]
    pub name: String,
    /// -1 nothing playing, -2 the Clip Stop slot fired (Live's own encoding)
    #[serde(default = "minus_one")]
    pub playing_slot_index: i64,
    /// The blinking slot; -1 none, -2 the Clip Stop button
    #[serde(default = "minus_one")]
    pub fired_slot_index: i64,
    #[serde(default)]
    pub arm: bool,
    #[serde(default)]
    pub is_recording: bool,
    #[serde(default)]
    pub playing_clip_name: Option<String>,
    #[serde(default)]
    pub fired_clip_name: Option<String>,
    #[serde(default)]
    pub slots_with_clips: Vec<i64>,
    /// Empty slots whose stop button was removed: a scene launch leaves the
    /// track playing through these rows
    #[serde(default)]
    pub no_stop_slots: Vec<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SceneState {
    #[serde(default)]
    pub index: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tempo: Option<f64>,
    #[serde(default)]
    pub is_triggered: bool,
    #[serde(default)]
    pub is_playing: bool,
    /// Tracks that have a clip in this scene's row
    #[serde(default)]
    pub clip_tracks: Vec<i64>,
    #[serde(default)]
    pub phrase_bars: Option<i64>,
    #[serde(default)]
    pub phrase_default: bool,
    #[serde(default)]
    pub started_bar: Option<i64>,
    /// The section name without the phrase suffix (the script parses it)
    #[serde(default)]
    pub section: Option<String>,
}

/// The loudest a track got, in dB from Live's 0–1 output meter.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TrackLevel {
    #[serde(default)]
    pub index: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub peak_db: f64,
}

/// The meter peaks the script's tick keeps: this bar (and the last one),
/// per track, and the master's peak while each scene row played.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Levels {
    #[serde(default)]
    pub bar: i64,
    #[serde(default)]
    pub master_peak_db: f64,
    #[serde(default)]
    pub tracks: Vec<TrackLevel>,
    /// scene index (as a string) → the master's peak while that row played
    #[serde(default)]
    pub section_peaks: std::collections::BTreeMap<String, f64>,
}

impl Levels {
    pub fn section_peak(&self, scene_index: i64) -> Option<f64> {
        self.section_peaks.get(&scene_index.to_string()).copied()
    }
}

/// The phrase the set is in: `bars` per phrase from the bar the current
/// scene started on; `ends_bar` is where the next phrase begins.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Phrase {
    #[serde(default)]
    pub scene_index: i64,
    #[serde(default)]
    pub started_bar: i64,
    #[serde(default = "sixteen")]
    pub bars: i64,
    #[serde(default)]
    pub ends_bar: i64,
    #[serde(default)]
    pub default: bool,
}

fn sixteen() -> i64 {
    16
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CueStepState {
    #[serde(default)]
    pub index: i64,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub beat: f64,
    #[serde(default)]
    pub bar: Option<f64>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub end_beat: Option<f64>,
    #[serde(default)]
    pub to: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CueState {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub steps: Vec<CueStepState>,
    #[serde(default)]
    pub pending: i64,
}

/// Everything `get_performance_state` returns.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PerfState {
    #[serde(default)]
    pub is_playing: bool,
    #[serde(default = "hundred_twenty")]
    pub tempo: f64,
    #[serde(default = "four_i")]
    pub signature_numerator: i64,
    #[serde(default = "four_i")]
    pub signature_denominator: i64,
    #[serde(default)]
    pub beat: f64,
    #[serde(default = "one_i")]
    pub bar: i64,
    #[serde(default = "one_i")]
    pub beat_in_bar: i64,
    /// Song.clip_trigger_quantization: 0 none, 1 8 bars … 4 1 bar, 5 1/2 … 13 1/32
    #[serde(default = "four_i")]
    pub clip_trigger_quantization: i64,
    #[serde(default)]
    pub clip_trigger_quantization_name: Option<String>,
    #[serde(default)]
    pub scale_mode: Option<bool>,
    #[serde(default)]
    pub scale_name: Option<String>,
    #[serde(default)]
    pub root_note: Option<i64>,
    #[serde(default)]
    pub root_note_name: Option<String>,
    #[serde(default)]
    pub tracks: Vec<TrackState>,
    #[serde(default)]
    pub scenes: Vec<SceneState>,
    #[serde(default)]
    pub cues: Vec<CueState>,
    #[serde(default)]
    pub pending_record: Option<Value>,
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub performance_mode: bool,
    #[serde(default)]
    pub phrase: Option<Phrase>,
    #[serde(default)]
    pub current_scene: Option<i64>,
    #[serde(default)]
    pub levels: Option<Levels>,
}

pub const PITCH_CLASSES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The global launch quantization value that means "1 bar".
pub const ONE_BAR: i64 = 4;

impl PerfState {
    pub fn from_value(v: &Value) -> Result<Self, String> {
        serde_json::from_value(v.clone())
            .map_err(|e| format!("Could not read the performance state Live sent: {e}"))
    }

    pub fn beats_per_bar(&self) -> f64 {
        (self.signature_numerator.max(1)) as f64
    }

    /// The beat a bar (Live's 1-based numbering) starts on.
    pub fn bar_start(&self, bar: f64) -> f64 {
        (bar - 1.0) * self.beats_per_bar()
    }

    /// The next bar after the current position.
    pub fn next_bar(&self) -> i64 {
        self.bar + 1
    }

    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.tempo.max(1.0)
    }

    pub fn seconds_to_next_bar(&self) -> f64 {
        let next = self.bar_start(self.next_bar() as f64);
        ((next - self.beat).max(0.0)) * self.seconds_per_beat()
    }

    /// "14.3": bar and beat, Live's own numbering.
    pub fn position(&self) -> String {
        format!("{}.{}", self.bar, self.beat_in_bar)
    }

    pub fn quantization_name(&self) -> String {
        self.clip_trigger_quantization_name
            .clone()
            .unwrap_or_else(|| quantization_display(self.clip_trigger_quantization))
    }

    /// "F minor", from Live 12's scale settings when the set has them.
    pub fn key_from_set(&self) -> Option<String> {
        match (&self.root_note_name, &self.scale_name) {
            (Some(root), Some(scale)) if !root.is_empty() && !scale.is_empty() => {
                Some(format!("{root} {scale}"))
            }
            _ => None,
        }
    }

    pub fn track_by(&self, which: &Value) -> Result<&TrackState, String> {
        match which {
            Value::Number(n) => {
                let i = n.as_i64().unwrap_or(-1);
                self.tracks
                    .iter()
                    .find(|t| t.index == i)
                    .ok_or_else(|| format!("no track {i} (the set has {})", self.tracks.len()))
            }
            Value::String(s) => {
                let want = s.trim().to_lowercase();
                if let Ok(i) = want.parse::<i64>() {
                    return self.track_by(&json!(i));
                }
                self.tracks
                    .iter()
                    .find(|t| t.name.to_lowercase() == want)
                    .ok_or_else(|| {
                        format!(
                            "no track named '{s}'; tracks: {}",
                            self.tracks
                                .iter()
                                .map(|t| t.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
            }
            other => Err(format!("a track is a name or an index, not {other}")),
        }
    }

    pub fn scene_by(&self, which: &Value) -> Result<&SceneState, String> {
        match which {
            Value::Number(n) => {
                let i = n.as_i64().unwrap_or(-1);
                self.scenes
                    .iter()
                    .find(|s| s.index == i)
                    .ok_or_else(|| format!("no scene {i} (the set has {})", self.scenes.len()))
            }
            Value::String(s) => {
                let want = s.trim().to_lowercase();
                if let Ok(i) = want.parse::<i64>() {
                    return self.scene_by(&json!(i));
                }
                self.scenes
                    .iter()
                    .find(|sc| sc.name.to_lowercase() == want)
                    .ok_or_else(|| {
                        format!(
                            "no scene named '{s}'; scenes: {}",
                            self.scenes
                                .iter()
                                .map(|sc| sc.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
            }
            other => Err(format!("a scene is a name or an index, not {other}")),
        }
    }

    /// The bar a launch issued now lands on, given the global quantization.
    pub fn launch_lands_on(&self) -> String {
        match self.clip_trigger_quantization {
            0 => "immediately (launch quantization is none)".to_string(),
            1..=4 => {
                let bars = match self.clip_trigger_quantization {
                    1 => 8,
                    2 => 4,
                    3 => 2,
                    _ => 1,
                };
                let next = ((self.bar - 1) / bars + 1) * bars + 1;
                format!(
                    "at bar {next} ({}-bar quantization; it is bar {})",
                    bars,
                    self.position()
                )
            }
            _ => format!(
                "on the next {} (it is bar {})",
                self.quantization_name(),
                self.position()
            ),
        }
    }
}

/// "D minor" → (2, "Minor"); "F#" → (6, "Major"); "Bb dorian" → (10, "Dorian").
/// None when the root is not a note name.
pub fn parse_key(key: &str) -> Option<(i64, String)> {
    let mut parts = key.split_whitespace();
    let root_text = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    let (root_name, tail) = split_root(root_text)?;
    let scale_text = if rest.is_empty() {
        tail.to_string()
    } else {
        rest.join(" ")
    };
    let scale = match scale_text.trim().to_lowercase().as_str() {
        "" | "maj" | "major" => "Major".to_string(),
        "m" | "min" | "minor" | "aeolian" | "natural minor" => "Minor".to_string(),
        other => other
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    };
    Some((root_name, scale))
}

fn split_root(text: &str) -> Option<(i64, &str)> {
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let base: i64 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (offset, tail) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') if rest.len() > 1 || text.len() == 2 => (-1, &rest[1..]),
        _ => (0, rest),
    };
    Some(((base + offset).rem_euclid(12), tail))
}

pub fn quantization_display(v: i64) -> String {
    match v {
        0 => "none",
        1 => "8_bars",
        2 => "4_bars",
        3 => "2_bars",
        4 => "1_bar",
        5 => "1/2",
        6 => "1/2t",
        7 => "1/4",
        8 => "1/4t",
        9 => "1/8",
        10 => "1/8t",
        11 => "1/16",
        12 => "1/16t",
        13 => "1/32",
        _ => "?",
    }
    .to_string()
}

pub const GLOBAL_QUANTIZATIONS: &[&str] = &[
    "none", "8_bars", "4_bars", "2_bars", "1_bar", "1/2", "1/2t", "1/4", "1/4t", "1/8", "1/8t",
    "1/16", "1/16t", "1/32",
];

// ── Cue parameters ──────────────────────────────────────────────────────────

/// When a step happens: "next_bar", a bar number (Live's 1-based numbering,
/// `{"bar": 49}` or plain `49`), or `{"bars_after": 8}` counted from the next
/// bar.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[serde(untagged)]
pub enum CueTime {
    /// A beat inside a bar (needs a launch quantization finer than a bar)
    BarBeat { bar: f64, beat: f64 },
    /// A bar number
    Bar { bar: f64 },
    /// Bars after the next bar
    After { bars_after: f64 },
    /// Phrases after the current one (1 = the phrase after next_phrase)
    PhrasesAfter { phrases_after: f64 },
    /// "next_bar" or "next_phrase"
    Named(String),
    /// A bar number
    Number(f64),
}

/// A clip in a Session slot: the track by name or index, the slot index.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct ClipRef {
    /// Track name or index
    pub track: Value,
    /// Session slot index (scene row)
    pub clip: i64,
}

/// What a `set` step writes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SetSpec {
    /// "tempo", "crossfader", "volume", "mute", "send" or "device"
    pub target: String,
    /// Track name or index (volume, mute, send, device)
    #[serde(default)]
    pub track: Option<Value>,
    /// "track" (default), "return" or "master"
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub device_index: Option<i64>,
    #[serde(default)]
    pub parameter_index: Option<i64>,
    #[serde(default)]
    pub send_index: Option<i64>,
    /// The value: BPM for tempo, 0–1 for crossfader (A→B) and volume, 0/1 for mute, Live units for a device parameter
    pub value: f64,
}

/// One step of a cue: a time and exactly one action.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct CueStep {
    /// When (one-shot actions): "next_bar", {"bar": N}, {"bars_after": k} or N
    #[serde(default)]
    pub at: Option<CueTime>,
    /// When a ramp starts (same forms as `at`)
    #[serde(default)]
    pub from: Option<CueTime>,
    /// How many bars a ramp takes
    #[serde(default)]
    pub bars: Option<f64>,
    /// Fire a scene by name or index
    #[serde(default)]
    pub fire_scene: Option<Value>,
    #[serde(default)]
    pub fire_clip: Option<ClipRef>,
    #[serde(default)]
    pub stop_clip: Option<ClipRef>,
    /// Stop every clip (the Stop All Clips button)
    #[serde(default)]
    pub stop_all_clips: Option<bool>,
    /// Write one value at the beat
    #[serde(default)]
    pub set: Option<SetSpec>,
    /// A ramp over `bars`: {"tempo": 134}, {"crossfader": 1.0}, {"volume": 0.5, "track": "Bass"},
    /// {"send": 0.3, "track": "Pad", "send_index": 0}, or {"target": "device", "track": …, "device_index": …, "parameter_index": …, "to": …}
    #[serde(default)]
    pub ramp: Option<Value>,
    /// One live move, expanded into primitive steps: {"breakdown": {"keep": ["Pad"], "bars": 8}},
    /// {"drop": {"bars": 1, "keep": []}}, {"mute_except": {"keep": ["Kick"], "bars": 4}},
    /// {"sweep": {"track": "Pad", "device_index": 0, "parameter_index": 3, "from": 0.2, "to": 1.0, "bars": 8}},
    /// {"build": {"track": "Riser", "send": 0.8, "send_index": 0, "bars": 8, "hats": "Hats"}},
    /// {"panic": {"keep": ["Pad"], "bars": 1}}, {"restore_mix": {"snapshot": 1}}
    #[serde(default)]
    pub gesture: Option<Value>,
}

/// A cue: named steps the Remote Script runs on its own clock.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct CueParams {
    /// A name for the cue, shown in the state
    #[serde(default)]
    pub name: Option<String>,
    pub steps: Vec<CueStep>,
    /// Allow a plan that leaves a bar with nothing playing
    #[serde(default)]
    pub allow_silence: bool,
}

/// A cue resolved against the state: the wire steps for the script, one
/// display line per step, and the warnings.
#[derive(Debug, Clone, Default)]
pub struct ResolvedCue {
    pub steps: Vec<Value>,
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
}

/// Resolve a time to (bar, beat); a beat inside a bar gives a fractional bar.
pub fn resolve_time(state: &PerfState, t: &CueTime) -> Result<(f64, f64), String> {
    resolve_time_ex(state, t).map(|r| (r.0, r.1))
}

/// (bar, beat, sub_bar): `sub_bar` when the time names a beat inside a bar.
pub fn resolve_time_ex(state: &PerfState, t: &CueTime) -> Result<(f64, f64, bool), String> {
    let bpb = state.beats_per_bar();
    let phrase = |what: &str| -> Result<&Phrase, String> {
        state.phrase.as_ref().ok_or_else(|| {
            format!("\"{what}\" — no scene has been fired, so there is no phrase to count from. Use \"next_bar\" or {{\"bar\": N}}.")
        })
    };
    let bar = match t {
        CueTime::BarBeat { bar, beat } => {
            if *beat < 1.0 || *beat > bpb {
                return Err(format!("beat {beat} is outside 1–{}", bpb as i64));
            }
            if *bar < 1.0 {
                return Err(format!("bar {bar} is before bar 1"));
            }
            let b = state.bar_start(*bar) + (beat - 1.0);
            return Ok((bar + (beat - 1.0) / bpb, b, *beat != 1.0));
        }
        CueTime::Bar { bar } => *bar,
        CueTime::Number(n) => *n,
        CueTime::After { bars_after } => state.next_bar() as f64 + bars_after,
        CueTime::PhrasesAfter { phrases_after } => {
            let p = phrase("phrases_after")?;
            p.ends_bar as f64 + phrases_after * p.bars as f64
        }
        CueTime::Named(s) => match s.trim().to_lowercase().as_str() {
            "next_bar" | "next bar" | "next" => state.next_bar() as f64,
            "next_phrase" | "next phrase" => phrase("next_phrase")?.ends_bar as f64,
            "now" => return Ok((state.bar as f64, state.beat, false)),
            other => match other.parse::<f64>() {
                Ok(n) => n,
                Err(_) => return Err(format!("unknown time '{s}': use \"next_bar\", \"next_phrase\", {{\"bar\": N}}, {{\"bars_after\": k}} or {{\"phrases_after\": n}}")),
            },
        },
    };
    if bar < 1.0 {
        return Err(format!("bar {bar} is before bar 1"));
    }
    Ok((bar, state.bar_start(bar), false))
}

/// The clock line the Remote Script attaches to every response while a
/// performance runs, rendered for the end of a tool result.
pub fn clock_line(clock: &Value) -> String {
    clock_line_with(clock, None)
}

/// The clock line and, when the script sent meter peaks, the level line
/// under it. `plan_cue_id` is the song's plan cue: its next step reads as
/// "next jump" rather than "next cue".
pub fn clock_lines(clock: &Value, plan_cue_id: Option<i64>) -> String {
    let mut s = clock_line_with(clock, plan_cue_id);
    if let Some(levels) = clock.get("levels").and_then(level_line) {
        s.push('\n');
        s.push_str(&levels);
    }
    s
}

/// `🔊 master −4.0 dB peak this bar · Kick −6 · Bass −7`: Live's 0–1 output
/// meters as dB, the three loudest tracks named.
pub fn level_line(levels: &Value) -> Option<String> {
    let l: Levels = serde_json::from_value(levels.clone()).ok()?;
    let mut s = format!(
        "🔊 master {} dB peak this bar",
        crate::song::fmt_db(l.master_peak_db, 1)
    );
    let mut tracks: Vec<&TrackLevel> = l.tracks.iter().filter(|t| t.peak_db > -40.0).collect();
    tracks.sort_by(|a, b| b.peak_db.partial_cmp(&a.peak_db).unwrap());
    for t in tracks.iter().take(3) {
        s.push_str(&format!(
            " · {} {}",
            t.name,
            crate::song::fmt_db(t.peak_db, 0)
        ));
    }
    Some(s)
}

fn clock_line_with(clock: &Value, plan_cue_id: Option<i64>) -> String {
    let bar = clock.get("bar").and_then(Value::as_i64).unwrap_or(0);
    let bib = clock
        .get("beat_in_bar")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let playing = clock
        .get("is_playing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !playing {
        return format!("⏱ stopped at bar {bar}.{bib} (transport stopped outside this server)");
    }
    let mut s = format!(
        "⏱ bar {bar}.{bib} · next bar in {:.1} s",
        clock
            .get("seconds_to_next_bar")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    );
    if let Some(p) = clock.get("phrase").filter(|p| p.is_object()) {
        if let Some(e) = p.get("ends_bar").and_then(Value::as_i64) {
            s.push_str(&format!(" · phrase ends bar {e}"));
        }
    }
    if let Some(c) = clock.get("next_cue").filter(|c| c.is_object()) {
        let id = c.get("cue_id").and_then(Value::as_i64).unwrap_or(0);
        let label = c.get("label").and_then(Value::as_str).unwrap_or("");
        s.push_str(&format!(
            " · {}: bar {} {} (cue {id})",
            if plan_cue_id == Some(id) && label.starts_with("fire scene") {
                "next jump"
            } else {
                "next cue"
            },
            c.get("bar")
                .and_then(Value::as_f64)
                .map(fmt_bar)
                .unwrap_or_else(|| "?".into()),
            label,
        ));
    }
    s
}

/// The next `count` bars from `from`: every cue step and phrase boundary.
pub fn bar_map_text(state: &PerfState, from: i64, count: i64) -> String {
    let bpb = state.beats_per_bar();
    let to = from + count - 1;
    let mut lines: Vec<(f64, u8, String)> = Vec::new();
    if let Some(p) = &state.phrase {
        let mut b = p.ends_bar;
        let mut first = true;
        while b <= to {
            if b >= from {
                let name = state
                    .scenes
                    .iter()
                    .find(|s| s.index == p.scene_index)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                lines.push((
                    b as f64,
                    0,
                    if first {
                        format!(
                            "{b:<6} | phrase ends ({name}, {} bars from {}{})",
                            p.bars,
                            p.started_bar,
                            if p.default { ", default length" } else { "" }
                        )
                    } else {
                        format!("{b:<6} | phrase")
                    },
                ));
            }
            first = false;
            b += p.bars.max(1);
        }
    }
    for c in &state.cues {
        for s in &c.steps {
            if s.done {
                continue;
            }
            let bar = s.bar.unwrap_or(s.beat / bpb + 1.0);
            if bar < from as f64 || bar > to as f64 {
                continue;
            }
            let label = s.label.clone().unwrap_or_else(|| s.action.clone());
            let when = match s.end_beat {
                Some(e) => format!("{}–{}", fmt_bar(bar), fmt_bar(e / bpb + 1.0)),
                None => fmt_bar(bar),
            };
            lines.push((bar, 1, format!("{when:<6}   cue {}: {label}", c.id)));
        }
    }
    lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
    let mut out = format!("Bars {from}–{to}:\n");
    if lines.is_empty() {
        out.push_str("  nothing scheduled, no phrase boundary\n");
    }
    for (_, _, l) in lines {
        out.push_str("  ");
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn fmt_bar(bar: f64) -> String {
    if bar.fract() == 0.0 {
        format!("{}", bar as i64)
    } else {
        format!("{bar:.2}")
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

fn ramp_target(state: &PerfState, ramp: &Value) -> Result<(Value, String, f64), String> {
    let obj = ramp
        .as_object()
        .ok_or_else(|| "ramp must be an object, e.g. {\"tempo\": 134}".to_string())?;
    let track_of = |wire: &mut serde_json::Map<String, Value>| -> Result<String, String> {
        let which = obj
            .get("track")
            .ok_or_else(|| "this ramp needs \"track\"".to_string())?;
        let kind = obj
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("track")
            .to_string();
        if kind == "master" {
            wire.insert("kind".into(), json!("master"));
            return Ok("master".into());
        }
        let t = state.track_by(which)?;
        wire.insert("track_index".into(), json!(t.index));
        wire.insert("kind".into(), json!(kind));
        Ok(t.name.clone())
    };
    let mut wire = serde_json::Map::new();
    let (target, to, label) = if let Some(v) = obj.get("tempo").and_then(Value::as_f64) {
        if !(20.0..=999.0).contains(&v) {
            return Err(format!("tempo {v} is outside 20–999 BPM"));
        }
        (
            "tempo",
            v,
            format!("tempo {} → {}", fmt_num(state.tempo), fmt_num(v)),
        )
    } else if let Some(v) = obj.get("crossfader").and_then(Value::as_f64) {
        (
            "crossfader",
            v,
            format!("crossfader → {}", if v >= 0.5 { "B" } else { "A" }),
        )
    } else if let Some(v) = obj.get("volume").and_then(Value::as_f64) {
        let name = track_of(&mut wire)?;
        ("volume", v, format!("{name} volume → {}", fmt_num(v)))
    } else if let Some(v) = obj.get("send").and_then(Value::as_f64) {
        let name = track_of(&mut wire)?;
        let si = obj.get("send_index").and_then(Value::as_i64).unwrap_or(0);
        wire.insert("send_index".into(), json!(si));
        ("send", v, format!("{name} send {si} → {}", fmt_num(v)))
    } else if let (Some(target), Some(to)) = (
        obj.get("target").and_then(Value::as_str),
        obj.get("to").and_then(Value::as_f64),
    ) {
        match target {
            "tempo" => (
                "tempo",
                to,
                format!("tempo {} → {}", fmt_num(state.tempo), fmt_num(to)),
            ),
            "crossfader" => ("crossfader", to, format!("crossfader → {}", fmt_num(to))),
            "volume" | "send" | "device" | "mute" => {
                let name = track_of(&mut wire)?;
                for k in ["device_index", "parameter_index", "send_index"] {
                    if let Some(v) = obj.get(k) {
                        wire.insert(k.into(), v.clone());
                    }
                }
                let what = if target == "device" {
                    format!(
                        "{name} device {} parameter {}",
                        obj.get("device_index").map(display_v).unwrap_or_default(),
                        obj.get("parameter_index")
                            .map(display_v)
                            .unwrap_or_default()
                    )
                } else {
                    format!("{name} {target}")
                };
                (target, to, format!("{what} → {}", fmt_num(to)))
            }
            other => return Err(format!("unknown ramp target '{other}'")),
        }
    } else {
        return Err("ramp needs one of tempo, crossfader, volume, send, or target + to".into());
    };
    wire.insert("target".into(), json!(target));
    wire.insert("to".into(), json!(to));
    Ok((Value::Object(wire), label, to))
}

fn display_v(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Resolve a cue against the state. Errors name the step; warnings are
/// returned for the result text.
pub fn resolve_cue(state: &PerfState, p: &CueParams) -> Result<ResolvedCue, String> {
    if p.steps.is_empty() {
        return Err("A cue needs at least one step.".into());
    }
    if !state.is_playing {
        return Err(
            "Nothing is playing: cues run on the transport. start_performance first.".into(),
        );
    }
    let bpb = state.beats_per_bar();
    let mut out = ResolvedCue::default();
    if state.clip_trigger_quantization != ONE_BAR {
        out.warnings.push(format!(
            "Launch quantization is {}, not 1 bar: launches are fired inside the bar before their target and Live places them on that grid. set_launch_quantization \"1_bar\" for bar-exact cues.",
            state.quantization_name()
        ));
    }
    // Simulated playing slot per track, for the silence check.
    let mut playing: Vec<Option<i64>> = state
        .tracks
        .iter()
        .map(|t| {
            if t.fired_slot_index >= 0 {
                Some(t.fired_slot_index)
            } else if t.fired_slot_index == -2 {
                None
            } else if t.playing_slot_index >= 0 {
                Some(t.playing_slot_index)
            } else {
                None
            }
        })
        .collect();
    let mut launches: Vec<(f64, usize)> = Vec::new();
    for (i, step) in p.steps.iter().enumerate() {
        let n = i + 1;
        let actions = [
            step.fire_scene.is_some(),
            step.fire_clip.is_some(),
            step.stop_clip.is_some(),
            step.stop_all_clips == Some(true),
            step.set.is_some(),
            step.ramp.is_some(),
            step.gesture.is_some(),
        ]
        .iter()
        .filter(|a| **a)
        .count();
        if actions != 1 {
            return Err(format!(
                "step {n}: give exactly one of fire_scene, fire_clip, stop_clip, stop_all_clips, set, ramp or gesture"
            ));
        }
        let when = step
            .at
            .as_ref()
            .or(step.from.as_ref())
            .ok_or_else(|| format!("step {n}: give \"at\" (or \"from\" for a ramp)"))?;
        let (bar, beat, sub_bar) =
            resolve_time_ex(state, when).map_err(|e| format!("step {n}: {e}"))?;
        if sub_bar
            && state.clip_trigger_quantization != 0
            && state.clip_trigger_quantization <= ONE_BAR
        {
            return Err(format!(
                "step {n} is at bar {} but launch quantization is {}, so it would land on bar {}. set_launch_quantization \"1/4\" first, or use {{\"bar\": {}}}.",
                fmt_bar(bar),
                quantization_words(&state.quantization_name()),
                bar.floor() as i64 + 1,
                bar.floor() as i64
            ));
        }
        if let Some(g) = &step.gesture {
            if beat < state.beat {
                return Err(format!(
                    "step {n} is at bar {} but it is bar {}. Use \"next_bar\" or a bar ahead of {}.",
                    fmt_bar(bar),
                    state.position(),
                    state.next_bar()
                ));
            }
            let expanded = expand_gesture(state, &mut playing, bar, beat, g, p.allow_silence)
                .map_err(|e| format!("step {n}: {e}"))?;
            for (wire, line) in expanded.steps {
                out.steps.push(wire);
                out.lines.push(line);
            }
            out.warnings.extend(expanded.warnings);
            let ahead = beat - state.beat;
            if ahead < bpb {
                out.warnings.push(format!(
                    "step {n} is at bar {} and it is {} — {:.1} s away; cue two bars ahead for certainty.",
                    fmt_bar(bar),
                    state.position(),
                    ahead * state.seconds_per_beat()
                ));
            }
            launches.push((beat, i));
            continue;
        }
        let mut wire = json!({"beat": beat, "bar": bar});
        let line_time;
        let label;
        if let Some(ramp) = &step.ramp {
            let bars = step
                .bars
                .ok_or_else(|| format!("step {n}: a ramp needs \"bars\""))?;
            if bars <= 0.0 || bars > 64.0 {
                return Err(format!("step {n}: a ramp takes 1 to 64 bars"));
            }
            let (target, text, _) =
                ramp_target(state, ramp).map_err(|e| format!("step {n}: {e}"))?;
            let end_beat = beat + bars * bpb;
            wire["action"] = json!("ramp");
            wire["end_beat"] = json!(end_beat);
            for (k, v) in target.as_object().unwrap() {
                wire[k] = v.clone();
            }
            line_time = format!("bars {}–{}", fmt_bar(bar), fmt_bar(bar + bars));
            label = format!("ramp {text}");
        } else {
            if beat <= state.beat {
                return Err(format!(
                    "step {n} is at bar {} but it is bar {}. Use \"next_bar\" or a bar ahead of {}.",
                    fmt_bar(bar),
                    state.position(),
                    state.next_bar()
                ));
            }
            line_time = format!("bar {}", fmt_bar(bar));
            if let Some(which) = &step.fire_scene {
                let scene = state
                    .scene_by(which)
                    .map_err(|e| format!("step {n}: {e}"))?;
                wire["action"] = json!("fire_scene");
                wire["scene_index"] = json!(scene.index);
                let names: Vec<String> = scene
                    .clip_tracks
                    .iter()
                    .filter_map(|ti| state.tracks.iter().find(|t| t.index == *ti))
                    .map(|t| t.name.clone())
                    .collect();
                let mut stops: Vec<String> = Vec::new();
                let mut would_record: Vec<String> = Vec::new();
                for (ti, t) in state.tracks.iter().enumerate() {
                    if scene.clip_tracks.contains(&t.index) {
                        playing[ti] = Some(scene.index);
                    } else if t.no_stop_slots.contains(&scene.index) {
                        // no stop button in this row: the track keeps playing
                    } else {
                        if playing[ti].is_some() {
                            stops.push(t.name.clone());
                        }
                        if t.arm {
                            would_record.push(t.name.clone());
                        }
                        playing[ti] = None;
                    }
                }
                let mut text = format!(
                    "fire scene '{}' ({} clip{}: {})",
                    scene.name,
                    names.len(),
                    if names.len() == 1 { "" } else { "s" },
                    names.join(", ")
                );
                if !stops.is_empty() {
                    text.push_str(&format!("; {} out", stops.join(", ")));
                }
                if !would_record.is_empty() {
                    out.warnings.push(format!(
                        "step {n}: {} armed with an empty slot in '{}' — Live records into it if Start Recording on Scene Launch is on; disarm first or give the track a clip",
                        would_record.join(", "),
                        scene.name
                    ));
                }
                label = text;
                launches.push((beat, i));
            } else if let Some(c) = &step.fire_clip {
                let t = state
                    .track_by(&c.track)
                    .map_err(|e| format!("step {n}: {e}"))?;
                if !t.slots_with_clips.contains(&c.clip) {
                    return Err(format!(
                        "step {n}: slot {} on '{}' holds no clip",
                        c.clip, t.name
                    ));
                }
                wire["action"] = json!("fire_clip");
                wire["track_index"] = json!(t.index);
                wire["clip_index"] = json!(c.clip);
                let ti = state
                    .tracks
                    .iter()
                    .position(|x| x.index == t.index)
                    .unwrap();
                playing[ti] = Some(c.clip);
                label = format!("fire {}/slot {}", t.name, c.clip);
                launches.push((beat, i));
            } else if let Some(c) = &step.stop_clip {
                let t = state
                    .track_by(&c.track)
                    .map_err(|e| format!("step {n}: {e}"))?;
                wire["action"] = json!("stop_clip");
                wire["track_index"] = json!(t.index);
                wire["clip_index"] = json!(c.clip);
                let ti = state
                    .tracks
                    .iter()
                    .position(|x| x.index == t.index)
                    .unwrap();
                playing[ti] = None;
                label = format!("stop {}/slot {}", t.name, c.clip);
                launches.push((beat, i));
            } else if step.stop_all_clips == Some(true) {
                wire["action"] = json!("stop_all_clips");
                for p in playing.iter_mut() {
                    *p = None;
                }
                label = "stop all clips".into();
                launches.push((beat, i));
            } else if let Some(set) = &step.set {
                wire["action"] = json!("set");
                wire["target"] = json!(set.target);
                wire["value"] = json!(set.value);
                let mut who = String::new();
                if !matches!(set.target.as_str(), "tempo" | "crossfader") {
                    let kind = set.kind.clone().unwrap_or_else(|| "track".into());
                    wire["kind"] = json!(kind);
                    if kind != "master" {
                        let which = set.track.as_ref().ok_or_else(|| {
                            format!("step {n}: set {} needs \"track\"", set.target)
                        })?;
                        let t = state
                            .track_by(which)
                            .map_err(|e| format!("step {n}: {e}"))?;
                        wire["track_index"] = json!(t.index);
                        who = format!("{} ", t.name);
                    } else {
                        who = "master ".into();
                    }
                }
                for (k, v) in [
                    ("device_index", set.device_index),
                    ("parameter_index", set.parameter_index),
                    ("send_index", set.send_index),
                ] {
                    if let Some(v) = v {
                        wire[k] = json!(v);
                    }
                }
                if set.target == "tempo" && !(20.0..=999.0).contains(&set.value) {
                    return Err(format!(
                        "step {n}: tempo {} is outside 20–999 BPM",
                        set.value
                    ));
                }
                label = format!("set {who}{} = {}", set.target, fmt_num(set.value));
            } else {
                unreachable!("exactly one action")
            }
            let ahead = beat - state.beat;
            if ahead < bpb {
                out.warnings.push(format!(
                    "step {n} is at bar {} and it is {} — {:.1} s away. The script fires it during bar {} and Live quantizes it to {}, but if the socket is slow it lands a bar late. For certainty, cue two bars ahead.",
                    fmt_bar(bar),
                    state.position(),
                    ahead * state.seconds_per_beat(),
                    state.bar,
                    fmt_bar(bar)
                ));
            }
        }
        wire["label"] = json!(label);
        out.lines.push(format!("{line_time:<12} {label}"));
        out.steps.push(wire);
        // Silence check after every launch step.
        if let Some((_, idx)) = launches.last() {
            if *idx == i && playing.iter().all(Option::is_none) && !p.allow_silence {
                return Err(format!(
                    "cue: at bar {} nothing would be playing ({}). Add \"allow_silence\": true if that is the plan.",
                    fmt_bar(bar),
                    label_for_silence(step)
                ));
            }
        }
    }
    if launches.is_empty() {
        out.warnings.retain(|w| !w.contains("Launch quantization"));
    }
    Ok(out)
}

struct Expansion {
    steps: Vec<(Value, String)>,
    warnings: Vec<String>,
}

fn names_of(v: Option<&Value>) -> Vec<Value> {
    v.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn track_list(state: &PerfState, which: &[Value]) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for w in which {
        let t = state.track_by(w)?;
        out.push(
            state
                .tracks
                .iter()
                .position(|x| x.index == t.index)
                .unwrap(),
        );
    }
    Ok(out)
}

fn join(names: &[String]) -> String {
    names.join(", ")
}

/// Expand one gesture into primitive steps at `beat`, updating the simulated
/// playing slots as the steps would.
fn expand_gesture(
    state: &PerfState,
    playing: &mut [Option<i64>],
    bar: f64,
    beat: f64,
    g: &Value,
    allow_silence: bool,
) -> Result<Expansion, String> {
    let obj = g.as_object().filter(|o| o.len() == 1).ok_or_else(|| {
        "gesture must be one of breakdown, drop, mute_except, sweep, build, panic, restore_mix"
            .to_string()
    })?;
    let (name, spec) = obj.iter().next().unwrap();
    let bpb = state.beats_per_bar();
    let mut ex = Expansion {
        steps: Vec::new(),
        warnings: Vec::new(),
    };
    let keep = track_list(state, &names_of(spec.get("keep")))?;
    let bars = spec.get("bars").and_then(Value::as_f64);
    let mut stops: Vec<String> = Vec::new();
    match name.as_str() {
        "breakdown" | "drop" => {
            let bars = bars.unwrap_or(if name == "drop" { 1.0 } else { 8.0 });
            if bars <= 0.0 || bars > 64.0 {
                return Err(format!("{name}: bars must be 1 to 64"));
            }
            for &ti in &keep {
                if playing[ti].is_none() {
                    return Err(format!(
                        "{name} keeps {} — it is not playing anything at bar {} (nothing queued either). Fire it first, or keep another track.",
                        state.tracks[ti].name,
                        fmt_bar(bar)
                    ));
                }
            }
            let mut refire: Vec<(usize, i64)> = Vec::new();
            for (ti, t) in state.tracks.iter().enumerate() {
                if keep.contains(&ti) {
                    continue;
                }
                if let Some(slot) = playing[ti] {
                    ex.steps.push((
                        json!({"action": "stop_clip", "beat": beat, "bar": bar, "track_index": t.index, "clip_index": slot,
                               "label": format!("{name}: stop {}/slot {slot}", t.name)}),
                        String::new(),
                    ));
                    refire.push((ti, slot));
                    stops.push(t.name.clone());
                    playing[ti] = None;
                }
            }
            if refire.is_empty() {
                return Err(format!("{name}: nothing is playing that it would take out"));
            }
            let kept: Vec<String> = keep.iter().map(|ti| state.tracks[*ti].name.clone()).collect();
            let end_beat = beat + bars * bpb;
            let end_bar = bar + bars;
            let first = ex.steps.len() - refire.len();
            ex.steps[first].1 = format!(
                "{:<12} {name}: {} out (stop clips){}",
                format!("bar {}", fmt_bar(bar)),
                join(&stops),
                if kept.is_empty() { String::new() } else { format!(", {} stay{}", join(&kept), if kept.len() == 1 { "s" } else { "" }) }
            );
            for (i, (ti, slot)) in refire.iter().enumerate() {
                let t = &state.tracks[*ti];
                ex.steps.push((
                    json!({"action": "fire_clip", "beat": end_beat, "bar": end_bar, "track_index": t.index, "clip_index": slot,
                           "label": format!("{name} ends: {}/slot {slot} back", t.name)}),
                    if i == 0 {
                        format!("{:<12} {name} ends: {} back (fire the slots they were playing)", format!("bar {}", fmt_bar(end_bar)), join(&stops))
                    } else {
                        String::new()
                    },
                ));
            }
            if playing.iter().all(Option::is_none) && !allow_silence {
                return Err(format!(
                    "{name}: at bar {} nothing would be playing (keep at least one track, or add \"allow_silence\": true).",
                    fmt_bar(bar)
                ));
            }
            for (ti, slot) in refire {
                playing[ti] = Some(slot);
            }
        }
        "mute_except" => {
            if keep.is_empty() {
                return Err("mute_except needs \"keep\"".into());
            }
            let mut muted: Vec<String> = Vec::new();
            for (ti, t) in state.tracks.iter().enumerate() {
                if keep.contains(&ti) || playing[ti].is_none() {
                    continue;
                }
                ex.steps.push((
                    json!({"action": "set", "beat": beat, "bar": bar, "target": "mute", "kind": "track", "track_index": t.index, "value": 1.0,
                           "label": format!("mute {}", t.name)}),
                    String::new(),
                ));
                muted.push(t.name.clone());
            }
            if muted.is_empty() {
                return Err("mute_except: nothing else is playing".into());
            }
            let n0 = ex.steps.len() - muted.len();
            ex.steps[n0].1 = format!("{:<12} mute {}", format!("bar {}", fmt_bar(bar)), join(&muted));
            if let Some(b) = bars {
                let end_beat = beat + b * bpb;
                let end_bar = bar + b;
                for (i, t) in state.tracks.iter().enumerate().filter(|(_, t)| muted.contains(&t.name)) {
                    let _ = i;
                    ex.steps.push((
                        json!({"action": "set", "beat": end_beat, "bar": end_bar, "target": "mute", "kind": "track", "track_index": t.index, "value": 0.0,
                               "label": format!("unmute {}", t.name)}),
                        String::new(),
                    ));
                }
                let n1 = ex.steps.len() - muted.len();
                ex.steps[n1].1 = format!("{:<12} unmute {}", format!("bar {}", fmt_bar(end_bar)), join(&muted));
            }
        }
        "sweep" => {
            let track = state.track_by(spec.get("track").ok_or("sweep needs \"track\"")?)?;
            let di = spec.get("device_index").and_then(Value::as_i64).ok_or("sweep needs \"device_index\"")?;
            let pi = spec.get("parameter_index").and_then(Value::as_i64).ok_or("sweep needs \"parameter_index\"")?;
            let to = spec.get("to").and_then(Value::as_f64).ok_or("sweep needs \"to\"")?;
            let bars = bars.ok_or("sweep needs \"bars\"")?;
            if bars <= 0.0 || bars > 64.0 {
                return Err("sweep: bars must be 1 to 64".into());
            }
            if let Some(from) = spec.get("from").and_then(Value::as_f64) {
                ex.steps.push((
                    json!({"action": "set", "beat": beat - 0.01, "bar": bar, "target": "device", "kind": "track", "track_index": track.index,
                           "device_index": di, "parameter_index": pi, "value": from, "label": format!("sweep start {}", track.name)}),
                    String::new(),
                ));
            }
            let end_beat = beat + bars * bpb;
            ex.steps.push((
                json!({"action": "ramp", "beat": beat, "end_beat": end_beat, "bar": bar, "target": "device", "kind": "track", "track_index": track.index,
                       "device_index": di, "parameter_index": pi, "to": to, "label": format!("sweep {} device {di} parameter {pi} → {}", track.name, fmt_num(to))}),
                format!("{:<12} sweep {} device {di} parameter {pi} → {}", format!("bars {}–{}", fmt_bar(bar), fmt_bar(bar + bars)), track.name, fmt_num(to)),
            ));
        }
        "build" => {
            let track = state.track_by(spec.get("track").ok_or("build needs \"track\" (the riser)")?)?;
            let to = spec.get("send").and_then(Value::as_f64).ok_or("build needs \"send\" (the send level to reach)")?;
            let si = spec.get("send_index").and_then(Value::as_i64).unwrap_or(0);
            let bars = bars.ok_or("build needs \"bars\"")?;
            if bars <= 0.0 || bars > 64.0 {
                return Err("build: bars must be 1 to 64".into());
            }
            let end_beat = beat + bars * bpb;
            ex.steps.push((
                json!({"action": "ramp", "beat": beat, "end_beat": end_beat, "bar": bar, "target": "send", "kind": "track", "track_index": track.index,
                       "send_index": si, "to": to, "label": format!("build: {} send {si} → {}", track.name, fmt_num(to))}),
                format!("{:<12} build: {} send {si} → {}", format!("bars {}–{}", fmt_bar(bar), fmt_bar(bar + bars)), track.name, fmt_num(to)),
            ));
            if let Some(h) = spec.get("hats") {
                let hats = state.track_by(h)?;
                let hi = state.tracks.iter().position(|x| x.index == hats.index).unwrap();
                let current = playing[hi];
                let next = hats
                    .slots_with_clips
                    .iter()
                    .copied()
                    .find(|s| current.is_none_or(|c| *s > c))
                    .ok_or_else(|| format!("build: {} has no denser clip in a later slot to swap to", hats.name))?;
                let last_bar_beat = beat + (bars - 1.0).max(0.0) * bpb;
                ex.steps.push((
                    json!({"action": "fire_clip", "beat": last_bar_beat, "bar": bar + (bars - 1.0).max(0.0), "track_index": hats.index, "clip_index": next,
                           "label": format!("build: {} → slot {next} for the last bar", hats.name)}),
                    format!("{:<12} build: {} → slot {next} for the last bar", format!("bar {}", fmt_bar(bar + (bars - 1.0).max(0.0))), hats.name),
                ));
                playing[hi] = Some(next);
            }
        }
        "panic" => {
            let bars = bars.unwrap_or(1.0).clamp(0.25, 16.0);
            let end_beat = beat + bars * bpb;
            let mut gone: Vec<String> = Vec::new();
            for (ti, t) in state.tracks.iter().enumerate() {
                if keep.contains(&ti) || playing[ti].is_none() {
                    continue;
                }
                ex.steps.push((
                    json!({"action": "ramp", "beat": beat, "end_beat": end_beat, "bar": bar, "target": "volume", "kind": "track", "track_index": t.index, "to": 0.0,
                           "label": format!("panic: fade {}", t.name)}),
                    String::new(),
                ));
                ex.steps.push((
                    json!({"action": "stop_clip", "beat": end_beat, "bar": bar + bars, "track_index": t.index, "clip_index": playing[ti].unwrap(),
                           "label": format!("panic: stop {}", t.name)}),
                    String::new(),
                ));
                gone.push(t.name.clone());
                playing[ti] = None;
            }
            if gone.is_empty() {
                return Err("panic: nothing to fade — every playing track is in keep".into());
            }
            let n0 = ex.steps.len() - gone.len() * 2;
            ex.steps[n0].1 = format!("{:<12} panic: fade {} to silence, then stop (faders stay down: snapshot_mix first to restore them)", format!("bars {}–{}", fmt_bar(bar), fmt_bar(bar + bars)), join(&gone));
            if playing.iter().all(Option::is_none) && !allow_silence {
                ex.warnings.push("panic leaves nothing playing; that is usually the point.".into());
            }
        }
        "restore_mix" => {
            let id = spec.get("snapshot").or(spec.get("id")).and_then(Value::as_i64).ok_or("restore_mix needs \"snapshot\"")?;
            ex.steps.push((
                json!({"action": "restore_mix", "beat": beat, "bar": bar, "snapshot_id": id, "label": format!("restore mix snapshot {id}")}),
                format!("{:<12} restore mix snapshot {id}", format!("bar {}", fmt_bar(bar))),
            ));
        }
        other => return Err(format!("unknown gesture '{other}'; one of breakdown, drop, mute_except, sweep, build, panic, restore_mix")),
    }
    ex.steps.retain(|(_, _)| true);
    Ok(ex)
}

fn label_for_silence(step: &CueStep) -> String {
    if let Some(s) = &step.fire_scene {
        format!("scene {} has no clip on any playing track", display_v(s))
    } else if step.stop_all_clips == Some(true) {
        "stop all clips".into()
    } else {
        "the last clip would stop".into()
    }
}

// ── Text ────────────────────────────────────────────────────────────────────

/// The header line: bar, tempo, signature, since, quantization, key.
pub fn header_line(state: &PerfState, since: Option<(i64, f64)>, key: Option<&str>) -> String {
    let mut s = format!(
        "bar {} · {} BPM · {}/{}",
        state.position(),
        fmt_num(state.tempo),
        state.signature_numerator,
        state.signature_denominator
    );
    if state.is_playing {
        match since {
            Some((bar, secs)) => {
                s.push_str(&format!(" · playing since bar {bar} ({})", fmt_secs(secs)))
            }
            None => s.push_str(" · playing"),
        }
    } else {
        s.push_str(" · stopped");
    }
    s.push_str(&format!(
        " · quantization {}",
        quantization_words(&state.quantization_name())
    ));
    if let Some(k) = key {
        s.push_str(&format!(" · key {k}"));
    }
    s
}

fn quantization_words(name: &str) -> String {
    match name {
        "1_bar" => "1 bar".into(),
        "2_bars" => "2 bars".into(),
        "4_bars" => "4 bars".into(),
        "8_bars" => "8 bars".into(),
        other => other.to_string(),
    }
}

fn fmt_secs(s: f64) -> String {
    if s >= 60.0 {
        format!(
            "{} min {} s",
            (s / 60.0).floor() as i64,
            (s % 60.0).round() as i64
        )
    } else {
        format!("{} s", s.round() as i64)
    }
}

/// The full state readout the producer sees.
pub fn state_text(state: &PerfState, since: Option<(i64, f64)>, key: Option<&str>) -> String {
    state_text_with(state, since, key, &[])
}

/// The readout with extra lines (the song line) after the scenes.
pub fn state_text_with(
    state: &PerfState,
    since: Option<(i64, f64)>,
    key: Option<&str>,
    extra: &[String],
) -> String {
    let mut out = header_line(state, since, key);
    out.push('\n');
    let width = state
        .tracks
        .iter()
        .map(|t| t.name.chars().count() + if t.arm { 2 } else { 0 })
        .max()
        .unwrap_or(5)
        .clamp(5, 26);
    out.push_str(&format!(
        "{:<w$}  {:<28} {}\n",
        "Track",
        "Playing",
        "Queued",
        w = width
    ));
    for t in &state.tracks {
        let playing = match t.playing_slot_index {
            i if i >= 0 => t
                .playing_clip_name
                .clone()
                .unwrap_or_else(|| format!("slot {i}")),
            _ => "–".into(),
        };
        let mut playing = playing;
        if t.is_recording {
            playing.push_str(" (recording)");
        }
        let queued = match t.fired_slot_index {
            -2 => "stop".to_string(),
            i if i >= 0 => t
                .fired_clip_name
                .clone()
                .unwrap_or_else(|| format!("slot {i}")),
            _ => "–".into(),
        };
        let mut name = t.name.clone();
        if t.arm {
            name.push_str(" ●");
        }
        out.push_str(&format!(
            "{:<w$}  {:<28} {}\n",
            trunc(&name, width),
            trunc(&playing, 28),
            queued,
            w = width
        ));
    }
    let rows: Vec<&SceneState> = state
        .scenes
        .iter()
        .filter(|s| !crate::song::is_setlist_scene(&s.name))
        .collect();
    let named: Vec<String> = rows
        .iter()
        .filter(|s| !s.clip_tracks.is_empty())
        .map(|s| {
            let mut name = if s.is_playing {
                format!("[{}]", s.name)
            } else if s.is_triggered {
                format!("({})", s.name)
            } else {
                s.name.clone()
            };
            let (_, suffix) = crate::song::parse_section_name(&s.name);
            if let Some(pb) = s
                .phrase_bars
                .filter(|_| !s.phrase_default && suffix.is_none())
            {
                name.push_str(&format!(" (phrase {pb})"));
            }
            name
        })
        .collect();
    let empty = rows.len() - named.len();
    out.push_str("Scenes: ");
    out.push_str(&named.join(" · "));
    if empty > 0 {
        out.push_str(&format!(" · ({empty} empty)"));
    }
    out.push('\n');
    for line in extra {
        out.push_str(line);
        out.push('\n');
    }
    if state.cues.is_empty() {
        out.push_str("Cues: none pending\n");
    } else {
        for c in &state.cues {
            let next = c.steps.iter().find(|s| !s.done);
            let done = c.steps.iter().filter(|s| s.done).count();
            out.push_str(&format!("Cue {} '{}'", c.id, c.name));
            if let Some(n) = next {
                out.push_str(&format!(
                    " — next step bar {} {}",
                    n.bar.map(fmt_bar).unwrap_or_else(|| "?".into()),
                    n.label.clone().unwrap_or_else(|| n.action.clone())
                ));
            }
            if done > 0 {
                out.push_str(&format!(
                    "; {done} step{} done",
                    if done == 1 { "" } else { "s" }
                ));
            }
            out.push('\n');
        }
    }
    if let Some(pr) = &state.pending_record {
        out.push_str(&format!(
            "Recording: {} slot {} '{}'\n",
            pr.get("track_index").map(display_v).unwrap_or_default(),
            pr.get("slot").map(display_v).unwrap_or_default(),
            pr.get("name").map(display_v).unwrap_or_default()
        ));
    }
    if state.is_playing {
        out.push_str(&format!(
            "Next bar ({}) in {:.1} s.",
            state.next_bar(),
            state.seconds_to_next_bar()
        ));
    }
    let ev = events_text(&state.events);
    if !ev.is_empty() {
        out.push('\n');
        out.push_str(&ev);
    }
    out
}

fn trunc(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        s.chars().take(width - 1).collect::<String>() + "…"
    }
}

/// "While I was away: …" — what the clock did since the last read.
pub fn events_text(events: &[Value]) -> String {
    if events.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    for e in events {
        let kind = e.get("type").and_then(Value::as_str).unwrap_or("");
        let cue = e.get("cue_id").map(display_v).unwrap_or_default();
        let bar = e
            .get("target_bar")
            .and_then(Value::as_f64)
            .map(fmt_bar)
            .unwrap_or_else(|| {
                e.get("beat")
                    .map(|b| format!("beat {}", display_v(b)))
                    .unwrap_or_default()
            });
        let label = e
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let line = match kind {
            "cue_step_fired" => format!(
                "cue {cue} fired at bar {bar}: {}{}",
                if label.is_empty() {
                    e.get("action").map(display_v).unwrap_or_default()
                } else {
                    label
                },
                if e.get("late") == Some(&json!(true)) {
                    " (late)"
                } else {
                    ""
                }
            ),
            "cue_step_failed" => format!(
                "cue {cue} step at bar {bar} failed: {}",
                e.get("error").map(display_v).unwrap_or_default()
            ),
            "ramp_started" => format!("cue {cue}: {label} started"),
            "ramp_done" => format!("cue {cue}: {label} done"),
            "cue_done" => format!("cue {cue} complete"),
            "cue_cancelled" => format!(
                "cue {cue} cancelled ({}), {} step(s) not run",
                e.get("reason").map(display_v).unwrap_or_default(),
                e.get("steps_left").map(display_v).unwrap_or_default()
            ),
            "recording_done" => format!(
                "recording '{}' on {} finished ({} beats), track disarmed",
                e.get("name").map(display_v).unwrap_or_default(),
                e.get("track").map(display_v).unwrap_or_default(),
                e.get("length").map(display_v).unwrap_or_default()
            ),
            "recording_abandoned" => format!(
                "recording '{}' on {} abandoned ({}), track disarmed",
                e.get("name").map(display_v).unwrap_or_default(),
                e.get("track").map(display_v).unwrap_or_default(),
                e.get("reason").map(display_v).unwrap_or_default()
            ),
            other => other.to_string(),
        };
        lines.push(line);
    }
    format!("Since the last call: {}", lines.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> PerfState {
        PerfState::from_value(&json!({
            "is_playing": true, "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
            "beat": 53.5, "bar": 14, "beat_in_bar": 2, "clip_trigger_quantization": 4,
            "clip_trigger_quantization_name": "1_bar",
            "scale_name": "Minor", "root_note": 5, "root_note_name": "F",
            "tracks": [
                {"index": 0, "name": "Kick", "playing_slot_index": 1, "playing_clip_name": "Groove/Kick", "slots_with_clips": [0, 1, 2]},
                {"index": 1, "name": "Bass", "playing_slot_index": 1, "playing_clip_name": "Groove/Bass", "slots_with_clips": [1, 2]},
                {"index": 2, "name": "Pad", "playing_slot_index": -1, "slots_with_clips": [2, 3]},
                {"index": 3, "name": "Lead", "playing_slot_index": -1, "arm": true, "slots_with_clips": [2]}
            ],
            "scenes": [
                {"index": 0, "name": "Intro", "clip_tracks": [0]},
                {"index": 1, "name": "Groove", "is_playing": true, "clip_tracks": [0, 1]},
                {"index": 2, "name": "Groove+Pad", "clip_tracks": [0, 1, 2, 3]},
                {"index": 3, "name": "Break", "clip_tracks": [2]},
                {"index": 4, "name": "Outro", "clip_tracks": []},
                {"index": 5, "name": "", "clip_tracks": []}
            ],
            "cues": [], "events": []
        }))
        .unwrap()
    }

    #[test]
    fn bar_arithmetic_uses_lives_numbering() {
        let s = state();
        assert_eq!(s.beats_per_bar(), 4.0);
        assert_eq!(s.bar_start(1.0), 0.0);
        assert_eq!(s.bar_start(49.0), 192.0);
        assert_eq!(s.next_bar(), 15);
        // bar 15 starts at beat 56; we are at 53.5 → 2.5 beats at 126 BPM
        assert!((s.seconds_to_next_bar() - 2.5 * 60.0 / 126.0).abs() < 1e-9);
        assert_eq!(s.position(), "14.2");
        assert_eq!(s.key_from_set().as_deref(), Some("F Minor"));
        assert_eq!(
            resolve_time(&s, &CueTime::Named("next_bar".into())).unwrap(),
            (15.0, 56.0)
        );
        assert_eq!(
            resolve_time(&s, &CueTime::After { bars_after: 8.0 }).unwrap(),
            (23.0, 88.0)
        );
        assert_eq!(
            resolve_time(&s, &CueTime::Bar { bar: 49.0 }).unwrap(),
            (49.0, 192.0)
        );
        assert_eq!(resolve_time(&s, &CueTime::Number(3.0)).unwrap(), (3.0, 8.0));
        assert!(resolve_time(&s, &CueTime::Named("later".into())).is_err());
    }

    #[test]
    fn keys_parse_like_a_producer_writes_them() {
        assert_eq!(parse_key("D minor"), Some((2, "Minor".into())));
        assert_eq!(parse_key("F# major"), Some((6, "Major".into())));
        assert_eq!(parse_key("Bb dorian"), Some((10, "Dorian".into())));
        assert_eq!(parse_key("Am"), Some((9, "Minor".into())));
        assert_eq!(parse_key("G"), Some((7, "Major".into())));
        assert_eq!(
            parse_key("Eb harmonic minor"),
            Some((3, "Harmonic Minor".into()))
        );
        assert_eq!(parse_key("techno"), None);
    }

    #[test]
    fn a_track_without_stop_buttons_keeps_playing_through_a_scene() {
        let mut s = state();
        // Lead plays slot 2 and has no stop button in the Break row (3).
        s.tracks[3].playing_slot_index = 2;
        s.tracks[3].no_stop_slots = vec![3, 4];
        s.tracks[3].arm = false;
        let p: CueParams =
            serde_json::from_value(json!({"steps": [{"at": {"bar": 49}, "fire_scene": "Break"}]}))
                .unwrap();
        let r = resolve_cue(&s, &p).unwrap();
        assert!(
            r.lines[0].contains("Kick, Bass out") && !r.lines[0].contains("Lead"),
            "{}",
            r.lines[0]
        );
        // Outro has no clips at all, but Lead keeps playing through row 4 → not silent.
        let p: CueParams =
            serde_json::from_value(json!({"steps": [{"at": {"bar": 49}, "fire_scene": "Outro"}]}))
                .unwrap();
        assert!(resolve_cue(&s, &p).is_ok());
    }

    #[test]
    fn launch_landing_bar_follows_the_quantization() {
        let mut s = state();
        assert!(s.launch_lands_on().starts_with("at bar 15 (1-bar"));
        s.clip_trigger_quantization = 2; // 4 bars: bars 1,5,9,13,17 → next is 17
        assert!(s.launch_lands_on().starts_with("at bar 17 (4-bar"));
        s.clip_trigger_quantization = 0;
        assert!(s.launch_lands_on().starts_with("immediately"));
    }

    #[test]
    fn cue_resolves_bars_names_and_ramps() {
        let s = state();
        let p: CueParams = serde_json::from_value(json!({
            "name": "into breaks",
            "steps": [
                {"at": {"bar": 49}, "fire_scene": "Break"},
                {"from": {"bar": 49}, "bars": 8, "ramp": {"tempo": 134}},
                {"at": {"bar": 57}, "fire_scene": "Groove+Pad"}
            ]
        }))
        .unwrap();
        let r = resolve_cue(&s, &p).unwrap();
        assert_eq!(r.steps.len(), 3);
        assert_eq!(r.steps[0]["action"], "fire_scene");
        assert_eq!(r.steps[0]["scene_index"], 3);
        assert_eq!(r.steps[0]["beat"], 192.0);
        assert_eq!(r.steps[1]["action"], "ramp");
        assert_eq!(r.steps[1]["target"], "tempo");
        assert_eq!(r.steps[1]["to"], 134.0);
        assert_eq!(r.steps[1]["end_beat"], 224.0);
        assert_eq!(r.steps[2]["scene_index"], 2);
        assert!(
            r.lines[0].contains("fire scene 'Break' (1 clip: Pad); Kick, Bass out"),
            "{}",
            r.lines[0]
        );
        assert!(
            r.lines[1].contains("bars 49–57") && r.lines[1].contains("tempo 126 → 134"),
            "{}",
            r.lines[1]
        );
        // Lead is armed and has no clip in Break → warned, not refused.
        assert!(
            r.warnings.iter().any(|w| w.contains("Lead armed")),
            "{:?}",
            r.warnings
        );
    }

    #[test]
    fn cue_refuses_the_past_unknown_scenes_and_silence() {
        let s = state();
        let past: CueParams =
            serde_json::from_value(json!({"steps": [{"at": {"bar": 10}, "fire_scene": "Break"}]}))
                .unwrap();
        let e = resolve_cue(&s, &past).unwrap_err();
        assert!(e.contains("step 1 is at bar 10 but it is bar 14.2"), "{e}");
        let unknown: CueParams =
            serde_json::from_value(json!({"steps": [{"at": "next_bar", "fire_scene": "Drop"}]}))
                .unwrap();
        assert!(resolve_cue(&s, &unknown)
            .unwrap_err()
            .contains("no scene named 'Drop'"));
        let silent: CueParams =
            serde_json::from_value(json!({"steps": [{"at": {"bar": 20}, "fire_scene": "Outro"}]}))
                .unwrap();
        let e = resolve_cue(&s, &silent).unwrap_err();
        assert!(e.contains("at bar 20 nothing would be playing"), "{e}");
        let allowed: CueParams = serde_json::from_value(
            json!({"allow_silence": true, "steps": [{"at": {"bar": 20}, "fire_scene": "Outro"}]}),
        )
        .unwrap();
        assert!(resolve_cue(&s, &allowed).is_ok());
        let two: CueParams = serde_json::from_value(
            json!({"steps": [{"at": {"bar": 20}, "fire_scene": "Break", "stop_all_clips": true}]}),
        )
        .unwrap();
        assert!(resolve_cue(&s, &two).unwrap_err().contains("exactly one"));
        let no_clip: CueParams = serde_json::from_value(
            json!({"steps": [{"at": {"bar": 20}, "fire_clip": {"track": "Pad", "clip": 0}}]}),
        )
        .unwrap();
        assert!(resolve_cue(&s, &no_clip)
            .unwrap_err()
            .contains("holds no clip"));
    }

    #[test]
    fn cue_warns_when_less_than_a_bar_ahead_or_off_grid() {
        let mut s = state();
        let soon: CueParams = serde_json::from_value(
            json!({"steps": [{"at": "next_bar", "fire_scene": "Groove+Pad"}]}),
        )
        .unwrap();
        let r = resolve_cue(&s, &soon).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("step 1 is at bar 15 and it is 14.2")),
            "{:?}",
            r.warnings
        );
        s.clip_trigger_quantization = 0;
        s.clip_trigger_quantization_name = Some("none".into());
        let r = resolve_cue(&s, &soon).unwrap();
        assert!(r
            .warnings
            .iter()
            .any(|w| w.contains("Launch quantization is none")));
    }

    #[test]
    fn state_text_reads_like_the_prototype() {
        let mut s = state();
        s.cues = vec![CueState {
            id: 1,
            name: "open up".into(),
            steps: vec![
                CueStepState {
                    index: 0,
                    action: "fire_scene".into(),
                    beat: 32.0,
                    bar: Some(9.0),
                    label: Some("fire scene 'Groove'".into()),
                    done: true,
                    ..Default::default()
                },
                CueStepState {
                    index: 1,
                    action: "fire_scene".into(),
                    beat: 64.0,
                    bar: Some(17.0),
                    label: Some("fire scene 'Groove+Pad'".into()),
                    done: false,
                    ..Default::default()
                },
            ],
            pending: 1,
        }];
        s.events = vec![
            json!({"type": "cue_step_fired", "cue_id": 1, "target_bar": 9.0, "label": "fire scene 'Groove'"}),
        ];
        let t = state_text(&s, Some((1, 26.0)), Some("F minor"));
        assert!(t.starts_with("bar 14.2 · 126 BPM · 4/4 · playing since bar 1 (26 s) · quantization 1 bar · key F minor\n"), "{t}");
        assert!(
            t.lines()
                .any(|l| l.starts_with("Kick") && l.contains("Groove/Kick")),
            "{t}"
        );
        assert!(t.contains("Lead ●"), "{t}");
        assert!(
            t.contains("Scenes: Intro · [Groove] · Groove+Pad · Break · (2 empty)"),
            "{t}"
        );
        assert!(
            t.contains("Cue 1 'open up' — next step bar 17 fire scene 'Groove+Pad'; 1 step done"),
            "{t}"
        );
        assert!(t.contains("Next bar (15) in 1.2 s."), "{t}");
        assert!(
            t.contains("Since the last call: cue 1 fired at bar 9: fire scene 'Groove'"),
            "{t}"
        );
        assert!(t.len() < 1200);
    }
}
