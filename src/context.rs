//! `get_context`: the one call an agent makes to orient. The Remote Script
//! gathers everything in a single round trip (`get_context`); this module
//! turns it into the compact text the model reads, and holds the server
//! instructions every MCP client receives at `initialize`. Pure logic,
//! unit-tested on a synthetic payload.

use serde_json::Value;

/// The instructions the server hands every client at `initialize`. Clients
/// that surface them give the model the whole workflow before its first
/// call; `get_context` repeats the short form as a footer for those that
/// do not.
pub const INSTRUCTIONS: &str = "AbletonMusicMaker drives a running Ableton Live set through its Remote Script (third-party, not made by Ableton).

START with get_context: one call returns the set (tempo, key, position), every track with its devices, clips by slot and play state, the returns, the sections and the song, and the performance clock.

UNITS: faders and levels are in dB (0 dB is unity; the master's meter tops at 0 dB); positions in the arrangement are Live's 1-based bars; note times inside a clip are beats. Live's API cannot save the set: the producer presses Cmd+S in Live, and that is said once here, not in every reply.

MAKING MUSIC: build_song builds a whole set from one document: key, tempo, sections (scene rows with phrase lengths), tracks (instrument as plain words, searched for you, or a browser URI), Session clips with notes in compact forms (step strings per pitch, notes_csv lines, patterns, loop_every tiling), `slots` for copies in several rows, Arrangement placements and locators. It validates everything before the first command reaches Live; dry_run previews. Afterwards: create_clip and add_notes_to_clip (same note forms), set_key, set_tempo, load_instrument_or_effect (words or URI; kind return/master for effects), create_return, set_track_mixer (volume_db), set_send, shape_sound (cutoff, attack, reverb … as words, or any parameter by name), feel (swing, humanize, groove, retime, a variation; undo: true), arrange (place, repeat, move, delete, shorten, list — in bars). Tracks and sections are addressed by name.

HEARING IT: capture_mix records N bars of the master through a Capture track and reports peak, RMS per bar, silent bars, clipping and stereo width; clear_captures removes that track when you are done. Nothing leaves the machine.

SECTIONS AND SONGS: a section is a scene row named \"<name> · <bars>\" (its phrase length; Live's Save keeps it). make_section makes one from what plays, as a copy of another with per-track changes, or from clips per track. set_song writes the setlist into the 'Setlist:' scene: an entry without repeats loops until the performer says go. play_song fires the first section and cues the counted jumps; then go (end of the playing phrase, or at: next_bar which cuts it short and says so), hold_section, next_section, previous_section, back (the jump history) and jump_to {section, repeats, at, transition} steer it — each is one state read and one cue, and a jump into a section that ran more than 3 dB hotter warns. The level line (🔊) rides under the clock line on every reply while performing.

PERFORMING: start_performance (1-bar launch quantization, disarms tracks, sets the key, fires the first scene, turns on guards). Then cue for anything timed: steps at bar numbers (\"next_bar\", {\"bar\": N}, {\"bars_after\": k}) with fire_scene, fire_clip, stop_clip, set, or a ramp of tempo, crossfader or volume over bars. The Remote Script runs cues on Live's own clock, so plan two bars ahead and never try to hit a beat with a tool call. get_performance_state tells you the bar, what plays, what is queued and what fired since you last asked. keep_track_playing lets a layer added mid-set survive scene changes. record_clip records the producer playing. While a performance runs, stop_playback, set_tempo, playhead moves, capture_mix and deleting playing clips are refused with the on-the-bar alternative; end_performance stops on the bar, fades, or now.

RULES: Live's bar numbers are 1-based. Every launch lands on the next bar. A section launch stops tracks without a clip in that row (unless keep_track_playing). A fresh Live 12 set sits in its default C Major scale: set_key first. Live arms new MIDI tracks by itself: an armed track with an empty slot records on a scene launch.

THE SURFACE: the tools without a prefix are the artist's set and cover the whole workflow; the adv_ tools are the raw layer of scenes, clips, cues, meters, snapshots and the browser underneath, for when the artist's set has no word for it.";

/// The short form appended to every get_context result.
pub const FOOTER: &str = "Workflow: build_song (key, tempo, sections, tracks with instrument words, clips as step strings, notes_csv or patterns) → shape_sound and feel → arrange in bars → hear it with capture_mix → play_song, then go / hold_section / next_section / back / jump_to steer the sections; end_performance stops. Faders and levels in dB, positions in bars. Live's Save is yours (Cmd+S); the API cannot save.";

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn f(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
fn i(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(-1)
}
fn b(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
fn num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

/// The compact readout. `since` is the running performance (start bar,
/// seconds) if any.
pub fn context_text(ctx: &Value, since: Option<(i64, f64)>) -> String {
    let session = ctx.get("session").cloned().unwrap_or(Value::Null);
    let mut out = String::new();
    let live_version = s(ctx, "live_version");
    if !live_version.is_empty() {
        out.push_str(&format!(
            "Live {live_version} · script {} · ",
            s(ctx, "script_version")
        ));
    }
    out.push_str(&format!(
        "{} BPM {}/{} · {} at bar {}.{} · launch quantization {}",
        num(f(&session, "tempo")),
        i(&session, "signature_numerator"),
        i(&session, "signature_denominator"),
        if b(&session, "is_playing") {
            "playing"
        } else {
            "stopped"
        },
        i(&session, "bar"),
        i(&session, "beat_in_bar"),
        quant_words(s(&session, "clip_trigger_quantization_name"))
    ));
    let root = s(&session, "root_note_name");
    let scale = s(&session, "scale_name");
    if !root.is_empty() && !scale.is_empty() {
        out.push_str(&format!(
            " · key {root} {scale}{}",
            if session.get("scale_mode") == Some(&Value::Bool(false)) {
                " (scale mode off)"
            } else {
                ""
            }
        ));
    }
    if b(&session, "loop") {
        out.push_str(&format!(
            " · arrangement loop {}–{}",
            num(f(&session, "loop_start")),
            num(f(&session, "loop_start") + f(&session, "loop_length"))
        ));
    }
    out.push('\n');

    let tracks = arr(ctx, "tracks");
    out.push_str(&format!("Tracks ({}):\n", tracks.len()));
    for t in tracks {
        let mut flags = vec![s(t, "kind").to_string()];
        if b(t, "is_group") {
            flags.push("group".into());
        }
        if b(t, "arm") {
            flags.push("armed".into());
        }
        if b(t, "mute") {
            flags.push("muted".into());
        }
        if b(t, "solo") {
            flags.push("solo".into());
        }
        let devices: Vec<String> = arr(t, "devices")
            .iter()
            .map(|d| {
                let ty = s(d, "type");
                if ty == "drum_machine" {
                    format!("{} (drum rack)", s(d, "name"))
                } else {
                    s(d, "name").to_string()
                }
            })
            .collect();
        let devices = if devices.is_empty() {
            "no devices".to_string()
        } else {
            devices.join(" › ")
        };
        let vol = match t.get("volume_db").and_then(Value::as_f64) {
            Some(db) if db <= -70.0 => "-inf dB".to_string(),
            Some(db) => format!("{db:.1} dB"),
            None => num(f(t, "volume")),
        };
        let mut line = format!(
            "  {} {} [{}] {} · vol {vol}",
            i(t, "index"),
            s(t, "name"),
            flags.join(", "),
            devices
        );
        let pan = f(t, "panning");
        if pan.abs() > 0.005 {
            line.push_str(&format!(" pan {}", num(pan)));
        }
        let playing = i(t, "playing_slot_index");
        let fired = i(t, "fired_slot_index");
        if playing >= 0 {
            line.push_str(&format!(" · ▶ slot {playing}"));
        }
        if fired >= 0 && fired != playing {
            line.push_str(&format!(" · queued slot {fired}"));
        }
        if b(t, "is_recording") {
            line.push_str(" · recording");
        }
        let clips = arr(t, "clips");
        if clips.is_empty() {
            line.push_str(" · no clips");
        } else {
            let list: Vec<String> = clips
                .iter()
                .map(|c| {
                    format!(
                        "{} '{}' {}b",
                        i(c, "slot"),
                        s(c, "name"),
                        num(f(c, "length"))
                    )
                })
                .collect();
            line.push_str(&format!(" · clips: {}", list.join(", ")));
        }
        let no_stop = arr(t, "no_stop_slots");
        if !no_stop.is_empty() {
            line.push_str(" · keeps playing through scenes");
        }
        let arr_clips = i(t, "arrangement_clips");
        if arr_clips > 0 {
            line.push_str(&format!(" · {arr_clips} in the Arrangement"));
        }
        out.push_str(&line);
        out.push('\n');
    }

    let returns = arr(ctx, "returns");
    if returns.is_empty() {
        out.push_str("Returns: none");
    } else {
        let list: Vec<String> = returns
            .iter()
            .map(|r| {
                let devs: Vec<&str> = arr(r, "devices").iter().filter_map(Value::as_str).collect();
                if devs.is_empty() {
                    format!("{} {}", s(r, "letter"), s(r, "name"))
                } else {
                    format!("{} {} ({})", s(r, "letter"), s(r, "name"), devs.join(", "))
                }
            })
            .collect();
        out.push_str(&format!("Returns: {}", list.join(" · ")));
    }
    out.push_str(&format!(
        " · master {}\n",
        match session.get("master_volume_db").and_then(Value::as_f64) {
            Some(db) => format!("{db:.1} dB"),
            None => format!("vol {}", num(f(&session, "master_volume"))),
        }
    ));

    let scenes = arr(ctx, "scenes");
    let mut named: Vec<String> = Vec::new();
    let mut empty: Vec<i64> = Vec::new();
    let mut setlist: Option<&str> = None;
    let mut has_sections = false;
    let mut section_names: Vec<String> = Vec::new();
    for sc in scenes {
        if crate::song::is_setlist_scene(s(sc, "name")) {
            setlist = Some(s(sc, "name"));
            continue;
        }
        let (base, suffix) = crate::song::parse_section_name(s(sc, "name"));
        if suffix.is_some() {
            has_sections = true;
        }
        section_names.push(base);
        let count = sc
            .get("clip_count")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| arr(sc, "clip_tracks").len() as i64);
        if count == 0 {
            empty.push(i(sc, "index"));
            continue;
        }
        let name = if s(sc, "name").is_empty() {
            format!("scene {}", i(sc, "index"))
        } else {
            s(sc, "name").to_string()
        };
        let name = if b(sc, "is_playing") {
            format!("[{name}]")
        } else if b(sc, "is_triggered") {
            format!("({name})")
        } else {
            name
        };
        let mut item = format!("{} {name} ({count})", i(sc, "index"));
        if let Some(t) = sc.get("tempo").and_then(Value::as_f64).filter(|t| *t > 0.0) {
            item.push_str(&format!(" @{}", num(t)));
        }
        named.push(item);
    }
    out.push_str(if has_sections {
        "Sections (scenes): "
    } else {
        "Scenes: "
    });
    out.push_str(&named.join(" · "));
    if !empty.is_empty() {
        if !named.is_empty() {
            out.push_str(" · ");
        }
        out.push_str(&ranges(&empty));
        out.push_str(" empty");
    }
    out.push('\n');
    if let Some(name) = setlist {
        match crate::song::parse_setlist(name) {
            Ok(entries) if entries.is_empty() => {
                out.push_str("Song: none yet (the 'Setlist:' scene is empty; set_song)\n")
            }
            Ok(entries) => match crate::song::unknown_entry(&entries, &section_names) {
                None => out.push_str(&format!(
                    "Song: {} (from the 'Setlist:' scene; play_song, or jump_to a section)\n",
                    crate::song::setlist_text(&entries)
                )),
                Some(problem) => out.push_str(&format!(
                    "Song: the 'Setlist:' scene reads \"{}\"; {problem}. Fix the scene name in Live or set_song again.\n",
                    name.trim()
                )),
            },
            Err(e) => out.push_str(&format!(
                "Song: the 'Setlist:' scene reads \"{}\"; {e}. Fix the scene name in Live or set_song again.\n",
                name.trim()
            )),
        }
    }

    let cues = arr(ctx, "cues");
    match since {
        Some((bar, secs)) => out.push_str(&format!(
            "Performance: running since bar {bar} ({} s), {} cue{} pending",
            secs.round() as i64,
            cues.len(),
            if cues.len() == 1 { "" } else { "s" }
        )),
        None if !cues.is_empty() => out.push_str(&format!(
            "Performance: not started here, but {} cue{} pending in Live",
            cues.len(),
            if cues.len() == 1 { "" } else { "s" }
        )),
        None => out.push_str("Performance: not running (start_performance to go live)"),
    }
    if let Some(pr) = ctx.get("pending_record").filter(|v| !v.is_null()) {
        out.push_str(&format!(
            "; recording pending on track {} slot {}",
            i(pr, "track_index"),
            i(pr, "slot")
        ));
    }
    out.push('\n');

    if let Some(lib) = ctx.get("library") {
        let names = |k: &str| -> Vec<String> {
            arr(lib, k)
                .iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str).or_else(|| x.as_str()))
                .map(str::to_string)
                .collect()
        };
        let inst = names("instruments");
        out.push_str(&format!(
            "Library: {} instruments ({}) · {} audio effects · {} packs\n",
            inst.len(),
            inst.join(", "),
            names("audio_effects").len(),
            names("packs").len()
        ));
    } else if let Some(e) = ctx.get("library_error").and_then(Value::as_str) {
        out.push_str(&format!("Library: not read ({e})\n"));
    }

    let events = crate::performance::events_text(arr(ctx, "events"));
    if !events.is_empty() {
        out.push_str(&events);
        out.push('\n');
    }
    out.push_str(FOOTER);
    out
}

fn quant_words(name: &str) -> String {
    match name {
        "" => "?".into(),
        "1_bar" => "1 bar".into(),
        "2_bars" => "2 bars".into(),
        "4_bars" => "4 bars".into(),
        "8_bars" => "8 bars".into(),
        other => other.to_string(),
    }
}

/// [5, 6, 7, 9] → "5–7, 9"
fn ranges(idx: &[i64]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < idx.len() {
        let start = idx[i];
        let mut end = start;
        while i + 1 < idx.len() && idx[i + 1] == end + 1 {
            i += 1;
            end = idx[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}–{end}")
        });
        i += 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload() -> Value {
        json!({
            "live_version": "12.4.6", "script_version": "1.15.0",
            "session": {"tempo": 126.0, "signature_numerator": 4, "signature_denominator": 4, "is_playing": true,
                "bar": 14, "beat_in_bar": 2, "beat": 53.5, "clip_trigger_quantization": 4, "clip_trigger_quantization_name": "1_bar",
                "scale_mode": true, "scale_name": "Minor", "root_note_name": "F", "loop": false, "master_volume": 0.85},
            "tracks": [
                {"index": 0, "name": "Kick", "kind": "midi", "arm": false, "mute": false, "solo": false, "volume": 0.85, "panning": 0.0,
                 "devices": [{"name": "Rollin Breaks Kit", "type": "rack"}, {"name": "Drum Rack", "type": "drum_machine"}],
                 "clips": [{"slot": 0, "name": "Intro kick", "length": 4.0}, {"slot": 1, "name": "Groove kick", "length": 4.0}],
                 "arrangement_clips": 12, "playing_slot_index": 1, "fired_slot_index": -1, "no_stop_slots": []},
                {"index": 1, "name": "Lead", "kind": "midi", "arm": true, "mute": false, "solo": false, "volume": 0.7, "panning": -0.25,
                 "devices": [], "clips": [], "arrangement_clips": 0, "playing_slot_index": -1, "fired_slot_index": 2, "no_stop_slots": [0, 1]},
                {"index": 2, "name": "Capture", "kind": "audio", "arm": true, "mute": true, "solo": false, "volume": 0.85, "panning": 0.0,
                 "devices": [], "clips": [{"slot": 0, "name": "drop @ 128 | -0.9 dBFS", "length": 32.0}], "arrangement_clips": 0,
                 "playing_slot_index": -1, "fired_slot_index": -1, "no_stop_slots": []}
            ],
            "returns": [{"index": 0, "letter": "A", "name": "Reverb", "devices": ["Reverb"]}, {"index": 1, "letter": "B", "name": "Delay", "devices": []}],
            "scenes": [
                {"index": 0, "name": "Intro", "clip_count": 1}, {"index": 1, "name": "Groove", "clip_count": 2, "is_playing": true},
                {"index": 2, "name": "", "clip_count": 0}, {"index": 3, "name": "", "clip_count": 0}, {"index": 4, "name": "Drop", "clip_count": 3, "tempo": 134.0}
            ],
            "cues": [{"id": 1}], "pending_record": null, "events": []
        })
    }

    #[test]
    fn context_text_is_one_compact_readout() {
        let t = context_text(&payload(), Some((1, 26.0)));
        assert!(t.starts_with("Live 12.4.6 · script 1.15.0 · 126 BPM 4/4 · playing at bar 14.2 · launch quantization 1 bar · key F Minor\n"), "{t}");
        assert!(t.contains("Tracks (3):\n  0 Kick [midi] Rollin Breaks Kit › Drum Rack (drum rack) · vol 0.85 · ▶ slot 1 · clips: 0 'Intro kick' 4b, 1 'Groove kick' 4b · 12 in the Arrangement\n"), "{t}");
        assert!(t.contains("  1 Lead [midi, armed] no devices · vol 0.70 pan -0.25 · queued slot 2 · no clips · keeps playing through scenes\n"), "{t}");
        assert!(
            t.contains("  2 Capture [audio, armed, muted] no devices"),
            "{t}"
        );
        assert!(
            t.contains("Returns: A Reverb (Reverb) · B Delay · master vol 0.85\n"),
            "{t}"
        );
        assert!(
            t.contains("Scenes: 0 Intro (1) · 1 [Groove] (2) · 4 Drop (3) @134 · 2–3 empty\n"),
            "{t}"
        );
        assert!(
            t.contains("Performance: running since bar 1 (26 s), 1 cue pending\n"),
            "{t}"
        );
        assert!(t.ends_with(FOOTER), "{t}");
        assert!(t.len() < 1500, "{}", t.len());
    }

    #[test]
    fn sections_and_the_song_read_back_from_the_scene_names() {
        let mut p = payload();
        p["scenes"] = json!([
            {"index": 0, "name": "Intro · 8", "clip_count": 2}, {"index": 1, "name": "Groove · 8", "clip_count": 4, "is_playing": true},
            {"index": 2, "name": "Break · 16", "clip_count": 2},
            {"index": 3, "name": "Setlist: Intro×2 → Groove → Break", "clip_count": 0}
        ]);
        let t = context_text(&p, None);
        assert!(
            t.contains(
                "Sections (scenes): 0 Intro · 8 (2) · 1 [Groove · 8] (4) · 2 Break · 16 (2)\n"
            ),
            "{t}"
        );
        assert!(t.contains("Song: Intro×2 → Groove → Break (from the 'Setlist:' scene; play_song, or jump_to a section)\n"), "{t}");
        p["scenes"][3]["name"] = json!("Setlist: intro x2, grove x4");
        let t = context_text(&p, None);
        assert!(t.contains("Song: the 'Setlist:' scene reads \"Setlist: intro x2, grove x4\"; 'grove' is not a section (did you mean Groove?)."), "{t}");
        p["scenes"][3]["name"] = json!("Setlist: Intro → → Groove");
        let t = context_text(&p, None);
        assert!(t.contains("Song: the 'Setlist:' scene reads \"Setlist: Intro → → Groove\"; entry 2 of the setlist is empty"), "{t}");
        assert!(
            t.contains("Fix the scene name in Live or set_song again."),
            "{t}"
        );
    }

    #[test]
    fn context_text_without_a_performance_or_library() {
        let mut p = payload();
        p["cues"] = json!([]);
        p["session"]["is_playing"] = json!(false);
        p["library"] = json!({"instruments": [{"name": "Analog"}, {"name": "Drift"}], "audio_effects": [{"name": "Reverb"}], "packs": []});
        let t = context_text(&p, None);
        assert!(t.contains("stopped at bar 14.2"), "{t}");
        assert!(
            t.contains("Performance: not running (start_performance to go live)\n"),
            "{t}"
        );
        assert!(
            t.contains("Library: 2 instruments (Analog, Drift) · 1 audio effects · 0 packs\n"),
            "{t}"
        );
    }

    #[test]
    fn instructions_cover_the_four_jobs() {
        for word in [
            "get_context",
            "build_song",
            "capture_mix",
            "start_performance",
            "cue",
            "get_performance_state",
            "end_performance",
            "not made by Ableton",
            "make_section",
            "set_song",
            "play_song",
            "jump_to",
        ] {
            assert!(INSTRUCTIONS.contains(word), "{word}");
        }
        assert!(INSTRUCTIONS.len() < 5000);
        assert_eq!(ranges(&[5, 6, 7, 9]), "5–7, 9");
    }
}
