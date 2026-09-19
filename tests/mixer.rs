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
        track_index: 4,
        kind: "track".into(),
        volume: None,
        volume_db: None,
        fader: Some(0.7),
        pan: Some(-0.2),
        mute: None,
        solo: None,
        arm: None,
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
        track_index: 0,
        kind: "track".into(),
        volume: None,
        volume_db: None,
        fader: None,
        pan: None,
        mute: None,
        solo: None,
        arm: None,
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
        track_index: 4,
        kind: "track".into(),
        send_name: "Delay".into(),
        send_index: None,
        value: 0.35,
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
        track_index: 4,
        kind: "track".into(),
        send_name: String::new(),
        send_index: None,
        value: 0.5,
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
