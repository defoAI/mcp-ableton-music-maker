//! The artist's surface from the review of 2026-09-19: `arrange` in bars,
//! one `feel` tool with one undo, `set_key`, `create_return`,
//! `clear_captures`, faders in dB, the raw layer marked as advanced and
//! annotated, and the claims the review caught (Arrangement automation).

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::arrange::{
    ArrangeParams, ClearCapturesParams, CreateReturnParams, FeelParams, SetKeyParams,
};
use mcp_ableton_music_maker::tools::{
    self, AutomationTarget, LoadInstrumentParams, Ramp, SetClipAutomationParams,
    SetTrackMixerParams, CORE_TOOLS,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn state() -> Value {
    json!({
        "is_playing": false, "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
        "beat": 0.0, "bar": 1, "beat_in_bar": 1, "clip_trigger_quantization": 4,
        "tracks": [
            {"index": 0, "name": "Kick", "playing_slot_index": -1, "slots_with_clips": [0, 1]},
            {"index": 1, "name": "Congas", "playing_slot_index": -1, "slots_with_clips": [1]}
        ],
        "scenes": [{"index": 0, "name": "Intro · 8", "clip_tracks": [0]}, {"index": 1, "name": "Groove · 8", "clip_tracks": [0, 1]}],
        "cues": [], "events": []
    })
}

fn arrangement(clips: Vec<(&str, f64, f64)>) -> Value {
    json!({"track_index": 0, "track_name": "Kick", "clip_count": clips.len(),
           "clips": clips.iter().map(|(n, s, e)| json!({"name": n, "start_time": s, "end_time": e, "length": e - s})).collect::<Vec<_>>()})
}

fn bridge() -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script("get_performance_state", vec![state()]);
    b.script(
        "get_clip_info",
        vec![json!({"name": "Kick", "length": 8.0})],
    );
    b.script("place_clips", vec![json!({"track": "Kick", "clip": "Kick", "length": 8.0, "placed": [0.0, 8.0, 16.0, 24.0], "failed": [], "arrangement_clips": 4})]);
    b.script(
        "get_arrangement_clips",
        vec![arrangement(vec![
            ("Kick", 0.0, 8.0),
            ("Kick", 8.0, 16.0),
            ("Kick", 16.0, 24.0),
            ("Kick", 24.0, 32.0),
        ])],
    );
    b.script("duplicate_arrangement_clip", vec![json!({"track": "Kick", "clip": "Kick", "length": 8.0, "placed": [32.0, 40.0], "failed": [], "arrangement_clips": 6})]);
    b.script("delete_arrangement_clips", vec![json!({"track": "Kick", "removed": [{"index": 2, "name": "Kick", "start_time": 16.0, "end_time": 24.0}, {"index": 3, "name": "Kick", "start_time": 24.0, "end_time": 32.0}], "remaining": 2})]);
    b
}

fn arrange(action: &str) -> ArrangeParams {
    ArrangeParams {
        action: action.into(),
        track: Some(json!("Kick")),
        ..Default::default()
    }
}

#[tokio::test]
async fn arrange_places_repeats_moves_deletes_and_lists_in_bars_one_round_trip_per_track() {
    let b = bridge();
    let server = server_with(b.clone());
    // place: bars 1 to 9, every clip length (2 bars): four copies in one command
    let r = server
        .run(
            &tools::ARRANGE,
            ArrangeParams {
                clip: Some(json!(0)),
                at_bar: Some(json!(1.0)),
                until_bar: Some(json!(9.0)),
                ..arrange("place")
            },
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec!["get_performance_state", "get_clip_info", "place_clips"]
    );
    assert_eq!(
        b.sent()[2].1,
        json!({"track_index": 0, "clip_index": 0, "times": [0.0, 8.0, 16.0, 24.0]})
    );
    assert_eq!(
        text_of(&r),
        "Placed 'Kick' (2 bars) on Kick 4 times from bar 1 to bar 9 — one round trip."
    );

    // repeat the third clip twice after itself
    let r = server
        .run(
            &tools::ARRANGE,
            ArrangeParams {
                clip: Some(json!(2)),
                times: Some(2),
                ..arrange("repeat")
            },
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.sent().last().unwrap().1,
        json!({"track_index": 0, "clip_index": 2, "times": [24.0, 32.0]})
    );
    assert_eq!(
        text_of(&r),
        "Repeated 'Kick' (2 bars) on Kick 2 times: bars 7 to 11 — one round trip."
    );

    // move clip 1 to bar 17: a copy there, the original removed
    let r = server
        .run(
            &tools::ARRANGE,
            ArrangeParams {
                clip: Some(json!(1)),
                at_bar: Some(json!(17.0)),
                ..arrange("move")
            },
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = b.sent();
    assert_eq!(sent[sent.len() - 2].1["times"], json!([64.0]));
    assert_eq!(
        sent[sent.len() - 1],
        (
            "delete_arrangement_clips".to_string(),
            json!({"track_index": 0, "indices": [1]})
        )
    );
    assert_eq!(
        text_of(&r),
        "Moved 'Kick' on Kick from bar 3 to bar 17 (2 bars)."
    );

    // delete bars 5–9 on Kick
    let r = server
        .run(
            &tools::ARRANGE,
            ArrangeParams {
                from_bar: Some(json!(5.0)),
                to_bar: Some(json!(9.0)),
                ..arrange("delete")
            },
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.sent().last().unwrap().1,
        json!({"track_index": 0, "from_beat": 16.0, "to_beat": 32.0})
    );
    assert_eq!(
        text_of(&r),
        "Deleted 2 Arrangement clips starting in bars 5–9 (Kick 2)."
    );

    // shorten the whole arrangement to end at bar 5: every track, one command each
    b.script(
        "get_arrangement_clips",
        vec![arrangement(vec![("Kick", 0.0, 8.0), ("Long", 8.0, 40.0)])],
    );
    let r = server
        .run(
            &tools::ARRANGE,
            ArrangeParams {
                action: "shorten".into(),
                to_bar: Some(json!(5.0)),
                ..Default::default()
            },
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let deletes: Vec<Value> = b
        .sent()
        .iter()
        .rev()
        .take(4)
        .filter(|(c, _)| c == "delete_arrangement_clips")
        .map(|(_, p)| p.clone())
        .collect();
    assert_eq!(deletes.len(), 2, "both tracks in one command each");
    assert!(deletes
        .iter()
        .all(|d| d["from_beat"] == 16.0 && d.get("to_beat").is_none()));
    let t = text_of(&r);
    assert!(
        t.starts_with("Arrangement shortened to end at bar 5: 4 clips removed (Kick 2, Congas 2)."),
        "{t}"
    );
    assert!(t.contains("Live's API cannot trim a clip, so 'Long' on Kick (bars 3–11), 'Long' on Congas (bars 3–11) still run past that bar"), "{t}");

    // list
    let r = server
        .run(
            &tools::ARRANGE,
            ArrangeParams {
                action: "list".into(),
                ..Default::default()
            },
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Kick: 0 'Kick' bars 1–3, 1 'Long' bars 3–11"),
        "{}",
        text_of(&r)
    );
    assert!(
        text_of(&r).ends_with(
            "4 Arrangement clips (bars are Live's 1-based bars; end bars are exclusive)."
        ),
        "{}",
        text_of(&r)
    );

    // bars start at 1; an unknown action names the six
    let r = server
        .run(
            &tools::ARRANGE,
            ArrangeParams {
                clip: Some(json!(0)),
                at_bar: Some(json!(0.0)),
                ..arrange("place")
            },
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("bar 0 is before bar 1"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::ARRANGE,
            arrange("slide"),
            mcp_ableton_music_maker::arrange::arrange_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("place, repeat, move, delete, shorten or list"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn feel_is_one_tool_with_one_undo() {
    let b = bridge();
    b.script("get_clip_notes", vec![json!({"notes": (0..16).map(|i| json!({"pitch": 60, "start_time": i as f64 * 0.25, "duration": 0.2, "velocity": 100.0, "mute": false, "note_id": i + 1})).collect::<Vec<_>>()})]);
    b.script("get_grooves", vec![json!({"groove_amount": 1.0, "grooves": [{"index": 0, "name": "Swing 16", "timing_amount": 1.0}]})]);
    b.script("set_clip_groove", vec![json!({"track": "Congas", "clip": "x", "groove": {"index": 0, "name": "Swing 16", "timing_amount": 1.0}})]);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::FEEL,
            FeelParams {
                track: json!("Congas"),
                clip: 1,
                swing: Some(0.33),
                humanize_ms: Some(10.0),
                groove: Some(json!("Swing 16")),
                seed: 2,
                ..Default::default()
            },
            mcp_ableton_music_maker::arrange::feel_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands();
    assert_eq!(
        cmds.iter().filter(|c| *c == "add_notes_to_clip").count(),
        2,
        "swing then humanize rewrite the clip"
    );
    assert!(cmds.contains(&"set_clip_groove".to_string()));
    let t = text_of(&r);
    assert!(
        t.contains("swung") && t.contains("humanized") && t.contains("groove 'Swing 16'"),
        "{t}"
    );
    assert!(
        t.ends_with("feel {\"undo\": true} puts the clip back as it was before this call."),
        "{t}"
    );
    // one undo for the whole call: the notes as first read
    let r = server
        .run(
            &tools::FEEL,
            FeelParams {
                track: json!("Congas"),
                clip: 1,
                undo: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::arrange::feel_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let restored = b.sent().last().unwrap().1["notes"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(restored.len(), 16);
    assert_eq!(restored[1]["start_time"], 0.25, "back on the grid");
    // nothing asked
    let r = server
        .run(
            &tools::FEEL,
            FeelParams {
                track: json!("Congas"),
                clip: 1,
                ..Default::default()
            },
            mcp_ableton_music_maker::arrange::feel_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).starts_with("Say what to change: swing, humanize_ms, groove"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn set_key_create_return_clear_captures_and_faders_in_db() {
    let b = bridge();
    b.script("set_scale", vec![json!({"root_note": 5, "root_note_name": "F", "scale_name": "Minor", "scale_mode": true})]);
    b.script(
        "create_return_track",
        vec![json!({"index": 2, "letter": "C", "name": "C-Verb", "return_count": 3})],
    );
    b.script(
        "load_browser_item",
        vec![json!({"loaded": true, "loaded_device": {"name": "Reverb", "index": 0}})],
    );
    b.script("set_track_mixer", vec![json!({"name": "Kick", "volume": 0.7, "volume_db": -6.0, "panning": 0.0, "mute": false, "sends": []})]);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SET_KEY,
            SetKeyParams {
                key: "F minor".into(),
            },
            mcp_ableton_music_maker::arrange::set_key_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.sent()[0],
        (
            "set_scale".to_string(),
            json!({"root_note": 5, "scale_name": "Minor"})
        )
    );
    assert!(
        text_of(&r).starts_with("Key F Minor: set in Live's scale settings"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::SET_KEY,
            SetKeyParams {
                key: "techno".into(),
            },
            mcp_ableton_music_maker::arrange::set_key_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("'techno' is not a key I can read"),
        "{}",
        text_of(&r)
    );

    // a return with a reverb on it, by words: the index goes into the library search
    let r = server
        .run(
            &tools::CREATE_RETURN,
            CreateReturnParams {
                name: Some("Verb".into()),
                effect: Some("query:AudioFx#Reverb".into()),
            },
            mcp_ableton_music_maker::arrange::create_return_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = b.sent();
    let n = sent.len();
    assert_eq!(sent[n - 2].0, "create_return_track");
    assert_eq!(
        sent[n - 1].1,
        json!({"track_index": 2, "item_uri": "query:AudioFx#Reverb", "kind": "return"})
    );
    assert!(
        text_of(&r).starts_with(
            "Return C 'C-Verb' created (return 2). set_send {\"send_name\": \"C\"} feeds it; "
        ),
        "{}",
        text_of(&r)
    );

    // an effect on the master by kind
    let r = server
        .run(
            &tools::LOAD_INSTRUMENT_OR_EFFECT,
            LoadInstrumentParams {
                track_index: Some(0),
                uri: "query:AudioFx#Limiter".into(),
                kind: "master".into(),
                ..Default::default()
            },
            tools::load_instrument_or_effect_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.sent().last().unwrap().1["kind"], "master");

    // faders in dB
    let r = server
        .run(
            &tools::SET_TRACK_MIXER,
            SetTrackMixerParams {
                track_index: Some(0),
                kind: "track".into(),
                volume: Some(-6.0),
                volume_db: None,
                fader: None,
                pan: None,
                mute: None,
                solo: None,
                arm: None,
                ..Default::default()
            },
            tools::set_track_mixer_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.sent().last().unwrap().1["volume_db"], -6.0);
    assert!(
        text_of(&r).starts_with("'Kick' now: volume -6.0 dB, pan 0"),
        "{}",
        text_of(&r)
    );

    // clear_captures removes the Capture track
    b.script("get_context", vec![json!({"tracks": [
        {"index": 0, "name": "Kick", "kind": "midi", "clips": []},
        {"index": 3, "name": "Capture", "kind": "audio", "clips": [{"slot": 0, "name": "peak @ bar 33 | -3.3 dBFS"}, {"slot": 1, "name": "x"}]}]})]);
    let r = server
        .run(
            &tools::CLEAR_CAPTURES,
            ClearCapturesParams { keep_track: false },
            mcp_ableton_music_maker::arrange::clear_captures_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.sent().last().unwrap(),
        &("delete_track".to_string(), json!({"track_index": 3}))
    );
    assert!(
        text_of(&r).starts_with("Removed the Capture track (track 3) and its 2 captures"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::CLEAR_CAPTURES,
            ClearCapturesParams { keep_track: true },
            mcp_ableton_music_maker::arrange::clear_captures_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands()
            .iter()
            .rev()
            .take(2)
            .filter(|c| *c == "delete_clip")
            .count(),
        2
    );
}

#[tokio::test]
async fn arrangement_automation_is_refused_before_live_and_the_surface_is_marked() {
    let b = bridge();
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SET_CLIP_AUTOMATION,
            SetClipAutomationParams {
                track_index: 0,
                clip_index: 0,
                arrangement: true,
                target: AutomationTarget {
                    device_index: Some(0),
                    parameter_index: Some(3),
                    mixer: None,
                    send_index: None,
                },
                points: vec![],
                ramp: Some(Ramp {
                    from: 0.2,
                    to: 0.9,
                    over: 16.0,
                    start: 0.0,
                }),
                mode: "linear".into(),
                resolution: 0.25,
                clear: true,
            },
            tools::set_clip_automation_body,
        )
        .await;
    assert!(
        is_error(&r)
            && text_of(&r).starts_with("Live's API writes automation into Session clips only"),
        "{}",
        text_of(&r)
    );
    assert!(b.commands().is_empty());

    let tools_list = server.tool_list();
    for n in CORE_TOOLS {
        assert!(
            tools_list.iter().any(|t| t.name == *n),
            "{n} is served under its own name"
        );
    }
    for n in [
        "fire_clip",
        "fire_scene",
        "cue",
        "create_scene",
        "set_scene",
        "get_session_snapshot",
        "start_performance",
        "listen",
        "vary_clip",
        "undo_vary",
        "groove_clip",
        "humanize",
        "swing_notes",
        "retime_clip",
        "duplicate_to_arrangement",
        "delete_arrangement_clip",
        "get_remote_script_info",
        // Editing a device chain is the raw layer: the artist verbs are
        // load_instrument_or_effect and shape_sound (decision 0006).
        "edit_devices",
        "get_device_parameters",
        "set_device_parameter",
    ] {
        assert!(
            !tools_list.iter().any(|t| t.name == n),
            "{n} is not served bare"
        );
        assert!(
            tools_list.iter().any(|t| t.name == format!("adv_{n}")),
            "{n} is served as adv_{n}"
        );
    }
    let by_name = |n: &str| {
        tools_list
            .iter()
            .find(|t| t.name == n || t.name == format!("adv_{n}"))
            .unwrap()
            .clone()
    };
    let lib = by_name("get_library_status");
    assert!(
        lib.description
            .as_deref()
            .unwrap_or("")
            .contains("What this Live can actually use")
            && !lib
                .description
                .as_deref()
                .unwrap_or("")
                .contains("Run several tool calls"),
        "{:?}",
        lib.description
    );
    assert!(by_name("batch")
        .description
        .as_deref()
        .unwrap_or("")
        .contains("Run several tool calls"));
    let ann = by_name("get_context").annotations.unwrap();
    assert_eq!(
        (
            ann.read_only_hint,
            ann.destructive_hint,
            ann.open_world_hint
        ),
        (Some(true), Some(false), Some(false))
    );
    let ann = by_name("delete_track").annotations.unwrap();
    assert_eq!(
        (ann.read_only_hint, ann.destructive_hint),
        (Some(false), Some(true))
    );
    assert_eq!(
        by_name("create_clip").annotations.unwrap().destructive_hint,
        Some(false)
    );
}

#[test]
fn a_call_that_held_live_while_playing_says_so() {
    use mcp_ableton_music_maker::connection::CallTrace;
    use mcp_ableton_music_maker::tools::{held_line, HELD_MS_THRESHOLD};
    let playing = json!({"is_playing": true, "bar": 9, "beat": 32.0});
    let mut t = CallTrace {
        commands: vec![
            "create_midi_track".into(),
            "load_browser_item".into(),
            "load_browser_item".into(),
        ],
        live_ms: 1200.0,
        main_ms: 839.0,
        slices: 2,
        clock: Some(playing.clone()),
    };
    let line = held_line(&t).expect("over the threshold while playing");
    assert!(
        line.starts_with(
            "Held Live for 839 ms while the music played (create_midi_track, load_browser_item)."
        ),
        "{line}"
    );
    t.main_ms = HELD_MS_THRESHOLD - 1.0;
    assert!(held_line(&t).is_none(), "under the threshold: nothing said");
    t.main_ms = 839.0;
    t.clock = Some(json!({"is_playing": false}));
    assert!(
        held_line(&t).is_none(),
        "stopped: Live's cost is nobody's problem"
    );
    t.clock = None;
    assert!(
        held_line(&t).is_none(),
        "no performance clock: nothing said"
    );
}

// ── Names instead of numbers (#66) ──────────────────────────────────────────

/// #66's whole point: the damage was not the seven tools, it was that they
/// **interleaved** — an agent that had just succeeded with a name had no way
/// to know the next tool would refuse one.
///
/// So this is the invariant rather than seven separate assertions: every
/// core tool that addresses a track by index also takes `track`, and every
/// one that addresses a Session clip by index also takes `clip`. A new tool
/// that takes `track_index` alone fails here, and so would half-doing this
/// again.
#[tokio::test]
async fn every_core_tool_that_takes_a_track_takes_a_name() {
    let server = server_with(FakeBridge::responding(json!({})));
    let tools_list = server.tool_list();
    // `arrange` and `build_song` address a track inside their own documents;
    // `arrange` is covered by tests/arrangement.rs, and `build_song`'s
    // document names every track it creates.
    let by_document = ["build_song", "arrange"];
    let mut checked = 0;
    for name in CORE_TOOLS {
        if by_document.contains(name) {
            continue;
        }
        let tool = tools_list
            .iter()
            .find(|t| t.name == **name)
            .unwrap_or_else(|| panic!("{name} is served"));
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        let Some(properties) = schema["properties"].as_object() else {
            continue;
        };
        if properties.contains_key("track_index") {
            assert!(
                properties.contains_key("track"),
                "{name} takes a track by index only; #66 says every core tool takes the name"
            );
            checked += 1;
        }
        if properties.contains_key("clip_index") {
            assert!(
                properties.contains_key("clip"),
                "{name} takes a clip by index only; #66 says a clip has a name too"
            );
        }
    }
    assert!(
        checked >= 6,
        "only {checked} core tools address a track by index — has the list moved?"
    );
}

/// The audit in #66, the other way round: the tools it named must each take
/// `track`, whatever else changes around them.
#[tokio::test]
async fn the_seven_tools_the_audit_named_all_take_a_track() {
    let server = server_with(FakeBridge::responding(json!({})));
    let tools_list = server.tool_list();
    for name in [
        "set_track_mixer",
        "set_send",
        "load_instrument_or_effect",
        "delete_track",
        "create_clip",
        "add_notes_to_clip",
        "delete_clip",
        // and the one added for consistency
        "adv_set_clip_name",
    ] {
        let tool = tools_list
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} is served"));
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("track"), "{name} has no track");
        // The index form is what build_song documents and batch payloads
        // send: it is kept, never required.
        assert!(
            properties.contains_key("track_index"),
            "{name} dropped track_index"
        );
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(
            !required.iter().any(|r| r == "track_index"),
            "{name} still requires track_index"
        );
    }
}
