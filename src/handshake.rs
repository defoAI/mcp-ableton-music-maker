//! Remote Script version handshake and capability checks.
//!
//! The server only talks to the Remote Script it ships with. On startup it
//! asks Live for `get_script_info`; every tool then checks that the command it
//! needs is in the script's advertised capability list.
//!
//! The script is two files, and which one is behind decides what the producer
//! is asked to do. The **body** (`body.py`, every handler) can be replaced in
//! a running Live: the server writes it and sends `reload_body`, and nothing
//! is restarted. The **loader** (`__init__.py`, the socket and the tick) is
//! what Live holds, and a change there is the one that still costs a restart.
//! [`Staleness`] is that decision, and it is the only place the two messages
//! are written.

use crate::connection::{LiveBridge, LiveError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

/// Read `NAME = "x.y.z"` out of a Python source file.
fn declared_version(source: &str, name: &str) -> String {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(name)?;
            let rest = rest.trim_start().strip_prefix('=')?;
            Some(
                rest.trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string(),
            )
        })
        .next()
        .unwrap_or_else(|| "unknown".to_string())
}

/// The `SCRIPT_VERSION` declared by the embedded Remote Script body: the
/// single source of truth for the version of the handlers the server expects.
pub fn expected_remote_script_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| declared_version(crate::REMOTE_SCRIPT_BODY, "SCRIPT_VERSION"))
}

/// The `LOADER_VERSION` declared by the embedded loader: the half Live only
/// reads when it starts.
pub fn expected_loader_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| declared_version(crate::REMOTE_SCRIPT_SOURCE, "LOADER_VERSION"))
}

/// Which half of the script Live is running is behind, and therefore what the
/// producer has to do about it. One enum, because it is one decision and it is
/// made in three places: the handshake, the installer and `--check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staleness {
    /// Both halves are the ones this binary embeds.
    UpToDate,
    /// The loader matches and only the body is behind: a reload, in place,
    /// with the transport running.
    BodyBehind,
    /// The loader is behind. Live only reads it when it starts.
    LoaderBehind,
    /// The script did not answer, or is old enough not to report a loader.
    Unknown,
}

impl Staleness {
    /// True when a `reload_body` can fix this without Live being restarted.
    pub fn recoverable_in_place(self) -> bool {
        self == Staleness::BodyBehind
    }

    /// What to tell the producer. `None` when there is nothing to say.
    pub fn message(self) -> Option<&'static str> {
        match self {
            Staleness::UpToDate => None,
            Staleness::BodyBehind => Some(
                "Live's Remote Script was a version behind and has been updated in place. Nothing to restart.",
            ),
            Staleness::LoaderBehind => Some(
                "The Remote Script's loader changed, which is the one part Live only reads when it starts. Run `ableton-music-maker-install-script`, then restart Live or re-select AbletonMusicMaker under Settings → Link, Tempo & MIDI. The handlers update on their own; the loader does not.",
            ),
            Staleness::Unknown => Some(
                "The Ableton Remote Script did not answer. Run `ableton-music-maker-install-script`, then restart Ableton Live.",
            ),
        }
    }
}

/// What Live told us about the loaded Remote Script.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScriptInfo {
    /// Version string the script reports, or None if it could not be reached
    /// or does not implement `get_script_info`.
    pub script_version: Option<String>,
    /// The loader's own version. `None` from a script older than the split,
    /// which is a single file and is treated as one: "restart Live".
    #[serde(default)]
    pub loader_version: Option<String>,
    #[serde(default)]
    pub protocol_version: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub expected_version: String,
    #[serde(default)]
    pub expected_loader_version: String,
    #[serde(default)]
    pub up_to_date: bool,
    /// What the script says about reloading: `supported`, `count`,
    /// `body_file`, `last_ms`, `body_loaded`. Absent before the split.
    #[serde(default)]
    pub reload: Option<Value>,
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
            loader_version: None,
            protocol_version: None,
            capabilities: Vec::new(),
            expected_version: expected_remote_script_version().to_string(),
            expected_loader_version: expected_loader_version().to_string(),
            up_to_date: false,
            reload: None,
            error,
            extra: Default::default(),
        }
    }

    /// Which half is behind. A script that reports no `loader_version` is one
    /// file, from before the split: its handlers cannot be replaced in place,
    /// so a mismatch there is `LoaderBehind` and says "restart Live" exactly
    /// as it did before this existed.
    pub fn staleness(&self) -> Staleness {
        let Some(script) = self.script_version.as_deref() else {
            return Staleness::Unknown;
        };
        match self.loader_version.as_deref() {
            None => {
                if script == expected_remote_script_version() {
                    Staleness::UpToDate
                } else {
                    Staleness::LoaderBehind
                }
            }
            Some(loader) if loader != expected_loader_version() => Staleness::LoaderBehind,
            Some(_) if script == expected_remote_script_version() => Staleness::UpToDate,
            Some(_) => Staleness::BodyBehind,
        }
    }

    /// True when the loaded script can be told to re-read its body.
    pub fn can_reload(&self) -> bool {
        self.reload
            .as_ref()
            .and_then(|r| r.get("supported"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// How many reloads this script has done, and what the last one cost
    /// Live's main thread.
    pub fn reload_count(&self) -> u64 {
        self.reload
            .as_ref()
            .and_then(|r| r.get("count"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    pub fn reload_last_ms(&self) -> Option<f64> {
        self.reload
            .as_ref()
            .and_then(|r| r.get("last_ms"))
            .and_then(Value::as_f64)
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
            loader_version: Some(expected_loader_version().to_string()),
            capabilities: crate::tools::ALL_REMOTE_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            expected_version: expected_remote_script_version().to_string(),
            expected_loader_version: expected_loader_version().to_string(),
            up_to_date: true,
            reload: Some(json!({"supported": true, "count": 0, "last_ms": null})),
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
                parsed.expected_loader_version = expected_loader_version().to_string();
                parsed.up_to_date = parsed.staleness() == Staleness::UpToDate;
                parsed.error = None;
                // The command list has one home, and it is this side:
                // `tools::ALL_REMOTE_COMMANDS`. When Live is running the
                // script this binary embeds, the binary already knows what
                // that script serves and says so, rather than trusting a
                // list read back over the socket. The script still derives
                // and sends its own (see `_served_commands` there), and that
                // is what is used when the versions differ — the only case
                // where this side cannot know. A test pins the two together
                // against the embedded script, so they cannot disagree.
                if parsed.up_to_date {
                    parsed.capabilities = crate::tools::ALL_REMOTE_COMMANDS
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect();
                }
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
            match info.staleness() {
                Staleness::BodyBehind => tracing::warn!(
                    "Remote Script body v{} loaded, server expects v{}; the loader matches, so this is a reload rather than a restart.",
                    v,
                    expected
                ),
                _ => tracing::warn!(
                    "Remote Script v{} (loader v{}) loaded, server expects v{} (loader v{}). {}",
                    v,
                    info.loader_version.as_deref().unwrap_or("none"),
                    expected,
                    expected_loader_version(),
                    Staleness::LoaderBehind.message().unwrap_or_default()
                ),
            }
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
        let how = self
            .get()
            .map(|i| i.staleness())
            .unwrap_or(Staleness::Unknown);
        let advice = match how {
            Staleness::BodyBehind => "Live's Remote Script body is behind and can be updated in place: run `ableton-music-maker-install-script --reload`.",
            _ => "Run `ableton-music-maker-install-script`, then restart Ableton Live.",
        };
        Some(format!(
            "The Ableton Remote Script cannot run `{name}` (loaded: {loaded}, expected: {}). {advice}",
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

    /// When Live is running the script this binary embeds, the binary is the
    /// authority on what that script serves: it uses `ALL_REMOTE_COMMANDS`
    /// and does not depend on the list read back over the socket. The reply
    /// below under-reports on purpose; the server knows better, and a unit
    /// test in `tools` proves the embedded script really has those handlers.
    #[test]
    fn a_matching_script_is_described_by_the_servers_own_command_list() {
        let cache = ScriptInfoCache::default();
        let expected = expected_remote_script_version();
        let info = cache.handshake(&Script(Ok(json!({
            "script_version": expected, "protocol_version": 1,
            "capabilities": ["get_clip_notes"], "live_version": "12.1"
        }))));
        assert!(info.up_to_date);
        assert_eq!(info.protocol_version, Some(1));
        assert_eq!(info.extra["live_version"], json!("12.1"));
        assert_eq!(
            info.capabilities.len(),
            crate::tools::ALL_REMOTE_COMMANDS.len()
        );
        assert!(cache.has_capability("get_clip_notes"));
        assert!(
            cache.has_capability("delete_clip"),
            "the server should trust its own list for the script it ships"
        );
    }

    /// The one case where this side cannot know: a script that is not the one
    /// the binary embeds. Then what it reports is all there is to go on, and
    /// a command outside it is refused with the version that is loaded.
    #[test]
    fn a_different_script_is_taken_at_its_word() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Ok(json!({
            "script_version": "1.0.0", "capabilities": ["get_clip_notes"]
        }))));
        assert!(!info.up_to_date);
        assert_eq!(info.capabilities, vec!["get_clip_notes".to_string()]);
        assert!(cache.has_capability("get_clip_notes"));
        assert!(!cache.has_capability("delete_clip"));
    }

    /// A newer or hand-installed script that sends no list at all leaves the
    /// server with nothing to go on, and every command says so plainly
    /// rather than failing halfway through.
    #[test]
    fn a_script_that_reports_no_commands_refuses_clearly() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Ok(json!({"script_version": "9.9.9"}))));
        assert!(!info.up_to_date);
        assert!(info.capabilities.is_empty());
        let bridge = Script(Ok(json!({"script_version": "9.9.9"})));
        let msg = cache.require_capability(&bridge, "set_tempo").unwrap();
        assert!(msg.contains("loaded: 9.9.9"), "{msg}");
        assert!(msg.contains("install-script"), "{msg}");
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

    /// AC18: the loader matches and only the body is behind. That is a
    /// reload, in place, and every message says so.
    #[test]
    fn a_body_behind_a_matching_loader_is_recoverable_without_a_restart() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Ok(json!({
            "script_version": "0.9.0",
            "loader_version": expected_loader_version(),
            "capabilities": ["set_tempo"],
            "reload": {"supported": true, "count": 0, "last_ms": null},
        }))));
        assert!(!info.up_to_date);
        assert_eq!(info.staleness(), Staleness::BodyBehind);
        assert!(info.staleness().recoverable_in_place());
        assert!(info.can_reload());
        assert!(info
            .staleness()
            .message()
            .unwrap()
            .contains("Nothing to restart"));
        let bridge = Script(Ok(json!({
            "script_version": "0.9.0", "loader_version": expected_loader_version(),
            "capabilities": ["set_tempo"]
        })));
        let msg = cache.require_capability(&bridge, "create_locator").unwrap();
        assert!(msg.contains("--reload"), "{msg}");
        assert!(!msg.contains("restart Ableton Live"), "{msg}");
    }

    /// AC18: the loader is behind. Live only reads it when it starts, and no
    /// arrangement of reloads changes that.
    #[test]
    fn a_loader_behind_says_restart_live_and_says_why() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Ok(json!({
            "script_version": expected_remote_script_version(),
            "loader_version": "0.0.1",
            "reload": {"supported": true},
        }))));
        assert!(!info.up_to_date);
        assert_eq!(info.staleness(), Staleness::LoaderBehind);
        assert!(!info.staleness().recoverable_in_place());
        let text = info.staleness().message().unwrap();
        assert!(text.contains("restart Live"), "{text}");
        assert!(text.contains("The handlers update on their own"), "{text}");
    }

    /// AC27: a script from before the split reports no `loader_version`. It
    /// is one file, its handlers cannot be replaced in place, and it gets
    /// exactly the behaviour it got before any of this existed.
    #[test]
    fn a_script_from_before_the_split_behaves_as_it_did() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Ok(json!({
            "script_version": "1.35.0", "capabilities": ["set_tempo"]
        }))));
        assert_eq!(info.loader_version, None);
        assert!(!info.up_to_date);
        assert!(!info.can_reload());
        assert_eq!(info.staleness(), Staleness::LoaderBehind);
        let bridge = Script(Ok(json!({
            "script_version": "1.35.0", "capabilities": ["set_tempo"]
        })));
        let msg = cache.require_capability(&bridge, "create_locator").unwrap();
        assert!(msg.contains("restart Ableton Live"), "{msg}");
    }

    /// Both halves current: nothing to say and nothing to do.
    #[test]
    fn both_halves_current_is_up_to_date() {
        let cache = ScriptInfoCache::default();
        let info = cache.handshake(&Script(Ok(json!({
            "script_version": expected_remote_script_version(),
            "loader_version": expected_loader_version(),
            "reload": {"supported": true, "count": 3, "last_ms": 0.21},
        }))));
        assert!(info.up_to_date);
        assert_eq!(info.staleness(), Staleness::UpToDate);
        assert_eq!(info.staleness().message(), None);
        assert_eq!(info.reload_count(), 3);
        assert_eq!(info.reload_last_ms(), Some(0.21));
    }

    /// The two versions come from the two files, and they are not the same
    /// number: the body moves on every fix, the loader almost never.
    #[test]
    fn the_two_versions_are_read_from_the_two_files() {
        assert_eq!(expected_remote_script_version().split('.').count(), 3);
        assert_eq!(expected_loader_version().split('.').count(), 3);
        assert!(crate::REMOTE_SCRIPT_BODY.contains("SCRIPT_VERSION = "));
        assert!(crate::REMOTE_SCRIPT_SOURCE.contains("LOADER_VERSION = "));
        assert!(
            !crate::REMOTE_SCRIPT_SOURCE.contains("\nSCRIPT_VERSION = "),
            "SCRIPT_VERSION has one home and it is the body"
        );
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
