//! Remembering what Live plays: the Mac app's tap hands this module audio
//! and the Remote Script's clock, and it keeps measurements stamped with
//! bars, sections and the set's revision. **No audio is written.** A bar's
//! samples are held in memory until the bar ends, measured with the same
//! functions a capture is measured with (`audio::measure`, `meter`), and
//! dropped; what reaches the disk is numbers.
//!
//! Three pieces, each pure and testable without Live:
//!
//! - [`ClockTrack`] turns the clock events (one per Live tick, uneven) into
//!   a beat for any instant: tempo between ticks, jumps and loops placed on
//!   the grid line they landed on, a phase-locked correction otherwise.
//! - [`Stamper`] holds the tap's audio until the clock has spoken past it,
//!   so a jump discovered one tick late still stamps the audio after it
//!   correctly. Audio while the transport is stopped is dropped (a browser
//!   preview has no bar).
//! - [`Recorder`] splits stamped audio at bar and 16th lines and keeps a row
//!   per bar, an RMS per 16th, and about thirty spectrum + chroma frames a
//!   second; [`Remembering`] starts and ends takes and writes them through
//!   [`crate::listening`]'s store.

use crate::audio::{self, Audio};
use crate::meter;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

/// Frames of spectrum and chroma kept per second of playback.
pub const FRAMES_PER_SECOND: f64 = 30.0;
/// RMS steps per beat: 16ths in 4/4.
pub const STEPS_PER_BEAT: usize = 4;
/// A clock event further than this from where the playhead was predicted is
/// a jump (or a loop), not jitter. Live's tick wanders by about 10 ms.
const JUMP_TOLERANCE_S: f64 = 0.06;
/// How quickly the phase-locked line pulls a drift back to zero.
const PULL_S: f64 = 0.5;
/// The largest tempo correction the pull may apply, as a fraction.
const PULL_MAX: f64 = 0.05;
/// Audio waits at most this long for the clock before it is stamped by
/// extrapolation (Live stalled, or the clock socket dropped).
const HOLD_MAX_S: f64 = 1.0;
/// A bar longer than this is closed anyway: memory stays bounded at any tempo.
const MAX_BAR_S: f64 = 20.0;
/// A stamped block that does not continue the last one by this many beats
/// starts afresh (a jump the clock reported, or audio the clock never saw).
const CONTINUITY_BEATS: f64 = 0.02;
/// Less of a bar than this is not kept as a bar.
const MIN_BAR_BEATS: f64 = 0.25;
/// No playing audio for this long ends the take.
const TAKE_GAP_S: f64 = 2.0;

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ── the clock ───────────────────────────────────────────────────────────────

/// One clock event from the Remote Script's `clock` channel, stamped with
/// when it was sent on this machine's monotonic clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    pub at_s: f64,
    pub beat: f64,
    pub tempo: f64,
    pub beats_per_bar: f64,
    pub playing: bool,
    #[serde(default)]
    pub scene: Option<i64>,
}

impl ClockEvent {
    /// From the script's event (`{"event": "clock", "beat", "tempo",
    /// "beats_per_bar", "playing", "scene", "t"}`), received at `at_s`.
    pub fn from_live(event: &Value, at_s: f64) -> Option<Self> {
        if event.get("event").and_then(Value::as_str) != Some("clock") {
            return None;
        }
        Some(Self {
            at_s,
            beat: event.get("beat")?.as_f64()?,
            tempo: event.get("tempo").and_then(Value::as_f64).unwrap_or(120.0),
            beats_per_bar: event
                .get("beats_per_bar")
                .and_then(Value::as_f64)
                .unwrap_or(4.0),
            playing: event
                .get("playing")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            scene: event.get("scene").and_then(Value::as_i64),
        })
    }
}

/// Maps the script's own timestamp (`t`, seconds since the script started)
/// onto this machine's clock. The receive time is the send time plus a delay
/// that is never negative, so the smallest difference seen is the best
/// estimate of the offset; it is re-learned slowly in case either clock
/// drifts.
#[derive(Debug, Clone, Default)]
pub struct SendTime {
    offset: Option<f64>,
}

impl SendTime {
    /// When the event with script time `t` was sent, given it arrived at
    /// `received_s`. Without a `t` the receive time is the best there is.
    pub fn sent(&mut self, t: Option<f64>, received_s: f64) -> f64 {
        let Some(t) = t else { return received_s };
        let d = received_s - t;
        let offset = match self.offset {
            // A big step means the script restarted: start over.
            Some(o) if (d - o).abs() > 5.0 => d,
            // Any lower delay wins at once; a higher one creeps in at
            // 1 ms a second's worth of events, so drift is followed.
            Some(o) if d < o => d,
            Some(o) => o + (d - o).min(0.0001),
            None => d,
        };
        self.offset = Some(offset);
        t + offset
    }
}

/// Something the clock showed that the reading should mention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub at_s: f64,
    /// "start", "stop", "jump", "loop" or "tempo"
    pub kind: String,
    pub from_beat: f64,
    pub to_beat: f64,
    pub tempo: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    from_s: f64,
    beat0: f64,
    /// The rate the line runs at: the tempo, plus the phase-locked pull.
    rate_bpm: f64,
    tempo: f64,
    bpb: f64,
    playing: bool,
    scene: Option<i64>,
}

impl Segment {
    fn beat_at(&self, t: f64) -> f64 {
        if self.playing {
            self.beat0 + (t - self.from_s) * self.rate_bpm / 60.0
        } else {
            self.beat0
        }
    }
}

/// Where the playhead was at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub beat: f64,
    /// Beats per second of audio at this instant.
    pub beats_per_s: f64,
    pub tempo: f64,
    pub beats_per_bar: f64,
    pub playing: bool,
    pub scene: Option<i64>,
}

/// The playhead as a function of time, built from clock events.
#[derive(Debug, Clone, Default)]
pub struct ClockTrack {
    segments: Vec<Segment>,
    last: Option<ClockEvent>,
    markers: Vec<Marker>,
    jumps: Vec<(f64, f64)>,
}

impl ClockTrack {
    pub fn new() -> Self {
        Self::default()
    }

    /// The time of the newest event: everything before it is settled.
    pub fn latest_s(&self) -> Option<f64> {
        self.last.as_ref().map(|e| e.at_s)
    }

    pub fn last_event(&self) -> Option<&ClockEvent> {
        self.last.as_ref()
    }

    /// Jumps, loops, tempo changes, starts and stops since the last call.
    pub fn take_markers(&mut self) -> Vec<Marker> {
        std::mem::take(&mut self.markers)
    }

    fn mark(&mut self, at_s: f64, kind: &str, from_beat: f64, to_beat: f64, tempo: f64) {
        self.markers.push(Marker {
            at_s,
            kind: kind.to_string(),
            from_beat: r3(from_beat),
            to_beat: r3(to_beat),
            tempo,
        });
    }

    fn segment_from(ev: &ClockEvent, from_s: f64, beat0: f64, rate_bpm: f64) -> Segment {
        Segment {
            from_s,
            beat0,
            rate_bpm,
            tempo: ev.tempo,
            bpb: ev.beats_per_bar.max(0.25),
            playing: ev.playing,
            scene: ev.scene,
        }
    }

    /// Where a discontinuity landed: Live launches and loops on the grid,
    /// so walk the new line back to the newest bar line — or failing that,
    /// beat — that falls after the previous event. Without one, the middle
    /// of the gap is the least wrong.
    fn landing(ev: &ClockEvent, since_s: f64) -> f64 {
        let bpb = ev.beats_per_bar.max(0.25);
        let bps = ev.tempo.max(1.0) / 60.0;
        for grid in [bpb, 1.0] {
            let target = (ev.beat / grid + 1e-9).floor() * grid;
            let t = ev.at_s - (ev.beat - target) / bps;
            if t >= since_s - 0.002 {
                return t.clamp(since_s, ev.at_s);
            }
        }
        (since_s + ev.at_s) / 2.0
    }

    pub fn push(&mut self, ev: ClockEvent) {
        let Some(prev) = self.last.clone() else {
            // Nothing is known before the first event: the transport counts
            // as stopped, and a first event that is already playing started
            // at most one tick ago (on the grid if it fits).
            self.segments.push(Segment {
                from_s: f64::MIN,
                beat0: ev.beat,
                rate_bpm: ev.tempo,
                tempo: ev.tempo,
                bpb: ev.beats_per_bar.max(0.25),
                playing: false,
                scene: ev.scene,
            });
            if ev.playing {
                let t0 = Self::landing(&ev, ev.at_s - 0.15);
                let beat0 = ev.beat - (ev.at_s - t0) * ev.tempo.max(1.0) / 60.0;
                self.segments
                    .push(Self::segment_from(&ev, t0, beat0, ev.tempo));
                self.mark(t0, "start", beat0, beat0, ev.tempo);
            }
            self.last = Some(ev);
            return;
        };
        if ev.at_s <= prev.at_s {
            return;
        }
        let seg = *self
            .segments
            .last()
            .expect("a segment exists once an event has");
        let bps = ev.tempo.max(1.0) / 60.0;
        match (prev.playing, ev.playing) {
            (false, true) => {
                // Started within the last tick: from where the stopped
                // playhead stood if that fits the tick, else from the grid.
                let window = (ev.at_s - 0.2).max(prev.at_s);
                let from_stopped = ev.at_s - (ev.beat - seg.beat0) / bps;
                let t0 = if (window..=ev.at_s).contains(&from_stopped) {
                    from_stopped
                } else {
                    Self::landing(&ev, window)
                };
                let beat0 = ev.beat - (ev.at_s - t0) * bps;
                self.segments
                    .push(Self::segment_from(&ev, t0, beat0, ev.tempo));
                self.mark(t0, "start", beat0, beat0, ev.tempo);
            }
            (true, false) => {
                // Stopped where the old line reaches the reported beat, if it
                // does inside the gap; else (stop + return to the start
                // marker) in the middle of it.
                let t_line = seg.from_s + (ev.beat - seg.beat0) / (seg.rate_bpm.max(1.0) / 60.0);
                let t_stop = if (prev.at_s..=ev.at_s).contains(&t_line) {
                    t_line
                } else {
                    (prev.at_s + ev.at_s) / 2.0
                };
                let at = seg.beat_at(t_stop);
                self.segments
                    .push(Self::segment_from(&ev, t_stop, ev.beat, ev.tempo));
                self.mark(t_stop, "stop", at, ev.beat, ev.tempo);
            }
            (false, false) => {
                if (ev.beat - seg.beat0).abs() > 1e-6 {
                    self.segments
                        .push(Self::segment_from(&ev, ev.at_s, ev.beat, ev.tempo));
                }
            }
            (true, true) => {
                let predicted = seg.beat_at(ev.at_s);
                let err_s = (ev.beat - predicted) / bps;
                if err_s.abs() > JUMP_TOLERANCE_S {
                    let t_j = Self::landing(&ev, prev.at_s);
                    let from = seg.beat_at(t_j);
                    let to = ev.beat - (ev.at_s - t_j) * bps;
                    // A backward jump that repeats one already seen is a loop.
                    let looped = to < from
                        && self
                            .jumps
                            .iter()
                            .any(|(f, t)| (t - to).abs() < 0.05 && (f - from).abs() < 0.25);
                    self.jumps.push((from, to));
                    if self.jumps.len() > 32 {
                        self.jumps.remove(0);
                    }
                    self.segments
                        .push(Self::segment_from(&ev, t_j, to, ev.tempo));
                    self.mark(
                        t_j,
                        if looped { "loop" } else { "jump" },
                        from,
                        to,
                        ev.tempo,
                    );
                } else {
                    if (ev.tempo - seg.tempo).abs() > 1e-3 {
                        self.mark(ev.at_s, "tempo", predicted, predicted, ev.tempo);
                    }
                    // Phase-locked: the line stays continuous and its rate
                    // pulls the drift back to zero over the next half second.
                    let pull = ((ev.beat - predicted) / PULL_S * 60.0)
                        .clamp(-ev.tempo * PULL_MAX, ev.tempo * PULL_MAX);
                    self.segments.push(Self::segment_from(
                        &ev,
                        ev.at_s,
                        predicted,
                        ev.tempo + pull,
                    ));
                }
            }
        }
        self.last = Some(ev);
        // Keep a minute of history: far more than any audio waits.
        if self.segments.len() > 2048 {
            self.segments.drain(..1024);
        }
    }

    fn segment_at(&self, t: f64) -> Option<&Segment> {
        let i = self.segments.partition_point(|s| s.from_s <= t);
        if i == 0 {
            self.segments.first()
        } else {
            self.segments.get(i - 1)
        }
    }

    /// The playhead at `t`; None before the first event.
    pub fn at(&self, t: f64) -> Option<Position> {
        let s = self.segment_at(t)?;
        Some(Position {
            beat: s.beat_at(t),
            beats_per_s: if s.playing { s.rate_bpm / 60.0 } else { 0.0 },
            tempo: s.tempo,
            beats_per_bar: s.bpb,
            playing: s.playing,
            scene: s.scene,
        })
    }

    /// Segment starts strictly inside `(t0, t1)`: where a block must be cut.
    fn boundaries(&self, t0: f64, t1: f64) -> Vec<f64> {
        self.segments
            .iter()
            .map(|s| s.from_s)
            .filter(|t| *t > t0 && *t < t1)
            .collect()
    }
}

// ── stamping ────────────────────────────────────────────────────────────────

/// A run of audio with its position: sample `i` sits at
/// `beat0 + i × beats_per_sample`.
#[derive(Debug, Clone)]
pub struct Stamped {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub start_s: f64,
    pub beat0: f64,
    pub beats_per_sample: f64,
    pub tempo: f64,
    pub beats_per_bar: f64,
    pub scene: Option<i64>,
}

#[derive(Debug)]
struct Block {
    l: Vec<f32>,
    r: Vec<f32>,
    end_s: f64,
}

/// Holds the tap's audio until the clock has reported past it.
#[derive(Debug)]
pub struct Stamper {
    rate: f64,
    queue: VecDeque<Block>,
}

impl Stamper {
    pub fn new(rate: f64) -> Self {
        Self {
            rate: if rate > 0.0 { rate } else { 48_000.0 },
            queue: VecDeque::new(),
        }
    }

    /// Audio whose last sample left Live at `end_s`.
    pub fn push(&mut self, l: &[f32], r: &[f32], end_s: f64) {
        let n = l.len().min(r.len());
        if n == 0 {
            return;
        }
        self.queue.push_back(Block {
            l: l[..n].to_vec(),
            r: r[..n].to_vec(),
            end_s,
        });
    }

    pub fn pending_s(&self) -> f64 {
        self.queue
            .iter()
            .map(|b| b.l.len() as f64 / self.rate)
            .sum()
    }

    /// Every held block the clock has settled, stamped; blocks while the
    /// transport was stopped are dropped here.
    pub fn release(&mut self, clock: &ClockTrack, now_s: f64) -> Vec<Stamped> {
        let mut out = Vec::new();
        let settled = clock.latest_s().unwrap_or(f64::NEG_INFINITY);
        while let Some(front) = self.queue.front() {
            if front.end_s > settled && now_s - front.end_s < HOLD_MAX_S {
                break;
            }
            let block = self.queue.pop_front().unwrap();
            self.stamp(block, clock, &mut out);
        }
        out
    }

    fn stamp(&self, block: Block, clock: &ClockTrack, out: &mut Vec<Stamped>) {
        let n = block.l.len();
        let start_s = block.end_s - n as f64 / self.rate;
        let mut cuts: Vec<usize> = clock
            .boundaries(start_s, block.end_s)
            .into_iter()
            .map(|t| (((t - start_s) * self.rate).round() as usize).min(n))
            .collect();
        cuts.push(n);
        let mut from = 0usize;
        for to in cuts {
            if to <= from {
                continue;
            }
            let t = start_s + from as f64 / self.rate;
            // The position half a sample in, so a cut on a boundary reads
            // the segment that starts there.
            if let Some(p) = clock.at(t + 0.5 / self.rate) {
                if p.playing {
                    out.push(Stamped {
                        l: block.l[from..to].to_vec(),
                        r: block.r[from..to].to_vec(),
                        start_s: t,
                        beat0: p.beat - 0.5 / self.rate * p.beats_per_s,
                        beats_per_sample: p.beats_per_s / self.rate,
                        tempo: p.tempo,
                        beats_per_bar: p.beats_per_bar,
                        scene: p.scene,
                    });
                }
            }
            from = to;
        }
    }
}

// ── what a take keeps ───────────────────────────────────────────────────────

/// A section of the set as it stood when the take started: a scene row (its
/// name without the " · 8") or a locator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionMark {
    pub name: String,
    /// "scene" or "locator"
    pub kind: String,
    /// The scene's row, for a scene
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<i64>,
    /// The locator's beat, for a locator
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<f64>,
}

/// One bar as it was heard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarRow {
    /// Live's 1-based bar
    pub bar: i64,
    /// Seconds into the take when the bar began
    pub at_s: f64,
    /// How many of the bar's beats were heard; a jump cuts a bar short
    pub beats: f64,
    pub complete: bool,
    #[serde(default)]
    pub section: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<i64>,
    /// The set's revision while the bar played
    pub revision: u64,
    pub tempo: f64,
    pub peak: f64,
    pub true_peak: f64,
    pub rms: f64,
    pub crest: f64,
    pub lufs: f64,
    /// Energy per octave band (31 Hz … 16 kHz), dB per sample — absolute,
    /// so bars combine by energy
    pub octaves: Vec<f64>,
    /// Energy per range (sub … air), dB per sample
    pub ranges: Vec<f64>,
    pub correlation: Option<f64>,
    pub clipped: usize,
    pub silent: bool,
    /// RMS per 16th, dBFS; null for a step that was not heard
    pub steps: Vec<Option<f64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Take {
    pub version: u32,
    pub id: String,
    pub song_key: String,
    #[serde(default)]
    pub set_name: String,
    /// Local wall clock, RFC 3339
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    /// Still being written
    pub open: bool,
    pub sample_rate: f64,
    #[serde(default = "four")]
    pub beats_per_bar: f64,
    pub revision_start: u64,
    pub revision_end: u64,
    /// Whether Session clips were playing at any point: then a bar number is
    /// the song's clock, not a place in the Arrangement
    #[serde(default)]
    pub follows_session: bool,
    #[serde(default)]
    pub sections: Vec<SectionMark>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    pub bars: Vec<BarRow>,
    pub frames: u64,
    pub frames_per_second: f64,
}

fn four() -> f64 {
    4.0
}

impl Take {
    pub fn first_bar(&self) -> Option<i64> {
        self.bars.iter().map(|b| b.bar).min()
    }
    pub fn last_bar(&self) -> Option<i64> {
        self.bars.iter().map(|b| b.bar).max()
    }
}

/// One frame of the spectrum, as the frames file stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRow {
    pub beat: f64,
    /// Short-term loudness, LUFS
    pub lufs: f32,
    /// [`meter::BANDS`] levels, dBFS
    pub bands: Vec<f32>,
    pub chroma: [f32; 12],
}

/// Bytes per stored frame: the beat (f32), loudness, the bands, the chroma.
pub const FRAME_BYTES: usize = 4 + 1 + meter::BANDS + 12;

impl FrameRow {
    pub fn encode(&self) -> [u8; FRAME_BYTES] {
        let mut out = [0u8; FRAME_BYTES];
        out[..4].copy_from_slice(&(self.beat as f32).to_le_bytes());
        out[4] = ((self.lufs as f64 - meter::LUFS_FLOOR) * 3.0)
            .round()
            .clamp(0.0, 255.0) as u8;
        for (i, b) in self.bands.iter().take(meter::BANDS).enumerate() {
            out[5 + i] = ((*b as f64 - meter::FLOOR_DB) * 3.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        for (i, c) in self.chroma.iter().enumerate() {
            out[5 + meter::BANDS + i] = (*c * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        out
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < FRAME_BYTES {
            return None;
        }
        let beat = f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64;
        let lufs = (b[4] as f64 / 3.0 + meter::LUFS_FLOOR) as f32;
        let bands = (0..meter::BANDS)
            .map(|i| (b[5 + i] as f64 / 3.0 + meter::FLOOR_DB) as f32)
            .collect();
        let mut chroma = [0.0f32; 12];
        for (i, c) in chroma.iter_mut().enumerate() {
            *c = b[5 + meter::BANDS + i] as f32 / 255.0;
        }
        Some(Self {
            beat,
            lufs,
            bands,
            chroma,
        })
    }
}

// ── the recorder ────────────────────────────────────────────────────────────

struct BarAcc {
    bar: i64,
    bar_start_beat: f64,
    at_s: f64,
    l: Vec<f32>,
    r: Vec<f32>,
    beats: f64,
    steps: Vec<(f64, usize)>,
    scene: Option<i64>,
    tempo: f64,
}

/// What a bar's section is resolved from.
#[derive(Debug, Clone, Default)]
pub struct SetView {
    pub sections: Vec<SectionMark>,
    /// Session clips are playing: the scene names the section
    pub session: bool,
}

impl SetView {
    /// The section a bar played in: the scene when Session clips play,
    /// otherwise the newest locator at or before the bar's first beat.
    pub fn section_at(&self, beat: f64, scene: Option<i64>) -> String {
        if self.session {
            if let Some(i) = scene {
                if let Some(s) = self
                    .sections
                    .iter()
                    .find(|s| s.kind == "scene" && s.index == Some(i))
                {
                    return s.name.clone();
                }
            }
        }
        self.sections
            .iter()
            .filter(|s| s.kind == "locator" && s.beat.is_some_and(|b| b <= beat + 1e-6))
            .max_by(|a, b| {
                a.beat
                    .partial_cmp(&b.beat)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }
}

pub struct Recorder {
    rate: f64,
    take: Take,
    view: SetView,
    revision: u64,
    bar: Option<BarAcc>,
    expected: Option<f64>,
    first_s: Option<f64>,
    last_s: f64,
    spectrum: meter::Spectrum,
    loudness: meter::Loudness,
    window: Vec<f32>,
    window_pos: usize,
    since_frame: usize,
    frame_every: usize,
    frames: Vec<FrameRow>,
}

impl Recorder {
    pub fn new(take: Take, view: SetView) -> Self {
        let rate = take.sample_rate;
        let revision = take.revision_start;
        Self {
            rate,
            spectrum: meter::Spectrum::new(rate),
            loudness: meter::Loudness::new(rate),
            window: vec![0.0; meter::FFT_SIZE],
            window_pos: 0,
            since_frame: 0,
            frame_every: (rate / FRAMES_PER_SECOND).round().max(1.0) as usize,
            frames: Vec::new(),
            take,
            view,
            revision,
            bar: None,
            expected: None,
            first_s: None,
            last_s: 0.0,
        }
    }

    pub fn take(&self) -> &Take {
        &self.take
    }

    pub fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
        self.take.revision_end = revision;
    }

    pub fn set_view(&mut self, view: SetView) {
        if view.session {
            self.take.follows_session = true;
        }
        if !view.sections.is_empty() {
            self.take.sections = view.sections.clone();
        }
        self.view = view;
    }

    pub fn mark(&mut self, mut m: Marker) {
        if let Some(t0) = self.first_s {
            m.at_s = r3(m.at_s - t0);
        }
        self.take.markers.push(m);
    }

    /// When the newest heard audio was, on the monotonic clock.
    pub fn last_heard_s(&self) -> f64 {
        self.last_s
    }

    pub fn push(&mut self, s: &Stamped) {
        let n = s.l.len().min(s.r.len());
        if n == 0 {
            return;
        }
        let t0 = *self.first_s.get_or_insert(s.start_s);
        self.last_s = s.start_s + n as f64 / self.rate;
        if let Some(want) = self.expected {
            if (s.beat0 - want).abs() > CONTINUITY_BEATS {
                self.close_bar();
                self.loudness.reset();
            }
        }
        self.loudness.push(&s.l[..n], &s.r[..n]);
        let bpb = s.beats_per_bar.max(0.25);
        for i in 0..n {
            let beat = s.beat0 + i as f64 * s.beats_per_sample;
            let bar = (beat / bpb + 1e-9).floor() as i64 + 1;
            let needs_new = match &self.bar {
                Some(b) => b.bar != bar || b.l.len() as f64 >= MAX_BAR_S * self.rate,
                None => true,
            };
            if needs_new {
                self.close_bar();
                let at_s = s.start_s + i as f64 / self.rate - t0;
                self.bar = Some(BarAcc {
                    bar,
                    bar_start_beat: (bar - 1) as f64 * bpb,
                    at_s,
                    l: Vec::with_capacity((self.rate * 2.5) as usize),
                    r: Vec::with_capacity((self.rate * 2.5) as usize),
                    beats: 0.0,
                    steps: vec![(0.0, 0); (bpb * STEPS_PER_BEAT as f64).ceil() as usize],
                    scene: s.scene,
                    tempo: s.tempo,
                });
            }
            let b = self.bar.as_mut().unwrap();
            let (x, y) = (s.l[i], s.r[i]);
            b.l.push(x);
            b.r.push(y);
            b.beats += s.beats_per_sample;
            let step = ((beat - b.bar_start_beat) * STEPS_PER_BEAT as f64)
                .floor()
                .max(0.0) as usize;
            if let Some(st) = b.steps.get_mut(step) {
                st.0 += (x as f64) * (x as f64) + (y as f64) * (y as f64);
                st.1 += 2;
            }
            self.window[self.window_pos] = 0.5 * (x + y);
            self.window_pos = (self.window_pos + 1) % meter::FFT_SIZE;
            self.since_frame += 1;
            if self.since_frame >= self.frame_every {
                self.since_frame = 0;
                let mut ordered = Vec::with_capacity(meter::FFT_SIZE);
                ordered.extend_from_slice(&self.window[self.window_pos..]);
                ordered.extend_from_slice(&self.window[..self.window_pos]);
                let (bands, chroma) = self.spectrum.frame(&ordered);
                self.frames.push(FrameRow {
                    beat,
                    lufs: self.loudness.short_term() as f32,
                    bands,
                    chroma,
                });
                self.take.frames += 1;
            }
        }
        self.expected = Some(s.beat0 + n as f64 * s.beats_per_sample);
    }

    fn close_bar(&mut self) {
        let Some(b) = self.bar.take() else { return };
        // A sliver of a bar — the few milliseconds a start, a stop or a
        // jump leaves on the wrong side of a bar line — says nothing about
        // that bar, and would count it as heard.
        if b.beats < MIN_BAR_BEATS {
            return;
        }
        let seconds = b.l.len() as f64 / self.rate;
        let bpb = b.steps.len() as f64 / STEPS_PER_BEAT as f64;
        let audio = Audio {
            sample_rate: self.rate.round() as u32,
            channels: vec![b.l, b.r],
        };
        let m = audio::measure(&audio, seconds.max(0.01));
        let frames = audio.frames().max(1) as f64;
        let per_sample_db = |e: f64| {
            r1(if e <= 0.0 {
                -120.0
            } else {
                10.0 * (e / frames).log10()
            })
        };
        let octaves = audio::octave_energy(&audio)
            .iter()
            .map(|e| per_sample_db(*e))
            .collect();
        let ranges = meter::range_energy(&audio)
            .iter()
            .map(|e| per_sample_db(*e))
            .collect();
        let steps = b
            .steps
            .iter()
            .map(|(sq, n)| {
                (*n > 0).then(|| {
                    let rms = (sq / *n as f64).sqrt();
                    r1(if rms <= 0.0 {
                        -120.0
                    } else {
                        (20.0 * rms.log10()).max(-120.0)
                    })
                })
            })
            .collect();
        let row = BarRow {
            bar: b.bar,
            at_s: r3(b.at_s),
            beats: r3(b.beats),
            complete: b.beats >= bpb - 1.0 / 32.0,
            section: self.view.section_at(b.bar_start_beat, b.scene),
            scene: if self.view.session { b.scene } else { None },
            revision: self.revision,
            tempo: r3(b.tempo),
            peak: m.peak_dbfs,
            true_peak: r1(meter::true_peak(&audio)),
            rms: m.rms_dbfs,
            crest: m.crest_db,
            lufs: r1(meter::lufs(&audio)),
            octaves,
            ranges,
            correlation: m.stereo_correlation,
            clipped: m.clipped_samples,
            silent: m.rms_dbfs < -60.0,
            steps,
        };
        self.take.bars.push(row);
    }

    /// Frames measured since the last call, for the frames file.
    pub fn drain_frames(&mut self) -> Vec<FrameRow> {
        std::mem::take(&mut self.frames)
    }

    /// Bars closed so far (the open bar is not in it).
    pub fn bars(&self) -> usize {
        self.take.bars.len()
    }

    /// Close the bar in progress and hand back the take.
    pub fn finish(mut self, ended_at: String) -> (Take, Vec<FrameRow>) {
        self.close_bar();
        self.take.open = false;
        self.take.ended_at = Some(ended_at);
        self.take.revision_end = self.revision;
        let frames = std::mem::take(&mut self.frames);
        (self.take, frames)
    }

    /// The take so far, with the bar in progress left out, for a flush.
    pub fn snapshot(&self) -> Take {
        self.take.clone()
    }
}

// ── takes from start to end ─────────────────────────────────────────────────

/// Where takes go and what the set is: the part of [`Remembering`] that
/// touches the world, so tests can stand in for it.
pub trait TakeSink {
    /// The set's identity and sections, read from Live when a take starts.
    fn set_info(&mut self) -> (String, String, SetView);
    /// The set's revision: how many changes its change log holds.
    fn revision(&mut self, song_key: &str) -> u64;
    fn write(&mut self, take: &Take, frames: &[FrameRow]);
    fn now_text(&self) -> String {
        now_rfc3339()
    }
}

/// The app's loop in one object: clock events and audio in, takes out.
pub struct Remembering {
    pub clock: ClockTrack,
    stamper: Stamper,
    recorder: Option<Recorder>,
    rate: f64,
    flushed_bars: usize,
    session: bool,
}

impl Remembering {
    pub fn new(rate: f64) -> Self {
        Self {
            clock: ClockTrack::new(),
            stamper: Stamper::new(rate),
            recorder: None,
            rate,
            flushed_bars: 0,
            session: false,
        }
    }

    pub fn clock_event(&mut self, ev: ClockEvent) {
        self.clock.push(ev);
    }

    /// Whether Session clips are playing, from the `changes` channel.
    pub fn set_session(&mut self, session: bool) {
        self.session = session;
        if let Some(r) = self.recorder.as_mut() {
            let mut view = r.view.clone();
            view.session = session;
            r.set_view(view);
        }
    }

    pub fn audio(&mut self, l: &[f32], r: &[f32], end_s: f64) {
        self.stamper.push(l, r, end_s);
    }

    pub fn recording(&self) -> bool {
        self.recorder.is_some()
    }

    pub fn current(&self) -> Option<&Take> {
        self.recorder.as_ref().map(|r| r.take())
    }

    /// Release settled audio into the take, start one on the first playing
    /// audio, end it once the transport stops, and flush every few bars.
    pub fn tick(&mut self, now_s: f64, sink: &mut dyn TakeSink) {
        let stamped = self.stamper.release(&self.clock, now_s);
        let markers = self.clock.take_markers();
        for s in &stamped {
            if self.recorder.is_none() {
                let (song_key, set_name, mut view) = sink.set_info();
                view.session = self.session;
                let revision = sink.revision(&song_key);
                let take = Take {
                    version: 1,
                    id: take_id(),
                    song_key,
                    set_name,
                    started_at: sink.now_text(),
                    ended_at: None,
                    open: true,
                    sample_rate: self.rate,
                    beats_per_bar: s.beats_per_bar,
                    revision_start: revision,
                    revision_end: revision,
                    follows_session: view.session,
                    sections: view.sections.clone(),
                    markers: Vec::new(),
                    bars: Vec::new(),
                    frames: 0,
                    frames_per_second: FRAMES_PER_SECOND,
                };
                self.recorder = Some(Recorder::new(take, view));
                self.flushed_bars = 0;
            }
            let rec = self.recorder.as_mut().unwrap();
            rec.push(s);
        }
        if let Some(rec) = self.recorder.as_mut() {
            for m in markers {
                if m.kind != "start" {
                    rec.mark(m);
                }
            }
            let stopped = self
                .clock
                .last_event()
                .is_some_and(|e| !e.playing && self.stamper.queue.is_empty());
            let silent_for = now_s - rec.last_heard_s();
            if stopped || silent_for > TAKE_GAP_S {
                let rec = self.recorder.take().unwrap();
                let key = rec.take().song_key.clone();
                let revision = sink.revision(&key);
                let mut rec = rec;
                rec.set_revision(revision);
                let (take, frames) = rec.finish(sink.now_text());
                if !take.bars.is_empty() {
                    sink.write(&take, &frames);
                }
            } else if rec.bars() >= self.flushed_bars + 4 {
                let key = rec.take().song_key.clone();
                let revision = sink.revision(&key);
                rec.set_revision(revision);
                let frames = rec.drain_frames();
                sink.write(&rec.snapshot(), &frames);
                self.flushed_bars = rec.bars();
            }
        }
    }

    /// End the take now (the app is quitting, or Remember was turned off).
    pub fn finish(&mut self, sink: &mut dyn TakeSink) {
        if let Some(mut rec) = self.recorder.take() {
            let key = rec.take().song_key.clone();
            rec.set_revision(sink.revision(&key));
            let (take, frames) = rec.finish(sink.now_text());
            if !take.bars.is_empty() {
                sink.write(&take, &frames);
            }
        }
    }
}

/// Now, on the local wall clock, to the second.
pub fn now_rfc3339() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

// ── what the app's clock socket says ───────────────────────────────────────

/// Something on the app's Live socket that is not a beat.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    /// The transport started or stopped (from the `changes` channel)
    Started,
    Stopped,
    /// An edit made in Live: a tempo, a name, a mute, a solo, a track or a
    /// scene added or removed
    Edit(String),
    /// Session clips started (true) or all stopped (false)
    Session(bool),
}

/// Turns the raw events of a socket subscribed to `clock` and `changes` into
/// clock events for [`Remembering`] and the things the app acts on.
#[derive(Debug, Default)]
pub struct Feed {
    send_time: SendTime,
    playing_slots: std::collections::BTreeMap<String, i64>,
    session: bool,
}

impl Feed {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed what Session clips play, from a state read.
    pub fn seed_session(&mut self, slots: &[(String, i64)]) {
        self.playing_slots = slots.iter().cloned().collect();
        self.session = self.playing_slots.values().any(|s| *s >= 0);
    }

    pub fn session(&self) -> bool {
        self.session
    }

    pub fn event(&mut self, ev: &Value, received_s: f64, rem: &mut Remembering) -> Vec<Seen> {
        let mut out = Vec::new();
        match ev.get("event").and_then(Value::as_str) {
            Some("clock") => {
                let t = ev.get("t").and_then(Value::as_f64);
                let at = self.send_time.sent(t, received_s);
                if let Some(c) = ClockEvent::from_live(ev, at) {
                    rem.clock_event(c);
                }
            }
            Some("changes") => {
                let changed = ev
                    .get("changed")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut edits = Vec::new();
                for c in changed {
                    let path = c.get("path").and_then(Value::as_str).unwrap_or("");
                    let to = c.get("to").cloned().unwrap_or(Value::Null);
                    if path == "song.is_playing" {
                        out.push(if to.as_bool() == Some(true) {
                            Seen::Started
                        } else {
                            Seen::Stopped
                        });
                    } else if path.ends_with(".playing_slot_index") {
                        self.playing_slots
                            .insert(path.to_string(), to.as_i64().unwrap_or(-1));
                    } else if path == "song.tempo"
                        || path == "song.signature_numerator"
                        || path.ends_with(".count")
                        || path.ends_with(".name")
                        || path.ends_with(".mute")
                        || path.ends_with(".solo")
                    {
                        edits.push(format!(
                            "{path} {}",
                            match &to {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            }
                        ));
                    }
                }
                let session = self.playing_slots.values().any(|s| *s >= 0);
                if session != self.session {
                    self.session = session;
                    rem.set_session(session);
                    out.push(Seen::Session(session));
                }
                if !edits.is_empty() {
                    let n = edits.len();
                    let mut what = edits.into_iter().take(3).collect::<Vec<_>>().join(", ");
                    if n > 3 {
                        what.push_str(&format!(" (+{} more)", n - 3));
                    }
                    out.push(Seen::Edit(format!("in Live: {what}")));
                }
            }
            _ => {}
        }
        out
    }
}

/// A take's id: when it started, to the millisecond, so ids sort by time.
pub fn take_id() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

// ── a Live that is not Live, for the tests and the probe ────────────────────

/// A playhead that follows a script of transport moves exactly, and the
/// clock events Live would send for it: one per tick, about 100 ms apart
/// with 10 ms of jitter, each reading the beat at the moment of the tick.
/// What the recorder's stamping is checked against.
#[doc(hidden)]
pub mod sim {
    use super::ClockEvent;

    #[derive(Debug, Clone, Copy)]
    pub enum Move {
        /// Start playing from a beat
        Play { at_s: f64, from_beat: f64 },
        /// Jump to a beat (a launch or a locator)
        Jump { at_s: f64, to_beat: f64 },
        /// Change tempo
        Tempo { at_s: f64, bpm: f64 },
        /// Stop the transport
        Stop { at_s: f64 },
        /// Loop between two beats from now on
        Loop { at_s: f64, start: f64, end: f64 },
    }

    impl Move {
        fn at(&self) -> f64 {
            match *self {
                Move::Play { at_s, .. }
                | Move::Jump { at_s, .. }
                | Move::Tempo { at_s, .. }
                | Move::Stop { at_s }
                | Move::Loop { at_s, .. } => at_s,
            }
        }
    }

    /// The true playhead, integrated in 0.1 ms steps.
    pub struct Playhead {
        pub bpm: f64,
        pub bpb: f64,
        moves: Vec<Move>,
        cache: Vec<(f64, f64, bool, f64)>,
    }

    pub const STEP_S: f64 = 0.0001;

    impl Playhead {
        pub fn new(bpm: f64, bpb: f64, mut moves: Vec<Move>, until_s: f64) -> Self {
            moves.sort_by(|a, b| a.at().partial_cmp(&b.at()).unwrap());
            let mut ph = Self {
                bpm,
                bpb,
                moves,
                cache: Vec::new(),
            };
            ph.run(until_s);
            ph
        }

        fn run(&mut self, until_s: f64) {
            let (mut t, mut beat, mut playing, mut bpm) = (0.0f64, 0.0f64, false, self.bpm);
            let mut looping: Option<(f64, f64)> = None;
            let mut next = 0usize;
            while t <= until_s + STEP_S {
                while next < self.moves.len() && self.moves[next].at() <= t + 1e-12 {
                    match self.moves[next] {
                        Move::Play { from_beat, .. } => {
                            playing = true;
                            beat = from_beat;
                        }
                        Move::Jump { to_beat, .. } => beat = to_beat,
                        Move::Tempo { bpm: b, .. } => bpm = b,
                        Move::Stop { .. } => playing = false,
                        Move::Loop { start, end, .. } => looping = Some((start, end)),
                    }
                    next += 1;
                }
                self.cache.push((t, beat, playing, bpm));
                if playing {
                    beat += STEP_S * bpm / 60.0;
                    if let Some((start, end)) = looping {
                        if beat >= end {
                            beat = start + (beat - end);
                        }
                    }
                }
                t += STEP_S;
            }
        }

        /// (beat, playing, bpm) at `t`.
        pub fn at(&self, t: f64) -> (f64, bool, f64) {
            let i = ((t / STEP_S).round() as usize).min(self.cache.len() - 1);
            let (_, b, p, bpm) = self.cache[i];
            (b, p, bpm)
        }

        /// The events Live sends: a tick every `tick_s` ± `jitter_s`, from a
        /// fixed pseudo-random sequence so a failure reproduces.
        pub fn events(&self, until_s: f64, tick_s: f64, jitter_s: f64) -> Vec<ClockEvent> {
            let mut out = Vec::new();
            let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
            let mut rand = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed % 10_000) as f64 / 10_000.0
            };
            let mut t = tick_s;
            let mut sent_stopped = false;
            while t < until_s {
                let (beat, playing, bpm) = self.at(t);
                if playing || !sent_stopped {
                    out.push(ClockEvent {
                        at_s: t,
                        beat,
                        tempo: bpm,
                        beats_per_bar: self.bpb,
                        playing,
                        scene: None,
                    });
                }
                sent_stopped = !playing;
                t += tick_s + (rand() * 2.0 - 1.0) * jitter_s;
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sim::{Move, Playhead};
    use super::*;

    const RATE: f64 = 48_000.0;

    /// Feed the simulated clock and blocks of silence-with-a-pulse through
    /// the stamper, and return the worst stamping error in milliseconds for
    /// every sample that was stamped.
    fn worst_error_ms(ph: &Playhead, until_s: f64, block: usize) -> (f64, usize) {
        let events = ph.events(until_s, 0.1, 0.012);
        let mut clock = ClockTrack::new();
        let mut stamper = Stamper::new(RATE);
        let mut next_event = 0usize;
        let mut t = 0.0f64;
        let mut worst = 0.0f64;
        let mut stamped_samples = 0usize;
        let zeros = vec![0.0f32; block];
        while t < until_s {
            t += block as f64 / RATE;
            while next_event < events.len() && events[next_event].at_s <= t {
                clock.push(events[next_event].clone());
                next_event += 1;
            }
            stamper.push(&zeros, &zeros, t);
            for s in stamper.release(&clock, t) {
                for i in (0..s.l.len()).step_by(64) {
                    let at = s.start_s + i as f64 / RATE;
                    let (truth, playing, bpm) = ph.at(at);
                    if !playing {
                        continue;
                    }
                    let got = s.beat0 + i as f64 * s.beats_per_sample;
                    let err_ms = (got - truth).abs() * 60.0 / bpm * 1000.0;
                    worst = worst.max(err_ms);
                    stamped_samples += 1;
                }
            }
        }
        (worst, stamped_samples)
    }

    #[test]
    fn steady_playback_stamps_within_a_few_milliseconds() {
        let ph = Playhead::new(
            120.0,
            4.0,
            vec![Move::Play {
                at_s: 0.3,
                from_beat: 0.0,
            }],
            20.0,
        );
        let (worst, n) = worst_error_ms(&ph, 20.0, 512);
        assert!(n > 1000);
        // Only the first tick's start is uncertain; after it, jitter only.
        assert!(worst < 50.0, "worst {worst:.1} ms");
    }

    /// The spec's bar: ±50 ms of Live's own position after a quantized
    /// jump, an arrangement loop and a tempo change.
    #[test]
    fn jumps_loops_and_tempo_changes_stay_within_fifty_ms() {
        let moves = vec![
            Move::Play {
                at_s: 0.25,
                from_beat: 0.0,
            },
            // A launch quantized to the bar: at bar 5's downbeat, to bar 17.
            Move::Jump {
                at_s: 0.25 + 8.0,
                to_beat: 64.0,
            },
            Move::Tempo {
                at_s: 12.37,
                bpm: 128.0,
            },
            // A two-bar loop from bar 21, entered on its downbeat.
            Move::Loop {
                at_s: 13.0,
                start: 80.0,
                end: 88.0,
            },
        ];
        let ph = Playhead::new(120.0, 4.0, moves, 30.0);
        let (worst, n) = worst_error_ms(&ph, 30.0, 471);
        assert!(n > 1000);
        assert!(worst <= 50.0, "worst stamping error {worst:.1} ms");
    }

    #[test]
    fn audio_while_stopped_is_dropped() {
        let ph = Playhead::new(
            120.0,
            4.0,
            vec![
                Move::Play {
                    at_s: 1.0,
                    from_beat: 0.0,
                },
                Move::Stop { at_s: 3.0 },
            ],
            6.0,
        );
        let events = ph.events(6.0, 0.1, 0.0);
        let mut clock = ClockTrack::new();
        for e in events {
            clock.push(e);
        }
        let mut stamper = Stamper::new(RATE);
        let block = vec![0.1f32; 4800];
        let mut t = 0.0;
        let mut kept = 0.0;
        while t < 5.9 {
            t += 0.1;
            stamper.push(&block, &block, t);
        }
        for s in stamper.release(&clock, 10.0) {
            kept += s.l.len() as f64 / RATE;
        }
        // Two seconds played; a tick of uncertainty at each end.
        assert!((kept - 2.0).abs() < 0.15, "kept {kept:.3} s");
    }

    #[test]
    fn the_clock_names_what_happened() {
        let ph = Playhead::new(
            120.0,
            4.0,
            vec![
                Move::Play {
                    at_s: 0.2,
                    from_beat: 0.0,
                },
                Move::Jump {
                    at_s: 4.2,
                    to_beat: 32.0,
                },
                Move::Tempo {
                    at_s: 6.0,
                    bpm: 100.0,
                },
                Move::Loop {
                    at_s: 7.0,
                    start: 40.0,
                    end: 44.0,
                },
                Move::Stop { at_s: 15.0 },
            ],
            16.0,
        );
        let mut clock = ClockTrack::new();
        for e in ph.events(16.0, 0.1, 0.008) {
            clock.push(e);
        }
        let kinds: Vec<String> = clock.take_markers().into_iter().map(|m| m.kind).collect();
        assert_eq!(
            kinds.first().map(String::as_str),
            Some("start"),
            "{kinds:?}"
        );
        assert!(kinds.contains(&"jump".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"tempo".to_string()), "{kinds:?}");
        // The first wrap is a jump back like any other; once it repeats, it is
        // named a loop.
        assert!(
            kinds.iter().filter(|k| *k == "loop").count() >= 1,
            "a jump back that repeats is called a loop: {kinds:?}"
        );
        assert_eq!(kinds.last().map(String::as_str), Some("stop"), "{kinds:?}");
    }

    #[test]
    fn the_send_time_ignores_receive_delay() {
        let mut st = SendTime::default();
        // Sent every 0.1 s on the script's clock; received 2–9 ms later.
        let mut worst: f64 = 0.0;
        for i in 0..50 {
            let t = i as f64 * 0.1;
            let delay = 0.002 + ((i * 7) % 8) as f64 * 0.001;
            let sent = st.sent(Some(t), 1000.0 + t + delay);
            if i > 5 {
                worst = worst.max((sent - (1000.0 + t + 0.002)).abs());
            }
        }
        assert!(worst < 0.0015, "{worst}");
    }

    #[test]
    fn frames_round_trip_through_the_file_format() {
        let f = FrameRow {
            beat: 129.25,
            lufs: -14.3,
            bands: (0..meter::BANDS).map(|i| -80.0 + i as f32).collect(),
            chroma: [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 0.25],
        };
        let back = FrameRow::decode(&f.encode()).unwrap();
        assert!((back.beat - f.beat).abs() < 1e-3);
        assert!((back.lufs - f.lufs).abs() < 0.2);
        for (a, b) in back.bands.iter().zip(&f.bands) {
            assert!((a - b).abs() < 0.2, "{a} vs {b}");
        }
        for (a, b) in back.chroma.iter().zip(&f.chroma) {
            assert!((a - b).abs() < 0.005);
        }
        // Thirty a second is the spec's "about 3 KB a second".
        assert!(FRAME_BYTES as f64 * FRAMES_PER_SECOND < 3000.0);
    }

    #[test]
    fn a_locator_names_the_bars_after_it_and_a_scene_names_session_bars() {
        let view = SetView {
            sections: vec![
                SectionMark {
                    name: "Intro".into(),
                    kind: "locator".into(),
                    index: None,
                    beat: Some(0.0),
                },
                SectionMark {
                    name: "Drop".into(),
                    kind: "locator".into(),
                    index: None,
                    beat: Some(64.0),
                },
                SectionMark {
                    name: "Break".into(),
                    kind: "scene".into(),
                    index: Some(2),
                    beat: None,
                },
            ],
            session: false,
        };
        assert_eq!(view.section_at(0.0, None), "Intro");
        assert_eq!(view.section_at(63.9, None), "Intro");
        assert_eq!(view.section_at(64.0, Some(2)), "Drop");
        let session = SetView {
            session: true,
            ..view
        };
        assert_eq!(session.section_at(64.0, Some(2)), "Break");
    }
}
