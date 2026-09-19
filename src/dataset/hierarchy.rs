//! Heuristic hierarchical labels (Level 1–5) for trajectory events.

use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

const TOOL_LEVELS: &[(&str, i64, &str)] = &[
    ("set_tempo", 1, "adjust_tempo"),
    ("set_device_parameter", 1, "adjust_parameter"),
    ("set_track_name", 1, "rename_track"),
    ("set_clip_name", 1, "rename_clip"),
    ("set_arrangement_clip_name", 1, "rename_clip"),
    ("fire_clip", 1, "trigger_clip"),
    ("stop_clip", 1, "stop_clip"),
    ("start_playback", 1, "transport"),
    ("stop_playback", 1, "transport"),
    ("set_arrangement_time", 1, "move_playhead"),
    ("switch_to_arrangement_view", 1, "change_view"),
    ("create_midi_track", 2, "create_track"),
    ("create_audio_track", 2, "create_track"),
    ("create_clip", 2, "create_clip"),
    ("create_audio_clip", 2, "create_clip"),
    ("add_notes_to_clip", 2, "edit_midi"),
    ("load_instrument_or_effect", 2, "load_device"),
    ("load_drum_kit", 2, "load_device"),
    ("duplicate_to_arrangement", 3, "arrange_section"),
];

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

fn section_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    re(
        &CELL,
        r"(?i)\b(intro|verse|chorus|drop|bridge|outro|build|breakdown|pre[- ]?chorus|hook)\b",
    )
}

fn song_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    re(
        &CELL,
        r"(?i)\b(whole (song|track)|full (song|track)|finish (the )?(song|track|arrangement)|complete (the )?(song|track)|change genre|restyle)\b",
    )
}

fn aesthetic_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    re(
        &CELL,
        r"(?i)\b(darker|brighter|warmer|colder|nostalgic|energetic|aggressive|chill|emotional|dreamy|gritty|clean|human|robotic|like .+)\b",
    )
}

fn mix_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    re(
        &CELL,
        r"(?i)\b(mix|muddy|loudness|sidechain|compress|eq|reverb|space|sit in the mix)\b",
    )
}

fn atomic_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    re(
        &CELL,
        r"(?i)\b(set|change|adjust|rename|nudge|turn (up|down)|increase|decrease)\b.{0,40}\b(tempo|bpm|volume|pan|name|level|value|parameter|knob)\b",
    )
}

fn musical_op_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    re(
        &CELL,
        r"(?i)\b(add|write|create|make|load|duplicate|delete|remove|play|insert)\b.{0,40}\b(clip|track|note|midi|melody|bassline|chord|drum|kit|beat|loop|instrument|effect|preset|sample|pattern)\b",
    )
}

/// Infer Level 1–5 from intent text (an explicit level in range wins).
pub fn infer_intent_level(text: &str, explicit_level: Option<i64>) -> i64 {
    if let Some(l) = explicit_level {
        if (1..=5).contains(&l) {
            return l;
        }
    }
    if text.is_empty() {
        return 5;
    }
    if song_re().is_match(text) {
        return 4;
    }
    if section_re().is_match(text) {
        return 3;
    }
    if atomic_re().is_match(text) {
        return 1;
    }
    if musical_op_re().is_match(text) {
        return 2;
    }
    5
}

pub fn infer_intent_op_label(text: &str, level: i64) -> String {
    if text.is_empty() {
        return "creative_goal".to_string();
    }
    if let Some(m) = section_re().captures(text) {
        let section = m[1].to_lowercase().replace(' ', "_");
        let lower = text.to_lowercase();
        if lower.contains("bigger") || lower.contains("larger") {
            return format!("make_{section}_bigger");
        }
        if lower.contains("smaller") || lower.contains("quieter") {
            return format!("make_{section}_smaller");
        }
        return format!("edit_{section}");
    }
    if song_re().is_match(text) {
        return "song_level_change".to_string();
    }
    if aesthetic_re().is_match(text) {
        return "aesthetic_shift".to_string();
    }
    if mix_re().is_match(text) {
        return "mix_improve".to_string();
    }
    if level <= 2 {
        return "musical_operation".to_string();
    }
    "creative_goal".to_string()
}

/// `{op_level, op_label[, source]}` for an MCP tool action.
pub fn label_action(tool: &str, params: Option<&Map<String, Value>>) -> Map<String, Value> {
    let mut out = Map::new();
    if tool.starts_with("human_") {
        out.insert("op_level".into(), json!(1));
        out.insert("op_label".into(), json!(tool));
        out.insert("source".into(), json!("live_ui"));
        return out;
    }
    let (mut level, mut label) = TOOL_LEVELS
        .iter()
        .find(|(name, _, _)| *name == tool)
        .map(|(_, level, label)| (*level, label.to_string()))
        .unwrap_or((1, tool.to_string()));

    if tool == "add_notes_to_clip" {
        let count = params.and_then(|p| {
            p.get("notes_count").and_then(Value::as_i64).or_else(|| {
                p.get("notes")
                    .and_then(Value::as_array)
                    .map(|a| a.len() as i64)
            })
        });
        if count.is_some_and(|c| c >= 8) {
            label = "write_midi_phrase".to_string();
        }
    }
    let _ = &mut level;
    out.insert("op_level".into(), json!(level));
    out.insert("op_label".into(), json!(label));
    out
}

pub struct IntentLabels {
    pub op_level: i64,
    pub op_label: String,
    pub intent_level: i64,
}

pub fn label_intent(text: &str, level: Option<i64>) -> IntentLabels {
    let resolved = infer_intent_level(text, level);
    IntentLabels {
        op_level: resolved,
        op_label: infer_intent_op_label(text, resolved),
        intent_level: resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_specificity() {
        assert_eq!(infer_intent_level("finish the song", None), 4);
        assert_eq!(infer_intent_level("make the chorus feel bigger", None), 3);
        assert_eq!(infer_intent_level("set the tempo to 120", None), 1);
        assert_eq!(infer_intent_level("add a bassline clip", None), 2);
        assert_eq!(infer_intent_level("make it darker", None), 5);
        assert_eq!(infer_intent_level("anything", Some(2)), 2);
        assert_eq!(infer_intent_level("anything", Some(9)), 5);
    }

    #[test]
    fn labels() {
        assert_eq!(
            infer_intent_op_label("make the chorus feel bigger", 3),
            "make_chorus_bigger"
        );
        assert_eq!(
            infer_intent_op_label("make it darker", 5),
            "aesthetic_shift"
        );
        assert_eq!(infer_intent_op_label("fix the muddy mix", 5), "mix_improve");
        assert_eq!(infer_intent_op_label("add a clip", 2), "musical_operation");
        let l = label_action(
            "add_notes_to_clip",
            Some(
                &serde_json::json!({"notes_count": 12})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        );
        assert_eq!(l["op_label"], "write_midi_phrase");
        assert_eq!(l["op_level"], 2);
        assert_eq!(
            label_action("human_tracks_changed", None)["source"],
            "live_ui"
        );
    }
}
