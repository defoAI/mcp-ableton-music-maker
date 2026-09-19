//! mcp-ableton-music-maker: Ableton Live integration through the Model Context Protocol.
//!
//! Two processes make up the product. This crate is the MCP server side: it
//! speaks MCP over stdio to a client (Claude Desktop, Claude Code, Cursor) and
//! a small JSON-over-TCP protocol to the AbletonMusicMaker Remote Script running
//! inside Live. The Remote Script itself must stay Python (Live only loads
//! control surfaces through its embedded interpreter); its source is embedded
//! in this binary and installed by `ableton-music-maker-install-script`.
//!
//! Module map:
//! - [`connection`]   AbletonConnection, the TCP bridge and reconnect logic
//! - [`handshake`]    Remote Script version / capability negotiation
//! - [`telemetry`]    anonymous usage tier (opt-in, off by default)
//! - [`dataset`]      trajectory dataset tier (opt-in, off by default)
//! - [`tools`]        the MCP server and all tools
//! - [`install`]      the Remote Script installer
//! - [`app`]          process lifecycle for the server binary

pub mod app;
pub mod connection;
pub mod dataset;
pub mod handshake;
pub mod install;
pub mod telemetry;
pub mod tools;

/// Package version, reported as `MCP_VERSION` in telemetry and to MCP clients.
pub const MCP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Remote Script source, embedded at build time. There is exactly one copy
/// of the script in the repo; rebuilding the binary picks up edits to it.
pub const REMOTE_SCRIPT_SOURCE: &str =
    include_str!("../AbletonMusicMaker_Remote_Script/__init__.py");

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
