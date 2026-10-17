//! Sidechains: one device keyed from another track.
//!
//! What Live's API can route, measured on Live 12.4.6 (2026-10-15) and in
//! Cycling '74's LOM reference:
//!
//! - **Compressor** (`CompressorDevice`, class `Compressor2`) has its own
//!   routable sidechain input: source track, tap (Pre FX, Post FX, Post
//!   Mixer) and its S/C controls. It is keyed natively.
//! - **Glue Compressor, Gate, Auto Filter, Multiband Dynamics, Drum Buss,
//!   Vocoder and every plug-in** expose no routing to the API — their
//!   sidechain chooser is not a parameter, and `PluginDevice` offers only its
//!   editor and presets. They are keyed through **AMM Sidechain**, the Max for
//!   Live device this repo ships (`AbletonMusicMaker_Devices/`), whose audio
//!   input *is* routable (a Max device's `DeviceIO`): on each hit it moves one
//!   parameter of the device (Modulate). The reply says so; it never claims
//!   the device's own input.
//! - **A track with no device named** gets AMM Sidechain in Duck (or Gate)
//!   mode: keyed gain reduction on the track, optionally only below a split
//!   frequency.
//!
//! Trigger tracks, measured: a Pre FX or Post FX tap keys whatever the
//! source's fader, mute or output (Sends Only included); a Post Mixer tap is
//! silent when the source is muted, at -inf, or when another track is soloed.
//! Live's own undo does not cover routing changes; `undo: true` does.

use crate::connection::LiveState;
use crate::lom::Path;
use crate::tools::{get_display, live_err, read_perf_state, require, ToolResult, TrackTarget};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

/// The device this repo ships, as Live's browser finds it once installed.
pub const AMM_DEVICE_NAME: &str = "AMM Sidechain";
pub const AMM_FOLDER: &str = "AbletonMusicMaker Devices";
pub const AMM_FILE: &str = "AMM Sidechain.amxd";
pub const AMM_URI: &str = "query:UserLibrary#AbletonMusicMaker%20Devices:AMM%20Sidechain.amxd";

/// Classes with a sidechain in Live's interface that the API cannot route
/// (measured on Live 12.4.6), and the parameter AMM Sidechain moves for them
/// when the call names none.
const UNROUTABLE: &[(&str, &str, Option<&str>)] = &[
    ("GlueCompressor", "Glue Compressor", Some("Threshold")),
    ("Gate", "Gate", Some("Threshold")),
    ("AutoFilter2", "Auto Filter", Some("Frequency")),
    ("AutoFilter", "Auto Filter", Some("Frequency")),
    ("MultibandDynamics", "Multiband Dynamics", None),
    ("DrumBuss", "Drum Buss", None),
    ("Vocoder", "Vocoder", None),
];

/// What the device readout says about a class whose sidechain the API
/// cannot route; None for every other class.
pub fn unroutable_note(class: &str) -> Option<String> {
    UNROUTABLE.iter().find(|(c, _, _)| *c == class).map(|(_, shown, default)| {
        format!(
            "Live's API cannot route {shown}'s own sidechain input. set_sidechain keys it through AMM Sidechain instead, moving {} on each hit.",
            default.map(|p| format!("its {p} (or any parameter you name)")).unwrap_or_else(|| "a parameter you name".to_string())
        )
    })
}

/// `Some(Value::Null)` for an explicit `null`, `None` when the field is
/// absent: a source of `null` means No Input, a missing one means "keep".
fn some_value<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Ok(Some(Value::deserialize(d)?))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SetSidechainParams {
    /// The track the keyed device is on: a name, an index, "master", or a
    /// return's name or letter
    #[serde(default)]
    pub track: Option<Value>,
    /// "track" (default), "return" or "master" — with a numeric track
    #[serde(default)]
    pub kind: Option<String>,
    /// The track by index, when `track` is not given
    #[serde(default)]
    pub track_index: Option<i64>,
    /// The device to key: a name ("Compressor", "Glue Compressor") or an
    /// index. Leave it out to duck the whole track with AMM Sidechain (or to
    /// change the Compressor or AMM Sidechain already keyed there).
    #[serde(default)]
    pub device: Option<Value>,
    /// The device by index, when `device` is not given
    #[serde(default)]
    pub device_index: Option<i64>,
    /// Inside a rack: the chain holding the device, as adv_set_device_parameter takes it
    #[serde(default)]
    pub in_chain: Option<String>,
    /// The key: a track's name (matched exactly: "Kick" never means
    /// "Kick B") or index, a return's name or letter, "master", or null for
    /// No Input. Leave it out to keep the current source.
    #[serde(default, deserialize_with = "some_value")]
    #[schemars(with = "Option<Value>")]
    pub source: Option<Value>,
    /// Where on the source the key is taken: "pre_fx", "post_fx" (default)
    /// or "post_mixer"
    #[serde(default)]
    pub tap: Option<String>,
    /// Compressor: S/C On (default true when a source is set)
    #[serde(default)]
    pub on: Option<bool>,
    /// Compressor: S/C Gain in dB
    #[serde(default)]
    pub gain_db: Option<f64>,
    /// Compressor S/C Mix, or AMM Sidechain's Mix: 0.0–1.0
    #[serde(default)]
    pub mix: Option<f64>,
    /// Compressor: S/C Listen
    #[serde(default)]
    pub listen: Option<bool>,
    /// Compressor: S/C EQ on. A new Compressor has it on, as an 80 Hz high pass.
    #[serde(default)]
    pub eq_on: Option<bool>,
    /// Compressor: S/C EQ type — "Low Shelf", "Bell", "High Shelf", "Low pass", "Peak", "High pass"
    #[serde(default)]
    pub eq_type: Option<String>,
    /// Compressor: S/C EQ frequency, Hz or a display like "1.2 kHz"
    #[serde(default)]
    pub eq_freq: Option<Value>,
    /// Compressor: S/C EQ Q
    #[serde(default)]
    pub eq_q: Option<f64>,
    /// Compressor: S/C EQ gain in dB
    #[serde(default)]
    pub eq_gain_db: Option<f64>,
    /// AMM Sidechain: "duck" (default), "gate" or "modulate"
    #[serde(default)]
    pub mode: Option<String>,
    /// AMM Sidechain: the key level, dB, above which it acts (default -30)
    #[serde(default)]
    pub threshold_db: Option<f64>,
    /// AMM Sidechain, duck: dB over the threshold per dB of reduction (default 4)
    #[serde(default)]
    pub ratio: Option<f64>,
    /// AMM Sidechain: the most a duck takes off, or how far a gate closes, dB (default 24)
    #[serde(default)]
    pub depth_db: Option<f64>,
    /// AMM Sidechain: attack in ms (default 1)
    #[serde(default)]
    pub attack_ms: Option<f64>,
    /// AMM Sidechain: release in ms (default 120)
    #[serde(default)]
    pub release_ms: Option<f64>,
    /// AMM Sidechain, duck: only duck below this frequency, Hz (0 = the whole band)
    #[serde(default)]
    pub split_hz: Option<f64>,
    /// Keying a device Live cannot route (Glue Compressor, Gate, Auto Filter,
    /// a plug-in…): the parameter AMM Sidechain moves on each hit. Defaults:
    /// Glue Compressor and Gate "Threshold", Auto Filter "Frequency".
    #[serde(default)]
    pub param: Option<String>,
    /// That parameter with no key, in its own display units ("-10 dB"); default its current value
    #[serde(default)]
    pub from: Option<Value>,
    /// That parameter on a full hit, in its own display units ("-30 dB")
    #[serde(default)]
    pub to: Option<Value>,
    /// Put back what the last set_sidechain on this device replaced. Live's
    /// own undo does not cover sidechain routing.
    #[serde(default)]
    pub undo: bool,
}

/// A source the producer named, as Live addresses it.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub kind: String,
    pub index: i64,
    pub name: String,
}

impl Source {
    fn json(&self) -> Value {
        json!({"kind": self.kind, "index": self.index})
    }
}

/// Resolve a sidechain source. A name matches one track **exactly** — case
/// aside, never by substring — so "Kick" is never "Kick B"; a name two tracks
/// share, or no track has, is refused with the candidates. A return goes by
/// its name or letter, the master by "master"/"main". `null` is No Input.
pub fn resolve_source(live: &LiveState, given: &Value) -> Result<Option<Source>, String> {
    let text = match given {
        Value::Null => return Ok(None),
        Value::Number(n) => {
            let index = n.as_i64().ok_or("a track index is a whole number")?;
            let state = read_perf_state(live)?;
            return state
                .tracks
                .iter()
                .find(|t| t.index == index)
                .map(|t| {
                    Some(Source {
                        kind: "track".into(),
                        index,
                        name: t.name.clone(),
                    })
                })
                .ok_or_else(|| {
                    format!(
                        "there is no track {index}; the set has {}",
                        state.tracks.len()
                    )
                });
        }
        Value::String(s) => s.trim().to_string(),
        other => {
            return Err(format!(
                "a source is a track name, an index or null, not {other}"
            ))
        }
    };
    if let Ok(index) = text.parse::<i64>() {
        return resolve_source(live, &json!(index));
    }
    let lower = text.to_lowercase();
    if matches!(lower.as_str(), "master" | "main") {
        return Ok(Some(Source {
            kind: "master".into(),
            index: -1,
            name: "Master".into(),
        }));
    }
    let state = read_perf_state(live)?;
    let exact: Vec<&crate::performance::TrackState> =
        state.tracks.iter().filter(|t| t.name == text).collect();
    let same: Vec<&crate::performance::TrackState> = if exact.is_empty() {
        state
            .tracks
            .iter()
            .filter(|t| t.name.trim().to_lowercase() == lower)
            .collect()
    } else {
        exact
    };
    match same.as_slice() {
        [one] => {
            return Ok(Some(Source {
                kind: "track".into(),
                index: one.index,
                name: one.name.clone(),
            }))
        }
        [] => {}
        many => {
            return Err(format!(
                "'{text}' names {} tracks (indexes {}); give the index of the one to key from",
                many.len(),
                many.iter()
                    .map(|t| t.index.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
    let returns = live
        .send_command("get_returns", None)
        .map_err(|e| live_err("read the returns", e))?;
    for r in returns
        .get("returns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = get_display(r, "name", "");
        let letter = get_display(r, "letter", "");
        if name.trim().to_lowercase() == lower
            || letter.to_lowercase() == lower
            || format!("return {}", letter.to_lowercase()) == lower
        {
            return Ok(Some(Source {
                kind: "return".into(),
                index: r.get("index").and_then(Value::as_i64).unwrap_or(0),
                name,
            }));
        }
    }
    // Refused, with what was probably meant: names that contain it or that it
    // contains — offered, never taken.
    let near: Vec<String> = state
        .tracks
        .iter()
        .filter(|t| {
            let n = t.name.to_lowercase();
            n.contains(&lower) || lower.contains(&n)
        })
        .map(|t| format!("'{}'", t.name))
        .collect();
    Err(if near.is_empty() {
        format!(
            "no track is called '{text}' (the set has: {})",
            state
                .tracks
                .iter()
                .map(|t| format!("'{}'", t.name))
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        format!(
            "no track is called exactly '{text}'; a source is matched exactly, never by part of a name. Did you mean {}?",
            near.join(" or ")
        )
    })
}

/// One sidechain state as a phrase: "keyed from Kick (Post FX) · S/C On ·
/// gain 0.00 dB · mix 100 % · S/C EQ High pass 80.0 Hz".
pub fn state_text(sc: &Value) -> String {
    let source = sc.get("source").and_then(Value::as_str);
    let mut parts: Vec<String> = vec![match source {
        Some(s) => format!(
            "keyed from {s}{}",
            sc.get("tap_display")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(|t| format!(" ({t})"))
                .unwrap_or_default()
        ),
        None => "No Input".to_string(),
    }];
    if let Some(amm) = sc.get("amm").filter(|a| a.is_object()) {
        parts.push(amm_text(amm));
        return parts.join(" · ");
    }
    parts.push(if sc.get("on").and_then(Value::as_bool) == Some(true) {
        "S/C On".into()
    } else {
        "S/C Off".into()
    });
    if let Some(g) = sc.get("gain_display").and_then(Value::as_str) {
        parts.push(format!("gain {g}"));
    }
    if let Some(m) = sc.get("mix").and_then(Value::as_f64) {
        parts.push(format!("mix {:.0} %", m * 100.0));
    }
    if sc.get("eq_on").and_then(Value::as_bool) == Some(true) {
        parts.push(format!(
            "S/C EQ {} {}",
            get_display(sc, "eq_type_display", "?"),
            get_display(sc, "eq_freq_display", "")
        ));
    }
    if sc.get("listen").and_then(Value::as_bool) == Some(true) {
        parts.push("Listen on (you hear the key, not the track)".into());
    }
    parts.join(" · ")
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn amm_text(amm: &Value) -> String {
    let mode = get_display(amm, "mode", "duck");
    let mut out = match mode.as_str() {
        "gate" => format!(
            "gate: opens over {:.1} dB, closes {:.0} dB",
            num(amm, "threshold_db").unwrap_or(-30.0),
            num(amm, "depth_db").unwrap_or(24.0)
        ),
        "modulate" => {
            let target = amm.get("target").filter(|t| t.is_object());
            match target {
                Some(t) => format!(
                    "moves {} › {} from {:.0} % to {:.0} % of its range on each hit{}",
                    get_display(t, "device", "?"),
                    get_display(t, "parameter", "?"),
                    num(amm, "from_pct").unwrap_or(0.0),
                    num(amm, "to_pct").unwrap_or(100.0),
                    if t.get("linked").and_then(Value::as_bool) == Some(true) {
                        ""
                    } else {
                        " — NOT linked: Live does not show that parameter as held"
                    }
                ),
                None => "modulate, with no target set".to_string(),
            }
        }
        _ => format!(
            "duck: threshold {:.1} dB, ratio {:.1}, up to {:.0} dB{}",
            num(amm, "threshold_db").unwrap_or(-30.0),
            num(amm, "ratio").unwrap_or(4.0),
            num(amm, "depth_db").unwrap_or(24.0),
            match num(amm, "split_hz") {
                Some(h) if h > 0.0 => format!(", only below {h:.0} Hz"),
                _ => String::new(),
            }
        ),
    };
    if mode != "modulate" {
        out.push_str(&format!(
            ", attack {:.1} ms, release {:.0} ms",
            num(amm, "attack_ms").unwrap_or(1.0),
            num(amm, "release_ms").unwrap_or(120.0)
        ));
        if let Some(m) = num(amm, "mix_pct").filter(|m| (*m - 100.0).abs() > 0.05) {
            out.push_str(&format!(", mix {m:.0} %"));
        }
    }
    format!("AMM Sidechain {out}")
}

/// One `get_context` warning as a sentence a producer can act on.
pub fn warning_text(w: &Value) -> String {
    let at = format!(
        "{} › {}{}",
        get_display(w, "track", "?"),
        w.get("in_chain")
            .and_then(Value::as_str)
            .map(|c| format!("{c} › "))
            .unwrap_or_default(),
        get_display(w, "device", "?")
    );
    match get_display(w, "problem", "").as_str() {
        "no_source" => format!(
            "{at} has its sidechain on and no source (No Input), so it ducks nothing — its source was deleted, or never set. set_sidechain gives it one; on: false turns it off."
        ),
        "own_track" => format!(
            "{at} is keyed from its own track: it compresses on itself, not on another part."
        ),
        "post_mixer_source_muted" => format!(
            "{at} keys from {} at Post Mixer, and {} is muted: a Post Mixer tap is silent then, so nothing ducks. Key from Post FX, which keys whatever the source's mute or fader.",
            get_display(w, "source", "?"),
            get_display(w, "source", "?")
        ),
        "post_mixer_source_silent" => format!(
            "{at} keys from {} at Post Mixer, and its fader is at -inf: that tap is silent, so nothing ducks. Key from Post FX.",
            get_display(w, "source", "?")
        ),
        "amm_no_target" => format!(
            "{at} is in Modulate mode with no target: it moves nothing. set_sidechain with device and param names one."
        ),
        "amm_target_unlinked" => format!(
            "{at} should move {} › {}, but Live does not show that parameter as held: the target moved or the device was reloaded. set_sidechain again re-links it.",
            get_display(w, "target_device", "?"),
            get_display(w, "target_parameter", "?")
        ),
        other => format!("{at}: {other}"),
    }
}

/// The lines a track removal adds for what it unkeyed.
pub fn unkeyed_text(deleted: &str, unkeyed: &Value) -> String {
    let mut out = String::new();
    for k in unkeyed.as_array().into_iter().flatten() {
        out.push_str(&format!(
            "\n{} › {}{} was keyed from {deleted}; it now has {}{}",
            get_display(k, "track", "?"),
            k.get("in_chain")
                .and_then(Value::as_str)
                .map(|c| format!("{c} › "))
                .unwrap_or_default(),
            get_display(k, "device", "?"),
            get_display(k, "now", "No Input"),
            if k.get("on").and_then(Value::as_bool) == Some(true) {
                ", with S/C On still on, so it ducks nothing — set_sidechain to key it again, or on: false."
            } else {
                "."
            }
        ));
    }
    out
}

/// The devices on a track that can be keyed, as Live lists them.
fn list(live: &LiveState, target: &TrackTarget) -> Result<Value, String> {
    require(live, "list_sidechains")?;
    live.send_command(
        "list_sidechains",
        Some(json!({"kind": target.kind, "index": target.index})),
    )
    .map_err(|e| live_err("read the track's sidechains", e))
}

/// Every AMM Sidechain in Modulate on a track's own chain, as (its own
/// position, the position of the device it moves). AMM Sidechain holds its
/// target by position, so a move or a removal on the chain re-targets it
/// unless it is told; empty when the script cannot list sidechains.
pub(crate) fn amm_targets(live: &LiveState, target: &TrackTarget) -> Vec<(i64, i64)> {
    if !live.script.has_capability("list_sidechains") {
        return Vec::new();
    }
    let Ok(listed) = list(live, target) else {
        return Vec::new();
    };
    listed
        .get("sidechains")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| e.get("in_chain").is_none_or(Value::is_null))
        .filter(|e| e.pointer("/sidechain/amm/mode").and_then(Value::as_str) == Some("modulate"))
        .filter_map(|e| {
            let own = e
                .get("path")
                .and_then(Value::as_str)?
                .rsplit("devices[")
                .next()?
                .trim_end_matches(']')
                .parse::<i64>()
                .ok()?;
            let moved = e
                .pointer("/sidechain/amm/target_device")
                .and_then(Value::as_i64)?;
            Some((own, moved))
        })
        .collect()
}

/// After the chain changed, point each AMM Sidechain at the device it moved
/// before. `now` maps a position before the change to the one after, None
/// for a device that is gone. Lines for the reply.
pub(crate) fn repoint_amm(
    live: &LiveState,
    target: &TrackTarget,
    before: &[(i64, i64)],
    names: &[String],
    now: &dyn Fn(i64) -> Option<i64>,
) -> String {
    let mut out = String::new();
    for &(own, moved) in before {
        let Some(own_now) = now(own) else {
            continue;
        };
        let name = names
            .get(moved as usize)
            .cloned()
            .unwrap_or_else(|| format!("device {moved}"));
        match now(moved) {
            None => out.push_str(&format!(
                "\nAMM Sidechain (device {own_now}) moved {name}, which is gone: it moves nothing now. Point it at another device with set_sidechain, or remove it."
            )),
            Some(m) if m == moved => {}
            Some(m) => {
                let path = top_path(target, own_now);
                match set_amm_params(live, &path, &[("Target Device", m as f64)]) {
                    Ok(()) => out.push_str(&format!(
                        "\nAMM Sidechain (device {own_now}) still moves {name}: its target follows it to device {m}."
                    )),
                    Err(e) => out.push_str(&format!(
                        "\nAMM Sidechain (device {own_now}) moved {name}, now at {m}, and could not be re-pointed ({e}): it moves device {moved} until set_sidechain is asked again."
                    )),
                }
            }
        }
    }
    out
}

fn listing_text(listed: &Value) -> String {
    let entries: Vec<String> = listed
        .get("sidechains")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|e| {
            format!(
                "{}{} (device {})",
                e.get("in_chain")
                    .and_then(Value::as_str)
                    .map(|c| format!("{c} › "))
                    .unwrap_or_default(),
                get_display(e, "device", "?"),
                get_display(e, "device_index", "?")
            )
        })
        .collect();
    if entries.is_empty() {
        "none".into()
    } else {
        entries.join(", ")
    }
}

/// Where AMM Sidechain is, installing it into the User Library when Live's
/// browser does not have it yet. The file goes beside the Remote Script, in
/// the User Library Live itself reads, and is the only file this writes.
pub fn ensure_device_installed(live: &LiveState) -> Result<(), String> {
    let listed = live
        .send_command(
            "get_browser_items_at_path",
            Some(json!({"path": format!("user_library/{AMM_FOLDER}")})),
        )
        .ok();
    let present = listed
        .as_ref()
        .and_then(|r| r.get("items"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|i| get_display(i, "name", "") == AMM_DEVICE_NAME)
        });
    if present {
        return Ok(());
    }
    // The User Library is where Live's Remote Script body is: two up from it.
    let body = live
        .script
        .get()
        .and_then(|i| i.reload)
        .and_then(|r| r.get("body_file").and_then(Value::as_str).map(std::path::PathBuf::from))
        .ok_or("AMM Sidechain is not in Live's User Library and the Remote Script did not say where that is: run ableton-music-maker-install-script")?;
    let library = body
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .ok_or("could not find the User Library from the Remote Script's path")?;
    crate::install::write_device(library).map_err(|e| {
        format!(
            "could not install AMM Sidechain into {}: {e}",
            library.display()
        )
    })?;
    // Live notices a new file in the User Library on its own; wait for it.
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let seen = live
            .send_command(
                "get_browser_items_at_path",
                Some(json!({"path": format!("user_library/{AMM_FOLDER}")})),
            )
            .ok()
            .and_then(|r| r.get("items").and_then(Value::as_array).cloned())
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|i| get_display(i, "name", "") == AMM_DEVICE_NAME)
            });
        if seen {
            return Ok(());
        }
    }
    Err("AMM Sidechain was installed into the User Library, but Live's browser has not listed it yet; try again in a moment".into())
}

/// Load AMM Sidechain at the end of a track's chain; its device index.
fn load_amm(live: &LiveState, target: &TrackTarget) -> Result<i64, String> {
    ensure_device_installed(live)?;
    require(live, "ensure_effects")?;
    let r = live
        .send_command(
            "ensure_effects",
            Some(json!({"kind": target.kind, "index": target.index,
                        "effects": [{"uri": AMM_URI, "name": AMM_DEVICE_NAME}], "on_existing": "add"})),
        )
        .map_err(|e| live_err("load AMM Sidechain", e))?;
    r.get("effects")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|e| e.get("index"))
        .and_then(Value::as_i64)
        .ok_or_else(|| "AMM Sidechain did not load".to_string())
}

fn lpath(text: &str) -> Result<Path, String> {
    Path::from_str_checked(text)
}

/// The class of the device at `path`, and its name.
fn class_at(live: &LiveState, path: &str) -> Result<(String, String), String> {
    let r = Value::Object(
        crate::lom::Batch::new()
            .push(crate::lom::Op::get(
                &lpath(&format!("{path}.class_name"))?,
                "class",
            ))
            .push(crate::lom::Op::get(
                &lpath(&format!("{path}.name"))?,
                "name",
            ))
            .run(live)?,
    );
    Ok((get_display(&r, "class", ""), get_display(&r, "name", "")))
}

fn top_path(target: &TrackTarget, device_index: i64) -> String {
    Path::track_of(&target.kind, target.index)
        .attr("devices")
        .at(device_index)
        .to_string()
}

fn mode_value(mode: &str) -> Result<f64, String> {
    match mode.trim().to_lowercase().as_str() {
        "duck" | "" => Ok(0.0),
        "gate" => Ok(1.0),
        "modulate" => Ok(2.0),
        other => Err(format!("mode is duck, gate or modulate, not '{other}'")),
    }
}

/// AMM Sidechain's own parameters by name, raw (its raw values are its units).
fn set_amm_params(live: &LiveState, path: &str, values: &[(&str, f64)]) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    let listed = crate::lom::Batch::new()
        .push(crate::lom::Op::get(
            &lpath(&format!("{path}.parameters"))?,
            "n",
        ))
        .run(live)?;
    let n = listed
        .get("n")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut names = crate::lom::Batch::new();
    for i in 0..n {
        names = names.push(crate::lom::Op::get(
            &lpath(&format!("{path}.parameters[{i}].name"))?,
            &format!("p{i}"),
        ));
    }
    let names = Value::Object(names.run(live)?);
    let mut writes = crate::lom::Batch::new();
    for (name, value) in values {
        let i = (0..n)
            .find(|i| get_display(&names, &format!("p{i}"), "") == *name)
            .ok_or_else(|| format!("AMM Sidechain has no '{name}' parameter: reinstall it"))?;
        writes = writes.push(crate::lom::Op::set(
            &lpath(&format!("{path}.parameters[{i}].value"))?,
            *value,
        ));
    }
    writes.run(live).map(|_| ())
}

fn get_state(live: &LiveState, path: &str) -> Result<Value, String> {
    live.send_command("get_sidechain", Some(json!({"path": path})))
        .map_err(|e| live_err("read the sidechain back", e))
}

pub fn set_sidechain_body(live: &LiveState, p: &SetSidechainParams) -> ToolResult {
    for c in ["set_sidechain", "get_sidechain", "list_sidechains"] {
        require(live, c)?;
    }
    let target = crate::tools::resolve_track_target(
        live,
        p.track.as_ref(),
        p.kind.as_deref(),
        p.track_index,
    )?;
    let in_chain = p
        .in_chain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let named_device = p.device.is_some() || p.device_index.is_some();
    let listed = list(live, &target)?;
    let track_name = get_display(&listed, "track", "the track");

    // Which device, and at what path.
    let (path, class, device_name, loaded) = if named_device || in_chain.is_some() {
        let di =
            crate::tools::resolve_device_index(live, &target, p.device.as_ref(), p.device_index)?;
        let path = match in_chain {
            Some(which) => {
                let (_, nested) = crate::tools::nested_device(live, &target, di, which)?;
                nested
                    .path(&Path::track_of(&target.kind, target.index), di)
                    .to_string()
            }
            None => top_path(&target, di),
        };
        let (class, name) = class_at(live, &path)?;
        // Inside a rack it is named by where it sits: "Duck › Compressor".
        let label = match in_chain {
            Some(which) => format!("{} › {name}", which.replace('/', " › ")),
            None => name,
        };
        (path, class, label, false)
    } else {
        // No device named: the one device here that keys this track — a
        // Compressor, or an AMM Sidechain ducking or gating it (one that
        // moves another device's parameter belongs to that device) — else a
        // new AMM Sidechain. Two candidates is a question, not a guess.
        let candidates: Vec<Value> = listed
            .get("sidechains")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|e| e.get("in_chain").is_none_or(Value::is_null))
            .filter(|e| {
                e.pointer("/sidechain/amm/mode").and_then(Value::as_str) != Some("modulate")
            })
            .cloned()
            .collect();
        if candidates.len() > 1 {
            return Err(format!(
                "{track_name} has more than one device that can be keyed — {} — name the one (device).",
                candidates
                    .iter()
                    .map(|e| format!("{} (device {})", get_display(e, "device", "?"), get_display(e, "device_index", "?")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        match candidates.into_iter().next() {
            Some(e) => (
                get_display(&e, "path", ""),
                get_display(&e, "class", ""),
                get_display(&e, "device", ""),
                false,
            ),
            None if p.undo => {
                return Err(format!("nothing on {track_name} has been keyed to undo"))
            }
            None => {
                let di = load_amm(live, &target)?;
                (
                    top_path(&target, di),
                    "MxDeviceAudioEffect".to_string(),
                    AMM_DEVICE_NAME.to_string(),
                    true,
                )
            }
        }
    };

    if p.undo {
        let r = live
            .send_command("set_sidechain", Some(json!({"path": path, "undo": true})))
            .map_err(|e| live_err("undo the sidechain change", e))?;
        return Ok(format!(
            "Put back {track_name} › {device_name}: {} (it was {}). Live's own undo does not cover sidechain routing; this did.",
            state_text(r.get("sidechain").unwrap_or(&Value::Null)),
            state_text(r.get("previous").unwrap_or(&Value::Null))
        ));
    }

    let unroutable = UNROUTABLE.iter().find(|(c, _, _)| *c == class);
    let routable_here =
        class == "Compressor2" || class == "MxDeviceAudioEffect" || class == "MxDeviceInstrument";
    if unroutable.is_some() || (!routable_here && named_device) {
        return key_through_amm(
            live,
            p,
            &target,
            &track_name,
            &path,
            &device_name,
            &class,
            unroutable,
        );
    }

    // The source: given, or kept.
    let current = get_state(live, &path)?;
    let current_sc = current.get("sidechain").cloned().unwrap_or(Value::Null);
    let source = match &p.source {
        Some(v) => resolve_source(live, v)?,
        None => match current_sc.get("source_kind").and_then(Value::as_str) {
            Some(kind) => Some(Source {
                kind: kind.to_string(),
                index: current_sc
                    .get("source_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                name: get_display(&current_sc, "source", ""),
            }),
            None if loaded => {
                return Err(
                    "give a source: the track whose hits duck this one, e.g. \"Kick\"".into(),
                )
            }
            None => None,
        },
    };
    let tap = match p.tap.as_deref().map(|t| t.trim().to_lowercase()) {
        Some(t) if matches!(t.as_str(), "pre_fx" | "post_fx" | "post_mixer") => t,
        Some(t) => return Err(format!("tap is pre_fx, post_fx or post_mixer, not '{t}'")),
        None => current_sc
            .get("tap")
            .and_then(Value::as_str)
            .filter(|_| p.source.is_none())
            .unwrap_or("post_fx")
            .to_string(),
    };

    let is_amm = current_sc.get("amm").is_some_and(Value::is_object) || loaded;
    let mut params = serde_json::Map::new();
    let mut displays = serde_json::Map::new();
    if !is_amm {
        let on = p.on.unwrap_or(
            source.is_some() || current_sc.get("on").and_then(Value::as_bool) == Some(true),
        );
        params.insert("on".into(), json!(if on { 1.0 } else { 0.0 }));
        if let Some(b) = p.listen {
            params.insert("listen".into(), json!(if b { 1.0 } else { 0.0 }));
        }
        if let Some(b) = p.eq_on {
            params.insert("eq_on".into(), json!(if b { 1.0 } else { 0.0 }));
        }
        if let Some(g) = p.gain_db {
            displays.insert("gain".into(), json!(format!("{g} dB")));
        }
        if let Some(m) = p.mix {
            if !(0.0..=1.0).contains(&m) {
                return Err(format!("mix is 0.0–1.0, not {m}"));
            }
            displays.insert("mix".into(), json!(format!("{} %", m * 100.0)));
        }
        if let Some(t) = &p.eq_type {
            displays.insert("eq_type".into(), json!(t));
        }
        match &p.eq_freq {
            Some(Value::Number(n)) => {
                displays.insert("eq_freq".into(), json!(format!("{} Hz", n)));
            }
            Some(Value::String(s)) => {
                displays.insert("eq_freq".into(), json!(s));
            }
            Some(other) => {
                return Err(format!(
                    "eq_freq is Hz or a display like \"1.2 kHz\", not {other}"
                ))
            }
            None => {}
        }
        if let Some(q) = p.eq_q {
            displays.insert("eq_q".into(), json!(format!("{q}")));
        }
        if let Some(g) = p.eq_gain_db {
            displays.insert("eq_gain".into(), json!(format!("{g} dB")));
        }
    } else if p.on == Some(false) {
        // AMM Sidechain has no S/C On: off is No Input.
        return set_and_report(
            live,
            &path,
            &track_name,
            &device_name,
            None,
            &tap,
            &params,
            &displays,
            &[],
            loaded,
        );
    }

    let mut amm: Vec<(&str, f64)> = Vec::new();
    if is_amm {
        if let Some(m) = &p.mode {
            if m.trim().eq_ignore_ascii_case("modulate") {
                return Err("modulate moves another device's parameter: name that device (device) and the parameter (param) instead".into());
            }
            amm.push(("Mode", mode_value(m)?));
        } else if loaded {
            amm.push(("Mode", 0.0));
        }
        for (name, value, lo, hi) in [
            ("Threshold", p.threshold_db, -60.0, 0.0),
            ("Ratio", p.ratio, 1.0, 20.0),
            ("Depth", p.depth_db, 0.0, 60.0),
            ("Attack", p.attack_ms, 0.1, 100.0),
            ("Release", p.release_ms, 5.0, 2000.0),
            ("Split", p.split_hz, 0.0, 500.0),
            ("Mix", p.mix.map(|m| m * 100.0), 0.0, 100.0),
        ] {
            if let Some(v) = value {
                if !(lo..=hi).contains(&v) {
                    return Err(format!("AMM Sidechain {name} runs {lo} to {hi}, not {v}"));
                }
                amm.push((name, v));
            }
        }
    }
    set_and_report(
        live,
        &path,
        &track_name,
        &device_name,
        source.as_ref(),
        &tap,
        &params,
        &displays,
        &amm,
        loaded,
    )
}

#[allow(clippy::too_many_arguments)]
fn set_and_report(
    live: &LiveState,
    path: &str,
    track_name: &str,
    device_name: &str,
    source: Option<&Source>,
    tap: &str,
    params: &serde_json::Map<String, Value>,
    displays: &serde_json::Map<String, Value>,
    amm: &[(&str, f64)],
    loaded: bool,
) -> ToolResult {
    let r = live
        .send_command(
            "set_sidechain",
            Some(
                json!({"path": path, "source": source.map(Source::json), "tap": tap,
                        "params": params, "displays": displays}),
            ),
        )
        .map_err(|e| sidechain_refusal(live, path, e))?;
    set_amm_params(live, path, amm)?;
    let after = if amm.is_empty() {
        r.get("sidechain").cloned().unwrap_or(Value::Null)
    } else {
        get_state(live, path)?
            .get("sidechain")
            .cloned()
            .unwrap_or(Value::Null)
    };
    let mut out = String::new();
    if loaded {
        out.push_str(&format!(
            "Loaded AMM Sidechain at the end of {track_name}'s chain (Duck; a track with no Compressor is ducked by it).\n"
        ));
    }
    out.push_str(&format!(
        "{track_name} › {device_name}: {} — read back from Live.",
        state_text(&after)
    ));
    if !loaded {
        out.push_str(&format!(
            " Was: {}.",
            state_text(r.get("previous").unwrap_or(&Value::Null))
        ));
    }
    out.push_str(&notes(&after));
    out.push_str("\nLive's own undo does not cover sidechain routing; set_sidechain {\"undo\": true} on this device puts the previous one back.");
    Ok(out)
}

/// What is worth saying about a state just set.
fn notes(sc: &Value) -> String {
    let mut out = String::new();
    if sc.get("tap").and_then(Value::as_str) == Some("post_mixer") {
        out.push_str("\nPost Mixer goes silent — and nothing ducks — while the source is muted, at -inf, or another track is soloed (a capture_mix of one track solos it). Post FX keys regardless.");
    }
    if sc.get("eq_on").and_then(Value::as_bool) == Some(true)
        && sc.get("amm").is_none()
        && get_display(sc, "eq_type_display", "").eq_ignore_ascii_case("high pass")
    {
        out.push_str(&format!(
            "\nThe S/C EQ is on as a high pass at {}: the compressor hears the key above it — a kick's fundamental may sit below. eq_on: false keys from all of it.",
            get_display(sc, "eq_freq_display", "?")
        ));
    }
    out
}

/// The script's refusal, with what the track does have that can be keyed.
fn sidechain_refusal(live: &LiveState, path: &str, e: crate::connection::LiveError) -> String {
    let msg = live_err("set the sidechain", e);
    if msg.contains("no sidechain input") {
        if let Some(track) = path.split(".devices[").next() {
            let kind_index = if track == "song.master_track" {
                Some(TrackTarget::at("master", -1))
            } else if let Some(i) = track
                .strip_prefix("song.tracks[")
                .and_then(|s| s.strip_suffix(']'))
            {
                i.parse().ok().map(|i| TrackTarget::at("track", i))
            } else {
                track
                    .strip_prefix("song.return_tracks[")
                    .and_then(|s| s.strip_suffix(']'))
                    .and_then(|i| i.parse().ok())
                    .map(|i| TrackTarget::at("return", i))
            };
            if let Some(t) = kind_index {
                if let Ok(listed) = list(live, &t) {
                    return format!(
                        "{msg}. Devices on this track that can be keyed: {}.",
                        listing_text(&listed)
                    );
                }
            }
        }
    }
    msg
}

/// A device Live cannot route, keyed through AMM Sidechain in Modulate mode:
/// on each hit from the source, one of its parameters moves from `from` to
/// `to`. One AMM Sidechain per keyed parameter, reused when it is already
/// the one driving it.
#[allow(clippy::too_many_arguments)]
fn key_through_amm(
    live: &LiveState,
    p: &SetSidechainParams,
    target: &TrackTarget,
    track_name: &str,
    path: &str,
    device_name: &str,
    class: &str,
    known: Option<&(&str, &str, Option<&str>)>,
) -> ToolResult {
    let why = match known {
        Some((_, shown, _)) => format!(
            "Live's API cannot route {shown}'s own sidechain input (only Compressor's and a Max device's)"
        ),
        None => format!(
            "Live's API cannot route a sidechain into {device_name} ({class}): only Compressor and Max devices have one it can set"
        ),
    };
    let device_index = path
        .rsplit("devices[")
        .next()
        .and_then(|s| s.trim_end_matches(']').parse::<i64>().ok())
        .ok_or("could not read the device's position")?;
    if path.contains(".chains[") {
        return Err(format!(
            "{why}, and AMM Sidechain can only move a parameter of a device on a track's own chain, not inside a rack. Move {device_name} out of the rack, or key a Compressor there."
        ));
    }
    let param_name = match p.param.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => name.to_string(),
        None => match known.and_then(|k| k.2) {
            Some(default) => default.to_string(),
            None => {
                return Err(format!(
                    "{why}. It can be keyed through AMM Sidechain moving one of its parameters on each hit: say which (param) and where it goes (to, in its own units). {}",
                    keyable_text(live, target, track_name)
                ))
            }
        },
    };
    // The parameter, by name, on the device.
    let listed = crate::lom::Batch::new()
        .push(crate::lom::Op::get(
            &lpath(&format!("{path}.parameters"))?,
            "n",
        ))
        .run(live)?;
    let n = listed
        .get("n")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut names = crate::lom::Batch::new();
    for i in 0..n {
        names = names.push(crate::lom::Op::get(
            &lpath(&format!("{path}.parameters[{i}].name"))?,
            &format!("p{i}"),
        ));
    }
    let names = Value::Object(names.run(live)?);
    let all: Vec<String> = (0..n)
        .map(|i| get_display(&names, &format!("p{i}"), ""))
        .collect();
    let want = param_name.to_lowercase();
    let param_index = all
        .iter()
        .position(|n| n.to_lowercase() == want)
        .ok_or_else(|| {
            format!(
                "{device_name} has no parameter '{param_name}'; it has: {}",
                all.iter().skip(1).cloned().collect::<Vec<_>>().join(", ")
            )
        })?;
    let source = match &p.source {
        Some(v) => resolve_source(live, v)?,
        None => return Err("give a source: the track whose hits move it, e.g. \"Kick\"".into()),
    };
    let Some(source) = source else {
        // No Input: release whatever AMM Sidechain drives this parameter.
        return release_amm(
            live,
            target,
            track_name,
            device_name,
            &all[param_index],
            device_index,
            param_index as i64,
        );
    };
    let param_path = format!("{path}.parameters[{param_index}]");
    let mut displays: Vec<Value> = Vec::new();
    displays.push(p.from.clone().unwrap_or(Value::Null));
    displays.push(
        p.to.clone()
            .or_else(|| default_to(class, &all[param_index]))
            .ok_or_else(|| {
                format!(
                    "{why}. Say where {} goes on each hit (to, in its own units, e.g. \"-30 dB\").",
                    all[param_index]
                )
            })?,
    );
    let fr = fractions(live, &param_path, &displays)?;
    let from_pct = fr.0;
    let to_pct = fr.1;

    // Reuse the AMM Sidechain already driving this parameter, else load one.
    let sc_listed = list(live, target)?;
    let existing = sc_listed
        .get("sidechains")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|e| {
            e.pointer("/sidechain/amm/mode").and_then(Value::as_str) == Some("modulate")
                && e.pointer("/sidechain/amm/target_device")
                    .and_then(Value::as_i64)
                    == Some(device_index)
                && e.pointer("/sidechain/amm/target_param")
                    .and_then(Value::as_i64)
                    == Some(param_index as i64)
        })
        .and_then(|e| e.get("path").and_then(Value::as_str).map(str::to_string));
    let (amm_path, loaded) = match existing {
        Some(path) => (path, false),
        None => (top_path(target, load_amm(live, target)?), true),
    };
    let tap = p.tap.clone().unwrap_or_else(|| "post_fx".into());
    live.send_command(
        "set_sidechain",
        Some(json!({"path": amm_path, "source": source.json(), "tap": tap})),
    )
    .map_err(|e| live_err("route AMM Sidechain's key", e))?;
    let mut values: Vec<(&str, f64)> = vec![
        ("Target Device", device_index as f64),
        ("Target Param", param_index as f64),
        ("From", from_pct),
        ("To", to_pct),
        ("Mode", 2.0),
    ];
    for (name, value) in [
        ("Threshold", p.threshold_db),
        ("Depth", p.depth_db),
        ("Attack", p.attack_ms),
        ("Release", p.release_ms),
    ] {
        if let Some(v) = value {
            values.push((name, v));
        }
    }
    set_amm_params(live, &amm_path, &values)?;
    // live.path resolves the target on Live's own thread: give it a moment.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let after = get_state(live, &amm_path)?;
    let sc = after.get("sidechain").cloned().unwrap_or(Value::Null);
    let linked = sc.pointer("/amm/target/linked").and_then(Value::as_bool) == Some(true);
    let mut out = format!(
        "{why}, so {track_name} › {device_name} is keyed through AMM Sidechain{}: on each hit from {} ({}), its {} moves from {} to {}.\n{track_name} › AMM Sidechain: {} — read back from Live.",
        if loaded { " (loaded at the end of the chain)" } else { "" },
        source.name,
        tap.replace('_', " "),
        all[param_index],
        fr.2,
        fr.3,
        state_text(&sc)
    );
    if !linked {
        out.push_str("\nLive does not yet show the parameter as held by AMM Sidechain; get_context re-checks it, and set_sidechain again re-links it.");
    }
    if all[param_index].eq_ignore_ascii_case("threshold") && class != "Gate" {
        out.push_str(&format!(
            "\nOn each hit the Threshold is {}: material on {track_name} quieter than that is not compressed. {}",
            fr.3,
            if to_pct <= 0.5 {
                "That is as low as it goes: for a deeper duck raise the ratio, or shorten the release so it recovers inside the beat."
            } else {
                "Pass `to` lower to go deeper, or raise the ratio."
            }
        ));
    }
    if known.is_some() {
        out.push_str(&format!(
            "\nThis is {device_name} moved by the key, not its own sidechain input: its detector still hears its own track."
        ));
    }
    Ok(out)
}

/// The devices on the track whose own sidechain can be keyed, named for a
/// refusal so the producer has somewhere to go.
fn keyable_text(live: &LiveState, target: &TrackTarget, track_name: &str) -> String {
    let names: Vec<String> = list(live, target)
        .ok()
        .and_then(|l| l.get("sidechains").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .map(|e| {
            let d = get_display(e, "device", "?");
            match e.get("in_chain").and_then(Value::as_str) {
                Some(chain) => format!("{d} (in {chain})"),
                None => d,
            }
        })
        .collect();
    if names.is_empty() {
        format!(
            "Nothing on {track_name} has a sidechain Live's API can route; with no device named, set_sidechain ducks {track_name} with AMM Sidechain."
        )
    } else {
        format!(
            "On {track_name}, these can be keyed directly: {}.",
            names.join(", ")
        )
    }
}

fn default_to(class: &str, param: &str) -> Option<Value> {
    // A keyed threshold drops all the way on each hit: for a compressor's,
    // as much compression under the kick as it has; for a gate's, it opens.
    // Glue's floor is -40 dB. Measured on Live 12.4.6, 2026-10-15, a held
    // Operator note into Glue at its defaults (ratio 4, release 0.6 s),
    // 16th steps at 128 BPM: "12 dB below" its 0 dB ducked 0.0 dB at
    // -13.6 dBFS; on a track peaking at -18 dBFS, -30 dB ducked 1.8 dB and
    // -40 dB 4.8 dB. Glue's release is longer than a beat, so it never
    // recovers fully; a shallower move barely shows.
    match (class, param) {
        ("GlueCompressor", "Threshold") => Some(json!("min")),
        ("Gate", "Threshold") => Some(json!("min")),
        _ => None,
    }
}

/// From and To as % of the target's range, with Live's displays for them.
fn fractions(
    live: &LiveState,
    param_path: &str,
    given: &[Value],
) -> Result<(f64, f64, String, String), String> {
    require(live, "param_fraction")?;
    let now = live
        .send_command(
            "param_fraction",
            Some(json!({"path": param_path, "displays": []})),
        )
        .map_err(|e| live_err("read the target parameter", e))?;
    let current = now.get("value").and_then(Value::as_f64).unwrap_or(0.0);
    let lo = now.get("min").and_then(Value::as_f64).unwrap_or(0.0);
    let hi = now.get("max").and_then(Value::as_f64).unwrap_or(1.0);
    let resolve = |v: &Value| -> Result<Value, String> {
        match v {
            Value::Null => Ok(json!(current)),
            Value::String(s) if s.trim().eq_ignore_ascii_case("min") => Ok(json!(lo)),
            Value::String(s) if s.trim().eq_ignore_ascii_case("max") => Ok(json!(hi)),
            Value::String(s) if s.ends_with(" below") => {
                // "-12 dB below": relative to where it is now.
                let shown = get_display(&now, "display", "");
                let base = shown.replace("dB", "").trim().parse::<f64>().map_err(|_| {
                    format!("'{shown}' is not a dB value to go below; give `to` in its units")
                })?;
                let delta = s
                    .trim_end_matches(" below")
                    .replace("dB", "")
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("'{s}' is not a dB offset"))?;
                Ok(json!(format!("{} dB", base + delta)))
            }
            other => Ok(other.clone()),
        }
    };
    let a = resolve(&given[0])?;
    let b = resolve(&given[1])?;
    let r = live
        .send_command(
            "param_fraction",
            Some(json!({"path": param_path, "displays": [a, b]})),
        )
        .map_err(|e| live_err("convert From and To", e))?;
    let f = r
        .get("fractions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pct = |i: usize| {
        f.get(i)
            .and_then(|x| x.get("pct"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let shown = |i: usize| {
        f.get(i)
            .map(|x| get_display(x, "display", "?"))
            .unwrap_or_default()
    };
    Ok((
        pct(0).clamp(0.0, 100.0),
        pct(1).clamp(0.0, 100.0),
        shown(0),
        shown(1),
    ))
}

fn release_amm(
    live: &LiveState,
    target: &TrackTarget,
    track_name: &str,
    device_name: &str,
    param: &str,
    device_index: i64,
    param_index: i64,
) -> ToolResult {
    let listed = list(live, target)?;
    let found = listed
        .get("sidechains")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|e| {
            e.pointer("/sidechain/amm/target_device")
                .and_then(Value::as_i64)
                == Some(device_index)
                && e.pointer("/sidechain/amm/target_param")
                    .and_then(Value::as_i64)
                    == Some(param_index)
        })
        .and_then(|e| e.get("path").and_then(Value::as_str).map(str::to_string));
    let Some(path) = found else {
        return Ok(format!(
            "Nothing keys {track_name} › {device_name} › {param}; nothing changed."
        ));
    };
    live.send_command("set_sidechain", Some(json!({"path": path, "source": null})))
        .map_err(|e| live_err("unkey AMM Sidechain", e))?;
    set_amm_params(live, &path, &[("Mode", 0.0), ("Target Device", -1.0)])?;
    Ok(format!(
        "{track_name} › {device_name} › {param} is no longer keyed: its AMM Sidechain has No Input and let go of it (the parameter is yours again, at the value it had)."
    ))
}
