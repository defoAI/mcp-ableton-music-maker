//! Samples land in the song: the index the Remote Script walks, the search
//! that answers with paths, and `add_sample` into a section or at a bar —
//! fitted, transposed, named, in one command. The state directory is
//! process-wide, so these take a lock, as the library and sets suites do.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::samples::{AddSampleParams, SampleFoldersParams};
use mcp_ableton_music_maker::tools::{self, SearchBrowserParams};
use serde_json::{json, Value};
use std::sync::Arc;

static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn temp_state() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
    dir
}

/// A WAV file that is only a header: `header_seconds` reads the declared `data`
/// size, which is what makes a whole library measurable in a moment.
fn wav(path: &std::path::Path, seconds: f64) {
    let (rate, channels, bits) = (48_000u32, 2u16, 16u16);
    let per_second = rate * channels as u32 * (bits / 8) as u32;
    let bytes = (seconds * per_second as f64).round() as u32;
    let mut v = Vec::new();
    v.extend(b"RIFF");
    v.extend((36 + bytes).to_le_bytes());
    v.extend(b"WAVE");
    v.extend(b"fmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(channels.to_le_bytes());
    v.extend(rate.to_le_bytes());
    v.extend(per_second.to_le_bytes());
    v.extend((channels * bits / 8).to_le_bytes());
    v.extend(bits.to_le_bytes());
    v.extend(b"data");
    v.extend(bytes.to_le_bytes());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, v).unwrap();
}

/// The same for AIFF, whose COMM chunk carries the rate as an 80-bit float.
fn aiff(path: &std::path::Path, seconds: f64) {
    let rate = 44_100u32;
    let frames = (seconds * rate as f64).round() as u32;
    let mut mantissa = rate as u64;
    let mut unbiased = 63i32;
    while mantissa & (1 << 63) == 0 {
        mantissa <<= 1;
        unbiased -= 1;
    }
    let mut comm = Vec::new();
    comm.extend(2u16.to_be_bytes());
    comm.extend(frames.to_be_bytes());
    comm.extend(16u16.to_be_bytes());
    comm.extend(((unbiased + 16383) as u16).to_be_bytes());
    comm.extend(mantissa.to_be_bytes());
    let mut v = Vec::new();
    v.extend(b"FORM");
    v.extend((4 + 8 + comm.len() as u32).to_be_bytes());
    v.extend(b"AIFF");
    v.extend(b"COMM");
    v.extend((comm.len() as u32).to_be_bytes());
    v.extend(&comm);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, v).unwrap();
}

/// A sample folder as a producer's looks: packs, subfolders, and things that
/// are not samples.
fn sample_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let breaks = dir.path().join("Chop and Swing/Samples/Breaks");
    wav(&breaks.join("Break 90bpm 11 Loose.wav"), 10.7);
    wav(&breaks.join("Break 90bpm 04.wav"), 5.3);
    aiff(
        &dir.path()
            .join("Chop and Swing/Samples/Cymbals/Crash Bright 01.aif"),
        1.8,
    );
    // Ignored: not audio, hidden, and too deep to walk.
    std::fs::write(breaks.join("notes.txt"), "not a sample").unwrap();
    std::fs::write(breaks.join(".DS_Store"), "junk").unwrap();
    let deep = dir.path().join("a/b/c/d/e/f/g/h/i");
    wav(&deep.join("Too Deep.wav"), 1.0);
    dir
}

fn state() -> Value {
    json!({
        "is_playing": false, "tempo": 120.0, "signature_numerator": 4, "signature_denominator": 4,
        "beat": 0.0, "bar": 1, "beat_in_bar": 1, "clip_trigger_quantization": 4,
        "tracks": [
            {"index": 0, "name": "Kick", "playing_slot_index": -1, "slots_with_clips": [0]},
            {"index": 1, "name": "FX", "playing_slot_index": -1, "slots_with_clips": []}
        ],
        "scenes": [
            {"index": 0, "name": "Intro · 8", "clip_tracks": [0]},
            {"index": 1, "name": "Verse · 16", "clip_tracks": [0]}
        ],
        "cues": [], "events": []
    })
}

/// A bridge with the folder list, one scan page and a placed clip.
/// Live names the folders; the server walks them. `roots` is what a producer
/// added, which the script would resolve and hand back.
fn bridge_for(tree: &tempfile::TempDir) -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script(
        "list_sample_folders",
        vec![
            json!({"folders": [{"name": "Factory Packs", "path": tree.path(), "source": "packs"}],
                    "places_without_path": ["Splice"]}),
        ],
    );
    b.script("get_performance_state", vec![state()]);
    b
}

/// What the script answers for a warped four-bar loop.
fn placed_loop(where_: &str) -> Value {
    let mut v = json!({
        "track": "Break", "track_index": 2, "where": where_, "name": "Break 90bpm 11 Loose",
        "beats_per_bar": 4.0, "duration_s": 10.7, "heard_bars": 4.02, "fitted": "snapped",
        "bars": 4.0, "warping": true, "looping": true, "loop_end": 16.0, "length": 16.0,
        "file_path": "/packs/Chop and Swing/Samples/Breaks/Break 90bpm 11 Loose.wav",
        "read_back": true
    });
    if where_ == "arrangement" {
        v["start_time"] = json!(64.0);
        v["end_time"] = json!(80.0);
    } else {
        v["slot"] = json!(1);
    }
    v
}

fn add(sample: &str) -> AddSampleParams {
    AddSampleParams {
        sample: sample.into(),
        fit: true,
        ..Default::default()
    }
}

fn search(category: &str, query: &str) -> SearchBrowserParams {
    SearchBrowserParams {
        query: query.into(),
        queries: vec![],
        category: category.into(),
        limit: 30,
        best: false,
        refresh: false,
    }
}

#[tokio::test]
async fn a_sample_search_indexes_once_and_answers_with_paths() {
    let _lock = LOCK.lock().await;
    let dir = temp_state();
    let tree = sample_tree();
    let b = bridge_for(&tree);
    let server = server_with(b.clone());

    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            search("samples", "break 90"),
            tools::search_browser_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("From the sample index: 3 files in 1 folder (Factory Packs), indexed in"),
        "{t}"
    );
    assert!(t.contains("2 matches for \"break 90\""), "{t}");
    assert!(
        t.contains("Break 90bpm 04 — ")
            && t.contains("Chop and Swing/Samples/Breaks    wav · 5.3 s"),
        "the folder as Live shows it, and a duration from the header: {t}"
    );
    assert!(
        t.contains(&format!(
            "path: {}/Chop and Swing/Samples/Breaks/Break 90bpm 11 Loose.wav",
            tree.path().display()
        )),
        "a path, which is what add_sample needs: {t}"
    );
    assert!(t.contains("add_sample(sample, section or at_bar)"), "{t}");
    assert_eq!(
        b.commands(),
        vec!["list_sample_folders"],
        "one question to Live — the walk is this process's own work"
    );

    // The index is on disk and in memory: a second search costs no command.
    let saved: Vec<_> = std::fs::read_dir(dir.path().join("library"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        saved.iter().any(|f| f.starts_with("samples-")),
        "kept beside the browser index: {saved:?}"
    );
    let before = b.commands().len();
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            search("samples", "crash"),
            tools::search_browser_body,
        )
        .await;
    assert!(text_of(&r).contains("Crash Bright 01"), "{}", text_of(&r));
    assert_eq!(b.commands().len(), before, "no round trip once indexed");
}

#[tokio::test]
async fn a_search_for_instruments_never_scans() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    let b = bridge_for(&tree);
    b.script(
        "search_browser",
        vec![json!({"items": [{"name": "Analog", "path": "Instruments/Analog", "uri": "u1", "category": "instruments", "is_device": true}], "complete": true})],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            search("instruments", "analog"),
            tools::search_browser_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        !b.commands().iter().any(|c| c == "scan_samples"),
        "the browser answers for devices: {:?}",
        b.commands()
    );
}

#[tokio::test]
async fn words_land_in_a_section_on_a_new_audio_track() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    let b = bridge_for(&tree);
    b.script(
        "create_tracks",
        vec![json!({"created": [{"index": 2, "name": "Break"}]})],
    );
    b.script("place_sample", vec![placed_loop("session")]);
    let server = server_with(b.clone());

    let r = server
        .run(
            &tools::ADD_SAMPLE,
            AddSampleParams {
                sample: "break 90 loose".into(),
                track: Some(json!("Break")),
                section: Some(json!("Verse")),
                fit: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::samples::add_sample_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with(
            "Added 'Break 90bpm 11 Loose' to Verse on Break (new audio track 'Break', index 2)."
        ),
        "{t}"
    );
    assert!(
        t.contains("10.7 s of audio, warped and looping 4 bars (Live heard 4.02 bars)."),
        "what Live ended up with, read back: {t}"
    );
    assert!(
        t.contains("referenced where it is: /packs/Chop and Swing/Samples/Breaks/Break 90bpm 11 Loose.wav — nothing copied"),
        "the file Live reported, not the one the search guessed: {t}"
    );
    let (_, sent) = b
        .sent()
        .into_iter()
        .find(|(c, _)| c == "place_sample")
        .unwrap();
    assert_eq!(sent["slot"], 1, "the Verse is scene row 1");
    assert_eq!(sent["track_index"], 2);
    assert_eq!(sent["fit"], true);
    assert!(sent.get("position").is_none(), "a row, not the Arrangement");
    assert_eq!(
        b.commands()
            .iter()
            .filter(|c| c.as_str() == "place_sample")
            .count(),
        1,
        "one command, so Live undoes it in one step"
    );
}

#[tokio::test]
async fn a_path_needs_no_index_and_a_bar_goes_to_the_arrangement() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    let b = bridge_for(&tree);
    b.script("place_sample", vec![placed_loop("arrangement")]);
    let server = server_with(b.clone());

    let r = server
        .run(
            &tools::ADD_SAMPLE,
            AddSampleParams {
                sample: "/Users/x/Samples/Risers/Riser White 8bar.wav".into(),
                track: Some(json!("FX")),
                at_bar: Some(17.0),
                bars: Some(8.0),
                fit: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::samples::add_sample_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("to the Arrangement on Break at bar 17."), "{t}");
    assert!(t.contains("Covers bar 17 to bar 21."), "read back: {t}");
    assert!(
        !b.commands().iter().any(|c| c == "scan_samples"),
        "a path is used as given: {:?}",
        b.commands()
    );
    let (_, sent) = b
        .sent()
        .into_iter()
        .find(|(c, _)| c == "place_sample")
        .unwrap();
    assert_eq!(sent["position"], 64.0, "bar 17 is beat 64 in 4/4");
    assert_eq!(sent["path"], "/Users/x/Samples/Risers/Riser White 8bar.wav");
    assert_eq!(sent["bars"], 8.0);
    assert_eq!(sent["track_index"], 1, "FX, which exists");
}

#[tokio::test]
async fn fitting_reports_what_live_did_for_every_outcome() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    for (fitted, extra, expected) in [
        (
            "one_shot",
            json!({"duration_s": 1.8, "heard_bars": 0.9, "warping": false, "looping": false}),
            "1.8 s of audio: a one-shot, left as Live loaded it — unwarped, not looping.",
        ),
        (
            "off_grid",
            json!({"duration_s": 7.0, "heard_bars": 3.4, "warping": true, "looping": true, "loop_end": 13.6}),
            "7.0 s of audio: Live heard 3.40 bars, not a whole number of bars, so the markers were left alone — give `bars` to force a loop length.",
        ),
        (
            "asked",
            json!({"duration_s": 15.5, "heard_bars": 8.0, "warping": true, "looping": true, "loop_end": 32.0, "bars": 8.0}),
            "15.5 s of audio, warped and looping 8 bars as asked (Live heard 8 bars).",
        ),
        (
            "asked",
            json!({"duration_s": 5.3, "heard_bars": 2.0, "warping": true, "looping": true,
                   "loop_end": 16.0, "bars": 4.0, "start_time": 32.0, "end_time": 40.0}),
            "On the timeline it is 2 bars long, not 4 bars: Live gives an Arrangement clip the length of its material and the API cannot stretch that. `arrange repeat` fills the rest.",
        ),
        (
            "at_bar",
            json!({"duration_s": 2.3, "heard_bars": 1.0, "warping": true, "looping": false,
                   "start_time": 16.0, "end_time": 20.0}),
            "2.3 s of audio, warped to the tempo by Live and not looping — one hit at that bar. Give `bars` to make it a loop of a set length.",
        ),
        (
            "raw",
            json!({"duration_s": 4.0, "heard_bars": 2.0, "warping": false, "looping": false}),
            "4.0 s of audio, placed exactly as dragging the file in would (fit: false).",
        ),
        (
            "snapped",
            json!({"duration_s": 2.0, "heard_bars": 1.0, "warping": true, "looping": true, "loop_end": 4.0}),
            "2.0 s of audio, warped and looping 1 bar (Live heard 1 bar).",
        ),
        (
            "unmeasured",
            json!({"warping": false, "looping": false}),
            "The sample: Live did not report a length, so nothing was fitted.",
        ),
    ] {
        let b = bridge_for(&tree);
        let mut reply = json!({"track": "FX", "track_index": 1, "where": "session", "slot": 1,
                               "name": "thing", "fitted": fitted, "beats_per_bar": 4.0, "read_back": true});
        for (k, v) in extra.as_object().unwrap() {
            reply[k] = v.clone();
        }
        b.script("place_sample", vec![reply]);
        let server = server_with(b.clone());
        let r = server
            .run(
                &tools::ADD_SAMPLE,
                AddSampleParams {
                    sample: "/x/thing.wav".into(),
                    track: Some(json!("FX")),
                    slot: Some(1),
                    fit: fitted != "raw",
                    ..Default::default()
                },
                mcp_ableton_music_maker::samples::add_sample_body,
            )
            .await;
        assert!(!is_error(&r), "{fitted}: {}", text_of(&r));
        assert!(text_of(&r).contains(expected), "{fitted}: {}", text_of(&r));
    }
}

#[tokio::test]
async fn a_browser_sample_goes_in_a_row_and_is_refused_at_a_bar() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    let b = bridge_for(&tree);
    b.script(
        "place_sample",
        vec![json!({"track": "FX", "track_index": 1, "where": "session", "slot": 1,
                    "name": "Perc Loop 02", "fitted": "snapped", "beats_per_bar": 4.0,
                    "duration_s": 4.0, "heard_bars": 2.0, "warping": true, "looping": true,
                    "loop_end": 8.0, "file_path": "/packs/Perc/Perc Loop 02.aif", "read_back": true})],
    );
    let server = server_with(b.clone());

    // Into a row: the script highlights the slot and loads the browser item.
    let r = server
        .run(
            &tools::ADD_SAMPLE,
            AddSampleParams {
                sample: "query:Samples#Perc%20Loop%2002".into(),
                track: Some(json!("FX")),
                slot: Some(1),
                fit: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::samples::add_sample_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let (_, sent) = b
        .sent()
        .into_iter()
        .find(|(c, _)| c == "place_sample")
        .unwrap();
    assert_eq!(sent["item_uri"], "query:Samples#Perc%20Loop%2002");
    assert!(sent.get("path").is_none(), "the browser has no path");
    // And Live tells us the file afterwards, so the reply can name it.
    assert!(
        text_of(&r).contains("referenced where it is: /packs/Perc/Perc Loop 02.aif"),
        "{}",
        text_of(&r)
    );

    // At a bar: refused, with the way round.
    let r = server
        .run(
            &tools::ADD_SAMPLE,
            AddSampleParams {
                sample: "query:Samples#Perc%20Loop%2002".into(),
                track: Some(json!("FX")),
                at_bar: Some(5.0),
                fit: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::samples::add_sample_body,
        )
        .await;
    assert!(is_error(&r));
    let t = text_of(&r);
    assert!(
        t.contains("Live tells a client an item's file only once it is a clip"),
        "{t}"
    );
    assert!(t.contains("Put it in a section first"), "{t}");
}

#[tokio::test]
async fn every_refusal_says_what_to_do_instead() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    let b = bridge_for(&tree);
    let server = server_with(b.clone());
    let run = |p: AddSampleParams| {
        let s = &server;
        async move {
            let r = s
                .run(
                    &tools::ADD_SAMPLE,
                    p,
                    mcp_ableton_music_maker::samples::add_sample_body,
                )
                .await;
            (is_error(&r), text_of(&r))
        }
    };

    // Nowhere to put it.
    let (err, t) = run(AddSampleParams {
        sample: "/x/a.wav".into(),
        ..add("/x/a.wav")
    })
    .await;
    assert!(err);
    assert!(
        t.contains("Where should it go?") && t.contains("at_bar"),
        "{t}"
    );

    // Two places at once.
    let (err, t) = run(AddSampleParams {
        sample: "/x/a.wav".into(),
        section: Some(json!("Verse")),
        at_bar: Some(9.0),
        ..Default::default()
    })
    .await;
    assert!(err);
    assert!(t.contains("Give one place"), "{t}");

    // A section that is not there.
    let (err, t) = run(AddSampleParams {
        sample: "/x/a.wav".into(),
        section: Some(json!("Bridge")),
        ..Default::default()
    })
    .await;
    assert!(err);
    assert!(
        t.contains("No section called \"Bridge\"") && t.contains("Sections: Intro, Verse"),
        "{t}"
    );

    // Out of range for Live's own Transpose.
    let (err, t) = run(AddSampleParams {
        sample: "/x/a.wav".into(),
        section: Some(json!("Verse")),
        transpose: Some(60),
        ..Default::default()
    })
    .await;
    assert!(err);
    assert!(t.contains("-48 to 48"), "{t}");

    // Bar 0 does not exist in Live's numbering.
    let (err, t) = run(AddSampleParams {
        sample: "/x/a.wav".into(),
        at_bar: Some(0.0),
        ..Default::default()
    })
    .await;
    assert!(err);
    assert!(t.contains("starts at bar 1"), "{t}");

    // Nothing matches the words, and the browser has nothing either.
    let (err, t) = run(add("tabla in seven eight")).await;
    assert!(err);
    assert!(
        t.contains("No sample matches \"tabla in seven eight\"")
            && t.contains("3 files in 1 folder"),
        "{t}"
    );

    // A MIDI track is Live's own refusal, passed through.
    let midi = FakeBridge::responding(json!({}));
    midi.script(
        "list_sample_folders",
        vec![json!({"folders": [{"name": "Packs", "path": tree.path(), "source": "packs"}]})],
    );
    midi.script("get_performance_state", vec![state()]);
    let server = server_with(midi.clone());
    // The state read is the first command; place_sample is the second.
    midi.fail_from(
        1,
        mcp_ableton_music_maker::connection::LiveError::Ableton(
            "'Kick' is a MIDI track; a sample needs an audio track".into(),
        ),
    );
    let r = server
        .run(
            &tools::ADD_SAMPLE,
            AddSampleParams {
                sample: "/x/a.wav".into(),
                track: Some(json!("Kick")),
                section: Some(json!("Verse")),
                ..Default::default()
            },
            mcp_ableton_music_maker::samples::add_sample_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        text_of(&r).contains("is a MIDI track; a sample needs an audio track"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn folders_are_listed_added_and_removed_and_live_has_the_last_word() {
    let _lock = LOCK.lock().await;
    let dir = temp_state();
    let tree = sample_tree();
    let mine = tempfile::tempdir().unwrap();
    wav(&mine.path().join("Vocals/Vocal Chop F 120.wav"), 4.0);
    let b = bridge_for(&tree);
    let server = server_with(b.clone());

    // list: what Live named, walked here — and the Place Live would not locate.
    let r = server
        .run(
            &tools::SAMPLE_FOLDERS,
            SampleFoldersParams {
                action: "list".into(),
                path: None,
            },
            mcp_ableton_music_maker::samples::sample_folders_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("3 files in 1 folder (Factory Packs)"),
        "the deep file, the text file and the hidden file are not samples: {t}"
    );
    assert!(
        t.contains("Live did not say where these Places are, so they are not searched: Splice"),
        "{t}"
    );
    assert!(
        !dir.path().join("sample_folders.json").exists(),
        "nothing is written until a folder is added"
    );

    // add: the script must come back with the folder, or it is not kept.
    b.script(
        "list_sample_folders",
        vec![json!({"folders": [
            {"name": "Factory Packs", "path": tree.path(), "source": "packs"},
            {"name": "sounds", "path": mine.path(), "source": "added"}
        ], "places_without_path": []})],
    );
    let r = server
        .run(
            &tools::SAMPLE_FOLDERS,
            SampleFoldersParams {
                action: "add".into(),
                path: Some(mine.path().to_string_lossy().to_string()),
            },
            mcp_ableton_music_maker::samples::sample_folders_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with(&format!("Added {} — 1 audio file.", mine.path().display())),
        "{t}"
    );
    assert!(t.contains("(you added this)"), "{t}");
    assert!(
        t.contains(&dir.path().join("sample_folders.json").display().to_string()),
        "the reply names the real file, wherever the state dir is: {t}"
    );
    let kept: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("sample_folders.json")).unwrap())
            .unwrap();
    assert_eq!(
        kept["folders"],
        json!([mine.path().to_string_lossy()]),
        "paths only"
    );
    let (_, sent) = b
        .sent()
        .into_iter()
        .rev()
        .find(|(c, _)| c == "list_sample_folders")
        .unwrap();
    assert_eq!(
        sent["roots"],
        json!([mine.path().to_string_lossy()]),
        "the script is told what the producer added, so it can resolve it"
    );
    // And it is searchable now.
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            search("samples", "vocal chop"),
            tools::search_browser_body,
        )
        .await;
    assert!(text_of(&r).contains("Vocal Chop F 120"), "{}", text_of(&r));

    // A folder Live does not report back is not kept.
    let r = server
        .run(
            &tools::SAMPLE_FOLDERS,
            SampleFoldersParams {
                action: "add".into(),
                path: Some("/Users/x/Nowhere".into()),
            },
            mcp_ableton_music_maker::samples::sample_folders_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        text_of(&r).contains("Live cannot see /Users/x/Nowhere"),
        "{}",
        text_of(&r)
    );
    let kept: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("sample_folders.json")).unwrap())
            .unwrap();
    assert_eq!(
        kept["folders"],
        json!([mine.path().to_string_lossy()]),
        "the one that worked is still there, the other is gone"
    );

    // remove takes it out, and the file goes with the last folder.
    let r = server
        .run(
            &tools::SAMPLE_FOLDERS,
            SampleFoldersParams {
                action: "remove".into(),
                path: Some(mine.path().to_string_lossy().to_string()),
            },
            mcp_ableton_music_maker::samples::sample_folders_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(!dir.path().join("sample_folders.json").exists());

    // A relative path is not a folder.
    let r = server
        .run(
            &tools::SAMPLE_FOLDERS,
            SampleFoldersParams {
                action: "add".into(),
                path: Some("Samples".into()),
            },
            mcp_ableton_music_maker::samples::sample_folders_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        text_of(&r).contains("is not an absolute path"),
        "{}",
        text_of(&r)
    );
}

/// The container case: Live names folders this process cannot read. Nothing is
/// searchable, the reply says so plainly, and Live's browser answers instead.
#[tokio::test]
async fn a_server_that_cannot_see_the_folders_says_so() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let b = FakeBridge::responding(json!({}));
    b.script(
        "list_sample_folders",
        vec![json!({"folders": [{"name": "User Library", "path": "/Users/someone-else/Samples", "source": "user_library"}]})],
    );
    b.script(
        "search_browser",
        vec![json!({"items": [{"name": "Perc Loop 02", "path": "Samples/Perc", "uri": "query:Samples#Perc", "category": "samples", "is_device": false}], "complete": true})],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SAMPLE_FOLDERS,
            SampleFoldersParams {
                action: "list".into(),
                path: None,
            },
            mcp_ableton_music_maker::samples::sample_folders_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Claude cannot read /Users/someone-else/Samples from where it runs"),
        "{}",
        text_of(&r)
    );
    // The browser still finds samples, with a uri instead of a path.
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            search("samples", "perc"),
            tools::search_browser_body,
        )
        .await;
    let t = text_of(&r);
    assert!(
        t.contains("Perc Loop 02") && t.contains("uri: query:Samples#Perc"),
        "{t}"
    );
}

#[tokio::test]
async fn the_index_off_switch_keeps_it_in_memory() {
    let _lock = LOCK.lock().await;
    let dir = temp_state();
    std::env::set_var("ABLETON_MCP_LIBRARY_INDEX", "false");
    let tree = sample_tree();
    let b = bridge_for(&tree);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            search("samples", "break"),
            tools::search_browser_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        !dir.path().join("library").exists(),
        "the same switch as the browser index: nothing on disk"
    );
    // Still searchable: the index lives in memory for this process.
    let before = b.commands().len();
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            search("samples", "crash"),
            tools::search_browser_body,
        )
        .await;
    assert!(text_of(&r).contains("Crash Bright 01"), "{}", text_of(&r));
    assert_eq!(b.commands().len(), before);
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
}

#[tokio::test]
async fn the_artist_surface_carries_the_sample_tools() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    let names: Vec<String> = server_with(bridge_for(&tree))
        .tool_list()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(names.contains(&"add_sample".to_string()), "a build tool");
    assert!(
        names.contains(&"adv_sample_folders".to_string()),
        "plumbing is advanced"
    );
    assert!(
        !names.contains(&"sample_folders".to_string()),
        "and not served twice"
    );
    assert!(
        mcp_ableton_music_maker::tools::CORE_TOOLS.contains(&"add_sample"),
        "decision 0006: audio into the song is the artist's"
    );
    for command in ["place_sample", "list_sample_folders"] {
        assert!(
            mcp_ableton_music_maker::tools::ALL_REMOTE_COMMANDS.contains(&command),
            "{command} is cross-checked against the script"
        );
    }
}

#[tokio::test]
async fn batch_can_add_a_sample() {
    let _lock = LOCK.lock().await;
    let _dir = temp_state();
    let tree = sample_tree();
    let b = bridge_for(&tree);
    b.script("place_sample", vec![placed_loop("session")]);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::BATCH,
            tools::BatchParams {
                steps: vec![tools::BatchStep {
                    tool: "add_sample".into(),
                    args: json!({"sample": "/x/a.wav", "track": "FX", "slot": 1}),
                }],
                stop_on_error: true,
            },
            tools::batch_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        b.commands().iter().any(|c| c == "place_sample"),
        "{:?}",
        b.commands()
    );
}
