//! The listening memory, read: what the Mac app heard while the producer
//! played, by bar and by section, and how current it is.
//!
//! The app does the listening (`recorder`) and writes takes under
//! `state_dir()/listening/<song-key>/`; the server is short-lived and only
//! reads them. Nothing here talks to Live except to learn which set is open,
//! and **nothing here starts the transport** — `listen {play: true}` is the
//! one exception, and only while the producer has said Claude may play the
//! set (`feedback::away`).
//!
//! Every reading is labelled as **measured from playback at <time>**, with
//! the take's revision against the set's: a take from before the last change
//! is never presented as current.

use crate::audio;
use crate::connection::LiveState;
use crate::meter;
use crate::recorder::{BarRow, FrameRow, Take, FRAME_BYTES};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

pub type ToolResult = Result<String, String>;

/// How long a take is kept unless the app's setting says otherwise.
pub const RETENTION_DAYS: u32 = 30;
/// An app heartbeat older than this means the app is not running.
const APP_STALE_MS: i64 = 20_000;

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

// ── where things are ────────────────────────────────────────────────────────

pub fn song_dir(song_key: &str) -> PathBuf {
    crate::state::listening_dir().join(safe(song_key))
}

fn takes_dir(song_key: &str) -> PathBuf {
    song_dir(song_key).join("takes")
}

/// A key is a file name: nothing that could climb out of the folder.
fn safe(key: &str) -> String {
    let s: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if s.is_empty() {
        "set".into()
    } else {
        s
    }
}

// ── takes on disk ───────────────────────────────────────────────────────────

/// Write a take: the JSON whole (atomically, so a reader never sees half of
/// it) and the new frames appended to its frames file.
pub fn write_take(take: &Take, frames: &[FrameRow]) -> Result<(), String> {
    let dir = takes_dir(&take.song_key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !frames.is_empty() {
        let path = dir.join(format!("{}.frames", take.id));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut buf = Vec::with_capacity(frames.len() * FRAME_BYTES);
        for fr in frames {
            buf.extend_from_slice(&fr.encode());
        }
        f.write_all(&buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let path = dir.join(format!("{}.json", take.id));
    let tmp = dir.join(format!(".{}.json.tmp", take.id));
    let body = serde_json::to_vec(take).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every take of a song, oldest first.
pub fn takes(song_key: &str) -> Vec<Take> {
    let mut out: Vec<Take> = std::fs::read_dir(takes_dir(song_key))
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.ends_with(".json") && !n.starts_with('.')
                })
                .filter_map(|e| std::fs::read(e.path()).ok())
                .filter_map(|b| serde_json::from_slice::<Take>(&b).ok())
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// A take's frames, in order.
pub fn frames(song_key: &str, take_id: &str) -> Vec<FrameRow> {
    std::fs::read(takes_dir(song_key).join(format!("{}.frames", safe(take_id))))
        .map(|b| {
            b.chunks_exact(FRAME_BYTES)
                .filter_map(FrameRow::decode)
                .collect()
        })
        .unwrap_or_default()
}

/// The song whose takes were written most recently: what "this set" means
/// when Live cannot be asked.
pub fn newest_song() -> Option<String> {
    let rd = std::fs::read_dir(crate::state::listening_dir()).ok()?;
    rd.flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let key = e.file_name().to_string_lossy().to_string();
            let newest = std::fs::read_dir(e.path().join("takes"))
                .ok()?
                .flatten()
                .filter_map(|f| f.metadata().ok()?.modified().ok())
                .max()?;
            Some((newest, key))
        })
        .max()
        .map(|(_, k)| k)
}

/// Delete takes older than `days`. Returns how many files went.
pub fn prune(days: u32) -> usize {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(u64::from(days.max(1)) * 86_400);
    let mut removed = 0;
    let Ok(songs) = std::fs::read_dir(crate::state::listening_dir()) else {
        return 0;
    };
    for song in songs.flatten() {
        let Ok(files) = std::fs::read_dir(song.path().join("takes")) else {
            continue;
        };
        for f in files.flatten() {
            let old = f
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t < cutoff)
                .unwrap_or(false);
            if old && std::fs::remove_file(f.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

// ── the change log: what "revision" counts ──────────────────────────────────

/// One change to the set, as the change log keeps it. The set's revision is
/// how many there are: revision 43 is the set after 43 changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub n: u64,
    pub at: String,
    pub at_ms: i64,
    /// "claude" (a tool call) or "live" (seen in Live by the app)
    pub by: String,
    pub what: String,
}

fn changes_path(song_key: &str) -> PathBuf {
    song_dir(song_key).join("changes.jsonl")
}

pub fn changes(song_key: &str) -> Vec<Change> {
    std::fs::read_to_string(changes_path(song_key))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str::<Change>(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The set's revision: the number of changes logged.
pub fn revision(song_key: &str) -> u64 {
    changes(song_key).last().map(|c| c.n).unwrap_or(0)
}

/// Log a change and return the new revision. A change the app saw in Live
/// within two seconds of one Claude made is the same change, seen twice,
/// and is not logged again.
pub fn record_change(song_key: &str, by: &str, what: &str) -> u64 {
    let log = changes(song_key);
    let now = now_ms();
    if by == "live" {
        if let Some(last) = log.last() {
            if last.by == "claude" && now - last.at_ms < 2_000 {
                return last.n;
            }
        }
    }
    let n = log.last().map(|c| c.n).unwrap_or(0) + 1;
    let change = Change {
        n,
        at: crate::recorder::now_rfc3339(),
        at_ms: now,
        by: by.to_string(),
        what: what.chars().take(120).collect(),
    };
    let path = changes_path(song_key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{}", serde_json::to_string(&change).unwrap_or_default());
    }
    n
}

/// Tools that do not change how the set sounds: they read, play, steer,
/// remember, or talk to the producer. Everything else that is not read-only
/// moves the revision.
const NOT_A_CHANGE: &[&str] = &[
    "play_song",
    "go",
    "jump_to",
    "back",
    "hold_section",
    "next_section",
    "previous_section",
    "record_clip",
    "capture_mix",
    "clear_captures",
    "end_performance",
    "start_performance",
    "fire_scene",
    "fire_clip",
    "stop_clip",
    "start_playback",
    "stop_playback",
    "cue",
    "cancel_cue",
    "set_arrangement_time",
    "play_from",
    "back_to_arrangement",
    "panic",
    "remember",
    "stash",
    "song_memory",
    "export_set",
    "offer_options",
    "reply",
    "get_feedback",
    "heard",
    "listen",
    "compare",
    "listen_picture",
    "listen_meters",
    "keep_track_playing",
    "set_launch_quantization",
    "adv_sample_folders",
    "sample_folders",
    "device_vocabulary",
    "subscribe",
    "unsubscribe",
    "describe",
    "run",
    "search_browser",
    "measure_capture",
    "list_captures",
];

/// Whether a successful call to `tool` changed the set.
pub fn is_change(tool: &str, read_only: bool) -> bool {
    !read_only && !NOT_A_CHANGE.contains(&tool)
}

/// A change in a few words: the tool and its scalar arguments, e.g.
/// `set_track_mixer track=Pad volume=-3`.
pub fn describe_change(tool: &str, params: &Value) -> String {
    let mut parts = vec![tool.to_string()];
    if let Some(map) = params.as_object() {
        for (k, v) in map {
            let text = match v {
                Value::String(s) if !s.is_empty() => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(true) => "on".into(),
                _ => continue,
            };
            parts.push(format!("{k}={}", text.chars().take(24).collect::<String>()));
            if parts.len() >= 4 {
                break;
            }
        }
    }
    parts.join(" ")
}

// ── is the app listening? ───────────────────────────────────────────────────

/// What the app writes every few seconds while it runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppBeat {
    pub pid: u32,
    /// Remember what Live plays is on
    pub remembering: bool,
    /// The Mac can listen (macOS 14.4 or later)
    pub available: bool,
    pub updated_ms: i64,
    /// What the app last heard from Live: "playing", "stopped", "no_live"
    #[serde(default)]
    pub live: String,
    #[serde(default)]
    pub retention_days: Option<u32>,
}

pub fn app_beat_path() -> PathBuf {
    crate::state::listening_dir().join("app.json")
}

pub fn write_app_beat(beat: &AppBeat) {
    let path = app_beat_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec(beat).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    NotRunning,
    Unsupported,
    Off,
    Remembering,
}

pub fn app_state() -> AppState {
    let Some(beat) = std::fs::read(app_beat_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<AppBeat>(&b).ok())
    else {
        return AppState::NotRunning;
    };
    if now_ms() - beat.updated_ms > APP_STALE_MS {
        AppState::NotRunning
    } else if !beat.available {
        AppState::Unsupported
    } else if !beat.remembering {
        AppState::Off
    } else {
        AppState::Remembering
    }
}

/// Why there is nothing to read, in the producer's words.
pub fn nothing_heard(state: AppState) -> &'static str {
    match state {
        AppState::NotRunning => "The Mac app is not running, so nothing has been heard. Open Ableton Music Maker and turn on Listen › Remember what Live plays; from then on every time the producer plays, it is measured here.",
        AppState::Unsupported => "Listening needs macOS 14.4 or later on the Mac that runs Live, so nothing has been heard. capture_mix still measures by recording.",
        AppState::Off => "Remember what Live plays is off in the Mac app (Listen › Remember), so nothing has been heard. It is the producer's switch to turn on.",
        AppState::Remembering => "The Mac app is remembering, but nothing has been played in this set since it started. Ask the producer to play it — never start the transport to measure while they are listening.",
    }
}

// ── which set ───────────────────────────────────────────────────────────────

/// The open set's song key, as song memory keys it; when Live cannot be
/// asked, the song most recently heard (and a note saying so).
pub fn song_key(live: &LiveState) -> (String, Option<String>) {
    use crate::lom::{Batch, Op, Path};
    let asked = Batch::new()
        .push(Op::get(&Path::song().attr("file_path"), "set"))
        .run(live);
    match asked {
        Ok(out) => {
            let path = out.get("set").and_then(Value::as_str).unwrap_or("");
            (crate::memory::key_for(path), None)
        }
        Err(_) => match newest_song() {
            Some(k) => (
                k,
                Some("Live did not answer, so this is the set heard most recently.".into()),
            ),
            None => (crate::memory::key_for(""), None),
        },
    }
}

// ── the app's view of the set ───────────────────────────────────────────────

/// What a take is stamped with when it starts, read from Live through the
/// same readers the tools use: the set's identity (as song memory keys it),
/// its name, its locators and its section rows, and which Session clips play.
pub fn set_view(
    live: &LiveState,
) -> (String, String, crate::recorder::SetView, Vec<(String, i64)>) {
    use crate::recorder::{SectionMark, SetView};
    let (key, _) = song_key(live);
    let path = crate::lom::Batch::new()
        .push(crate::lom::Op::get(
            &crate::lom::Path::song().attr("file_path"),
            "set",
        ))
        .run(live)
        .ok()
        .and_then(|o| o.get("set").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    let mut sections: Vec<SectionMark> = crate::tools::locators(live)
        .unwrap_or_default()
        .into_iter()
        .map(|(name, beat)| SectionMark {
            name,
            kind: "locator".into(),
            index: None,
            beat: Some(beat),
        })
        .collect();
    let mut slots = Vec::new();
    if let Ok(state) = crate::tools::read_perf_state(live) {
        for s in crate::song::sections(&state) {
            sections.push(SectionMark {
                name: s.name,
                kind: "scene".into(),
                index: Some(s.index),
                beat: None,
            });
        }
        slots = state
            .tracks
            .iter()
            .map(|t| {
                (
                    format!("song.tracks[{}].playing_slot_index", t.index),
                    t.playing_slot_index,
                )
            })
            .collect();
    }
    let session = slots.iter().any(|(_, s)| *s >= 0);
    let name = crate::memory::set_name_of(&path);
    (
        key,
        if name.is_empty() {
            "Untitled".into()
        } else {
            name
        },
        SetView { sections, session },
        slots,
    )
}

/// The app's [`TakeSink`](crate::recorder::TakeSink): Live is asked about
/// the set when a take starts, and takes go to the store.
pub struct LiveSink<'a> {
    pub live: &'a LiveState,
    /// Set by `set_info`, for the app's own use (comments, the strip)
    pub song_key: Option<String>,
}

impl crate::recorder::TakeSink for LiveSink<'_> {
    fn set_info(&mut self) -> (String, String, crate::recorder::SetView) {
        let (key, name, view, _) = set_view(self.live);
        self.song_key = Some(key.clone());
        (key, name, view)
    }
    fn revision(&mut self, song_key: &str) -> u64 {
        revision(song_key)
    }
    fn write(&mut self, take: &Take, frames: &[FrameRow]) {
        if let Err(e) = write_take(take, frames) {
            tracing::warn!("could not write take {}: {e}", take.id);
        }
    }
}

// ── reading takes ───────────────────────────────────────────────────────────

/// What part of the song a reading is about.
#[derive(Debug, Clone, PartialEq)]
pub enum Range {
    Section(String),
    Bars(i64, i64),
    /// The newest take, whole
    Last,
}

impl Range {
    pub fn parse(section: Option<&str>, bars: Option<&Value>, last: bool) -> Result<Self, String> {
        if let Some(b) = bars {
            let (a, z) = parse_bars(b)?;
            return Ok(Range::Bars(a, z));
        }
        if let Some(s) = section.map(str::trim).filter(|s| !s.is_empty()) {
            return Ok(Range::Section(s.to_string()));
        }
        if last {
            return Ok(Range::Last);
        }
        Ok(Range::Last)
    }

    fn label(&self) -> String {
        match self {
            Range::Section(s) => s.clone(),
            Range::Bars(a, z) if a == z => format!("bar {a}"),
            Range::Bars(a, z) => format!("bars {a}–{z}"),
            Range::Last => "the last take".into(),
        }
    }

    fn holds(&self, row: &BarRow) -> bool {
        match self {
            Range::Section(s) => section_matches(&row.section, s),
            Range::Bars(a, z) => row.bar >= *a && row.bar <= *z,
            Range::Last => true,
        }
    }
}

fn section_matches(heard: &str, asked: &str) -> bool {
    let h = crate::song::parse_section_name(heard).0.to_lowercase();
    let a = crate::song::parse_section_name(asked).0.to_lowercase();
    !h.is_empty() && h == a
}

/// `33`, `"33-40"`, `"33–40"`, `[33, 40]`.
pub fn parse_bars(v: &Value) -> Result<(i64, i64), String> {
    let bad = || format!("bars is a bar (33), a range (\"33-40\") or [33, 40], not {v}");
    let (a, z) = match v {
        Value::Number(n) => {
            let b = n.as_i64().ok_or_else(bad)?;
            (b, b)
        }
        Value::Array(a) if a.len() == 2 => (
            a[0].as_i64().ok_or_else(bad)?,
            a[1].as_i64().ok_or_else(bad)?,
        ),
        Value::String(s) => {
            let s = s
                .trim()
                .trim_start_matches("bars")
                .trim_start_matches("bar")
                .trim();
            let parts: Vec<&str> = s.split(['-', '–', '—']).map(str::trim).collect();
            match parts.as_slice() {
                [one] => {
                    let b = one.parse().map_err(|_| bad())?;
                    (b, b)
                }
                [x, y] => (x.parse().map_err(|_| bad())?, y.parse().map_err(|_| bad())?),
                _ => return Err(bad()),
            }
        }
        _ => return Err(bad()),
    };
    if a < 1 || z < a {
        return Err(format!(
            "bars run from 1 upwards, first to last: got {a}–{z}"
        ));
    }
    Ok((a, z))
}

/// The newest take that played any of `range`, and those bars of it. With
/// `revision`, only bars heard at that revision count.
pub fn pick<'a>(
    takes: &'a [Take],
    range: &Range,
    revision: Option<u64>,
) -> Option<(&'a Take, Vec<&'a BarRow>)> {
    for take in takes.iter().rev() {
        let bars: Vec<&BarRow> = take
            .bars
            .iter()
            .filter(|b| range.holds(b) && revision.is_none_or(|r| b.revision == r))
            .collect();
        if !bars.is_empty() {
            return Some((take, bars));
        }
    }
    None
}

/// What a set of bars measured, combined by energy and by time.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub bars: usize,
    pub first: i64,
    pub last: i64,
    pub seconds: f64,
    pub lufs: f64,
    pub peak: f64,
    pub true_peak: f64,
    pub rms: f64,
    pub crest: f64,
    /// Each range's share of the whole, dB (0 = all of it)
    pub range_share_db: [f64; 6],
    /// Energy per octave band, as shares 0–1 (what a capture reports)
    pub octave_shares: Vec<f64>,
    /// Energy below 250 Hz (the three lowest octaves), dB
    pub low_db: f64,
    /// Share of the energy below 250 Hz, 0–1
    pub low_share: f64,
    pub clipped: usize,
    pub clipped_bars: Vec<i64>,
    pub silent_bars: Vec<i64>,
    pub duck: Option<audio::Duck>,
    pub incomplete: usize,
}

fn power(db: f64) -> f64 {
    10f64.powf(db / 10.0)
}

fn to_db(p: f64) -> f64 {
    if p <= 0.0 {
        -120.0
    } else {
        10.0 * p.log10()
    }
}

fn bar_seconds(b: &BarRow) -> f64 {
    b.beats * 60.0 / b.tempo.max(1.0)
}

pub fn summarize(bars: &[&BarRow]) -> Summary {
    let total_s: f64 = bars.iter().map(|b| bar_seconds(b)).sum::<f64>().max(1e-9);
    let mean = |f: &dyn Fn(&BarRow) -> f64| -> f64 {
        to_db(
            bars.iter()
                .map(|b| power(f(b)) * bar_seconds(b))
                .sum::<f64>()
                / total_s,
        )
    };
    let lufs = mean(&|b| b.lufs).max(meter::LUFS_FLOOR);
    // RMS is an amplitude in dB: its square is the power.
    let rms = mean(&|b| b.rms);
    let peak = bars.iter().map(|b| b.peak).fold(f64::MIN, f64::max);
    let true_peak = bars.iter().map(|b| b.true_peak).fold(f64::MIN, f64::max);
    let energy = |pick: &dyn Fn(&BarRow) -> &Vec<f64>, n: usize| -> Vec<f64> {
        (0..n)
            .map(|i| {
                bars.iter()
                    .map(|b| pick(b).get(i).map_or(0.0, |d| power(*d)) * bar_seconds(b))
                    .sum::<f64>()
            })
            .collect()
    };
    let ranges = energy(&|b| &b.ranges, 6);
    let range_total: f64 = ranges.iter().sum();
    let mut range_share_db = [meter::FLOOR_DB; 6];
    for (o, e) in range_share_db.iter_mut().zip(&ranges) {
        if range_total > 0.0 && *e > 0.0 {
            *o = r1((10.0 * (e / range_total).log10()).max(meter::FLOOR_DB));
        }
    }
    let octaves = energy(&|b| &b.octaves, audio::OCTAVE_CENTERS.len());
    let oct_total: f64 = octaves.iter().sum();
    let octave_shares: Vec<f64> = octaves
        .iter()
        .map(|e| if oct_total > 0.0 { e / oct_total } else { 0.0 })
        .collect();
    let low: f64 = octaves.iter().take(3).sum();
    let mut steps: Vec<f64> = Vec::new();
    for b in bars {
        if b.complete {
            steps.extend(b.steps.iter().map(|s| s.unwrap_or(-120.0)));
        }
    }
    let mut clipped_bars: Vec<i64> = bars
        .iter()
        .filter(|b| b.clipped > 0)
        .map(|b| b.bar)
        .collect();
    clipped_bars.dedup();
    let mut silent_bars: Vec<i64> = bars.iter().filter(|b| b.silent).map(|b| b.bar).collect();
    silent_bars.dedup();
    Summary {
        bars: bars.len(),
        first: bars.iter().map(|b| b.bar).min().unwrap_or(0),
        last: bars.iter().map(|b| b.bar).max().unwrap_or(0),
        seconds: total_s,
        lufs: r1(lufs),
        peak: r1(peak),
        true_peak: r1(true_peak),
        rms: r1(rms),
        crest: r1(peak - rms),
        range_share_db,
        octave_shares,
        low_db: r1(to_db(low / total_s)),
        low_share: if oct_total > 0.0 {
            low / oct_total
        } else {
            0.0
        },
        clipped: bars.iter().map(|b| b.clipped).sum(),
        clipped_bars,
        silent_bars,
        duck: audio::duck(&steps, crate::recorder::STEPS_PER_BEAT),
        incomplete: bars.iter().filter(|b| !b.complete).count(),
    }
}

/// The chroma of a set of bars, averaged over their frames, and the chord it
/// reads as.
fn heard_as(song_key: &str, take: &Take, bars: &[&BarRow]) -> Option<String> {
    let wanted: std::collections::BTreeSet<i64> = bars.iter().map(|b| b.bar).collect();
    let bpb = take.beats_per_bar.max(0.25);
    let mut sum = [0.0f32; 12];
    let mut n = 0;
    for f in frames(song_key, &take.id) {
        let bar = (f.beat / bpb + 1e-9).floor() as i64 + 1;
        if wanted.contains(&bar) && f.lufs > -50.0 {
            for (s, c) in sum.iter_mut().zip(f.chroma) {
                *s += c;
            }
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let max = sum.iter().cloned().fold(0.0, f32::max);
    if max <= 0.0 {
        return None;
    }
    let profile = sum.map(|v| v / max);
    meter::chord_of(&profile).map(|(chord, _)| chord)
}

/// "14:02" today, "Oct 16 14:02" another day.
pub fn when(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339) {
        Ok(t) => {
            let t = t.with_timezone(&chrono::Local);
            if t.date_naive() == chrono::Local::now().date_naive() {
                t.format("%H:%M").to_string()
            } else {
                t.format("%b %-d %H:%M").to_string()
            }
        }
        Err(_) => rfc3339.to_string(),
    }
}

/// The take's standing against the set now: "revision 41 of 43: since then,
/// set_track_mixer track=Pad volume=-3 (14:10)", or "at the current revision".
fn standing(revision: u64, current: u64, log: &[Change]) -> String {
    if revision >= current {
        return format!("at the current revision ({current})");
    }
    let since: Vec<&Change> = log.iter().filter(|c| c.n > revision).collect();
    let mut list: Vec<String> = since
        .iter()
        .rev()
        .take(3)
        .map(|c| {
            format!(
                "{} ({}{})",
                c.what,
                when(&c.at),
                if c.by == "live" { ", in Live" } else { "" }
            )
        })
        .collect();
    if since.len() > 3 {
        list.push(format!("and {} more", since.len() - 3));
    }
    format!(
        "revision {revision} of {current}: {} since — not in this take: {}",
        if since.len() == 1 {
            "1 change".to_string()
        } else {
            format!("{} changes", since.len())
        },
        list.join("; ")
    )
}

fn percent(share: f64) -> String {
    format!("{:.0} %", share * 100.0)
}

fn signed(v: f64, unit: &str) -> String {
    let v = r1(v);
    if v > 0.0 {
        format!("+{v:.1} {unit}")
    } else if v < 0.0 {
        format!("−{:.1} {unit}", -v)
    } else {
        format!("±0.0 {unit}")
    }
}

fn summary_line(s: &Summary) -> String {
    let ranges = meter::RANGES
        .iter()
        .zip(s.range_share_db)
        .map(|((n, _, _), v)| format!("{n} {v:.1}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut out = format!(
        "loudness {:.1} LUFS · peak {:.1} dBFS (true peak {:.1} dBTP) · RMS {:.1} dBFS · crest {:.1} dB · {} of the energy below 250 Hz\n  ranges (share of the whole, dB): {ranges}",
        s.lufs,
        s.peak,
        s.true_peak,
        s.rms,
        s.crest,
        percent(s.low_share)
    );
    if let Some(d) = s.duck.as_ref().filter(|d| d.median_db >= 3.0) {
        let trough = d
            .profile
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i + 1)
            .unwrap_or(1);
        out.push_str(&format!(
            "\n  ducks {:.1} dB inside the beat (median of {} beats; the trough is 16th {trough} of 4)",
            d.median_db, d.beats
        ));
    }
    if s.clipped > 0 {
        out.push_str(&format!(
            "\n  clipped: {} samples at full scale in bar{} {}",
            s.clipped,
            if s.clipped_bars.len() == 1 { "" } else { "s" },
            s.clipped_bars
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    } else if s.true_peak > -1.0 {
        out.push_str("\n  true peak within 1 dB of full scale");
    }
    if !s.silent_bars.is_empty() {
        out.push_str(&format!(
            "\n  silent: bar{} {}",
            if s.silent_bars.len() == 1 { "" } else { "s" },
            s.silent_bars
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

/// Bars grouped by section in the order they were played.
fn by_section<'a>(bars: &[&'a BarRow]) -> Vec<(String, Vec<&'a BarRow>)> {
    let mut out: Vec<(String, Vec<&BarRow>)> = Vec::new();
    for b in bars {
        let name = crate::song::parse_section_name(&b.section).0;
        let name = if name.is_empty() {
            "(no section)".to_string()
        } else {
            name
        };
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => v.push(b),
            None => out.push((name, vec![b])),
        }
    }
    out
}

// ── the tools ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct HeardParams {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListenParams {
    /// A section by name ("Drop"): the bars that played in it
    #[serde(default)]
    pub section: Option<String>,
    /// Bars instead: 33, "33-40" or [33, 40] (Live's 1-based bars)
    #[serde(default)]
    pub bars: Option<Value>,
    /// The newest take, whole (the default when nothing else is given)
    #[serde(default)]
    pub last: bool,
    /// "any" (default): the newest take, labelled with its revision.
    /// "current": only bars heard since the last change
    #[serde(default)]
    pub revision: Option<String>,
    /// "bar" (default), "beat" or "16th": the per-step table under the summary
    #[serde(default)]
    pub resolution: Option<String>,
    /// Play the range now and read it once it has played. Only while the
    /// producer has switched on "Claude may play the set" in the app
    #[serde(default)]
    pub play: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CompareParams {
    /// A section by name: the same bars in two takes (before and after a change)
    #[serde(default)]
    pub section: Option<String>,
    /// Or bars: 33, "33-40" or [33, 40]
    #[serde(default)]
    pub bars: Option<Value>,
    /// Option A against option B: a second section to compare the first with
    #[serde(default)]
    pub against_section: Option<String>,
    /// Or second bars
    #[serde(default)]
    pub against_bars: Option<Value>,
    /// The earlier take's id (default: the newest take of these bars at an
    /// older revision)
    #[serde(default)]
    pub a: Option<String>,
    /// The later take's id (default: the newest take of these bars)
    #[serde(default)]
    pub b: Option<String>,
}

pub fn heard_body(live: &LiveState, _p: &HeardParams) -> ToolResult {
    let (key, note) = song_key(live);
    Ok(heard_text(&key, note.as_deref()))
}

/// Sections of the set, in order: the newest take's list, then any section
/// a bar was heard in that the list lacks.
fn set_sections(takes: &[Take]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(t) = takes.iter().rev().find(|t| !t.sections.is_empty()) {
        let mut marks = t.sections.clone();
        marks.sort_by(|a, b| {
            (
                a.kind != "locator",
                a.beat.unwrap_or(0.0),
                a.index.unwrap_or(0),
            )
                .partial_cmp(&(
                    b.kind != "locator",
                    b.beat.unwrap_or(0.0),
                    b.index.unwrap_or(0),
                ))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for m in marks {
            let name = crate::song::parse_section_name(&m.name).0;
            if !name.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
                out.push(name);
            }
        }
    }
    for t in takes {
        for b in &t.bars {
            let name = crate::song::parse_section_name(&b.section).0;
            if !name.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
                out.push(name);
            }
        }
    }
    out
}

/// Where each section stands: heard at the current revision, heard before a
/// change (and which), or never heard.
pub struct HeardState {
    pub current: Vec<(String, String)>,
    pub stale: Vec<(String, u64, String)>,
    pub never: Vec<String>,
    pub revision: u64,
}

pub fn heard_state(song_key: &str) -> HeardState {
    let all = takes(song_key);
    let log = changes(song_key);
    let current = log.last().map(|c| c.n).unwrap_or(0);
    let mut st = HeardState {
        current: Vec::new(),
        stale: Vec::new(),
        never: Vec::new(),
        revision: current,
    };
    for name in set_sections(&all) {
        let range = Range::Section(name.clone());
        if let Some((take, _)) = pick(&all, &range, Some(current)) {
            st.current.push((name, when(&take.started_at)));
        } else if let Some((take, bars)) = pick(&all, &range, None) {
            let rev = bars.iter().map(|b| b.revision).max().unwrap_or(0);
            let _ = take;
            st.stale.push((name, rev, when(&take.started_at)));
        } else {
            st.never.push(name);
        }
    }
    st
}

pub fn heard_text(song_key: &str, note: Option<&str>) -> String {
    let all = takes(song_key);
    let app = app_state();
    let mut out = String::new();
    if let Some(n) = note {
        out.push_str(n);
        out.push('\n');
    }
    if all.is_empty() {
        out.push_str(nothing_heard(app));
        return out;
    }
    let log = changes(song_key);
    let st = heard_state(song_key);
    let newest = all.last().unwrap();
    out.push_str(&format!(
        "Measured from playback (the Mac app's memory of this set: {} take{}, the newest played {}). The set is at revision {}.\n",
        all.len(),
        if all.len() == 1 { "" } else { "s" },
        when(&newest.started_at),
        st.revision
    ));
    let names = |v: Vec<String>| {
        if v.is_empty() {
            "none".to_string()
        } else {
            v.join(", ")
        }
    };
    out.push_str(&format!(
        "Heard at the current revision: {}.\n",
        names(
            st.current
                .iter()
                .map(|(n, at)| format!("{n} ({at})"))
                .collect()
        )
    ));
    if !st.stale.is_empty() {
        let lines: Vec<String> = st
            .stale
            .iter()
            .map(|(n, rev, at)| {
                let last_change = log
                    .iter()
                    .rfind(|c| c.n > *rev)
                    .map(|c| format!("; since then: {} at {}", c.what, when(&c.at)))
                    .unwrap_or_default();
                format!("{n} (played {at}, revision {rev}{last_change})")
            })
            .collect();
        out.push_str(&format!(
            "Heard before the last change: {}.\n",
            lines.join(", ")
        ));
    }
    out.push_str(&format!("Never heard: {}.\n", names(st.never.clone())));
    let open: Vec<&Take> = all.iter().filter(|t| t.open).collect();
    if !open.is_empty() {
        out.push_str("A take is being recorded right now (the producer is playing).\n");
    }
    match app {
        AppState::Remembering => {}
        other => {
            out.push_str("Note: ");
            out.push_str(match other {
                AppState::NotRunning => {
                    "the Mac app is not running now, so new playback is not being heard."
                }
                AppState::Off => {
                    "Remember what Live plays is off now, so new playback is not being heard."
                }
                AppState::Unsupported => "this Mac cannot listen (macOS 14.4 or later is needed).",
                AppState::Remembering => "",
            });
            out.push('\n');
        }
    }
    if !st.stale.is_empty() || !st.never.is_empty() {
        out.push_str("To hear what is stale, ask the producer to play it — or, if they have switched on \"Claude may play the set\" in the app, listen {section, play: true}.");
    }
    out.trim_end().to_string()
}

/// The line `get_context` carries under its clock line: none when there is
/// nothing to say.
pub fn context_line(song_key: &str) -> Option<String> {
    let all = takes(song_key);
    let app = app_state();
    if all.is_empty() {
        return match app {
            AppState::Remembering => Some("🎧 remembering: nothing of this set played yet".into()),
            _ => None,
        };
    }
    let st = heard_state(song_key);
    let total = st.current.len() + st.stale.len() + st.never.len();
    let mut line = format!("🎧 heard: {} of {total} sections current", st.current.len());
    let log = changes(song_key);
    if let Some((name, rev, _)) = st.stale.first() {
        if let Some(c) = log.iter().rfind(|c| c.n > *rev) {
            line.push_str(&format!("; {name} not heard since {}", c.what));
        }
        if st.stale.len() > 1 {
            line.push_str(&format!(" (+{} more stale)", st.stale.len() - 1));
        }
    }
    if !st.never.is_empty() {
        line.push_str(&format!("; never heard: {}", st.never.join(", ")));
    }
    if app != AppState::Remembering {
        line.push_str(" · the app is not remembering now");
    }
    line.push('.');
    Some(line)
}

fn resolution(p: Option<&str>) -> Result<usize, String> {
    match p.map(|s| s.trim().to_lowercase()).as_deref() {
        None | Some("") | Some("bar") | Some("bars") => Ok(0),
        Some("beat") | Some("beats") => Ok(1),
        Some("16th") | Some("16ths") | Some("sixteenth") => Ok(4),
        Some(other) => Err(format!(
            "resolution is \"bar\", \"beat\" or \"16th\", not \"{other}\""
        )),
    }
}

pub fn listen_body(live: &LiveState, p: &ListenParams) -> ToolResult {
    let range = Range::parse(p.section.as_deref(), p.bars.as_ref(), p.last)?;
    let steps = resolution(p.resolution.as_deref())?;
    let only_current = match p
        .revision
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .as_deref()
    {
        None | Some("") | Some("any") => false,
        Some("current") => true,
        Some(other) => {
            return Err(format!(
                "revision is \"current\" or \"any\", not \"{other}\""
            ))
        }
    };
    if p.play {
        if range == Range::Last {
            return Err("play needs a section or bars to play.".into());
        }
        crate::feedback::play_to_listen(live, &range)?;
    }
    let (key, note) = song_key(live);
    Ok(listen_text(
        &key,
        &range,
        only_current,
        steps,
        note.as_deref(),
    ))
}

pub fn listen_text(
    song_key: &str,
    range: &Range,
    only_current: bool,
    steps: usize,
    note: Option<&str>,
) -> String {
    let all = takes(song_key);
    let mut out = String::new();
    if let Some(n) = note {
        out.push_str(n);
        out.push('\n');
    }
    if all.is_empty() {
        out.push_str(nothing_heard(app_state()));
        return out;
    }
    let log = changes(song_key);
    let current = log.last().map(|c| c.n).unwrap_or(0);
    let picked = pick(&all, range, only_current.then_some(current));
    let Some((take, bars)) = picked else {
        let stale = only_current.then(|| pick(&all, range, None)).flatten();
        out.push_str(&match (stale, range) {
            (Some((t, _)), _) => format!(
                "{} has not been heard since the last change (it was last played {}). Ask the producer to play it, or read it with revision: \"any\" — labelled as from before the change.",
                range.label(),
                when(&t.started_at)
            ),
            (None, Range::Section(s)) => {
                let known = set_sections(&all);
                format!(
                    "{s} has never been heard. Sections heard or in the set: {}.",
                    if known.is_empty() { "none".into() } else { known.join(", ") }
                )
            }
            (None, _) => format!("{} has never been heard.", range.label()),
        });
        return out;
    };
    let rev = bars
        .iter()
        .map(|b| b.revision)
        .max()
        .unwrap_or(take.revision_start);
    out.push_str(&format!(
        "{} — measured from playback at {} (take {}), {}.\n",
        range.label(),
        when(&take.started_at),
        take.id,
        standing(rev, current, &log)
    ));
    if take.open {
        out.push_str("This take is still being played; the newest bars may not be in it yet.\n");
    }
    if take.follows_session {
        out.push_str("Session clips were playing in this take, so a bar number is the song's clock, not a place in the Arrangement.\n");
    }
    let key_now = crate::memory::load(song_key)
        .map(|m| m.as_of.key)
        .unwrap_or_default();
    for (name, rows) in by_section(&bars) {
        let s = summarize(&rows);
        out.push_str(&format!(
            "{name} (bar{} {}{}, {:.0} s):\n  {}\n",
            if s.first == s.last { "" } else { "s" },
            s.first,
            if s.first == s.last {
                String::new()
            } else {
                format!("–{}", s.last)
            },
            s.seconds,
            summary_line(&s)
        ));
        let low: Vec<String> = rows
            .iter()
            .map(|b| {
                let tot: f64 = b.octaves.iter().map(|d| power(*d)).sum();
                let lo: f64 = b.octaves.iter().take(3).map(|d| power(*d)).sum();
                format!(
                    "{}:{}",
                    b.bar,
                    if tot > 0.0 {
                        format!("{:.0}%", lo / tot * 100.0)
                    } else {
                        "—".into()
                    }
                )
            })
            .collect();
        out.push_str(&format!(
            "  low end (below 250 Hz) per bar: {}\n",
            low.join(" ")
        ));
        if let Some(chord) = heard_as(song_key, take, &rows) {
            out.push_str(&format!(
                "  heard as {chord} (chroma){}\n",
                if key_now.is_empty() {
                    String::new()
                } else {
                    format!(" — the set's key is {key_now}")
                }
            ));
        }
    }
    let marks: Vec<String> = take
        .markers
        .iter()
        .filter(|m| m.kind == "jump" || m.kind == "loop" || m.kind == "tempo")
        .map(|m| match m.kind.as_str() {
            "tempo" => format!("tempo → {:.1} at {:.0} s", m.tempo, m.at_s),
            k => format!(
                "{k} at {:.0} s (beat {:.0} → {:.0})",
                m.at_s, m.from_beat, m.to_beat
            ),
        })
        .collect();
    if !marks.is_empty() {
        out.push_str(&format!(
            "The playhead moved in this take: {}.\n",
            marks.join("; ")
        ));
    }
    out.push_str(&bar_table(&bars, steps));
    out.trim_end().to_string()
}

fn bar_table(bars: &[&BarRow], steps: usize) -> String {
    let mut out = String::new();
    let shown: Vec<&&BarRow> = bars.iter().take(64).collect();
    match steps {
        0 => {
            out.push_str("bar · LUFS · peak · RMS · crest\n");
            for b in &shown {
                out.push_str(&format!(
                    "{}{} · {:.1} · {:.1} · {:.1} · {:.1}{}\n",
                    b.bar,
                    if b.complete { "" } else { "*" },
                    b.lufs,
                    b.peak,
                    b.rms,
                    b.crest,
                    if b.clipped > 0 { " · clipped" } else { "" }
                ));
            }
        }
        n => {
            out.push_str(if n == 1 {
                "RMS per beat (dBFS):\n"
            } else {
                "RMS per 16th (dBFS):\n"
            });
            for b in &shown {
                let vals: Vec<String> = if n == 1 {
                    b.steps
                        .chunks(crate::recorder::STEPS_PER_BEAT)
                        .map(|c| {
                            let p: Vec<f64> =
                                c.iter().flatten().map(|d| 10f64.powf(d / 10.0)).collect();
                            if p.is_empty() {
                                "—".into()
                            } else {
                                format!("{:.1}", to_db(p.iter().sum::<f64>() / p.len() as f64))
                            }
                        })
                        .collect()
                } else {
                    b.steps
                        .iter()
                        .map(|s| s.map_or("—".to_string(), |v| format!("{v:.1}")))
                        .collect()
                };
                out.push_str(&format!("{}: {}\n", b.bar, vals.join(" ")));
            }
        }
    }
    if bars.len() > shown.len() {
        out.push_str(&format!(
            "… {} more bars; ask for fewer bars to see them.\n",
            bars.len() - shown.len()
        ));
    }
    if bars.iter().any(|b| !b.complete) {
        out.push_str("* only part of the bar was heard (a jump or a stop cut it).\n");
    }
    out
}

pub fn compare_body(live: &LiveState, p: &CompareParams) -> ToolResult {
    let (key, note) = song_key(live);
    let range = Range::parse(p.section.as_deref(), p.bars.as_ref(), false)?;
    if range == Range::Last {
        return Err("compare needs a section or bars.".into());
    }
    let against = match (p.against_section.as_deref(), p.against_bars.as_ref()) {
        (None, None) => None,
        (s, b) => Some(Range::parse(s, b, false)?),
    };
    compare_text(
        &key,
        &range,
        against.as_ref(),
        p.a.as_deref(),
        p.b.as_deref(),
        note.as_deref(),
    )
}

pub fn compare_text(
    song_key: &str,
    range: &Range,
    against: Option<&Range>,
    a_id: Option<&str>,
    b_id: Option<&str>,
    note: Option<&str>,
) -> ToolResult {
    let all = takes(song_key);
    if all.is_empty() {
        return Ok(nothing_heard(app_state()).to_string());
    }
    let log = changes(song_key);
    let current = log.last().map(|c| c.n).unwrap_or(0);
    let by_id = |id: &str, r: &Range| -> Result<(usize, Vec<&BarRow>), String> {
        let i = all
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| format!("no take {id}; listen names the take it read"))?;
        let bars: Vec<&BarRow> = all[i].bars.iter().filter(|b| r.holds(b)).collect();
        if bars.is_empty() {
            return Err(format!("take {id} did not play {}", r.label()));
        }
        Ok((i, bars))
    };
    let newest =
        |r: &Range, below: Option<u64>, before: Option<usize>| -> Option<(usize, Vec<&BarRow>)> {
            for (i, t) in all.iter().enumerate().rev() {
                if before.is_some_and(|b| i >= b) {
                    continue;
                }
                let bars: Vec<&BarRow> = t
                    .bars
                    .iter()
                    .filter(|b| r.holds(b) && below.is_none_or(|rev| b.revision < rev))
                    .collect();
                if !bars.is_empty() {
                    return Some((i, bars));
                }
            }
            None
        };
    // B: the later reading — the newest take of the range (or of `against`).
    let b_range = against.unwrap_or(range);
    let (bi, b_bars) = match b_id {
        Some(id) => by_id(id, b_range)?,
        None => newest(b_range, None, None)
            .ok_or_else(|| format!("{} has never been heard.", b_range.label()))?,
    };
    let b_rev = b_bars.iter().map(|b| b.revision).max().unwrap_or(0);
    // A: with `against`, the newest take of the first range; else the
    // newest take of the same bars at an older revision, else an older play.
    let (ai, a_bars) = match (a_id, against) {
        (Some(id), _) => by_id(id, range)?,
        (None, Some(_)) => newest(range, None, None).ok_or_else(|| format!("{} has never been heard.", range.label()))?,
        (None, None) => match newest(range, Some(b_rev), None) {
            Some(x) => x,
            None => newest(range, None, Some(bi)).ok_or_else(|| {
                format!(
                    "{} has been heard only once (take {}, {}), so there is nothing to compare it with yet. After the next change, ask the producer to play it again.",
                    range.label(),
                    all[bi].id,
                    when(&all[bi].started_at)
                )
            })?,
        },
    };
    let a_rev = a_bars.iter().map(|b| b.revision).max().unwrap_or(0);
    let a = summarize(&a_bars);
    let b = summarize(&b_bars);
    let mut out = String::new();
    if let Some(n) = note {
        out.push_str(n);
        out.push('\n');
    }
    let title = match against {
        Some(other) => format!("{} against {}", range.label(), other.label()),
        None if a_rev != b_rev => format!("{}, revision {a_rev} against {b_rev}", range.label()),
        None => format!("{}, two plays at revision {b_rev}", range.label()),
    };
    out.push_str(&format!(
        "{title}: {} below 250 Hz, crest {}, loudness {}, peak {}.\n",
        signed(b.low_db - a.low_db, "dB"),
        signed(b.crest - a.crest, "dB"),
        signed(b.lufs - a.lufs, "LU"),
        signed(b.peak - a.peak, "dB"),
    ));
    out.push_str(&format!(
        "A: take {} played {} (revision {a_rev}), bars {}–{}. B: take {} played {} (revision {b_rev}{}), bars {}–{}. Both measured from playback.\n",
        all[ai].id,
        when(&all[ai].started_at),
        a.first,
        a.last,
        all[bi].id,
        when(&all[bi].started_at),
        if b_rev >= current { ", current" } else { "" },
        b.first,
        b.last
    ));
    let moved: Vec<String> = meter::RANGES
        .iter()
        .enumerate()
        .filter(|(i, _)| (b.range_share_db[*i] - a.range_share_db[*i]).abs() >= 0.5)
        .map(|(i, (n, _, _))| {
            format!(
                "{n} {}",
                signed(b.range_share_db[i] - a.range_share_db[i], "dB")
            )
        })
        .collect();
    out.push_str(&format!(
        "Share of the whole by range: {}.\n",
        if moved.is_empty() {
            "unchanged within 0.5 dB".to_string()
        } else {
            moved.join(", ")
        }
    ));
    match (a.duck.as_ref(), b.duck.as_ref()) {
        (Some(x), Some(y))
            if (x.median_db >= 3.0 || y.median_db >= 3.0)
                && (y.median_db - x.median_db).abs() >= 0.5 =>
        {
            out.push_str(&format!(
                "Duck depth {:.1} → {:.1} dB.\n",
                x.median_db, y.median_db
            ));
        }
        _ => {}
    }
    if a.clipped != b.clipped {
        out.push_str(&format!("Clipped samples {} → {}.\n", a.clipped, b.clipped));
    }
    if against.is_none() {
        let mut per_bar = Vec::new();
        for bb in &b_bars {
            if let Some(ab) = a_bars.iter().rev().find(|x| x.bar == bb.bar) {
                let d = bb.lufs - ab.lufs;
                if d.abs() >= 1.0 {
                    per_bar.push(format!("{} {}", bb.bar, signed(d, "LU")));
                }
            }
        }
        if !per_bar.is_empty() {
            out.push_str(&format!(
                "Bars that moved by 1 LU or more: {}.\n",
                per_bar.join(", ")
            ));
        }
        if a_rev < b_rev {
            let between: Vec<String> = log
                .iter()
                .filter(|c| c.n > a_rev && c.n <= b_rev)
                .map(|c| c.what.clone())
                .collect();
            if !between.is_empty() {
                out.push_str(&format!("Changes between them: {}.\n", between.join("; ")));
            }
        }
    }
    if b_rev < current {
        out.push_str(&format!(
            "B is not current either: {}.\n",
            standing(b_rev, current, &log)
        ));
    }
    Ok(out.trim_end().to_string())
}

/// What the app's Feedback strip draws: the set's sections in order, each
/// with its status and a loudness value per bar from the newest take.
pub fn strip(song_key: &str) -> Value {
    let all = takes(song_key);
    let st = heard_state(song_key);
    let status_of = |name: &str| -> &str {
        if st.current.iter().any(|(n, _)| n == name) {
            "current"
        } else if st.stale.iter().any(|(n, _, _)| n == name) {
            "stale"
        } else {
            "never"
        }
    };
    let mut sections: Vec<Value> = Vec::new();
    for name in set_sections(&all) {
        let picked = pick(&all, &Range::Section(name.clone()), None);
        let bars: Vec<Value> = picked
            .as_ref()
            .map(|(_, rows)| {
                rows.iter()
                    .map(|b| json!({"bar": b.bar, "lufs": b.lufs, "peak": b.peak, "revision": b.revision}))
                    .collect()
            })
            .unwrap_or_default();
        sections.push(json!({
            "name": name,
            "status": status_of(&name),
            "bars": bars,
            "played": picked.map(|(t, _)| when(&t.started_at)),
        }));
    }
    json!({
        "song_key": song_key,
        "revision": st.revision,
        "takes": all.len(),
        "sections": sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_parse_every_way_a_producer_writes_them() {
        assert_eq!(parse_bars(&json!(33)).unwrap(), (33, 33));
        assert_eq!(parse_bars(&json!("33-40")).unwrap(), (33, 40));
        assert_eq!(parse_bars(&json!("bars 33–40")).unwrap(), (33, 40));
        assert_eq!(parse_bars(&json!([9, 16])).unwrap(), (9, 16));
        assert!(parse_bars(&json!("40-33")).is_err());
        assert!(parse_bars(&json!(0)).is_err());
        assert!(parse_bars(&json!({"a": 1})).is_err());
    }

    #[test]
    fn a_change_is_a_write_that_does_not_only_play() {
        assert!(is_change("set_track_mixer", false));
        assert!(is_change("shape_sound", false));
        assert!(!is_change("get_context", true));
        assert!(!is_change("play_song", false));
        assert!(!is_change("offer_options", false));
        assert!(!is_change("listen", true));
        assert_eq!(
            describe_change(
                "set_track_mixer",
                &json!({"track": "Pad", "volume": -3, "pan": null})
            ),
            "set_track_mixer track=Pad volume=-3"
        );
    }

    #[test]
    fn standing_says_what_a_stale_take_is_missing() {
        let log = vec![
            Change {
                n: 42,
                at: "2026-10-17T14:05:00+02:00".into(),
                at_ms: 0,
                by: "claude".into(),
                what: "feel mode=swing".into(),
            },
            Change {
                n: 43,
                at: "2026-10-17T14:10:00+02:00".into(),
                at_ms: 0,
                by: "claude".into(),
                what: "set_track_mixer track=Pad volume=-3".into(),
            },
        ];
        let s = standing(41, 43, &log);
        assert!(s.starts_with("revision 41 of 43"), "{s}");
        assert!(s.contains("Pad volume=-3"), "{s}");
        assert_eq!(standing(43, 43, &log), "at the current revision (43)");
    }
}
