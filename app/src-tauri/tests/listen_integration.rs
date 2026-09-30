//! The Listen feature through the same entry points the window calls, on a
//! mock Tauri runtime. The parts that need no audio run everywhere; the ones
//! that tap Live are marked `#[ignore]` and are the live check:
//!
//! ```bash
//! cd app/src-tauri && cargo test --test listen_integration -- --ignored --test-threads=1
//! ```
//!
//! Run those with Live open and playing.

use ableton_music_maker_app_lib::listen::{self, tap};
use serde_json::Value;
use std::sync::mpsc;
use std::time::Duration;
use tauri::{Listener, Manager};

fn app() -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_builder()
        .build(ableton_music_maker_app_lib::context())
        .expect("mock app");
    app.manage(listen::Listen::default());
    app
}

#[test]
fn status_answers_before_anything_is_started() {
    let app = app();
    let st: Value = listen::status(app.handle()).expect("status");
    assert_eq!(st["listening"], false);
    assert_eq!(st["available"], tap::available());
    // Live may or may not be running on the machine this test runs on; the
    // field must exist either way so the screen can decide.
    assert!(st.get("live_pid").is_some());
}

#[test]
fn stopping_when_nothing_runs_is_harmless() {
    let app = app();
    let before = tap::audio_device_count();
    let out = listen::stop(app.handle());
    assert_eq!(out["listening"], false);
    assert_eq!(tap::audio_device_count(), before, "stop left a device behind");
    // And again, because the window may ask twice.
    listen::stop(app.handle());
    assert!(!listen::is_listening(app.handle()));
}

#[test]
fn an_unwatched_session_stops_itself() {
    let app = app();
    // No windows exist on the mock runtime, so nothing is watching.
    listen::stop_if_unwatched(app.handle(), "main");
    assert!(!listen::is_listening(app.handle()));
}

#[test]
#[ignore = "needs Ableton Live running and playing"]
fn a_real_session_emits_frames_and_leaves_nothing_behind() {
    let app = app();
    let baseline = tap::audio_device_count();

    let (tx, rx) = mpsc::channel::<Value>();
    app.handle().listen("listen:frame", move |event| {
        let _ = tx.send(serde_json::from_str(event.payload()).unwrap_or(Value::Null));
    });

    let started: Value = listen::start(app.handle()).expect("Live must be running");
    assert_eq!(started["listening"], true);
    assert!(started["live_pid"].as_i64().unwrap_or(0) > 0);
    assert!(started["sample_rate"].as_f64().unwrap_or(0.0) >= 44_100.0);
    assert_eq!(started["bands"], 72);
    assert!(listen::is_listening(app.handle()));

    // Frames should arrive about thirty times a second.
    let mut frames = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if let Ok(f) = rx.recv_timeout(Duration::from_millis(500)) {
            frames.push(f);
        }
    }
    assert!(
        frames.len() > 30,
        "expected ~90 frames in three seconds, got {}",
        frames.len()
    );

    let f = frames.last().expect("a frame");
    assert_eq!(f["bands"].as_array().map(Vec::len), Some(72));
    assert_eq!(f["hold"].as_array().map(Vec::len), Some(72));
    assert_eq!(f["ranges"].as_array().map(Vec::len), Some(6));
    for key in ["peak", "rms"] {
        let v = f[key].as_array().expect(key);
        assert_eq!(v.len(), 2);
        for x in v {
            let db = x.as_f64().expect("a number");
            assert!((-80.0..=6.0).contains(&db), "{key} out of range: {db}");
        }
    }
    let corr = f["correlation"].as_f64().expect("correlation");
    assert!((-1.0..=1.0).contains(&corr), "correlation {corr}");

    // Starting twice is the same session, not a second tap.
    let again: Value = listen::start(app.handle()).expect("idempotent");
    assert_eq!(again["live_pid"], started["live_pid"]);

    listen::stop(app.handle());
    std::thread::sleep(Duration::from_millis(600));
    assert!(!listen::is_listening(app.handle()));
    assert_eq!(
        tap::audio_device_count(),
        baseline,
        "the private aggregate device outlived the session"
    );
}

#[test]
#[ignore = "needs Ableton Live running"]
fn a_session_heard_live_and_not_the_rest_of_the_machine() {
    let app = app();
    listen::start(app.handle()).expect("Live must be running");
    std::thread::sleep(Duration::from_secs(1));
    let st: Value = listen::status(app.handle()).expect("status");
    // The tap is delivering buffers even when Live is silent: that is how the
    // screen tells "quiet" from "the permission is off".
    assert!(
        st["frames_seen"].as_u64().unwrap_or(0) > 0,
        "no audio frames arrived at all"
    );
    assert!(st["meter_lag_ms"].as_f64().unwrap_or(0.0) < 200.0);
    assert!(st["window_ms"].as_f64().unwrap_or(0.0) > 0.0);
    listen::stop(app.handle());
}
