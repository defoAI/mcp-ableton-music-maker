//! Set memory on request: `export_set` reads the set into a `build_song`
//! document plus the sections and the setlist and writes it under
//! `state_dir()/sets/<name>.json`; `import_set` rebuilds it through
//! `build_song` and `set_song`. Nothing here runs on its own: the file
//! holds the producer's notes and names and appears only when asked for.
//! The Live set itself stays the memory (scene names); this is a backup.

use crate::connection::LiveState;
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
    #[serde(default)]
    pub notes: Vec<Note>,
    /// An audio clip's file, when it had one (not rebuilt; named for the reader)
    #[serde(default)]
    pub file_path: Option<String>,
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
                notes,
                file_path: clip
                    .get("file_path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
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
    SetDocument {
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
    }
}

/// The build_song document an exported set rebuilds through.
pub fn build_params(doc: &SetDocument) -> BuildSongParams {
    BuildSongParams {
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
                track: c.track.clone(),
                slot: Some(c.slot),
                name: c.name.clone(),
                length: c.length,
                slots: Vec::new(),
                notes: crate::notes::NotesInput {
                    notes: c.notes.clone(),
                    ..Default::default()
                },
            })
            .collect(),
        placements: Vec::new(),
        locators: Vec::new(),
        dry_run: false,
    }
}

/// What the file holds, for the reply.
pub fn summary(doc: &SetDocument) -> String {
    let with_uri = doc.tracks.iter().filter(|t| t.instrument.is_some()).count();
    let notes: usize = doc.clips.iter().map(|c| c.notes.len()).sum();
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
