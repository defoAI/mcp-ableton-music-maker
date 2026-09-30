//! Inside a rack: the chain a producer means, and the device in it.
//!
//! A drum rack's per-pad Simpler and an instrument rack's nested Operator are
//! where the sound actually lives; the rack's eight macros are only what its
//! maker chose to expose. `get_device_parameters` already serialises the whole
//! tree (`_serialize_chains` in the Remote Script), so resolving "the snare's
//! decay" costs no extra round trip: this module reads that reply.
//!
//! Names, not numbers (#66): a chain is named ("Kick Dump"), a device by a
//! substring of its name. A word that matches two chains is an error that
//! lists both — it never picks one.
//!
//! A drum pad's **note** is not a way in, and `Chain.out_note` is not it.
//! `out_note` is what the chain *plays* — Live feeds every pad's sampler the
//! same C3 so it sounds at root pitch — so it identifies nothing: measured on
//! a real Live 12.4.6 on 2026-09-20, all sixteen chains of a Rollin Breaks Kit
//! and all sixteen of an LDre Mellow Kit read 60. The pad's own note is
//! `DrumPad.note`, on the rack's separate `drum_pads` collection, which
//! `adv_get_drum_rack_pads` walks — so a producer holding a note asks there
//! for the pad's name, and names it here.
//!
//! Writing is the generic ops layer's job ([`crate::lom`]): the Remote Script
//! resolves `song.tracks[0].devices[0].chains[2].devices[0].parameters[1]` on
//! its own whitelist, so nested tuning needs no new command, no
//! `SCRIPT_VERSION` bump and no reinstall.

use crate::lom::Path;
use crate::sound::Param;
use serde_json::Value;

/// What Live shows for one parameter: its value, the labels of a chooser, and
/// the two ends of its range. Handed to [`Nested::quote_displays`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamDisplay {
    pub index: i64,
    pub display: Option<String>,
    pub items: Vec<String>,
    pub display_min: Option<String>,
    pub display_max: Option<String>,
}

/// One device inside a rack, resolved against a `get_device_parameters` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Nested {
    /// Chain indices, outermost rack first
    pub chain_path: Vec<i64>,
    /// At each level but the innermost, the device that holds the next rack
    pub rack_path: Vec<i64>,
    /// What those chains are called, for the reply
    pub chain_names: Vec<String>,
    /// Position of the device in the innermost chain
    pub device_index: i64,
    pub device_name: String,
    pub class_name: String,
    pub params: Vec<Param>,
    /// The device as the walk reported it, for a readout that reuses the
    /// same renderer as a top-level device.
    pub device_json: Value,
}

impl Nested {
    /// "Kick Dump (C1) › Simpler": where a value landed, in the producer's words.
    pub fn label(&self) -> String {
        let mut parts = self.chain_names.clone();
        parts.push(self.device_name.clone());
        parts.join(" › ")
    }

    /// This device in Live's object model, under the track that owns the rack:
    /// `…devices[2].chains[0].devices[1]`.
    pub fn path(&self, track: &Path, top_device: i64) -> Path {
        let mut p = track.clone().attr("devices").at(top_device);
        for (level, chain) in self.chain_path.iter().enumerate() {
            let last = level + 1 == self.chain_path.len();
            p = p.attr("chains").at(*chain).attr("devices").at(if last {
                self.device_index
            } else {
                self.rack_path.get(level).copied().unwrap_or(0)
            });
        }
        p
    }

    /// One of its parameters, by position in `parameters`.
    pub fn parameter_path(&self, track: &Path, top_device: i64, parameter: i64) -> Path {
        self.path(track, top_device)
            .attr("parameters")
            .at(parameter)
    }

    /// Put Live's own words on each parameter: what it displays for the value
    /// it holds, and the labels of a chooser.
    ///
    /// The rack walk the script sends does not carry them — a chain's
    /// parameters arrive with `value`, `min` and `max` and nothing else — so a
    /// pad's filter read as "100%" of 0…127 where the same parameter on a
    /// top-level device reads "22.0 kHz". A number with no unit is not what a
    /// producer set, and it is not something `shape_sound`'s words can be
    /// matched against either. The caller asks Live (`str_for_value`, and
    /// `value_items` where the parameter is quantized) and hands the answers
    /// here; both the readout and the vocabulary then quote Live.
    ///
    /// Entries are `(index, display, items, ends)`, `ends` being what Live
    /// shows for `min` and `max`. A parameter with no entry keeps what the
    /// walk gave it.
    pub fn quote_displays(&mut self, shown: &[ParamDisplay]) {
        if let Some(ps) = self
            .device_json
            .get_mut("parameters")
            .and_then(Value::as_array_mut)
        {
            for p in ps.iter_mut() {
                let at = p.get("index").and_then(Value::as_i64);
                let Some(shown) = shown.iter().find(|d| Some(d.index) == at) else {
                    continue;
                };
                if let Some(text) = &shown.display {
                    p["display"] = Value::String(text.clone());
                }
                if !shown.items.is_empty() {
                    p["items"] = Value::Array(
                        shown
                            .items
                            .iter()
                            .map(|i| Value::String(i.clone()))
                            .collect(),
                    );
                }
                if let Some(text) = &shown.display_min {
                    p["display_min"] = Value::String(text.clone());
                }
                if let Some(text) = &shown.display_max {
                    p["display_max"] = Value::String(text.clone());
                }
            }
        }
        self.params = params_of(&self.device_json);
    }
}

/// `"Kick Dump/Sub"` → `["Kick Dump", "Sub"]`. Empty segments are dropped, so
/// a trailing slash is not an error.
pub fn segments(text: &str) -> Vec<String> {
    text.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// One chain as the rack walk reports it.
#[derive(Debug, Clone)]
struct ChainInfo {
    index: i64,
    name: String,
    json: Value,
}

impl ChainInfo {
    /// Just the name. It used to append the pad's note — `out_note` — which
    /// made a real kit list as "Kick Alpha (C3)", "Snare Rolling (C3)",
    /// "Bongo 1 (C3)" … sixteen pads, one note, because that is what
    /// `out_note` is (see the module doc). A label that is the same for
    /// everything is not a label.
    fn display(&self) -> String {
        self.name.clone()
    }
}

fn chains_of(device: &Value) -> Vec<ChainInfo> {
    device
        .get("chains")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, c)| ChainInfo {
                    index: c.get("index").and_then(Value::as_i64).unwrap_or(i as i64),
                    name: c
                        .get("chain_name")
                        .or_else(|| c.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    json: c.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One device on a chain, as the rack walk reports it.
#[derive(Debug, Clone)]
struct DeviceInfo {
    index: i64,
    name: String,
    class_name: String,
    /// What the script calls it: "instrument", "audio_effect", "rack", …
    kind: String,
    json: Value,
}

impl DeviceInfo {
    fn is_sound_source(&self) -> bool {
        matches!(self.kind.as_str(), "instrument" | "rack" | "drum_machine")
            || crate::sound::is_rack(&self.class_name)
    }
}

fn devices_of(chain: &Value) -> Vec<DeviceInfo> {
    chain
        .get("devices")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, d)| DeviceInfo {
                    index: d.get("index").and_then(Value::as_i64).unwrap_or(i as i64),
                    name: d
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    class_name: d
                        .get("class_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    kind: d
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    json: d.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn params_of(device: &Value) -> Vec<Param> {
    device
        .get("parameters")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Param::from_value).collect())
        .unwrap_or_default()
}

fn listing(chains: &[ChainInfo]) -> String {
    chains
        .iter()
        .map(|c| format!("'{}'", c.display()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One name against one level of chains: the whole name first, then a drum
/// pad's note, then a substring. Two hits is an error that names both.
/// Why a chain was not picked. **Nothing matched** and **several matched**
/// are different answers: the first may be a device's name instead, the
/// second is the producer naming a real pad that this kit has more than one
/// of, and telling them it does not exist is a lie. A real Rollin Breaks Kit
/// has three pads called "Snare Rolling" (measured on Live 12.4.6).
struct ChainMiss {
    message: String,
    ambiguous: bool,
}

fn pick_chain(given: &str, chains: &[ChainInfo], rack: &str) -> Result<usize, ChainMiss> {
    if chains.is_empty() {
        return Err(ChainMiss {
            message: format!("'{rack}' has no chains to look inside"),
            ambiguous: false,
        });
    }
    let want = given.trim().to_lowercase();
    let exact: Vec<usize> = chains
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name.to_lowercase() == want)
        .map(|(i, _)| i)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(ChainMiss {
            message: format!(
                "'{given}' is the name of {} chains in '{rack}' — Live lets a kit repeat a pad \
                 name, and nothing here can tell them apart. Rename one in Live. Its chains: {}.",
                exact.len(),
                listing(chains)
            ),
            ambiguous: true,
        });
    }
    let partial: Vec<usize> = chains
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name.to_lowercase().contains(&want))
        .map(|(i, _)| i)
        .collect();
    match partial.len() {
        1 => Ok(partial[0]),
        0 => Err(ChainMiss {
            message: format!(
                "no chain called '{given}' in '{rack}'; its chains are: {}",
                listing(chains)
            ),
            ambiguous: false,
        }),
        _ => {
            let names: Vec<String> = partial.iter().map(|i| chains[*i].display()).collect();
            // "Say the whole name" is only advice when the whole names differ — a
            // kit with three pads called "Snare Rolling" cannot be told apart
            // by name at all, and saying otherwise sends the producer in a
            // circle.
            let distinct: std::collections::BTreeSet<&String> = names.iter().collect();
            let advice = if distinct.len() == names.len() {
                "Say the whole name."
            } else {
                "They share a name, so nothing here can tell them apart: rename one in Live."
            };
            Err(ChainMiss {
                message: format!(
                    "'{given}' matches {} chains in '{rack}': {}. {advice}",
                    partial.len(),
                    names
                        .iter()
                        .map(|n| format!("'{n}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                ambiguous: true,
            })
        }
    }
}

/// The device a chain means: the one named, else the instrument or rack, else
/// the first. An ambiguous name is an error that lists the candidates.
fn pick_device(given: Option<&str>, devices: &[DeviceInfo], chain: &str) -> Result<usize, String> {
    if devices.is_empty() {
        return Err(format!("'{chain}' holds no devices"));
    }
    let all = || {
        devices
            .iter()
            .map(|d| format!("'{}'", d.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let Some(given) = given else {
        // The same default as a top-level track: what makes the sound, not
        // the effect after it.
        return Ok(devices
            .iter()
            .position(DeviceInfo::is_sound_source)
            .unwrap_or(0));
    };
    let want = given.trim().to_lowercase();
    if let Some(i) = devices.iter().position(|d| d.name.to_lowercase() == want) {
        return Ok(i);
    }
    let partial: Vec<usize> = devices
        .iter()
        .enumerate()
        .filter(|(_, d)| d.name.to_lowercase().contains(&want))
        .map(|(i, _)| i)
        .collect();
    match partial.len() {
        1 => Ok(partial[0]),
        0 => Err(format!(
            "no chain or device called '{given}' in '{chain}'; it holds: {}",
            all()
        )),
        _ => Err(format!(
            "'{given}' matches {} devices in '{chain}': {}. Say the whole name.",
            partial.len(),
            partial
                .iter()
                .map(|i| format!("'{}'", devices[*i].name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Resolve `in_chain` against the `device` of a `get_device_parameters` reply.
///
/// Every segment but the last names a chain; racks nest, so `"Kick Dump/Sub"`
/// reads as far in as it says. The last segment may instead name a device
/// inside the chain reached so far, which is how the second device on a pad is
/// asked for.
pub fn resolve(device: &Value, in_chain: &str) -> Result<Nested, String> {
    let wanted = segments(in_chain);
    if wanted.is_empty() {
        return Err("in_chain names a chain inside the rack, e.g. \"Kick Dump\"".into());
    }
    let mut here = device.clone();
    let mut chain: Option<ChainInfo> = None;
    let mut chain_path: Vec<i64> = Vec::new();
    let mut rack_path: Vec<i64> = Vec::new();
    let mut chain_names: Vec<String> = Vec::new();
    let mut device_name: Option<String> = None;

    for (step, name) in wanted.iter().enumerate() {
        let rack = here
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("this device")
            .to_string();
        let chains = chains_of(&here);
        let last = step + 1 == wanted.len();
        match pick_chain(name, &chains, &rack) {
            Ok(i) => {
                chain_path.push(chains[i].index);
                chain_names.push(chains[i].display());
                chain = Some(chains[i].clone());
                if !last {
                    // A word follows. It names a chain deeper in — step
                    // through the rack on this chain — or, when this chain
                    // holds no rack and it is the last word, a device here.
                    let devices = devices_of(&chains[i].json);
                    match devices
                        .iter()
                        .position(|d| crate::sound::is_rack(&d.class_name))
                    {
                        Some(d) => {
                            rack_path.push(devices[d].index);
                            here = devices[d].json.clone();
                        }
                        None if step + 2 == wanted.len() => {
                            device_name = Some(wanted[step + 1].clone());
                            break;
                        }
                        None => {
                            return Err(format!(
                                "'{}' holds no rack to look inside, so '{}' names nothing; it holds: {}",
                                chains[i].name,
                                wanted[step + 1],
                                devices
                                    .iter()
                                    .map(|d| format!("'{}'", d.name))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ))
                        }
                    }
                }
            }
            // A last word that names *no* chain names a device in the chain
            // already reached — "Kick Dump/Sub", the Sub on the kick's pad.
            //
            // A word that names *several* chains is not that: it is a pad the
            // kit has more than one of, and the answer is the list of them.
            // Falling through here reported "no chain or device called 'Snare
            // Rolling'" for a kit holding three of them — the producer told a
            // real pad does not exist (found on a real Live 12.4.6).
            Err(e) if last && chain.is_some() && !e.ambiguous => device_name = Some(name.clone()),
            Err(e) => return Err(e.message),
        }
    }

    let chain = chain.ok_or_else(|| format!("'{in_chain}' names no chain here"))?;
    let devices = devices_of(&chain.json);
    let d = pick_device(device_name.as_deref(), &devices, &chain.display())?;
    Ok(Nested {
        chain_path,
        rack_path,
        chain_names,
        device_index: devices[d].index,
        device_name: devices[d].name.clone(),
        class_name: devices[d].class_name.clone(),
        params: params_of(&devices[d].json),
        device_json: devices[d].json.clone(),
    })
}

/// What the chains of a rack are called, in order. Empty for a device that is
/// not a rack.
pub fn chain_names(device: &Value) -> Vec<String> {
    chains_of(device).iter().map(|c| c.display()).collect()
}

/// What a rack holds, for an error or a readout: every chain and the devices
/// on it, which is what a producer needs to name one.
pub fn chain_listing(device: &Value) -> String {
    chains_of(device)
        .iter()
        .map(|c| {
            let devices = devices_of(&c.json)
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            if devices.is_empty() {
                format!("'{}'", c.display())
            } else {
                format!("'{}' ({devices})", c.display())
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The number a Live display string leads with: "200 Hz" → 200, "-6.0 dB" →
/// -6, "Low Cut 48 dB" → none (it leads with a word). Used to pick the
/// closest sample when no display matches exactly.
pub fn leading_number(text: &str) -> Option<f64> {
    let t = text.trim();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        let keep = c.is_ascii_digit()
            || (c == '-' && i == 0)
            || (c == '+' && i == 0)
            || (c == '.' && !t[..i].contains('.'));
        if !keep {
            break;
        }
        end = i + c.len_utf8();
    }
    t[..end].parse().ok()
}

/// The value whose display is `wanted`, out of what Live answered for a set
/// of sampled values: the same string first, then the nearest number.
///
/// Live gives no inverse of `str_for_value`, and the Remote Script's own
/// bisection runs inside one tick — which a nested device cannot use, because
/// no command reaches inside a rack. Sampling is the same measurement, taken
/// from here: every candidate is a value Live itself displayed.
pub fn nearest_display(samples: &[(f64, String)], wanted: &str) -> Option<f64> {
    let want = wanted.trim().to_lowercase();
    if let Some((v, _)) = samples
        .iter()
        .find(|(_, s)| s.trim().to_lowercase() == want)
    {
        return Some(*v);
    }
    let number = leading_number(wanted)?;
    let mut best: Option<(f64, f64)> = None;
    for (v, shown) in samples {
        let Some(n) = leading_number(shown) else {
            continue;
        };
        let gap = (n - number).abs();
        if best.is_none_or(|(b, _)| gap < b) {
            best = Some((gap, *v));
        }
    }
    best.map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    /// What Live shows lands on the parameters, and the readout and the
    /// vocabulary both read it off the same device JSON.
    #[test]
    fn quoting_live_puts_its_words_on_the_parameters() {
        let device = serde_json::json!({
            "name": "Snare Drum", "class_name": "InstrumentGroupDevice",
            "parameters": [
                {"index": 0, "name": "Device On", "value": 1.0, "min": 0.0, "max": 1.0,
                 "is_quantized": true},
                {"index": 3, "name": "Low Pass Filter", "value": 127.0, "min": 0.0, "max": 127.0,
                 "is_quantized": false},
            ],
        });
        let mut n = Nested {
            chain_path: vec![2],
            rack_path: vec![],
            chain_names: vec!["Snare Drum".into()],
            device_index: 0,
            device_name: "Snare Drum".into(),
            class_name: "InstrumentGroupDevice".into(),
            params: params_of(&device),
            device_json: device,
        };
        // Before: a percentage of a range nobody can see.
        assert_eq!(n.params[1].display(), "100%");
        n.quote_displays(&[
            ParamDisplay {
                index: 0,
                display: Some("On".into()),
                items: vec!["Off".into(), "On".into()],
                ..Default::default()
            },
            ParamDisplay {
                index: 3,
                display: Some("22.0 kHz".into()),
                display_min: Some("30.0 Hz".into()),
                display_max: Some("22.0 kHz".into()),
                ..Default::default()
            },
        ]);
        assert_eq!(n.params[0].display(), "On");
        assert_eq!(n.params[0].range_text(), "[Off, On]");
        assert_eq!(n.params[1].display(), "22.0 kHz");
        assert_eq!(n.params[1].range_text(), "30.0 Hz … 22.0 kHz");
        // And on the JSON the readout renders from, not only on the copy.
        assert_eq!(n.device_json["parameters"][1]["display"], "22.0 kHz");
    }

    /// A parameter the caller could not read keeps what the walk gave it,
    /// rather than losing the number it already had.
    #[test]
    fn a_parameter_with_no_answer_keeps_what_the_walk_gave_it() {
        let device = serde_json::json!({
            "parameters": [
                {"index": 0, "name": "Tone", "value": 93.0, "min": 0.0, "max": 127.0},
            ],
        });
        let mut n = Nested {
            chain_path: vec![0],
            rack_path: vec![],
            chain_names: vec!["Pad".into()],
            device_index: 0,
            device_name: "Simpler".into(),
            class_name: "OriginalSimpler".into(),
            params: params_of(&device),
            device_json: device,
        };
        n.quote_displays(&[]);
        assert_eq!(n.params[0].value, 93.0);
        assert_eq!(n.params[0].range_text(), "0 … 127");
    }

    use super::*;
    use serde_json::json;

    /// A drum rack as `get_device_parameters` reports one: two pads, each
    /// with its own Simpler, and a second device on the kick's pad.
    fn drum_rack() -> Value {
        json!({
            "index": 0,
            "name": "808 Core Kit",
            "class_name": "DrumGroupDevice",
            "type": "drum_machine",
            "parameters": [
                {"index": 0, "name": "Macro 1", "value": 0.0, "min": 0.0, "max": 127.0}
            ],
            "chains": [
                {"index": 0, "kind": "chains", "chain_name": "Kick Dump", "out_note": 60,
                 "devices": [
                     {"index": 0, "name": "Simpler", "class_name": "OriginalSimpler",
                      "type": "instrument", "parameters": [
                          {"index": 0, "name": "Volume", "value": 0.5, "min": 0.0, "max": 1.0},
                          {"index": 1, "name": "Snap", "value": 0.0, "min": 0.0, "max": 1.0},
                          {"index": 2, "name": "Decay", "value": 0.3, "min": 0.0, "max": 1.0}
                      ]},
                     {"index": 1, "name": "Sub", "class_name": "Operator",
                      "type": "instrument", "parameters": [
                          {"index": 0, "name": "Volume", "value": 0.2, "min": 0.0, "max": 1.0}
                      ]}
                 ]},
                {"index": 1, "kind": "chains", "chain_name": "Snare Top", "out_note": 60,
                 "devices": [
                     {"index": 0, "name": "Simpler", "class_name": "OriginalSimpler",
                      "type": "instrument", "parameters": [
                          {"index": 0, "name": "Volume", "value": 0.5, "min": 0.0, "max": 1.0},
                          {"index": 1, "name": "Decay", "value": 0.7, "min": 0.0, "max": 1.0}
                      ]}
                 ]}
            ]
        })
    }

    #[test]
    fn a_pad_is_reached_by_its_name() {
        for word in ["Snare Top", "snare"] {
            let n = resolve(&drum_rack(), word).unwrap_or_else(|e| panic!("{word}: {e}"));
            assert_eq!(n.chain_path, vec![1], "{word}");
            assert_eq!(n.device_name, "Simpler");
            assert_eq!(n.params.len(), 2, "the pad's own Simpler, not the rack");
        }
    }

    #[test]
    fn a_second_device_on_the_pad_is_the_last_word() {
        let n = resolve(&drum_rack(), "Kick Dump/Sub").unwrap();
        assert_eq!(n.device_index, 1);
        assert_eq!(n.device_name, "Sub");
        assert_eq!(n.chain_names, vec!["Kick Dump"]);
    }

    #[test]
    fn the_path_is_the_one_live_answers_to() {
        let n = resolve(&drum_rack(), "Kick Dump/Sub").unwrap();
        let track = Path::track(3);
        assert_eq!(
            n.path(&track, 0).as_str(),
            "song.tracks[3].devices[0].chains[0].devices[1]"
        );
        assert_eq!(
            n.parameter_path(&track, 0, 2).as_str(),
            "song.tracks[3].devices[0].chains[0].devices[1].parameters[2]"
        );
    }

    #[test]
    fn a_word_that_matches_two_chains_names_both_and_picks_neither() {
        let mut rack = drum_rack();
        rack["chains"][1]["chain_name"] = json!("Kick Tail");
        let err = resolve(&rack, "kick").unwrap_err();
        assert!(err.contains("matches 2 chains"), "{err}");
        assert!(
            err.contains("Kick Dump") && err.contains("Kick Tail"),
            "{err}"
        );
    }

    #[test]
    fn an_unknown_chain_says_what_the_rack_holds() {
        let err = resolve(&drum_rack(), "Hi Hat").unwrap_err();
        assert!(err.contains("no chain called 'Hi Hat'"), "{err}");
        assert!(err.contains("Kick Dump"), "it lists them: {err}");
        assert!(err.contains("Snare Top"), "{err}");
    }

    #[test]
    fn a_device_that_is_not_a_rack_says_so_rather_than_naming_a_chain() {
        let plain = json!({"index": 0, "name": "Operator", "class_name": "Operator",
                           "type": "instrument", "parameters": []});
        let err = resolve(&plain, "Kick").unwrap_err();
        assert!(err.contains("no chains to look inside"), "{err}");
    }

    #[test]
    fn a_rack_inside_a_rack_is_a_path_of_names() {
        let nested = json!({
            "index": 0, "name": "Bass Rack", "class_name": "InstrumentGroupDevice",
            "type": "rack", "parameters": [],
            "chains": [
                {"index": 0, "kind": "chains", "chain_name": "Low", "devices": [
                    {"index": 0, "name": "Inner Rack", "class_name": "InstrumentGroupDevice",
                     "type": "rack", "parameters": [], "chains": [
                        {"index": 0, "kind": "chains", "chain_name": "Sine", "devices": [
                            {"index": 0, "name": "Operator", "class_name": "Operator",
                             "type": "instrument", "parameters": [
                                {"index": 0, "name": "Volume", "value": 0.4, "min": 0.0, "max": 1.0}
                             ]}
                        ]}
                     ]}
                ]}
            ]
        });
        let n = resolve(&nested, "Low/Sine").unwrap();
        assert_eq!(n.device_name, "Operator");
        assert_eq!(
            n.path(&Path::track(0), 0).as_str(),
            "song.tracks[0].devices[0].chains[0].devices[0].chains[0].devices[0]"
        );
    }

    /// A kit with three pads of the same name — which is what a real Rollin
    /// Breaks Kit has ("Snare Rolling" ×3, measured on Live 12.4.6).
    fn kit_with_repeats() -> Value {
        let pad = |name: &str| {
            json!({"index": 0, "kind": "chains", "chain_name": name,
                   "devices": [{"index": 0, "name": name, "class_name": "DrumCell",
                                "type": "instrument",
                                "parameters": [{"index": 0, "name": "Volume",
                                                "value": 0.5, "min": 0.0, "max": 1.0}]}]})
        };
        let mut chains = vec![pad("Kick Alpha")];
        for _ in 0..3 {
            chains.push(pad("Snare Rolling"));
        }
        json!({"index": 0, "name": "Rollin Breaks Kit", "class_name": "DrumGroupDevice",
               "parameters": [], "chains": chains})
    }

    /// A name that matches several pads is answered with those pads — never
    /// with "no such chain".
    ///
    /// The walk used to retry a failed chain lookup as a *device* name on the
    /// last word, discarding why it failed. So a kit with three "Snare
    /// Rolling" pads answered "no chain or device called 'Snare Rolling'":
    /// the producer named a pad the kit really has, three times over, and was
    /// told it did not exist. Found against a real Live.
    #[test]
    fn a_pad_name_the_kit_has_several_of_lists_them_rather_than_denying_it() {
        let kit = kit_with_repeats();
        for given in ["Snare Rolling", "snare rolling", "Snare"] {
            let e = resolve(&kit, given).unwrap_err();
            assert!(
                e.contains("Snare Rolling"),
                "'{given}' should be answered with the pads it matches, got: {e}"
            );
            assert!(
                !e.contains("no chain or device called") && !e.contains("no chain called"),
                "'{given}' names real pads, so it must not be denied: {e}"
            );
        }
        // And the one that is unambiguous still resolves.
        assert_eq!(
            resolve(&kit, "Kick Alpha").unwrap().device_name,
            "Kick Alpha"
        );
    }

    /// The fall-through it must not break: a last word that matches no chain
    /// is still tried as a device on the chain already reached.
    #[test]
    fn a_last_word_that_names_no_chain_is_still_a_device_on_the_pad() {
        let n = resolve(&drum_rack(), "Kick Dump/Sub").unwrap();
        assert_eq!(n.device_name, "Sub");
    }

    /// A real Live reads the same `out_note` on every pad, so it can neither
    /// name a pad nor find one. Measured on 12.4.6, 2026-09-20: a Rollin
    /// Breaks Kit and an LDre Mellow Kit, sixteen chains each, all 60.
    #[test]
    fn a_pads_note_is_not_a_way_in_because_out_note_is_the_same_on_every_pad() {
        let rack = json!({
            "name": "Rollin Breaks Kit", "class_name": "DrumGroupDevice",
            "parameters": [],
            "chains": [
                {"index": 0, "kind": "chains", "chain_name": "Kick Alpha", "out_note": 60,
                 "devices": [{"index": 0, "name": "Kick", "class_name": "DrumCell", "parameters": []}]},
                {"index": 1, "kind": "chains", "chain_name": "Snare Rolling", "out_note": 60,
                 "devices": [{"index": 0, "name": "Snare", "class_name": "DrumCell", "parameters": []}]},
            ]
        });
        for given in ["C3", "60", "C1", "36"] {
            let e = resolve(&rack, given).unwrap_err();
            assert!(
                e.contains("Kick Alpha") && e.contains("Snare Rolling"),
                "'{given}' should be refused with the chain names, got: {e}"
            );
        }
        assert!(resolve(&rack, "Kick Alpha").is_ok());
    }

    /// And the readout never labels a pad with that note: every pad would
    /// carry the same one, which tells a producer nothing.
    #[test]
    fn a_chain_is_listed_by_its_name_alone() {
        let rack = json!({
            "name": "Rollin Breaks Kit", "class_name": "DrumGroupDevice", "parameters": [],
            "chains": [{"index": 0, "kind": "chains", "chain_name": "Kick Alpha",
                        "out_note": 60, "devices": []}]
        });
        let listed = chains_of(&rack);
        assert_eq!(listed[0].display(), "Kick Alpha");
    }

    #[test]
    fn a_display_resolves_to_the_value_live_showed_it_for() {
        let samples: Vec<(f64, String)> = vec![
            (0.0, "20 Hz".into()),
            (0.5, "200 Hz".into()),
            (0.75, "2.00 kHz".into()),
            (1.0, "20.0 kHz".into()),
        ];
        assert_eq!(nearest_display(&samples, "200 Hz"), Some(0.5));
        assert_eq!(nearest_display(&samples, "190 hz"), Some(0.5), "nearest");
        assert_eq!(nearest_display(&samples, "nowhere"), None);
        let choices: Vec<(f64, String)> =
            vec![(0.0, "Low Cut 48 dB".into()), (1.0, "Low Cut 12 dB".into())];
        assert_eq!(nearest_display(&choices, "low cut 12 db"), Some(1.0));
    }

    #[test]
    fn a_number_is_read_off_the_front_of_a_display_or_not_at_all() {
        assert_eq!(leading_number("200 Hz"), Some(200.0));
        assert_eq!(leading_number("-6.0 dB"), Some(-6.0));
        assert_eq!(leading_number("Low Cut 48 dB"), None);
        assert_eq!(leading_number(""), None);
    }
}
