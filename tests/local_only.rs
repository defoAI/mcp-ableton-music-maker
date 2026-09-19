//! Local data only: the server has no upload path. This file is the policy.
//!
//! The dependency-tree gate in CI (`cargo tree` must list no HTTP client) is
//! the other half; together they make "nothing leaves your machine" a
//! property of every build rather than a default.

mod common;

use common::{server_with, FakeBridge};
use serde_json::json;

const REMOVED_TOOLS: &[&str] = &[
    "set_dataset_consent",
    "submit_intent",
    "rate_last_action",
    "prefer_candidate",
    "reject_last_action",
    "record_audition",
];

const REMOVED_VARIABLES: &[&str] = &[
    "ABLETON_MCP_SUPABASE_URL",
    "ABLETON_MCP_SUPABASE_ANON_KEY",
    "ABLETON_MCP_ENABLE_TELEMETRY",
    "ABLETON_MCP_DISABLE_TELEMETRY",
    "ABLETON_MCP_ENABLE_DATASET",
    "ABLETON_MCP_DISABLE_DATASET",
    "ABLETON_MCP_TELEMETRY_CONSENT",
];

#[test]
fn no_dataset_tool_is_served() {
    let bridge = FakeBridge::responding(json!({}));
    let server = server_with(bridge);
    let tools = server.tool_list();
    for name in REMOVED_TOOLS {
        assert!(
            !tools.iter().any(|t| t.name == *name),
            "{name} is still served"
        );
    }
    assert_eq!(tools.len(), 101);
}

#[test]
fn source_reads_none_of_the_removed_variables() {
    // The crate's own source is the evidence: no module names a removed
    // variable, so no environment can turn an upload on.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    for entry in walk(&root) {
        let text = std::fs::read_to_string(&entry).unwrap();
        for var in REMOVED_VARIABLES {
            if text.contains(var) {
                offenders.push(format!("{}: {var}", entry.display()));
            }
        }
        for word in ["supabase", "Supabase"] {
            if text.contains(word) {
                offenders.push(format!("{}: {word}", entry.display()));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}

#[test]
fn status_names_no_upload() {
    let status = mcp_ableton_music_maker::app::status();
    assert_eq!(status["uploads"], "none");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// Decision 0003: the Remote Script listens on this machine only unless a
/// `bind_host.txt` beside it says otherwise. A default of `0.0.0.0` would put
/// Live, which has no authentication on the socket, on the LAN.
#[test]
fn remote_script_binds_loopback_by_default() {
    let script = include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");
    assert!(
        script.contains("DEFAULT_HOST = \"127.0.0.1\""),
        "the script's DEFAULT_HOST must be loopback"
    );
    assert!(
        script.contains("HOST = _configured_host()"),
        "HOST must come from the resolver, not a literal"
    );
    for line in script.lines() {
        let code = line.split('#').next().unwrap_or("").trim();
        assert!(
            !(code.starts_with("HOST") && code.contains("0.0.0.0")),
            "the script must not bind 0.0.0.0 by default: {line}"
        );
    }
    // The escape hatch is documented where someone looking at the port finds it.
    assert!(script.contains("bind_host.txt"));
}
