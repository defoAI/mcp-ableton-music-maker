//! #67 — a device vocabulary the server learns and keeps.
//!
//! The two facts the session of 2026-09-20 paid for: **Vinyl Drawbs does not
//! answer to a cutoff** (its parameters are Vinyl Drive, Release and Rotation
//! Amount), and **VHS Dreams' Release macro runs backwards**. Neither is a
//! fact about that song. They are facts about Ableton's factory content, true
//! in every set anyone builds, so they are keyed on the device and the Live
//! version and they survive the session that learned them.
//!
//! Every assertion here is about what the server **kept** and what it **said**
//! next time — not about a new round trip, because there is none: the reads
//! and the read-back already happen.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::devices;
use mcp_ableton_music_maker::tools::{self, DeviceVocabularyParams, ShapeSoundParams};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock};

/// `ABLETON_MCP_STATE_DIR` and `ABLETON_MCP_LIBRARY_INDEX` belong to the
/// process, not to a test, and the harness runs these on several threads.
static ENV: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The handshake is where the Live version comes from, and every row is
/// stamped with it.
fn script_info(version: &str) -> Value {
    json!({
        "name": "AbletonMCP",
        "script_version": mcp_ableton_music_maker::handshake::expected_remote_script_version(),
        "capabilities": mcp_ableton_music_maker::tools::ALL_REMOTE_COMMANDS,
        "live": {"version": version, "python": "3.11.2"}
    })
}

/// The set `shape_sound` resolves a track name against.
fn with_tracks(b: &FakeBridge, names: &[&str]) {
    b.script(
        "get_performance_state",
        vec![json!({
            "is_playing": false, "tempo": 120.0, "signature_numerator": 4,
            "signature_denominator": 4, "beat": 0.0, "bar": 1, "beat_in_bar": 1,
            "clip_trigger_quantization": 4,
            "tracks": names.iter().enumerate().map(|(i, n)| json!({
                "index": i, "name": n, "playing_slot_index": -1, "slots_with_clips": []
            })).collect::<Vec<_>>(),
            "scenes": [], "cues": [], "events": []
        })],
    );
}

fn param(index: i64, name: &str, value: f64, display: &str) -> Value {
    json!({"index": index, "name": name, "value": value, "min": 0.0, "max": 1.0,
           "display": display, "is_quantized": false, "is_enabled": true})
}

/// A rack the producer loaded: the macro names its maker chose, and no
/// cutoff among them.
fn vinyl_drawbs(b: &FakeBridge) {
    with_tracks(b, &["Keys"]);
    b.script(
        "get_track_info",
        vec![json!({"index": 0, "kind": "track", "name": "Keys", "clip_slots": [],
                    "devices": [{"index": 0, "name": "Vinyl Drawbs", "class_name": "InstrumentGroupDevice", "type": "rack"}]})],
    );
    b.script(
        "get_device_parameters",
        vec![json!({"track_name": "Keys", "device": {
        "index": 0, "name": "Vinyl Drawbs", "class_name": "InstrumentGroupDevice",
        "parameters": [
            param(0, "Device On", 1.0, "On"),
            param(1, "Vinyl Drive", 0.30, "30 %"),
            param(2, "Release", 0.80, "2.04 s"),
            param(3, "Rotation Amount", 0.25, "25 %")
        ]}})],
    );
}

fn server_for(version: &str, b: Arc<FakeBridge>) -> tools::Server {
    let live = Arc::new(
        mcp_ableton_music_maker::connection::LiveState::with_activity(
            b,
            mcp_ableton_music_maker::activity::Activity::disabled(),
        ),
    );
    live.script
        .set(serde_json::from_value(script_info(version)).expect("script info"));
    tools::Server::new(live)
}

fn shape(track: &str, word: &str, to: f64) -> ShapeSoundParams {
    let mut p = ShapeSoundParams {
        track: json!(track),
        ..Default::default()
    };
    match word {
        "cutoff" => p.cutoff = Some(json!(to)),
        "release" => p.release = Some(json!(to)),
        other => panic!("this test does not use {other}"),
    }
    p
}

// ── The failure that is not paid for twice ──────────────────────────────────

/// A word a device does not answer to is a fact about that device. The first
/// call learns it; the next one says so, with the day and the Live version,
/// and with the names the device *does* answer to.
#[tokio::test]
async fn a_word_a_device_refused_is_said_back_next_time_with_its_stamp() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    // The afternoon that paid for it.
    let b = FakeBridge::responding(json!({}));
    vinyl_drawbs(&b);
    let first = server_for("12.4.6", b.clone());
    let r = first
        .run(
            &tools::SHAPE_SOUND,
            shape("Keys", "cutoff", 0.5),
            tools::shape_sound_body,
        )
        .await;
    assert!(is_error(&r), "no macro on this rack says cutoff");
    assert!(
        text_of(&r).contains("Vinyl Drive, Release, Rotation Amount"),
        "{}",
        text_of(&r)
    );

    // A different session, a different song, the same Live: a new server
    // process reading the file the first one wrote.
    let b2 = FakeBridge::responding(json!({}));
    vinyl_drawbs(&b2);
    let later = server_for("12.4.6", b2.clone());
    let r = later
        .run(
            &tools::SHAPE_SOUND,
            shape("Keys", "cutoff", 0.5),
            tools::shape_sound_body,
        )
        .await;
    let text = text_of(&r);
    assert!(is_error(&r), "{text}");
    assert!(
        text.contains("'cutoff' was not on 'Vinyl Drawbs' on"),
        "the second session pays for it again: {text}"
    );
    assert!(text.contains("Live 12.4.6"), "{text}");
    assert!(
        text.contains("Vinyl Drive, Release, Rotation Amount"),
        "{text}"
    );
}

/// A row is believed only for the Live version it was measured on. Another
/// Live is another file, and it starts empty.
#[tokio::test]
async fn what_one_live_answered_is_not_claimed_of_another() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    let b = FakeBridge::responding(json!({}));
    vinyl_drawbs(&b);
    let on_12 = server_for("12.4.6", b.clone());
    let _ = on_12
        .run(
            &tools::SHAPE_SOUND,
            shape("Keys", "cutoff", 0.5),
            tools::shape_sound_body,
        )
        .await;
    assert!(devices::load_from_disk("12.4.6").is_some());

    let b2 = FakeBridge::responding(json!({}));
    vinyl_drawbs(&b2);
    let on_11 = server_for("11.3.0", b2.clone());
    let r = on_11
        .run(
            &tools::SHAPE_SOUND,
            shape("Keys", "cutoff", 0.5),
            tools::shape_sound_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        !text_of(&r).contains("was not on 'Vinyl Drawbs' on"),
        "a Live 12 measurement is not a claim about Live 11: {}",
        text_of(&r)
    );
}

/// An unstamped measurement does not ship: a Live whose version has not been
/// read yet is kept in memory and never written down.
#[tokio::test]
async fn nothing_is_written_for_a_live_whose_version_is_not_known() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    let b = FakeBridge::responding(json!({}));
    vinyl_drawbs(&b);
    // No `live.version` in the handshake at all.
    let server = server_with(b.clone());
    let _ = server
        .run(
            &tools::SHAPE_SOUND,
            shape("Keys", "cutoff", 0.5),
            tools::shape_sound_body,
        )
        .await;
    assert!(
        !dir.path().join("devices").exists(),
        "an unstamped row was written to disk"
    );
}

/// The off switch is the one the browser and sample indexes already have.
#[tokio::test]
async fn the_library_index_switch_keeps_it_in_memory_only() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::set_var("ABLETON_MCP_LIBRARY_INDEX", "false");

    let b = FakeBridge::responding(json!({}));
    vinyl_drawbs(&b);
    let server = server_for("12.4.6", b.clone());
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            shape("Keys", "cutoff", 0.5),
            tools::shape_sound_body,
        )
        .await;
    assert!(is_error(&r), "the call still works, it just is not kept");
    assert!(
        !dir.path().join("devices").exists(),
        "the off switch did not stop the write"
    );
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
}

// ── The macro that runs the other way ───────────────────────────────────────

/// VHS Dreams: a `Release` macro whose reading **falls** as the macro rises.
/// No parameter list shows this; it is only learnable by writing a value and
/// reading the display back, which the server already does. It is reported
/// with its own measurements, and the value the producer asked for is the
/// value that was written — never quietly inverted.
#[tokio::test]
async fn a_macro_that_ran_backwards_is_reported_with_its_measurements() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    let b = FakeBridge::responding(json!({}));
    with_tracks(&b, &["Pad"]);
    b.script(
        "get_track_info",
        vec![json!({"index": 0, "kind": "track", "name": "Pad", "clip_slots": [],
                    "devices": [{"index": 0, "name": "VHS Dreams", "class_name": "InstrumentGroupDevice", "type": "rack"}]})],
    );
    // Live reads back the preset sitting at 2.04 s; the write to 0.6 lands
    // on 168 ms, which is the wrong way round.
    b.script(
        "get_device_parameters",
        vec![json!({"track_name": "Pad", "device": {
            "index": 0, "name": "VHS Dreams", "class_name": "InstrumentGroupDevice",
            "parameters": [param(0, "Release", 0.10, "2.04 s")]}})],
    );
    b.script(
        "set_device_parameters",
        vec![json!({"parameters": [
            {"index": 0, "name": "Release", "value": 0.6, "min": 0.0, "max": 1.0,
             "display": "168 ms", "landed": true, "is_enabled": true}]})],
    );
    let server = server_for("12.4.6", b.clone());
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            shape("Pad", "release", 0.6),
            tools::shape_sound_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    // The value asked for is the value written: nothing is inverted.
    let sent = b.last("set_device_parameters").expect("the write happened");
    assert_eq!(sent["values"][0]["value"], 0.6);

    // The next call, knowing both measurements, says which way it runs.
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            shape("Pad", "release", 0.6),
            tools::shape_sound_body,
        )
        .await;
    let text = text_of(&r);
    assert!(
        text.contains("'Release' ran backwards here"),
        "the direction is reported: {text}"
    );
    assert!(text.contains("2.04 s") && text.contains("168 ms"), "{text}");
}

// ── Seeing it and deleting it ───────────────────────────────────────────────

/// A local cache the producer cannot see or delete is not one that ships.
#[tokio::test]
async fn the_vocabulary_can_be_read_back_and_deleted_in_one_call_each() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    let b = FakeBridge::responding(json!({}));
    vinyl_drawbs(&b);
    let server = server_for("12.4.6", b.clone());
    let _ = server
        .run(
            &tools::SHAPE_SOUND,
            shape("Keys", "cutoff", 0.5),
            tools::shape_sound_body,
        )
        .await;

    let before = b.commands().len();
    let r = server
        .run(
            &tools::DEVICE_VOCABULARY,
            DeviceVocabularyParams::default(),
            tools::device_vocabulary_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert_eq!(b.commands().len(), before, "it asks Live nothing");
    assert!(text.contains("Vinyl Drawbs"), "{text}");
    assert!(text.contains("no cutoff"), "{text}");
    // It says what it does *not* hold, and how to switch it off.
    assert!(text.contains("no note, no audio and no path"), "{text}");
    assert!(text.contains("ABLETON_MCP_LIBRARY_INDEX=false"), "{text}");
    assert!(text.contains("nothing about any song"), "{text}");

    // One device, in detail.
    let r = server
        .run(
            &tools::DEVICE_VOCABULARY,
            DeviceVocabularyParams {
                device: Some("Vinyl Drawbs".into()),
                ..Default::default()
            },
            tools::device_vocabulary_body,
        )
        .await;
    let text = text_of(&r);
    assert!(
        text.contains("Vinyl Drive, Release, Rotation Amount"),
        "{text}"
    );
    assert!(text.contains("Live 12.4.6"), "{text}");

    // And gone.
    assert!(dir.path().join("devices").join("12.4.6.json").exists());
    let r = server
        .run(
            &tools::DEVICE_VOCABULARY,
            DeviceVocabularyParams {
                action: "forget".into(),
                ..Default::default()
            },
            tools::device_vocabulary_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Nothing in Live changed"),
        "{}",
        text_of(&r)
    );
    assert!(!dir.path().join("devices").join("12.4.6.json").exists());
}

/// Decision 0006: this is the raw layer, so it is served as `adv_`, and
/// `CORE_TOOLS` does not grow. `shape_sound` needed no new parameter either.
#[tokio::test]
async fn the_vocabulary_is_advanced_and_the_artist_surface_is_unchanged() {
    let server = server_with(FakeBridge::responding(json!({})));
    let names: Vec<String> = server
        .tool_list()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(
        names.contains(&"adv_device_vocabulary".to_string()),
        "{names:?}"
    );
    assert!(!names.contains(&"device_vocabulary".to_string()));
    assert!(!tools::CORE_TOOLS.contains(&"device_vocabulary"));
    assert!(!tools::CORE_TOOLS.contains(&"adv_device_vocabulary"));
}
