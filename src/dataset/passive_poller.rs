//! Background poller: drain Live UI events into the dataset while consent is on.

use crate::connection::LiveState;
use crate::dataset::recorder::{dataset_enabled, get_recorder};
use crate::dataset::snapshot::fetch_snapshot;
use crate::env_f64;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn default_interval() -> Duration {
    Duration::from_secs_f64(env_f64("ABLETON_MCP_PASSIVE_POLL_SEC", 2.0))
}

fn snapshot_debounce() -> Duration {
    Duration::from_secs_f64(env_f64("ABLETON_MCP_PASSIVE_SNAPSHOT_DEBOUNCE", 1.5))
}

pub struct PassiveEventPoller {
    interval: Duration,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl PassiveEventPoller {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            stop: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        }
    }

    pub fn start(&self, live: Arc<LiveState>) {
        let mut slot = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        self.stop.store(false, Ordering::SeqCst);
        let stop = self.stop.clone();
        let interval = self.interval;
        let handle = std::thread::Builder::new()
            .name("passive-poller".into())
            .spawn(move || {
                let mut pending_snapshot = false;
                let mut last_burst = Instant::now();
                while !wait_or_stop(&stop, interval) {
                    if !dataset_enabled() {
                        continue;
                    }
                    if let Err(e) = tick(&live, &mut pending_snapshot, &mut last_burst) {
                        tracing::debug!("Passive poll tick failed: {}", e);
                    }
                }
            })
            .expect("spawn passive poller");
        *slot = Some(handle);
        tracing::info!(
            "Passive event poller started (interval={:.1}s)",
            self.interval.as_secs_f64()
        );
    }

    pub fn is_running(&self) -> bool {
        let slot = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref().is_some_and(|t| !t.is_finished()) && !self.stop.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let handle = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

/// Sleep for `interval` in short slices; true if stop was requested.
fn wait_or_stop(stop: &AtomicBool, interval: Duration) -> bool {
    let deadline = Instant::now() + interval;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.load(Ordering::SeqCst)
}

fn tick(
    live: &LiveState,
    pending_snapshot: &mut bool,
    last_burst: &mut Instant,
) -> Result<(), String> {
    let Some(recorder) = get_recorder() else {
        return Ok(());
    };
    // Age out edits that nobody undid → implicit "keep".
    recorder.sweep_implicit_preferences();

    if !live.script.has_capability("drain_passive_events") {
        return Ok(());
    }
    let result = live
        .send_command("drain_passive_events", None)
        .map_err(|e| e.to_string())?;
    let events = result
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !events.is_empty() {
        *pending_snapshot = true;
        *last_burst = Instant::now();
        for ev in &events {
            recorder.record_passive_event(
                ev.get("type").and_then(Value::as_str).unwrap_or("unknown"),
                ev.get("detail").and_then(Value::as_object).cloned(),
                ev.get("track_index").and_then(Value::as_i64),
                ev.get("clip_index").and_then(Value::as_i64),
                ev.get("ts").and_then(Value::as_f64),
                None,
            );
        }
    }

    // Debounced post-burst snapshot.
    if *pending_snapshot && last_burst.elapsed() >= snapshot_debounce() {
        *pending_snapshot = false;
        if !live.script.has_capability("get_session_snapshot") {
            return Ok(());
        }
        if let Some(snapshot) = fetch_snapshot(live.bridge.as_ref(), true, true) {
            let (hash, _) = recorder.record_state(&snapshot);
            tracing::debug!("Passive burst snapshot hash={}", hash);
        }
    }
    Ok(())
}

static POLLER: Mutex<Option<Arc<PassiveEventPoller>>> = Mutex::new(None);

pub fn start_passive_poller(live: Arc<LiveState>) -> Option<Arc<PassiveEventPoller>> {
    if !dataset_enabled() {
        return None;
    }
    let mut slot = POLLER.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        let poller = Arc::new(PassiveEventPoller::new(default_interval()));
        poller.start(live);
        *slot = Some(poller);
    }
    slot.clone()
}

/// True when the poller thread is alive and draining Live events.
pub fn poller_is_running() -> bool {
    let poller = POLLER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    poller.is_some_and(|p| p.is_running())
}

pub fn stop_passive_poller() {
    let poller = POLLER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(p) = poller {
        p.stop();
    }
}
