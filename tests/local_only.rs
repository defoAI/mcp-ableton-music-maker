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
    assert_eq!(tools.len(), 104);
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

/// The generic layer (`describe` and `run`) lets a client on the loopback
/// socket walk Live's object model by name. What keeps that from being a way
/// to run arbitrary Python is that the script has no way to run arbitrary
/// Python: the refusals are tested in `tests/remote_script`, and here the
/// source itself is walked for the builtins that would undo them.
///
/// `open(` is allowed in exactly one place — reading the two configuration
/// files beside the script at import time, before any socket exists. A test
/// rather than a promise: a request can never reach it.
#[test]
fn the_scripts_request_path_cannot_run_arbitrary_python() {
    let script = include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");
    let mut offenders = Vec::new();
    // The configuration readers, by name: everything above the socket.
    let config_readers = ["_configured_host", "_configured_reader"];
    let mut current_fn = String::new();
    for (i, line) in script.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("");
        if let Some(rest) = code.trim_start().strip_prefix("def ") {
            current_fn = rest.split('(').next().unwrap_or("").trim().to_string();
        }
        for needle in ["eval(", "exec(", "compile(", "__import__", "getattr(__"] {
            if code.contains(needle) {
                offenders.push(format!("line {}: {needle} in {current_fn}", i + 1));
            }
        }
        if code.contains("open(") && !config_readers.contains(&current_fn.as_str()) {
            offenders.push(format!("line {}: open( in {current_fn}", i + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "the script's request path must reach Live and nothing else: {offenders:?}"
    );
    // And the refusals the generic layer rests on are stated in the source,
    // so deleting one is a visible change rather than a silent widening.
    assert!(
        script.contains("names starting with _ are refused"),
        "the dunder refusal is gone"
    );
    assert!(
        script.contains("path must start with"),
        "the root whitelist is gone"
    );
}

/// The Remote Script runs inside Live's own interpreter: Python 2.7 on Live
/// 10, 3.x on 11 and 12. Two things a modern editor reaches for by reflex
/// would stop it loading on the old one, so the source is checked for them.
#[test]
fn remote_script_stays_compatible_with_the_python_live_bundles() {
    let script = include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");
    let mut offenders = Vec::new();
    for (i, line) in script.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("");
        // f"..." / f'...' at a token boundary; the prefix is never valid 2.7.
        let bytes = code.as_bytes();
        for (j, w) in bytes.windows(2).enumerate() {
            if (w[0] == b'f' || w[0] == b'F') && (w[1] == b'"' || w[1] == b'\'') {
                let boundary =
                    j == 0 || !(bytes[j - 1].is_ascii_alphanumeric() || bytes[j - 1] == b'_');
                if boundary {
                    offenders.push(format!("line {}: f-string", i + 1));
                    break;
                }
            }
        }
        if code.trim_start().starts_with("def ") && code.contains(") ->") {
            offenders.push(format!("line {}: return annotation", i + 1));
        }
        if code.trim_start().starts_with("nonlocal ") {
            offenders.push(format!("line {}: nonlocal", i + 1));
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}
