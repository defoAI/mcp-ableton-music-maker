//! Privacy defaults: telemetry and dataset recording are opt-in.
//!
//! Credentials are placed in the environment so the tests prove that having
//! them is not enough: nothing is sent, and no Supabase request is built,
//! unless the user explicitly opts in. The network sink is replaced, so no
//! socket is opened.

use mcp_ableton_music_maker::dataset::consent;
use mcp_ableton_music_maker::dataset::recorder::{self, dataset_enabled, RowSink};
use mcp_ableton_music_maker::dataset::supabase::RowError;
use mcp_ableton_music_maker::telemetry::{
    self, EventDraft, EventType, TelemetryConfig, TelemetryEvent, TelemetrySink,
};
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const ENV_VARS: &[&str] = &[
    "ABLETON_MCP_ENABLE_TELEMETRY",
    "ABLETON_MCP_DISABLE_TELEMETRY",
    "DISABLE_TELEMETRY",
    "MCP_DISABLE_TELEMETRY",
    "ABLETON_MCP_ENABLE_DATASET",
    "ABLETON_MCP_DISABLE_DATASET",
    "ABLETON_MCP_TELEMETRY_CONSENT",
];

/// Process-wide state (env vars, singletons) means these run one at a time.
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct RecordingSink(Mutex<Vec<TelemetryEvent>>);
impl TelemetrySink for RecordingSink {
    fn send(&self, _: &TelemetryConfig, event: &TelemetryEvent) -> Result<(), String> {
        self.0.lock().unwrap().push(event.clone());
        Ok(())
    }
}

#[derive(Default)]
struct RecordingRows(Mutex<Vec<(String, Value)>>);
impl RowSink for RecordingRows {
    fn write(&self, table: &str, row: &Value) -> Result<(), RowError> {
        self.0
            .lock()
            .unwrap()
            .push((table.to_string(), row.clone()));
        Ok(())
    }
}

struct Env {
    _guard: MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    sink: Arc<RecordingSink>,
    rows: Arc<RecordingRows>,
}

/// Clean env, fake credentials, isolated state and data dirs, sinks stubbed.
fn privacy_env() -> Env {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for var in ENV_VARS {
        std::env::remove_var(var);
    }
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path().join("state"));
    std::env::set_var("ABLETON_MCP_DATA_DIR", dir.path().join("data"));
    std::env::set_var("ABLETON_MCP_SUPABASE_URL", "https://example.invalid");
    std::env::set_var("ABLETON_MCP_SUPABASE_ANON_KEY", "anon-key");

    let sink = Arc::new(RecordingSink::default());
    let rows = Arc::new(RecordingRows::default());
    telemetry::set_sink_for_tests(Some(sink.clone()));
    recorder::set_row_sink_for_tests(Some(rows.clone()));
    telemetry::reset_for_tests();
    recorder::reset_recorder_for_tests();
    consent::reset_for_tests();
    Env {
        _guard: guard,
        _dir: dir,
        sink,
        rows,
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        telemetry::reset_for_tests();
        recorder::reset_recorder_for_tests();
        consent::reset_for_tests();
        telemetry::set_sink_for_tests(None);
        recorder::set_row_sink_for_tests(None);
        for var in ENV_VARS {
            std::env::remove_var(var);
        }
    }
}

fn record_two_events() -> Arc<telemetry::TelemetryCollector> {
    let collector = telemetry::get_telemetry();
    collector.record_event(EventType::Startup, EventDraft::new());
    collector.record_event(
        EventType::ToolExecution,
        EventDraft {
            tool_name: Some("get_session_info".into()),
            duration_ms: Some(1.0),
            ..EventDraft::new()
        },
    );
    assert!(
        collector.flush(Duration::from_secs(5)),
        "worker did not drain the queue"
    );
    collector
}

// ── Anonymous telemetry tier ────────────────────────────────────────────────

#[test]
fn telemetry_off_by_default_even_with_credentials() {
    let env = privacy_env();
    assert!(
        TelemetryConfig::from_env().has_credentials(),
        "the test must have credentials to be meaningful"
    );
    let collector = record_two_events();
    assert!(!collector.config.enabled);
    assert!(!telemetry::is_telemetry_enabled());
    assert!(
        env.sink.0.lock().unwrap().is_empty(),
        "nothing may be sent without opt-in"
    );
}

#[test]
fn telemetry_opt_in_sends() {
    let env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
    let collector = record_two_events();
    assert!(collector.config.enabled);
    let sent = env.sink.0.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].event_type, EventType::Startup);
    assert_eq!(sent[1].tool_name.as_deref(), Some("get_session_info"));
    assert_eq!(sent[0].customer_uuid.len(), 36);
    assert_eq!(collector.config.supabase_url, "https://example.invalid");
}

#[test]
fn disable_wins_over_enable() {
    for disable_var in [
        "ABLETON_MCP_DISABLE_TELEMETRY",
        "DISABLE_TELEMETRY",
        "MCP_DISABLE_TELEMETRY",
    ] {
        let env = privacy_env();
        std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
        std::env::set_var(disable_var, "1");
        let collector = record_two_events();
        assert!(!collector.config.enabled, "{disable_var} must win");
        assert!(env.sink.0.lock().unwrap().is_empty());
    }
}

#[test]
fn enable_value_must_be_truthy() {
    let _env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "false");
    assert!(!telemetry::get_telemetry().config.enabled);
}

#[test]
fn install_id_is_persisted_in_the_data_dir() {
    let _env = privacy_env();
    let first = telemetry::get_telemetry().customer_uuid().to_string();
    telemetry::reset_for_tests();
    let second = telemetry::get_telemetry().customer_uuid().to_string();
    assert_eq!(first, second);
    let file = telemetry::TelemetryCollector::data_directory().join("customer_uuid.txt");
    assert_eq!(std::fs::read_to_string(file).unwrap().trim(), first);
}

// ── Dataset tier ────────────────────────────────────────────────────────────

#[test]
fn dataset_off_by_default_and_never_prompts() {
    let _env = privacy_env();
    telemetry::get_telemetry();
    assert_eq!(consent::consent_state(), consent::UNKNOWN);
    assert!(!consent::recording_allowed());
    assert!(!dataset_enabled());
    assert!(recorder::get_recorder().is_none());
    // With telemetry off a yes could not start recording, so do not ask.
    assert!(!consent::needs_prompt());
    assert_eq!(consent::maybe_consent_notice(), "");
}

#[test]
fn unanswered_question_does_not_record() {
    let _env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
    telemetry::get_telemetry();
    // Telemetry is on and the user has never answered: ask, but do not record.
    assert!(consent::needs_prompt());
    assert!(!consent::recording_allowed());
    assert!(!dataset_enabled());
    assert!(!telemetry::get_telemetry_consent());
    assert!(consent::maybe_consent_notice().contains("Would you like to contribute"));
    assert_eq!(consent::maybe_consent_notice(), "", "asked once per hour");
}

#[test]
fn explicit_yes_enables_dataset_and_rows_flow_to_the_sink() {
    let env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
    telemetry::get_telemetry();
    assert_eq!(
        consent::record_consent(true, Some("yes please")),
        consent::GRANTED
    );
    telemetry::refresh_consent_from_dataset();

    assert!(consent::recording_allowed());
    assert!(!consent::needs_prompt());
    assert!(dataset_enabled());
    let rec = recorder::get_recorder().expect("recorder starts after consent");
    assert!(rec.flush(Duration::from_secs(5)));
    let rows = env.rows.0.lock().unwrap();
    assert_eq!(rows[0].0, recorder::TABLE_SESSIONS);
    assert_eq!(rows[1].1["event_type"], "session_start");

    // The stored answer keeps the evidence of what the user said.
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(consent::state_dir().join("consent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["state"], "granted");
    assert_eq!(state["user_said"], "yes please");
}

#[test]
fn explicit_yes_without_telemetry_still_records_nothing() {
    let env = privacy_env();
    telemetry::get_telemetry();
    consent::record_consent(true, Some("yes"));
    telemetry::refresh_consent_from_dataset();
    // Rich-tier consent alone is not enough: the pipe itself is off.
    assert!(!dataset_enabled());
    assert!(recorder::get_recorder().is_none());
    assert!(env.rows.0.lock().unwrap().is_empty());
}

#[test]
fn env_enable_dataset_counts_as_grant() {
    let _env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
    std::env::set_var("ABLETON_MCP_ENABLE_DATASET", "1");
    telemetry::get_telemetry();
    assert_eq!(consent::consent_state(), consent::GRANTED);
    assert!(dataset_enabled());
}

#[test]
fn disable_dataset_overrides_stored_yes_and_env_enable() {
    let env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
    consent::record_consent(true, Some("yes"));
    std::env::set_var("ABLETON_MCP_ENABLE_DATASET", "1");
    std::env::set_var("ABLETON_MCP_DISABLE_DATASET", "true");
    telemetry::get_telemetry();

    assert_eq!(consent::consent_state(), consent::DENIED);
    assert!(!consent::recording_allowed());
    assert!(!dataset_enabled());
    assert!(!consent::needs_prompt());
    assert!(env.rows.0.lock().unwrap().is_empty());
}

#[test]
fn stored_no_is_respected() {
    let _env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
    consent::record_consent(false, Some("no thanks"));
    telemetry::get_telemetry();
    assert_eq!(consent::consent_state(), consent::DENIED);
    assert!(!dataset_enabled());
    assert!(!consent::needs_prompt());
}

#[test]
fn without_consent_only_anonymous_fields_are_sent() {
    let env = privacy_env();
    std::env::set_var("ABLETON_MCP_ENABLE_TELEMETRY", "true");
    let collector = telemetry::get_telemetry();
    collector.record_event(
        EventType::ToolExecution,
        EventDraft {
            tool_name: Some("set_track_name".into()),
            prompt_text: Some("name it after my client".into()),
            error_message: Some("failed: /Users/nick/secret.als".into()),
            metadata: Some(serde_json::json!({"params": {"name": "Client X"}})),
            ..EventDraft::new()
        },
    );
    assert!(collector.flush(Duration::from_secs(5)));
    let sent = env.sink.0.lock().unwrap();
    assert_eq!(sent[0].prompt_text, None);
    assert_eq!(sent[0].metadata, None);
    assert_eq!(
        sent[0].error_message.as_deref(),
        Some("Error occurred (details withheld without consent)")
    );
    assert_eq!(sent[0].tool_name.as_deref(), Some("set_track_name"));
}
