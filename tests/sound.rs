//! Sound without leaving the flow: shape_sound resolves words against a
//! rack's macros, an instrument's parameters or an unknown device's names,
//! writes several in one round trip and reports before/after in Live's
//! units; set_device_parameter takes a name substring; a cue ramp takes a
//! word.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::performance::CueParams;
use mcp_ableton_music_maker::tools::{self, SetDeviceParameterParams, ShapeSoundParams};
use rmcp::model::CallToolResult;
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
async fn a_batch_summary_counts_the_steps_that_only_half_applied() {
    // A word no parameter answers to is a line in shape_sound's own reply.
    // Inside a long batch that line used to be as invisible as it was before
    // the per-word lines existed: the summary is where it has to show.
    let b = bridge();
    let server = server_with(b.clone());
    let steps: Vec<Value> = (0..11)
        .map(|_| {
            json!({"tool": "shape_sound",
                        "args": {"track": "Pad", "cutoff": "-25%", "attack": 0.5}})
        })
        .collect();
    let p: tools::BatchParams = serde_json::from_value(json!({"steps": steps})).unwrap();
    let r = server.run(&tools::BATCH, p, tools::batch_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("11 steps, 11 ok — 11 with something skipped\n"),
        "{t}"
    );
    assert!(
        t.contains("  shape_sound ×11 ✓ (11 with something skipped)\n"),
        "{t}"
    );
    assert!(
        t.contains("Skipped:\n  1. shape_sound: skipped  attack — no macro on this device says it"),
        "the skipped lines are pulled up under the summary: {t}"
    );
}

/// The Beat Repeat case from the producer's session: two writes to
/// Variation in a row, both answered "Set Variation 0.0 → 0.0", both counted
/// as steps that happened.
fn beat_repeat(landing: Value) -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script("get_performance_state", vec![state()]);
    b.script(
        "get_track_info",
        vec![json!({"index": 1, "name": "Bass", "devices": [
        {"index": 0, "name": "Beat Repeat", "class_name": "BeatRepeat", "type": "audio_effect"}]})],
    );
    b.script("get_device_parameters", vec![json!({"track_index": 1, "device": {"index": 0, "name": "Beat Repeat", "class_name": "BeatRepeat", "parameters": [
        param(0, "Device On", 1.0, 0.0, 1.0, None),
        param(1, "Variation", 0.0, 0.0, 1.0, None)]}})]);
    let mut reply = json!({"track_index": 1, "kind": "track", "track_name": "Bass",
        "device_index": 0, "device": "Beat Repeat", "parameter_index": 1,
        "name": "Variation", "old_value": 0.0, "value": 0.0, "min": 0.0, "max": 1.0});
    for (k, v) in landing.as_object().expect("an object").clone() {
        reply[k] = v;
    }
    b.script("set_device_parameter", vec![reply]);
    b
}

async fn set_variation(b: Arc<FakeBridge>) -> CallToolResult {
    server_with(b)
        .run(
            &tools::SET_DEVICE_PARAMETER,
            SetDeviceParameterParams {
                track: Some(json!("Bass")),
                parameter: Some("Variation".into()),
                value: json!(0.35),
                ..Default::default()
            },
            tools::set_device_parameter_body,
        )
        .await
}

#[tokio::test]
async fn a_write_live_ignored_is_an_error_naming_the_reason() {
    // Switched off: a rack macro owns it, or the device disabled it.
    let r = set_variation(beat_repeat(
        json!({"asked": 0.35, "landed": false, "is_enabled": false, "is_quantized": false}),
    ))
    .await;
    let t = text_of(&r);
    assert!(
        is_error(&r),
        "a step that did not happen is not a step: {t}"
    );
    assert!(
        t.contains("'Variation' did not move") && t.contains("is_enabled false"),
        "{t}"
    );

    // Automated: the envelope overwrites the write on the next playback tick.
    let r = set_variation(beat_repeat(json!({"asked": 0.35, "landed": false,
        "is_enabled": true, "is_quantized": false, "automation_state": 1})))
    .await;
    let t = text_of(&r);
    assert!(is_error(&r), "{t}");
    assert!(
        t.contains("automated (automation_state 1)") && t.contains("envelope"),
        "{t}"
    );

    // Nothing in the reply says why: still an error, and it says that too.
    let r = set_variation(beat_repeat(
        json!({"asked": 0.35, "landed": false, "is_enabled": true, "is_quantized": false}),
    ))
    .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Nothing in the reply says why"),
        "{}",
        text_of(&r)
    );

    // An older script sends no `landed`: nothing may be assumed, so the
    // before-and-after echo stands as it did.
    let r = set_variation(beat_repeat(json!({}))).await;
    assert!(!is_error(&r), "{}", text_of(&r));
}

#[tokio::test]
async fn a_quantized_write_that_snapped_says_which_step_it_landed_on() {
    // Beat Repeat's Grid asked for 0.25 and given step 0, reported as a
    // success with no hint that it had snapped.
    let b = beat_repeat(json!({"asked": 0.25, "landed": false, "is_enabled": true,
        "is_quantized": true, "old_value": 0.0, "value": 0.125,
        "display": "1/16", "old_display": "1/32"}));
    let r = set_variation(b).await;
    let t = text_of(&r);
    assert!(!is_error(&r), "a snap is a success, not a failure: {t}");
    assert!(
        t.contains("Variation takes steps: 0.25 is nearest 1/16."),
        "the producer sees the snap instead of hearing it later: {t}"
    );

    // It snapped to the step it was already on: that is worth saying too.
    let b = beat_repeat(json!({"asked": 0.25, "landed": false, "is_enabled": true,
        "is_quantized": true, "old_value": 0.0, "value": 0.0, "display": "1/32"}));
    let t = text_of(&set_variation(b).await);
    assert!(t.contains("the step it was already on"), "{t}");
}

#[tokio::test]
async fn shape_sound_skips_a_word_whose_parameter_refused_to_move() {
    let b = bridge();
    // Cutoff lands; Res is switched off and does not.
    b.script("set_device_parameters", vec![json!({"track_index": 0, "device_index": 0,
        "device": "Evolving Pad", "class_name": "InstrumentGroupDevice", "parameters": [
        {"index": 2, "name": "Cutoff", "old_value": 79.0, "value": 47.0, "min": 0.0, "max": 127.0,
         "display": "37 %", "asked": 47.0, "landed": true, "is_enabled": true, "is_quantized": false},
        {"index": 3, "name": "Res", "old_value": 25.4, "value": 25.4, "min": 0.0, "max": 127.0,
         "display": "20 %", "asked": 38.1, "landed": false, "is_enabled": false, "is_quantized": false}]})]);
    let r = server_with(b)
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Pad"),
                cutoff: Some(json!("-25%")),
                resonance: Some(json!("+10%")),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    let t = text_of(&r);
    assert!(
        t.starts_with("Pad (rack 'Evolving Pad', InstrumentGroupDevice) — 1 of 2 applied:\n"),
        "the header counts what landed, not what was sent: {t}"
    );
    assert!(
        t.contains("  applied  macro 'Cutoff' 62 % → 37 %  (cutoff)"),
        "{t}"
    );
    assert!(
        t.contains("  skipped  resonance — 'Res' did not move") && t.contains("is_enabled false"),
        "{t}"
    );
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
