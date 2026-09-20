//! The performance tools end to end through the real bodies with a scripted
//! bridge: start and end sequences, the guards, cue resolution and text, the
//! silence check, recording, the crossfader, and the readout after a
//! reconnect. The Remote Script's clock itself cannot run here; the manual
//! pass in the story verifies it.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::performance::CueParams;
use mcp_ableton_music_maker::tools::{
    self, CancelCueParams, CaptureMixParams, ClipParams, CreateSceneParams, Empty,
    EndPerformanceParams, FireClipParams, FireSceneParams, KeepTrackPlayingParams, ListenParams,
    PanicParams, RecordClipParams, RestoreMixParams, SetCrossfaderParams,
    SetLaunchQuantizationParams, SetTempoParams, StartPerformanceParams, TrackParams,
    UndoVaryParams, VaryClipParams,
};
use rmcp::model::CallToolResult;
use serde_json::{json, Value};
use std::sync::Arc;

/// A five-track set at 126 BPM in 4/4: Kick and Bass play the Groove row.
fn state(bar: i64, beat_in_bar: i64, playing: bool) -> Value {
    let beat = ((bar - 1) * 4 + (beat_in_bar - 1)) as f64 + 0.5;
    let (kick, bass) = if playing { (1, 1) } else { (-1, -1) };
    json!({
        "is_playing": playing, "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
        "beat": beat, "bar": bar, "beat_in_bar": beat_in_bar,
        "clip_trigger_quantization": 4, "clip_trigger_quantization_name": "1_bar",
        "scale_name": "Minor", "root_note": 5, "root_note_name": "F",
        "tracks": [
            {"index": 0, "name": "Kick", "playing_slot_index": kick, "playing_clip_name": if playing { Value::from("Groove/Kick") } else { Value::Null }, "slots_with_clips": [0, 1, 2]},
            {"index": 1, "name": "Bass", "playing_slot_index": bass, "playing_clip_name": if playing { Value::from("Groove/Bass") } else { Value::Null }, "slots_with_clips": [1, 2]},
            {"index": 2, "name": "Pad", "playing_slot_index": -1, "slots_with_clips": [2, 3]},
            {"index": 3, "name": "Lead", "playing_slot_index": -1, "slots_with_clips": [2]},
            {"index": 4, "name": "Breaks", "playing_slot_index": -1, "slots_with_clips": [4]}
        ],
        "scenes": [
            {"index": 0, "name": "Intro", "clip_tracks": [0]},
            {"index": 1, "name": "Groove", "is_playing": playing, "clip_tracks": [0, 1]},
            {"index": 2, "name": "Groove+Pad", "clip_tracks": [0, 1, 2, 3]},
            {"index": 3, "name": "Break", "clip_tracks": [2]},
            {"index": 4, "name": "Breakbeat", "clip_tracks": [4]},
            {"index": 5, "name": "Outro", "clip_tracks": []}
        ],
        "cues": [], "events": []
    })
}

fn bridge() -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script("get_performance_state", vec![state(14, 2, true)]);
    // Live's own meter taper, as get_meter_scale hands it over. The numbers
    // here are a stand-in shape, not Live's own curve: what is pinned is that
    // a reading is interpolated on the curve rather than 20·log10 of it.
    b.script(
        "get_meter_scale",
        vec![
            json!({"points": [[0.0, -80.0], [0.4, -30.0], [0.7, -12.0], [0.85, 0.0], [1.0, 6.0]],
                    "reference": "post_fader"}),
        ],
    );
    b.script(
        "set_launch_quantization",
        vec![json!({"clip_trigger_quantization": 4, "name": "1_bar"})],
    );
    b.script(
        "fire_scene",
        vec![json!({"fired": true, "scene_index": 0, "name": "Intro", "clips": [{"track": "Kick"}, {"track": "Bass"}], "off_grid_clips": [], "would_record": []})],
    );
    b.script(
        "schedule_cue",
        vec![json!({"id": 2, "name": "into breaks", "beat_now": 53.5, "steps": []})],
    );
    b.script(
        "cancel_cue",
        vec![json!({"cancelled": [2], "reason": "cancelled"})],
    );
    b.script(
        "get_session_info",
        vec![json!({"tempo": 126.0, "signature_numerator": 4, "master_track": {"volume": 0.7}})],
    );
    b
}

async fn start(server: &tools::Server) -> CallToolResult {
    server
        .run(
            &tools::START_PERFORMANCE,
            StartPerformanceParams {
                scene: Some(json!("Intro")),
                quantization: "1_bar".into(),
                key: None,
                tempo: None,
                disarm: true,
                limiter: false,
                follow_key: false,
                record: "off".into(),
            },
            tools::start_performance_body,
        )
        .await
}

#[tokio::test]
async fn start_performance_sets_quantization_fires_the_scene_and_guards() {
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(1, 1, false), state(1, 1, true)],
    );
    let server = server_with(b.clone());
    let r = start(&server).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec![
            "get_performance_state",
            "set_launch_quantization",
            "set_performance_mode",
            "fire_scene",
            "get_performance_state"
        ]
    );
    assert_eq!(b.sent()[1].1["name"], "1_bar");
    assert_eq!(b.sent()[3].1["scene_index"], 0);
    let t = text_of(&r);
    assert!(t.starts_with("Performance started at "), "{t}");
    assert!(
        t.contains("Playing scene 'Intro' (2 clips) from bar 1 · 126 BPM · 4/4 · key F Minor (from your set's scale)"),
        "{t}"
    );
    assert!(
        t.contains("Launch quantization is 1 bar: everything you or I fire lands on the next bar."),
        "{t}"
    );
    assert!(
        t.ends_with("State: get_performance_state. Timed moves: cue."),
        "{t}"
    );

    // The guards: nothing reaches Live.
    let before = b.commands().len();
    let r = server
        .run(&tools::STOP_PLAYBACK, Empty {}, tools::stop_playback_body)
        .await;
    assert!(is_error(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("A performance is running (since bar 1, started "),
        "{t}"
    );
    assert!(
        t.contains("stop_playback would cut the audio mid-bar."),
        "{t}"
    );
    assert!(
        t.contains("end_performance {\"at\": \"next_bar\"}")
            && t.contains("\"fade_bars\": 8")
            && t.contains("\"now\": true"),
        "{t}"
    );
    let r = server
        .run(
            &tools::SET_TEMPO,
            SetTempoParams { tempo: 134.0 },
            tools::set_tempo_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("ramp it with cue"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            CaptureMixParams {
                start_bar: None,
                start: Some(0.0),
                bars: 4,
                name: "x".into(),
            },
            tools::capture_mix_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("would stop and move the transport"));
    let r = server
        .run(
            &tools::BACK_TO_ARRANGEMENT,
            Empty {},
            tools::back_to_arrangement_body,
        )
        .await;
    assert!(is_error(&r));
    assert_eq!(
        b.commands().len(),
        before,
        "no guarded command reached Live"
    );

    // A second start is refused; the first one stands.
    let r = start(&server).await;
    assert!(
        is_error(&r) && text_of(&r).contains("already running"),
        "{}",
        text_of(&r)
    );
    assert_eq!(b.commands().len(), before);
}

#[tokio::test]
async fn delete_is_refused_only_for_playing_or_queued_targets() {
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(1, 1, false), state(1, 1, true)],
    );
    let server = server_with(b.clone());
    assert!(!is_error(&start(&server).await));
    // Kick plays slot 1 → refused; Kick slot 0 is idle → allowed.
    let r = server
        .run(
            &tools::DELETE_CLIP,
            ClipParams {
                track_index: Some(0),
                clip_index: Some(1),
                ..Default::default()
            },
            tools::delete_clip_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("slot 1 on 'Kick' is playing or queued"),
        "{}",
        text_of(&r)
    );
    assert!(!b.commands().contains(&"delete_clip".to_string()));
    let r = server
        .run(
            &tools::DELETE_CLIP,
            ClipParams {
                track_index: Some(0),
                clip_index: Some(0),
                ..Default::default()
            },
            tools::delete_clip_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(b.commands().contains(&"delete_clip".to_string()));
    let r = server
        .run(
            &tools::DELETE_TRACK,
            TrackParams {
                track_index: Some(1),
                ..Default::default()
            },
            tools::delete_track_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("track 'Bass' is playing or queued"));
    let r = server
        .run(
            &tools::DELETE_TRACK,
            TrackParams {
                track_index: Some(2),
                ..Default::default()
            },
            tools::delete_track_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
}

#[tokio::test]
async fn cue_resolves_bars_to_beats_checks_silence_and_reports_the_plan() {
    let b = bridge();
    let server = server_with(b.clone());
    let p: CueParams = serde_json::from_value(json!({
        "name": "into breaks",
        "steps": [
            {"at": {"bar": 49}, "fire_scene": "Break"},
            {"from": {"bar": 49}, "bars": 8, "ramp": {"tempo": 134}},
            {"at": {"bar": 57}, "fire_scene": "Breakbeat"}
        ]
    }))
    .unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands(), vec!["get_performance_state", "schedule_cue"]);
    let sent = &b.sent()[1].1["cue"];
    assert_eq!(sent["name"], "into breaks");
    let steps = sent["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0]["action"], "fire_scene");
    assert_eq!(steps[0]["scene_index"], 3);
    assert_eq!(steps[0]["beat"], 192.0);
    assert_eq!(steps[1]["action"], "ramp");
    assert_eq!(steps[1]["target"], "tempo");
    assert_eq!(steps[1]["to"], 134.0);
    assert_eq!(steps[1]["end_beat"], 224.0);
    assert_eq!(steps[2]["scene_index"], 4);
    assert_eq!(steps[2]["beat"], 224.0);
    let t = text_of(&r);
    assert!(
        t.starts_with("Cue 'into breaks' (id 2) scheduled — it is bar 14.2 now.\n"),
        "{t}"
    );
    assert!(
        t.contains("bar 49       fire scene 'Break' (1 clip: Pad); Kick, Bass out"),
        "{t}"
    );
    assert!(t.contains("bars 49–57   ramp tempo 126 → 134"), "{t}");
    assert!(
        t.contains("bar 57       fire scene 'Breakbeat' (1 clip: Breaks); Pad out"),
        "{t}"
    );
    assert!(
        t.contains("Check: every bar from the first step has at least one clip playing."),
        "{t}"
    );
    assert!(t.ends_with("Cancel with cancel_cue {\"id\": 2}."), "{t}");

    // The past, silence, and an unknown scene never reach Live.
    let before = b.commands().len();
    for (steps, expect) in [
        (
            json!([{"at": {"bar": 10}, "fire_scene": "Break"}]),
            "step 1 is at bar 10 but it is bar 14.2",
        ),
        (
            json!([{"at": {"bar": 20}, "fire_scene": "Outro"}]),
            "at bar 20 nothing would be playing",
        ),
        (
            json!([{"at": {"bar": 20}, "fire_scene": "Drop"}]),
            "no scene named 'Drop'",
        ),
        (
            json!([{"at": {"bar": 20}, "stop_all_clips": true}]),
            "nothing would be playing",
        ),
    ] {
        let p: CueParams = serde_json::from_value(json!({"steps": steps})).unwrap();
        let r = server.run(&tools::CUE, p, tools::cue_body).await;
        assert!(
            is_error(&r) && text_of(&r).contains(expect),
            "{}",
            text_of(&r)
        );
    }
    assert!(!b.commands()[before..].contains(&"schedule_cue".to_string()));

    // Silence is allowed when asked for; a close step warns.
    let p: CueParams = serde_json::from_value(
        json!({"allow_silence": true, "steps": [{"at": "next_bar", "stop_all_clips": true}]}),
    )
    .unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.contains("Warning: step 1 is at bar 15 and it is 14.2"),
        "{t}"
    );
    assert!(!t.contains("Check: every bar"));
    assert_eq!(b.sent().last().unwrap().1["cue"]["steps"][0]["beat"], 56.0);

    let r = server
        .run(
            &tools::CANCEL_CUE,
            CancelCueParams { id: 2 },
            tools::cancel_cue_body,
        )
        .await;
    assert!(!is_error(&r));
    assert_eq!(
        text_of(&r),
        "Cancelled cue 2; steps already fired stay fired."
    );
    assert_eq!(b.sent().last().unwrap().1["id"], 2);
}

#[tokio::test]
async fn cue_needs_the_transport_running() {
    let b = bridge();
    b.script("get_performance_state", vec![state(1, 1, false)]);
    let server = server_with(b.clone());
    let p: CueParams =
        serde_json::from_value(json!({"steps": [{"at": "next_bar", "fire_scene": "Groove"}]}))
            .unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(
        is_error(&r) && text_of(&r).contains("Nothing is playing"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn end_performance_stops_on_the_bar_fades_or_stops_now() {
    // On the bar (the default).
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(1, 1, false), state(1, 1, true), state(78, 3, true)],
    );
    let server = server_with(b.clone());
    assert!(!is_error(&start(&server).await));
    let r = server
        .run(
            &tools::END_PERFORMANCE,
            EndPerformanceParams::default(),
            tools::end_performance_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands();
    let n = cmds.len();
    assert_eq!(
        &cmds[n - 4..],
        &[
            "get_performance_state",
            "cancel_cue",
            "schedule_cue",
            "set_performance_mode"
        ]
    );
    assert_eq!(b.sent()[n - 3].1["id"], Value::Null, "every cue cancelled");
    assert_eq!(b.sent()[n - 1].1["on"], false, "performance mode off");
    let steps = b.sent()[n - 2].1["cue"]["steps"].clone();
    assert_eq!(steps[0]["action"], "stop_playback");
    assert_eq!(steps[0]["beat"], 312.0, "bar 79 starts at beat 312");
    let t = text_of(&r);
    assert!(
        t.starts_with("Performance ends at bar 79: the transport stops on the bar. 0 min"),
        "{t}"
    );
    assert!(
        t.contains("0 cues scheduled, 0 cancelled. Guards lifted."),
        "{t}"
    );
    // Guards are lifted: stop_playback reaches Live now.
    let r = server
        .run(&tools::STOP_PLAYBACK, Empty {}, tools::stop_playback_body)
        .await;
    assert!(!is_error(&r));
    assert_eq!(b.commands().last().unwrap(), "stop_playback");
    let r = server
        .run(
            &tools::END_PERFORMANCE,
            EndPerformanceParams::default(),
            tools::end_performance_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r) == "No performance is running.");

    // Fade.
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(1, 1, false), state(1, 1, true), state(78, 3, true)],
    );
    let server = server_with(b.clone());
    assert!(!is_error(&start(&server).await));
    let r = server
        .run(
            &tools::END_PERFORMANCE,
            EndPerformanceParams {
                at: None,
                fade_bars: Some(8.0),
                now: false,
            },
            tools::end_performance_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = b.sent();
    let steps = sent[sent.len() - 2].1["cue"]["steps"].clone();
    assert_eq!(steps[0]["action"], "ramp");
    assert_eq!(steps[0]["target"], "volume");
    assert_eq!(steps[0]["kind"], "master");
    assert_eq!(steps[0]["to"], 0.0);
    assert_eq!(steps[0]["beat"], 312.0);
    assert_eq!(steps[0]["end_beat"], 344.0);
    assert_eq!(steps[1]["action"], "stop_playback");
    assert_eq!(steps[1]["beat"], 344.0);
    assert_eq!(steps[2]["action"], "set");
    assert_eq!(
        steps[2]["value"], 0.7,
        "the master volume read from the set is restored"
    );
    assert!(
        text_of(&r).contains("fades over 8 bars from bar 79 and the transport stops at bar 87"),
        "{}",
        text_of(&r)
    );

    // Now.
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(1, 1, false), state(1, 1, true), state(78, 3, true)],
    );
    let server = server_with(b.clone());
    assert!(!is_error(&start(&server).await));
    let r = server
        .run(
            &tools::END_PERFORMANCE,
            EndPerformanceParams {
                at: None,
                fade_bars: None,
                now: true,
            },
            tools::end_performance_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands();
    let n = cmds.len();
    assert_eq!(
        &cmds[n - 4..],
        &[
            "cancel_cue",
            "stop_all_clips",
            "stop_playback",
            "set_performance_mode"
        ]
    );
    assert!(text_of(&r).starts_with("Performance ended now, at bar 78.3"));
}

#[tokio::test]
async fn state_readout_reports_what_the_clock_did_while_away() {
    let b = bridge();
    let mut s = state(96, 1, true);
    s["cues"] = json!([{"id": 3, "name": "into breaks now", "pending": 1, "steps": [
        {"index": 0, "action": "fire_scene", "beat": 172.0, "bar": 44.0, "label": "fire scene 'Break'", "done": true},
        {"index": 1, "action": "fire_scene", "beat": 204.0, "bar": 52.0, "label": "fire scene 'Breakbeat'", "done": false}
    ]}]);
    s["events"] = json!([
        {"type": "cue_step_fired", "cue_id": 3, "target_bar": 44.0, "label": "fire scene 'Break'"},
        {"type": "ramp_done", "cue_id": 3, "label": "ramp tempo 126 → 134"}
    ]);
    b.script("get_performance_state", vec![s]);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::GET_PERFORMANCE_STATE,
            tools::GetPerformanceStateParams::default(),
            tools::get_performance_state_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("bar 96.1 · 126 BPM · 4/4 · playing · quantization 1 bar · key F Minor\n"),
        "{t}"
    );
    assert!(
        t.contains("Scenes: Intro · [Groove] · Groove+Pad · Break · Breakbeat · (1 empty)"),
        "{t}"
    );
    assert!(
        t.contains(
            "Cue 3 'into breaks now' — next step bar 52 fire scene 'Breakbeat'; 1 step done"
        ),
        "{t}"
    );
    assert!(t.contains("Since the last call: cue 3 fired at bar 44: fire scene 'Break'; cue 3: ramp tempo 126 → 134 done"), "{t}");
    assert!(t.len() < 1200, "{}", t.len());

    // A performance is running but Live's transport is stopped.
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(1, 1, false), state(1, 1, true), state(88, 3, false)],
    );
    let server = server_with(b.clone());
    assert!(!is_error(&start(&server).await));
    let r = server
        .run(
            &tools::GET_PERFORMANCE_STATE,
            tools::GetPerformanceStateParams::default(),
            tools::get_performance_state_body,
        )
        .await;
    let t = text_of(&r);
    assert!(
        t.contains("· stopped ·") && t.contains("The transport is stopped"),
        "{t}"
    );
}

#[tokio::test]
async fn fire_scene_record_clip_crossfader_and_quantization() {
    let b = bridge();
    b.script(
        "fire_scene",
        vec![
            json!({"fired": true, "scene_index": 2, "name": "Groove+Pad", "clips": [{}, {}, {}, {}],
            "off_grid_clips": [{"track": "Lead", "name": "Lead A", "launch_quantization": "1/16"}],
            "would_record": ["Breaks"]}),
        ],
    );
    b.script(
        "record_clip",
        vec![
            json!({"track_index": 3, "track": "Lead", "slot": 3, "record_length": 16.0, "bars": 4}),
        ],
    );
    b.script(
        "set_crossfader",
        vec![json!({"crossfader": 1.0, "assigned": [{"track": "Pad", "side": "B"}]})],
    );
    b.script(
        "create_scene",
        vec![json!({"index": 6, "name": "Break 2", "scene_count": 7})],
    );
    let server = server_with(b.clone());

    let r = server
        .run(
            &tools::FIRE_SCENE,
            FireSceneParams {
                scene: json!("groove+pad"),
                no_later_than: None,
            },
            tools::fire_scene_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.starts_with("Fired scene 'Groove+Pad' (4 clips) — starts at bar 15 (1-bar quantization; it is bar 14.2)."), "{t}");
    assert!(
        t.contains("Lead/Lead A (1/16) launch on their own quantization"),
        "{t}"
    );
    assert!(t.contains("Breaks armed with an empty slot"), "{t}");
    assert_eq!(b.sent().last().unwrap().1["scene_index"], 2);
    let r = server
        .run(
            &tools::FIRE_SCENE,
            FireSceneParams {
                scene: json!("Drop"),
                no_later_than: None,
            },
            tools::fire_scene_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("no scene named 'Drop'"));

    let r = server
        .run(
            &tools::RECORD_CLIP,
            RecordClipParams {
                track: json!("Lead"),
                bars: 4,
                name: Some("Lead live".into()),
                no_later_than: None,
            },
            tools::record_clip_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).starts_with("Recording Lead slot 3 'Lead live' — starts at bar 15 (1-bar quantization; it is bar 14.2), 4 bars, then loops."),
        "{}",
        text_of(&r)
    );
    let sent = b.sent().last().unwrap().1.clone();
    assert_eq!(sent["track_index"], 3);
    assert_eq!(sent["bars"], 4);
    let r = server
        .run(
            &tools::RECORD_CLIP,
            RecordClipParams {
                track: json!("Lead"),
                bars: 65,
                name: None,
                no_later_than: None,
            },
            tools::record_clip_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("between 1 and 64"));

    let r = server
        .run(
            &tools::SET_CROSSFADER,
            serde_json::from_value::<SetCrossfaderParams>(
                json!({"value": 1.0, "assign": [{"track": "Pad", "side": "b"}]}),
            )
            .unwrap(),
            tools::set_crossfader_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = b.sent().last().unwrap().1.clone();
    assert_eq!(sent["assign"][0]["track_index"], 2);
    assert_eq!(sent["assign"][0]["side"], "b");
    assert_eq!(
        text_of(&r),
        "Crossfader: crossfader at 1 (B), Pad → B. For a timed blend, ramp it in a cue."
    );
    let r = server
        .run(
            &tools::SET_CROSSFADER,
            serde_json::from_value::<SetCrossfaderParams>(
                json!({"assign": [{"track": "Pad", "side": "left"}]}),
            )
            .unwrap(),
            tools::set_crossfader_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("side must be A, B or none"));
    let r = server
        .run(
            &tools::SET_CROSSFADER,
            SetCrossfaderParams::default(),
            tools::set_crossfader_body,
        )
        .await;
    assert!(is_error(&r));

    b.script(
        "set_launch_quantization",
        vec![json!({"clip_trigger_quantization": 3, "name": "2_bars"})],
    );
    let r = server
        .run(
            &tools::SET_LAUNCH_QUANTIZATION,
            SetLaunchQuantizationParams {
                quantization: "2_bars".into(),
            },
            tools::set_launch_quantization_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(text_of(&r).starts_with("Launch quantization is now 2 bars:"));
    let r = server
        .run(
            &tools::SET_LAUNCH_QUANTIZATION,
            SetLaunchQuantizationParams {
                quantization: "3 bars".into(),
            },
            tools::set_launch_quantization_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("must be one of none, 8_bars"));

    let r = server
        .run(
            &tools::CREATE_SCENE,
            CreateSceneParams {
                index: -1,
                name: Some("Break 2".into()),
                tempo: None,
                phrase_bars: None,
            },
            tools::create_scene_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        text_of(&r),
        "Created scene 6 'Break 2' (7 scenes now). create_clip with clip_index 6 fills its row; fire_scene or a cue plays it."
    );
}

#[tokio::test]
async fn every_performance_tool_is_listed_and_refuses_without_the_script() {
    let b = FakeBridge::responding(json!({}));
    let server = server_with(b.clone());
    let names: Vec<String> = server
        .tool_list()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    for n in [
        "adv_start_performance",
        "adv_get_performance_state",
        "adv_cue",
        "adv_cancel_cue",
        "adv_fire_scene",
        "adv_create_scene",
        "record_clip",
        "adv_set_launch_quantization",
        "adv_set_crossfader",
        "end_performance",
    ] {
        assert!(names.contains(&n.to_string()), "{n} missing");
    }
    // The cue schema names the step actions the model must know.
    let cue = server
        .tool_list()
        .into_iter()
        .find(|t| t.name == "adv_cue")
        .unwrap();
    let schema = serde_json::to_string(&cue.input_schema).unwrap();
    for k in [
        "fire_scene",
        "fire_clip",
        "stop_clip",
        "stop_all_clips",
        "ramp",
        "allow_silence",
    ] {
        assert!(schema.contains(k), "cue schema lacks {k}");
    }
}

#[tokio::test]
async fn start_performance_disarms_and_sets_the_key_in_live() {
    let b = bridge();
    let mut armed = state(1, 1, false);
    armed["tracks"][3]["arm"] = json!(true);
    armed["tracks"][4]["arm"] = json!(true);
    b.script("get_performance_state", vec![armed, state(1, 1, true)]);
    b.script("set_track_mixer", vec![json!({"arm": false})]);
    b.script(
        "set_scale",
        vec![json!({"root_note": 2, "root_note_name": "D", "scale_name": "Minor", "scale_mode": true})],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::START_PERFORMANCE,
            StartPerformanceParams {
                scene: Some(json!("Intro")),
                quantization: "1_bar".into(),
                key: Some("D minor".into()),
                tempo: None,
                disarm: true,
                limiter: false,
                follow_key: false,
                record: "off".into(),
            },
            tools::start_performance_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands();
    assert_eq!(
        cmds,
        vec![
            "get_performance_state",
            "set_launch_quantization",
            "set_performance_mode",
            "set_track_mixer",
            "set_track_mixer",
            "set_scale",
            "fire_scene",
            "get_performance_state"
        ]
    );
    assert_eq!(b.sent()[3].1["track_index"], 3);
    assert_eq!(b.sent()[3].1["arm"], false);
    assert_eq!(b.sent()[5].1["root_note"], 2);
    assert_eq!(b.sent()[5].1["scale_name"], "Minor");
    let t = text_of(&r);
    assert!(t.contains("key D Minor (set in Live)"), "{t}");
    assert!(t.contains("Disarmed Lead, Breaks"), "{t}");

    // The parameter wins over the set's scale even without set_scale.
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(1, 1, false), state(1, 1, true)],
    );
    let server = server_with(b.clone());
    server.live().script.assume_all_capabilities();
    let r = server
        .run(
            &tools::START_PERFORMANCE,
            StartPerformanceParams {
                scene: None,
                quantization: "1_bar".into(),
                key: Some("nonsense".into()),
                tempo: None,
                disarm: false,
                limiter: false,
                follow_key: false,
                record: "off".into(),
            },
            tools::start_performance_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains("key nonsense") && t.contains("not understood"),
        "{t}"
    );
    assert!(!b.commands().contains(&"set_track_mixer".to_string()));
}

#[tokio::test]
async fn keep_track_playing_removes_stop_buttons_and_cues_respect_it() {
    let b = bridge();
    b.script(
        "set_slot_stop_buttons",
        vec![json!({"track_index": 3, "track": "Lead", "has_stop_button": false, "slots": [0, 1, 3, 4, 5]})],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::KEEP_TRACK_PLAYING,
            KeepTrackPlayingParams {
                track: json!("Lead"),
                keep: true,
            },
            tools::keep_track_playing_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(text_of(&r).starts_with("Lead keeps playing through scene launches: stop buttons removed from its 5 empty slots."), "{}", text_of(&r));
    let sent = b.sent().last().unwrap().1.clone();
    assert_eq!(sent["track_index"], 3);
    assert_eq!(sent["has_stop_button"], false);

    // Lead plays and has no stop button in the Break row: the cue keeps it.
    let mut s = state(14, 2, true);
    s["tracks"][3]["playing_slot_index"] = json!(2);
    s["tracks"][3]["playing_clip_name"] = json!("Lead A");
    s["tracks"][3]["no_stop_slots"] = json!([0, 1, 3, 4, 5]);
    b.script("get_performance_state", vec![s]);
    let p: CueParams =
        serde_json::from_value(json!({"steps": [{"at": {"bar": 49}, "fire_scene": "Break"}]}))
            .unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.contains("fire scene 'Break' (1 clip: Pad); Kick, Bass out"),
        "{t}"
    );
    assert!(!t.contains("Lead out"), "{t}");
}

fn clock(bar: i64, beat_in_bar: i64, seconds_to_next_bar: f64, playing: bool) -> serde_json::Value {
    json!({"bar": bar, "beat_in_bar": beat_in_bar, "beat": ((bar - 1) * 4 + beat_in_bar - 1) as f64 + 0.5,
        "tempo": 126.0, "beats_per_bar": 4, "is_playing": playing, "seconds_to_next_bar": seconds_to_next_bar,
        "phrase": {"scene_index": 1, "started_bar": 33, "bars": 16, "ends_bar": 48, "default": false},
        "next_cue": {"cue_id": 4, "bar": 51.0, "beat": 200.0, "label": "fire scene 'Break'"}, "pending_cues": 1})
}

#[tokio::test]
async fn every_result_ends_with_the_clock_line_while_performing() {
    let b = bridge();
    b.set_clock(Some(clock(39, 2, 2.2, true)));
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SET_LAUNCH_QUANTIZATION,
            SetLaunchQuantizationParams {
                quantization: "1_bar".into(),
            },
            tools::set_launch_quantization_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).ends_with("\n⏱ bar 39.2 · next bar in 2.2 s · phrase ends bar 48 · next cue: bar 51 fire scene 'Break' (cue 4)"),
        "{}",
        text_of(&r)
    );
    // An error that went through Live carries it too.
    let r = server
        .run(
            &tools::FIRE_SCENE,
            FireSceneParams {
                scene: json!("Drop"),
                no_later_than: None,
            },
            tools::fire_scene_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        text_of(&r).contains("no scene named 'Drop'") && text_of(&r).ends_with("(cue 4)"),
        "{}",
        text_of(&r)
    );
    // Stopped in Live.
    b.set_clock(Some(clock(88, 3, 0.0, false)));
    let r = server
        .run(
            &tools::SET_LAUNCH_QUANTIZATION,
            SetLaunchQuantizationParams {
                quantization: "1_bar".into(),
            },
            tools::set_launch_quantization_body,
        )
        .await;
    assert!(
        text_of(&r).ends_with("⏱ stopped at bar 88.3 (transport stopped outside this server)"),
        "{}",
        text_of(&r)
    );
    // No clock (no performance): no line, not one character changed.
    b.set_clock(None);
    let r = server
        .run(
            &tools::SET_LAUNCH_QUANTIZATION,
            SetLaunchQuantizationParams {
                quantization: "1_bar".into(),
            },
            tools::set_launch_quantization_body,
        )
        .await;
    assert!(!text_of(&r).contains('⏱'), "{}", text_of(&r));
}

#[tokio::test]
async fn launches_report_the_bar_they_made_and_no_later_than_defers() {
    let b = bridge();
    b.script("fire_clip", vec![json!({"fired": true, "lands_on_bar": 40, "issued_at_bar": 39, "issued_at_beat_in_bar": 3})]);
    let server = server_with(b.clone());
    let fire = |n: Option<i64>| FireClipParams {
        track_index: 4,
        clip_index: 2,
        no_later_than: n,
    };
    let r = server
        .run(&tools::FIRE_CLIP, fire(None), tools::fire_clip_body)
        .await;
    assert_eq!(
        text_of(&r),
        "Fired track 4, slot 2 — lands on bar 40 (issued at 39.3)."
    );

    // The bar line passed while the call was in flight: the last clock said 39 → expected 40.
    server.live().set_last_clock(clock(39, 4, 0.4, true));
    b.script("fire_clip", vec![json!({"fired": true, "lands_on_bar": 41, "issued_at_bar": 40, "issued_at_beat_in_bar": 1})]);
    let r = server
        .run(&tools::FIRE_CLIP, fire(None), tools::fire_clip_body)
        .await;
    assert!(
        text_of(&r)
            .contains("lands on bar 41, not 40: the call arrived at 40.1, after the bar line"),
        "{}",
        text_of(&r)
    );

    // no_later_than with the bar too close: a cue for the next certain bar, nothing fired.
    server.live().set_last_clock(clock(39, 4, 0.1, true));
    b.script(
        "schedule_cue",
        vec![json!({"id": 5, "name": "x", "steps": []})],
    );
    let before = b.commands().len();
    let r = server
        .run(&tools::FIRE_CLIP, fire(Some(40)), tools::fire_clip_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).starts_with("Not fired: bar 40 is 0.1 s away"),
        "{}",
        text_of(&r)
    );
    assert!(
        text_of(&r).contains("Scheduled as cue 5 for bar 41"),
        "{}",
        text_of(&r)
    );
    let sent = b.sent();
    assert_eq!(sent.len(), before + 1);
    assert_eq!(sent.last().unwrap().0, "schedule_cue");
    let step = &sent.last().unwrap().1["cue"]["steps"][0];
    assert_eq!(step["action"], "fire_clip");
    assert_eq!(step["beat"], 160.0, "bar 41 starts at beat 160");
    assert_eq!(step["clip_index"], 2);

    // Plenty of time: fires as usual.
    server.live().set_last_clock(clock(39, 2, 1.5, true));
    b.script("fire_clip", vec![json!({"fired": true, "lands_on_bar": 40, "issued_at_bar": 39, "issued_at_beat_in_bar": 2})]);
    let r = server
        .run(&tools::FIRE_CLIP, fire(Some(40)), tools::fire_clip_body)
        .await;
    assert_eq!(b.commands().last().unwrap(), "fire_clip");
    assert!(
        text_of(&r).starts_with("Fired track 4, slot 2 — lands on bar 40"),
        "{}",
        text_of(&r)
    );

    // A target that has passed.
    let r = server
        .run(&tools::FIRE_CLIP, fire(Some(38)), tools::fire_clip_body)
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("bar 38 has passed"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn phrase_times_sub_bar_times_and_gestures_expand() {
    let b = bridge();
    let mut s = state(41, 2, true);
    s["phrase"] =
        json!({"scene_index": 1, "started_bar": 33, "bars": 16, "ends_bar": 49, "default": false});
    b.script("get_performance_state", vec![s]);
    b.script(
        "schedule_cue",
        vec![json!({"id": 6, "name": "pad then break", "steps": []})],
    );
    let server = server_with(b.clone());
    let p: CueParams = serde_json::from_value(json!({"name": "pad then break", "steps": [
        {"at": "next_phrase", "fire_clip": {"track": "Pad", "clip": 2}},
        {"at": {"phrases_after": 2}, "fire_scene": "Break"}
    ]}))
    .unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let steps = b.sent().last().unwrap().1["cue"]["steps"].clone();
    assert_eq!(steps[0]["beat"], 192.0, "next phrase = bar 49");
    assert_eq!(steps[1]["beat"], 320.0, "two phrases after = bar 81");
    assert!(
        text_of(&r).contains("bar 49") && text_of(&r).contains("bar 81"),
        "{}",
        text_of(&r)
    );

    let p: CueParams = serde_json::from_value(json!({"steps": [{"at": {"bar": 49, "beat": 3}, "fire_clip": {"track": "Pad", "clip": 2}}]})).unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(
        is_error(&r)
            && text_of(&r).contains("launch quantization is 1 bar")
            && text_of(&r).contains("set_launch_quantization"),
        "{}",
        text_of(&r)
    );

    let p: CueParams = serde_json::from_value(json!({"steps": [{"at": {"bar": 49}, "gesture": {"breakdown": {"keep": ["Pad"], "bars": 8}}}]})).unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(
        is_error(&r) && text_of(&r).contains("breakdown keeps Pad — it is not playing"),
        "{}",
        text_of(&r)
    );

    let p: CueParams = serde_json::from_value(json!({"name": "breakdown", "steps": [{"at": {"bar": 49}, "gesture": {"breakdown": {"keep": ["Kick"], "bars": 8}}}]})).unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let steps = b.sent().last().unwrap().1["cue"]["steps"].clone();
    let steps = steps.as_array().unwrap();
    assert_eq!(steps.len(), 2, "Bass out, Bass back");
    assert_eq!(steps[0]["action"], "stop_clip");
    assert_eq!(steps[0]["track_index"], 1);
    assert_eq!(steps[0]["clip_index"], 1);
    assert_eq!(steps[0]["beat"], 192.0);
    assert_eq!(steps[1]["action"], "fire_clip");
    assert_eq!(steps[1]["beat"], 224.0);
    let t = text_of(&r);
    assert!(
        t.contains("breakdown: Bass out (stop clips), Kick stays"),
        "{t}"
    );
    assert!(
        t.contains("breakdown ends: Bass back (fire the slots they were playing)"),
        "{t}"
    );

    let p: CueParams =
        serde_json::from_value(json!({"steps": [{"at": {"bar": 49}, "gesture": {"drop": {}}}]}))
            .unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(
        is_error(&r) && text_of(&r).contains("nothing would be playing"),
        "{}",
        text_of(&r)
    );

    // panic: now, one bar, keep Kick.
    let r = server
        .run(
            &tools::PANIC,
            PanicParams {
                keep: vec![json!("Kick")],
                bars: 1.0,
            },
            tools::panic_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let steps = b.sent().last().unwrap().1["cue"]["steps"].clone();
    assert_eq!(steps[0]["action"], "ramp");
    assert_eq!(steps[0]["target"], "volume");
    assert_eq!(steps[0]["to"], 0.0);
    assert_eq!(steps[1]["action"], "stop_clip");
    assert!(
        text_of(&r).starts_with("Panic (cue 6): "),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn listen_reads_meters_over_a_bar_without_touching_the_transport() {
    let b = bridge();
    let mut s = state(14, 2, true);
    s["tempo"] = json!(6000.0);
    b.script("get_performance_state", vec![s]);
    b.script(
        "get_track_meters",
        vec![json!({"tracks": [{"name": "Kick", "left": 0.6, "right": 0.55}], "returns": [], "master": {"name": "Master", "level": 0.98}})],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::LISTEN,
            ListenParams {
                bars: 0.25,
                capture: false,
            },
            tools::listen_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands();
    assert_eq!(cmds[0], "get_performance_state");
    assert!(
        cmds[1..]
            .iter()
            .all(|c| c == "get_track_meters" || c == "get_meter_scale"),
        "nothing but reads; the transport is never touched: {cmds:?}"
    );
    let t = text_of(&r);
    assert!(t.starts_with("Listened for 0.25 bars from bar 15"), "{t}");
    // 0.6 on the curve above: −30 dB at 0.4, −12 dB at 0.7, so −18.0 dB.
    assert!(t.contains("Kick") && t.contains("peak −18.0 dB"), "{t}");
    assert!(t.contains("over 0 dB: clipping"), "{t}");
    assert!(t.contains("post-fader"), "{t}");
}

#[tokio::test]
async fn snapshots_variations_and_undo() {
    let b = bridge();
    b.script(
        "snapshot_mix",
        vec![json!({"id": 1, "tracks": 5, "returns": 2, "beat": 180.0})],
    );
    b.script("restore_mix", vec![json!({"id": 1, "restored": 8})]);
    b.script(
        "get_clip_notes",
        vec![json!({"notes": [{"pitch": 36, "start_time": 0.0, "duration": 0.25, "velocity": 100}, {"pitch": 42, "start_time": 0.5, "duration": 0.25, "velocity": 90}, {"pitch": 42, "start_time": 1.5, "duration": 0.25, "velocity": 90}]})],
    );
    b.script(
        "get_clip_info",
        vec![json!({"name": "Groove/Kick", "length": 4.0})],
    );
    let server = server_with(b.clone());
    let r = server
        .run(&tools::SNAPSHOT_MIX, Empty {}, tools::snapshot_mix_body)
        .await;
    assert!(
        text_of(&r).starts_with("Mix snapshot 1 taken at beat 180 (5 tracks, 2 returns"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::RESTORE_MIX,
            RestoreMixParams { id: 1 },
            tools::restore_mix_body,
        )
        .await;
    assert_eq!(
        text_of(&r),
        "Mix snapshot 1 restored on 8 tracks, returns and master."
    );

    let r = server
        .run(
            &tools::VARY_CLIP,
            VaryClipParams {
                track: json!("Kick"),
                clip: 1,
                variation: "thin".into(),
                seed: 1,
                to_slot: None,
            },
            tools::vary_clip_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let last = b.sent().last().unwrap().clone();
    assert_eq!(last.0, "add_notes_to_clip");
    assert!(
        b.commands().contains(&"clear_notes_from_clip".to_string()),
        "in place = clear, then write"
    );
    assert_eq!(
        last.1["notes"].as_array().unwrap().len(),
        2,
        "one off-beat note thinned"
    );
    assert!(
        text_of(&r).contains("in place (thin, seed 1): 3 notes → 2"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::UNDO_VARY,
            UndoVaryParams {
                track: json!("Kick"),
                clip: 1,
            },
            tools::undo_vary_body,
        )
        .await;
    assert!(
        text_of(&r).starts_with("Restored 'Kick' slot 1 to its 3 notes"),
        "{}",
        text_of(&r)
    );
    assert_eq!(
        b.sent().last().unwrap().1["notes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let r = server
        .run(
            &tools::UNDO_VARY,
            UndoVaryParams {
                track: json!("Kick"),
                clip: 1,
            },
            tools::undo_vary_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("nothing to undo"));
    let r = server
        .run(
            &tools::VARY_CLIP,
            VaryClipParams {
                track: json!("Kick"),
                clip: 1,
                variation: "fill_last_bar".into(),
                seed: 3,
                to_slot: Some(0),
            },
            tools::vary_clip_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("already holds a clip"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::VARY_CLIP,
            VaryClipParams {
                track: json!("Kick"),
                clip: 1,
                variation: "fill_last_bar".into(),
                seed: 3,
                to_slot: Some(5),
            },
            tools::vary_clip_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(b.commands().contains(&"create_clip".to_string()));
    assert!(
        text_of(&r).contains("→ slot 5 (fill_last_bar, seed 3)"),
        "{}",
        text_of(&r)
    );
}

// ── the Arrangement take ────────────────────────────────────────────────────
// A performance is kept as a take by default. When the Arrangement already
// holds something, the producer is asked what to do with it, and nothing in
// Live moves until they answer.

/// The bridge with an Arrangement that already has 128 bars on 5 tracks.
fn bridge_with_arrangement(bars: i64) -> Arc<FakeBridge> {
    let b = bridge();
    b.script(
        "arrangement_summary",
        vec![
            json!({"supported": true, "end_beat": (bars * 4) as f64, "bars": bars,
                    "clips": bars / 8, "tracks": if bars > 0 { 5 } else { 0 }, "beats_per_bar": 4}),
        ],
    );
    b.script(
        "start_arrangement_record",
        vec![
            json!({"from_beat": 512.0, "from_bar": 129, "replaced_clips": 16,
                    "replaced_tracks": 5, "record_mode": true}),
        ],
    );
    b.script(
        "stop_arrangement_record",
        vec![
            json!({"clips": 20, "tracks": 5, "end_beat": 856.0, "end_bar": 214,
                    "from_beat": 512.0, "from_bar": 129, "record_mode": false}),
        ],
    );
    b
}

async fn start_with(server: &tools::Server, record: &str) -> CallToolResult {
    server
        .run(
            &tools::START_PERFORMANCE,
            StartPerformanceParams {
                scene: Some(json!("Intro")),
                quantization: "1_bar".into(),
                key: None,
                tempo: None,
                disarm: true,
                limiter: false,
                follow_key: false,
                record: record.into(),
            },
            tools::start_performance_body,
        )
        .await
}

#[tokio::test]
async fn a_full_arrangement_is_asked_about_and_nothing_moves() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    let r = start_with(&server, "ask").await;
    assert!(is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("128 bars on 5 tracks"), "{t}");
    assert!(t.contains("after    record the take from bar 129"), "{t}");
    assert!(t.contains("replace  delete those 128 bars"), "{t}");
    assert!(t.contains("off      play without recording"), "{t}");
    // AC1: the read, and nothing else. No quantization, no fire, no arming.
    assert_eq!(
        b.commands(),
        vec!["get_performance_state", "arrangement_summary"],
        "a question must arrive before anything in Live has moved"
    );
}

#[tokio::test]
async fn an_empty_arrangement_records_from_bar_one_without_asking() {
    let b = bridge_with_arrangement(0);
    b.script(
        "start_arrangement_record",
        vec![json!({"from_beat": 0.0, "from_bar": 1, "replaced_clips": 0,
                    "replaced_tracks": 0, "record_mode": true})],
    );
    let server = server_with(b.clone());
    let r = start_with(&server, "ask").await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("from bar 1"), "{t}");
    assert!(t.contains("nothing to ask about"), "{t}");
    let sent = b.sent();
    let arm = sent
        .iter()
        .find(|(c, _)| c == "start_arrangement_record")
        .expect("armed");
    assert_eq!(arm.1["from_beat"], json!(0.0));
    assert_eq!(arm.1["replace"], json!(false));
}

#[tokio::test]
async fn after_records_past_what_is_already_there() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    let r = start_with(&server, "after").await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("Recording this take from bar 129"), "{t}");
    assert!(t.contains("Everything before it is untouched"), "{t}");
    let sent = b.sent();
    let arm = sent
        .iter()
        .find(|(c, _)| c == "start_arrangement_record")
        .expect("armed");
    // AC6/AC9: the bar line after the last clip, and no deletion.
    assert_eq!(arm.1["from_beat"], json!(512.0));
    assert_eq!(arm.1["replace"], json!(false));
    // AC9: the playhead and the arming happen before the fire.
    let order = b.commands();
    let armed_at = order.iter().position(|c| c == "start_arrangement_record");
    let fired_at = order.iter().position(|c| c == "fire_scene");
    assert!(armed_at < fired_at, "{order:?}");
}

#[tokio::test]
async fn replace_deletes_what_is_there_and_says_there_is_no_undo() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    let r = start_with(&server, "replace").await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("Deleted 128 bars"), "{t}");
    assert!(t.contains("Cmd+Z in Live is the way back"), "{t}");
    let sent = b.sent();
    let arm = sent
        .iter()
        .find(|(c, _)| c == "start_arrangement_record")
        .expect("armed");
    assert_eq!(arm.1["from_beat"], json!(0.0));
    assert_eq!(arm.1["replace"], json!(true));
}

#[tokio::test]
async fn off_records_nothing_at_all() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    let r = start_with(&server, "off").await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands();
    assert!(
        !cmds.contains(&"arrangement_summary".to_string()),
        "{cmds:?}"
    );
    assert!(
        !cmds.contains(&"start_arrangement_record".to_string()),
        "{cmds:?}"
    );
}

#[tokio::test]
async fn an_unknown_answer_is_refused_rather_than_guessed() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    let r = start_with(&server, "probably").await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert!(b.commands().is_empty(), "{:?}", b.commands());
}

#[tokio::test]
async fn the_answer_is_remembered_but_replace_never_is() {
    // "after" answered once, then a second performance does not ask.
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    assert!(!is_error(&start_with(&server, "after").await));
    end_now(&server).await;
    let r = start_with(&server, "ask").await;
    assert!(!is_error(&r), "asked again: {}", text_of(&r));
    assert!(text_of(&r).contains("from bar 129"), "{}", text_of(&r));

    // "replace" is not remembered: the next performance asks again.
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    assert!(!is_error(&start_with(&server, "replace").await));
    end_now(&server).await;
    let r = start_with(&server, "ask").await;
    assert!(is_error(&r), "replace was remembered: {}", text_of(&r));
}

#[tokio::test]
async fn a_live_that_cannot_report_the_arrangement_does_not_record() {
    let b = bridge();
    b.script(
        "arrangement_summary",
        vec![json!({"supported": false, "end_beat": 0.0, "bars": 0, "clips": 0, "tracks": 0})],
    );
    let server = server_with(b.clone());
    let r = start_with(&server, "ask").await;
    // AC13: it plays, it just does not record, and it says why.
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("Live 11 or newer"), "{t}");
    assert!(
        !b.commands()
            .contains(&"start_arrangement_record".to_string()),
        "{:?}",
        b.commands()
    );
}

#[tokio::test]
async fn a_performance_is_never_blocked_by_a_failed_read() {
    let b = bridge();
    // arrangement_summary is left unscripted and the bridge's default answer
    // carries no "supported", so the take is refused rather than guessed.
    let server = server_with(b.clone());
    let r = start_with(&server, "after").await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(text_of(&r).contains("Not recording"), "{}", text_of(&r));
}

async fn end_now(server: &tools::Server) -> CallToolResult {
    server
        .run(
            &tools::END_PERFORMANCE,
            EndPerformanceParams {
                at: None,
                fade_bars: None,
                now: true,
            },
            tools::end_performance_body,
        )
        .await
}

#[tokio::test]
async fn end_performance_stops_the_take_and_reports_the_bars() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    assert!(!is_error(&start_with(&server, "after").await));
    let r = end_now(&server).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("Take recorded: bars 129–214"), "{t}");
    assert!(t.contains("86 bars on 5 tracks"), "{t}");
    assert!(t.contains("Back to Arrangement is on"), "{t}");
    let sent = b.sent();
    let stop = sent
        .iter()
        .find(|(c, _)| c == "stop_arrangement_record")
        .expect("stopped");
    assert_eq!(stop.1["from_beat"], json!(512.0));
    assert_eq!(stop.1["back_to_arrangement"], json!(true));
}

#[tokio::test]
async fn the_readout_says_a_take_is_recording() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    assert!(!is_error(&start_with(&server, "after").await));
    let r = server
        .run(
            &tools::GET_PERFORMANCE_STATE,
            tools::GetPerformanceStateParams::default(),
            tools::get_performance_state_body,
        )
        .await;
    assert!(
        text_of(&r).contains("into the Arrangement from bar 129"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn a_take_is_disarmed_when_the_performance_cannot_start() {
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    // The scene cannot be resolved — a failure after the take is armed.
    let r = server
        .run(
            &tools::START_PERFORMANCE,
            StartPerformanceParams {
                scene: Some(json!("No Such Section")),
                quantization: "1_bar".into(),
                key: None,
                tempo: None,
                disarm: true,
                limiter: false,
                follow_key: false,
                record: "after".into(),
            },
            tools::start_performance_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    // AC16: Live is not left recording into a performance that never began.
    assert!(
        b.commands()
            .contains(&"stop_arrangement_record".to_string()),
        "{:?}",
        b.commands()
    );
}

#[tokio::test]
async fn a_take_never_touches_what_is_already_in_the_arrangement() {
    // AC7: the criterion the whole design rests on. An appended take sends
    // nothing that could shorten, move or delete existing Arrangement clips.
    let b = bridge_with_arrangement(128);
    let server = server_with(b.clone());
    assert!(!is_error(&start_with(&server, "after").await));
    end_now(&server).await;
    let destructive = [
        "delete_arrangement_clip",
        "delete_arrangement_clips",
        "duplicate_arrangement_clip",
        "place_clips",
        "set_arrangement_clip_name",
        "delete_clip",
        "delete_track",
    ];
    for c in b.commands() {
        assert!(
            !destructive.contains(&c.as_str()),
            "an appended take sent '{c}'"
        );
    }
    let sent = b.sent();
    let arm = sent
        .iter()
        .find(|(c, _)| c == "start_arrangement_record")
        .expect("armed");
    assert_eq!(arm.1["replace"], json!(false), "nothing may be replaced");
}

// ── A performance nobody is playing does not block the session ──────────────

fn stale_performance(server: &tools::Server, hours: i64) {
    use chrono::Local;
    *server.live().performance.lock().unwrap() =
        Some(mcp_ableton_music_maker::connection::Performance {
            started_at: Local::now() - chrono::Duration::hours(hours),
            start_bar: 1,
            start_beat: 0.0,
            key: None,
            quantization: "1_bar".into(),
            cues_scheduled: 0,
            cues_cancelled: 0,
            follow_key: false,
            song: None,
            take: None,
        });
}

#[tokio::test]
async fn a_performance_left_running_is_ended_by_the_next_tool_that_needs_the_transport() {
    let b = bridge();
    // Live's transport has been stopped since, and nothing is cued.
    b.script("get_performance_state", vec![state(14, 2, false)]);
    b.script("set_tempo", vec![json!({"tempo": 128.0})]);
    let server = server_with(b.clone());
    stale_performance(&server, 10);
    let r = server
        .run(
            &tools::SET_TEMPO,
            SetTempoParams { tempo: 128.0 },
            tools::set_tempo_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "the work goes through: {t}");
    assert!(
        t.starts_with("A performance was left running from 10 h")
            && t.contains("Live's transport has been stopped since — I ended it."),
        "the reply says what was ended and why: {t}"
    );
    assert!(
        b.commands().contains(&"set_tempo".to_string()),
        "the tempo change reached Live"
    );
    assert!(
        server.live().performance.lock().unwrap().is_none(),
        "the stale performance is gone"
    );
}

#[tokio::test]
async fn a_performance_that_is_playing_still_guards() {
    let b = bridge(); // state(14, 2, true): the transport is playing
    let server = server_with(b.clone());
    stale_performance(&server, 10);
    let before = b.commands().len();
    let r = server
        .run(
            &tools::SET_TEMPO,
            SetTempoParams { tempo: 128.0 },
            tools::set_tempo_body,
        )
        .await;
    let t = text_of(&r);
    assert!(is_error(&r), "{t}");
    assert!(t.contains("transport playing"), "{t}");
    assert!(t.contains("ramp it with cue"), "{t}");
    assert!(
        !b.commands()[before..].contains(&"set_tempo".to_string()),
        "nothing was written"
    );
    assert!(server.live().performance.lock().unwrap().is_some());
}

#[tokio::test]
async fn a_pending_cue_keeps_a_stopped_performance_alive() {
    let b = bridge();
    // Stopped, but a cue is waiting to fire: this performance is not over.
    let mut stopped = state(14, 2, false);
    stopped["cues"] = json!([{"id": 3, "name": "drop", "beat": 64.0}]);
    b.script("get_performance_state", vec![stopped]);
    let server = server_with(b.clone());
    stale_performance(&server, 10);
    let r = server
        .run(
            &tools::SET_TEMPO,
            SetTempoParams { tempo: 128.0 },
            tools::set_tempo_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert!(server.live().performance.lock().unwrap().is_some());
}
