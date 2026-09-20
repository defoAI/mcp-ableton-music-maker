//! End to end: the real `ableton-music-maker` binary, spoken to over stdio by
//! an MCP client, against the real Remote Script on a fake Live
//! (`scripts/fake-live.py`). Proves the whole stack — stdio transport,
//! handshake, capability gate, tool dispatch, the Live wire protocol, and
//! that stdout carries nothing but the protocol — and then **builds
//! something and asks the set about it**.
//!
//! It used to answer four commands from an inline stub and error on the
//! other 93, so `build_song` and the note path were never exercised here at
//! all. Now it builds four tracks with instruments, writes a clip, makes a
//! section, arranges it to bars and plays it, and every assertion about the
//! result is a read back out of Live rather than the tool's own words.

mod common;

use common::spawn_fake_live;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde_json::{json, Map, Value};

/// Call a tool with arguments, as a client does.
async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &str,
    args: Value,
) -> rmcp::model::CallToolResult {
    let mut request = CallToolRequestParams::new(name.to_string());
    if let Value::Object(map) = args {
        request.arguments = Some(map);
    }
    client
        .call_tool(request)
        .await
        .unwrap_or_else(|e| panic!("{name} failed at the protocol level: {e}"))
}

fn text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

#[tokio::test]
async fn full_stack_over_stdio() {
    // One shared set: this is the server the way a producer runs it, and a
    // real Live is one set.
    let live = spawn_fake_live(&["--shared-set", "--latency", "none", "--quiet"]);
    let state_dir = tempfile::tempdir().unwrap();

    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_ableton-music-maker"));
    cmd.env("ABLETON_HOST", "127.0.0.1")
        .env("ABLETON_PORT", live.port.to_string())
        .env("ABLETON_MCP_STATE_DIR", state_dir.path())
        .env("RUST_LOG", "warn");
    let client =
        ().serve(TokioChildProcess::new(cmd).unwrap())
            .await
            .expect("initialize handshake over stdio");

    let info = client.peer_info().expect("server info");
    let server_info = info.server_info.as_ref().expect("server implementation");
    assert_eq!(server_info.name, "AbletonMusicMaker");
    assert_eq!(server_info.version, mcp_ableton_music_maker::MCP_VERSION);
    let instructions = info.instructions.as_deref().expect("server instructions");
    for word in [
        "get_context",
        "build_song",
        "capture_mix",
        "start_performance",
        "cue",
    ] {
        assert!(instructions.contains(word), "instructions lack {word}");
    }

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 105);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in [
        "adv_edit_devices",
        "adv_get_session_info",
        "add_notes_to_clip",
        "adv_load_drum_kit",
        "adv_get_remote_script_info",
        "get_context",
        "add_sample",
        "adv_sample_folders",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
    for gone in ["set_dataset_consent", "record_audition", "submit_intent"] {
        assert!(!names.contains(&gone), "{gone} should not exist");
    }

    // The heartbeat names the client that started the server.
    let sessions = std::fs::read_dir(state_dir.path().join("sessions")).unwrap();
    let heartbeat: Vec<_> = sessions.map(|e| e.unwrap().path()).collect();
    assert_eq!(heartbeat.len(), 1, "one heartbeat per running server");
    let beat: Value = serde_json::from_slice(&std::fs::read(&heartbeat[0]).unwrap()).unwrap();
    assert_eq!(beat["server_version"], mcp_ableton_music_maker::MCP_VERSION);
    assert!(beat["client"]["name"].is_string(), "{beat}");
    assert!(
        beat["activity_file"].is_string(),
        "activity on by default: {beat}"
    );

    // A read tool returns Live's payload as JSON.
    let result = client
        .call_tool(CallToolRequestParams::new("adv_get_session_info"))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    let payload: Value = serde_json::from_str(&text(&result)).unwrap();
    assert_eq!(payload["tempo"], 120.0, "a new Live set is 120 BPM");
    assert_eq!(
        payload["track_count"], 4,
        "two MIDI and two audio: {payload}"
    );

    // A write tool with arguments reaches Live, and the set moves.
    let result = call(&client, "set_tempo", json!({"tempo": 128.0})).await;
    assert_eq!(text(&result), "Set tempo to 128 BPM");
    let session: Value = serde_json::from_str(&text(
        &call(&client, "adv_get_session_info", json!({})).await,
    ))
    .unwrap();
    assert_eq!(session["tempo"], 128.0, "the tempo did not move in the set");

    // ── build something, then ask the set ──────────────────────────────
    let built = call(
        &client,
        "build_song",
        json!({
            "tempo": 124.0,
            "tracks": [
                {"name": "Kick",  "kind": "midi", "instrument_query": "drum rack"},
                {"name": "Bass",  "kind": "midi", "instrument_query": "analog"},
                {"name": "Keys",  "kind": "midi", "instrument_query": "operator"},
                {"name": "Vox",   "kind": "audio"}
            ],
            "clips": [
                {"track": "Kick", "slot": 0, "name": "Kick 1", "length": 4.0,
                 "steps": {"36": "x...x...x...x..."}},
                {"track": "Bass", "slot": 0, "name": "Bass 1", "length": 4.0,
                 "steps": {"38": "x.x.x.x.x.x.x.x."}}
            ],
            // Beats, as the document takes them: bars 1-4 and bars 3-4.
            "placements": [
                {"track": "Kick", "slot": 0, "times": [0.0, 4.0, 8.0, 12.0]},
                {"track": "Bass", "slot": 0, "times": [8.0, 12.0]}
            ],
            "locators": [{"name": "Drop", "time": 8.0}]
        }),
    )
    .await;
    assert!(
        !built.is_error.unwrap_or(false),
        "build_song: {}",
        text(&built)
    );

    // Tracks, by name, out of Live. The artist tools answer in prose on
    // purpose, so the names come off the raw per-track read.
    let session: Value = serde_json::from_str(&text(
        &call(&client, "adv_get_session_info", json!({})).await,
    ))
    .unwrap();
    let count = session["track_count"].as_u64().expect("track_count") as usize;
    let mut names: Vec<String> = Vec::new();
    for index in 0..count {
        let track: Value = serde_json::from_str(&text(
            &call(&client, "adv_get_track_info", json!({"track_index": index})).await,
        ))
        .unwrap_or_else(|e| panic!("get_track_info {index} was not JSON: {e}"));
        names.push(track["name"].as_str().unwrap_or_default().to_string());
    }
    for wanted in ["Kick", "Bass", "Keys", "Vox"] {
        assert!(
            names.iter().any(|n| n == wanted),
            "no track {wanted} in {names:?}"
        );
    }
    let kick = names.iter().position(|n| n == "Kick").unwrap();
    let bass = names.iter().position(|n| n == "Bass").unwrap();

    // And `get_context` still answers the artist, in words.
    let readout = text(&call(&client, "get_context", json!({})).await);
    assert!(readout.contains("Kick"), "get_context readout: {readout}");

    // The tempo build_song was given.
    let session: Value = serde_json::from_str(&text(
        &call(&client, "adv_get_session_info", json!({})).await,
    ))
    .unwrap();
    assert_eq!(session["tempo"], 124.0);

    // Notes, read back through get_clip_notes.
    let notes: Value = serde_json::from_str(&text(
        &call(
            &client,
            "adv_get_clip_notes",
            json!({"track_index": kick, "clip_index": 0}),
        )
        .await,
    ))
    .unwrap();
    let pitches: Vec<i64> = notes["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .filter_map(|n| n["pitch"].as_i64())
        .collect();
    assert_eq!(pitches, vec![36, 36, 36, 36], "four kicks in the bar");

    // Arrangement clips at the bars they were placed on. Bars are 1-based
    // for the artist; Live counts beats from zero.
    let arrangement: Value = serde_json::from_str(&text(
        &call(
            &client,
            "adv_get_arrangement_clips",
            json!({"track_index": bass}),
        )
        .await,
    ))
    .unwrap();
    let starts: Vec<f64> = arrangement["clips"]
        .as_array()
        .expect("arrangement clips")
        .iter()
        .filter_map(|c| c["start_time"].as_f64())
        .collect();
    assert_eq!(starts, vec![8.0, 12.0], "Bass was placed at bars 3 and 4");

    // A section, by the name it was given.
    let section = call(
        &client,
        "make_section",
        json!({
            "name": "Drop",
            "phrase_bars": 8,
            "clips": {
                "Kick": {"steps": {"36": "x...x...x...x..."}},
                "Bass": {"notes_csv": "41,0,4,90"}
            }
        }),
    )
    .await;
    assert!(
        !section.is_error.unwrap_or(false),
        "make_section: {}",
        text(&section)
    );
    // The section is a scene in Live, named "<name> · <bars>". The artist
    // readout is where it is visible to a client.
    let readout = text(&call(&client, "adv_get_performance_state", json!({})).await);
    assert!(readout.contains("Drop"), "no Drop section in:\n{readout}");

    // And the transport runs when told to.
    let played = call(&client, "adv_start_playback", json!({})).await;
    assert!(!played.is_error.unwrap_or(false), "{}", text(&played));
    let session: Value = serde_json::from_str(&text(
        &call(&client, "adv_get_session_info", json!({})).await,
    ))
    .unwrap();
    assert_eq!(session["is_playing"], true);
    let _ = call(&client, "adv_stop_playback", json!({})).await;

    // A Live-side error becomes an error result, not a protocol failure.
    let result = call(&client, "adv_get_track_info", json!({"track_index": 99})).await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).to_lowercase().contains("range"),
        "{}",
        text(&result)
    );

    // Invalid arguments are rejected before anything reaches Live.
    let mut bad = CallToolRequestParams::new("set_tempo");
    let mut bad_args = Map::new();
    bad_args.insert("tempo".into(), json!("fast"));
    bad.arguments = Some(bad_args);
    let bad_result = client.call_tool(bad).await;
    assert!(bad_result.is_err() || bad_result.as_ref().unwrap().is_error == Some(true));

    client.cancel().await.unwrap();

    // Clean shutdown removes the heartbeat; the activity file stays.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        std::fs::read_dir(state_dir.path().join("sessions"))
            .unwrap()
            .next()
            .is_none(),
        "heartbeat removed on shutdown"
    );
    let activity_dir = state_dir.path().join("activity");
    let log = std::fs::read_dir(&activity_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let lines: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(lines.len() >= 3, "one line per tool call: {lines:?}");
    let tempo = lines.iter().find(|l| l["tool"] == "set_tempo").unwrap();
    assert_eq!(tempo["ok"], true);
    assert_eq!(tempo["commands"], json!(["set_tempo"]));
    assert!(tempo.get("params").is_none(), "payloads off by default");
    assert!(tempo["in_chars"].as_u64().unwrap() > 0);
    // The out-of-range read is the one that failed; the loop over the real
    // tracks is in the log too, and succeeded.
    let failed = lines
        .iter()
        .find(|l| l["tool"] == "get_track_info" && l["ok"] == false)
        .expect("the failing get_track_info is not in the log");
    assert!(failed["error"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("range"));
    assert!(
        lines
            .iter()
            .any(|l| l["tool"] == "get_track_info" && l["ok"] == true),
        "the reads that worked are in the log too"
    );

    // The build is in the log as one call carrying several commands.
    let build = lines.iter().find(|l| l["tool"] == "build_song").unwrap();
    assert_eq!(build["ok"], true, "{build}");
    let commands = build["commands"].as_array().expect("commands on the line");
    for expected in ["create_tracks", "write_clips", "place_clips"] {
        assert!(
            commands.iter().any(|c| c == expected),
            "build_song did not send {expected}: {commands:?}"
        );
    }
    drop(live);
}

#[tokio::test]
async fn status_reports_paths_and_no_uploads() {
    let state_dir = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_ableton-music-maker"))
        .arg("--status")
        .env("ABLETON_MCP_STATE_DIR", state_dir.path())
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["uploads"], "none");
    assert_eq!(status["activity"]["enabled"], true);
    assert_eq!(status["activity"]["payloads"], false);
    assert_eq!(status["state_dir"], state_dir.path().to_str().unwrap());
    assert_eq!(
        status["expected_remote_script_version"],
        mcp_ableton_music_maker::handshake::expected_remote_script_version()
    );
    for gone in [
        "telemetry_enabled",
        "dataset_enabled",
        "has_supabase_credentials",
    ] {
        assert!(status.get(gone).is_none(), "{gone} must not exist");
    }
}

#[tokio::test]
async fn check_reports_live_and_exits_by_script_state() {
    let live = spawn_fake_live(&["--shared-set", "--latency", "none", "--quiet"]);
    let state_dir = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_ableton-music-maker"))
        .arg("--check")
        .env("ABLETON_HOST", "127.0.0.1")
        .env("ABLETON_PORT", live.port.to_string())
        .env("ABLETON_MCP_STATE_DIR", state_dir.path())
        .env("RUST_LOG", "warn")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // #53 section C: what `--check` reports against the fake.
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["live_reachable"], true);
    assert_eq!(report["up_to_date"], true, "{report}");
    assert_eq!(report["protocol_version"], 2);
    assert_eq!(report["socket_reader"], "main_thread_tick");
    assert_eq!(
        report["capabilities"],
        mcp_ableton_music_maker::tools::ALL_REMOTE_COMMANDS.len(),
        "{report}"
    );
    assert_eq!(report["live"]["version"], "12.4.6");
    assert_eq!(report["bind_is_loopback"], true);
    // The script measured the fake's own clock and reported it, the way it
    // reports Live's. The fake aims at Live's 100 ms, but a loaded CI
    // runner can stretch a Python ticker thread well past that, so this
    // asserts the shape — a tick, in the right order — not the figure. What
    // Live's tick actually is, is a real-Live check
    // (`ableton-music-maker --check`, and #53 section E).
    let period = report["tick"]["period_ms"].as_f64().expect("a tick period");
    assert!(
        (50.0..500.0).contains(&period),
        "the tick was {period} ms, which is not a tick"
    );
    assert!(
        report["tick"]["samples"].as_u64().unwrap_or(0) > 0,
        "{report}"
    );
    assert_eq!(report["session"]["tempo"], 120.0);
    assert_eq!(report["session"]["track_count"], 4);

    // Nothing listening: exit 1, still valid JSON, nothing on stdout but it.
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_ableton-music-maker"))
        .arg("--check")
        .env("ABLETON_HOST", "127.0.0.1")
        .env("ABLETON_PORT", "1")
        .env("ABLETON_MCP_STATE_DIR", state_dir.path())
        .env("RUST_LOG", "error")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["live_reachable"], false);
    assert!(report["error"].is_string());
}
