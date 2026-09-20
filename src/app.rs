//! Process lifecycle for the `ableton-music-maker` binary: startup handshake,
//! the heartbeat the Mac app reads, stdio serving, orderly shutdown — and
//! the two reports, `--status` and `--check`.

use crate::connection::{live_address, AbletonConnection, LiveState, RealBridge};
use crate::tools::Server;
use rmcp::ServiceExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// What this binary is and where it writes. No network state to report:
/// the only socket it opens is the one to Live.
pub fn status() -> Value {
    let activity = crate::activity::Activity::from_env();
    let (host, port) = live_address();
    json!({
        "version": crate::MCP_VERSION,
        "expected_remote_script_version": crate::handshake::expected_remote_script_version(),
        "live": {"host": host, "port": port},
        "state_dir": crate::state::state_dir(),
        "activity": {
            "enabled": activity.enabled(),
            "payloads": activity.payloads(),
            "dir": crate::state::activity_dir(),
        },
        "sessions_dir": crate::state::sessions_dir(),
        "uploads": "none",
    })
}

/// Ask Live for the loaded Remote Script and the current session. Changes
/// nothing. `true` when the script is loaded and up to date.
pub fn check() -> (Value, bool) {
    let (host, port) = live_address();
    let live = LiveState::new(Arc::new(AbletonConnection::new(host, port)));
    let info = live.script.handshake(live.bridge.as_ref());
    let reachable = info.script_version.is_some();
    let session = if reachable {
        live.send_command("get_session_info", None).ok()
    } else {
        None
    };
    live.bridge.disconnect();
    let (host, port) = live_address();
    let report = json!({
        "live_reachable": reachable,
        "host": host,
        "port": port,
        "script_version": info.script_version,
        "expected_version": info.expected_version,
        "up_to_date": info.up_to_date,
        "capabilities": info.capabilities.len(),
        "bind_host": info.extra.get("bind_host"),
        "bind_is_loopback": info.extra.get("bind_is_loopback"),
        "protocol_version": info.protocol_version,
        // Phase 0 of the streams story: the tick period the script measures,
        // and where it reads its sockets. Absent on a script older than 1.28.
        "tick": info.extra.get("tick"),
        "socket_reader": info.extra.get("socket_reader"),
        "live": info.extra.get("live"),
        "session": session.as_ref().map(|s| json!({
            "tempo": s.get("tempo"),
            "track_count": s.get("track_count"),
            "view": s.get("current_view").or_else(|| s.get("view")),
        })),
        "error": info.error,
    });
    let ok = info.up_to_date;
    (report, ok)
}

/// The captures on the Capture track, for the app and `--captures`. Connects
/// to Live once; an unreachable Live is an error string, not a panic.
pub fn captures() -> Result<Value, String> {
    let (host, port) = live_address();
    let live = LiveState::with_activity(
        Arc::new(AbletonConnection::new(host, port)),
        crate::activity::Activity::disabled(),
    );
    let info = live.script.handshake(live.bridge.as_ref());
    if info.script_version.is_none() {
        return Err(info.error.unwrap_or_else(|| "Live not reachable".into()));
    }
    if !live.script.has_capability("list_captures") {
        return Err(format!(
            "the loaded Remote Script ({}) has no captures; reinstall and restart Live",
            info.script_version.unwrap_or_default()
        ));
    }
    let r = live
        .send_command("list_captures", None)
        .map_err(|e| e.to_string());
    live.bridge.disconnect();
    r
}

/// The heartbeat file for this process: `<sessions_dir>/<pid>.json`.
fn heartbeat_path() -> PathBuf {
    crate::state::sessions_dir().join(format!("{}.json", std::process::id()))
}

/// Tell the Mac app that this server exists, who started it and what it
/// found in Live. Written at startup (client unknown), rewritten on the MCP
/// `initialize` request, removed on shutdown.
pub fn write_heartbeat(live: &LiveState, client: Option<(String, String)>) {
    let path = heartbeat_path();
    let (host, port) = live_address();
    let started = STARTED.get_or_init(|| chrono::Local::now().to_rfc3339());
    let script_version = live.script.get().and_then(|i| i.script_version);
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((client.clone(), script_version.clone()));
    let doc = json!({
        "pid": std::process::id(),
        "started": started,
        "client": client.map(|(name, version)| json!({"name": name, "version": version})),
        "server_version": crate::MCP_VERSION,
        "script_version": script_version,
        "activity_file": live.activity.enabled().then(|| live.activity.path().to_path_buf()),
        "live": {"host": host, "port": port},
    });
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&doc)?)
    };
    if let Err(e) = write() {
        tracing::warn!("could not write heartbeat {}: {e}", path.display());
    }
}

static STARTED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
type Client = Option<(String, String)>;
static LAST: std::sync::Mutex<Option<(Client, Option<String>)>> = std::sync::Mutex::new(None);

/// The handshake can succeed after startup (Live opened later, or the
/// script was selected after the client connected). Rewrite the heartbeat
/// when the script version it carries no longer matches what Live said.
pub fn refresh_heartbeat(live: &LiveState) {
    let current = live.script.get().and_then(|i| i.script_version);
    let stale = {
        let last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        match last.as_ref() {
            Some((_, written)) => *written != current,
            None => false,
        }
    };
    if stale {
        let client = LAST
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|(c, _)| c.clone());
        write_heartbeat(live, client);
    }
}

pub fn remove_heartbeat() {
    let path = heartbeat_path();
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!("could not remove heartbeat {}: {e}", path.display());
        }
    }
}

/// Run the MCP server over stdio until the client disconnects.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        "AbletonMusicMaker server v{} starting up",
        crate::MCP_VERSION
    );

    let live = Arc::new(LiveState::new(Arc::new(RealBridge::from_env())));
    let script_info = tokio::task::spawn_blocking({
        let live = live.clone();
        move || live.script.handshake(live.bridge.as_ref())
    })
    .await?;
    if script_info.script_version.is_none() {
        tracing::warn!(
            "Could not reach the Ableton Remote Script on startup; tools will retry on use"
        );
    }
    if live.activity.enabled() {
        tracing::info!("Activity log: {}", live.activity.path().display());
    } else {
        tracing::info!("Activity log off (ABLETON_MCP_ACTIVITY=false)");
    }
    write_heartbeat(&live, None);
    crate::library::start_warm_up(live.clone());

    let server = Server::new(live.clone());
    let service = server.serve(rmcp::transport::stdio()).await?;
    let outcome = service.waiting().await;

    shutdown(&live);
    outcome?;
    Ok(())
}

fn shutdown(live: &LiveState) {
    remove_heartbeat();
    live.bridge.disconnect();
    tracing::info!("AbletonMusicMaker server shut down");
}
