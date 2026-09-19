//! End to end: the real `ableton-music-maker` binary, spoken to over stdio by an MCP
//! client, with a fake Remote Script listening on TCP. Proves the whole
//! stack: stdio transport, handshake, capability gate, tool dispatch, the
//! Live wire protocol, and that stdout carries nothing but the protocol.

use rmcp::model::CallToolRequestParams;
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A fake AbletonMusicMaker Remote Script: one JSON document in, one out.
async fn fake_live(received: Arc<Mutex<Vec<Value>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let received = received.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    let Ok(command) = serde_json::from_slice::<Value>(&buf) else {
                        continue;
                    };
                    buf.clear();
                    received.lock().unwrap().push(command.clone());
                    let reply = match command["type"].as_str().unwrap_or("") {
                        "get_script_info" => json!({"status": "success", "result": {
                            "script_version": mcp_ableton_music_maker::handshake::expected_remote_script_version(),
                            "protocol_version": 1,
                            "capabilities": mcp_ableton_music_maker::tools::ALL_REMOTE_COMMANDS,
                        }}),
                        "get_session_info" => json!({"status": "success", "result": {
                            "tempo": 120.0, "signature_numerator": 4, "track_count": 2,
                            "tracks": [{"index": 0, "name": "Kick"}, {"index": 1, "name": "Bass"}]
                        }}),
                        "set_tempo" => {
                            json!({"status": "success", "result": {"tempo": command["params"]["tempo"]}})
                        }
                        "get_browser_tree" => {
                            json!({"status": "error", "message": "Browser is not available"})
                        }
                        other => {
                            json!({"status": "error", "message": format!("Unknown command: {other}")})
                        }
                    };
                    stream
                        .write_all(reply.to_string().as_bytes())
                        .await
                        .unwrap();
                }
            });
        }
    });
    port
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
    let received = Arc::new(Mutex::new(Vec::new()));
    let port = fake_live(received.clone()).await;
    let state_dir = tempfile::tempdir().unwrap();

    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_ableton-music-maker"));
    cmd.env("ABLETON_HOST", "127.0.0.1")
        .env("ABLETON_PORT", port.to_string())
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
    assert_eq!(tools.len(), 92);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in [
        "get_session_info",
        "add_notes_to_clip",
        "load_drum_kit",
        "get_remote_script_info",
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
        .call_tool(CallToolRequestParams::new("get_session_info"))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    let payload: Value = serde_json::from_str(&text(&result)).unwrap();
    assert_eq!(payload["tempo"], 120.0);
    assert_eq!(payload["tracks"][1]["name"], "Bass");

    // A write tool with arguments reaches Live with the right params.
    let mut args = serde_json::Map::new();
    args.insert("tempo".into(), json!(128.0));
    let mut call = CallToolRequestParams::new("set_tempo");
    call.arguments = Some(args);
    let result = client.call_tool(call).await.unwrap();
    assert_eq!(text(&result), "Set tempo to 128 BPM");

    // A Live-side error becomes an error result, not a protocol failure.
    let result = client
        .call_tool(CallToolRequestParams::new("get_browser_tree"))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("browser is not available"),
        "{}",
        text(&result)
    );

    // Invalid arguments are rejected before anything reaches Live.
    let mut bad = CallToolRequestParams::new("set_tempo");
    let mut bad_args = serde_json::Map::new();
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
    let tree = lines
        .iter()
        .find(|l| l["tool"] == "get_browser_tree")
        .unwrap();
    assert_eq!(tree["ok"], false);
    assert!(tree["error"].as_str().unwrap().contains("not available"));

    let commands: Vec<String> = received
        .lock()
        .unwrap()
        .iter()
        .map(|c| c["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        commands[0], "get_script_info",
        "startup handshake comes first"
    );
    assert!(commands.contains(&"get_session_info".to_string()));
    let tempo_cmd = received
        .lock()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "set_tempo")
        .cloned()
        .unwrap();
    assert_eq!(tempo_cmd["params"], json!({"tempo": 128.0}));
    assert!(commands.contains(&"get_browser_tree".to_string()));
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
    let received = Arc::new(Mutex::new(Vec::new()));
    let port = fake_live(received.clone()).await;
    let state_dir = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_ableton-music-maker"))
        .arg("--check")
        .env("ABLETON_HOST", "127.0.0.1")
        .env("ABLETON_PORT", port.to_string())
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
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["live_reachable"], true);
    assert_eq!(report["up_to_date"], true);
    assert_eq!(report["session"]["tempo"], 120.0);
    assert_eq!(report["session"]["track_count"], 2);

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
