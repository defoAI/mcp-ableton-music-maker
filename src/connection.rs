//! The TCP bridge to the AbletonMusicMaker Remote Script.
//!
//! Wire protocol: the server writes one JSON document `{"type": ..., "params":
//! {...}}` and the script answers with one JSON document `{"status": "success",
//! "result": ...}` or `{"status": "error", "message": ...}`. There is no
//! framing, so the reader accumulates bytes until the buffer parses.

use crate::activity::Activity;
use crate::handshake::ScriptInfoCache;
use serde::Serialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Everything that can go wrong talking to Live.
#[derive(Debug, thiserror::Error, Clone)]
pub enum LiveError {
    #[error("could not connect to Ableton at {host}:{port} ({reason}). Make sure Live is running with the AbletonMusicMaker control surface selected")]
    Connect {
        host: String,
        port: u16,
        reason: String,
    },
    #[error("timed out after {secs}s waiting for Ableton to answer `{command}`")]
    Timeout { command: String, secs: f64 },
    #[error("connection to Ableton lost: {0}")]
    Lost(String),
    #[error("invalid response from Ableton: {0}")]
    InvalidResponse(String),
    /// The Remote Script reported an error; the message is Live's own text.
    #[error("{0}")]
    Ableton(String),
}

pub type LiveResult<T> = Result<T, LiveError>;

/// Anything that can execute a Remote Script command. The real implementation
/// is [`RealBridge`]; tests substitute a recorder that returns canned replies.
pub trait LiveBridge: Send + Sync {
    fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value>;

    /// Drop the current connection, if any (shutdown).
    fn disconnect(&self) {}
}

/// What one tool call did on the Live socket: every command it sent and
/// the time spent waiting for Live. Collected per blocking thread, so a
/// tool body needs no extra plumbing to be observed.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CallTrace {
    pub commands: Vec<String>,
    pub live_ms: f64,
    /// The Remote Script's clock from the last response of this call, when a
    /// performance runs (the script attaches it to every response envelope).
    #[serde(skip)]
    pub clock: Option<Value>,
}

thread_local! {
    static TRACE: RefCell<Option<CallTrace>> = const { RefCell::new(None) };
    static EXCHANGE_CLOCK: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// Called by the connection when a response envelope carries `clock`; a
/// test bridge may call it to attach one.
pub fn note_exchange_clock(clock: Option<Value>) {
    EXCHANGE_CLOCK.with(|c| *c.borrow_mut() = clock);
}

fn take_exchange_clock() -> Option<Value> {
    EXCHANGE_CLOCK.with(|c| c.borrow_mut().take())
}

/// Start observing the Live commands sent from this thread.
pub fn begin_trace() {
    TRACE.with(|t| *t.borrow_mut() = Some(CallTrace::default()));
}

/// Stop observing and return what was seen (empty if nothing was started).
pub fn end_trace() -> CallTrace {
    TRACE.with(|t| t.borrow_mut().take().unwrap_or_default())
}

fn note_command(command: &str, elapsed: Duration, clock: Option<&Value>) {
    TRACE.with(|t| {
        if let Some(trace) = t.borrow_mut().as_mut() {
            trace.commands.push(command.to_string());
            trace.live_ms += elapsed.as_secs_f64() * 1000.0;
            if let Some(c) = clock {
                trace.clock = Some(c.clone());
            }
        }
    });
}

/// `ABLETON_HOST` (default `localhost`) and `ABLETON_PORT` (default 9877).
pub fn live_address() -> (String, u16) {
    let host = std::env::var("ABLETON_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("ABLETON_PORT")
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(9877);
    (host, port)
}

/// A running performance: set by `start_performance`, cleared by
/// `end_performance`. While it is set, the transport-touching tools refuse.
/// In memory only; it vanishes with the process.
#[derive(Debug, Clone, Serialize)]
pub struct Performance {
    pub started_at: chrono::DateTime<chrono::Local>,
    pub start_bar: i64,
    pub start_beat: f64,
    pub key: Option<String>,
    pub quantization: String,
    pub cues_scheduled: u32,
    pub cues_cancelled: u32,
    /// Re-key the assistant's clips after every recording (`follow_key: true`)
    #[serde(default)]
    pub follow_key: bool,
    /// The song cursor while a setlist is being played or steered
    #[serde(default)]
    pub song: Option<crate::song::Song>,
}

/// Everything a tool needs to talk to Live: the bridge, the cached
/// handshake result, the local activity log, the running performance, the
/// library index, and what the last exchange told us about time.
pub struct LiveState {
    pub bridge: Arc<dyn LiveBridge>,
    pub script: ScriptInfoCache,
    pub activity: Activity,
    pub performance: Mutex<Option<Performance>>,
    pub library: crate::library::Library,
    /// The last 20 round trips in seconds, for latency compensation.
    pub round_trips: Mutex<std::collections::VecDeque<f64>>,
    /// The last clock the Remote Script attached, and when it arrived.
    pub last_clock: Mutex<Option<(Instant, Value)>>,
    /// One level of undo for vary_clip: (track, slot) → the notes before.
    pub vary_undo: Mutex<std::collections::HashMap<(i64, i64), Vec<Value>>>,
}

impl LiveState {
    /// Activity log configured from the environment (the server's normal path).
    pub fn new(bridge: Arc<dyn LiveBridge>) -> Self {
        Self::with_activity(bridge, Activity::from_env())
    }

    pub fn with_activity(bridge: Arc<dyn LiveBridge>, activity: Activity) -> Self {
        Self {
            bridge,
            script: ScriptInfoCache::default(),
            activity,
            performance: Mutex::new(None),
            library: crate::library::Library::default(),
            round_trips: Mutex::new(std::collections::VecDeque::new()),
            last_clock: Mutex::new(None),
            vary_undo: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        let start = Instant::now();
        note_exchange_clock(None);
        let result = self.bridge.send_command(command_type, params);
        let elapsed = start.elapsed();
        let clock = take_exchange_clock();
        note_command(command_type, elapsed, clock.as_ref());
        if result.is_ok() {
            let mut rt = self.round_trips.lock().unwrap_or_else(|e| e.into_inner());
            rt.push_back(elapsed.as_secs_f64());
            while rt.len() > 20 {
                rt.pop_front();
            }
        }
        if let Some(c) = clock {
            *self.last_clock.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), c));
        }
        result
    }

    /// Trailing-average round trip to Live in seconds (0.2 s before any call).
    pub fn round_trip_s(&self) -> f64 {
        let rt = self.round_trips.lock().unwrap_or_else(|e| e.into_inner());
        if rt.is_empty() {
            0.2
        } else {
            rt.iter().sum::<f64>() / rt.len() as f64
        }
    }

    /// The last clock the script sent, with how many seconds ago it arrived.
    pub fn last_clock(&self) -> Option<(f64, Value)> {
        self.last_clock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(at, v)| (at.elapsed().as_secs_f64(), v.clone()))
    }

    /// Tests: pretend the script just sent this clock.
    pub fn set_last_clock(&self, clock: Value) {
        *self.last_clock.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), clock));
    }
}

/// Commands that change the Live set; they get the wider socket timeout.
const MODIFYING_COMMANDS: &[&str] = &[
    "create_midi_track",
    "create_audio_track",
    "set_track_name",
    "create_clip",
    "create_audio_clip",
    "add_notes_to_clip",
    "set_clip_name",
    "set_arrangement_clip_name",
    "delete_clip",
    "clear_notes_from_clip",
    "set_tempo",
    "fire_clip",
    "stop_clip",
    "set_device_parameter",
    "start_playback",
    "stop_playback",
    "load_browser_item",
    "switch_to_arrangement_view",
    "set_current_song_time",
    "duplicate_session_clip_to_arrangement",
    "create_locator",
    "set_track_mixer",
    "set_send",
    "set_track_color",
    "set_clip_color",
    "delete_arrangement_clip",
    "delete_locator",
    "set_clip_loop",
    "set_clip_launch",
    "set_clip_automation",
    "ensure_capture_track",
    "start_capture",
    "stop_capture",
    "play_from",
    "delete_track",
    "back_to_arrangement",
    "set_arrangement_loop",
    "set_launch_quantization",
    "create_scene",
    "fire_scene",
    "stop_all_clips",
    "set_crossfader",
    "record_clip",
    "schedule_cue",
    "cancel_cue",
    "set_scale",
    "set_slot_stop_buttons",
    "set_performance_mode",
    "set_scene",
    "start_live_capture",
    "snapshot_mix",
    "restore_mix",
    "capture_scene",
    "duplicate_scene",
    "set_clip_groove",
];
// get_context and get_browser_index are reads: the default budget applies.

/// Socket budget per command. Importing a large audio file can keep Live's
/// main thread busy far longer than any other command.
pub fn command_timeout(command_type: &str) -> Duration {
    match command_type {
        "create_audio_clip" => Duration::from_secs(65),
        // Walks Live's browser; the script stops itself after its own budget.
        "search_browser" => Duration::from_secs(25),
        c if MODIFYING_COMMANDS.contains(&c) => Duration::from_secs(15),
        _ => Duration::from_secs(10),
    }
}

/// One socket to the Remote Script. The request and its response are one
/// indivisible exchange, so the whole round-trip (including reconnect) runs
/// under the mutex: tool calls and the passive poller share this socket.
pub struct AbletonConnection {
    pub host: String,
    pub port: u16,
    sock: Mutex<Option<TcpStream>>,
}

impl AbletonConnection {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            sock: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<TcpStream>> {
        self.sock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Connect to the Remote Script socket server (no-op if connected).
    pub fn connect(&self) -> LiveResult<()> {
        let mut guard = self.lock();
        Self::connect_locked(&mut guard, &self.host, self.port)
    }

    fn connect_locked(guard: &mut Option<TcpStream>, host: &str, port: u16) -> LiveResult<()> {
        if guard.is_some() {
            return Ok(());
        }
        let connect_err = |reason: String| LiveError::Connect {
            host: host.to_string(),
            port,
            reason,
        };
        let addrs: Vec<_> = (host, port)
            .to_socket_addrs()
            .map_err(|e| connect_err(e.to_string()))?
            .collect();
        let mut last = "no address resolved".to_string();
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    tracing::info!("Connected to Ableton at {}:{}", host, port);
                    *guard = Some(stream);
                    return Ok(());
                }
                Err(e) => last = e.to_string(),
            }
        }
        tracing::error!(
            "Failed to connect to Ableton at {}:{}: {}",
            host,
            port,
            last
        );
        Err(connect_err(last))
    }

    pub fn is_connected(&self) -> bool {
        self.lock().is_some()
    }

    pub fn disconnect(&self) {
        let mut guard = self.lock();
        if let Some(stream) = guard.take() {
            if let Err(e) = stream.shutdown(std::net::Shutdown::Both) {
                if e.kind() != ErrorKind::NotConnected {
                    tracing::error!("Error disconnecting from Ableton: {}", e);
                }
            }
        }
    }

    /// Liveness probe before reusing a connection: a non-blocking peek.
    /// `Ok(0)` means the remote end closed; `WouldBlock` means alive.
    pub fn is_alive(&self) -> Result<bool, String> {
        let guard = self.lock();
        let Some(stream) = guard.as_ref() else {
            return Ok(false);
        };
        stream.set_nonblocking(true).map_err(|e| e.to_string())?;
        let mut byte = [0u8; 1];
        let outcome = match stream.peek(&mut byte) {
            Ok(0) => Err("remote end closed".to_string()),
            Ok(_) => Ok(true),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(true),
            Err(e) => Err(e.to_string()),
        };
        let _ = stream.set_nonblocking(false);
        outcome
    }

    /// Read until the accumulated bytes parse as one JSON document.
    fn receive_document(
        stream: &mut TcpStream,
        command: &str,
        timeout: Duration,
    ) -> LiveResult<Value> {
        let mut data: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 8192];
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| LiveError::Lost(e.to_string()))?;
        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    return Err(LiveError::Lost(if data.is_empty() {
                        "connection closed before any data arrived".to_string()
                    } else {
                        format!(
                            "connection closed after {} bytes of an incomplete response",
                            data.len()
                        )
                    }));
                }
                Ok(n) => {
                    data.extend_from_slice(&buf[..n]);
                    if let Ok(doc) = serde_json::from_slice::<Value>(&data) {
                        tracing::debug!("Received complete response ({} bytes)", data.len());
                        return Ok(doc);
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Err(LiveError::Timeout {
                        command: command.to_string(),
                        secs: timeout.as_secs_f64(),
                    });
                }
                Err(e) => return Err(LiveError::Lost(e.to_string())),
            }
        }
    }

    /// Send a command to Ableton and return its `result` payload.
    pub fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        let mut guard = self.lock();
        Self::connect_locked(&mut guard, &self.host, self.port)?;
        let outcome = Self::exchange(guard.as_mut().expect("connected"), command_type, params);
        if outcome.is_err() {
            // Any failure poisons the socket: the next call reconnects.
            *guard = None;
        }
        outcome
    }

    fn exchange(
        stream: &mut TcpStream,
        command_type: &str,
        params: Option<Value>,
    ) -> LiveResult<Value> {
        let params = params.unwrap_or_else(|| json!({}));
        let command = json!({"type": command_type, "params": params});
        tracing::debug!("Sending command: {}", command);

        let payload =
            serde_json::to_vec(&command).map_err(|e| LiveError::InvalidResponse(e.to_string()))?;
        let timeout = command_timeout(command_type);
        stream.set_write_timeout(Some(timeout)).ok();
        stream.write_all(&payload).map_err(|e| match e.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => LiveError::Timeout {
                command: command_type.to_string(),
                secs: timeout.as_secs_f64(),
            },
            _ => LiveError::Lost(e.to_string()),
        })?;

        let response = Self::receive_document(stream, command_type, timeout)?;
        let status = response
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if status == "error" {
            let message = match response.get("message") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "unknown error from Ableton".to_string(),
            };
            tracing::error!("Ableton error for {}: {}", command_type, message);
            note_exchange_clock(response.get("clock").cloned());
            return Err(LiveError::Ableton(message));
        }
        note_exchange_clock(response.get("clock").cloned());
        Ok(response.get("result").cloned().unwrap_or_else(|| json!({})))
    }
}

/// A bare connection is also a bridge: one attempt, no retry loop. `--check`
/// and the Mac app use it so an absent Live answers in milliseconds.
impl LiveBridge for AbletonConnection {
    fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        AbletonConnection::send_command(self, command_type, params)
    }

    fn disconnect(&self) {
        AbletonConnection::disconnect(self)
    }
}

/// The production bridge: a lazily created, health-checked, reconnecting
/// [`AbletonConnection`].
pub struct RealBridge {
    host: String,
    port: u16,
    connection: Mutex<Option<Arc<AbletonConnection>>>,
}

impl RealBridge {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            connection: Mutex::new(None),
        }
    }

    /// `ABLETON_HOST` (default `localhost`) and `ABLETON_PORT` (default 9877).
    pub fn from_env() -> Self {
        let (host, port) = live_address();
        Self::new(host, port)
    }

    /// Get or create a persistent Ableton connection.
    pub fn get_connection(&self) -> LiveResult<Arc<AbletonConnection>> {
        let mut slot = self.connection.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(existing) = slot.as_ref() {
            if existing.is_connected() {
                match existing.is_alive() {
                    Ok(true) => return Ok(existing.clone()),
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!("Existing connection is no longer valid: {}", e);
                        existing.disconnect();
                        *slot = None;
                    }
                }
            }
        }

        if let Some(existing) = slot.as_ref() {
            return Ok(existing.clone());
        }

        let max_attempts = 3;
        let mut last_err = None;
        for attempt in 1..=max_attempts {
            tracing::info!(
                "Connecting to Ableton at {}:{} (attempt {}/{})...",
                self.host,
                self.port,
                attempt,
                max_attempts
            );
            let conn = Arc::new(AbletonConnection::new(self.host.clone(), self.port));
            match conn.connect() {
                Ok(()) => {
                    tracing::info!("Created new persistent connection to Ableton");
                    *slot = Some(conn.clone());
                    return Ok(conn);
                }
                Err(e) => last_err = Some(e),
            }
            if attempt < max_attempts {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        tracing::error!(
            "Failed to connect to Ableton after {} attempts",
            max_attempts
        );
        Err(last_err.expect("at least one attempt"))
    }
}

impl LiveBridge for RealBridge {
    fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        let conn = self.get_connection()?;
        conn.send_command(command_type, params)
    }

    fn disconnect(&self) {
        let mut slot = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = slot.take() {
            tracing::info!("Disconnecting from Ableton on shutdown");
            conn.disconnect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A one-shot fake Remote Script: accepts one connection, reads one
    /// command, answers with `reply` in two chunks.
    fn fake_live(reply: &'static str) -> (u16, std::thread::JoinHandle<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let n = stream.read(&mut buf).unwrap();
            let received: Value = serde_json::from_slice(&buf[..n]).unwrap();
            let (a, b) = reply.split_at(reply.len() / 2);
            stream.write_all(a.as_bytes()).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            stream.write_all(b.as_bytes()).unwrap();
            received
        });
        (port, handle)
    }

    #[test]
    fn round_trip_returns_result_payload() {
        let (port, live) =
            fake_live(r#"{"status": "success", "result": {"tempo": 120.0, "tracks": []}}"#);
        let conn = AbletonConnection::new("127.0.0.1", port);
        let result = conn.send_command("get_session_info", None).unwrap();
        assert_eq!(result, json!({"tempo": 120.0, "tracks": []}));
        assert_eq!(
            live.join().unwrap(),
            json!({"type": "get_session_info", "params": {}})
        );
    }

    #[test]
    fn ableton_error_is_surfaced_and_drops_socket() {
        let (port, _live) =
            fake_live(r#"{"status": "error", "message": "Browser is not available"}"#);
        let conn = AbletonConnection::new("127.0.0.1", port);
        let err = conn
            .send_command("get_browser_tree", Some(json!({"category_type": "all"})))
            .unwrap_err();
        assert!(matches!(err, LiveError::Ableton(ref m) if m == "Browser is not available"));
        assert!(!conn.is_connected());
    }

    #[test]
    fn refused_connection_is_a_connect_error() {
        let bridge = RealBridge::new("127.0.0.1", 1);
        let err = bridge.send_command("get_session_info", None).unwrap_err();
        assert!(matches!(err, LiveError::Connect { port: 1, .. }), "{err}");
        assert!(err.to_string().contains("Make sure Live is running"));
    }

    #[test]
    fn timeouts_by_command() {
        assert_eq!(
            command_timeout("create_audio_clip"),
            Duration::from_secs(65)
        );
        assert_eq!(command_timeout("set_tempo"), Duration::from_secs(15));
        assert_eq!(command_timeout("get_session_info"), Duration::from_secs(10));
    }
}
