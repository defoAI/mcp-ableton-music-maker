//! Sound without leaving the flow: shape_sound resolves words against a
//! rack's macros, an instrument's parameters or an unknown device's names,
//! writes several in one round trip and reports before/after in Live's
//! units; set_device_parameter takes a name substring; a cue ramp takes a
//! word.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::performance::CueParams;
use mcp_ableton_music_maker::tools::{self, SetDeviceParameterParams, ShapeSoundParams};
use serde_json::{json, Value};
use std::sync::Arc;

fn state() -> Value {
    json!({
        "is_playing": true, "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
        "beat": 53.5, "bar": 14, "beat_in_bar": 2, "clip_trigger_quantization": 4,
        "tracks": [
            {"index": 0, "name": "Pad", "playing_slot_index": 1, "slots_with_clips": [1]},
            {"index": 1, "name": "Bass", "playing_slot_index": 1, "slots_with_clips": [1]}
        ],
        "scenes": [{"index": 1, "name": "Groove · 8", "clip_tracks": [0, 1], "is_playing": true}],
        "cues": [], "events": []
    })
}

fn param(i: i64, name: &str, value: f64, min: f64, max: f64, shown: Option<&str>) -> Value {
    let mut v = json!({"index": i, "name": name, "value": value, "min": min, "max": max});
    if let Some(s) = shown {
        v["display"] = json!(s);
        v["value_string"] = json!(s);
    }
    v
}

fn bridge() -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script("get_performance_state", vec![state()]);
    b.script(
        "get_track_info",
        vec![json!({"index": 0, "name": "Pad", "devices": [
        {"index": 0, "name": "Evolving Pad", "class_name": "InstrumentGroupDevice", "type": "rack"},
        {"index": 1, "name": "Reverb", "class_name": "Reverb", "type": "audio_effect"}]})],
    );
    b.script("get_device_parameters", vec![json!({"track_index": 0, "device": {"index": 0, "name": "Evolving Pad", "class_name": "InstrumentGroupDevice", "parameters": [
        param(0, "Device On", 1.0, 0.0, 1.0, None), param(1, "Chain Selector", 0.0, 0.0, 127.0, None),
        param(2, "Cutoff", 79.0, 0.0, 127.0, Some("62 %")), param(3, "Res", 25.4, 0.0, 127.0, Some("20 %")),
        param(4, "Space", 0.0, 0.0, 127.0, Some("0 %")), param(5, "Macro 4", 0.0, 0.0, 127.0, None)]}})]);
    b.script("set_device_parameters", vec![json!({"track_index": 0, "device_index": 0, "device": "Evolving Pad", "class_name": "InstrumentGroupDevice", "parameters": [
        param(2, "Cutoff", 47.0, 0.0, 127.0, Some("37 %")), param(3, "Res", 38.1, 0.0, 127.0, Some("30 %"))]})]);
    b
}

#[tokio::test]
async fn shape_sound_asks_the_rack_macros_first_and_writes_them_in_one_round_trip() {
    let b = bridge();
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Pad"),
                cutoff: Some(json!("-25%")),
                resonance: Some(json!("+10%")),
                attack: Some(json!(0.5)),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec![
            "get_performance_state",
            "get_track_info",
            "get_device_parameters",
            "set_device_parameters"
        ]
    );
    let sent = b.sent();
    assert_eq!(
        sent[2].1,
        json!({"track_index": 0, "kind": "track", "device_index": 0}),
        "the rack, not the reverb after it"
    );
    let values = sent[3].1["values"].as_array().unwrap().clone();
    assert_eq!(values.len(), 2, "attack has no macro: not written");
    assert_eq!(values[0]["index"], 2);
    assert!(
        (values[0]["value"].as_f64().unwrap() - (79.0 - 0.25 * 127.0)).abs() < 1e-9,
        "−25% of the range from where it sits"
    );
    assert_eq!(values[1]["index"], 3);
    assert!((values[1]["value"].as_f64().unwrap() - (25.4 + 0.1 * 127.0)).abs() < 1e-9);
    let t = text_of(&r);
    assert!(
        t.starts_with("Pad (rack 'Evolving Pad', InstrumentGroupDevice) — 2 of 3 applied:\n"),
        "partial success is never reported as success: {t}"
    );
    assert!(
        t.contains("  applied  macro 'Cutoff' 62 % → 37 %  (cutoff)\n"),
        "{t}"
    );
    assert!(
        t.contains("  applied  macro 'Res' 20 % → 30 %  (resonance)\n"),
        "{t}"
    );
    assert!(
        t.contains("  skipped  attack — no macro on this device says it"),
        "every word asked for gets a line: {t}"
    );
    assert!(
        t.contains(
            "Racks are asked first: their macros are how the preset's maker meant it to be shaped."
        ),
        "{t}"
    );
    assert!(t.contains("Words this device answers to: cutoff (Cutoff), resonance (Res), reverb (Space). Sweep one in a cue: {\"ramp\": {\"sound\": \"cutoff\", \"track\": \"Pad\", \"to\": 0.8}, \"bars\": 8}."), "{t}");

    // No words: nothing is read.
    let before = b.commands().len();
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Pad"),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    assert!(
        is_error(&r)
            && text_of(&r).starts_with("Give at least one word: cutoff, resonance, attack"),
        "{}",
        text_of(&r)
    );
    assert_eq!(b.commands().len(), before);
}

#[tokio::test]
async fn an_unknown_device_lists_its_parameters_and_set_device_parameter_takes_a_name() {
    let b = bridge();
    b.script(
        "get_track_info",
        vec![json!({"index": 1, "name": "Bass", "devices": [
        {"index": 0, "name": "Serum", "class_name": "PluginDevice", "type": "instrument"}]})],
    );
    b.script("get_device_parameters", vec![json!({"track_index": 1, "device": {"index": 0, "name": "Serum", "class_name": "PluginDevice", "parameters": [
        param(0, "Enable", 1.0, 0.0, 1.0, None), param(1, "Cutoff", 0.4, 0.0, 1.0, None), param(2, "Res", 0.1, 0.0, 1.0, None),
        param(3, "Env1 Atk", 0.0, 0.0, 1.0, None)]}})]);
    b.script(
        "set_device_parameter",
        vec![
            json!({"track_index": 1, "kind": "track", "track_name": "Bass",
                    "device_index": 0, "device": "Serum", "parameter_index": 3,
                    "name": "Env1 Atk", "old_value": 0.0, "value": 0.25,
                    "min": 0.0, "max": 1.0}),
        ],
    );
    let server = server_with(b.clone());
    // A word the device has no name for.
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Bass"),
                width: Some(json!(0.8)),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert_eq!(
        text_of(&r),
        "Bass ('Serum', PluginDevice) is not in the vocabulary for width; its parameters by name are: Enable, Cutoff, Res, Env1 Atk. adv_set_device_parameter takes a name substring: {\"track\": \"Bass\", \"device\": \"Serum\", \"parameter\": \"<name>\", \"value\": …}."
    );
    assert_eq!(
        b.commands().last().unwrap(),
        "get_device_parameters",
        "nothing written"
    );
    // By name substring: one read, one write.
    let before = b.commands().len();
    let r = server
        .run(
            &tools::SET_DEVICE_PARAMETER,
            SetDeviceParameterParams {
                track: Some(json!("Bass")),
                parameter: Some("atk".into()),
                value: json!(0.25),
                ..Default::default()
            },
            tools::set_device_parameter_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands()[before..],
        [
            "get_performance_state",
            "get_device_parameters",
            "set_device_parameter"
        ]
    );
    assert_eq!(b.sent().last().unwrap().1["parameter_index"], 3);
    assert_eq!(b.sent().last().unwrap().1["value"], 0.25);
    assert_eq!(
        text_of(&r),
        "Bass · Serum · Env1 Atk: 0 → 0.25 (raw 0 → 0.25 of 0 … 1)."
    );
    let r = server
        .run(
            &tools::SET_DEVICE_PARAMETER,
            SetDeviceParameterParams {
                track: Some(json!("Bass")),
                parameter: Some("wobble".into()),
                value: json!(0.25),
                ..Default::default()
            },
            tools::set_device_parameter_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("no parameter named 'wobble' on 'Serum'; its parameters: Enable, Cutoff, Res, Env1 Atk"), "{}", text_of(&r));
    let r = server
        .run(
            &tools::SET_DEVICE_PARAMETER,
            SetDeviceParameterParams {
                track: Some(json!("Bass")),
                value: json!(0.25),
                ..Default::default()
            },
            tools::set_device_parameter_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("give parameter_index, or parameter"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn a_cue_ramp_takes_a_word() {
    let b = bridge();
    b.script(
        "schedule_cue",
        vec![json!({"id": 9, "name": "sweep", "steps": []})],
    );
    let server = server_with(b.clone());
    let p: CueParams = serde_json::from_value(json!({"name": "sweep", "steps": [
        {"from": {"bar": 17}, "bars": 8, "ramp": {"sound": "cutoff", "track": "Pad", "to": 0.8}}
    ]}))
    .unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec![
            "get_performance_state",
            "get_track_info",
            "get_device_parameters",
            "schedule_cue"
        ]
    );
    let step = b.sent()[3].1["cue"]["steps"][0].clone();
    assert_eq!(
        (
            step["action"].as_str(),
            step["target"].as_str(),
            step["track_index"].as_i64(),
            step["device_index"].as_i64(),
            step["parameter_index"].as_i64()
        ),
        (Some("ramp"), Some("device"), Some(0), Some(0), Some(2))
    );
    assert!(
        (step["to"].as_f64().unwrap() - 0.8 * 127.0).abs() < 1e-9,
        "a fraction of the parameter's range"
    );
    assert!(
        text_of(&r).contains("Pad device 0 parameter 2 → 101.60"),
        "{}",
        text_of(&r)
    );
    let p: CueParams = serde_json::from_value(json!({"steps": [{"from": {"bar": 17}, "bars": 8, "ramp": {"sound": "attack", "track": "Pad", "to": 0.8}}]})).unwrap();
    let r = server.run(&tools::CUE, p, tools::cue_body).await;
    assert!(
        is_error(&r)
            && text_of(&r).contains("step 1: 'attack' is not in the vocabulary for 'Evolving Pad'"),
        "{}",
        text_of(&r)
    );
}
