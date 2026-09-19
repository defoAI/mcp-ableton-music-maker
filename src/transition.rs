//! Transitions on a jump: `tempo`, `retime`, `crossfade`, `fill`, `drop`
//! and `sweep`, composed into the cue primitives the script already runs
//! (ramps, sets, clip fires and stops) and, for retime and fill, note
//! rewrites written into the set before the cue is scheduled. Nothing is
//! rebuilt: every clip stays where it was, and the target row fires the
//! retimed copies.

use crate::connection::LiveState;
use crate::performance::{CueStep, CueTime, PerfState, SetSpec};
use crate::song::Section;
use crate::tools::{
    clip_notes, get_display, live_err, require, write_notes, CreateClipParams, Note,
};
use crate::variation::{vary, VNote};
use serde_json::{json, Value};

/// What a transition adds to the jump's cue.
#[derive(Debug, Default)]
pub struct Transition {
    /// Steps scheduled with the jump (ramps, sets, fires, stops)
    pub steps: Vec<CueStep>,
    /// One line per primitive, for the reply
    pub lines: Vec<String>,
    /// A drop with nothing kept is silence on purpose
    pub allow_silence: bool,
}

const KNOWN: &[&str] = &["tempo", "retime", "crossfade", "fill", "drop", "sweep"];

fn names_of(v: Option<&Value>) -> Result<Vec<Value>, String> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(Value::String(s)) => Ok(vec![Value::String(s.clone())]),
        Some(other) => Err(format!("expected a list of tracks, got {other}")),
    }
}

fn fmt(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

/// Expand a `transition` object into the steps and rewrites that go with
/// the jump to `target` at `bar`.
pub fn expand(
    live: &LiveState,
    state: &mut PerfState,
    _secs: &[Section],
    target: &Section,
    bar: i64,
    t: &Value,
) -> Result<Transition, String> {
    let obj = t.as_object().ok_or_else(|| {
        format!(
            "transition must be an object with any of {}",
            KNOWN.join(", ")
        )
    })?;
    if obj.is_empty() {
        return Err(format!(
            "transition is empty; give any of {}",
            KNOWN.join(", ")
        ));
    }
    for k in obj.keys() {
        if !KNOWN.contains(&k.as_str()) {
            return Err(format!(
                "transition: unknown key '{k}'; one of {}",
                KNOWN.join(", ")
            ));
        }
    }
    let mut out = Transition::default();
    let next_bar = state.next_bar();
    let lead_bars = bar - next_bar; // bars left under the outgoing section
    if let Some(v) = obj.get("tempo") {
        let to = v
            .as_f64()
            .ok_or_else(|| "transition.tempo must be a number (BPM)".to_string())?;
        if !(20.0..=999.0).contains(&to) {
            return Err(format!("transition.tempo {to} is outside 20–999 BPM"));
        }
        if lead_bars >= 1 {
            out.steps.push(CueStep {
                from: Some(CueTime::Bar {
                    bar: next_bar as f64,
                }),
                bars: Some(lead_bars as f64),
                ramp: Some(json!({"tempo": to})),
                ..Default::default()
            });
            out.lines.push(format!(
                "{:<12} ramp tempo {} → {} under the outgoing phrase",
                format!("bars {next_bar}–{bar}"),
                fmt(state.tempo),
                fmt(to)
            ));
        } else {
            out.steps.push(CueStep {
                at: Some(CueTime::Bar { bar: bar as f64 }),
                set: Some(SetSpec {
                    target: "tempo".into(),
                    value: to,
                    ..Default::default()
                }),
                ..Default::default()
            });
            out.lines.push(format!(
                "{:<12} set tempo {} (no bars left to ramp before the jump)",
                format!("bar {bar}"),
                fmt(to)
            ));
        }
    }
    if let Some(v) = obj.get("retime") {
        retime(live, state, target, bar, v, &mut out)?;
    }
    if let Some(v) = obj.get("crossfade") {
        crossfade(live, state, bar, v, &mut out)?;
    }
    if let Some(v) = obj.get("fill") {
        fill(live, state, target, bar, v, &mut out)?;
    }
    if let Some(v) = obj.get("drop") {
        drop_before(state, bar, v, &mut out)?;
    }
    if let Some(v) = obj.get("sweep") {
        let spec = v.as_object().ok_or(
            "transition.sweep must be an object {track, device_index, parameter_index, from?, to}",
        )?;
        if lead_bars < 1 {
            return Err(format!(
                "transition.sweep: no bars left before bar {bar} to sweep over; jump at a later bar"
            ));
        }
        let mut g = spec.clone();
        g.insert("bars".into(), json!(lead_bars));
        let track = state.track_by(
            spec.get("track")
                .ok_or("transition.sweep needs \"track\"")?,
        )?;
        out.steps.push(CueStep {
            at: Some(CueTime::Bar {
                bar: next_bar as f64,
            }),
            gesture: Some(json!({"sweep": Value::Object(g)})),
            ..Default::default()
        });
        out.lines.push(format!(
            "{:<12} sweep {} device {} parameter {} → {} under the outgoing phrase",
            format!("bars {next_bar}–{bar}"),
            track.name,
            spec.get("device_index").map(display).unwrap_or_default(),
            spec.get("parameter_index").map(display).unwrap_or_default(),
            spec.get("to")
                .and_then(Value::as_f64)
                .map(fmt)
                .unwrap_or_default()
        ));
    }
    Ok(out)
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn clip_info(live: &LiveState, track_index: i64, clip_index: i64) -> Result<(String, f64), String> {
    require(live, "get_clip_info")?;
    let info = live
        .send_command(
            "get_clip_info",
            Some(
                json!({"track_index": track_index, "clip_index": clip_index, "arrangement": false}),
            ),
        )
        .map_err(|e| live_err("read the clip", e))?;
    Ok((
        get_display(&info, "name", "clip"),
        info.get("length").and_then(Value::as_f64).unwrap_or(0.0),
    ))
}

fn vnotes(raw: &[Value]) -> Vec<VNote> {
    raw.iter().filter_map(VNote::from_value).collect()
}

fn length_of(notes: &[VNote], given: f64, bpb: f64) -> f64 {
    if given > 0.0 {
        return given;
    }
    let extent = notes
        .iter()
        .map(|n| n.start + n.duration)
        .fold(0.0, f64::max);
    ((extent / bpb).ceil().max(1.0)) * bpb
}

/// `retime {tracks, to}`: half- or double-time copies of the named tracks'
/// clips in the target row, written now so the target fires them. A track
/// with no clip in the target row gets a retimed copy of what it plays.
/// A clip written now: the state the cue is checked against must know it.
fn note_new_clip(state: &mut PerfState, track_index: i64, slot: i64) {
    if let Some(t) = state.tracks.iter_mut().find(|t| t.index == track_index) {
        if !t.slots_with_clips.contains(&slot) {
            t.slots_with_clips.push(slot);
        }
    }
    if let Some(sc) = state.scenes.iter_mut().find(|s| s.index == slot) {
        if !sc.clip_tracks.contains(&track_index) {
            sc.clip_tracks.push(track_index);
        }
    }
}

fn retime(
    live: &LiveState,
    state: &mut PerfState,
    target: &Section,
    bar: i64,
    v: &Value,
    out: &mut Transition,
) -> Result<(), String> {
    let spec = v.as_object().ok_or(
        "transition.retime must be {\"tracks\": [...], \"to\": \"half_time\" | \"double_time\"}",
    )?;
    let to = spec
        .get("to")
        .and_then(Value::as_str)
        .unwrap_or("half_time")
        .to_string();
    if to != "half_time" && to != "double_time" {
        return Err(format!(
            "transition.retime.to must be half_time or double_time, not '{to}'"
        ));
    }
    let seed = spec.get("seed").and_then(Value::as_u64).unwrap_or(1);
    let tracks = names_of(spec.get("tracks"))?;
    if tracks.is_empty() {
        return Err("transition.retime needs \"tracks\"".into());
    }
    let bpb = state.beats_per_bar();
    let mut done: Vec<String> = Vec::new();
    let mut clips: Vec<String> = Vec::new();
    let mut written: Vec<i64> = Vec::new();
    for which in &tracks {
        let t = state.track_by(which)?;
        let in_row = t.slots_with_clips.contains(&target.index);
        let source_slot = if in_row {
            target.index
        } else if t.playing_slot_index >= 0 {
            t.playing_slot_index
        } else {
            return Err(format!(
                "transition.retime: '{}' has no clip in '{}'s row and plays nothing to copy",
                t.name, target.name
            ));
        };
        let (name, length) = clip_info(live, t.index, source_slot)?;
        let raw = clip_notes(live, t.index, source_slot)?;
        let notes = vnotes(&raw);
        if notes.is_empty() {
            return Err(format!(
                "transition.retime: '{}' slot {source_slot} has no notes to retime",
                t.name
            ));
        }
        let length = length_of(&notes, length, bpb);
        let varied = vary(&notes, &to, seed, length, bpb)?;
        let values: Vec<Value> = varied.iter().map(|n| n.to_value()).collect();
        let new_name = format!("{name} ({to})");
        if in_row {
            live.vary_undo
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((t.index, target.index), raw.clone());
            write_notes(live, t.index, target.index, &values)?;
            require(live, "set_clip_name")?;
            live.send_command(
                "set_clip_name",
                Some(json!({"track_index": t.index, "clip_index": target.index, "name": new_name})),
            )
            .map_err(|e| live_err("rename the retimed clip", e))?;
        } else {
            let plain: Vec<Note> = values
                .iter()
                .filter_map(|n| serde_json::from_value(n.clone()).ok())
                .collect();
            crate::tools::create_clip_body(
                live,
                &CreateClipParams {
                    track_index: t.index,
                    clip_index: target.index,
                    length,
                    name: new_name.clone(),
                    input: crate::notes::NotesInput {
                        notes: plain,
                        ..Default::default()
                    },
                },
            )?;
        }
        done.push(t.name.clone());
        clips.push(format!("'{new_name}'"));
        written.push(t.index);
    }
    for ti in written {
        note_new_clip(state, ti, target.index);
    }
    out.lines.push(format!(
        "{:<12} {} retimed to {} in {}'s row (clips {} written now, seed {seed}{})",
        format!("bar {bar}"),
        done.join(", "),
        to.replace('_', " "),
        target.name,
        clips.join(", "),
        if done.len() == 1 {
            "; undo_vary restores an in-place rewrite"
        } else {
            ""
        }
    ));
    Ok(())
}

/// `crossfade {out, in, bars}`: the `out` tracks fade to silence from the
/// boundary, the `in` tracks start at silence and fade up to their current
/// level, both over `bars`.
fn crossfade(
    live: &LiveState,
    state: &PerfState,
    bar: i64,
    v: &Value,
    out: &mut Transition,
) -> Result<(), String> {
    let spec = v
        .as_object()
        .ok_or("transition.crossfade must be {\"out\": [...], \"in\": [...], \"bars\": N}")?;
    let bars = spec.get("bars").and_then(Value::as_f64).unwrap_or(8.0);
    if !(1.0..=64.0).contains(&bars) {
        return Err("transition.crossfade.bars must be 1 to 64".into());
    }
    let outs = names_of(spec.get("out"))?;
    let ins = names_of(spec.get("in"))?;
    if outs.is_empty() && ins.is_empty() {
        return Err("transition.crossfade needs \"out\" and/or \"in\" tracks".into());
    }
    let mut out_names = Vec::new();
    for which in &outs {
        let t = state.track_by(which)?;
        out.steps.push(CueStep {
            from: Some(CueTime::Bar { bar: bar as f64 }),
            bars: Some(bars),
            ramp: Some(json!({"volume": 0.0, "track": t.index})),
            ..Default::default()
        });
        out_names.push(t.name.clone());
    }
    let mut in_names = Vec::new();
    if !ins.is_empty() {
        // Their current faders, in one round trip, so the ramp ends where they sit now.
        require(live, "get_context")?;
        let ctx = live
            .send_command("get_context", Some(json!({"include_library": false})))
            .map_err(|e| live_err("read the track levels", e))?;
        let volume_of = |index: i64| -> f64 {
            ctx.get("tracks")
                .and_then(Value::as_array)
                .and_then(|a| {
                    a.iter()
                        .find(|t| t.get("index").and_then(Value::as_i64) == Some(index))
                })
                .and_then(|t| t.get("volume").and_then(Value::as_f64))
                .unwrap_or(0.85)
        };
        let next_bar = state.next_bar().min(bar);
        for which in &ins {
            let t = state.track_by(which)?;
            let level = volume_of(t.index);
            out.steps.push(CueStep {
                at: Some(CueTime::Bar {
                    bar: next_bar as f64,
                }),
                set: Some(SetSpec {
                    target: "volume".into(),
                    track: Some(json!(t.index)),
                    value: 0.0,
                    ..Default::default()
                }),
                ..Default::default()
            });
            out.steps.push(CueStep {
                from: Some(CueTime::Bar { bar: bar as f64 }),
                bars: Some(bars),
                ramp: Some(json!({"volume": level, "track": t.index})),
                ..Default::default()
            });
            in_names.push(format!("{} (to {})", t.name, fmt(level)));
            if t.playing_slot_index >= 0 {
                out.lines.push(format!(
                    "Note: {} is playing now and its fader goes to 0 at bar {next_bar} for the crossfade.",
                    t.name
                ));
            }
        }
    }
    out.lines.push(format!(
        "{:<12} crossfade: {}{}{}",
        format!("bars {bar}–{}", bar + bars as i64),
        if out_names.is_empty() {
            String::new()
        } else {
            format!("{} → 0", out_names.join(", "))
        },
        if !out_names.is_empty() && !in_names.is_empty() {
            " · "
        } else {
            ""
        },
        if in_names.is_empty() {
            String::new()
        } else {
            format!("{} from 0 up to their current level", in_names.join(", "))
        }
    ));
    Ok(())
}

/// `fill {track, bars?}`: the last bar of a fill_last_bar variation of what
/// the track plays, as a one-bar clip in a free slot, fired `bars` before
/// the jump; the target row takes over at the boundary.
fn fill(
    live: &LiveState,
    state: &mut PerfState,
    target: &Section,
    bar: i64,
    v: &Value,
    out: &mut Transition,
) -> Result<(), String> {
    let spec = match v {
        Value::String(s) => json!({"track": s}),
        other => other.clone(),
    };
    let spec = spec
        .as_object()
        .ok_or("transition.fill must be {\"track\": \"Drums\"} (or the track name)")?;
    let t = state
        .track_by(spec.get("track").ok_or("transition.fill needs \"track\"")?)?
        .clone();
    let bars = spec
        .get("bars")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .clamp(1, 4);
    let seed = spec.get("seed").and_then(Value::as_u64).unwrap_or(1);
    if t.playing_slot_index < 0 {
        return Err(format!(
            "transition.fill: '{}' plays nothing to make a fill from",
            t.name
        ));
    }
    let fire_bar = bar - bars;
    if fire_bar < state.next_bar() {
        return Err(format!(
            "transition.fill: the fill would start at bar {fire_bar}, before the next bar ({}); jump later or shorten it",
            state.next_bar()
        ));
    }
    let bpb = state.beats_per_bar();
    let (name, length) = clip_info(live, t.index, t.playing_slot_index)?;
    let raw = clip_notes(live, t.index, t.playing_slot_index)?;
    let notes = vnotes(&raw);
    if notes.is_empty() {
        return Err(format!(
            "transition.fill: '{}' slot {} has no notes",
            t.name, t.playing_slot_index
        ));
    }
    let length = length_of(&notes, length, bpb);
    let varied = vary(&notes, "fill_last_bar", seed, length, bpb)?;
    let fill_len = bars as f64 * bpb;
    let last_bar_start = (length - bpb).max(0.0);
    // The fill bar, moved to the end of a `bars`-long clip whose earlier bars
    // are the clip's own material.
    let mut fill_notes: Vec<Value> = Vec::new();
    for n in &varied {
        if n.start >= last_bar_start {
            let mut m = n.clone();
            m.start = n.start - last_bar_start + (fill_len - bpb);
            fill_notes.push(m.to_value());
        }
    }
    if bars > 1 {
        for n in &notes {
            if n.start < fill_len - bpb {
                fill_notes.push(n.to_value());
            }
        }
    }
    let slot = (0..64)
        .find(|s| {
            !t.slots_with_clips.contains(s)
                && *s != target.index
                && *s as usize >= state.scenes.len().saturating_sub(64)
        })
        .ok_or_else(|| {
            format!(
                "transition.fill: no free slot on '{}' for the fill clip",
                t.name
            )
        })?;
    let plain: Vec<Note> = fill_notes
        .iter()
        .filter_map(|n| serde_json::from_value(n.clone()).ok())
        .collect();
    let fill_name = format!("{name} fill");
    crate::tools::create_clip_body(
        live,
        &CreateClipParams {
            track_index: t.index,
            clip_index: slot,
            length: fill_len,
            name: fill_name.clone(),
            input: crate::notes::NotesInput {
                notes: plain,
                ..Default::default()
            },
        },
    )?;
    note_new_clip(state, t.index, slot);
    out.steps.push(CueStep {
        at: Some(CueTime::Bar {
            bar: fire_bar as f64,
        }),
        fire_clip: Some(crate::performance::ClipRef {
            track: json!(t.index),
            clip: slot,
        }),
        ..Default::default()
    });
    let replaced = target.clip_tracks.contains(&t.index);
    if !replaced {
        out.steps.push(CueStep {
            at: Some(CueTime::Bar { bar: bar as f64 }),
            stop_clip: Some(crate::performance::ClipRef {
                track: json!(t.index),
                clip: slot,
            }),
            ..Default::default()
        });
    }
    out.lines.push(format!(
        "{:<12} fill: '{}' (slot {slot}, {} bar{}, seed {seed}) on {} until the jump{}",
        format!("bar {fire_bar}"),
        fill_name,
        bars,
        if bars == 1 { "" } else { "s" },
        t.name,
        if replaced {
            String::new()
        } else {
            format!(
                "; stopped at bar {bar} ({} has no clip in {}'s row)",
                t.name, target.name
            )
        }
    ));
    Ok(())
}

/// `drop {bars?, keep?}`: everything but `keep` stops `bars` before the
/// jump; the target row takes over at the boundary.
fn drop_before(state: &PerfState, bar: i64, v: &Value, out: &mut Transition) -> Result<(), String> {
    let spec = match v {
        Value::Bool(true) | Value::Null => json!({}),
        Value::Number(n) => json!({"bars": n}),
        other => other.clone(),
    };
    let spec = spec
        .as_object()
        .ok_or("transition.drop must be {\"bars\": 1, \"keep\": [\"Kick\"]}")?;
    let bars = spec
        .get("bars")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .clamp(1, 16);
    let keep = names_of(spec.get("keep"))?;
    let mut kept: Vec<i64> = Vec::new();
    for which in &keep {
        kept.push(state.track_by(which)?.index);
    }
    let drop_bar = bar - bars;
    if drop_bar < state.next_bar() {
        return Err(format!(
            "transition.drop: the drop would start at bar {drop_bar}, before the next bar ({}); jump later or shorten it",
            state.next_bar()
        ));
    }
    let mut stopped: Vec<String> = Vec::new();
    for t in &state.tracks {
        if kept.contains(&t.index) || t.playing_slot_index < 0 {
            continue;
        }
        out.steps.push(CueStep {
            at: Some(CueTime::Bar {
                bar: drop_bar as f64,
            }),
            stop_clip: Some(crate::performance::ClipRef {
                track: json!(t.index),
                clip: t.playing_slot_index,
            }),
            ..Default::default()
        });
        stopped.push(t.name.clone());
    }
    if stopped.is_empty() {
        return Err("transition.drop: nothing is playing that it would take out".into());
    }
    if kept.is_empty() {
        out.allow_silence = true;
    }
    out.lines.push(format!(
        "{:<12} drop: {} out{} for the last {} bar{} before the jump",
        format!("bar {drop_bar}"),
        stopped.join(", "),
        if kept.is_empty() {
            " (silence)".to_string()
        } else {
            format!(
                ", {} stay{}",
                keep.iter().map(display).collect::<Vec<_>>().join(", "),
                if keep.len() == 1 { "s" } else { "" }
            )
        },
        bars,
        if bars == 1 { "" } else { "s" }
    ));
    Ok(())
}
