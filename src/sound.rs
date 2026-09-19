//! Sound without leaving the flow: a shared vocabulary (cutoff, resonance,
//! attack, decay, sustain, release, drive, detune, width, lfo_rate,
//! reverb, delay) resolved against a device's parameters at call time.
//! Rack macros are asked first, by name, because most Live presets are
//! racks whose maker exposed the intent as eight named macros; then a table
//! of candidate parameter names per Live instrument; then the word itself
//! as a name substring. Pure: the tool bodies read the parameters and
//! write the values.

use serde_json::Value;

/// One parameter as `get_device_parameters` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub index: i64,
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub value_string: Option<String>,
}

impl Param {
    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self {
            index: v.get("index")?.as_i64()?,
            name: v.get("name")?.as_str()?.to_string(),
            value: v.get("value").and_then(Value::as_f64).unwrap_or(0.0),
            min: v.get("min").and_then(Value::as_f64).unwrap_or(0.0),
            max: v.get("max").and_then(Value::as_f64).unwrap_or(1.0),
            value_string: v
                .get("value_string")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// Where the value sits in its range, 0–1.
    pub fn fraction(&self) -> f64 {
        let span = self.max - self.min;
        if span.abs() < 1e-12 {
            0.0
        } else {
            ((self.value - self.min) / span).clamp(0.0, 1.0)
        }
    }

    /// The value at a fraction of the range.
    pub fn at_fraction(&self, f: f64) -> f64 {
        self.min + (self.max - self.min) * f.clamp(0.0, 1.0)
    }

    /// `62%`, or Live's own display string when it has one.
    pub fn display(&self) -> String {
        match &self.value_string {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => format!("{}%", (self.fraction() * 100.0).round() as i64),
        }
    }
}

/// The words `shape_sound` answers to, in the order the reply lists them.
pub const WORDS: &[&str] = &[
    "cutoff",
    "resonance",
    "attack",
    "decay",
    "sustain",
    "release",
    "drive",
    "detune",
    "width",
    "lfo_rate",
    "reverb",
    "delay",
];

/// What a word may be called on a rack macro or any device, in order of preference.
fn aliases(word: &str) -> &'static [&'static str] {
    match word {
        "cutoff" => &[
            "cutoff",
            "filter freq",
            "filter frequency",
            "freq",
            "frequency",
            "filter",
            "cut",
            "brightness",
            "tone",
        ],
        "resonance" => &["resonance", "reso", "res", "q"],
        "attack" => &["attack", "atk"],
        "decay" => &["decay", "dec"],
        "sustain" => &["sustain", "sus"],
        "release" => &["release", "rel"],
        "drive" => &[
            "drive",
            "saturation",
            "sat",
            "distortion",
            "dist",
            "overdrive",
            "grit",
            "dirt",
        ],
        "detune" => &["detune", "unison", "spread", "fine"],
        "width" => &["width", "stereo", "wide", "pan spread"],
        "lfo_rate" => &[
            "lfo rate",
            "lfo freq",
            "lfo 1 rate",
            "lfo1 rate",
            "rate",
            "lfo",
        ],
        "reverb" => &["reverb", "verb", "space", "room", "hall", "ambience"],
        "delay" => &["delay", "echo", "feedback"],
        _ => &[],
    }
}

/// Candidate parameter names per Live instrument (class names as the API
/// reports them), tried before the aliases so the right envelope and
/// filter win when a device has several.
fn table(class_name: &str, word: &str) -> &'static [&'static str] {
    let class = class_name.to_lowercase();
    if class.contains("vector") || class.contains("wavetable") {
        // Wavetable (parameter names as Live 12.4 reports them)
        return match word {
            "cutoff" => &["flt 1 freq", "filter 1 freq"],
            "resonance" => &["flt 1 res", "filter 1 res"],
            "drive" => &["flt 1 drive", "filter 1 drive"],
            "attack" => &["amp attack"],
            "decay" => &["amp decay"],
            "sustain" => &["amp sustain"],
            "release" => &["amp release"],
            "detune" => &["unison amount", "osc 1 detune"],
            "lfo_rate" => &["lfo 1 rate"],
            _ => &[],
        };
    }
    if class.contains("ultraanalog") || class == "analog" {
        // Analog: the amp envelope is AEG1, the filter envelope FEG1
        return match word {
            "cutoff" => &["f1 freq"],
            "resonance" => &["f1 resonance"],
            "drive" => &["f1 drive"],
            "attack" => &["aeg1 attack"],
            "decay" => &["aeg1 decay"],
            "sustain" => &["aeg1 sustain"],
            "release" => &["aeg1 rel"],
            "detune" => &["osc1 detune", "unison detune"],
            "width" => &["unison detune"],
            "lfo_rate" => &["lfo1 speed", "lfo1 sncrate"],
            _ => &[],
        };
    }
    if class == "operator" {
        return match word {
            "cutoff" => &["filter freq"],
            "resonance" => &["filter res"],
            "drive" => &["filter drive", "drive"],
            "attack" => &["ae attack", "a attack"],
            "decay" => &["ae decay", "a decay"],
            "sustain" => &["ae sustain", "a sustain"],
            "release" => &["ae release", "a release"],
            "detune" => &["osc-a fine", "a fine"],
            "lfo_rate" => &["lfo rate"],
            _ => &[],
        };
    }
    if class == "drift" {
        // Drift: Env 1 is the amp envelope, LP Freq the filter
        return match word {
            "cutoff" => &["lp freq"],
            "resonance" => &["lp res"],
            "attack" => &["env 1 attack"],
            "decay" => &["env 1 decay"],
            "sustain" => &["env 1 sustain"],
            "release" => &["env 1 release"],
            "detune" => &["osc 2 detune"],
            "width" => &["spread"],
            "lfo_rate" => &["lfo rate"],
            _ => &[],
        };
    }
    if class.contains("simpler") || class.contains("sampler") || class.contains("multisampler") {
        return match word {
            "cutoff" => &["filter freq"],
            "resonance" => &["filter res"],
            "drive" => &["filter drive"],
            "attack" => &["ve attack"],
            "decay" => &["ve decay"],
            "sustain" => &["ve sustain"],
            "release" => &["ve release"],
            "detune" => &["detune", "transpose"],
            "lfo_rate" => &["l 1 rate", "lfo 1 rate", "lfo rate"],
            _ => &[],
        };
    }
    if class == "meld" {
        return match word {
            "cutoff" => &["filter 1 freq", "filter freq"],
            "resonance" => &["filter 1 res", "filter res"],
            "attack" => &["amp attack", "env 1 attack"],
            "decay" => &["amp decay", "env 1 decay"],
            "sustain" => &["amp sustain", "env 1 sustain"],
            "release" => &["amp release", "env 1 release"],
            "lfo_rate" => &["lfo 1 rate"],
            _ => &[],
        };
    }
    &[]
}

/// Rack devices expose their macros as parameters; the standard ones are not macros.
pub fn is_rack(class_name: &str) -> bool {
    class_name.to_lowercase().contains("rack") || class_name.to_lowercase().contains("groupdevice")
}

const RACK_FIXED: &[&str] = &["device on", "chain selector", "macro variations"];

/// The macros of a rack: its parameters minus the fixed ones.
pub fn macros(params: &[Param]) -> Vec<&Param> {
    params
        .iter()
        .filter(|p| {
            !RACK_FIXED
                .iter()
                .any(|f| p.name.to_lowercase().starts_with(f))
        })
        .collect()
}

/// How a word was resolved, for the reply.
#[derive(Debug, Clone, PartialEq)]
pub enum Via {
    Macro,
    Table,
    Alias,
    Name,
}

/// Resolve a word to a parameter: a rack's macros by name first, then the
/// instrument table, then the aliases, then the word itself as a substring.
pub fn resolve<'a>(word: &str, class_name: &str, params: &'a [Param]) -> Option<(&'a Param, Via)> {
    let word = word.trim().to_lowercase().replace(' ', "_");
    let candidates_named = |names: &[&str], pool: &[&'a Param]| -> Option<&'a Param> {
        for c in names {
            if let Some(p) = pool
                .iter()
                .find(|p| p.name.to_lowercase() == *c)
                .or_else(|| pool.iter().find(|p| p.name.to_lowercase().contains(c)))
            {
                return Some(p);
            }
        }
        None
    };
    let all: Vec<&Param> = params.iter().collect();
    if is_rack(class_name) {
        let ms = macros(params);
        if let Some(p) = candidates_named(aliases(&word), &ms) {
            return Some((p, Via::Macro));
        }
    }
    if let Some(p) = candidates_named(table(class_name, &word), &all) {
        return Some((p, Via::Table));
    }
    if let Some(p) = candidates_named(aliases(&word), &all) {
        return Some((p, Via::Alias));
    }
    let plain = word.replace('_', " ");
    all.iter()
        .find(|p| p.name.to_lowercase().contains(&plain))
        .map(|p| (*p, Via::Name))
}

/// A parameter by name substring (exact name first), the fallback for any device.
pub fn by_name<'a>(name: &str, params: &'a [Param]) -> Option<&'a Param> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    params
        .iter()
        .find(|p| p.name.to_lowercase() == want)
        .or_else(|| {
            params
                .iter()
                .find(|p| p.name.to_lowercase().contains(&want))
        })
}

/// A value for a parameter: a number 0–1 (a fraction of the range), or
/// "±N%" relative to where it sits now. Returns the new Live value.
pub fn target_value(p: &Param, given: &Value) -> Result<f64, String> {
    match given {
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            if !(0.0..=1.0).contains(&f) {
                return Err(format!(
                    "{}: give a fraction 0–1 of the range or \"±N%\", not {f}",
                    p.name
                ));
            }
            Ok(p.at_fraction(f))
        }
        Value::String(s) => {
            let t = s.trim().replace('%', "");
            let t = t.trim();
            if let Some(rest) = t.strip_prefix('+').or_else(|| t.strip_prefix('-')) {
                let n: f64 = rest
                    .trim()
                    .parse()
                    .map_err(|_| format!("{}: '{s}' is not a number", p.name))?;
                let sign = if t.starts_with('-') { -1.0 } else { 1.0 };
                let f = p.fraction() + sign * n / 100.0;
                Ok(p.at_fraction(f))
            } else {
                let n: f64 = t
                    .parse()
                    .map_err(|_| format!("{}: '{s}' is not a value (0–1, or \"±N%\")", p.name))?;
                let f = if s.contains('%') { n / 100.0 } else { n };
                if !(0.0..=1.0).contains(&f) {
                    return Err(format!(
                        "{}: {s} is outside the range (0–1 or 0–100%)",
                        p.name
                    ));
                }
                Ok(p.at_fraction(f))
            }
        }
        other => Err(format!("{}: {other} is not a value", p.name)),
    }
}

/// The words a device answers to, for the reply.
pub fn vocabulary(class_name: &str, params: &[Param]) -> Vec<(String, String)> {
    WORDS
        .iter()
        .filter_map(|w| {
            resolve(w, class_name, params).map(|(p, _)| (w.to_string(), p.name.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(names: &[&str]) -> Vec<Param> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| Param {
                index: i as i64,
                name: n.to_string(),
                value: 0.5,
                min: 0.0,
                max: 1.0,
                value_string: None,
            })
            .collect()
    }

    #[test]
    fn rack_macros_win_then_the_table_then_aliases_then_the_name() {
        let rack = params(&[
            "Device On",
            "Chain Selector",
            "Cutoff",
            "Res",
            "Space",
            "Grit",
            "Macro 5",
            "Macro 6",
            "Macro 7",
            "Macro 8",
        ]);
        let (p, via) = resolve("cutoff", "InstrumentGroupDevice", &rack).unwrap();
        assert_eq!((p.name.as_str(), via), ("Cutoff", Via::Macro));
        assert_eq!(
            resolve("resonance", "InstrumentGroupDevice", &rack)
                .unwrap()
                .0
                .name,
            "Res"
        );
        assert_eq!(
            resolve("reverb", "InstrumentGroupDevice", &rack)
                .unwrap()
                .0
                .name,
            "Space"
        );
        assert_eq!(
            resolve("drive", "InstrumentGroupDevice", &rack)
                .unwrap()
                .0
                .name,
            "Grit"
        );
        assert!(
            resolve("attack", "InstrumentGroupDevice", &rack).is_none(),
            "no macro says attack"
        );
        assert_eq!(macros(&rack).len(), 8);

        let wavetable = params(&[
            "Device On",
            "Filter 1 Freq",
            "Filter 2 Freq",
            "Filter 1 Res",
            "Amp Attack",
            "Filter Env Attack",
            "Amp Release",
            "Unison Amount",
            "LFO 1 Rate",
            "LFO 2 Rate",
        ]);
        let (p, via) = resolve("cutoff", "InstrumentVector", &wavetable).unwrap();
        assert_eq!((p.name.as_str(), via), ("Filter 1 Freq", Via::Table));
        assert_eq!(
            resolve("attack", "InstrumentVector", &wavetable)
                .unwrap()
                .0
                .name,
            "Amp Attack"
        );
        assert_eq!(
            resolve("lfo_rate", "InstrumentVector", &wavetable)
                .unwrap()
                .0
                .name,
            "LFO 1 Rate"
        );
        assert_eq!(
            resolve("detune", "InstrumentVector", &wavetable)
                .unwrap()
                .0
                .name,
            "Unison Amount"
        );

        let analog = params(&[
            "Device On",
            "F1 Freq",
            "F1 Res",
            "AE1 Attack",
            "AE1 Decay",
            "FE1 Attack",
            "F1 Drive",
            "OSC1 Detune",
        ]);
        assert_eq!(
            resolve("attack", "UltraAnalog", &analog).unwrap().0.name,
            "AE1 Attack"
        );
        assert_eq!(
            resolve("drive", "UltraAnalog", &analog).unwrap().0.name,
            "F1 Drive"
        );
        assert_eq!(
            resolve("detune", "UltraAnalog", &analog).unwrap().0.name,
            "OSC1 Detune"
        );

        // An unknown device: aliases, then the word as a substring.
        let serum = params(&[
            "Enable",
            "Cutoff",
            "Res",
            "Env1 Atk",
            "Osc A Unison Detune",
            "Fx Reverb Mix",
        ]);
        let (p, via) = resolve("cutoff", "PluginDevice", &serum).unwrap();
        assert_eq!((p.name.as_str(), via), ("Cutoff", Via::Alias));
        assert_eq!(
            resolve("attack", "PluginDevice", &serum).unwrap().0.name,
            "Env1 Atk"
        );
        assert_eq!(
            resolve("reverb", "PluginDevice", &serum).unwrap().0.name,
            "Fx Reverb Mix"
        );
        assert!(resolve("width", "PluginDevice", &serum).is_none());
        assert_eq!(by_name("atk", &serum).unwrap().name, "Env1 Atk");
        assert_eq!(
            by_name("res", &serum).unwrap().name,
            "Res",
            "exact name beats a substring"
        );
        assert!(by_name("", &serum).is_none());
        let words: Vec<String> = vocabulary("PluginDevice", &serum)
            .into_iter()
            .map(|(w, _)| w)
            .collect();
        assert_eq!(
            words,
            vec!["cutoff", "resonance", "attack", "detune", "reverb"]
        );
    }

    #[test]
    fn values_are_fractions_or_relative_percentages() {
        let p = Param {
            index: 3,
            name: "Cutoff".into(),
            value: 62.0,
            min: 0.0,
            max: 100.0,
            value_string: Some("62 %".into()),
        };
        assert_eq!(target_value(&p, &json!(0.37)).unwrap(), 37.0);
        assert_eq!(target_value(&p, &json!("-25%")).unwrap(), 37.0);
        assert_eq!(
            target_value(&p, &json!("+50%")).unwrap(),
            100.0,
            "clamped to the range"
        );
        assert_eq!(target_value(&p, &json!("40%")).unwrap(), 40.0);
        assert!(target_value(&p, &json!(1.5)).is_err());
        assert!(target_value(&p, &json!("dark")).is_err());
        assert_eq!(p.display(), "62 %");
        let q = Param {
            index: 0,
            name: "Drive".into(),
            value: 0.3,
            min: 0.0,
            max: 1.0,
            value_string: None,
        };
        assert_eq!(q.display(), "30%");
        assert_eq!(q.fraction(), 0.3);
    }
}
