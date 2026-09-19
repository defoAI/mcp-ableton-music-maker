//! Remote Script version handshake and capability checks.
//!
//! The server only talks to the Remote Script it ships with. On startup it
//! asks Live for `get_script_info`; every tool then checks that the command it
//! needs is in the script's advertised capability list. A script that cannot
//! answer, or answers with a different version, gets a clear "reinstall and
//! restart Live" message instead of a half-working session.

use crate::connection::{LiveBridge, LiveError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

/// The `SCRIPT_VERSION` declared by the embedded Remote Script: the single
/// source of truth for the version the server expects.
pub fn expected_remote_script_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        crate::REMOTE_SCRIPT_SOURCE
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("SCRIPT_VERSION")?;
                let rest = rest.trim_start().strip_prefix('=')?;
                Some(
                    rest.trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .to_string(),
                )
            })
            .next()
            .unwrap_or_else(|| "unknown".to_string())
    })
}

/// What Live told us about the loaded Remote Script.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScriptInfo {
    /// Version string the script reports, or None if it could not be reached
    /// or does not implement `get_script_info`.
    pub script_version: Option<String>,
    #[serde(default)]
    pub protocol_version: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub expected_version: String,
    #[serde(default)]
    pub up_to_date: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Any other fields the script reported, passed through untouched.
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, Value>,
}

impl ScriptInfo {
    fn unreached(error: Option<String>) -> Self {
        Self {
            script_version: None,
            protocol_version: None,
            capabilities: Vec::new(),
            expected_version: expected_remote_script_version().to_string(),
            up_to_date: false,
            error,
            extra: Default::default(),
        }
    }
}

/// Cached result of the last handshake with Live.
#[derive(Default)]
pub struct ScriptInfoCache {
    info: Mutex<Option<ScriptInfo>>,
}

impl ScriptInfoCache {
    pub fn get(&self) -> Option<ScriptInfo> {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, info: ScriptInfo) {
        *self.info.lock().unwrap_or_else(|e| e.into_inner()) = Some(info);
    }

    /// A cache that advertises every command the server knows, for tests that
    /// exercise tool bodies rather than the missing-script early return.
    pub fn assume_all_capabilities(&self) {
        self.set(ScriptInfo {
            script_version: Some(expected_remote_script_version().to_string()),
            protocol_version: Some(1),
            capabilities: crate::tools::ALL_REMOTE_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            expected_version: expected_remote_script_version().to_string(),
            up_to_date: true,
            error: None,
            extra: Default::default(),
        });
    }

    pub fn has_capability(&self, name: &str) -> bool {
        self.get()
            .map(|info| info.capabilities.iter().any(|c| c == name))
            .unwrap_or(false)
    }

    /// Query Live for script info and cache the answer.
    pub fn handshake(&self, bridge: &dyn LiveBridge) -> ScriptInfo {
        let expected = expected_remote_script_version();
        let info = match bridge.send_command("get_script_info", None) {
            Ok(Value::Object(result)) => {
                let mut parsed: ScriptInfo = serde_json::from_value(Value::Object(result.clone()))
                    .unwrap_or_else(|_| {
                        let mut i = ScriptInfo::unreached(None);
                        i.extra = result;
                        i
                    });
                parsed.expected_version = expected.to_string();
                parsed.up_to_date = parsed.script_version.as_deref() == Some(expected);
                parsed.error = None;
                parsed
            }
            Ok(other) => {
                ScriptInfo::unreached(Some(format!("unexpected get_script_info payload: {other}")))
            }
            Err(LiveError::Ableton(msg)) if msg.to_lowercase().contains("unknown command") => {
                tracing::warn!(
                    "The loaded Remote Script does not implement get_script_info; expected v{}. Run `ableton-music-maker-install-script`, then restart Live.",
                    expected
                );
                ScriptInfo::unreached(Some(msg))
            }
            Err(e) => {
                tracing::warn!("Remote Script handshake failed: {}", e);
                ScriptInfo::unreached(Some(e.to_string()))
            }
        };

        if info.up_to_date {
            tracing::info!(
                "Remote Script handshake OK (v{}, {} capabilities)",
                info.script_version.as_deref().unwrap_or("?"),
                info.capabilities.len()
            );
        } else if let Some(v) = &info.script_version {
            tracing::warn!(
                "Remote Script v{} loaded, server expects v{}. Run `ableton-music-maker-install-script`, then restart Ableton.",
                v,
                expected
            );
        }
        self.set(info.clone());
        info
    }

    /// Error text if the script cannot serve `name`, else None. If Live was
    /// not reachable at startup the handshake is retried here, so a server
    /// started before Live recovers on the first tool call.
    pub fn require_capability(&self, bridge: &dyn LiveBridge, name: &str) -> Option<String> {
        if self.has_capability(name) {
            return None;
        }
        if self.get().is_none_or(|info| info.script_version.is_none()) {
            self.handshake(bridge);
            if self.has_capability(name) {
                return None;
            }
        }
        let loaded = self
            .get()
            .and_then(|i| i.script_version)
            .unwrap_or_else(|| "not reachable".to_string());
        Some(format!(
            "The Ableton Remote Script cannot run `{name}` (loaded: {loaded}, expected: {}). Run `ableton-music-maker-install-script`, then restart Ableton Live.",
            expected_remote_script_version()
        ))
    }

    pub fn as_json(&self) -> Value {
        match self.get() {
            Some(info) => serde_json::to_value(info).unwrap_or(Value::Null),
            None => json!(null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::LiveResult;

    struct Script(LiveResult<Value>);
    impl LiveBridge for Script {
        fn send_command(&self, _: &str, _: Option<Value>) -> LiveResult<Value> {
            self.0.clone()
        }
    }

    #[test]
    fn version_comes_from_embedded_script() {
        let v = expected_remote_script_version();
        assert_eq!(v.split('.').count(), 3, "got {v}");
    }

    #[test]
    fn script_without_get_script_info_has_no_capabilities() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Err(LiveError::Ableton(
            "Unknown command: get_script_info".into(),
        ))));
        assert_eq!(info.script_version, None);
        assert!(!cache.has_capability("get_session_info"));
        let bridge = Script(Err(LiveError::Ableton(
            "Unknown command: get_script_info".into(),
        )));
        let msg = cache.require_capability(&bridge, "get_clip_notes").unwrap();
        assert!(msg.contains("loaded: not reachable"), "{msg}");
    }

    #[test]
    fn current_script_reports_capabilities() {
        let cache = ScriptInfoCache::default();
        let expected = expected_remote_script_version();
        let info = cache.handshake(&Script(Ok(json!({
            "script_version": expected, "protocol_version": 1,
            "capabilities": ["get_clip_notes"], "live_version": "12.1"
        }))));
        assert!(info.up_to_date);
        assert_eq!(info.protocol_version, Some(1));
        assert_eq!(info.extra["live_version"], json!("12.1"));
        assert!(cache.has_capability("get_clip_notes"));
        assert!(!cache.has_capability("delete_clip"));
    }

    #[test]
    fn older_script_is_not_up_to_date() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Ok(
            json!({"script_version": "0.1.0", "capabilities": ["set_tempo"]}),
        )));
        assert!(!info.up_to_date);
        assert!(cache.has_capability("set_tempo"));
        let bridge = Script(Ok(
            json!({"script_version": "0.1.0", "capabilities": ["set_tempo"]}),
        ));
        assert!(cache
            .require_capability(&bridge, "create_locator")
            .unwrap()
            .contains("loaded: 0.1.0"));
    }

    #[test]
    fn unreached_script_is_retried_on_demand() {
        let cache = ScriptInfoCache::default();
        let expected = expected_remote_script_version();
        // Startup: Live not running.
        cache.handshake(&Script(Err(LiveError::Lost("refused".into()))));
        assert!(!cache.has_capability("set_tempo"));
        // First tool call: Live is up now.
        let bridge = Script(Ok(
            json!({"script_version": expected, "capabilities": ["set_tempo"]}),
        ));
        assert_eq!(cache.require_capability(&bridge, "set_tempo"), None);
        assert!(cache.has_capability("set_tempo"));
    }
}
