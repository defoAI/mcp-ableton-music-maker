//! The artist-facing tools the review of 2026-09-19 asked for: `arrange`
//! (place, repeat, move, delete, shorten, list — in bars, one round trip per
//! track), `feel` (swing, humanize, groove, retime, a variation — one tool,
//! one undo), `set_key`, `create_return` and `clear_captures`. The bodies
//! compose the existing ones; nothing here talks to Live on its own path.

use crate::connection::LiveState;
use crate::performance::PerfState;
use crate::tools::{
    self, clip_notes, get_display, live_err, performance_running, read_perf_state, require,
    ToolResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

fn one_u64() -> u64 {
    1
}
fn fifteen() -> i64 {
    15
}
fn sixteenth() -> String {
    "1/16".into()
}

// ── feel ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FeelParams {
    /// Track name or index
    pub track: Value,
    /// Session slot of the clip
    pub clip: i64,
    /// Swing: delay the off-beat steps by this fraction of the grid step (0.33 is a classic swing, 0.5 halfway)
    #[serde(default)]
    pub swing: Option<f64>,
    /// The swing grid, "1/16" (default) or "1/8"
    #[serde(default = "sixteenth")]
    pub grid: String,
    /// Humanize: how far a hit may land early or late, in milliseconds
    #[serde(default)]
    pub humanize_ms: Option<f64>,
    /// With humanize: how much velocities vary (default 15)
    #[serde(default = "fifteen")]
    pub velocity: i64,
    /// A groove from this set's Groove Pool by name or index (Live's own, non-destructive), or "none" to remove it
    #[serde(default)]
    pub groove: Option<Value>,
    /// The set's global groove amount, 0–1
    #[serde(default)]
    pub groove_amount: Option<f64>,
    /// "half_time" or "double_time"
    #[serde(default)]
    pub retime: Option<String>,
    /// A variation: "fill_last_bar", "ghost_notes", "invert_chords" or "thin"
    #[serde(default)]
    pub variation: Option<String>,
    /// Seed: the same seed gives the same feel (default 1)
    #[serde(default = "one_u64")]
    pub seed: u64,
    /// Write the result into this empty slot instead of in place (retime and variation only)
    #[serde(default)]
    pub to_slot: Option<i64>,
    /// Put the notes back as they were before the last feel call on this clip
    #[serde(default)]
    pub undo: bool,
}

pub fn feel_body(live: &LiveState, p: &FeelParams) -> ToolResult {
    if p.undo {
        return tools::undo_vary_body(
            live,
            &tools::UndoVaryParams {
                track: p.track.clone(),
                clip: p.clip,
            },
        );
    }
    let asked = [
        p.swing.is_some(),
        p.humanize_ms.is_some(),
        p.groove.is_some(),
        p.groove_amount.is_some(),
        p.retime.is_some(),
        p.variation.is_some(),
    ]
    .iter()
    .filter(|a| **a)
    .count();
    if asked == 0 {
        return Err("Say what to change: swing, humanize_ms, groove, groove_amount, retime or variation (or undo: true).".into());
    }
    let rewrites = [
        p.swing.is_some(),
        p.humanize_ms.is_some(),
        p.retime.is_some(),
        p.variation.is_some(),
    ]
    .iter()
    .filter(|a| **a)
    .count();
    // One undo for the whole call: the notes as they were before any rewrite.
    let mut before: Option<(i64, Vec<Value>)> = None;
    if rewrites > 0 && p.to_slot.is_none() {
        let state = read_perf_state(live)?;
        let track = state.track_by(&p.track)?;
        if track.slots_with_clips.contains(&p.clip) {
            before = Some((track.index, clip_notes(live, track.index, p.clip)?));
        }
    }
    let mut lines: Vec<String> = Vec::new();
    if let Some(v) = &p.variation {
        lines.push(tools::vary_clip_body(
            live,
            &tools::VaryClipParams {
                track: p.track.clone(),
                clip: p.clip,
                variation: v.clone(),
                seed: p.seed,
                to_slot: p.to_slot,
            },
        )?);
    }
    if let Some(r) = &p.retime {
        lines.push(tools::retime_clip_body(
            live,
            &tools::RetimeClipParams {
                track: p.track.clone(),
                clip: p.clip,
                to: r.clone(),
                seed: p.seed,
                to_slot: p.to_slot,
            },
        )?);
    }
    if let Some(amount) = p.swing {
        lines.push(tools::swing_notes_body(
            live,
            &tools::SwingNotesParams {
                track: p.track.clone(),
                clip: p.clip,
                amount,
                grid: if p.grid.trim().is_empty() {
                    "1/16".into()
                } else {
                    p.grid.clone()
                },
                seed: p.seed,
            },
        )?);
    }
    if let Some(ms) = p.humanize_ms {
        lines.push(tools::humanize_body(
            live,
            &tools::HumanizeParams {
                track: p.track.clone(),
                clip: p.clip,
                timing_ms: ms,
                velocity: p.velocity,
                seed: p.seed,
            },
        )?);
    }
    if let Some(g) = &p.groove {
        lines.push(tools::groove_clip_body(
            live,
            &tools::GrooveClipParams {
                track: p.track.clone(),
                clip: p.clip,
                groove: g.clone(),
                amount: None,
                random: None,
                velocity: None,
            },
        )?);
    }
    if let Some(a) = p.groove_amount {
        lines.push(tools::groove_amount_body(
            live,
            &tools::GrooveAmountParams { value: a },
        )?);
    }
    if let Some((ti, raw)) = before {
        live.vary_undo
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((ti, p.clip), raw);
        if rewrites > 1 {
            lines.push(
                "feel {\"undo\": true} puts the clip back as it was before this call.".into(),
            );
        }
    }
    Ok(lines.join("\n"))
}

// ── set_key ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SetKeyParams {
    /// The key, e.g. "F minor", "Bb dorian", "G"
    pub key: String,
}

pub fn set_key_body(live: &LiveState, p: &SetKeyParams) -> ToolResult {
    require(live, "set_scale")?;
    let Some((root, scale)) = crate::performance::parse_key(&p.key) else {
        return Err(format!(
            "'{}' is not a key I can read; say it like \"F minor\", \"D dorian\" or \"G\".",
            p.key.trim()
        ));
    };
    let r = live
        .send_command(
            "set_scale",
            Some(json!({"root_note": root, "scale_name": scale})),
        )
        .map_err(|e| live_err("set the key", e))?;
    let name = format!(
        "{} {}",
        get_display(
            &r,
            "root_note_name",
            crate::performance::PITCH_CLASSES[root as usize]
        ),
        get_display(&r, "scale_name", &scale)
    );
    if let Some(pf) = live
        .performance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        pf.key = Some(name.clone());
    }
    Ok(format!(
        "Key {name}: set in Live's scale settings (Live 12), so the clip editor highlights it and everything I write follows it."
    ))
}

// ── create_return ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CreateReturnParams {
    /// A name for the return (Live prefixes its letter), e.g. "Reverb"
    #[serde(default)]
    pub name: Option<String>,
    /// An effect to put on it, by browser URI or plain words ("reverb", "ping pong delay")
    #[serde(default)]
    pub effect: Option<String>,
}

pub fn create_return_body(live: &LiveState, p: &CreateReturnParams) -> ToolResult {
    require(live, "create_return_track")?;
    let r = live
        .send_command("create_return_track", Some(json!({"name": p.name})))
        .map_err(|e| live_err("create the return track", e))?;
    let index = r.get("index").and_then(Value::as_i64).unwrap_or(0);
    let letter = get_display(&r, "letter", "?");
    let mut text = format!(
        "Return {letter} '{}' created (return {index}). set_send {{\"send_name\": \"{letter}\"}} feeds it",
        get_display(&r, "name", "return")
    );
    if let Some(effect) = p.effect.as_ref().filter(|e| !e.trim().is_empty()) {
        let loaded = tools::load_instrument_or_effect_body(
            live,
            &tools::LoadInstrumentParams {
                track_index: Some(index),
                uri: effect.clone(),
                kind: "return".into(),
                ..Default::default()
            },
        )?;
        text.push_str(&format!("; {}", loaded.trim_end_matches('.')));
    }
    text.push('.');
    Ok(text)
}

// ── clear_captures ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ClearCapturesParams {
    /// Keep the Capture track and only remove its clips (default: the whole track goes)
    #[serde(default)]
    pub keep_track: bool,
}

pub fn clear_captures_body(live: &LiveState, p: &ClearCapturesParams) -> ToolResult {
    require(live, "get_context")?;
    let ctx = live
        .send_command("get_context", Some(json!({"include_library": false})))
        .map_err(|e| live_err("read the set", e))?;
    let capture = ctx
        .get("tracks")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter().find(|t| {
                t.get("name").and_then(Value::as_str) == Some("Capture")
                    && t.get("kind").and_then(Value::as_str) == Some("audio")
            })
        })
        .cloned();
    let Some(track) = capture else {
        return Ok("No Capture track in this set; nothing to clear.".into());
    };
    let index = track.get("index").and_then(Value::as_i64).unwrap_or(-1);
    let clips: Vec<i64> = track
        .get("clips")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("slot").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default();
    if performance_running(live).is_some() && track.get("is_recording") == Some(&json!(true)) {
        return Err(
            "The Capture track is recording right now; wait for the capture to finish.".into(),
        );
    }
    if p.keep_track {
        require(live, "delete_clip")?;
        for slot in &clips {
            live.send_command(
                "delete_clip",
                Some(json!({"track_index": index, "clip_index": slot})),
            )
            .map_err(|e| live_err("delete a capture", e))?;
        }
        return Ok(format!(
            "Cleared {} capture{} from the Capture track (track {index}); the track stays for the next capture. The audio files stay in your project's Samples/Recorded folder.",
            clips.len(),
            if clips.len() == 1 { "" } else { "s" }
        ));
    }
    require(live, "delete_track")?;
    live.send_command("delete_track", Some(json!({"track_index": index})))
        .map_err(|e| live_err("delete the Capture track", e))?;
    Ok(format!(
        "Removed the Capture track (track {index}) and its {} capture{}; later tracks moved up by one. The audio files stay in your project's Samples/Recorded folder; capture_mix makes a new track when needed.",
        clips.len(),
        if clips.len() == 1 { "" } else { "s" }
    ))
}

// ── arrange ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ArrangeParams {
    /// "place" a Session clip in the Arrangement, "repeat" an Arrangement clip after itself, "move" one, "delete" clips in a bar range, "shorten" the whole arrangement to end at a bar, or "list" what is there
    pub action: String,
    /// Track name or index (delete and list: every track when omitted)
    #[serde(default)]
    pub track: Option<Value>,
    /// place: the Session clip by name, or its slot; repeat/move: the Arrangement clip by name, or its position as `list` shows it
    #[serde(default)]
    pub clip: Option<Value>,
    /// place/move: the bar to land on — a number (Live's 1-based bars) or a locator's name; repeat: where the copies start (default: right after the clip)
    #[serde(default)]
    pub at_bar: Option<Value>,
    /// place: keep placing copies every `every_bars` up to this bar (exclusive) — a number or a locator's name
    #[serde(default)]
    pub until_bar: Option<Value>,
    /// place: bars between copies (default: the clip's length)
    #[serde(default)]
    pub every_bars: Option<f64>,
    /// place/repeat: how many copies (default 1)
    #[serde(default)]
    pub times: Option<i64>,
    /// delete: from this bar (inclusive) — a number or a locator's name
    #[serde(default)]
    pub from_bar: Option<Value>,
    /// delete: up to this bar (exclusive); shorten: the bar the arrangement ends on — a number or a locator's name
    #[serde(default)]
    pub to_bar: Option<Value>,
}

fn beat_of(state: &PerfState, bar: f64) -> Result<f64, String> {
    if bar < 1.0 {
        return Err(format!(
            "bar {bar} is before bar 1 (Live's bars start at 1)"
        ));
    }
    Ok((bar - 1.0) * state.beats_per_bar())
}

fn bar_of(state: &PerfState, beat: f64) -> f64 {
    beat / state.beats_per_bar() + 1.0
}

fn fmt(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.2}")
    }
}

/// A bar as this module prints one — whole when it is whole. Shared so a
/// locator listing reads the same as an `arrange list`.
pub fn fmt_bar(v: f64) -> String {
    fmt(v)
}

fn arrangement_clips(live: &LiveState, track_index: i64) -> Result<Vec<Value>, String> {
    require(live, "get_arrangement_clips")?;
    let r = live
        .send_command(
            "get_arrangement_clips",
            Some(json!({"track_index": track_index})),
        )
        .map_err(|e| live_err("read the arrangement", e))?;
    Ok(r.get("clips")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// The Arrangement clip `repeat` and `move` mean: its position as `list`
/// prints it, or its name. The copies of one Session clip share a name, so a
/// name that matches several is an error naming each with its bars — never a
/// guess at which copy was meant.
fn arrangement_clip_index(
    clips: &[Value],
    which: &Value,
    state: &PerfState,
    track: &str,
) -> Result<i64, String> {
    let bars_of = |c: &Value| {
        let s = c.get("start_time").and_then(Value::as_f64).unwrap_or(0.0);
        let e = c.get("end_time").and_then(Value::as_f64).unwrap_or(0.0);
        format!("bars {}–{}", fmt(bar_of(state, s)), fmt(bar_of(state, e)))
    };
    let given = match which {
        Value::Number(n) => return Ok(n.as_i64().unwrap_or(0)),
        Value::String(s) => s.trim().to_string(),
        other => return Err(format!("a clip is a name or a position, not {other}")),
    };
    if let Ok(i) = given.parse::<i64>() {
        return Ok(i);
    }
    let want = given.to_lowercase();
    for exact in [true, false] {
        let hits: Vec<(usize, &Value)> = clips
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                let name = get_display(c, "name", "").to_lowercase();
                if exact {
                    name == want
                } else {
                    name.contains(&want)
                }
            })
            .collect();
        match hits.len() {
            0 => continue,
            1 => return Ok(hits[0].0 as i64),
            _ => {
                return Err(format!(
                    "'{given}' matches {} Arrangement clips on '{track}': {}. Say the position instead.",
                    hits.len(),
                    hits.iter()
                        .map(|(i, c)| format!("{i} ({})", bars_of(c)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
    Err(format!(
        "no Arrangement clip named '{given}' on '{track}'; it has {}.",
        if clips.is_empty() {
            "none".to_string()
        } else {
            clips
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{i} '{}' {}", get_display(c, "name", "clip"), bars_of(c)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ))
}

pub fn arrange_body(live: &LiveState, p: &ArrangeParams) -> ToolResult {
    let action = p.action.trim().to_lowercase();
    let state = read_perf_state(live)?;
    let tracks: Vec<crate::performance::TrackState> = match &p.track {
        Some(w) => vec![state.track_by(w)?.clone()],
        None => state.tracks.clone(),
    };
    // Every bar this call names, resolved once: a number is Live's bar, a
    // word is a locator the producer already dropped in the Arrangement.
    let bar = |which: &Option<Value>| {
        crate::tools::resolve_bar(live, state.beats_per_bar(), which.as_ref())
    };
    let from_bar = bar(&p.from_bar)?;
    let to_bar = bar(&p.to_bar)?;
    // `place` reads from a bar up to a bar and so does `delete`; the tool
    // called the first pair at_bar/until_bar and the second from_bar/to_bar,
    // which is two vocabularies for one idea. Either says either now.
    let at_bar = bar(&p.at_bar)?.or(from_bar);
    let until_bar = bar(&p.until_bar)?.or(to_bar);
    match action.as_str() {
        "place" => {
            require(live, "place_clips")?;
            let track = single(&p.track, &tracks, "place")?;
            let on = crate::tools::TrackTarget {
                index: track.index,
                kind: "track".into(),
                name: track.name.clone(),
            };
            let which = p
                .clip
                .as_ref()
                .filter(|c| !c.is_null())
                .ok_or("place needs `clip` (the Session clip's name, or its slot)")?;
            let slot = crate::tools::resolve_clip_slot(live, &on, Some(which), None)?;
            if !track.slots_with_clips.contains(&slot) {
                return Err(format!("slot {slot} on '{}' holds no clip", track.name));
            }
            let at = beat_of(&state, at_bar.ok_or("place needs `at_bar`")?)?;
            let length = clip_length(live, track.index, slot)?;
            let step = match p.every_bars {
                Some(b) if b > 0.0 => b * state.beats_per_bar(),
                Some(b) => return Err(format!("every_bars must be positive, got {b}")),
                None => length,
            };
            let mut times: Vec<f64> = Vec::new();
            match (until_bar, p.times) {
                (Some(until), _) => {
                    let end = beat_of(&state, until)?;
                    let mut t = at;
                    while t < end - 1e-9 && times.len() < 512 {
                        times.push(t);
                        t += step;
                    }
                }
                (None, Some(n)) => {
                    for i in 0..n.clamp(1, 512) {
                        times.push(at + i as f64 * step);
                    }
                }
                (None, None) => times.push(at),
            }
            if times.is_empty() {
                return Err("nothing to place: until_bar is not after at_bar".into());
            }
            let r = live
                .send_command(
                    "place_clips",
                    Some(json!({"track_index": track.index, "clip_index": slot, "times": times})),
                )
                .map_err(|e| live_err("place the clip", e))?;
            let placed = r
                .get("placed")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let failed = r
                .get("failed")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let last = times.last().copied().unwrap_or(at);
            Ok(format!(
                "Placed '{}' ({} bar{}) on {} {} time{} from bar {} to bar {}{} — one round trip.{}",
                get_display(&r, "clip", "clip"),
                fmt(length / state.beats_per_bar()),
                if (length - state.beats_per_bar()).abs() < 1e-9 {
                    ""
                } else {
                    "s"
                },
                track.name,
                placed,
                if placed == 1 { "" } else { "s" },
                fmt(bar_of(&state, at)),
                fmt(bar_of(&state, last + length)),
                if step > length + 1e-9 {
                    // A gap between copies is silence the producer did not
                    // ask for and cannot see in a reply that only counts
                    // clips. A 1-bar loop placed every 4 bars is three bars
                    // of nothing, eight times over.
                    format!(
                        " (every {} bars, so {} bar{} of silence between them — the clip is {} bar{} long)",
                        fmt(step / state.beats_per_bar()),
                        fmt((step - length) / state.beats_per_bar()),
                        if (step - length - state.beats_per_bar()).abs() < 1e-9 {
                            ""
                        } else {
                            "s"
                        },
                        fmt(length / state.beats_per_bar()),
                        if (length - state.beats_per_bar()).abs() < 1e-9 {
                            ""
                        } else {
                            "s"
                        }
                    )
                } else if step != length {
                    format!(" (every {} bars)", fmt(step / state.beats_per_bar()))
                } else {
                    String::new()
                },
                if failed > 0 {
                    format!(" {failed} could not be placed: {}", r["failed"])
                } else {
                    String::new()
                }
            ))
        }
        "repeat" | "move" => {
            require(live, "duplicate_arrangement_clip")?;
            let track = single(&p.track, &tracks, &action)?;
            let which = p.clip.as_ref().filter(|c| !c.is_null()).ok_or_else(|| {
                format!("{action} needs `clip` (the Arrangement clip's name, or its position as list shows it)")
            })?;
            let clips = arrangement_clips(live, track.index)?;
            let ci = arrangement_clip_index(&clips, which, &state, &track.name)?;
            let clip = clips.get(ci as usize).ok_or_else(|| {
                format!(
                    "'{}' has {} Arrangement clip{}; no position {ci}",
                    track.name,
                    clips.len(),
                    if clips.len() == 1 { "" } else { "s" }
                )
            })?;
            let start = clip
                .get("start_time")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let end = clip
                .get("end_time")
                .and_then(Value::as_f64)
                .unwrap_or(start);
            let length = (end - start).max(0.0);
            let name = get_display(clip, "name", "clip");
            if action == "move" {
                let to = beat_of(&state, at_bar.ok_or("move needs `at_bar`")?)?;
                live.send_command(
                    "duplicate_arrangement_clip",
                    Some(json!({"track_index": track.index, "clip_index": ci, "times": [to]})),
                )
                .map_err(|e| live_err("copy the clip to its new place", e))?;
                require(live, "delete_arrangement_clips")?;
                live.send_command(
                    "delete_arrangement_clips",
                    Some(json!({"track_index": track.index, "indices": [ci]})),
                )
                .map_err(|e| live_err("remove the clip from its old place", e))?;
                return Ok(format!(
                    "Moved '{name}' on {} from bar {} to bar {} ({} bars).",
                    track.name,
                    fmt(bar_of(&state, start)),
                    fmt(bar_of(&state, to)),
                    fmt(length / state.beats_per_bar())
                ));
            }
            let n = p.times.unwrap_or(1).clamp(1, 256);
            let first = match at_bar {
                Some(b) => beat_of(&state, b)?,
                None => end,
            };
            let times: Vec<f64> = (0..n).map(|i| first + i as f64 * length).collect();
            let r = live
                .send_command(
                    "duplicate_arrangement_clip",
                    Some(json!({"track_index": track.index, "clip_index": ci, "times": times})),
                )
                .map_err(|e| live_err("repeat the clip", e))?;
            let placed = r
                .get("placed")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Ok(format!(
                "Repeated '{name}' ({} bars) on {} {} time{}: bars {} to {} — one round trip.",
                fmt(length / state.beats_per_bar()),
                track.name,
                placed,
                if placed == 1 { "" } else { "s" },
                fmt(bar_of(&state, first)),
                fmt(bar_of(&state, first + n as f64 * length))
            ))
        }
        "delete" | "shorten" => {
            require(live, "delete_arrangement_clips")?;
            let (from, to) = if action == "shorten" {
                let end = to_bar
                    .ok_or("shorten needs `to_bar` (the bar the arrangement should end on)")?;
                (beat_of(&state, end)?, f64::INFINITY)
            } else {
                let from = match from_bar {
                    Some(b) => beat_of(&state, b)?,
                    None => 0.0,
                };
                let to = match to_bar {
                    Some(b) => beat_of(&state, b)?,
                    None => f64::INFINITY,
                };
                if from_bar.is_none() && to_bar.is_none() && p.track.is_none() {
                    return Err(
                        "delete on every track needs from_bar and/or to_bar; say which bars."
                            .into(),
                    );
                }
                (from, to)
            };
            let mut removed = 0;
            let mut per_track: Vec<String> = Vec::new();
            let mut crossing: Vec<String> = Vec::new();
            for t in &tracks {
                let mut params = json!({"track_index": t.index, "from_beat": from});
                if to.is_finite() {
                    params["to_beat"] = json!(to);
                }
                let r = live
                    .send_command("delete_arrangement_clips", Some(params))
                    .map_err(|e| live_err("delete arrangement clips", e))?;
                let n = r
                    .get("removed")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if n > 0 {
                    removed += n;
                    per_track.push(format!("{} {n}", t.name));
                }
                if action == "shorten" {
                    // Clips that start before the end bar and run past it cannot be
                    // trimmed through Live's API; name them.
                    for c in arrangement_clips(live, t.index)? {
                        let s = c.get("start_time").and_then(Value::as_f64).unwrap_or(0.0);
                        let e = c.get("end_time").and_then(Value::as_f64).unwrap_or(0.0);
                        if s < from && e > from {
                            crossing.push(format!(
                                "'{}' on {} (bars {}–{})",
                                get_display(&c, "name", "clip"),
                                t.name,
                                fmt(bar_of(&state, s)),
                                fmt(bar_of(&state, e))
                            ));
                        }
                    }
                }
            }
            let mut text = if action == "shorten" {
                format!(
                    "Arrangement shortened to end at bar {}: {removed} clip{} removed{}.",
                    fmt(bar_of(&state, from)),
                    if removed == 1 { "" } else { "s" },
                    if per_track.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", per_track.join(", "))
                    }
                )
            } else {
                format!(
                    "Deleted {removed} Arrangement clip{} starting in bars {}–{}{}.",
                    if removed == 1 { "" } else { "s" },
                    fmt(bar_of(&state, from)),
                    if to.is_finite() {
                        fmt(bar_of(&state, to))
                    } else {
                        "the end".into()
                    },
                    if per_track.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", per_track.join(", "))
                    }
                )
            };
            if !crossing.is_empty() {
                text.push_str(&format!(
                    " Live's API cannot trim a clip, so {} still run{} past that bar; shorten {} in Live, or place shorter clips.",
                    crossing.join(", "),
                    if crossing.len() == 1 { "s" } else { "" },
                    if crossing.len() == 1 { "it" } else { "them" }
                ));
            }
            Ok(text)
        }
        "list" => {
            let mut out = String::new();
            let mut total = 0;
            for t in &tracks {
                let clips = arrangement_clips(live, t.index)?;
                if clips.is_empty() {
                    continue;
                }
                total += clips.len();
                let items: Vec<String> = clips
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let s = c.get("start_time").and_then(Value::as_f64).unwrap_or(0.0);
                        let e = c.get("end_time").and_then(Value::as_f64).unwrap_or(0.0);
                        format!(
                            "{i} '{}' bars {}–{}",
                            get_display(c, "name", "clip"),
                            fmt(bar_of(&state, s)),
                            fmt(bar_of(&state, e))
                        )
                    })
                    .collect();
                out.push_str(&format!("{}: {}\n", t.name, items.join(", ")));
            }
            if total == 0 {
                return Ok("Nothing in the Arrangement yet. arrange {\"action\": \"place\", \"track\": …, \"clip\": <slot>, \"at_bar\": 1, \"until_bar\": 33} puts a Session clip there.".into());
            }
            out.push_str(&format!("{total} Arrangement clip{} (bars are Live's 1-based bars; end bars are exclusive).", if total == 1 { "" } else { "s" }));
            Ok(out)
        }
        other => Err(format!(
            "action must be place, repeat, move, delete, shorten or list, not '{other}'"
        )),
    }
}

fn single<'a>(
    which: &Option<Value>,
    tracks: &'a [crate::performance::TrackState],
    action: &str,
) -> Result<&'a crate::performance::TrackState, String> {
    if which.is_none() {
        return Err(format!("{action} needs `track`"));
    }
    tracks.first().ok_or_else(|| "no such track".to_string())
}

fn clip_length(live: &LiveState, track_index: i64, slot: i64) -> Result<f64, String> {
    require(live, "get_clip_info")?;
    let info = live
        .send_command(
            "get_clip_info",
            Some(json!({"track_index": track_index, "clip_index": slot, "arrangement": false})),
        )
        .map_err(|e| live_err("read the clip", e))?;
    Ok(info
        .get("length")
        .and_then(Value::as_f64)
        .filter(|l| *l > 0.0)
        .unwrap_or(4.0))
}
