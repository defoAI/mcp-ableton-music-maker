//! Set memory on request: `export_set` reads the set into a `build_song`
//! document plus the sections and the setlist and writes it under
//! `state_dir()/sets/<name>.json`; `import_set` rebuilds it through
//! `build_song` and `set_song`. Nothing here runs on its own: the file
//! holds the producer's notes and names and appears only when asked for.
//! The Live set itself stays the memory (scene names); this is a backup.

use crate::connection::LiveState;
use crate::notes::NotesInput;
use crate::song::{self, SetlistEntry};
use crate::tools::{
    self, live_err, performance_running, require, BuildSongParams, Note, SongClip, SongScene,
    SongTrack, ToolResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExportSetParams {
    /// A name for the file (letters, digits, - and _), e.g. "techno-friday"
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImportSetParams {
    /// The exported set's name (its file under the sets folder)
    pub name: String,
    /// Rebuild into a set that already has tracks (default false: an occupied set is refused)
    #[serde(default)]
    pub merge: bool,
    /// Validate and describe the document without touching Live
    #[serde(default)]
    pub dry_run: bool,
}

/// A track as exported: a build_song track plus what the set had on it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetTrack {
    pub name: String,
    pub kind: String,
    /// The first instrument's browser URI when the library index knows it
    #[serde(default)]
    pub instrument: Option<String>,
    /// Every device's name, in chain order
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub volume: Option<f64>,
    #[serde(default)]
    pub pan: Option<f64>,
    #[serde(default)]
    pub color_index: Option<i64>,
    #[serde(default)]
    pub sends: BTreeMap<String, f64>,
    #[serde(default)]
    pub mute: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetClip {
    pub track: String,
    pub slot: i64,
    #[serde(default)]
    pub name: String,
    pub length: f64,
    /// The notes in whichever compact form fits them — `steps` for a part on
    /// a grid, `notes_csv` otherwise — which is what `build_song` takes, so
    /// the document can be posted straight back. A document written before
    /// this carried `notes: [...]`, which still reads.
    #[serde(flatten)]
    pub notes: NotesInput,
    /// An audio clip's file, when it had one (not rebuilt; named for the reader)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

impl SetClip {
    /// The notes themselves, whatever form they were written in.
    pub fn notes(&self) -> Vec<Note> {
        crate::notes::expand(&self.notes).unwrap_or_default()
    }
}

/// Where a clip sits in the Arrangement. A `build_song` placement plus the
/// clip's name, which `build_song` ignores and a reader wants.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SetPlacement {
    pub track: String,
    #[serde(default)]
    pub slot: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Beat positions, when they are not evenly spaced
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub times: Vec<f64>,
    /// Or an evenly spaced run: from `start`, every `step`, up to `end`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

impl SetPlacement {
    /// Every beat position this placement names.
    pub fn beats(&self) -> Vec<f64> {
        if !self.times.is_empty() {
            return self.times.clone();
        }
        let (Some(start), Some(end), Some(step)) = (self.start, self.end, self.step) else {
            return self.start.map(|s| vec![s]).unwrap_or_default();
        };
        if step <= 0.0 {
            return vec![start];
        }
        let mut out = Vec::new();
        let mut t = start;
        while t < end - 1e-6 && out.len() < 4096 {
            out.push(t);
            t += step;
        }
        out
    }
}

/// A locator in the Arrangement.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SetLocator {
    pub name: String,
    pub time: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetSection {
    pub index: i64,
    pub name: String,
    #[serde(default)]
    pub phrase_bars: Option<i64>,
    #[serde(default)]
    pub tempo: Option<f64>,
}

/// The exported document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetDocument {
    pub name: String,
    pub exported_at: String,
    #[serde(default)]
    pub live_version: String,
    #[serde(default)]
    pub tempo: Option<f64>,
    #[serde(default)]
    pub signature: Option<(i64, i64)>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub tracks: Vec<SetTrack>,
    #[serde(default)]
    pub clips: Vec<SetClip>,
    #[serde(default)]
    pub sections: Vec<SetSection>,
    #[serde(default)]
    pub setlist: Vec<SetlistEntry>,
    /// Where those clips sit in the Arrangement
    #[serde(default)]
    pub placements: Vec<SetPlacement>,
    /// The Arrangement's locators
    #[serde(default)]
    pub locators: Vec<SetLocator>,
    /// What the set looked like when this was read. Post it back and
    /// `update_song` checks the set has not moved under you.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default)]
    pub returns: Vec<String>,
    #[serde(default)]
    pub master_volume: Option<f64>,
}

/// A file name from a set name: letters, digits, `-` and `_` only.
pub fn file_name(name: &str) -> Result<String, String> {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_matches('-').to_string();
    if clean.is_empty() {
        return Err("give a name for the set, e.g. \"techno-friday\"".into());
    }
    Ok(clean)
}

pub fn path_for(name: &str) -> Result<PathBuf, String> {
    Ok(crate::state::sets_dir().join(format!("{}.json", file_name(name)?)))
}

/// Build the document from a session snapshot (tracks, clips with notes,
/// sends, devices), the context (scenes, key, Live version) and the
/// library index (device names → URIs). Pure.
pub fn document(
    name: &str,
    snapshot: &Value,
    context: &Value,
    library: Option<&crate::library::Index>,
    exported_at: String,
) -> SetDocument {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let f = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let session = snapshot.get("session").cloned().unwrap_or(Value::Null);
    let ctx_session = context.get("session").cloned().unwrap_or(Value::Null);
    let uri_of = |device_name: &str| -> Option<String> {
        let ix = library?;
        let want = device_name.trim().to_lowercase();
        ix.items
            .iter()
            .find(|i| {
                i.name.to_lowercase() == want
                    && (i.category == "instruments" || i.category == "sounds" || i.is_device)
            })
            .or_else(|| ix.items.iter().find(|i| i.name.to_lowercase() == want))
            .map(|i| i.uri.clone())
    };
    let mut tracks = Vec::new();
    let mut clips = Vec::new();
    let mut placements: Vec<SetPlacement> = Vec::new();
    for t in snapshot
        .get("tracks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let name = s(&t, "name");
        let is_audio = t
            .get("is_audio_track")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let devices: Vec<String> = t
            .get("devices")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|d| s(d, "name")).collect())
            .unwrap_or_default();
        let instrument = devices.iter().find_map(|d| uri_of(d));
        let mut sends = BTreeMap::new();
        for send in t
            .get("sends")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(v) = f(&send, "value") {
                if v > 0.0 {
                    sends.insert(s(&send, "name"), v);
                }
            }
        }
        tracks.push(SetTrack {
            name: name.clone(),
            kind: if is_audio {
                "audio".into()
            } else {
                "midi".into()
            },
            instrument,
            devices,
            volume: f(&t, "volume"),
            pan: f(&t, "panning"),
            color_index: t.get("color_index").and_then(Value::as_i64),
            sends,
            mute: t.get("mute").and_then(Value::as_bool).unwrap_or(false),
        });
        for slot in t
            .get("clip_slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let Some(clip) = slot.get("clip").filter(|c| c.is_object()) else {
                continue;
            };
            let notes: Vec<Note> = clip
                .get("notes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|n| serde_json::from_value(n.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            clips.push(SetClip {
                track: name.clone(),
                slot: slot.get("index").and_then(Value::as_i64).unwrap_or(0),
                name: s(clip, "name"),
                length: f(clip, "length").unwrap_or(4.0),
                notes: crate::notes::compact(&notes),
                file_path: clip
                    .get("file_path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
        placements.extend(placements_of(&name, &t, &clips));
    }
    let mut sections = Vec::new();
    let mut setlist = Vec::new();
    for sc in context
        .get("scenes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let scene_name = s(&sc, "name");
        if song::is_setlist_scene(&scene_name) {
            setlist = song::parse_setlist(&scene_name).unwrap_or_default();
            continue;
        }
        // The stash is the producer's parked ideas, not part of the song an
        // export rebuilds.
        if song::is_stash_scene(&scene_name) {
            continue;
        }
        let (base, bars) = song::parse_section_name(&scene_name);
        sections.push(SetSection {
            index: sc.get("index").and_then(Value::as_i64).unwrap_or(0),
            name: base,
            phrase_bars: bars,
            tempo: sc.get("tempo").and_then(Value::as_f64).filter(|t| *t > 0.0),
        });
    }
    let key = match (
        s(&ctx_session, "root_note_name"),
        s(&ctx_session, "scale_name"),
    ) {
        (r, sc) if !r.is_empty() && !sc.is_empty() => Some(format!("{r} {sc}")),
        _ => None,
    };
    let locators: Vec<SetLocator> = snapshot
        .get("cue_points")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| SetLocator {
                    name: s(c, "name"),
                    time: f(c, "time").unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut doc = SetDocument {
        name: name.to_string(),
        exported_at,
        live_version: s(context, "live_version"),
        tempo: f(&session, "tempo")
            .or_else(|| f(&ctx_session, "tempo"))
            .map(|t| (t * 100.0).round() / 100.0),
        signature: match (
            session.get("signature_numerator").and_then(Value::as_i64),
            session.get("signature_denominator").and_then(Value::as_i64),
        ) {
            (Some(n), Some(d)) => Some((n, d)),
            _ => None,
        },
        key,
        tracks,
        clips,
        sections,
        setlist,
        returns: context
            .get("returns")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|r| s(r, "name")).collect())
            .unwrap_or_default(),
        master_volume: f(&ctx_session, "master_volume").or_else(|| {
            snapshot
                .pointer("/master_track/volume")
                .and_then(Value::as_f64)
        }),
        placements: Vec::new(),
        locators: Vec::new(),
        revision: None,
    };
    doc.placements = placements;
    doc.locators = locators;
    doc.revision = Some(song::context_revision(context));
    doc
}

/// One track's Arrangement clips as placements: grouped by the Session clip
/// they carry the name of, and written as a run (`start`, `end`, `step`)
/// when they are evenly spaced, which is what a placed section looks like.
fn placements_of(track: &str, t: &Value, clips: &[SetClip]) -> Vec<SetPlacement> {
    let mut by_name: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for c in t
        .get("arrangement_clips")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let name = c
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let start = c.get("start_time").and_then(Value::as_f64).unwrap_or(0.0);
        by_name.entry(name).or_default().push(start);
    }
    let mut out = Vec::new();
    for (name, mut times) in by_name {
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // The Session clip this came from, by name on the same track; a clip
        // the producer renamed or recorded in the Arrangement has none, and
        // keeps its name so a reader still knows what sits there.
        let slot = clips
            .iter()
            .find(|c| c.track == track && c.name == name)
            .map(|c| c.slot)
            .unwrap_or(0);
        let step = even_step(&times);
        let mut p = SetPlacement {
            track: track.to_string(),
            slot,
            name,
            ..Default::default()
        };
        match step {
            Some(step) => {
                p.start = times.first().copied();
                p.end = times.last().map(|t| t + step);
                p.step = Some(step);
            }
            None => p.times = times,
        }
        out.push(p);
    }
    out
}

/// The gap between evenly spaced placements, when there are enough of them
/// for a run to be shorter than the list.
fn even_step(times: &[f64]) -> Option<f64> {
    if times.len() < 3 {
        return None;
    }
    let step = times[1] - times[0];
    if step <= 0.0 {
        return None;
    }
    times
        .windows(2)
        .all(|w| (w[1] - w[0] - step).abs() < 1e-6)
        .then_some(step)
}

/// The build_song document an exported set rebuilds through.
pub fn build_params(doc: &SetDocument) -> BuildSongParams {
    BuildSongParams {
        // An exported set is notes as they stand, not the parts they came
        // from: what a document reuses is the author's business, and the set
        // no longer remembers it.
        parts: Default::default(),
        on_existing: "converge".into(),
        tempo: doc.tempo,
        key: doc.key.clone(),
        scenes: doc
            .sections
            .iter()
            .map(|sec| SongScene {
                name: song::section_name(&sec.name, sec.phrase_bars),
                tempo: sec.tempo,
                phrase_bars: sec.phrase_bars,
            })
            .collect(),
        tracks: doc
            .tracks
            .iter()
            .map(|t| SongTrack {
                name: t.name.clone(),
                kind: t.kind.clone(),
                instrument: t.instrument.clone(),
                instrument_query: None,
                volume: None,
                volume_db: None,
                fader: t.volume,
                pan: t.pan,
                color_index: t.color_index,
                sends: t.sends.clone(),
            })
            .collect(),
        clips: doc
            .clips
            .iter()
            .filter(|c| !c.notes.is_empty())
            .map(|c| SongClip {
                part: None,
                transpose: None,
                map: Default::default(),
                bars: None,
                track: c.track.clone(),
                slot: Some(c.slot),
                name: c.name.clone(),
                length: c.length,
                slots: Vec::new(),
                notes: c.notes.clone(),
            })
            .collect(),
        placements: doc
            .placements
            .iter()
            .map(|p| crate::tools::SongPlacement {
                track: p.track.clone(),
                slot: p.slot,
                times: p.times.clone(),
                start: p.start,
                end: p.end,
                step: p.step,
            })
            .collect(),
        locators: doc
            .locators
            .iter()
            .map(|l| crate::tools::SongLocator {
                name: l.name.clone(),
                time: l.time,
            })
            .collect(),
        dry_run: false,
        snapshot: false,
    }
}

/// What the file holds, for the reply.
pub fn summary(doc: &SetDocument) -> String {
    let with_uri = doc.tracks.iter().filter(|t| t.instrument.is_some()).count();
    let notes: usize = doc.clips.iter().map(|c| c.notes().len()).sum();
    format!(
        "{} track{} ({} instrument{} by URI, every device by name), {} section{} with {} clip{} and {} notes, mixer and sends, {}, tempo{}",
        doc.tracks.len(),
        if doc.tracks.len() == 1 { "" } else { "s" },
        with_uri,
        if with_uri == 1 { "" } else { "s" },
        doc.sections.len(),
        if doc.sections.len() == 1 { "" } else { "s" },
        doc.clips.len(),
        if doc.clips.len() == 1 { "" } else { "s" },
        notes,
        if doc.setlist.is_empty() { "no setlist".to_string() } else { format!("the setlist ({} entries)", doc.setlist.len()) },
        doc.key.as_ref().map(|k| format!(" and key {k}")).unwrap_or_default()
    )
}

pub fn export_set_body(live: &LiveState, p: &ExportSetParams) -> ToolResult {
    for c in ["get_session_snapshot", "get_context"] {
        require(live, c)?;
    }
    let path = path_for(&p.name)?;
    let snapshot = live
        .send_command(
            "get_session_snapshot",
            Some(json!({"include_notes": true, "include_params": false})),
        )
        .map_err(|e| live_err("read the set", e))?;
    let context = live
        .send_command("get_context", Some(json!({"include_library": false})))
        .map_err(|e| live_err("read the scenes", e))?;
    let library = live.library.snapshot();
    let doc = document(
        &file_name(&p.name)?,
        &snapshot,
        &context,
        library.as_ref(),
        chrono::Local::now().to_rfc3339(),
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(format!(
        "Exported to {}: {}. import_set {{\"name\": \"{}\"}} rebuilds it into an empty set through build_song and set_song. The file holds your notes and names; delete it from the sets folder or with \"Delete all local data\". The Live set itself is still the memory: save it in Live (Cmd+S).",
        path.display(),
        summary(&doc),
        doc.name
    ))
}

pub fn read_document(name: &str) -> Result<SetDocument, String> {
    let path = path_for(name)?;
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "no exported set '{}' ({}: {e}); export_set writes one",
            file_name(name).unwrap_or_default(),
            path.display()
        )
    })?;
    let doc: SetDocument = serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} is not a set export this server can read: {e}",
            path.display()
        )
    })?;
    validate(&doc)?;
    Ok(doc)
}

pub fn validate(doc: &SetDocument) -> Result<(), String> {
    if doc.tracks.is_empty() {
        return Err("the export has no tracks".into());
    }
    let names: Vec<&str> = doc.tracks.iter().map(|t| t.name.as_str()).collect();
    for c in &doc.clips {
        if !names.contains(&c.track.as_str()) {
            return Err(format!(
                "clip '{}' names a track the export lacks: '{}'",
                c.name, c.track
            ));
        }
    }
    let sections: Vec<&str> = doc.sections.iter().map(|s| s.name.as_str()).collect();
    for e in &doc.setlist {
        if !sections.iter().any(|s| s.eq_ignore_ascii_case(&e.section)) {
            return Err(format!(
                "the setlist names a section the export lacks: '{}'",
                e.section
            ));
        }
    }
    Ok(())
}

pub fn import_set_body(live: &LiveState, p: &ImportSetParams) -> ToolResult {
    let doc = read_document(&p.name)?;
    let params = build_params(&doc);
    if p.dry_run {
        return Ok(format!(
            "Set '{}' (exported {}, Live {}): {}. Dry run: nothing was sent to Live.",
            doc.name,
            doc.exported_at,
            if doc.live_version.is_empty() {
                "?"
            } else {
                &doc.live_version
            },
            summary(&doc)
        ));
    }
    if performance_running(live).is_some() {
        return Err("A performance is running; end_performance before rebuilding a set.".into());
    }
    require(live, "get_context")?;
    let ctx = live
        .send_command("get_context", Some(json!({"include_library": false})))
        .map_err(|e| live_err("read the set", e))?;
    let existing = ctx
        .get("tracks")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if existing > 0 && !p.merge {
        return Err(format!(
            "This set already has {existing} track{}; import_set rebuilds into an empty set. Open a new set in Live, or merge: true to add the exported tracks after the existing ones.",
            if existing == 1 { "" } else { "s" }
        ));
    }
    let mut text = format!("Rebuilding '{}' ({}):\n", doc.name, summary(&doc));
    let built = tools::build_song_body(live, &params)?;
    text.push_str(&built);
    if !doc.setlist.is_empty() {
        let song = crate::sections::set_song_body(
            live,
            &crate::sections::SetSongParams {
                setlist: doc.setlist.clone(),
            },
        )?;
        text.push('\n');
        text.push_str(song.lines().next().unwrap_or(""));
    }
    let audio: Vec<&SetClip> = doc
        .clips
        .iter()
        .filter(|c| c.notes.is_empty() && c.file_path.is_some())
        .collect();
    if !audio.is_empty() {
        text.push_str(&format!(
            "\nNot rebuilt: {} audio clip{} ({}); create_audio_clip places them from their files.",
            audio.len(),
            if audio.len() == 1 { "" } else { "s" },
            audio
                .iter()
                .map(|c| format!("{} on {}", c.name, c.track))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    text.push_str("\nSave the set in Live (Cmd+S) to keep it.");
    Ok(text)
}

// ── What the server remembers of the set ────────────────────────────────────
//
// Reading a set costs what its notes cost: `get_session_snapshot` serialises
// every note of every clip on Live's own main thread. Between two edits
// almost none of them have changed, so the server keeps them, and proves
// them fresh before it uses them rather than hoping.
//
// The proof has three parts, and every one of them is cheap:
//   - `get_context` says what the set looks like now — the scenes, and every
//     clip's slot, name and length. A clip that moved, was renamed or
//     resized is re-read.
//   - `drain_passive_events` says what a human did in Live: the script's own
//     listeners raise `clip_notes_changed` when someone edits a clip by hand.
//     Its queue is capped at 500 and drops the oldest silently, so a drain
//     that comes back full means events were lost and everything is re-read.
//   - the connection generation: anything learned on an older socket is
//     dropped, because Live may have been restarted or another set opened.
// Nothing here is written to disk, and nothing survives the process.

/// A clip's notes as last read, with what proves they are still that clip.
#[derive(Debug, Clone)]
struct CachedClip {
    name: String,
    length: f64,
    notes: Vec<Note>,
}

#[derive(Debug, Default)]
struct CacheInner {
    clips: std::collections::HashMap<(String, i64), CachedClip>,
    generation: u64,
}

/// The notes of the Session clips as last read. One per server.
#[derive(Default)]
pub struct SetCache {
    inner: std::sync::Mutex<CacheInner>,
}

impl SetCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Everything the server thought it knew about the set: gone.
    pub fn forget_all(&self) {
        self.lock().clips.clear();
    }

    /// One clip the server has just written, or been told about.
    pub fn forget_clip(&self, track: &str, slot: i64) {
        self.lock().clips.remove(&(track.to_string(), slot));
    }

    pub fn remember(&self, track: &str, slot: i64, name: &str, length: f64, notes: Vec<Note>) {
        self.lock().clips.insert(
            (track.to_string(), slot),
            CachedClip {
                name: name.to_string(),
                length,
                notes,
            },
        );
    }

    fn notes_if_same(&self, track: &str, slot: i64, name: &str, length: f64) -> Option<Vec<Note>> {
        let inner = self.lock();
        let c = inner.clips.get(&(track.to_string(), slot))?;
        (c.name == name && (c.length - length).abs() < 1e-6).then(|| c.notes.clone())
    }

    fn is_empty(&self) -> bool {
        self.lock().clips.is_empty()
    }

    /// Anything learned on an older socket is not this set's.
    fn check_generation(&self, now: u64) {
        let mut inner = self.lock();
        if inner.generation != now {
            inner.clips.clear();
            inner.generation = now;
        }
    }
}

/// What one read of the set cost, for the reply.
#[derive(Debug, Clone, Default)]
pub struct ReadCost {
    pub round_trips: usize,
    pub clips_read: usize,
    pub clips_cached: usize,
    /// Why the whole set had to be read, when it did
    pub full_because: Option<String>,
}

impl ReadCost {
    /// The half-sentence a reply adds when the read was cheaper than it
    /// looks, and nothing at all when the set had to be read in full.
    pub fn line(&self) -> String {
        match (self.clips_cached, self.clips_read) {
            (0, _) => String::new(),
            (_, 0) => " (no notes re-read: nothing in the set had changed since the last call)"
                .to_string(),
            (_, n) => format!(
                " ({} clip{} changed in Live since the last call and {} re-read; the other {} {} not)",
                n,
                if n == 1 { "" } else { "s" },
                if n == 1 { "was" } else { "were" },
                self.clips_cached,
                if self.clips_cached == 1 { "was" } else { "were" }
            ),
        }
    }
}

/// More clips than this changed: one snapshot with the notes in it beats a
/// round trip each.
const NOTE_READS_BEFORE_A_FULL_READ: usize = 6;

/// The set as a document, reading only what it has to.
pub fn read_set(live: &LiveState, name: &str) -> Result<(SetDocument, ReadCost), String> {
    for c in ["get_session_snapshot", "get_context"] {
        require(live, c)?;
    }
    let mut cost = ReadCost::default();
    live.sets
        .check_generation(live.bridge.connection_generation());
    let context = live
        .send_command("get_context", Some(json!({"include_library": false})))
        .map_err(|e| live_err("read the set", e))?;
    cost.round_trips += 1;
    let cold = live.sets.is_empty();
    // What a human changed in Live since the last read. A queue that came
    // back at its cap dropped events, so nothing in it can be trusted.
    let mut dirty: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();
    let mut lost_events = false;
    if !cold && crate::tools::require(live, "drain_passive_events").is_ok() {
        match live.send_command("drain_passive_events", None) {
            Ok(events) => {
                cost.round_trips += 1;
                let count = events.get("count").and_then(Value::as_u64).unwrap_or(0);
                lost_events = count >= 500;
                for e in events
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let kind = e.get("type").and_then(Value::as_str).unwrap_or("");
                    if matches!(kind, "clip_notes_changed" | "clip_name_changed") {
                        if let (Some(t), Some(c)) = (
                            e.get("track_index").and_then(Value::as_i64),
                            e.get("clip_index").and_then(Value::as_i64),
                        ) {
                            dirty.insert((t, c));
                        }
                    }
                }
            }
            // A drain that failed says nothing, so nothing is assumed.
            Err(_) => lost_events = true,
        }
    }
    let full = cold || lost_events;
    if full {
        cost.full_because = Some(if cold {
            "first read of this set".into()
        } else {
            "Live reported more changes than it keeps".into()
        });
    }
    let mut snapshot = live
        .send_command(
            "get_session_snapshot",
            json!({"include_notes": full, "include_params": false}).into(),
        )
        .map_err(|e| live_err("read the set", e))?;
    cost.round_trips += 1;
    if !full {
        fill_notes(live, &mut snapshot, &dirty, &mut cost)?;
    }
    remember_notes(live, &snapshot);
    let library = live.library.snapshot();
    let doc = document(
        name,
        &snapshot,
        &context,
        library.as_ref(),
        chrono::Local::now().to_rfc3339(),
    );
    Ok((doc, cost))
}

/// The notes a snapshot was asked not to carry: from the cache where the
/// clip is provably the same one, read from Live where it is not.
fn fill_notes(
    live: &LiveState,
    snapshot: &mut Value,
    dirty: &std::collections::HashSet<(i64, i64)>,
    cost: &mut ReadCost,
) -> Result<(), String> {
    let mut wanted: Vec<(usize, usize, i64, i64, String)> = Vec::new();
    let mut cached: Vec<(usize, usize, Vec<Note>)> = Vec::new();
    let tracks = snapshot
        .get("tracks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (ti, t) in tracks.iter().enumerate() {
        let track_index = t.get("index").and_then(Value::as_i64).unwrap_or(ti as i64);
        let track_name = t
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        for (si, slot) in t
            .get("clip_slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let Some(clip) = slot.get("clip").filter(|c| c.is_object()) else {
                continue;
            };
            if !clip
                .get("is_midi_clip")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                continue;
            }
            let slot_index = slot
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or(si as i64);
            let name = clip.get("name").and_then(Value::as_str).unwrap_or("");
            let length = clip.get("length").and_then(Value::as_f64).unwrap_or(0.0);
            let known = (!dirty.contains(&(track_index, slot_index)))
                .then(|| {
                    live.sets
                        .notes_if_same(&track_name, slot_index, name, length)
                })
                .flatten();
            match known {
                Some(notes) => cached.push((ti, si, notes)),
                None => wanted.push((ti, si, track_index, slot_index, track_name.clone())),
            }
        }
    }
    // Past a handful, one snapshot with the notes in it is the cheaper read.
    if wanted.len() > NOTE_READS_BEFORE_A_FULL_READ {
        cost.full_because = Some(format!("{} clips had changed", wanted.len()));
        *snapshot = live
            .send_command(
                "get_session_snapshot",
                json!({"include_notes": true, "include_params": false}).into(),
            )
            .map_err(|e| live_err("read the set", e))?;
        cost.round_trips += 1;
        return Ok(());
    }
    for (ti, si, notes) in cached {
        put_notes(snapshot, ti, si, &notes);
        cost.clips_cached += 1;
    }
    for (ti, si, track_index, slot_index, _) in wanted {
        let raw = crate::tools::clip_notes(live, track_index, slot_index)?;
        cost.round_trips += 1;
        cost.clips_read += 1;
        let notes: Vec<Note> = raw
            .iter()
            .filter_map(|n| serde_json::from_value(n.clone()).ok())
            .collect();
        put_notes(snapshot, ti, si, &notes);
    }
    Ok(())
}

fn put_notes(snapshot: &mut Value, track: usize, slot: usize, notes: &[Note]) {
    if let Some(clip) = snapshot.pointer_mut(&format!("/tracks/{track}/clip_slots/{slot}/clip")) {
        if let Some(obj) = clip.as_object_mut() {
            obj.insert(
                "notes".into(),
                serde_json::to_value(notes).unwrap_or(Value::Array(Vec::new())),
            );
            obj.insert("note_count".into(), json!(notes.len()));
        }
    }
}

/// Keep what this read learned, so the next one does not pay for it again.
fn remember_notes(live: &LiveState, snapshot: &Value) {
    for t in snapshot
        .get("tracks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let track = t
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        for slot in t
            .get("clip_slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let Some(clip) = slot.get("clip").filter(|c| c.is_object()) else {
                continue;
            };
            let Some(raw) = clip.get("notes").and_then(Value::as_array) else {
                continue;
            };
            let notes: Vec<Note> = raw
                .iter()
                .filter_map(|n| serde_json::from_value(n.clone()).ok())
                .collect();
            live.sets.remember(
                &track,
                slot.get("index").and_then(Value::as_i64).unwrap_or(0),
                clip.get("name").and_then(Value::as_str).unwrap_or(""),
                clip.get("length").and_then(Value::as_f64).unwrap_or(0.0),
                notes,
            );
        }
    }
}

/// One section's clips, with their notes — the read a producer asking about
/// one part of the song should pay for, rather than the whole set. Notes
/// come from the cache when the clip is provably the same one.
pub fn read_section(live: &LiveState, context: &Value, slot: i64) -> Result<Vec<SetClip>, String> {
    let mut out = Vec::new();
    for t in context
        .get("tracks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let track = t
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let index = t.get("index").and_then(Value::as_i64).unwrap_or(0);
        for clip in t
            .get("clips")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if clip.get("slot").and_then(Value::as_i64) != Some(slot) {
                continue;
            }
            let name = clip
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let length = clip.get("length").and_then(Value::as_f64).unwrap_or(0.0);
            let is_midi = clip.get("is_midi").and_then(Value::as_bool).unwrap_or(true);
            let notes: Vec<Note> = if !is_midi {
                Vec::new()
            } else if let Some(known) = live.sets.notes_if_same(&track, slot, &name, length) {
                known
            } else {
                let raw = crate::tools::clip_notes(live, index, slot)?;
                let notes: Vec<Note> = raw
                    .iter()
                    .filter_map(|n| serde_json::from_value(n.clone()).ok())
                    .collect();
                live.sets
                    .remember(&track, slot, &name, length, notes.clone());
                notes
            };
            out.push(SetClip {
                track: track.clone(),
                slot,
                name,
                length,
                notes: crate::notes::compact(&notes),
                file_path: None,
            });
            break;
        }
    }
    Ok(out)
}
