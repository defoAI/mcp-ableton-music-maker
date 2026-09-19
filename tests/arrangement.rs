//! Fewer round-trips: create_clip that names and fills, multi-placement
//! duplicate_to_arrangement, the compact snapshot, the clear flag, and the
//! track index in create_*_track — all through the real tool bodies with a
//! recording bridge.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::notes::NotesInput;
use mcp_ableton_music_maker::tools::{
    self, AddNotesParams, CreateClipParams, CreateTrackParams, DuplicateToArrangementParams,
    SnapshotParams,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn steps(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn create_clip_names_and_fills_in_one_call() {
    let bridge = FakeBridge::responding(json!({}));
    let server = server_with(bridge.clone());
    let p = CreateClipParams {
        track_index: 2,
        clip_index: 0,
        length: 4.0,
        name: "Kick".into(),
        input: NotesInput {
            steps: steps(&[("36", "x...x...x...x...")]),
            ..Default::default()
        },
    };
    let r = server
        .run(&tools::CREATE_CLIP, p, tools::create_clip_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        bridge.commands(),
        vec!["create_clip", "set_clip_name", "add_notes_to_clip"]
    );
    let sent = bridge.sent();
    assert_eq!(sent[1].1["name"], "Kick");
    assert_eq!(sent[2].1["notes"].as_array().unwrap().len(), 4);
    assert_eq!(sent[2].1["notes"][1]["start_time"], 1.0);
    assert!(text_of(&r).contains("named 'Kick'") && text_of(&r).contains("4 notes"));
}

#[tokio::test]
async fn create_clip_with_a_bad_pattern_touches_nothing() {
    let bridge = FakeBridge::responding(json!({}));
    let server = server_with(bridge.clone());
    let p = CreateClipParams {
        track_index: 2,
        clip_index: 0,
        length: 4.0,
        name: String::new(),
        input: NotesInput {
            steps: steps(&[("36", "x..?")]),
            ..Default::default()
        },
    };
    let r = server
        .run(&tools::CREATE_CLIP, p, tools::create_clip_body)
        .await;
    assert!(is_error(&r));
    assert!(bridge.commands().is_empty(), "nothing reached Live");
}

#[tokio::test]
async fn add_notes_clear_flag_replaces_instead_of_appending() {
    let bridge = FakeBridge::responding(json!({"cleared_count": 12, "clip_name": "Bass"}));
    let server = server_with(bridge.clone());
    let p = AddNotesParams {
        track_index: 1,
        clip_index: 0,
        clear: true,
        propagate_to_arrangement: false,
        input: NotesInput {
            notes_csv: "C1,0,0.5,110\nC1,2,0.5,100".into(),
            loop_every: 4.0,
            until: 16.0,
            ..Default::default()
        },
    };
    let r = server
        .run(&tools::ADD_NOTES_TO_CLIP, p, tools::add_notes_to_clip_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        bridge.commands(),
        vec!["clear_notes_from_clip", "add_notes_to_clip"]
    );
    let notes = bridge.sent()[1].1["notes"].as_array().unwrap().clone();
    assert_eq!(notes.len(), 8, "two notes tiled over four bars");
    assert_eq!(notes.last().unwrap()["start_time"], 14.0);
    assert!(text_of(&r).contains("cleared 12 first"), "{}", text_of(&r));
}

#[tokio::test]
async fn add_notes_without_any_form_is_an_error_before_live() {
    let bridge = FakeBridge::responding(json!({}));
    let server = server_with(bridge.clone());
    let p = AddNotesParams {
        track_index: 1,
        clip_index: 0,
        clear: false,
        propagate_to_arrangement: false,
        input: NotesInput::default(),
    };
    let r = server
        .run(&tools::ADD_NOTES_TO_CLIP, p, tools::add_notes_to_clip_body)
        .await;
    assert!(is_error(&r));
    assert!(bridge.commands().is_empty());
}

#[tokio::test]
async fn duplicate_places_a_range_in_one_call() {
    let bridge = FakeBridge::responding(json!({"clip_name": "Kick", "track_name": "Drums"}));
    let server = server_with(bridge.clone());
    let p = DuplicateToArrangementParams {
        track_index: 0,
        clip_index: 0,
        destination_time: None,
        destination_times: vec![],
        start: Some(32.0),
        end: Some(96.0),
        step: Some(4.0),
        at_bar: None,
        until_bar: None,
        every_bars: None,
    };
    let r = server
        .run(
            &tools::DUPLICATE_TO_ARRANGEMENT,
            p,
            tools::duplicate_to_arrangement_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent: Vec<Value> = bridge
        .sent()
        .into_iter()
        .filter(|(c, _)| c == "place_clips")
        .map(|(_, a)| a)
        .collect();
    assert_eq!(sent.len(), 1, "every copy in one round trip");
    let times = sent[0]["times"].as_array().unwrap();
    assert_eq!(times.len(), 16);
    assert_eq!(times[0], 32.0);
    assert_eq!(times[15], 92.0);
    assert_eq!(
        bridge.commands()[0],
        "get_arrangement_clips",
        "overlap check first"
    );
    assert!(text_of(&r).contains("16 times"), "{}", text_of(&r));
}

#[tokio::test]
async fn duplicate_single_and_list_forms_still_work() {
    let bridge = FakeBridge::responding(json!({"clip_name": "Kick", "track_name": "Drums"}));
    let server = server_with(bridge.clone());
    let p = DuplicateToArrangementParams {
        track_index: 0,
        clip_index: 0,
        destination_time: Some(0.0),
        destination_times: vec![8.0, 16.0],
        start: None,
        end: None,
        step: None,
        at_bar: None,
        until_bar: None,
        every_bars: None,
    };
    let r = server
        .run(
            &tools::DUPLICATE_TO_ARRANGEMENT,
            p,
            tools::duplicate_to_arrangement_body,
        )
        .await;
    assert!(!is_error(&r));
    let times: Vec<Value> = bridge
        .sent()
        .iter()
        .filter(|(c, _)| c == "place_clips")
        .flat_map(|(_, a)| a["times"].as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(times, vec![json!(0.0), json!(8.0), json!(16.0)]);
    assert!(text_of(&r).contains("0, 8, 16"), "{}", text_of(&r));

    let none = DuplicateToArrangementParams {
        track_index: 0,
        clip_index: 0,
        destination_time: None,
        destination_times: vec![],
        start: Some(0.0),
        end: None,
        step: None,
        at_bar: None,
        until_bar: None,
        every_bars: None,
    };
    let r = server
        .run(
            &tools::DUPLICATE_TO_ARRANGEMENT,
            none,
            tools::duplicate_to_arrangement_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(text_of(&r).contains("all three"));
}

#[tokio::test]
async fn duplicate_reports_how_far_it_got_on_failure() {
    let bridge = FakeBridge::responding(json!({"clip_name": "Kick", "track_name": "Drums"}));
    let server = server_with(bridge.clone());
    // Succeed twice, then Live refuses.
    let p = DuplicateToArrangementParams {
        track_index: 0,
        clip_index: 0,
        destination_time: None,
        destination_times: vec![0.0, 4.0, 8.0],
        start: None,
        end: None,
        step: None,
        at_bar: None,
        until_bar: None,
        every_bars: None,
    };
    // Call 0 is the overlap read; call 1 places every copy in one round trip
    // and the script reports the ones Live refused.
    bridge.script(
        "place_clips",
        vec![
            json!({"track": "Drums", "clip": "Kick", "length": 4.0, "placed": [0.0, 4.0],
                    "failed": [{"time": 8.0, "error": "Track is frozen"}], "arrangement_clips": 2}),
        ],
    );
    let r = server
        .run(
            &tools::DUPLICATE_TO_ARRANGEMENT,
            p,
            tools::duplicate_to_arrangement_body,
        )
        .await;
    assert!(is_error(&r));
    let t = text_of(&r);
    assert!(
        t.contains("Placed 2 of 3, but 1 could not be placed"),
        "{t}"
    );
    assert!(t.contains("beat 8") && t.contains("Track is frozen"), "{t}");
    assert_eq!(bridge.sent().len(), 2, "one read, one placement round trip");
}

#[tokio::test]
async fn snapshot_is_compact_by_default() {
    let raw = json!({
        "schema": "ableton_mcp_snapshot_v2",
        "tracks": [{
            "index": 0, "name": "Drums",
            "clip_slots": [
                {"index": 0, "has_clip": true, "clip": {"name": "Kick"}},
                {"index": 1, "has_clip": false, "clip": null},
                {"index": 2, "has_clip": false, "clip": null}
            ],
            "arrangement_clips": []
        }],
        "scenes": [{"index": 0}, {"index": 1}]
    });
    let bridge = FakeBridge::responding(raw.clone());
    let server = server_with(bridge.clone());
    let p = SnapshotParams {
        include_notes: true,
        include_params: true,
        compact: true,
        include_scenes: false,
    };
    let r = server
        .run(
            &tools::GET_SESSION_SNAPSHOT,
            p,
            tools::get_session_snapshot_body,
        )
        .await;
    let out: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(out["tracks"][0]["clip_slots"].as_array().unwrap().len(), 1);
    assert_eq!(out["tracks"][0]["slot_count"], 3);
    assert!(out["tracks"][0].get("arrangement_clips").is_none());
    assert!(out.get("scenes").is_none());
    assert_eq!(out["scene_count"], 2);
    assert_eq!(out["compact"], true);
    assert!(text_of(&r).len() < serde_json::to_string_pretty(&raw).unwrap().len());

    let full = SnapshotParams {
        include_notes: true,
        include_params: true,
        compact: false,
        include_scenes: false,
    };
    let r = server
        .run(
            &tools::GET_SESSION_SNAPSHOT,
            full,
            tools::get_session_snapshot_body,
        )
        .await;
    let out: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(out["tracks"][0]["clip_slots"].as_array().unwrap().len(), 3);
    assert_eq!(out["scenes"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn schema_defaults_make_snapshot_compact_and_notes_optional() {
    let server = server_with(FakeBridge::responding(json!({})));
    let tools = server.tool_list();
    let snap = tools
        .iter()
        .find(|t| t.name == "adv_get_session_snapshot")
        .unwrap();
    let schema = serde_json::to_value(&snap.input_schema).unwrap();
    assert_eq!(schema["properties"]["compact"]["default"], true, "{schema}");
    let add = tools
        .iter()
        .find(|t| t.name == "add_notes_to_clip")
        .unwrap();
    let schema = serde_json::to_value(&add.input_schema).unwrap();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        !required.contains(&"notes"),
        "notes is one of several forms: {required:?}"
    );
    for form in [
        "notes_csv",
        "steps",
        "patterns",
        "loop_every",
        "until",
        "clear",
    ] {
        assert!(
            schema["properties"].get(form).is_some(),
            "missing {form}: {schema}"
        );
    }
}

#[tokio::test]
async fn created_track_reports_its_index() {
    let bridge = FakeBridge::responding(json!({"index": 6, "name": "6-MIDI"}));
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::CREATE_MIDI_TRACK,
            CreateTrackParams { index: -1 },
            tools::create_midi_track_body,
        )
        .await;
    assert!(!is_error(&r));
    assert!(
        text_of(&r).starts_with("Created MIDI track 6 ('6-MIDI')"),
        "{}",
        text_of(&r)
    );
    assert!(text_of(&r).contains("track_index 6"));
}
