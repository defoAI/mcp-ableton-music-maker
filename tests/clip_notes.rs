//! The clip-notes read / clear / write loop (get_clip_notes,
//! clear_notes_from_clip, add_notes_to_clip), driven through the real tool
//! bodies with the Live socket replaced by a recorder.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::connection::{LiveError, LiveState};
use mcp_ableton_music_maker::tools::{self, AddNotesParams, ClipParams, Note, Server};
use serde_json::{json, Value};
use std::sync::Arc;

fn sample_notes() -> Value {
    json!([
        {"pitch": 36, "start_time": 0.0, "duration": 0.25, "velocity": 100, "mute": false, "note_id": 1},
        {"pitch": 38, "start_time": 1.0, "duration": 0.50, "velocity": 90, "mute": true, "note_id": 2},
        {"pitch": 42, "start_time": 2.0, "duration": 0.25, "velocity": 80, "mute": false, "note_id": 3},
    ])
}

fn sample_response(notes: Value, name: &str, length: f64) -> Value {
    let count = notes.as_array().map(|a| a.len()).unwrap_or(0);
    json!({"clip_name": name, "length": length, "note_count": count, "notes": notes})
}

fn clip(track_index: i64, clip_index: i64) -> ClipParams {
    ClipParams {
        track_index,
        clip_index,
    }
}

async fn read(server: &Server, track: i64, slot: i64) -> rmcp::model::CallToolResult {
    server
        .run(
            &tools::GET_CLIP_NOTES,
            clip(track, slot),
            tools::get_clip_notes_body,
        )
        .await
}

async fn clear(server: &Server, track: i64, slot: i64) -> rmcp::model::CallToolResult {
    server
        .run(
            &tools::CLEAR_NOTES_FROM_CLIP,
            clip(track, slot),
            tools::clear_notes_from_clip_body,
        )
        .await
}

async fn add(
    server: &Server,
    track: i64,
    slot: i64,
    notes: Vec<Note>,
) -> rmcp::model::CallToolResult {
    let params = AddNotesParams {
        track_index: track,
        clip_index: slot,
        clear: false,
        propagate_to_arrangement: false,
        input: mcp_ableton_music_maker::notes::NotesInput {
            notes,
            ..Default::default()
        },
    };
    server
        .run(
            &tools::ADD_NOTES_TO_CLIP,
            params,
            tools::add_notes_to_clip_body,
        )
        .await
}

// ── get_clip_notes ──────────────────────────────────────────────────────────

#[tokio::test]
async fn read_sends_correct_command_and_params() {
    let bridge = FakeBridge::responding(sample_response(sample_notes(), "Fred Pattern", 8.0));
    let server = server_with(bridge.clone());
    read(&server, 2, 5).await;
    assert_eq!(
        bridge.sent(),
        vec![(
            "get_clip_notes".to_string(),
            json!({"track_index": 2, "clip_index": 5})
        )]
    );
}

#[tokio::test]
async fn read_returns_the_payload_as_json() {
    let bridge = FakeBridge::responding(sample_response(sample_notes(), "Fred Pattern", 8.0));
    let server = server_with(bridge);
    let result = read(&server, 0, 0).await;
    assert!(!is_error(&result));
    let payload: Value = serde_json::from_str(&text_of(&result)).unwrap();
    assert_eq!(payload["clip_name"], "Fred Pattern");
    assert_eq!(payload["length"], 8.0);
    assert_eq!(payload["note_count"], 3);
    assert_eq!(payload["notes"], sample_notes());
}

#[tokio::test]
async fn read_empty_clip_is_zero_notes_not_an_error() {
    let bridge = FakeBridge::responding(sample_response(json!([]), "Empty", 4.0));
    let server = server_with(bridge);
    let result = read(&server, 0, 0).await;
    assert!(!is_error(&result));
    let payload: Value = serde_json::from_str(&text_of(&result)).unwrap();
    assert_eq!(payload["notes"], json!([]));
    assert_eq!(payload["note_count"], 0);
}

#[tokio::test]
async fn read_connection_error_is_an_error_result() {
    let bridge = FakeBridge::failing(LiveError::Lost("boom".into()));
    let server = server_with(bridge);
    let result = read(&server, 0, 0).await;
    assert!(is_error(&result));
    let text = text_of(&result);
    assert!(text.starts_with("Could not get clip notes:"), "{text}");
    assert!(text.contains("boom"));
}

#[tokio::test]
async fn read_output_feeds_straight_into_add_notes() {
    // The reader's note objects, extended fields included, are exactly what
    // add_notes_to_clip forwards.
    let bridge = FakeBridge::responding(sample_response(sample_notes(), "Fred Pattern", 8.0));
    let server = server_with(bridge.clone());
    let payload: Value = serde_json::from_str(&text_of(&read(&server, 0, 0).await)).unwrap();
    let notes: Vec<Note> = serde_json::from_value(payload["notes"].clone()).unwrap();
    bridge.set_response(json!({"note_count": notes.len()}));
    add(&server, 0, 0, notes).await;
    let written = bridge
        .sent()
        .into_iter()
        .find(|(c, _)| c == "add_notes_to_clip")
        .unwrap()
        .1;
    assert_eq!(written["notes"], sample_notes());
}

#[tokio::test]
async fn add_notes_forwards_track_clip_and_notes() {
    let bridge = FakeBridge::responding(json!({"note_count": 1}));
    let server = server_with(bridge.clone());
    let one =
        json!([{"pitch": 60, "start_time": 0.0, "duration": 1.0, "velocity": 100, "mute": false}]);
    let notes: Vec<Note> = serde_json::from_value(one.clone()).unwrap();
    let result = add(&server, 3, 7, notes).await;
    assert!(
        text_of(&result).starts_with("Added 1 notes to clip at track 3, slot 7"),
        "{}",
        text_of(&result)
    );
    assert_eq!(
        bridge.sent(),
        vec![(
            "add_notes_to_clip".to_string(),
            json!({"track_index": 3, "clip_index": 7, "notes": one})
        )]
    );
}

// ── clear_notes_from_clip and the true replace loop ─────────────────────────

#[tokio::test]
async fn clear_sends_correct_command_and_params() {
    let bridge = FakeBridge::responding(json!({"clip_name": "Fred", "cleared_count": 3}));
    let server = server_with(bridge.clone());
    clear(&server, 1, 4).await;
    assert_eq!(
        bridge.sent(),
        vec![(
            "clear_notes_from_clip".to_string(),
            json!({"track_index": 1, "clip_index": 4})
        )]
    );
}

#[tokio::test]
async fn clear_output_reports_count_and_name() {
    let bridge = FakeBridge::responding(json!({"clip_name": "Fred Pattern", "cleared_count": 5}));
    let server = server_with(bridge);
    let text = text_of(&clear(&server, 0, 0).await);
    assert!(text.contains("Cleared 5 note"), "{text}");
    assert!(text.contains("Fred Pattern"));
}

#[tokio::test]
async fn clear_connection_error_is_reported() {
    let bridge = FakeBridge::failing(LiveError::Lost("boom".into()));
    let server = server_with(bridge);
    let result = clear(&server, 0, 0).await;
    assert!(is_error(&result));
    let text = text_of(&result);
    assert!(
        text.starts_with("Could not clear notes from clip:"),
        "{text}"
    );
    assert!(text.contains("boom"));
}

#[tokio::test]
async fn true_replace_loop_read_clear_add() {
    // read -> modify -> clear -> write yields a clip holding ONLY the
    // modified notes, not the union.
    let bridge = FakeBridge::responding(sample_response(sample_notes(), "Fred Pattern", 8.0));
    let server = server_with(bridge.clone());

    let payload: Value = serde_json::from_str(&text_of(&read(&server, 0, 0).await)).unwrap();
    let mut notes: Vec<Note> = serde_json::from_value(payload["notes"].clone()).unwrap();
    for n in &mut notes {
        n.pitch += 7; // transpose up a fifth
    }

    bridge.set_response(json!({"clip_name": "Fred Pattern", "cleared_count": 3}));
    clear(&server, 0, 0).await;
    bridge.set_response(json!({"note_count": 3}));
    add(&server, 0, 0, notes).await;

    assert_eq!(
        bridge.commands(),
        [
            "get_clip_notes",
            "clear_notes_from_clip",
            "add_notes_to_clip"
        ]
    );
    let sent = bridge.sent();
    assert_eq!(sent[1].1, json!({"track_index": 0, "clip_index": 0}));
    let pitches: Vec<i64> = sent[2].1["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["pitch"].as_i64().unwrap())
        .collect();
    assert_eq!(pitches, [43, 45, 49]);
}

// ── capability gate ─────────────────────────────────────────────────────────

#[tokio::test]
async fn missing_capability_is_reported_without_touching_live() {
    let bridge = FakeBridge::responding(json!({}));
    let live = Arc::new(LiveState::with_activity(
        bridge.clone(),
        mcp_ableton_music_maker::activity::Activity::disabled(),
    ));
    live.script
        .set(mcp_ableton_music_maker::handshake::ScriptInfo {
            script_version: Some("0.9.0".into()),
            protocol_version: Some(1),
            capabilities: vec!["get_session_info".into()],
            expected_version: mcp_ableton_music_maker::handshake::expected_remote_script_version()
                .into(),
            up_to_date: false,
            error: None,
            extra: Default::default(),
        });
    let server = Server::new(live);
    let result = read(&server, 0, 0).await;
    assert!(is_error(&result));
    let text = text_of(&result);
    assert!(text.contains("cannot run `get_clip_notes`"), "{text}");
    assert!(text.contains("loaded: 0.9.0"));
    assert!(text.contains("ableton-music-maker-install-script"));
    assert!(bridge.sent().is_empty(), "no command reaches Live");
}
