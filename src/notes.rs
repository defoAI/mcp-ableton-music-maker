//! Compact ways to write MIDI notes. Every form expands into the plain
//! [`Note`] objects the Remote Script already accepts, so nothing here
//! touches Live or the script — it only shrinks what the model has to type.
//!
//! Forms, all optional and freely mixed in one call:
//! - `notes`: the full objects (`pitch`, `start_time`, `duration`, `velocity`, `mute`)
//! - `notes_csv`: one `pitch,start,duration,velocity[,mute]` per line
//! - `steps`: a step-sequencer string per pitch, `"36": "x...x...x...x..."`
//! - `patterns`: `{pitch, every, offset, count, velocity, duration}`
//! - `loop_every` + `until`: tile everything above across a longer clip
//!
//! Pitches may be numbers or note names (`C1`, `F#2`, `Bb3`; `C4` = 60).

use crate::tools::Note;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Map;
use std::collections::BTreeMap;

fn quarter() -> f64 {
    0.25
}
fn hundred() -> i64 {
    100
}
fn accent() -> i64 {
    120
}

/// A note that repeats: hi-hats every beat, an off-beat bass, a 16th ride.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Pattern {
    /// MIDI pitch 0-127, or a note name like "C1" or "F#2"
    pub pitch: serde_json::Value,
    /// Beats between notes (1.0 = every beat, 0.5 = every eighth)
    pub every: f64,
    /// Beats before the first note (0.5 = off-beat)
    #[serde(default)]
    pub offset: f64,
    /// How many notes; 0 means "until the `until` length of the call"
    #[serde(default)]
    pub count: i64,
    /// Velocity 1-127 (default 100)
    #[serde(default = "hundred")]
    pub velocity: i64,
    /// Note length in beats (default 0.25)
    #[serde(default = "quarter")]
    pub duration: f64,
}

/// Everything a tool accepts as note input. Flattened into the tool's
/// parameters, so each field is a top-level argument.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesInput {
    /// Full note objects: pitch, start_time, duration, velocity; mute is
    /// optional and defaults to false. Prefer notes_csv, steps or patterns —
    /// they say the same thing in a fraction of the space.
    #[serde(default)]
    pub notes: Vec<Note>,
    /// One note per line: "pitch,start,duration,velocity" with an optional
    /// fifth ",mute" (0/1). Lines starting with # are ignored.
    /// Example: "36,0,0.25,110\n38,1,0.25,100\n42,0.5,0.125,70"
    #[serde(default)]
    pub notes_csv: String,
    /// Step sequencer, one string per pitch. Each character is one `step`
    /// beat: x = note (velocity), X = accent (accent_velocity), 1-9 = velocity
    /// level (9 loudest), _ = hold the previous note one more step, . or - =
    /// rest; spaces and | are ignored. Example for one bar of 16ths:
    /// {"36": "x...x...x...x...", "42": "..x...x...x...x.", "38": "....X.......X..."}
    #[serde(default)]
    pub steps: BTreeMap<String, String>,
    /// Beats per step character (default 0.25 = a 16th in 4/4)
    #[serde(default = "quarter")]
    pub step: f64,
    /// Velocity for x in steps (default 100)
    #[serde(default = "hundred")]
    pub velocity: i64,
    /// Velocity for X in steps (default 120)
    #[serde(default = "accent")]
    pub accent_velocity: i64,
    /// Repeating notes: [{"pitch": 42, "every": 0.5, "offset": 0.25, "count": 32, "velocity": 70}]
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    /// Tile everything above every N beats (e.g. 4 for one bar) up to `until`.
    /// 0 = no tiling.
    #[serde(default)]
    pub loop_every: f64,
    /// Where tiling stops, and the fill length for patterns with count 0, in
    /// beats (e.g. 64 for a 16-bar clip). 0 = not set.
    #[serde(default)]
    pub until: f64,
}

impl Default for NotesInput {
    /// The same values serde fills in for absent fields.
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            notes_csv: String::new(),
            steps: BTreeMap::new(),
            step: quarter(),
            velocity: hundred(),
            accent_velocity: accent(),
            patterns: Vec::new(),
            loop_every: 0.0,
            until: 0.0,
        }
    }
}

impl NotesInput {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.notes_csv.trim().is_empty()
            && self.steps.is_empty()
            && self.patterns.is_empty()
    }
}

/// Parse a pitch given as a number or a note name. `C4` is 60; `C-1` is 0.
pub fn parse_pitch(v: &serde_json::Value) -> Result<i64, String> {
    match v {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.round() as i64))
            .ok_or_else(|| format!("pitch {n} is not a whole number")),
        serde_json::Value::String(s) => parse_note_name(s),
        other => Err(format!(
            "pitch must be a number or a note name, got {other}"
        )),
    }
}

pub fn parse_note_name(s: &str) -> Result<i64, String> {
    let t = s.trim();
    if let Ok(n) = t.parse::<i64>() {
        return check_pitch(n);
    }
    let mut chars = t.chars();
    let letter = chars
        .next()
        .ok_or_else(|| "empty pitch".to_string())?
        .to_ascii_uppercase();
    let base = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => {
            return Err(format!(
                "`{s}` is not a pitch (use 0-127 or a name like C1, F#2, Bb3)"
            ))
        }
    };
    let rest: String = chars.collect();
    let (accidental, octave_str) = match rest.chars().next() {
        Some('#') | Some('♯') => (
            1,
            &rest[rest
                .char_indices()
                .nth(1)
                .map(|(i, _)| i)
                .unwrap_or(rest.len())..],
        ),
        Some('b') | Some('♭') => (
            -1,
            &rest[rest
                .char_indices()
                .nth(1)
                .map(|(i, _)| i)
                .unwrap_or(rest.len())..],
        ),
        _ => (0, rest.as_str()),
    };
    let octave: i64 = octave_str
        .trim()
        .parse()
        .map_err(|_| format!("`{s}` is not a pitch (use 0-127 or a name like C1, F#2, Bb3)"))?;
    check_pitch((octave + 1) * 12 + base + accidental)
}

fn check_pitch(p: i64) -> Result<i64, String> {
    if (0..=127).contains(&p) {
        Ok(p)
    } else {
        Err(format!("pitch {p} is outside 0-127"))
    }
}

fn check_velocity(v: i64, what: &str) -> Result<i64, String> {
    if (1..=127).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{what}: velocity {v} is outside 1-127"))
    }
}

fn note(pitch: i64, start: f64, duration: f64, velocity: i64) -> Note {
    Note {
        pitch,
        start_time: start,
        duration,
        velocity,
        mute: false,
        extra: Map::new(),
    }
}

fn parse_csv(text: &str) -> Result<Vec<Note>, String> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < 4 || f.len() > 5 {
            return Err(format!(
                "notes_csv line {}: expected pitch,start,duration,velocity[,mute], got `{line}`",
                i + 1
            ));
        }
        let num = |s: &str, what: &str| -> Result<f64, String> {
            s.parse::<f64>()
                .map_err(|_| format!("notes_csv line {}: {what} `{s}` is not a number", i + 1))
        };
        let pitch = parse_note_name(f[0]).map_err(|e| format!("notes_csv line {}: {e}", i + 1))?;
        let start = num(f[1], "start")?;
        let duration = num(f[2], "duration")?;
        let velocity = check_velocity(
            num(f[3], "velocity")?.round() as i64,
            &format!("notes_csv line {}", i + 1),
        )?;
        let mut n = note(pitch, start, duration, velocity);
        if f.len() == 5 {
            n.mute = matches!(f[4], "1" | "true" | "yes" | "x");
        }
        out.push(n);
    }
    Ok(out)
}

fn parse_steps(input: &NotesInput) -> Result<Vec<Note>, String> {
    if input.step <= 0.0 {
        return Err("step must be greater than 0".into());
    }
    let normal = check_velocity(input.velocity, "steps")?;
    let accent = check_velocity(input.accent_velocity, "steps accent")?;
    let mut out: Vec<Note> = Vec::new();
    for (key, pattern) in &input.steps {
        let pitch = parse_note_name(key).map_err(|e| format!("steps: {e}"))?;
        let mut pos = 0usize;
        let mut last: Option<usize> = None; // index in `out` of the note a `_` extends
        for c in pattern.chars() {
            match c {
                ' ' | '|' | '\t' => continue,
                '.' | '-' => last = None,
                '_' => {
                    let Some(i) = last else {
                        return Err(format!(
                            "steps for {key}: `_` at step {} has no note to extend",
                            pos + 1
                        ));
                    };
                    out[i].duration += input.step;
                }
                'x' | 'X' | '1'..='9' => {
                    let velocity = match c {
                        'x' => normal,
                        'X' => accent,
                        d => (d.to_digit(10).unwrap() as i64) * 14,
                    };
                    out.push(note(pitch, pos as f64 * input.step, input.step, velocity));
                    last = Some(out.len() - 1);
                }
                other => {
                    return Err(format!(
                        "steps for {key}: `{other}` is not a step character (use x X 1-9 _ . - and spaces)"
                    ))
                }
            }
            pos += 1;
        }
    }
    Ok(out)
}

fn expand_patterns(input: &NotesInput) -> Result<Vec<Note>, String> {
    let mut out = Vec::new();
    for (i, p) in input.patterns.iter().enumerate() {
        let where_ = format!("patterns[{i}]");
        let pitch = parse_pitch(&p.pitch).map_err(|e| format!("{where_}: {e}"))?;
        if p.every <= 0.0 {
            return Err(format!("{where_}: every must be greater than 0"));
        }
        let velocity = check_velocity(p.velocity, &where_)?;
        let count = if p.count > 0 {
            p.count
        } else if input.until > 0.0 {
            ((input.until - p.offset) / p.every).ceil().max(0.0) as i64
        } else {
            return Err(format!(
                "{where_}: give count, or set until (the clip length in beats) for count 0"
            ));
        };
        for k in 0..count {
            let start = p.offset + k as f64 * p.every;
            if input.until > 0.0 && start >= input.until {
                break;
            }
            out.push(note(pitch, start, p.duration, velocity));
        }
    }
    Ok(out)
}

/// Expand every form into plain notes, then tile if asked.
pub fn expand(input: &NotesInput) -> Result<Vec<Note>, String> {
    let mut notes: Vec<Note> = Vec::new();
    for n in &input.notes {
        check_pitch(n.pitch)?;
        check_velocity(n.velocity, "notes")?;
        notes.push(n.clone());
    }
    notes.extend(parse_csv(&input.notes_csv)?);
    notes.extend(parse_steps(input)?);
    notes.extend(expand_patterns(input)?);

    if input.loop_every > 0.0 {
        if input.until <= 0.0 {
            return Err(
                "loop_every needs until (the clip length in beats) to know where to stop".into(),
            );
        }
        let base = notes.clone();
        let mut k = 1.0;
        while k * input.loop_every < input.until {
            let shift = k * input.loop_every;
            for n in &base {
                if n.start_time + shift < input.until {
                    let mut c = n.clone();
                    c.start_time += shift;
                    notes.push(c);
                }
            }
            k += 1.0;
        }
    }
    notes.sort_by(|a, b| {
        a.start_time
            .partial_cmp(&b.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
    });
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(notes: &[Note], pitch: i64) -> Vec<f64> {
        notes
            .iter()
            .filter(|n| n.pitch == pitch)
            .map(|n| n.start_time)
            .collect()
    }

    #[test]
    fn note_names() {
        assert_eq!(parse_note_name("C4").unwrap(), 60);
        assert_eq!(parse_note_name("c1").unwrap(), 24);
        assert_eq!(parse_note_name("F#2").unwrap(), 42);
        assert_eq!(parse_note_name("Bb3").unwrap(), 58);
        assert_eq!(parse_note_name("C-1").unwrap(), 0);
        assert_eq!(parse_note_name("36").unwrap(), 36);
        assert!(parse_note_name("H2").is_err());
        assert!(parse_note_name("200").is_err());
    }

    #[test]
    fn steps_make_a_bar_of_drums() {
        let mut steps = BTreeMap::new();
        steps.insert("36".to_string(), "x...x...x...x...".to_string());
        steps.insert("F#2".to_string(), "..X. ..x. | ..X. ..x.".to_string());
        let input = NotesInput {
            steps,
            ..Default::default()
        };
        let notes = expand(&input).unwrap();
        assert_eq!(starts(&notes, 36), vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(starts(&notes, 42), vec![0.5, 1.5, 2.5, 3.5]);
        let hats: Vec<i64> = notes
            .iter()
            .filter(|n| n.pitch == 42)
            .map(|n| n.velocity)
            .collect();
        assert_eq!(hats, vec![120, 100, 120, 100]);
        assert!(notes.iter().all(|n| n.duration == 0.25));
    }

    #[test]
    fn ties_extend_and_digits_set_velocity() {
        let mut steps = BTreeMap::new();
        steps.insert("C1".to_string(), "x___....9...".to_string());
        let input = NotesInput {
            steps,
            step: 0.5,
            ..Default::default()
        };
        let notes = expand(&input).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].duration, 2.0);
        assert_eq!(notes[1].start_time, 4.0);
        assert_eq!(notes[1].velocity, 126);
    }

    #[test]
    fn bad_step_character_is_named() {
        let mut steps = BTreeMap::new();
        steps.insert("36".to_string(), "x..?".to_string());
        let err = expand(&NotesInput {
            steps,
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.contains("`?`"), "{err}");
    }

    #[test]
    fn csv_lines() {
        let input = NotesInput {
            notes_csv: "# kick and snare\n36,0,0.25,110\n38, 1, 0.25, 100, 1\n".into(),
            ..Default::default()
        };
        let notes = expand(&input).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[1].pitch, 38);
        assert!(notes[1].mute);
        let bad = NotesInput {
            notes_csv: "36,0,0.25".into(),
            ..Default::default()
        };
        assert!(expand(&bad).unwrap_err().contains("line 1"));
    }

    #[test]
    fn patterns_count_or_until() {
        let p = Pattern {
            pitch: serde_json::json!("F#2"),
            every: 0.5,
            offset: 0.25,
            count: 0,
            velocity: 70,
            duration: 0.125,
        };
        let no_len = NotesInput {
            patterns: vec![p.clone()],
            ..Default::default()
        };
        assert!(expand(&no_len).unwrap_err().contains("until"));
        let with_len = NotesInput {
            patterns: vec![p],
            until: 4.0,
            ..Default::default()
        };
        let notes = expand(&with_len).unwrap();
        assert_eq!(notes.len(), 8);
        assert_eq!(notes[0].start_time, 0.25);
        assert_eq!(notes[7].start_time, 3.75);
    }

    #[test]
    fn tiling_one_bar_across_a_clip() {
        let mut steps = BTreeMap::new();
        steps.insert("36".to_string(), "x...x...x...x...".to_string());
        let input = NotesInput {
            steps,
            loop_every: 4.0,
            until: 64.0,
            ..Default::default()
        };
        let notes = expand(&input).unwrap();
        assert_eq!(notes.len(), 64);
        assert_eq!(notes.last().unwrap().start_time, 63.0);
        let missing = NotesInput {
            loop_every: 4.0,
            ..Default::default()
        };
        assert!(expand(&missing).unwrap_err().contains("until"));
    }

    #[test]
    fn forms_mix_and_sort() {
        let mut steps = BTreeMap::new();
        steps.insert("36".to_string(), "x...".to_string());
        let input = NotesInput {
            notes: vec![note(60, 2.0, 1.0, 90)],
            notes_csv: "38,1,0.25,100".into(),
            steps,
            ..Default::default()
        };
        let notes = expand(&input).unwrap();
        let order: Vec<(i64, f64)> = notes.iter().map(|n| (n.pitch, n.start_time)).collect();
        assert_eq!(order, vec![(36, 0.0), (38, 1.0), (60, 2.0)]);
    }
}
