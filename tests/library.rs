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
