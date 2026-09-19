//! Fetch, scrub and hash Ableton session snapshots.

use crate::connection::LiveBridge;
use crate::dataset::schema::stable_hash;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Names users never wrote: Live's own defaults plus generic musical labels
/// that carry training signal without identifying anyone. Anything else is
/// treated as user-authored free text and redacted.
fn safe_name_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(
            r"(?i)^((audio|midi|return) ?track( \d+)?|track ?\d*|[a-z]-return|\d+-(audio|midi)|master|return|scene( \d+)?|clip( \d+)?|kick|snare|hat|hi-?hat|clap|tom|crash|ride|perc(ussion)?|drums?|bass|sub|lead|pad|keys|piano|synth|pluck|arp|guitar|vocal(s)?|vox|strings|brass|fx|riser|sweep|noise|chord(s)?|melody|harmony|loop|sample|intro|verse|chorus|bridge|drop|outro|break(down)?)$",
        )
        .expect("valid regex")
    })
}

const NAME_KEYS: &[&str] = &[
    "name",
    "clip_name",
    "track_name",
    "scene_name",
    "chain_name",
];

/// Keep Live defaults and generic musical labels; redact anything else,
/// preserving length as a coarse signal.
pub fn scrub_name(value: &Value) -> Value {
    let Value::String(s) = value else {
        return value.clone();
    };
    let stripped = s.trim();
    if stripped.is_empty() || safe_name_re().is_match(stripped) {
        return value.clone();
    }
    json!(format!("<name:{}>", stripped.chars().count()))
}

/// Recursively redact user-authored names in a snapshot tree.
pub fn scrub_snapshot(node: &Value) -> Value {
    match node {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let scrubbed = if NAME_KEYS.contains(&k.as_str()) {
                        scrub_name(v)
                    } else {
                        scrub_snapshot(v)
                    };
                    (k.clone(), scrubbed)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(scrub_snapshot).collect()),
        other => other.clone(),
    }
}

/// Fetch a session snapshot via the Ableton bridge. Scrubbed here, at the
/// single point where snapshots enter the dataset pipeline. None on failure.
pub fn fetch_snapshot(
    bridge: &dyn LiveBridge,
    include_notes: bool,
    include_params: bool,
) -> Option<Value> {
    match bridge.send_command(
        "get_session_snapshot",
        Some(json!({"include_notes": include_notes, "include_params": include_params})),
    ) {
        Ok(Value::Null) => None,
        Ok(raw) => Some(scrub_snapshot(&raw)),
        Err(e) => {
            tracing::warn!("Failed to fetch session snapshot: {}", e);
            None
        }
    }
}

pub fn snapshot_hash(snapshot: Option<&Value>) -> Option<String> {
    snapshot.map(stable_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_only_user_authored_names() {
        assert_eq!(scrub_name(&json!("Audio Track 3")), json!("Audio Track 3"));
        assert_eq!(scrub_name(&json!("Kick")), json!("Kick"));
        assert_eq!(scrub_name(&json!("1-MIDI")), json!("1-MIDI"));
        assert_eq!(scrub_name(&json!("chorus for Sarah")), json!("<name:16>"));
        assert_eq!(scrub_name(&json!(7)), json!(7));
        let tree = json!({"tracks": [{"name": "Secret Song", "clips": [{"clip_name": "Verse", "notes": []}]}]});
        let out = scrub_snapshot(&tree);
        assert_eq!(out["tracks"][0]["name"], "<name:11>");
        assert_eq!(out["tracks"][0]["clips"][0]["clip_name"], "Verse");
    }
}
