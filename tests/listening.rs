//! Claude listens: the listening memory end to end, with no Live.
//!
//! A fixture render — sixteen bars in four sections, generated here so the
//! repo carries no audio — is played through the same path the Mac app
//! uses (`recorder::Remembering`, fed by a simulated Live clock with Live's
//! tick jitter) and through the capture path (`audio::read_file` +
//! `audio::measure` on the WAV). The spec's requirements are the tests:
//!
//! - 2: every bar is stamped with its bar, section and revision (and the
//!   recorder's own tests hold stamping to ±50 ms across a jump, a loop
//!   and a tempo change);
//! - 3: heard, listen and compare answer from the memory alone in under
//!   300 ms for a ten-minute song;
//! - 4: a stale take is never presented as current;
//! - 5: per-bar peak, RMS and bands match capture_mix's within 0.5 dB.
//!
//! Plus: no audio reaches the disk, the get_context line, the tools through
//! the server (with the FakeBridge), listen_picture's PNG, and Part D's
//! away switch.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::audio;
use mcp_ableton_music_maker::feedback;
use mcp_ableton_music_maker::listening::{self, Range};
use mcp_ableton_music_maker::meter;
use mcp_ableton_music_maker::recorder::sim::{Move, Playhead};
use mcp_ableton_music_maker::recorder::{
    FrameRow, Remembering, SectionMark, SetView, Take, TakeSink, FRAME_BYTES,
};
use mcp_ableton_music_maker::tools;
use serde_json::{json, Value};
use std::time::Instant;

const RATE: u32 = 48_000;
const BPM: f64 = 120.0;
const BAR_S: f64 = 2.0;

/// app.json and away.json are one file each for the whole state dir, so the
/// tests that write them take turns.
static GLOBAL_FILES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn state() {
    common::isolate_state_dir();
}

fn key(name: &str) -> String {
    mcp_ableton_music_maker::memory::key_for(&format!("/Users/producer/Music/{name}.als"))
}

fn set_path(name: &str) -> String {
    format!("/Users/producer/Music/{name}.als")
}

fn remembering(on: bool) {
    listening::write_app_beat(&listening::AppBeat {
        pid: std::process::id(),
        remembering: on,
        available: true,
        updated_ms: chrono::Local::now().timestamp_millis(),
        live: "playing".into(),
        retention_days: None,
    });
}

fn app_gone() {
    let _ = std::fs::remove_file(listening::app_beat_path());
}

// ── the fixture render ──────────────────────────────────────────────────────

fn sections_view() -> SetView {
    let loc = |name: &str, beat: f64| SectionMark {
        name: name.into(),
        kind: "locator".into(),
        index: None,
        beat: Some(beat),
    };
    SetView {
        sections: vec![
            loc("Intro", 0.0),
            loc("Build", 16.0),
            loc("Drop", 32.0),
            loc("Outro", 48.0),
        ],
        session: false,
    }
}

/// Sixteen bars at 120 BPM: an A minor pad throughout (ducked on the beat
/// in the Drop, fading in the Outro), hats from the Build, kick and bass in
/// the Drop. `pad_db` moves the pad, as a mixer change would.
fn render(pad_db: f64) -> (Vec<f32>, Vec<f32>) {
    let n = (16.0 * BAR_S * RATE as f64) as usize;
    let pad_gain = 10f64.powf(pad_db / 20.0);
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    let mut seed: u32 = 12345;
    let two_pi = 2.0 * std::f64::consts::PI;
    for i in 0..n {
        let t = i as f64 / RATE as f64;
        let beat = t * BPM / 60.0;
        let bar = (beat / 4.0).floor() as usize;
        let in_beat = beat.fract() * 60.0 / BPM;
        let in_8th = (beat * 2.0).fract() * 30.0 / BPM;
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 9) as f64 / (1u32 << 23) as f64 * 2.0 - 1.0;
        let mut pad = 0.06
            * pad_gain
            * ((two_pi * 220.0 * t).sin()
                + (two_pi * 261.63 * t).sin()
                + (two_pi * 329.63 * t).sin());
        let (mut kick, mut bass, mut hats) = (0.0, 0.0, 0.0);
        if (4..12).contains(&bar) {
            hats = 0.08 * noise * (-in_8th / 0.015).exp();
        }
        if (8..12).contains(&bar) {
            pad *= 0.25 + 0.75 * (in_beat / 0.25).min(1.0);
            kick = 0.7 * (-in_beat / 0.12).exp() * (two_pi * 55.0 * t).sin();
            bass = 0.18 * (two_pi * 110.0 * t).sin();
        }
        if bar >= 12 {
            pad *= 1.0 - (t - 24.0) / 8.0;
        }
        let mono = pad + kick + bass;
        l.push((mono + hats) as f32);
        r.push((mono * 0.95 - hats * 0.6) as f32);
    }
    (l, r)
}

struct DiskSink {
    key: String,
}

impl TakeSink for DiskSink {
    fn set_info(&mut self) -> (String, String, SetView) {
        (self.key.clone(), "Fixture".into(), sections_view())
    }
    fn revision(&mut self, song_key: &str) -> u64 {
        listening::revision(song_key)
    }
    fn write(&mut self, take: &Take, frames: &[FrameRow]) {
        listening::write_take(take, frames).expect("the take is written");
    }
}

/// Play `audio` (starting at beat `from_beat`) through the app's path: a
/// simulated Live starts at 0.5 s, its clock ticks with Live's jitter, and
/// the tap delivers blocks of uneven size. Returns when the take is written.
fn play(song_key: &str, l: &[f32], r: &[f32], from_beat: f64) {
    let t0 = 0.5;
    let seconds = l.len() as f64 / RATE as f64;
    let end = t0 + seconds;
    let ph = Playhead::new(
        BPM,
        4.0,
        vec![
            Move::Play {
                at_s: t0,
                from_beat,
            },
            Move::Stop { at_s: end },
        ],
        end + 3.0,
    );
    let events = ph.events(end + 3.0, 0.1, 0.012);
    let mut rem = Remembering::new(RATE as f64);
    let mut sink = DiskSink {
        key: song_key.into(),
    };
    let mut next = 0usize;
    let mut g = 0usize;
    let total = ((end + 3.0) * RATE as f64) as usize;
    let mut k = 0usize;
    while g < total {
        let block = 400 + (k * 37) % 223;
        k += 1;
        let mut bl = vec![0.0f32; block];
        let mut br = vec![0.0f32; block];
        for j in 0..block {
            let at = (g + j) as f64 / RATE as f64 - t0;
            if at >= 0.0 {
                let idx = (at * RATE as f64).round() as usize;
                if idx < l.len() {
                    bl[j] = l[idx];
                    br[j] = r[idx];
                }
            }
        }
        g += block;
        let now = g as f64 / RATE as f64;
        while next < events.len() && events[next].at_s <= now {
            rem.clock_event(events[next].clone());
            next += 1;
        }
        rem.audio(&bl, &br, now);
        rem.tick(now, &mut sink);
    }
    rem.finish(&mut sink);
}

fn capture_of(l: &[f32], r: &[f32]) -> audio::Audio {
    // Through the capture path for real: a WAV Live would have recorded,
    // read back by the same reader capture_mix uses.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capture.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for (a, b) in l.iter().zip(r) {
        w.write_sample(*a).unwrap();
        w.write_sample(*b).unwrap();
    }
    w.finalize().unwrap();
    audio::read_file(&path).unwrap()
}

fn slice(a: &audio::Audio, bar: usize) -> audio::Audio {
    let n = (BAR_S * a.sample_rate as f64) as usize;
    audio::Audio {
        sample_rate: a.sample_rate,
        channels: a
            .channels
            .iter()
            .map(|c| c[bar * n..((bar + 1) * n).min(c.len())].to_vec())
            .collect(),
    }
}

fn db(share: f64) -> f64 {
    10.0 * share.max(1e-12).log10()
}

// ── requirement 5: the same numbers as capture_mix ──────────────────────────

#[test]
fn per_bar_values_match_capture_mix_within_half_a_db() {
    state();
    let k = key("Fixture Match");
    let (l, r) = render(0.0);
    play(&k, &l, &r, 0.0);
    let takes = listening::takes(&k);
    assert_eq!(takes.len(), 1, "one take");
    let take = &takes[0];
    assert!(!take.open);
    let bars: Vec<i64> = take
        .bars
        .iter()
        .filter(|b| b.complete)
        .map(|b| b.bar)
        .collect();
    assert_eq!(
        bars,
        (1..=16).collect::<Vec<_>>(),
        "every bar stamped once: {:?}",
        take.bars
            .iter()
            .map(|b| (b.bar, b.beats))
            .collect::<Vec<_>>()
    );

    let cap = capture_of(&l, &r);
    let whole = audio::measure(&cap, BAR_S);
    assert_eq!(whole.rms_per_bar.len(), 16);
    for (i, row) in take.bars.iter().filter(|b| b.complete).enumerate() {
        // RMS per bar: what capture_mix reports for the take.
        assert!(
            (row.rms - whole.rms_per_bar[i]).abs() <= 0.5,
            "bar {}: RMS {} heard, {} captured",
            row.bar,
            row.rms,
            whole.rms_per_bar[i]
        );
        // Peak and bands: what capture_mix reports for that bar alone.
        let one = audio::measure(&slice(&cap, i), BAR_S);
        assert!(
            (row.peak - one.peak_dbfs).abs() <= 0.5,
            "bar {}: peak {} heard, {} captured",
            row.bar,
            row.peak,
            one.peak_dbfs
        );
        let tot: f64 = row.octaves.iter().map(|d| 10f64.powf(d / 10.0)).sum();
        for (b, cap_share) in one.octaves.iter().enumerate() {
            if *cap_share < 0.02 {
                continue;
            }
            let heard = 10f64.powf(row.octaves[b] / 10.0) / tot;
            assert!(
                (db(heard) - db(*cap_share)).abs() <= 0.5,
                "bar {} octave {}: {:.2} dB heard, {:.2} dB captured",
                row.bar,
                audio::octave_labels()[b],
                db(heard),
                db(*cap_share)
            );
        }
    }
    // The whole take combined, against the whole capture.
    let rows: Vec<_> = take.bars.iter().collect();
    let s = listening::summarize(&rows);
    assert!(
        (s.peak - whole.peak_dbfs).abs() <= 0.5,
        "peak {} vs {}",
        s.peak,
        whole.peak_dbfs
    );
    assert!(
        (s.rms - whole.rms_dbfs).abs() <= 0.5,
        "RMS {} vs {}",
        s.rms,
        whole.rms_dbfs
    );
    for (b, cap_share) in whole.octaves.iter().enumerate() {
        if *cap_share < 0.02 {
            continue;
        }
        assert!(
            (db(s.octave_shares[b]) - db(*cap_share)).abs() <= 0.5,
            "octave {}: {:.2} vs {:.2} dB",
            audio::octave_labels()[b],
            db(s.octave_shares[b]),
            db(*cap_share)
        );
    }
    let lufs = meter::lufs(&cap);
    assert!(
        (s.lufs - lufs).abs() <= 0.5,
        "loudness {} vs {}",
        s.lufs,
        lufs
    );
}

// ── requirement 2: bar, section, revision on every bar ──────────────────────

#[test]
fn every_bar_carries_its_section_and_revision_and_nothing_is_audio() {
    state();
    let k = key("Fixture Stamps");
    listening::record_change(&k, "claude", "build_song");
    listening::record_change(&k, "claude", "set_tempo tempo=120");
    let (l, r) = render(0.0);
    play(&k, &l, &r, 0.0);
    let take = listening::takes(&k).pop().unwrap();
    for b in &take.bars {
        let want = match b.bar {
            1..=4 => "Intro",
            5..=8 => "Build",
            9..=12 => "Drop",
            _ => "Outro",
        };
        assert_eq!(b.section, want, "bar {}", b.bar);
        assert_eq!(b.revision, 2, "bar {}", b.bar);
        assert_eq!(b.steps.len(), 16, "sixteen 16ths in a 4/4 bar");
    }
    assert_eq!(take.revision_start, 2);
    assert_eq!(take.set_name, "Fixture");

    // What reached the disk: numbers. No WAV or AIFF, and about 3 KB a second.
    let dir = listening::song_dir(&k).join("takes");
    let mut bytes = 0u64;
    for f in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        assert!(
            name.ends_with(".json") || name.ends_with(".frames"),
            "{name}"
        );
        let body = std::fs::read(f.path()).unwrap();
        assert!(
            !body.starts_with(b"RIFF") && !body.starts_with(b"FORM"),
            "{name} is audio"
        );
        if name.ends_with(".frames") {
            assert_eq!(body.len() as u64, take.frames * FRAME_BYTES as u64);
        }
        bytes += body.len() as u64;
    }
    let per_second = bytes as f64 / 32.0;
    assert!(per_second < 3_500.0, "{per_second:.0} bytes a second");
    // Thirty frames a second of playback.
    assert!(
        (take.frames as f64 / 32.0 - 30.0).abs() < 1.0,
        "{} frames",
        take.frames
    );
}

// ── requirement 4: a stale take is never current ────────────────────────────

#[test]
fn readings_name_their_take_and_never_pass_a_stale_one_off_as_current() {
    let _g = GLOBAL_FILES.blocking_lock();
    state();
    remembering(true);
    let k = key("Fixture Stale");
    let (l, r) = render(0.0);
    play(&k, &l, &r, 0.0);

    let now = listening::listen_text(&k, &Range::Section("Drop".into()), false, 0, None);
    assert!(now.contains("measured from playback at"), "{now}");
    assert!(now.contains("at the current revision (0)"), "{now}");
    assert!(now.contains("Drop (bars 9–12"), "{now}");
    assert!(
        now.contains("ducks"),
        "the Drop's pad ducks on the beat: {now}"
    );
    let intro = listening::listen_text(&k, &Range::Section("intro".into()), false, 0, None);
    assert!(intro.contains("heard as Am (chroma)"), "{intro}");

    let heard = listening::heard_text(&k, None);
    assert!(
        heard.contains("Heard at the current revision: Intro"),
        "{heard}"
    );
    assert!(heard.contains("Never heard: none"), "{heard}");

    // A change: everything heard is now from before it.
    listening::record_change(&k, "claude", "set_track_mixer track=Pad volume=-3");
    let stale = listening::listen_text(&k, &Range::Section("Drop".into()), false, 0, None);
    assert!(stale.contains("revision 0 of 1"), "{stale}");
    assert!(
        stale.contains("not in this take: set_track_mixer track=Pad volume=-3"),
        "{stale}"
    );
    assert!(!stale.contains("current revision"), "{stale}");
    let current_only = listening::listen_text(&k, &Range::Section("Drop".into()), true, 0, None);
    assert!(
        current_only.contains("has not been heard since the last change"),
        "{current_only}"
    );
    let heard = listening::heard_text(&k, None);
    assert!(
        heard.contains("Heard at the current revision: none"),
        "{heard}"
    );
    assert!(
        heard.contains("Heard before the last change: Intro"),
        "{heard}"
    );
    assert!(
        heard.contains("since then: set_track_mixer track=Pad volume=-3"),
        "{heard}"
    );
    let line = listening::context_line(&k).unwrap();
    assert!(
        line.starts_with(
            "🎧 heard: 0 of 4 sections current; Intro not heard since set_track_mixer"
        ),
        "{line}"
    );

    // The producer plays the Drop again, pad 3 dB down: it is current, and
    // compare reads the change.
    let (l2, r2) = render(-3.0);
    let drop_start = (8.0 * BAR_S * RATE as f64) as usize;
    let drop_end = (12.0 * BAR_S * RATE as f64) as usize;
    play(
        &k,
        &l2[drop_start..drop_end],
        &r2[drop_start..drop_end],
        32.0,
    );
    let line = listening::context_line(&k).unwrap();
    assert!(
        line.starts_with("🎧 heard: 1 of 4 sections current"),
        "{line}"
    );
    let cmp = listening::compare_text(&k, &Range::Section("Drop".into()), None, None, None, None)
        .unwrap();
    assert!(cmp.starts_with("Drop, revision 0 against 1:"), "{cmp}");
    assert!(
        cmp.contains("Changes between them: set_track_mixer track=Pad volume=-3"),
        "{cmp}"
    );
    assert!(
        cmp.contains("loudness −"),
        "the pad came down, so the drop is quieter: {cmp}"
    );
    // Option A against option B: two sections.
    let ab = listening::compare_text(
        &k,
        &Range::Section("Build".into()),
        Some(&Range::Section("Drop".into())),
        None,
        None,
        None,
    )
    .unwrap();
    assert!(ab.starts_with("Build against Drop:"), "{ab}");
    assert!(ab.contains("below 250 Hz"), "{ab}");
    let beats = listening::listen_text(&k, &Range::Bars(9, 9), false, 4, None);
    assert!(beats.contains("RMS per 16th"), "{beats}");
}

#[test]
fn with_nothing_heard_the_tools_say_why() {
    let _g = GLOBAL_FILES.blocking_lock();
    state();
    let k = key("Never Played");
    app_gone();
    assert!(listening::heard_text(&k, None)
        .starts_with("The Mac app is not running, so nothing has been heard"));
    assert!(listening::listen_text(&k, &Range::Last, false, 0, None)
        .starts_with("The Mac app is not running"));
    assert_eq!(
        listening::context_line(&k),
        None,
        "no line when there is nothing to say"
    );
    remembering(false);
    assert!(listening::heard_text(&k, None).contains("Remember what Live plays is off"));
    remembering(true);
    assert!(listening::heard_text(&k, None).contains("nothing has been played in this set"));
    assert_eq!(
        listening::context_line(&k).as_deref(),
        Some("🎧 remembering: nothing of this set played yet")
    );
    // A stale heartbeat is an app that quit.
    listening::write_app_beat(&listening::AppBeat {
        pid: 1,
        remembering: true,
        available: true,
        updated_ms: chrono::Local::now().timestamp_millis() - 60_000,
        ..Default::default()
    });
    assert_eq!(listening::app_state(), listening::AppState::NotRunning);
}

// ── requirement 3: under 300 ms for a ten-minute song ───────────────────────

fn synthetic_take(song_key: &str, id: &str, revision: u64, minutes: f64) -> Take {
    let bars_n = (minutes * 60.0 / BAR_S) as i64;
    let names = [
        "Intro", "Verse", "Build", "Drop", "Break", "Drop 2", "Outro",
    ];
    let bars = (1..=bars_n)
        .map(|bar| {
            let section = names[((bar - 1) * names.len() as i64 / bars_n) as usize].to_string();
            let x = (bar % 7) as f64;
            mcp_ableton_music_maker::recorder::BarRow {
                bar,
                at_s: (bar - 1) as f64 * BAR_S,
                beats: 4.0,
                complete: true,
                section,
                scene: None,
                revision,
                tempo: BPM,
                peak: -3.0 - x * 0.1,
                true_peak: -2.8,
                rms: -16.0 - x * 0.3,
                crest: 13.0,
                lufs: -13.0 - x * 0.2,
                octaves: (0..10).map(|o| -30.0 - o as f64 * 2.0 - x).collect(),
                ranges: (0..6).map(|o| -28.0 - o as f64 * 3.0).collect(),
                correlation: Some(0.8),
                clipped: 0,
                silent: false,
                steps: (0..16)
                    .map(|s| Some(if s % 4 == 0 { -12.0 } else { -18.0 }))
                    .collect(),
            }
        })
        .collect();
    let take = Take {
        version: 1,
        id: id.into(),
        song_key: song_key.into(),
        set_name: "Long".into(),
        started_at: mcp_ableton_music_maker::recorder::now_rfc3339(),
        ended_at: None,
        open: false,
        sample_rate: 48_000.0,
        beats_per_bar: 4.0,
        revision_start: revision,
        revision_end: revision,
        follows_session: false,
        sections: names
            .iter()
            .enumerate()
            .map(|(i, n)| SectionMark {
                name: (*n).into(),
                kind: "locator".into(),
                index: None,
                beat: Some(i as f64 * 4.0 * bars_n as f64 / 7.0),
            })
            .collect(),
        markers: vec![],
        bars,
        frames: 0,
        frames_per_second: 30.0,
    };
    let frames: Vec<FrameRow> = (0..(minutes * 60.0 * 30.0) as usize)
        .map(|i| FrameRow {
            beat: i as f64 / 30.0 * BPM / 60.0,
            lufs: -14.0,
            bands: (0..meter::BANDS).map(|b| -20.0 - b as f32 * 0.5).collect(),
            chroma: [1.0, 0.1, 0.1, 0.6, 0.1, 0.1, 0.1, 0.7, 0.1, 0.9, 0.1, 0.1],
        })
        .collect();
    let mut t = take;
    t.frames = frames.len() as u64;
    listening::write_take(&t, &frames).unwrap();
    t
}

#[test]
fn heard_listen_and_compare_answer_a_ten_minute_song_in_under_300_ms() {
    state();
    let k = key("Ten Minutes");
    synthetic_take(&k, "20261017-100000-000", 0, 10.0);
    listening::record_change(&k, "claude", "feel mode=swing");
    synthetic_take(&k, "20261017-101500-000", 1, 10.0);
    let timed = |label: &str, f: &dyn Fn() -> String| {
        let t = Instant::now();
        let text = f();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        assert!(ms < 300.0, "{label} took {ms:.0} ms");
        assert!(!text.is_empty());
        text
    };
    let h = timed("heard", &|| listening::heard_text(&k, None));
    assert!(h.contains("Heard at the current revision: Intro"), "{h}");
    let l = timed("listen", &|| {
        listening::listen_text(&k, &Range::Section("Drop".into()), false, 0, None)
    });
    assert!(l.contains("at the current revision (1)"), "{l}");
    assert!(
        l.contains("heard as Cm (chroma)") || l.contains("heard as"),
        "{l}"
    );
    timed("listen last", &|| {
        listening::listen_text(&k, &Range::Last, false, 4, None)
    });
    let c = timed("compare", &|| {
        listening::compare_text(&k, &Range::Section("Drop".into()), None, None, None, None).unwrap()
    });
    assert!(c.starts_with("Drop, revision 0 against 1"), "{c}");
    timed("picture", &|| {
        mcp_ableton_music_maker::picture::picture_of(&k, &Range::Section("Drop".into()), None, 720)
            .unwrap()
            .0
    });
}

// ── through the server ──────────────────────────────────────────────────────

fn bridge_for(name: &str) -> std::sync::Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script("run", vec![json!({"set": set_path(name)})]);
    b
}

#[tokio::test]
async fn the_tools_read_the_open_set_and_never_touch_the_transport() {
    state();
    let name = "Server Read";
    let k = key(name);
    synthetic_take(&k, "20261017-120000-000", 0, 1.0);
    let b = bridge_for(name);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::HEARD,
            listening::HeardParams {},
            listening::heard_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Heard at the current revision"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::LISTEN,
            listening::ListenParams {
                bars: Some(json!("3-6")),
                ..Default::default()
            },
            listening::listen_body,
        )
        .await;
    assert!(
        text_of(&r).starts_with("bars 3–6 — measured from playback at"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::COMPARE,
            listening::CompareParams {
                section: Some("Drop".into()),
                ..Default::default()
            },
            listening::compare_body,
        )
        .await;
    assert!(text_of(&r).contains("heard only once"), "{}", text_of(&r));
    // Nothing but the one read that says which set is open.
    assert!(
        b.commands().iter().all(|c| c == "run"),
        "{:?}",
        b.commands()
    );

    let r = server
        .run(
            &tools::LISTEN,
            listening::ListenParams {
                resolution: Some("quarter".into()),
                ..Default::default()
            },
            listening::listen_body,
        )
        .await;
    assert!(is_error(&r), "a bad resolution is refused");
}

#[tokio::test]
async fn a_change_through_a_tool_moves_the_revision_and_a_read_does_not() {
    state();
    let name = "Server Changes";
    let k = key(name);
    let b = bridge_for(name);
    let server = server_with(b.clone());
    // Opening the song is what get_context does first.
    mcp_ableton_music_maker::memory::open_for_set(server.live());
    assert_eq!(listening::revision(&k), 0);
    let r = server
        .run(
            &tools::SET_TEMPO,
            tools::SetTempoParams { tempo: 124.0 },
            tools::set_tempo_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(listening::revision(&k), 1);
    assert_eq!(listening::changes(&k)[0].what, "set_tempo tempo=124.0");
    assert_eq!(listening::changes(&k)[0].by, "claude");
    let _ = server
        .run(
            &tools::HEARD,
            listening::HeardParams {},
            listening::heard_body,
        )
        .await;
    assert_eq!(listening::revision(&k), 1, "a read is not a change");
    // The app sees the same change in Live a moment later: not counted twice.
    assert_eq!(listening::record_change(&k, "live", "song.tempo 124"), 1);
}

#[tokio::test]
async fn listen_picture_returns_a_png_with_its_caption() {
    state();
    let name = "Server Picture";
    let k = key(name);
    let (l, r) = render(0.0);
    play(&k, &l, &r, 0.0);
    let server = server_with(bridge_for(name));
    let res = server
        .run_picture(
            &tools::LISTEN_PICTURE,
            mcp_ableton_music_maker::picture::ListenPictureParams {
                section: Some("Drop".into()),
                ..Default::default()
            },
            mcp_ableton_music_maker::picture::listen_picture_body,
        )
        .await;
    assert!(!is_error(&res), "{}", text_of(&res));
    let v = serde_json::to_value(&res).unwrap();
    let content = v["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert!(content[0]["text"]
        .as_str()
        .unwrap()
        .contains("of bars 9–12, measured from playback"));
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    let data = content[1]["data"].as_str().unwrap();
    assert!(data.starts_with("iVBORw0KGgo"), "a PNG signature in base64");
    assert!(data.len() < 400_000, "{} bytes of base64", data.len());
    // Keep one for a person to look at.
    if let Ok(dir) = std::env::var("AMM_PICTURE_OUT") {
        let png = mcp_ableton_music_maker::picture::picture_of(&k, &Range::Last, None, 900)
            .unwrap()
            .1;
        std::fs::write(std::path::Path::new(&dir).join("listen_picture.png"), png).unwrap();
    }
}

// ── the producer answers: options, comments, replies ───────────────────────

fn option(name: &str, hear: Value) -> feedback::OptionItem {
    feedback::OptionItem {
        name: name.into(),
        what: format!("{name}, built after the song's end"),
        hear: serde_json::from_value(hear).unwrap(),
    }
}

#[tokio::test]
async fn an_options_round_is_two_clicks_and_a_read() {
    state();
    let name = "Options Round";
    let k = key(name);
    let server = server_with(bridge_for(name));
    let bad = server
        .run(
            &tools::OFFER_OPTIONS,
            feedback::OfferOptionsParams {
                question: "Which?".into(),
                options: vec![option("A", json!({"bars": "65-72"}))],
            },
            feedback::offer_options_body,
        )
        .await;
    assert!(is_error(&bad), "one option is not a choice");
    let no_way = server
        .run(
            &tools::OFFER_OPTIONS,
            feedback::OfferOptionsParams {
                question: "Which?".into(),
                options: vec![
                    option("A", json!({"bars": "65-72"})),
                    option("B", json!({})),
                ],
            },
            feedback::offer_options_body,
        )
        .await;
    assert!(
        text_of(&no_way).contains("needs one way to hear it"),
        "{}",
        text_of(&no_way)
    );
    let r = server
        .run(
            &tools::OFFER_OPTIONS,
            feedback::OfferOptionsParams {
                question: "Which breakdown?".into(),
                options: vec![
                    option("Breakdown A", json!({"bars": "65-72"})),
                    option("Breakdown B", json!({"locator": "Break B"})),
                    option("Breakdown C", json!({"scene": "Break C"})),
                    option("Breakdown D", json!({"clip": {"track": "Pad", "slot": 4}})),
                ],
            },
            feedback::offer_options_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains("1. Breakdown A — Breakdown A, built after the song's end (▶ bars 65–72)"),
        "{t}"
    );
    let id = t.split_whitespace().nth(1).unwrap().to_string();
    assert!(id.starts_with("o-"), "{t}");

    // Before the pick, the question waits.
    let before = feedback::feedback_text(&k, 0, None);
    assert!(before.contains("Waiting for a pick:"), "{before}");
    let cursor: i64 = before
        .rsplit("cursor: ")
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // The producer clicks This one on the third, with a note.
    feedback::pick_option(&k, &id, 3, "but shorter").unwrap();
    assert!(feedback::pick_option(&k, &id, 9, "").is_err());
    let got = server
        .run(
            &tools::GET_FEEDBACK,
            feedback::GetFeedbackParams {
                since: Some(json!(cursor)),
            },
            feedback::get_feedback_body,
        )
        .await;
    let g = text_of(&got);
    assert!(
        g.contains("option 3 \"Breakdown C\" — note: \"but shorter\""),
        "{g}"
    );
    assert!(!g.contains("Waiting for a pick"), "{g}");
    let next: i64 = g.rsplit("cursor: ").next().unwrap().trim().parse().unwrap();
    assert!(next > cursor);
    let again = feedback::feedback_text(&k, next, None);
    assert!(
        !again.contains("Picks:"),
        "a cursor reads only what is new: {again}"
    );
}

#[tokio::test]
async fn comments_land_on_their_bar_and_replies_land_in_their_thread() {
    state();
    let name = "Comments";
    let k = key(name);
    listening::record_change(&k, "claude", "build_song");
    let server = server_with(bridge_for(name));
    for (bar, section, text) in [
        (12, "Build", "too long"),
        (19, "Drop", "hats harsh"),
        (25, "Drop", "love this"),
        (40, "Outro", "ending abrupt"),
    ] {
        feedback::add_comment(&k, Some(bar), section, text).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(feedback::add_comment(&k, Some(1), "", "  ").is_err());
    let got = server
        .run(
            &tools::GET_FEEDBACK,
            feedback::GetFeedbackParams::default(),
            feedback::get_feedback_body,
        )
        .await;
    let g = text_of(&got);
    assert!(g.contains("bar 19 (Drop)"), "{g}");
    assert!(g.contains("\"hats harsh\""), "{g}");
    let comments = feedback::comments(&k);
    assert_eq!(comments.len(), 4);
    assert!(
        comments.iter().all(|c| c.revision == 1),
        "a comment carries the revision it was about"
    );
    let first = comments[1].id.clone();
    let r = server
        .run(
            &tools::REPLY,
            feedback::ReplyParams {
                comment: first.clone(),
                text: "hats down 2 dB from bar 17".into(),
            },
            feedback::reply_body,
        )
        .await;
    assert!(
        text_of(&r).contains("bar 19: \"hats harsh\""),
        "{}",
        text_of(&r)
    );
    let missing = server
        .run(
            &tools::REPLY,
            feedback::ReplyParams {
                comment: "c-1".into(),
                text: "?".into(),
            },
            feedback::reply_body,
        )
        .await;
    assert!(is_error(&missing));
    let screen = feedback::screen(&k);
    let thread = screen["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["comment"]["id"] == first)
        .unwrap();
    assert_eq!(thread["replies"][0]["text"], "hats down 2 dB from bar 17");
    assert!(feedback::feedback_text(&k, 0, None).contains("— answered"));
}

// ── Part D: Claude plays only when the room is empty ────────────────────────

#[tokio::test]
async fn playing_to_measure_needs_the_away_switch_and_stops_with_it() {
    let _g = GLOBAL_FILES.lock().await;
    state();
    let name = "Away";
    let b = bridge_for(name);
    // A very fast tempo, so eight bars play in a fraction of a second.
    b.script(
        "get_session_info",
        vec![json!({"tempo": 6000.0, "signature_numerator": 4, "is_playing": true})],
    );
    let server = server_with(b.clone());
    remembering(true);
    feedback::set_away(false, 0, "producer").unwrap();
    let r = server
        .run(
            &tools::LISTEN,
            listening::ListenParams {
                bars: Some(json!("9-16")),
                play: true,
                ..Default::default()
            },
            listening::listen_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        text_of(&r).contains("Claude may play the set is off, so nothing was played"),
        "{}",
        text_of(&r)
    );
    assert!(
        !b.commands().iter().any(|c| c == "play_from"),
        "{:?}",
        b.commands()
    );

    feedback::set_away(true, 20, "").unwrap();
    assert!(feedback::away_on());
    let r = server
        .run(
            &tools::LISTEN,
            listening::ListenParams {
                bars: Some(json!("9-16")),
                play: true,
                ..Default::default()
            },
            listening::listen_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.last("play_from"),
        Some(json!({"time": 32.0})),
        "from bar 9"
    );
    assert!(
        b.commands().iter().any(|c| c == "stop_playback"),
        "{:?}",
        b.commands()
    );
    assert!(
        !feedback::claude_playing(),
        "the marker is gone once Claude stops"
    );

    // The producer stops Live mid-way: Claude's playback ends and says so.
    b.script(
        "get_session_info",
        vec![
            json!({"tempo": 60.0, "signature_numerator": 4, "is_playing": true}),
            json!({"tempo": 60.0, "signature_numerator": 4, "is_playing": true}),
            json!({"tempo": 60.0, "signature_numerator": 4, "is_playing": false}),
        ],
    );
    let r = server
        .run(
            &tools::LISTEN,
            listening::ListenParams {
                bars: Some(json!("1-8")),
                play: true,
                ..Default::default()
            },
            listening::listen_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        text_of(&r).contains("the transport was stopped in Live"),
        "{}",
        text_of(&r)
    );

    // Pressing play themselves turns the switch off; Claude's own start does not.
    std::fs::write(
        mcp_ableton_music_maker::state::feedback_dir().join("claude_playing.json"),
        json!({"until_ms": chrono::Local::now().timestamp_millis() + 5_000}).to_string(),
    )
    .unwrap();
    assert_eq!(
        feedback::transport_moved_by_someone(),
        None,
        "Claude's own start"
    );
    std::fs::remove_file(
        mcp_ableton_music_maker::state::feedback_dir().join("claude_playing.json"),
    )
    .unwrap();
    let off = feedback::transport_moved_by_someone().unwrap();
    assert!(!off.on);
    assert_eq!(off.off_reason.as_deref(), Some("played"));
    assert!(feedback::feedback_text("x", 0, None).contains("off (the producer pressed play)"));
}

// ── the app's clock path, on the real script ────────────────────────────────

/// The Mac app's clock feed against `scripts/fake-live.py` — the real Remote
/// Script on its own socket and tick: subscribe to `clock` and `changes`,
/// read with `wait_events` (sending nothing), turn the events into a
/// playhead, and see the transport start and stop. Then read the set the way
/// a take is stamped (`listening::set_view`).
#[test]
fn the_apps_clock_feed_follows_the_script_and_reads_the_set() {
    use mcp_ableton_music_maker::connection::{AbletonConnection, LiveState};
    use mcp_ableton_music_maker::recorder::{Feed, Seen};
    if common::targets_a_real_live() {
        // It starts and stops the transport: not on a set someone is using.
        return;
    }
    state();
    let fake = common::spawn_fake_live(&["--shared-set", "--quiet", "--latency", "none"]);
    let clock = AbletonConnection::new("127.0.0.1", fake.port);
    let r = clock.subscribe(&["clock", "changes"], Some(50.0)).unwrap();
    assert!(r.to_string().contains("clock"), "{r}");
    let live = LiveState::new(std::sync::Arc::new(AbletonConnection::new(
        "127.0.0.1",
        fake.port,
    )));
    live.script.assume_all_capabilities();
    tools::run_named(&live, "create_locator", json!({"name": "Intro", "bar": 1})).unwrap();
    tools::run_named(&live, "create_locator", json!({"name": "Drop", "bar": 9})).unwrap();
    let (key, name, view, _) = listening::set_view(&live);
    assert!(!key.is_empty());
    assert!(!name.is_empty());
    let locs: Vec<(&str, Option<f64>)> = view
        .sections
        .iter()
        .filter(|s| s.kind == "locator")
        .map(|s| (s.name.as_str(), s.beat))
        .collect();
    assert_eq!(locs, vec![("Intro", Some(0.0)), ("Drop", Some(32.0))]);

    let mut rem = Remembering::new(48_000.0);
    let mut feed = Feed::new();
    let t0 = Instant::now();
    let mut seen: Vec<Seen> = Vec::new();
    let read = |until_s: f64, rem: &mut Remembering, feed: &mut Feed, seen: &mut Vec<Seen>| {
        while t0.elapsed().as_secs_f64() < until_s {
            for ev in clock
                .wait_events(std::time::Duration::from_millis(150))
                .unwrap()
            {
                seen.extend(feed.event(&ev, t0.elapsed().as_secs_f64(), rem));
            }
        }
    };
    read(0.5, &mut rem, &mut feed, &mut seen);
    live.send_command("start_playback", None).unwrap();
    read(2.5, &mut rem, &mut feed, &mut seen);
    let last = rem
        .clock
        .last_event()
        .cloned()
        .expect("clock events arrived");
    assert!(last.playing, "{last:?}");
    assert!(seen.contains(&Seen::Started), "{seen:?}");
    // The playhead runs at the set's tempo between ticks.
    let now = t0.elapsed().as_secs_f64();
    let p = rem.clock.at(now).unwrap();
    assert!(p.playing);
    assert!(
        (p.beats_per_s - last.tempo / 60.0).abs() / (last.tempo / 60.0) < 0.06,
        "{p:?} at tempo {}",
        last.tempo
    );
    live.send_command("stop_playback", None).unwrap();
    read(3.5, &mut rem, &mut feed, &mut seen);
    assert!(seen.contains(&Seen::Stopped), "{seen:?}");
    assert!(!rem.clock.last_event().unwrap().playing);
    // A tempo set in Live is an edit the app logs.
    live.send_command("set_tempo", Some(json!({"tempo": 131.0})))
        .unwrap();
    read(5.0, &mut rem, &mut feed, &mut seen);
    assert!(
        seen.iter()
            .any(|s| matches!(s, Seen::Edit(w) if w.contains("song.tempo 131"))),
        "{seen:?}"
    );
}

/// The 🎧 line rides directly under get_context's clock line, on the real
/// script's payload, once the open set has been heard — and a change made
/// through a tool after that makes it say what is stale.
#[tokio::test]
async fn get_context_carries_the_heard_line_under_its_clock_line() {
    let _g = GLOBAL_FILES.lock().await;
    if common::targets_a_real_live() {
        return;
    }
    let (server, _bridge) = common::server_on_fake_live();
    remembering(true);
    let ctx = || async {
        text_of(
            &server
                .run(
                    &tools::GET_CONTEXT,
                    tools::GetContextParams::default(),
                    tools::get_context_body,
                )
                .await,
        )
    };
    let first = ctx().await;
    let key = server
        .live()
        .songs
        .snapshot()
        .expect("get_context opened the song")
        .key;
    assert!(!first.contains("🎧 heard:"), "nothing heard yet: {first}");
    synthetic_take(&key, "20261017-130000-000", listening::revision(&key), 0.5);
    let text = ctx().await;
    let lines: Vec<&str> = text.lines().collect();
    let clock = lines
        .iter()
        .position(|l| l.contains(" BPM "))
        .expect("the clock line");
    assert!(
        lines[clock + 1].starts_with("🎧 heard: 7 of 7 sections current"),
        "{}",
        lines[clock + 1]
    );
    let r = server
        .run(
            &tools::SET_TEMPO,
            tools::SetTempoParams { tempo: 97.0 },
            tools::set_tempo_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let text = ctx().await;
    assert!(
        text.contains(
            "🎧 heard: 0 of 7 sections current; Intro not heard since set_tempo tempo=97.0"
        ),
        "{text}"
    );
}
