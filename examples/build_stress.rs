//! Issue #45, the stress case: try to take Live down with track creation.
//!
//! ```bash
//! cargo run --example build_stress -- --tracks 20 --rounds 3
//! cargo run --example build_stress -- --tracks 20 --rounds 2 --play
//! cargo run --example build_stress -- --cleanup          # delete what it made
//! ```
//!
//! Each round builds a fresh set of tracks (named by round, so `converge`
//! cannot short-circuit the work), then reports what it cost Live: the
//! worst main-thread slice from Live's own log, resident memory, and
//! whether Live is still answering. It stops the moment Live goes.

use mcp_ableton_music_maker::connection::{live_address, AbletonConnection, LiveState, RealBridge};
use mcp_ableton_music_maker::notes::NotesInput;
use mcp_ableton_music_maker::tools::{self, BuildSongParams, SongClip, SongPlacement, SongTrack};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

const QUERIES: [&str; 7] = ["kick", "bass", "piano", "pad", "lead", "pluck", "arp"];

fn document(round: usize, tracks: usize) -> BuildSongParams {
    let mut t = Vec::new();
    for i in 0..tracks {
        let midi = i % 4 != 3; // three MIDI with a device, then one audio
        t.push(SongTrack {
            name: format!("R{round}-{i}"),
            kind: if midi { "midi".into() } else { "audio".into() },
            instrument: None,
            instrument_query: midi.then(|| QUERIES[i % QUERIES.len()].to_string()),
            volume: None,
            volume_db: Some(-9.0),
            fader: None,
            pan: None,
            color_index: None,
            sends: BTreeMap::new(),
        });
    }
    // A clip and a row of placements on every MIDI track, so the round does
    // the whole document's worth of work, not just the tracks.
    let mut clips = Vec::new();
    let mut placements = Vec::new();
    for (i, track) in t.iter().enumerate() {
        if track.kind != "midi" {
            continue;
        }
        let mut steps = BTreeMap::new();
        steps.insert(
            (36 + (i % 24)).to_string(),
            "x...x...x...x...".repeat(2).to_string(),
        );
        clips.push(SongClip {
            track: track.name.clone(),
            slot: Some(0),
            slots: vec![],
            name: format!("{} clip", track.name),
            length: 8.0,
            notes: NotesInput {
                steps,
                loop_every: 8.0,
                until: 32.0,
                ..Default::default()
            },
        });
        placements.push(SongPlacement {
            track: track.name.clone(),
            slot: 0,
            times: vec![],
            start: Some(0.0),
            end: Some(128.0),
            step: Some(32.0),
        });
    }
    BuildSongParams {
        tempo: Some(86.0),
        key: None,
        scenes: vec![],
        tracks: t,
        clips,
        placements,
        locators: vec![],
        on_existing: "converge".into(),
        dry_run: false,
    }
}

/// Live's own log, read from `at` onwards: the worst slice and last memory.
fn live_cost(at: u64) -> (u64, String) {
    let Some(home) = std::env::var_os("HOME") else {
        return (0, "?".into());
    };
    let mut path = std::path::PathBuf::from(home);
    path.push("Library/Preferences/Ableton");
    let log = std::fs::read_dir(&path)
        .ok()
        .and_then(|entries| {
            entries
                .flatten()
                .map(|e| e.path().join("Log.txt"))
                .find(|p| p.exists())
        })
        .unwrap_or_default();
    let Ok(text) = std::fs::read_to_string(&log) else {
        return (0, "?".into());
    };
    let mut worst = 0u64;
    let mut memory = String::from("?");
    for line in text.lines().skip(at as usize) {
        if let Some(rest) = line.split("held Live's main thread ").nth(1) {
            if let Some(ms) = rest.split(' ').next().and_then(|n| n.parse::<u64>().ok()) {
                worst = worst.max(ms);
            }
        }
        if let Some(rest) = line.split("MemoryUsage: ").nth(1) {
            memory = rest.trim().to_string();
        }
    }
    (worst, memory)
}

fn log_lines() -> u64 {
    let Some(home) = std::env::var_os("HOME") else {
        return 0;
    };
    let mut path = std::path::PathBuf::from(home);
    path.push("Library/Preferences/Ableton");
    let log = std::fs::read_dir(&path)
        .ok()
        .and_then(|e| {
            e.flatten()
                .map(|e| e.path().join("Log.txt"))
                .find(|p| p.exists())
        })
        .unwrap_or_default();
    std::fs::read_to_string(log)
        .map(|t| t.lines().count() as u64)
        .unwrap_or(0)
}

fn session(host: &str, port: u16) -> Option<(i64, bool)> {
    let c = AbletonConnection::new(host, port);
    let out = c.send_command("get_session_info", None).ok().map(|v| {
        (
            v.get("track_count").and_then(|t| t.as_i64()).unwrap_or(-1),
            v.get("is_playing")
                .and_then(|t| t.as_bool())
                .unwrap_or(false),
        )
    });
    c.disconnect();
    out
}

fn arg(name: &str, default: usize) -> usize {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn cleanup(live: &LiveState) {
    println!("Deleting every R<n>-<i> track this harness made.\n");
    loop {
        let Ok(info) = live.send_command("get_session_info", None) else {
            break;
        };
        let count = info
            .get("track_count")
            .and_then(|t| t.as_i64())
            .unwrap_or(0);
        let mut ops = Vec::new();
        for i in 0..count {
            ops.push(serde_json::json!({
                "op": "get", "path": format!("song.tracks[{i}].name"), "as": format!("n{i}")
            }));
        }
        let Ok(names) = live.send_command("run", Some(serde_json::json!({"ops": ops}))) else {
            break;
        };
        let victim = (0..count).find(|i| {
            names
                .get(format!("n{i}"))
                .and_then(|v| v.as_str())
                .is_some_and(|n| n.starts_with('R') && n.contains('-'))
        });
        match victim {
            Some(i) => {
                let t0 = Instant::now();
                match live.send_command("delete_track", Some(serde_json::json!({"track_index": i})))
                {
                    Ok(_) => println!("  deleted track {i} in {} ms", t0.elapsed().as_millis()),
                    Err(e) => {
                        println!("  delete_track {i} failed: {e}");
                        return;
                    }
                }
            }
            None => break,
        }
    }
    println!("\nClean.");
}

fn main() {
    let (host, port) = live_address();
    let live = Arc::new(LiveState::new(Arc::new(RealBridge::new(&host, port))));
    live.script.handshake(live.bridge.as_ref());
    if live.script.get().and_then(|i| i.script_version).is_none() {
        eprintln!("FAIL: Live at {host}:{port} did not answer");
        std::process::exit(1);
    }
    if std::env::args().any(|a| a == "--cleanup") {
        cleanup(&live);
        return;
    }

    let tracks = arg("--tracks", 20);
    let rounds = arg("--rounds", 3);
    let play = std::env::args().any(|a| a == "--play");
    println!(
        "Stress: {rounds} rounds of {tracks} tracks each, transport {}.\n",
        if play { "PLAYING" } else { "stopped" }
    );

    if play {
        let _ = live.send_command("start_playback", None);
    }

    for round in 1..=rounds {
        let Some((before, playing)) = session(&host, port) else {
            eprintln!("Live was gone before round {round}");
            std::process::exit(1);
        };
        let mark = log_lines();
        let t0 = Instant::now();
        let result = tools::build_song_body(&live, &document(round, tracks));
        let secs = t0.elapsed().as_secs_f64();
        let (worst, memory) = live_cost(mark);

        let after = session(&host, port);
        let made = after.map(|(n, _)| n - before).unwrap_or(-1);
        println!(
            "round {round}: {} in {secs:5.1}s | +{made} tracks | worst slice {worst:5} ms | playing {playing} | {memory}",
            if result.is_ok() { "built " } else { "STOPPED" }
        );
        if let Err(text) = &result {
            let last = text.lines().last().unwrap_or("");
            println!("          {}", &last[..last.len().min(150)]);
        }
        if after.is_none() {
            eprintln!("\nLIVE WENT AWAY in round {round}. This is #45, still reproducing.");
            std::process::exit(1);
        }
    }

    if play {
        let _ = live.send_command("stop_playback", None);
    }
    println!("\nPASS: Live answered after every round. Run with --cleanup to remove the tracks.");
}
