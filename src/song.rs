//! Sections and songs. A section is a scene row named `<name> · <bars>`
//! (its phrase length); a song is the setlist written into the name of the
//! `Setlist:` scene. Both live in the Live set, so Live's own Save is the
//! memory and `get_context` reads them back with no server file. Pure
//! logic: parsing and rendering the names, the plan a setlist makes on
//! Live's bar numbers, and the cursor (which entry, which pass) derived from
//! the state the script reports. The tool bodies in `tools.rs` send the
//! commands.

use crate::performance::{PerfState, SceneState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The separator between a section's name and its phrase length.
pub const SEP: &str = " · ";
/// The scene whose name holds the song.
pub const SETLIST_PREFIX: &str = "Setlist:";
/// Bars per phrase for a scene whose name carries no suffix.
pub const DEFAULT_PHRASE: i64 = 16;

/// `"Groove · 8"` → `("Groove", Some(8))`; `"Groove"` → `("Groove", None)`.
/// A `Setlist:` scene is never parsed.
pub fn parse_section_name(name: &str) -> (String, Option<i64>) {
    let text = name.trim();
    if is_setlist_scene(text) {
        return (text.to_string(), None);
    }
    if let Some((head, tail)) = text.rsplit_once(SEP) {
        let tail = tail.trim();
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) && !head.trim().is_empty() {
            if let Ok(bars) = tail.parse::<i64>() {
                return (head.trim().to_string(), Some(bars));
            }
        }
    }
    (text.to_string(), None)
}

/// The scene name for a section: `Groove · 8`, or just the name.
pub fn section_name(base: &str, bars: Option<i64>) -> String {
    match bars {
        Some(b) => format!("{}{SEP}{b}", base.trim()),
        None => base.trim().to_string(),
    }
}

pub fn is_setlist_scene(name: &str) -> bool {
    name.trim().starts_with(SETLIST_PREFIX)
}

/// One entry of the song: a section, looping until `go` unless it carries a
/// repeat count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SetlistEntry {
    /// Section name
    pub section: String,
    /// Passes before the song moves on by itself; without it the section loops until you say go
    #[serde(default)]
    pub repeats: Option<u32>,
}

impl SetlistEntry {
    /// `Intro×2` or `Groove`.
    pub fn text(&self) -> String {
        match self.repeats {
            Some(n) => format!("{}×{n}", self.section),
            None => self.section.clone(),
        }
    }
}

/// `"Setlist: Intro×2 → Groove → Break×1"` → the entries. Also reads `->`,
/// `x2` and `×2` with or without a space. The error names the first
/// unreadable token.
pub fn parse_setlist(name: &str) -> Result<Vec<SetlistEntry>, String> {
    let body = name
        .trim()
        .strip_prefix(SETLIST_PREFIX)
        .ok_or_else(|| format!("not a setlist scene: '{}'", name.trim()))?
        .trim();
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (i, raw) in body
        .replace("->", "→")
        .replace(',', "→")
        .split('→')
        .enumerate()
    {
        let token = raw.trim();
        if token.is_empty() {
            return Err(format!(
                "entry {} of the setlist is empty (two arrows in a row?)",
                i + 1
            ));
        }
        out.push(parse_entry(token).ok_or_else(|| {
            format!(
                "entry {} of the setlist, \"{token}\", is not a section (write \"Name\" or \"Name×4\")",
                i + 1
            )
        })?);
    }
    Ok(out)
}

fn parse_entry(token: &str) -> Option<SetlistEntry> {
    // "Drop×4", "Drop x4", "Drop x 4", "Drop ×4"
    let lower = token.to_lowercase();
    for marker in ["×", " x"] {
        if let Some(pos) = lower.rfind(marker) {
            let count = token[pos + marker.len()..].trim();
            if !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()) {
                let section = token[..pos].trim();
                if section.is_empty() {
                    return None;
                }
                let n: u32 = count.parse().ok()?;
                if n == 0 {
                    return None;
                }
                return Some(SetlistEntry {
                    section: section.to_string(),
                    repeats: Some(n),
                });
            }
        }
    }
    let section = token.trim();
    if section.is_empty() || section.contains('×') {
        return None;
    }
    Some(SetlistEntry {
        section: section.to_string(),
        repeats: None,
    })
}

/// The scene name that stores the song.
pub fn render_setlist(entries: &[SetlistEntry]) -> String {
    let body: Vec<String> = entries.iter().map(SetlistEntry::text).collect();
    format!("{SETLIST_PREFIX} {}", body.join(" → "))
}

/// `Intro×2 → Groove → …` without the prefix.
pub fn setlist_text(entries: &[SetlistEntry]) -> String {
    entries
        .iter()
        .map(SetlistEntry::text)
        .collect::<Vec<_>>()
        .join(" → ")
}

/// A section as the state reports it: a scene row with a name and a phrase.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub index: i64,
    /// The name without the phrase suffix
    pub name: String,
    /// The scene's full name as Live shows it
    pub scene_name: String,
    pub bars: i64,
    /// True when the name carries no suffix (the 16-bar default)
    pub default: bool,
    pub clip_tracks: Vec<i64>,
    pub tempo: Option<f64>,
}

impl Section {
    /// `Groove · 8`, whatever the scene is actually called.
    pub fn label(&self) -> String {
        section_name(&self.name, Some(self.bars))
    }
}

fn section_of(sc: &SceneState) -> Section {
    let (name, suffix) = parse_section_name(&sc.name);
    let bars = suffix
        .or_else(|| sc.phrase_bars.filter(|_| !sc.phrase_default))
        .unwrap_or(DEFAULT_PHRASE)
        .max(1);
    Section {
        index: sc.index,
        name: if name.is_empty() {
            format!("scene {}", sc.index)
        } else {
            name
        },
        scene_name: sc.name.clone(),
        bars,
        default: suffix.is_none() && (sc.phrase_default || sc.phrase_bars.is_none()),
        clip_tracks: sc.clip_tracks.clone(),
        tempo: sc.tempo,
    }
}

/// Every scene row that is not the `Setlist:` scene.
pub fn sections(state: &PerfState) -> Vec<Section> {
    state
        .scenes
        .iter()
        .filter(|s| !is_setlist_scene(&s.name))
        .map(section_of)
        .collect()
}

/// A section by name (case-insensitive, with or without the suffix) or by
/// scene index written as a number.
pub fn find_section<'a>(secs: &'a [Section], which: &str) -> Option<&'a Section> {
    let want = which.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    if let Ok(i) = want.parse::<i64>() {
        return secs.iter().find(|s| s.index == i);
    }
    secs.iter()
        .find(|s| s.name.to_lowercase() == want || s.scene_name.trim().to_lowercase() == want)
}

pub fn section_by_index(secs: &[Section], index: i64) -> Option<&Section> {
    secs.iter().find(|s| s.index == index)
}

/// The names the producer can say.
pub fn section_names(secs: &[Section]) -> String {
    secs.iter()
        .filter(|s| !s.clip_tracks.is_empty() || !s.default)
        .map(|s| s.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A close name for a typo, when one is close enough to be meant.
pub fn suggest(name: &str, secs: &[Section]) -> Option<String> {
    let names: Vec<String> = secs.iter().map(|s| s.name.clone()).collect();
    suggest_from_names(name, &names)
}

pub fn suggest_from_names(name: &str, names: &[String]) -> Option<String> {
    let want = name.trim().to_lowercase();
    names
        .iter()
        .map(|s| (edit_distance(&want, &s.to_lowercase()), s))
        .filter(|(d, s)| *d <= 2 && *d * 3 <= s.len())
        .min_by_key(|(d, _)| *d)
        .map(|(_, s)| s.clone())
}

/// The first entry that names no section, as "'grove' is not a section
/// (did you mean Groove?)".
pub fn unknown_entry(entries: &[SetlistEntry], names: &[String]) -> Option<String> {
    entries
        .iter()
        .find(|e| {
            !names
                .iter()
                .any(|n| n.eq_ignore_ascii_case(e.section.trim()))
        })
        .map(|e| {
            let hint = suggest_from_names(&e.section, names)
                .map(|s| format!(" (did you mean {s}?)"))
                .unwrap_or_default();
            format!("'{}' is not a section{hint}", e.section)
        })
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The `Setlist:` scene, if the set has one.
pub fn setlist_scene(state: &PerfState) -> Option<&SceneState> {
    state.scenes.iter().find(|s| is_setlist_scene(&s.name))
}

/// The song stored in the set: `None` when there is no `Setlist:` scene, an
/// error naming the first unreadable token when there is one that does not
/// parse.
pub fn read_setlist(state: &PerfState) -> Result<Option<Vec<SetlistEntry>>, String> {
    match setlist_scene(state) {
        None => Ok(None),
        Some(sc) => parse_setlist(&sc.name).map(Some).map_err(|e| {
            format!(
                "The 'Setlist:' scene reads \"{}\"; {e}. Fix the scene name in Live or set_song again.",
                sc.name.trim()
            )
        }),
    }
}

/// Every entry names a section, and no section would leave the set silent
/// when it fires (no clip on any track, and no track kept playing through
/// its row).
pub fn validate_setlist(
    entries: &[SetlistEntry],
    secs: &[Section],
    state: &PerfState,
) -> Result<(), String> {
    if entries.is_empty() {
        return Err("A song needs at least one section.".into());
    }
    for e in entries {
        let Some(sec) = find_section(secs, &e.section) else {
            let hint = suggest(&e.section, secs)
                .map(|s| format!(" (did you mean {s}?)"))
                .unwrap_or_default();
            return Err(format!(
                "'{}' is not a section{hint}. Sections: {}. make_section first.",
                e.section,
                section_names(secs)
            ));
        };
        if e.repeats == Some(0) {
            return Err(format!("'{}': repeats must be at least 1", e.section));
        }
        let kept = state
            .tracks
            .iter()
            .any(|t| t.no_stop_slots.contains(&sec.index));
        if sec.clip_tracks.is_empty() && !kept {
            return Err(format!(
                "'{}' has no clips (scene {}): the song would go silent when it fires. Give it clips, or keep_track_playing a track through it.",
                sec.name, sec.index
            ));
        }
    }
    Ok(())
}

/// Where a jump came from, so `back` can return there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JumpFrom {
    pub section: String,
    pub position: Option<usize>,
}

/// The song cursor the server keeps in the performance record. Everything
/// else about the song is read back from the set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Song {
    pub entries: Vec<SetlistEntry>,
    /// The setlist entry the plan is on (None: a section outside the setlist)
    pub position: Option<usize>,
    /// The section playing or queued
    pub current: String,
    /// The cue that holds the counted jumps, while one is pending
    pub plan_cue_id: Option<i64>,
    pub jump_history: Vec<JumpFrom>,
    /// The bar the current section was fired on, as planned
    pub started_bar: i64,
}

/// One planned jump.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanStep {
    pub bar: i64,
    pub section: String,
    pub scene_index: i64,
    pub repeats: Option<u32>,
    pub entry: Option<usize>,
}

/// A plan: the counted jumps from a start, up to the first section that
/// waits for `go`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// steps[0] is the first section at the start bar; the rest are the jumps
    pub steps: Vec<PlanStep>,
    /// The section that loops until go (the last step), if the plan waits
    pub waits_at: Option<String>,
    /// When every entry is counted: the bar the last one ends on (it keeps looping)
    pub runs_out_at: Option<i64>,
}

/// Plan from `first` fired at `start_bar` with `first_repeats`, continuing
/// with the setlist entries after `position` (None: the entries from the
/// top are not touched; the target just loops).
pub fn plan(
    secs: &[Section],
    entries: &[SetlistEntry],
    first: &Section,
    first_repeats: Option<u32>,
    position: Option<usize>,
    start_bar: i64,
) -> Result<Plan, String> {
    let mut out = Plan::default();
    out.steps.push(PlanStep {
        bar: start_bar,
        section: first.name.clone(),
        scene_index: first.index,
        repeats: first_repeats,
        entry: position,
    });
    let Some(mut repeats) = first_repeats else {
        out.waits_at = Some(first.name.clone());
        return Ok(out);
    };
    let mut bar = start_bar;
    let mut sec = first.clone();
    let mut next = position.map(|p| p + 1).unwrap_or(entries.len());
    loop {
        bar += repeats as i64 * sec.bars;
        let Some(entry) = entries.get(next) else {
            out.runs_out_at = Some(bar);
            return Ok(out);
        };
        sec = find_section(secs, &entry.section)
            .cloned()
            .ok_or_else(|| format!("'{}' is not a section", entry.section))?;
        out.steps.push(PlanStep {
            bar,
            section: sec.name.clone(),
            scene_index: sec.index,
            repeats: entry.repeats,
            entry: Some(next),
        });
        match entry.repeats {
            Some(n) => repeats = n,
            None => {
                out.waits_at = Some(sec.name.clone());
                return Ok(out);
            }
        }
        next += 1;
        if out.steps.len() > 64 {
            return Err("a song plan is at most 64 jumps ahead; give a later section no count so the plan waits there".into());
        }
    }
}

/// Where the song is, read from the state: the section whose row plays,
/// its setlist position (searched forward from the cursor, then anywhere),
/// and the pass it is in.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub section: Option<Section>,
    /// The setlist entry the song is on; through a detour, the entry it left
    pub position: Option<usize>,
    /// False while the playing section is not the entry at `position`
    pub in_setlist: bool,
    pub pass: u32,
    pub repeats: Option<u32>,
    pub started_bar: Option<i64>,
}

pub fn cursor(state: &PerfState, song: &Song, secs: &[Section]) -> Cursor {
    let scene_index = state
        .phrase
        .as_ref()
        .map(|p| p.scene_index)
        .or(state.current_scene);
    let section = scene_index.and_then(|i| section_by_index(secs, i).cloned());
    let name = section.as_ref().map(|s| s.name.to_lowercase());
    let matches = |i: usize| {
        song.entries
            .get(i)
            .is_some_and(|e| Some(e.section.to_lowercase()) == name)
    };
    let position = match (&name, song.position) {
        (None, p) => p,
        (Some(_), Some(p)) if matches(p) => Some(p),
        (Some(_), Some(p)) => (p..song.entries.len())
            .find(|i| matches(*i))
            .or_else(|| (0..song.entries.len()).find(|i| matches(*i)))
            .or(Some(p)),
        (Some(_), None) => (0..song.entries.len()).find(|i| matches(*i)),
    };
    let in_setlist = name.is_some() && position.is_some_and(matches);
    let started_bar = state.phrase.as_ref().map(|p| p.started_bar);
    let pass = match (state.phrase.as_ref(), &section) {
        (Some(p), Some(s)) if s.bars > 0 => {
            ((state.bar - p.started_bar).max(0) / s.bars + 1) as u32
        }
        _ => 1,
    };
    let repeats = if in_setlist {
        position.and_then(|p| song.entries[p].repeats)
    } else {
        None
    };
    Cursor {
        section,
        position,
        in_setlist,
        pass,
        repeats,
        started_bar,
    }
}

/// `Song: Intro×2 → [Groove ×4: pass 2] → Groove+Pad → …`
pub fn song_line(song: &Song, cur: &Cursor) -> String {
    let mut parts = Vec::new();
    for (i, e) in song.entries.iter().enumerate() {
        if cur.in_setlist && cur.position == Some(i) {
            let how = match e.repeats {
                Some(n) => format!("pass {} of {n}", cur.pass.min(n)),
                None => "looping".to_string(),
            };
            parts.push(format!("[{}: {how}]", e.section));
        } else {
            parts.push(e.text());
        }
    }
    let mut line = format!("Song: {}", parts.join(" → "));
    if !cur.in_setlist {
        if let Some(s) = &cur.section {
            line.push_str(&format!(" · now in '{}' (not in the setlist)", s.name));
        }
    }
    line
}

/// Live's meter units as dB, through the curve Live itself draws the meters
/// against — the fader taper, read out of Live by `get_meter_scale`. A meter
/// value is not linear amplitude, so 20·log10 of it is not dB: that is why a
/// 12 dB fader move used to read as about 2 dB.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeterScale {
    /// (meter value, dB), ascending by value.
    pub points: Vec<(f64, f64)>,
}

impl MeterScale {
    pub fn from_value(v: &serde_json::Value) -> Self {
        let points = v
            .get("points")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let pair = p.as_array()?;
                        Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Self { points }
    }

    /// True when the Remote Script gave no curve; [`meter_db`] — the same law,
    /// measured — answers instead, so a reading is always real dB.
    pub fn is_empty(&self) -> bool {
        self.points.len() < 2
    }

    /// A meter reading as dB, interpolated on Live's own curve.
    pub fn db(&self, v: f64) -> f64 {
        if v <= 0.0 {
            return -80.0;
        }
        if self.is_empty() {
            return meter_db(v);
        }
        let first = self.points[0];
        if v <= first.0 {
            return first.1;
        }
        for pair in self.points.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            if v <= x1 {
                if x1 - x0 <= 0.0 {
                    return y1;
                }
                return y0 + (y1 - y0) * (v - x0) / (x1 - x0);
            }
        }
        self.points[self.points.len() - 1].1
    }
}

/// Live's meter units as dB. The meters are linear in dB: 0.0 reads −70 dB
/// and 1.0 reads +6 dB, measured on Live 12.4.6 against a file of known level
/// over 42 dB (a straight-line fit with zero residuals). This is the law the
/// Remote Script hands over in `get_meter_scale`; it is here as well so a
/// script that predates that command still reads in real dB.
pub const METER_FLOOR_DB: f64 = -70.0;
pub const METER_TOP_DB: f64 = 6.0;

pub fn meter_db(v: f64) -> f64 {
    if v <= 0.0 {
        return -80.0;
    }
    ((METER_TOP_DB - METER_FLOOR_DB) * v + METER_FLOOR_DB).clamp(-80.0, METER_TOP_DB)
}

/// `−4.0 dB` with a real minus sign.
pub fn fmt_db(db: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, db.abs());
    if db < 0.0 && s.trim_start_matches(['0', '.']).is_empty() {
        return s;
    }
    if db < 0.0 {
        format!("−{s}")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn section_names_round_trip() {
        assert_eq!(parse_section_name("Groove · 8"), ("Groove".into(), Some(8)));
        assert_eq!(
            parse_section_name("Groove+Pad · 16"),
            ("Groove+Pad".into(), Some(16))
        );
        assert_eq!(parse_section_name("Groove"), ("Groove".into(), None));
        assert_eq!(
            parse_section_name("  Break · x "),
            ("Break · x".into(), None)
        );
        assert_eq!(
            parse_section_name("Setlist: Intro · 8"),
            ("Setlist: Intro · 8".into(), None)
        );
        assert_eq!(section_name("Groove", Some(8)), "Groove · 8");
        assert_eq!(section_name(" Groove ", None), "Groove");
        let (b, n) = parse_section_name(&section_name("A · B", Some(4)));
        assert_eq!((b.as_str(), n), ("A · B", Some(4)));
    }

    #[test]
    fn setlists_parse_and_render() {
        let entries =
            parse_setlist("Setlist: Intro×2 → Groove → Break x1 → Drop ×4 -> Outro").unwrap();
        assert_eq!(
            entries,
            vec![
                SetlistEntry {
                    section: "Intro".into(),
                    repeats: Some(2)
                },
                SetlistEntry {
                    section: "Groove".into(),
                    repeats: None
                },
                SetlistEntry {
                    section: "Break".into(),
                    repeats: Some(1)
                },
                SetlistEntry {
                    section: "Drop".into(),
                    repeats: Some(4)
                },
                SetlistEntry {
                    section: "Outro".into(),
                    repeats: None
                },
            ]
        );
        assert_eq!(
            render_setlist(&entries),
            "Setlist: Intro×2 → Groove → Break×1 → Drop×4 → Outro"
        );
        assert_eq!(parse_setlist(&render_setlist(&entries)).unwrap(), entries);
        assert_eq!(parse_setlist("Setlist:").unwrap(), vec![]);
        // A name with an x in it is a name; commas separate too.
        assert_eq!(parse_setlist("Setlist: Mix").unwrap()[0].section, "Mix");
        let loose = parse_setlist("Setlist: intro x2, grove x4").unwrap();
        assert_eq!(loose.len(), 2);
        assert_eq!(
            loose[1],
            SetlistEntry {
                section: "grove".into(),
                repeats: Some(4)
            }
        );
        let names: Vec<String> = ["Intro", "Groove"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            unknown_entry(&loose, &names).as_deref(),
            Some("'grove' is not a section (did you mean Groove?)")
        );
        assert_eq!(
            parse_setlist("Setlist: Max x3").unwrap()[0],
            SetlistEntry {
                section: "Max".into(),
                repeats: Some(3)
            }
        );
        let e = parse_setlist("Setlist: Intro → → Groove").unwrap_err();
        assert!(e.contains("entry 2"), "{e}");
        let e = parse_setlist("Setlist: Intro×0").unwrap_err();
        assert!(e.contains("entry 1") && e.contains("Intro×0"), "{e}");
        assert!(parse_setlist("Groove · 8").is_err());
    }

    fn state() -> PerfState {
        PerfState::from_value(&json!({
            "is_playing": true, "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
            "beat": 117.5, "bar": 30, "beat_in_bar": 2, "clip_trigger_quantization": 4,
            "tracks": [
                {"index": 0, "name": "Kick", "playing_slot_index": 1, "slots_with_clips": [0, 1, 2, 3, 4]},
                {"index": 1, "name": "Bass", "playing_slot_index": 1, "slots_with_clips": [1, 2, 4]},
                {"index": 2, "name": "Pad", "playing_slot_index": -1, "slots_with_clips": [2, 3]}
            ],
            "scenes": [
                {"index": 0, "name": "Intro · 8", "clip_tracks": [0]},
                {"index": 1, "name": "Groove · 8", "is_playing": true, "clip_tracks": [0, 1]},
                {"index": 2, "name": "Groove+Pad · 8", "clip_tracks": [0, 1, 2]},
                {"index": 3, "name": "Break · 16", "clip_tracks": [0, 2]},
                {"index": 4, "name": "Drop", "clip_tracks": [0, 1], "phrase_bars": 16, "phrase_default": true},
                {"index": 5, "name": "Outro · 8", "clip_tracks": []},
                {"index": 6, "name": "Setlist: Intro×2 → Groove → Groove+Pad → Break → Drop → Groove", "clip_tracks": []}
            ],
            "phrase": {"scene_index": 1, "started_bar": 17, "bars": 8, "ends_bar": 33, "default": false},
            "current_scene": 1,
            "cues": [], "events": []
        }))
        .unwrap()
    }

    #[test]
    fn sections_come_from_the_scene_names() {
        let s = state();
        let secs = sections(&s);
        assert_eq!(secs.len(), 6, "the Setlist scene is not a section");
        assert_eq!(secs[3].name, "Break");
        assert_eq!(secs[3].bars, 16);
        assert!(!secs[3].default);
        assert_eq!(secs[4].name, "Drop");
        assert_eq!(secs[4].bars, 16);
        assert!(secs[4].default);
        assert_eq!(find_section(&secs, "groove+pad").unwrap().index, 2);
        assert_eq!(find_section(&secs, "Break · 16").unwrap().index, 3);
        assert_eq!(find_section(&secs, "3").unwrap().name, "Break");
        assert!(find_section(&secs, "Peak").is_none());
        assert_eq!(suggest("grove", &secs).as_deref(), Some("Groove"));
        assert_eq!(suggest("Peak", &secs), None);
        let entries = read_setlist(&s).unwrap().unwrap();
        assert_eq!(entries.len(), 6);
        assert!(validate_setlist(&entries, &secs, &s).is_ok());
        let e = validate_setlist(
            &[SetlistEntry {
                section: "Outro".into(),
                repeats: None,
            }],
            &secs,
            &s,
        )
        .unwrap_err();
        assert!(e.contains("'Outro' has no clips"), "{e}");
        let e = validate_setlist(
            &[SetlistEntry {
                section: "grove".into(),
                repeats: None,
            }],
            &secs,
            &s,
        )
        .unwrap_err();
        assert!(
            e.contains("did you mean Groove") && e.contains("make_section first"),
            "{e}"
        );
        // A track kept playing through an empty row keeps the set from going silent.
        let mut s2 = state();
        s2.tracks[2].no_stop_slots = vec![5];
        s2.tracks[2].playing_slot_index = 2;
        assert!(validate_setlist(
            &[SetlistEntry {
                section: "Outro".into(),
                repeats: None
            }],
            &secs,
            &s2
        )
        .is_ok());
    }

    #[test]
    fn a_plan_counts_phrases_and_waits_at_the_first_uncounted_entry() {
        let s = state();
        let secs = sections(&s);
        let entries = read_setlist(&s).unwrap().unwrap();
        // play_song from Intro at bar 1: Intro×2 = 16 bars, then Groove waits.
        let p = plan(&secs, &entries, &secs[0], Some(2), Some(0), 1).unwrap();
        assert_eq!(p.steps.len(), 2);
        assert_eq!(
            (p.steps[1].bar, p.steps[1].section.as_str()),
            (17, "Groove")
        );
        assert_eq!(p.waits_at.as_deref(), Some("Groove"));
        // jump_to Drop ×8 at bar 41: eight 16-bar passes, then Groove at 169 (loops).
        let p = plan(&secs, &entries, &secs[4], Some(8), Some(4), 41).unwrap();
        assert_eq!(p.steps.len(), 2);
        assert_eq!(
            (p.steps[1].bar, p.steps[1].section.as_str()),
            (169, "Groove")
        );
        // Every entry counted: the plan runs out and says where.
        let counted: Vec<SetlistEntry> = ["Intro", "Groove"]
            .iter()
            .map(|n| SetlistEntry {
                section: n.to_string(),
                repeats: Some(2),
            })
            .collect();
        let p = plan(&secs, &counted, &secs[0], Some(2), Some(0), 1).unwrap();
        assert_eq!(p.waits_at, None);
        assert_eq!(p.runs_out_at, Some(33));
        // A detour: no setlist position, the target just loops.
        let p = plan(&secs, &entries, &secs[3], None, None, 49).unwrap();
        assert_eq!(p.steps.len(), 1);
        assert_eq!(p.waits_at.as_deref(), Some("Break"));
    }

    #[test]
    fn the_cursor_finds_the_entry_and_the_pass() {
        let s = state();
        let secs = sections(&s);
        let song = Song {
            entries: read_setlist(&s).unwrap().unwrap(),
            position: Some(0),
            current: "Intro".into(),
            ..Default::default()
        };
        // The plan moved on to Groove (entry 1) since the cursor was written.
        let c = cursor(&s, &song, &secs);
        assert_eq!(c.section.as_ref().unwrap().name, "Groove");
        assert_eq!(c.position, Some(1));
        assert_eq!(
            c.pass, 2,
            "bar 30 with 8-bar phrases from 17 is the second pass"
        );
        assert_eq!(c.started_bar, Some(17));
        let line = song_line(&song, &c);
        assert_eq!(
            line,
            "Song: Intro×2 → [Groove: looping] → Groove+Pad → Break → Drop → Groove"
        );
        // A counted entry shows its pass; a detour says so.
        let mut song2 = song.clone();
        song2.entries[1].repeats = Some(4);
        let c2 = cursor(&s, &song2, &secs);
        assert!(song_line(&song2, &c2).contains("[Groove: pass 2 of 4]"));
        let mut s3 = state();
        s3.phrase.as_mut().unwrap().scene_index = 5;
        let c3 = cursor(&s3, &song, &secs);
        assert_eq!(c3.position, Some(0), "a detour keeps the entry it left");
        assert!(!c3.in_setlist);
        assert_eq!(c3.repeats, None);
        assert!(song_line(&song, &c3).ends_with("now in 'Outro' (not in the setlist)"));
    }

    #[test]
    fn lives_own_curve_converts_a_meter_reading() {
        // Live draws its meters against the fader taper: a 12 dB fader move
        // must read as 12 dB, which 20·log10 of the meter value does not.
        let scale = MeterScale::from_value(&json!({"points": [
            [0.0, -80.0], [0.4, -30.0], [0.7, -12.0], [0.85, 0.0], [1.0, 6.0]]}));
        assert!(!scale.is_empty());
        assert!((scale.db(0.85) - 0.0).abs() < 1e-9, "unity");
        assert!((scale.db(1.0) - 6.0).abs() < 1e-9, "the top");
        assert!((scale.db(0.55) - (-21.0)).abs() < 1e-9, "interpolated");
        assert_eq!(scale.db(0.0), -80.0, "the floor");
        let unity = scale.db(0.85);
        let cut = scale.db(0.7);
        assert!(
            (unity - cut - 12.0).abs() < 1e-9,
            "a 12 dB move reads as 12 dB"
        );
        // Without a curve the old formula is the fallback, and the caller
        // says "meter units" rather than dB.
        let none = MeterScale::default();
        // With no curve from the script the same measured law answers, so a
        // reading is still real dB.
        assert!(none.is_empty());
        assert!((none.db(0.5) - (-32.0)).abs() < 1e-9, "{}", none.db(0.5));
    }

    #[test]
    fn meters_read_as_db() {
        // Measured on Live 12.4.6: a -12.0 dBFS file read 0.76314, and every
        // 12 dB of fader moved the value by 0.15789 — dB = 76·v − 70.
        assert_eq!(fmt_db(meter_db(1.0), 1), "6.0");
        assert_eq!(fmt_db(meter_db(0.0), 1), "−80.0");
        assert_eq!(fmt_db(meter_db(0.76314), 1), "−12.0");
        assert_eq!(fmt_db(meter_db(0.60524), 1), "−24.0");
        assert_eq!(fmt_db(meter_db(0.84209), 1), "−6.0");
        assert_eq!(fmt_db(meter_db(0.28945), 1), "−48.0");
        // 0 dBFS, full scale, sits at 0.921 — not at 1.0.
        assert!((meter_db(0.92105)).abs() < 0.01, "{}", meter_db(0.92105));
        // A 12 dB fader move must move the reading by 12 dB.
        assert!(
            ((meter_db(0.76314) - meter_db(0.60524)) - 12.0).abs() < 0.01,
            "{}",
            meter_db(0.76314) - meter_db(0.60524)
        );
        assert_eq!(fmt_db(-6.4, 0), "−6");
        assert_eq!(fmt_db(-0.04, 1), "0.0");
    }
}
