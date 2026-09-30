//! The local activity log: one JSON line per tool call, on this machine only.
//!
//! On by default because the Mac app is built on it; `ABLETON_MCP_ACTIVITY=false`
//! turns it off. Parameters and results — which carry MIDI notes and the
//! producer's own names — are written only with `ABLETON_MCP_ACTIVITY_PAYLOADS=true`.
//! Sizes are always written, so the app can show what each call added to the
//! conversation without seeing its content. Nothing here opens a socket.

use crate::connection::CallTrace;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

pub struct Activity {
    enabled: bool,
    payloads: bool,
    session_id: String,
    path: PathBuf,
    file: Mutex<Option<File>>,
    warned: AtomicBool,
}

impl Activity {
    /// Read the two switches from the environment and pick this process's file.
    pub fn from_env() -> Self {
        let off = matches!(
            crate::env_str("ABLETON_MCP_ACTIVITY")
                .to_ascii_lowercase()
                .as_str(),
            "0" | "false" | "no" | "off"
        );
        let session_id = format!(
            "{}-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            std::process::id()
        );
        let path = crate::state::activity_dir().join(format!("{session_id}.jsonl"));
        Self {
            enabled: !off,
            payloads: crate::env_flag("ABLETON_MCP_ACTIVITY_PAYLOADS"),
            session_id,
            path,
            file: Mutex::new(None),
            warned: AtomicBool::new(false),
        }
    }

    /// A log that writes nothing — for tests and tools that must not touch
    /// the user's state directory.
    pub fn disabled() -> Self {
        let mut a = Self::from_env();
        a.enabled = false;
        a
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn payloads(&self) -> bool {
        self.payloads
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Where this process's lines go (whether or not any have been written).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One line for one tool call. Never fails the call: a write error is
    /// logged to stderr once and the tool's result is returned regardless.
    pub fn record<P: Serialize>(
        &self,
        tool: &str,
        params: &P,
        result: &Result<String, String>,
        duration: Duration,
        trace: CallTrace,
    ) {
        if !self.enabled {
            return;
        }
        let params_value = serde_json::to_value(params).unwrap_or(Value::Null);
        let in_chars = params_value.to_string().chars().count();
        let (ok, out_chars, error) = match result {
            Ok(text) => (true, text.chars().count(), None),
            Err(e) => (false, e.chars().count(), Some(e.as_str())),
        };
        let mut line = json!({
            "ts": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "tool": tool,
            "commands": trace.commands,
            "ok": ok,
            "error": error,
            "duration_ms": (duration.as_secs_f64() * 10_000.0).round() / 10.0,
            "live_ms": (trace.live_ms * 10.0).round() / 10.0,
            "main_ms": (trace.main_ms * 10.0).round() / 10.0,
            "slices": trace.slices,
            "in_chars": in_chars,
            "out_chars": out_chars,
        });
        if self.payloads {
            line["params"] = params_value;
            if let Ok(text) = result {
                line["result"] = json!(text);
            }
        }
        self.append(&line);
    }

    fn append(&self, line: &Value) {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            match self.open() {
                Ok(file) => *guard = Some(file),
                Err(e) => {
                    self.warn_once(&e);
                    return;
                }
            }
        }
        // One write call per line: the file is opened for append, so a whole
        // line lands atomically even if another process shares the path.
        let mut text = line.to_string();
        text.push('\n');
        let file = guard.as_mut().expect("opened above");
        if let Err(e) = file.write_all(text.as_bytes()) {
            self.warn_once(&e.to_string());
            *guard = None;
        }
    }

    fn open(&self) -> Result<File, String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }

    fn warn_once(&self, what: &str) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!("activity log unavailable ({what}); tool calls continue unlogged");
        }
    }
}
