//! Issue #45, against a real Live: the ten-track document that took Live
//! down twice, run through the real `build_song` body.
//!
//! ```bash
//! cargo run --example ten_track_build
//! ```
//!
//! It prints how long each stage took, the reply the producer would see,
//! and whether Live is still answering afterwards. A set with four default
//! tracks is what this expects; it converges, so running it twice is safe.

use mcp_ableton_music_maker::connection::{live_address, AbletonConnection, LiveState, RealBridge};
use mcp_ableton_music_maker::notes::NotesInput;
use mcp_ableton_music_maker::tools::{
    self, BuildSongParams, SongClip, SongLocator, SongPlacement, SongTrack,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

/// Seven MIDI tracks with instruments and three audio tracks, as in the
/// report. The instruments are searched for, so this exercises the browser
/// too — the load is what makes a track expensive.
fn document() -> BuildSongParams {
    let midi: [(&str, &str); 7] = [
        ("Kick", "kick"),
        ("Bass", "bass"),
        ("Keys", "piano"),
        ("Pad", "pad"),
        ("Lead", "lead"),
        ("Pluck", "pluck"),
        ("Arp", "arp"),
    ];
    let mut tracks: Vec<SongTrack> = midi
        .iter()
        .map(|(name, query)| SongTrack {
            name: (*name).into(),
            kind: "midi".into(),
            instrument: None,
            instrument_query: Some((*query).into()),
            volume: None,
            volume_db: Some(-6.0),
            fader: None,
            pan: None,
            color_index: None,
            sends: BTreeMap::new(),
        })
        .collect();
    for name in ["Vocal", "Texture", "FX"] {
        tracks.push(SongTrack {
            name: name.into(),
            kind: "audio".into(),
            instrument: None,
            instrument_query: None,
            volume: None,
            volume_db: Some(-8.0),
            fader: None,
            pan: None,
            color_index: None,
            sends: BTreeMap::new(),
        });
    }

    // Seven clips, ~264 notes between them.
    let bars: [(&str, &[(&str, &str)]); 7] = [
        ("Kick", &[("36", "x...x...x...x...")]),
        (
            "Bass",
            &[("38", "x.x.x.x.x.x.x.x."), ("41", "..x...x...x...x.")],
        ),
        (
            "Keys",
            &[("60", "x..x..x..x..x..."), ("64", "..x..x..x..x..x.")],
        ),
        ("Pad", &[("48", "x_______________")]),
        (
            "Lead",
            &[("72", "..x.x...x.x...x."), ("76", "x...x...x...x...")],
        ),
        ("Pluck", &[("67", "x.x.x.x.x.x.x.x.")]),
        ("Arp", &[("55", "xxxxxxxxxxxxxxxx")]),
    ];
    let clips: Vec<SongClip> = bars
        .iter()
        .map(|(track, rows)| {
            let mut steps = BTreeMap::new();
            for (pitch, pattern) in rows.iter() {
                steps.insert((*pitch).to_string(), (*pattern).to_string());
            }
            SongClip {
                track: (*track).into(),
                slot: Some(0),
                slots: vec![],
                name: format!("{track} 1"),
                length: 4.0,
                notes: NotesInput {
                    steps,
                    loop_every: 4.0,
                    until: 16.0,
                    ..Default::default()
                },
            }
        })
        .collect();

    // 46 placements across the seven MIDI tracks.
    let mut placements = Vec::new();
    for (i, (track, _)) in bars.iter().enumerate() {
        placements.push(SongPlacement {
            track: (*track).into(),
            slot: 0,
            times: vec![],
            start: Some(i as f64 * 16.0),
            end: Some(i as f64 * 16.0 + 96.0),
            step: Some(16.0),
        });
    }

    let locators: Vec<SongLocator> = ["Intro", "Verse", "Chorus", "Break", "Drop", "Outro"]
        .iter()
        .enumerate()
        .map(|(i, name)| SongLocator {
            name: (*name).into(),
            time: i as f64 * 32.0,
        })
        .collect();

    BuildSongParams {
        tempo: Some(86.0),
        key: Some("D minor".into()),
        scenes: vec![],
        tracks,
        clips,
        placements,
        locators,
        on_existing: "converge".into(),
        dry_run: false,
        snapshot: false,
    }
}

fn alive(host: &str, port: u16) -> Option<i64> {
    let c = AbletonConnection::new(host, port);
    let out = c
        .send_command("get_session_info", None)
        .ok()
        .and_then(|v| v.get("track_count").and_then(|t| t.as_i64()));
    c.disconnect();
    out
}

fn main() {
    let (host, port) = live_address();
    let Some(before) = alive(&host, port) else {
        eprintln!("FAIL: Live at {host}:{port} did not answer");
        std::process::exit(1);
    };
    let doc = document();
    println!(
        "Document: {} tracks, {} clips, {} placement groups, {} locators, tempo 86, D minor",
        doc.tracks.len(),
        doc.clips.len(),
        doc.placements.len(),
        doc.locators.len()
    );
    println!("Set before: {before} tracks\n");

    let live = Arc::new(LiveState::new(Arc::new(RealBridge::new(&host, port))));
    live.script.handshake(live.bridge.as_ref());

    let t0 = Instant::now();
    let result = tools::build_song_body(&live, &doc);
    let elapsed = t0.elapsed();

    println!("──────── the reply a producer would see ────────");
    match &result {
        Ok(text) => println!("{text}"),
        Err(text) => println!("{text}"),
    }
    println!("───────────────────────────────────────────────");
    println!("build_song took {:.1} s", elapsed.as_secs_f64());

    match alive(&host, port) {
        Some(after) => {
            println!("Live is still answering: {after} tracks (was {before})");
            if result.is_ok() {
                println!("\nPASS: the ten-track document built and Live survived it.");
            } else {
                println!(
                    "\nThe build stopped, but Live is alive and the reply above says what \
                     exists and how to finish it — which is the behaviour #45 asked for."
                );
            }
        }
        None => {
            eprintln!("Live stopped answering: this is the #45 crash, still happening.");
            std::process::exit(1);
        }
    }
}
