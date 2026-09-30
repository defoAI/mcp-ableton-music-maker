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
//! ## Isolation
//!
//! **Every test gets its own fake Live, in its own process, with one set** —
//! which is exactly what Live is, so there is no divergence to remember and
//! no test can see another's tracks. It costs about 110 ms to start one, and
//! eight start in 131 ms because they overlap, so the whole suite pays a few
//! seconds and buys perfect isolation for it. The process is killed when the
//! test's bridge is dropped, and `--exit-with-pid` stops any that outlive a
//! crashed test binary.
//!
//! A test that wants several connections onto **one** set (two clients, the
//! event channels) starts its own with [`spawn_fake_live`] and connects
//! twice.
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
use std::sync::{Arc, Mutex};

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

    /// The params of the last `command`, if it was sent — the same reader
    /// [`RecordingBridge`] gives, so a test reads either double the same way.
    pub fn last(&self, command: &str) -> Option<Value> {
        self.sent()
            .into_iter()
            .rev()
            .find(|(c, _)| c == command)
            .map(|(_, p)| p)
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
    /// The fake Live this bridge talks to, when it owns one: dropping the
    /// bridge stops the process, so a test cleans up after itself.
    process: Option<FakeLive>,
}

impl<B: LiveBridge> RecordingBridge<B> {
    pub fn wrapping(inner: B) -> Arc<Self> {
        Arc::new(Self {
            inner,
            sent: Mutex::new(Vec::new()),
            process: None,
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

/// A state directory of this test binary's own.
///
/// The song memory (#63) is read on every `get_context`, so a suite that
/// left `ABLETON_MCP_STATE_DIR` unset read whatever the developer's own
/// `~/.ableton-music-maker` happened to hold — a real session's overview and
/// notes turned up in the middle of an assertion about a context header, and
/// the suite passed or failed by what the machine had been used for. Tests
/// are hermetic: a binary with no state dir of its own gets a temporary one.
/// A suite that sets its own (song_memory, samples, library, sets, activity,
/// device_vocabulary, stdio_integration) still wins — this only fills a gap.
static SCRATCH_STATE: std::sync::LazyLock<tempfile::TempDir> =
    std::sync::LazyLock::new(|| tempfile::tempdir().expect("a temp state dir"));

pub fn isolate_state_dir() {
    if std::env::var_os("ABLETON_MCP_STATE_DIR").is_none() {
        std::env::set_var("ABLETON_MCP_STATE_DIR", SCRATCH_STATE.path());
    }
}

/// A server wired to the fake bridge, with every capability advertised and
/// the activity log off, so tests never write into the developer's home.
pub fn server_with(bridge: Arc<FakeBridge>) -> Server {
    isolate_state_dir();
    let live = Arc::new(LiveState::with_activity(
        bridge,
        mcp_ableton_music_maker::activity::Activity::disabled(),
    ));
    live.script.assume_all_capabilities();
    Server::new(live)
}

// ── The fake Live, one per test binary ──────────────────────────────────────

/// A `scripts/fake-live.py` process.
pub struct FakeLive {
    pub port: u16,
    child: Option<Child>,
}

impl Drop for FakeLive {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Start a fake Live of this test's own, with whatever flags it needs
/// (`--shared-set`, `--die-on`, `--record`, ...). Killed when the returned
/// value is dropped. For the ordinary case use [`server_on_fake_live`],
/// which shares one per test binary.
pub fn spawn_fake_live(args: &[&str]) -> FakeLive {
    let root = repo_root();
    let mut command = Command::new("python3");
    command
        .arg(root.join("scripts/fake-live.py"))
        .arg("--exit-with-pid")
        .arg(std::process::id().to_string())
        .args(args)
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command
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
}

/// True when the suites should talk to a real Ableton Live instead of
/// spawning the fake: `ABLETON_TARGET=live`, or an explicit `ABLETON_PORT`.
pub fn targets_a_real_live() -> bool {
    std::env::var("ABLETON_TARGET")
        .map(|v| v == "live")
        .unwrap_or(false)
}

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A server wired through the real wire to a fake Live of this test's own:
/// a real `AbletonConnection` over a real socket, tapped by a
/// [`RecordingBridge`].
///
/// One process, one set — what Live is. The process is killed when the
/// returned bridge is dropped, so nothing leaks between tests and nothing
/// outlives the run.
///
/// Calls are free here (`--latency none`) because most tests are about what
/// the set became, not what it cost. A test that is about the cost asks for
/// the measured table with [`server_on_fake_live_with`].
///
/// With `ABLETON_TARGET=live` this is a real Ableton Live and the suite is
/// a real-Live verification pass. It builds in the open set; use a scratch
/// one and `--test-threads=1`.
pub fn server_on_fake_live() -> (Server, Arc<RecordingBridge<AbletonConnection>>) {
    server_on_fake_live_with(&["--latency", "none"])
}

/// The same, with extra flags for the fake — `--latency measured` when the
/// test is about what a call costs Live, `--die-on <command>` when it is
/// about Live going away.
pub fn server_on_fake_live_with(
    args: &[&str],
) -> (Server, Arc<RecordingBridge<AbletonConnection>>) {
    isolate_state_dir();
    let (host, port, process) = if targets_a_real_live() {
        let (host, port) = mcp_ableton_music_maker::connection::live_address();
        (host, port, None)
    } else {
        // `--quiet`: a green run says nothing. A run with an error or a
        // slow slice still prints its readout.
        let mut with_shared: Vec<&str> = vec!["--shared-set", "--quiet"];
        with_shared.extend_from_slice(args);
        let fake = spawn_fake_live(&with_shared);
        ("127.0.0.1".to_string(), fake.port, Some(fake))
    };
    let bridge = Arc::new(RecordingBridge {
        inner: AbletonConnection::new(host, port),
        sent: Mutex::new(Vec::new()),
        process,
    });
    let live = Arc::new(LiveState::with_activity(
        bridge.clone(),
        mcp_ableton_music_maker::activity::Activity::disabled(),
    ));
    live.script.assume_all_capabilities();
    // A fake Live is a process of this test's own, so it starts empty. A
    // real one is the set the producer has open, and it is shared by every
    // test in the run: without this, test three built on what test two left
    // behind — "slot 0 on 'Drums' already holds a clip", a tempo from the
    // test before, a stash row from the one before that. The failures were
    // the harness's, not the code's, which is worse than a red suite
    // because it hides the ones that are real. Each test starts from the
    // set Live starts with.
    if targets_a_real_live() {
        reset_the_open_set(bridge.as_ref());
    }
    (Server::new(live), bridge)
}

/// The tool's own reply, without the live readout appended to it.
///
/// A real Live attaches a clock to every reply, so `run` ends the text with
/// "⏱ …" and, once anything has played, "🔊 …". The fake attaches one only
/// when a test asks for it. An assertion about how a tool *finishes* its
/// sentence is about the tool, not about the transport, so it reads the
/// text with those lines taken off and holds on both targets.
pub fn without_readout(text: &str) -> String {
    text.lines()
        .rev()
        .skip_while(|l| l.starts_with('⏱') || l.starts_with('🔊') || l.trim().is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Back to what Live starts with: 2 MIDI + 2 audio tracks, 8 scenes, 120
/// BPM, the returns kept. Only ever called against a real Live.
fn reset_the_open_set(bridge: &dyn LiveBridge) {
    if let Err(e) = bridge.send_command("reset_set", Some(json!({}))) {
        panic!(
            "could not reset the open Live set between tests: {e}. \
             ABLETON_TARGET=live needs a scratch set and a Remote Script \
             that serves reset_set."
        );
    }
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

    /// One command straight to the script. Set-up, not the thing under
    /// test, so it never goes through a tool.
    pub fn ask(&self, command: &str, params: Value) -> Value {
        self.bridge
            .send_command(command, Some(params))
            .unwrap_or_else(|e| panic!("{command} failed against the set: {e}"))
    }

    /// Put named tracks in the set before the test starts, the way a
    /// producer's session would already have them. `(name, kind,
    /// instrument_uri)`; an empty uri loads nothing.
    ///
    /// This is set-up, not the thing under test, so it goes straight to the
    /// script rather than through a tool.
    pub fn build(&self, tracks: &[(&str, &str, &str)]) -> Vec<usize> {
        let specs: Vec<Value> = tracks
            .iter()
            .map(|(name, kind, uri)| {
                let mut spec = json!({"name": name, "kind": kind});
                if !uri.is_empty() {
                    spec["instrument_uri"] = json!(uri);
                }
                spec
            })
            .collect();
        let reply = self.ask(
            "create_tracks",
            json!({"tracks": specs, "on_existing": "converge"}),
        );
        reply["created"]
            .as_array()
            .expect("create_tracks said nothing about what it created")
            .iter()
            .map(|c| c["index"].as_u64().unwrap_or_default() as usize)
            .collect()
    }

    /// A clip in a Session slot, with notes, as set-up.
    pub fn write_clip(&self, track: usize, slot: usize, name: &str, notes: Value) {
        self.ask(
            "write_clips",
            json!({"clips": [{
                "track_index": track, "clip_index": slot,
                "length": 4.0, "name": name, "notes": notes
            }]}),
        );
    }

    /// Put a Session clip into the Arrangement at these beats, as set-up.
    pub fn place(&self, track: usize, slot: usize, times: &[f64]) {
        self.ask(
            "place_clips",
            json!({"track_index": track, "clip_index": slot, "times": times}),
        );
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

    /// Everything Live says about a Session clip: name, length, loop points,
    /// launch mode, colour.
    pub fn clip_info(&self, track: usize, clip: usize) -> Value {
        self.ask(
            "get_clip_info",
            json!({"track_index": track, "clip_index": clip}),
        )
    }

    /// One device parameter's value, read back off the device.
    pub fn parameter(&self, track: usize, device: usize, index: usize) -> f64 {
        let reply = self.ask(
            "get_device_parameters",
            json!({"track_index": track, "device_index": device}),
        );
        reply["device"]["parameters"][index]["value"]
            .as_f64()
            .unwrap_or_else(|| panic!("no parameter {index} on device {device}: {reply}"))
    }

    /// A clip's automation for one device parameter, sampled by Live.
    pub fn automation(&self, track: usize, clip: usize, device: usize, parameter: usize) -> Value {
        self.ask(
            "get_clip_automation",
            json!({
                "track_index": track, "clip_index": clip, "arrangement": false,
                "target": {"device_index": device, "parameter_index": parameter},
                "resolution": 1.0
            }),
        )
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

    /// The locators, as `(name, beat)`. They ride on the session snapshot,
    /// which is where the script serialises `song.cue_points`.
    pub fn locators(&self) -> Vec<(String, f64)> {
        let reply = self.ask(
            "get_session_snapshot",
            json!({"include_notes": false, "include_params": false}),
        );
        reply["cue_points"]
            .as_array()
            .map(|ls| {
                ls.iter()
                    .map(|l| {
                        (
                            l["name"].as_str().unwrap_or_default().to_string(),
                            l["time"].as_f64().unwrap_or_default(),
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
        // `devices` is a list of names; `device` is the one that was asked for.
        reply["devices"]
            .as_array()
            .map(|ds| {
                ds.iter()
                    .map(|d| {
                        d.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| d["name"].as_str().unwrap_or_default().to_string())
                    })
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
