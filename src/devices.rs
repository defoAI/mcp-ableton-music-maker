//! What a device actually answered to — kept, and keyed on the device.
//!
//! `shape_sound` already produces this knowledge and throws it away at the
//! end of the turn: when a word is not in the vocabulary it reads the real
//! parameter names off the device and prints them, and every write is read
//! back with Live's own display string. That "Vinyl Drawbs has no cutoff —
//! its parameters are Vinyl Drive, Release and Rotation Amount" is not a
//! fact about one song. It is a fact about Ableton's factory content, true
//! in every set anyone builds, so it is keyed on the **device** (its name
//! and class) and on the **Live version**, never on a song.
//!
//! Two rules keep it honest.
//!
//! - **Every row is stamped.** Live version, device name and class, and the
//!   date it was observed. These are measurements taken from a real Live,
//!   not a table somebody wrote, and a row with no source does not ship —
//!   so a set whose Live version is not known yet is kept in memory and
//!   never written to disk.
//! - **A stored value is a hint, never truth.** Parameter *names* are cheap
//!   to re-verify and are re-read on every use; an observed value → display
//!   pair is only ever *reported* ("Release ran backwards here last time"),
//!   never used to compute a value. A server that quietly inverts a number
//!   the producer can see in Live is worse than one that says what it saw.
//!
//! Privacy: this sits with the browser and sample indexes — device and
//! parameter names from Live's own content plus the values that were
//! written, local, on by default, and off with the same
//! `ABLETON_MCP_LIBRARY_INDEX=false`. It holds no note, no audio and no path.

use crate::sound::Param;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// The key a Live whose version has not been read yet gets. Rows under it
/// stay in memory: an unstamped measurement is not written down.
pub const UNKNOWN_VERSION: &str = "unknown";

/// How many observations one parameter keeps. Enough to see a direction,
/// few enough that a long session cannot grow the file without bound.
const SEEN_PER_PARAM: usize = 8;
/// How many devices one Live version's file keeps, oldest observation first
/// out. A big library has hundreds of presets; this is the ceiling, not a
/// target.
const MAX_DEVICES: usize = 500;
/// Parameters kept per device. An EQ Eight has 84; nothing has 256.
const MAX_PARAMS: usize = 256;

/// One value written or read, and what Live showed for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    /// Live's own raw parameter value.
    pub value: f64,
    /// What Live displayed for it ("168 ms", "1.58 s", "200 Hz").
    pub display: String,
    /// The day it was observed.
    pub at: String,
}

/// One parameter of one device, as this Live reported it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParamFact {
    pub index: i64,
    pub name: String,
    pub min: f64,
    pub max: f64,
    #[serde(default)]
    pub seen: Vec<Seen>,
}

impl ParamFact {
    /// Where a raw value sits in the parameter's range, 0–1.
    fn fraction(&self, value: f64) -> f64 {
        if (self.max - self.min).abs() < f64::EPSILON {
            0.0
        } else {
            (value - self.min) / (self.max - self.min)
        }
    }
}

/// Everything learned about one device on one Live.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceFacts {
    /// Live's name for it — a preset name such as "Vinyl Drawbs".
    pub device: String,
    /// Live's class, which is what a static table can key on.
    pub class_name: String,
    pub live_version: String,
    /// The day it was last read.
    pub observed_at: String,
    pub parameters: Vec<ParamFact>,
    /// Words `shape_sound` could not map here, and when that was found out.
    #[serde(default)]
    pub unknown_words: BTreeMap<String, String>,
}

impl DeviceFacts {
    pub fn parameter(&self, name: &str) -> Option<&ParamFact> {
        let want = name.to_lowercase();
        self.parameters
            .iter()
            .find(|p| p.name.to_lowercase() == want)
    }

    /// The parameter names, for a reply.
    pub fn names(&self) -> Vec<String> {
        self.parameters.iter().map(|p| p.name.clone()).collect()
    }
}

/// One Live version's file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Vocabulary {
    pub live_version: String,
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceFacts>,
    #[serde(default)]
    pub written_at: String,
}

/// The vocabulary this server process holds, loaded from disk on first use.
#[derive(Default)]
pub struct Devices {
    vocab: Mutex<Option<Vocabulary>>,
}

/// Which device a fact is about, and which Live it was measured on. The
/// three travel together everywhere, because a row is believed only when all
/// three match.
#[derive(Debug, Clone, Copy)]
pub struct DeviceRef<'a> {
    pub live_version: &'a str,
    pub device: &'a str,
    pub class_name: &'a str,
}

impl DeviceRef<'_> {
    fn key(&self) -> String {
        format!("{}/{}", self.class_name.trim(), self.device.trim())
    }

    fn is_named(&self) -> bool {
        !self.device.trim().is_empty()
    }
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// `ABLETON_MCP_LIBRARY_INDEX` again: one switch for every local cache of
/// what Live's own content is called.
pub fn disk_enabled() -> bool {
    crate::library::disk_enabled()
}

fn file_for(live_version: &str) -> PathBuf {
    let safe: String = live_version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    crate::state::devices_dir().join(format!("{safe}.json"))
}

pub fn load_from_disk(live_version: &str) -> Option<Vocabulary> {
    if !disk_enabled() || live_version == UNKNOWN_VERSION {
        return None;
    }
    let bytes = std::fs::read(file_for(live_version)).ok()?;
    let v: Vocabulary = serde_json::from_slice(&bytes).ok()?;
    // A row is believed only for the Live version it was measured on.
    (v.live_version == live_version).then_some(v)
}

fn save_to_disk(v: &Vocabulary) {
    if !disk_enabled() || v.live_version == UNKNOWN_VERSION {
        return;
    }
    let path = file_for(&v.live_version);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec(v) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!(
                    "could not write the device vocabulary {}: {e}",
                    path.display()
                );
            }
        }
        Err(e) => tracing::warn!("could not serialise the device vocabulary: {e}"),
    }
}

impl Devices {
    /// The vocabulary for this Live, loading the disk copy the first time
    /// and starting a fresh one when the version changed.
    fn with<T>(&self, live_version: &str, f: impl FnOnce(&mut Vocabulary) -> T) -> T {
        let mut guard = self.vocab.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = !matches!(guard.as_ref(), Some(v) if v.live_version == live_version);
        if fresh {
            *guard = Some(load_from_disk(live_version).unwrap_or(Vocabulary {
                live_version: live_version.to_string(),
                ..Default::default()
            }));
        }
        f(guard.as_mut().expect("just filled"))
    }

    pub fn snapshot(&self, live_version: &str) -> Vocabulary {
        self.with(live_version, |v| v.clone())
    }

    /// What is known about one device, if anything. Both the class and the
    /// name must match, and so must the Live version: that is the whole of
    /// what makes a stored row believable.
    pub fn recall(&self, dev: DeviceRef) -> Option<DeviceFacts> {
        self.with(dev.live_version, |v| v.devices.get(&dev.key()).cloned())
    }

    /// Every `get_device_parameters` read feeds this — the names, the ranges
    /// and where each parameter sat. Nothing extra is asked of Live.
    pub fn note_device(&self, dev: DeviceRef, params: &[Param]) {
        if !dev.is_named() || params.is_empty() {
            return;
        }
        let day = today();
        self.with(dev.live_version, |v| {
            let facts = v.devices.entry(dev.key()).or_insert_with(|| DeviceFacts {
                device: dev.device.to_string(),
                class_name: dev.class_name.to_string(),
                live_version: dev.live_version.to_string(),
                ..Default::default()
            });
            facts.observed_at = day.clone();
            facts.live_version = dev.live_version.to_string();
            for p in params.iter().take(MAX_PARAMS) {
                let fact = match facts.parameters.iter_mut().find(|f| f.index == p.index) {
                    Some(f) => f,
                    None => {
                        facts.parameters.push(ParamFact {
                            index: p.index,
                            ..Default::default()
                        });
                        facts.parameters.last_mut().expect("just pushed")
                    }
                };
                // The names are re-read every time: a preset the producer
                // edited keeps its name, so the names Live just sent win.
                fact.name = p.name.clone();
                fact.min = p.min;
                fact.max = p.max;
                if let Some(display) = p.value_string.as_deref() {
                    push_seen(fact, p.value, display, &day);
                }
            }
            facts.parameters.sort_by_key(|f| f.index);
            trim(v);
            save_to_disk(v);
        });
    }

    /// What Live showed after a write. This is the half a parameter list
    /// cannot teach: a macro that runs the other way is only learnable by
    /// writing a value and reading the display back.
    ///
    /// `class_name` may be empty — the Remote Script's `set_device_parameter`
    /// reply names the device but not its class, and one field is not worth a
    /// `SCRIPT_VERSION` bump and a reinstall. The row being written to was
    /// created by a read that *did* carry the class, so an empty one falls
    /// back to the device's name to find it. Believing a stored value still
    /// needs both (see [`Devices::recall`]); this only locates the row.
    pub fn note_write(&self, dev: DeviceRef, index: i64, name: &str, value: f64, display: &str) {
        if !dev.is_named() || display.trim().is_empty() {
            return;
        }
        let day = today();
        self.with(dev.live_version, |v| {
            let Some(facts) = (match v.devices.get_mut(&dev.key()) {
                Some(facts) => Some(facts),
                None if dev.class_name.trim().is_empty() => {
                    v.devices.values_mut().find(|d| d.device == dev.device)
                }
                None => None,
            }) else {
                return;
            };
            let Some(fact) = facts
                .parameters
                .iter_mut()
                .find(|f| f.index == index || f.name == name)
            else {
                return;
            };
            push_seen(fact, value, display, &day);
            facts.observed_at = day.clone();
            save_to_disk(v);
        });
    }

    /// A word `shape_sound` could not map on this device. This is the row
    /// that stops the same failed call being paid for twice.
    pub fn note_unknown_word(&self, dev: DeviceRef, word: &str) {
        if !dev.is_named() || word.trim().is_empty() {
            return;
        }
        let day = today();
        self.with(dev.live_version, |v| {
            let Some(facts) = v.devices.get_mut(&dev.key()) else {
                return;
            };
            facts.unknown_words.insert(word.to_lowercase(), day.clone());
            save_to_disk(v);
        });
    }

    /// Delete this Live's file and forget it. Nothing in Live changes.
    pub fn forget(&self, live_version: &str) -> String {
        let mut guard = self.vocab.lock().unwrap_or_else(|e| e.into_inner());
        let had = guard
            .as_ref()
            .filter(|v| v.live_version == live_version)
            .map_or(0, |v| v.devices.len());
        *guard = None;
        let path = file_for(live_version);
        let removed = std::fs::remove_file(&path).is_ok();
        format!(
            "Forgot {had} device{} learned on Live {live_version}. {}",
            if had == 1 { "" } else { "s" },
            if removed {
                format!("{} deleted.", path.display())
            } else {
                "There was no file to delete.".to_string()
            }
        )
    }
}

/// Keep the observations distinct and bounded: the same raw value written
/// twice is one measurement, not two.
fn push_seen(fact: &mut ParamFact, value: f64, display: &str, day: &str) {
    let display = display.trim();
    if display.is_empty() {
        return;
    }
    if let Some(existing) = fact
        .seen
        .iter_mut()
        .find(|s| (s.value - value).abs() < 1e-9)
    {
        existing.display = display.to_string();
        existing.at = day.to_string();
        return;
    }
    fact.seen.push(Seen {
        value,
        display: display.to_string(),
        at: day.to_string(),
    });
    if fact.seen.len() > SEEN_PER_PARAM {
        fact.seen.remove(0);
    }
}

/// Hold the file to [`MAX_DEVICES`], dropping what was seen longest ago.
fn trim(v: &mut Vocabulary) {
    v.written_at = today();
    if v.devices.len() <= MAX_DEVICES {
        return;
    }
    let mut by_age: Vec<(String, String)> = v
        .devices
        .iter()
        .map(|(k, d)| (d.observed_at.clone(), k.clone()))
        .collect();
    by_age.sort();
    for (_, key) in by_age.into_iter().take(v.devices.len() - MAX_DEVICES) {
        v.devices.remove(&key);
    }
}

// ── Reading what was learned ────────────────────────────────────────────────

/// A display string as a number, with the family its unit belongs to, so
/// "168 ms" and "1.58 s" can be compared and "200 Hz" and "-6 dB" cannot.
///
/// Live pads and formats these itself; anything that does not start with a
/// number is not a measurement and is left alone.
pub fn numeric(display: &str) -> Option<(f64, &'static str)> {
    let text = display.trim();
    let cut = text
        .char_indices()
        .find(|(i, c)| {
            !(c.is_ascii_digit() || *c == '.' || ((*c == '-' || *c == '+') && *i == 0) || *c == ',')
        })
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let (head, tail) = text.split_at(cut);
    let n: f64 = head.replace(',', "").parse().ok()?;
    match tail.trim().to_lowercase().as_str() {
        "ms" => Some((n / 1000.0, "time")),
        "s" | "sec" => Some((n, "time")),
        "hz" => Some((n, "frequency")),
        "khz" => Some((n * 1000.0, "frequency")),
        "db" => Some((n, "level")),
        "%" => Some((n, "percent")),
        "" => Some((n, "plain")),
        _ => None,
    }
}

/// Whether this parameter ran the other way here: higher raw value, lower
/// reading. Returns the sentence to print, with the measurements in it.
///
/// It **reports**; it never corrects. A server that silently inverts a
/// number is a server the producer cannot reconcile with what Live shows.
pub fn backwards(fact: &ParamFact) -> Option<String> {
    let mut points: Vec<(f64, f64, &Seen)> = Vec::new();
    let mut family = "";
    for s in &fact.seen {
        let (n, fam) = numeric(&s.display)?;
        if family.is_empty() {
            family = fam;
        } else if family != fam {
            return None;
        }
        points.push((fact.fraction(s.value), n, s));
    }
    if points.len() < 2 {
        return None;
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (lo_f, lo_n, lo) = points.first().copied()?;
    let (hi_f, hi_n, hi) = points.last().copied()?;
    if (hi_f - lo_f).abs() < 0.05 || hi_n >= lo_n {
        return None;
    }
    Some(format!(
        "'{}' ran backwards here: {:.2} gave {}, {:.2} gave {} (measured {}).",
        fact.name, hi_f, hi.display, lo_f, lo.display, hi.at
    ))
}

/// What to say about a word this device has refused before.
pub fn advice(facts: &DeviceFacts, word: &str) -> Option<String> {
    let when = facts.unknown_words.get(&word.to_lowercase())?;
    Some(format!(
        "'{word}' was not on '{}' on {when} either (Live {}); its parameters are: {}.",
        facts.device,
        facts.live_version,
        facts.names().join(", ")
    ))
}

/// The line `adv_device_vocabulary show` leads with.
pub fn status_line(v: &Vocabulary) -> String {
    if v.devices.is_empty() {
        return format!(
            "Nothing learned yet about the devices on Live {}.",
            v.live_version
        );
    }
    let observations: usize = v
        .devices
        .values()
        .flat_map(|d| d.parameters.iter())
        .map(|p| p.seen.len())
        .sum();
    format!(
        "{} device{} learned on Live {}, {} parameter{} and {observations} measured value{}.",
        v.devices.len(),
        if v.devices.len() == 1 { "" } else { "s" },
        v.live_version,
        v.devices
            .values()
            .map(|d| d.parameters.len())
            .sum::<usize>(),
        if v.devices
            .values()
            .map(|d| d.parameters.len())
            .sum::<usize>()
            == 1
        {
            ""
        } else {
            "s"
        },
        if observations == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(value: f64, display: &str) -> Seen {
        Seen {
            value,
            display: display.into(),
            at: "2026-09-20".into(),
        }
    }

    #[test]
    fn a_display_string_becomes_a_number_and_its_family() {
        assert_eq!(numeric("168 ms"), Some((0.168, "time")));
        assert_eq!(numeric("1.58 s"), Some((1.58, "time")));
        assert_eq!(numeric("2.04 s"), Some((2.04, "time")));
        assert_eq!(numeric("200 Hz"), Some((200.0, "frequency")));
        assert_eq!(numeric("1.2 kHz"), Some((1200.0, "frequency")));
        assert_eq!(numeric("-6.0 dB"), Some((-6.0, "level")));
        assert_eq!(numeric("50 %"), Some((50.0, "percent")));
        assert_eq!(numeric("0.50"), Some((0.5, "plain")));
        // Not a measurement: a chooser's label.
        assert_eq!(numeric("Low Cut 48 dB"), None);
        assert_eq!(numeric("On"), None);
    }

    /// The VHS Dreams case from the session of 2026-09-20: the reading falls
    /// as the macro rises, which no parameter list can show.
    #[test]
    fn a_macro_that_runs_the_other_way_is_reported_with_its_measurements() {
        let fact = ParamFact {
            index: 2,
            name: "Release".into(),
            min: 0.0,
            max: 1.0,
            seen: vec![seen(0.2, "2.04 s"), seen(0.9, "168 ms")],
        };
        let said = backwards(&fact).expect("this one ran backwards");
        assert!(said.contains("'Release' ran backwards here"), "{said}");
        assert!(said.contains("0.90 gave 168 ms"), "{said}");
        assert!(said.contains("0.20 gave 2.04 s"), "{said}");
        assert!(said.contains("measured 2026-09-20"), "{said}");
    }

    #[test]
    fn a_parameter_that_runs_the_usual_way_is_not_reported() {
        let fact = ParamFact {
            index: 0,
            name: "Cutoff".into(),
            min: 0.0,
            max: 1.0,
            seen: vec![seen(0.2, "200 Hz"), seen(0.9, "8.00 kHz")],
        };
        assert_eq!(backwards(&fact), None);
    }

    #[test]
    fn one_measurement_says_nothing_about_direction() {
        let fact = ParamFact {
            index: 0,
            name: "Release".into(),
            min: 0.0,
            max: 1.0,
            seen: vec![seen(0.6, "168 ms")],
        };
        assert_eq!(backwards(&fact), None, "one point is not a direction");
    }

    #[test]
    fn units_that_cannot_be_compared_are_not_compared() {
        let fact = ParamFact {
            index: 0,
            name: "Mode".into(),
            min: 0.0,
            max: 1.0,
            seen: vec![seen(0.2, "200 Hz"), seen(0.9, "-6.0 dB")],
        };
        assert_eq!(backwards(&fact), None);
    }

    /// The Vinyl Drawbs case: the word is not there, and the names are.
    #[test]
    fn a_word_that_failed_before_is_recalled_with_its_stamp() {
        let mut facts = DeviceFacts {
            device: "Vinyl Drawbs".into(),
            class_name: "InstrumentGroupDevice".into(),
            live_version: "12.4.6".into(),
            observed_at: "2026-09-20".into(),
            parameters: ["Vinyl Drive", "Release", "Rotation Amount"]
                .iter()
                .enumerate()
                .map(|(i, n)| ParamFact {
                    index: i as i64,
                    name: (*n).into(),
                    min: 0.0,
                    max: 1.0,
                    seen: vec![],
                })
                .collect(),
            ..Default::default()
        };
        facts
            .unknown_words
            .insert("cutoff".into(), "2026-09-20".into());
        let said = advice(&facts, "cutoff").expect("this word failed here before");
        assert!(
            said.contains("'cutoff' was not on 'Vinyl Drawbs'"),
            "{said}"
        );
        assert!(said.contains("2026-09-20"), "{said}");
        assert!(said.contains("Live 12.4.6"), "{said}");
        assert!(
            said.contains("Vinyl Drive, Release, Rotation Amount"),
            "{said}"
        );
        assert_eq!(advice(&facts, "attack"), None);
    }

    #[test]
    fn observations_are_distinct_and_bounded() {
        let mut fact = ParamFact {
            index: 0,
            name: "Release".into(),
            min: 0.0,
            max: 1.0,
            seen: vec![],
        };
        push_seen(&mut fact, 0.5, "500 ms", "2026-09-20");
        push_seen(&mut fact, 0.5, "505 ms", "2026-09-21");
        assert_eq!(fact.seen.len(), 1, "the same value is one measurement");
        assert_eq!(fact.seen[0].display, "505 ms");
        assert_eq!(fact.seen[0].at, "2026-09-21");
        for i in 0..20 {
            push_seen(&mut fact, i as f64 / 20.0, &format!("{i} ms"), "2026-09-21");
        }
        assert_eq!(fact.seen.len(), SEEN_PER_PARAM);
    }
}
