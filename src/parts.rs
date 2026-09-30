//! Named parts: one idea written once, played by several tracks (#85).
//!
//! A `build_song` clip carries its own notes and only its own, so offering a
//! producer three drum kits meant writing the same four step-strings three
//! times over — 354 notes of pure duplication in one reported session, then
//! 324 more when the kits were replaced. Worse than the tokens: a change to
//! the shared idea is one edit per copy, and each copy is a chance for the
//! parts to stop being comparable.
//!
//! So a document may define `parts`, and a clip may reference one by name.
//! Two rules make it worth having:
//!
//! **A part resolves once.** Its notes — and its feel — are expanded a single
//! time and copied, so three clips made from one part are identical note for
//! note. Humanise is seeded and swing is arithmetic, but applying either
//! *after* the copy would give three different results from one idea, which
//! is the bug this exists to prevent.
//!
//! **A clip may move it.** `transpose` shifts every pitch (the melodic case);
//! `map` rewrites named pitches (the drum case, because kits disagree about
//! which note is the clap). A clip may also add notes of its own on top.
//!
//! Nothing here touches Live: a part is resolved in the server and what
//! reaches Live is the notes it always received. Pure, so the whole of it is
//! testable without a set.

use crate::notes::NotesInput;
use crate::tools::Note;
use crate::variation::VNote;
use serde_json::Value;
use std::collections::BTreeMap;

/// The feel a part carries, baked into it once.
///
/// `groove` is deliberately absent: a groove is an assignment to one of
/// Live's own pool grooves, made per clip by `feel`, and there is nothing to
/// bake here. Swing and humanise are arithmetic on the notes, so they can be.
#[derive(Debug, Clone, Default)]
pub struct PartFeel {
    /// 0–1, how far the off-beats are pushed late
    pub swing: Option<f64>,
    /// The grid swing works on, in beats (default: the part's own `step`)
    pub swing_step: Option<f64>,
    /// Timing scatter in milliseconds
    pub humanize_ms: Option<f64>,
    /// Velocity scatter, 0–127
    pub humanize_velocity: Option<i64>,
    /// The seed, so the same document always gives the same notes
    pub seed: Option<u64>,
}

impl PartFeel {
    pub fn is_none(&self) -> bool {
        self.swing.is_none() && self.humanize_ms.is_none() && self.humanize_velocity.is_none()
    }
}

/// Apply a part's feel to its notes, once, before any clip copies them.
///
/// `bpm` turns `humanize_ms` into beats; `bpb` is the bar, which humanise
/// keeps its notes inside.
pub fn bake(notes: &[Note], feel: &PartFeel, bpm: f64, bpb: f64) -> Vec<Note> {
    if feel.is_none() {
        return notes.to_vec();
    }
    let mut v: Vec<VNote> = notes
        .iter()
        .map(|n| VNote {
            pitch: n.pitch,
            start: n.start_time,
            duration: n.duration,
            velocity: n.velocity,
            mute: n.mute,
        })
        .collect();
    if let Some(amount) = feel.swing {
        let step = feel.swing_step.filter(|s| *s > 0.0).unwrap_or(0.25);
        v = crate::variation::swing(&v, amount, step);
    }
    let ms = feel.humanize_ms.unwrap_or(0.0);
    let vel = feel.humanize_velocity.unwrap_or(0);
    if ms > 0.0 || vel > 0 {
        // Milliseconds are the producer's unit; beats are Live's.
        let beats = ms / 1000.0 * (bpm.max(1.0) / 60.0);
        v = crate::variation::humanize(&v, beats, vel, feel.seed.unwrap_or(0), bpb);
    }
    v.into_iter()
        .map(|n| Note {
            pitch: n.pitch,
            start_time: n.start,
            duration: n.duration,
            velocity: n.velocity,
            mute: n.mute,
            extra: serde_json::Map::new(),
        })
        .collect()
}

/// Move a part's notes onto the pitches this clip wants.
///
/// `map` is applied first and `transpose` after it, so a map written in the
/// part's own pitches keeps meaning what it says whatever the transpose is. A
/// pitch the map does not mention is left where it is; a pitch mapped off the
/// 0–127 range is an error rather than a silently dropped note.
pub fn move_pitches(
    notes: &[Note],
    map: &BTreeMap<String, Value>,
    transpose: i64,
    part: &str,
) -> Result<Vec<Note>, String> {
    let mut lookup: BTreeMap<i64, i64> = BTreeMap::new();
    for (from, to) in map {
        let from_pitch = crate::notes::parse_note_name(from.trim())
            .map_err(|e| format!("part '{part}': map key '{from}': {e}"))?;
        let to_pitch = crate::notes::parse_pitch(to)
            .map_err(|e| format!("part '{part}': map '{from}' → {to}: {e}"))?;
        lookup.insert(from_pitch, to_pitch);
    }
    notes
        .iter()
        .map(|n| {
            let mapped = lookup.get(&n.pitch).copied().unwrap_or(n.pitch);
            let moved = mapped + transpose;
            if !(0..=127).contains(&moved) {
                return Err(format!(
                    "part '{part}': pitch {} becomes {moved}, which is outside 0–127",
                    n.pitch
                ));
            }
            let mut c = n.clone();
            c.pitch = moved;
            Ok(c)
        })
        .collect()
}

/// Every part in the document, resolved once.
///
/// The names are checked here, before anything reaches Live: a clip naming a
/// part the document does not define is an error listing the ones it does,
/// which is the difference between a typo caught in validation and a half-
/// built set.
#[derive(Debug)]
pub struct Resolved {
    by_name: BTreeMap<String, Vec<Note>>,
}

impl Resolved {
    pub fn build(
        parts: &BTreeMap<String, (NotesInput, PartFeel)>,
        bpm: f64,
        bpb: f64,
    ) -> Result<Self, String> {
        let mut by_name = BTreeMap::new();
        for (name, (input, feel)) in parts {
            if name.trim().is_empty() {
                return Err("a part needs a name".into());
            }
            let notes = crate::notes::expand(input).map_err(|e| format!("part '{name}': {e}"))?;
            if notes.is_empty() {
                return Err(format!(
                    "part '{name}' has no notes; give it steps, notes_csv, patterns or notes"
                ));
            }
            by_name.insert(name.clone(), bake(&notes, feel, bpm, bpb));
        }
        Ok(Self { by_name })
    }

    pub fn names(&self) -> Vec<&str> {
        self.by_name.keys().map(String::as_str).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The notes a clip gets from the part it names.
    pub fn notes_for(&self, part: &str) -> Result<&[Note], String> {
        self.by_name
            .get(part.trim())
            .map(Vec::as_slice)
            .ok_or_else(|| {
                if self.by_name.is_empty() {
                    format!("this document defines no parts, so '{part}' names nothing")
                } else {
                    format!(
                        "no part called '{part}'; this document defines: {}",
                        self.names()
                            .iter()
                            .map(|n| format!("'{n}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            })
    }

    /// Parts nothing referenced — worth saying, because a part defined and
    /// never used is usually a typo at the other end.
    pub fn unreferenced(&self, used: &[String]) -> Vec<&str> {
        self.by_name
            .keys()
            .filter(|n| !used.iter().any(|u| u.trim() == n.as_str()))
            .map(String::as_str)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step_part(pitch: &str, pattern: &str) -> NotesInput {
        let mut input = NotesInput::default();
        input.steps.insert(pitch.to_string(), pattern.to_string());
        input.step = 0.25;
        input
    }

    fn resolved(name: &str, input: NotesInput, feel: PartFeel) -> Resolved {
        let mut m = BTreeMap::new();
        m.insert(name.to_string(), (input, feel));
        Resolved::build(&m, 120.0, 4.0).expect("the part resolves")
    }

    #[test]
    fn a_part_resolves_to_notes_a_clip_can_play() {
        let r = resolved(
            "break_a",
            step_part("C1", "x...x...x...x..."),
            PartFeel::default(),
        );
        let notes = r.notes_for("break_a").unwrap();
        assert_eq!(notes.len(), 4);
        assert!(notes.iter().all(|n| n.pitch == 36));
    }

    /// The whole reason a part exists: three clips from one idea are the same
    /// idea. A feel applied per copy would give three different humanisations
    /// and the parts would stop being comparable — which is the bug, not a
    /// nicety.
    #[test]
    fn a_parts_feel_is_baked_once_so_every_clip_gets_the_same_notes() {
        let feel = PartFeel {
            humanize_ms: Some(18.0),
            humanize_velocity: Some(12),
            seed: Some(7),
            ..Default::default()
        };
        let r = resolved("break_a", step_part("C1", "x.x.x.x.x.x.x.x."), feel);
        let once = r.notes_for("break_a").unwrap().to_vec();
        let a = move_pitches(&once, &BTreeMap::new(), 0, "break_a").unwrap();
        let b = move_pitches(&once, &BTreeMap::new(), 0, "break_a").unwrap();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.pitch, y.pitch);
            assert_eq!(x.velocity, y.velocity);
            assert!((x.start_time - y.start_time).abs() < 1e-12);
        }
        // And it really was humanised, or the test proves nothing.
        let plain = resolved(
            "p",
            step_part("C1", "x.x.x.x.x.x.x.x."),
            PartFeel::default(),
        );
        let raw = plain.notes_for("p").unwrap();
        assert!(
            raw.iter().zip(&once).any(
                |(r, h)| (r.start_time - h.start_time).abs() > 1e-9 || r.velocity != h.velocity
            ),
            "the feel did nothing, so sameness proves nothing"
        );
    }

    /// The case the story is named after: kits disagree about which note is
    /// the clap.
    #[test]
    fn map_moves_named_pitches_and_transpose_moves_all_of_them() {
        let r = resolved("break_a", step_part("C1", "x..."), PartFeel::default());
        let notes = r.notes_for("break_a").unwrap();
        let mut map = BTreeMap::new();
        map.insert("C1".to_string(), json!("D1"));
        let moved = move_pitches(notes, &map, 0, "break_a").unwrap();
        assert_eq!(moved[0].pitch, 38, "C1 -> D1");

        // map first, then transpose: a map written in the part's own pitches
        // keeps meaning what it says.
        let both = move_pitches(notes, &map, 12, "break_a").unwrap();
        assert_eq!(both[0].pitch, 50, "D1 up an octave");

        let plain = move_pitches(notes, &BTreeMap::new(), -12, "break_a").unwrap();
        assert_eq!(plain[0].pitch, 24);
    }

    #[test]
    fn a_pitch_moved_off_the_keyboard_is_an_error_not_a_lost_note() {
        let r = resolved("break_a", step_part("C1", "x..."), PartFeel::default());
        let e = move_pitches(
            r.notes_for("break_a").unwrap(),
            &BTreeMap::new(),
            -60,
            "break_a",
        )
        .unwrap_err();
        assert!(e.contains("outside 0–127"), "{e}");
    }

    #[test]
    fn an_unknown_part_is_refused_with_the_names_the_document_defines() {
        let r = resolved("break_a", step_part("C1", "x..."), PartFeel::default());
        let e = r.notes_for("brk_a").unwrap_err();
        assert!(e.contains("no part called 'brk_a'"), "{e}");
        assert!(e.contains("'break_a'"), "it lists what there is: {e}");
    }

    #[test]
    fn a_part_nothing_plays_is_named() {
        let mut m = BTreeMap::new();
        m.insert(
            "used".to_string(),
            (step_part("C1", "x..."), PartFeel::default()),
        );
        m.insert(
            "idle".to_string(),
            (step_part("D1", "x..."), PartFeel::default()),
        );
        let r = Resolved::build(&m, 120.0, 4.0).unwrap();
        assert_eq!(r.unreferenced(&["used".to_string()]), vec!["idle"]);
    }

    #[test]
    fn a_part_with_no_notes_is_refused_before_anything_reaches_live() {
        let mut m = BTreeMap::new();
        m.insert(
            "empty".to_string(),
            (NotesInput::default(), PartFeel::default()),
        );
        let e = Resolved::build(&m, 120.0, 4.0).unwrap_err();
        assert!(e.contains("has no notes"), "{e}");
    }
}
