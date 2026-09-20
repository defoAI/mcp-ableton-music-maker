//! The local activity log, driven through the real tool wrapper with the
//! Live socket replaced by a recorder. Each test builds its own `LiveState`
//! after pointing the state directory at a fresh temp dir, so the log's
//! switches are read per test; the environment is process-wide, so tests
//! run one at a time.

mod common;

use common::{is_error, text_of, FakeBridge};
use mcp_ableton_music_maker::connection::LiveState;
use mcp_ableton_music_maker::tools::{self, SetTempoParams};
use std::sync::Arc;

/// Like `common::server_with`, but with the activity log read from the
/// environment — the thing under test.
fn server_with(bridge: Arc<FakeBridge>) -> tools::Server {
    let live = Arc::new(LiveState::new(bridge));
    live.script.assume_all_capabilities();
    tools::Server::new(live)
}
use serde_json::{json, Value};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (k, v) in vars {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    let out = f();
    for (k, _) in vars {
        std::env::remove_var(k);
    }
    out
}

fn lines(dir: &std::path::Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(dir.join("activity")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        out.extend(
            text.lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap()),
        );
    }
    out
}

fn set_tempo(server: &tools::Server, tempo: f64) -> rmcp::model::CallToolResult {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(server.run(
        &tools::SET_TEMPO,
        SetTempoParams { tempo },
        tools::set_tempo_body,
    ))
}

#[test]
fn one_line_per_call_with_sizes_and_commands_but_no_payloads() {
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[
            ("ABLETON_MCP_STATE_DIR", Some(dir.path().to_str().unwrap())),
            ("ABLETON_MCP_ACTIVITY", None),
            ("ABLETON_MCP_ACTIVITY_PAYLOADS", None),
        ],
        || {
            let bridge = FakeBridge::responding(json!({"tempo": 96.0}));
            let server = server_with(bridge.clone());
            let ok = set_tempo(&server, 96.0);
            assert!(!is_error(&ok), "{}", text_of(&ok));

            bridge.set_response(json!({}));
            *bridge.response.lock().unwrap() =
                Err(mcp_ableton_music_maker::connection::LiveError::Ableton(
                    "Tempo out of range".into(),
                ));
            let err = set_tempo(&server, 9999.0);
            assert!(is_error(&err));

            let all = lines(dir.path());
            assert_eq!(all.len(), 2, "{all:?}");
            let first = &all[0];
            assert_eq!(first["tool"], "set_tempo");
            assert_eq!(first["ok"], true);
            assert_eq!(first["commands"], json!(["set_tempo"]));
            assert!(first["in_chars"].as_u64().unwrap() > 0);
            assert!(first["out_chars"].as_u64().unwrap() > 0);
            assert!(first["duration_ms"].is_number());
            assert!(first["live_ms"].is_number());
            assert!(first["ts"].is_string());
            assert!(first.get("params").is_none(), "payloads are off by default");
            assert!(first.get("result").is_none());
            let second = &all[1];
            assert_eq!(second["ok"], false);
            assert!(second["error"]
                .as_str()
                .unwrap()
                .contains("Tempo out of range"));
        },
    );
}

#[test]
fn payloads_are_written_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[
            ("ABLETON_MCP_STATE_DIR", Some(dir.path().to_str().unwrap())),
            ("ABLETON_MCP_ACTIVITY_PAYLOADS", Some("true")),
        ],
        || {
            let server = server_with(FakeBridge::responding(json!({"tempo": 120.0})));
            set_tempo(&server, 120.0);
            let all = lines(dir.path());
            assert_eq!(all.len(), 1);
            assert_eq!(all[0]["params"], json!({"tempo": 120.0}));
            assert!(all[0]["result"].as_str().unwrap().contains("120"));
        },
    );
}

#[test]
fn activity_can_be_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[
            ("ABLETON_MCP_STATE_DIR", Some(dir.path().to_str().unwrap())),
            ("ABLETON_MCP_ACTIVITY", Some("false")),
        ],
        || {
            let server = server_with(FakeBridge::responding(json!({"tempo": 120.0})));
            let result = set_tempo(&server, 120.0);
            assert!(!is_error(&result), "the tool still works");
            assert!(lines(dir.path()).is_empty());
            assert!(!dir.path().join("activity").exists(), "nothing created");
        },
    );
}

/// Protocol 2 lets Live push `clock`, `levels`, `changes` and `cue` events on
/// the same socket the replies come back on. They are not tool calls, and the
/// log is a record of tool calls: an event never becomes a line, and its
/// contents — which carry track names and meter readings — never reach disk.
///
/// Two things hold that, and both are pinned here rather than promised. A
/// tool body cannot observe an event at all: `LiveBridge`, the whole of what
/// a body is given, has one method and it is `send_command`. And the log's
/// own source has no notion of an event to write.
#[test]
fn an_event_stream_leaves_the_activity_log_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[
            ("ABLETON_MCP_STATE_DIR", Some(dir.path().to_str().unwrap())),
            ("ABLETON_MCP_ACTIVITY", None),
            ("ABLETON_MCP_ACTIVITY_PAYLOADS", None),
        ],
        || {
            let bridge = FakeBridge::responding(json!({"tempo": 126.0}));
            let server = server_with(bridge.clone());
            assert!(!is_error(&set_tempo(&server, 126.0)));

            let all = lines(dir.path());
            assert_eq!(all.len(), 1, "one call, one line: {all:?}");
            let keys: Vec<&str> = all[0]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            for key in &keys {
                assert!(
                    !key.contains("event")
                        && !key.contains("clock")
                        && !key.contains("level")
                        && !key.contains("cue"),
                    "an event reached the log as {key:?}"
                );
            }
            let text = serde_json::to_string(&all[0]).unwrap();
            for word in ["clock", "levels", "changes", "beat_in_bar", "master"] {
                assert!(!text.contains(word), "{word:?} reached the log: {text}");
            }
        },
    );

    // The log cannot write what it cannot name.
    let source = include_str!("../src/activity.rs");
    assert!(
        !source.contains("event"),
        "src/activity.rs learned about events; this test is now a real one"
    );
}
