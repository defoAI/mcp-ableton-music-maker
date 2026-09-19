//! Feel: a Groove Pool groove on a clip (Live's own, through the script),
//! the global groove amount, and the note rewrites humanize and swing_notes
//! with their undo, through the real bodies with a scripted bridge.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::tools::{
    self, GrooveAmountParams, GrooveClipParams, HumanizeParams, SwingNotesParams, UndoVaryParams,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn state() -> Value {
    json!({
        "is_playing": true, "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
        "beat": 53.5, "bar": 14, "beat_in_bar": 2, "clip_trigger_quantization": 4,
        "tracks": [
            {"index": 0, "name": "Kick", "playing_slot_index": 1, "slots_with_clips": [0, 1]},
            {"index": 1, "name": "Congas", "playing_slot_index": 1, "slots_with_clips": [1]}
        ],
        "scenes": [{"index": 0, "name": "Intro · 8", "clip_tracks": [0]}, {"index": 1, "name": "Groove · 8", "clip_tracks": [0, 1], "is_playing": true}],
        "cues": [], "events": []
    })
}

fn sixteenths() -> Value {
    json!({"notes": (0..16).map(|i| json!({"pitch": 60 + (i % 3), "start_time": i as f64 * 0.25, "duration": 0.2, "velocity": 100, "mute": false})).collect::<Vec<_>>()})
}

fn bridge() -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script("get_performance_state", vec![state()]);
    b.script("get_clip_notes", vec![sixteenths()]);
    b.script(
        "get_grooves",
        vec![json!({"groove_amount": 1.0, "can_add": false, "pool_functions": [],
            "grooves": [{"index": 0, "name": "Swing 16", "timing_amount": 0.6, "random_amount": 0.0, "velocity_amount": 0.2},
                        {"index": 1, "name": "MPC 16 Swing-62", "timing_amount": 1.0}]})],
    );
    b.script(
        "set_clip_groove",
        vec![json!({"track": "Congas", "clip": "Congas/Groove", "groove": {"index": 0, "name": "Swing 16", "timing_amount": 0.6, "random_amount": 0.0, "velocity_amount": 0.2}})],
    );
    b
}

#[tokio::test]
async fn groove_clip_assigns_a_pool_groove_by_name_and_names_the_pool_when_it_cannot() {
    let b = bridge();
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::GROOVE_CLIP,
            GrooveClipParams {
                track: json!("Congas"),
                clip: 1,
                groove: json!("swing 16"),
                amount: Some(0.6),
                random: None,
                velocity: None,
            },
            tools::groove_clip_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec!["get_performance_state", "get_grooves", "set_clip_groove"]
    );
    assert_eq!(
        b.sent()[2].1,
        json!({"track_index": 1, "clip_index": 1, "groove_index": 0, "timing": 0.6, "random": null, "velocity": null})
    );
    assert_eq!(
        text_of(&r),
        "Congas slot 1: groove 'Swing 16' from the Groove Pool at timing 60%, random 0%, velocity 20% (non-destructive; Live's own groove, shared by every clip that uses it). Set the pool's global amount with groove_amount (now 100%)."
    );

    // A name the pool lacks: the pool is listed and the API's limit is said.
    let r = server
        .run(
            &tools::GROOVE_CLIP,
            GrooveClipParams {
                track: json!("Congas"),
                clip: 1,
                groove: json!("Shuffle 8"),
                amount: None,
                random: None,
                velocity: None,
            },
            tools::groove_clip_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert_eq!(
        text_of(&r),
        "No groove named 'Shuffle 8' in this set's Groove Pool and the API cannot add one; drag one in from the browser (Grooves) so it appears in the pool, or use humanize / swing_notes (a note rewrite, undoable). In the pool: Swing 16, MPC 16 Swing-62."
    );
    // An empty pool.
    b.script(
        "get_grooves",
        vec![json!({"groove_amount": 1.0, "grooves": []})],
    );
    let r = server
        .run(
            &tools::GROOVE_CLIP,
            GrooveClipParams {
                track: json!("Congas"),
                clip: 1,
                groove: json!("Swing 16"),
                amount: None,
                random: None,
                velocity: None,
            },
            tools::groove_clip_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).starts_with("No groove named 'Swing 16' in this set's Groove Pool (it is empty) and the API cannot add one"), "{}", text_of(&r));
    // "none" removes it; a slot without a clip is refused before the pool is read.
    b.script(
        "set_clip_groove",
        vec![json!({"track": "Congas", "clip": "x", "groove": null})],
    );
    let r = server
        .run(
            &tools::GROOVE_CLIP,
            GrooveClipParams {
                track: json!("Congas"),
                clip: 1,
                groove: json!("none"),
                amount: None,
                random: None,
                velocity: None,
            },
            tools::groove_clip_body,
        )
        .await;
    assert!(
        !is_error(&r)
            && text_of(&r) == "Congas slot 1: groove removed; the clip plays straight again.",
        "{}",
        text_of(&r)
    );
    assert_eq!(b.sent().last().unwrap().1["groove_index"], -1);
    let before = b.commands().len();
    let r = server
        .run(
            &tools::GROOVE_CLIP,
            GrooveClipParams {
                track: json!("Congas"),
                clip: 0,
                groove: json!("Swing 16"),
                amount: None,
                random: None,
                velocity: None,
            },
            tools::groove_clip_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("slot 0 on 'Congas' holds no clip"),
        "{}",
        text_of(&r)
    );
    assert_eq!(b.commands().len(), before + 1);

    // The global amount is one command.
    b.script("set_clip_groove", vec![json!({"groove_amount": 0.5})]);
    let r = server
        .run(
            &tools::GROOVE_AMOUNT,
            GrooveAmountParams { value: 0.5 },
            tools::groove_amount_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.sent().last().unwrap().1, json!({"global_amount": 0.5}));
    assert_eq!(
        text_of(&r),
        "Groove Pool amount 50%: every clip with a groove follows it that much."
    );
}

#[tokio::test]
async fn humanize_and_swing_rewrite_notes_describe_the_feel_and_undo() {
    let b = bridge();
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::HUMANIZE,
            HumanizeParams {
                track: json!("Congas"),
                clip: 1,
                timing_ms: 12.0,
                velocity: 15,
                seed: 3,
            },
            tools::humanize_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec![
            "get_performance_state",
            "get_clip_notes",
            "clear_notes_from_clip",
            "add_notes_to_clip"
        ]
    );
    let written = b.sent()[3].1["notes"].as_array().unwrap().clone();
    assert_eq!(written.len(), 16, "a rewrite, not a note count change");
    assert!(
        written
            .iter()
            .any(|n| (n["start_time"].as_f64().unwrap() * 4.0).fract() != 0.0),
        "something left the grid"
    );
    assert!(
        written
            .iter()
            .all(|n| n["start_time"].as_f64().unwrap() < 4.0),
        "nothing crossed the bar line"
    );
    let t = text_of(&r);
    // 12 ms at 126 BPM is 0.0252 beats: a little under a 128th (0.03125).
    assert_eq!(t, "Congas slot 1 humanized (seed 3): hits now land up to 12 ms early or late — a little under a 128th at 126 BPM — so they drift around the grid instead of sitting on it; velocities vary ±15, the off-beat notes most. undo_vary puts them back.");
    assert!(!t.contains("16 notes"), "no note counts");

    // undo_vary restores the notes read before the rewrite.
    let r = server
        .run(
            &tools::UNDO_VARY,
            UndoVaryParams {
                track: json!("Congas"),
                clip: 1,
            },
            tools::undo_vary_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let restored = b.sent().last().unwrap().1["notes"].clone();
    assert_eq!(restored, sixteenths()["notes"]);

    // swing_notes: odd 16ths delayed by half a step, the reply in note values.
    let r = server
        .run(
            &tools::SWING_NOTES,
            SwingNotesParams {
                track: json!("Congas"),
                clip: 1,
                amount: 0.5,
                grid: "1/16".into(),
                seed: 1,
            },
            tools::swing_notes_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let written = b.sent().last().unwrap().1["notes"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(written[1]["start_time"], 0.375);
    assert_eq!(written[2]["start_time"], 0.5);
    assert_eq!(text_of(&r), "Congas slot 1 swung: the off-beat 16ths now lag by about a 32nd (50% of the step); the on-beat notes stay where they were. undo_vary straightens them.");
    let r = server
        .run(
            &tools::SWING_NOTES,
            SwingNotesParams {
                track: json!("Congas"),
                clip: 1,
                amount: 1.5,
                grid: "1/16".into(),
                seed: 1,
            },
            tools::swing_notes_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("amount must be between 0 and 1"),
        "{}",
        text_of(&r)
    );
}
