//! Where the server writes on this machine — and the only place it does.
//!
//! Everything lives under one state directory: `ABLETON_MCP_STATE_DIR` when
//! set (the Docker image points it at `/state`), else
//! `~/.ableton-music-maker/`. The Mac app reads these paths; nothing uploads
//! them. `sets/` appears only when `export_set` is called.

use std::path::PathBuf;

/// The root of everything the server writes.
pub fn state_dir() -> PathBuf {
    let explicit = crate::env_str("ABLETON_MCP_STATE_DIR");
    if !explicit.is_empty() {
        return PathBuf::from(explicit);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ableton-music-maker")
}

/// One `<session-id>.jsonl` per server process; see [`crate::activity`].
pub fn activity_dir() -> PathBuf {
    state_dir().join("activity")
}

/// One `<pid>.json` per running server; see [`crate::app`].
pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

/// One `<library-key>.json` per Live library: the browser index; see [`crate::library`].
pub fn library_dir() -> PathBuf {
    state_dir().join("library")
}

/// One `<name>.json` per exported set, written only by `export_set`; see [`crate::sets`].
pub fn sets_dir() -> PathBuf {
    state_dir().join("sets")
}
