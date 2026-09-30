//! mcp-ableton-music-maker: Ableton Live integration through the Model Context Protocol.
//!
//! Two processes make up the product. This crate is the MCP server side: it
//! speaks MCP over stdio to a client (Claude Desktop, Claude Code, Cursor) and
//! a small JSON-over-TCP protocol to the AbletonMusicMaker Remote Script running
//! inside Live. The Remote Script itself must stay Python (Live only loads
//! control surfaces through its embedded interpreter); its source is embedded
//! in this binary and installed by `ableton-music-maker-install-script`.
//!
//! The server opens exactly one kind of socket: the one to Live. There is no
//! upload path, and CI fails on an HTTP client in the dependency tree.
//!
//! Module map:
//! - [`connection`]   AbletonConnection, the TCP bridge and reconnect logic
//! - [`handshake`]    Remote Script version / capability negotiation
//! - [`tools`]        the MCP server and all tools
//! - [`notes`]        compact note forms (csv, step strings, patterns, tiling)
//! - [`audio`]        reads the WAV/AIFF Live recorded for a capture and measures it
//! - [`performance`]  live performance: state, bar arithmetic, cue resolution, the readout text
//! - [`context`]      get_context's readout and the server instructions clients receive at initialize
//! - [`lom`]        Live's object model reached generically: built paths, batches of ops, what this Live has (pure + cached)
//! - [`library`]      the server's copy of Live's browser: paged from the script, on disk, searched locally
//! - [`devices`]      what a device answered to, keyed on the device and the Live version, not on a song
//! - [`memory`]       the song's own memory: the overview, the notes, the digest, identity from song.file_path
//! - [`variation`]    clip variations and the key of a recording (pure)
//! - [`song`]         sections (scene names) and songs (the Setlist: scene): parsing, the plan, the cursor (pure)
//! - [`sections`]     the section and song tools: make_section, set_song, play_song, the steering verbs
//! - [`transition`]   a jump's transition (tempo, retime, crossfade, fill, drop, sweep) as cue primitives
//! - [`sound`]        the sound vocabulary: words → a device's rack macros, the instrument table, or a parameter name (pure)
//! - [`rack`]         inside a rack: the chain and the device a producer names, resolved against the walk the script already sends (pure)
//! - [`sets`]         set memory on request: export_set writes a rebuildable document under the state dir, import_set rebuilds it
//! - [`arrange`]      the artist-facing tools: arrange (bars), feel (one tool, one undo), set_key, create_return, clear_captures
//! - [`activity`]     the local activity log (one JSON line per tool call)
//! - [`state`]        where the server writes on this machine
//! - [`install`]      the Remote Script installer
//! - [`app`]          process lifecycle, `--status` and `--check`
//!
//! The Mac companion app (`app/`) links this crate: `install`, `handshake`,
//! `connection`, `state` and `app::check` are its API. A signature change in
//! those modules is a breaking change for it.

pub mod activity;
pub mod app;
pub mod arrange;
pub mod audio;
pub mod connection;
pub mod context;
pub mod devices;
pub mod handshake;
pub mod install;
pub mod library;
pub mod lom;
pub mod memory;
pub mod notes;
pub mod parts;
pub mod performance;
pub mod rack;
pub mod samples;
pub mod sections;
pub mod sets;
pub mod song;
pub mod sound;
pub mod state;
pub mod tools;
pub mod transition;
pub mod variation;

/// Package version, reported to MCP clients and in `--status`.
pub const MCP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Remote Script's loader, embedded at build time: the half Live holds.
/// There is exactly one copy of it in the repo; rebuilding the binary picks up
/// edits to it. A change here is the one change that still needs Live
/// restarted, and `handshake::expected_loader_version` reads its version.
pub const REMOTE_SCRIPT_SOURCE: &str =
    include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");

/// The Remote Script's body: every handler, and the half a running Live can
/// be given without being restarted. `handshake::expected_remote_script_version`
/// reads its `SCRIPT_VERSION`; the installer writes it beside the loader, and
/// `reload_body` makes Live re-read it in place.
pub const REMOTE_SCRIPT_BODY: &str = include_str!("../AbletonMusicMaker_Remote_Script/body.py");

/// True for the string values the Python code accepted as "on".
pub fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// `os.environ.get(name, "")` with the value trimmed.
pub fn env_str(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

/// Parse a float from the environment, falling back to `default`.
pub fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Parse an integer from the environment, falling back to `default`.
pub fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}
