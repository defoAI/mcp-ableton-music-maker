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
    assert_eq!(tools.len(), 110);
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
/// Python: the refusals are tested in `tests/remote_script`, and here both
/// files are walked for the builtins that would undo them.
///
/// `open(` is allowed in four named functions, and `importlib` in one. Three
/// of the four read a file at import time, before a socket exists: the two
/// configuration files beside the loader, and a dispatch read back to declare
/// what it serves. The fourth is `_load_body_module`, the reload (decision
/// 0011): it takes no argument, opens `body.py` beside the loader and nothing
/// else, and the request that reaches it carries no path and no source — a
/// test rather than a promise, in `tests/remote_script/test_reload.py`.
///
/// The **body** — every handler a request can reach — has none of them at all.
#[test]
fn the_scripts_request_path_cannot_run_arbitrary_python() {
    let loader = include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");
    let body = include_str!("../AbletonMusicMaker_Remote_Script/body.py");
    let mut offenders = Vec::new();
    // The four functions that read a file, and the one that imports one.
    let file_readers = [
        "_configured_host",
        "_configured_reader",
        "_served_commands",
        "_load_body_module",
    ];
    for (file, source) in [("__init__.py", loader), ("body.py", body)] {
        let mut current_fn = String::new();
        for (i, line) in source.lines().enumerate() {
            let code = line.split('#').next().unwrap_or("");
            if let Some(rest) = code.trim_start().strip_prefix("def ") {
                current_fn = rest.split('(').next().unwrap_or("").trim().to_string();
            }
            for needle in ["eval(", "exec(", "compile(", "__import__", "getattr(__"] {
                // Only the bare builtin counts: `re.compile` and `self.exec_x`
                // are qualified names, not a way to run source.
                for (at, _) in code.match_indices(needle) {
                    // Qualified (`re.compile`) or part of a longer identifier
                    // (`_my_eval(`) is not the builtin; anything else is.
                    let qualified = at > 0 && {
                        let prev = code.as_bytes()[at - 1];
                        prev == b'.' || prev == b'_' || prev.is_ascii_alphanumeric()
                    };
                    if !qualified {
                        offenders.push(format!("{file} line {}: {needle} in {current_fn}", i + 1));
                    }
                }
            }
            let allowed_here = file == "__init__.py" && file_readers.contains(&current_fn.as_str());
            if code.contains("open(") && !allowed_here {
                offenders.push(format!("{file} line {}: open( in {current_fn}", i + 1));
            }
            // `importlib` belongs to the reload alone, and the reload belongs
            // to the loader alone. In the body it is banned outright.
            let importer = file == "__init__.py"
                && (current_fn.is_empty() || current_fn == "_load_body_module");
            if code.contains("importlib") && !importer {
                offenders.push(format!("{file} line {}: importlib in {current_fn}", i + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the script's request path must reach Live and nothing else: {offenders:?}"
    );
    // And the refusals the generic layer rests on are stated in the source,
    // so deleting one is a visible change rather than a silent widening.
    assert!(
        body.contains("names starting with _ are refused"),
        "the dunder refusal is gone"
    );
    assert!(
        body.contains("path must start with"),
        "the root whitelist is gone"
    );
    // The reload carries neither a path nor source, and is loopback only.
    assert!(
        loader.contains("reload_body takes no parameters"),
        "the reload started accepting arguments"
    );
    assert!(
        loader.contains("the reload is\nloopback only") || loader.contains("loopback only"),
        "the reload's loopback refusal is gone"
    );
}

/// Every statement in the loader is one that needs Live restarted to change.
/// That is the whole reason the script is two files, so the loader's size is a
/// ratchet: a handler that drifts back into it takes its fixability with it.
///
/// The number counts **statements** — blank lines, comments and docstrings are
/// excluded, because changing one of those changes nothing a restart would
/// have to pick up, and the loader is commented at length on purpose. A line
/// total is asserted too, so it cannot grow without bound in prose either.
///
/// The story that split the file (decision 0011) proposed 600. The split as
/// built is 782 statements, because the executor, the framing and the client
/// drain are all things Live holds; 600 was an estimate made before the
/// measurement, and this is the measurement.
#[test]
fn the_loader_stays_small_because_every_line_in_it_needs_a_restart() {
    // Measured 2026-09-21: 802 statements in 1,230 lines, against 5,680 in
    // the body. Raise these only with a reason in the PR that says why the
    // new code cannot live in `body.py`. The twenty since the split are the
    // `Tick` sentinel and the executor branch that honours it (#87) — only
    // the executor can hand Live's frame back, so a body cannot wait for one
    // on its own — and the `NEEDS_LOADER` check moving *before* the body is
    // executed, which has to be in the file doing the executing.
    const MAX_STATEMENTS: usize = 815;
    const MAX_LINES: usize = 1260;

    let loader = include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");
    let statements = python_statements(loader);
    let lines = loader.lines().count();
    assert!(
        statements <= MAX_STATEMENTS,
        "the loader has {statements} statements (limit {MAX_STATEMENTS}). Every one of \
         them needs Live restarted to change: put it in body.py."
    );
    assert!(
        lines <= MAX_LINES,
        "the loader is {lines} lines (limit {MAX_LINES})"
    );
    // And the split is real: the body is where the weight is.
    let body = include_str!("../AbletonMusicMaker_Remote_Script/body.py");
    assert!(
        python_statements(body) > statements * 3,
        "the loader is no longer small beside the body"
    );
}

/// Lines that are neither blank, nor a comment, nor inside a docstring.
/// Good enough for a ratchet: it counts what the file does.
fn python_statements(source: &str) -> usize {
    let triple_double = "\"\"\"";
    let triple_single = "'''";
    let mut statements = 0usize;
    let mut open_quote: Option<&str> = None;
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(quote) = open_quote {
            if trimmed.contains(quote) {
                open_quote = None;
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let bare = trimmed.trim_start_matches(['r', 'u', 'b']);
        let opener = if bare.starts_with(triple_double) {
            Some(triple_double)
        } else if bare.starts_with(triple_single) {
            Some(triple_single)
        } else {
            None
        };
        if let Some(quote) = opener {
            // A one-line docstring closes on the same line; anything else
            // runs on until the closing quotes.
            if !bare[quote.len()..].contains(quote) {
                open_quote = Some(quote);
            }
            continue;
        }
        statements += 1;
    }
    statements
}

/// The loader answers four commands and holds no handler. A `_get_`, `_set_`
/// or `_create_` method appearing there is a handler that quietly stopped
/// being fixable without a restart.
#[test]
fn the_loader_holds_no_handler() {
    let loader = include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");
    assert!(
        !loader.contains("def _dispatch("),
        "the dispatch belongs to the body"
    );
    let handlerish: Vec<&str> = loader
        .lines()
        .filter_map(|l| l.trim().strip_prefix("def "))
        .map(|rest| rest.split('(').next().unwrap_or("").trim())
        .filter(|name| {
            [
                "_get_", "_set_", "_create_", "_delete_", "_fire_", "_start_", "_stop_",
            ]
            .iter()
            .any(|p| name.starts_with(p))
        })
        // Three exceptions, each one the loader's by necessity: the handshake
        // must answer when the body cannot, and the socket is the loader's.
        .filter(|name| !["_get_script_info", "_start_server"].contains(name))
        .collect();
    assert!(
        handlerish.is_empty(),
        "these look like handlers and belong in body.py: {handlerish:?}"
    );
}

/// The Remote Script runs inside Live's own interpreter: Python 2.7 on Live
/// 10, 3.x on 11 and 12. Two things a modern editor reaches for by reflex
/// would stop it loading on the old one, so the source is checked for them.
#[test]
fn remote_script_stays_compatible_with_the_python_live_bundles() {
    let mut offenders = Vec::new();
    for (file, script) in [
        (
            "__init__.py",
            include_str!("../AbletonMusicMaker_Remote_Script/__init__.py"),
        ),
        (
            "body.py",
            include_str!("../AbletonMusicMaker_Remote_Script/body.py"),
        ),
    ] {
        for (i, line) in script.lines().enumerate() {
            let code = line.split('#').next().unwrap_or("");
            // f"..." / f'...' at a token boundary; the prefix is never valid 2.7.
            let bytes = code.as_bytes();
            for (j, w) in bytes.windows(2).enumerate() {
                if (w[0] == b'f' || w[0] == b'F') && (w[1] == b'"' || w[1] == b'\'') {
                    let boundary =
                        j == 0 || !(bytes[j - 1].is_ascii_alphanumeric() || bytes[j - 1] == b'_');
                    if boundary {
                        offenders.push(format!("{file} line {}: f-string", i + 1));
                        break;
                    }
                }
            }
            if code.trim_start().starts_with("def ") && code.contains(") ->") {
                offenders.push(format!("{file} line {}: return annotation", i + 1));
            }
            if code.trim_start().starts_with("nonlocal ") {
                offenders.push(format!("{file} line {}: nonlocal", i + 1));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}
