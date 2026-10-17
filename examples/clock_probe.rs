//! Release 0 of Claude listens: does a bar stamp hold within ±50 ms of
//! Live's own position, across a jump, a loop and a tempo change?
//!
//!   cargo run --example clock_probe -- [--seconds 30] [--drive]
//!
//! Subscribes to the Remote Script's clock exactly as the Mac app does
//! (`clock` every tick, read with `wait_events`), builds the same playhead
//! (`recorder::ClockTrack`), and before each event arrives asks where the
//! playhead was predicted to be. The difference is how far a bar stamp
//! taken between ticks can be off. Jumps, loops and tempo changes the
//! clock reports are listed with where they were placed.
//!
//! Without `--drive` it only listens: play the set, jump, loop, change the
//! tempo, and read the report. With `--drive` it does those itself, in this
//! order — play from bar 1, jump to bar 9 at 4 s, tempo +8 BPM at 8 s, a
//! two-bar Arrangement loop at 12 s — then stops the transport and puts the
//! tempo and the loop back. Use a scratch set: `--drive` moves the transport.
//!
//! What it cannot see: the audio itself. How far the tap's audio trails
//! Live's reported position is the other half of the spike, measured with
//! the tap by `app/src-tauri/examples/listen_probe.rs --stamp`; the median it
//! prints is the app's `clock_offset_ms` setting.

use mcp_ableton_music_maker::connection::{live_address, AbletonConnection};
use mcp_ableton_music_maker::recorder::{ClockEvent, ClockTrack, SendTime};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds: f64 = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(30.0);
    let drive = args.iter().any(|a| a == "--drive");
    let (host, port) = live_address();
    let clock = AbletonConnection::new(host.clone(), port);
    let control = AbletonConnection::new(host, port);
    if let Err(e) = clock.subscribe(&["clock"], Some(0.0)) {
        eprintln!("Live is not reachable or the script has no clock channel: {e}");
        std::process::exit(1);
    }
    let session = control
        .send_command("get_session_info", None)
        .unwrap_or(Value::Null);
    let tempo0 = session
        .get("tempo")
        .and_then(Value::as_f64)
        .unwrap_or(120.0);
    let t0 = Instant::now();
    let mut track = ClockTrack::new();
    let mut send = SendTime::default();
    let mut errors_ms: Vec<f64> = Vec::new();
    let mut intervals_ms: Vec<f64> = Vec::new();
    let mut last_at: Option<f64> = None;
    let mut steps = vec![
        (0.0, "play", json!({"time": 0.0})),
        (4.0, "jump", json!({"time": 32.0})),
        (8.0, "tempo", json!({"tempo": tempo0 + 8.0})),
        (
            12.0,
            "loop",
            json!({"start": 32.0, "length": 8.0, "enabled": true}),
        ),
    ];
    if !drive {
        steps.clear();
    }
    println!(
        "Listening to Live's clock for {seconds:.0} s{}.",
        if drive { ", driving the transport" } else { "" }
    );
    while t0.elapsed().as_secs_f64() < seconds {
        let now = t0.elapsed().as_secs_f64();
        while let Some((at, what, params)) = steps.first().cloned() {
            if now < at {
                break;
            }
            steps.remove(0);
            let r = match what {
                "play" | "jump" => control.send_command("play_from", Some(params)),
                "tempo" => control.send_command("set_tempo", Some(params)),
                _ => control.send_command("set_arrangement_loop", Some(params)),
            };
            println!(
                "{now:6.2} s  {what}: {}",
                if r.is_ok() {
                    "sent".to_string()
                } else {
                    format!("{r:?}")
                }
            );
        }
        let events = clock
            .wait_events(Duration::from_millis(50))
            .unwrap_or_default();
        for ev in events {
            let received = t0.elapsed().as_secs_f64();
            let at = send.sent(ev.get("t").and_then(Value::as_f64), received);
            let Some(c) = ClockEvent::from_live(&ev, at) else {
                continue;
            };
            if let Some(prev) = last_at {
                intervals_ms.push((at - prev) * 1000.0);
            }
            last_at = Some(at);
            if let (Some(p), true) = (track.at(at), c.playing) {
                if p.playing {
                    let err = (c.beat - p.beat) * 60.0 / c.tempo.max(1.0) * 1000.0;
                    // A jump or loop is a discontinuity, not an error; it is
                    // reported below with where it was placed.
                    if err.abs() < 60.0 {
                        errors_ms.push(err.abs());
                    }
                }
            }
            track.push(c);
        }
        for m in track.take_markers() {
            println!(
                "{:6.2} s  {} — beat {:.2} → {:.2} at {:.1} BPM",
                m.at_s, m.kind, m.from_beat, m.to_beat, m.tempo
            );
        }
    }
    if drive {
        let _ = control.send_command("stop_playback", None);
        let _ = control.send_command("set_tempo", Some(json!({"tempo": tempo0})));
        let _ = control.send_command("set_arrangement_loop", Some(json!({"enabled": false})));
    }
    errors_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    intervals_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!();
    println!(
        "tick interval: p50 {:.1} ms · p95 {:.1} ms · max {:.1} ms ({} events)",
        pct(&intervals_ms, 0.5),
        pct(&intervals_ms, 0.95),
        intervals_ms.last().copied().unwrap_or(f64::NAN),
        intervals_ms.len() + 1
    );
    let worst = errors_ms.last().copied().unwrap_or(f64::NAN);
    println!(
        "position between ticks, predicted against reported: p50 {:.1} ms · p95 {:.1} ms · worst {:.1} ms",
        pct(&errors_ms, 0.5),
        pct(&errors_ms, 0.95),
        worst
    );
    println!(
        "{}",
        if errors_ms.is_empty() {
            "No playing ticks were seen: play the set while the probe listens, or pass --drive."
                .to_string()
        } else if worst <= 50.0 {
            "±50 ms holds on the clock side for this run.".to_string()
        } else {
            format!("±50 ms does NOT hold: the worst tick was {worst:.1} ms off the prediction.")
        }
    );
}
