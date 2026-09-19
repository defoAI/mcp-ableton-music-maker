//! Samples: the audio already on this machine, and how one lands in the song.
//!
//! Two halves. **Finding**: Live names the folders that hold samples — the
//! Core Library inside the application, the Packs and the User Library, the
//! open set's own folder, its Places — and this module walks them, keeps the
//! result in memory and, like the browser index, as one JSON file under the
//! state dir, and searches it locally. The walk is here and not in the Remote
//! Script because file I/O in Live's embedded interpreter runs about two files
//! a second (measured 2026-09-19 against Live 12.4.6); natively it is three
//! thousand. A server that cannot see those folders — the Docker variant,
//! whose filesystem is not the producer's — finds nothing and says so, and
//! Live's own browser answers instead.
//!
//! Privacy: names, folders and absolute paths of audio files, and the first
//! 4 KB of a WAV or AIFF for its length — never the audio, and never a copy.
//! `ABLETON_MCP_LIBRARY_INDEX=false` keeps the index in memory, as for the
//! browser index; the folder list is written only when the producer adds one.

use crate::connection::LiveState;
use crate::performance::PerfState;
use crate::song;
use crate::tools::{get_display, live_err, read_perf_state, require, ToolResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

/// One audio file the walk found.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub name: String,
    pub path: String,
    /// The containing folder, from the root's parent down ("Factory Packs/Chop and Swing/Samples/Breaks")
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub ext: String,
    /// From the WAV/AIFF header; absent for a format whose header is not parsed
    #[serde(default)]
    pub seconds: Option<f64>,
}

impl Sample {
    /// "5.3 s" — or nothing at all when the header did not say.
    pub fn length_text(&self) -> String {
        match self.seconds {
            Some(s) => format!("{} · {:.1} s", self.ext, s),
            None => self.ext.clone(),
        }
    }
}

/// One folder to look in, as Live named it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Folder {
    #[serde(default)]
    pub name: String,
    pub path: String,
    /// core_library, packs, user_library, project, places, added
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub files: i64,
}

impl Folder {
    pub fn added(&self) -> bool {
        self.source == "added"
    }
}

/// Every audio file under every folder, as of the last scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    /// The folders that were walked, so a changed set of folders is a new index
    pub key: String,
    pub files: Vec<Sample>,
    pub folders: Vec<Folder>,
    pub complete: bool,
    /// Local time the last page arrived
    pub scanned_at: String,
    /// Seconds the script spent walking
    pub scanned_s: f64,
    /// Live Places whose folder Live would not name
    #[serde(default)]
    pub places_without_path: Vec<String>,
    /// Folders Live named that this process cannot read (a container, say)
    #[serde(default)]
    pub unreadable: Vec<String>,
    #[serde(default)]
    pub truncated: bool,
}

impl Index {
    /// "11,264 files in 4 places (Core Library, Factory Packs, …), indexed in 3.1 s"
    pub fn status_line(&self) -> String {
        let places = self
            .folders
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} file{} in {} folder{}{}{}, indexed in {:.1} s",
            self.files.len(),
            if self.files.len() == 1 { "" } else { "s" },
            self.folders.len(),
            if self.folders.len() == 1 { "" } else { "s" },
            if places.is_empty() {
                String::new()
            } else {
                format!(" ({places})")
            },
            if self.truncated {
                ", and that is as many as one index holds"
            } else {
                ""
            },
            self.scanned_s
        )
    }
}

/// The index in memory.
#[derive(Default)]
pub struct Samples {
    pub index: Mutex<Option<Index>>,
}

impl Samples {
    pub fn snapshot(&self) -> Option<Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn store(&self, ix: Index) {
        *self.index.lock().unwrap_or_else(|e| e.into_inner()) = Some(ix);
    }

    pub fn clear(&self) {
        *self.index.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

// ── the folders the producer added ───────────────────────────────────────────

/// `~/.ableton-music-maker/sample_folders.json` — written only when a folder is
/// added, and holding nothing but paths.
fn folders_file() -> PathBuf {
    crate::state::state_dir().join("sample_folders.json")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AddedFolders {
    #[serde(default)]
    folders: Vec<String>,
}

pub fn added_folders() -> Vec<String> {
    std::fs::read(folders_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<AddedFolders>(&b).ok())
        .map(|f| f.folders)
        .unwrap_or_default()
}

fn save_added_folders(folders: &[String]) -> Result<(), String> {
    let path = folders_file();
    if folders.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    }
    let body = serde_json::to_vec_pretty(&AddedFolders {
        folders: folders.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| format!("could not write {}: {e}", path.display()))
}

// ── the index on disk, beside the browser index ──────────────────────────────

fn file_for(key: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    crate::state::library_dir().join(format!("samples-{hash:016x}.json"))
}

fn load_from_disk(key: &str) -> Option<Index> {
    if !crate::library::disk_enabled() {
        return None;
    }
    let ix: Index = serde_json::from_slice(&std::fs::read(file_for(key)).ok()?).ok()?;
    (ix.key == key).then_some(ix)
}

fn save_to_disk(ix: &Index) {
    if !crate::library::disk_enabled() {
        return;
    }
    let path = file_for(&ix.key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec(ix) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!("could not write the sample index {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("could not serialise the sample index: {e}"),
    }
}

fn forget_on_disk(key: &str) {
    let _ = std::fs::remove_file(file_for(key));
}

fn now_hhmm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn key_for(folders: &[Folder]) -> String {
    let mut paths: Vec<&str> = folders.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    paths.join("\n")
}

fn folders_of(v: &Value) -> Vec<Folder> {
    v.get("folders")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| serde_json::from_value(f.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn strings_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// What the walk reads, and where it stops: a producer's library, not their
/// whole disk, and never so long that a search feels stuck.
const EXTENSIONS: &[&str] = &[
    "wav", "wave", "aif", "aiff", "aifc", "flac", "mp3", "m4a", "ogg",
];
const MAX_FILES: usize = 60_000;
const MAX_DEPTH: usize = 8;
const WALK_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// Every audio file under these folders, with the length the header gives.
/// Stops at [`MAX_FILES`] or [`WALK_BUDGET`], saying which happened.
fn walk(folders: &mut [Folder]) -> (Vec<Sample>, bool, Vec<String>) {
    let started = std::time::Instant::now();
    let mut files: Vec<Sample> = Vec::new();
    let mut truncated = false;
    let mut unreadable: Vec<String> = Vec::new();
    for folder in folders.iter_mut() {
        let root = PathBuf::from(&folder.path);
        // What the producer sees in a hit: the folder, from the root's parent
        // down, so "Factory Packs/Chop and Swing/Samples/Breaks" reads like Live.
        let base = root.parent().map(PathBuf::from).unwrap_or_default();
        let mut stack = vec![(root.clone(), 0usize)];
        let before = files.len();
        let mut opened = false;
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            opened = true;
            let mut here: Vec<Sample> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let path = entry.path();
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    if depth + 1 < MAX_DEPTH {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if !EXTENSIONS.contains(&ext.as_str()) {
                    continue;
                }
                here.push(Sample {
                    name: path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| name.clone()),
                    folder: dir
                        .strip_prefix(&base)
                        .unwrap_or(&dir)
                        .to_string_lossy()
                        .to_string(),
                    seconds: crate::audio::header_seconds(&path),
                    path: path.to_string_lossy().to_string(),
                    ext,
                });
            }
            here.sort_by(|a, b| a.name.cmp(&b.name));
            files.append(&mut here);
            if files.len() >= MAX_FILES || started.elapsed() > WALK_BUDGET {
                truncated = true;
                break;
            }
        }
        folder.files = (files.len() - before) as i64;
        if !opened {
            unreadable.push(folder.path.clone());
        }
        if truncated {
            break;
        }
    }
    (files, truncated, unreadable)
}

/// The index, built on first use: the folders from Live, then the disk copy if
/// it matches, else a scan. Later calls answer from memory.
pub fn ensure(live: &LiveState) -> Result<Index, String> {
    if let Some(ix) = live.samples.snapshot().filter(|ix| ix.complete) {
        return Ok(ix);
    }
    require(live, "list_sample_folders")?;
    let added = added_folders();
    let listed = live
        .send_command("list_sample_folders", Some(json!({"roots": added})))
        .map_err(|e| live_err("ask Live which folders hold samples", e))?;
    let folders = folders_of(&listed);
    let key = key_for(&folders);
    if let Some(disk) = load_from_disk(&key) {
        if disk.complete {
            live.samples.store(disk.clone());
            return Ok(disk);
        }
    }
    let mut ix = Index {
        key,
        folders,
        places_without_path: strings_of(&listed, "places_without_path"),
        scanned_at: now_hhmm(),
        complete: true,
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let (files, truncated, unreadable) = walk(&mut ix.folders);
    ix.files = files;
    ix.truncated = truncated;
    ix.unreadable = unreadable;
    ix.scanned_s = started.elapsed().as_secs_f64();
    save_to_disk(&ix);
    live.samples.store(ix.clone());
    Ok(ix)
}

/// Every word must appear in the file's name or its folder; more words in the
/// name wins, then the shorter folder, then alphabetical — the browser index's
/// rule, so both searches rank the same way.
pub fn search<'a>(ix: &'a Index, query: &str, limit: usize) -> Vec<&'a Sample> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(i64, bool, usize, usize, &Sample)> = ix
        .files
        .iter()
        .filter_map(|f| {
            let name = f.name.to_lowercase();
            let folder = f.folder.to_lowercase();
            let mut in_name = 0i64;
            for w in &words {
                if name.contains(w.as_str()) {
                    in_name += 1;
                } else if !folder.contains(w.as_str()) {
                    return None;
                }
            }
            // A file called "Crash 505" is what someone asking for a crash
            // means; "Kick Crash Combo" is a kick. So the name leading with
            // the first word wins, then the shorter name.
            let leads = name.starts_with(words[0].as_str());
            Some((-in_name, !leads, f.name.len(), f.folder.len(), f))
        })
        .collect();
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
            .then(a.4.name.cmp(&b.4.name))
    });
    hits.into_iter().take(limit).map(|h| h.4).collect()
}

// ── resolving what the producer said ────────────────────────────────────────

/// What `sample` turned out to mean.
pub enum Found {
    /// A file: the Arrangement and a Session row can both take it
    File(Sample, Vec<String>),
    /// A browser item: Live will only say where its file is once it is a clip
    Item { name: String, uri: String },
}

fn looks_like_path(text: &str) -> bool {
    text.starts_with('/')
        || text.starts_with("~/")
        || (text.len() > 2 && text.as_bytes()[0].is_ascii_alphabetic() && &text[1..3] == ":\\")
}

fn stem_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Plain words are a search; a path is used as given; anything else with a ':'
/// is a browser URI.
pub fn resolve(live: &LiveState, sample: &str) -> Result<Found, String> {
    let text = sample.trim();
    if text.is_empty() {
        return Err(
            "Say which sample: words to search for (\"break 90\"), a file path, or a browser URI."
                .into(),
        );
    }
    if looks_like_path(text) {
        let path = if let Some(rest) = text.strip_prefix("~/") {
            dirs::home_dir()
                .map(|h| h.join(rest).to_string_lossy().to_string())
                .unwrap_or_else(|| text.to_string())
        } else {
            text.to_string()
        };
        return Ok(Found::File(
            Sample {
                name: stem_of(&path),
                ext: std::path::Path::new(&path)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
                path,
                ..Default::default()
            },
            Vec::new(),
        ));
    }
    if !text.contains(' ') && text.contains(':') {
        // A browser URI is an id, not a name ("query:Samples#FileId_19408"):
        // it is quoted as the producer gave it, and the name the reply uses
        // afterwards is the one Live gives the clip.
        return Ok(Found::Item {
            name: text.to_string(),
            uri: text.to_string(),
        });
    }
    let ix = ensure(live)?;
    let hits = search(&ix, text, 4);
    if let Some(best) = hits.first() {
        let others = hits.iter().skip(1).map(|h| h.name.clone()).collect();
        return Ok(Found::File((*best).clone(), others));
    }
    // Nothing on disk: Live's browser may still know it (a Place Live will not
    // locate, a pack whose samples live somewhere this does not walk).
    if let Some(item) = crate::tools::browser_sample(live, text)? {
        return Ok(Found::Item {
            name: item.name,
            uri: item.uri,
        });
    }
    Err(format!(
        "No sample matches \"{text}\". {}. Try fewer words, give the file's path, or add a folder with adv_sample_folders.",
        ix.status_line()
    ))
}

// ── add_sample ──────────────────────────────────────────────────────────────

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AddSampleParams {
    /// Words to search for ("break 90", "vinyl crackle"), an absolute file path, or a browser URI
    pub sample: String,
    /// Audio track by name or index; leave out and an audio track is made, named after the sample
    #[serde(default)]
    pub track: Option<Value>,
    /// The section whose row the clip goes in, by name (so it plays as part of the song)
    #[serde(default)]
    pub section: Option<Value>,
    /// Or the Arrangement: Live's 1-based bar
    #[serde(default)]
    pub at_bar: Option<f64>,
    /// Or the Arrangement in beats, when a bar is not fine enough
    #[serde(default)]
    pub at_beat: Option<f64>,
    /// Or a Session slot by index, for a set without sections
    #[serde(default)]
    pub slot: Option<i64>,
    /// Force the loop length in bars instead of letting Live's own warp decide
    #[serde(default)]
    pub bars: Option<f64>,
    /// Warp a loop and set its loop to whole bars (default true); false places it exactly as dragging the file in would
    #[serde(default = "yes")]
    pub fit: bool,
    /// Transpose the clip, in semitones (-48 to 48)
    #[serde(default)]
    pub transpose: Option<i64>,
    /// Clip name (default: the file's name)
    #[serde(default)]
    pub name: Option<String>,
}

/// Where the sample is going.
enum Target {
    /// A Session row, with the words for it ("the Verse", "slot 3")
    Slot(i64, String),
    /// A beat in the Arrangement
    Beat(f64),
}

fn target_of(p: &AddSampleParams, state: &PerfState) -> Result<Target, String> {
    let asked = [
        p.section.is_some(),
        p.at_bar.is_some() || p.at_beat.is_some(),
        p.slot.is_some(),
    ]
    .iter()
    .filter(|a| **a)
    .count();
    if asked == 0 {
        return Err("Where should it go? `section` puts it in that row so it plays with the song; `at_bar` puts it in the Arrangement.".into());
    }
    if asked > 1 {
        return Err(
            "Give one place: `section` (a Session row) or `at_bar` (the Arrangement), not both."
                .into(),
        );
    }
    if let Some(section) = &p.section {
        let sections = song::sections(state);
        let which = match section {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let found = song::find_section(&sections, &which).ok_or_else(|| {
            format!(
                "No section called \"{}\".{} Sections: {}. Make one with make_section, or give at_bar to place the sample in the Arrangement.",
                which.trim(),
                song::suggest(&which, &sections)
                    .map(|n| format!(" Did you mean \"{n}\"?"))
                    .unwrap_or_default(),
                if sections.is_empty() {
                    "none yet".to_string()
                } else {
                    song::section_names(&sections)
                }
            )
        })?;
        return Ok(Target::Slot(found.index, format!("{} ", found.name)));
    }
    if let Some(slot) = p.slot {
        if slot < 0 {
            return Err(format!("slot {slot} is not a row"));
        }
        return Ok(Target::Slot(slot, format!("slot {slot} ")));
    }
    if let Some(beat) = p.at_beat {
        if beat < 0.0 {
            return Err("at_beat cannot be before the start of the Arrangement".into());
        }
        return Ok(Target::Beat(beat));
    }
    let bar = p.at_bar.unwrap_or(1.0);
    if bar < 1.0 {
        return Err(format!(
            "at_bar is Live's own numbering, so the Arrangement starts at bar 1 (got {bar})"
        ));
    }
    Ok(Target::Beat(state.bar_start(bar)))
}

fn bars_text(state: &PerfState, beats: f64) -> String {
    let bar = beats / state.beats_per_bar() + 1.0;
    if (bar.fract()).abs() < 0.01 {
        format!("bar {}", bar.round() as i64)
    } else {
        format!("bar {bar:.2}")
    }
}

/// "4 bars", "1 bar", "3.40 bars".
fn bars(v: f64) -> String {
    format!(
        "{} bar{}",
        num(v),
        if (v - 1.0).abs() < 0.001 { "" } else { "s" }
    )
}

fn num(v: f64) -> String {
    if (v.fract()).abs() < 0.001 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.2}")
    }
}

pub fn add_sample_body(live: &LiveState, p: &AddSampleParams) -> ToolResult {
    require(live, "place_sample")?;
    if let Some(t) = p.transpose {
        if !(-48..=48).contains(&t) {
            return Err(format!(
                "transpose is in semitones, -48 to 48 (got {t}); Live's clip Transpose goes no further"
            ));
        }
    }
    if let Some(b) = p.bars {
        if b <= 0.0 {
            return Err(format!("bars must be more than 0 (got {b})"));
        }
    }
    let found = resolve(live, &p.sample)?;
    let state = read_perf_state(live)?;
    let target = target_of(p, &state)?;
    let (file, uri, sample_name, others) = match &found {
        Found::File(s, others) => (Some(s.clone()), None, s.name.clone(), others.clone()),
        Found::Item { name, uri } => (None, Some(uri.clone()), name.clone(), Vec::new()),
    };
    if uri.is_some() {
        if let Target::Beat(_) = target {
            return Err(format!(
                "'{sample_name}' is in Live's browser, and Live tells a client an item's file only once it is a clip — which the Arrangement needs. Put it in a section first (add_sample with `section`), then arrange it; or add its folder with adv_sample_folders."
            ));
        }
    }
    // The track: the one named, or a new audio track named after the sample.
    let mut made_track = None;
    let track = match p.track.as_ref().map(|t| state.track_by(t)) {
        Some(Ok(t)) => t.index,
        _ => {
            let wanted = match p.track.as_ref() {
                Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
                _ => sample_name.clone(),
            };
            require(live, "create_tracks")?;
            let r = live
                .send_command(
                    "create_tracks",
                    Some(json!({"tracks": [{"kind": "audio", "name": wanted}]})),
                )
                .map_err(|e| live_err("make an audio track for the sample", e))?;
            let created = r
                .get("created")
                .and_then(Value::as_array)
                .and_then(|a| a.first().cloned())
                .unwrap_or_default();
            let index = created
                .get("index")
                .and_then(Value::as_i64)
                .ok_or("Live made the audio track but did not say where")?;
            made_track = Some((get_display(&created, "name", &wanted), index));
            index
        }
    };
    let mut command = json!({"track_index": track, "fit": p.fit});
    if let Some(f) = &file {
        command["path"] = json!(f.path);
    }
    if let Some(u) = &uri {
        command["item_uri"] = json!(u);
    }
    match &target {
        Target::Slot(slot, _) => command["slot"] = json!(slot),
        Target::Beat(beat) => command["position"] = json!(beat),
    }
    if let Some(b) = p.bars {
        command["bars"] = json!(b);
    }
    if let Some(t) = p.transpose {
        command["transpose"] = json!(t);
    }
    if let Some(n) = p.name.as_ref().filter(|n| !n.trim().is_empty()) {
        command["name"] = json!(n.trim());
    }
    let r = live
        .send_command("place_sample", Some(command))
        .map_err(|e| live_err("put the sample in the set", e))?;

    // What Live ended up with, read back off the clip.
    let name = get_display(&r, "name", &sample_name);
    let track_name = get_display(&r, "track", "the track");
    let mut out = match &target {
        Target::Slot(_, words) => format!("Added '{name}' to {words}on {track_name}"),
        Target::Beat(beat) => format!(
            "Added '{name}' to the Arrangement on {track_name} at {}",
            bars_text(&state, *beat)
        ),
    };
    if let Some((made, index)) = &made_track {
        out.push_str(&format!(" (new audio track '{made}', index {index})"));
    }
    out.push('.');
    if let (Some(start), Some(end)) = (
        r.get("start_time").and_then(Value::as_f64),
        r.get("end_time").and_then(Value::as_f64),
    ) {
        out.push_str(&format!(
            " Covers {} to {}.",
            bars_text(&state, start),
            bars_text(&state, end)
        ));
    }
    let seconds = r
        .get("duration_s")
        .and_then(Value::as_f64)
        .or(file.as_ref().and_then(|f| f.seconds));
    let heard = r.get("heard_bars").and_then(Value::as_f64);
    let audio = match seconds {
        Some(s) => format!("\n{s:.1} s of audio"),
        None => "\nThe sample".to_string(),
    };
    let fitted = get_display(&r, "fitted", "");
    let loop_bars = r
        .get("loop_end")
        .and_then(Value::as_f64)
        .map(|e| e / state.beats_per_bar());
    let heard_text = heard
        .map(|h| format!(" (Live heard {})", bars(h)))
        .unwrap_or_default();
    let warped = r
        .get("warping")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    match fitted.as_str() {
        "snapped" => out.push_str(&format!(
            "{audio}, warped and looping {}{heard_text}.",
            loop_bars.map(bars).unwrap_or_else(|| "?".into())
        )),
        "asked" => out.push_str(&format!(
            "{audio}, warped and looping {} as asked{heard_text}.",
            loop_bars.map(bars).unwrap_or_else(|| "?".into())
        )),
        // At a bar it is one hit, not a loop: Live's own warp decision stands.
        "at_bar" => out.push_str(&format!(
            "{audio}, {} and not looping — one hit at that bar. Give `bars` to make it a loop of a set length.",
            if warped {
                "warped to the tempo by Live"
            } else {
                "unwarped, as Live loaded it"
            }
        )),
        "one_shot" => out.push_str(&format!(
            "{audio}: a one-shot, left as Live loaded it — unwarped, not looping."
        )),
        "off_grid" => out.push_str(&format!(
            "{audio}:{} not a whole number of bars, so the markers were left alone — give `bars` to force a loop length.",
            heard
                .map(|h| format!(" Live heard {},", bars(h)))
                .unwrap_or_default()
        )),
        "raw" => out.push_str(&format!(
            "{audio}, placed exactly as dragging the file in would (fit: false)."
        )),
        _ => out.push_str(&format!(
            "{audio}: Live did not report a length, so nothing was fitted."
        )),
    }
    if let Some(t) = r.get("transpose").and_then(Value::as_i64) {
        out.push_str(&format!(" Transposed {t:+} semitones."));
    }
    // An Arrangement clip is as long as its material: Live's own end_time is
    // read-only (LOM: "get, observe"), so a longer `bars` sets the clip's loop
    // and cannot stretch what the Arrangement shows. Say so rather than let
    // the loop length read as the length on the timeline.
    if let (Some(start), Some(end), Some(loop_end)) = (
        r.get("start_time").and_then(Value::as_f64),
        r.get("end_time").and_then(Value::as_f64),
        r.get("loop_end").and_then(Value::as_f64),
    ) {
        let covered = end - start;
        if loop_end > covered + 0.01 {
            out.push_str(&format!(
                "\nOn the timeline it is {} long, not {}: Live gives an Arrangement clip the length of its material and the API cannot stretch that. `arrange repeat` fills the rest.",
                bars(covered / state.beats_per_bar()),
                bars(loop_end / state.beats_per_bar())
            ));
        }
    }
    if let Some(refused) = r.get("refused").and_then(Value::as_array) {
        if !refused.is_empty() {
            out.push_str(&format!(
                "\nLive refused: {}.",
                refused
                    .iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }
    let placed_path = r
        .get("file_path")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(file.as_ref().map(|f| f.path.clone()));
    if let Some(path) = placed_path {
        out.push_str(&format!(
            "\nThe file is referenced where it is: {path} — nothing copied."
        ));
    }
    if !others.is_empty() {
        out.push_str(&format!(
            "\nAlso matched: {} — say one to swap it.",
            others.join(", ")
        ));
    }
    Ok(out)
}

// ── adv_sample_folders ──────────────────────────────────────────────────────

fn list_action() -> String {
    "list".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SampleFoldersParams {
    /// "list" (default), "add", "remove" or "refresh"
    #[serde(default = "list_action")]
    pub action: String,
    /// With add or remove: the folder's absolute path
    #[serde(default)]
    pub path: Option<String>,
}

pub fn sample_folders_body(live: &LiveState, p: &SampleFoldersParams) -> ToolResult {
    let action = p.action.trim().to_lowercase();
    let mut added = added_folders();
    match action.as_str() {
        "list" => {}
        "refresh" => {
            if let Some(ix) = live.samples.snapshot() {
                forget_on_disk(&ix.key);
            }
            live.samples.clear();
        }
        "add" | "remove" => {
            let path = p
                .path
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("{action} needs `path`, the folder's absolute path"))?;
            let path = match path.strip_prefix("~/") {
                Some(rest) => dirs::home_dir()
                    .map(|h| h.join(rest).to_string_lossy().to_string())
                    .unwrap_or(path),
                None => path,
            };
            if !looks_like_path(&path) {
                return Err(format!(
                    "'{path}' is not an absolute path. Give the folder in full, e.g. /Users/you/Samples."
                ));
            }
            if action == "add" {
                if added.iter().any(|f| f == &path) {
                    return Err(format!("Claude already looks in {path}."));
                }
                added.push(path.clone());
            } else {
                let before = added.len();
                added.retain(|f| f != &path);
                if added.len() == before {
                    return Err(format!(
                        "{path} is not one of the folders you added. Folders you added: {}.",
                        if added.is_empty() {
                            "none".to_string()
                        } else {
                            added.join(", ")
                        }
                    ));
                }
            }
            save_added_folders(&added)?;
            if let Some(ix) = live.samples.snapshot() {
                forget_on_disk(&ix.key);
            }
            live.samples.clear();
            if action == "add" {
                // Live is the judge of whether the folder is there: the script
                // sees the real filesystem, this process may not.
                let ix = ensure(live).inspect_err(|_| {
                    // The folder never became searchable, so it is not kept.
                    let _ = save_added_folders(&added[..added.len() - 1]);
                })?;
                if !ix.folders.iter().any(|f| f.path == path) {
                    added.pop();
                    save_added_folders(&added)?;
                    live.samples.clear();
                    return Err(format!(
                        "Live cannot see {path} — check it is a folder, and that it is on the machine Live runs on."
                    ));
                }
                let found = ix
                    .folders
                    .iter()
                    .find(|f| f.path == path)
                    .map(|f| f.files)
                    .unwrap_or(0);
                return Ok(format!(
                    "Added {path} — {found} audio file{}.\n{}",
                    if found == 1 { "" } else { "s" },
                    folder_list(&ix)
                ));
            }
        }
        other => {
            return Err(format!(
                "action is list, add, remove or refresh — not '{other}'"
            ))
        }
    }
    let ix = ensure(live)?;
    Ok(format!(
        "{}\n{}",
        match action.as_str() {
            "refresh" => format!("Looked again: {}", ix.status_line()),
            "remove" => format!("Removed it. {}", ix.status_line()),
            _ => ix.status_line(),
        },
        folder_list(&ix)
    ))
}

fn folder_list(ix: &Index) -> String {
    let mut out = String::new();
    for f in &ix.folders {
        out.push_str(&format!(
            "  {:<16} {:>6} files  {}{}\n",
            f.name,
            f.files,
            f.path,
            if f.added() { "  (you added this)" } else { "" }
        ));
    }
    if !ix.unreadable.is_empty() {
        out.push_str(&format!(
            "Claude cannot read {} from where it runs, so nothing in there is searchable — give add_sample a file's path and Live still opens it, or run the server on this machine.\n",
            ix.unreadable.join(", ")
        ));
    }
    if !ix.places_without_path.is_empty() {
        out.push_str(&format!(
            "Live did not say where these Places are, so they are not searched: {}. Add one with action \"add\" and its path.\n",
            ix.places_without_path.join(", ")
        ));
    }
    if ix.truncated {
        out.push_str("That is as many files as one index holds; narrow the folders if something is missing.\n");
    }
    out.push_str(&format!(
        "Live's own folders need no adding. The list of yours is kept in {} — paths only, never audio.",
        folders_file().display()
    ));
    out
}

/// The `search_browser` answer for `category: "samples"`: the files on disk,
/// with their paths, or None when the index has nothing to say and Live's
/// browser should answer instead.
pub fn search_text(
    live: &LiveState,
    queries: &[String],
    limit: usize,
    best: bool,
) -> Result<Option<String>, String> {
    let ix = ensure(live)?;
    if ix.files.is_empty() {
        return Ok(None);
    }
    let mut hits: Vec<(&String, Vec<&Sample>)> = Vec::new();
    for q in queries {
        hits.push((q, search(&ix, q, if best { 1 } else { limit })));
    }
    if hits.iter().all(|(_, h)| h.is_empty()) {
        return Ok(None);
    }
    let mut out = format!("From the sample index: {}.\n", ix.status_line());
    for (q, found) in &hits {
        if found.is_empty() {
            out.push_str(&format!("  nothing matches \"{q}\"\n"));
            continue;
        }
        out.push_str(&format!(
            "{} match{} for \"{q}\":\n",
            found.len(),
            if found.len() == 1 { "" } else { "es" }
        ));
        for f in found {
            out.push_str(&format!(
                "  {} — {}    {}\n    path: {}\n",
                f.name,
                f.folder,
                f.length_text(),
                f.path
            ));
        }
    }
    out.push_str("Put one in the song with add_sample(sample, section or at_bar); words, a path or a uri all work.");
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Index {
        let file = |name: &str, folder: &str, seconds: Option<f64>| Sample {
            name: name.into(),
            path: format!("/packs/{folder}/{name}.wav"),
            folder: folder.into(),
            ext: "wav".into(),
            seconds,
        };
        Index {
            key: "k".into(),
            complete: true,
            files: vec![
                file("Kick Crash Combo", "One Shots/Kick", Some(2.3)),
                file("Crash 505", "One Shots/Cymbal", Some(1.4)),
                file("Crash Bright 01", "One Shots/Cymbal", None),
                file("Break 90bpm 11 Loose", "Loops/Breaks", Some(10.7)),
            ],
            folders: vec![Folder {
                name: "Factory Packs".into(),
                path: "/packs".into(),
                source: "packs".into(),
                files: 4,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_name_that_leads_with_the_word_wins() {
        let ix = index();
        let names: Vec<&str> = search(&ix, "crash", 5)
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Crash 505", "Crash Bright 01", "Kick Crash Combo"],
            "someone asking for a crash does not mean the kick"
        );
        assert_eq!(
            search(&ix, "break loose", 5)[0].name,
            "Break 90bpm 11 Loose"
        );
        // A word in the folder still matches, and nothing matches nothing.
        assert_eq!(search(&ix, "cymbal", 5).len(), 2);
        assert!(search(&ix, "tabla", 5).is_empty());
        assert!(search(&ix, "", 5).is_empty());
    }

    #[test]
    fn a_length_is_shown_only_when_the_header_gave_one() {
        let ix = index();
        assert_eq!(ix.files[1].length_text(), "wav · 1.4 s");
        assert_eq!(
            ix.files[2].length_text(),
            "wav",
            "no guess for a missing header"
        );
    }

    #[test]
    fn bars_and_numbers_read_like_a_musician_writes_them() {
        assert_eq!(bars(1.0), "1 bar");
        assert_eq!(bars(4.0), "4 bars");
        assert_eq!(bars(3.4), "3.40 bars");
        assert_eq!(num(2.0), "2");
    }

    #[test]
    fn a_path_is_told_from_words_and_from_a_uri() {
        assert!(looks_like_path("/Users/x/a.wav"));
        assert!(looks_like_path("~/Samples/a.wav"));
        assert!(looks_like_path("C:\\Samples\\a.wav"));
        assert!(!looks_like_path("query:Samples#FileId_1"));
        assert!(!looks_like_path("break 90"));
        assert_eq!(stem_of("/Users/x/Break 90.wav"), "Break 90");
    }

    #[test]
    fn the_index_is_keyed_by_the_folders_whatever_their_order() {
        let a = vec![
            Folder {
                path: "/b".into(),
                ..Default::default()
            },
            Folder {
                path: "/a".into(),
                ..Default::default()
            },
        ];
        let b = vec![
            Folder {
                path: "/a".into(),
                ..Default::default()
            },
            Folder {
                path: "/b".into(),
                ..Default::default()
            },
        ];
        assert_eq!(key_for(&a), key_for(&b));
        assert_ne!(key_for(&a), key_for(&b[..1]));
    }

    #[test]
    fn the_status_line_says_what_was_found_and_where() {
        let line = index().status_line();
        assert!(
            line.starts_with("4 files in 1 folder (Factory Packs), indexed in"),
            "{line}"
        );
    }

    #[test]
    fn the_walk_finds_audio_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("Breaks");
        std::fs::create_dir_all(&deep).unwrap();
        // A WAV header is enough: the length comes from the declared data size.
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend(44u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(48_000u32.to_le_bytes());
        wav.extend(192_000u32.to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(384_000u32.to_le_bytes()); // two seconds
        std::fs::write(deep.join("Loop.wav"), &wav).unwrap();
        std::fs::write(deep.join("notes.txt"), b"not a sample").unwrap();
        std::fs::write(deep.join(".hidden.wav"), &wav).unwrap();
        let mut folders = vec![Folder {
            name: "Test".into(),
            path: dir.path().to_string_lossy().to_string(),
            source: "added".into(),
            files: 0,
        }];
        let (files, truncated, unreadable) = walk(&mut folders);
        assert_eq!(files.len(), 1, "only the audio: {files:?}");
        assert_eq!(files[0].name, "Loop");
        assert_eq!(files[0].seconds, Some(2.0), "two seconds, from the header");
        assert!(files[0].folder.ends_with("Breaks"), "{}", files[0].folder);
        assert_eq!(folders[0].files, 1);
        assert!(!truncated && unreadable.is_empty());

        // A folder this process cannot read is named, not silently empty.
        let mut gone = vec![Folder {
            path: dir.path().join("nowhere").to_string_lossy().to_string(),
            ..Default::default()
        }];
        let (files, _, unreadable) = walk(&mut gone);
        assert!(files.is_empty());
        assert_eq!(unreadable.len(), 1);
    }
}
