//! #63 — the song remembers itself.
//!
//! A producer's session ends and everything Claude knew that Live cannot
//! hold goes with it: that the Sitar is the lead, that the Smoke Bass is
//! theirs and must never be regenerated, the ideas parked but unused, and —
//! the biggest loss — **the plan**.
//!
//! What Live can hold, Live holds, so the roles and the stash are asserted
//! against **the set** (a real Remote Script on the model, over a real
//! socket): they must survive this file being deleted. The overview and the
//! notes have nowhere to go in Live, so they are asserted against the file
//! and against the `get_context` header the agent actually reads.

mod common;

use common::{is_error, server_on_fake_live, server_with, text_of, FakeBridge, LiveSet};
use mcp_ableton_music_maker::memory::{self, RememberParams, SongMemoryParams, StashParams};
use mcp_ableton_music_maker::tools::{self, Server};
use serde_json::{json, Value};
use std::sync::LazyLock;

/// `ABLETON_MCP_STATE_DIR` and `ABLETON_MCP_SONG_MEMORY` belong to the
/// process, not to a test, and the harness runs these on several threads.
static ENV: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// A state dir of this test's own, with the memory on.
fn state_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_SONG_MEMORY");
    dir
}

async fn remember(server: &Server, p: RememberParams) -> rmcp::model::CallToolResult {
    server.run(&tools::REMEMBER, p, memory::remember_body).await
}

async fn stash(server: &Server, p: StashParams) -> rmcp::model::CallToolResult {
    server.run(&tools::STASH, p, memory::stash_body).await
}

async fn song_memory(server: &Server, p: SongMemoryParams) -> rmcp::model::CallToolResult {
    server
        .run(&tools::SONG_MEMORY, p, memory::song_memory_body)
        .await
}

async fn context(server: &Server) -> String {
    text_of(
        &server
            .run(
                &tools::GET_CONTEXT,
                tools::GetContextParams::default(),
                tools::get_context_body,
            )
            .await,
    )
}

fn overview(v: Value) -> RememberParams {
    RememberParams {
        overview: Some(v),
        ..Default::default()
    }
}

// ── The overview: the agent's model, returned without being asked ──────────

/// AC4, AC6, AC7: it round-trips, a patch merges, `replace` rewrites.
#[tokio::test]
async fn overview_round_trips_and_reports_the_keys_that_changed() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();

    let r = remember(
        &server,
        overview(json!({"what_it_is": "a dub track at 140", "next": "the answer phrase"})),
    )
    .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(text.contains("Keys changed: next, what_it_is"), "{text}");
    assert!(text.contains("bytes used"), "the size is reported: {text}");

    // A patch of one key leaves every other key untouched (AC6).
    let r = remember(&server, overview(json!({"next": "the bridge"}))).await;
    assert!(
        text_of(&r).contains("Keys changed: next"),
        "{}",
        text_of(&r)
    );
    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(shown.contains("a dub track at 140"), "{shown}");
    assert!(shown.contains("the bridge"), "{shown}");

    // replace: true rewrites the whole object (AC7).
    let r = remember(
        &server,
        RememberParams {
            overview: Some(json!({"plan": "intro 8, drop 16"})),
            replace: true,
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(shown.contains("intro 8, drop 16"), "{shown}");
    assert!(!shown.contains("a dub track at 140"), "replaced: {shown}");
}

/// AC5: the whole overview comes back in the `get_context` header — no
/// second call, no flag.
#[tokio::test]
async fn the_whole_overview_is_in_the_context_header() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    remember(
        &server,
        overview(json!({"next": "the answer phrase an octave up"})),
    )
    .await;

    let text = context(&server).await;
    assert!(
        text.contains("the answer phrase an octave up"),
        "the overview rides on the call the agent makes first anyway: {text}"
    );
}

/// AC8: a key the server has never heard of is kept exactly as written, and
/// rendered after the ones it knows.
#[tokio::test]
async fn unknown_keys_are_kept_verbatim() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    remember(
        &server,
        overview(json!({
            "mixdown": {"sub": "3 dB hot at the drop"},
            "what_it_is": "a dub track"
        })),
    )
    .await;
    let text = context(&server).await;
    assert!(text.contains("3 dB hot at the drop"), "{text}");
    let known = text.find("what_it_is").unwrap();
    let unknown = text.find("mixdown").unwrap();
    assert!(known < unknown, "known keys render first: {text}");
}

/// AC9: over the cap the write is refused, nothing is stored, and the error
/// names the largest key.
#[tokio::test]
async fn an_overview_over_the_cap_is_refused_and_names_the_largest_key() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    remember(&server, overview(json!({"what_it_is": "a dub track"}))).await;

    let r = remember(
        &server,
        overview(json!({"plan": "x".repeat(memory::OVERVIEW_CAP + 1)})),
    )
    .await;
    let text = text_of(&r);
    assert!(is_error(&r), "{text}");
    assert!(text.contains("the cap is 8192"), "{text}");
    assert!(text.contains("The largest key is 'plan'"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    // And nothing was: the overview is what it was before.
    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(shown.contains("a dub track"), "{shown}");
    assert!(!shown.contains("xxxxxxxx"), "{shown}");
}

/// AC10: an `overview` that is not an object is refused, with the keys the
/// header renders.
#[tokio::test]
async fn an_overview_that_is_not_an_object_is_refused() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    let r = remember(&server, overview(json!("the drop is thin"))).await;
    let text = text_of(&r);
    assert!(is_error(&r), "{text}");
    assert!(text.contains("overview is an object"), "{text}");
    for key in memory::KNOWN_KEYS {
        assert!(text.contains(key), "{key} is not named: {text}");
    }
}

/// AC11, AC12, AC13: `as_of` is the server's, an agent value is ignored, and
/// the drift is stated with the set called the truth.
#[tokio::test]
async fn as_of_is_the_servers_and_drift_is_reported_against_the_set() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());

    // An agent's as_of is ignored: the server writes what Live says (AC11).
    remember(
        &server,
        overview(json!({"what_it_is": "a dub track", "as_of": {"tempo": 999.0}})),
    )
    .await;
    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(
        !shown.contains("999"),
        "an agent's as_of is ignored: {shown}"
    );

    // The set matches: no drift line (AC13).
    let text = context(&server).await;
    assert!(!text.contains("The set has moved"), "{text}");

    // Now move the set. The header states the drift and says the set is the
    // truth (AC12).
    let before = set.tempo();
    server
        .run(
            &tools::SET_TEMPO,
            tools::SetTempoParams { tempo: 128.0 },
            tools::set_tempo_body,
        )
        .await;
    assert_ne!(set.tempo(), before);
    let text = context(&server).await;
    assert!(text.contains("The set has moved"), "{text}");
    assert!(text.contains("tempo"), "{text}");
    assert!(text.contains("The set is the truth"), "{text}");
}

/// AC15, AC16: full once, then a line — and in full again after a change.
/// A server rule, not a parameter: the agent decides nothing.
#[tokio::test]
async fn full_on_the_first_context_of_a_session_then_a_digest() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    remember(
        &server,
        overview(json!({"next": "the answer phrase an octave up"})),
    )
    .await;

    let first = context(&server).await;
    assert!(first.contains("the answer phrase an octave up"), "{first}");

    let second = context(&server).await;
    assert!(
        !second.contains("the answer phrase an octave up"),
        "a repeat call stops spending the cap on something unchanged: {second}"
    );
    assert!(second.contains("Overview: next"), "{second}");

    // A change makes the next one render it in full again (AC16).
    remember(&server, overview(json!({"next": "the bridge, half time"}))).await;
    let third = context(&server).await;
    assert!(third.contains("the bridge, half time"), "{third}");
}

/// AC17: the drift line travels. `get_context` is called once per session,
/// which is exactly why the staleness check cannot ride there alone.
#[tokio::test]
async fn the_drift_line_rides_on_play_song() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Kick", "midi", "")]);
    let kick = set.track_index("Kick").unwrap();
    set.write_clip(kick, 0, "Kick", json!([]));
    server
        .run(
            &tools::MAKE_SECTION,
            mcp_ableton_music_maker::sections::MakeSectionParams {
                name: "Intro".into(),
                phrase_bars: Some(8),
                clips: std::collections::BTreeMap::from([(
                    "Kick".to_string(),
                    json!({"notes_csv": "36,0,4,100"}),
                )]),
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    remember(&server, overview(json!({"plan": "Intro only"}))).await;
    // Read it once so the header is not the thing under test.
    context(&server).await;

    // The producer nudges the tempo in Live.
    server
        .run(
            &tools::SET_TEMPO,
            tools::SetTempoParams { tempo: 131.0 },
            tools::set_tempo_body,
        )
        .await;
    let r = server
        .run(
            &tools::SET_SONG,
            mcp_ableton_music_maker::sections::SetSongParams {
                setlist: vec![mcp_ableton_music_maker::song::SetlistEntry {
                    section: "Intro".into(),
                    repeats: Some(1),
                }],
            },
            mcp_ableton_music_maker::sections::set_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let r = server
        .run(
            &tools::PLAY_SONG,
            mcp_ableton_music_maker::sections::PlaySongParams::default(),
            mcp_ableton_music_maker::sections::play_song_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(
        text.contains("The set has moved"),
        "the drift line rides beside the clock line: {text}"
    );
}

// ── Identity ────────────────────────────────────────────────────────────────

/// AC19, AC20: identity is `song.file_path`, read through a generic `run`
/// op — no new command, no `SCRIPT_VERSION` bump — and a set that has never
/// been saved gets a provisional key whose notes are kept.
#[tokio::test]
async fn identity_comes_from_song_file_path_through_one_op() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    bridge.clear();
    remember(
        &server,
        RememberParams {
            about: Some("song".into()),
            note: Some("D Dorian, and it stays there".into()),
            ..Default::default()
        },
    )
    .await;
    // The identity was read with `run`, not with a command of its own.
    let sent = bridge.last("run").expect("the ops layer was used");
    assert_eq!(sent["ops"][0]["path"], "song.file_path");
    assert!(
        !bridge.commands().iter().any(|c| c.contains("file_path")),
        "no new Remote Script command: {:?}",
        bridge.commands()
    );

    // The note is kept against whichever identity the set has (AC20). The
    // fake's set has never been saved, so that is the provisional one; a
    // real Live may have a saved set open, and then the note belongs to
    // that song by name. What must hold on both is that the note came back.
    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(shown.contains("D Dorian, and it stays there"), "{shown}");
    if !common::targets_a_real_live() {
        assert!(shown.contains("not saved yet"), "{shown}");
    }
}

/// AC14, AC22: the overview survives a server restart and comes back
/// attached to the same song, and opening a different set switches the notes.
#[tokio::test]
async fn the_overview_survives_a_restart_and_another_set_has_its_own() {
    let _env = ENV.lock().await;
    let dir = state_dir();
    // Two songs, addressed the way the server does: by the key their path
    // gives. This is the part a restart has to reproduce.
    let smoke = memory::key_for("/Users/p/Music/Smoke Project/Smoke.als");
    let other = memory::key_for("/Users/p/Music/Other Project/Other.als");
    assert_ne!(smoke, other);

    let mut written = memory::SongMemory {
        key: smoke.clone(),
        set_path: "/Users/p/Music/Smoke Project/Smoke.als".into(),
        set_name: "Smoke".into(),
        ..Default::default()
    };
    written
        .overview
        .insert("next".into(), json!("the answer phrase an octave up"));
    memory::save(&written);

    // A fresh process: nothing in memory, everything on disk.
    let back = memory::load(&smoke).expect("the file survived");
    assert_eq!(back.overview["next"], "the answer phrase an octave up");
    assert_eq!(back.set_name, "Smoke");
    assert!(
        memory::load(&other).is_none(),
        "another set has its own notes, not these"
    );
    assert!(dir.path().join("songs").exists());
}

/// AC21: the first save renames the provisional file to the set's own key,
/// and says so once.
#[tokio::test]
async fn the_first_save_renames_the_file_and_says_so_once() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let b = FakeBridge::responding(json!({}));
    b.script(
        "get_performance_state",
        vec![json!({"tracks": [], "scenes": []})],
    );
    b.script(
        "get_context",
        vec![json!({"session": {}, "tracks": [], "scenes": []})],
    );
    // The set has not been saved yet.
    b.script("run", vec![json!({"set": ""})]);
    let server = server_with(b.clone());
    remember(
        &server,
        RememberParams {
            about: Some("song".into()),
            note: Some("keep the sitar".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(
        memory::load(memory::PROVISIONAL_KEY).is_some(),
        "filed provisionally"
    );

    // The producer presses Cmd+S: Live starts answering with a path.
    b.script(
        "run",
        vec![json!({"set": "/Users/p/Music/Smoke Project/Smoke.als"})],
    );
    let text = context(&server).await;
    assert!(
        text.contains("has been saved as 'Smoke'"),
        "the rename is reported: {text}"
    );
    let moved = memory::load(&memory::key_for("/Users/p/Music/Smoke Project/Smoke.als"))
        .expect("the notes moved to the song's own file");
    assert_eq!(moved.notes.len(), 1, "nothing was lost in the move");
    assert_eq!(moved.notes[0].note, "keep the sitar");
    assert!(
        memory::load(memory::PROVISIONAL_KEY).is_none(),
        "the provisional file is gone, not left as a second copy"
    );

    // Said once: the next get_context does not repeat it.
    let again = context(&server).await;
    assert!(!again.contains("has been saved as"), "{again}");
}

/// AC18, the case the test above cannot reach: the save happens **between
/// sessions**.
///
/// The producer builds with one client open, closes it, presses Cmd+S, and
/// comes back tomorrow — so the process that wrote the provisional notes is
/// not the process that first sees a path. Reading only the open memory
/// orphaned the provisional file and started an empty one under the saved
/// key, losing the overview at the moment the set was committed (measured
/// against Live 12.4.6 on 2026-09-20).
#[tokio::test]
async fn the_first_save_is_adopted_by_a_later_session_too() {
    let _env = ENV.lock().await;
    let _dir = state_dir();

    // Session one: an unsaved set, and something worth keeping.
    {
        let b = FakeBridge::responding(json!({}));
        b.script("run", vec![json!({"set": ""})]);
        let server = server_with(b.clone());
        remember(
            &server,
            overview(json!({"next": "the answer phrase an octave up"})),
        )
        .await;
        remember(
            &server,
            RememberParams {
                about: Some("song".into()),
                note: Some("keep the sitar".into()),
                ..Default::default()
            },
        )
        .await;
    } // the client goes away; the Server and its open memory are dropped.

    assert!(
        memory::load(memory::PROVISIONAL_KEY).is_some(),
        "filed provisionally"
    );

    // Session two, a new process in all but name: the set now has a path.
    let b = FakeBridge::responding(json!({}));
    b.script(
        "get_performance_state",
        vec![json!({"tracks": [], "scenes": []})],
    );
    b.script(
        "get_context",
        vec![json!({"session": {}, "tracks": [], "scenes": []})],
    );
    b.script(
        "run",
        vec![json!({"set": "/Users/p/Music/Smoke Project/Smoke.als"})],
    );
    let server = server_with(b.clone());
    let text = context(&server).await;

    assert!(
        text.contains("has been saved as 'Smoke'"),
        "the adoption is reported: {text}"
    );
    assert!(
        text.contains("the answer phrase an octave up"),
        "the overview came with it: {text}"
    );
    let moved = memory::load(&memory::key_for("/Users/p/Music/Smoke Project/Smoke.als"))
        .expect("the notes moved to the song's own file");
    assert_eq!(moved.notes.len(), 1, "nothing was lost in the move");
    assert_eq!(moved.notes[0].note, "keep the sitar");
    assert_eq!(moved.overview.len(), 1, "the overview moved too");
    assert!(
        memory::load(memory::PROVISIONAL_KEY).is_none(),
        "the provisional file is gone, not left as a second copy"
    );
}

/// A song that already has a memory is not waiting to be told what it is.
///
/// One `unsaved-set.json` serves the whole machine, so adopting it into any
/// set that happens to be saved next would hand one song's notes to another.
/// A set with a file of its own keeps it, and the provisional one is left
/// where it is rather than merged or deleted.
#[tokio::test]
async fn a_song_that_already_remembers_is_not_given_the_provisional_notes() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let path = "/Users/p/Music/Smoke Project/Smoke.als";

    // Smoke has its own memory, from a session of its own.
    {
        let b = FakeBridge::responding(json!({}));
        b.script("run", vec![json!({"set": path})]);
        let server = server_with(b.clone());
        remember(&server, overview(json!({"what_it_is": "Smoke's own plan"}))).await;
    }
    // …and some other unsaved set left a provisional file behind.
    {
        let b = FakeBridge::responding(json!({}));
        b.script("run", vec![json!({"set": ""})]);
        let server = server_with(b.clone());
        remember(&server, overview(json!({"what_it_is": "a different song"}))).await;
    }

    let b = FakeBridge::responding(json!({}));
    b.script(
        "get_performance_state",
        vec![json!({"tracks": [], "scenes": []})],
    );
    b.script(
        "get_context",
        vec![json!({"session": {}, "tracks": [], "scenes": []})],
    );
    b.script("run", vec![json!({"set": path})]);
    let server = server_with(b.clone());
    let text = context(&server).await;

    assert!(
        text.contains("Smoke's own plan"),
        "Smoke kept its own overview: {text}"
    );
    assert!(
        !text.contains("a different song"),
        "the other set's notes were not adopted: {text}"
    );
    assert!(
        !text.contains("has been saved as"),
        "nothing was adopted, so nothing is announced: {text}"
    );
    assert!(
        memory::load(memory::PROVISIONAL_KEY).is_some(),
        "the provisional file is left where it is, not deleted"
    );
}

/// AC23: a renamed track keeps its notes, the move is reported, and where it
/// is not certain nothing is repointed.
#[tokio::test]
async fn a_renamed_track_is_reported_and_never_repointed_by_guess() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", ""), ("Sub", "midi", "")]);
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            note: Some("the lead; it earned the ending".into()),
            ..Default::default()
        },
    )
    .await;
    remember(
        &server,
        RememberParams {
            about: Some("Sub".into()),
            note: Some("mine".into()),
            ..Default::default()
        },
    )
    .await;

    // The producer renames it in Live. One subject gone, one track nobody
    // speaks for: certain, so it is reattached and said.
    let sitar = set.track_index("Sitar").unwrap() as i64;
    let r = server
        .run(
            &tools::SET_TRACK_NAME,
            tools::SetTrackNameParams {
                track_index: sitar,
                name: "Sitar Lead".into(),
            },
            tools::set_track_name_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let text = context(&server).await;
    assert!(
        text.contains("now read as 'Sitar Lead'"),
        "the move is reported: {text}"
    );

    // Two candidates is not certain: nothing is repointed.
    let (server2, bridge2) = server_on_fake_live();
    let set2 = LiveSet::of(bridge2.as_ref());
    set2.build(&[("Sitar A", "midi", ""), ("Sitar B", "midi", "")]);
    remember(
        &server2,
        RememberParams {
            about: Some("Sitar".into()),
            note: Some("the lead".into()),
            ..Default::default()
        },
    )
    .await;
    let text = context(&server2).await;
    assert!(text.contains("not attached to a track any more"), "{text}");
    assert!(text.contains("Nothing was repointed"), "{text}");
}

// ── Roles, in the set ───────────────────────────────────────────────────────

/// AC24, AC25, AC27, AC28: the role is written into the track's name, it
/// then addresses the track, a second role replaces the suffix, and none of
/// it depends on the server's file.
#[tokio::test]
async fn a_role_is_written_into_the_track_name_and_then_addresses_it() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", ""), ("Sub", "midi", "")]);

    let r = remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            role: Some("lead".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    // AC24: it is in the set, not in a file.
    assert!(
        set.track_names().contains(&"Sitar [lead]".to_string()),
        "{:?}",
        set.track_names()
    );

    // AC25: the role addresses the track in every tool that takes one.
    let r = server
        .run(
            &tools::SET_TRACK_MIXER,
            tools::SetTrackMixerParams {
                track: Some(json!("lead")),
                kind: "track".into(),
                volume: Some(-6.0),
                ..Default::default()
            },
            tools::set_track_mixer_body,
        )
        .await;
    assert!(!is_error(&r), "'lead' addresses it: {}", text_of(&r));
    let sitar = set.track_index("Sitar [lead]").unwrap();
    assert!(set.volume_db(sitar).unwrap_or(0.0) < -3.0);

    // AC27: a second role replaces the suffix instead of stacking it.
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            role: Some("counter".into()),
            ..Default::default()
        },
    )
    .await;
    let names = set.track_names();
    assert!(names.contains(&"Sitar [counter]".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("[lead] [")), "{names:?}");
}

/// AC26: a role that would name two tracks addresses neither, so it is
/// refused and nothing is renamed.
#[tokio::test]
async fn an_ambiguous_role_names_both_tracks_and_changes_nothing() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", ""), ("Bells", "midi", "")]);
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            role: Some("lead".into()),
            ..Default::default()
        },
    )
    .await;

    let before = set.track_names();
    let r = remember(
        &server,
        RememberParams {
            about: Some("Bells".into()),
            role: Some("lead".into()),
            ..Default::default()
        },
    )
    .await;
    let text = text_of(&r);
    assert!(is_error(&r), "{text}");
    assert!(
        text.contains("already the role of 'Sitar [lead]'"),
        "{text}"
    );
    assert!(text.contains("addresses neither"), "{text}");
    assert_eq!(set.track_names(), before, "nothing was renamed");
}

// ── The stash, in the set ───────────────────────────────────────────────────

/// AC29, AC33, AC34, AC35: save makes the row, list reports what is there,
/// place copies and keeps the original, drop removes only that clip.
#[tokio::test]
async fn the_stash_saves_lists_places_and_drops_in_the_set() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", ""), ("Sub", "midi", "")]);
    let sitar = set.track_index("Sitar").unwrap();
    set.write_clip(
        sitar,
        0,
        "answer phrase",
        json!([{"pitch": 64, "start_time": 0.0, "duration": 1.0, "velocity": 100}]),
    );

    let r = stash(
        &server,
        StashParams {
            action: "save".into(),
            track: Some(json!("Sitar")),
            clip: Some(json!("answer phrase")),
            tags: Some("octave up".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    // AC29: the row is a scene whose name starts with Stash:.
    let scenes = set.scene_names();
    assert!(
        scenes.iter().any(|s| s.starts_with("Stash:")),
        "the row was made: {scenes:?}"
    );
    // The original is untouched.
    assert_eq!(set.clip_name(sitar, 0).as_deref(), Some("answer phrase"));

    // AC33: list reports track, bars, notes and tags.
    let text = text_of(
        &stash(
            &server,
            StashParams {
                action: "list".into(),
                ..Default::default()
            },
        )
        .await,
    );
    assert!(text.contains("'answer phrase' on Sitar"), "{text}");
    assert!(text.contains("1 notes"), "{text}");
    assert!(text.contains("octave up"), "the tags survive: {text}");

    // AC34: place copies into the section and leaves the parked clip.
    server
        .run(
            &tools::MAKE_SECTION,
            mcp_ableton_music_maker::sections::MakeSectionParams {
                name: "Drop".into(),
                phrase_bars: Some(8),
                // The Drop is written on Sub, so the Sitar row in it is free
                // for the parked idea to land in.
                clips: std::collections::BTreeMap::from([(
                    "Sub".to_string(),
                    json!({"notes_csv": "36,0,4,100"}),
                )]),
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    let r = stash(
        &server,
        StashParams {
            action: "place".into(),
            clip: Some(json!("answer phrase")),
            section: Some(json!("Drop")),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("still in the Stash: row"),
        "{}",
        text_of(&r)
    );
    let still = text_of(
        &stash(
            &server,
            StashParams {
                action: "list".into(),
                ..Default::default()
            },
        )
        .await,
    );
    assert!(
        still.contains("'answer phrase'"),
        "the parked copy stays: {still}"
    );

    // AC35: drop deletes only that clip.
    let r = stash(
        &server,
        StashParams {
            action: "drop".into(),
            clip: Some(json!("answer phrase")),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let after = text_of(
        &stash(
            &server,
            StashParams {
                action: "list".into(),
                ..Default::default()
            },
        )
        .await,
    );
    assert!(after.contains("Nothing is parked"), "{after}");
    assert_eq!(
        set.clip_name(sitar, 0).as_deref(),
        Some("answer phrase"),
        "the Session clip it came from is untouched"
    );
}

/// AC32: a `Stash:` row is not a section. It is never listed, launched,
/// counted or played — the same rule as the `Setlist:` scene.
#[tokio::test]
async fn a_stash_row_is_not_a_section() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    let sitar = set.track_index("Sitar").unwrap();
    set.write_clip(sitar, 0, "idea", json!([]));
    stash(
        &server,
        StashParams {
            action: "save".into(),
            track: Some(json!("Sitar")),
            clip: Some(json!("idea")),
            ..Default::default()
        },
    )
    .await;
    assert!(set.scene_names().iter().any(|s| s.starts_with("Stash:")));

    // get_context does not count it as a section: the scene list and the
    // section list never name it. (The memory header does say how many
    // ideas are parked — that is the point of it.)
    let text = context(&server).await;
    let scenes = text
        .lines()
        .find(|l| l.starts_with("Scenes:"))
        .expect("a scene line");
    assert!(!scenes.contains("Stash:"), "the row is listed: {scenes}");
    assert!(
        !text.contains("the 'Setlist:' scene reads"),
        "a Stash: row was read as the setlist: {text}"
    );

    // And make_section refuses the name outright.
    let r = server
        .run(
            &tools::MAKE_SECTION,
            mcp_ableton_music_maker::sections::MakeSectionParams {
                name: "Stash: mine".into(),
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::make_section_body,
        )
        .await;
    assert!(is_error(&r), "{}", text_of(&r));
    assert!(text_of(&r).contains("parked ideas"), "{}", text_of(&r));
}

// ── The header, and the session that remembered nothing ─────────────────────

/// AC1: the header carries the song, the roles, the last note and the stash.
#[tokio::test]
async fn context_header_lists_roles_last_note_and_stash() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    let sitar = set.track_index("Sitar").unwrap();
    set.write_clip(sitar, 0, "idea", json!([]));
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            role: Some("lead".into()),
            ..Default::default()
        },
    )
    .await;
    remember(
        &server,
        RememberParams {
            about: Some("song".into()),
            note: Some("the Drop is thin".into()),
            ..Default::default()
        },
    )
    .await;
    stash(
        &server,
        StashParams {
            action: "save".into(),
            track: Some(json!("lead")),
            clip: Some(json!("idea")),
            ..Default::default()
        },
    )
    .await;

    let text = context(&server).await;
    assert!(text.contains("Roles: lead = Sitar [lead]"), "{text}");
    assert!(text.contains("the Drop is thin"), "{text}");
    assert!(text.contains("Stash: 1 idea parked"), "{text}");
}

/// AC1: and it adds nothing when there is nothing to say.
#[tokio::test]
async fn context_header_is_absent_when_there_is_nothing_to_say() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    let text = context(&server).await;
    assert!(!text.contains("Roles:"), "{text}");
    assert!(!text.contains("Overview:"), "{text}");
    assert!(!text.contains("Stash:"), "{text}");
    assert!(!text.contains("Last note"), "{text}");
}

/// AC2: a session in which neither `remember` nor `stash` was called still
/// writes a digest, and the next `get_context` reports it.
#[tokio::test]
async fn a_session_with_no_memory_calls_still_writes_a_digest() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    // Ordinary work, no memory call at all.
    server
        .run(
            &tools::SET_TEMPO,
            tools::SetTempoParams { tempo: 140.0 },
            tools::set_tempo_body,
        )
        .await;
    context(&server).await;

    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(
        shown.contains("Sessions: ") && shown.contains("call"),
        "a session that remembered nothing still left a trace: {shown}"
    );
}

/// AC3, AC49: no `notes()` tool — the header carries it — and `CORE_TOOLS`
/// gains exactly `remember` and `stash`.
#[tokio::test]
async fn no_notes_tool_is_served_and_the_surface_gained_exactly_two() {
    let server = server_with(FakeBridge::responding(json!({})));
    let names: Vec<String> = server
        .tool_list()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(!names.contains(&"notes".to_string()), "{names:?}");
    assert!(!names.contains(&"alias".to_string()), "{names:?}");
    assert!(names.contains(&"remember".to_string()));
    assert!(names.contains(&"stash".to_string()));
    // The raw layer is marked, and the artist's set did not gain it.
    assert!(names.contains(&"adv_song_memory".to_string()), "{names:?}");
    assert!(!names.contains(&"song_memory".to_string()), "{names:?}");
    assert!(tools::CORE_TOOLS.contains(&"remember"));
    assert!(tools::CORE_TOOLS.contains(&"stash"));
    assert!(!tools::CORE_TOOLS.contains(&"song_memory"));
    assert!(!tools::CORE_TOOLS.contains(&"adv_song_memory"));
}

// ── Privacy ─────────────────────────────────────────────────────────────────

/// AC18, AC40: the off switch stops every write, and the roles and the stash
/// still work — because they are in the set, not here.
#[tokio::test]
async fn the_off_switch_stops_every_write_and_the_set_still_remembers() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::set_var("ABLETON_MCP_SONG_MEMORY", "false");

    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    let sitar = set.track_index("Sitar").unwrap();
    set.write_clip(sitar, 0, "idea", json!([]));

    // AC18: the write is refused and says so.
    let r = remember(&server, overview(json!({"next": "the bridge"}))).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("song memory is off"),
        "{}",
        text_of(&r)
    );
    assert!(
        !dir.path().join("songs").exists(),
        "nothing was written to songs/"
    );

    // AC40: the roles and the stash are in the .als and still work.
    let r = remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            role: Some("lead".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(set.track_names().contains(&"Sitar [lead]".to_string()));
    let r = stash(
        &server,
        StashParams {
            action: "save".into(),
            track: Some(json!("lead")),
            clip: Some(json!("idea")),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(set.scene_names().iter().any(|s| s.starts_with("Stash:")));
    assert!(
        !dir.path().join("songs").exists(),
        "still nothing in songs/"
    );
    std::env::remove_var("ABLETON_MCP_SONG_MEMORY");
}

/// AC41: `show` names the file, what is in it, **what is not**, the off
/// switch and how to delete it.
#[tokio::test]
async fn show_names_the_file_what_is_in_it_and_what_is_not() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    remember(&server, overview(json!({"next": "the bridge"}))).await;

    let text = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(text.contains("Overview:"), "{text}");
    assert!(text.contains("the bridge"), "{text}");
    assert!(
        text.contains("no MIDI note, no audio, no path outside the set's own"),
        "it says what it is NOT: {text}"
    );
    assert!(text.contains("ABLETON_MCP_SONG_MEMORY=false"), "{text}");
    assert!(text.contains("\"forget\""), "{text}");
    assert!(
        text.contains("are in your Live set, not here"),
        "the roles and the stash are not in this file: {text}"
    );
}

/// AC42: `forget` deletes the file and touches nothing in Live.
#[tokio::test]
async fn forget_deletes_the_file_and_sends_nothing_to_live() {
    let _env = ENV.lock().await;
    let dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            role: Some("lead".into()),
            ..Default::default()
        },
    )
    .await;
    remember(&server, overview(json!({"next": "the bridge"}))).await;
    assert!(dir.path().join("songs").exists());

    bridge.clear();
    let r = song_memory(
        &server,
        SongMemoryParams {
            action: "forget".into(),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("Nothing in Live changed"),
        "{}",
        text_of(&r)
    );
    assert!(
        bridge.commands().is_empty(),
        "not one command went to Live: {:?}",
        bridge.commands()
    );
    // AC28, AC36: the role is in the .als and survives the file going.
    assert!(set.track_names().contains(&"Sitar [lead]".to_string()));
    assert!(
        std::fs::read_dir(dir.path().join("songs"))
            .map(|d| d.count())
            .unwrap_or(0)
            == 0,
        "the file is gone"
    );
}

/// AC43: the file holds no MIDI note, no audio and no path but the set's own.
#[tokio::test]
async fn the_file_holds_no_midi_note_no_audio_and_no_foreign_path() {
    let _env = ENV.lock().await;
    let dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    let sitar = set.track_index("Sitar").unwrap();
    set.write_clip(
        sitar,
        0,
        "idea",
        json!([{"pitch": 64, "start_time": 0.0, "duration": 1.0, "velocity": 100}]),
    );
    remember(&server, overview(json!({"next": "the bridge"}))).await;
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            note: Some("the lead".into()),
            ..Default::default()
        },
    )
    .await;
    stash(
        &server,
        StashParams {
            action: "save".into(),
            track: Some(json!("Sitar")),
            clip: Some(json!("idea")),
            ..Default::default()
        },
    )
    .await;
    context(&server).await;

    let files: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path().join("songs"))
        .expect("songs/ exists")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(!files.is_empty());
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        for word in [
            "pitch",
            "velocity",
            "start_time",
            ".wav",
            ".aif",
            "notes\":[{",
        ] {
            assert!(
                !text.contains(word),
                "{} holds `{word}`: {text}",
                file.display()
            );
        }
        // The only path in the file is the set's own. The fake's set has
        // never been saved, so that is the empty string; a real Live may
        // have one open and saved, and then it is exactly that path and
        // nothing else — never a sample, a recording or a project folder.
        let set_path = parsed["set_path"].as_str().unwrap_or_default();
        if !set_path.is_empty() {
            let open = mcp_ableton_music_maker::memory::set_path_of_for_test(server.live());
            assert_eq!(set_path, open, "no path but the set's own: {text}");
        }
    }
}

// ── Starting over ───────────────────────────────────────────────────────────

/// `reset_set` empties the set back to what a new one is — which means the
/// overview, the notes, the roles and the stash were all about music that no
/// longer exists. Carrying them into the empty set would have the next agent
/// read a plan for a song that is not there, so the memory goes with it and
/// the reply says what to do instead.
#[tokio::test]
async fn reset_set_clears_the_memory_and_says_how_to_start_the_new_one() {
    let _env = ENV.lock().await;
    let dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    let sitar = set.track_index("Sitar").unwrap();
    set.write_clip(sitar, 0, "idea", json!([]));

    remember(
        &server,
        overview(json!({"what_it_is": "a dub track", "next": "the answer phrase"})),
    )
    .await;
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            note: Some("the lead".into()),
            ..Default::default()
        },
    )
    .await;
    remember(
        &server,
        RememberParams {
            about: Some("Sitar".into()),
            role: Some("lead".into()),
            ..Default::default()
        },
    )
    .await;
    stash(
        &server,
        StashParams {
            action: "save".into(),
            track: Some(json!("lead")),
            clip: Some(json!("idea")),
            ..Default::default()
        },
    )
    .await;
    assert!(context(&server).await.contains("a dub track"));

    let r = server
        .run(
            &tools::RESET_SET,
            tools::ResetSetParams::default(),
            tools::reset_set_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    // It says what went, and what to do now.
    assert!(text.contains("The memory of"), "{text}");
    assert!(text.contains("no longer exist"), "{text}");
    assert!(text.contains("This set now remembers nothing"), "{text}");
    assert!(
        text.contains("remember(overview:"),
        "the agent is told to start the new one's overview: {text}"
    );
    assert!(text.contains("set_key and set_tempo first"), "{text}");

    // The file is gone.
    assert_eq!(
        std::fs::read_dir(dir.path().join("songs"))
            .map(|d| d.count())
            .unwrap_or(0),
        0,
        "the song file went with the song"
    );
    // The set is empty, and so is the header: no roles, no stash, no
    // overview — because the tracks and the Stash: row went with the reset.
    let after = context(&server).await;
    assert!(!after.contains("a dub track"), "{after}");
    assert!(!after.contains("Roles:"), "{after}");
    assert!(!after.contains("Stash:"), "{after}");
    assert!(!after.contains("Last note"), "{after}");
    assert!(
        !set.track_names().iter().any(|n| n.contains("[lead]")),
        "{:?}",
        set.track_names()
    );

    // And the new song starts its own memory cleanly.
    let r = remember(&server, overview(json!({"what_it_is": "something else"}))).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(context(&server).await.contains("something else"));
}

/// A reset on a set that remembered nothing says nothing about a memory it
/// did not have — but still tells the agent how to start.
#[tokio::test]
async fn reset_set_with_no_memory_still_points_at_the_overview() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, _bridge) = server_on_fake_live();
    let r = server
        .run(
            &tools::RESET_SET,
            tools::ResetSetParams::default(),
            tools::reset_set_body,
        )
        .await;
    let text = text_of(&r);
    assert!(!is_error(&r), "{text}");
    assert!(!text.contains("The memory of"), "{text}");
    assert!(text.contains("This set now remembers nothing"), "{text}");
    assert!(text.contains("remember(overview:"), "{text}");
}

/// #69: placing into a row the producer has not named says which slot, not
/// "into  ".
///
/// An unnamed scene parses to an empty section name, and the reply read
/// "Placed 'sitar wide' on Sitar into  (2 notes)" — measured on Live 12.4.6,
/// 2026-09-20.
#[tokio::test]
async fn placing_into_an_unnamed_row_names_the_slot() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Sitar", "midi", "")]);
    let sitar = set.track_index("Sitar").unwrap();
    set.write_clip(
        sitar,
        0,
        "answer phrase",
        json!([{"pitch": 64, "start_time": 0.0, "duration": 1.0, "velocity": 100}]),
    );

    let r = stash(
        &server,
        StashParams {
            action: "save".into(),
            track: Some(json!("Sitar")),
            clip: Some(json!("answer phrase")),
            name: Some("sitar wide".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));

    // Slot 3 is a row nobody has named.
    let r = stash(
        &server,
        StashParams {
            action: "place".into(),
            clip: Some(json!("sitar wide")),
            slot: Some(3),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.contains("into slot 3"), "the slot is named: {t}");
    assert!(!t.contains("into  "), "no empty section name: {t}");
}

/// A section that changes its name takes the producer's words with it.
/// Renaming a row used to orphan the setlist; it orphaned these too, and
/// these are the half nobody can reconstruct.
#[tokio::test]
async fn a_renamed_section_carries_the_notes_filed_about_it() {
    let _env = ENV.lock().await;
    let _dir = state_dir();
    let (server, bridge) = server_on_fake_live();
    let set = LiveSet::of(bridge.as_ref());
    set.build(&[("Kick", "midi", "")]);
    set.ask("set_scene", json!({"index": 0, "name": "Break · 16"}));
    set.ask("set_scene", json!({"index": 1, "name": "Drop · 16"}));
    set.write_clip(
        0,
        0,
        "Break/Kick",
        json!([{"pitch": 36, "start_time": 0.0, "duration": 0.25, "velocity": 100}]),
    );
    let r = remember(
        &server,
        RememberParams {
            about: Some("section:Break".into()),
            note: Some("the filter opens over the whole 16".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));

    let r = server
        .run(
            &tools::UPDATE_SONG,
            mcp_ableton_music_maker::sections::UpdateSongParams {
                edits: vec![
                    json!({"edit": "set_section", "section": "Break", "rename_to": "Bridge"}),
                ],
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::update_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));

    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(
        shown.contains("section:Bridge"),
        "the note did not follow the rename: {shown}"
    );
    assert!(
        !shown.contains("section:Break"),
        "the note is still filed under a section that no longer exists: {shown}"
    );
    assert!(
        shown.contains("the filter opens over the whole 16"),
        "{shown}"
    );

    // Deleting a section never deletes what someone wrote about it.
    let r = server
        .run(
            &tools::UPDATE_SONG,
            mcp_ableton_music_maker::sections::UpdateSongParams {
                edits: vec![json!({"edit": "delete_section", "section": "Bridge"})],
                ..Default::default()
            },
            mcp_ableton_music_maker::sections::update_song_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert!(
        text_of(&r).contains("your 1 note about it is kept"),
        "{}",
        text_of(&r)
    );
    let shown = text_of(&song_memory(&server, SongMemoryParams::default()).await);
    assert!(
        shown.contains("the filter opens over the whole 16"),
        "{shown}"
    );
}
