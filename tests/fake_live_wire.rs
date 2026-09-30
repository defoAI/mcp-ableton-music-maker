//! The wire itself: a Rust suite against the fake Live, end to end.
//!
//! Every other converted suite is built on what this one proves — that a
//! `Server` wired through a real `AbletonConnection` to
//! `scripts/fake-live.py` runs the real Remote Script against a real set,
//! and that the set can be read back to assert against.
//!
//! With `ABLETON_TARGET=live` this runs against a real Ableton Live, which
//! is what makes the whole converted suite a real-Live verification pass.

mod common;

use common::{is_error, server_on_fake_live, text_of, LiveSet};
use mcp_ableton_music_maker::notes::NotesInput;
use mcp_ableton_music_maker::tools::{
    self, AddNotesParams, CreateClipParams, Note, SetTempoParams,
};

fn note(pitch: i64, start_time: f64) -> Note {
    Note {
        pitch,
        start_time,
        duration: 1.0,
        velocity: 100,
        mute: false,
        extra: Default::default(),
    }
}

#[tokio::test]
async fn the_server_reaches_the_script_and_the_script_reaches_the_set() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());

    let before = set.tempo();
    let result = server
        .run(
            &tools::SET_TEMPO,
            SetTempoParams { tempo: 126.5 },
            tools::set_tempo_body,
        )
        .await;
    assert!(!is_error(&result), "{}", text_of(&result));

    // Not the tool's own words: Live's, asked afterwards.
    assert_eq!(
        set.tempo(),
        126.5,
        "the tempo was {before} and did not move"
    );
    assert!(
        bridge.commands().contains(&"set_tempo".to_string()),
        "the wire did not carry set_tempo: {:?}",
        bridge.commands()
    );
}

#[tokio::test]
async fn notes_written_through_the_tool_read_back_out_of_the_clip() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());

    let created = server
        .run(
            &tools::CREATE_CLIP,
            CreateClipParams {
                track_index: Some(0),
                clip_index: Some(0),
                length: 4.0,
                ..Default::default()
            },
            tools::create_clip_body,
        )
        .await;
    assert!(!is_error(&created), "{}", text_of(&created));

    let added = server
        .run(
            &tools::ADD_NOTES_TO_CLIP,
            AddNotesParams {
                track_index: Some(0),
                clip_index: Some(0),
                input: NotesInput {
                    notes: vec![note(60, 0.0), note(64, 1.0)],
                    ..Default::default()
                },
                ..Default::default()
            },
            tools::add_notes_to_clip_body,
        )
        .await;
    assert!(!is_error(&added), "{}", text_of(&added));

    assert_eq!(
        set.clip_pitches(0, 0),
        vec![60, 64],
        "the notes are not in the clip"
    );
}

#[tokio::test]
async fn every_test_gets_a_live_of_its_own() {
    // Two `server_on_fake_live()` calls are two processes, each with one
    // set — which is what Live is. Nothing a test builds can reach another.
    let (_a, bridge_a) = server_on_fake_live();
    let (_b, bridge_b) = server_on_fake_live();
    let set_a = LiveSet::of(bridge_a.as_ref());
    let set_b = LiveSet::of(bridge_b.as_ref());

    if common::targets_a_real_live() {
        // A real Live is one Live. That is the point of the flag, and why
        // that run is single-threaded.
        assert_eq!(set_a.track_count(), set_b.track_count());
        return;
    }
    let before = set_b.track_count();
    set_a.build(&[("Only in A", "midi", "")]);
    assert_eq!(set_a.track_names().last().unwrap(), "Only in A");
    assert_eq!(
        set_b.track_count(),
        before,
        "one test's work showed up in another's set"
    );
    assert!(!set_b.track_names().contains(&"Only in A".to_string()));
}

// ── update_song against the real Remote Script ──────────────────────────────

/// The scene names Live actually has, asked of Live rather than of the tool.
fn scene_names(set: &LiveSet) -> Vec<String> {
    set.context()["scenes"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|s| s["name"].as_str().unwrap_or("").to_string())
        .collect()
}

async fn update(server: &tools::Server, p: serde_json::Value) -> rmcp::model::CallToolResult {
    server
        .run(
            &tools::UPDATE_SONG,
            serde_json::from_value(p).expect("update_song parameters"),
            mcp_ableton_music_maker::sections::update_song_body,
        )
        .await
}

#[tokio::test]
async fn update_song_renames_deletes_and_keeps_the_song_in_step_in_a_real_set() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Kick", "midi", ""), ("Bass", "midi", "")]);
    for (slot, name) in [(0, "Intro · 8"), (1, "Break · 16"), (2, "Drop · 8")] {
        set.ask(
            "set_scene",
            serde_json::json!({"index": slot, "name": name}),
        );
        set.write_clip(
            0,
            slot,
            &format!("{name}/Kick"),
            serde_json::json!([{"pitch": 36, "start_time": 0.0, "duration": 0.25, "velocity": 100}]),
        );
    }
    let r = server
        .run(
            &tools::SET_SONG,
            mcp_ableton_music_maker::sections::SetSongParams {
                setlist: mcp_ableton_music_maker::song::parse_setlist(
                    "Setlist: Intro×2 → Break → Drop",
                )
                .unwrap(),
            },
            mcp_ableton_music_maker::sections::set_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));

    let r = update(
        &server,
        serde_json::json!({"edits": [
            {"edit": "set_section", "section": "Break", "rename_to": "Bridge", "phrase_bars": 24},
            {"edit": "delete_section", "section": "Intro"}
        ]}),
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));

    // Live's own answer, not the tool's words.
    let names = scene_names(&set);
    assert!(
        names.iter().any(|n| n == "Bridge · 24"),
        "the row was not renamed: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.starts_with("Intro")),
        "the row was not deleted: {names:?}"
    );
    let setlist = names
        .iter()
        .find(|n| n.starts_with("Setlist:"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        setlist, "Setlist: Bridge → Drop",
        "the song did not follow the rename and the delete"
    );
    // The row went through the generic layer, so no new Remote Script
    // command and no reinstall.
    assert!(
        bridge.commands().contains(&"run".to_string()),
        "delete_scene did not go through run: {:?}",
        bridge.commands()
    );

    // A deleted row moves every row under it, so nothing the server
    // remembered about a clip by its slot is true any more: the next read
    // asks Live for the notes again rather than serving what it had.
    bridge.clear();
    let after = server
        .run(
            &tools::GET_CONTEXT,
            tools::GetContextParams {
                as_level: Some("document".into()),
                ..Default::default()
            },
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&after), "{}", text_of(&after));
    assert_eq!(
        bridge.last("get_session_snapshot").unwrap()["include_notes"],
        serde_json::json!(true),
        "the cache survived a row moving under it"
    );
}

#[tokio::test]
async fn the_second_read_of_an_unchanged_set_re_reads_no_notes() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Kick", "midi", ""), ("Bass", "midi", "")]);
    set.write_clip(
        0,
        0,
        "Intro/Kick",
        serde_json::json!([{"pitch": 36, "start_time": 0.0, "duration": 0.25, "velocity": 100},
                           {"pitch": 36, "start_time": 1.0, "duration": 0.25, "velocity": 100}]),
    );
    let document = tools::GetContextParams {
        as_level: Some("document".into()),
        ..Default::default()
    };

    bridge.clear();
    let first = server
        .run(
            &tools::GET_CONTEXT,
            document.clone(),
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&first), "{}", text_of(&first));
    let asked = bridge
        .last("get_session_snapshot")
        .expect("the first read asks Live for the set");
    assert_eq!(asked["include_notes"], serde_json::json!(true));
    assert!(text_of(&first).contains("\"36\""), "{}", text_of(&first));

    // Nothing changed in between: the notes are not read again, and the
    // snapshot is asked for without them — which is the expensive half.
    bridge.clear();
    let second = server
        .run(
            &tools::GET_CONTEXT,
            document.clone(),
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&second), "{}", text_of(&second));
    let asked = bridge.last("get_session_snapshot").unwrap();
    assert_eq!(
        asked["include_notes"],
        serde_json::json!(false),
        "the set was read again with its notes"
    );
    assert!(
        !bridge.commands().contains(&"get_clip_notes".to_string()),
        "a clip nobody touched was read again: {:?}",
        bridge.commands()
    );
    let t = text_of(&second);
    assert!(t.contains("no notes re-read"), "{t}");
    assert!(
        t.contains("\"36\""),
        "the cached notes are in the document: {t}"
    );

    // A clip edited by hand in Live is read again, and only that one.
    set.write_clip(
        1,
        0,
        "Intro/Bass",
        serde_json::json!([{"pitch": 41, "start_time": 0.0, "duration": 1.0, "velocity": 90}]),
    );
    bridge.clear();
    let third = server
        .run(&tools::GET_CONTEXT, document, tools::get_context_body)
        .await;
    assert!(!is_error(&third), "{}", text_of(&third));
    let t = text_of(&third);
    assert!(
        t.contains("\"41\""),
        "the hand-written clip is missing: {t}"
    );
    let note_reads = bridge
        .commands()
        .iter()
        .filter(|c| *c == "get_clip_notes")
        .count();
    assert!(
        note_reads <= 1,
        "more than the changed clip was read: {note_reads}"
    );
}
