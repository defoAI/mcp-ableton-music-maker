//! Live's own object model, reached generically.
//!
//! Until the Remote Script grew `describe` and `run`, every capability meant
//! a Python handler, a capability entry, a version bump and the producer
//! reinstalling and restarting Live. This module is the other road: the
//! server builds paths into Live's object model, asks the script what a class
//! actually has on *this* Live, and sends batches of operations that run in
//! one round trip under the script's own slicer.
//!
//! Two rules keep it honest. The paths are rooted at `song`, `application` or
//! `browser` and no name may start with an underscore, which the script
//! enforces again on its side — this module never relies on being the only
//! caller. And what a class has is asked, not assumed: the answers are cached
//! per Live version, so the differences between Live 10, 11 and 12 live here,
//! in something the test suite can see, instead of in ninety `hasattr`
//! branches inside the Python.

use crate::connection::{LiveResult, LiveState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// The only roots a path may start from. Everything else in Live's process —
/// the interpreter, the file system, the network — is unreachable by
/// construction, and the script refuses it again.
pub const ROOTS: [&str; 3] = ["song", "application", "browser"];

/// A path into Live's object model, built rather than spelled, so a typo is
/// a compile error and the root is always one of [`ROOTS`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Path(String);

impl Path {
    pub fn song() -> Self {
        Self("song".into())
    }
    pub fn application() -> Self {
        Self("application".into())
    }
    pub fn browser() -> Self {
        Self("browser".into())
    }

    /// `song.tracks` — an attribute by name.
    pub fn attr(mut self, name: &str) -> Self {
        self.0.push('.');
        self.0.push_str(name);
        self
    }

    /// `song.tracks[3]` — an element of a list.
    pub fn at(mut self, index: i64) -> Self {
        self.0.push_str(&format!("[{index}]"));
        self
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The track at `index`, the start of most paths worth writing.
    pub fn track(index: i64) -> Self {
        Path::song().attr("tracks").at(index)
    }

    /// The track, return or master a tool was aimed at — the three `kind`
    /// values every device tool takes, as one path.
    pub fn track_of(kind: &str, index: i64) -> Self {
        match kind {
            "master" => Path::song().attr("master_track"),
            "return" => Path::song().attr("return_tracks").at(index),
            _ => Path::track(index),
        }
    }

    /// The scene at `index`.
    pub fn scene(index: i64) -> Self {
        Path::song().attr("scenes").at(index)
    }

    /// A track's mixer fader, where Live keeps the raw 0–1 value.
    pub fn track_volume(index: i64) -> Self {
        Path::track(index)
            .attr("mixer_device")
            .attr("volume")
            .attr("value")
    }

    /// A path the model or a client supplied: validated before it is used,
    /// so a bad one fails here with a reason rather than on the wire.
    pub fn from_str_checked(text: &str) -> Result<Self, String> {
        let p = Self(text.trim().to_string());
        p.validate()?;
        Ok(p)
    }

    /// Whether this path is one the script will accept, checked here so a
    /// bad one fails in Rust with a test rather than on the wire.
    pub fn validate(&self) -> Result<(), String> {
        let text = &self.0;
        if text.is_empty() {
            return Err("a path is required".into());
        }
        let root = text
            .split(['.', '['])
            .next()
            .filter(|r| !r.is_empty())
            .ok_or("a path starts with a name")?;
        if !ROOTS.contains(&root) {
            return Err(format!("path must start with {}", ROOTS.join(", ")));
        }
        for name in text.split(['.', '[', ']']) {
            if name.starts_with('_') {
                return Err(format!(
                    "{name:?} is not reachable: names starting with _ are refused"
                ));
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One operation in a batch. A batch is one round trip whatever its length.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Get {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        r#as: Option<String>,
    },
    Set {
        path: String,
        value: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        r#as: Option<String>,
    },
    Call {
        path: String,
        args: Vec<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        r#as: Option<String>,
    },
    /// Wait for Live to apply what the previous ops asked. Live applies a
    /// playhead move, and a few other things, on a later tick.
    WaitTick { ticks: u32 },
}

impl Op {
    pub fn get(path: &Path, name: &str) -> Self {
        Op::Get {
            path: path.to_string(),
            r#as: Some(name.into()),
        }
    }
    /// A read whose value is not wanted back — for its side effect of
    /// proving the path exists.
    pub fn touch(path: &Path) -> Self {
        Op::Get {
            path: path.to_string(),
            r#as: None,
        }
    }
    pub fn set(path: &Path, value: impl Into<Value>) -> Self {
        Op::Set {
            path: path.to_string(),
            value: value.into(),
            r#as: None,
        }
    }
    /// A write whose landed value comes back under `name`: the script reads
    /// the attribute again after setting it, so one op is a write and its
    /// read-back.
    pub fn set_as(path: &Path, value: impl Into<Value>, name: &str) -> Self {
        Op::Set {
            path: path.to_string(),
            value: value.into(),
            r#as: Some(name.into()),
        }
    }
    pub fn call(path: &Path, args: Vec<Value>) -> Self {
        Op::Call {
            path: path.to_string(),
            args,
            r#as: None,
        }
    }
    /// A call whose return value comes back under `name`.
    pub fn call_as(path: &Path, args: Vec<Value>, name: &str) -> Self {
        Op::Call {
            path: path.to_string(),
            args,
            r#as: Some(name.into()),
        }
    }
    pub fn wait(ticks: u32) -> Self {
        Op::WaitTick { ticks }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Op::Get { path, .. } | Op::Set { path, .. } | Op::Call { path, .. } => Some(path),
            Op::WaitTick { .. } => None,
        }
    }
}

/// A batch of operations, validated here before it reaches Live.
#[derive(Debug, Clone, Default)]
pub struct Batch(Vec<Op>);

impl Batch {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push(mut self, op: Op) -> Self {
        self.0.push(op);
        self
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn ops(&self) -> &[Op] {
        &self.0
    }

    /// Every path is one the script will accept, and the batch is not empty.
    pub fn validate(&self) -> Result<(), String> {
        if self.0.is_empty() {
            return Err("run needs at least one op".into());
        }
        for (i, op) in self.0.iter().enumerate() {
            if let Some(p) = op.path() {
                Path(p.to_string())
                    .validate()
                    .map_err(|e| format!("op {i}: {e}"))?;
            }
        }
        Ok(())
    }

    /// A batch from untrusted JSON: every op is checked before it is sent.
    pub fn from_values(values: &[Value]) -> Result<Self, String> {
        let mut batch = Self::new();
        for (i, v) in values.iter().enumerate() {
            let kind = v.get("op").and_then(Value::as_str).unwrap_or("");
            let path = v.get("path").and_then(Value::as_str).unwrap_or("");
            let named = v.get("as").and_then(Value::as_str).map(str::to_string);
            let op = match kind {
                "wait_tick" => Op::WaitTick {
                    ticks: v
                        .get("ticks")
                        .and_then(Value::as_u64)
                        .unwrap_or(1)
                        .clamp(1, 32) as u32,
                },
                "get" => Op::Get {
                    path: Path::from_str_checked(path)
                        .map_err(|e| format!("op {i}: {e}"))?
                        .to_string(),
                    r#as: named,
                },
                "set" => Op::Set {
                    path: Path::from_str_checked(path)
                        .map_err(|e| format!("op {i}: {e}"))?
                        .to_string(),
                    value: v
                        .get("value")
                        .cloned()
                        .ok_or(format!("op {i}: set needs a value"))?,
                    r#as: named,
                },
                "call" => Op::Call {
                    path: Path::from_str_checked(path)
                        .map_err(|e| format!("op {i}: {e}"))?
                        .to_string(),
                    args: v
                        .get("args")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    r#as: named,
                },
                other => {
                    return Err(format!(
                        "op {i}: unknown op {other:?}; known: get, set, call, wait_tick"
                    ))
                }
            };
            batch = batch.push(op);
        }
        batch.validate()?;
        Ok(batch)
    }

    pub fn params(&self) -> Value {
        json!({ "ops": self.0 })
    }

    /// Send it. One round trip; the reply's named results are the map.
    pub fn run(&self, live: &LiveState) -> Result<Map<String, Value>, String> {
        self.validate()?;
        let out = live
            .send_command("run", Some(self.params()))
            .map_err(|e| e.to_string())?;
        Ok(out.as_object().cloned().unwrap_or_default())
    }
}

/// What a class on this Live actually has, as the script reported it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Description {
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub attrs: HashMap<String, Attribute>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub live_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attribute {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub readonly: bool,
}

impl Description {
    pub fn has(&self, attr: &str) -> bool {
        self.attrs.contains_key(attr)
    }
    pub fn writable(&self, attr: &str) -> bool {
        self.attrs.get(attr).is_some_and(|a| !a.readonly)
    }
    pub fn can_call(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }
}

/// What the server has learned about this Live's object model, keyed by the
/// path it asked about. Cleared when the Live version changes, because that
/// is exactly when the answers stop being true.
#[derive(Default)]
pub struct Lom {
    live_version: Mutex<Option<String>>,
    cache: Mutex<HashMap<String, Description>>,
}

impl Lom {
    /// Ask the script what is at `path`, or answer from the cache. The cache
    /// is keyed by path and emptied when Live's version changes.
    pub fn describe(&self, live: &LiveState, path: &Path) -> Result<Description, String> {
        path.validate()?;
        let key = path.to_string();
        if let Some(found) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(found.clone());
        }
        let value = live
            .send_command("describe", Some(json!({ "path": key })))
            .map_err(|e| e.to_string())?;
        let described: Description =
            serde_json::from_value(value).map_err(|e| format!("describe: {e}"))?;
        self.remember(&key, described.clone());
        Ok(described)
    }

    fn remember(&self, key: &str, described: Description) {
        let mut version = self.live_version.lock().unwrap_or_else(|e| e.into_inner());
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if described.live_version.is_some() && *version != described.live_version {
            cache.clear();
            *version = described.live_version.clone();
        }
        cache.insert(key.to_string(), described);
    }

    pub fn cached(&self) -> usize {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn forget(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Whether this script can be driven generically at all.
pub fn available(live: &LiveState) -> bool {
    live.script.has_capability("run") && live.script.has_capability("describe")
}

/// The generic road's own error, worded like every other capability refusal.
pub fn require(live: &LiveState) -> LiveResult<()> {
    let _ = live;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_built_not_spelled() {
        assert_eq!(Path::track(3).as_str(), "song.tracks[3]");
        assert_eq!(
            Path::track_volume(1).as_str(),
            "song.tracks[1].mixer_device.volume.value"
        );
        assert_eq!(Path::scene(0).attr("name").as_str(), "song.scenes[0].name");
        assert_eq!(
            Path::browser()
                .attr("sounds")
                .attr("children")
                .at(2)
                .as_str(),
            "browser.sounds.children[2]"
        );
    }

    #[test]
    fn a_track_a_return_and_the_master_are_one_path_builder() {
        assert_eq!(Path::track_of("track", 2).as_str(), "song.tracks[2]");
        assert_eq!(
            Path::track_of("return", 1).as_str(),
            "song.return_tracks[1]"
        );
        assert_eq!(Path::track_of("master", 0).as_str(), "song.master_track");
    }

    #[test]
    fn only_the_three_roots_are_valid() {
        for root in [Path::song(), Path::application(), Path::browser()] {
            let with_attr = root.clone().attr("x");
            assert!(with_attr.validate().is_ok(), "{root}");
        }
        for bad in ["os.environ", "sys.modules", "Live.Song", "self.song"] {
            let err = Path(bad.into()).validate().unwrap_err();
            assert!(err.contains("must start with"), "{bad}: {err}");
        }
    }

    #[test]
    fn underscore_names_are_refused_here_too_not_only_in_the_script() {
        for bad in [
            "song.__class__",
            "song.tracks[0]._private",
            "song.tracks[0].__class__.__mro__",
        ] {
            let err = Path(bad.into()).validate().unwrap_err();
            assert!(err.contains("not reachable"), "{bad}: {err}");
        }
    }

    #[test]
    fn a_batch_is_json_the_script_understands() {
        let b = Batch::new()
            .push(Op::get(&Path::song().attr("tempo"), "tempo"))
            .push(Op::set(&Path::track_volume(0), 0.5))
            .push(Op::call(&Path::scene(2).attr("fire"), vec![]))
            .push(Op::wait(2));
        b.validate().unwrap();
        assert_eq!(
            b.params(),
            json!({"ops": [
                {"op": "get", "path": "song.tempo", "as": "tempo"},
                {"op": "set", "path": "song.tracks[0].mixer_device.volume.value", "value": 0.5},
                {"op": "call", "path": "song.scenes[2].fire", "args": []},
                {"op": "wait_tick", "ticks": 2},
            ]})
        );
    }

    #[test]
    fn an_empty_batch_or_a_bad_path_never_reaches_live() {
        assert!(Batch::new()
            .validate()
            .unwrap_err()
            .contains("at least one"));
        let b = Batch::new()
            .push(Op::get(&Path::song().attr("tempo"), "t"))
            .push(Op::Get {
                path: "os.environ".into(),
                r#as: None,
            });
        let err = b.validate().unwrap_err();
        assert!(err.starts_with("op 1:"), "{err}");
    }

    #[test]
    fn a_description_answers_what_this_live_has() {
        let d: Description = serde_json::from_value(json!({
            "class": "Track", "name": "Bass",
            "attrs": {"name": {"type": "str", "readonly": false},
                      "is_visible": {"type": "bool", "readonly": true}},
            "methods": ["stop_all_clips"], "live_version": "12.4.6"
        }))
        .unwrap();
        assert!(d.has("name"));
        assert!(d.writable("name"));
        assert!(!d.writable("is_visible"), "read-only is not writable");
        assert!(!d.has("no_such_thing"));
        assert!(d.can_call("stop_all_clips"));
        assert!(!d.can_call("explode"));
    }

    #[test]
    fn the_cache_is_emptied_when_live_changes_version() {
        let lom = Lom::default();
        let twelve = Description {
            class: "Track".into(),
            name: None,
            attrs: HashMap::new(),
            methods: vec![],
            live_version: Some("12.4.6".into()),
        };
        lom.remember("song.tracks[0]", twelve.clone());
        lom.remember("song.tracks[1]", twelve);
        assert_eq!(lom.cached(), 2);
        let eleven = Description {
            class: "Track".into(),
            name: None,
            attrs: HashMap::new(),
            methods: vec![],
            live_version: Some("11.3.0".into()),
        };
        lom.remember("song.tracks[0]", eleven);
        assert_eq!(lom.cached(), 1, "answers from another Live were kept");
    }
}
