//! Process lifecycle for the `ableton-music-maker` binary: startup handshake,
//! dataset wiring, stdio serving, and orderly shutdown.

use crate::connection::{LiveState, RealBridge};
use crate::dataset::passive_poller::{start_passive_poller, stop_passive_poller};
use crate::dataset::recorder::{dataset_enabled, get_recorder};
use crate::telemetry::{get_telemetry_consent, is_telemetry_enabled, record_startup};
use crate::tools::Server;
use rmcp::ServiceExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// A JSON report of every privacy gate, for operators and CI.
pub fn privacy_status() -> Value {
    let config = crate::telemetry::TelemetryConfig::from_env();
    json!({
        "version": crate::MCP_VERSION,
        "telemetry_enabled": is_telemetry_enabled(),
        "telemetry_opt_in_variable": crate::env_flag("ABLETON_MCP_ENABLE_TELEMETRY"),
        "has_supabase_credentials": config.has_credentials(),
        "dataset_enabled": dataset_enabled(),
        "consent_state": crate::dataset::consent::consent_state(),
        "would_prompt_for_consent": crate::dataset::consent::needs_prompt(),
        "state_dir": crate::dataset::consent::state_dir(),
        "data_dir": crate::telemetry::TelemetryCollector::data_directory(),
        "expected_remote_script_version": crate::handshake::expected_remote_script_version(),
    })
}

/// Run the MCP server over stdio until the client disconnects.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        "AbletonMusicMaker server v{} starting up",
        crate::MCP_VERSION
    );
    record_startup(None);

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

    if dataset_enabled() {
        if let Some(recorder) = get_recorder() {
            tracing::info!(
                "Dataset recording enabled → Supabase session {}",
                recorder.session_id
            );
        }
        start_passive_poller(live.clone());
        if !live.script.has_capability("drain_passive_events") {
            tracing::warn!("Passive Live listeners unavailable — Remote Script outdated or not loaded. Restart Ableton after updating the script.");
        }
    } else {
        let reason = if !is_telemetry_enabled() {
            "telemetry off (default; needs ABLETON_MCP_ENABLE_TELEMETRY=true and no DISABLE_TELEMETRY variable)"
        } else if !get_telemetry_consent() {
            "no dataset consent (default; say yes or set ABLETON_MCP_ENABLE_DATASET=1)"
        } else {
            "ABLETON_MCP_DISABLE_DATASET set"
        };
        tracing::info!("Dataset recording off — {}", reason);
    }

    let server = Server::new(live.clone());
    let service = server.serve(rmcp::transport::stdio()).await?;
    let outcome = service.waiting().await;

    shutdown(&live);
    outcome?;
    Ok(())
}

fn shutdown(live: &LiveState) {
    stop_passive_poller();
    live.bridge.disconnect();
    if let Some(recorder) = get_recorder() {
        // Bounded wait so queued rows land before exit.
        let timeout = crate::env_f64("ABLETON_MCP_DATASET_FLUSH_SEC", 5.0);
        recorder.end(Duration::from_secs_f64(timeout));
    }
    tracing::info!("AbletonMusicMaker server shut down");
}
