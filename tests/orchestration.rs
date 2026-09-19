//! search_browser, clip settings, meters, automation, batch and build_song —
//! through the real bodies with a recording bridge.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::notes::NotesInput;
use mcp_ableton_music_maker::tools::{
    self, AutomationTarget, BatchParams, BatchStep, BuildSongParams, LoadInstrumentParams,
    PlayAndMeasureParams, Ramp, SearchBrowserParams, SetClipAutomationParams, SetClipLoopParams,
    SongClip, SongLocator, SongPlacement, SongTrack,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[tokio::test]
async fn search_lists_hits_with_uris() {
    let bridge = FakeBridge::responding(json!({
        "query": "analog bass", "category": "all", "total_matches": 2, "returned": 2, "truncated_walk": false,
        "items": [
            {"name": "Deep Sub Bass", "uri": "query:Synths#Analog:Deep", "path": "Instruments/Analog/Bass/Deep Sub Bass", "category": "instruments"},
            {"name": "Analog Bass 2", "uri": "query:Synths#Analog:Bass2", "path": "Instruments/Analog/Bass/Analog Bass 2", "category": "instruments"}
        ]
    }));
    let server = server_with(bridge.clone());
    let p = SearchBrowserParams {
        query: "analog bass".into(),
        category: "all".into(),
        limit: 30,
    };
    let r = server
        .run(&tools::SEARCH_BROWSER, p, tools::search_browser_body)
        .await;
    assert!(!is_error(&r));
    let t = text_of(&r);
    assert!(
        t.contains("2 of 2 matches") && t.contains("uri: query:Synths#Analog:Deep"),
        "{t}"
    );
    assert_eq!(bridge.sent()[0].1["query"], "analog bass");
}

#[tokio::test]
async fn load_reports_the_device_it_added() {
    let bridge = FakeBridge::responding(json!({
        "loaded": true, "item_name": "Analog", "track_name": "Bass", "uri": "query:x",
        "devices_after": ["Analog", "Compressor"], "new_devices": [{"index": 0, "name": "Analog"}],
        "loaded_device": {"index": 0, "name": "Analog"}
    }));
    let server = server_with(bridge.clone());
    let p = LoadInstrumentParams {
        track_index: 6,
        uri: "query:x".into(),
    };
    let r = server
        .run(
            &tools::LOAD_INSTRUMENT_OR_EFFECT,
            p,
            tools::load_instrument_or_effect_body,
        )
        .await;
    assert!(!is_error(&r));
    assert!(
        text_of(&r).starts_with("Loaded 'Analog' as device 0 on track 6 ('Bass')"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn clip_loop_needs_something_to_set_and_summarises() {
    let bridge = FakeBridge::responding(
        json!({"name": "Bass", "looping": true, "loop_start": 0.0, "loop_end": 8.0, "start_marker": 0.0, "end_marker": 8.0, "launch_mode_name": "trigger"}),
    );
    let server = server_with(bridge.clone());
    let empty = SetClipLoopParams {
        track_index: 1,
        clip_index: 0,
        arrangement: false,
        looping: None,
        loop_start: None,
        loop_end: None,
        start_marker: None,
        end_marker: None,
    };
    let r = server
        .run(&tools::SET_CLIP_LOOP, empty, tools::set_clip_loop_body)
        .await;
    assert!(is_error(&r) && bridge.sent().is_empty());
    let p = SetClipLoopParams {
        track_index: 1,
        clip_index: 0,
        arrangement: false,
        looping: Some(true),
        loop_start: None,
        loop_end: Some(8.0),
        start_marker: None,
        end_marker: None,
    };
    let r = server
        .run(&tools::SET_CLIP_LOOP, p, tools::set_clip_loop_body)
        .await;
    assert!(!is_error(&r));
    assert!(
        text_of(&r).contains("loop 0–8") && text_of(&r).contains("launch trigger"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn play_and_measure_reports_peaks_and_silence() {
    let bridge = FakeBridge::responding(json!({
        "is_playing": true, "song_time": 32.0,
        "tracks": [{"index": 0, "name": "Kick", "left": 0.71, "right": 0.69}, {"index": 1, "name": "Pad", "left": 0.0, "right": 0.0}],
        "returns": [{"index": 0, "name": "Reverb", "left": 0.2, "right": 0.2}],
        "master": {"name": "Master", "left": 0.8, "right": 0.8}
    }));
    let server = server_with(bridge.clone());
    let p = PlayAndMeasureParams {
        start_time: Some(32.0),
        seconds: 0.5,
        interval_ms: 50,
        stop_after: true,
    };
    let r = server
        .run(&tools::PLAY_AND_MEASURE, p, tools::play_and_measure_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = bridge.commands();
    assert_eq!(
        cmds[0], "play_from",
        "plays from the position, not the start marker"
    );
    assert_eq!(bridge.sent()[0].1["time"], 32.0);
    assert_eq!(cmds.last().unwrap(), "stop_playback");
    assert!(cmds.iter().filter(|c| *c == "get_track_meters").count() >= 3);
    let t = text_of(&r);
    assert!(
        t.contains("track 0 'Kick': 0.71") && t.contains("Silent during this stretch: Pad"),
        "{t}"
    );
}

#[tokio::test]
async fn automation_ramp_becomes_two_points_and_targets_are_checked() {
    let bridge = FakeBridge::responding(
        json!({"clip": "Pad", "target": "Auto Filter > Frequency", "points": 2, "steps_written": 128, "mode": "linear", "range": [0.0, 1.0]}),
    );
    let server = server_with(bridge.clone());
    let p = SetClipAutomationParams {
        track_index: 3,
        clip_index: 0,
        arrangement: true,
        target: AutomationTarget {
            device_index: Some(1),
            parameter_index: Some(4),
            mixer: None,
            send_index: None,
        },
        points: vec![],
        ramp: Some(Ramp {
            from: 0.2,
            to: 0.9,
            over: 32.0,
            start: 0.0,
        }),
        mode: "linear".into(),
        resolution: 0.25,
        clear: true,
    };
    let r = server
        .run(
            &tools::SET_CLIP_AUTOMATION,
            p,
            tools::set_clip_automation_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = &bridge.sent()[0].1;
    assert_eq!(
        sent["points"],
        json!([{"time": 0.0, "value": 0.2}, {"time": 32.0, "value": 0.9}])
    );
    assert_eq!(sent["arrangement"], true);
    let bad = SetClipAutomationParams {
        track_index: 3,
        clip_index: 0,
        arrangement: false,
        target: AutomationTarget::default(),
        points: vec![],
        ramp: None,
        mode: "linear".into(),
        resolution: 0.25,
        clear: true,
    };
    let r = server
        .run(
            &tools::SET_CLIP_AUTOMATION,
            bad,
            tools::set_clip_automation_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("target"));
}

#[tokio::test]
async fn batch_runs_in_order_substitutes_the_new_track_and_stops_on_error() {
    let bridge = FakeBridge::responding(json!({"index": 7, "name": "7-MIDI", "tempo": 120}));
    let server = server_with(bridge.clone());
    let p = BatchParams {
        steps: vec![
            BatchStep {
                tool: "create_midi_track".into(),
                args: json!({}),
            },
            BatchStep {
                tool: "set_track_name".into(),
                args: json!({"track_index": "$last_track", "name": "Bass"}),
            },
            BatchStep {
                tool: "create_clip".into(),
                args: json!({"track_index": "$last_track", "clip_index": 0, "steps": {"C1": "x...x..."}, "step": 0.5}),
            },
            BatchStep {
                tool: "no_such_tool".into(),
                args: json!({}),
            },
            BatchStep {
                tool: "set_tempo".into(),
                args: json!({"tempo": 120}),
            },
        ],
        stop_on_error: true,
    };
    let r = server.run(&tools::BATCH, p, tools::batch_body).await;
    assert!(is_error(&r), "the unknown tool fails the batch");
    let cmds = bridge.commands();
    assert_eq!(
        cmds,
        vec![
            "create_midi_track",
            "set_track_name",
            "create_clip",
            "add_notes_to_clip"
        ]
    );
    assert_eq!(bridge.sent()[1].1["track_index"], 7, "$last_track resolved");
    let t = text_of(&r);
    assert!(
        t.contains("1. create_midi_track ✓")
            && t.contains("4. no_such_tool ✗")
            && t.contains("1 step(s) not run"),
        "{t}"
    );

    let nested = BatchParams {
        steps: vec![BatchStep {
            tool: "batch".into(),
            args: json!({}),
        }],
        stop_on_error: true,
    };
    let r = server.run(&tools::BATCH, nested, tools::batch_body).await;
    assert!(is_error(&r) && text_of(&r).contains("cannot run inside a batch"));
}

fn song() -> BuildSongParams {
    let mut steps = BTreeMap::new();
    steps.insert("C1".to_string(), "x...x...x...x...".to_string());
    let mut sends = BTreeMap::new();
    sends.insert("Reverb".to_string(), 0.3);
    BuildSongParams {
        tempo: Some(128.0),
        tracks: vec![
            SongTrack {
                name: "Drums".into(),
                kind: "midi".into(),
                instrument: Some("query:Drums#Kit".into()),
                instrument_query: None,
                volume: Some(0.8),
                pan: None,
                color_index: Some(3),
                sends: BTreeMap::new(),
            },
            SongTrack {
                name: "Pad".into(),
                kind: "midi".into(),
                instrument: None,
                instrument_query: None,
                volume: None,
                pan: None,
                color_index: None,
                sends,
            },
        ],
        clips: vec![SongClip {
            track: "Drums".into(),
            slot: 0,
            name: "Kick".into(),
            length: 4.0,
            notes: NotesInput {
                steps,
                ..Default::default()
            },
        }],
        placements: vec![SongPlacement {
            track: "Drums".into(),
            slot: 0,
            times: vec![],
            start: Some(0.0),
            end: Some(16.0),
            step: Some(4.0),
        }],
        locators: vec![SongLocator {
            name: "Intro".into(),
            time: 0.0,
        }],
        dry_run: false,
    }
}

#[tokio::test]
async fn build_song_validates_first_and_dry_runs() {
    let bridge = FakeBridge::responding(json!({"index": 2, "name": "2-MIDI"}));
    let server = server_with(bridge.clone());
    let mut dry = song();
    dry.dry_run = true;
    let r = server
        .run(&tools::BUILD_SONG, dry, tools::build_song_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains(
            "2 track(s), 1 clip(s) with 4 notes, 4 placement(s), 1 locator(s), tempo 128"
        ),
        "{}",
        text_of(&r)
    );
    assert!(bridge.sent().is_empty(), "dry run touches nothing");

    let mut bad = song();
    bad.clips[0].track = "Nope".into();
    let r = server
        .run(&tools::BUILD_SONG, bad, tools::build_song_body)
        .await;
    assert!(is_error(&r) && text_of(&r).contains("Nope") && bridge.sent().is_empty());
}

#[tokio::test]
async fn build_song_executes_in_order() {
    let bridge = FakeBridge::responding(json!({
        "index": 2, "name": "2-MIDI", "tempo": 128.0, "loaded": true, "item_name": "Kit", "track_name": "Drums",
        "devices_after": ["Kit"], "loaded_device": {"index": 0, "name": "Kit"},
        "volume": 0.8, "panning": 0.0, "sends": [], "color_index": 3, "color": 0,
        "track": "Pad", "send_index": 0, "return_name": "Reverb", "value": 0.3,
        "clip_name": "Kick", "time": 0.0
    }));
    let server = server_with(bridge.clone());
    let r = server
        .run(&tools::BUILD_SONG, song(), tools::build_song_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = bridge.commands();
    let expected: Vec<&str> = vec![
        "set_tempo",
        "create_midi_track",
        "set_track_name",
        "load_browser_item",
        "set_track_mixer",
        "set_track_color",
        "create_midi_track",
        "set_track_name",
        "set_send",
        "create_clip",
        "set_clip_name",
        "add_notes_to_clip",
        "get_arrangement_clips",
        "duplicate_session_clip_to_arrangement",
        "duplicate_session_clip_to_arrangement",
        "duplicate_session_clip_to_arrangement",
        "duplicate_session_clip_to_arrangement",
        "create_locator",
    ];
    assert_eq!(cmds, expected);
    let t = text_of(&r);
    assert!(
        t.contains("Track 2 'Drums'")
            && t.contains("Placed track 2 slot 0 at 4 position(s)")
            && t.ends_with("hear the balance."),
        "{t}"
    );
    let _: Value = json!(null);
}

#[tokio::test]
async fn library_status_names_what_is_missing() {
    let bridge = FakeBridge::responding(json!({
        "live_version": "12.1.5", "edition_hint": "Standard or Intro-level instrument set",
        "instruments": [{"name": "Drift"}, {"name": "Simpler"}, {"name": "Drum Rack"}],
        "audio_effects": [{"name": "Reverb"}], "midi_effects": [], "packs": [{"name": "Core Library"}],
        "drums": [{"name": "Drum Hits"}], "sounds": []
    }));
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::GET_LIBRARY_STATUS,
            tools::Empty::default(),
            tools::get_library_status_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("Live 12.1.5 (Standard or Intro-level instrument set)."),
        "{t}"
    );
    assert!(
        t.contains("Not available here")
            && t.contains("Wavetable")
            && !t.contains("here (11): Drift"),
        "{t}"
    );
    assert!(
        t.contains("Packs installed (1): Core Library") && t.contains("Packs tab"),
        "{t}"
    );
}

#[tokio::test]
async fn batch_returns_whole_multi_line_results() {
    let bridge = FakeBridge::responding(
        json!({"count": 1, "returns": [{"index": 0, "letter": "A", "name": "Reverb", "volume": 0.85, "mute": false, "devices": ["Reverb"]}]}),
    );
    let server = server_with(bridge.clone());
    let p = BatchParams {
        steps: vec![BatchStep {
            tool: "get_returns".into(),
            args: json!({}),
        }],
        stop_on_error: true,
    };
    let r = server.run(&tools::BATCH, p, tools::batch_body).await;
    assert!(!is_error(&r));
    let t = text_of(&r);
    assert!(
        t.contains("\"letter\": \"A\"") && t.contains("\"devices\""),
        "full JSON kept: {t}"
    );
}

#[tokio::test]
async fn delete_arrangement_clips_all_and_by_indices() {
    let bridge = FakeBridge::responding(
        json!({"name": "x", "start_time": 0.0, "end_time": 4.0, "remaining": 0}),
    );
    bridge.script(
        "get_arrangement_clips",
        vec![json!({"clip_count": 3, "clips": []})],
    );
    let server = server_with(bridge.clone());
    let p = tools::DeleteArrangementClipParams {
        track_index: 2,
        clip_index: -1,
        clip_indices: vec![],
        all: true,
    };
    let r = server
        .run(
            &tools::DELETE_ARRANGEMENT_CLIP,
            p,
            tools::delete_arrangement_clip_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let deletes: Vec<i64> = bridge
        .sent()
        .iter()
        .filter(|(c, _)| c == "delete_arrangement_clip")
        .map(|(_, a)| a["clip_index"].as_i64().unwrap())
        .collect();
    assert_eq!(deletes, vec![2, 1, 0], "highest index first");
    assert!(text_of(&r).contains("Removed 3 Arrangement clip(s)"));
    let none = tools::DeleteArrangementClipParams {
        track_index: 2,
        clip_index: -1,
        clip_indices: vec![],
        all: false,
    };
    let r = server
        .run(
            &tools::DELETE_ARRANGEMENT_CLIP,
            none,
            tools::delete_arrangement_clip_body,
        )
        .await;
    assert!(is_error(&r));
}

#[tokio::test]
async fn add_notes_can_refresh_arrangement_copies() {
    let bridge = FakeBridge::responding(json!({"clip_name": "bass", "track_name": "Bass"}));
    bridge.script("get_clip_info", vec![json!({"name": "bass"})]);
    bridge.script("get_arrangement_clips", vec![json!({"clip_count": 3, "clips": [
        {"name": "bass", "start_time": 0.0, "end_time": 4.0}, {"name": "other", "start_time": 4.0, "end_time": 8.0}, {"name": "bass", "start_time": 8.0, "end_time": 12.0}]})]);
    let server = server_with(bridge.clone());
    let p = tools::AddNotesParams {
        track_index: 1,
        clip_index: 0,
        clear: true,
        propagate_to_arrangement: true,
        input: NotesInput {
            notes_csv: "36,0,1,100".into(),
            ..Default::default()
        },
    };
    let r = server
        .run(&tools::ADD_NOTES_TO_CLIP, p, tools::add_notes_to_clip_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = bridge.commands();
    let expected = vec![
        "clear_notes_from_clip",
        "add_notes_to_clip",
        "get_clip_info",
        "get_arrangement_clips",
        "delete_arrangement_clip",
        "delete_arrangement_clip",
        "duplicate_session_clip_to_arrangement",
        "duplicate_session_clip_to_arrangement",
    ];
    assert_eq!(cmds, expected);
    let deletes: Vec<i64> = bridge
        .sent()
        .iter()
        .filter(|(c, _)| c == "delete_arrangement_clip")
        .map(|(_, a)| a["clip_index"].as_i64().unwrap())
        .collect();
    assert_eq!(deletes, vec![2, 0]);
    assert!(
        text_of(&r).contains("Refreshed 2 Arrangement copies of 'bass' at beat(s) 0, 8"),
        "{}",
        text_of(&r)
    );
}
