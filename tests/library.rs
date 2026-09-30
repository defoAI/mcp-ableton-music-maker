//! The browser index: paged out of the Remote Script, kept on disk under the
//! state dir (off with ABLETON_MCP_LIBRARY_INDEX=false), searched locally
//! with no round trip once complete, many queries per call.

mod common;

use common::{is_error, server_with, text_of, FakeBridge};
use mcp_ableton_music_maker::library;
use mcp_ableton_music_maker::tools::{self, SearchBrowserParams};
use serde_json::json;
use std::sync::LazyLock;

/// `ABLETON_MCP_LIBRARY_INDEX` and `ABLETON_MCP_STATE_DIR` belong to the
/// process, not to a test, and the harness runs these on several threads at
/// once. Without this every run is a coin toss: one test clearing the index
/// switch while another is relying on it turned off makes the search take the
/// index path and the assertion read the wrong command. A tokio mutex rather
/// than a std one because it is held across awaits.
static ENV: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn page(offset: usize, items: Vec<(&str, &str, &str)>, complete: bool) -> serde_json::Value {
    json!({
        "category": "all", "offset": offset, "returned": items.len(), "total_walked": offset + items.len(),
        "index_complete": complete, "library_key": "live-12.4.6-packs-3-abc",
        "items": items.iter().map(|(n, p, u)| json!({"name": n, "path": p, "uri": u, "category": "sounds", "is_device": false})).collect::<Vec<_>>()
    })
}

fn params(query: &str) -> SearchBrowserParams {
    SearchBrowserParams {
        query: query.into(),
        queries: vec![],
        category: "all".into(),
        limit: 30,
        best: false,
        refresh: false,
    }
}

#[tokio::test]
async fn warm_up_pages_the_walk_and_search_needs_no_round_trip() {
    let _env = ENV.lock().await;

    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
    let b = FakeBridge::responding(json!({}));
    b.script(
        "get_browser_index",
        vec![
            page(
                0,
                vec![
                    ("Analog Bass Warm", "Sounds/Bass/Analog Bass Warm", "u1"),
                    ("Techno Kit 909", "Drums/Kits/Techno Kit 909", "u2"),
                ],
                false,
            ),
            page(
                2,
                vec![("Evolving Pad", "Sounds/Pad/Evolving Pad", "u3")],
                true,
            ),
        ],
    );
    let server = server_with(b.clone());
    let live = server.live();
    assert!(
        !library::warm_up_step(live, 1.0).unwrap(),
        "first page: not complete"
    );
    assert_eq!(b.sent()[0].1["offset"], 0);
    assert!(
        library::warm_up_step(live, 1.0).unwrap(),
        "second page completes it"
    );
    assert_eq!(b.sent()[1].1["offset"], 2);
    let ix = live.library.snapshot().unwrap();
    assert_eq!(ix.items.len(), 3);
    assert!(ix.complete);
    assert!(
        dir.path()
            .join("library")
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "saved under <state>/library/"
    );

    // A complete index answers searches without a command.
    let before = b.commands().len();
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            params("analog bass"),
            tools::search_browser_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands().len(), before, "no round trip");
    let t = text_of(&r);
    assert!(
        t.starts_with("From the library index: 3 items, complete"),
        "{t}"
    );
    assert!(
        t.contains("Analog Bass Warm — Sounds/Bass/Analog Bass Warm\n    uri: u1"),
        "{t}"
    );

    // Several queries, one best hit each, one call, no command.
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                query: String::new(),
                queries: vec!["techno kit".into(), "evolving pad".into(), "piano".into()],
                category: "all".into(),
                limit: 30,
                best: true,
                refresh: false,
            },
            tools::search_browser_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(b.commands().len(), before);
    let t = text_of(&r);
    assert!(
        t.contains("techno kit   → 'Techno Kit 909'  u2  (Drums/Kits/Techno Kit 909)"),
        "{t}"
    );
    assert!(t.contains("evolving pad → 'Evolving Pad'  u3"), "{t}");
    assert!(t.contains("piano        → nothing\n"), "{t}");

    // A new server process loads the disk copy on its first page.
    let b2 = FakeBridge::responding(json!({}));
    b2.script("get_browser_index", vec![page(0, vec![], false)]);
    let server2 = server_with(b2.clone());
    assert!(
        library::warm_up_step(server2.live(), 1.0).unwrap(),
        "disk copy with the same key is complete"
    );
    assert_eq!(server2.live().library.snapshot().unwrap().items.len(), 3);

    // refresh drops it; the off switch keeps it in memory only.
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                refresh: true,
                ..params("")
            },
            tools::search_browser_body,
        )
        .await;
    assert!(
        text_of(&r).starts_with("Library index dropped"),
        "{}",
        text_of(&r)
    );
    assert!(live.library.snapshot().is_none());
    std::env::set_var("ABLETON_MCP_LIBRARY_INDEX", "false");
    library::warm_up_step(live, 1.0).ok();
    library::warm_up_step(live, 1.0).ok();
    assert!(
        !dir.path().join("library").exists()
            || dir
                .path()
                .join("library")
                .read_dir()
                .unwrap()
                .next()
                .is_none(),
        "nothing written when off"
    );
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
    std::env::remove_var("ABLETON_MCP_STATE_DIR");
}

#[tokio::test]
async fn search_falls_back_to_live_while_the_index_walks() {
    let _env = ENV.lock().await;

    std::env::set_var("ABLETON_MCP_LIBRARY_INDEX", "false");
    let b = FakeBridge::responding(json!({}));
    b.script(
        "get_browser_index",
        vec![page(
            0,
            vec![("Analog Bass Warm", "Sounds/Bass/Analog Bass Warm", "u1")],
            false,
        )],
    );
    b.script(
        "search_browser",
        vec![json!({"items": [{"name": "Bass Station", "path": "Sounds/Bass/Bass Station", "uri": "u9", "category": "sounds"}], "returned": 1, "total_matches": 1, "category": "all"})],
    );
    let server = server_with(b.clone());
    library::warm_up_step(server.live(), 1.0).unwrap();
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            params("bass"),
            tools::search_browser_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(
        b.commands().last().unwrap(),
        "search_browser",
        "incomplete index: Live is asked"
    );
    let t = text_of(&r);
    assert!(
        t.starts_with("From Live plus the library index (1 items so far, walking)"),
        "{t}"
    );
    assert!(
        t.contains("Bass Station") && t.contains("Analog Bass Warm"),
        "merged: {t}"
    );
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
}

/// #68: `index_complete` means the browser *walk* finished, not that this
/// page is all of it.
///
/// The script walks the whole library, then answers with
/// `items[offset:offset + limit]` and reports the real count in
/// `total_walked`. Reading the flag as "done" stopped the paging after one
/// page: on a real Live 12.4.6 Suite that left exactly 1000 items on disk
/// marked complete, so `build_song {"instrument": "Boom Bap Kit"}` and
/// `load_instrument_or_effect {"uri": "reverb"}` both answered "nothing in
/// the browser matches" against a library that had them (measured
/// 2026-09-20).
#[tokio::test]
async fn a_finished_walk_is_still_paged_to_the_end() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    // The walk is finished from the very first reply — and there are three
    // items behind it, of which each page carries one.
    let walked = 3;
    let full = |offset: usize, items: Vec<(&str, &str, &str)>| {
        json!({
            "category": "all", "offset": offset, "returned": items.len(),
            "total_walked": walked, "index_complete": true,
            "library_key": "live-12.4.6-packs-7-xyz",
            "items": items.iter().map(|(n, p, u)| json!({
                "name": n, "path": p, "uri": u, "category": "drums", "is_device": false
            })).collect::<Vec<_>>()
        })
    };
    let b = FakeBridge::responding(json!({}));
    b.script(
        "get_browser_index",
        vec![
            full(0, vec![("Boom Bap Kit", "Drums/Boom Bap Kit.adg", "u1")]),
            full(1, vec![("Reverb", "Audio Effects/Reverb", "u2")]),
            full(2, vec![("EQ Eight", "Audio Effects/EQ Eight", "u3")]),
        ],
    );
    let server = server_with(b.clone());
    let live = server.live();

    assert!(
        !library::warm_up_step(live, 1.0).unwrap(),
        "the walk is finished but only 1 of 3 items is here: not complete"
    );
    assert!(!library::warm_up_step(live, 1.0).unwrap(), "2 of 3");
    assert!(
        library::warm_up_step(live, 1.0).unwrap(),
        "all three fetched: now it is complete"
    );

    assert_eq!(b.sent()[1].1["offset"], 1, "it asked for the next page");
    assert_eq!(b.sent()[2].1["offset"], 2);
    let ix = live.library.snapshot().unwrap();
    assert_eq!(ix.items.len(), 3, "nothing past the first page was dropped");
    assert!(ix.complete);
    assert!(
        ix.items.iter().any(|i| i.name == "Reverb"),
        "the item the producer asked for is in the index"
    );
    std::env::remove_var("ABLETON_MCP_STATE_DIR");
}

/// A truncated index written by an older build is re-walked, not trusted.
///
/// The 1000-item file it left behind says `complete`, so without this every
/// later session would keep answering from a library that is mostly missing.
#[tokio::test]
async fn an_index_from_an_older_build_is_not_believed() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    let key = "live-12.4.6-packs-7-old";
    let stale = json!({
        "key": key, "complete": true, "walked_at": "17:26",
        "items": [{"name": "Analog", "path": "Instruments/Analog", "uri": "u0",
                   "category": "sounds", "is_device": true}]
        // no "format": written before the field existed
    });
    let libdir = dir.path().join("library");
    std::fs::create_dir_all(&libdir).unwrap();
    std::fs::write(
        libdir.join(format!("{key}.json")),
        serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();

    assert!(
        library::load_from_disk(key).is_none(),
        "a format-less file is not loaded"
    );
    std::env::remove_var("ABLETON_MCP_STATE_DIR");
}

#[test]
fn index_matching_agrees_with_the_remote_scripts_rule() {
    // The script matches every query word against the lower-cased path
    // (which ends in the name); the index must accept the same items.
    let items = [
        ("Analog Bass Warm", "Sounds/Bass/Analog Bass Warm", "u1"),
        ("909 Kit", "Drums/Techno/909 Kit", "u2"),
        ("Evolving Pad", "Sounds/Pad/Evolving Pad", "u3"),
    ];
    let ix = library::Index {
        key: "k".into(),
        complete: true,
        walked_at: "now".into(),
        format: library::INDEX_FORMAT,
        items: items
            .iter()
            .map(|(n, p, u)| library::Item {
                name: n.to_string(),
                path: p.to_string(),
                uri: u.to_string(),
                category: "x".into(),
                is_device: false,
            })
            .collect(),
        ..Default::default()
    };
    for query in ["analog bass", "techno kit", "pad", "bass warm", "kit 909"] {
        let script_rule: Vec<&str> = items
            .iter()
            .filter(|(_, p, _)| {
                query
                    .split_whitespace()
                    .all(|w| p.to_lowercase().contains(&w.to_lowercase()))
            })
            .map(|(_, _, u)| *u)
            .collect();
        let mut ours: Vec<&str> = library::search(&ix, query, "all", 10)
            .iter()
            .map(|i| i.uri.as_str())
            .collect();
        ours.sort();
        let mut theirs = script_rule.clone();
        theirs.sort();
        assert_eq!(ours, theirs, "{query}");
    }
}

// ── Plug-ins and Max for Live (#73) ─────────────────────────────────────────

/// The plug-in fixture is the model's: `Serum`, `Pro-Q 3`, `Poli.amxd`, and
/// the synthetic vendor folder behind `--plugins N`. A real Live has whatever
/// the producer installed, which on the machine these were written on is
/// nothing (`browser.plugins.children` is empty).
///
/// So against a real Live these check the **walk** — that it ran, that it
/// marked itself walked, that what it found is what Live lists — and leave
/// the fixture's names to the model. Asserting "Serum" against someone's Live
/// would be the suite pretending, which is the one thing the fake is not
/// allowed to do (#73 stays open on a machine that has a plug-in installed).
fn against_a_real_live() -> bool {
    common::targets_a_real_live()
}

/// A VST, an AU and a Max for Live device are in Live's browser and were
/// invisible to every search: `BROWSER_CATEGORIES` in the Remote Script has
/// neither root, so `search_browser` could never return one and the words a
/// producer would type ("Serum") found nothing.
///
/// This runs against the **real Remote Script** on the model, over a real
/// socket: the walk is the thing under test, and a `FakeBridge` answering
/// with the items the code wants would prove nothing about it.
#[tokio::test]
async fn the_plugin_walk_finds_a_vst_an_au_and_a_max_device() {
    let _env = ENV.lock().await;
    let (server, _bridge) = common::server_on_fake_live();
    let live = server.live();

    let found =
        library::walk_plugins(live, library::PLUGIN_WALK_BUDGET).expect("the plugin roots walk");
    let ix = live.library.snapshot().expect("the index exists");
    assert!(ix.plugins_walked, "the walk did not mark itself done");
    if against_a_real_live() {
        // Whatever this Live lists is the right answer, including nothing.
        assert_eq!(
            found,
            ix.plugins.len(),
            "the count and the index disagree: {found} vs {}",
            ix.plugins.len()
        );
        return;
    }
    assert!(found >= 3, "a VST3, an AU and two Max devices: {found}");

    let serum = ix
        .plugins
        .iter()
        .find(|i| i.name == "Serum")
        .expect("Serum is in the walk");
    assert_eq!(serum.category, "plugins");
    assert_eq!(serum.uri, "vst3:instr:Serum");
    assert_eq!(
        serum.path, "VST3/Xfer Records/Serum",
        "the folder path Live shows, so a search can match on the vendor too"
    );
    assert!(ix
        .plugins
        .iter()
        .any(|i| i.name == "Pro-Q 3" && i.uri.starts_with("au:")));
    assert!(ix
        .plugins
        .iter()
        .any(|i| i.category == "max_for_live" && i.name == "Poli.amxd"));
}

/// The words a producer types reach a plug-in: by its own category, and under
/// "all", which is what `load_instrument_or_effect` searches.
#[tokio::test]
async fn a_plugin_is_searchable_by_name_and_loadable_by_the_uri_that_search_returned() {
    let _env = ENV.lock().await;
    if against_a_real_live() {
        // The name is the model's. See `against_a_real_live`.
        return;
    }
    let (server, bridge) = common::server_on_fake_live();
    let set = common::LiveSet::of(bridge.as_ref());
    set.build(&[("Lead", "midi", "")]);
    library::walk_plugins(server.live(), library::PLUGIN_WALK_BUDGET)
        .expect("the plugin roots walk");

    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                query: "serum".into(),
                category: "plugins".into(),
                ..params("serum")
            },
            tools::search_browser_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("vst3:instr:Serum"), "{t}");

    // And under "all", because that is what a producer means by "Serum".
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            params("serum"),
            tools::search_browser_body,
        )
        .await;
    assert!(text_of(&r).contains("vst3:instr:Serum"), "{}", text_of(&r));

    // The URI the search returned loads, which is the half that was never
    // reachable: the script's own load path already walks browser.plugins.
    let r = server
        .run(
            &tools::LOAD_INSTRUMENT_OR_EFFECT,
            tools::LoadInstrumentParams {
                track: Some(json!("Lead")),
                uri: "vst3:instr:Serum".into(),
                kind: "track".into(),
                ..Default::default()
            },
            tools::load_instrument_or_effect_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("Serum"), "{t}");

    // …and by plain words, the way build_song resolves an instrument.
    let r = server
        .run(
            &tools::LOAD_INSTRUMENT_OR_EFFECT,
            tools::LoadInstrumentParams {
                track: Some(json!("Lead")),
                uri: "Pro-Q 3".into(),
                kind: "track".into(),
                ..Default::default()
            },
            tools::load_instrument_or_effect_body,
        )
        .await;
    assert!(!is_error(&r), "{}", text_of(&r));
}

/// A producer with no plug-ins installed and a producer whose Live has not
/// scanned them are two different people, and the reply says which (#73).
#[tokio::test]
async fn nothing_found_says_whether_live_itself_lists_any_plugins() {
    let _env = ENV.lock().await;
    let (server, _bridge) = common::server_on_fake_live();
    library::walk_plugins(server.live(), library::PLUGIN_WALK_BUDGET)
        .expect("the plugin roots walk");
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                query: "massive".into(),
                category: "plugins".into(),
                ..params("massive")
            },
            tools::search_browser_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("Nothing in the plugins browser matches"), "{t}");
    // Which of the two answers is right depends on the machine: the model has
    // a Serum and a Pro-Q 3, a real Live has whatever the producer installed —
    // on the one this was measured against (12.4.6, 2026-09-21) Live lists no
    // plug-ins at all. The point of the message is that it says *which*, so
    // that is what is asserted, in both directions.
    let blames_the_words = t.contains("indexed, so the words are what missed");
    let blames_lives_scan = t.contains("lists nothing under Plug-Ins");
    assert!(
        blames_the_words != blames_lives_scan,
        "it has to say whether Live lists any plug-ins, and say only one of the two: {t}"
    );

    // And the status tool reports both counts — the server's and Live's own.
    let r = server
        .run(
            &tools::GET_LIBRARY_STATUS,
            tools::Empty::default(),
            tools::get_library_status_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("Plug-ins and Max for Live:"), "{t}");
    assert!(t.contains("Live's browser has"), "{t}");
}

/// "Nothing matches" on a Live with no plug-ins scanned says so, and does not
/// blame the words.
///
/// The counts are per category, not across both: `plugins` and `max_for_live`
/// share one index, and counting the pair told a producer searching for a VST
/// that "3 item(s) are indexed, so the words are what missed" on a machine
/// whose Plug-Ins folder Live lists as empty and whose three items are all Max
/// for Live. Measured on exactly that machine (Live 12.4.6, 2026-09-21) — it
/// is the one thing #73 exists to tell apart.
#[tokio::test]
async fn an_empty_plugin_scan_is_not_blamed_on_the_words() {
    let _env = ENV.lock().await;
    let (server, _bridge) =
        common::server_on_fake_live_with(&["--latency", "none", "--no-plugins"]);
    library::walk_plugins(server.live(), library::PLUGIN_WALK_BUDGET)
        .expect("the plugin roots walk");
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                query: "serum".into(),
                category: "plugins".into(),
                ..params("serum")
            },
            tools::search_browser_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains("lists nothing under Plug-Ins"),
        "it blamed the words instead of Live's scan: {t}"
    );
    assert!(
        !t.contains("so the words are what missed"),
        "it blamed the words: {t}"
    );
    // And it says the other root is not empty, so the producer knows the walk
    // itself ran.
    assert!(t.contains("Max for Live device(s)"), "{t}");
    // The same search under Max for Live still finds things: the index is
    // there, and only one of its two roots is empty.
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                query: "max".into(),
                category: "max_for_live".into(),
                ..params("max")
            },
            tools::search_browser_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(t.contains("match"), "{t}");
}

/// An index on disk that predates the plugin walk describes a library that is
/// missing a whole category, so it is walked again rather than believed (#68's
/// mechanism, one format on).
#[tokio::test]
async fn an_index_written_before_plugins_were_walked_is_not_believed() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
    std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");

    let key = "live-12.4.6-packs-3-abc";
    let before_plugins = json!({
        "key": key, "complete": true, "walked_at": "17:26", "format": 2,
        "items": [{"name": "Analog", "path": "Instruments/Analog", "uri": "u0",
                   "category": "instruments", "is_device": true}]
    });
    let libdir = dir.path().join("library");
    std::fs::create_dir_all(&libdir).unwrap();
    std::fs::write(
        libdir.join(format!("{key}.json")),
        serde_json::to_vec(&before_plugins).unwrap(),
    )
    .unwrap();
    assert!(
        library::load_from_disk(key).is_none(),
        "format 2 knew nothing of plug-ins; it is re-walked"
    );
    std::env::remove_var("ABLETON_MCP_STATE_DIR");
}

/// The background pass walks the plugin roots after Live's own categories,
/// but a producer may search before it has run — or after `refresh: true`
/// dropped the index. Nothing else can answer a plugin search (the script's
/// `search_browser` has no such category), so the search does the walk.
#[tokio::test]
async fn a_plugin_search_walks_the_roots_when_nothing_else_has() {
    let _env = ENV.lock().await;
    if against_a_real_live() {
        // The name is the model's. See `against_a_real_live`.
        return;
    }
    let (server, _bridge) = common::server_on_fake_live();
    assert!(
        server
            .live()
            .library
            .snapshot()
            .is_none_or(|ix| !ix.plugins_walked),
        "nothing has walked them yet"
    );
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                query: "serum".into(),
                category: "plugins".into(),
                ..params("serum")
            },
            tools::search_browser_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains("vst3:instr:Serum"),
        "the search walked them: {t}"
    );
}

/// A folder with more plug-ins than the script can render in one read.
///
/// `_jsonable` renders a Live sequence as at most 256 entries, so a vendor
/// folder of 400 would be walked as 256 and the rest would read as not
/// installed — the same shape of bug as #68, one layer down. The walk counts
/// past the cap by probing indices, and the last plug-in in the folder is
/// findable.
#[tokio::test]
async fn a_folder_with_more_children_than_one_read_can_carry_is_walked_whole() {
    let _env = ENV.lock().await;
    if against_a_real_live() {
        // `--plugins N` is a knob on the model. A real Live has the folders
        // the producer installed, and no way to be asked for 300.
        return;
    }
    let (server, _bridge) =
        common::server_on_fake_live_with(&["--latency", "none", "--plugins", "300"]);
    let found = library::walk_plugins(server.live(), library::PLUGIN_WALK_BUDGET)
        .expect("the plugin roots walk");
    assert!(found >= 300, "300 in the folder plus the rest: {found}");
    let r = server
        .run(
            &tools::SEARCH_BROWSER,
            SearchBrowserParams {
                query: "plug 299".into(),
                category: "plugins".into(),
                ..params("plug 299")
            },
            tools::search_browser_body,
        )
        .await;
    let t = text_of(&r);
    assert!(!is_error(&r), "{t}");
    assert!(
        t.contains("Plug 299"),
        "the last one past the 256-entry cap: {t}"
    );
}
