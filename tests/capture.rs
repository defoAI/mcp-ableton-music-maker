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
        start: 128.0,
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
        named["name"].as_str().unwrap().starts_with("drop @ 128 | "),
        "{named}"
    );
    let sent_start = &bridge.sent()[2].1;
    assert_eq!(sent_start["bars"], 2);
    assert_eq!(sent_start["start"], 128.0);
    let t = text_of(&r);
    assert!(t.starts_with("Created the Capture track at index 7"), "{t}");
    assert!(
        t.contains(
            "Captured 'drop @ 128' (Capture track, slot 0) — 2 bars at 120 BPM, 4.0 s, stereo"
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
        start: 0.0,
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
async fn capture_refuses_bad_lengths_before_live() {
    let bridge = FakeBridge::responding(json!({}));
    let server = server_with(bridge.clone());
    let p = CaptureMixParams {
        start: 0.0,
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
