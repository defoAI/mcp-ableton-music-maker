//! capture_mix end to end through the real body with a scripted bridge and a
//! real WAV on disk: command order, polling, the timeout guard, naming, and
//! the readout.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::tools::{self, CaptureMixParams, MeasureCaptureParams};
use serde_json::json;
use std::path::Path;

fn write_wav(path: &Path, seconds: f64, amp_first_half: f32, amp_second_half: f32) {
    let rate = 8000u32;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let frames = (seconds * rate as f64) as usize;
    for i in 0..frames {
        let amp = if i < frames / 2 {
            amp_first_half
        } else {
            amp_second_half
        };
        let s = (amp
            * (2.0 * std::f32::consts::PI * 110.0 * i as f32 / rate as f32).sin()
            * 32767.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn capture_bridge(file: &Path) -> std::sync::Arc<FakeBridge> {
    let bridge = FakeBridge::responding(json!({}));
    bridge.script(
        "get_session_info",
        vec![json!({"tempo": 120.0, "signature_numerator": 4})],
    );
    bridge.script(
        "ensure_capture_track",
        vec![json!({"index": 7, "created": true, "input": "Resampling", "slots": 8})],
    );
    bridge.script(
        "start_capture",
        vec![json!({"slot": 0, "record_length": 8.0, "preroll_beats": 1.0, "tempo": 120.0})],
    );
    bridge.script("capture_status", vec![
        json!({"slot": 0, "has_clip": false, "is_recording": false}),
        json!({"slot": 0, "has_clip": true, "is_recording": true, "name": "drop @ 128"}),
        json!({"slot": 0, "has_clip": true, "is_recording": false, "name": "drop @ 128", "length": 8.0, "file_path": file.to_str().unwrap()}),
    ]);
    bridge.script(
        "stop_capture",
        vec![json!({"stopped_slot": null, "is_playing": false})],
    );
    bridge.script("set_clip_name", vec![json!({"name": "x"})]);
    bridge
}

#[tokio::test]
async fn capture_records_measures_and_names_the_clip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Capture 0001.wav");
    write_wav(&file, 4.0, 0.1, 0.4); // 2 bars at 120 BPM in 4/4: quiet bar, louder bar
    let bridge = capture_bridge(&file);
    let server = server_with(bridge.clone());
    let p = CaptureMixParams {
        start_bar: None,
        start: Some(128.0),
        bars: 2,
        name: "drop".into(),
    };
    let r = server
        .run(&tools::CAPTURE_MIX, p, tools::capture_mix_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = bridge.commands();
    assert_eq!(
        &cmds[..3],
        &["get_session_info", "ensure_capture_track", "start_capture"]
    );
    assert_eq!(cmds.iter().filter(|c| *c == "capture_status").count(), 3);
    assert!(
        cmds.contains(&"stop_capture".to_string()),
        "transport stopped after the recording"
    );
    assert_eq!(cmds.last().unwrap(), "set_clip_name");
    let named = bridge.sent().last().unwrap().1.clone();
    assert_eq!(named["track_index"], 7);
    assert!(
        named["name"].as_str().unwrap().starts_with("drop @ 33 | "),
        "{named}"
    );
    let sent_start = &bridge.sent()[2].1;
    assert_eq!(sent_start["bars"], 2);
    assert_eq!(sent_start["start"], 128.0);
    let t = text_of(&r);
    assert!(t.starts_with("Created the Capture track at index 7"), "{t}");
    assert!(
        t.contains(
            "Captured 'drop @ bar 33' (Capture track, slot 0) — 2 bars at 120 BPM, 4.0 s, stereo"
        ),
        "{t}"
    );
    assert!(
        t.contains("RMS per bar:") && t.contains("louder") && t.contains("Capture slot 0"),
        "{t}"
    );
}

#[tokio::test]
async fn capture_times_out_and_stops_live() {
    let bridge = FakeBridge::responding(json!({}));
    bridge.script(
        "get_session_info",
        vec![json!({"tempo": 6000.0, "signature_numerator": 4})],
    ); // absurd tempo: tiny budget
    bridge.script(
        "ensure_capture_track",
        vec![json!({"index": 3, "created": false})],
    );
    bridge.script(
        "start_capture",
        vec![json!({"slot": 2, "record_length": 4.0, "preroll_beats": 1.0})],
    );
    bridge.script(
        "capture_status",
        vec![json!({"slot": 2, "has_clip": true, "is_recording": true})],
    );
    bridge.script("stop_capture", vec![json!({"stopped_slot": 2})]);
    let server = server_with(bridge.clone());
    let p = CaptureMixParams {
        start_bar: None,
        start: Some(0.0),
        bars: 1,
        name: "x".into(),
    };
    let r = server
        .run(&tools::CAPTURE_MIX, p, tools::capture_mix_body)
        .await;
    assert!(is_error(&r));
    assert!(text_of(&r).contains("did not finish"), "{}", text_of(&r));
    let cmds = bridge.commands();
    assert_eq!(
        cmds.last().unwrap(),
        "stop_capture",
        "the guard stopped the capture"
    );
    assert_eq!(bridge.sent().last().unwrap().1["slot"], 2);
}

#[tokio::test]
async fn a_recording_the_server_cannot_see_says_why_and_not_no_such_file() {
    // The hardened container mounts nothing but its state folder, so Live's
    // recording folder is not there. A bare io error sends the producer
    // looking in Live for a clip that is sitting right in front of them.
    let missing = std::path::Path::new("/nowhere/a-set/Samples/Recorded/Capture 0001.wav");
    let e = mcp_ableton_music_maker::audio::read_file(missing).unwrap_err();
    assert!(e.contains("Capture 0001.wav"), "{e}");
    assert!(
        e.contains("this server's own filesystem") && e.contains("the container mounts nothing"),
        "the reply says why the file is unreachable: {e}"
    );
    assert!(
        e.contains("the native binary or the Mac app"),
        "and what to use instead: {e}"
    );
    assert!(
        e.contains("on the Capture track in Live either way"),
        "and that the take itself is not lost: {e}"
    );
}

#[tokio::test]
async fn capture_refuses_bad_lengths_before_live() {
    let bridge = FakeBridge::responding(json!({}));
    let server = server_with(bridge.clone());
    let p = CaptureMixParams {
        start_bar: None,
        start: Some(0.0),
        bars: 65,
        name: "x".into(),
    };
    let r = server
        .run(&tools::CAPTURE_MIX, p, tools::capture_mix_body)
        .await;
    assert!(is_error(&r) && bridge.sent().is_empty());
}

#[tokio::test]
async fn measure_capture_rereads_a_slot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Capture 0002.aif.wav");
    write_wav(&file, 2.0, 0.3, 0.3);
    let bridge = FakeBridge::responding(json!({}));
    bridge.script(
        "get_session_info",
        vec![json!({"tempo": 120.0, "signature_numerator": 4})],
    );
    bridge.script("capture_status", vec![json!({"slot": 1, "has_clip": true, "is_recording": false, "name": "intro @ 0", "length": 4.0, "file_path": file.to_str().unwrap()})]);
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::MEASURE_CAPTURE,
            MeasureCaptureParams { slot: 1 },
            tools::measure_capture_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Captured 'intro @ 0' (Capture track, slot 1) — 1 bars"),
        "{}",
        text_of(&r)
    );
    let empty = FakeBridge::responding(json!({"has_clip": false}));
    let server = server_with(empty);
    let r = server
        .run(
            &tools::MEASURE_CAPTURE,
            MeasureCaptureParams { slot: 4 },
            tools::measure_capture_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("holds no capture"));
}

// ── A capture is the bars you asked for, or it is an error ──────────────────

/// A take whose first `silent_seconds` are digital silence: what Live gives
/// back when recording starts before the playhead reaches the bar.
fn write_wav_with_silent_head(path: &Path, seconds: f64, silent_seconds: f64) {
    let rate = 8000u32;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let frames = (seconds * rate as f64) as usize;
    let silent = (silent_seconds * rate as f64) as usize;
    for i in 0..frames {
        let amp = if i < silent { 0.0 } else { 0.4 };
        let s = (amp
            * (2.0 * std::f32::consts::PI * 110.0 * i as f32 / rate as f32).sin()
            * 32767.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

#[tokio::test]
async fn a_take_that_starts_early_is_discarded_and_recorded_again() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("early.wav");
    let good = dir.path().join("clean.wav");
    write_wav_with_silent_head(&bad, 4.0, 1.8); // the playhead had not arrived
    write_wav_with_silent_head(&good, 4.0, 0.0);
    let bridge = capture_bridge(&bad);
    bridge.script(
        "start_capture",
        vec![
            json!({"slot": 0, "record_length": 8.0, "preroll_beats": 4.0, "tempo": 120.0,
                    "confirmed_at": 126.10, "begin_beat": 124.0, "launch_quantization": "q_bar"}),
        ],
    );
    // Two takes: the first opens with silence, the second is clean.
    bridge.script("capture_status", vec![
        json!({"slot": 0, "has_clip": true, "is_recording": false, "name": "drop", "length": 8.0, "file_path": bad.to_str().unwrap()}),
        json!({"slot": 0, "has_clip": true, "is_recording": false, "name": "drop", "length": 8.0, "file_path": good.to_str().unwrap()}),
    ]);
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            CaptureMixParams {
                start_bar: Some(json!(33.0)),
                start: None,
                bars: 2,
                name: "drop".into(),
            },
            tools::capture_mix_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    let cmds = bridge.commands();
    assert_eq!(
        cmds.iter().filter(|c| *c == "start_capture").count(),
        2,
        "the bad take was recorded again: {cmds:?}"
    );
    assert!(
        cmds.contains(&"delete_clip".to_string()),
        "the bad take was cleared out of the slot: {cmds:?}"
    );
    assert!(
        t.contains("The first take's opening 1.80 s was silent"),
        "the retry is reported, not hidden: {t}"
    );
    assert!(
        t.contains("Playhead confirmed at beat 126.10 before recording began."),
        "{t}"
    );
    assert!(
        !t.contains("louder than bars"),
        "silence is never narrated as a musical difference: {t}"
    );
}

#[tokio::test]
async fn two_bad_takes_are_an_error_with_no_measurements() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("early-again.wav");
    write_wav_with_silent_head(&bad, 4.0, 2.1);
    let bridge = capture_bridge(&bad);
    bridge.script(
        "capture_status",
        vec![json!({"slot": 0, "has_clip": true, "is_recording": false, "name": "drop", "length": 8.0, "file_path": bad.to_str().unwrap()})],
    );
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            CaptureMixParams {
                start_bar: Some(json!(33.0)),
                start: None,
                bars: 2,
                name: "drop".into(),
            },
            tools::capture_mix_body,
        )
        .await;
    let t = text_of(&r);
    assert!(is_error(&r), "{t}");
    assert!(
        t.contains("both takes began before the playhead reached"),
        "{t}"
    );
    assert!(
        !t.contains("peak") && !t.contains("RMS"),
        "no measurement is offered for an invalid take: {t}"
    );
    assert_eq!(
        bridge
            .commands()
            .iter()
            .filter(|c| *c == "delete_clip")
            .count(),
        2,
        "both bad takes were cleared"
    );
}

#[tokio::test]
async fn the_reading_names_a_spectral_problem() {
    // A 110 Hz tone: all the energy is at the bottom, which peak and RMS
    // alone never said.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bass-heavy.wav");
    write_wav(&file, 4.0, 0.4, 0.4);
    let bridge = capture_bridge(&file);
    bridge.script("capture_status", vec![
        json!({"slot": 0, "has_clip": true, "is_recording": false, "name": "low", "length": 8.0, "file_path": file.to_str().unwrap()}),
    ]);
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            CaptureMixParams {
                start_bar: Some(json!(1.0)),
                start: None,
                bars: 2,
                name: "low".into(),
            },
            tools::capture_mix_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("crest "), "crest factor is reported: {t}");
    assert!(
        t.contains("LF/HF "),
        "the low-to-high ratio is reported: {t}"
    );
    assert!(t.contains("bands "), "the octave bands are reported: {t}");
    assert!(
        t.contains("below 250 Hz"),
        "the reading names the low end: {t}"
    );
    assert!(
        t.contains("access grant"),
        "the reply says the folder may need a grant: {t}"
    );
}

// ── Names instead of numbers (#66) ──────────────────────────────────────────

/// #66: a named bar is a locator. `create_locator` already makes them and
/// Live's Save keeps them, so `capture_mix` can start at one by name.
#[tokio::test]
async fn capture_mix_starts_at_a_locator_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Capture 0001.wav");
    write_wav(&file, 4.0, 0.4, 0.4);
    let bridge = capture_bridge(&file);
    // The locators, as the generic ops layer reads them.
    bridge.script(
        "run",
        vec![
            json!({"cues": ["Drop"]}),
            json!({"n0": "Drop", "t0": 128.0}),
        ],
    );
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            CaptureMixParams {
                start_bar: Some(json!("Drop")),
                start: None,
                bars: 2,
                name: "drop".into(),
            },
            tools::capture_mix_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = bridge
        .last("start_capture")
        .expect("start_capture was sent");
    assert_eq!(sent["start"], 128.0, "the locator's own beat, not bar 1");
}

/// An unknown locator names what the set has, and records nothing.
#[tokio::test]
async fn capture_mix_refuses_an_unknown_locator_without_recording() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Capture 0001.wav");
    write_wav(&file, 4.0, 0.4, 0.4);
    let bridge = capture_bridge(&file);
    bridge.script(
        "run",
        vec![
            json!({"cues": ["Drop"]}),
            json!({"n0": "Drop", "t0": 128.0}),
        ],
    );
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            CaptureMixParams {
                start_bar: Some(json!("Chorus")),
                start: None,
                bars: 2,
                name: "drop".into(),
            },
            tools::capture_mix_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert!(text_of(&r).contains("'Drop' (bar 33)"), "{}", text_of(&r));
    assert!(!bridge.commands().contains(&"start_capture".to_string()));
}
