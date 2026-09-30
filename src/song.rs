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
use serde_json::Value;
use std::collections::BTreeMap;

/// The separator between a section's name and its phrase length.
pub const SEP: &str = " · ";
/// The scene whose name holds the song.
pub const SETLIST_PREFIX: &str = "Setlist:";
/// The scene row that holds parked ideas. The same trick as `Setlist:`: a
/// reserved prefix on a scene name, kept by Live's own Save, travelling in
/// the `.als` — an idea you cannot hear is not an idea.
pub const STASH_PREFIX: &str = "Stash:";
/// Bars per phrase for a scene whose name carries no suffix.
pub const DEFAULT_PHRASE: i64 = 16;

/// `"Groove · 8"` → `("Groove", Some(8))`; `"Groove"` → `("Groove", None)`.
/// A `Setlist:` scene is never parsed.
pub fn parse_section_name(name: &str) -> (String, Option<i64>) {
    let text = name.trim();
    if is_reserved_scene(text) {
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

pub fn is_stash_scene(name: &str) -> bool {
    name.trim().starts_with(STASH_PREFIX)
}

/// A scene the server owns rather than the song: the setlist and the stash.
/// Neither is a section, so neither is listed, launched, counted or played.
pub fn is_reserved_scene(name: &str) -> bool {
    is_setlist_scene(name) || is_stash_scene(name)
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
        .filter(|s| !is_reserved_scene(&s.name))
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

// ── Editing a song ──────────────────────────────────────────────────────────
//
// The nouns of `update_song`: the eight edits, the scene rows they move, and
// the setlist arithmetic. All pure — an edit is resolved against a model of
// the set built from one state read, so a list can be checked whole before
// the first command goes out, and a dry run can print exactly what the
// commit will do. The commands live in `sections.rs`.

/// Where an entry goes: a section's name, and which of its entries when the
/// song names it more than once. Positions are how the song is *printed*,
/// never how it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub section: String,
    pub occurrence: Option<usize>,
}

impl Anchor {
    fn parse(v: &Value, field: &str) -> Result<Self, String> {
        match v {
            Value::String(s) if !s.trim().is_empty() => Ok(Self {
                section: s.trim().to_string(),
                occurrence: None,
            }),
            Value::Object(o) => {
                let section = o
                    .get("section")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("{field} needs a section name"))?;
                Ok(Self {
                    section: section.to_string(),
                    occurrence: match o.get("occurrence") {
                        None | Some(Value::Null) => None,
                        Some(n) => Some(occurrence_of(n, field)?),
                    },
                })
            }
            other => Err(format!(
                "{field} is a section's name (\"Drop\") or {{\"section\": \"Drop\", \"occurrence\": 2}}, not {other}"
            )),
        }
    }
}

fn occurrence_of(v: &Value, field: &str) -> Result<usize, String> {
    v.as_u64()
        .filter(|n| *n >= 1)
        .map(|n| n as usize)
        .ok_or_else(|| {
            format!("{field}: occurrence counts from 1 (the first entry of that section)")
        })
}

/// One change to a song: a section, or the setlist.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Change a section that exists: its name, its phrase, its scene tempo,
    /// and the clips of the tracks it names. Every field is optional.
    SetSection {
        section: String,
        rename_to: Option<String>,
        phrase_bars: Option<i64>,
        tempo: Option<f64>,
        clips: BTreeMap<String, Value>,
    },
    /// A copy of a section under a new name, with per-track changes.
    CopySection {
        section: String,
        name: String,
        after: Option<String>,
        changes: BTreeMap<String, Value>,
    },
    /// A row somewhere else in the set. Live has no move: the row is written
    /// again where it is wanted and the old one deleted.
    MoveSection {
        section: String,
        after: Option<String>,
        before: Option<String>,
    },
    /// The row and every entry of it in the song.
    DeleteSection { section: String },
    /// One more entry in the song.
    InsertEntry {
        section: String,
        after: Option<Anchor>,
        before: Option<Anchor>,
        repeats: Option<u32>,
    },
    /// An entry that is already in the song: its repeats, or where it sits.
    SetEntry {
        section: String,
        occurrence: Option<usize>,
        /// `None` = not asked for; `Some(None)` = loop until go.
        repeats: Option<Option<u32>>,
        after: Option<Anchor>,
        before: Option<Anchor>,
    },
    /// One entry of a section, or every entry of it.
    RemoveEntry {
        section: String,
        occurrence: Option<usize>,
        all: bool,
    },
    /// The whole song, in order.
    SetSetlist { setlist: Vec<SetlistEntry> },
}

/// The verbs, in the order the tool description lists them.
pub const EDIT_VERBS: &[&str] = &[
    "set_section",
    "copy_section",
    "move_section",
    "delete_section",
    "insert_entry",
    "set_entry",
    "remove_entry",
    "set_setlist",
];

/// The older spellings a model may reach for, and the verb that does it now.
const EDIT_ALIASES: &[(&str, &str)] = &[
    ("set_clips", "set_section"),
    ("rename_section", "set_section"),
    ("set_phrase", "set_section"),
    ("set_phrase_bars", "set_section"),
    ("set_section_tempo", "set_section"),
    ("set_repeats", "set_entry"),
    ("move_entry", "set_entry"),
    ("add_entry", "insert_entry"),
    ("add_to_song", "insert_entry"),
    ("remove_from_song", "remove_entry"),
    ("delete_entry", "remove_entry"),
    ("set_song", "set_setlist"),
];

impl Edit {
    pub fn verb(&self) -> &'static str {
        match self {
            Edit::SetSection { .. } => "set_section",
            Edit::CopySection { .. } => "copy_section",
            Edit::MoveSection { .. } => "move_section",
            Edit::DeleteSection { .. } => "delete_section",
            Edit::InsertEntry { .. } => "insert_entry",
            Edit::SetEntry { .. } => "set_entry",
            Edit::RemoveEntry { .. } => "remove_entry",
            Edit::SetSetlist { .. } => "set_setlist",
        }
    }

    /// The section an edit is about, for a message.
    pub fn about(&self) -> &str {
        match self {
            Edit::SetSection { section, .. }
            | Edit::CopySection { section, .. }
            | Edit::MoveSection { section, .. }
            | Edit::DeleteSection { section }
            | Edit::InsertEntry { section, .. }
            | Edit::SetEntry { section, .. }
            | Edit::RemoveEntry { section, .. } => section,
            Edit::SetSetlist { .. } => "the song",
        }
    }

    /// Read one edit. The errors name the verb and say what to write, because
    /// this is the first thing a model gets wrong.
    pub fn parse(v: &Value) -> Result<Self, String> {
        let o = v.as_object().ok_or_else(|| {
            format!("an edit is an object like {{\"edit\": \"set_section\", …}}, not {v}")
        })?;
        let verb = o
            .get("edit")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!(
                    "this edit has no \"edit\": one of {}",
                    EDIT_VERBS.join(", ")
                )
            })?;
        let section = |field: &str| -> Result<String, String> {
            o.get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("{verb} needs \"{field}\": the section's name"))
        };
        let anchor = |field: &str| -> Result<Option<Anchor>, String> {
            match o.get(field) {
                None | Some(Value::Null) => Ok(None),
                Some(x) => Anchor::parse(x, field).map(Some),
            }
        };
        let map = |field: &str| -> Result<BTreeMap<String, Value>, String> {
            match o.get(field) {
                None | Some(Value::Null) => Ok(BTreeMap::new()),
                Some(Value::Object(m)) => {
                    Ok(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                }
                Some(other) => Err(format!(
                    "{verb}: {field} is a track-name map like {{\"Hats\": \"none\"}}, not {other}"
                )),
            }
        };
        let repeats = |field: &str| -> Result<Option<u32>, String> {
            match o.get(field) {
                None | Some(Value::Null) => Ok(None),
                Some(n) => n
                    .as_u64()
                    .filter(|n| (1..=512).contains(n))
                    .map(|n| Some(n as u32))
                    .ok_or_else(|| {
                        format!("{verb}: {field} is how many times the section plays (1–512); leave it out to loop until go")
                    }),
            }
        };
        match verb {
            "set_section" => Ok(Edit::SetSection {
                section: section("section")?,
                rename_to: o
                    .get("rename_to")
                    .or_else(|| o.get("name"))
                    .or_else(|| o.get("to"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                phrase_bars: match o.get("phrase_bars").or_else(|| o.get("bars")) {
                    None | Some(Value::Null) => None,
                    Some(n) => Some(n.as_i64().filter(|b| (1..=128).contains(b)).ok_or_else(
                        || "phrase_bars is a whole number of bars, 1–128".to_string(),
                    )?),
                },
                tempo: match o.get("tempo") {
                    None | Some(Value::Null) => None,
                    Some(t) => Some(
                        t.as_f64()
                            .filter(|t| (20.0..=999.0).contains(t))
                            .ok_or_else(|| "tempo is in BPM, 20–999".to_string())?,
                    ),
                },
                clips: map("clips")?,
            }),
            "copy_section" => Ok(Edit::CopySection {
                section: section("section")?,
                name: o
                    .get("as")
                    .or_else(|| o.get("name"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        "copy_section needs \"as\": what to call the copy".to_string()
                    })?,
                after: o
                    .get("after")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string()),
                changes: map("changes")?,
            }),
            "move_section" => Ok(Edit::MoveSection {
                section: section("section")?,
                after: o
                    .get("after")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string()),
                before: o
                    .get("before")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string()),
            }),
            "delete_section" => Ok(Edit::DeleteSection {
                section: section("section")?,
            }),
            "insert_entry" => Ok(Edit::InsertEntry {
                section: section("section")?,
                after: anchor("after")?,
                before: anchor("before")?,
                repeats: repeats("repeats")?,
            }),
            "set_entry" => Ok(Edit::SetEntry {
                section: section("section")?,
                occurrence: match o.get("occurrence") {
                    None | Some(Value::Null) => None,
                    Some(n) => Some(occurrence_of(n, "set_entry")?),
                },
                repeats: match o.get("repeats") {
                    None => None,
                    Some(Value::Null) => Some(None),
                    Some(_) => Some(repeats("repeats")?),
                },
                after: anchor("after")?,
                before: anchor("before")?,
            }),
            "remove_entry" => Ok(Edit::RemoveEntry {
                section: section("section")?,
                occurrence: match o.get("occurrence") {
                    None | Some(Value::Null) => None,
                    Some(n) => Some(occurrence_of(n, "remove_entry")?),
                },
                all: o.get("all").and_then(Value::as_bool).unwrap_or(false),
            }),
            "set_setlist" => {
                let raw = o.get("setlist").or_else(|| o.get("song")).ok_or_else(|| {
                    "set_setlist needs \"setlist\": the song in order".to_string()
                })?;
                let setlist: Vec<SetlistEntry> = serde_json::from_value(raw.clone()).map_err(|e| {
                    format!("set_setlist: {e} — the setlist is [{{\"section\": \"Intro\", \"repeats\": 2}}, …]")
                })?;
                Ok(Edit::SetSetlist { setlist })
            }
            other => Err(match EDIT_ALIASES.iter().find(|(a, _)| *a == other) {
                Some((_, now)) => format!(
                    "'{other}' is not an edit any more — {now} does it. The edits: {}.",
                    EDIT_VERBS.join(", ")
                ),
                None => {
                    let hint = suggest_from_names(
                        other,
                        &EDIT_VERBS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    )
                    .map(|s| format!(" (did you mean {s}?)"))
                    .unwrap_or_default();
                    format!(
                        "'{other}' is not an edit{hint}. The edits: {}.",
                        EDIT_VERBS.join(", ")
                    )
                }
            }),
        }
    }
}

// ── The setlist, as arithmetic ──────────────────────────────────────────────

/// Every position of a section in the song, counting from 0.
pub fn occurrences(entries: &[SetlistEntry], section: &str) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.section.eq_ignore_ascii_case(section.trim()))
        .map(|(i, _)| i)
        .collect()
}

fn positions_text(found: &[usize]) -> String {
    let one_based: Vec<String> = found.iter().map(|i| (i + 1).to_string()).collect();
    match one_based.len() {
        0 => String::new(),
        1 => one_based[0].clone(),
        2 => format!("{} and {}", one_based[0], one_based[1]),
        _ => format!(
            "{} and {}",
            one_based[..one_based.len() - 1].join(", "),
            one_based[one_based.len() - 1]
        ),
    }
}

fn times(n: usize) -> String {
    match n {
        2 => "twice".to_string(),
        n => format!("{n} times"),
    }
}

/// Which entries an edit means. A name the song uses more than once is never
/// guessed at: the refusal prints the call to make.
pub fn pick_entries(
    entries: &[SetlistEntry],
    section: &str,
    occurrence: Option<usize>,
    all: bool,
    verb: &str,
) -> Result<Vec<usize>, String> {
    let found = occurrences(entries, section);
    if found.is_empty() {
        let names: Vec<String> = entries.iter().map(|e| e.section.clone()).collect();
        let hint = suggest_from_names(section, &names)
            .map(|s| format!(" (did you mean {s}?)"))
            .unwrap_or_default();
        return Err(format!(
            "'{section}' is not in the song{hint}: {}",
            setlist_text(entries)
        ));
    }
    if all {
        return Ok(found);
    }
    match occurrence {
        Some(n) => found.get(n - 1).map(|i| vec![*i]).ok_or_else(|| {
            format!(
                "'{section}' is in the song {} (positions {}), so there is no {n}{}.",
                times(found.len()),
                positions_text(&found),
                ordinal_suffix(n)
            )
        }),
        None if found.len() == 1 => Ok(vec![found[0]]),
        None => Err(format!(
            "'{section}' is in the song {} (positions {}). Say which: {{\"edit\": \"{verb}\", \"section\": \"{section}\", \"occurrence\": 1}}{}",
            times(found.len()),
            positions_text(&found),
            if verb == "remove_entry" {
                " — or \"all\": true for every one."
            } else {
                "."
            }
        )),
    }
}

fn ordinal_suffix(n: usize) -> &'static str {
    match n % 10 {
        1 if n % 100 != 11 => "st",
        2 if n % 100 != 12 => "nd",
        3 if n % 100 != 13 => "rd",
        _ => "th",
    }
}

/// The position an anchor names, for an entry that is being put somewhere.
pub fn anchor_position(entries: &[SetlistEntry], a: &Anchor, field: &str) -> Result<usize, String> {
    let found = occurrences(entries, &a.section);
    if found.is_empty() {
        let names: Vec<String> = entries.iter().map(|e| e.section.clone()).collect();
        let hint = suggest_from_names(&a.section, &names)
            .map(|s| format!(" (did you mean {s}?)"))
            .unwrap_or_default();
        return Err(format!(
            "{field}: '{}' is not in the song{hint}: {}",
            a.section,
            setlist_text(entries)
        ));
    }
    match a.occurrence {
        Some(n) => found.get(n - 1).copied().ok_or_else(|| {
            format!(
                "{field}: '{}' is in the song {} (positions {}), so there is no {n}{}.",
                a.section,
                times(found.len()),
                positions_text(&found),
                ordinal_suffix(n)
            )
        }),
        None if found.len() == 1 => Ok(found[0]),
        None => Err(format!(
            "{field}: '{}' is in the song {} (positions {}). Say which with {{\"section\": \"{}\", \"occurrence\": 1}}.",
            a.section,
            times(found.len()),
            positions_text(&found),
            a.section
        )),
    }
}

// ── The scene rows, as a model ──────────────────────────────────────────────

/// One scene row while a plan is being worked out.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The scene name as Live shows it, phrase suffix and all
    pub name: String,
    pub tempo: Option<f64>,
    /// The tracks that have a clip in this row, by track index
    pub clip_tracks: Vec<i64>,
}

impl Row {
    pub fn base(&self) -> String {
        parse_section_name(&self.name).0
    }
    pub fn bars(&self) -> Option<i64> {
        parse_section_name(&self.name).1
    }
    pub fn reserved(&self) -> bool {
        is_reserved_scene(&self.name)
    }
}

/// Every scene row in order, the reserved ones included — because deleting a
/// row moves every row below it, and the `Setlist:` row is one of them. The
/// planner mutates this, so edit 4 sees what edits 1 to 3 did.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows(pub Vec<Row>);

impl Rows {
    pub fn from_state(state: &PerfState) -> Self {
        Rows(
            state
                .scenes
                .iter()
                .map(|sc| Row {
                    name: sc.name.clone(),
                    tempo: sc.tempo,
                    clip_tracks: sc.clip_tracks.clone(),
                })
                .collect(),
        )
    }

    /// A section by name (case-insensitive, with or without the phrase
    /// suffix). Reserved rows are not sections and are never found here.
    pub fn find(&self, which: &str) -> Option<usize> {
        let want = which.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        self.0.iter().position(|r| {
            !r.reserved()
                && (r.base().to_lowercase() == want || r.name.trim().to_lowercase() == want)
        })
    }

    /// The names a producer can say.
    pub fn section_names(&self) -> Vec<String> {
        self.0
            .iter()
            .filter(|r| !r.reserved())
            .map(|r| r.base())
            .collect()
    }

    pub fn setlist_row(&self) -> Option<usize> {
        self.0.iter().position(|r| is_setlist_scene(&r.name))
    }

    pub fn section_count(&self) -> usize {
        self.0.iter().filter(|r| !r.reserved()).count()
    }

    /// "'Drop 3' is not a section", with a spelling hint and what to do.
    pub fn no_such_section(&self, which: &str) -> String {
        let names = self.section_names();
        let hint = suggest_from_names(which, &names)
            .map(|s| format!(" (did you mean {s}?)"))
            .unwrap_or_default();
        format!(
            "'{which}' is not a section{hint}. Sections: {}. make_section creates one.",
            names.join(", ")
        )
    }

    /// The index a new row goes to, given `after`/`before` by name.
    pub fn place_after(
        &self,
        after: Option<&str>,
        before: Option<&str>,
        default_end: bool,
    ) -> Result<usize, String> {
        match (after, before) {
            (Some(_), Some(_)) => Err("give after or before, not both".into()),
            (Some(a), None) => self
                .find(a)
                .map(|i| i + 1)
                .ok_or_else(|| format!("after: {}", self.no_such_section(a))),
            (None, Some(b)) => self
                .find(b)
                .ok_or_else(|| format!("before: {}", self.no_such_section(b))),
            (None, None) if default_end => Ok(self
                .last_section_index()
                .map(|i| i + 1)
                .unwrap_or(self.0.len())),
            (None, None) => Ok(self.0.len()),
        }
    }

    /// The last row that is a section, so a new one lands above the reserved
    /// rows rather than under them.
    pub fn last_section_index(&self) -> Option<usize> {
        self.0.iter().rposition(|r| !r.reserved())
    }

    pub fn insert(&mut self, at: usize, row: Row) {
        let at = at.min(self.0.len());
        self.0.insert(at, row);
    }

    pub fn remove(&mut self, at: usize) -> Row {
        self.0.remove(at)
    }
}

/// A short fingerprint of the set's shape: its tempo, the scene rows, the
/// tracks and which of them have a clip in which row. One id, printed
/// wherever the song is read and checked wherever it is changed, so a plan
/// can tell that the set moved underneath it. It costs nothing — every
/// reader already has what it is made of.
fn shape_revision(
    tempo: f64,
    scenes: &[(String, Vec<i64>)],
    tracks: &[(String, Vec<i64>)],
) -> String {
    let mut text = format!("{tempo:.3}|");
    for (name, slots) in scenes.iter().chain(tracks.iter()) {
        text.push_str(name);
        text.push(':');
        for s in slots {
            text.push_str(&s.to_string());
            text.push(',');
        }
        text.push(';');
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("r{h:016x}")[..5].to_string()
}

/// The set's shape as a performance state reports it.
pub fn state_revision(state: &PerfState) -> String {
    shape_revision(
        state.tempo,
        &state
            .scenes
            .iter()
            .map(|sc| (sc.name.clone(), sc.clip_tracks.clone()))
            .collect::<Vec<_>>(),
        &state
            .tracks
            .iter()
            .map(|t| (t.name.clone(), t.slots_with_clips.clone()))
            .collect::<Vec<_>>(),
    )
}

/// The same set's shape as `get_context` reports it — the same id, from the
/// payload the reader already has.
pub fn context_revision(ctx: &Value) -> String {
    let list = |v: Option<&Value>, key: &str| -> Vec<(String, Vec<i64>)> {
        v.and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| {
                        (
                            x.get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            x.get(key)
                                .and_then(Value::as_array)
                                .map(|c| {
                                    c.iter()
                                        .filter_map(|e| {
                                            e.as_i64()
                                                .or_else(|| e.get("slot").and_then(Value::as_i64))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    shape_revision(
        ctx.pointer("/session/tempo")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        &list(ctx.get("scenes"), "clip_tracks"),
        &list(ctx.get("tracks"), "clips"),
    )
}

/// One planned edit: what it will do, in the words the reply uses, and
/// whether it does anything at all.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub n: usize,
    pub verb: &'static str,
    /// What the plan and the reply both print
    pub line: String,
    /// An edit whose target already holds that value: reported, never sent
    pub no_change: bool,
}

impl Step {
    pub fn text(&self) -> String {
        format!("{:>2}. {:<15}{}", self.n, self.verb, self.line)
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
