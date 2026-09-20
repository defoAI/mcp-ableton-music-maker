//! Shared test doubles — and the one real Live-shaped thing the suites run
//! against.
//!
//! There are two ways a suite reaches Live here, and they are the same
//! double seen from two sides:
//!
//! - [`FakeBridge`] — a recorder that answers canned JSON. It proves the
//!   right commands were sent. It cannot prove the set became right,
//!   because there is no set. Kept for the cases where a *failure* has to be
//!   produced that a healthy Live cannot give (`fail_from`, `failing`).
//! - [`server_on_fake_live`] — a real `AbletonConnection` over a real socket
//!   to `scripts/fake-live.py`: the real Remote Script, the real model,
//!   Live's 100 ms tick. Wrapped in a [`RecordingBridge`], so a test gets
//!   both what was sent **and** what the set became.
//!
//! ## Pointing the same tests at a real Live
//!
//! ```bash
//! cargo test --test orchestration                     # the fake
//! ABLETON_TARGET=live cargo test --test orchestration  # a real Live, open, script loaded
//! ABLETON_TARGET=live ABLETON_PORT=9877 cargo test -- --test-threads=1
//! ```
//!
//! With `ABLETON_TARGET=live` nothing is spawned: the suites connect to
//! `ABLETON_HOST`/`ABLETON_PORT` (`localhost:9877` by default), which is the
//! real Live. **This builds in the open set** — tracks, clips and scenes are
//! created — so point it at a scratch set, and run single-threaded, because
//! Live is one set and the suites assume the one they are looking at.
#![allow(dead_code)]

use mcp_ableton_music_maker::connection::{
    AbletonConnection, LiveBridge, LiveError, LiveResult, LiveState,
};
use mcp_ableton_music_maker::tools::Server;
use rmcp::model::CallToolResult;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

/// Stand-in for the Live bridge. Records every command sent and returns a
/// canned response (or error) so tool logic is tested in isolation.
pub struct FakeBridge {
    pub response: Mutex<LiveResult<Value>>,
    pub sent: Mutex<Vec<(String, Value)>>,
    /// Fail every command from the Nth one on (0-based), to test partial
    /// progress in multi-command bodies.
    pub fail_from: Mutex<Option<(usize, LiveError)>>,
    /// Per-command response sequences; the last one repeats. Commands not
    /// scripted get `response`.
    pub scripted: Mutex<HashMap<String, VecDeque<Value>>>,
    /// A clock to attach to every response, as the Remote Script does while
    /// a performance runs.
    pub clock: Mutex<Option<Value>>,
}

impl FakeBridge {
    pub fn responding(response: Value) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Ok(response)),
            sent: Mutex::new(Vec::new()),
            fail_from: Mutex::new(None),
            scripted: Mutex::new(HashMap::new()),
            clock: Mutex::new(None),
        })
    }

    /// Attach this clock to every response from now on (None to stop).
    pub fn set_clock(&self, clock: Option<Value>) {
        *self.clock.lock().unwrap() = clock;
    }

    /// Answer `command` with these responses in order; the last one repeats.
    pub fn script(&self, command: &str, responses: Vec<Value>) {
        self.scripted
            .lock()
            .unwrap()
            .insert(command.to_string(), responses.into());
    }

    pub fn failing(error: LiveError) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Err(error)),
            sent: Mutex::new(Vec::new()),
            fail_from: Mutex::new(None),
            scripted: Mutex::new(HashMap::new()),
            clock: Mutex::new(None),
        })
    }

    pub fn set_response(&self, response: Value) {
        *self.response.lock().unwrap() = Ok(response);
    }

    pub fn fail_from(&self, nth: usize, error: LiveError) {
        *self.fail_from.lock().unwrap() = Some((nth, error));
    }

    pub fn sent(&self) -> Vec<(String, Value)> {
        self.sent.lock().unwrap().clone()
    }

    pub fn commands(&self) -> Vec<String> {
        self.sent().into_iter().map(|(c, _)| c).collect()
    }
}

impl LiveBridge for FakeBridge {
    fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        let n = {
            let mut sent = self.sent.lock().unwrap();
            sent.push((
                command_type.to_string(),
                params.unwrap_or_else(|| json!({})),
            ));
            sent.len() - 1
        };
        mcp_ableton_music_maker::connection::note_exchange_clock(
            self.clock.lock().unwrap().clone(),
        );
        if let Some((from, err)) = self.fail_from.lock().unwrap().as_ref() {
            if n >= *from {
                return Err(err.clone());
            }
        }
        if let Some(queue) = self.scripted.lock().unwrap().get_mut(command_type) {
            if queue.len() > 1 {
                return Ok(queue.pop_front().unwrap());
            }
            if let Some(last) = queue.front() {
                return Ok(last.clone());
            }
        }
        self.response.lock().unwrap().clone()
    }
}

/// A tap on the way to Live: records `(command, params)` and forwards.
///
/// Wrapping the fake Live gives a test both halves — what was sent, and
/// what the set became. Wrapping [`FakeBridge`] gives exactly today's
/// recorder. Nothing here answers anything itself: a double that both
/// records and invents is how a suite ends up proving only that it called
/// what it meant to call.
pub struct RecordingBridge<B: LiveBridge> {
    inner: B,
    sent: Mutex<Vec<(String, Value)>>,
}

impl<B: LiveBridge> RecordingBridge<B> {
    pub fn wrapping(inner: B) -> Arc<Self> {
        Arc::new(Self {
            inner,
            sent: Mutex::new(Vec::new()),
        })
    }

    pub fn sent(&self) -> Vec<(String, Value)> {
        self.sent.lock().unwrap().clone()
    }

    pub fn commands(&self) -> Vec<String> {
        self.sent().into_iter().map(|(c, _)| c).collect()
    }

    /// Forget what has been sent so far, so a test can assert about one
    /// step without the set-up it needed to get there.
    pub fn clear(&self) {
        self.sent.lock().unwrap().clear();
    }

    /// The params of the last `command`, if it was sent.
    pub fn last(&self, command: &str) -> Option<Value> {
        self.sent()
            .into_iter()
            .rev()
            .find(|(c, _)| c == command)
            .map(|(_, p)| p)
    }
}

impl<B: LiveBridge> LiveBridge for RecordingBridge<B> {
    fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        self.sent.lock().unwrap().push((
            command_type.to_string(),
            params.clone().unwrap_or_else(|| json!({})),
        ));
        self.inner.send_command(command_type, params)
    }

    fn disconnect(&self) {
        self.inner.disconnect();
    }
}

/// A server wired to the fake bridge, with every capability advertised and
/// the activity log off, so tests never write into the developer's home.
pub fn server_with(bridge: Arc<FakeBridge>) -> Server {
    let live = Arc::new(LiveState::with_activity(
        bridge,
        mcp_ableton_music_maker::activity::Activity::disabled(),
    ));
    live.script.assume_all_capabilities();
    Server::new(live)
}

// ── The fake Live, one per test binary ──────────────────────────────────────

/// A `scripts/fake-live.py` owned by this test binary.
struct FakeLive {
    port: u16,
    #[allow(dead_code)]
    child: Option<Child>,
}

static FAKE_LIVE: OnceLock<FakeLive> = OnceLock::new();

/// True when the suites should talk to a real Ableton Live instead of
/// spawning the fake: `ABLETON_TARGET=live`, or an explicit `ABLETON_PORT`.
pub fn targets_a_real_live() -> bool {
    std::env::var("ABLETON_TARGET").map(|v| v == "live").unwrap_or(false)
}

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Start (or join) this binary's fake Live and return its port.
///
/// One process per test binary, started once. Each connection gets its own
/// set, so tests running in parallel inside the binary cannot collide —
/// which is not what Live is, and is written down as such in
/// `docs/architecture/overview.md`.
fn fake_live() -> &'static FakeLive {
    FAKE_LIVE.get_or_init(|| {
        if targets_a_real_live() {
            let (_, port) = mcp_ableton_music_maker::connection::live_address();
            eprintln!("tests: ABLETON_TARGET=live — talking to a real Live on port {port}");
            return FakeLive { port, child: None };
        }
        let root = repo_root();
        let mut child = Command::new("python3")
            .arg(root.join("scripts/fake-live.py"))
            .arg("--exit-with-pid")
            .arg(std::process::id().to_string())
            .current_dir(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("could not start scripts/fake-live.py — is python3 on PATH?");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("fake-live stdout"))
            .read_line(&mut line)
            .expect("fake-live printed no port");
        let port: u16 = line
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("fake-live printed {line:?} where a port should be"));
        FakeLive {
            port,
            child: Some(child),
        }
    })
}

/// A server wired through the real wire to the fake Live: a real
/// `AbletonConnection` over a real socket, tapped by a [`RecordingBridge`].
///
/// The returned bridge answers both questions — `commands()` for what was
/// sent, and [`LiveSet`] (`set()`) for what the set became.
///
/// With `ABLETON_TARGET=live` this is a real Ableton Live and the suite is
/// a real-Live verification pass. It builds in the open set; use a scratch
/// one and `--test-threads=1`.
pub fn server_on_fake_live() -> (Server, Arc<RecordingBridge<AbletonConnection>>) {
    let fake = fake_live();
    let host = if targets_a_real_live() {
        mcp_ableton_music_maker::connection::live_address().0
    } else {
        "127.0.0.1".to_string()
    };
    let bridge = RecordingBridge::wrapping(AbletonConnection::new(host, fake.port));
    let live = Arc::new(LiveState::with_activity(
        bridge.clone(),
        mcp_ableton_music_maker::activity::Activity::disabled(),
    ));
    live.script.assume_all_capabilities();
    (Server::new(live), bridge)
}

/// What the set became, read back through the same wire.
///
/// Every reader here goes to Live and asks; nothing is remembered from a
/// reply the tool under test produced. That is the whole point: a test that
/// asserts against the tool's own words proves only that the tool is
/// consistent with itself.
pub struct LiveSet<'a> {
    bridge: &'a dyn LiveBridge,
}

impl<'a> LiveSet<'a> {
    pub fn of(bridge: &'a dyn LiveBridge) -> Self {
        Self { bridge }
    }

    fn ask(&self, command: &str, params: Value) -> Value {
        self.bridge
            .send_command(command, Some(params))
            .unwrap_or_else(|e| panic!("{command} failed against the set: {e}"))
    }

    pub fn session(&self) -> Value {
        self.ask("get_session_info", json!({}))
    }

    pub fn context(&self) -> Value {
        self.ask("get_context", json!({}))
    }

    pub fn tempo(&self) -> f64 {
        self.session()["tempo"].as_f64().unwrap_or_default()
    }

    pub fn track_count(&self) -> usize {
        self.session()["track_count"].as_u64().unwrap_or_default() as usize
    }

    /// Every track's name, in order.
    pub fn track_names(&self) -> Vec<String> {
        self.context()["tracks"]
            .as_array()
            .map(|ts| {
                ts.iter()
                    .map(|t| t["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn track_index(&self, name: &str) -> Option<usize> {
        self.track_names().iter().position(|n| n == name)
    }

    /// The pitches in a Session clip, sorted, read back out of Live.
    pub fn clip_pitches(&self, track: usize, clip: usize) -> Vec<i64> {
        let reply = self.ask(
            "get_clip_notes",
            json!({"track_index": track, "clip_index": clip}),
        );
        let mut pitches: Vec<i64> = reply["notes"]
            .as_array()
            .map(|ns| ns.iter().filter_map(|n| n["pitch"].as_i64()).collect())
            .unwrap_or_default();
        pitches.sort_unstable();
        pitches
    }

    pub fn clip_name(&self, track: usize, clip: usize) -> Option<String> {
        let reply = self.ask(
            "get_clip_info",
            json!({"track_index": track, "clip_index": clip}),
        );
        reply["name"].as_str().map(|s| s.to_string())
    }

    /// Arrangement clips on a track, as `(name, start_beat)`.
    pub fn arrangement(&self, track: usize) -> Vec<(String, f64)> {
        let reply = self.ask("get_arrangement_clips", json!({"track_index": track}));
        reply["clips"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|c| {
                        (
                            c["name"].as_str().unwrap_or_default().to_string(),
                            c["start_time"].as_f64().unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Scene names, in order — sections, as the artist names them.
    pub fn scene_names(&self) -> Vec<String> {
        let reply = self.ask("get_performance_state", json!({}));
        reply["scenes"]
            .as_array()
            .map(|ss| {
                ss.iter()
                    .map(|s| s["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A track's fader, in dB off Live's own curve.
    pub fn volume_db(&self, track: usize) -> Option<f64> {
        let reply = self.ask("get_track_info", json!({"track_index": track}));
        reply["volume_db"]
            .as_f64()
            .or_else(|| reply["mixer"]["volume_db"].as_f64())
    }

    pub fn is_playing(&self) -> bool {
        self.session()["is_playing"].as_bool().unwrap_or(false)
    }

    /// Device names on a track, in order.
    pub fn devices(&self, track: usize) -> Vec<String> {
        let reply = self.ask(
            "get_device_parameters",
            json!({"track_index": track, "device_index": 0}),
        );
        reply["devices"]
            .as_array()
            .map(|ds| {
                ds.iter()
                    .map(|d| d["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("")
}

pub fn is_error(result: &CallToolResult) -> bool {
    result.is_error.unwrap_or(false)
}
