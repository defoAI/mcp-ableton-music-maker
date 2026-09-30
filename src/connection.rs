//! The TCP bridge to the AbletonMusicMaker Remote Script.
//!
//! Wire protocol: the server writes one JSON document `{"id": n, "type": ...,
//! "params": {...}}` and the script answers with one JSON document carrying
//! the same `id`: `{"status": "success", "result": ...}` or `{"status":
//! "error", "message": ...}`. Documents are newline-delimited or simply
//! concatenated; the reader pulls complete documents off the front of its
//! buffer. A script from before protocol 2 answers without an `id`, and since
//! one request is in flight per socket that answer is unambiguous. Documents
//! with an `event` field are pushed by the script, never answers.

use crate::activity::Activity;
use crate::handshake::ScriptInfoCache;
use serde::Serialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
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

    /// Counts up every time a new socket to Live is opened. Anything the
    /// server remembers about the set was learned on one connection; a new
    /// one means Live may have been restarted, or another set opened, and
    /// what was remembered is dropped rather than trusted.
    fn connection_generation(&self) -> u64 {
        0
    }
}

/// What one tool call did on the Live socket: every command it sent and
/// the time spent waiting for Live. Collected per blocking thread, so a
/// tool body needs no extra plumbing to be observed.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CallTrace {
    pub commands: Vec<String>,
    pub live_ms: f64,
    /// Time the script's tasks held Live's main thread, summed over the
    /// commands' slices (`main_ms` in each reply; 0 for an older script).
    pub main_ms: f64,
    /// How many main-thread slices those commands took in total.
    pub slices: u64,
    /// The Remote Script's clock from the last response of this call, when a
    /// performance runs (the script attaches it to every response envelope).
    #[serde(skip)]
    pub clock: Option<Value>,
}

thread_local! {
    static TRACE: RefCell<Option<CallTrace>> = const { RefCell::new(None) };
    static EXCHANGE_CLOCK: RefCell<Option<Value>> = const { RefCell::new(None) };
    static EXCHANGE_COST: RefCell<(f64, u64)> = const { RefCell::new((0.0, 0)) };
}

/// Called by the connection with the reply's `main_ms` and `slices`.
pub fn note_exchange_cost(main_ms: f64, slices: u64) {
    EXCHANGE_COST.with(|c| *c.borrow_mut() = (main_ms, slices));
}

fn take_exchange_cost() -> (f64, u64) {
    EXCHANGE_COST.with(|c| std::mem::replace(&mut *c.borrow_mut(), (0.0, 0)))
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

fn note_command_cost(
    command: &str,
    elapsed: Duration,
    clock: Option<&Value>,
    main_ms: f64,
    slices: u64,
) {
    TRACE.with(|t| {
        if let Some(trace) = t.borrow_mut().as_mut() {
            trace.commands.push(command.to_string());
            trace.live_ms += elapsed.as_secs_f64() * 1000.0;
            trace.main_ms += main_ms;
            trace.slices += slices;
            if let Some(c) = clock {
                trace.clock = Some(c.clone());
            }
        }
    });
}

/// `main_ms` and `slices` from a reply envelope (a script before 1.23.0
/// sends neither).
fn cost_of(response: &Value) -> (f64, u64) {
    (
        response
            .get("main_ms")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        response.get("slices").and_then(Value::as_u64).unwrap_or(0),
    )
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
    /// The take this performance is recording into the Arrangement, if any
    #[serde(default)]
    pub take: Option<Take>,
}

/// The Arrangement take a performance is being recorded into. The server
/// holds where it starts so `end_performance` can say what it covered; Live
/// holds the recording itself.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Take {
    pub start_bar: i64,
    pub start_beat: f64,
    /// Bars deleted to make room, when the producer answered `replace`
    pub replaced_bars: i64,
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
    /// What a device on this Live answered to, across songs and sessions.
    pub devices: crate::devices::Devices,
    /// This song's own memory: the overview, the notes and the digest.
    pub songs: crate::memory::Songs,
    /// Which Live this is, learned from whatever reply carries it first.
    /// Every device fact is stamped with it, so an unknown one is never
    /// written to disk.
    pub live_version: Mutex<Option<String>>,
    /// What the server has learned about this Live's object model.
    pub lom: crate::lom::Lom,
    /// The notes of the Session clips as last read, so a revision does not
    /// read the whole set again to change one thing.
    pub sets: crate::sets::SetCache,
    /// The last plan a dry run left behind, for `update_song {commit: …}`.
    /// In memory, like everything else here: a plan is cheap to make again.
    pub plan: Mutex<Option<crate::sections::StoredPlan>>,
    pub samples: crate::samples::Samples,
    /// The last 20 round trips in seconds, for latency compensation.
    pub round_trips: Mutex<std::collections::VecDeque<f64>>,
    /// The last clock the Remote Script attached, and when it arrived.
    pub last_clock: Mutex<Option<(Instant, Value)>>,
    /// One level of undo for vary_clip: (track, slot) → the notes before.
    pub vary_undo: Mutex<std::collections::HashMap<(i64, i64), Vec<Value>>>,
    /// What the producer last answered about recording into an Arrangement
    /// that already had something in it, so the next performance of this
    /// session does not ask again. `replace` is never kept here.
    pub record_answer: Mutex<Option<String>>,
    /// Live's own meter curve, read once per session (`get_meter_scale`).
    pub meter_scale: Mutex<Option<crate::song::MeterScale>>,
    /// Lines a guard added on the way in — a stale performance it ended, say —
    /// for the reply of whichever tool was being called.
    pub notes: Mutex<Vec<String>>,
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
            devices: crate::devices::Devices::default(),
            songs: crate::memory::Songs::default(),
            live_version: Mutex::new(None),
            lom: crate::lom::Lom::default(),
            sets: crate::sets::SetCache::default(),
            plan: Mutex::new(None),
            samples: crate::samples::Samples::default(),
            round_trips: Mutex::new(std::collections::VecDeque::new()),
            last_clock: Mutex::new(None),
            vary_undo: Mutex::new(std::collections::HashMap::new()),
            record_answer: Mutex::new(None),
            meter_scale: Mutex::new(None),
            notes: Mutex::new(Vec::new()),
        }
    }

    /// Which Live this is — the key every learned device fact is stamped
    /// with. `"unknown"` until a reply carries it, and nothing is written to
    /// disk under that key.
    pub fn live_version(&self) -> String {
        if let Some(v) = self
            .live_version
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return v;
        }
        // The handshake carries it as `live.version`, so the common path
        // costs nothing extra.
        self.script
            .get()
            .and_then(|i| {
                i.extra
                    .get("live")
                    .and_then(|l| l.get("version"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| crate::devices::UNKNOWN_VERSION.to_string())
    }

    /// Remember the Live version a reply carried (`get_context`, `describe`).
    pub fn note_live_version(&self, version: &str) {
        let version = version.trim();
        if version.is_empty() || version == crate::devices::UNKNOWN_VERSION {
            return;
        }
        *self.live_version.lock().unwrap_or_else(|e| e.into_inner()) = Some(version.to_string());
    }

    pub fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        let start = Instant::now();
        note_exchange_clock(None);
        let result = self.bridge.send_command(command_type, params);
        let elapsed = start.elapsed();
        let clock = take_exchange_clock();
        let (main_ms, slices) = take_exchange_cost();
        note_command_cost(command_type, elapsed, clock.as_ref(), main_ms, slices);
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
/// `reload_body` changes no set but is here for the same reason: it reads and
/// compiles a file off Live's thread and then waits for a tick, and a reload
/// during a heavy session must not race a 10 s read timeout.
const MODIFYING_COMMANDS: &[&str] = &[
    "reload_body",
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
    "delete_device",
    "move_device",
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
    "set_device_parameters",
    "create_tracks",
    "write_clips",
    "place_sample",
];
// get_context, get_browser_index and list_sample_folders are reads: the
// default budget applies, except where named below.

/// Socket budget per command. Importing a large audio file can keep Live's
/// main thread busy far longer than any other command.
pub fn command_timeout(command_type: &str) -> Duration {
    match command_type {
        "create_audio_clip" => Duration::from_secs(65),
        // Creates the clip from the file and fits it, in one undo step.
        "place_sample" => Duration::from_secs(65),
        // Many tracks with instruments loaded from the browser, one round trip.
        "create_tracks" => Duration::from_secs(190),
        // Many clips with their notes, one round trip.
        "write_clips" => Duration::from_secs(65),
        // Walks Live's browser; the script stops itself after its own budget.
        "search_browser" => Duration::from_secs(25),
        // A batch may wait for Live's own frames (`wait_tick`, capped at 32
        // of them — 3.2 s), and the script gives itself 20 s for one. The
        // socket has to outlast that or the server gives up on work that is
        // still running.
        "run" => Duration::from_secs(25),
        // Two of Live's frames per locator it removes, plus the tracks,
        // scenes and clips. The script's own budget for it is 60 s.
        "reset_set" => Duration::from_secs(65),
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
    /// Bytes read but not yet a whole document, and whole documents read
    /// but not yet claimed: a reply that arrived before its request was
    /// waited on, or an event pushed by the script.
    pending: Mutex<Inbox>,
    next_id: AtomicU64,
}

#[derive(Default)]
pub struct Inbox {
    bytes: Vec<u8>,
    docs: std::collections::VecDeque<Value>,
    /// Events the script pushed (clock, levels, changes, cue). They arrive
    /// on the same socket as the replies and are set aside here as they are
    /// met; a consumer drains them with [`AbletonConnection::take_events`].
    events: std::collections::VecDeque<Value>,
}

/// Events older than this are dropped rather than grown without bound: a
/// clock at 10 a second fills it in half a minute if nobody is draining.
const MAX_HELD_EVENTS: usize = 512;

impl Inbox {
    fn clear(&mut self) {
        self.bytes.clear();
        self.docs.clear();
        self.events.clear();
    }

    fn note_event(&mut self, event: Value) {
        self.events.push_back(event);
        while self.events.len() > MAX_HELD_EVENTS {
            self.events.pop_front();
        }
    }
}

/// Every complete JSON document at the front of `buf`, removed from it. A
/// partial document at the end stays for the next read.
pub fn pop_documents(buf: &mut Vec<u8>) -> Vec<Value> {
    let mut docs = Vec::new();
    let mut consumed = 0usize;
    {
        let mut it = serde_json::Deserializer::from_slice(buf).into_iter::<Value>();
        loop {
            match it.next() {
                Some(Ok(v)) => {
                    docs.push(v);
                    consumed = it.byte_offset();
                }
                Some(Err(e)) if e.is_eof() => break,
                Some(Err(_)) => {
                    // Not JSON at all: drop it rather than wedge the socket.
                    consumed = buf.len();
                    break;
                }
                None => {
                    consumed = buf.len();
                    break;
                }
            }
        }
    }
    // Whitespace (a newline delimiter) between documents is consumed too.
    let rest_start = buf[consumed..]
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map(|p| consumed + p)
        .unwrap_or(buf.len());
    buf.drain(..rest_start);
    docs
}

impl AbletonConnection {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            sock: Mutex::new(None),
            pending: Mutex::new(Inbox::default()),
            next_id: AtomicU64::new(1),
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

    /// Every event the script has pushed since this was last called. Events
    /// share the socket with replies, so they are picked up as commands are
    /// sent; a consumer that wants them sooner sends a cheap read.
    pub fn take_events(&self) -> Vec<Value> {
        let mut inbox = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        inbox.events.drain(..).collect()
    }

    /// Ask the script to push these channels on this socket.
    pub fn subscribe(&self, channels: &[&str], clock_every_ms: Option<f64>) -> LiveResult<Value> {
        let mut params = json!({ "channels": channels });
        if let Some(ms) = clock_every_ms {
            params["clock_every_ms"] = json!(ms);
        }
        self.send_command("subscribe", Some(params))
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

    /// Read until the reply to request `id` arrives. Documents that are not
    /// it — an event pushed by the script, a stray reply — are set aside or
    /// dropped; a reply without an `id` is from a script before protocol 2
    /// and, with one request in flight, is the answer.
    fn receive_reply(
        stream: &mut TcpStream,
        inbox: &mut Inbox,
        id: u64,
        command: &str,
        timeout: Duration,
    ) -> LiveResult<Value> {
        let mut buf = vec![0u8; 8192];
        let deadline = Instant::now() + timeout;
        loop {
            inbox.docs.extend(pop_documents(&mut inbox.bytes));
            // Walk what has arrived: our reply (or an id-less one from an old
            // script) is the answer; an event or a reply to an earlier id is
            // noise and goes; a reply to a later id stays for its own turn.
            let mut i = 0;
            while i < inbox.docs.len() {
                let doc = &inbox.docs[i];
                if doc.get("event").is_some() {
                    let event = inbox.docs.remove(i).expect("indexed");
                    inbox.note_event(event);
                    continue;
                }
                match doc.get("id").and_then(Value::as_u64) {
                    Some(got) if got == id => return Ok(inbox.docs.remove(i).unwrap()),
                    Some(other) if other > id => i += 1,
                    Some(other) => {
                        tracing::warn!("reply for request {other} while waiting for {id}; dropped");
                        inbox.docs.remove(i);
                    }
                    None => return Ok(inbox.docs.remove(i).unwrap()),
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(LiveError::Timeout {
                    command: command.to_string(),
                    secs: timeout.as_secs_f64(),
                });
            }
            stream
                .set_read_timeout(Some(left))
                .map_err(|e| LiveError::Lost(e.to_string()))?;
            match stream.read(&mut buf) {
                Ok(0) => {
                    return Err(LiveError::Lost(if inbox.bytes.is_empty() {
                        "connection closed before any data arrived".to_string()
                    } else {
                        format!(
                            "connection closed after {} bytes of an incomplete response",
                            inbox.bytes.len()
                        )
                    }));
                }
                Ok(n) => inbox.bytes.extend_from_slice(&buf[..n]),
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
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = Self::exchange(
            guard.as_mut().expect("connected"),
            &mut pending,
            id,
            command_type,
            params,
        );
        if outcome.is_err() {
            // Any failure poisons the socket: the next call reconnects.
            *guard = None;
            pending.clear();
        }
        outcome
    }

    fn exchange(
        stream: &mut TcpStream,
        pending: &mut Inbox,
        id: u64,
        command_type: &str,
        params: Option<Value>,
    ) -> LiveResult<Value> {
        let params = params.unwrap_or_else(|| json!({}));
        let command = json!({"id": id, "type": command_type, "params": params});
        tracing::debug!("Sending command: {}", command);

        let mut payload =
            serde_json::to_vec(&command).map_err(|e| LiveError::InvalidResponse(e.to_string()))?;
        payload.push(b'\n');
        let timeout = command_timeout(command_type);
        stream.set_write_timeout(Some(timeout)).ok();
        stream.write_all(&payload).map_err(|e| match e.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => LiveError::Timeout {
                command: command_type.to_string(),
                secs: timeout.as_secs_f64(),
            },
            _ => LiveError::Lost(e.to_string()),
        })?;

        let response = Self::receive_reply(stream, pending, id, command_type, timeout)?;
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
            let (main_ms, slices) = cost_of(&response);
            note_exchange_cost(main_ms, slices);
            return Err(LiveError::Ableton(message));
        }
        note_exchange_clock(response.get("clock").cloned());
        let (main_ms, slices) = cost_of(&response);
        note_exchange_cost(main_ms, slices);
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
    /// How many sockets have been opened to Live in this process.
    generation: std::sync::atomic::AtomicU64,
}

impl RealBridge {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            connection: Mutex::new(None),
            generation: std::sync::atomic::AtomicU64::new(0),
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
                    self.generation
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

    fn connection_generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
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
        // Protocol 2: the request carries an id the reply is matched by.
        let received = live.join().unwrap();
        assert_eq!(received["type"], json!("get_session_info"));
        assert_eq!(received["params"], json!({}));
        assert!(received["id"].is_u64(), "{received}");
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

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn documents_come_off_the_front_delimited_or_not() {
        let mut buf = b"{\"a\":1}\n{\"b\":2}{\"c\":3}\n{\"d\":".to_vec();
        let docs = pop_documents(&mut buf);
        assert_eq!(docs, vec![json!({"a":1}), json!({"b":2}), json!({"c":3})]);
        assert_eq!(buf, b"{\"d\":".to_vec(), "the partial one waits");
        buf.extend_from_slice(b"4}\n");
        assert_eq!(pop_documents(&mut buf), vec![json!({"d":4})]);
        assert!(buf.is_empty());
    }

    #[test]
    fn a_split_multibyte_character_waits() {
        let text = serde_json::to_vec(&json!({"name": "Groove \u{00b7} 8"})).unwrap();
        let cut = text.iter().position(|b| *b == 0xc2).unwrap() + 1;
        let mut buf = text[..cut].to_vec();
        assert!(pop_documents(&mut buf).is_empty());
        assert_eq!(buf.len(), cut);
        buf.extend_from_slice(&text[cut..]);
        assert_eq!(
            pop_documents(&mut buf),
            vec![json!({"name": "Groove \u{00b7} 8"})]
        );
    }

    /// A fake script that answers on one socket in a scripted order.
    fn fake_script(replies: Vec<String>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut got = [0u8; 4096];
            let _ = stream.read(&mut got);
            for r in replies {
                stream.write_all(r.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
            std::thread::sleep(Duration::from_millis(300));
        });
        port
    }

    #[test]
    fn events_are_kept_not_dropped_and_drain_once() {
        let port = fake_script(vec![
            "{\"event\":\"clock\",\"bar\":3}\n{\"event\":\"levels\",\"bar\":3}\n".into(),
            "{\"id\":1,\"status\":\"success\",\"result\":{}}\n".into(),
        ]);
        let conn = AbletonConnection::new("127.0.0.1", port);
        conn.send_command("get_session_info", None).unwrap();
        let events = conn.take_events();
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0]["event"], json!("clock"));
        assert_eq!(events[1]["event"], json!("levels"));
        assert!(
            conn.take_events().is_empty(),
            "draining twice returns them twice"
        );
    }

    #[test]
    fn the_reply_is_matched_by_id_past_events_and_strays() {
        let port = fake_script(vec![
            "{\"event\":\"clock\",\"bar\":3}\n".into(),
            "{\"id\":999,\"status\":\"success\",\"result\":{\"stray\":true}}\n".into(),
            "{\"id\":1,\"status\":\"success\",\"result\":{\"tempo\":126}}\n".into(),
        ]);
        let conn = AbletonConnection::new("127.0.0.1", port);
        let r = conn.send_command("get_session_info", None).unwrap();
        assert_eq!(r, json!({"tempo":126}));
    }

    /// The script may answer out of order, so a reply for a later id is held
    /// rather than dropped. The command it belongs to is then served from the
    /// inbox: two answers, one round trip, correctly paired.
    #[test]
    fn a_reply_held_for_a_later_id_serves_its_own_command() {
        let port = fake_script(vec![
            "{\"id\":2,\"status\":\"success\",\"result\":{\"who\":\"second\"}}".into(),
            "{\"id\":1,\"status\":\"success\",\"result\":{\"who\":\"first\"}}".into(),
        ]);
        let conn = AbletonConnection::new("127.0.0.1", port);
        // Answered in the order asked, not the order sent.
        assert_eq!(
            conn.send_command("get_session_info", None).unwrap(),
            json!({"who": "first"})
        );
        // The held one is reused; the fake script writes nothing more.
        assert_eq!(
            conn.send_command("get_session_info", None).unwrap(),
            json!({"who": "second"})
        );
    }

    #[test]
    fn a_script_before_ids_still_answers() {
        let port = fake_script(vec![
            "{\"status\":\"success\",\"result\":{\"old\":true}}".into()
        ]);
        let conn = AbletonConnection::new("127.0.0.1", port);
        let r = conn.send_command("get_session_info", None).unwrap();
        assert_eq!(r, json!({"old":true}));
    }

    #[test]
    fn the_request_carries_an_id_and_a_newline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut got = vec![0u8; 4096];
            let n = stream.read(&mut got).unwrap();
            seen2.lock().unwrap().extend_from_slice(&got[..n]);
            stream
                .write_all(b"{\"id\":1,\"status\":\"success\",\"result\":{}}\n")
                .unwrap();
        });
        let conn = AbletonConnection::new("127.0.0.1", port);
        conn.send_command("get_session_info", None).unwrap();
        let raw = seen.lock().unwrap().clone();
        assert!(raw.ends_with(b"\n"));
        let doc: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(doc["id"], json!(1));
        assert_eq!(doc["type"], json!("get_session_info"));
    }

    #[test]
    fn ids_climb_per_connection() {
        let port = fake_script(vec![
            "{\"id\":1,\"status\":\"success\",\"result\":1}\n{\"id\":2,\"status\":\"success\",\"result\":2}\n".into(),
        ]);
        let conn = AbletonConnection::new("127.0.0.1", port);
        assert_eq!(conn.send_command("a", None).unwrap(), json!(1));
        // The second reply was already in the buffer: matched without a read.
        assert_eq!(conn.send_command("b", None).unwrap(), json!(2));
    }
}
