//! Listening to Live: start a process tap, measure what arrives about thirty
//! times a second, and send each frame to the window.
//!
//! Nothing is stored. The audio lives in the tap's ring buffer for a few
//! milliseconds, is turned into about ninety numbers, and is gone. No file is
//! written and no socket is opened — except the one the rest of the app
//! already uses to ask Live whether its transport is running, which is the
//! only way to tell "Live is quiet" from "macOS is handing us silence".

pub mod analysis;
pub mod tap;

use analysis::{Analyzer, Frame};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// How often a frame goes to the window.
const FRAME_MS: u64 = 33;
/// The largest block the analyser takes in one go.
const DRAIN_FRAMES: usize = 8192;
/// Silence longer than this, with Live playing, means something is wrong.
pub const SILENCE_GRACE_MS: f64 = 3000.0;

// ── what the window receives ────────────────────────────────────────────────

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

#[derive(Serialize)]
struct FramePayload {
    bands: Vec<f32>,
    hold: Vec<f32>,
    peak: [f32; 2],
    rms: [f32; 2],
    peak_hold: f32,
    correlation: f32,
    clip: bool,
    ranges: [f32; 6],
    silent: bool,
    silent_for_ms: Option<f64>,
    /// −1 unknown, 0 stopped, 1 playing. Only asked for while silent.
    live_playing: i8,
    /// The newest audio as a short mono waveform, for the visual.
    wave: Vec<f32>,
}

/// Points of waveform per frame: enough to draw, small enough to send
/// thirty times a second.
const WAVE_POINTS: usize = 256;

impl FramePayload {
    fn from(f: Frame, silent_for_ms: Option<f64>, live_playing: i8, wave: Vec<f32>) -> Self {
        Self {
            wave: wave.into_iter().map(|v| (v * 1000.0).round() / 1000.0).collect(),
            bands: f.bands.into_iter().map(r1).collect(),
            hold: f.hold.into_iter().map(r1).collect(),
            peak: [r1(f.peak[0]), r1(f.peak[1])],
            rms: [r1(f.rms[0]), r1(f.rms[1])],
            peak_hold: r1(f.peak_hold),
            correlation: (f.correlation * 100.0).round() / 100.0,
            clip: f.clip,
            ranges: f.ranges.map(r1),
            silent: f.silent,
            silent_for_ms,
            live_playing,
        }
    }
}

// ── the running session ─────────────────────────────────────────────────────

struct Session {
    stop: Arc<AtomicBool>,
    /// The analysis thread. `stop` joins it before the tap is freed, so the
    /// ring buffer is never pulled out from under a drain.
    thread: Option<std::thread::JoinHandle<()>>,
    /// Set by the thread when it tore the tap down itself (Live quit).
    ended: Arc<AtomicBool>,
    live_playing: Arc<AtomicI8>,
    frames_emitted: Arc<AtomicU64>,
    pid: i32,
    live_name: String,
    sample_rate: f64,
    channels: i32,
    buffer_frames: u32,
    started: Instant,
}

#[derive(Default)]
pub struct Listen {
    session: Mutex<Option<Session>>,
}

impl Listen {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How far behind the speakers the meters are: one device buffer plus the
/// wait for the next frame. The spectrum additionally averages over its
/// window, which the screen reports separately.
fn meter_lag_ms(buffer_frames: u32, rate: f64) -> f64 {
    let buf = if rate > 0.0 {
        buffer_frames as f64 / rate * 1000.0
    } else {
        0.0
    };
    ((buf + FRAME_MS as f64) * 10.0).round() / 10.0
}

fn window_ms(rate: f64) -> f64 {
    if rate > 0.0 {
        ((analysis::FFT_SIZE as f64 / rate * 1000.0) * 10.0).round() / 10.0
    } else {
        0.0
    }
}

fn pid_alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Ask Live whether its transport is running. Only called while the tap has
/// been silent for a while, at most once every five seconds, because it costs
/// a round trip on the same socket Claude uses.
fn probe_transport() -> Option<bool> {
    use mcp_ableton_music_maker::connection::{live_address, AbletonConnection};
    let (host, port) = live_address();
    let conn = AbletonConnection::new(host, port);
    let out = conn.send_command("get_session_info", None);
    conn.disconnect();
    out.ok()
        .and_then(|v| v.get("is_playing").and_then(Value::as_bool))
}

/// Whether a tap is running. Cheap on purpose: a mutex and nothing else, so
/// the menu bar can ask from its background thread.
pub fn is_listening<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<Listen>()
        .map(|s| s.lock().is_some())
        .unwrap_or(false)
}

/// Start listening. Errors are the sentence the screen shows, unchanged.
pub fn start<R: Runtime>(app: &AppHandle<R>) -> Result<Value, String> {
    let state = app.state::<Listen>();
    {
        let guard = state.lock();
        if guard.is_some() {
            drop(guard);
            return status(app);
        }
    }
    if !tap::available() {
        return Err(
            "This screen needs macOS 14.4 or later. Everything else in the app works here."
                .into(),
        );
    }
    let (pid, live_name) = tap::find_live()
        .ok_or("Ableton Live is not running. There is nothing to listen to yet.")?;

    tap::start(pid)?;
    let (sample_rate, channels, buffer_frames) = tap::format().unwrap_or((48_000.0, 2, 0));

    let stop = Arc::new(AtomicBool::new(false));
    let ended = Arc::new(AtomicBool::new(false));
    let live_playing = Arc::new(AtomicI8::new(-1));
    let frames_emitted = Arc::new(AtomicU64::new(0));

    // The analysis thread.
    let thread = {
        let app = app.clone();
        let stop = stop.clone();
        let ended = ended.clone();
        let live_playing = live_playing.clone();
        let frames_emitted = frames_emitted.clone();
        std::thread::Builder::new()
            .name("listen-analysis".into())
            .spawn(move || {
                let mut analyzer = Analyzer::new(sample_rate);
                let mut l = vec![0.0f32; DRAIN_FRAMES];
                let mut r = vec![0.0f32; DRAIN_FRAMES];
                let started = Instant::now();
                let mut next_frame = Duration::from_millis(FRAME_MS);
                let mut last_probe = Duration::ZERO;
                let mut last_pid_check = Duration::ZERO;
                while !stop.load(Ordering::Relaxed) {
                    let now = started.elapsed();
                    let now_ms = now.as_secs_f64() * 1000.0;
                    let n = tap::drain(&mut l, &mut r);
                    if n > 0 {
                        analyzer.push(&l[..n], &r[..n], now_ms);
                    }
                    if now >= next_frame {
                        next_frame = now + Duration::from_millis(FRAME_MS);
                        let silent_for = analyzer.silent_for(now_ms);
                        let wave = analyzer.wave(WAVE_POINTS);
                        let frame = analyzer.frame(now_ms);
                        let payload = FramePayload::from(
                            frame,
                            silent_for.map(|v| (v * 10.0).round() / 10.0),
                            live_playing.load(Ordering::Relaxed),
                            wave,
                        );
                        let _ = app.emit("listen:frame", &payload);
                        frames_emitted.fetch_add(1, Ordering::Relaxed);

                        // Silence is ambiguous: macOS never says the
                        // permission is off, it just hands us nothing. Ask
                        // Live whether it is playing, rarely.
                        let quiet = silent_for.unwrap_or(0.0) > SILENCE_GRACE_MS;
                        if quiet && now.saturating_sub(last_probe) > Duration::from_secs(5) {
                            last_probe = now;
                            let live_playing = live_playing.clone();
                            std::thread::spawn(move || {
                                let v = match probe_transport() {
                                    Some(true) => 1,
                                    Some(false) => 0,
                                    None => -1,
                                };
                                live_playing.store(v, Ordering::Relaxed);
                            });
                        } else if !quiet {
                            live_playing.store(-1, Ordering::Relaxed);
                        }

                        // Live quitting ends the session. This is the one
                        // path where the thread tears down by itself; a
                        // `stop` call joins the thread and does it instead.
                        if now.saturating_sub(last_pid_check) > Duration::from_secs(2) {
                            last_pid_check = now;
                            if !pid_alive(pid) {
                                let _ = app.emit(
                                    "listen:stopped",
                                    json!({"reason": "live_quit",
                                           "message": "Ableton Live closed, so listening stopped."}),
                                );
                                if let Some(state) = app.try_state::<Listen>() {
                                    *state.lock() = None;
                                }
                                tap::stop();
                                ended.store(true, Ordering::Release);
                                let _ = app.emit("listen:ended", json!({}));
                                break;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(4));
                }
            })
            .map_err(|e| format!("Could not start the analysis thread: {e}"))?
    };

    *state.lock() = Some(Session {
        stop,
        thread: Some(thread),
        ended,
        live_playing,
        frames_emitted,
        pid,
        live_name,
        sample_rate,
        channels,
        buffer_frames,
        started: Instant::now(),
    });
    status(app)
}

/// Stop listening and free the tap. Never fails, and returns only once the
/// analysis thread is gone, so nothing is still reading the ring buffer.
pub fn stop<R: Runtime>(app: &AppHandle<R>) -> Value {
    let session = app.try_state::<Listen>().and_then(|state| state.lock().take());
    if let Some(mut s) = session {
        s.stop.store(true, Ordering::Relaxed);
        if let Some(t) = s.thread.take() {
            let _ = t.join();
        }
        tap::stop();
        if !s.ended.load(Ordering::Acquire) {
            let _ = app.emit("listen:ended", json!({}));
        }
    } else {
        // Nothing was running; make sure nothing is left either.
        tap::stop();
    }
    json!({"listening": false})
}

/// What the state line and the menu bar show.
pub fn status<R: Runtime>(app: &AppHandle<R>) -> Result<Value, String> {
    let live = tap::find_live();
    let state = app.state::<Listen>();
    let guard = state.lock();
    let Some(s) = guard.as_ref() else {
        return Ok(json!({
            "listening": false,
            "available": tap::available(),
            "live_pid": live.as_ref().map(|(p, _)| *p),
            "live_name": live.as_ref().map(|(_, n)| n.clone()),
        }));
    };
    Ok(json!({
        "listening": true,
        "available": true,
        "live_pid": s.pid,
        "live_name": s.live_name,
        "sample_rate": s.sample_rate,
        "channels": s.channels,
        "buffer_frames": s.buffer_frames,
        "meter_lag_ms": meter_lag_ms(s.buffer_frames, s.sample_rate),
        "window_ms": window_ms(s.sample_rate),
        "bands": analysis::BANDS,
        "frames_seen": tap::frames_seen(),
        "frames_emitted": s.frames_emitted.load(Ordering::Relaxed),
        "live_playing": s.live_playing.load(Ordering::Relaxed),
        "uptime_ms": s.started.elapsed().as_millis() as u64,
        "ranges": analysis::RANGES.iter().map(|(n, lo, hi)| json!({"name": n, "lo": lo, "hi": hi})).collect::<Vec<_>>(),
    }))
}

/// Listening is something the producer can see: it stops when no window is
/// showing it. Called whenever a window is hidden or closed — `closing` names
/// the window that is on its way out, which still reports itself visible.
pub fn stop_if_unwatched<R: Runtime>(app: &AppHandle<R>, closing: &str) {
    let watched = ["main", "listen-float", "listen-visual"].iter().any(|label| {
        *label != closing
            && app
                .get_webview_window(label)
                .and_then(|w| w.is_visible().ok())
                .unwrap_or(false)
    });
    if !watched {
        stop(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lag_is_one_buffer_plus_one_frame() {
        // 256 frames at 48 kHz is 5.3 ms, plus the 33 ms frame interval.
        assert_eq!(meter_lag_ms(256, 48_000.0), 38.3);
        assert_eq!(meter_lag_ms(512, 48_000.0), 43.7);
        // An unknown rate must not divide by zero.
        assert_eq!(meter_lag_ms(512, 0.0), 33.0);
    }

    #[test]
    fn the_window_is_reported_in_milliseconds() {
        assert_eq!(window_ms(48_000.0), 85.3);
        assert_eq!(window_ms(44_100.0), 92.9);
        assert_eq!(window_ms(0.0), 0.0);
    }

    #[test]
    fn frames_round_to_one_decimal_so_the_json_stays_small() {
        let f = Frame {
            bands: vec![-12.345678; analysis::BANDS],
            hold: vec![-6.0; analysis::BANDS],
            peak: [-3.21987, -3.6],
            rms: [-14.149, -14.2],
            peak_hold: -1.84,
            correlation: 0.7123,
            clip: false,
            ranges: [-9.0; 6],
            silent: false,
        };
        let p = FramePayload::from(f, Some(0.0), -1, vec![0.123456; WAVE_POINTS]);
        assert_eq!(p.bands[0], -12.3);
        assert_eq!(p.wave.len(), WAVE_POINTS);
        assert_eq!(p.wave[0], 0.123);
        assert_eq!(p.peak[0], -3.2);
        assert_eq!(p.rms[0], -14.1);
        assert_eq!(p.correlation, 0.71);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.len() < 4000, "a frame is {} bytes", json.len());
    }

    /// The module must never grow a way to keep the audio. This walks its own
    /// source the way `tests/local_only.rs` walks the server's.
    #[test]
    fn the_listen_module_writes_nothing_and_opens_no_socket() {
        let sources = [
            ("mod.rs", include_str!("mod.rs")),
            ("analysis.rs", include_str!("analysis.rs")),
            ("tap.rs", include_str!("tap.rs")),
            ("tap.m", include_str!("tap.m")),
        ];
        let forbidden = [
            "std::fs", "File::create", "fs::write", "OpenOptions", "TcpStream", "TcpListener",
            "UdpSocket", "fopen", "NSFileManager", "NSURLSession", "reqwest", "ureq",
        ];
        for (name, src) in sources {
            // Only shipped code: the test module below names the very APIs it
            // forbids, and is compiled out of the app.
            let code_only = src.split("#[cfg(test)]").next().unwrap_or(src);
            for line in code_only.lines() {
                let code = line.trim();
                // Comments explain the rule; only code may not break it.
                if code.starts_with("//") || code.starts_with("*") || code.starts_with("///") {
                    continue;
                }
                for bad in forbidden {
                    assert!(
                        !code.contains(bad),
                        "{name} must not use {bad}: {line}"
                    );
                }
            }
        }
    }
}
