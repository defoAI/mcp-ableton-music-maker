//! The mixer, colour, drum-pad and deletion tools, through the real bodies
//! with a recording bridge: what reaches Live, and what the producer reads.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::tools::{
    self, DeleteArrangementClipParams, DeleteLocatorParams, DrumRackPadsParams, SetColorParams,
    SetSendParams, SetTrackMixerParams,
};
use serde_json::json;

#[tokio::test]
async fn mixer_forwards_only_what_was_given_and_reads_back() {
    let bridge = FakeBridge::responding(json!({
        "name": "Pad", "volume": 0.7, "panning": -0.2, "mute": false, "solo": false, "arm": false,
        "sends": [{"index": 0, "name": "A", "value": 0.4}]
    }));
    let server = server_with(bridge.clone());
    let p = SetTrackMixerParams {
        track_index: Some(4),
        kind: "track".into(),
        volume: None,
        volume_db: None,
        fader: Some(0.7),
        pan: Some(-0.2),
        mute: None,
        solo: None,
        arm: None,
        ..Default::default()
    };
    let r = server
        .run(&tools::SET_TRACK_MIXER, p, tools::set_track_mixer_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = bridge.sent();
    assert_eq!(sent[0].0, "set_track_mixer");
    assert_eq!(sent[0].1["volume"], 0.7);
    assert!(sent[0].1["mute"].is_null(), "unset fields travel as null");
    assert!(
        text_of(&r).contains("'Pad' now: volume 0.7, pan -0.2, unmuted, sends A=0.4"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn mixer_with_nothing_to_set_is_refused_before_live() {
    let bridge = FakeBridge::responding(json!({}));
    let server = server_with(bridge.clone());
    let p = SetTrackMixerParams {
        track_index: Some(0),
        kind: "track".into(),
        volume: None,
        volume_db: None,
        fader: None,
        pan: None,
        mute: None,
        solo: None,
        arm: None,
        ..Default::default()
    };
    let r = server
        .run(&tools::SET_TRACK_MIXER, p, tools::set_track_mixer_body)
        .await;
    assert!(is_error(&r));
    assert!(bridge.commands().is_empty());
}

#[tokio::test]
async fn send_by_name_and_by_index() {
    let bridge = FakeBridge::responding(
        json!({"track": "Pad", "send_index": 1, "return_name": "Delay", "value": 0.35}),
    );
    let server = server_with(bridge.clone());
    let p = SetSendParams {
        track_index: Some(4),
        kind: "track".into(),
        send_name: "Delay".into(),
        send_index: None,
        value: 0.35,
        ..Default::default()
    };
    let r = server.run(&tools::SET_SEND, p, tools::set_send_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(bridge.sent()[0].1["send_name"], "Delay");
    assert!(
        text_of(&r).contains("send 1 (Delay) set to 0.35"),
        "{}",
        text_of(&r)
    );

    let none = SetSendParams {
        track_index: Some(4),
        kind: "track".into(),
        send_name: String::new(),
        send_index: None,
        value: 0.5,
        ..Default::default()
    };
    let r = server
        .run(&tools::SET_SEND, none, tools::set_send_body)
        .await;
    assert!(is_error(&r) && text_of(&r).contains("get_returns"));
}

#[tokio::test]
async fn color_goes_to_track_or_clip() {
    let bridge =
        FakeBridge::responding(json!({"name": "Kick", "color_index": 12, "color": 16711680}));
    let server = server_with(bridge.clone());
    let track = SetColorParams {
        track_index: 1,
        color_index: 12,
        clip_index: None,
        arrangement: false,
        kind: "track".into(),
    };
    let r = server
        .run(&tools::SET_COLOR, track, tools::set_color_body)
        .await;
    assert!(!is_error(&r));
    let clip = SetColorParams {
        track_index: 1,
        color_index: 12,
        clip_index: Some(3),
        arrangement: true,
        kind: "track".into(),
    };
    let r = server
        .run(&tools::SET_COLOR, clip, tools::set_color_body)
        .await;
    assert!(!is_error(&r));
    assert_eq!(bridge.commands(), vec!["set_track_color", "set_clip_color"]);
    assert_eq!(bridge.sent()[1].1["arrangement"], true);
    let bad = SetColorParams {
        track_index: 1,
        color_index: 99,
        clip_index: None,
        arrangement: false,
        kind: "track".into(),
    };
    let r = server
        .run(&tools::SET_COLOR, bad, tools::set_color_body)
        .await;
    assert!(
        is_error(&r) && bridge.sent().len() == 2,
        "palette range checked before Live"
    );
}

#[tokio::test]
async fn drum_rack_pads_read_as_a_list() {
    let bridge = FakeBridge::responding(json!({
        "track_index": 0, "track_name": "Drums", "device_index": 0, "device_name": "AG Techno Kit", "pad_count": 2,
        "pads": [
            {"pitch": 36, "note_name": "C1", "pad_name": "Kick 909", "chains": ["Kick 909"], "mute": false, "solo": false},
            {"pitch": 37, "note_name": "C#1", "pad_name": "Rim", "chains": ["Rim"], "mute": true, "solo": false}
        ]
    }));
    let server = server_with(bridge.clone());
    let p = DrumRackPadsParams {
        track_index: 0,
        device_index: None,
        ..Default::default()
    };
    let r = server
        .run(
            &tools::GET_DRUM_RACK_PADS,
            p,
            tools::get_drum_rack_pads_body,
        )
        .await;
    assert!(!is_error(&r));
    assert_eq!(
        bridge.sent()[0].1["device_index"],
        -1,
        "first Drum Rack by default"
    );
    let t = text_of(&r);
    assert!(
        t.contains("36 (C1): Kick 909") && t.contains("37 (C#1): Rim [muted]"),
        "{t}"
    );
}

#[tokio::test]
async fn deleting_reports_what_went_and_what_is_left() {
    let bridge = FakeBridge::responding(
        json!({"name": "Kick", "start_time": 32.0, "end_time": 36.0, "remaining": 14}),
    );
    let server = server_with(bridge.clone());
    let p = DeleteArrangementClipParams {
        track_index: 0,
        clip_index: 5,
        clip_indices: vec![],
        all: false,
    };
    let r = server
        .run(
            &tools::DELETE_ARRANGEMENT_CLIP,
            p,
            tools::delete_arrangement_clip_body,
        )
        .await;
    assert!(!is_error(&r));
    assert!(
        text_of(&r).contains("Removed 'Kick' (beats 32–36)")
            && text_of(&r).contains("14 clips remain"),
        "{}",
        text_of(&r)
    );

    bridge.set_response(json!({"deleted": "Drop", "time": 128.0, "remaining": 3}));
    let p = DeleteLocatorParams {
        name: "Drop".into(),
        time: None,
    };
    let r = server
        .run(&tools::DELETE_LOCATOR, p, tools::delete_locator_body)
        .await;
    assert!(!is_error(&r));
    assert!(
        text_of(&r).contains("Deleted locator 'Drop' at beat 128"),
        "{}",
        text_of(&r)
    );
    let neither = DeleteLocatorParams {
        name: String::new(),
        time: None,
    };
    let r = server
        .run(&tools::DELETE_LOCATOR, neither, tools::delete_locator_body)
        .await;
    assert!(is_error(&r));
}

// ── The device chain: addressable, and editable ─────────────────────────────

fn device_bridge() -> std::sync::Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script(
        "get_performance_state",
        vec![json!({"tempo": 120.0, "signature_numerator": 4, "bar": 1, "beat": 0.0,
                    "tracks": [{"index": 0, "name": "Pad"}], "scenes": [], "cues": [], "events": []})],
    );
    b.script(
        "get_returns",
        vec![json!({"count": 1, "returns": [{"index": 0, "letter": "A", "name": "Reverb"}]})],
    );
    b.script(
        "get_track_info",
        vec![json!({"index": 0, "kind": "master", "name": "Master", "devices": [
            {"index": 0, "name": "EQ Eight", "class_name": "Eq8", "type": "audio_effect"},
            {"index": 1, "name": "Glue Compressor", "class_name": "Compressor2", "type": "audio_effect"},
            {"index": 2, "name": "Saturator", "class_name": "Saturator", "type": "audio_effect"}]})],
    );
    b
}

#[tokio::test]
async fn a_device_on_the_master_can_be_read_set_and_removed() {
    let b = device_bridge();
    b.script(
        "get_device_parameters",
        vec![json!({
        "track_index": 0, "kind": "master", "track_name": "Master",
        "devices": ["EQ Eight", "Glue Compressor", "Saturator"],
        "device": {"index": 2, "name": "Saturator", "class_name": "Saturator", "parameters": [
            {"index": 0, "name": "Device On", "value": 1.0, "min": 0.0, "max": 1.0,
             "is_quantized": true, "display": "On", "items": ["Off", "On"]},
            {"index": 1, "name": "Drive", "value": 0.5, "min": -36.0, "max": 36.0,
             "display": "0.00 dB", "display_min": "-36.0 dB", "display_max": "36.0 dB"},
            {"index": 2, "name": "Type", "value": 0.0, "min": 0.0, "max": 6.0, "is_quantized": true,
             "display": "Analog Clip", "items": ["Analog Clip", "Soft Sine", "Medium Curve"]}]}})],
    );
    let server = server_with(b.clone());

    // Read it back: the master is addressed by name, like the loader takes it.
    let r = server
        .run(
            &tools::GET_DEVICE_PARAMETERS,
            tools::DeviceParams {
                track: Some(json!("master")),
                device: Some(json!("Saturator")),
                ..Default::default()
            },
            tools::get_device_parameters_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    let sent = b.sent();
    let read = sent
        .iter()
        .find(|(c, _)| c == "get_device_parameters")
        .unwrap();
    assert_eq!(read.1["kind"], "master");
    assert_eq!(read.1["device_index"], 2, "resolved by name");
    assert!(
        t.starts_with("Master · device 2 'Saturator' (Saturator) — 3 parameters\n"),
        "{t}"
    );
    assert!(t.contains("Drive"), "{t}");
    assert!(
        t.contains("0.00 dB"),
        "Live's display string, not the float: {t}"
    );
    assert!(
        t.contains("-36.0 dB … 36.0 dB"),
        "the range reads as Live shows it: {t}"
    );
    assert!(
        t.contains("[Analog Clip, Soft Sine, Medium Curve]"),
        "a chooser lists what it accepts: {t}"
    );

    // Set it by what Live shows.
    b.script(
        "set_device_parameter",
        vec![
            json!({"track_index": 0, "kind": "master", "track_name": "Master",
                    "device_index": 2, "device": "Saturator", "parameter_index": 1,
                    "name": "Drive", "old_value": 0.5, "old_display": "0.00 dB",
                    "value": 0.542, "display": "3.00 dB", "min": -36.0, "max": 36.0}),
        ],
    );
    let r = server
        .run(
            &tools::SET_DEVICE_PARAMETER,
            tools::SetDeviceParameterParams {
                track: Some(json!("master")),
                device: Some(json!("Saturator")),
                parameter_index: Some(1),
                value: json!("3 dB"),
                ..Default::default()
            },
            tools::set_device_parameter_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    let last = b.sent().last().unwrap().clone();
    assert_eq!(
        last.1["value_display"], "3 dB",
        "a display string is resolved by Live"
    );
    assert!(last.1["value"].is_null());
    assert_eq!(
        t,
        "Master · Saturator · Drive: 0.00 dB → 3.00 dB (raw 0.5 → 0.542 of -36 … 36)."
    );

    // And take it off again.
    b.script(
        "delete_device",
        vec![json!({"deleted": "Saturator", "index": 2, "kind": "master",
                    "track_name": "Master", "devices": ["EQ Eight", "Glue Compressor"]})],
    );
    let r = server
        .run(
            &tools::EDIT_DEVICES,
            tools::EditDevicesParams {
                track: Some(json!("master")),
                device: Some(json!("Saturator")),
                action: "remove".into(),
                ..Default::default()
            },
            tools::edit_devices_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert_eq!(b.sent().last().unwrap().1["kind"], "master");
    assert!(
        t.starts_with("Removed 'Saturator' (was device 2) from Master."),
        "{t}"
    );
    assert!(t.contains("Chain: EQ Eight, Glue Compressor."), "{t}");
    assert!(t.contains("Cmd-Z"), "the way back is in the reply: {t}");
}

#[tokio::test]
async fn a_device_can_be_moved_and_bypassed() {
    let b = device_bridge();
    b.script(
        "move_device",
        vec![
            json!({"moved": "Limiter", "from_index": 1, "to_index": 2, "kind": "master",
                    "track_name": "Master", "devices": ["EQ Eight", "Glue Compressor", "Limiter"]}),
        ],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::EDIT_DEVICES,
            tools::EditDevicesParams {
                track: Some(json!("master")),
                device_index: Some(1),
                action: "move".into(),
                to_index: Some(2),
                ..Default::default()
            },
            tools::edit_devices_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert_eq!(b.sent().last().unwrap().1["to_index"], 2);
    assert!(
        t.starts_with("Moved 'Limiter' on Master from 1 to 2."),
        "{t}"
    );

    // move without to_index says what is missing, and sends nothing
    let before = b.commands().len();
    let r = server
        .run(
            &tools::EDIT_DEVICES,
            tools::EditDevicesParams {
                track: Some(json!("master")),
                device_index: Some(1),
                action: "move".into(),
                ..Default::default()
            },
            tools::edit_devices_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("move needs to_index"),
        "{}",
        text_of(&r)
    );
    assert_eq!(b.commands().len(), before, "nothing was sent");

    // bypass is the device's own switch, so it works wherever the device is
    b.script(
        "get_device_parameters",
        vec![json!({
        "track_index": 0, "kind": "track", "track_name": "Pad",
        "device": {"index": 1, "name": "EQ Eight", "class_name": "Eq8", "parameters": [
            {"index": 0, "name": "Device On", "value": 1.0, "min": 0.0, "max": 1.0,
             "is_quantized": true, "display": "On", "items": ["Off", "On"]}]}})],
    );
    let r = server
        .run(
            &tools::EDIT_DEVICES,
            tools::EditDevicesParams {
                track: Some(json!("Pad")),
                device_index: Some(1),
                action: "bypass".into(),
                ..Default::default()
            },
            tools::edit_devices_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    let last = b.sent().last().unwrap().clone();
    assert_eq!(last.0, "set_device_parameter");
    assert_eq!(last.1["parameter_index"], 0);
    assert_eq!(last.1["value"], 0.0);
    assert!(
        t.starts_with("Bypassed 'EQ Eight' on Pad (device 1)"),
        "{t}"
    );
    assert!(t.contains("still in the chain, not processing"), "{t}");
}

#[tokio::test]
async fn a_return_is_addressed_by_name_or_letter() {
    let b = device_bridge();
    b.script(
        "get_device_parameters",
        vec![json!({
        "track_index": 0, "kind": "return", "track_name": "Reverb",
        "device": {"index": 0, "name": "Reverb", "class_name": "Reverb", "parameters": [
            {"index": 0, "name": "Device On", "value": 1.0, "min": 0.0, "max": 1.0,
             "is_quantized": true, "display": "On", "items": ["Off", "On"]}]}})],
    );
    let server = server_with(b.clone());
    for name in ["Reverb", "A"] {
        let r = server
            .run(
                &tools::GET_DEVICE_PARAMETERS,
                tools::DeviceParams {
                    track: Some(json!(name)),
                    ..Default::default()
                },
                tools::get_device_parameters_body,
            )
            .await;
        assert!(!is_error(&r), "{}", text_of(&r));
        let read = b
            .sent()
            .into_iter()
            .rfind(|(c, _)| c == "get_device_parameters")
            .unwrap();
        assert_eq!(read.1["kind"], "return", "addressed as '{name}'");
        assert_eq!(read.1["track_index"], 0);
    }
}

// ── Names instead of numbers (#66) ──────────────────────────────────────────

/// A set the way a producer's is: named tracks, two returns Live lettered.
fn named_set() -> std::sync::Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({
        "name": "Sub", "track": "Sub", "volume": 0.85, "volume_db": 0.0,
        "panning": 0.0, "mute": false, "solo": false, "arm": false,
        "send_index": 1, "return_name": "Delay", "value": 0.35
    }));
    b.script(
        "get_performance_state",
        vec![json!({
            "is_playing": false, "tempo": 120.0, "signature_numerator": 4,
            "signature_denominator": 4, "beat": 0.0, "bar": 1, "beat_in_bar": 1,
            "clip_trigger_quantization": 4,
            "tracks": [
                {"index": 0, "name": "Pad", "playing_slot_index": -1, "slots_with_clips": []},
                {"index": 1, "name": "Keys", "playing_slot_index": -1, "slots_with_clips": []},
                {"index": 2, "name": "Sub", "playing_slot_index": -1, "slots_with_clips": []}
            ],
            "scenes": [], "cues": [], "events": []
        })],
    );
    b.script(
        "get_returns",
        vec![json!({"returns": [
            {"index": 0, "name": "Reverb", "letter": "A"},
            {"index": 1, "name": "Delay", "letter": "B"}
        ]})],
    );
    b
}

/// #66: the mixing move is one move. `shape_sound` already took a name;
/// `set_track_mixer` beside it must too — by track name, by a return's
/// letter, and by the index that always worked.
#[tokio::test]
async fn set_track_mixer_takes_a_name_a_return_letter_and_an_index() {
    for (given, want_index, want_kind) in [
        (json!("Sub"), 2, "track"),
        (json!("sub"), 2, "track"),
        (json!("Delay"), 1, "return"),
        (json!("B"), 1, "return"),
        (json!("master"), 0, "master"),
        (json!(2), 2, "track"),
    ] {
        let b = named_set();
        let server = server_with(b.clone());
        let r = server
            .run(
                &tools::SET_TRACK_MIXER,
                SetTrackMixerParams {
                    track: Some(given.clone()),
                    kind: "track".into(),
                    volume: Some(-6.0),
                    ..Default::default()
                },
                tools::set_track_mixer_body,
            )
            .await;
        assert!(!is_error(&r), "{given}: {}", text_of(&r));
        let sent = b.last("set_track_mixer").expect("set_track_mixer was sent");
        assert_eq!(sent["track_index"], want_index, "{given}");
        assert_eq!(sent["kind"], want_kind, "{given}");
        assert_eq!(sent["volume_db"], -6.0, "{given}");
    }
}

/// The index form is the one `build_song` documents and `batch` payloads
/// send: it must keep working exactly as it did.
#[tokio::test]
async fn set_track_mixer_still_takes_track_index() {
    let b = named_set();
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SET_TRACK_MIXER,
            SetTrackMixerParams {
                track_index: Some(1),
                kind: "track".into(),
                pan: Some(-0.5),
                ..Default::default()
            },
            tools::set_track_mixer_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = b.last("set_track_mixer").unwrap();
    assert_eq!(sent["track_index"], 1);
    assert_eq!(sent["pan"], -0.5);
    // No state read was needed: an index costs nothing extra.
    assert!(!b.commands().contains(&"get_performance_state".to_string()));
}

/// The other half of that same move.
#[tokio::test]
async fn set_send_takes_a_name_and_an_index() {
    for (given, want_index) in [(json!("Sub"), 2), (json!("Pad"), 0), (json!(2), 2)] {
        let b = named_set();
        let server = server_with(b.clone());
        let r = server
            .run(
                &tools::SET_SEND,
                SetSendParams {
                    track: Some(given.clone()),
                    kind: "track".into(),
                    send_name: "Delay".into(),
                    value: 0.35,
                    ..Default::default()
                },
                tools::set_send_body,
            )
            .await;
        assert!(!is_error(&r), "{given}: {}", text_of(&r));
        let sent = b.last("set_send").expect("set_send was sent");
        assert_eq!(sent["track_index"], want_index, "{given}");
        assert_eq!(sent["send_name"], "Delay", "{given}");
    }
}

/// A name nothing answers to names what is there, and changes nothing.
#[tokio::test]
async fn an_unknown_track_name_lists_the_tracks_and_writes_nothing() {
    let b = named_set();
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SET_TRACK_MIXER,
            SetTrackMixerParams {
                track: Some(json!("Bells")),
                kind: "track".into(),
                volume: Some(-3.0),
                ..Default::default()
            },
            tools::set_track_mixer_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    let text = text_of(&r);
    assert!(text.contains("Pad, Keys, Sub"), "{text}");
    assert!(!b.commands().contains(&"set_track_mixer".to_string()));
}
