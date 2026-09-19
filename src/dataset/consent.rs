//! Persisted answer to the dataset consent question.
//!
//! Consent is per install, not per project: one answer, stored in
//! `$ABLETON_MCP_STATE_DIR/consent.json` (default `~/.ableton-music-maker`).
//! Recording is opt-in: only an explicit yes turns it on, and
//! `ABLETON_MCP_DISABLE_DATASET` overrides any stored answer.

use crate::env_flag;
use rmcp::service::ElicitationError;
use rmcp::{Peer, RoleServer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

pub const UNKNOWN: &str = "unknown";
pub const GRANTED: &str = "granted";
pub const DENIED: &str = "denied";

pub fn state_dir() -> PathBuf {
    let explicit = crate::env_str("ABLETON_MCP_STATE_DIR");
    if !explicit.is_empty() {
        return PathBuf::from(explicit);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ableton-music-maker")
}

fn state_file() -> PathBuf {
    state_dir().join("consent.json")
}

/// In-memory copy of the state file, keyed by its path so tests that point
/// `ABLETON_MCP_STATE_DIR` elsewhere never see a stale cache.
static CACHE: Mutex<Option<(PathBuf, Map<String, Value>)>> = Mutex::new(None);

fn read_state() -> Map<String, Value> {
    let path = state_file();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((cached_path, data)) = cache.as_ref() {
        if *cached_path == path {
            return data.clone();
        }
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            _ => {
                // A damaged file must not wedge the server: treat as unasked.
                tracing::debug!("Consent state unreadable; treating as unasked");
                Map::new()
            }
        },
        Err(_) => Map::new(),
    };
    *cache = Some((path, data.clone()));
    data
}

fn write_state(state: Map<String, Value>) {
    let path = state_file();
    let result = std::fs::create_dir_all(state_dir()).and_then(|_| {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&Value::Object(state.clone())).unwrap_or_default(),
        )?;
        std::fs::rename(&tmp, &path)
    });
    if let Err(e) = result {
        // Losing the answer means re-asking next session: annoying, not fatal.
        tracing::warn!("Could not persist dataset consent: {}", e);
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((path, state));
}

/// UNKNOWN / GRANTED / DENIED, honouring env overrides. The kill switch wins
/// outright; the env opt-in counts as a grant for headless setups.
pub fn consent_state() -> &'static str {
    if env_flag("ABLETON_MCP_DISABLE_DATASET") {
        return DENIED;
    }
    if env_flag("ABLETON_MCP_ENABLE_DATASET") {
        return GRANTED;
    }
    match read_state().get("state").and_then(Value::as_str) {
        Some(GRANTED) => GRANTED,
        Some(DENIED) => DENIED,
        _ => UNKNOWN,
    }
}

/// True only after an explicit yes.
pub fn recording_allowed() -> bool {
    consent_state() == GRANTED
}

/// Persist the user's answer. `quote` is what they actually said: consent
/// relayed through a model is only as good as the model's faithfulness, so
/// keep the evidence.
pub fn record_consent(granted: bool, quote: Option<&str>) -> &'static str {
    let state = if granted { GRANTED } else { DENIED };
    let mut payload = read_state();
    payload.insert("state".into(), json!(state));
    payload.insert("answered_at".into(), json!(crate::telemetry::now_secs()));
    let said: String = quote.unwrap_or("").trim().chars().take(500).collect();
    payload.insert(
        "user_said".into(),
        if said.is_empty() {
            Value::Null
        } else {
            json!(said)
        },
    );
    write_state(payload);
    tracing::info!("Dataset consent recorded: {}", state);
    state
}

/// True when the user has never been asked and an answer would matter. With
/// telemetry off (the default) a yes could not start recording, so the
/// question is not asked at all.
pub fn needs_prompt() -> bool {
    consent_state() == UNKNOWN && crate::telemetry::is_telemetry_enabled()
}

/// Note that the question has been surfaced, so it is asked once per session.
pub fn mark_prompted() {
    let mut payload = read_state();
    payload.insert(
        "last_prompted_at".into(),
        json!(crate::telemetry::now_secs()),
    );
    write_state(payload);
}

pub const CONSENT_NOTICE: &str = "\n\n---\n\
[Ask the user this now, before continuing. Do not answer it yourself, and do not assume — relay their reply verbatim.]\n\n\
**Would you like to contribute this session to the open Ableton Music Maker training dataset?**\n\n\
Recording is off by default and nothing has been uploaded. If you say yes, what you make here — your prompts, MIDI notes, track and clip names, and device settings — is uploaded and may be published as part of an open dataset used to train music-production models. Email addresses and file paths are stripped first.\n\n\
Reply **yes** to contribute, or **no** to leave recording off. You will only be asked once; you can change your mind later by saying so.\n\
---";

pub const ELICIT_MESSAGE: &str = "Contribute this session to the open Ableton Music Maker training dataset?\n\n\
Recording is off by default and nothing has been uploaded. If you agree, what you make here — your prompts, MIDI notes, track and clip names, and device settings — is uploaded and may be published as part of an open dataset used to train music-production models. Email addresses and file paths are stripped first.\n\n\
Decline to leave recording off. You are asked once, and can change your mind later.";

/// The form shown by clients that support elicitation.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DatasetConsent {
    /// Yes, contribute my sessions to the open dataset
    pub contribute: bool,
}
rmcp::elicit_safe!(DatasetConsent);

/// Ask for consent through a real client dialog. Returns the resulting state,
/// or None when elicitation is unavailable (the caller then falls back to the
/// text notice). A user who cancels or declines the dialog is a real answer.
pub async fn try_elicit_consent(peer: &Peer<RoleServer>) -> Option<&'static str> {
    if !needs_prompt() {
        return None;
    }
    match peer.elicit::<DatasetConsent>(ELICIT_MESSAGE).await {
        Ok(Some(answer)) => Some(record_consent(
            answer.contribute,
            Some("(via client dialog)"),
        )),
        Ok(None) => Some(record_consent(
            false,
            Some("(via client dialog, no content)"),
        )),
        Err(ElicitationError::UserDeclined) => {
            Some(record_consent(false, Some("(declined in client dialog)")))
        }
        Err(ElicitationError::UserCancelled) => {
            // Dismissed without answering: left UNKNOWN so it can be asked in a
            // later session. Nothing is recorded in the meantime.
            tracing::debug!("Consent dialog dismissed without an answer — recording stays off");
            mark_prompted();
            Some(UNKNOWN)
        }
        Err(e) => {
            tracing::debug!("Elicitation unavailable ({}); falling back to text", e);
            None
        }
    }
}

/// The consent question to append to a tool result, or "" once the user has
/// answered or was already asked within the hour.
pub fn maybe_consent_notice() -> &'static str {
    if !needs_prompt() {
        return "";
    }
    let last = read_state()
        .get("last_prompted_at")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if crate::telemetry::now_secs() - last < 3600.0 {
        return "";
    }
    mark_prompted();
    CONSENT_NOTICE
}

pub fn reset_for_tests() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}
