//! Sections and songs through the real tool bodies with a scripted bridge:
//! the scene-name convention, the Setlist: scene, play_song's one cue, the
//! phrase rule on every steering verb, "cutting N bars", the jump history,
//! the level line and the 3 dB guard, make_section's three sources, and
//! that steering never costs more than one state read and one cue. The
//! script's own behaviour (capture-and-insert, restored phrases after a
//! Live restart) is the manual pass in the story.

mod common;

use chrono::Local;
use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::connection::Performance;
use mcp_ableton_music_maker::sections::{
    AddToSongParams, JumpToParams, MakeSectionParams, PlaySongParams, RemoveFromSongParams,
    SetSongParams, SteerParams,
};
use mcp_ableton_music_maker::song::{JumpFrom, SetlistEntry, Song};
use mcp_ableton_music_maker::tools::{self, Empty, GetContextParams, GetPerformanceStateParams};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

const SETLIST: &str = "Setlist: Intro×2 → Groove → Groove+Pad → Break → Drop → Groove";

/// The prototype's set: five tracks, six sections, the Setlist: scene at
/// the bottom. `playing` is the scene row that plays; `phrase` says since
/// which bar.
fn state(bar: i64, beat_in_bar: i64, playing: Option<(i64, i64)>, with_setlist: bool) -> Value {
    let beat = ((bar - 1) * 4 + (beat_in_bar - 1)) as f64 + 0.5;
    let rows: Vec<(&str, Vec<i64>)> = vec![
        ("Intro · 8", vec![0, 1]),
        ("Groove · 8", vec![0, 1, 2]),
        ("Groove+Pad · 8", vec![0, 1, 2, 3]),
        ("Break · 16", vec![3, 4]),
        ("Drop · 8", vec![0, 1, 2, 3, 4]),
        ("Peak · 8", vec![0, 1, 2, 3, 4]),
        ("Outro", vec![]),
    ];
    let row = playing.map(|(r, _)| r).unwrap_or(-1);
    let tracks: Vec<Value> = ["Kick", "Hats+Perc", "Bass", "Pad", "Lead"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let slots: Vec<i64> = rows
                .iter()
                .enumerate()
                .filter(|(_, (_, t))| t.contains(&(i as i64)))
                .map(|(s, _)| s as i64)
                .collect();
            let plays = row >= 0 && slots.contains(&row);
            json!({"index": i, "name": name, "playing_slot_index": if plays { row } else { -1 },
                   "playing_clip_name": if plays { Value::from(format!("{}/{}", rows[row as usize].0, name)) } else { Value::Null },
                   "slots_with_clips": slots})
        })
        .collect();
    let mut scenes: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, (name, t))| {
            json!({"index": i, "name": name, "clip_tracks": t, "is_playing": i as i64 == row,
                   "phrase_bars": mcp_ableton_music_maker::song::parse_section_name(name).1.unwrap_or(16),
                   "phrase_default": mcp_ableton_music_maker::song::parse_section_name(name).1.is_none()})
        })
        .collect();
    if with_setlist {
        scenes.push(json!({"index": 7, "name": SETLIST, "clip_tracks": []}));
    }
    let phrase = playing.map(|(r, started)| {
        let bars = mcp_ableton_music_maker::song::parse_section_name(rows[r as usize].0).1.unwrap_or(16);
        let k = ((bar - started) / bars + 1).max(1);
        json!({"scene_index": r, "started_bar": started, "bars": bars, "ends_bar": started + k * bars, "default": false})
    });
    json!({
        "is_playing": playing.is_some(), "tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4,
        "beat": beat, "bar": bar, "beat_in_bar": beat_in_bar,
        "clip_trigger_quantization": 4, "clip_trigger_quantization_name": "1_bar",
        "scale_name": "Minor", "root_note": 5, "root_note_name": "F",
        "tracks": tracks, "scenes": scenes, "cues": [], "events": [],
        "phrase": phrase, "current_scene": playing.map(|(r, _)| r),
        "levels": {"bar": bar, "master_peak_db": -3.8,
                   "tracks": [{"index": 0, "name": "Kick", "peak_db": -6.2}, {"index": 2, "name": "Bass", "peak_db": -7.0}, {"index": 3, "name": "Pad", "peak_db": -45.0}],
                   "section_peaks": {"1": -4.0, "2": -3.8, "4": -0.5, "3": -12.0}}
    })
}

fn bridge() -> Arc<FakeBridge> {
    let b = FakeBridge::responding(json!({}));
    b.script(
        "set_launch_quantization",
        vec![json!({"clip_trigger_quantization": 4, "name": "1_bar"})],
    );
    b.script("fire_scene", vec![json!({"fired": true, "scene_index": 0, "name": "Intro · 8", "clips": [{"track": "Kick"}, {"track": "Hats+Perc"}], "off_grid_clips": [], "would_record": [], "lands_on_bar": 1})]);
    b.script(
        "schedule_cue",
        vec![json!({"id": 1, "name": "song", "steps": [], "replaced": []})],
    );
    b.script(
        "cancel_cue",
        vec![json!({"cancelled": [5], "reason": "hold"})],
    );
    b.script(
        "create_scene",
        vec![json!({"index": 7, "name": SETLIST, "scene_count": 8})],
    );
    b.script("set_scene", vec![json!({"index": 7, "name": SETLIST})]);
    b
}

fn entries() -> Vec<SetlistEntry> {
    mcp_ableton_music_maker::song::parse_setlist(SETLIST).unwrap()
}

/// A running performance with the song at `position`, the plan cue `plan`.
fn running_song(
    server: &tools::Server,
    position: usize,
    current: &str,
    plan: Option<i64>,
    history: Vec<JumpFrom>,
) {
    *server.live().performance.lock().unwrap() = Some(Performance {
        started_at: Local::now(),
        start_bar: 1,
        start_beat: 0.0,
        key: Some("F Minor".into()),
        quantization: "1_bar".into(),
        cues_scheduled: 1,
        cues_cancelled: 0,
        follow_key: false,
        song: Some(Song {
            entries: entries(),
            position: Some(position),
            current: current.into(),
            plan_cue_id: plan,
            jump_history: history,
            started_bar: 17,
        }),
        take: None,
    });
}

fn song_of(server: &tools::Server) -> Song {
    server
        .live()
        .performance
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .song
        .clone()
        .unwrap()
}

fn steer(at: Option<&str>) -> SteerParams {
    SteerParams {
        at: at.map(str::to_string),
        force: false,
    }
}

#[tokio::test]
async fn set_song_writes_the_setlist_scene_and_refuses_bad_ones() {
    let b = bridge();
    b.script("get_performance_state", vec![state(1, 1, None, false)]);
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::SET_SONG,
            SetSongParams { setlist: entries() },
            mcp_ableton_music_maker::sections::set_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands(), vec!["get_performance_state", "create_scene"]);
    assert_eq!(b.sent()[1].1, json!({"index": -1, "name": SETLIST}));
    let t = text_of(&r);
    assert!(t.starts_with("Song: Intro×2 → Groove → Groove+Pad → Break → Drop → Groove. Counted: Intro×2 = 16 bars; the other 5 loop until you say go.\n"), "{t}");
    assert!(
        t.contains(&format!(
            "Stored as the scene '{SETLIST}' (an empty row at the bottom of the set)"
        )),
        "{t}"
    );
    assert!(t.ends_with("play_song starts it; go, next_section, back and jump_to steer it. Give an entry a count (\"Drop×4\") to pre-plan it."), "{t}");

    // An existing Setlist: scene is renamed, not duplicated.
    b.script("get_performance_state", vec![state(1, 1, None, true)]);
    let r = server
        .run(
            &tools::SET_SONG,
            SetSongParams {
                setlist: entries()[..2].to_vec(),
            },
            mcp_ableton_music_maker::sections::set_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let last = b.sent().last().unwrap().clone();
    assert_eq!(last.0, "set_scene");
    assert_eq!(
        last.1,
        json!({"index": 7, "name": "Setlist: Intro×2 → Groove"})
    );

    // Unknown, misspelt and silent sections are refused before anything is written.
    let before = b.commands().len();
    let bad = |name: &str| SetSongParams {
        setlist: vec![SetlistEntry {
            section: name.into(),
            repeats: None,
        }],
    };
    let r = server
        .run(
            &tools::SET_SONG,
            bad("Outro"),
            mcp_ableton_music_maker::sections::set_song_body,
        )
        .await;
    assert!(
        is_error(&r)
            && text_of(&r).contains("'Outro' has no clips (scene 6): the song would go silent"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::SET_SONG,
            bad("grove"),
            mcp_ableton_music_maker::sections::set_song_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("'grove' is not a section (did you mean Groove?). Sections: Intro, Groove, Groove+Pad, Break, Drop, Peak. make_section first."), "{}", text_of(&r));
    assert_eq!(
        b.commands().len(),
        before + 2,
        "one state read each, nothing written"
    );

    // add_to_song and remove_from_song edit the scene name.
    let r = server
        .run(
            &tools::ADD_TO_SONG,
            AddToSongParams {
                section: "Peak".into(),
                after: Some("Drop".into()),
                before: None,
                repeats: Some(2),
            },
            mcp_ableton_music_maker::sections::add_to_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.sent().last().unwrap().1["name"],
        "Setlist: Intro×2 → Groove → Groove+Pad → Break → Drop → Peak×2 → Groove"
    );
    let r = server
        .run(
            &tools::REMOVE_FROM_SONG,
            RemoveFromSongParams {
                section: "Groove".into(),
            },
            mcp_ableton_music_maker::sections::remove_from_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.sent().last().unwrap().1["name"],
        "Setlist: Intro×2 → Groove+Pad → Break → Drop"
    );
    let r = server
        .run(
            &tools::REMOVE_FROM_SONG,
            RemoveFromSongParams {
                section: "Peak".into(),
            },
            mcp_ableton_music_maker::sections::remove_from_song_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("'Peak' is not in the song"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn play_song_starts_a_performance_fires_the_first_entry_and_schedules_one_cue() {
    let b = bridge();
    // stopped set → start_performance reads before and after → play_song reads once more
    b.script(
        "get_performance_state",
        vec![
            state(1, 1, None, true),
            state(1, 1, None, true),
            state(1, 1, Some((0, 1)), true),
            state(1, 1, Some((0, 1)), true),
        ],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::PLAY_SONG,
            PlaySongParams {
                from: None,
                record: "off".into(),
            },
            mcp_ableton_music_maker::sections::play_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec![
            "get_performance_state",
            "get_performance_state",
            "set_launch_quantization",
            "set_performance_mode",
            "fire_scene",
            "get_performance_state",
            "get_performance_state",
            "schedule_cue"
        ]
    );
    assert_eq!(b.sent()[4].1["scene_index"], 0, "Intro fired now");
    let cue = &b.sent()[7].1["cue"];
    assert_eq!(cue["name"], "song");
    assert!(cue["replaces"].is_null());
    let steps = cue["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1, "one counted jump: Intro×2 then Groove");
    assert_eq!(steps[0]["action"], "fire_scene");
    assert_eq!(steps[0]["scene_index"], 1);
    assert_eq!(steps[0]["bar"], 17.0);
    assert_eq!(steps[0]["beat"], 64.0);
    let t = text_of(&r);
    assert!(t.starts_with("Performance started at "), "{t}");
    assert!(t.contains("play_song starts a performance; the transport was stopped, so 'Intro' fires now and counts from bar 1."), "{t}");
    assert!(t.contains("Song from 'Intro' (1 of 6): Intro×2 from bar 1, then Groove at bar 17 — Groove loops until you say go. Cue 1 holds that one jump."), "{t}");
    assert!(t.contains("Say go to continue, next_section, back or jump_to <section> at any time; hold_section stops a counted section where it is."), "{t}");
    let song = song_of(&server);
    assert_eq!(song.position, Some(0));
    assert_eq!(song.plan_cue_id, Some(1));
    assert_eq!(song.started_bar, 1);

    // Without a Setlist: scene there is nothing to play.
    let b2 = bridge();
    b2.script("get_performance_state", vec![state(1, 1, None, false)]);
    let server2 = server_with(b2.clone());
    let r = server2
        .run(
            &tools::PLAY_SONG,
            PlaySongParams {
                from: None,
                record: "off".into(),
            },
            mcp_ableton_music_maker::sections::play_song_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).starts_with("No song in this set: set_song first"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn go_lands_at_the_end_of_the_playing_phrase_and_next_bar_says_what_it_cuts() {
    let b = bridge();
    b.script(
        "schedule_cue",
        vec![json!({"id": 2, "name": "song", "steps": []})],
    );
    // Bar 30.3, Groove (8 bars) playing since 17: the phrase ends at 33.
    b.script(
        "get_performance_state",
        vec![state(30, 3, Some((1, 17)), true)],
    );
    let server = server_with(b.clone());
    running_song(&server, 1, "Groove", None, vec![]);
    let r = server
        .run(
            &tools::GO,
            steer(None),
            mcp_ableton_music_maker::sections::go_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec!["get_performance_state", "schedule_cue"],
        "one state read, one cue"
    );
    let cue = &b.sent()[1].1["cue"];
    let steps = cue["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["scene_index"], 2, "Groove+Pad");
    assert_eq!(
        steps[0]["bar"], 33.0,
        "the phrase rule: Groove's phrase from 17 ends at 33"
    );
    let t = text_of(&r);
    assert!(
        t.starts_with(
            "Continuing: 'Groove+Pad' at bar 33 (end of this phrase); it loops until go. Cue 2."
        ),
        "{t}"
    );
    let song = song_of(&server);
    assert_eq!(
        (
            song.position,
            song.current.as_str(),
            song.plan_cue_id,
            song.started_bar
        ),
        (Some(2), "Groove+Pad", Some(2), 33)
    );
    assert_eq!(song.jump_history.len(), 1);
    assert_eq!(song.jump_history[0].section, "Groove");

    // Break (16 bars) is queued next, but "next phrase" is still counted in the playing section's phrase.
    b.script(
        "get_performance_state",
        vec![state(38, 2, Some((2, 33)), true)],
    );
    let r = server
        .run(
            &tools::NEXT_SECTION,
            steer(Some("next_bar")),
            mcp_ableton_music_maker::sections::next_section_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let steps = b.sent().last().unwrap().1["cue"]["steps"].clone();
    assert_eq!(steps[0]["scene_index"], 3, "Break");
    assert_eq!(steps[0]["bar"], 39.0);
    let t = text_of(&r);
    assert!(t.starts_with("Next: 'Break' at bar 39 (next bar) — cutting 2 bars off Groove+Pad's phrase, as asked; it loops until go. Cue 2 replaces cue 2."), "{t}");
    assert_eq!(
        b.sent().last().unwrap().1["cue"]["replaces"],
        2,
        "the old plan goes in the same call"
    );

    // A bar in the past, and a time that is not one.
    let r = server
        .run(
            &tools::NEXT_SECTION,
            steer(Some("20")),
            mcp_ableton_music_maker::sections::next_section_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("bar 20 has passed"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::NEXT_SECTION,
            steer(Some("soon")),
            mcp_ableton_music_maker::sections::next_section_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("'soon' is not a time"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn jump_to_with_repeats_plans_the_continuation_warns_when_hotter_and_back_returns() {
    let b = bridge();
    b.script(
        "schedule_cue",
        vec![json!({"id": 3, "name": "song", "steps": [], "replaced": [2]})],
    );
    // Bar 38.2 in Groove+Pad (from 33): the phrase ends at 41. Drop ran 3.3 dB hotter.
    b.script(
        "get_performance_state",
        vec![state(38, 2, Some((2, 33)), true)],
    );
    let server = server_with(b.clone());
    running_song(
        &server,
        2,
        "Groove+Pad",
        Some(2),
        vec![JumpFrom {
            section: "Groove".into(),
            position: Some(1),
        }],
    );
    let r = server
        .run(
            &tools::JUMP_TO,
            JumpToParams {
                section: "Drop".into(),
                repeats: Some(8),
                at: None,
                transition: None,
                force: false,
            },
            mcp_ableton_music_maker::sections::jump_to_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands(), vec!["get_performance_state", "schedule_cue"]);
    let cue = &b.sent()[1].1["cue"];
    assert_eq!(cue["replaces"], 2);
    let steps = cue["steps"].as_array().unwrap();
    assert_eq!(
        steps.len(),
        2,
        "Drop at 41, then the song continues after Drop: Groove at 105"
    );
    assert_eq!(
        (steps[0]["scene_index"].as_i64(), steps[0]["bar"].as_f64()),
        (Some(4), Some(41.0))
    );
    assert_eq!(
        (steps[1]["scene_index"].as_i64(), steps[1]["bar"].as_f64()),
        (Some(1), Some(105.0))
    );
    let t = text_of(&r);
    assert!(t.starts_with("Warning: 'Drop' last peaked at −0.5 dB, 3.3 dB hotter than Groove+Pad (−3.8 dB). Jumping anyway (force: true to silence this; pull the master with set_track_mixer master, or cue a ramp, before bar 41).\n"), "{t}");
    assert!(t.contains("Jumping to 'Drop' at bar 41 (end of this phrase), 8 passes, then the song continues: Groove at bar 105 (loops). Cue 3 replaces cue 2."), "{t}");
    assert!(t.contains("fire scene 'Drop · 8'"), "{t}");
    let song = song_of(&server);
    assert_eq!((song.position, song.current.as_str()), (Some(4), "Drop"));
    assert_eq!(song.jump_history.last().unwrap().section, "Groove+Pad");

    // force: true silences the warning; a section not in the setlist is a detour.
    b.script(
        "get_performance_state",
        vec![state(38, 2, Some((2, 33)), true)],
    );
    let r = server
        .run(
            &tools::JUMP_TO,
            JumpToParams {
                section: "Peak".into(),
                repeats: None,
                at: None,
                transition: None,
                force: true,
            },
            mcp_ableton_music_maker::sections::jump_to_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(!text_of(&r).contains("Warning"), "{}", text_of(&r));
    assert!(
        text_of(&r)
            .starts_with("Jumping to 'Peak' at bar 41 (end of this phrase); it loops until go."),
        "{}",
        text_of(&r)
    );

    // back: to where the performer was (Groove+Pad), not to the entry before Drop.
    b.script(
        "get_performance_state",
        vec![state(46, 1, Some((4, 41)), true)],
    );
    let r = server
        .run(
            &tools::BACK,
            steer(None),
            mcp_ableton_music_maker::sections::back_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let steps = b.sent().last().unwrap().1["cue"]["steps"].clone();
    assert_eq!(steps[0]["scene_index"], 2, "Groove+Pad");
    assert_eq!(
        steps[0]["bar"], 49.0,
        "Drop's 8-bar phrase from 41 ends at 49"
    );
    let t = text_of(&r);
    assert!(t.starts_with("Back to where you were: 'Groove+Pad' at bar 49 (end of this phrase); it loops until go. Cue 3 replaces cue 3."), "{t}");
    let song = song_of(&server);
    assert_eq!(song.position, Some(2));
    assert_eq!(song.jump_history.len(), 2, "back pops the history");

    // Unknown target, and back with nothing to return to.
    let r = server
        .run(
            &tools::JUMP_TO,
            JumpToParams {
                section: "Outro2".into(),
                repeats: None,
                at: None,
                transition: None,
                force: false,
            },
            mcp_ableton_music_maker::sections::jump_to_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("'Outro2' is not a section (did you mean Outro?)"),
        "{}",
        text_of(&r)
    );
    running_song(&server, 2, "Groove+Pad", None, vec![]);
    let r = server
        .run(
            &tools::BACK,
            steer(None),
            mcp_ableton_music_maker::sections::back_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("Nothing to go back to"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn hold_cancels_the_count_and_go_reports_a_running_one() {
    let b = bridge();
    let mut s = state(20, 1, Some((1, 17)), true);
    s["cues"] = json!([{"id": 5, "name": "song", "pending": 1, "steps": [
        {"index": 0, "action": "fire_scene", "beat": 128.0, "bar": 33.0, "label": "fire scene 'Groove+Pad · 8' (4 clips: Kick, Hats+Perc, Bass, Pad)", "done": false}
    ]}]);
    b.script("get_performance_state", vec![s]);
    let server = server_with(b.clone());
    let mut song_entries = entries();
    song_entries[1].repeats = Some(2);
    running_song(&server, 1, "Groove", Some(5), vec![]);
    server
        .live()
        .performance
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .song
        .as_mut()
        .unwrap()
        .entries = song_entries;

    let r = server
        .run(
            &tools::GO,
            steer(None),
            mcp_ableton_music_maker::sections::go_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec!["get_performance_state"],
        "nothing scheduled"
    );
    assert_eq!(text_of(&r), "Nothing is held: the song is running ('Groove', pass 1 of 2, next jump bar 33). next_section moves on now; hold_section stops the count.");

    let r = server
        .run(
            &tools::HOLD_SECTION,
            Empty {},
            mcp_ableton_music_maker::sections::hold_section_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands()[1..], ["get_performance_state", "cancel_cue"]);
    assert_eq!(
        b.sent().last().unwrap().1,
        json!({"id": 5, "reason": "hold"})
    );
    assert_eq!(text_of(&r), "Holding 'Groove': it loops from here (8-bar phrases from bar 17); the counted jump to Groove+Pad · 8 at bar 33 is off (cue 5 cancelled). Say go to continue.");
    assert_eq!(song_of(&server).plan_cue_id, None);

    let r = server
        .run(
            &tools::HOLD_SECTION,
            Empty {},
            mcp_ableton_music_maker::sections::hold_section_body,
        )
        .await;
    assert!(
        !is_error(&r) && text_of(&r) == "Nothing to hold: 'Groove' loops until you say go already.",
        "{}",
        text_of(&r)
    );

    // No performance: every verb says so without a command.
    *server.live().performance.lock().unwrap() = None;
    let before = b.commands().len();
    for (spec, body) in [
        (
            &tools::GO,
            mcp_ableton_music_maker::sections::go_body as fn(&_, &_) -> _,
        ),
        (
            &tools::NEXT_SECTION,
            mcp_ableton_music_maker::sections::next_section_body,
        ),
        (&tools::BACK, mcp_ableton_music_maker::sections::back_body),
    ] {
        let r = server.run(spec, steer(None), body).await;
        assert!(
            is_error(&r) && text_of(&r).starts_with("No performance is running"),
            "{}",
            text_of(&r)
        );
    }
    assert_eq!(b.commands().len(), before);
}

#[tokio::test]
async fn make_section_captures_copies_with_changes_or_writes_from_clips() {
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(30, 1, Some((1, 17)), true)],
    );
    b.script("capture_scene", vec![json!({"index": 2, "name": "Peak2 · 8", "section": "Peak2", "phrase_bars": 8, "scene_count": 9, "launched": true,
        "clips": [{"track_index": 0, "track": "Kick", "name": "Groove/Kick"}, {"track_index": 1, "track": "Hats+Perc", "name": "x"}, {"track_index": 2, "track": "Bass", "name": "y"}]})]);
    let server = server_with(b.clone());
    let mut p = MakeSectionParams {
        name: "Peak2".into(),
        phrase_bars: Some(8),
        from: Some(json!("playing")),
        ..Default::default()
    };
    let r = server
        .run(
            &tools::MAKE_SECTION,
            p.clone(),
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands(), vec!["get_performance_state", "capture_scene"]);
    assert_eq!(
        b.sent()[1].1,
        json!({"name": "Peak2", "phrase_bars": 8, "after": null})
    );
    let t = text_of(&r);
    assert!(t.starts_with("Section 'Peak2 · 8' saved as scene 2: the playing clips (Kick, Hats+Perc, Bass) captured into a new row below the playing row (Live's capture-and-insert-scene"), "{t}");
    assert!(
        t.contains("Not in the setlist yet: add_to_song {\"section\": \"Peak2\"} or jump_to it."),
        "{t}"
    );

    // A duplicate name is refused with the existing row.
    p.name = "Groove".into();
    let r = server
        .run(
            &tools::MAKE_SECTION,
            p.clone(),
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert_eq!(text_of(&r), "make_section: a scene named 'Groove · 8' exists (scene 1). Names are how sections are remembered; call this one 'Groove 2' or replace: true.");
    // Nothing playing: nothing to capture.
    b.script("get_performance_state", vec![state(1, 1, None, true)]);
    p.name = "Peak3".into();
    let r = server
        .run(
            &tools::MAKE_SECTION,
            p.clone(),
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).starts_with("Nothing is playing to capture."),
        "{}",
        text_of(&r)
    );

    // "Like Groove but…": duplicate_scene, then per-track changes.
    b.script(
        "get_performance_state",
        vec![state(30, 1, Some((1, 17)), true)],
    );
    b.script("duplicate_scene", vec![json!({"index": 2, "name": "Groove Low · 8", "section": "Groove Low", "source_index": 1, "scene_count": 9,
        "clips": [{"track_index": 0, "track": "Kick", "name": "k"}, {"track_index": 1, "track": "Hats+Perc", "name": "h"}, {"track_index": 2, "track": "Bass", "name": "b"}]})]);
    b.script("get_clip_notes", vec![json!({"notes": [
        {"pitch": 42, "start_time": 0.0, "duration": 0.25, "velocity": 100, "mute": false}, {"pitch": 42, "start_time": 0.5, "duration": 0.25, "velocity": 80, "mute": false},
        {"pitch": 42, "start_time": 1.0, "duration": 0.25, "velocity": 100, "mute": false}, {"pitch": 42, "start_time": 1.5, "duration": 0.25, "velocity": 80, "mute": false}]})]);
    let mut changes = BTreeMap::new();
    changes.insert("Hats+Perc".to_string(), json!("thin"));
    changes.insert("Bass".to_string(), json!({"transpose": -12}));
    changes.insert("Lead".to_string(), json!("empty"));
    let before = b.commands().len();
    let r = server
        .run(
            &tools::MAKE_SECTION,
            MakeSectionParams {
                name: "Groove Low".into(),
                from: Some(json!({"section": "Groove"})),
                changes,
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands()[before..].to_vec();
    assert_eq!(
        cmds,
        vec![
            "get_performance_state",
            "duplicate_scene",
            "get_clip_notes",
            "clear_notes_from_clip",
            "add_notes_to_clip",
            "get_clip_notes",
            "clear_notes_from_clip",
            "add_notes_to_clip"
        ]
    );
    let sent = b.sent();
    assert_eq!(
        sent[before + 1].1,
        json!({"index": 1, "name": "Groove Low", "phrase_bars": null})
    );
    // Bass transposed −12 (the BTreeMap orders Bass first), Hats thinned; Lead had no clip in the copy: nothing to delete.
    let bass = &sent[before + 4].1;
    assert_eq!(
        (bass["track_index"].as_i64(), bass["clip_index"].as_i64()),
        (Some(2), Some(2))
    );
    assert_eq!(bass["notes"][0]["pitch"], 30);
    let hats = &sent[before + 7].1;
    assert_eq!(hats["track_index"], 1);
    assert_eq!(
        hats["notes"].as_array().unwrap().len(),
        3,
        "every other off-beat dropped"
    );
    let t = text_of(&r);
    assert_eq!(t, "Section 'Groove Low · 8' created as scene 2, a copy of 'Groove · 8' with: Bass transposed -12, Hats+Perc thinned (every other off-beat dropped) (seed 1), Lead emptied. Kick is identical to Groove.\nNot in the setlist: jump_to it or add_to_song {\"section\": \"Groove Low\"}.");

    // Written from clips, after Break: create_scene at Break+1, one clip per named track.
    b.script(
        "get_performance_state",
        vec![state(30, 1, Some((1, 17)), true)],
    );
    b.script(
        "create_scene",
        vec![json!({"index": 4, "name": "Dark · 8", "section": "Dark", "scene_count": 9})],
    );
    let mut clips = BTreeMap::new();
    clips.insert(
        "Kick".to_string(),
        json!({"steps": {"C1": "x.......x......."}}),
    );
    clips.insert(
        "Bass".to_string(),
        json!({"notes_csv": "41,0,4,90\n41,4,4,90", "length": 8}),
    );
    let before = b.commands().len();
    let r = server
        .run(
            &tools::MAKE_SECTION,
            MakeSectionParams {
                name: "Dark".into(),
                phrase_bars: Some(8),
                clips: clips.clone(),
                after: Some("Break".into()),
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands()[before..].to_vec();
    assert_eq!(
        cmds,
        vec!["get_performance_state", "create_scene", "write_clips"]
    );
    let sent = b.sent();
    assert_eq!(
        sent[before + 1].1,
        json!({"index": 4, "name": "Dark · 8", "tempo": null})
    );
    let written = sent[before + 2].1["clips"].as_array().unwrap().clone();
    assert_eq!(written.len(), 2, "both clips in one round trip");
    assert_eq!(
        (
            written[0]["track_index"].as_i64(),
            written[0]["clip_index"].as_i64(),
            written[0]["length"].as_f64()
        ),
        (Some(2), Some(4), Some(8.0)),
        "Bass: the given length"
    );
    assert_eq!(written[0]["name"], "Dark/Bass");
    assert_eq!(
        (
            written[1]["track_index"].as_i64(),
            written[1]["length"].as_f64()
        ),
        (Some(0), Some(4.0)),
        "Kick: a 16-step string is one bar"
    );
    let t = text_of(&r);
    assert!(t.starts_with("Section 'Dark · 8' created as scene 4 (after Break; later rows moved down): Bass, Kick have clips; Hats+Perc, Pad, Lead are empty (they stop when the section fires; keep_track_playing carries a track through)."), "{t}");
    assert!(
        t.contains("add_to_song {\"section\": \"Dark\", \"after\": \"Break\"}"),
        "{t}"
    );

    // replace: true rewrites an existing clips section in place.
    b.script(
        "get_performance_state",
        vec![state(30, 1, Some((1, 17)), true)],
    );
    let before = b.commands().len();
    let r = server
        .run(
            &tools::MAKE_SECTION,
            MakeSectionParams {
                name: "Break".into(),
                clips,
                replace: true,
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let cmds = b.commands()[before..].to_vec();
    assert_eq!(
        cmds[..3],
        ["get_performance_state", "delete_clip", "delete_clip"],
        "Pad and Lead cleared from row 3"
    );
    assert!(
        cmds.iter().all(|c| c != "create_scene" && c != "set_scene"),
        "{cmds:?}"
    );
    assert!(text_of(&r).starts_with("Section 'Break · 16' rewritten in place (scene 3, 2 old clips removed): Bass, Kick have clips"), "{}", text_of(&r));
    // Bad input never touches the set.
    let before = b.commands().len();
    let r = server
        .run(
            &tools::MAKE_SECTION,
            MakeSectionParams {
                name: "X".into(),
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).starts_with("Give the material:"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::MAKE_SECTION,
            MakeSectionParams {
                name: "Setlist: x".into(),
                from: Some(json!("playing")),
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands().len(), before);
}

#[tokio::test]
async fn the_readouts_show_sections_and_the_song_and_the_level_line_rides_under_the_clock() {
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![state(30, 3, Some((1, 17)), true)],
    );
    b.set_clock(Some(json!({"bar": 30, "beat_in_bar": 3, "is_playing": true, "seconds_to_next_bar": 0.9,
        "phrase": {"ends_bar": 33}, "next_cue": {"cue_id": 2, "bar": 33.0, "label": "fire scene 'Groove+Pad · 8'"},
        "levels": {"bar": 30, "master_peak_db": -3.8, "tracks": [{"index": 0, "name": "Kick", "peak_db": -6.2}, {"index": 2, "name": "Bass", "peak_db": -7.0}, {"index": 3, "name": "Pad", "peak_db": -45.0}], "section_peaks": {}}})));
    let server = server_with(b.clone());
    running_song(&server, 1, "Groove", Some(2), vec![]);
    let r = server
        .run(
            &tools::GET_PERFORMANCE_STATE,
            GetPerformanceStateParams { bar_map: false },
            tools::get_performance_state_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("Scenes: Intro · 8 · [Groove · 8] · Groove+Pad · 8 · Break · 16 · Drop · 8 · Peak · 8 · (1 empty)\n"), "no double suffix, no Setlist row: {t}");
    assert!(
        t.contains("Song: Intro×2 → [Groove: looping] → Groove+Pad → Break → Drop → Groove\n"),
        "{t}"
    );
    assert!(t.ends_with("⏱ bar 30.3 · next bar in 0.9 s · phrase ends bar 33 · next jump: bar 33 fire scene 'Groove+Pad · 8' (cue 2)\n🔊 master −3.8 dB peak this bar · Kick −6 · Bass −7"), "{t}");

    // get_context reads the song from the scene names, with no server memory at all.
    *server.live().performance.lock().unwrap() = None;
    b.set_clock(None);
    b.script("get_context", vec![json!({"live_version": "12.4.6", "script_version": "1.17.0",
        "session": {"tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4, "is_playing": false, "bar": 1, "beat_in_bar": 1, "clip_trigger_quantization_name": "1_bar", "scale_name": "Minor", "root_note_name": "F", "master_volume": 0.85},
        "tracks": [], "returns": [], "cues": [], "events": [],
        "scenes": state(1, 1, None, true)["scenes"].as_array().unwrap().iter().map(|sc| { let mut sc = sc.clone(); sc["clip_count"] = json!(sc["clip_tracks"].as_array().unwrap().len()); sc }).collect::<Vec<_>>()
    })]);
    let r = server
        .run(
            &tools::GET_CONTEXT,
            GetContextParams {
                include_library: false,
                json: false,
            },
            tools::get_context_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("Sections (scenes): 0 Intro · 8 (2) · 1 Groove · 8 (3) · 2 Groove+Pad · 8 (4) · 3 Break · 16 (2) · 4 Drop · 8 (5) · 5 Peak · 8 (5) · 6 empty\n"), "{t}");
    assert!(t.contains("Song: Intro×2 → Groove → Groove+Pad → Break → Drop → Groove (from the 'Setlist:' scene; play_song, or jump_to a section)\n"), "{t}");
    assert!(!t.contains('⏱'), "{t}");
}

#[tokio::test]
async fn every_section_tool_is_listed_and_a_set_without_suffixes_behaves_as_before() {
    let b = bridge();
    let server = server_with(b.clone());
    let names: Vec<String> = server
        .tool_list()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    for n in [
        "make_section",
        "set_song",
        "add_to_song",
        "remove_from_song",
        "play_song",
        "hold_section",
        "go",
        "next_section",
        "previous_section",
        "back",
        "jump_to",
    ] {
        assert!(names.contains(&n.to_string()), "{n} missing");
    }
    // Plain scene names: 16-bar default phrases, jump_to still works by name.
    let mut s = state(20, 1, Some((1, 17)), false);
    for (i, name) in [
        "Intro",
        "Groove",
        "Groove+Pad",
        "Break",
        "Drop",
        "Peak",
        "Outro",
    ]
    .iter()
    .enumerate()
    {
        s["scenes"][i]["name"] = json!(name);
        s["scenes"][i]["phrase_bars"] = json!(16);
        s["scenes"][i]["phrase_default"] = json!(true);
    }
    s["phrase"] =
        json!({"scene_index": 1, "started_bar": 17, "bars": 16, "ends_bar": 33, "default": true});
    b.script("get_performance_state", vec![s]);
    *server.live().performance.lock().unwrap() = Some(Performance {
        started_at: Local::now(),
        start_bar: 1,
        start_beat: 0.0,
        key: None,
        quantization: "1_bar".into(),
        cues_scheduled: 0,
        cues_cancelled: 0,
        follow_key: false,
        song: None,
        take: None,
    });
    let r = server
        .run(
            &tools::JUMP_TO,
            JumpToParams {
                section: "Break".into(),
                repeats: None,
                at: None,
                transition: None,
                force: false,
            },
            mcp_ableton_music_maker::sections::jump_to_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands(), vec!["get_performance_state", "schedule_cue"]);
    assert_eq!(
        b.sent()[1].1["cue"]["steps"][0]["bar"],
        33.0,
        "16-bar default phrase from 17"
    );
    assert!(
        text_of(&r).starts_with(
            "Jumping to 'Break' at bar 33 (end of this phrase); it loops until go. Cue 1."
        ),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::NEXT_SECTION,
            steer(None),
            mcp_ableton_music_maker::sections::next_section_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("there is no song"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn a_transition_composes_tempo_retime_and_crossfade_into_the_jump() {
    let b = bridge();
    b.script(
        "schedule_cue",
        vec![json!({"id": 4, "name": "song", "steps": []})],
    );
    // Bar 38.2 in Groove+Pad (from 33): the phrase ends at 41, the next bar is 39.
    b.script(
        "get_performance_state",
        vec![state(38, 2, Some((2, 33)), true)],
    );
    b.script(
        "get_clip_info",
        vec![json!({"name": "Groove+Pad/Kick", "length": 4.0})],
    );
    b.script("get_clip_notes", vec![json!({"notes": (0..4).map(|i| json!({"pitch": 36, "start_time": i as f64, "duration": 0.25, "velocity": 100, "mute": false})).collect::<Vec<_>>()})]);
    b.script("get_context", vec![json!({"tracks": [{"index": 3, "name": "Pad", "volume": 0.8}, {"index": 4, "name": "Lead", "volume": 0.7}]})]);
    let server = server_with(b.clone());
    running_song(&server, 2, "Groove+Pad", Some(2), vec![]);
    let r = server
        .run(
            &tools::JUMP_TO,
            JumpToParams {
                section: "Break".into(), repeats: None, at: None, force: true,
                transition: Some(json!({"tempo": 122, "retime": {"tracks": ["Kick"], "to": "half_time"}, "crossfade": {"out": ["Pad"], "in": ["Lead"], "bars": 8}})),
            },
            mcp_ableton_music_maker::sections::jump_to_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec![
            "get_performance_state",
            "get_clip_info",
            "get_clip_notes",
            "create_clip",
            "set_clip_name",
            "add_notes_to_clip",
            "get_context",
            "get_performance_state",
            "schedule_cue"
        ],
        "the retimed copy is written now; the faders are read once; the state is read again after the round trips"
    );
    let sent = b.sent();
    // Kick has no clip in Break's row: a half-time copy of what it plays goes there.
    assert_eq!(
        sent[3].1,
        json!({"track_index": 0, "clip_index": 3, "length": 4.0})
    );
    assert_eq!(sent[4].1["name"], "Groove+Pad/Kick (half_time)");
    assert_eq!(
        sent[5].1["notes"].as_array().unwrap().len(),
        2,
        "four quarter notes become two half notes"
    );
    let steps = sent[8].1["cue"]["steps"].as_array().unwrap().clone();
    let describe = |s: &Value| {
        format!(
            "{} {} {}→{}",
            s["action"],
            s["target"].as_str().unwrap_or("-"),
            s["bar"],
            s["end_beat"]
        )
    };
    assert_eq!(
        steps.len(),
        5,
        "{:?}",
        steps.iter().map(describe).collect::<Vec<_>>()
    );
    assert_eq!(
        (
            steps[0]["action"].as_str(),
            steps[0]["target"].as_str(),
            steps[0]["bar"].as_f64(),
            steps[0]["end_beat"].as_f64(),
            steps[0]["to"].as_f64()
        ),
        (
            Some("ramp"),
            Some("tempo"),
            Some(39.0),
            Some(160.0),
            Some(122.0)
        ),
        "tempo ramps under the outgoing phrase and ends at the boundary"
    );
    assert_eq!(
        (
            steps[1]["action"].as_str(),
            steps[1]["target"].as_str(),
            steps[1]["track_index"].as_i64(),
            steps[1]["bar"].as_f64(),
            steps[1]["to"].as_f64()
        ),
        (Some("ramp"), Some("volume"), Some(3), Some(41.0), Some(0.0)),
        "Pad fades out from the boundary"
    );
    assert_eq!(
        (
            steps[2]["action"].as_str(),
            steps[2]["target"].as_str(),
            steps[2]["track_index"].as_i64(),
            steps[2]["bar"].as_f64(),
            steps[2]["value"].as_f64()
        ),
        (Some("set"), Some("volume"), Some(4), Some(40.0), Some(0.0)),
        "Lead's fader is at 0 one bar before the boundary"
    );
    assert_eq!(
        (
            steps[3]["action"].as_str(),
            steps[3]["track_index"].as_i64(),
            steps[3]["bar"].as_f64(),
            steps[3]["end_beat"].as_f64(),
            steps[3]["to"].as_f64()
        ),
        (Some("ramp"), Some(4), Some(41.0), Some(192.0), Some(0.7)),
        "Lead fades up to its own level over 8 bars"
    );
    assert_eq!(
        (
            steps[4]["action"].as_str(),
            steps[4]["scene_index"].as_i64(),
            steps[4]["bar"].as_f64()
        ),
        (Some("fire_scene"), Some(3), Some(41.0))
    );
    let t = text_of(&r);
    assert!(t.starts_with("Jumping to 'Break' at bar 41 (end of this phrase); it loops until go. Cue 4 replaces cue 2.\n"), "{t}");
    assert!(
        t.contains("bars 39–41   ramp tempo 126 → 122 under the outgoing phrase"),
        "{t}"
    );
    assert!(t.contains("bar 41       Kick retimed to half time in Break's row (clips 'Groove+Pad/Kick (half_time)' written now, seed 1"), "{t}");
    assert!(
        t.contains(
            "bars 41–49   crossfade: Pad → 0 · Lead (to 0.70) from 0 up to their current level"
        ),
        "{t}"
    );
    assert_eq!(
        t.matches("ramp tempo").count(),
        1,
        "the transition's line, not the generic one too: {t}"
    );
    assert!(t.contains("fire scene 'Break · 16'"), "{t}");

    // An unknown key never touches the set.
    let before = b.commands().len();
    let r = server
        .run(
            &tools::JUMP_TO,
            JumpToParams {
                section: "Break".into(),
                repeats: None,
                at: None,
                force: true,
                transition: Some(json!({"morph": 1})),
            },
            mcp_ableton_music_maker::sections::jump_to_body,
        )
        .await;
    assert!(is_error(&r) && text_of(&r).contains("transition: unknown key 'morph'; one of tempo, retime, crossfade, fill, drop, sweep"), "{}", text_of(&r));
    assert_eq!(b.commands().len(), before + 1, "only the state read");
}

#[tokio::test]
async fn fill_drop_and_sweep_transitions_and_the_standalone_retime() {
    let b = bridge();
    b.script(
        "schedule_cue",
        vec![json!({"id": 5, "name": "song", "steps": []})],
    );
    b.script(
        "get_performance_state",
        vec![state(38, 2, Some((2, 33)), true)],
    );
    b.script(
        "get_clip_info",
        vec![json!({"name": "Groove+Pad/Hats+Perc", "length": 4.0})],
    );
    b.script("get_clip_notes", vec![json!({"notes": (0..8).map(|i| json!({"pitch": 42, "start_time": i as f64 * 0.5, "duration": 0.25, "velocity": 100, "mute": false})).collect::<Vec<_>>()})]);
    let server = server_with(b.clone());
    running_song(&server, 2, "Groove+Pad", Some(2), vec![]);
    let r = server
        .run(
            &tools::JUMP_TO,
            JumpToParams {
                section: "Break".into(), repeats: None, at: Some("45".into()), force: true,
                transition: Some(json!({"fill": {"track": "Hats+Perc"}, "drop": {"bars": 2, "keep": ["Kick"]}, "sweep": {"track": "Pad", "device_index": 0, "parameter_index": 3, "from": 0.2, "to": 1.0}})),
            },
            mcp_ableton_music_maker::sections::jump_to_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands(),
        vec![
            "get_performance_state",
            "get_clip_info",
            "get_clip_notes",
            "create_clip",
            "set_clip_name",
            "add_notes_to_clip",
            "get_performance_state",
            "schedule_cue"
        ]
    );
    let sent = b.sent();
    assert_eq!(
        sent[3].1,
        json!({"track_index": 1, "clip_index": 6, "length": 4.0}),
        "the fill clip goes into the first free slot that is not the target row"
    );
    assert_eq!(sent[4].1["name"], "Groove+Pad/Hats+Perc fill");
    assert_eq!(
        sent[5].1["notes"].as_array().unwrap().len(),
        16,
        "a bar of 16ths"
    );
    let steps = sent[7].1["cue"]["steps"].as_array().unwrap().clone();
    let of = |action: &str| -> Vec<Value> {
        steps
            .iter()
            .filter(|s| s["action"] == action)
            .cloned()
            .collect()
    };
    let fires = of("fire_clip");
    assert_eq!(fires.len(), 1);
    assert_eq!(
        (
            fires[0]["track_index"].as_i64(),
            fires[0]["clip_index"].as_i64(),
            fires[0]["bar"].as_f64()
        ),
        (Some(1), Some(6), Some(44.0)),
        "the fill fires one bar before the jump"
    );
    let stops = of("stop_clip");
    assert_eq!(stops.len(), 4, "the fill stops at the jump (no Hats+Perc clip in Break); the drop stops Hats+Perc, Bass and Pad at 43");
    assert!(
        stops
            .iter()
            .any(|s| s["clip_index"] == 6 && s["bar"] == 45.0),
        "{stops:?}"
    );
    assert_eq!(
        stops.iter().filter(|s| s["bar"] == 43.0).count(),
        3,
        "{stops:?}"
    );
    assert!(!stops.iter().any(|s| s["track_index"] == 0), "Kick is kept");
    let ramps = of("ramp");
    assert_eq!(ramps.len(), 1);
    assert_eq!(
        (
            ramps[0]["target"].as_str(),
            ramps[0]["bar"].as_f64(),
            ramps[0]["end_beat"].as_f64(),
            ramps[0]["to"].as_f64()
        ),
        (Some("device"), Some(39.0), Some(176.0), Some(1.0)),
        "the sweep runs from the next bar to the jump"
    );
    assert_eq!(of("fire_scene")[0]["bar"], 45.0);
    let t = text_of(&r);
    assert!(t.contains("bar 44       fill: 'Groove+Pad/Hats+Perc fill' (slot 6, 1 bar, seed 1) on Hats+Perc until the jump; stopped at bar 45 (Hats+Perc has no clip in Break's row)"), "{t}");
    assert!(t.contains("bar 43       drop: Hats+Perc, Bass, Pad out, Kick stays for the last 2 bars before the jump"), "{t}");
    assert!(
        t.contains("bars 39–45   sweep Pad device 0 parameter 3 → 1 under the outgoing phrase"),
        "{t}"
    );

    // retime_clip on its own is vary_clip's half_time with one undo.
    b.script(
        "get_performance_state",
        vec![state(38, 2, Some((2, 33)), true)],
    );
    let before = b.commands().len();
    let r = server
        .run(
            &tools::RETIME_CLIP,
            tools::RetimeClipParams {
                track: json!("Kick"),
                clip: 2,
                to: "half time".into(),
                seed: 1,
                to_slot: None,
            },
            tools::retime_clip_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands()[before..],
        [
            "get_performance_state",
            "get_clip_notes",
            "get_clip_info",
            "clear_notes_from_clip",
            "add_notes_to_clip"
        ]
    );
    assert!(
        text_of(&r).starts_with("Varied 'Kick' slot 2 in place (half_time, seed 1)"),
        "{}",
        text_of(&r)
    );
    let r = server
        .run(
            &tools::RETIME_CLIP,
            tools::RetimeClipParams {
                track: json!("Kick"),
                clip: 2,
                to: "reverse".into(),
                seed: 1,
                to_slot: None,
            },
            tools::retime_clip_body,
        )
        .await;
    assert!(
        is_error(&r) && text_of(&r).contains("to must be half_time or double_time"),
        "{}",
        text_of(&r)
    );
}

#[tokio::test]
async fn play_song_asks_about_a_full_arrangement_before_a_note_plays() {
    // AC5: the same question, whichever tool the producer reached for.
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![
            state(1, 1, None, true),
            state(1, 1, None, true),
            state(1, 1, Some((0, 1)), true),
            state(1, 1, Some((0, 1)), true),
        ],
    );
    b.script(
        "arrangement_summary",
        vec![
            json!({"supported": true, "end_beat": 512.0, "bars": 128, "clips": 16,
                    "tracks": 5, "beats_per_bar": 4}),
        ],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::PLAY_SONG,
            PlaySongParams {
                from: None,
                record: "ask".into(),
            },
            mcp_ableton_music_maker::sections::play_song_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("128 bars on 5 tracks"), "{t}");
    assert!(t.contains("after"), "{t}");
    assert!(
        !b.commands().contains(&"fire_scene".to_string()),
        "the song started anyway: {:?}",
        b.commands()
    );
}

#[tokio::test]
async fn play_song_records_the_take_when_told_to() {
    let b = bridge();
    b.script(
        "get_performance_state",
        vec![
            state(1, 1, None, true),
            state(1, 1, None, true),
            state(1, 1, Some((0, 1)), true),
            state(1, 1, Some((0, 1)), true),
        ],
    );
    b.script(
        "arrangement_summary",
        vec![
            json!({"supported": true, "end_beat": 512.0, "bars": 128, "clips": 16,
                    "tracks": 5, "beats_per_bar": 4}),
        ],
    );
    b.script(
        "start_arrangement_record",
        vec![
            json!({"from_beat": 512.0, "from_bar": 129, "replaced_clips": 0,
                    "replaced_tracks": 0, "record_mode": true}),
        ],
    );
    let server = server_with(b.clone());
    let r = server
        .run(
            &tools::PLAY_SONG,
            PlaySongParams {
                from: None,
                record: "after".into(),
            },
            mcp_ableton_music_maker::sections::play_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Recording this take from bar 129"),
        "the take is not in the reply: {}",
        text_of(&r)
    );
}
