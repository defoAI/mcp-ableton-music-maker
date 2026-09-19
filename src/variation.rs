//! Variations on an existing clip's notes, and the key of a recording. Pure
//! functions on the note objects `get_clip_notes` returns; seeded, so the
//! same call gives the same variation. The tool bodies read the clip, run
//! one of these, and write the result back with `clear: true`.

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct VNote {
    pub pitch: i64,
    pub start: f64,
    pub duration: f64,
    pub velocity: i64,
    pub mute: bool,
}

impl VNote {
    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self {
            pitch: v.get("pitch")?.as_i64()?,
            start: v.get("start_time")?.as_f64()?,
            duration: v.get("duration").and_then(Value::as_f64).unwrap_or(0.25),
            velocity: v.get("velocity").and_then(Value::as_i64).unwrap_or(100),
            mute: v.get("mute").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    pub fn to_value(&self) -> Value {
        json!({"pitch": self.pitch, "start_time": self.start, "duration": self.duration,
               "velocity": self.velocity, "mute": self.mute})
    }
}

/// A tiny deterministic generator (xorshift), so a `seed` reproduces a variation.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1) ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_f64(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            let i = (self.next_f64() * items.len() as f64) as usize;
            items.get(i.min(items.len() - 1))
        }
    }
}

pub const VARIATIONS: &[&str] = &[
    "fill_last_bar",
    "ghost_notes",
    "invert_chords",
    "thin",
    "half_time",
    "double_time",
];

/// Apply one named variation. `length` is the clip length in beats,
/// `beats_per_bar` the set's signature.
pub fn vary(
    notes: &[VNote],
    variation: &str,
    seed: u64,
    length: f64,
    beats_per_bar: f64,
) -> Result<Vec<VNote>, String> {
    let mut rng = Rng::new(seed);
    match variation {
        "fill_last_bar" => Ok(fill_last_bar(notes, &mut rng, length, beats_per_bar)),
        "ghost_notes" => Ok(ghost_notes(notes, &mut rng, length)),
        "invert_chords" => Ok(invert_chords(notes)),
        "thin" => Ok(thin(notes)),
        "half_time" => Ok(stretch(notes, 2.0, length)),
        "double_time" => Ok(stretch(notes, 0.5, length)),
        other => Err(format!(
            "unknown variation '{other}'; one of {}",
            VARIATIONS.join(", ")
        )),
    }
}

/// The last bar becomes 16th-note repeats of the clip's own pitches,
/// velocities rising into the downbeat.
fn fill_last_bar(notes: &[VNote], rng: &mut Rng, length: f64, bpb: f64) -> Vec<VNote> {
    let bar_start = (length - bpb).max(0.0);
    let pitches: Vec<i64> = {
        let mut p: Vec<i64> = notes.iter().map(|n| n.pitch).collect();
        p.sort();
        p.dedup();
        p
    };
    let mut out: Vec<VNote> = notes
        .iter()
        .filter(|n| n.start < bar_start)
        .cloned()
        .collect();
    if pitches.is_empty() {
        return out;
    }
    let steps = (bpb * 4.0) as usize;
    for i in 0..steps {
        let t = bar_start + i as f64 * 0.25;
        if t >= length {
            break;
        }
        let pitch = *rng.pick(&pitches).unwrap();
        let velocity = 70 + ((i as f64 / steps as f64) * 50.0) as i64;
        out.push(VNote {
            pitch,
            start: t,
            duration: 0.2,
            velocity: velocity.min(127),
            mute: false,
        });
    }
    out
}

/// Quiet notes on empty 16ths of the pitches already used (density 0.2).
fn ghost_notes(notes: &[VNote], rng: &mut Rng, length: f64) -> Vec<VNote> {
    let pitches: Vec<i64> = {
        let mut p: Vec<i64> = notes.iter().map(|n| n.pitch).collect();
        p.sort();
        p.dedup();
        p
    };
    let mut out = notes.to_vec();
    if pitches.is_empty() {
        return out;
    }
    let steps = (length * 4.0) as usize;
    for i in 0..steps {
        let t = i as f64 * 0.25;
        let occupied = notes.iter().any(|n| (n.start - t).abs() < 0.01);
        if occupied || rng.next_f64() > 0.2 {
            continue;
        }
        out.push(VNote {
            pitch: *rng.pick(&pitches).unwrap(),
            start: t,
            duration: 0.15,
            velocity: 30,
            mute: false,
        });
    }
    out
}

/// Each group of simultaneous notes goes up one inversion: the lowest note
/// moves up an octave.
fn invert_chords(notes: &[VNote]) -> Vec<VNote> {
    let mut out = notes.to_vec();
    let mut starts: Vec<f64> = notes.iter().map(|n| n.start).collect();
    starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    starts.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    for s in starts {
        let group: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, n)| (n.start - s).abs() < 0.01)
            .map(|(i, _)| i)
            .collect();
        if group.len() < 2 {
            continue;
        }
        let lowest = *group.iter().min_by_key(|i| out[**i].pitch).unwrap();
        if out[lowest].pitch + 12 <= 127 {
            out[lowest].pitch += 12;
        }
    }
    out
}

/// Drop every other note that is not on a beat.
fn thin(notes: &[VNote]) -> Vec<VNote> {
    let mut off = 0;
    notes
        .iter()
        .filter(|n| {
            if (n.start - n.start.round()).abs() < 0.01 {
                true
            } else {
                off += 1;
                off % 2 == 1
            }
        })
        .cloned()
        .collect()
}

/// Stretch the note grid by `factor` (2 = half time), keeping the clip length:
/// half time drops what falls past the end; double time repeats the material.
fn stretch(notes: &[VNote], factor: f64, length: f64) -> Vec<VNote> {
    let mut out = Vec::new();
    let repeats = if factor < 1.0 {
        (1.0 / factor).round() as usize
    } else {
        1
    };
    for r in 0..repeats {
        let offset = r as f64 * length * factor;
        for n in notes {
            let start = n.start * factor + offset;
            if start >= length {
                continue;
            }
            out.push(VNote {
                start,
                duration: (n.duration * factor).max(0.05),
                ..n.clone()
            });
        }
    }
    out
}

/// Humanize: every note lands up to `timing` beats early or late (a
/// seeded, bell-shaped offset) and its velocity varies by up to
/// `velocity`, off-beat notes more than on-beat ones. A note never crosses
/// a bar line and never starts before the clip.
pub fn humanize(notes: &[VNote], timing: f64, velocity: i64, seed: u64, bpb: f64) -> Vec<VNote> {
    let mut rng = Rng::new(seed);
    let bell = |rng: &mut Rng| (rng.next_f64() + rng.next_f64() + rng.next_f64()) / 1.5 - 1.0;
    notes
        .iter()
        .map(|n| {
            let bar_start = (n.start / bpb).floor() * bpb;
            let bar_end = bar_start + bpb;
            let start = (n.start + bell(&mut rng) * timing).clamp(bar_start, bar_end - 0.001);
            let on_beat = (n.start - n.start.round()).abs() < 0.01;
            let scale = if on_beat { 0.5 } else { 1.0 };
            let v = n.velocity + (bell(&mut rng) * velocity as f64 * scale).round() as i64;
            VNote {
                start: (start * 1000.0).round() / 1000.0,
                velocity: v.clamp(1, 127),
                ..n.clone()
            }
        })
        .collect()
}

/// Swing: the off-beat steps of a grid (`step` beats: 0.25 for 16ths, 0.5
/// for 8ths) are delayed by `amount` of the step. On-beat notes stay.
pub fn swing(notes: &[VNote], amount: f64, step: f64) -> Vec<VNote> {
    let delay = amount.clamp(0.0, 1.0) * step;
    notes
        .iter()
        .map(|n| {
            let idx = (n.start / step).round();
            let on_grid = (n.start - idx * step).abs() < 0.02;
            if on_grid && (idx as i64) % 2 == 1 {
                VNote {
                    start: ((n.start + delay) * 1000.0).round() / 1000.0,
                    ..n.clone()
                }
            } else {
                n.clone()
            }
        })
        .collect()
}

/// A duration in beats as the nearest note value: "about a 64th".
pub fn note_value_words(beats: f64) -> String {
    if beats <= 0.0 {
        return "nothing".into();
    }
    let whole = beats / 4.0;
    let mut best = (f64::MAX, 1u32);
    for d in [1u32, 2, 4, 8, 16, 32, 64, 128, 256] {
        let v = 1.0 / d as f64;
        let err = (whole.ln() - v.ln()).abs();
        if err < best.0 {
            best = (err, d);
        }
    }
    let name = match best.1 {
        1 => "a whole note".to_string(),
        2 => "a half note".to_string(),
        4 => "a quarter note".to_string(),
        8 => "an 8th".to_string(),
        16 => "a 16th".to_string(),
        32 => "a 32nd".to_string(),
        d => format!("a {d}th"),
    };
    let ratio = whole / (1.0 / best.1 as f64);
    if (0.9..=1.1).contains(&ratio) {
        format!("about {name}")
    } else if ratio < 1.0 {
        format!("a little under {name}")
    } else {
        format!("a little over {name}")
    }
}

/// Krumhansl-Kessler key profiles.
const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
pub const PITCH_CLASSES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The most likely key of a set of notes: (root 0–11, "Major"/"Minor", confidence 0–1).
pub fn estimate_key(notes: &[VNote]) -> Option<(i64, &'static str, f64)> {
    if notes.is_empty() {
        return None;
    }
    let mut hist = [0.0f64; 12];
    for n in notes {
        hist[(n.pitch.rem_euclid(12)) as usize] += n.duration.max(0.05) * n.velocity as f64;
    }
    let mut best: Option<(i64, &'static str, f64)> = None;
    let mut second = f64::MIN;
    for root in 0..12 {
        for (name, profile) in [("Major", &MAJOR), ("Minor", &MINOR)] {
            let score = correlation(&hist, profile, root);
            match best {
                Some((_, _, b)) if score <= b => {
                    if score > second {
                        second = score;
                    }
                }
                Some((_, _, b)) => {
                    second = b;
                    best = Some((root as i64, name, score));
                }
                None => best = Some((root as i64, name, score)),
            }
        }
    }
    best.map(|(r, n, s)| {
        let confidence = if second == f64::MIN {
            1.0
        } else {
            ((s - second) / (s.abs() + 1e-9)).clamp(0.0, 1.0)
        };
        (r, n, confidence)
    })
}

fn correlation(hist: &[f64; 12], profile: &[f64; 12], root: usize) -> f64 {
    let mh = hist.iter().sum::<f64>() / 12.0;
    let mp = profile.iter().sum::<f64>() / 12.0;
    let mut num = 0.0;
    let mut dh = 0.0;
    let mut dp = 0.0;
    for i in 0..12 {
        let h = hist[(i + root) % 12] - mh;
        let p = profile[i] - mp;
        num += h * p;
        dh += h * h;
        dp += p * p;
    }
    if dh == 0.0 || dp == 0.0 {
        0.0
    } else {
        num / (dh * dp).sqrt()
    }
}

/// Semitones to move from `from_root` to `to_root`, the short way (−6…+5).
pub fn transpose_interval(from_root: i64, to_root: i64) -> i64 {
    let d = (to_root - from_root).rem_euclid(12);
    if d > 6 {
        d - 12
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(pitch: i64, start: f64) -> VNote {
        VNote {
            pitch,
            start,
            duration: 0.25,
            velocity: 100,
            mute: false,
        }
    }

    #[test]
    fn variations_are_seeded_and_keep_the_length() {
        let notes: Vec<VNote> = (0..16)
            .map(|i| n(if i % 4 == 0 { 36 } else { 42 }, i as f64 * 0.5))
            .collect();
        let a = vary(&notes, "fill_last_bar", 7, 8.0, 4.0).unwrap();
        let b = vary(&notes, "fill_last_bar", 7, 8.0, 4.0).unwrap();
        assert_eq!(a, b, "same seed, same fill");
        assert!(a.iter().all(|x| x.start < 8.0));
        assert_eq!(
            a.iter().filter(|x| x.start < 4.0).count(),
            8,
            "first bar untouched"
        );
        assert_eq!(
            a.iter().filter(|x| x.start >= 4.0).count(),
            16,
            "16ths in the last bar"
        );
        let c = vary(&notes, "fill_last_bar", 8, 8.0, 4.0).unwrap();
        assert_ne!(a, c, "another seed, another fill");
        let g = vary(&notes, "ghost_notes", 1, 8.0, 4.0).unwrap();
        assert!(
            g.len() > notes.len()
                && g.iter()
                    .filter(|x| x.velocity == 30)
                    .all(|x| x.pitch == 36 || x.pitch == 42)
        );
        let t = vary(&notes, "thin", 1, 8.0, 4.0).unwrap();
        assert_eq!(
            t.len(),
            8 + 4,
            "on-beat notes kept, every other off-beat dropped"
        );
        let h = vary(&notes, "half_time", 1, 8.0, 4.0).unwrap();
        assert_eq!(h.len(), 8);
        assert!((h[1].start - 1.0).abs() < 1e-9);
        let d = vary(&notes, "double_time", 1, 8.0, 4.0).unwrap();
        assert_eq!(d.len(), 32);
        assert!(vary(&notes, "reverse", 1, 8.0, 4.0).is_err());
    }

    #[test]
    fn humanize_is_seeded_stays_in_the_bar_and_swing_delays_off_beats_only() {
        // Sixteen 8ths over two bars, the last one on the very last 8th of bar 2.
        let notes: Vec<VNote> = (0..16).map(|i| n(42, i as f64 * 0.5)).collect();
        let a = humanize(&notes, 0.125, 15, 3, 4.0);
        let b = humanize(&notes, 0.125, 15, 3, 4.0);
        assert_eq!(a, b, "same seed, same feel");
        assert_ne!(a, humanize(&notes, 0.125, 15, 4, 4.0));
        assert!(
            a.iter()
                .any(|x| x.start.fract() != 0.0 && (x.start - x.start.round()).abs() > 0.01),
            "something moved off the grid"
        );
        for (h, o) in a.iter().zip(notes.iter()) {
            assert!(
                (h.start - o.start).abs() <= 0.125 + 1e-9,
                "{} moved {}",
                o.start,
                h.start - o.start
            );
            assert_eq!(
                (h.start / 4.0).floor(),
                (o.start / 4.0).floor(),
                "never across a bar line"
            );
            assert!(h.start >= 0.0);
            assert!((h.velocity - o.velocity).abs() <= 15);
            assert!((1..=127).contains(&h.velocity));
        }
        // Swing: odd 16th steps delayed by half a step, even ones untouched.
        let sixteenths: Vec<VNote> = (0..8).map(|i| n(42, i as f64 * 0.25)).collect();
        let s = swing(&sixteenths, 0.5, 0.25);
        assert_eq!(s[0].start, 0.0);
        assert_eq!(s[1].start, 0.375);
        assert_eq!(s[2].start, 0.5);
        assert_eq!(s[3].start, 0.875);
        assert_eq!(swing(&sixteenths, 0.0, 0.25), sixteenths);
        assert_eq!(note_value_words(0.0625), "about a 64th");
        assert_eq!(note_value_words(0.126), "about a 32nd");
        assert_eq!(note_value_words(1.0), "about a quarter note");
        assert_eq!(note_value_words(0.05), "a little under a 64th");
    }

    #[test]
    fn chord_inversion_lifts_the_lowest_note() {
        let chord = vec![n(60, 0.0), n(64, 0.0), n(67, 0.0), n(62, 2.0)];
        let inv = invert_chords(&chord);
        let mut pitches: Vec<i64> = inv
            .iter()
            .filter(|x| x.start == 0.0)
            .map(|x| x.pitch)
            .collect();
        pitches.sort();
        assert_eq!(pitches, vec![64, 67, 72]);
        assert_eq!(inv[3].pitch, 62, "a single note is not a chord");
    }

    #[test]
    fn key_estimate_finds_scales() {
        let d_minor: Vec<VNote> = [62, 64, 65, 67, 69, 70, 72, 74]
            .iter()
            .enumerate()
            .map(|(i, p)| n(*p, i as f64))
            .collect();
        let (root, mode, conf) = estimate_key(&d_minor).unwrap();
        assert_eq!((root, mode), (2, "Minor"), "conf {conf}");
        let g_major: Vec<VNote> = [67, 69, 71, 72, 74, 76, 78, 79]
            .iter()
            .enumerate()
            .map(|(i, p)| n(*p, i as f64))
            .collect();
        assert_eq!(
            estimate_key(&g_major).map(|k| (k.0, k.1)),
            Some((7, "Major"))
        );
        assert!(estimate_key(&[]).is_none());
        assert_eq!(transpose_interval(2, 5), 3);
        assert_eq!(transpose_interval(9, 2), 5);
        assert_eq!(transpose_interval(2, 9), -5);
    }
}
