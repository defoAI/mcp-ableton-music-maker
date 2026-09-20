//! search_browser, clip settings, meters, automation, batch and build_song —
//! through the real bodies against a real set.
//!
//! This is where #45 lives (`build_song` taking Live down on a ten-track
//! burst), so it is the suite that most needed a Live behind it. The tools
//! run against `scripts/fake-live.py` — the real Remote Script on the model,
//! over a real socket — and every test asks the **set** what happened, not
//! the reply. `FakeBridge` is kept only where a *failure* has to be produced
//! that a healthy Live cannot give.

mod common;

use common::{is_error, server_on_fake_live, server_with, text_of, FakeBridge, LiveSet};
use mcp_ableton_music_maker::notes::NotesInput;
use mcp_ableton_music_maker::tools::{
    self, AutomationTarget, BatchParams, BatchStep, BuildSongParams, LoadInstrumentParams,
    PlayAndMeasureParams, Ramp, SearchBrowserParams, SetClipAutomationParams, SetClipLoopParams,
    SongClip, SongLocator, SongPlacement, SongTrack,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The params of the first `name` the bridge was sent. Tests address a
/// command by what it is rather than by where it landed, so adding a look
/// before or between the real work does not rewrite every assertion.
fn sent_for(bridge: &FakeBridge, name: &str) -> Value {
    bridge
        .sent()
        .into_iter()
        .find(|(c, _)| c == name)
        .unwrap_or_else(|| panic!("{name} was never sent: {:?}", bridge.commands()))
        .1
}

#[tokio::test]
async fn search_lists_hits_with_uris() {
    let (server, bridge) = server_on_fake_live();
    let p = SearchBrowserParams {
        queries: vec![],
        best: false,
        refresh: false,
        query: "bass".into(),
        category: "all".into(),
        limit: 30,
    };
    let r = server
        .run(&tools::SEARCH_BROWSER, p, tools::search_browser_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    // The hits are what walking Live's own browser found, and each carries
    // the uri `load_instrument_or_effect` takes.
    assert!(t.contains("matches for"), "{t}");
    assert!(t.contains("uri: query:"), "{t}");
    assert!(t.contains("Bass"), "{t}");
    assert_eq!(bridge.last("search_browser").unwrap()["query"], "bass");
}

#[tokio::test]
async fn load_reports_the_device_it_added() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let track = set.build(&[("Bass", "midi", "")])[0];
    bridge.clear();

    let r = server
        .run(
            &tools::LOAD_INSTRUMENT_OR_EFFECT,
            LoadInstrumentParams {
                track_index: track as i64,
                uri: "query:Synths#Analog".into(),
                kind: "track".into(),
            },
            tools::load_instrument_or_effect_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).starts_with(&format!(
            "Loaded 'Analog' as device 0 on track {track} ('Bass')"
        )),
        "{}",
        text_of(&r)
    );
    // And the device is on the track, in Live.
    assert_eq!(set.devices(track), vec!["Analog".to_string()]);
}

#[tokio::test]
async fn clip_loop_needs_something_to_set_and_summarises() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let track = set.build(&[("Bass", "midi", "")])[0];
    set.write_clip(track, 0, "Bass", json!([]));
    bridge.clear();

    let empty = SetClipLoopParams {
        track_index: track as i64,
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
    assert!(
        is_error(&r) && bridge.sent().is_empty(),
        "nothing to set must be refused before Live: {}",
        text_of(&r)
    );

    let p = SetClipLoopParams {
        track_index: track as i64,
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
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("loop 0–8") && text_of(&r).contains("launch trigger"),
        "{}",
        text_of(&r)
    );
    // And the clip really loops to 8 in Live.
    let clip = set.clip_info(track, 0);
    assert_eq!(clip["looping"], true, "{clip}");
    assert_eq!(clip["loop_end"], 8.0, "{clip}");
}

/// **A real-Live check, standing in for audio.** The fake Live has no sound
/// behind its meters — they read what nothing put there — so the meter
/// values here are canned on purpose. What this test can still prove is the
/// arithmetic: that a reading goes through Live's own taper rather than
/// 20·log10, that silence is named, and that the curve is read once. That
/// the numbers Live reports are these numbers is #48's pass, not this one.
#[tokio::test]
async fn play_and_measure_reports_peaks_and_silence() {
    let bridge = FakeBridge::responding(json!({
        "is_playing": true, "song_time": 32.0,
        "tracks": [{"index": 0, "name": "Kick", "left": 0.71, "right": 0.69}, {"index": 1, "name": "Pad", "left": 0.0, "right": 0.0}],
        "returns": [{"index": 0, "name": "Reverb", "left": 0.2, "right": 0.2}],
        "master": {"name": "Master", "left": 0.8, "right": 0.8}
    }));
    // Live's own meter curve: a meter value is not linear amplitude, so the
    // script hands over the taper it draws the meters against.
    bridge.script(
        "get_meter_scale",
        vec![
            json!({"points": [[0.0, -80.0], [0.4, -30.0], [0.7, -12.0], [0.85, 0.0], [1.0, 6.0]],
                    "reference": "post_fader"}),
        ],
    );
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
    let played = cmds
        .iter()
        .position(|c| c == "play_from")
        .expect("plays from the position, not the start marker");
    assert_eq!(bridge.sent()[played].1["time"], 32.0);
    assert_eq!(cmds.last().unwrap(), "stop_playback");
    assert!(cmds.iter().filter(|c| *c == "get_track_meters").count() >= 3);
    let t = text_of(&r);
    assert!(
        t.contains("track 0 'Kick': −11.2 dB"),
        "the meter reading goes through Live's curve, not 20·log10: {t}"
    );
    assert!(t.contains("Silent during this stretch: Pad"), "{t}");
    assert!(
        t.contains("post-fader"),
        "a meter reading says where it was measured: {t}"
    );
    assert_eq!(
        bridge
            .commands()
            .iter()
            .filter(|c| *c == "get_meter_scale")
            .count(),
        1,
        "the curve is read once per session"
    );
}

/// Also canned, and for the same reason: there is no audio behind a fake
/// meter. See the note on `play_and_measure_reports_peaks_and_silence`.
#[tokio::test]
async fn an_older_script_still_reads_in_real_db() {
    // A script that predates get_meter_scale gives no curve; the server holds
    // the same measured law, so the reading is still dB and still post-fader.
    let bridge = FakeBridge::responding(json!({
        "is_playing": true, "song_time": 0.0,
        "tracks": [{"index": 0, "name": "Kick", "left": 0.71, "right": 0.69}],
        "returns": [], "master": {"name": "Master", "left": 0.8, "right": 0.8}
    }));
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::GET_TRACK_METERS,
            mcp_ableton_music_maker::tools::Empty::default(),
            tools::get_track_meters_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    // 0.71 on the measured law: 76 × 0.71 − 70 = −16.0 dB.
    assert!(t.contains("'Kick': −16.0 dB"), "{t}");
    assert!(t.contains("post-fader"), "{t}");
    assert!(
        bridge.commands().iter().all(|c| c != "set_track_mixer"),
        "a read changes nothing"
    );
}

#[tokio::test]
async fn automation_ramp_becomes_two_points_and_targets_are_checked() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let track = set.build(&[("Pad", "midi", "query:Synths#Analog")])[0];
    set.write_clip(track, 0, "Pad", json!([]));
    bridge.clear();

    let p = SetClipAutomationParams {
        track_index: track as i64,
        clip_index: 0,
        arrangement: true,
        // Analog's Filter Freq, on the device the track really has.
        target: AutomationTarget {
            device_index: Some(0),
            parameter_index: Some(1),
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
            p.clone(),
            tools::set_clip_automation_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("Session clips only"),
        "an Arrangement clip is refused before Live: {}",
        text_of(&r)
    );
    assert!(bridge.sent().is_empty(), "nothing sent");
    let mut session = p.clone();
    session.arrangement = false;
    let r = server
        .run(
            &tools::SET_CLIP_AUTOMATION,
            session,
            tools::set_clip_automation_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let sent = bridge
        .last("set_clip_automation")
        .expect("nothing was sent");
    assert_eq!(
        sent["points"],
        json!([{"time": 0.0, "value": 0.2}, {"time": 32.0, "value": 0.9}])
    );
    assert_eq!(sent["arrangement"], false);
    // The envelope exists in the clip, and it is the ramp that was asked for.
    let read = bridge
        .last("set_clip_automation")
        .map(|_| {
            set.clip_info(track, 0);
            set.automation(track, 0, 0, 1)
        })
        .unwrap();
    assert_eq!(read["has_envelope"], true, "{read}");
    let samples = read["samples"].as_array().expect("samples");
    assert!(samples.len() > 2, "{read}");

    let bad = SetClipAutomationParams {
        track_index: track as i64,
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
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let before = set.track_count();
    bridge.clear(); // the set-up read is not part of what the batch sent
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
        verbose: false,
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
    assert_eq!(
        bridge.sent()[1].1["track_index"],
        before as i64,
        "$last_track resolved to the track create_midi_track really made"
    );
    // The three steps that ran are in the set: one more track, named Bass,
    // with a clip holding the two notes the step string says.
    assert_eq!(set.track_count(), before + 1);
    assert_eq!(set.track_names()[before], "Bass");
    assert_eq!(set.clip_pitches(before, 0), vec![36, 36]);
    // And the step after the failure never ran: the tempo is untouched.
    assert_eq!(set.tempo(), 120.0);
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
        verbose: false,
    };
    let r = server.run(&tools::BATCH, nested, tools::batch_body).await;
    assert!(is_error(&r) && text_of(&r).contains("cannot run inside a batch"));
}

fn song() -> BuildSongParams {
    let mut steps = BTreeMap::new();
    steps.insert("C1".to_string(), "x...x...x...x...".to_string());
    let mut sends = BTreeMap::new();
    // Live names its returns "A Reverb" and "B Delay" in a new set, and
    // `_set_send` resolves a send by that name. The document used to say
    // "Reverb", which a canned bridge accepted and a real Live refuses.
    sends.insert("A Reverb".to_string(), 0.3);
    BuildSongParams {
        tempo: Some(128.0),
        key: None,
        on_existing: String::new(),
        scenes: vec![],
        tracks: vec![
            SongTrack {
                name: "Drums".into(),
                kind: "midi".into(),
                instrument: Some("query:Drums#Drum%20Rack".into()),
                instrument_query: None,
                volume: None,
                volume_db: None,
                fader: Some(0.8),
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
                volume_db: None,
                fader: None,
                pan: None,
                color_index: None,
                sends,
            },
        ],
        clips: vec![SongClip {
            track: "Drums".into(),
            slot: Some(0),
            slots: vec![],
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
        snapshot: false,
    }
}

#[tokio::test]
async fn build_song_validates_first_and_dry_runs() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let before = set.track_count();
    bridge.clear();
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

    // Neither of those built anything: the set is where it started.
    assert_eq!(
        set.track_count(),
        before,
        "a dry run or a refusal built a track"
    );
    assert_eq!(set.tempo(), 120.0, "the tempo moved on a dry run");
}

#[tokio::test]
async fn build_song_executes_in_order() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let first = set.track_count();
    bridge.clear();
    let r = server
        .run(&tools::BUILD_SONG, song(), tools::build_song_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = bridge.commands();
    let expected: Vec<&str> = vec![
        "set_tempo",
        // One look before building: it proves Live is answering and says
        // whether the transport is running (#43, #45).
        "get_session_info",
        "create_tracks",
        "write_clips",
        "get_arrangement_clips",
        "place_clips",
        "create_locator",
    ];
    assert_eq!(
        cmds, expected,
        "every track, then every clip, in one round trip each"
    );
    let tracks = bridge.last("create_tracks").expect("create_tracks")["tracks"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0]["name"], "Drums");
    assert!(
        tracks[0]["instrument_uri"].as_str().unwrap().contains(':'),
        "{}",
        tracks[0]
    );
    assert_eq!(
        tracks[1]["sends"],
        json!([{"name": "A Reverb", "value": 0.3}])
    );
    let clips = bridge.last("write_clips").expect("write_clips")["clips"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0]["notes"].as_array().unwrap().len(), 4);
    let t = text_of(&r);
    assert!(
        t.contains(&format!("Track {first} 'Drums'"))
            && t.contains(&format!("Placed track {first} slot 0 at 4 position(s)"))
            && t.contains("hear the balance."),
        "{t}"
    );

    // And now the set, not the reply. Two tracks, the Drum Rack on the
    // first, the tempo, four notes in the clip, four Arrangement clips one
    // bar apart, and the locator.
    assert_eq!(set.track_count(), first + 2);
    assert_eq!(&set.track_names()[first..], ["Drums", "Pad"]);
    assert_eq!(set.devices(first), vec!["Drum Rack".to_string()]);
    assert_eq!(set.tempo(), 128.0);
    assert_eq!(set.clip_pitches(first, 0), vec![36, 36, 36, 36]);
    assert_eq!(set.clip_name(first, 0).as_deref(), Some("Kick"));
    let placed: Vec<f64> = set
        .arrangement(first)
        .into_iter()
        .map(|(_, at)| at)
        .collect();
    assert_eq!(placed, vec![0.0, 4.0, 8.0, 12.0]);
    assert!(
        set.locators()
            .iter()
            .any(|(name, at)| name == "Intro" && *at == 0.0),
        "{:?}",
        set.locators()
    );
    // A build that changed the set ends on the snapshot offer: a crash costs
    // whatever is only in Live's memory and nobody reaches for export_set.
    assert!(
        t.ends_with("build_song takes snapshot: true to write one as part of the build.")
            && t.contains("Cmd+S — the Live API has no save of its own"),
        "{t}"
    );
    let _: Value = json!(null);
}

#[tokio::test]
async fn library_status_names_what_is_missing() {
    // The fake's browser is a couple of dozen items, not Live's library —
    // so this reads as a Live with a partial instrument set, which is
    // exactly the case the readout exists for.
    let (server, _bridge) = server_on_fake_live();
    let r = server
        .run(
            &tools::GET_LIBRARY_STATUS,
            tools::Empty::default(),
            tools::get_library_status_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.starts_with("Live 12.4.6 ("), "{t}");
    assert!(
        t.contains("Analog") && t.contains("Operator") && t.contains("Wavetable"),
        "the instruments that are there are named: {t}"
    );
    assert!(
        t.contains("Not available here"),
        "what Live does not have is named too: {t}"
    );
    assert!(
        t.contains("Packs installed (1): Core Library") && t.contains("Packs tab"),
        "{t}"
    );
}

#[tokio::test]
async fn a_long_batch_answers_with_a_grouped_summary_and_verbose_prints_every_step() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let track = set.build(&[("Drums", "midi", "")])[0];
    set.write_clip(track, 0, "a", json!([]));
    // Twenty steps, each clearing a freshly filled Arrangement, so the
    // grouped summary is counting real removals.
    set.place(track, 0, &[0.0, 4.0, 8.0]);
    assert_eq!(set.arrangement(track).len(), 3);
    bridge.clear();
    let steps: Vec<BatchStep> = (0..20)
        .map(|_| BatchStep {
            tool: "delete_arrangement_clip".into(),
            args: json!({"track_index": track, "all": true}),
        })
        .collect();
    let r = server
        .run(
            &tools::BATCH,
            BatchParams {
                steps: steps.clone(),
                stop_on_error: true,
                verbose: false,
            },
            tools::batch_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.starts_with("20 steps, 20 ok\n"), "{t}");
    assert!(
        t.contains("  delete_arrangement_clip ×20 ✓ — removed 3\n"),
        "the group line carries the work, not only the step count: {t}"
    );
    // The first step cleared the track; the other nineteen found nothing,
    // and the Arrangement really is empty.
    assert!(set.arrangement(track).is_empty());
    assert!(
        !t.contains("1. delete_arrangement_clip"),
        "twenty identical confirmations are not the reply: {t}"
    );
    assert!(
        t.len() < 200,
        "{} characters for twenty steps: {t}",
        t.len()
    );

    let r = server
        .run(
            &tools::BATCH,
            BatchParams {
                steps,
                stop_on_error: true,
                verbose: true,
            },
            tools::batch_body,
        )
        .await;
    let t = text_of(&r);
    assert!(
        t.starts_with("20 steps, 20 ok\n"),
        "the summary comes first: {t}"
    );
    assert!(
        t.contains("1. delete_arrangement_clip ✓")
            && t.contains("20. delete_arrangement_clip ✓"),
        "verbose keeps every step: {t}"
    );
}

#[tokio::test]
async fn batch_returns_whole_multi_line_results() {
    let (server, _bridge) = server_on_fake_live();
    let p = BatchParams {
        steps: vec![BatchStep {
            tool: "get_returns".into(),
            args: json!({}),
        }],
        stop_on_error: true,
        verbose: true,
    };
    let r = server.run(&tools::BATCH, p, tools::batch_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    // A new Live set has two returns, A Reverb and B Delay, and the batch
    // keeps the whole reply rather than a one-line confirmation.
    assert!(
        t.contains("\"letter\": \"A\"") && t.contains("\"devices\""),
        "full JSON kept: {t}"
    );
    assert!(t.contains("A Reverb") && t.contains("B Delay"), "{t}");
}

#[tokio::test]
async fn delete_arrangement_clips_all_and_by_indices() {
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let track = set.build(&[("Drums", "midi", "")])[0];
    set.write_clip(track, 0, "x", json!([]));
    set.place(track, 0, &[0.0, 4.0, 8.0]);
    assert_eq!(set.arrangement(track).len(), 3);
    bridge.clear();

    let p = tools::DeleteArrangementClipParams {
        track_index: track as i64,
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
    assert_eq!(
        bridge.sent()[0],
        (
            "delete_arrangement_clips".to_string(),
            json!({"track_index": track, "all": true})
        ),
        "every clip in one round trip"
    );
    assert!(
        text_of(&r).contains(&format!(
            "Removed 3 Arrangement clip(s) from track {track} in one round trip"
        )),
        "{}",
        text_of(&r)
    );
    assert!(set.arrangement(track).is_empty(), "the clips are still there");

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
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    let track = set.build(&[("Bass", "midi", "")])[0];
    // Two copies of 'bass' in the Arrangement, with something else between
    // them, so the refresh has to pick out its own.
    set.write_clip(track, 0, "bass", json!([{"pitch": 41, "start_time": 0.0, "duration": 1.0, "velocity": 90}]));
    set.write_clip(track, 1, "other", json!([]));
    set.place(track, 0, &[0.0]);
    set.place(track, 1, &[4.0]);
    set.place(track, 0, &[8.0]);
    assert_eq!(
        set.arrangement(track),
        vec![
            ("bass".to_string(), 0.0),
            ("other".to_string(), 4.0),
            ("bass".to_string(), 8.0)
        ]
    );
    bridge.clear();

    let p = tools::AddNotesParams {
        track_index: track as i64,
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
    // The Arrangement still has three clips in the same places, and the
    // refreshed ones carry the new note.
    assert_eq!(
        set.arrangement(track),
        vec![
            ("bass".to_string(), 0.0),
            ("other".to_string(), 4.0),
            ("bass".to_string(), 8.0)
        ]
    );
    assert_eq!(set.clip_pitches(track, 0), vec![36], "the Session clip");
}

#[tokio::test]
async fn build_song_treats_plain_words_in_instrument_as_a_search() {
    // The model often puts search words where a URI belongs; a URI always
    // carries a ':' so plain words go through search_browser first.
    let bridge = FakeBridge::responding(json!({
        "index": 0, "name": "Pad", "loaded": true, "item_name": "Evolving Pad", "track_name": "Pad",
        "devices_after": ["Evolving Pad"], "loaded_device": {"index": 0, "name": "Evolving Pad"}
    }));
    bridge.script(
        "search_browser",
        vec![
            json!({"items": [{"name": "Evolving Pad", "uri": "query:Sounds#Pad:Evolving%20Pad"}]}),
        ],
    );
    bridge.script(
        "create_tracks",
        vec![json!({"created": [{"index": 2, "name": "Drums", "device": "Evolving Pad"}]})],
    );
    let server = server_with(bridge.clone());
    let mut p = song();
    p.tracks.truncate(1);
    p.tracks[0].instrument = Some("ambient evolving pad".into());
    p.tracks[0].instrument_query = None;
    p.clips.clear();
    p.placements.clear();
    p.locators.clear();
    let r = server
        .run(&tools::BUILD_SONG, p, tools::build_song_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = bridge.commands();
    assert_eq!(
        &cmds[..4],
        &[
            "set_tempo",
            "search_browser",
            "get_session_info",
            "create_tracks"
        ]
    );
    assert_eq!(bridge.sent()[1].1["query"], "ambient evolving pad");
    assert_eq!(
        sent_for(&bridge, "create_tracks")["tracks"][0]["instrument_uri"],
        "query:Sounds#Pad:Evolving%20Pad"
    );
    assert!(
        text_of(&r).contains("Found in the library: 'Evolving Pad' for \"ambient evolving pad\""),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn build_song_copies_a_clip_into_extra_slots() {
    let bridge = FakeBridge::responding(json!({"index": 0, "name": "Pad", "loaded": true}));
    bridge.script(
        "create_tracks",
        vec![json!({"created": [{"index": 0, "name": "Drums"}]})],
    );
    let server = server_with(bridge.clone());
    let mut p = song();
    p.tracks.truncate(1);
    p.tracks[0].instrument = None;
    p.clips.truncate(1);
    p.clips[0].track = "Drums".into();
    p.clips[0].slot = Some(0);
    p.clips[0].slots = vec![1, 2, 0];
    p.placements.clear();
    p.locators.clear();
    let r = server
        .run(&tools::BUILD_SONG, p, tools::build_song_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let clips = bridge
        .sent()
        .iter()
        .find(|(c, _)| c == "write_clips")
        .map(|(_, a)| a["clips"].as_array().unwrap().clone())
        .unwrap();
    let slots: Vec<i64> = clips
        .iter()
        .map(|c| c["clip_index"].as_i64().unwrap())
        .collect();
    assert_eq!(
        slots,
        vec![0, 1, 2],
        "each extra slot once, the main slot first"
    );
    assert_eq!(
        clips[0]["notes"].as_array().unwrap().len(),
        4,
        "the notes travel once"
    );
    assert_eq!(clips[1]["copy_of"], 0, "copies are made inside Live");
    assert!(clips[1].get("notes").is_none());
    assert!(text_of(&r).contains("slots 0, 1, 2"), "{}", text_of(&r));
}

#[tokio::test]
async fn get_context_is_one_round_trip_with_the_workflow_footer() {
    let bridge = FakeBridge::responding(json!({
        "live_version": "12.4.6", "script_version": "1.15.0",
        "session": {"tempo": 120.0, "signature_numerator": 4, "signature_denominator": 4, "is_playing": false,
            "bar": 1, "beat_in_bar": 1, "clip_trigger_quantization_name": "1_bar", "scale_mode": true,
            "scale_name": "Major", "root_note_name": "C", "master_volume": 0.85},
        "tracks": [{"index": 0, "name": "1-MIDI", "kind": "midi", "volume": 0.85, "devices": [], "clips": []}],
        "returns": [], "scenes": [{"index": 0, "name": "", "clip_count": 0}], "cues": [], "events": []
    }));
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::GET_CONTEXT,
            tools::GetContextParams::default(),
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(bridge.commands(), vec!["get_context"], "one round trip");
    assert_eq!(bridge.sent()[0].1["include_library"], false);
    let t = text_of(&r);
    assert!(t.starts_with("Live 12.4.6 · script 1.15.0 · 120 BPM 4/4 · stopped at bar 1.1 · launch quantization 1 bar · key C Major\n"), "{t}");
    assert!(
        t.contains("  0 1-MIDI [midi] no devices · vol 0.85 · no clips\n"),
        "{t}"
    );
    assert!(t.contains("Scenes: 0 empty\n"), "{t}");
    assert!(t.contains("Performance: not running"), "{t}");
    assert!(t.ends_with(mcp_ableton_music_maker::context::FOOTER), "{t}");
    let r = server
        .run(
            &tools::GET_CONTEXT,
            tools::GetContextParams {
                include_library: true,
                json: true,
            },
            tools::get_context_body,
        )
        .await;
    assert!(
        text_of(&r).trim_start().starts_with('{'),
        "raw JSON on request"
    );
    assert_eq!(bridge.sent()[1].1["include_library"], true);
}

#[tokio::test]
async fn batch_resolves_last_clip_and_build_song_handles_scenes_and_slots() {
    let bridge = FakeBridge::responding(json!({"index": 3, "name": "3-MIDI"}));
    bridge.script(
        "create_tracks",
        vec![json!({"created": [{"index": 3, "name": "Drums"}]})],
    );
    let server = server_with(bridge.clone());
    let p: tools::BatchParams = serde_json::from_value(json!({"steps": [
        {"tool": "create_midi_track", "args": {}},
        {"tool": "create_clip", "args": {"track_index": "$last_track", "clip_index": 2, "length": 4}},
        {"tool": "fire_clip", "args": {"track_index": "$last_track", "clip_index": "$last_clip"}}
    ]}))
    .unwrap();
    let r = server.run(&tools::BATCH, p, tools::batch_body).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let fired = bridge
        .sent()
        .into_iter()
        .find(|(c, _)| c == "fire_clip")
        .unwrap()
        .1;
    assert_eq!(
        fired["clip_index"], 2,
        "$last_clip is the slot of the last create_clip"
    );
    assert_eq!(fired["track_index"], 3);

    // A scenes block names rows; clips with only `slots` skip slot 0.
    let bridge = FakeBridge::responding(json!({"index": 0, "name": "Drums", "loaded": true}));
    bridge.script(
        "create_tracks",
        vec![json!({"created": [{"index": 0, "name": "Drums"}]})],
    );
    let server = server_with(bridge.clone());
    let mut p = song();
    p.scenes = vec![
        tools::SongScene {
            name: "Intro".into(),
            tempo: None,
            phrase_bars: Some(8),
        },
        tools::SongScene {
            name: "Groove".into(),
            tempo: Some(128.0),
            phrase_bars: None,
        },
    ];
    p.tracks.truncate(1);
    p.tracks[0].instrument = None;
    p.clips.truncate(1);
    p.clips[0].slot = None;
    p.clips[0].slots = vec![1, 2];
    p.placements.clear();
    p.locators.clear();
    let r = server
        .run(&tools::BUILD_SONG, p, tools::build_song_body)
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = bridge.commands();
    assert_eq!(
        &cmds[..4],
        &[
            "set_tempo",
            "get_performance_state",
            "create_scene",
            "create_scene"
        ],
        "{cmds:?}"
    );
    let scene_args: Vec<_> = bridge
        .sent()
        .into_iter()
        .filter(|(c, _)| c == "create_scene")
        .map(|(_, a)| a)
        .collect();
    assert_eq!(scene_args[0]["name"], "Intro");
    assert_eq!(scene_args[0]["phrase_bars"], 8);
    assert_eq!(scene_args[1]["tempo"], 128.0);
    let slots: Vec<i64> = bridge
        .sent()
        .iter()
        .filter(|(c, _)| c == "write_clips")
        .flat_map(|(_, a)| {
            a["clips"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["clip_index"].as_i64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(slots, vec![1, 2], "no slot 0 when only slots is given");
    assert!(
        text_of(&r).contains("Scenes: 0 Intro, 1 Groove."),
        "{}",
        text_of(&r)
    );
}

// ── A build that died half-way converges when it is run again ───────────────

#[tokio::test]
async fn build_song_converges_on_a_re_run_instead_of_duplicating() {
    let bridge = FakeBridge::responding(json!({"index": 2, "name": "2-MIDI", "tempo": 128.0}));
    // Live answers the second run with the tracks it already has.
    bridge.script(
        "create_tracks",
        vec![json!({"created": [
            {"index": 0, "name": "Drums", "reused": true},
            {"index": 1, "name": "Pad", "reused": true}]})],
    );
    bridge.script(
        "get_track_info",
        vec![json!({"index": 0, "kind": "track", "name": "Drums", "devices": [],
                    "clip_slots": [{"index": 0, "has_clip": true, "clip": {"name": "Kick", "length": 4.0}}]})],
    );
    bridge.script(
        "get_arrangement_clips",
        vec![json!({"clips": [
            {"name": "Kick", "start_time": 0.0, "end_time": 4.0, "length": 4.0},
            {"name": "Kick", "start_time": 4.0, "end_time": 8.0, "length": 4.0}]})],
    );
    bridge.script("write_clips", vec![json!({"written": []})]);
    bridge.script("place_clips", vec![json!({"placed": 2})]);
    let server = server_with(bridge.clone());
    let r = server
        .run(&tools::BUILD_SONG, song(), tools::build_song_body)
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert_eq!(
        sent_for(&bridge, "create_tracks")["on_existing"],
        "converge",
        "Live decides what already exists, in the same round trip"
    );
    assert!(t.contains("Track 0 'Drums' — reused"), "{t}");
    assert!(
        !bridge.commands().contains(&"write_clips".to_string()),
        "the slot already holds that clip: nothing was written"
    );
    let placed = bridge
        .sent()
        .into_iter()
        .find(|(c, _)| c == "place_clips")
        .expect("the two free bars are still placed");
    let times = placed.1["times"]
        .as_array()
        .cloned()
        .or_else(|| placed.1["clips"][0]["times"].as_array().cloned())
        .unwrap_or_default();
    assert_eq!(
        times,
        vec![json!(8.0), json!(12.0)],
        "beats 0 and 4 were taken: {placed:?}"
    );
    assert!(
        t.contains("Converged: 2 track(s) reused, 1 clip(s) and 2 placement(s) were already there"),
        "{t}"
    );
}

#[tokio::test]
async fn build_song_can_be_told_to_fail_on_what_exists() {
    let bridge = FakeBridge::responding(json!({"index": 2, "name": "2-MIDI", "tempo": 128.0}));
    let server = server_with(bridge.clone());
    let mut p = song();
    p.on_existing = "fail".into();
    let _ = server
        .run(&tools::BUILD_SONG, p, tools::build_song_body)
        .await;
    assert_eq!(sent_for(&bridge, "create_tracks")["on_existing"], "fail");

    let mut p = song();
    p.on_existing = "sideways".into();
    let before = bridge.sent().len();
    let r = server
        .run(&tools::BUILD_SONG, p, tools::build_song_body)
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("on_existing must be converge, add or fail"),
        "{}",
        text_of(&r)
    );
    assert_eq!(bridge.sent().len(), before, "nothing was sent");
}

#[tokio::test]
async fn a_failed_build_says_what_exists_and_that_the_document_can_be_re_run() {
    let bridge = FakeBridge::responding(json!({"index": 2, "name": "2-MIDI", "tempo": 128.0}));
    bridge.fail_from(
        2,
        mcp_ableton_music_maker::connection::LiveError::Lost("socket closed".into()),
    );
    let server = server_with(bridge.clone());
    let r = server
        .run(&tools::BUILD_SONG, song(), tools::build_song_body)
        .await;
    let t = text_of(&r);
    assert!(is_error(&r), "{t}");
    assert!(t.contains("Stopped after 0 of 2 tracks"), "{t}");
    assert!(
        t.contains("on_existing \"converge\" (the default) reuses the tracks that exist"),
        "the way out is in the failure: {t}"
    );
    assert!(
        !t.contains("  "),
        "the resume sentence runs together or is double-spaced: {t:?}"
    );
}

/// A five-track document is sent to Live in groups (#45). Ten tracks in one
/// command took Live down twice: every track reinitialises the audio graph
/// and most load a device, and the socket died mid-command with half a set
/// built and nothing said about it.
#[tokio::test]
async fn tracks_are_built_in_groups_rather_than_one_burst() {
    let bridge = FakeBridge::responding(json!({"tempo": 128.0}));
    bridge.script(
        "create_tracks",
        vec![
            json!({"created": [
                {"index": 0, "name": "T1", "device": "Analog"},
                {"index": 1, "name": "T2", "device": "Analog"},
                {"index": 2, "name": "T3", "device": "Analog"}]}),
            json!({"created": [
                {"index": 3, "name": "T4", "device": "Analog"},
                {"index": 4, "name": "T5", "device": "Analog"}]}),
        ],
    );
    bridge.script("write_clips", vec![json!({"written": []})]);
    bridge.script("place_clips", vec![json!({"placed": 0})]);
    bridge.script("get_arrangement_clips", vec![json!({"clips": []})]);
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::BUILD_SONG,
            five_track_song(),
            tools::build_song_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    let groups: Vec<Value> = bridge
        .sent()
        .into_iter()
        .filter(|(c, _)| c == "create_tracks")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(groups.len(), 2, "five tracks should go in two groups");
    assert_eq!(groups[0]["tracks"].as_array().unwrap().len(), 3);
    assert_eq!(groups[1]["tracks"].as_array().unwrap().len(), 2);
    for g in &groups {
        assert_eq!(g["on_existing"], "converge", "every group converges");
    }
    for n in 1..=5 {
        assert!(t.contains(&format!("'T{n}'")), "track T{n} missing: {t}");
    }
}

/// When Live goes away part-way through, the reply is the way back in: the
/// tracks that exist are named, and the sentence that finishes the build is
/// there to be followed.
#[tokio::test]
async fn a_death_between_groups_costs_one_group_and_names_what_exists() {
    let bridge = FakeBridge::responding(json!({"tempo": 128.0}));
    bridge.script(
        "create_tracks",
        vec![json!({"created": [
            {"index": 0, "name": "T1", "device": "Analog"},
            {"index": 1, "name": "T2", "device": "Analog"},
            {"index": 2, "name": "T3", "device": "Analog"}]})],
    );
    // set_tempo, get_session_info, create_tracks, then the look before the
    // second group is where Live has gone.
    bridge.fail_from(
        3,
        mcp_ableton_music_maker::connection::LiveError::Lost(
            "connection closed before any data arrived".into(),
        ),
    );
    let server = server_with(bridge.clone());
    let r = server
        .run(
            &tools::BUILD_SONG,
            five_track_song(),
            tools::build_song_body,
        )
        .await;
    let t = text_of(&r);
    assert!(is_error(&r), "{t}");
    assert!(t.contains("Stopped after 3 of 5 tracks"), "{t}");
    for n in 1..=3 {
        assert!(t.contains(&format!("'T{n}'")), "T{n} is in the set: {t}");
    }
    for n in 4..=5 {
        assert!(!t.contains(&format!("'T{n}'")), "T{n} is not: {t}");
    }
    assert!(
        t.contains("run build_song again with it") && t.contains("carries on from there"),
        "the resume path is untold: {t}"
    );
    // The fourth and fifth tracks were never attempted: one group was lost,
    // not the document.
    assert_eq!(
        bridge
            .commands()
            .iter()
            .filter(|c| *c == "create_tracks")
            .count(),
        1
    );
}

/// #43: adding a track or a device pops, and pretending otherwise is worse
/// than saying it.
#[tokio::test]
async fn building_while_the_transport_runs_says_so() {
    let bridge = FakeBridge::responding(json!({"tempo": 128.0}));
    bridge.script("get_session_info", vec![json!({"is_playing": true})]);
    bridge.script(
        "create_tracks",
        vec![json!({"created": [{"index": 0, "name": "Drums"}, {"index": 1, "name": "Pad"}]})],
    );
    bridge.script("write_clips", vec![json!({"written": []})]);
    bridge.script("place_clips", vec![json!({"placed": 0})]);
    bridge.script("get_arrangement_clips", vec![json!({"clips": []})]);
    let server = server_with(bridge.clone());
    let r = server
        .run(&tools::BUILD_SONG, song(), tools::build_song_body)
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains("Built while playing") && t.contains("build before the performance"),
        "{t}"
    );

    // Stopped: nothing about popping.
    let quiet = FakeBridge::responding(json!({"tempo": 128.0}));
    quiet.script("get_session_info", vec![json!({"is_playing": false})]);
    quiet.script(
        "create_tracks",
        vec![json!({"created": [{"index": 0, "name": "Drums"}, {"index": 1, "name": "Pad"}]})],
    );
    quiet.script("write_clips", vec![json!({"written": []})]);
    quiet.script("place_clips", vec![json!({"placed": 0})]);
    quiet.script("get_arrangement_clips", vec![json!({"clips": []})]);
    let t = text_of(
        &server_with(quiet)
            .run(&tools::BUILD_SONG, song(), tools::build_song_body)
            .await,
    );
    assert!(!t.contains("Built while playing"), "{t}");
}

/// Five tracks with no clips, placements or locators: the tracks are the
/// point.
fn five_track_song() -> BuildSongParams {
    let mut p = song();
    p.clips.clear();
    p.placements.clear();
    p.locators.clear();
    p.tracks = (1..=5)
        .map(|n| SongTrack {
            name: format!("T{n}"),
            kind: "midi".into(),
            instrument: Some("query:Synths#Analog".into()),
            instrument_query: None,
            volume: None,
            volume_db: None,
            fader: None,
            pan: None,
            color_index: None,
            sends: BTreeMap::new(),
        })
        .collect();
    p
}
