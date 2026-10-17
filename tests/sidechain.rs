//! Sidechains through the tools, against the fake Live (the real Remote
//! Script on the model of Live 12.4.6, measured 2026-10-15) — the tests the
//! requirements ask for, R1–R10. The duck itself (R8) is audio, which the
//! model does not render: it is measured from a capture through a scripted
//! bridge, and on a real Live in the session that built this.

mod common;

use common::{is_error, server_on_fake_live, server_with, text_of, FakeBridge, LiveSet};
use mcp_ableton_music_maker::sidechain::{set_sidechain_body, SetSidechainParams};
use mcp_ableton_music_maker::tools::{self, BuildSongParams, Server};
use serde_json::{json, Value};

async fn key(server: &Server, args: Value) -> (bool, String) {
    let p: SetSidechainParams = serde_json::from_value(args).expect("set_sidechain args");
    let r = server
        .run(&tools::SET_SIDECHAIN, p, set_sidechain_body)
        .await;
    (is_error(&r), text_of(&r))
}

async fn build(server: &Server, doc: Value) -> (bool, String) {
    let p: BuildSongParams = serde_json::from_value(doc).expect("build_song doc");
    let r = server
        .run(&tools::BUILD_SONG, p, tools::build_song_body)
        .await;
    (is_error(&r), text_of(&r))
}

/// Kick (audio) and Bass (MIDI with Operator) with a Compressor on it.
fn kick_and_bass(set: &LiveSet) -> (usize, usize) {
    let made = set.build(&[
        ("Kick", "audio", ""),
        ("Bass", "midi", "query:Synths#Operator"),
    ]);
    set.ask(
        "ensure_effects",
        json!({"kind": "track", "index": made[1], "on_existing": "add",
               "effects": [{"uri": "query:AudioFx#Compressor", "name": "Compressor"}]}),
    );
    (made[0], made[1])
}

/// Every sidechain on a track, as get_context's JSON gives them.
fn sidechains(set: &LiveSet, track: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let ctx = set.context();
    ctx["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == track)
        .map(|t| {
            t["sidechains"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e["device"].as_str().unwrap_or("").to_string(),
                        e["sidechain"]["source"].as_str().map(str::to_string),
                        e["sidechain"]["tap"].as_str().map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── R2 / R1 ──────────────────────────────────────────────────────────────────

/// R2's test: each tap set and read back, by R1's reading.
#[tokio::test]
async fn a_compressor_is_keyed_with_each_tap_and_read_back() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    kick_and_bass(&set);
    for (tap, shown) in [
        ("pre_fx", "Pre FX"),
        ("post_fx", "Post FX"),
        ("post_mixer", "Post Mixer"),
    ] {
        let (err, t) = key(
            &server,
            json!({"track": "Bass", "device": "Compressor", "source": "Kick", "tap": tap, "eq_on": false}),
        )
        .await;
        assert!(!err, "{t}");
        assert!(
            t.contains(&format!(
                "Bass › Compressor: keyed from Kick ({shown}) · S/C On"
            )),
            "{t}"
        );
        assert!(t.contains("read back from Live"), "{t}");
        assert_eq!(
            sidechains(&set, "Bass"),
            vec![("Compressor".into(), Some("Kick".into()), Some(tap.into()))]
        );
    }
}

/// R2's refusals, each leaving the routing as it was.
#[tokio::test]
async fn sources_are_matched_exactly_and_refusals_change_nothing() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let (kick, _) = kick_and_bass(&set);
    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Kick"}),
    )
    .await;
    assert!(!err, "{t}");
    let before = sidechains(&set, "Bass");

    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Kik"}),
    )
    .await;
    assert!(err && t.contains("no track is called 'Kik'"), "{t}");

    set.ask(
        "set_track_name",
        json!({"track_index": kick, "name": "Kick B"}),
    );
    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Kick"}),
    )
    .await;
    assert!(err, "{t}");
    assert!(
        t.contains("no track is called exactly 'Kick'") && t.contains("Did you mean 'Kick B'"),
        "{t}"
    );
    set.ask(
        "set_track_name",
        json!({"track_index": kick, "name": "Kick"}),
    );

    set.build(&[("Twin", "audio", "")]);
    set.ask(
        "create_tracks",
        json!({"tracks": [{"name": "Twin", "kind": "audio"}], "on_existing": "add"}),
    );
    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Twin"}),
    )
    .await;
    assert!(err && t.contains("'Twin' names 2 tracks"), "{t}");

    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Bass"}),
    )
    .await;
    assert!(err && t.contains("cannot key from its own track"), "{t}");

    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Kick", "tap": "side"}),
    )
    .await;
    assert!(
        err && t.contains("tap is pre_fx, post_fx or post_mixer"),
        "{t}"
    );

    assert_eq!(sidechains(&set, "Bass"), before, "nothing changed");
}

/// R2: Live's own undo does not cover routing; `undo: true` does.
#[tokio::test]
async fn undo_puts_the_previous_routing_back() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    kick_and_bass(&set);
    key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Kick", "tap": "pre_fx"}),
    )
    .await;
    key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Kick", "tap": "post_mixer"}),
    )
    .await;
    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "undo": true}),
    )
    .await;
    assert!(!err, "{t}");
    assert!(
        t.contains("Put back Bass › Compressor: keyed from Kick (Pre FX)"),
        "{t}"
    );
    assert_eq!(sidechains(&set, "Bass")[0].2.as_deref(), Some("pre_fx"));
}

// ── R3: the devices Live cannot route ────────────────────────────────────────

/// Glue Compressor is keyed through AMM Sidechain moving its Threshold, and
/// the reply says so; a device with no known parameter needs one named.
#[tokio::test]
async fn devices_live_cannot_route_are_keyed_through_amm_sidechain() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let (_, bass) = kick_and_bass(&set);
    set.ask(
        "ensure_effects",
        json!({"kind": "track", "index": bass, "on_existing": "add",
               "effects": [{"uri": "query:AudioFx#Glue%20Compressor", "name": "Glue Compressor"}]}),
    );
    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Glue Compressor", "source": "Kick"}),
    )
    .await;
    assert!(!err, "{t}");
    assert!(
        t.contains("Live's API cannot route Glue Compressor's own sidechain input"),
        "{t}"
    );
    assert!(
        t.contains("its Threshold moves from 0.00 dB to -40.00 dB"),
        "{t}"
    );
    assert!(
        t.contains("On each hit the Threshold is -40.00 dB: material on Bass quieter than that is not compressed. That is as low as it goes: for a deeper duck raise the ratio, or shorten the release"),
        "{t}"
    );
    assert!(t.contains("moves Glue Compressor › Threshold"), "{t}");
    assert!(
        !t.contains("NOT linked"),
        "the model holds the target as Live did: {t}"
    );

    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Operator", "source": "Kick"}),
    )
    .await;
    assert!(err, "{t}");
    assert!(t.contains("say which (param) and where it goes (to"), "{t}");
}

/// With no device named, the track is ducked by AMM Sidechain; asking again
/// changes the same device rather than loading another.
#[tokio::test]
async fn a_track_with_no_device_named_is_ducked_by_amm_sidechain() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let made = set.build(&[
        ("Kick", "audio", ""),
        ("Pad", "midi", "query:Synths#Operator"),
    ]);
    let (err, t) = key(
        &server,
        json!({"track": "Pad", "source": "Kick", "depth_db": 12, "split_hz": 150}),
    )
    .await;
    assert!(!err, "{t}");
    assert!(
        t.contains("Loaded AMM Sidechain at the end of Pad's chain"),
        "{t}"
    );
    assert!(
        t.contains("duck: threshold -30.0 dB, ratio 4.0, up to 12 dB, only below 150 Hz"),
        "{t}"
    );
    let (err, t) = key(&server, json!({"track": "Pad", "depth_db": 6})).await;
    assert!(!err, "{t}");
    assert!(!t.contains("Loaded AMM Sidechain"), "{t}");
    assert!(
        t.contains("keyed from Kick") && t.contains("up to 6 dB"),
        "{t}"
    );
    let devices = set.devices(made[1]);
    assert_eq!(
        devices.iter().filter(|d| *d == "AMM Sidechain").count(),
        1,
        "{devices:?}"
    );
}

// ── R6: a trigger nobody hears ───────────────────────────────────────────────

#[tokio::test]
async fn set_track_mixer_routes_a_trigger_to_sends_only() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Trigger", "audio", "")]);
    let r = server
        .run(
            &tools::SET_TRACK_MIXER,
            serde_json::from_value(json!({"track": "Trigger", "output": "sends_only"})).unwrap(),
            tools::set_track_mixer_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains("'Trigger' output → Sends Only (was Main), read back from Live"),
        "{t}"
    );
    assert!(t.contains("The master no longer hears it"), "{t}");
}

// ── R4 / R5 / R7 ─────────────────────────────────────────────────────────────

fn doc(trigger_source: &str) -> Value {
    json!({
        "tracks": [
            {"name": "Bass", "kind": "midi", "instrument": "query:Synths#Operator",
             "effects": [{"device": "Compressor",
                          "params": {"Threshold": "-24 dB", "Ratio": "4.00 : 1"},
                          "sidechain": {"source": trigger_source, "tap": "post_fx", "mix": 1.0}}]},
            {"name": "Pad", "kind": "midi", "instrument": "query:Synths#Operator",
             "sidechain": {"source": trigger_source, "depth_db": 9}},
            {"name": "Trigger", "kind": "audio", "output": "sends_only"}
        ],
        "returns": [{"name": "Verb", "effects": [{"device": "Reverb"},
                     {"device": "Compressor", "sidechain": {"source": trigger_source}}]}]
    })
}

/// R4's test: a source listed after the track it keys, read back; the same
/// document again converges to one Compressor; a dry run with a misspelled
/// source changes nothing and names it.
#[tokio::test]
async fn build_song_wires_sidechains_after_every_track_and_converges() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let tracks_before = set.track_count();
    let mut dry = doc("Triger");
    dry["dry_run"] = json!(true);
    let (err, t) = build(&server, dry).await;
    assert!(err && t.contains("no track is called 'Triger'"), "{t}");
    assert_eq!(
        set.track_count(),
        tracks_before,
        "a dry run touches nothing"
    );

    let (err, t) = build(&server, doc("Trigger")).await;
    assert!(!err, "{t}");
    assert!(
        t.contains("3 sidechains wired") || t.contains("3 sidechain"),
        "{t}"
    );
    assert_eq!(
        sidechains(&set, "Bass"),
        vec![(
            "Compressor".into(),
            Some("Trigger".into()),
            Some("post_fx".into())
        )]
    );
    assert_eq!(sidechains(&set, "Pad")[0].1.as_deref(), Some("Trigger"));
    let ctx = set.context();
    let trigger = ctx["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Trigger")
        .unwrap();
    assert_eq!(trigger["output"], "Sends Only");
    let verb = ctx["returns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"].as_str().unwrap().ends_with("Verb"))
        .unwrap();
    assert_eq!(verb["sidechains"][0]["sidechain"]["source"], "Trigger");

    let (err, t) = build(&server, doc("Trigger")).await;
    assert!(!err, "{t}");
    let bass = set.track_index("Bass").unwrap();
    assert_eq!(
        set.devices(bass)
            .iter()
            .filter(|d| *d == "Compressor")
            .count(),
        1,
        "converge loads no second Compressor: {t}"
    );
}

/// The second report's problem 1: the default set's "A-Reverb" is named
/// after its devices, and Live renames it "A-Reverb | Compressor" once a
/// Compressor is added. Run the same document twice: still two returns, one
/// Compressor on A, and A keeps the name the document gave it.
#[tokio::test]
async fn converge_finds_a_return_live_renamed_after_its_devices() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let doc = json!({
        "tracks": [{"name": "Trigger", "kind": "audio", "output": "sends_only"}],
        "returns": [{"name": "Reverb", "effects": [
            {"device": "Compressor", "sidechain": {"source": "Trigger"}}]}]
    });
    let returns = |set: &LiveSet| set.ask("get_returns", json!({}))["returns"].clone();
    assert_eq!(returns(&set)[0]["name"], "A-Reverb");
    for run in 1..=2 {
        let mut d = doc.clone();
        d["on_existing"] = json!("converge");
        let (err, t) = build(&server, d).await;
        assert!(!err, "run {run}: {t}");
        assert!(!t.contains("created"), "run {run} made a return: {t}");
        let r = returns(&set);
        assert_eq!(r.as_array().unwrap().len(), 2, "run {run}: {r}");
        assert_eq!(r[0]["name"], "A-Reverb", "run {run}: {r}");
        assert_eq!(
            r[0]["devices"],
            json!(["Reverb", "Compressor"]),
            "run {run}: {r}"
        );
    }
}

#[test]
fn a_return_is_matched_by_its_letter_its_name_or_its_first_device() {
    use mcp_ableton_music_maker::effects::return_matches;
    assert!(return_matches("A-Reverb", "A", "Reverb"));
    assert!(return_matches("A-Reverb | Compressor", "A", "Reverb"));
    assert!(return_matches("A-Reverb | Compressor", "A", "A-Reverb"));
    assert!(return_matches("C-Duck Verb", "C", "duck verb"));
    assert!(!return_matches("B-Delay", "B", "Reverb"));
    assert!(!return_matches("A-Compressor | Reverb", "A", "Reverb"));
}

/// R5's test: export, empty, import — the routings come back, the trigger
/// still feeds nothing to the master.
#[tokio::test]
async fn sidechains_survive_an_export_and_an_import() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    common::isolate_state_dir();
    let (err, t) = build(&server, doc("Trigger")).await;
    assert!(!err, "{t}");
    let before: Vec<_> = ["Bass", "Pad"]
        .iter()
        .map(|n| sidechains(&set, n))
        .collect();
    let r = server
        .run(
            &tools::EXPORT_SET,
            mcp_ableton_music_maker::sets::ExportSetParams {
                name: "keyed".into(),
            },
            mcp_ableton_music_maker::sets::export_set_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    for name in ["Trigger", "Pad", "Bass"] {
        let index = set.track_index(name).unwrap();
        set.ask("delete_track", json!({"track_index": index}));
    }
    let r = server
        .run(
            &tools::IMPORT_SET,
            mcp_ableton_music_maker::sets::ImportSetParams {
                name: "keyed".into(),
                merge: true,
                dry_run: false,
            },
            mcp_ableton_music_maker::sets::import_set_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let after: Vec<_> = ["Bass", "Pad"]
        .iter()
        .map(|n| sidechains(&set, n))
        .collect();
    assert_eq!(before, after);
    let ctx = set.context();
    let trigger = ctx["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Trigger")
        .unwrap();
    assert_eq!(trigger["output"], "Sends Only");
}

/// R7's test: deleting the trigger names every device it keyed, and
/// get_context warns about each one.
#[tokio::test]
async fn deleting_a_trigger_names_what_it_keyed_and_get_context_warns() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let (err, t) = build(&server, doc("Trigger")).await;
    assert!(!err, "{t}");
    let r = server
        .run(
            &tools::DELETE_TRACK,
            tools::TrackParams {
                track: Some(json!("Trigger")),
                track_index: None,
            },
            tools::delete_track_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains(
            "Bass › Compressor was keyed from Trigger; it now has No Input, with S/C On still on"
        ),
        "{t}"
    );
    assert!(
        t.contains("Pad › AMM Sidechain was keyed from Trigger"),
        "{t}"
    );
    assert!(
        t.contains("Verb › Compressor was keyed from Trigger"),
        "{t}"
    );
    assert_eq!(
        set.track_index("Trigger"),
        None,
        "the trigger is gone from the set"
    );
    let r = server
        .run(
            &tools::GET_CONTEXT,
            tools::GetContextParams::default(),
            tools::get_context_body,
        )
        .await;
    let t = text_of(&r);
    assert!(t.contains("Sidechain warnings:"), "{t}");
    assert!(
        t.contains("Bass › Compressor has its sidechain on and no source"),
        "{t}"
    );
}

/// R1: the readout names each keyed device and its source.
#[tokio::test]
async fn get_context_shows_each_sidechain_in_the_track_line() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    kick_and_bass(&set);
    key(
        &server,
        json!({"track": "Bass", "device": "Compressor", "source": "Kick"}),
    )
    .await;
    let r = server
        .run(
            &tools::GET_CONTEXT,
            tools::GetContextParams::default(),
            tools::get_context_body,
        )
        .await;
    let t = text_of(&r);
    assert!(t.contains("· Compressor ⟵ Kick Post FX"), "{t}");
    let r = server
        .run(
            &tools::GET_DEVICE_PARAMETERS,
            serde_json::from_value(json!({"track": "Bass", "device": "Compressor"})).unwrap(),
            tools::get_device_parameters_body,
        )
        .await;
    assert!(
        text_of(&r).contains("Sidechain: keyed from Kick (Post FX) · S/C On"),
        "{}",
        text_of(&r)
    );
}

// ── R8: the duck ─────────────────────────────────────────────────────────────

fn write_ducked(path: &std::path::Path, bpm: f64, beats: usize, dip: f32) {
    let rate = 8000u32;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let per_beat = (60.0 / bpm * rate as f64) as usize;
    for i in 0..per_beat * beats {
        // Down on the first 16th of each beat, recovering by the fourth.
        let pos = (i % per_beat) as f32 / per_beat as f32;
        // dip 1.0 is a part with no sidechain: level throughout.
        let gain = if dip >= 1.0 {
            1.0
        } else if pos < 0.25 {
            dip
        } else if pos < 0.5 {
            0.5
        } else {
            1.0
        };
        let s = (0.3
            * gain
            * (2.0 * std::f32::consts::PI * 110.0 * i as f32 / rate as f32).sin()
            * 32767.0) as i16;
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn capture_bridge(file: &std::path::Path) -> std::sync::Arc<FakeBridge> {
    let bridge = FakeBridge::responding(json!({}));
    bridge.script(
        "get_session_info",
        vec![json!({"tempo": 120.0, "signature_numerator": 4})],
    );
    bridge.script(
        "ensure_capture_track",
        vec![json!({"index": 7, "created": false, "slots": 8})],
    );
    bridge.script(
        "start_capture",
        vec![
            json!({"slot": 0, "record_length": 8.0, "preroll_beats": 4.0, "tempo": 120.0,
                    "confirmed_at": 0.1, "begin_beat": 0.0}),
        ],
    );
    bridge.script(
        "capture_status",
        vec![
            json!({"slot": 0, "has_clip": true, "is_recording": true, "song_time": 4.2, "off_arrangement": []}),
            json!({"slot": 0, "has_clip": true, "is_recording": false, "file_path": file.to_str().unwrap(),
                   "song_time": 12.0, "off_arrangement": []}),
        ],
    );
    bridge
}

#[tokio::test]
async fn capture_reports_the_duck_per_16th() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ducked.wav");
    write_ducked(&file, 120.0, 8, 0.25);
    let server = server_with(capture_bridge(&file));
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            serde_json::from_value(json!({"start_bar": 2, "bars": 2, "resolution": "16th"}))
                .unwrap(),
            tools::capture_mix_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("RMS per 16th:"), "{t}");
    // 0.25 of full level on the first 16th: 12 dB under the beat's peak.
    assert!(
        t.contains("Duck: 12.0 dB per beat (median of 8 beats); the trough falls on 16th 1 of 4"),
        "{t}"
    );

    let flat = dir.path().join("flat.wav");
    write_ducked(&flat, 120.0, 8, 1.0);
    let server = server_with(capture_bridge(&flat));
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            serde_json::from_value(json!({"start_bar": 2, "bars": 2, "resolution": "16th"}))
                .unwrap(),
            tools::capture_mix_body,
        )
        .await;
    let t = text_of(&r);
    assert!(t.contains("Duck: none"), "S/C off measures no duck: {t}");

    let server = server_with(capture_bridge(&flat));
    let r = server
        .run(
            &tools::CAPTURE_MIX,
            serde_json::from_value(json!({"bars": 2, "resolution": "32nd"})).unwrap(),
            tools::capture_mix_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("resolution is bar, beat, 8th or 16th"));
}

/// The second report's problem 4: the duck as data, so a program reads
/// `duck_db` instead of parsing the sentence; "beat" says why it has none.
#[tokio::test]
async fn capture_hands_the_duck_back_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ducked.wav");
    write_ducked(&file, 120.0, 8, 0.25);
    let capture = |args: Value| {
        let server = server_with(capture_bridge(&file));
        async move {
            let r = server
                .run(
                    &tools::CAPTURE_MIX,
                    serde_json::from_value(args).unwrap(),
                    tools::capture_mix_body,
                )
                .await;
            assert!(!is_error(&r), "{}", text_of(&r));
            serde_json::from_str::<Value>(&text_of(&r)).expect("as json is JSON")
        }
    };
    let doc = capture(json!({"start_bar": 2, "bars": 2, "resolution": "16th", "as": "json"})).await;
    let master = &doc["master"];
    assert_eq!(master["track"], Value::Null);
    assert_eq!(master["step"], "16th");
    assert_eq!(master["duck_db"], 12.0, "{doc}");
    assert_eq!(master["duck_trough_step"], 1, "{doc}");
    assert_eq!(master["beats"], 8, "{doc}");
    assert_eq!(master["duck_steps_db"].as_array().unwrap().len(), 4);
    assert_eq!(master["rms_steps_db"].as_array().unwrap().len(), 32);
    assert_eq!(doc["tracks"], json!([]));

    let doc = capture(json!({"start_bar": 2, "bars": 2, "resolution": "beat", "as": "json"})).await;
    assert_eq!(doc["master"]["duck_db"], Value::Null);
    assert!(doc["master"]["duck_why_not"]
        .as_str()
        .unwrap()
        .contains("8th or 16th"));
}

/// The second report's problem 6: two tracks Live lets share a name export
/// as "Twin" and "Twin 2", the export says so, and the import rebuilds both
/// — with the sidechain keyed from the second still keyed from it.
#[tokio::test]
async fn tracks_that_share_a_name_export_unique_and_import_back() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    common::isolate_state_dir();
    let (_, bass) = kick_and_bass(&set);
    set.ask(
        "create_tracks",
        json!({"tracks": [{"name": "Twin", "kind": "audio"}, {"name": "Twin", "kind": "audio"}],
               "on_existing": "add"}),
    );
    let twins: Vec<usize> = set.context()["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"] == "Twin")
        .map(|t| t["index"].as_u64().unwrap() as usize)
        .collect();
    assert_eq!(twins.len(), 2, "Live allows two tracks of one name");
    let (err, t) = key(
        &server,
        json!({"track": bass, "device": "Compressor", "source": twins[1]}),
    )
    .await;
    assert!(!err, "{t}");

    let r = server
        .run(
            &tools::EXPORT_SET,
            mcp_ableton_music_maker::sets::ExportSetParams {
                name: "twins".into(),
            },
            mcp_ableton_music_maker::sets::export_set_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains(&format!(
            "Renamed in the export, because an import needs every track's name to be its own: track {} 'Twin' → 'Twin 2'",
            twins[1] + 1
        )),
        "{t}"
    );
    let doc = mcp_ableton_music_maker::sets::read_document("twins").expect("it reads back");
    let bass_doc = doc.tracks.iter().find(|t| t.name == "Bass").unwrap();
    assert_eq!(
        bass_doc.effects[0]
            .sidechain
            .as_ref()
            .unwrap()
            .source
            .as_deref(),
        Some("Twin 2"),
        "the key follows the track, not the name"
    );

    for name in ["Twin", "Twin", "Bass", "Kick"] {
        let index = set.track_index(name).unwrap();
        set.ask("delete_track", json!({"track_index": index}));
    }
    let r = server
        .run(
            &tools::IMPORT_SET,
            mcp_ableton_music_maker::sets::ImportSetParams {
                name: "twins".into(),
                merge: true,
                dry_run: false,
            },
            mcp_ableton_music_maker::sets::import_set_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(set.track_index("Twin").is_some() && set.track_index("Twin 2").is_some());
    assert_eq!(
        sidechains(&set, "Bass"),
        vec![(
            "Compressor".into(),
            Some("Twin 2".into()),
            Some("post_fx".into())
        )]
    );
}

#[test]
fn shared_names_are_numbered_past_the_names_already_taken() {
    use mcp_ableton_music_maker::sets::unique_track_names;
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        unique_track_names(&names(&["Twin", "Perc", "Twin", "Twin 2", "Twin"])),
        names(&["Twin", "Perc", "Twin 3", "Twin 2", "Twin 4"])
    );
    assert_eq!(unique_track_names(&names(&["A", "B"])), names(&["A", "B"]));
}

/// The second report's 7: a device with no sidechain is answered with the
/// devices on the track that have one; and AMM Sidechain, which holds its
/// target by position, follows that device when the chain is rearranged.
#[tokio::test]
async fn amm_follows_its_target_when_the_chain_moves_and_refusals_name_what_can_be_keyed() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let (_, bass) = kick_and_bass(&set);
    set.ask(
        "ensure_effects",
        json!({"kind": "track", "index": bass, "on_existing": "add",
               "effects": [{"uri": "query:AudioFx#Utility", "name": "Utility"},
                           {"uri": "query:AudioFx#Glue%20Compressor", "name": "Glue Compressor"}]}),
    );
    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Utility", "source": "Kick"}),
    )
    .await;
    assert!(err, "{t}");
    assert!(
        t.contains("On Bass, these can be keyed directly: Compressor."),
        "{t}"
    );

    let (err, t) = key(
        &server,
        json!({"track": "Bass", "device": "Glue Compressor", "source": "Kick"}),
    )
    .await;
    assert!(!err, "{t}");
    assert_eq!(
        set.devices(bass),
        [
            "Operator",
            "Compressor",
            "Utility",
            "Glue Compressor",
            "AMM Sidechain"
        ]
    );
    let edit = |args: Value| {
        let server = &server;
        async move {
            let r = server
                .run(
                    &tools::EDIT_DEVICES,
                    serde_json::from_value(args).unwrap(),
                    tools::edit_devices_body,
                )
                .await;
            assert!(!is_error(&r), "{}", text_of(&r));
            text_of(&r)
        }
    };
    let t = edit(
        json!({"track": "Bass", "device": "Glue Compressor", "action": "move", "to_index": 1}),
    )
    .await;
    assert!(
        t.contains("AMM Sidechain (device 4) still moves Glue Compressor: its target follows it to device 1."),
        "{t}"
    );
    let amm_target = |set: &LiveSet| {
        set.context()["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "Bass")
            .unwrap()["sidechains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["device"] == "AMM Sidechain")
            .unwrap()["sidechain"]["amm"]["target"]
            .clone()
    };
    let target = amm_target(&set);
    assert_eq!(target["device"], "Glue Compressor", "{target}");
    assert_eq!(target["linked"], true, "{target}");

    let t = edit(json!({"track": "Bass", "device": "Glue Compressor", "action": "remove"})).await;
    assert!(
        t.contains(
            "AMM Sidechain (device 3) moved Glue Compressor, which is gone: it moves nothing now."
        ),
        "{t}"
    );
}

#[test]
fn a_device_moved_along_a_chain_shifts_the_ones_between() {
    use mcp_ableton_music_maker::tools::moved_position;
    // A B C D E, C (2) to 0: C A B D E.
    assert_eq!(
        (0..5).map(|i| moved_position(i, 2, 0)).collect::<Vec<_>>(),
        [1, 2, 0, 3, 4]
    );
    // B (1) to 3: A C D B E.
    assert_eq!(
        (0..5).map(|i| moved_position(i, 1, 3)).collect::<Vec<_>>(),
        [0, 3, 1, 2, 4]
    );
}

/// The second report's 7: an effect rack is not written into an export, and
/// the export says so instead of losing a sidechain inside one in silence.
#[test]
fn an_export_names_the_racks_it_did_not_write() {
    use mcp_ableton_music_maker::sets::{document, export_notes};
    let snapshot = json!({
        "tracks": [{
            "name": "Bass", "is_audio_track": false,
            "devices": [
                {"name": "Operator", "type_code": 1},
                {"name": "Duck Rack", "type_code": 2, "chains": [{"name": "Duck"}]},
                {"name": "Compressor", "type_code": 2, "parameters": []}
            ]
        }],
        "return_tracks": [{"name": "A-Reverb", "devices": [
            {"name": "Audio Effect Rack", "type_code": 2, "chains": []}]}]
    });
    let doc = document("racks", &snapshot, &json!({}), None, "now".into());
    assert_eq!(
        doc.tracks[0]
            .effects
            .iter()
            .map(|e| e.device.as_str())
            .collect::<Vec<_>>(),
        ["Compressor"]
    );
    assert_eq!(
        doc.skipped_racks,
        ["Bass › Duck Rack", "Reverb › Audio Effect Rack"]
    );
    assert!(export_notes(&doc).contains(
        "Not exported: Bass › Duck Rack, Reverb › Audio Effect Rack — a rack's devices, and any sidechain inside one, are not rebuilt by import_set."
    ));
}

/// Live keeps the instrument ahead of the audio effects: a move that asks
/// otherwise is answered with why nothing moved, not "from 1 to 1".
#[tokio::test]
async fn a_move_live_refuses_says_why() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let (_, bass) = kick_and_bass(&set);
    let r = server
        .run(
            &tools::EDIT_DEVICES,
            serde_json::from_value(
                json!({"track": "Bass", "device": "Compressor", "action": "move", "to_index": 0}),
            )
            .unwrap(),
            tools::edit_devices_body,
        )
        .await;
    let t = text_of(&r);
    assert!(
        t.contains("Live left 'Compressor' on Bass at 1: it keeps a track's instrument ahead of its audio effects"),
        "{t}"
    );
    assert_eq!(set.devices(bass), ["Operator", "Compressor"]);
}
