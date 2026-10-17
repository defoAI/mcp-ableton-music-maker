//! Remember what Live plays: the tap keeps running while the app is open and
//! Live plays, with or without a window, and every bar is measured and kept
//! — numbers, never audio (`mcp_ableton_music_maker::recorder`).
//!
//! Three loops share one [`Memory`]:
//!
//! - the analysis thread (`listen::start`) hands it the tap's audio;
//! - the clock thread subscribes to the Remote Script's `clock` and
//!   `changes` channels on a socket of its own, turns them into a playhead,
//!   logs edits made in Live, watches the transport for the away switch, and
//!   releases settled audio into takes;
//! - the supervisor starts the tap when Live is running and Remember is on,
//!   writes the heartbeat the server reads, and prunes old takes.
//!
//! Like the rest of this module it writes nothing itself: takes, the change
//! log and the heartbeat go through the crate's `listening` store.

use mcp_ableton_music_maker::connection::{live_address, AbletonConnection, LiveState, RealBridge};
use mcp_ableton_music_maker::recorder::{
    Feed, FrameRow, Remembering, Seen, SetView, Take, TakeSink,
};
use mcp_ableton_music_maker::{feedback, listening};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// One monotonic clock for the audio and the clock events.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn now_s() -> f64 {
    EPOCH.elapsed().as_secs_f64()
}

/// The set as it stood when last asked: what a take is stamped with.
#[derive(Clone)]
struct SetInfo {
    key: String,
    name: String,
    view: SetView,
    slots: Vec<(String, i64)>,
    at: Instant,
}

pub struct Memory {
    rem: Mutex<Remembering>,
    on: AtomicBool,
    clock_up: AtomicBool,
    lag_s: Mutex<f64>,
    info: Mutex<Option<SetInfo>>,
    live: OnceLock<Arc<LiveState>>,
}

pub static MEMORY: LazyLock<Memory> = LazyLock::new(|| Memory {
    rem: Mutex::new(Remembering::new(48_000.0)),
    on: AtomicBool::new(false),
    clock_up: AtomicBool::new(false),
    lag_s: Mutex::new(0.0),
    info: Mutex::new(None),
    live: OnceLock::new(),
});

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Memory {
    /// Remember is on.
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// The app's own view of Live, through the same readers the tools use.
    pub fn live(&self) -> Arc<LiveState> {
        self.live
            .get_or_init(|| {
                let bridge = Arc::new(RealBridge::from_env());
                let live = Arc::new(LiveState::new(bridge.clone()));
                live.script.handshake(bridge.as_ref());
                live
            })
            .clone()
    }

    /// A new tap: its sample rate, and how far its audio trails Live.
    pub fn tap_started(&self, rate: f64, buffer_frames: u32, offset_ms: f64) {
        *lock(&self.rem) = Remembering::new(rate);
        let buf = if rate > 0.0 {
            buffer_frames as f64 / rate
        } else {
            0.0
        };
        *lock(&self.lag_s) = buf + offset_ms / 1000.0;
    }

    /// Audio the tap just handed over. Kept only while Remember is on.
    pub fn audio(&self, l: &[f32], r: &[f32]) {
        if !self.on() {
            return;
        }
        let end = now_s() - *lock(&self.lag_s);
        lock(&self.rem).audio(l, r, end);
    }

    fn refresh_info(&self) -> Option<SetInfo> {
        let live = self.live();
        // `set_view` asks Live; outside every lock, so the audio never waits.
        let (key, name, view, slots) = listening::set_view(&live);
        if key.is_empty() {
            return None;
        }
        let info = SetInfo {
            key,
            name,
            view,
            slots,
            at: Instant::now(),
        };
        *lock(&self.info) = Some(info.clone());
        Some(info)
    }

    fn info(&self, max_age: Duration) -> Option<SetInfo> {
        let cached = lock(&self.info).clone();
        match cached {
            Some(i) if i.at.elapsed() < max_age => Some(i),
            _ => self.refresh_info(),
        }
    }

    /// The open set's key and name, as the Feedback screen shows it.
    pub fn song(&self) -> Option<(String, String)> {
        self.info(Duration::from_secs(10)).map(|i| (i.key, i.name))
    }

    /// The bar the playhead is in and its section: from the clock when it
    /// runs, else asked of Live once.
    pub fn position(&self) -> Option<(i64, String, bool)> {
        let info = lock(&self.info).clone();
        let from_clock = {
            let rem = lock(&self.rem);
            rem.clock
                .at(now_s())
                .filter(|_| self.clock_up.load(Ordering::Relaxed))
        };
        if let Some(p) = from_clock {
            let bar = (p.beat / p.beats_per_bar.max(0.25)).floor() as i64 + 1;
            let section = info
                .as_ref()
                .map(|i| {
                    i.view
                        .section_at((bar - 1) as f64 * p.beats_per_bar, p.scene)
                })
                .unwrap_or_default();
            return Some((bar, section, p.playing));
        }
        let live = self.live();
        let s = live.send_command("get_session_info", None).ok()?;
        let bpb = s
            .get("signature_numerator")
            .and_then(Value::as_f64)
            .unwrap_or(4.0);
        let beat = s
            .get("current_song_time")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let bar = (beat / bpb.max(0.25)).floor() as i64 + 1;
        let section = info
            .map(|i| i.view.section_at((bar - 1) as f64 * bpb, None))
            .unwrap_or_default();
        Some((
            bar,
            section,
            s.get("is_playing")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ))
    }

    pub fn status(&self) -> Value {
        let rem = lock(&self.rem);
        let take = rem.current();
        json!({
            "on": self.on(),
            "clock": self.clock_up.load(Ordering::Relaxed),
            "recording": rem.recording(),
            "take": take.map(|t| json!({"id": t.id, "bars": t.bars.len(), "first": t.first_bar(), "last": t.last_bar()})),
        })
    }
}

/// Where takes go: the cached set info (asked of Live outside the lock), and
/// the crate's store.
struct CachedSink(Option<SetInfo>);

impl TakeSink for CachedSink {
    fn set_info(&mut self) -> (String, String, SetView) {
        match &self.0 {
            Some(i) => (i.key.clone(), i.name.clone(), i.view.clone()),
            None => (
                mcp_ableton_music_maker::memory::key_for(""),
                "Untitled".into(),
                SetView::default(),
            ),
        }
    }
    fn revision(&mut self, song_key: &str) -> u64 {
        listening::revision(song_key)
    }
    fn write(&mut self, take: &Take, frames: &[FrameRow]) {
        if let Err(e) = listening::write_take(take, frames) {
            eprintln!("could not keep take {}: {e}", take.id);
        }
    }
}

/// Start the supervisor and the clock thread. Called once from setup.
pub fn start(app: &AppHandle) {
    let a = app.clone();
    let _ = std::thread::Builder::new()
        .name("remember-supervisor".into())
        .spawn(move || supervise(a));
    let a = app.clone();
    let _ = std::thread::Builder::new()
        .name("remember-clock".into())
        .spawn(move || clock_loop(a));
}

fn supervise(app: AppHandle) {
    let mut last_prune = Instant::now() - Duration::from_secs(7200);
    loop {
        let s = crate::settings::load(&app);
        let was = MEMORY.on.swap(s.remember, Ordering::Relaxed);
        let live = super::tap::find_live();
        if s.remember && super::tap::available() && live.is_some() && !super::is_listening(&app) {
            if let Err(e) = super::start(&app) {
                eprintln!("remember: could not start listening: {e}");
            }
        }
        if was && !s.remember {
            // Switched off: finish the take, and stop a tap nobody watches.
            let info = lock(&MEMORY.info).clone();
            lock(&MEMORY.rem).finish(&mut CachedSink(info));
            super::stop_if_unwatched(&app, "");
        }
        listening::write_app_beat(&listening::AppBeat {
            pid: std::process::id(),
            remembering: s.remember,
            available: super::tap::available(),
            updated_ms: chrono::Local::now().timestamp_millis(),
            live: match live {
                None => "no_live".into(),
                Some(_) if lock(&MEMORY.rem).recording() => "playing".into(),
                Some(_) => "open".into(),
            },
            retention_days: Some(s.listening_retention_days),
        });
        // The switch's own time limit, even while nothing else watches it.
        if feedback::away().on && !feedback::away_on() {
            feedback::transport_moved_by_someone();
            let _ = app.emit("feedback:changed", json!({"away": false}));
        }
        if last_prune.elapsed() > Duration::from_secs(3600) {
            last_prune = Instant::now();
            listening::prune(s.listening_retention_days);
        }
        crate::tray::set_remembering(&app, s.remember && super::is_listening(&app));
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn clock_loop(app: AppHandle) {
    loop {
        // The clock runs while Remember is on, and while the away switch is
        // on (the producer pressing play turns it off).
        if !(MEMORY.on() || feedback::away_on()) {
            MEMORY.clock_up.store(false, Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let (host, port) = live_address();
        let conn = AbletonConnection::new(host, port);
        // 0 ms: every tick Live gives.
        if conn.subscribe(&["clock", "changes"], Some(0.0)).is_err() {
            MEMORY.clock_up.store(false, Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        MEMORY.clock_up.store(true, Ordering::Relaxed);
        let mut feed = Feed::new();
        let mut info = MEMORY.refresh_info();
        if let Some(i) = &info {
            feed.seed_session(&i.slots);
            lock(&MEMORY.rem).set_session(feed.session());
        }
        let mut was_recording = false;
        while MEMORY.on() || feedback::away_on() {
            let events = match conn.wait_events(Duration::from_millis(100)) {
                Ok(e) => e,
                Err(_) => break,
            };
            let mut seen = Vec::new();
            {
                let mut rem = lock(&MEMORY.rem);
                for ev in &events {
                    seen.extend(feed.event(ev, now_s(), &mut rem));
                }
            }
            for s in seen {
                match s {
                    Seen::Started => {
                        // A new start may be a new set: ask again, outside the lock.
                        info = MEMORY.refresh_info().or(info);
                        if let Some(a) = feedback::transport_moved_by_someone() {
                            let _ = app.emit("feedback:changed", json!({"away": a.on}));
                        }
                    }
                    Seen::Stopped => {
                        if let Some(a) = feedback::transport_moved_by_someone() {
                            let _ = app.emit("feedback:changed", json!({"away": a.on}));
                        }
                    }
                    Seen::Edit(what) => {
                        if MEMORY.on() {
                            if let Some(i) = &info {
                                listening::record_change(&i.key, "live", &what);
                            }
                        }
                    }
                    Seen::Session(_) => {}
                }
            }
            if info
                .as_ref()
                .is_none_or(|i| i.at.elapsed() > Duration::from_secs(30))
                && !lock(&MEMORY.rem).recording()
            {
                info = MEMORY.refresh_info().or(info);
            }
            let recording = {
                let mut rem = lock(&MEMORY.rem);
                rem.tick(now_s(), &mut CachedSink(info.clone()));
                rem.recording()
            };
            if was_recording && !recording {
                let _ = app.emit("feedback:changed", json!({"take": "ended"}));
            }
            was_recording = recording;
        }
        MEMORY.clock_up.store(false, Ordering::Relaxed);
        let i = lock(&MEMORY.info).clone();
        lock(&MEMORY.rem).finish(&mut CachedSink(i));
        conn.disconnect();
    }
}
