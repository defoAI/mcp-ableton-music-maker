//! Set memory on request: export_set writes one JSON file under
//! `<state dir>/sets/` and nothing else ever writes there; import_set
//! validates the document, refuses an occupied set, and rebuilds through
//! build_song and set_song. The state directory is process-wide, so the
//! tests take a lock.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::sets::{ExportSetParams, ImportSetParams};
use mcp_ableton_music_maker::tools::{self, BuildSongParams, SetTempoParams};
use serde_json::{json, Value};
use std::sync::Arc;

static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn snapshot() -> Value {
    let note = |p: i64, t: f64| json!({"pitch": p, "start_time": t, "duration": 0.25, "velocity": 100, "mute": false});
    json!({
        "session": {"tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4},
        "tracks": [
            {"index": 0, "name": "Kick", "is_audio_track": false, "mute": false, "volume": 0.85, "panning": 0.0, "color_index": 3,
             "sends": [{"index": 0, "name": "Reverb", "value": 0.0}],
             "devices": [{"index": 0, "name": "Drum Rack", "class_name": "DrumGroupDevice"}],
             "clip_slots": [
                {"index": 0, "has_clip": true, "clip": {"name": "Intro/Kick", "length": 4.0, "is_midi_clip": true, "notes": [note(36, 0.0), note(36, 2.0)]}},
                {"index": 1, "has_clip": true, "clip": {"name": "Groove/Kick", "length": 4.0, "is_midi_clip": true, "notes": [note(36, 0.0), note(36, 1.0), note(36, 2.0), note(36, 3.0)]}},
                {"index": 2, "has_clip": false, "clip": null}]},
            {"index": 1, "name": "Bass", "is_audio_track": false, "mute": false, "volume": 0.7, "panning": -0.1,
             "sends": [{"index": 0, "name": "Reverb", "value": 0.3}],
             "devices": [{"index": 0, "name": "Analog", "class_name": "UltraAnalog"}],
             "clip_slots": [
                {"index": 0, "has_clip": false, "clip": null},
                {"index": 1, "has_clip": true, "clip": {"name": "Groove/Bass", "length": 8.0, "is_midi_clip": true, "notes": [note(41, 0.0)]}}]},
            {"index": 2, "name": "Vox", "is_audio_track": true, "mute": false, "volume": 0.8, "panning": 0.0, "sends": [], "devices": [],
             "clip_slots": [{"index": 1, "has_clip": true, "clip": {"name": "take", "length": 16.0, "is_audio_clip": true, "file_path": "/x/take.wav"}}]}
        ],
        "master_track": {"volume": 0.85}
    })
}

fn context() -> Value {
    json!({
        "live_version": "12.4.6", "script_version": "1.19.0",
        "session": {"tempo": 126.0, "root_note_name": "F", "scale_name": "Minor", "master_volume": 0.85},
        "tracks": [
            {"index": 0, "name": "Kick", "clips": [
                {"slot": 0, "name": "Intro/Kick", "length": 4.0, "is_midi": true},
                {"slot": 1, "name": "Groove/Kick", "length": 4.0, "is_midi": true}],
             "arrangement_clips": 0},
            {"index": 1, "name": "Bass", "clips": [
                {"slot": 1, "name": "Groove/Bass", "length": 8.0, "is_midi": true}],
             "arrangement_clips": 0},
            {"index": 2, "name": "Vox", "clips": [
                {"slot": 1, "name": "take", "length": 16.0, "is_midi": false}],
             "arrangement_clips": 0}
        ],
        "returns": [{"index": 0, "letter": "A", "name": "Reverb"}],
        "scenes": [
            {"index": 0, "name": "Intro · 8", "clip_count": 1}, {"index": 1, "name": "Groove · 8", "clip_count": 3, "tempo": 126.0},
            {"index": 2, "name": "Setlist: Intro×2 → Groove", "clip_count": 0}
        ],
        "cues": [], "events": []
    })
}

fn bridge() -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script("get_session_snapshot", vec![snapshot()]);
    b.script("get_context", vec![context()]);
    b.script("set_tempo", vec![json!({"tempo": 126.0})]);
    b
}

#[tokio::test]
async fn export_set_writes_one_file_under_the_sets_folder_and_only_when_called() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    let b = bridge();
    let server = server_with(b.clone());
    // Any other tool: no sets folder appears.
    let r = server
        .run(
            &tools::SET_TEMPO,
            SetTempoParams { tempo: 126.0 },
            tools::set_tempo_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        !dir.path().join("sets").exists(),
        "nothing exports on its own"
    );
    assert_eq!(
        mcp_ableton_music_maker::state::sets_dir(),
        dir.path().join("sets")
    );

    let r = server
        .run(
            &tools::EXPORT_SET,
            ExportSetParams {
                name: "Techno Friday!".into(),
            },
            mcp_ableton_music_maker::sets::export_set_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands()[1..], ["get_session_snapshot", "get_context"]);
    let path = dir.path().join("sets").join("Techno-Friday.json");
    assert!(path.exists(), "{}", text_of(&r));
    let files: Vec<_> = std::fs::read_dir(dir.path().join("sets"))
        .unwrap()
        .collect();
    assert_eq!(files.len(), 1, "one file per export");
    let doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc["name"], "Techno-Friday");
    assert_eq!(doc["tempo"], 126.0);
    assert_eq!(doc["key"], "F Minor");
    assert_eq!(doc["signature"], json!([4, 4]));
    assert_eq!(doc["tracks"].as_array().unwrap().len(), 3);
    assert_eq!(doc["tracks"][0]["devices"], json!(["Drum Rack"]));
    assert_eq!(doc["tracks"][1]["sends"], json!({"Reverb": 0.3}));
    assert_eq!(doc["tracks"][2]["kind"], "audio");
    assert_eq!(doc["clips"].as_array().unwrap().len(), 4);
    // Notes come back in the form that fits them: four kicks on the beat are
    // a step string, not four objects.
    assert_eq!(doc["clips"][1]["steps"]["36"], "x...x...x...x");
    assert_eq!(
        doc["clips"][1].get("notes"),
        None,
        "no note objects: {}",
        doc["clips"][1]
    );
    assert_eq!(doc["clips"][3]["file_path"], "/x/take.wav");
    assert_eq!(
        doc["sections"],
        json!([{"index": 0, "name": "Intro", "phrase_bars": 8, "tempo": null}, {"index": 1, "name": "Groove", "phrase_bars": 8, "tempo": 126.0}])
    );
    assert_eq!(
        doc["setlist"],
        json!([{"section": "Intro", "repeats": 2}, {"section": "Groove", "repeats": null}])
    );
    let t = text_of(&r);
    assert!(t.starts_with(&format!("Exported to {}: 3 tracks (0 instruments by URI, every device by name), 2 sections with 4 clips and 7 notes, mixer and sends, the setlist (2 entries), tempo and key F Minor.", path.display())), "{t}");
    assert!(
        t.contains("import_set {\"name\": \"Techno-Friday\"} rebuilds it")
            && t.contains("\"Delete all local data\""),
        "{t}"
    );
    std::env::remove_var("ABLETON_MCP_STATE_DIR");
}

#[tokio::test]
async fn build_song_writes_a_snapshot_only_when_it_is_asked_for() {
    // A crash costs whatever exists only in Live's memory, and the Live API
    // has no save. build_song can write the rebuildable copy itself — but the
    // flag is the consent, because a set export is written only when asked.
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    let server = server_with(bridge());
    let doc = BuildSongParams {
        tempo: Some(126.0),
        ..Default::default()
    };
    let r = server
        .run(&tools::BUILD_SONG, doc.clone(), tools::build_song_body)
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        !dir.path().join("sets").exists(),
        "a build does not export on its own"
    );
    assert!(
        t.contains("export_set") && t.contains("Cmd+S"),
        "but it says, at the moment of risk, what the copy would be: {t}"
    );

    let r = server
        .run(
            &tools::BUILD_SONG,
            BuildSongParams {
                parts: Default::default(),
                snapshot: true,
                ..doc
            },
            tools::build_song_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    let files: Vec<_> = std::fs::read_dir(dir.path().join("sets"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1, "one snapshot per asked-for build: {t}");
    let name = files[0].file_name().to_string_lossy().to_string();
    assert!(
        name.starts_with("build-") && name.ends_with(".json"),
        "{name}"
    );
    assert!(
        t.contains("Snapshot: Exported to") && t.contains(&name),
        "the reply names the file, or nobody can import it: {t}"
    );
}

#[tokio::test]
async fn import_set_validates_refuses_an_occupied_set_and_rebuilds_through_build_song() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    let b = bridge();
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::EXPORT_SET,
            ExportSetParams {
                name: "friday".into(),
            },
            mcp_ableton_music_maker::sets::export_set_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));

    // A name that was never exported.
    let r = server
        .run(
            &tools::IMPORT_SET,
            ImportSetParams {
                name: "monday".into(),
                merge: false,
                dry_run: false,
            },
            mcp_ableton_music_maker::sets::import_set_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).starts_with("no exported set 'monday'"),
        "{}",
        text_of(&r)
    );
    // dry_run describes and sends nothing.
    let before = b.commands().len();
    let r = server
        .run(
            &tools::IMPORT_SET,
            ImportSetParams {
                name: "friday".into(),
                merge: false,
                dry_run: true,
            },
            mcp_ableton_music_maker::sets::import_set_body,
        )
        .await;
    assert!(
        !is_error(&r) && text_of(&r).contains("Dry run: nothing was sent to Live."),
        "{}",
        text_of(&r)
    );
    assert_eq!(b.commands().len(), before);
    // An occupied set is refused unless merge.
    b.script("get_context", vec![json!({"tracks": [{"index": 0, "name": "Old"}, {"index": 1, "name": "Older"}], "scenes": [], "session": {}})]);
    let r = server
        .run(
            &tools::IMPORT_SET,
            ImportSetParams {
                name: "friday".into(),
                merge: false,
                dry_run: false,
            },
            mcp_ableton_music_maker::sets::import_set_body,
        )
        .await;
    assert!(
        is_error(&r)
            && text_of(&r).starts_with(
                "This set already has 2 tracks; import_set rebuilds into an empty set."
            ),
        "{}",
        text_of(&r)
    );
    assert_eq!(b.commands().last().unwrap(), "get_context", "nothing built");

    // An empty set: build_song (scenes, tracks, clips), the key, the setlist.
    b.script(
        "get_context",
        vec![json!({"tracks": [], "scenes": [], "session": {}})],
    );
    b.script("get_performance_state", vec![
        json!({"scenes": [], "tracks": []}),
        json!({"scenes": [{"index": 0, "name": "Intro · 8", "clip_tracks": [0]}, {"index": 1, "name": "Groove · 8", "clip_tracks": [0, 1]}],
               "tracks": [{"index": 0, "name": "Kick", "slots_with_clips": [0, 1]}, {"index": 1, "name": "Bass", "slots_with_clips": [1]}]}),
    ]);
    b.script(
        "create_tracks",
        vec![json!({"created": [{"index": 0, "name": "Kick"}, {"index": 1, "name": "Bass"}, {"index": 2, "name": "Vox"}]})],
    );
    b.script(
        "set_scale",
        vec![json!({"root_note_name": "F", "scale_name": "Minor"})],
    );
    b.script(
        "create_scene",
        vec![
            json!({"index": 0, "name": "Intro · 8"}),
            json!({"index": 1, "name": "Groove · 8"}),
            json!({"index": 2, "name": "Setlist: Intro×2 → Groove"}),
        ],
    );
    let before = b.commands().len();
    let r = server
        .run(
            &tools::IMPORT_SET,
            ImportSetParams {
                name: "friday".into(),
                merge: false,
                dry_run: false,
            },
            mcp_ableton_music_maker::sets::import_set_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands()[before..].to_vec();
    assert_eq!(
        cmds[..3],
        ["get_context", "set_scale", "set_tempo"],
        "the key is set before anything is written"
    );
    assert_eq!(
        cmds.iter().filter(|c| *c == "create_scene").count(),
        3,
        "two sections and the Setlist: scene: {cmds:?}"
    );
    let tracks = b
        .sent()
        .iter()
        .find(|(c, _)| c == "create_tracks")
        .map(|(_, p)| p["tracks"].clone())
        .unwrap();
    assert_eq!(
        tracks.as_array().unwrap().len(),
        3,
        "every track in one round trip"
    );
    assert_eq!(tracks[2]["kind"], "audio");
    let clips = b
        .sent()
        .iter()
        .find(|(c, _)| c == "write_clips")
        .map(|(_, p)| p["clips"].clone())
        .unwrap();
    assert_eq!(
        clips.as_array().unwrap().len(),
        3,
        "the audio clip is not rebuilt; the MIDI clips go in one round trip"
    );
    assert!(cmds.contains(&"set_scale".to_string()));
    let scene_names: Vec<String> = b.sent()[before..]
        .iter()
        .filter(|(c, _)| c == "create_scene")
        .map(|(_, p)| p["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        scene_names,
        vec!["Intro · 8", "Groove · 8", "Setlist: Intro×2 → Groove"]
    );
    let t = text_of(&r);
    assert!(t.starts_with("Rebuilding 'friday' ("), "{t}");
    assert!(
        t.contains("Key F Minor") && t.contains("Song: Intro×2 → Groove."),
        "{t}"
    );
    assert!(
        t.contains("Not rebuilt: 1 audio clip (take on Vox)")
            && t.ends_with("Save the set in Live (Cmd+S) to keep it."),
        "{t}"
    );
    std::env::remove_var("ABLETON_MCP_STATE_DIR");
}

/// What `get_performance_state` says about the same set, so the edit side
/// and the read side agree about what is there.
fn perf_state() -> Value {
    json!({
        "is_playing": false, "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
        "beat": 0.0, "bar": 1, "beat_in_bar": 1, "clip_trigger_quantization": 4,
        "tracks": [
            {"index": 0, "name": "Kick", "slots_with_clips": [0, 1]},
            {"index": 1, "name": "Bass", "slots_with_clips": [1]},
            {"index": 2, "name": "Vox", "slots_with_clips": [1]}
        ],
        "scenes": [
            {"index": 0, "name": "Intro · 8", "clip_tracks": [0], "phrase_bars": 8},
            {"index": 1, "name": "Groove · 8", "clip_tracks": [0, 1, 2], "phrase_bars": 8, "tempo": 126.0},
            {"index": 2, "name": "Setlist: Intro×2 → Groove", "clip_tracks": []}
        ],
        "cues": [], "events": []
    })
}

/// The JSON a readout carries after its sentence.
fn document_in(text: &str) -> Value {
    let at = text.rfind("\n{").expect("the readout carries a document");
    serde_json::from_str(&text[at + 1..]).expect("the document parses")
}

#[tokio::test]
async fn the_song_reads_back_at_three_levels() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    let b = bridge();
    b.script("get_performance_state", vec![perf_state()]);
    let note = |p: i64, t: f64| json!({"pitch": p, "start_time": t, "duration": 0.25, "velocity": 100, "mute": false});
    b.script(
        "get_clip_notes",
        vec![
            json!({"notes": [note(36, 0.0), note(36, 1.0), note(36, 2.0), note(36, 3.0)]}),
            json!({"notes": [note(41, 0.0)]}),
        ],
    );
    let server = server_with(b.clone());
    let read = |level: &str, section: Option<&str>| tools::GetContextParams {
        as_level: Some(level.to_string()),
        section: section.map(str::to_string),
        ..Default::default()
    };

    // The shape of the song: no notes, so it is cheap to ask for often.
    let r = server
        .run(
            &tools::GET_CONTEXT,
            read("song", None),
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.starts_with("F Minor, 126 BPM. The set is r"), "{t}");
    assert!(
        t.contains("Sections: Intro · 8 (1 clip) · Groove · 8 (3 clips)"),
        "{t}"
    );
    assert!(
        t.contains("Song:     Intro×2 → Groove. 2 entries, 16 bars counted."),
        "{t}"
    );
    assert!(
        t.contains("Tracks:   3 (3 with clips), 0 in the Arrangement."),
        "{t}"
    );
    assert_eq!(
        b.commands(),
        vec!["get_context"],
        "the shape costs one read"
    );

    // One section: that row's notes, and only that row's.
    let r = server
        .run(
            &tools::GET_CONTEXT,
            read("section", Some("Groove")),
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("Groove · 8 bars, scene 1, 3 clips (5 notes)."),
        "{t}"
    );
    assert!(t.contains("\"steps\""), "{t}");
    assert!(!t.contains("Intro/Kick"), "a row nobody asked about: {t}");

    // A section the set does not have is answered with the ones it does.
    let r = server
        .run(
            &tools::GET_CONTEXT,
            read("section", Some("Groov")),
            tools::get_context_body,
        )
        .await;
    assert!(is_error(&r));
    assert!(
        text_of(&r).contains("did you mean Groove?"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn a_document_read_and_posted_straight_back_is_nothing_to_do() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    let b = bridge();
    b.script("get_performance_state", vec![perf_state()]);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::GET_CONTEXT,
            tools::GetContextParams {
                as_level: Some("document".into()),
                ..Default::default()
            },
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("The set as a document — 3 tracks, 4 clips (7 notes), 0 placements, 0 locators, 2 sections, 2 setlist entries. The set is r"),
        "{t}"
    );
    assert!(t.contains("Nothing was written to disk"), "{t}");
    assert!(!dir.path().join("sets").exists(), "a read wrote a file");
    let doc = document_in(&t);
    assert_eq!(doc["clips"][1]["steps"]["36"], "x...x...x...x");

    // The same document, straight back: the comparison is on the notes, so
    // nothing is rewritten.
    let r = server
        .run(
            &tools::UPDATE_SONG,
            mcp_ableton_music_maker::sections::UpdateSongParams {
                document: Some(doc.clone()),
                dry_run: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::update_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).starts_with("Nothing to do: the set already reads like that."),
        "{}",
        text_of(&r)
    );

    // One clip changed in the document: one clip written, and the reply says
    // what it left alone.
    let mut changed = doc.clone();
    changed["clips"][1]["steps"]["36"] = json!("x.......x.......");
    let r = server
        .run(
            &tools::UPDATE_SONG,
            mcp_ableton_music_maker::sections::UpdateSongParams {
                document: Some(changed),
                dry_run: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::update_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.starts_with("Plan — nothing was sent to Live."), "{t}");
    assert!(t.contains("Kick rewritten (2 notes)"), "{t}");
    assert!(t.contains("2 clips untouched"), "{t}");
}
