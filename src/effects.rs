//! Effects and sidechains in a `build_song` document.
//!
//! A track (a return, the master) lists its effects in chain order after the
//! instrument: each by words or a browser URI, with parameters in Live's
//! display units, and optionally a sidechain. A track can also carry a
//! sidechain of its own — ducked by AMM Sidechain, no effect named.
//!
//! Everything is loaded after every track exists, and every sidechain is
//! wired after every effect is loaded, so a trigger may come later in the
//! document than the tracks it keys — the usual order. Converging reuses the
//! k-th device of a name already on a track instead of loading a second, and
//! re-applies its parameters and its routing.

use crate::connection::LiveState;
use crate::sidechain::{SetSidechainParams, AMM_DEVICE_NAME, AMM_URI};
use crate::tools::{get_display, live_err, require, ToolResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// One effect on a track, in chain order.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongEffect {
    /// Words or a browser URI, as load_instrument_or_effect takes them:
    /// "Compressor", "Glue Compressor", "AMM Sidechain", "query:AudioFx#Saturator"
    pub device: String,
    /// Parameters by name, in Live's display units ("-24 dB", "4.00 : 1",
    /// "120 ms") or as raw numbers
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    /// Key this effect from another track (see set_sidechain): {"source":
    /// "Trigger", "tap": "post_fx", "mix": 1.0}
    #[serde(default)]
    pub sidechain: Option<SongSidechain>,
}

/// A sidechain in a document: set_sidechain's settings, without the track
/// and device, which the document's position supplies.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongSidechain {
    /// The trigger: a track's name (in this document or the set; matched
    /// exactly), a return's name or letter, "master", or null
    pub source: Value,
    /// Inside a rack effect: the chain that holds the Compressor to key, as
    /// adv_set_device_parameter takes it ("Duck")
    #[serde(default)]
    pub in_chain: Option<String>,
    #[serde(default)]
    pub tap: Option<String>,
    #[serde(default)]
    pub on: Option<bool>,
    #[serde(default)]
    pub gain_db: Option<f64>,
    #[serde(default)]
    pub mix: Option<f64>,
    #[serde(default)]
    pub listen: Option<bool>,
    #[serde(default)]
    pub eq_on: Option<bool>,
    #[serde(default)]
    pub eq_type: Option<String>,
    #[serde(default)]
    pub eq_freq: Option<Value>,
    #[serde(default)]
    pub eq_q: Option<f64>,
    #[serde(default)]
    pub eq_gain_db: Option<f64>,
    /// AMM Sidechain: "duck" or "gate"
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub threshold_db: Option<f64>,
    #[serde(default)]
    pub ratio: Option<f64>,
    #[serde(default)]
    pub depth_db: Option<f64>,
    #[serde(default)]
    pub attack_ms: Option<f64>,
    #[serde(default)]
    pub release_ms: Option<f64>,
    #[serde(default)]
    pub split_hz: Option<f64>,
    /// A device Live cannot route: the parameter AMM Sidechain moves
    #[serde(default)]
    pub param: Option<String>,
    #[serde(default)]
    pub from: Option<Value>,
    #[serde(default)]
    pub to: Option<Value>,
}

/// A return in a document: found by name (with or without Live's letter
/// prefix) or made, then given its effects.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongReturn {
    pub name: String,
    #[serde(default)]
    pub effects: Vec<SongEffect>,
    /// Duck the whole return from a trigger (AMM Sidechain)
    #[serde(default)]
    pub sidechain: Option<SongSidechain>,
}

/// The master's effects.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SongMaster {
    #[serde(default)]
    pub effects: Vec<SongEffect>,
}

impl SongSidechain {
    fn to_params(&self, kind: &str, index: i64, device_index: Option<i64>) -> SetSidechainParams {
        SetSidechainParams {
            track: Some(if kind == "master" {
                json!("master")
            } else {
                json!(index)
            }),
            kind: Some(kind.to_string()),
            device_index,
            in_chain: self.in_chain.clone(),
            source: Some(self.source.clone()),
            tap: self.tap.clone(),
            on: self.on,
            gain_db: self.gain_db,
            mix: self.mix,
            listen: self.listen,
            eq_on: self.eq_on,
            eq_type: self.eq_type.clone(),
            eq_freq: self.eq_freq.clone(),
            eq_q: self.eq_q,
            eq_gain_db: self.eq_gain_db,
            mode: self.mode.clone(),
            threshold_db: self.threshold_db,
            ratio: self.ratio,
            depth_db: self.depth_db,
            attack_ms: self.attack_ms,
            release_ms: self.release_ms,
            split_hz: self.split_hz,
            param: self.param.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            ..Default::default()
        }
    }
}

/// Where an effect list lives: a track by name, a return by name, the master.
#[derive(Debug, Clone)]
pub struct Holder {
    pub kind: &'static str,
    /// A track's or a return's name; "Master" for the master
    pub name: String,
    pub effects: Vec<SongEffect>,
    pub sidechain: Option<SongSidechain>,
}

/// An effect, resolved to what loads.
#[derive(Debug, Clone)]
struct Resolved {
    uri: String,
    name: String,
}

fn is_amm_word(words: &str) -> bool {
    let w = words.trim().to_lowercase();
    w == "amm sidechain" || w == "amm" || w.ends_with("amm%20sidechain.amxd")
}

fn resolve_effect(live: &LiveState, words: &str) -> Result<Resolved, String> {
    if is_amm_word(words) {
        return Ok(Resolved {
            uri: AMM_URI.into(),
            name: AMM_DEVICE_NAME.into(),
        });
    }
    if words.contains(':') {
        let name = words
            .rsplit(['#', ':', '/'])
            .next()
            .unwrap_or(words)
            .replace("%20", " ")
            .trim_end_matches(".adv")
            .trim_end_matches(".adg")
            .to_string();
        return Ok(Resolved {
            uri: words.to_string(),
            name,
        });
    }
    match crate::tools::best_item(live, words)? {
        Some(h) => Ok(Resolved {
            uri: h.uri.clone(),
            name: h
                .name
                .trim_end_matches(".adv")
                .trim_end_matches(".adg")
                .to_string(),
        }),
        None => Err(format!(
            "nothing in the browser matches the effect \"{words}\""
        )),
    }
}

/// Check the document's effects and sidechains before anything is written:
/// every effect resolves, every source exists (in the document or the set)
/// and names one track, a sidechain is not asked of a device that cannot
/// take one, and — where this Live has already shown the device — every
/// parameter is one it has. Returns lines for the plan.
pub fn validate(
    live: &LiveState,
    holders: &[Holder],
    doc_tracks: &[&str],
    doc_returns: &[&str],
) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    let vocab = live.devices.snapshot(&live.live_version());
    let mut sources: Vec<(String, Value)> = Vec::new();
    for h in holders {
        for (k, e) in h.effects.iter().enumerate() {
            let r = resolve_effect(live, &e.device)
                .map_err(|err| format!("{} effect {}: {err}", h.name, k + 1))?;
            if !e.params.is_empty() {
                // What this Live has shown of a device by that name — never a
                // rack that happens to share it (a rack called "Drum Buss"
                // has macros, not Drum Buss's parameters: found on a real
                // Live). A parameter is refused only when every such device
                // lacks it; otherwise it is checked when the device loads.
                // Nor a Max for Live device: it changes apart from Live's
                // version, so what was learned of an older copy can be wrong
                // (an earlier AMM Sidechain lacked From and To).
                let known: Vec<&crate::devices::DeviceFacts> = vocab
                    .devices
                    .values()
                    .filter(|d| {
                        d.device.eq_ignore_ascii_case(&r.name)
                            && !d.class_name.ends_with("GroupDevice")
                            && !d.class_name.starts_with("MxDevice")
                    })
                    .collect();
                if known.is_empty() {
                    // Our own device's parameters are known to the server;
                    // anything else unseen is checked when it loads.
                    if r.uri != AMM_URI {
                        notes.push(format!(
                        "{} › {}: its parameters are checked when it loads (this Live has not shown it yet).",
                        h.name, r.name
                    ));
                    }
                } else {
                    for name in e.params.keys() {
                        if known.iter().all(|f| f.parameter(name).is_none()) {
                            return Err(format!(
                                "{} › {}: no parameter '{name}'; it has: {}",
                                h.name,
                                r.name,
                                known[0]
                                    .names()
                                    .into_iter()
                                    .skip(1)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                    }
                }
            }
            if let Some(sc) = &e.sidechain {
                if let Some(problem) = sidechain_refusal(&r.name, sc) {
                    return Err(format!("{} › {}: {problem}", h.name, r.name));
                }
                sources.push((format!("{} › {}", h.name, r.name), sc.source.clone()));
            }
        }
        if let Some(sc) = &h.sidechain {
            sources.push((h.name.clone(), sc.source.clone()));
        }
    }
    notes.dedup();
    for (whose, source) in sources {
        check_source(live, &whose, &source, doc_tracks, doc_returns)?;
    }
    Ok(notes)
}

/// What cannot be keyed as asked, by the device's name before it loads.
fn sidechain_refusal(device: &str, sc: &SongSidechain) -> Option<String> {
    let lower = device.to_lowercase();
    let defaults = [
        ("glue compressor", true),
        ("gate", true),
        ("auto filter", true),
    ];
    let needs_param = ["multiband dynamics", "drum buss", "vocoder"];
    if lower == "compressor" || is_amm_word(device) {
        return None;
    }
    if defaults.iter().any(|(n, _)| *n == lower) {
        return None;
    }
    if needs_param.contains(&lower.as_str()) && sc.param.is_none() {
        return Some(
            "Live's API cannot route its own sidechain; it can be keyed through AMM Sidechain moving one of its parameters — give the sidechain a `param` and a `to`."
                .to_string(),
        );
    }
    None
}

fn check_source(
    live: &LiveState,
    whose: &str,
    source: &Value,
    doc_tracks: &[&str],
    doc_returns: &[&str],
) -> Result<(), String> {
    let Value::String(name) = source else {
        // An index or null is checked when it is wired.
        return Ok(());
    };
    let want = name.trim();
    if doc_tracks.contains(&want)
        || doc_tracks
            .iter()
            .filter(|t| t.eq_ignore_ascii_case(want))
            .count()
            == 1
        || doc_returns.iter().any(|r| r.eq_ignore_ascii_case(want))
    {
        return Ok(());
    }
    crate::sidechain::resolve_source(live, source)
        .map(|_| ())
        .map_err(|e| format!("{whose}'s sidechain source: {e}"))
}

/// A return's name without the letter Live puts in front of it: "C-Duck
/// Verb" is "Duck Verb" — naming a new return "C-Duck Verb" made it
/// "C-C-Duck Verb" on a real Live 12.4.6.
pub fn bare_return_name(name: &str) -> String {
    let t = name.trim();
    let b = t.as_bytes();
    if b.len() > 2 && b[0].is_ascii_uppercase() && b[0] <= b'L' && b[1] == b'-' {
        t[2..].trim().to_string()
    } else {
        t.to_string()
    }
}

/// Whether a return Live lists as `have` under `letter` is the one a
/// document calls `want`. A return nobody named is called after its devices
/// and changes name when one is added — Live 12.4.6 turned the default
/// set's "A-Reverb" into "A-Reverb | Compressor" — so the first device's
/// name is what matches.
pub fn return_matches(have: &str, letter: &str, want: &str) -> bool {
    let have = have.trim().to_lowercase();
    let letter = letter.trim().to_lowercase();
    let bare_want = bare_return_name(want).to_lowercase();
    let want = want.trim().to_lowercase();
    let bare = have
        .strip_prefix(&format!("{letter}-"))
        .or_else(|| have.strip_prefix(&format!("{letter} ")))
        .unwrap_or(&have)
        .to_string();
    let first = bare.split(" | ").next().unwrap_or("").trim().to_string();
    have == want || bare == want || bare == bare_want || first == bare_want
}

/// Find a return by name — Live's own ("A-Reverb", "A-Reverb | Compressor")
/// or the one given ("Reverb") — or make it. Its index, and whether it was
/// made. A return that is found is given the document's name, so adding
/// its effects does not rename it and the next run finds it again.
fn ensure_return(live: &LiveState, name: &str, reuse: bool) -> Result<(i64, bool), String> {
    let r = live
        .send_command("get_returns", None)
        .map_err(|e| live_err("read the returns", e))?;
    if reuse {
        for ret in r
            .get("returns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let have = get_display(ret, "name", "");
            let letter = get_display(ret, "letter", "");
            if return_matches(&have, &letter, name) {
                let index = ret.get("index").and_then(Value::as_i64).unwrap_or(0);
                // Always: "A-Reverb" reads the same named or not, and only
                // a named one keeps its name when a Compressor is added.
                require(live, "set_track_name")?;
                live.send_command(
                    "set_track_name",
                    Some(json!({"track_index": index, "kind": "return",
                                "name": bare_return_name(name)})),
                )
                .map_err(|e| live_err("name the return", e))?;
                return Ok((index, false));
            }
        }
    }
    require(live, "create_return_track")?;
    let made = live
        .send_command(
            "create_return_track",
            Some(json!({"name": bare_return_name(name)})),
        )
        .map_err(|e| live_err("create the return", e))?;
    Ok((made.get("index").and_then(Value::as_i64).unwrap_or(0), true))
}

/// Make the returns a document names that the set lacks, before the tracks
/// are made, so their sends can reach them. Lines for the reply.
pub fn ensure_returns(
    live: &LiveState,
    returns: &[SongReturn],
    reuse: bool,
) -> Result<String, String> {
    let mut out = String::new();
    for r in returns {
        let (index, made) = ensure_return(live, &r.name, reuse)?;
        if made {
            out.push_str(&format!("Return {index} '{}' created.\n", r.name));
        }
    }
    Ok(out)
}

/// Load every holder's effects, set their parameters, then wire every
/// sidechain — in that order, across the whole document. `index_of` finds a
/// track by the name the document gives it.
pub fn apply(
    live: &LiveState,
    holders: &[Holder],
    index_of: &dyn Fn(&str) -> Result<i64, String>,
    reuse: bool,
) -> ToolResult {
    let mut out = String::new();
    let mut wiring: Vec<(String, SetSidechainParams)> = Vec::new();
    let mut loaded = 0usize;
    let mut reused = 0usize;
    for h in holders {
        let (kind, index) = match h.kind {
            "master" => ("master", -1),
            "return" => ("return", ensure_return(live, &h.name, true)?.0),
            _ => ("track", index_of(&h.name)?),
        };
        if !h.effects.is_empty() {
            require(live, "ensure_effects")?;
            let resolved: Vec<Resolved> = h
                .effects
                .iter()
                .map(|e| resolve_effect(live, &e.device))
                .collect::<Result<_, _>>()?;
            if resolved.iter().any(|r| r.uri == AMM_URI) {
                crate::sidechain::ensure_device_installed(live)?;
            }
            let specs: Vec<Value> = resolved
                .iter()
                .map(|r| json!({"uri": r.uri, "name": r.name}))
                .collect();
            let r = live
                .send_command(
                    "ensure_effects",
                    Some(json!({"kind": kind, "index": index, "effects": specs,
                                "on_existing": if reuse { "converge" } else { "add" }})),
                )
                .map_err(|e| live_err(&format!("load {}'s effects", h.name), e))?;
            let placed: Vec<Value> = r
                .get("effects")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let names: Vec<String> = placed
                .iter()
                .map(|p| {
                    if p.get("reused").and_then(Value::as_bool) == Some(true) {
                        reused += 1;
                        format!("{} (reused)", get_display(p, "name", "?"))
                    } else {
                        loaded += 1;
                        get_display(p, "name", "?")
                    }
                })
                .collect();
            out.push_str(&format!("{} effects: {}.\n", h.name, names.join(" › ")));
            for (e, p) in h.effects.iter().zip(placed.iter()) {
                let di = p.get("index").and_then(Value::as_i64).unwrap_or(-1);
                if !e.params.is_empty() {
                    // One round trip per device: by name, a number raw and a
                    // string in Live's display units, as the device shows it.
                    require(live, "set_device_parameters")?;
                    let values: Vec<Value> = e
                        .params
                        .iter()
                        .map(|(name, value)| match value {
                            Value::String(text) => json!({"name": name, "value_display": text}),
                            other => json!({"name": name, "value": other}),
                        })
                        .collect();
                    live.send_command(
                        "set_device_parameters",
                        Some(json!({"track_index": index, "kind": kind, "device_index": di, "values": values})),
                    )
                    .map_err(|err| {
                        live_err(&format!("set {} › {}'s parameters", h.name, get_display(p, "name", "?")), err)
                    })?;
                }
                if let Some(sc) = &e.sidechain {
                    wiring.push((
                        format!("{} › {}", h.name, get_display(p, "name", "?")),
                        sc.to_params(kind, index, Some(di)),
                    ));
                }
            }
        }
        if let Some(sc) = &h.sidechain {
            wiring.push((h.name.clone(), sc.to_params(kind, index, None)));
        }
    }
    // Every track and every effect exists now: wire the keys.
    for (whose, params) in &wiring {
        let said = crate::sidechain::set_sidechain_body(live, params)
            .map_err(|e| format!("{whose}: sidechain: {e}"))?;
        let first = said
            .lines()
            .find(|l| l.contains("read back from Live"))
            .unwrap_or(said.lines().next().unwrap_or(""));
        out.push_str(&format!("Sidechain {whose}: {first}\n"));
    }
    if loaded + reused > 0 || !wiring.is_empty() {
        out.push_str(&format!(
            "{loaded} effect{} loaded, {reused} reused, {} sidechain{} wired.\n",
            if loaded == 1 { "" } else { "s" },
            wiring.len(),
            if wiring.len() == 1 { "" } else { "s" }
        ));
    }
    Ok(out)
}
