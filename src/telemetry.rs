//! Anonymous usage telemetry. Opt-in and off by default.
//!
//! Two tiers, both gated:
//! - Anonymous: tool names, success/duration, versions, an install ID. Needs
//!   `ABLETON_MCP_ENABLE_TELEMETRY=true` plus Supabase credentials in the
//!   environment. Any `*DISABLE_TELEMETRY` variable wins.
//! - Rich (prompts, MIDI, names): additionally needs the dataset consent
//!   grant (see [`crate::dataset::consent`]).
//!
//! Events go through a bounded queue to a background thread; nothing here
//! ever blocks a tool call, and a full queue drops events.

use crate::dataset::supabase::SupabaseClient;
use crate::{env_flag, env_str};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const TELEMETRY_TABLE: &str = "telemetry_events";
const QUEUE_CAPACITY: usize = 1000;

/// Operator credentials, read from the environment. No credentials ship with
/// the binary or the image.
#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    pub supabase_url: String,
    pub supabase_anon_key: String,
    /// Whether events are sent at all (credentials present, opted in, not
    /// disabled). Resolved once when the collector is created.
    pub enabled: bool,
    pub timeout: Duration,
    pub max_prompt_length: usize,
}

impl TelemetryConfig {
    pub fn from_env() -> Self {
        let supabase_url = env_str("ABLETON_MCP_SUPABASE_URL");
        let supabase_anon_key = env_str("ABLETON_MCP_SUPABASE_ANON_KEY");
        let has_credentials = !supabase_url.is_empty() && !supabase_anon_key.is_empty();
        Self {
            supabase_url,
            supabase_anon_key,
            enabled: has_credentials,
            timeout: Duration::from_secs_f64(1.5),
            max_prompt_length: 1000,
        }
    }

    pub fn has_credentials(&self) -> bool {
        !self.supabase_url.is_empty() && !self.supabase_anon_key.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Startup,
    ToolExecution,
    Connection,
    Error,
}

/// What a caller records; the collector fills in identity and timing.
#[derive(Clone, Debug, Default)]
pub struct EventDraft {
    pub tool_name: Option<String>,
    pub prompt_text: Option<String>,
    pub success: bool,
    pub duration_ms: Option<f64>,
    pub error_message: Option<String>,
    pub ableton_version: Option<String>,
    pub metadata: Option<Value>,
}

impl EventDraft {
    pub fn new() -> Self {
        Self {
            success: true,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TelemetryEvent {
    pub event_type: EventType,
    pub customer_uuid: String,
    pub session_id: String,
    pub timestamp: f64,
    pub version: String,
    pub platform: String,
    pub tool_name: Option<String>,
    pub prompt_text: Option<String>,
    pub success: bool,
    pub duration_ms: Option<f64>,
    pub error_message: Option<String>,
    pub ableton_version: Option<String>,
    pub metadata: Option<Value>,
}

impl TelemetryEvent {
    /// The row inserted into `telemetry_events`.
    pub fn to_row(&self) -> Value {
        json!({
            "customer_uuid": self.customer_uuid,
            "session_id": self.session_id,
            "event_type": self.event_type,
            "tool_name": self.tool_name,
            "prompt_text": self.prompt_text,
            "success": self.success,
            "duration_ms": self.duration_ms,
            "error_message": self.error_message,
            "version": self.version,
            "platform": self.platform,
            "ableton_version": self.ableton_version,
            "metadata": self.metadata.clone().unwrap_or_else(|| json!({})),
            "event_timestamp": self.timestamp as i64,
        })
    }
}

/// Where events go. The production sink is Supabase; tests install a
/// recorder so nothing leaves the process.
pub trait TelemetrySink: Send + Sync {
    fn send(&self, config: &TelemetryConfig, event: &TelemetryEvent) -> Result<(), String>;
}

pub struct SupabaseSink;

impl TelemetrySink for SupabaseSink {
    fn send(&self, config: &TelemetryConfig, event: &TelemetryEvent) -> Result<(), String> {
        if !config.has_credentials() {
            return Ok(());
        }
        let client = SupabaseClient::new(
            &config.supabase_url,
            &config.supabase_anon_key,
            None,
            config.timeout,
        );
        client
            .insert(TELEMETRY_TABLE, &event.to_row())
            .map_err(|e| format!("{e:?}"))
    }
}

// ── Rich-tier consent ───────────────────────────────────────────────────────

static USER_CONSENT: AtomicBool = AtomicBool::new(false);

pub fn set_telemetry_consent(consent: bool) {
    USER_CONSENT.store(consent, Ordering::SeqCst);
    tracing::debug!("Telemetry consent set to: {}", consent);
}

pub fn get_telemetry_consent() -> bool {
    USER_CONSENT.load(Ordering::SeqCst)
}

/// True when the user has explicitly opted in to dataset recording, which
/// implies consent to the rich tier (dataset rows are a superset of it).
fn dataset_opt_in() -> bool {
    env_flag("ABLETON_MCP_ENABLE_DATASET") || crate::dataset::consent::recording_allowed()
}

/// Re-apply rich-tier consent after a mid-session answer.
pub fn refresh_consent_from_dataset() -> bool {
    if dataset_opt_in() {
        set_telemetry_consent(true);
    }
    get_telemetry_consent()
}

// ── Collector ───────────────────────────────────────────────────────────────

pub struct TelemetryCollector {
    pub config: TelemetryConfig,
    customer_uuid: String,
    session_id: String,
    tx: SyncSender<TelemetryEvent>,
    pending: Arc<AtomicUsize>,
}

impl TelemetryCollector {
    pub fn new(sink: Arc<dyn TelemetrySink>) -> Self {
        let mut config = TelemetryConfig::from_env();

        // Off by default: even with credentials configured nothing is sent
        // unless the user opts in, and any disable variable wins over that.
        if !Self::is_opted_in() {
            if config.enabled {
                tracing::info!(
                    "Telemetry is off by default; set ABLETON_MCP_ENABLE_TELEMETRY=true to opt in"
                );
            }
            config.enabled = false;
        }
        if Self::is_disabled() {
            config.enabled = false;
            tracing::warn!("Telemetry disabled via environment variable");
        }

        // Opting in to dataset recording implies rich-tier consent, either as
        // an env var (headless) or as a persisted answer to the prompt.
        let mut raw_consent = env_str("ABLETON_MCP_TELEMETRY_CONSENT").to_ascii_lowercase();
        if raw_consent.is_empty() && dataset_opt_in() {
            raw_consent = "true".to_string();
        }
        match raw_consent.as_str() {
            "true" | "1" | "yes" | "on" => {
                set_telemetry_consent(true);
                tracing::info!("Rich telemetry consent granted");
            }
            "false" | "0" | "no" | "off" => {
                set_telemetry_consent(false);
                tracing::info!("Rich telemetry consent declined");
            }
            _ => {}
        }

        let customer_uuid = Self::get_or_create_uuid();
        let session_id = uuid::Uuid::new_v4().to_string();

        let (tx, rx) = sync_channel::<TelemetryEvent>(QUEUE_CAPACITY);
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = pending.clone();
        let worker_config = config.clone();
        std::thread::Builder::new()
            .name("telemetry".into())
            .spawn(move || {
                for event in rx {
                    if let Err(e) = sink.send(&worker_config, &event) {
                        tracing::debug!("Failed to send telemetry: {}", e);
                    }
                    worker_pending.fetch_sub(1, Ordering::SeqCst);
                }
            })
            .expect("spawn telemetry worker");

        tracing::debug!("Telemetry initialized (enabled={})", config.enabled);
        Self {
            config,
            customer_uuid,
            session_id,
            tx,
            pending,
        }
    }

    fn is_opted_in() -> bool {
        env_flag("ABLETON_MCP_ENABLE_TELEMETRY")
    }

    fn is_disabled() -> bool {
        [
            "DISABLE_TELEMETRY",
            "ABLETON_MCP_DISABLE_TELEMETRY",
            "MCP_DISABLE_TELEMETRY",
        ]
        .iter()
        .any(|v| env_flag(v))
    }

    /// Where the install ID lives. `ABLETON_MCP_DATA_DIR` overrides the
    /// platform default (`%APPDATA%`, `~/Library/Application Support`, or
    /// `$XDG_DATA_HOME` / `~/.local/share`) plus `AbletonMusicMaker`.
    pub fn data_directory() -> PathBuf {
        let explicit = env_str("ABLETON_MCP_DATA_DIR");
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let base = match std::env::consts::OS {
            "windows" => std::env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("AppData").join("Roaming")),
            "macos" => home.join("Library").join("Application Support"),
            _ => std::env::var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".local").join("share")),
        };
        base.join("AbletonMusicMaker")
    }

    fn get_or_create_uuid() -> String {
        let dir = Self::data_directory();
        let file = dir.join("customer_uuid.txt");
        if let Ok(existing) = std::fs::read_to_string(&file) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        let fresh = uuid::Uuid::new_v4().to_string();
        let persisted = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, &fresh));
        match persisted {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
                }
            }
            Err(e) => tracing::debug!("Failed to persist UUID: {}", e),
        }
        fresh
    }

    pub fn customer_uuid(&self) -> &str {
        &self.customer_uuid
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Queue an event (non-blocking). Without rich-tier consent only the
    /// anonymous fields survive.
    pub fn record_event(&self, event_type: EventType, draft: EventDraft) {
        if !self.config.enabled {
            return;
        }
        let consent = get_telemetry_consent();
        let mut prompt_text = draft.prompt_text;
        let mut metadata = draft.metadata;
        let mut error_message = draft.error_message;
        if !consent {
            prompt_text = None;
            metadata = None;
            if error_message.is_some() {
                error_message =
                    Some("Error occurred (details withheld without consent)".to_string());
            }
        }
        if let Some(p) = prompt_text.as_mut() {
            if p.chars().count() > self.config.max_prompt_length {
                *p = truncate_chars(p, self.config.max_prompt_length) + "...";
            }
        }
        if consent {
            if let Some(e) = error_message.as_mut() {
                if e.chars().count() > 200 {
                    *e = truncate_chars(e, 200) + "...";
                }
            }
        }
        let event = TelemetryEvent {
            event_type,
            customer_uuid: self.customer_uuid.clone(),
            session_id: self.session_id.clone(),
            timestamp: now_secs(),
            version: crate::MCP_VERSION.to_string(),
            platform: platform_name(),
            tool_name: draft.tool_name,
            prompt_text,
            success: draft.success,
            duration_ms: draft.duration_ms,
            error_message,
            ableton_version: draft.ableton_version,
            metadata,
        };
        self.pending.fetch_add(1, Ordering::SeqCst);
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.pending.fetch_sub(1, Ordering::SeqCst);
                tracing::debug!("Telemetry queue full, dropping event");
            }
            Err(TrySendError::Disconnected(_)) => {
                self.pending.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Number of events queued but not yet handed to the sink.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Wait for the worker to drain the queue. True if it did in time.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.pending() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.pending() == 0
    }
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn platform_name() -> String {
    match std::env::consts::OS {
        "macos" => "darwin".to_string(),
        other => other.to_string(),
    }
}

// ── Process-wide instance ───────────────────────────────────────────────────

static COLLECTOR: Mutex<Option<Arc<TelemetryCollector>>> = Mutex::new(None);
static SINK_OVERRIDE: OnceLock<Mutex<Option<Arc<dyn TelemetrySink>>>> = OnceLock::new();

fn sink_override() -> &'static Mutex<Option<Arc<dyn TelemetrySink>>> {
    SINK_OVERRIDE.get_or_init(|| Mutex::new(None))
}

/// The global collector, created on first use from the current environment.
pub fn get_telemetry() -> Arc<TelemetryCollector> {
    let mut slot = COLLECTOR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = slot.as_ref() {
        return existing.clone();
    }
    let sink: Arc<dyn TelemetrySink> = sink_override()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| Arc::new(SupabaseSink));
    let collector = Arc::new(TelemetryCollector::new(sink));
    *slot = Some(collector.clone());
    collector
}

/// Drop the global collector so the next call re-reads the environment.
pub fn reset_for_tests() {
    *COLLECTOR.lock().unwrap_or_else(|e| e.into_inner()) = None;
    set_telemetry_consent(false);
}

/// Route events of collectors created after this call into `sink`.
pub fn set_sink_for_tests(sink: Option<Arc<dyn TelemetrySink>>) {
    *sink_override().lock().unwrap_or_else(|e| e.into_inner()) = sink;
}

pub fn record_tool_usage(tool_name: &str, success: bool, duration_ms: f64, error: Option<&str>) {
    get_telemetry().record_event(
        EventType::ToolExecution,
        EventDraft {
            tool_name: Some(tool_name.to_string()),
            success,
            duration_ms: Some(duration_ms),
            error_message: error.map(str::to_string),
            ..EventDraft::new()
        },
    );
}

pub fn record_startup(ableton_version: Option<&str>) {
    get_telemetry().record_event(
        EventType::Startup,
        EventDraft {
            ableton_version: ableton_version.map(str::to_string),
            ..EventDraft::new()
        },
    );
}

pub fn is_telemetry_enabled() -> bool {
    get_telemetry().config.enabled
}
