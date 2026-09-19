//! Canonical schema for the Ableton Music Maker music-creation trajectory datasets.
//!
//! Training unit: (intent?, S_t, action, S_{t+1}, preference?)

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: i64 = 2;

pub mod event_type {
    pub const SESSION_START: &str = "session_start";
    pub const SESSION_END: &str = "session_end";
    pub const INTENT: &str = "intent";
    pub const ACTION: &str = "action";
    pub const STATE: &str = "state";
    pub const PREFERENCE: &str = "preference";
    pub const AUDITION: &str = "audition";
}

pub mod collection_mode {
    pub const AGENT: &str = "agent";
    pub const ASSISTED: &str = "assisted";
    pub const PASSIVE: &str = "passive";
}

pub mod preference_source {
    pub const HUMAN: &str = "human";
    pub const IMPLICIT: &str = "implicit";
}

pub const PREFERENCE_RATINGS: &[&str] = &[
    "better",
    "same",
    "worse",
    "keep",
    "reject",
    "thumbs_up",
    "thumbs_down",
    "pairwise",
];

const ACTION_CATEGORIES: &[(&str, &str)] = &[
    ("create_midi_track", "create"),
    ("create_audio_track", "create"),
    ("create_clip", "create"),
    ("create_audio_clip", "create"),
    ("set_track_name", "edit"),
    ("set_clip_name", "edit"),
    ("set_arrangement_clip_name", "edit"),
    ("add_notes_to_clip", "edit_midi"),
    ("set_tempo", "mix"),
    ("set_device_parameter", "mix"),
    ("fire_clip", "transport"),
    ("stop_clip", "transport"),
    ("start_playback", "transport"),
    ("stop_playback", "transport"),
    ("set_arrangement_time", "transport"),
    ("switch_to_arrangement_view", "transport"),
    ("duplicate_to_arrangement", "arrangement"),
    ("load_instrument_or_effect", "browse"),
    ("load_drum_kit", "browse"),
];

pub fn new_id(prefix: &str) -> String {
    let uid: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(12)
        .collect();
    if prefix.is_empty() {
        uid
    } else {
        format!("{prefix}_{uid}")
    }
}

pub fn now_ts() -> f64 {
    crate::telemetry::now_secs()
}

fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.clone(), sort_keys(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Canonical SHA-256 (first 16 hex chars) of a JSON value, for state
/// fingerprints. Key order does not affect the hash.
pub fn stable_hash(value: &Value) -> String {
    let canonical = serde_json::to_string(&sort_keys(value)).unwrap_or_default();
    let digest = Sha256::digest(canonical.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex.chars().take(16).collect()
}

pub fn categorize_tool(tool_name: &str) -> &'static str {
    if tool_name.starts_with("human_") {
        return "passive";
    }
    ACTION_CATEGORIES
        .iter()
        .find(|(name, _)| *name == tool_name)
        .map(|(_, category)| *category)
        .unwrap_or("other")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub started_at: f64,
    pub mode: String,
    pub schema_version: i64,
    pub ableton_mcp_version: Option<String>,
    pub notes: Option<String>,
    pub active_intent_id: Option<String>,
    pub ended_at: Option<f64>,
}

/// One trajectory event. Only the fields relevant to its `kind` are set.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrajectoryEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub session_id: String,
    pub ts: f64,
    pub v: i64,
    pub event_id: String,
    /// Monotonic position of this event within its session (1-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_path: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_a: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_b: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dwell_ms: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
}

impl TrajectoryEvent {
    pub fn new(kind: &str, session_id: &str) -> Self {
        Self {
            kind: kind.to_string(),
            session_id: session_id.to_string(),
            ts: now_ts(),
            v: SCHEMA_VERSION,
            event_id: new_id("evt"),
            ..Default::default()
        }
    }

    pub fn extra_mut(&mut self) -> &mut Map<String, Value> {
        self.extra.get_or_insert_with(Map::new)
    }
}

pub fn make_session_start(session_id: &str, mode: &str) -> TrajectoryEvent {
    let mut ev = TrajectoryEvent::new(event_type::SESSION_START, session_id);
    ev.extra_mut()
        .insert("mode".into(), Value::String(mode.to_string()));
    ev
}

pub fn make_intent(session_id: &str, text: &str, level: Option<i64>) -> TrajectoryEvent {
    let mut ev = TrajectoryEvent::new(event_type::INTENT, session_id);
    ev.intent_id = Some(new_id("intent"));
    ev.text = Some(text.to_string());
    ev.level = level;
    ev
}

#[derive(Default)]
pub struct ActionSpec<'a> {
    pub tool: &'a str,
    pub params: Option<Map<String, Value>>,
    pub pre_hash: Option<String>,
    pub post_hash: Option<String>,
    pub success: bool,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
    pub intent_id: Option<String>,
    pub text: Option<String>,
    pub level: Option<i64>,
}

pub fn make_action(session_id: &str, spec: ActionSpec<'_>) -> TrajectoryEvent {
    let mut ev = TrajectoryEvent::new(event_type::ACTION, session_id);
    ev.action_id = Some(new_id("act"));
    ev.tool = Some(spec.tool.to_string());
    ev.category = Some(categorize_tool(spec.tool).to_string());
    ev.params = spec.params;
    ev.pre_hash = spec.pre_hash;
    ev.post_hash = spec.post_hash;
    ev.success = Some(spec.success);
    ev.duration_ms = spec.duration_ms;
    ev.error = spec.error;
    ev.intent_id = spec.intent_id;
    ev.text = spec.text;
    ev.level = spec.level;
    ev
}

pub fn make_state(
    session_id: &str,
    state_hash: &str,
    state_path: &str,
    intent_id: Option<String>,
) -> TrajectoryEvent {
    let mut ev = TrajectoryEvent::new(event_type::STATE, session_id);
    ev.state_hash = Some(state_hash.to_string());
    ev.state_path = Some(state_path.to_string());
    ev.intent_id = intent_id;
    ev
}

#[derive(Default)]
pub struct PreferenceSpec<'a> {
    pub rating: &'a str,
    pub target_action_id: Option<String>,
    pub tags: Option<Vec<String>>,
    pub note: Option<String>,
    pub winner: Option<String>,
    pub candidate_a: Option<String>,
    pub candidate_b: Option<String>,
    pub intent_id: Option<String>,
    pub rating_source: Option<&'a str>,
}

pub fn make_preference(session_id: &str, spec: PreferenceSpec<'_>) -> TrajectoryEvent {
    let mut ev = TrajectoryEvent::new(event_type::PREFERENCE, session_id);
    ev.rating = Some(spec.rating.to_string());
    ev.rating_source = Some(
        spec.rating_source
            .unwrap_or(preference_source::HUMAN)
            .to_string(),
    );
    ev.target_action_id = spec.target_action_id;
    ev.tags = spec.tags;
    ev.note = spec.note;
    ev.winner = spec.winner;
    ev.candidate_a = spec.candidate_a;
    ev.candidate_b = spec.candidate_b;
    ev.intent_id = spec.intent_id;
    ev
}

pub fn make_audition(
    session_id: &str,
    uri: &str,
    kept: Option<bool>,
    search_query: Option<String>,
    dwell_ms: Option<f64>,
    intent_id: Option<String>,
) -> TrajectoryEvent {
    let mut ev = TrajectoryEvent::new(event_type::AUDITION, session_id);
    ev.uri = Some(uri.to_string());
    ev.kept = kept;
    ev.search_query = search_query;
    ev.dwell_ms = dwell_ms;
    ev.intent_id = intent_id;
    ev
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_is_order_independent_and_short() {
        let a = stable_hash(&json!({"x": 1, "y": [{"b": 2, "a": 1}]}));
        let b = stable_hash(&json!({"y": [{"a": 1, "b": 2}], "x": 1}));
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert_ne!(a, stable_hash(&json!({"x": 2})));
    }

    #[test]
    fn categories_and_ids() {
        assert_eq!(categorize_tool("add_notes_to_clip"), "edit_midi");
        assert_eq!(categorize_tool("human_clip_slot_changed"), "passive");
        assert_eq!(categorize_tool("get_session_info"), "other");
        assert!(new_id("act").starts_with("act_"));
        assert_eq!(new_id("").len(), 12);
    }

    #[test]
    fn events_omit_unset_fields() {
        let ev = make_intent("sess_1", "make it darker", Some(5));
        let row = serde_json::to_value(&ev).unwrap();
        assert_eq!(row["type"], "intent");
        assert!(row.get("action_id").is_none());
        assert_eq!(row["level"], 5);
    }
}
