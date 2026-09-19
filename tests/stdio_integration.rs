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
        .env("ABLETON_MCP_DISABLE_TELEMETRY", "true")
        .env("ABLETON_MCP_DISABLE_DATASET", "true")
        .env("ABLETON_MCP_STATE_DIR", state_dir.path())
        .env("ABLETON_MCP_DATA_DIR", state_dir.path())
        .env("RUST_LOG", "warn");
    let client =
        ().serve(TokioChildProcess::new(cmd).unwrap())
            .await
            .expect("initialize handshake over stdio");

    let info = client.peer_info().expect("server info");
    let server_info = info.server_info.as_ref().expect("server implementation");
    assert_eq!(server_info.name, "AbletonMusicMaker");
    assert_eq!(server_info.version, mcp_ableton_music_maker::MCP_VERSION);

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 37);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in [
        "get_session_info",
        "add_notes_to_clip",
        "load_drum_kit",
        "set_dataset_consent",
        "record_audition",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }

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
async fn privacy_status_reports_gates_off() {
    let state_dir = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_ableton-music-maker"))
        .arg("--privacy-status")
        .env("ABLETON_MCP_STATE_DIR", state_dir.path())
        .env("ABLETON_MCP_DATA_DIR", state_dir.path())
        .env_remove("ABLETON_MCP_ENABLE_TELEMETRY")
        .env_remove("ABLETON_MCP_SUPABASE_URL")
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["telemetry_enabled"], false);
    assert_eq!(status["dataset_enabled"], false);
    assert_eq!(status["has_supabase_credentials"], false);
    assert_eq!(status["would_prompt_for_consent"], false);
    assert_eq!(
        status["expected_remote_script_version"],
        mcp_ableton_music_maker::handshake::expected_remote_script_version()
    );
}
