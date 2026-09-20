//! The section and song tools: `make_section`, `set_song` and its edits,
//! `play_song`, and the steering verbs (`hold_section`, `go`,
//! `next_section`, `previous_section`, `back`, `jump_to`). The nouns and
//! the arithmetic live in [`crate::song`]; this module turns them into
//! commands. Steering is always one state read and one cue: the new plan
//! replaces the old one in the same `schedule_cue` call.

use crate::connection::LiveState;
use crate::notes::NotesInput;
use crate::performance::{self as perf, CueParams, CueStep, CueTime, PerfState};
use crate::song::{self, JumpFrom, Section, SetlistEntry, Song};
use crate::tools::{
    self, clip_notes, get_display, live_err, performance_running, read_perf_state, require,
    write_notes, CreateClipParams, StartPerformanceParams, ToolResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

// ── Parameters ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct MakeSectionParams {
    /// The section's name (unique; the scene is named "<name> · <bars>")
    pub name: String,
    /// Bars per phrase (default: the source section's, else 16)
    #[serde(default)]
    pub phrase_bars: Option<i64>,
    /// "playing" (Live's capture-and-insert-scene copies what plays into a new row) or {"section": "Groove"} (a copy of that section, then `changes`)
    #[serde(default)]
    pub from: Option<Value>,
    /// With from {"section": …}: per-track changes — a vary_clip variation name ("thin", "half_time", "double_time", "fill_last_bar", "ghost_notes", "invert_chords"), {"transpose": -12}, "empty", or replacement notes in any compact form
    #[serde(default)]
    pub changes: BTreeMap<String, Value>,
    /// Or write the section: clips per track, {"Kick": {"steps": {"C1": "x..."}}, "Bass": {"notes_csv": "41,0,4,90", "length": 8}}; tracks not named stay empty
    #[serde(default)]
    pub clips: BTreeMap<String, Value>,
    /// Insert after this section (default: at the end; for "playing", below the playing row)
    #[serde(default)]
    pub after: Option<String>,
    /// A scene tempo the section sets when it fires
    #[serde(default)]
    pub tempo: Option<f64>,
    /// Rewrite an existing section of this name in place (clips only), so the setlist keeps pointing at it
    #[serde(default)]
    pub replace: bool,
}

/// One track's clip in a written section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SectionClip {
    /// Length in beats (default: the notes' extent rounded up to whole bars)
    #[serde(default)]
    pub length: Option<f64>,
    /// Clip name (default "<section>/<track>")
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub notes: NotesInput,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SetSongParams {
    /// The song in order; an entry without repeats loops until you say go
    pub setlist: Vec<SetlistEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AddToSongParams {
    /// Section to add
    pub section: String,
    /// Put it after this section's first entry (default: at the end)
    #[serde(default)]
    pub after: Option<String>,
    /// Or before this section's first entry
    #[serde(default)]
    pub before: Option<String>,
    /// Passes before the song moves on by itself; without it the section loops until go
    #[serde(default)]
    pub repeats: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RemoveFromSongParams {
    /// Section to take out of the song (every entry of it)
    pub section: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PlaySongParams {
    /// Start from this section of the setlist (default: the first entry)
    #[serde(default)]
    pub from: Option<String>,
    /// What to do with the Arrangement while the song plays: "ask" (default — records from bar 1 when the Arrangement is empty, otherwise returns what is there and the choices), "after" (record after everything already there), "replace" (delete it and record from bar 1), "off" (do not record)
    #[serde(default = "ask_record")]
    pub record: String,
}

fn ask_record() -> String {
    "ask".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SteerParams {
    /// When: "next_phrase" (default: the end of the playing section's phrase), "next_bar" (cuts the phrase short, said in the reply), or a bar number
    #[serde(default)]
    pub at: Option<String>,
    /// Skip the level warning when the target ran more than 3 dB hotter
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct JumpToParams {
    /// Section name
    pub section: String,
    /// Passes before the song continues by itself (default: loop until go)
    #[serde(default)]
    pub repeats: Option<u32>,
    /// When: "next_phrase" (default: the end of the playing section's phrase), "next_bar" (cuts the phrase short, said in the reply), or a bar number
    #[serde(default)]
    pub at: Option<String>,
    /// How to get there: {"tempo": 122, "retime": {"tracks": ["Kick"], "to": "half_time"}, "crossfade": {"out": ["Pad"], "in": ["Keys"], "bars": 8}, "fill": {"track": "Drums"}, "drop": {"bars": 1, "keep": []}, "sweep": {…}}; the default is a straight cut
    #[serde(default)]
    pub transition: Option<Value>,
    /// Skip the level warning when the target ran more than 3 dB hotter
    #[serde(default)]
    pub force: bool,
}

// ── Sections ────────────────────────────────────────────────────────────────

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn track_names(state: &PerfState, indices: &[i64]) -> Vec<String> {
    indices
        .iter()
        .filter_map(|i| state.tracks.iter().find(|t| t.index == *i))
        .map(|t| t.name.clone())
        .collect()
}

fn clip_track_indices(r: &Value) -> Vec<i64> {
    r.get("clips")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("track_index").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

fn variation_past(v: &str) -> &'static str {
    match v {
        "thin" => "thinned (every other off-beat dropped)",
        "half_time" => "half-timed",
        "double_time" => "double-timed",
        "fill_last_bar" => "given a fill in its last bar",
        "ghost_notes" => "given ghost notes",
        "invert_chords" => "chords inverted",
        _ => "varied",
    }
}

enum Source {
    Playing,
    Section(String),
    Clips,
}

fn source_of(p: &MakeSectionParams) -> Result<Source, String> {
    let from = match &p.from {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("playing") => Some(Source::Playing),
        Some(Value::Object(o)) => match o.get("section").and_then(Value::as_str) {
            Some(s) => Some(Source::Section(s.to_string())),
            None => return Err("from must be \"playing\" or {\"section\": \"<name>\"}".into()),
        },
        Some(Value::String(s)) => Some(Source::Section(s.clone())),
        Some(other) => {
            return Err(format!(
                "from must be \"playing\" or {{\"section\": …}}, not {other}"
            ))
        }
    };
    match (from, p.clips.is_empty()) {
        (Some(_), false) => Err("give `from` or `clips`, not both".into()),
        (Some(s), true) => Ok(s),
        (None, false) => Ok(Source::Clips),
        (None, true) => Err("Give the material: from: \"playing\" (what plays now), from: {\"section\": \"Groove\"} with `changes`, or `clips` per track.".into()),
    }
}

pub fn make_section_body(live: &LiveState, p: &MakeSectionParams) -> ToolResult {
    let name = p.name.trim();
    if name.is_empty() {
        return Err("A section needs a name.".into());
    }
    if song::is_reserved_scene(name) || name.contains(song::SEP) {
        return Err(format!(
            "'{name}' cannot be a section name: \"{}\" is the phrase separator, \"{}\" marks the song and \"{}\" marks the parked ideas.",
            song::SEP.trim(),
            song::SETLIST_PREFIX,
            song::STASH_PREFIX
        ));
    }
    if let Some(pb) = p.phrase_bars {
        if !(1..=128).contains(&pb) {
            return Err(format!("phrase_bars must be between 1 and 128, got {pb}"));
        }
    }
    let source = source_of(p)?;
    let state = read_perf_state(live)?;
    let secs = song::sections(&state);
    let existing = song::find_section(&secs, name).cloned();
    if let Some(ex) = &existing {
        if !p.replace {
            return Err(format!(
                "make_section: a scene named '{}' exists (scene {}). Names are how sections are remembered; call this one '{name} 2' or replace: true.",
                ex.scene_name, ex.index
            ));
        }
        if !matches!(source, Source::Clips) {
            return Err(format!(
                "replace: true rewrites a section from `clips`; '{}' (scene {}) would need its row replaced by a capture or a copy, which Live inserts as a new row. Pick another name, or write it with `clips`.",
                ex.scene_name, ex.index
            ));
        }
    }
    let after = match &p.after {
        Some(a) => Some(
            song::find_section(&secs, a)
                .ok_or_else(|| {
                    format!(
                        "after: '{a}' is not a section; sections: {}",
                        song::section_names(&secs)
                    )
                })?
                .clone(),
        ),
        None => None,
    };
    let mut text = match source {
        Source::Playing => make_from_playing(live, &state, name, p, after.as_ref())?,
        Source::Section(src) => {
            make_from_section(live, &state, &secs, name, p, &src, after.as_ref())?
        }
        Source::Clips => make_from_clips(live, &state, name, p, existing.as_ref(), after.as_ref())?,
    };
    if performance_running(live).and_then(|r| r.song).is_some() {
        text.push_str("\nThe song's setlist is unchanged.");
    }
    Ok(text)
}

fn set_tempo_if_given(live: &LiveState, index: i64, tempo: Option<f64>) -> Result<String, String> {
    let Some(t) = tempo else {
        return Ok(String::new());
    };
    require(live, "set_scene")?;
    live.send_command("set_scene", Some(json!({"index": index, "tempo": t})))
        .map_err(|e| live_err("set the scene tempo", e))?;
    Ok(format!(" Scene tempo {t}."))
}

fn make_from_playing(
    live: &LiveState,
    state: &PerfState,
    name: &str,
    p: &MakeSectionParams,
    after: Option<&Section>,
) -> ToolResult {
    require(live, "capture_scene")?;
    if !state.is_playing || !state.tracks.iter().any(|t| t.playing_slot_index >= 0) {
        return Err("Nothing is playing to capture. Fire a scene or clips first, or write the section with `clips`.".into());
    }
    let r = live
        .send_command(
            "capture_scene",
            Some(json!({"name": name, "phrase_bars": p.phrase_bars, "after": after.map(|a| a.index)})),
        )
        .map_err(|e| live_err("capture the playing clips into a scene", e))?;
    let index = r.get("index").and_then(Value::as_i64).unwrap_or(-1);
    let names = track_names(state, &clip_track_indices(&r));
    let tempo = set_tempo_if_given(live, index, p.tempo)?;
    Ok(format!(
        "Section '{}' saved as scene {index}: the playing clips ({}) captured into a new row below {} (Live's capture-and-insert-scene, which launches the copy with no audible change; the phrase count carries on).{tempo} The set keeps playing.\nNot in the setlist yet: add_to_song {{\"section\": \"{name}\"}} or jump_to it.",
        get_display(&r, "name", name),
        if names.is_empty() { "none".to_string() } else { names.join(", ") },
        match after {
            Some(a) => format!("'{}'", a.name),
            None => "the playing row".to_string(),
        }
    ))
}

fn make_from_section(
    live: &LiveState,
    state: &PerfState,
    secs: &[Section],
    name: &str,
    p: &MakeSectionParams,
    src: &str,
    after: Option<&Section>,
) -> ToolResult {
    require(live, "duplicate_scene")?;
    let source = song::find_section(secs, src).ok_or_else(|| {
        format!(
            "from: '{src}' is not a section; sections: {}",
            song::section_names(secs)
        )
    })?;
    if let Some(a) = after {
        if a.index != source.index {
            return Err(format!(
                "a copy is inserted right below its source ('{}' is scene {}); `after` cannot move it. Leave `after` out.",
                source.name, source.index
            ));
        }
    }
    // Validate the changes before touching the set.
    let mut planned: Vec<(i64, String, Change)> = Vec::new();
    for (track, change) in &p.changes {
        let t = state.track_by(&json!(track))?;
        planned.push((
            t.index,
            t.name.clone(),
            Change::parse(change).map_err(|e| format!("changes for '{track}': {e}"))?,
        ));
    }
    let r = live
        .send_command(
            "duplicate_scene",
            Some(json!({"index": source.index, "name": name, "phrase_bars": p.phrase_bars})),
        )
        .map_err(|e| live_err("duplicate the scene", e))?;
    let index = r
        .get("index")
        .and_then(Value::as_i64)
        .unwrap_or(source.index + 1);
    let copied = clip_track_indices(&r);
    let mut diffs: Vec<String> = Vec::new();
    let mut changed: Vec<i64> = Vec::new();
    for (ti, tname, change) in planned {
        let has_clip = copied.contains(&ti);
        let what = apply_change(
            live,
            state,
            &Slot {
                track_index: ti,
                slot: index,
                has_clip,
                section: name,
                track_name: &tname,
            },
            &change,
        )?;
        diffs.push(format!("{tname} {what}"));
        changed.push(ti);
    }
    let same: Vec<String> = track_names(
        state,
        &copied
            .iter()
            .copied()
            .filter(|i| !changed.contains(i))
            .collect::<Vec<_>>(),
    );
    let tempo = set_tempo_if_given(live, index, p.tempo)?;
    let mut text = format!(
        "Section '{}' created as scene {index}, a copy of '{}'",
        get_display(&r, "name", name),
        source.scene_name
    );
    if diffs.is_empty() {
        text.push_str(" with nothing changed yet (give `changes` per track).");
    } else {
        text.push_str(&format!(" with: {}.", diffs.join(", ")));
    }
    if !same.is_empty() {
        text.push_str(&format!(
            " {} {} identical to {}.",
            same.join(", "),
            if same.len() == 1 { "is" } else { "are" },
            source.name
        ));
    }
    text.push_str(&tempo);
    text.push_str(&format!(
        "\nNot in the setlist: jump_to it or add_to_song {{\"section\": \"{name}\"}}."
    ));
    Ok(text)
}

enum Change {
    Empty,
    Vary(String),
    Transpose(i64),
    Notes(NotesInput),
}

impl Change {
    fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::String(s) if s.trim().eq_ignore_ascii_case("empty") => Ok(Change::Empty),
            Value::String(s) if crate::variation::VARIATIONS.contains(&s.trim()) => {
                Ok(Change::Vary(s.trim().to_string()))
            }
            Value::String(s) => Err(format!(
                "'{s}' is not a change: use \"empty\", a variation ({}), {{\"transpose\": n}} or notes",
                crate::variation::VARIATIONS.join(", ")
            )),
            Value::Object(o) if o.contains_key("transpose") => o
                .get("transpose")
                .and_then(Value::as_i64)
                .filter(|n| (-48..=48).contains(n))
                .map(Change::Transpose)
                .ok_or_else(|| "transpose must be a whole number of semitones (−48…48)".into()),
            Value::Object(_) => {
                let input: NotesInput = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
                if input.is_empty() {
                    return Err("no notes given (notes, notes_csv, steps or patterns)".into());
                }
                Ok(Change::Notes(input))
            }
            other => Err(format!("{other} is not a change")),
        }
    }
}

/// One clip slot in a copied row, for a change.
struct Slot<'a> {
    track_index: i64,
    slot: i64,
    has_clip: bool,
    section: &'a str,
    track_name: &'a str,
}

fn apply_change(
    live: &LiveState,
    state: &PerfState,
    at: &Slot<'_>,
    change: &Change,
) -> Result<String, String> {
    let Slot {
        track_index,
        slot,
        has_clip,
        section,
        track_name,
    } = *at;
    match change {
        Change::Empty => {
            if has_clip {
                require(live, "delete_clip")?;
                live.send_command(
                    "delete_clip",
                    Some(json!({"track_index": track_index, "clip_index": slot})),
                )
                .map_err(|e| live_err("empty the copied clip", e))?;
            }
            Ok("emptied".into())
        }
        Change::Vary(v) => {
            if !has_clip {
                return Err(format!(
                    "'{track_name}' has no clip in the source row to vary"
                ));
            }
            let raw = clip_notes(live, track_index, slot)?;
            let notes: Vec<crate::variation::VNote> = raw
                .iter()
                .filter_map(crate::variation::VNote::from_value)
                .collect();
            let length = notes
                .iter()
                .map(|n| n.start + n.duration)
                .fold(0.0, f64::max)
                .max(state.beats_per_bar());
            let length = (length / state.beats_per_bar()).ceil() * state.beats_per_bar();
            let varied = crate::variation::vary(&notes, v, 1, length, state.beats_per_bar())?;
            let values: Vec<Value> = varied.iter().map(|n| n.to_value()).collect();
            write_notes(live, track_index, slot, &values)?;
            Ok(format!("{} (seed 1)", variation_past(v)))
        }
        Change::Transpose(n) => {
            if !has_clip {
                return Err(format!(
                    "'{track_name}' has no clip in the source row to transpose"
                ));
            }
            let raw = clip_notes(live, track_index, slot)?;
            let shifted: Vec<Value> = raw
                .iter()
                .map(|note| {
                    let mut m = note.clone();
                    if let Some(p) = note.get("pitch").and_then(Value::as_i64) {
                        m["pitch"] = json!((p + n).clamp(0, 127));
                    }
                    m
                })
                .collect();
            write_notes(live, track_index, slot, &shifted)?;
            Ok(format!("transposed {n:+}"))
        }
        Change::Notes(input) => {
            let notes = crate::notes::expand(input)?;
            if has_clip {
                let values: Vec<Value> = notes
                    .iter()
                    .map(|n| serde_json::to_value(n).unwrap_or(Value::Null))
                    .collect();
                write_notes(live, track_index, slot, &values)?;
            } else {
                let length = clip_length(&notes, state.beats_per_bar(), None);
                tools::create_clip_body(
                    live,
                    &CreateClipParams {
                        track_index: Some(track_index),
                        clip_index: Some(slot),
                        length,
                        name: format!("{section}/{track_name}"),
                        input: NotesInput {
                            notes: notes.clone(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )?;
            }
            Ok(format!("rewritten ({})", plural(notes.len(), "note")))
        }
    }
}

fn clip_length(notes: &[tools::Note], bpb: f64, given: Option<f64>) -> f64 {
    if let Some(l) = given.filter(|l| *l > 0.0) {
        return l;
    }
    let extent = notes
        .iter()
        .map(|n| n.start_time + n.duration)
        .fold(0.0, f64::max);
    ((extent / bpb).ceil().max(1.0)) * bpb
}

fn make_from_clips(
    live: &LiveState,
    state: &PerfState,
    name: &str,
    p: &MakeSectionParams,
    existing: Option<&Section>,
    after: Option<&Section>,
) -> ToolResult {
    require(live, "create_scene")?;
    require(live, "create_clip")?;
    // Validate every clip before the first command.
    let mut planned: Vec<(i64, String, SectionClip, Vec<tools::Note>)> = Vec::new();
    for (track, doc) in &p.clips {
        let t = state.track_by(&json!(track))?;
        let clip: SectionClip =
            serde_json::from_value(doc.clone()).map_err(|e| format!("clips for '{track}': {e}"))?;
        let notes =
            crate::notes::expand(&clip.notes).map_err(|e| format!("clips for '{track}': {e}"))?;
        if notes.is_empty() {
            return Err(format!(
                "clips for '{track}': no notes (notes, notes_csv, steps or patterns)"
            ));
        }
        planned.push((t.index, t.name.clone(), clip, notes));
    }
    let bars = p.phrase_bars.or_else(|| existing.map(|e| e.bars));
    let scene_name = song::section_name(name, bars);
    let (index, how) = match existing {
        Some(ex) => {
            require(live, "delete_clip")?;
            let mut cleared = 0;
            for t in &state.tracks {
                if t.slots_with_clips.contains(&ex.index) {
                    live.send_command(
                        "delete_clip",
                        Some(json!({"track_index": t.index, "clip_index": ex.index})),
                    )
                    .map_err(|e| live_err("clear the old section's clips", e))?;
                    cleared += 1;
                }
            }
            if ex.scene_name != scene_name {
                require(live, "set_scene")?;
                live.send_command(
                    "set_scene",
                    Some(json!({"index": ex.index, "name": scene_name})),
                )
                .map_err(|e| live_err("rename the scene", e))?;
            }
            (
                ex.index,
                format!(
                    "rewritten in place (scene {}, {} old clip{} removed)",
                    ex.index,
                    cleared,
                    if cleared == 1 { "" } else { "s" }
                ),
            )
        }
        None => {
            let at = after.map(|a| a.index + 1).unwrap_or(-1);
            let r = live
                .send_command(
                    "create_scene",
                    Some(json!({"index": at, "name": scene_name, "tempo": p.tempo})),
                )
                .map_err(|e| live_err("create the scene", e))?;
            let index = r.get("index").and_then(Value::as_i64).unwrap_or(at);
            (
                index,
                match after {
                    Some(a) => format!(
                        "created as scene {index} (after {}; later rows moved down)",
                        a.name
                    ),
                    None => format!("created as scene {index} (at the end)"),
                },
            )
        }
    };
    // Every clip of the row in one round trip.
    require(live, "write_clips")?;
    let mut filled: Vec<String> = Vec::new();
    let mut specs: Vec<Value> = Vec::new();
    for (ti, tname, clip, notes) in planned {
        let length = clip_length(&notes, state.beats_per_bar(), clip.length);
        specs.push(json!({
            "track_index": ti, "clip_index": index, "length": length,
            "name": clip.name.clone().unwrap_or_else(|| format!("{name}/{tname}")),
            "notes": notes,
        }));
        filled.push(tname);
    }
    live.send_command("write_clips", Some(json!({"clips": specs})))
        .map_err(|e| live_err("write the section's clips", e))?;
    let empty: Vec<String> = state
        .tracks
        .iter()
        .filter(|t| !filled.contains(&t.name) && !t.no_stop_slots.contains(&index))
        .filter(|t| t.playing_slot_index >= 0 || !t.slots_with_clips.is_empty())
        .map(|t| t.name.clone())
        .collect();
    let tempo = if existing.is_some() {
        set_tempo_if_given(live, index, p.tempo)?
    } else {
        String::new()
    };
    let mut text = format!(
        "Section '{scene_name}' {how}: {} {} clips",
        filled.join(", "),
        if filled.len() == 1 { "has" } else { "have" }
    );
    if !empty.is_empty() {
        text.push_str(&format!(
            "; {} {} empty ({} when the section fires; keep_track_playing carries a track through)",
            empty.join(", "),
            if empty.len() == 1 { "is" } else { "are" },
            if empty.len() == 1 {
                "it stops"
            } else {
                "they stop"
            }
        ));
    }
    text.push('.');
    text.push_str(&tempo);
    if existing.is_none() {
        text.push_str(&format!(
            "\nNot in the setlist yet: add_to_song {{\"section\": \"{name}\"{}}} or jump_to it.",
            after
                .map(|a| format!(", \"after\": \"{}\"", a.name))
                .unwrap_or_default()
        ));
    }
    Ok(text)
}

// ── Songs ───────────────────────────────────────────────────────────────────

fn write_setlist(
    live: &LiveState,
    state: &PerfState,
    entries: &[SetlistEntry],
) -> Result<String, String> {
    let name = song::render_setlist(entries);
    match song::setlist_scene(state) {
        Some(sc) => {
            require(live, "set_scene")?;
            live.send_command("set_scene", Some(json!({"index": sc.index, "name": name})))
                .map_err(|e| live_err("write the setlist scene", e))?;
        }
        None => {
            require(live, "create_scene")?;
            live.send_command("create_scene", Some(json!({"index": -1, "name": name})))
                .map_err(|e| live_err("create the setlist scene", e))?;
        }
    }
    Ok(name)
}

fn counted_text(entries: &[SetlistEntry], secs: &[Section]) -> String {
    let counted: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            let n = e.repeats?;
            let bars = song::find_section(secs, &e.section)?.bars;
            Some(format!("{}×{n} = {} bars", e.section, n as i64 * bars))
        })
        .collect();
    let waiting = entries.iter().filter(|e| e.repeats.is_none()).count();
    let mut s = String::new();
    if counted.is_empty() {
        s.push_str("Every section loops until you say go.");
    } else {
        s.push_str(&format!("Counted: {}; ", counted.join(", ")));
        s.push_str(&match waiting {
            0 => "no section waits for go (the last one keeps looping when the count runs out)."
                .to_string(),
            1 => "one section loops until you say go.".to_string(),
            n => format!("the other {n} loop until you say go."),
        });
    }
    s
}

/// The reply to any setlist write; re-plans a running song.
fn song_written(
    live: &LiveState,
    state: &PerfState,
    secs: &[Section],
    entries: &[SetlistEntry],
    scene_name: &str,
) -> String {
    let mut text = format!(
        "Song: {}. {}\nStored as the scene '{scene_name}' (an empty row at the bottom of the set); edit the name in Live to change the order.",
        song::setlist_text(entries),
        counted_text(entries, secs)
    );
    match replan_running(live, state, secs, entries) {
        Ok(Some(line)) => {
            text.push('\n');
            text.push_str(&line);
        }
        Ok(None) => text.push_str("\nplay_song starts it; go, next_section, back and jump_to steer it. Give an entry a count (\"Drop×4\") to pre-plan it."),
        Err(e) => text.push_str(&format!("\nThe running song could not be re-planned: {e}")),
    }
    text
}

pub fn set_song_body(live: &LiveState, p: &SetSongParams) -> ToolResult {
    let state = read_perf_state(live)?;
    let secs = song::sections(&state);
    song::validate_setlist(&p.setlist, &secs, &state)?;
    let scene_name = write_setlist(live, &state, &p.setlist)?;
    Ok(song_written(live, &state, &secs, &p.setlist, &scene_name))
}

fn current_setlist(state: &PerfState) -> Result<Vec<SetlistEntry>, String> {
    song::read_setlist(state)?.ok_or_else(|| {
        "No song in this set: set_song first (the setlist lives in the name of a 'Setlist:' scene).".to_string()
    })
}

pub fn add_to_song_body(live: &LiveState, p: &AddToSongParams) -> ToolResult {
    let state = read_perf_state(live)?;
    let secs = song::sections(&state);
    let mut entries = current_setlist(&state).unwrap_or_default();
    let section = song::find_section(&secs, &p.section).ok_or_else(|| {
        format!(
            "'{}' is not a section. Sections: {}. make_section first.",
            p.section,
            song::section_names(&secs)
        )
    })?;
    let entry = SetlistEntry {
        section: section.name.clone(),
        repeats: p.repeats,
    };
    let anchor = |which: &Option<String>| -> Result<Option<usize>, String> {
        match which {
            None => Ok(None),
            Some(a) => entries
                .iter()
                .position(|e| e.section.eq_ignore_ascii_case(a.trim()))
                .map(Some)
                .ok_or_else(|| {
                    format!("'{a}' is not in the song: {}", song::setlist_text(&entries))
                }),
        }
    };
    let at = match (anchor(&p.after)?, anchor(&p.before)?) {
        (Some(_), Some(_)) => return Err("give after or before, not both".into()),
        (Some(a), None) => a + 1,
        (None, Some(b)) => b,
        (None, None) => entries.len(),
    };
    entries.insert(at, entry);
    song::validate_setlist(&entries, &secs, &state)?;
    let scene_name = write_setlist(live, &state, &entries)?;
    Ok(song_written(live, &state, &secs, &entries, &scene_name))
}

pub fn remove_from_song_body(live: &LiveState, p: &RemoveFromSongParams) -> ToolResult {
    let state = read_perf_state(live)?;
    let secs = song::sections(&state);
    let mut entries = current_setlist(&state)?;
    let before = entries.len();
    entries.retain(|e| !e.section.eq_ignore_ascii_case(p.section.trim()));
    if entries.len() == before {
        return Err(format!(
            "'{}' is not in the song: {}",
            p.section,
            song::setlist_text(&entries)
        ));
    }
    if entries.is_empty() {
        return Err("That would leave the song empty; set_song with a new setlist instead.".into());
    }
    let scene_name = write_setlist(live, &state, &entries)?;
    Ok(song_written(live, &state, &secs, &entries, &scene_name))
}

/// A running song whose setlist just changed: keep the cursor on the section
/// that plays and schedule the counted jumps again from there.
fn replan_running(
    live: &LiveState,
    state: &PerfState,
    secs: &[Section],
    entries: &[SetlistEntry],
) -> Result<Option<String>, String> {
    let Some(running) = performance_running(live) else {
        return Ok(None);
    };
    let Some(old) = running.song else {
        return Ok(None);
    };
    let mut song = Song {
        entries: entries.to_vec(),
        position: old.position,
        current: old.current.clone(),
        plan_cue_id: old.plan_cue_id,
        jump_history: old.jump_history.clone(),
        started_bar: old.started_bar,
    };
    let cur = song::cursor(state, &song, secs);
    let Some(sec) = cur.section.clone() else {
        store_song(live, song);
        return Ok(Some("The song is re-set; nothing is playing yet.".into()));
    };
    if !cur.in_setlist {
        song.position = None;
        store_song(live, song);
        return Ok(Some(format!(
            "'{}' (playing now) is not in the new song; jump_to a section of it to continue.",
            sec.name
        )));
    }
    let position = cur.position;
    let repeats = cur.repeats;
    let started = cur.started_bar.unwrap_or(state.bar);
    // The counted jump out of the current section: where the count ends, or
    // the end of this phrase when the count already passed.
    let start_bar = match repeats {
        Some(n) if started + n as i64 * sec.bars > state.bar => started,
        Some(n) => {
            state
                .phrase
                .as_ref()
                .map(|p| p.ends_bar)
                .unwrap_or(state.next_bar())
                - n as i64 * sec.bars
        }
        None => started,
    };
    let plan = song::plan(secs, entries, &sec, repeats, position, start_bar)?;
    let (cue_id, _, _) = schedule_plan(
        live,
        state,
        &plan,
        false,
        old.plan_cue_id,
        Vec::new(),
        false,
    )?;
    if cue_id.is_none() {
        if let Some(id) = old.plan_cue_id {
            let _ = live.send_command(
                "cancel_cue",
                Some(json!({"id": id, "reason": "song changed"})),
            );
        }
    }
    song.position = position;
    song.current = sec.name.clone();
    song.plan_cue_id = cue_id;
    store_song(live, song);
    Ok(Some(match plan.steps.get(1) {
        Some(s) => format!(
            "The running plan is re-planned from '{}': next {} at bar {}{}.",
            sec.name,
            s.section,
            s.bar,
            cue_id.map(|id| format!(" (cue {id})")).unwrap_or_default()
        ),
        None => format!(
            "The running plan is re-planned: '{}' loops until you say go.",
            sec.name
        ),
    }))
}

fn store_song(live: &LiveState, song: Song) {
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.song = Some(song);
    }
}

/// What scheduling a plan produced: the cue id (None when there was
/// nothing to schedule), one line per step, and the warnings.
pub(crate) type Scheduled = (Option<i64>, Vec<String>, Vec<String>);

/// Resolve a plan into one cue: the fires at their bars (the first one too
/// unless it was fired by hand), plus any transition steps, replacing the
/// old plan cue in the same call.
pub(crate) fn schedule_plan(
    live: &LiveState,
    state: &PerfState,
    plan: &song::Plan,
    include_first: bool,
    replaces: Option<i64>,
    extra: Vec<CueStep>,
    allow_silence: bool,
) -> Result<Scheduled, String> {
    let mut steps = extra;
    for (i, s) in plan.steps.iter().enumerate() {
        if i == 0 && !include_first {
            continue;
        }
        steps.push(CueStep {
            at: Some(CueTime::Bar { bar: s.bar as f64 }),
            fire_scene: Some(json!(s.scene_index)),
            ..Default::default()
        });
    }
    if steps.is_empty() {
        return Ok((None, Vec::new(), Vec::new()));
    }
    require(live, "schedule_cue")?;
    let params = CueParams {
        name: Some("song".into()),
        steps,
        allow_silence,
    };
    let resolved = perf::resolve_cue(state, &params)?;
    let sent = live
        .send_command(
            "schedule_cue",
            Some(json!({"cue": {"name": "song", "steps": resolved.steps, "replaces": replaces}})),
        )
        .map_err(|e| live_err("schedule the song's cue", e))?;
    let id = sent.get("id").and_then(Value::as_i64);
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.cues_scheduled += 1;
    }
    Ok((id, resolved.lines, resolved.warnings))
}

pub fn play_song_body(live: &LiveState, p: &PlaySongParams) -> ToolResult {
    for c in ["get_performance_state", "fire_scene", "schedule_cue"] {
        require(live, c)?;
    }
    let before = read_perf_state(live)?;
    let secs = song::sections(&before);
    let entries = current_setlist(&before)?;
    song::validate_setlist(&entries, &secs, &before)?;
    let from_pos = match &p.from {
        Some(f) => entries
            .iter()
            .position(|e| e.section.eq_ignore_ascii_case(f.trim()))
            .ok_or_else(|| format!("'{f}' is not in the song: {}", song::setlist_text(&entries)))?,
        None => 0,
    };
    let first = song::find_section(&secs, &entries[from_pos].section)
        .cloned()
        .ok_or_else(|| format!("'{}' is not a section", entries[from_pos].section))?;
    let mut text = String::new();
    if performance_running(live).is_none() {
        let started = tools::start_performance_body(
            live,
            &StartPerformanceParams {
                scene: Some(json!(first.index)),
                quantization: "1_bar".into(),
                key: None,
                tempo: None,
                disarm: true,
                limiter: false,
                follow_key: false,
                record: p.record.clone(),
            },
        )?;
        // The take line is the third, right after the scene that fired; it is
        // only there when one is being recorded.
        let recording = performance_running(live).and_then(|r| r.take).is_some();
        let head: Vec<&str> = started
            .lines()
            .take(if recording { 3 } else { 2 })
            .collect();
        text.push_str(&head.join("\n"));
        text.push_str("\nGuards on: the transport-touching tools refuse until end_performance.\n");
        if !before.is_playing {
            text.push_str(&format!(
                "play_song starts a performance; the transport was stopped, so '{}' fires now and counts from bar {}.\n",
                first.name, before.bar
            ));
        }
    } else {
        let r = live
            .send_command("fire_scene", Some(json!({"scene_index": first.index})))
            .map_err(|e| live_err("fire the first section", e))?;
        if let Some(b) = r.get("lands_on_bar").and_then(Value::as_i64) {
            text.push_str(&format!("'{}' fired, landing on bar {b}.\n", first.name));
        }
    }
    let state = read_perf_state(live)?;
    let started_bar = state
        .phrase
        .as_ref()
        .filter(|ph| ph.scene_index == first.index)
        .map(|ph| ph.started_bar)
        .unwrap_or_else(|| {
            if before.is_playing {
                state.next_bar()
            } else {
                state.bar
            }
        });
    let plan = song::plan(
        &secs,
        &entries,
        &first,
        entries[from_pos].repeats,
        Some(from_pos),
        started_bar,
    )?;
    let (cue_id, lines, warnings) =
        schedule_plan(live, &state, &plan, false, None, Vec::new(), false)?;
    store_song(
        live,
        Song {
            entries: entries.clone(),
            position: Some(from_pos),
            current: first.name.clone(),
            plan_cue_id: cue_id,
            jump_history: Vec::new(),
            started_bar,
        },
    );
    text.push_str(&format!(
        "Song from '{}' ({} of {}): {}",
        first.name,
        from_pos + 1,
        entries.len(),
        plan_sentence(&plan, &secs)
    ));
    match cue_id {
        Some(id) => text.push_str(&format!(
            " Cue {id} holds {}.",
            match plan.steps.len() - 1 {
                1 => "that one jump".to_string(),
                n => format!("those {n} jumps"),
            }
        )),
        None => text.push_str(" Nothing to schedule until you say go."),
    }
    for l in &lines {
        text.push_str("\n  ");
        text.push_str(l);
    }
    for w in &warnings {
        text.push_str("\nWarning: ");
        text.push_str(w);
    }
    text.push_str("\nSay go to continue, next_section, back or jump_to <section> at any time; hold_section stops a counted section where it is.");
    // #63: the drift line travels. get_context is called once per session,
    // so a set the producer nudged since the overview was written would
    // otherwise go unnoticed until the next one.
    if let Some(said) = tools::drift_note(live, &state) {
        text.push_str(&format!("\n{said}"));
    }
    Ok(text)
}

/// "Intro×2 from bar 1, then Groove at bar 17 — Groove loops until you say go"
fn plan_sentence(plan: &song::Plan, secs: &[Section]) -> String {
    let first = &plan.steps[0];
    let mut s = match first.repeats {
        Some(n) => format!("{}×{n} from bar {}", first.section, first.bar),
        None => format!("{} from bar {}", first.section, first.bar),
    };
    for step in plan.steps.iter().skip(1) {
        match step.repeats {
            Some(n) => s.push_str(&format!(", then {}×{n} at bar {}", step.section, step.bar)),
            None => s.push_str(&format!(", then {} at bar {}", step.section, step.bar)),
        }
    }
    if let Some(w) = &plan.waits_at {
        s.push_str(&format!(" — {w} loops until you say go."));
    } else if let Some(b) = plan.runs_out_at {
        let last = &plan.steps[plan.steps.len() - 1];
        let bars = song::find_section(secs, &last.section)
            .map(|x| x.bars)
            .unwrap_or(16);
        let _ = bars;
        s.push_str(&format!(
            " — the count runs out at bar {b}; {} keeps looping there.",
            last.section
        ));
    } else {
        s.push('.');
    }
    s
}

// ── Steering ────────────────────────────────────────────────────────────────

enum Target {
    Next,
    Previous,
    Back,
    Named(String),
}

struct Move<'a> {
    verb: &'static str,
    target: Target,
    repeats: Option<u32>,
    at: Option<&'a str>,
    force: bool,
    transition: Option<&'a Value>,
}

/// Where a move lands: the bar, how it is described, and the bars it cuts
/// off the playing section's phrase, if any.
struct Landing {
    bar: i64,
    how: String,
    cut: Option<i64>,
}

fn resolve_landing(state: &PerfState, at: Option<&str>) -> Result<Landing, String> {
    let phrase_end = state.phrase.as_ref().map(|p| p.ends_bar);
    let want = at.map(|s| s.trim().to_lowercase()).unwrap_or_default();
    let (bar, how) = match want.as_str() {
        "" | "next_phrase" | "next phrase" | "phrase" | "end_of_phrase" => match phrase_end {
            Some(b) => (b, "end of this phrase".to_string()),
            None => (state.next_bar(), "next bar; no section has been fired, so there is no phrase to end".to_string()),
        },
        "next_bar" | "next bar" | "bar" | "next" | "now" => (state.next_bar(), "next bar".to_string()),
        other => match other.parse::<i64>() {
            Ok(b) if b > state.bar => (b, format!("bar {b}")),
            Ok(b) => {
                return Err(format!(
                    "bar {b} has passed (it is bar {}); use \"next_bar\", \"next_phrase\" or a bar ahead of {}",
                    state.position(),
                    state.next_bar()
                ))
            }
            Err(_) => return Err(format!("at: '{other}' is not a time; use \"next_phrase\", \"next_bar\" or a bar number")),
        },
    };
    let cut = phrase_end.filter(|e| bar < *e).map(|e| e - bar);
    Ok(Landing { bar, how, cut })
}

fn steer(live: &LiveState, m: Move) -> ToolResult {
    for c in ["get_performance_state", "schedule_cue"] {
        require(live, c)?;
    }
    let started_now = performance_running(live).is_none() && matches!(m.target, Target::Named(_));
    if started_now {
        // A jump with no performance running starts one (start_performance's defaults).
        tools::start_performance_body(
            live,
            &StartPerformanceParams {
                scene: None,
                quantization: "1_bar".into(),
                key: None,
                tempo: None,
                disarm: true,
                limiter: false,
                follow_key: false,
                // Consults the session's remembered answer, so a steering verb
                // that starts a performance asks at most once.
                record: ask_record(),
            },
        )?;
    }
    let Some(running) = performance_running(live) else {
        return Err("No performance is running: play_song starts one from the setlist, or jump_to a section to begin.".into());
    };
    let state = read_perf_state(live)?;
    if !state.is_playing {
        return Err("The transport is stopped, so there is nothing to steer; fire_scene or play_song to resume.".into());
    }
    let secs = song::sections(&state);
    let song = running.song.clone().unwrap_or_default();
    let has_song = !song.entries.is_empty();
    let cur = song::cursor(&state, &song, &secs);
    let need_song = |verb: &str| -> Result<(), String> {
        if has_song {
            Ok(())
        } else {
            Err(format!("{verb} walks the setlist and there is no song: set_song then play_song, or jump_to a section by name."))
        }
    };
    let mut note = String::new();
    let (target, position, repeats): (Section, Option<usize>, Option<u32>) = match &m.target {
        Target::Named(n) => {
            let s = song::find_section(&secs, n).ok_or_else(|| {
                let hint = song::suggest(n, &secs)
                    .map(|x| format!(" (did you mean {x}?)"))
                    .unwrap_or_default();
                format!(
                    "'{n}' is not a section{hint}. Sections: {}.",
                    song::section_names(&secs)
                )
            })?;
            let pos = if has_song {
                let from = cur.position.unwrap_or(0);
                (from..song.entries.len())
                    .chain(0..from)
                    .find(|i| song.entries[*i].section.eq_ignore_ascii_case(&s.name))
            } else {
                None
            };
            let repeats = m
                .repeats
                .or_else(|| pos.and_then(|p| song.entries[p].repeats));
            (s.clone(), pos.or(cur.position), repeats)
        }
        Target::Next | Target::Previous => {
            need_song(m.verb)?;
            let Some(p) = cur.position else {
                return Err(format!(
                    "{}: the song has no position yet; jump_to a section of it first.",
                    m.verb
                ));
            };
            let np = if matches!(m.target, Target::Next) {
                p + 1
            } else if !cur.in_setlist {
                p
            } else if p == 0 {
                return Err(format!(
                    "'{}' is the first section of the song; jump_to a section by name, or back.",
                    song.entries[p].section
                ));
            } else {
                p - 1
            };
            let Some(entry) = song.entries.get(np) else {
                return Err(format!(
                    "'{}' is the last section of the song; jump_to a section by name, or end_performance.",
                    song.entries[p].section
                ));
            };
            let s = song::find_section(&secs, &entry.section)
                .ok_or_else(|| format!("'{}' is no longer a section of the set", entry.section))?;
            (s.clone(), Some(np), m.repeats.or(entry.repeats))
        }
        Target::Back => {
            let Some(from) = song.jump_history.last() else {
                return Err("Nothing to go back to: no jump has been made yet.".into());
            };
            let s = song::find_section(&secs, &from.section)
                .ok_or_else(|| format!("'{}' is no longer a section of the set", from.section))?;
            let pos = from.position.filter(|p| {
                song.entries
                    .get(*p)
                    .is_some_and(|e| e.section.eq_ignore_ascii_case(&s.name))
            });
            let repeats = pos.and_then(|p| song.entries[p].repeats);
            if let Some(c) = &cur.section {
                if has_song && !cur.in_setlist {
                    note = format!(
                        " {} is out of the plan; previous_section walks the setlist instead.",
                        c.name
                    );
                }
            }
            (s.clone(), pos.or(from.position), repeats)
        }
    };
    let landing = resolve_landing(&state, m.at)?;
    let mut warnings: Vec<String> = Vec::new();
    if !m.force {
        if let (Some(lv), Some(cs)) = (&state.levels, &cur.section) {
            if let (Some(tp), Some(cp)) = (lv.section_peak(target.index), lv.section_peak(cs.index))
            {
                if tp - cp > 3.0 {
                    warnings.push(format!(
                        "'{}' last peaked at {} dB, {:.1} dB hotter than {} ({} dB). Jumping anyway (force: true to silence this; pull the master with set_track_mixer master, or cue a ramp, before bar {}).",
                        target.name,
                        song::fmt_db(tp, 1),
                        tp - cp,
                        cs.name,
                        song::fmt_db(cp, 1),
                        landing.bar
                    ));
                }
            }
        }
    }
    let plan = song::plan(
        &secs,
        &song.entries,
        &target,
        repeats,
        position,
        landing.bar,
    )?;
    let mut state = state;
    let transition = match m.transition {
        Some(t) => {
            let tr = crate::transition::expand(live, &mut state, &secs, &target, landing.bar, t)?;
            // The rewrites took round trips: check the cue against where the
            // set is now, not where it was before them.
            state = read_perf_state(live)?;
            tr.apply_to(&mut state);
            tr
        }
        None => crate::transition::Transition::default(),
    };
    let transition_lines = transition.lines;
    let (cue_id, mut lines, cue_warnings) = schedule_plan(
        live,
        &state,
        &plan,
        true,
        song.plan_cue_id,
        transition.steps,
        transition.allow_silence,
    )?;
    let cue_id = cue_id.ok_or_else(|| "the jump produced no cue".to_string())?;
    warnings.extend(cue_warnings);
    if !transition_lines.is_empty() {
        // The transition's own lines say more than the generic ramp/set lines.
        lines.retain(|l| l.contains("fire scene"));
    }
    let mut history = song.jump_history.clone();
    match m.target {
        Target::Back => {
            history.pop();
        }
        _ => {
            if let Some(c) = &cur.section {
                history.push(JumpFrom {
                    section: c.name.clone(),
                    position: cur.position.filter(|_| cur.in_setlist),
                });
            }
        }
    }
    store_song(
        live,
        Song {
            entries: song.entries.clone(),
            position,
            current: target.name.clone(),
            plan_cue_id: Some(cue_id),
            jump_history: history,
            started_bar: landing.bar,
        },
    );
    // The reply.
    let mut text = String::new();
    if started_now {
        text.push_str("Performance started (1-bar launch quantization, guards on). ");
    }
    for w in &warnings {
        if w.contains("hotter") {
            text.push_str("Warning: ");
            text.push_str(w);
            text.push('\n');
        }
    }
    text.push_str(&format!(
        "{} '{}' at bar {} ({})",
        m.verb_lead(),
        target.name,
        landing.bar,
        landing.how
    ));
    if let (Some(cut), Some(cs)) = (landing.cut, &cur.section) {
        text.push_str(&format!(
            " — cutting {} off {}'s phrase, as asked",
            plural(cut as usize, "bar"),
            cs.name
        ));
    }
    match repeats {
        Some(n) => text.push_str(&format!(", {} pass{}", n, if n == 1 { "" } else { "es" })),
        None => text.push_str("; it loops until go"),
    }
    if plan.steps.len() > 1 {
        let rest: Vec<String> = plan
            .steps
            .iter()
            .skip(1)
            .map(|s| match s.repeats {
                Some(n) => format!("{}×{n} at bar {}", s.section, s.bar),
                None => format!("{} at bar {} (loops)", s.section, s.bar),
            })
            .collect();
        text.push_str(&format!(", then the song continues: {}", rest.join(", ")));
    } else if let Some(b) = plan.runs_out_at {
        text.push_str(&format!(
            ", then it keeps looping from bar {b} (the song has nothing after it)"
        ));
    }
    text.push('.');
    text.push_str(&note);
    text.push_str(&match song.plan_cue_id {
        Some(old) => format!(" Cue {cue_id} replaces cue {old}."),
        None => format!(" Cue {cue_id}."),
    });
    for l in transition_lines.iter().chain(lines.iter()) {
        text.push_str("\n  ");
        text.push_str(l);
    }
    for w in warnings.iter().filter(|w| !w.contains("hotter")) {
        text.push_str("\nWarning: ");
        text.push_str(w);
    }
    Ok(text)
}

impl Move<'_> {
    fn verb_lead(&self) -> &'static str {
        match self.verb {
            "go" => "Continuing:",
            "next_section" => "Next:",
            "previous_section" => "Previous:",
            "back" => "Back to where you were:",
            _ => "Jumping to",
        }
    }
}

pub fn hold_section_body(live: &LiveState, _p: &tools::Empty) -> ToolResult {
    require(live, "cancel_cue")?;
    let Some(running) = performance_running(live) else {
        return Err("No performance is running.".into());
    };
    let Some(song) = running.song.clone() else {
        return Err("No song is running: nothing is counted, so nothing to hold. play_song or jump_to first.".into());
    };
    let state = read_perf_state(live)?;
    let secs = song::sections(&state);
    let cur = song::cursor(&state, &song, &secs);
    let name = cur
        .section
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| song.current.clone());
    let pending = song
        .plan_cue_id
        .and_then(|id| state.cues.iter().find(|c| c.id == id));
    let Some(cue) = pending else {
        let mut s = song.clone();
        s.plan_cue_id = None;
        store_song(live, s);
        return Ok(format!(
            "Nothing to hold: '{name}' loops until you say go already."
        ));
    };
    let next = cue.steps.iter().find(|s| !s.done);
    live.send_command("cancel_cue", Some(json!({"id": cue.id, "reason": "hold"})))
        .map_err(|e| live_err("cancel the plan cue", e))?;
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.cues_cancelled += 1;
        if let Some(s) = pf.song.as_mut() {
            s.plan_cue_id = None;
        }
    }
    Ok(format!(
        "Holding '{name}': it loops from here ({}-bar phrases from bar {}); the counted jump{} off (cue {} cancelled). Say go to continue.",
        cur.section.as_ref().map(|s| s.bars).unwrap_or(16),
        cur.started_bar.unwrap_or(state.bar),
        match next {
            Some(n) => format!(
                " to {} at bar {} is",
                quoted_name(n.label.as_deref().unwrap_or("")),
                n.bar.map(|b| b.to_string()).unwrap_or_else(|| "?".into())
            ),
            None => "s are".to_string(),
        },
        cue.id
    ))
}

/// `fire scene 'Groove · 8' (4 clips: …)` → `Groove · 8`.
fn quoted_name(label: &str) -> String {
    let mut parts = label.split('\'');
    match (parts.next(), parts.next()) {
        (Some(_), Some(name)) => name.to_string(),
        _ => label.to_string(),
    }
}

pub fn go_body(live: &LiveState, p: &SteerParams) -> ToolResult {
    if let Some(running) = performance_running(live) {
        if let Some(song) = &running.song {
            if let Some(id) = song.plan_cue_id {
                let state = read_perf_state(live)?;
                if let Some(cue) = state.cues.iter().find(|c| c.id == id) {
                    let secs = song::sections(&state);
                    let cur = song::cursor(&state, song, &secs);
                    let next = cue.steps.iter().find(|s| !s.done);
                    return Ok(format!(
                        "Nothing is held: the song is running ('{}'{}, next jump bar {}). next_section moves on now; hold_section stops the count.",
                        cur.section.map(|s| s.name).unwrap_or_else(|| song.current.clone()),
                        match cur.repeats {
                            Some(n) => format!(", pass {} of {n}", cur.pass.min(n)),
                            None => String::new(),
                        },
                        next.and_then(|n| n.bar).map(|b| b.to_string()).unwrap_or_else(|| "?".into())
                    ));
                }
            }
        }
    }
    steer(
        live,
        Move {
            verb: "go",
            target: Target::Next,
            repeats: None,
            at: p.at.as_deref(),
            force: p.force,
            transition: None,
        },
    )
}

pub fn next_section_body(live: &LiveState, p: &SteerParams) -> ToolResult {
    steer(
        live,
        Move {
            verb: "next_section",
            target: Target::Next,
            repeats: None,
            at: p.at.as_deref(),
            force: p.force,
            transition: None,
        },
    )
}

pub fn previous_section_body(live: &LiveState, p: &SteerParams) -> ToolResult {
    steer(
        live,
        Move {
            verb: "previous_section",
            target: Target::Previous,
            repeats: None,
            at: p.at.as_deref(),
            force: p.force,
            transition: None,
        },
    )
}

pub fn back_body(live: &LiveState, p: &SteerParams) -> ToolResult {
    steer(
        live,
        Move {
            verb: "back",
            target: Target::Back,
            repeats: None,
            at: p.at.as_deref(),
            force: p.force,
            transition: None,
        },
    )
}

pub fn jump_to_body(live: &LiveState, p: &JumpToParams) -> ToolResult {
    if p.repeats == Some(0) {
        return Err("repeats must be at least 1 (leave it out to loop until go)".into());
    }
    steer(
        live,
        Move {
            verb: "jump_to",
            target: Target::Named(p.section.clone()),
            repeats: p.repeats,
            at: p.at.as_deref(),
            force: p.force,
            transition: p.transition.as_ref().filter(|t| !t.is_null()),
        },
    )
}

/// The song line for the state readout, when a song runs.
pub fn song_lines(live: &LiveState, state: &PerfState) -> Vec<String> {
    let Some(song) = performance_running(live).and_then(|r| r.song) else {
        return Vec::new();
    };
    let secs = song::sections(state);
    let cur = song::cursor(state, &song, &secs);
    vec![song::song_line(&song, &cur)]
}
