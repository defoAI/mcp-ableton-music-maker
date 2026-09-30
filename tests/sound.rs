//! Sound without leaving the flow: shape_sound resolves words against a
//! rack's macros, an instrument's parameters or an unknown device's names,
//! writes several in one round trip and reports before/after in Live's
//! units; set_device_parameter takes a name substring; a cue ramp takes a
//! word.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::performance::CueParams;
use mcp_ableton_music_maker::tools::{
    self, DeviceParams, SetDeviceParameterParams, ShapeSoundParams,
};
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

// ── Inside a rack (#74) ─────────────────────────────────────────────────────

/// A drum rack's per-pad Simpler and an instrument rack's nested Operator are
/// where the sound lives; the eight macros are only what the rack's maker
/// exposed. These run against the **real Remote Script** on the model, over a
/// real socket, and ask the set what actually moved — a reply that says
/// "applied" proves nothing about which parameter it landed on.
/// A drum rack with sounds on it.
///
/// This used to load the bare **Drum Rack**, and on a real Live that device
/// has 128 `drum_pads` and **no chains** — the grid with nothing on it
/// (measured on 12.4.6, 2026-09-20). Every test below therefore passed
/// against the model, which filled four pads on it, and failed against Live,
/// where the rack the fixture loaded was empty. A model that is generous
/// where Live is bare hides exactly the feature it exists to prove.
///
/// So it loads a **kit** now, by name rather than by URI — Live's URI is its
/// own (`query:Drums#FileId_5487` on one install, something else on the next)
/// and both a Live 12 Suite and the model have an 808 Core Kit.
fn drum_rack_set() -> (mcp_ableton_music_maker::tools::Server, usize) {
    let (server, bridge) = common::server_on_fake_live();
    let set = common::LiveSet::of(bridge.as_ref());
    set.build(&[("Drums", "midi", "")]);
    let track = set.track_index("Drums").expect("the track is in the set");
    let hit = server
        .live()
        .send_command(
            "search_browser",
            Some(json!({"query": "808 Core Kit", "category": "drums", "limit": 1})),
        )
        .expect("the browser answers");
    let uri = hit["items"][0]["uri"]
        .as_str()
        .unwrap_or_else(|| panic!("no 808 Core Kit in this browser: {hit}"))
        .to_string();
    server
        .live()
        .send_command(
            "load_browser_item",
            Some(json!({"track_index": track, "item_uri": uri})),
        )
        .expect("the kit loads");
    (server, track)
}

/// The pads, the devices on them and their parameters are **this kit's**, and
/// a kit is the producer's: an 808 Core Kit's first pad is "Bass Drum" and
/// holds an Instrument Rack; an LDre Mellow Kit's is "Kick LDre 5" and holds
/// a DrumCell. Both measured on Live 12.4.6.
///
/// So the tests that name "Kick", "Snap" or a particular display string are
/// about the model's kit and say so by standing down against a real Live —
/// the same rule the plug-in tests follow. What must hold on *any* kit is
/// proven by `in_chain_reaches_a_pad_of_whatever_kit_this_live_has`.
fn model_only() -> bool {
    common::targets_a_real_live()
}

fn nested_value(
    server: &mcp_ableton_music_maker::tools::Server,
    track: usize,
    chain: usize,
    device: usize,
    parameter: usize,
) -> f64 {
    let reply = server
        .live()
        .send_command(
            "get_device_parameters",
            Some(json!({"track_index": track, "device_index": 0})),
        )
        .expect("the rack reads back");
    reply["device"]["chains"][chain]["devices"][device]["parameters"][parameter]["value"]
        .as_f64()
        .unwrap_or_else(|| panic!("no such nested parameter: {reply}"))
}

fn macro_value(
    server: &mcp_ableton_music_maker::tools::Server,
    track: usize,
    parameter: usize,
) -> f64 {
    let reply = server
        .live()
        .send_command(
            "get_device_parameters",
            Some(json!({"track_index": track, "device_index": 0})),
        )
        .expect("the rack reads back");
    reply["device"]["parameters"][parameter]["value"]
        .as_f64()
        .unwrap_or_else(|| panic!("no such macro: {reply}"))
}

#[tokio::test]
async fn in_chain_moves_the_pads_own_device_and_leaves_the_macros_alone() {
    if model_only() {
        return;
    }
    let (server, track) = drum_rack_set();
    // The kick pad's Simpler: "Device On", then Snap, Volume, Decay, Filter Freq.
    let before = nested_value(&server, track, 0, 0, 3);
    let macros_before = macro_value(&server, track, 1);
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                in_chain: Some("Kick".into()),
                decay: Some(json!(0.9)),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(text.contains("Kick"), "it says which pad it moved: {text}");
    let after = nested_value(&server, track, 0, 0, 3);
    assert!(
        (after - 0.9).abs() < 1e-6,
        "the pad's own Decay moved to 0.9, not {after} (was {before})"
    );
    assert_eq!(
        macro_value(&server, track, 1),
        macros_before,
        "no rack macro moved"
    );
}

/// Without `in_chain` nothing changes: the rack's own macros answer, exactly
/// as they did before #74.
#[tokio::test]
async fn without_in_chain_the_rack_macro_is_still_what_answers() {
    if model_only() {
        return;
    }
    let (server, track) = drum_rack_set();
    let nested_before = nested_value(&server, track, 0, 0, 3);
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                set: [("Macro 1".to_string(), json!(0.5))].into_iter().collect(),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        (macro_value(&server, track, 1) - 63.5).abs() < 1e-6,
        "half of the macro's 0–127 range"
    );
    assert_eq!(
        nested_value(&server, track, 0, 0, 3),
        nested_before,
        "nothing inside the rack moved"
    );
}

/// A drum pad is known by its name, and the second device on a pad is
/// reached by naming it after a slash.
///
/// Its note is *not* a way in: `Chain.out_note` is the note the chain plays
/// and Live reads the same one on every pad, so "C1" names nothing and "C3"
/// would name all of them (#75, measured on 12.4.6). The pad's own note is on
/// `DrumPad.note`, which `adv_get_drum_rack_pads` reports with the pad's name.
#[tokio::test]
async fn a_pad_answers_to_its_name_and_a_second_device_to_its_name() {
    if model_only() {
        return;
    }
    let (server, track) = drum_rack_set();
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                in_chain: Some("C1".into()),
                set: [("Snap".to_string(), json!(0.25))].into_iter().collect(),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    assert!(
        is_error(&r),
        "a note must not resolve to a pad: {}",
        text_of(&r)
    );
    assert!(
        text_of(&r).contains("Kick"),
        "it lists the pads: {}",
        text_of(&r)
    );

    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                in_chain: Some("Kick".into()),
                set: [("Snap".to_string(), json!(0.25))].into_iter().collect(),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        (nested_value(&server, track, 0, 0, 1) - 0.25).abs() < 1e-6,
        "the kick pad, by its name"
    );

    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                in_chain: Some("Kick/Saturator".into()),
                drive: Some(json!(0.8)),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(text.contains("Saturator"), "{text}");
    assert!(
        (nested_value(&server, track, 0, 1, 1) - 0.8).abs() < 1e-6,
        "the Saturator's Drive, not the Simpler's"
    );
}

/// A chain nobody has is an error that lists the ones the rack does have,
/// and nothing is written.
#[tokio::test]
async fn an_unknown_chain_names_what_the_rack_holds_and_writes_nothing() {
    if model_only() {
        return;
    }
    let (server, track) = drum_rack_set();
    let before = nested_value(&server, track, 0, 0, 3);
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                in_chain: Some("Cowbell".into()),
                decay: Some(json!(0.9)),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    let text = text_of(&r);
    assert!(is_error(&r), "{text}");
    assert!(text.contains("no chain called 'Cowbell'"), "{text}");
    // The pads list by name alone: `out_note` is the same on every one, so a
    // note in the label told a producer nothing (#81).
    assert!(text.contains("'Kick'") && text.contains("Snare"), "{text}");
    assert!(!text.contains("(C1)"), "no pad carries a note here: {text}");
    assert_eq!(nested_value(&server, track, 0, 0, 3), before);
}

/// The raw layer reaches as deep as the artist layer: the same argument on
/// `adv_get_device_parameters` and `adv_set_device_parameter`.
#[tokio::test]
async fn the_raw_layer_reads_and_writes_the_same_nested_device() {
    if model_only() {
        return;
    }
    let (server, track) = drum_rack_set();
    let r = server
        .run(
            &tools::GET_DEVICE_PARAMETERS,
            tools::DeviceParams {
                track: Some(json!("Drums")),
                in_chain: Some("Snare".into()),
                ..Default::default()
            },
            tools::get_device_parameters_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(text.contains("Inside '808 Core Kit'"), "{text}");
    assert!(text.contains("Snap") && text.contains("Decay"), "{text}");
    assert!(
        !text.contains("Macro 4"),
        "the pad's parameters, not the rack's: {text}"
    );

    // A display string is resolved against what Live shows, the same as it is
    // for a top-level device.
    let r = server
        .run(
            &tools::SET_DEVICE_PARAMETER,
            tools::SetDeviceParameterParams {
                track: Some(json!("Drums")),
                in_chain: Some("Snare".into()),
                parameter: Some("Decay".into()),
                value: json!("2010 ms"),
                ..Default::default()
            },
            tools::set_device_parameter_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(text.contains("2010 ms") || text.contains("2009"), "{text}");
    // The fake's Simpler shows 10 + value * 4000 ms, so 2010 ms is 0.5.
    let landed = nested_value(&server, track, 1, 0, 3);
    assert!(
        (landed - 0.5).abs() < 0.01,
        "the value Live displayed as asked, not a guess: {landed}"
    );
}

/// #67 keeps its key: a nested Simpler is learned as a Simpler, not as a
/// position in this song, so the next song reuses what was learned here.
#[tokio::test]
async fn a_nested_device_joins_the_vocabulary_under_its_own_name() {
    if model_only() {
        return;
    }
    let (server, _) = drum_rack_set();
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                in_chain: Some("Snare".into()),
                decay: Some(json!(0.6)),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let r = server
        .run(
            &tools::DEVICE_VOCABULARY,
            tools::DeviceVocabularyParams {
                action: "show".into(),
                device: Some("Snare".into()),
            },
            tools::device_vocabulary_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(
        text.contains("OriginalSimpler") || text.contains("Decay"),
        "the pad's device is remembered by what it is: {text}"
    );
}

/// What `in_chain` must do on whatever kit this Live has.
///
/// Nothing here names a pad, a device or a parameter: all three are read off
/// the set first, because they belong to the producer's kit. What is asserted
/// is the mechanism — the write lands on the pad's own device, Live's own
/// display string comes back for it, and the rack's macros do not move.
///
/// This is the test that was missing. The six above prove the same thing
/// against a kit whose every parameter is known, which is how a whole feature
/// came to pass here and fail against Live twice over (#81, and the bare Drum
/// Rack before it).
#[tokio::test]
async fn in_chain_reaches_a_pad_of_whatever_kit_this_live_has() {
    let (server, track) = drum_rack_set();
    let read = || {
        server
            .live()
            .send_command(
                "get_device_parameters",
                Some(json!({"track_index": track, "device_index": 0})),
            )
            .expect("the rack reads back")
    };

    let rack = read();
    let chain = rack["device"]["chains"][0].clone();
    let pad = chain["chain_name"]
        .as_str()
        .unwrap_or_else(|| panic!("the kit loaded with no chains: {rack}"))
        .to_string();
    let params = chain["devices"][0]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("the pad's device has no parameters: {rack}"))
        .clone();
    // Not "Device On": a switch has two values and proves nothing about range.
    let (index, param) = params
        .iter()
        .enumerate()
        .find(|(_, p)| {
            p["name"].as_str() != Some("Device On")
                && p["max"].as_f64().unwrap_or(0.0) > p["min"].as_f64().unwrap_or(0.0)
        })
        .unwrap_or_else(|| panic!("nothing with a range on the pad's device: {rack}"));
    let name = param["name"].as_str().unwrap_or_default().to_string();
    let (min, max) = (
        param["min"].as_f64().unwrap_or(0.0),
        param["max"].as_f64().unwrap_or(1.0),
    );
    let before = param["value"].as_f64().unwrap_or(0.0);
    // `set` takes a fraction 0–1 of the parameter's range, not a raw value —
    // a real kit's macro runs 0–127 and the model's Simpler 0–1, and the
    // artist surface is the same words either way. Never the fraction it
    // already sits at, or the write proves nothing.
    let here = (before - min) / (max - min);
    let fraction = if (here - 1.0 / 3.0).abs() < 1e-3 {
        2.0 / 3.0
    } else {
        1.0 / 3.0
    };
    let target = min + (max - min) * fraction;
    let macros_before: Vec<f64> = rack["device"]["parameters"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|p| p["value"].as_f64().unwrap_or(0.0))
        .collect();

    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                in_chain: Some(pad.clone()),
                set: [(name.clone(), json!(fraction))].into_iter().collect(),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(
        text.contains(&pad),
        "the reply says which pad it went to: {text}"
    );

    let after = read();
    let landed = after["device"]["chains"][0]["devices"][0]["parameters"][index]["value"]
        .as_f64()
        .unwrap_or_else(|| panic!("the parameter vanished: {after}"));
    assert!(
        (landed - target).abs() < (max - min) * 1e-3,
        "'{name}' on pad '{pad}' should be {target}, is {landed} (was {before})"
    );

    let macros_after: Vec<f64> = after["device"]["parameters"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|p| p["value"].as_f64().unwrap_or(0.0))
        .collect();
    assert_eq!(
        macros_before, macros_after,
        "the rack's own macros must not move when a pad is shaped"
    );
}

// ── What a rack's readout says, and what it says when a word misses ────────
//
// #74's surface is only as good as what it prints: a chain a producer cannot
// type back is a chain they cannot reach.

async fn read_device(
    server: &mcp_ableton_music_maker::tools::Server,
    track: usize,
    in_chain: Option<&str>,
) -> String {
    let r = server
        .run(
            &tools::GET_DEVICE_PARAMETERS,
            DeviceParams {
                track_index: Some(track as i64),
                in_chain: in_chain.map(str::to_string),
                ..Default::default()
            },
            tools::get_device_parameters_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    text
}

/// The chains listed under a rack are named the way `in_chain` names them.
///
/// This line used to read `c["name"]`, which the script does not send — the
/// key is `chain_name` — so every chain printed as the literal word "chain"
/// followed by the device on it. On a real 808 Core Kit the readout said
/// "chain (Audio Effect Rack)" while the error a line later said 'a Mix Bus':
/// two listings of the same rack, disagreeing, and only one of them typeable
/// (measured on Live 12.4.6, 2026-09-21).
#[tokio::test]
async fn the_chains_are_listed_by_the_name_in_chain_takes() {
    let (server, track) = drum_rack_set();
    let out = read_device(&server, track, None).await;
    let chains = out
        .lines()
        .find(|l| l.starts_with("Chains:"))
        .unwrap_or_else(|| panic!("no chain listing in the readout:\n{out}"))
        .to_string();
    assert!(
        !chains.contains("chain ("),
        "the chains are listed as the literal word 'chain': {chains}"
    );
    // The name the listing printed resolves: in_chain takes it and finds one.
    let first = chains
        .split('\'')
        .nth(1)
        .unwrap_or_else(|| panic!("no quoted chain name: {chains}"))
        .to_string();
    let inside = read_device(&server, track, Some(&first)).await;
    assert!(
        inside.starts_with("Inside "),
        "the name the readout printed did not resolve: {first} -> {inside}"
    );
}

/// A word that misses on a rack points at what the rack holds.
///
/// A rack's own parameters are its macros, and a macro is called "Macro 7"
/// until someone names it — so "the snare's decay", the thing #74 was filed
/// about, misses at the top and lands one level down. The error has to say
/// that, or the producer is left reading a list of macros.
#[tokio::test]
async fn a_word_that_misses_on_a_rack_names_what_is_inside_it() {
    let (server, track) = drum_rack_set();
    let r = server
        .run(
            &tools::SHAPE_SOUND,
            ShapeSoundParams {
                track: json!("Drums"),
                // Not a word a drum rack's macros answer to, on any kit.
                lfo_rate: Some(json!(0.2)),
                ..Default::default()
            },
            tools::shape_sound_body,
        )
        .await;
    let err = text_of(&r);
    assert!(is_error(&r), "a drum rack answered to lfo_rate: {err}");
    assert!(err.contains("is a rack"), "{err}");
    assert!(err.contains("in_chain"), "{err}");
    assert!(err.contains("\"lfo_rate\""), "the word was dropped: {err}");
    // What it suggests is a chain that exists.
    let suggested = err
        .split("\"in_chain\": \"")
        .nth(1)
        .and_then(|t| t.split('"').next())
        .unwrap_or_else(|| panic!("the hint names no chain: {err}"))
        .to_string();
    let inside = read_device(&server, track, Some(&suggested)).await;
    assert!(inside.starts_with("Inside "), "{inside}");
}

/// A nested device reads in Live's own words, not as a percentage.
///
/// The rack walk carries `value`, `min` and `max` for a chain's devices and
/// nothing else, so a pad's filter read "100%" of 0…127 where the same
/// parameter on a top-level device reads "22.0 kHz". The server asks Live
/// (`str_for_value`, and `value_items` only where the parameter is quantized,
/// because reading it anywhere else raises) in one batch. Measured on a real
/// 808 Core Kit (12.4.6, 2026-09-21): Low Pass Filter 100% → 22.0 kHz over
/// 30.0 Hz … 22.0 kHz, Level 85% → 0.0 dB.
#[tokio::test]
async fn a_nested_device_reads_in_lives_own_words() {
    let (server, track) = drum_rack_set();
    let rack = server
        .live()
        .send_command(
            "get_device_parameters",
            Some(json!({"track_index": track, "device_index": 0})),
        )
        .expect("the rack reads back");
    let chain = rack["device"]["chains"][0]["chain_name"]
        .as_str()
        .unwrap_or_else(|| panic!("the kit loaded with no chains: {rack}"))
        .to_string();
    // What the walk alone carries: no display on any of the pad's parameters.
    let walked = rack["device"]["chains"][0]["devices"][0]["parameters"]
        .as_array()
        .expect("the pad's device has parameters")
        .clone();
    assert!(
        walked.iter().all(|p| p.get("display").is_none()),
        "the walk already carries displays; this test measures the wrong thing"
    );
    let out = read_device(&server, track, Some(&chain)).await;
    // A chooser reads as its labels rather than as a percentage of 0 … 1.
    assert!(
        out.contains("[Off, On]"),
        "no chooser read back as its labels:\n{out}"
    );
    assert!(
        !out.contains("  0 … 1\n"),
        "a chooser is still printed as a raw range:\n{out}"
    );
}
