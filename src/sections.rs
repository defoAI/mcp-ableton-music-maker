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

// ── Changing a song that exists ─────────────────────────────────────────────
//
// `update_song` is the one verb for "this should be different": the sections,
// the clips inside them, and the setlist. It reads the set once, checks every
// edit against that one read before the first command goes out, writes as
// little as it can, and reports what it did — including the parts of the
// producer's set it did not touch.

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSongParams {
    /// What you want changed, in order: [{"edit": "set_section", "section": "Drop 2", "phrase_bars": 24, "clips": {"Arp": "none"}}, …]
    #[serde(default)]
    pub edits: Vec<Value>,
    /// Or the whole song as it should look — the document get_context {"as": "document"} returns, changed
    #[serde(default)]
    pub document: Option<Value>,
    /// Show the plan and send nothing to Live
    #[serde(default)]
    pub dry_run: bool,
    /// Apply a plan a dry run returned, by its id
    #[serde(default)]
    pub commit: Option<String>,
    /// With `document`: also delete tracks the document does not list (default false — they are left alone and named)
    #[serde(default)]
    pub reconcile_tracks: bool,
}

/// A plan a dry run left behind, so `commit` applies that exact revision or
/// refuses because the set moved.
#[derive(Debug, Clone)]
pub struct StoredPlan {
    pub id: String,
    pub revision: String,
    pub text: String,
    pub scenes: Vec<String>,
    pub edits: Vec<Value>,
    pub document: Option<Value>,
    pub reconcile_tracks: bool,
}

/// One clip to write into a row.
#[derive(Debug, Clone)]
struct ClipWrite {
    track_index: i64,
    track_name: String,
    length: Option<f64>,
    name: Option<String>,
    notes: Vec<tools::Note>,
}

/// What a planned edit will send. Kept apart from the words so a dry run and
/// the commit print the same line.
#[derive(Debug, Clone)]
enum Job {
    /// The scene's name (a rename, a phrase, or both) and its tempo
    Scene {
        index: i64,
        name: Option<String>,
        tempo: Option<f64>,
        /// The rename, for the setlist and the producer's notes
        renamed: Option<(String, String)>,
    },
    /// The clips of one row: everything written in one command, everything
    /// taken out in one batch
    Clips {
        index: i64,
        section: String,
        writes: Vec<ClipWrite>,
        removes: Vec<(i64, String)>,
        varies: Vec<(i64, String, Value)>,
    },
    /// Live's own duplicate: the copy lands directly below its source
    Copy {
        source: i64,
        name: String,
        tempo: Option<f64>,
        changes: Vec<(i64, String, Value)>,
    },
    /// Live has no move for a row: it is written again where it is wanted
    /// and the old one deleted, so its clips are new objects
    Move {
        from: i64,
        to: i64,
        name: String,
    },
    Delete {
        index: i64,
    },
    /// The document's word about where clips sit: what to place, and which
    /// of a track's Arrangement clips the document no longer has
    Placements {
        place: Vec<(i64, i64, Vec<f64>)>,
        unplace: Vec<(i64, String, Vec<usize>)>,
    },
    Locators {
        add: Vec<crate::sets::SetLocator>,
        remove: Vec<crate::sets::SetLocator>,
    },
}

impl Job {
    /// How many commands this sends, for the cost a dry run promises.
    fn round_trips(&self) -> usize {
        match self {
            Job::Scene { .. } => 1,
            Job::Clips {
                writes,
                removes,
                varies,
                ..
            } => {
                usize::from(!writes.is_empty())
                    + usize::from(!removes.is_empty())
                    + varies.len() * 2
            }
            Job::Copy { changes, .. } => 1 + changes.len() * 2,
            Job::Move { .. } => 4,
            Job::Delete { .. } => 1,
            Job::Placements { place, unplace } => place.len() + unplace.len(),
            Job::Locators { add, remove } => add.len() + remove.len(),
        }
    }
}

struct Planned {
    step: song::Step,
    jobs: Vec<Job>,
}

/// Everything one `update_song` call is going to do.
struct Plan {
    planned: Vec<Planned>,
    rows: song::Rows,
    setlist: Vec<SetlistEntry>,
    setlist_changed: bool,
    revision: String,
    scenes: Vec<String>,
}

impl Plan {
    fn round_trips(&self) -> usize {
        1 + self
            .planned
            .iter()
            .flat_map(|p| p.jobs.iter())
            .map(Job::round_trips)
            .sum::<usize>()
            + usize::from(self.setlist_changed)
            + 2 // the undo step the whole revision is written inside
    }
}

/// "none" is the artist's word for a track that should not play in a section;
/// `make_section` has always called it "empty" and both still work.
fn change_of(v: &Value, what: &str) -> Result<Change, String> {
    if let Value::String(s) = v {
        let word = s.trim().to_lowercase();
        if matches!(word.as_str(), "none" | "remove" | "out" | "silent") {
            return Ok(Change::Empty);
        }
    }
    Change::parse(v).map_err(|e| format!("{what}: {e}"))
}

/// The tracks of a row that an edit is not touching, for the reply: the
/// whole point of `set_section` is that they keep their notes.
fn untouched_in_row(row: &song::Row, touched: &[i64]) -> usize {
    row.clip_tracks
        .iter()
        .filter(|t| !touched.contains(t))
        .count()
}

fn scene_name_for(row: &song::Row, rename_to: Option<&str>, bars: Option<i64>) -> String {
    let base = rename_to.unwrap_or(&row.base()).trim().to_string();
    let base = if base.is_empty() { row.base() } else { base };
    match bars.or_else(|| row.bars()) {
        Some(b) => song::section_name(&base, Some(b)),
        None => base,
    }
}

/// Work out everything, against one read of the set, before anything is sent.
fn plan_edits(live: &LiveState, state: &PerfState, edits: &[song::Edit]) -> Result<Plan, String> {
    let mut rows = song::Rows::from_state(state);
    let mut setlist = song::read_setlist(state)?.unwrap_or_default();
    let scenes: Vec<String> = rows.0.iter().map(|r| r.name.clone()).collect();
    let mut plan = Plan {
        planned: Vec::new(),
        rows: rows.clone(),
        setlist: setlist.clone(),
        setlist_changed: false,
        revision: song::state_revision(state),
        scenes,
    };
    let playing = playing_section(live, state, &rows);
    for (i, edit) in edits.iter().enumerate() {
        let n = i + 1;
        let at = |e: String| format!("Nothing was changed. Edit {n} ({}): {e}", edit.verb());
        let planned = plan_one(
            live,
            state,
            &mut rows,
            &mut setlist,
            &mut plan,
            playing.as_deref(),
            n,
            edit,
        )
        .map_err(at)?;
        plan.planned.push(planned);
    }
    plan.rows = rows;
    plan.setlist = setlist;
    Ok(plan)
}

/// The section the running song is in, which may not be deleted or moved
/// out from under the playhead.
fn playing_section(live: &LiveState, state: &PerfState, rows: &song::Rows) -> Option<String> {
    performance_running(live)?;
    let playing = state
        .tracks
        .iter()
        .map(|t| t.playing_slot_index)
        .find(|i| *i >= 0)
        .or(state.current_scene)?;
    rows.0
        .get(playing as usize)
        .filter(|r| !r.reserved())
        .map(|r| r.base())
}

#[allow(clippy::too_many_arguments)]
fn plan_one(
    live: &LiveState,
    state: &PerfState,
    rows: &mut song::Rows,
    setlist: &mut Vec<SetlistEntry>,
    plan: &mut Plan,
    playing: Option<&str>,
    n: usize,
    edit: &song::Edit,
) -> Result<Planned, String> {
    let step = |line: String, no_change: bool| song::Step {
        n,
        verb: edit.verb(),
        line,
        no_change,
    };
    match edit {
        song::Edit::SetSection {
            section,
            rename_to,
            phrase_bars,
            tempo,
            clips,
        } => {
            let at = rows
                .find(section)
                .ok_or_else(|| rows.no_such_section(section))?;
            let row = rows.0[at].clone();
            if let Some(new) = rename_to {
                if song::is_reserved_scene(new) || new.contains(song::SEP) {
                    return Err(format!(
                        "'{new}' cannot be a section name: \"{}\" is the phrase separator, \"{}\" marks the song and \"{}\" marks the parked ideas.",
                        song::SEP.trim(), song::SETLIST_PREFIX, song::STASH_PREFIX
                    ));
                }
                if let Some(other) = rows.find(new) {
                    if other != at {
                        return Err(format!(
                            "'{new}' is already a section (scene {other}). Names are how sections are remembered."
                        ));
                    }
                }
            }
            let name = scene_name_for(&row, rename_to.as_deref(), *phrase_bars);
            let renamed = rename_to
                .as_ref()
                .map(|to| (row.base(), to.trim().to_string()))
                .filter(|(from, to)| !from.eq_ignore_ascii_case(to));
            // The clips, checked against the set before anything is sent.
            let mut writes = Vec::new();
            let mut removes = Vec::new();
            let mut varies = Vec::new();
            let mut touched: Vec<i64> = Vec::new();
            let mut words: Vec<String> = Vec::new();
            for (track, change) in clips {
                let t = state.track_by(&json!(track))?;
                let has_clip = row.clip_tracks.contains(&t.index);
                touched.push(t.index);
                match change_of(change, &format!("clips for '{track}'"))? {
                    Change::Empty => {
                        if has_clip {
                            removes.push((t.index, t.name.clone()));
                            words.push(format!(
                                "{} taken out (its clip is deleted, so {} is silent when {} fires)",
                                t.name,
                                t.name,
                                row.base()
                            ));
                        } else {
                            words.push(format!("{} was already not in this section", t.name));
                        }
                    }
                    Change::Notes(input) => {
                        let notes = crate::notes::expand(&input)
                            .map_err(|e| format!("clips for '{track}': {e}"))?;
                        if notes.is_empty() {
                            return Err(format!(
                                "clips for '{track}': no notes (notes, notes_csv, steps or patterns) — write \"none\" to take the track out of the section"
                            ));
                        }
                        words.push(format!(
                            "{} rewritten ({})",
                            t.name,
                            plural(notes.len(), "note")
                        ));
                        writes.push(ClipWrite {
                            track_index: t.index,
                            track_name: t.name.clone(),
                            length: None,
                            name: None,
                            notes,
                        });
                    }
                    other => {
                        if !has_clip {
                            return Err(format!(
                                "'{}' has no clip in '{}' to change; give it notes instead",
                                t.name,
                                row.base()
                            ));
                        }
                        words.push(match &other {
                            Change::Vary(v) => format!("{} {}", t.name, variation_past(v)),
                            Change::Transpose(k) => format!("{} transposed {k:+}", t.name),
                            _ => t.name.clone(),
                        });
                        varies.push((t.index, t.name.clone(), change.clone()));
                    }
                }
            }
            let scene_changes = name != row.name || tempo.is_some_and(|t| Some(t) != row.tempo);
            let mut line = String::new();
            if let Some((from, to)) = &renamed {
                line.push_str(&format!("{from} → {to}"));
            } else {
                line.push_str(&row.base());
            }
            line.push_str(": ");
            let mut parts: Vec<String> = Vec::new();
            if phrase_bars.is_some() && row.bars() != *phrase_bars {
                parts.push(format!(
                    "phrase {} → {} bars",
                    row.bars()
                        .map(|b| b.to_string())
                        .unwrap_or_else(|| "the 16-bar default".into()),
                    phrase_bars.unwrap_or_default()
                ));
            }
            if let Some(t) = tempo {
                parts.push(format!("scene tempo {t}"));
            }
            parts.extend(words);
            if parts.is_empty() {
                parts.push("nothing to change".into());
            }
            line.push_str(&parts.join(", "));
            if !touched.is_empty() {
                let keep = untouched_in_row(&row, &touched);
                if keep > 0 {
                    line.push_str(&format!("; {} untouched", plural(keep, "clip")));
                }
            }
            let mut jobs = Vec::new();
            if scene_changes {
                jobs.push(Job::Scene {
                    index: at as i64,
                    name: (name != row.name).then(|| name.clone()),
                    tempo: tempo.filter(|t| Some(*t) != row.tempo),
                    renamed: renamed.clone(),
                });
            }
            if !writes.is_empty() || !removes.is_empty() || !varies.is_empty() {
                jobs.push(Job::Clips {
                    index: at as i64,
                    section: row.base(),
                    writes,
                    removes,
                    varies,
                });
            }
            // The model follows, so the next edit sees this one.
            rows.0[at].name = name;
            if let Some(t) = tempo {
                rows.0[at].tempo = Some(*t);
            }
            if let Some((from, to)) = &renamed {
                for e in setlist.iter_mut() {
                    if e.section.eq_ignore_ascii_case(from) {
                        e.section = to.clone();
                        plan.setlist_changed = true;
                    }
                }
            }
            let no_change = jobs.is_empty();
            Ok(Planned {
                step: step(line, no_change),
                jobs,
            })
        }
        song::Edit::CopySection {
            section,
            name,
            after,
            changes,
        } => {
            let at = rows
                .find(section)
                .ok_or_else(|| rows.no_such_section(section))?;
            if rows.find(name).is_some() {
                return Err(format!(
                    "'{name}' is already a section. Names are how sections are remembered; call the copy '{name} 2'."
                ));
            }
            if song::is_reserved_scene(name) || name.contains(song::SEP) {
                return Err(format!("'{name}' cannot be a section name."));
            }
            let row = rows.0[at].clone();
            let mut planned_changes = Vec::new();
            let mut words = Vec::new();
            for (track, change) in changes {
                let t = state.track_by(&json!(track))?;
                let c = change_of(change, &format!("changes for '{track}'"))?;
                if !row.clip_tracks.contains(&t.index)
                    && !matches!(c, Change::Notes(_) | Change::Empty)
                {
                    return Err(format!(
                        "'{}' has no clip in '{}' to change",
                        t.name,
                        row.base()
                    ));
                }
                words.push(match &c {
                    Change::Empty => format!("{} left out", t.name),
                    Change::Vary(v) => format!("{} {}", t.name, variation_past(v)),
                    Change::Transpose(k) => format!("{} transposed {k:+}", t.name),
                    Change::Notes(_) => format!("{} rewritten", t.name),
                });
                planned_changes.push((t.index, t.name.clone(), change.clone()));
            }
            let bars = row.bars();
            let scene_name = song::section_name(name, bars);
            let mut jobs = vec![Job::Copy {
                source: at as i64,
                name: scene_name.clone(),
                tempo: row.tempo,
                changes: planned_changes,
            }];
            // Live's duplicate lands directly below its source; anywhere else
            // is a move, which is a rewrite of the row.
            let landed = at + 1;
            rows.insert(
                landed,
                song::Row {
                    name: scene_name.clone(),
                    tempo: row.tempo,
                    clip_tracks: row.clip_tracks.clone(),
                },
            );
            let mut line = format!(
                "'{scene_name}' created below {} as a copy of it ({})",
                row.base(),
                plural(row.clip_tracks.len(), "clip")
            );
            if !words.is_empty() {
                line.push_str(&format!(", with {}", words.join(", ")));
            }
            if let Some(a) = after
                .as_deref()
                .filter(|a| !a.eq_ignore_ascii_case(&row.base()))
            {
                let to = rows
                    .find(a)
                    .ok_or_else(|| format!("after: {}", rows.no_such_section(a)))?;
                let to = if to > landed { to } else { to + 1 };
                jobs.push(Job::Move {
                    from: landed as i64,
                    to: to as i64,
                    name: scene_name.clone(),
                });
                let moved = rows.remove(landed);
                let to = if to > landed { to - 1 } else { to };
                rows.insert(to, moved);
                line.push_str(&format!(", then moved after {a}"));
            }
            Ok(Planned {
                step: step(line, false),
                jobs,
            })
        }
        song::Edit::MoveSection {
            section,
            after,
            before,
        } => {
            let at = rows
                .find(section)
                .ok_or_else(|| rows.no_such_section(section))?;
            let row = rows.0[at].clone();
            if playing.is_some_and(|p| p.eq_ignore_ascii_case(&row.base())) {
                return Err(playing_refusal(&row.base(), "moved"));
            }
            let to = rows.place_after(after.as_deref(), before.as_deref(), true)?;
            if to == at || to == at + 1 {
                return Ok(Planned {
                    step: step(format!("'{}' is already there", row.base()), true),
                    jobs: Vec::new(),
                });
            }
            let moved = rows.remove(at);
            let landing = if to > at { to - 1 } else { to };
            rows.insert(landing, moved);
            Ok(Planned {
                step: step(
                    format!(
                        "'{}' moved (Live has no move for a row: it is written again where you asked and the old one deleted, so its clips are new objects — Arrangement clips already placed from it are untouched)",
                        row.base()
                    ),
                    false,
                ),
                jobs: vec![Job::Move {
                    from: at as i64,
                    to: to as i64,
                    name: row.name.clone(),
                }],
            })
        }
        song::Edit::DeleteSection { section } => {
            let at = rows
                .find(section)
                .ok_or_else(|| rows.no_such_section(section))?;
            let row = rows.0[at].clone();
            if playing.is_some_and(|p| p.eq_ignore_ascii_case(&row.base())) {
                return Err(playing_refusal(&row.base(), "deleted"));
            }
            if rows.section_count() <= 1 {
                return Err(format!(
                    "'{}' is the only section in the set; deleting it would leave nothing to play.",
                    row.base()
                ));
            }
            let gone: Vec<usize> = song::occurrences(setlist, &row.base());
            if !gone.is_empty() {
                if setlist.len() == gone.len() {
                    return Err(format!(
                        "'{}' is the whole song ({}). Put another section in the song first, or set_setlist.",
                        row.base(),
                        song::setlist_text(setlist)
                    ));
                }
                setlist.retain(|e| !e.section.eq_ignore_ascii_case(&row.base()));
                plan.setlist_changed = true;
            }
            let notes = crate::memory::notes_about_section(live, &row.base());
            rows.remove(at);
            let mut line = format!(
                "'{}' deleted ({} gone with it)",
                row.base(),
                plural(row.clip_tracks.len(), "clip")
            );
            if !gone.is_empty() {
                line.push_str(&format!(
                    ", and {} out of the song",
                    plural(gone.len(), "entry")
                ));
            }
            if notes > 0 {
                line.push_str(&format!(
                    "; your {} about it {} kept (remember still has them)",
                    plural(notes, "note"),
                    if notes == 1 { "is" } else { "are" }
                ));
            }
            Ok(Planned {
                step: step(line, false),
                jobs: vec![Job::Delete { index: at as i64 }],
            })
        }
        song::Edit::InsertEntry {
            section,
            after,
            before,
            repeats,
        } => {
            let at = rows
                .find(section)
                .ok_or_else(|| rows.no_such_section(section))?;
            let name = rows.0[at].base();
            let position = match (after, before) {
                (Some(_), Some(_)) => return Err("give after or before, not both".into()),
                (Some(a), None) => song::anchor_position(setlist, a, "after")? + 1,
                (None, Some(b)) => song::anchor_position(setlist, b, "before")?,
                (None, None) => setlist.len(),
            };
            setlist.insert(
                position,
                SetlistEntry {
                    section: name.clone(),
                    repeats: *repeats,
                },
            );
            plan.setlist_changed = true;
            Ok(Planned {
                step: step(
                    format!(
                        "{name}{} into the song at position {}",
                        repeats.map(|r| format!("×{r}")).unwrap_or_default(),
                        position + 1
                    ),
                    false,
                ),
                jobs: Vec::new(),
            })
        }
        song::Edit::SetEntry {
            section,
            occurrence,
            repeats,
            after,
            before,
        } => {
            let which = song::pick_entries(setlist, section, *occurrence, false, "set_entry")?;
            let i = which[0];
            let mut words: Vec<String> = Vec::new();
            let was = setlist[i].repeats;
            if let Some(r) = repeats {
                if *r != was {
                    setlist[i].repeats = *r;
                    words.push(match r {
                        Some(n) => format!(
                            "×{n} (was {})",
                            was.map(|w| format!("×{w}"))
                                .unwrap_or_else(|| "loop until go".into())
                        ),
                        None => "loops until go".to_string(),
                    });
                }
            }
            match (after, before) {
                (Some(_), Some(_)) => return Err("give after or before, not both".into()),
                (Some(a), None) | (None, Some(a)) => {
                    let anchor_at = song::anchor_position(
                        setlist,
                        a,
                        if after.is_some() { "after" } else { "before" },
                    )?;
                    let target = if after.is_some() {
                        anchor_at + 1
                    } else {
                        anchor_at
                    };
                    if target != i && target != i + 1 {
                        let entry = setlist.remove(i);
                        let target = if target > i { target - 1 } else { target };
                        setlist.insert(target, entry);
                        words.push(format!(
                            "{} {}",
                            if after.is_some() { "after" } else { "before" },
                            a.section
                        ));
                    }
                }
                (None, None) => {}
            }
            if words.is_empty() {
                return Ok(Planned {
                    step: step(format!("{section} is already like that"), true),
                    jobs: Vec::new(),
                });
            }
            plan.setlist_changed = true;
            Ok(Planned {
                step: step(format!("{section} {}", words.join(", ")), false),
                jobs: Vec::new(),
            })
        }
        song::Edit::RemoveEntry {
            section,
            occurrence,
            all,
        } => {
            let which = song::pick_entries(setlist, section, *occurrence, *all, "remove_entry")?;
            if which.len() == setlist.len() {
                return Err(format!(
                    "That would leave the song empty. set_setlist writes a new song, or leave one entry of '{section}' in."
                ));
            }
            for i in which.iter().rev() {
                setlist.remove(*i);
            }
            plan.setlist_changed = true;
            Ok(Planned {
                step: step(
                    format!(
                        "{} of '{section}' out of the song",
                        plural(which.len(), "entry")
                    ),
                    false,
                ),
                jobs: Vec::new(),
            })
        }
        song::Edit::SetSetlist { setlist: wanted } => {
            let names = rows.section_names();
            if let Some(bad) = song::unknown_entry(wanted, &names) {
                return Err(format!("{bad}. Sections: {}.", names.join(", ")));
            }
            if wanted.is_empty() {
                return Err("A song needs at least one section.".into());
            }
            let same = *wanted == *setlist;
            *setlist = wanted.clone();
            plan.setlist_changed = plan.setlist_changed || !same;
            Ok(Planned {
                step: step(
                    if same {
                        "the song already reads like that".to_string()
                    } else {
                        song::setlist_text(wanted)
                    },
                    same,
                ),
                jobs: Vec::new(),
            })
        }
    }
}

fn playing_refusal(name: &str, what: &str) -> String {
    format!(
        "'{name}' is playing now, and a row cannot be {what} out from under the playhead — the song would stop. go to another section first, or end_performance."
    )
}

// ── Writing it ──────────────────────────────────────────────────────────────

/// The whole revision inside one of Live's undo steps, so the producer's
/// Cmd+Z means "that change" and not "the last third of that change". The
/// script opens one per mutating command; `run` is not one of those, so the
/// batch can hold the step open around all of them.
struct UndoStep<'a> {
    live: &'a LiveState,
    open: bool,
}

impl<'a> UndoStep<'a> {
    fn open(live: &'a LiveState) -> Self {
        let open = require(live, "run").is_ok()
            && crate::lom::Batch::new()
                .push(crate::lom::Op::call(
                    &crate::lom::Path::song().attr("begin_undo_step"),
                    Vec::new(),
                ))
                .run(live)
                .is_ok();
        Self { live, open }
    }

    fn close(&mut self) {
        if self.open {
            let _ = crate::lom::Batch::new()
                .push(crate::lom::Op::call(
                    &crate::lom::Path::song().attr("end_undo_step"),
                    Vec::new(),
                ))
                .run(self.live);
            self.open = false;
        }
    }
}

impl Drop for UndoStep<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

fn run_job(live: &LiveState, state: &PerfState, job: &Job) -> Result<(), String> {
    match job {
        Job::Scene {
            index,
            name,
            tempo,
            renamed,
        } => {
            require(live, "set_scene")?;
            let mut params = json!({"index": index});
            if let Some(n) = name {
                params["name"] = json!(n);
            }
            if let Some(t) = tempo {
                params["tempo"] = json!(t);
            }
            live.send_command("set_scene", Some(params))
                .map_err(|e| live_err("rename the section", e))?;
            if let Some((from, to)) = renamed {
                crate::memory::rename_section(live, from, to);
            }
            Ok(())
        }
        Job::Clips {
            index,
            section,
            writes,
            removes,
            varies,
        } => {
            if !writes.is_empty() {
                require(live, "write_clips")?;
                let specs: Vec<Value> = writes
                    .iter()
                    .map(|w| {
                        json!({
                            "track_index": w.track_index,
                            "clip_index": index,
                            "length": clip_length(&w.notes, state.beats_per_bar(), w.length),
                            "name": w.name.clone().unwrap_or_else(|| format!("{section}/{}", w.track_name)),
                            "notes": w.notes,
                        })
                    })
                    .collect();
                live.send_command("write_clips", Some(json!({"clips": specs})))
                    .map_err(|e| live_err("write the section's clips", e))?;
                for w in writes {
                    live.sets.forget_clip(&w.track_name, *index);
                }
            }
            if !removes.is_empty() {
                require(live, "delete_clip")?;
                for (track_index, track_name) in removes {
                    live.send_command(
                        "delete_clip",
                        Some(json!({"track_index": track_index, "clip_index": index})),
                    )
                    .map_err(|e| live_err("take the track out of the section", e))?;
                    live.sets.forget_clip(track_name, *index);
                }
            }
            for (track_index, track_name, change) in varies {
                let parsed = change_of(change, track_name)?;
                apply_change(
                    live,
                    state,
                    &Slot {
                        track_index: *track_index,
                        slot: *index,
                        has_clip: true,
                        section,
                        track_name,
                    },
                    &parsed,
                )?;
                live.sets.forget_clip(track_name, *index);
            }
            Ok(())
        }
        Job::Copy {
            source,
            name,
            tempo,
            changes,
        } => {
            require(live, "duplicate_scene")?;
            let r = live
                .send_command(
                    "duplicate_scene",
                    Some(json!({"index": source, "name": name})),
                )
                .map_err(|e| live_err("copy the section", e))?;
            let index = r.get("index").and_then(Value::as_i64).unwrap_or(source + 1);
            let copied = clip_track_indices(&r);
            // A new row moves every row under it: what the cache knows about
            // a clip is keyed on its slot, so none of it is true any more.
            live.sets.forget_all();
            for (track_index, track_name, change) in changes {
                let parsed = change_of(change, track_name)?;
                apply_change(
                    live,
                    state,
                    &Slot {
                        track_index: *track_index,
                        slot: index,
                        has_clip: copied.contains(track_index),
                        section: name,
                        track_name,
                    },
                    &parsed,
                )?;
            }
            if let Some(t) = tempo {
                set_tempo_if_given(live, index, Some(*t))?;
            }
            Ok(())
        }
        Job::Move { from, to, name } => move_row(live, state, *from, *to, name),
        Job::Placements { place, unplace } => {
            for (track_index, name, indices) in unplace {
                require(live, "delete_arrangement_clips")?;
                live.send_command(
                    "delete_arrangement_clips",
                    Some(json!({"track_index": track_index, "indices": indices})),
                )
                .map_err(|e| live_err(&format!("take {name}'s old placements out"), e))?;
            }
            for (track_index, slot, times) in place {
                require(live, "place_clips")?;
                live.send_command(
                    "place_clips",
                    Some(json!({"track_index": track_index, "clip_index": slot, "times": times})),
                )
                .map_err(|e| live_err("place the clip in the Arrangement", e))?;
            }
            Ok(())
        }
        Job::Locators { add, remove } => {
            for l in remove {
                require(live, "delete_locator")?;
                live.send_command("delete_locator", Some(json!({"time": l.time})))
                    .map_err(|e| live_err("remove a locator", e))?;
            }
            for l in add {
                require(live, "create_locator")?;
                live.send_command(
                    "create_locator",
                    Some(json!({"time": l.time, "name": l.name})),
                )
                .map_err(|e| live_err("set a locator", e))?;
            }
            Ok(())
        }
        Job::Delete { index } => {
            require(live, "run")?;
            crate::lom::Batch::new()
                .push(crate::lom::Op::call(
                    &crate::lom::Path::song().attr("delete_scene"),
                    vec![json!(index)],
                ))
                .run(live)
                .map_err(|e| format!("Could not delete the section: {e}"))?;
            // Deleting a row moves every row under it up one, so every slot
            // the cache is keyed on below this one is now a different clip.
            live.sets.forget_all();
            Ok(())
        }
    }
}

/// Live has no move for a scene row, so the row is written again where it is
/// wanted and the old one deleted. Its clips are new objects, which the reply
/// says.
fn move_row(
    live: &LiveState,
    state: &PerfState,
    from: i64,
    to: i64,
    name: &str,
) -> Result<(), String> {
    require(live, "create_scene")?;
    require(live, "run")?;
    // Everything the old row holds, in one round trip plus one read per clip.
    let tracks: Vec<(i64, String)> = state
        .tracks
        .iter()
        .filter(|t| t.slots_with_clips.contains(&from))
        .map(|t| (t.index, t.name.clone()))
        .collect();
    let mut batch = crate::lom::Batch::new();
    for (ti, _) in &tracks {
        let clip = crate::lom::Path::track(*ti)
            .attr("clip_slots")
            .at(from)
            .attr("clip");
        batch = batch
            .push(crate::lom::Op::get(&clip, &format!("name{ti}")))
            .push(crate::lom::Op::get(&clip, &format!("len{ti}")));
    }
    let info = if tracks.is_empty() {
        serde_json::Map::new()
    } else {
        batch.run(live)?
    };
    let mut specs: Vec<Value> = Vec::new();
    for (ti, tname) in &tracks {
        let notes = clip_notes(live, *ti, from)?;
        if notes.is_empty() {
            return Err(format!(
                "'{tname}' has an audio clip in this row, and Live cannot write an audio clip from notes — a row with audio in it cannot be moved. Leave it where it is, or move it in Live."
            ));
        }
        specs.push(json!({
            "track_index": ti,
            "clip_index": to,
            "length": info.get(&format!("len{ti}")).and_then(Value::as_f64).unwrap_or(4.0),
            "name": info.get(&format!("name{ti}")).and_then(Value::as_str).unwrap_or(tname),
            "notes": notes,
        }));
    }
    live.send_command("create_scene", Some(json!({"index": to, "name": name})))
        .map_err(|e| live_err("write the row where you asked for it", e))?;
    // Inserting above the old row moves it down one.
    let old = if to <= from { from + 1 } else { from };
    if !specs.is_empty() {
        require(live, "write_clips")?;
        live.send_command("write_clips", Some(json!({"clips": specs})))
            .map_err(|e| live_err("write the moved row's clips", e))?;
    }
    crate::lom::Batch::new()
        .push(crate::lom::Op::call(
            &crate::lom::Path::song().attr("delete_scene"),
            vec![json!(old)],
        ))
        .run(live)
        .map_err(|e| format!("Could not delete the row it moved from: {e}"))?;
    live.sets.forget_all();
    Ok(())
}

// ── The document path ───────────────────────────────────────────────────────
//
// A whole document posted back is not a second way to edit a set: it is
// compiled into the same edits, and what an edit cannot say — where clips sit
// in the Arrangement, and the locators — becomes two more jobs on the same
// plan. One executor, one set of refusals, one reply.

fn resolved_notes(notes: &[tools::Note]) -> Vec<(i64, i64, i64, i64, bool)> {
    crate::notes::resolved(notes)
}

/// What a posted document compiles to: the edits, the Arrangement work an
/// edit cannot say, and the tracks it left alone.
type Compiled = (Vec<song::Edit>, Vec<Planned>, Vec<String>);

/// The document as the producer sent it, against the set as it is now.
fn compile_document(
    live: &LiveState,
    state: &PerfState,
    posted: &Value,
    reconcile_tracks: bool,
) -> Result<Compiled, String> {
    let doc: crate::sets::SetDocument = serde_json::from_value(posted.clone()).map_err(|e| {
        format!("That is not a song document ({e}). get_context {{\"as\": \"document\"}} returns the shape update_song takes back.")
    })?;
    let (now, _cost) = crate::sets::read_set(live, "current")?;
    if let Some(sent) = doc.revision.as_deref() {
        if let Some(here) = now.revision.as_deref() {
            if sent != here {
                return Err(format!(
                    "Nothing was changed. This document was read when the set was {sent}; it is {here} now — something changed in Live since. Read it again with get_context {{\"as\": \"document\"}} and post that back, so a change you did not make is not undone."
                ));
            }
        }
    }
    let mut edits: Vec<song::Edit> = Vec::new();
    let mut extra: Vec<Planned> = Vec::new();
    let mut left_alone: Vec<String> = Vec::new();
    let rows = song::Rows::from_state(state);
    // Tracks: a document never creates one, and only removes one if asked.
    let here: Vec<String> = now.tracks.iter().map(|t| t.name.clone()).collect();
    for t in &doc.tracks {
        if !here.iter().any(|n| n.eq_ignore_ascii_case(&t.name)) {
            return Err(format!(
                "Nothing was changed. The document has a track '{}' the set does not: update_song changes what is there. build_song adds tracks.",
                t.name
            ));
        }
    }
    for name in &here {
        if !doc.tracks.iter().any(|t| t.name.eq_ignore_ascii_case(name)) {
            if reconcile_tracks {
                return Err(format!(
                    "Nothing was changed. reconcile_tracks would delete '{name}' with everything on it. delete_track does that one track at a time, so it is never a side effect of posting a document."
                ));
            }
            left_alone.push(name.clone());
        }
    }
    // Sections: the phrase, the scene tempo, and rows the document dropped.
    for sec in &doc.sections {
        let Some(at) = rows.find(&sec.name) else {
            return Err(format!(
                "Nothing was changed. The document has a section '{}' the set does not: make_section creates one, update_song changes what exists.",
                sec.name
            ));
        };
        let row = &rows.0[at];
        let wants_bars = sec.phrase_bars.filter(|b| Some(*b) != row.bars());
        let wants_tempo = sec.tempo.filter(|t| Some(*t) != row.tempo);
        if wants_bars.is_some() || wants_tempo.is_some() {
            edits.push(song::Edit::SetSection {
                section: row.base(),
                rename_to: None,
                phrase_bars: wants_bars,
                tempo: wants_tempo,
                clips: BTreeMap::new(),
            });
        }
    }
    let dropped: Vec<String> = rows
        .0
        .iter()
        .filter(|r| !r.reserved())
        .map(|r| r.base())
        .filter(|name| {
            !doc.sections
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(name))
        })
        .collect();
    // Clips, row by row, inside the tracks the document lists.
    let mut per_row: BTreeMap<i64, BTreeMap<String, Value>> = BTreeMap::new();
    for clip in &doc.clips {
        if dropped
            .iter()
            .any(|d| rows.find(d).is_some_and(|i| i as i64 == clip.slot))
        {
            continue;
        }
        let wanted = resolved_notes(&clip.notes());
        let current = now
            .clips
            .iter()
            .find(|c| c.track.eq_ignore_ascii_case(&clip.track) && c.slot == clip.slot);
        let same = current.is_some_and(|c| {
            resolved_notes(&c.notes()) == wanted
                && c.name == clip.name
                && (c.length - clip.length).abs() < 1e-6
        });
        if same || wanted.is_empty() {
            continue;
        }
        let mut form = serde_json::to_value(&clip.notes).unwrap_or(json!({}));
        if let Some(o) = form.as_object_mut() {
            o.insert("length".into(), json!(clip.length));
            if !clip.name.is_empty() {
                o.insert("name".into(), json!(clip.name));
            }
        }
        per_row
            .entry(clip.slot)
            .or_default()
            .insert(clip.track.clone(), form);
    }
    for c in &now.clips {
        let listed = doc
            .tracks
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case(&c.track));
        let in_doc = doc
            .clips
            .iter()
            .any(|d| d.track.eq_ignore_ascii_case(&c.track) && d.slot == c.slot);
        if listed && !in_doc && c.file_path.is_none() {
            per_row
                .entry(c.slot)
                .or_default()
                .insert(c.track.clone(), json!("none"));
        }
    }
    for (slot, clips) in per_row {
        let Some(row) = rows.0.get(slot as usize).filter(|r| !r.reserved()) else {
            continue;
        };
        edits.push(song::Edit::SetSection {
            section: row.base(),
            rename_to: None,
            phrase_bars: None,
            tempo: None,
            clips,
        });
    }
    for name in dropped {
        edits.push(song::Edit::DeleteSection { section: name });
    }
    // The song.
    let here_setlist = song::read_setlist(state)?.unwrap_or_default();
    if doc.setlist != here_setlist && !doc.setlist.is_empty() {
        edits.push(song::Edit::SetSetlist {
            setlist: doc.setlist.clone(),
        });
    }
    // The Arrangement: positions only, and only for the tracks the document
    // lists. `arrange` still owns the Arrangement as an intent; this is the
    // document keeping its word.
    if let Some(step) = placement_step(state, &doc, &now, edits.len() + 1)? {
        extra.push(step);
    }
    if let Some(step) = locator_step(&doc, &now, edits.len() + 2) {
        extra.push(step);
    }
    Ok((edits, extra, left_alone))
}

fn beats_of(p: &crate::sets::SetPlacement) -> Vec<(i64, f64)> {
    p.beats()
        .into_iter()
        .map(|t| ((t * 960.0).round() as i64, t))
        .collect()
}

fn placement_step(
    state: &PerfState,
    doc: &crate::sets::SetDocument,
    now: &crate::sets::SetDocument,
    n: usize,
) -> Result<Option<Planned>, String> {
    let mut place: Vec<(i64, i64, Vec<f64>)> = Vec::new();
    let mut unplace: Vec<(i64, String, Vec<usize>)> = Vec::new();
    let mut created = 0usize;
    let mut deleted = 0usize;
    for track in &doc.tracks {
        let Ok(t) = state.track_by(&json!(track.name)) else {
            continue;
        };
        let wanted: BTreeMap<i64, (i64, f64)> = doc
            .placements
            .iter()
            .filter(|p| p.track.eq_ignore_ascii_case(&track.name))
            .flat_map(|p| beats_of(p).into_iter().map(move |(k, v)| (k, (p.slot, v))))
            .collect();
        let current: BTreeMap<i64, (i64, f64)> = now
            .placements
            .iter()
            .filter(|p| p.track.eq_ignore_ascii_case(&track.name))
            .flat_map(|p| beats_of(p).into_iter().map(move |(k, v)| (k, (p.slot, v))))
            .collect();
        if wanted == current {
            continue;
        }
        let mut by_slot: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        for (tick, (slot, beat)) in &wanted {
            if !current.contains_key(tick) {
                by_slot.entry(*slot).or_default().push(*beat);
            }
        }
        for (slot, times) in by_slot {
            created += times.len();
            place.push((t.index, slot, times));
        }
        // Which of the track's Arrangement clips the document no longer has,
        // by their position in the track, which is what the script deletes by.
        let stale: Vec<usize> = now
            .placements
            .iter()
            .filter(|p| p.track.eq_ignore_ascii_case(&track.name))
            .flat_map(|p| p.beats())
            .enumerate()
            .filter(|(_, beat)| !wanted.contains_key(&((*beat * 960.0).round() as i64)))
            .map(|(i, _)| i)
            .collect();
        if !stale.is_empty() {
            deleted += stale.len();
            unplace.push((t.index, track.name.clone(), stale));
        }
    }
    if place.is_empty() && unplace.is_empty() {
        return Ok(None);
    }
    Ok(Some(Planned {
        step: song::Step {
            n,
            verb: "placements",
            line: format!(
                "{} created, {} deleted in the Arrangement",
                created, deleted
            ),
            no_change: false,
        },
        jobs: vec![Job::Placements { place, unplace }],
    }))
}

fn locator_step(
    doc: &crate::sets::SetDocument,
    now: &crate::sets::SetDocument,
    n: usize,
) -> Option<Planned> {
    let key = |l: &crate::sets::SetLocator| ((l.time * 960.0).round() as i64, l.name.clone());
    let wanted: Vec<(i64, String)> = doc.locators.iter().map(key).collect();
    let here: Vec<(i64, String)> = now.locators.iter().map(key).collect();
    if wanted == here {
        return None;
    }
    let add: Vec<crate::sets::SetLocator> = doc
        .locators
        .iter()
        .filter(|l| !here.contains(&key(l)))
        .cloned()
        .collect();
    let remove: Vec<crate::sets::SetLocator> = now
        .locators
        .iter()
        .filter(|l| !wanted.contains(&key(l)))
        .cloned()
        .collect();
    if add.is_empty() && remove.is_empty() {
        return None;
    }
    Some(Planned {
        step: song::Step {
            n,
            verb: "locators",
            line: format!("{} set, {} removed", add.len(), remove.len()),
            no_change: false,
        },
        jobs: vec![Job::Locators { add, remove }],
    })
}

// ── The tool ────────────────────────────────────────────────────────────────

fn plan_id(revision: &str, text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{revision}{text}").as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("plan-{h:016x}")[..9].to_string()
}

/// What the set still has that this call did not touch — said, not assumed.
fn untouched_line(state: &PerfState, left_alone: &[String]) -> String {
    let mut parts = vec![format!(
        "{} and every device and the mixer",
        plural(state.tracks.len(), "track")
    )];
    if !left_alone.is_empty() {
        parts.push(format!(
            "{} the document does not list ({})",
            plural(left_alone.len(), "track"),
            left_alone.join(", ")
        ));
    }
    format!("Untouched: {}.", parts.join("; "))
}

fn plan_text(live: &LiveState, plan: &Plan, secs_line: &str, id: &str) -> String {
    let mut out = format!(
        "Plan — nothing was sent to Live. The set is {}.\n",
        plan.revision
    );
    for p in &plan.planned {
        out.push_str(&p.step.text());
        if p.step.no_change {
            out.push_str("  (no change — nothing would be sent)");
        }
        out.push('\n');
    }
    out.push_str(secs_line);
    let trips = plan.round_trips();
    out.push_str(&format!(
        "\nCost: {} round trip{}, about {:.1} s at this session's measured round trip ({:.2} s).\n",
        trips,
        if trips == 1 { "" } else { "s" },
        trips as f64 * live.round_trip_s(),
        live.round_trip_s()
    ));
    out.push_str(&format!(
        "Run it with update_song {{\"commit\": \"{id}\"}}, or send the same edits without dry_run."
    ));
    out
}

pub fn update_song_body(live: &LiveState, p: &UpdateSongParams) -> ToolResult {
    let mut edits_json = p.edits.clone();
    let mut document = p.document.clone();
    let mut reconcile = p.reconcile_tracks;
    let mut expect: Option<StoredPlan> = None;
    if let Some(id) = p.commit.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let stored = live
            .plan
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|s| s.id == id)
            .ok_or_else(|| format!(
                "No plan called '{id}' is waiting. A plan lives until the next one is made or the server restarts; send the edits again with dry_run: true to see it, or without dry_run to do it."
            ))?;
        edits_json = stored.edits.clone();
        document = stored.document.clone();
        reconcile = stored.reconcile_tracks;
        expect = Some(stored);
    }
    if !edits_json.is_empty() && document.is_some() {
        return Err("Give `edits` (what you want changed) or `document` (what the song should look like), not both.".into());
    }
    if edits_json.is_empty() && document.is_none() {
        return Err("Nothing to do: give `edits`, e.g. [{\"edit\": \"set_section\", \"section\": \"Drop\", \"phrase_bars\": 24}], or `document`. get_context {\"as\": \"song\"} shows what there is to change.".into());
    }
    let state = read_perf_state(live)?;
    let revision = song::state_revision(&state);
    if let Some(stored) = &expect {
        if stored.revision != revision {
            let now: Vec<String> = state.scenes.iter().map(|s| s.name.clone()).collect();
            return Err(format!(
                "Nothing was changed. The set has changed since this plan was made ({} → {}): {}. Send the same edits again and I will plan against the set as it is now.",
                stored.revision,
                revision,
                what_moved(&stored.scenes, &now)
            ));
        }
    }
    let mut left_alone: Vec<String> = Vec::new();
    let mut extra: Vec<Planned> = Vec::new();
    let edits: Vec<song::Edit> = match &document {
        Some(doc) => {
            let (edits, jobs, alone) = compile_document(live, &state, doc, reconcile)?;
            extra = jobs;
            left_alone = alone;
            edits
        }
        None => {
            let mut out = Vec::new();
            for (i, raw) in edits_json.iter().enumerate() {
                out.push(
                    song::Edit::parse(raw)
                        .map_err(|e| format!("Nothing was changed. Edit {}: {e}", i + 1))?,
                );
            }
            out
        }
    };
    let mut plan = plan_edits(live, &state, &edits)?;
    let first_extra = plan.planned.len() + 1;
    for (i, mut step) in extra.into_iter().enumerate() {
        step.step.n = first_extra + i;
        plan.planned.push(step);
    }
    if plan.planned.is_empty() {
        return Ok(format!(
            "Nothing to do: the set already reads like that. {}",
            song_line(&plan)
        ));
    }
    let song_after = song_line(&plan);
    if p.dry_run {
        let text = {
            let id = plan_id(&revision, &song_after);
            let body = plan_text(live, &plan, &song_after, &id);
            *live.plan.lock().unwrap_or_else(|e| e.into_inner()) = Some(StoredPlan {
                id,
                revision: revision.clone(),
                text: body.clone(),
                scenes: plan.scenes.clone(),
                edits: edits_json.clone(),
                document: document.clone(),
                reconcile_tracks: reconcile,
            });
            body
        };
        let mut out = text;
        out.push('\n');
        out.push_str(&untouched_line(&state, &left_alone));
        return Ok(out);
    }
    apply(live, &state, plan, &left_alone)
}

/// "a scene was added ('Idea 3', now scene 4)", for a plan the set moved
/// under.
fn what_moved(before: &[String], now: &[String]) -> String {
    let added: Vec<&String> = now.iter().filter(|n| !before.contains(n)).collect();
    let gone: Vec<&String> = before.iter().filter(|n| !now.contains(n)).collect();
    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!(
            "{} now in the set ({})",
            plural(added.len(), "row"),
            added
                .iter()
                .map(|s| format!("'{}'", s.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !gone.is_empty() {
        parts.push(format!(
            "{} gone ({})",
            plural(gone.len(), "row"),
            gone.iter()
                .map(|s| format!("'{}'", s.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if parts.is_empty() {
        "a clip was written or a track changed".to_string()
    } else {
        parts.join(", ")
    }
}

fn song_line(plan: &Plan) -> String {
    if plan.setlist.is_empty() {
        return "No song yet: set_song, or update_song with insert_entry.".to_string();
    }
    let bars: i64 = plan
        .setlist
        .iter()
        .filter_map(|e| {
            let n = e.repeats? as i64;
            let at = plan.rows.find(&e.section)?;
            let row = plan.rows.0.get(at)?;
            Some(n * row.bars().unwrap_or(song::DEFAULT_PHRASE))
        })
        .sum();
    format!(
        "Song: {}. {} entries, {bars} bars counted.",
        song::setlist_text(&plan.setlist),
        plan.setlist.len()
    )
}

fn apply(live: &LiveState, state: &PerfState, plan: Plan, left_alone: &[String]) -> ToolResult {
    let mut undo = UndoStep::open(live);
    let one_step = undo.open;
    let mut done: Vec<String> = Vec::new();
    let mut failure: Option<(usize, String, String)> = None;
    for planned in &plan.planned {
        if planned.step.no_change {
            done.push(format!(
                "{}  (no change — nothing sent)",
                planned.step.text()
            ));
            continue;
        }
        let mut failed = None;
        for job in &planned.jobs {
            if let Err(e) = run_job(live, state, job) {
                failed = Some(e);
                break;
            }
        }
        match failed {
            None => done.push(planned.step.text()),
            Some(e) => {
                failure = Some((planned.step.n, planned.step.verb.to_string(), e));
                break;
            }
        }
    }
    let stopped_at = failure.as_ref().map(|(n, _, _)| *n);
    let mut wrote_setlist = false;
    if plan.setlist_changed && failure.is_none() {
        let name = song::render_setlist(&plan.setlist);
        // Where the row is *now*: deleting or moving a row above it moved it.
        match plan.rows.setlist_row() {
            Some(index) => {
                require(live, "set_scene")?;
                live.send_command("set_scene", Some(json!({"index": index, "name": name})))
                    .map_err(|e| live_err("write the setlist scene", e))?;
            }
            None => {
                require(live, "create_scene")?;
                live.send_command("create_scene", Some(json!({"index": -1, "name": name})))
                    .map_err(|e| live_err("create the setlist scene", e))?;
            }
        }
        wrote_setlist = true;
    }
    undo.close();
    let mut out = String::new();
    match &failure {
        None => out.push_str(&format!(
            "Song updated — {}{}.\n",
            plural(plan.planned.len(), "edit"),
            if one_step {
                ", one undo step".to_string()
            } else {
                String::new()
            }
        )),
        Some((n, _, e)) => out.push_str(&format!(
            "Song updated — {} of {} edits applied, then stopped: {e}\n",
            n - 1,
            plan.planned.len()
        )),
    }
    for line in &done {
        out.push_str(line);
        out.push('\n');
    }
    if let Some((n, verb, e)) = &failure {
        out.push_str(&format!("{n:>2}. {verb:<15}FAILED — {e}\n"));
        let rest: Vec<String> = plan
            .planned
            .iter()
            .filter(|p| p.step.n > *n)
            .map(|p| format!("{}. {}", p.step.n, p.step.verb))
            .collect();
        if !rest.is_empty() {
            out.push_str(&format!("    Not attempted: {}.\n", rest.join(", ")));
        }
        if plan.setlist_changed {
            out.push_str(
                "    The song itself was not rewritten, so it still names what is in the set.\n",
            );
        }
    }
    if wrote_setlist {
        out.push_str(&song_line(&plan));
        out.push('\n');
        match replan_running(live, state, &song::sections(state), &plan.setlist) {
            Ok(Some(line)) => {
                out.push_str(&line);
                out.push('\n');
            }
            Ok(None) => {}
            Err(e) => out.push_str(&format!("The running song could not be re-planned: {e}\n")),
        }
    } else if failure.is_none() {
        out.push_str(&song_line(&plan));
        out.push('\n');
    }
    out.push_str(&untouched_line(state, left_alone));
    if stopped_at.is_none() {
        out.push_str(&format!(
            "\n{} What is in the set exists only in Live's memory until you save it there (Cmd+S — the Live API has no save of its own).",
            if one_step {
                "Cmd+Z undoes the whole revision."
            } else {
                "Each write is its own undo step in Live."
            }
        ));
    }
    Ok(out)
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
