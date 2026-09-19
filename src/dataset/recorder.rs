//! Supabase-backed session recorder for music-creation trajectories.
//!
//! Enabled only when telemetry is on *and* the user has consented. Rows
//! stream to Supabase through a bounded queue and a worker thread; nothing
//! is written to the user's disk and nothing ever blocks a tool call.

use crate::dataset::schema::{
    collection_mode, event_type, make_action, make_audition, make_intent, make_preference,
    make_session_start, make_state, new_id, now_ts, preference_source, stable_hash, ActionSpec,
    PreferenceSpec, SessionMeta, TrajectoryEvent, SCHEMA_VERSION,
};
use crate::dataset::supabase::{
    create_supabase_client, get_customer_uuid, RowError, SupabaseClient,
};
use crate::{env_f64, env_i64, env_str};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const TABLE_SESSIONS: &str = "dataset_sessions";
pub const TABLE_STATES: &str = "dataset_states";
pub const TABLE_EVENTS: &str = "dataset_events";

pub const MAX_INTENT_CHARS: usize = 2000;
const MAX_TRACKED_STATE_HASHES: usize = 2000;
const MAX_ERROR_CHARS: usize = 1000;
const QUEUE_CAPACITY: usize = 2000;

/// Actions whose fate we watch to infer implicit preference (kept vs. undone).
const IMPLICIT_TRACKED_TOOLS: &[&str] = &[
    "create_clip",
    "create_audio_clip",
    "add_notes_to_clip",
    "create_midi_track",
    "create_audio_track",
    "load_instrument_or_effect",
    "load_drum_kit",
    "duplicate_to_arrangement",
    "set_device_parameter",
];

/// An edit undone within this many seconds reads as a rejection.
fn implicit_reject_window_sec() -> f64 {
    env_f64("ABLETON_MCP_IMPLICIT_REJECT_WINDOW", 45.0)
}

/// An edit that survives this many later steps reads as kept.
fn implicit_keep_after_steps() -> i64 {
    env_i64("ABLETON_MCP_IMPLICIT_KEEP_STEPS", 5)
}

fn email_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| Regex::new(r"\b[\w.+-]+@[\w-]+\.[\w.]+\b").expect("valid regex"))
}

/// Absolute POSIX paths and Windows drive/UNC paths. Path segments may
/// contain spaces, so keep consuming space-separated runs while they still
/// look like part of the path.
fn path_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r#"(?:[A-Za-z]:\\|\\\\|/(?:Users|home)/)[^\s"']*(?:[ ]+[^\s"']*[/\\][^\s"']*)*(?:[ ]+[^\s"']*\.[A-Za-z0-9]{1,8})?"#)
            .expect("valid regex")
    })
}

/// Strip obvious PII from free text before it leaves the machine: email
/// addresses and absolute filesystem paths (which leak usernames), then cap
/// length. None for empty input.
pub fn scrub_text(text: Option<&str>, max_chars: usize) -> Option<String> {
    let text = text?;
    if text.is_empty() {
        return None;
    }
    let cleaned = email_re().replace_all(text, "<email>");
    let cleaned = path_re().replace_all(&cleaned, "<path>");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.chars().count() > max_chars {
        let mut s: String = cleaned.chars().take(max_chars).collect();
        s.push('…');
        return Some(s);
    }
    Some(cleaned.to_string())
}

pub fn scrub_prompt(text: Option<&str>) -> Option<String> {
    scrub_text(text, MAX_INTENT_CHARS)
}

/// True only when the user has explicitly opted in: telemetry enabled, an
/// explicit yes, and no kill switch. An unanswered question means off.
pub fn dataset_enabled() -> bool {
    if !crate::dataset::consent::recording_allowed() {
        return false;
    }
    crate::telemetry::is_telemetry_enabled() && crate::telemetry::get_telemetry_consent()
}

fn ts_to_iso(ts: f64) -> String {
    chrono::DateTime::from_timestamp(ts.floor() as i64, ((ts.fract()) * 1e9) as u32)
        .unwrap_or_default()
        .to_rfc3339()
}

/// Where rows go. Production writes to Supabase; tests install a recorder.
pub trait RowSink: Send + Sync {
    fn write(&self, table: &str, row: &Value) -> Result<(), RowError>;
}

impl RowSink for SupabaseClient {
    fn write(&self, table: &str, row: &Value) -> Result<(), RowError> {
        self.insert(table, row)
    }
}

/// Builds a sink for a session id; None means "no credentials, drop rows".
pub type SinkFactory = Arc<dyn Fn(&str) -> Option<Arc<dyn RowSink>> + Send + Sync>;

enum QueueItem {
    Row { table: &'static str, row: Value },
    Stop,
}

#[derive(Debug, Clone)]
struct TrackedAction {
    action_id: String,
    track_index: Option<i64>,
    clip_index: Option<i64>,
    ts: f64,
    step_id: Option<i64>,
    settled: bool,
    keep_reason: Option<String>,
}

struct Inner {
    active_intent_id: Option<String>,
    active_intent_text: Option<String>,
    active_intent_level: Option<i64>,
    last_action_id: Option<String>,
    started: bool,
    seen_state_hashes: VecDeque<String>,
    seen_state_set: HashSet<String>,
    step_counter: i64,
    dropped: usize,
    last_drop_log: f64,
    recent_actions: Vec<TrackedAction>,
    /// Last post-action state hash, reusable as the next action's pre-hash.
    /// Invalidated by any human edit drained from Live.
    cached_state_hash: Option<String>,
    meta: SessionMeta,
}

/// In-memory session handle that streams trajectory rows to Supabase.
pub struct SessionRecorder {
    pub session_id: String,
    pub mode: String,
    pub customer_uuid: Option<String>,
    inner: Mutex<Inner>,
    tx: SyncSender<QueueItem>,
    pending: Arc<AtomicUsize>,
}

impl SessionRecorder {
    pub fn new(
        mode: &str,
        version: Option<&str>,
        customer_uuid: Option<String>,
        factory: SinkFactory,
    ) -> Self {
        let session_id = new_id("sess");
        let (tx, rx) = sync_channel::<QueueItem>(QUEUE_CAPACITY);
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = pending.clone();
        let worker_session = session_id.clone();
        std::thread::Builder::new()
            .name("dataset-recorder".into())
            .spawn(move || worker_loop(rx, factory, worker_session, worker_pending))
            .expect("spawn dataset worker");
        Self {
            session_id: session_id.clone(),
            mode: mode.to_string(),
            customer_uuid: customer_uuid.or_else(get_customer_uuid),
            inner: Mutex::new(Inner {
                active_intent_id: None,
                active_intent_text: None,
                active_intent_level: None,
                last_action_id: None,
                started: false,
                seen_state_hashes: VecDeque::new(),
                seen_state_set: HashSet::new(),
                step_counter: 0,
                dropped: 0,
                last_drop_log: 0.0,
                recent_actions: Vec::new(),
                cached_state_hash: None,
                meta: SessionMeta {
                    session_id,
                    started_at: now_ts(),
                    mode: mode.to_string(),
                    schema_version: SCHEMA_VERSION,
                    ableton_mcp_version: version.map(str::to_string),
                    notes: None,
                    active_intent_id: None,
                    ended_at: None,
                },
            }),
            tx,
            pending,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn active_intent_id(&self) -> Option<String> {
        self.lock().active_intent_id.clone()
    }

    pub fn last_action_id(&self) -> Option<String> {
        self.lock().last_action_id.clone()
    }

    pub fn session_dir(&self) -> String {
        format!("supabase://{TABLE_SESSIONS}/{}", self.session_id)
    }

    pub fn meta(&self) -> SessionMeta {
        self.lock().meta.clone()
    }

    pub fn start(&self) -> SessionMeta {
        let mut inner = self.lock();
        if inner.started {
            return inner.meta.clone();
        }
        let row = json!({
            "id": self.session_id,
            "customer_uuid": self.customer_uuid,
            "mode": self.mode,
            "schema_version": inner.meta.schema_version,
            "ableton_mcp_version": inner.meta.ableton_mcp_version,
            "platform": std::env::consts::OS,
            "started_at": ts_to_iso(inner.meta.started_at),
        });
        self.enqueue(&mut inner, TABLE_SESSIONS, row);
        let start = make_session_start(&self.session_id, &self.mode);
        self.enqueue_event(&mut inner, start);
        inner.started = true;
        tracing::info!("Dataset session started on Supabase: {}", self.session_id);
        inner.meta.clone()
    }

    pub fn end(&self, timeout: Duration) {
        {
            let mut inner = self.lock();
            if !inner.started {
                return;
            }
            let pending: Vec<String> = inner
                .recent_actions
                .iter_mut()
                .filter(|a| !a.settled)
                .map(|a| {
                    a.settled = true;
                    a.action_id.clone()
                })
                .collect();
            for action_id in pending {
                self.emit_implicit_preference(
                    &mut inner,
                    "keep",
                    &action_id,
                    "still present at session end",
                );
            }
            inner.meta.ended_at = Some(now_ts());
            inner.meta.active_intent_id = inner.active_intent_id.clone();
            // No session-row update: anon holds INSERT only, and everything
            // the update carried is recoverable from the events themselves.
            let end_event = TrajectoryEvent::new(event_type::SESSION_END, &self.session_id);
            self.enqueue_event(&mut inner, end_event);
            inner.started = false;
        }
        // Outside the lock: the worker needs to make progress.
        let drained = self.flush(timeout);
        let _ = self.tx.try_send(QueueItem::Stop);
        let inner = self.lock();
        if inner.dropped > 0 {
            tracing::warn!(
                "Dataset session {} dropped {} rows due to queue overflow",
                self.session_id,
                inner.dropped
            );
        }
        tracing::info!(
            "Dataset session ended: {} ({} steps, {})",
            self.session_id,
            inner.step_counter,
            if drained {
                "flushed"
            } else {
                "flush timed out"
            }
        );
    }

    pub fn set_intent(&self, text: &str, level: Option<i64>, source: &str) -> TrajectoryEvent {
        let labels = crate::dataset::hierarchy::label_intent(text, level);
        let mut event = make_intent(&self.session_id, text, Some(labels.intent_level));
        let extra = event.extra_mut();
        extra.insert("op_level".into(), json!(labels.op_level));
        extra.insert("op_label".into(), json!(labels.op_label));
        extra.insert("intent_source".into(), json!(source));
        let mut inner = self.lock();
        inner.active_intent_id = event.intent_id.clone();
        inner.active_intent_text = Some(text.to_string());
        inner.active_intent_level = Some(labels.intent_level);
        inner.meta.active_intent_id = event.intent_id.clone();
        self.enqueue_event(&mut inner, event.clone());
        event
    }

    /// Adopt the caller's natural-language prompt as the active intent. A
    /// repeated prompt keeps the existing intent so one request stays one
    /// intent across the whole sequence of tool calls it triggers.
    pub fn observe_prompt(&self, text: &str) -> Option<TrajectoryEvent> {
        let cleaned = scrub_prompt(Some(text))?;
        {
            let inner = self.lock();
            if inner.active_intent_text.as_deref() == Some(cleaned.as_str()) {
                return None;
            }
        }
        Some(self.set_intent(&cleaned, None, "user_prompt"))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_action(
        &self,
        tool: &str,
        params: Option<Map<String, Value>>,
        pre_hash: Option<String>,
        post_hash: Option<String>,
        success: bool,
        duration_ms: Option<f64>,
        error: Option<&str>,
        intent_id: Option<String>,
        pre_hash_source: Option<&str>,
    ) -> TrajectoryEvent {
        let (active_id, active_text, active_level) = {
            let inner = self.lock();
            (
                inner.active_intent_id.clone(),
                inner.active_intent_text.clone(),
                inner.active_intent_level,
            )
        };
        // Error strings are raw tool output and routinely quote the arguments
        // that failed, including absolute paths.
        let error = scrub_text(error, MAX_ERROR_CHARS);
        let labels = crate::dataset::hierarchy::label_action(tool, params.as_ref());
        let mut event = make_action(
            &self.session_id,
            ActionSpec {
                tool,
                params: params.clone(),
                pre_hash,
                post_hash,
                success,
                duration_ms,
                error,
                intent_id: intent_id.or(active_id),
                text: active_text,
                level: active_level,
            },
        );
        {
            let extra = event.extra_mut();
            for (k, v) in labels {
                extra.insert(k, v);
            }
            if let Some(src) = pre_hash_source {
                extra.insert("pre_hash_source".into(), json!(src));
            }
        }
        let mut inner = self.lock();
        inner.last_action_id = event.action_id.clone();
        self.enqueue_event(&mut inner, event.clone());
        if success && IMPLICIT_TRACKED_TOOLS.contains(&tool) {
            let params = params.unwrap_or_default();
            inner.recent_actions.push(TrackedAction {
                action_id: event.action_id.clone().unwrap_or_default(),
                track_index: params.get("track_index").and_then(Value::as_i64),
                clip_index: params.get("clip_index").and_then(Value::as_i64),
                ts: event.ts,
                step_id: event.step_id,
                settled: false,
                keep_reason: None,
            });
            // Bound memory: only the recent tail can still be undone.
            if inner.recent_actions.len() > 200 {
                let excess = inner.recent_actions.len() - 200;
                inner.recent_actions.drain(..excess);
            }
        }
        event
    }

    fn emit_implicit_preference(
        &self,
        inner: &mut Inner,
        rating: &str,
        target_action_id: &str,
        note: &str,
    ) {
        let event = make_preference(
            &self.session_id,
            PreferenceSpec {
                rating,
                target_action_id: Some(target_action_id.to_string()),
                note: Some(note.to_string()),
                intent_id: inner.active_intent_id.clone(),
                rating_source: Some(preference_source::IMPLICIT),
                ..Default::default()
            },
        );
        self.enqueue_event(inner, event);
    }

    /// Emit 'keep' for actions that survived. Called periodically by the
    /// poller. Either signal is sufficient on its own: outliving the undo
    /// window and outliving N further steps each mean "nobody reverted this".
    pub fn sweep_implicit_preferences(&self) -> usize {
        let mut inner = self.lock();
        let now = now_ts();
        let window = implicit_reject_window_sec();
        let keep_steps = implicit_keep_after_steps();
        let current_step = inner.step_counter;
        let mut survivors = Vec::new();
        for action in inner.recent_actions.iter_mut().filter(|a| !a.settled) {
            let step = action.step_id.unwrap_or(0);
            let aged_out = now - action.ts > window;
            let outlived = current_step - step >= keep_steps;
            if aged_out || outlived {
                action.settled = true;
                action.keep_reason = Some(if outlived {
                    format!("survived {keep_steps}+ steps without being undone")
                } else {
                    format!("survived the {window:.0}s undo window")
                });
                survivors.push(action.clone());
            }
        }
        for action in &survivors {
            self.emit_implicit_preference(
                &mut inner,
                "keep",
                &action.action_id,
                action
                    .keep_reason
                    .as_deref()
                    .unwrap_or("survived without being undone"),
            );
        }
        survivors.len()
    }

    /// Record a human Live-UI edit drained from the Remote Script.
    pub fn record_passive_event(
        &self,
        kind: &str,
        detail: Option<Map<String, Value>>,
        track_index: Option<i64>,
        clip_index: Option<i64>,
        ts: Option<f64>,
        post_hash: Option<String>,
    ) -> TrajectoryEvent {
        let mut params = detail.unwrap_or_default();
        if let Some(t) = track_index {
            params.insert("track_index".into(), json!(t));
        }
        if let Some(c) = clip_index {
            params.insert("clip_index".into(), json!(c));
        }
        let tool = format!("human_{kind}");
        let intent_id = self.lock().active_intent_id.clone();
        let mut event = make_action(
            &self.session_id,
            ActionSpec {
                tool: &tool,
                params: Some(params.clone()),
                post_hash,
                success: true,
                intent_id,
                ..Default::default()
            },
        );
        if let Some(t) = ts {
            // The Remote Script reports its own clock; clamp it into server
            // time so it stays comparable with agent-action timestamps.
            event.ts = t.min(now_ts());
        }
        {
            let extra = event.extra_mut();
            extra.insert("source".into(), json!("live_ui"));
            extra.insert("passive_type".into(), json!(kind));
            extra.insert("op_level".into(), json!(1));
            extra.insert("op_label".into(), json!(tool));
        }
        let mut inner = self.lock();
        inner.last_action_id = event.action_id.clone();
        self.enqueue_event(&mut inner, event.clone());
        inner.cached_state_hash = None;
        if let Some(reverted_id) =
            Self::match_undo(&mut inner, kind, &params, track_index, clip_index, event.ts)
        {
            let note = format!(
                "undone in Live within {:.0}s ({kind})",
                implicit_reject_window_sec()
            );
            self.emit_implicit_preference(&mut inner, "reject", &reverted_id, &note);
        }
        event
    }

    /// Find the tracked action that a human undo event most likely reverted.
    fn match_undo(
        inner: &mut Inner,
        kind: &str,
        detail: &Map<String, Value>,
        track_index: Option<i64>,
        clip_index: Option<i64>,
        ts: f64,
    ) -> Option<String> {
        // Only deletion-shaped events signal a rejection.
        let deletion = (kind == "clip_slot_changed"
            && detail.get("has_clip") == Some(&Value::Bool(false)))
            || (kind == "tracks_changed"
                && detail.get("removed").is_some_and(|r| {
                    r.as_bool() == Some(true) || r.as_array().is_some_and(|a| !a.is_empty())
                }));
        if !deletion {
            return None;
        }
        let window = implicit_reject_window_sec();
        for action in inner.recent_actions.iter_mut().rev().filter(|a| !a.settled) {
            // `continue`, not `break`: passive events carry Live's clock while
            // agent actions carry the server's, so timestamps can be skewed.
            if ts - action.ts > window {
                continue;
            }
            if track_index.is_some()
                && action.track_index.is_some()
                && action.track_index != track_index
            {
                continue;
            }
            if clip_index.is_some()
                && action.clip_index.is_some()
                && action.clip_index != clip_index
            {
                continue;
            }
            action.settled = true;
            return Some(action.action_id.clone());
        }
        None
    }

    /// The last post-action state hash, if still known to be valid. Lets a
    /// modifying tool skip its pre-snapshot.
    pub fn take_cached_state_hash(&self) -> Option<String> {
        self.lock().cached_state_hash.clone()
    }

    /// Forget the cached state hash: Live changed outside our control.
    pub fn invalidate_cached_state(&self) {
        self.lock().cached_state_hash = None;
    }

    /// Insert the snapshot into `dataset_states` (once per hash) and log a
    /// state event. Returns (hash, event).
    pub fn record_state(&self, snapshot: &Value) -> (String, TrajectoryEvent) {
        let state_hash = stable_hash(snapshot);
        let mut inner = self.lock();
        inner.cached_state_hash = Some(state_hash.clone());
        if !inner.seen_state_set.contains(&state_hash) {
            inner.seen_state_set.insert(state_hash.clone());
            inner.seen_state_hashes.push_back(state_hash.clone());
            if inner.seen_state_hashes.len() > MAX_TRACKED_STATE_HASHES {
                if let Some(old) = inner.seen_state_hashes.pop_front() {
                    inner.seen_state_set.remove(&old);
                }
            }
            let row = json!({
                "session_id": self.session_id,
                "customer_uuid": self.customer_uuid,
                "state_hash": state_hash,
                "snapshot": snapshot,
                "intent_id": inner.active_intent_id,
            });
            self.enqueue(&mut inner, TABLE_STATES, row);
        }
        let event = make_state(
            &self.session_id,
            &state_hash,
            &format!("dataset_states:{state_hash}"),
            inner.active_intent_id.clone(),
        );
        self.enqueue_event(&mut inner, event.clone());
        (state_hash, event)
    }

    pub fn record_preference(&self, spec: PreferenceSpec<'_>) -> TrajectoryEvent {
        let mut inner = self.lock();
        let mut spec = spec;
        if spec.target_action_id.is_none() {
            spec.target_action_id = inner.last_action_id.clone();
        }
        spec.intent_id = inner.active_intent_id.clone();
        let event = make_preference(&self.session_id, spec);
        self.enqueue_event(&mut inner, event.clone());
        event
    }

    pub fn record_audition(
        &self,
        uri: &str,
        kept: Option<bool>,
        search_query: Option<String>,
        dwell_ms: Option<f64>,
    ) -> TrajectoryEvent {
        let mut inner = self.lock();
        let event = make_audition(
            &self.session_id,
            uri,
            kept,
            search_query,
            dwell_ms,
            inner.active_intent_id.clone(),
        );
        self.enqueue_event(&mut inner, event.clone());
        event
    }

    /// Stamp the event's trajectory position and queue it.
    fn enqueue_event(&self, inner: &mut Inner, mut event: TrajectoryEvent) {
        if event.step_id.is_none() {
            inner.step_counter += 1;
            event.step_id = Some(inner.step_counter);
        }
        let row = self.event_row(&event);
        self.enqueue(inner, TABLE_EVENTS, row);
    }

    fn event_row(&self, event: &TrajectoryEvent) -> Value {
        json!({
            "event_id": event.event_id,
            "session_id": event.session_id,
            "customer_uuid": self.customer_uuid,
            "event_type": event.kind,
            "schema_version": event.v,
            "event_timestamp": event.ts,
            "step_id": event.step_id,
            "intent_id": event.intent_id,
            "action_id": event.action_id,
            "target_action_id": event.target_action_id,
            "state_hash": event.state_hash,
            "tool": event.tool,
            "category": event.category,
            "params": event.params,
            "pre_hash": event.pre_hash,
            "post_hash": event.post_hash,
            "success": event.success,
            "duration_ms": event.duration_ms,
            "error": event.error,
            "intent_text": event.text,
            "intent_level": event.level,
            "rating": event.rating,
            "rating_source": event.rating_source,
            "tags": event.tags,
            "note": event.note,
            "winner": event.winner,
            "candidate_a": event.candidate_a,
            "candidate_b": event.candidate_b,
            "uri": event.uri,
            "kept": event.kept,
            "search_query": event.search_query,
            "dwell_ms": event.dwell_ms,
            "extra": event.extra,
        })
    }

    /// Queue a row for insert. Nulls are dropped so Supabase defaults apply.
    fn enqueue(&self, inner: &mut Inner, table: &'static str, row: Value) {
        let clean = match row {
            Value::Object(map) => {
                Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).collect())
            }
            other => other,
        };
        self.pending.fetch_add(1, Ordering::SeqCst);
        match self.tx.try_send(QueueItem::Row { table, row: clean }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.pending.fetch_sub(1, Ordering::SeqCst);
                inner.dropped += 1;
                let now = now_ts();
                if now - inner.last_drop_log > 10.0 {
                    tracing::warn!(
                        "Dataset queue full — {} rows dropped so far (writes are best-effort and never block tool calls)",
                        inner.dropped
                    );
                    inner.last_drop_log = now;
                }
            }
        }
    }

    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Block until queued rows are written (or timeout). True if drained.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.pending() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let remaining = self.pending();
        if remaining > 0 {
            tracing::warn!("Dataset flush timed out with {} rows pending", remaining);
        }
        remaining == 0
    }
}

fn write_row(sink: &dyn RowSink, table: &str, row: &Value) -> Result<(), RowError> {
    match sink.write(table, row) {
        // dataset_states is unique on (session_id, state_hash) and states
        // legitimately recur; the hash cache absorbs most repeats but is
        // bounded, so a duplicate after eviction is expected.
        Err(RowError::Duplicate) => {
            tracing::debug!("Dataset: row already present in {}, skipping", table);
            Ok(())
        }
        other => other,
    }
}

fn process_row(
    client: &mut Option<Arc<dyn RowSink>>,
    session_row_ok: &mut bool,
    factory: &SinkFactory,
    session_id: &str,
    table: &str,
    row: &Value,
) -> Result<(), RowError> {
    if client.is_none() {
        *client = factory(session_id);
        if client.is_none() {
            tracing::debug!("Dataset: no Supabase client; dropping writes");
            return Ok(());
        }
    }
    // Every state/event row carries an FK to dataset_sessions, so nothing else
    // may be written until that parent row lands.
    let is_session_row = table == TABLE_SESSIONS;
    if !is_session_row && !*session_row_ok {
        tracing::debug!("Dataset: session row not confirmed, skipping {}", table);
        return Ok(());
    }
    let attempts = if is_session_row { 3 } else { 1 };
    let mut last_error = None;
    for attempt in 0..attempts {
        let sink = client.as_ref().expect("client present").clone();
        match write_row(sink.as_ref(), table, row) {
            Ok(()) => {
                last_error = None;
                break;
            }
            Err(e) => {
                last_error = Some(e);
                if attempt + 1 < attempts {
                    if let Some(fresh) = factory(session_id) {
                        *client = Some(fresh);
                    }
                    std::thread::sleep(Duration::from_secs_f64(0.5 * (attempt + 1) as f64));
                }
            }
        }
    }
    if let Some(e) = last_error {
        return Err(e);
    }
    if is_session_row {
        *session_row_ok = true;
    }
    Ok(())
}

fn worker_loop(
    rx: Receiver<QueueItem>,
    factory: SinkFactory,
    session_id: String,
    pending: Arc<AtomicUsize>,
) {
    let mut client: Option<Arc<dyn RowSink>> = None;
    let mut session_row_ok = false;
    for item in rx {
        match item {
            QueueItem::Stop => return,
            QueueItem::Row { table, row } => {
                if let Err(e) = process_row(
                    &mut client,
                    &mut session_row_ok,
                    &factory,
                    &session_id,
                    table,
                    &row,
                ) {
                    tracing::warn!("Dataset Supabase write failed: {:?}", e);
                    client = None;
                }
                pending.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

// ── Process-wide instance ───────────────────────────────────────────────────

static RECORDER: Mutex<Option<Arc<SessionRecorder>>> = Mutex::new(None);
static SINK_OVERRIDE: OnceLock<Mutex<Option<Arc<dyn RowSink>>>> = OnceLock::new();

fn sink_override() -> &'static Mutex<Option<Arc<dyn RowSink>>> {
    SINK_OVERRIDE.get_or_init(|| Mutex::new(None))
}

fn default_factory() -> SinkFactory {
    if let Some(sink) = sink_override()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Arc::new(move |_| Some(sink.clone()));
    }
    Arc::new(|session_id: &str| {
        create_supabase_client(Some(session_id)).map(|c| Arc::new(c) as Arc<dyn RowSink>)
    })
}

/// The active recorder, or None if dataset recording is disabled.
pub fn get_recorder() -> Option<Arc<SessionRecorder>> {
    if !dataset_enabled() {
        return None;
    }
    let mut slot = RECORDER.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        let mode = {
            let m = env_str("ABLETON_MCP_DATASET_MODE");
            if m.is_empty() {
                collection_mode::AGENT.to_string()
            } else {
                m
            }
        };
        let recorder = Arc::new(SessionRecorder::new(
            &mode,
            Some(crate::MCP_VERSION),
            None,
            default_factory(),
        ));
        recorder.start();
        *slot = Some(recorder);
    }
    slot.clone()
}

/// Clear the singleton (unit tests only).
pub fn reset_recorder_for_tests() {
    *RECORDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Route rows of recorders created after this call into `sink`.
pub fn set_row_sink_for_tests(sink: Option<Arc<dyn RowSink>>) {
    *sink_override().lock().unwrap_or_else(|e| e.into_inner()) = sink;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubbing_removes_pii() {
        assert_eq!(
            scrub_text(Some("mail me at a.b+c@example.com now"), 100).unwrap(),
            "mail me at <email> now"
        );
        assert_eq!(
            scrub_text(Some("open /Users/nick/Music/My Song.als please"), 100).unwrap(),
            "open <path> please"
        );
        assert_eq!(scrub_text(Some("   "), 100), None);
        assert_eq!(scrub_text(Some("abcdef"), 3).unwrap(), "abc…");
    }

    /// A sink that remembers every row it was given.
    #[derive(Default)]
    struct MemorySink(Mutex<Vec<(String, Value)>>);
    impl RowSink for MemorySink {
        fn write(&self, table: &str, row: &Value) -> Result<(), RowError> {
            self.0
                .lock()
                .unwrap()
                .push((table.to_string(), row.clone()));
            Ok(())
        }
    }

    fn recorder_with(sink: Arc<MemorySink>) -> SessionRecorder {
        let factory: SinkFactory = Arc::new(move |_| Some(sink.clone() as Arc<dyn RowSink>));
        SessionRecorder::new("agent", Some("test"), Some("cust-1".into()), factory)
    }

    #[test]
    fn session_rows_are_ordered_and_stepped() {
        let sink = Arc::new(MemorySink::default());
        let rec = recorder_with(sink.clone());
        rec.start();
        rec.set_intent("make the chorus bigger", None, "explicit");
        let action = rec.record_action(
            "create_clip",
            Some(
                json!({"track_index": 0, "clip_index": 1})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            None,
            Some("h1".into()),
            true,
            Some(3.0),
            None,
            None,
            Some("fresh"),
        );
        rec.record_preference(PreferenceSpec {
            rating: "keep",
            ..Default::default()
        });
        assert!(rec.flush(Duration::from_secs(5)));
        let rows = sink.0.lock().unwrap();
        let tables: Vec<&str> = rows.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            tables,
            [
                TABLE_SESSIONS,
                TABLE_EVENTS,
                TABLE_EVENTS,
                TABLE_EVENTS,
                TABLE_EVENTS
            ]
        );
        let steps: Vec<i64> = rows[1..]
            .iter()
            .map(|(_, r)| r["step_id"].as_i64().unwrap())
            .collect();
        assert_eq!(steps, [1, 2, 3, 4]);
        let pref = &rows[4].1;
        assert_eq!(pref["target_action_id"], json!(action.action_id.unwrap()));
        assert_eq!(
            pref["intent_text"],
            Value::Null,
            "nulls are dropped before insert"
        );
        assert_eq!(rows[3].1["extra"]["op_label"], "create_clip");
        assert_eq!(rows[3].1["extra"]["pre_hash_source"], "fresh");
    }

    #[test]
    fn undo_within_window_emits_reject_and_state_dedups() {
        let sink = Arc::new(MemorySink::default());
        let rec = recorder_with(sink.clone());
        rec.start();
        let action = rec.record_action(
            "add_notes_to_clip",
            Some(
                json!({"track_index": 2, "clip_index": 0})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            None,
            None,
            true,
            None,
            None,
            None,
            None,
        );
        rec.record_passive_event(
            "clip_slot_changed",
            Some(json!({"has_clip": false}).as_object().unwrap().clone()),
            Some(2),
            Some(0),
            None,
            None,
        );
        let snap = json!({"tracks": [{"name": "Kick"}]});
        rec.record_state(&snap);
        rec.record_state(&snap);
        assert!(rec.flush(Duration::from_secs(5)));
        let rows = sink.0.lock().unwrap();
        let rejects: Vec<&Value> = rows
            .iter()
            .filter(|(_, r)| r["rating"] == "reject")
            .map(|(_, r)| r)
            .collect();
        assert_eq!(rejects.len(), 1);
        assert_eq!(
            rejects[0]["target_action_id"],
            json!(action.action_id.unwrap())
        );
        assert_eq!(rejects[0]["rating_source"], "implicit");
        assert_eq!(
            rows.iter().filter(|(t, _)| t == TABLE_STATES).count(),
            1,
            "identical snapshot inserted once"
        );
        assert_eq!(
            rows.iter()
                .filter(|(_, r)| r["event_type"] == "state")
                .count(),
            2
        );
    }

    #[test]
    fn rows_are_held_until_session_row_lands() {
        struct FlakySink {
            calls: Mutex<Vec<String>>,
        }
        impl RowSink for FlakySink {
            fn write(&self, table: &str, _row: &Value) -> Result<(), RowError> {
                let mut calls = self.calls.lock().unwrap();
                calls.push(table.to_string());
                if table == TABLE_SESSIONS && calls.len() == 1 {
                    return Err(RowError::Other("transient".into()));
                }
                Ok(())
            }
        }
        let sink = Arc::new(FlakySink {
            calls: Mutex::new(vec![]),
        });
        let s2 = sink.clone();
        let factory: SinkFactory = Arc::new(move |_| Some(s2.clone() as Arc<dyn RowSink>));
        let rec = SessionRecorder::new("agent", None, Some("c".into()), factory);
        rec.start();
        assert!(rec.flush(Duration::from_secs(5)));
        let calls = sink.calls.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            [TABLE_SESSIONS, TABLE_SESSIONS, TABLE_EVENTS]
        );
    }
}
