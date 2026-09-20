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
                track_index: 0,
                clip_index: 0,
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
                track_index: 0,
                clip_index: 0,
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
async fn every_connection_gets_a_set_of_its_own_so_these_tests_do_not_collide() {
    let (_a, bridge_a) = server_on_fake_live();
    let (_b, bridge_b) = server_on_fake_live();
    let set_a = LiveSet::of(bridge_a.as_ref());
    let set_b = LiveSet::of(bridge_b.as_ref());

    if common::targets_a_real_live() {
        // A real Live is one set. That is the point of the flag.
        assert_eq!(set_a.track_count(), set_b.track_count());
        return;
    }
    let before = set_b.track_count();
    let _ = set_a.session();
    assert_eq!(
        set_b.track_count(),
        before,
        "one connection's work showed up in another's set"
    );
}
