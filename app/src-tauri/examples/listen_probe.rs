//! The live check for the Listen feature: tap Ableton Live for a few seconds
//! and print what arrived. Run it with Live open and a set that makes a sound:
//!
//! ```bash
//! cd app/src-tauri && cargo run --example listen_probe -- [seconds]
//! ```
//!
//! It asks Live to play over the same socket the server uses, listens, and
//! puts the transport back the way it found it. Nothing is written anywhere;
//! the audio is measured in memory and dropped, exactly as the app does it.
//!
//! `--stamp` is release 0 of Claude listens: with a set that plays a hit on
//! every beat (a kick, or Live's metronome), it subscribes to the Remote
//! Script's clock the way Remember does, stamps every onset the tap hears
//! with the beat the clock says it is, and reports how far the hits land
//! from the beat. The median is the app's `clock_offset_ms`; the spread
//! around it says whether a bar stamp holds within ±50 ms. Jump, loop and
//! change the tempo while it runs to see whether they move it.
//!
//! ```bash
//! cd app/src-tauri && cargo run --example listen_probe -- 20 --stamp
//! ```

use ableton_music_maker_app_lib::listen::{analysis, analysis::Analyzer, tap};
use mcp_ableton_music_maker::connection::{live_address, AbletonConnection};
use mcp_ableton_music_maker::recorder::{ClockEvent, ClockTrack, SendTime};
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn transport(conn: &AbletonConnection) -> Option<bool> {
    conn.send_command("get_session_info", None)
        .ok()
        .and_then(|v| v.get("is_playing").and_then(Value::as_bool))
}

fn main() {
    let seconds: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4.0);

    println!("tap API available on this macOS: {}", tap::available());
    let baseline = tap::audio_device_count();
    println!("audio devices before starting: {baseline}");
    if !tap::available() {
        eprintln!("FAIL: this macOS has no process-tap API (needs 14.2+)");
        std::process::exit(1);
    }

    let Some((pid, name)) = tap::find_live() else {
        eprintln!("FAIL: Ableton Live is not running");
        std::process::exit(1);
    };
    println!("found {name} at pid {pid}");

    // Make sure there is something to hear, and remember what to put back.
    let (host, port) = live_address();
    let conn = AbletonConnection::new(host, port);
    let leave_transport = std::env::var("AMM_PROBE_NO_TRANSPORT").is_ok();
    let was_playing = transport(&conn);
    println!("Live transport was playing: {was_playing:?}");
    let started_playback = if was_playing == Some(false) && !leave_transport {
        let ok = conn.send_command("start_playback", None).is_ok();
        println!("asked Live to play: {ok}");
        std::thread::sleep(Duration::from_millis(600));
        ok
    } else {
        if leave_transport {
            println!("AMM_PROBE_NO_TRANSPORT set: Live's transport left alone");
        }
        false
    };

    if let Err(e) = tap::start(pid) {
        eprintln!("FAIL: {e}");
        restore(&conn, started_playback);
        std::process::exit(1);
    }
    let (rate, channels, buffer) = tap::format().unwrap_or((0.0, 0, 0));
    println!("tap format: {rate} Hz, {channels} channels, {buffer}-frame buffer");

    // A second reading of the same signal: Live's own master meter, over the
    // socket. It is reported raw, because Live's 0-1 output meter is its
    // fader scale and not a linear amplitude -- measured here, an 18 dB swing
    // in true level moved it from 0.57 to 0.76. So it tells us the tap is
    // hearing the same thing, not what level it is.
    let live_peak = Arc::new(AtomicI64::new(-12000));
    {
        let live_peak = live_peak.clone();
        let (host, port) = live_address();
        std::thread::spawn(move || {
            let meter_conn = AbletonConnection::new(host, port);
            let until = Instant::now() + Duration::from_secs_f64(seconds);
            while Instant::now() < until {
                if let Ok(v) = meter_conn.send_command("get_track_meters", None) {
                    let m = v
                        .get("master")
                        .map(|m| {
                            let l = m.get("left").and_then(Value::as_f64).unwrap_or(0.0);
                            let r = m.get("right").and_then(Value::as_f64).unwrap_or(0.0);
                            l.max(r)
                        })
                        .unwrap_or(0.0);
                    let db = if m > 0.0 { 20.0 * m.log10() } else { -120.0 };
                    let hundredths = (db * 100.0) as i64;
                    let _ = live_peak.fetch_max(hundredths, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            meter_conn.disconnect();
        });
    }

    let stamp = std::env::args().any(|a| a == "--stamp");
    let start = Instant::now();
    // The clock, as Remember reads it: its own socket, every tick.
    let clock = Arc::new(Mutex::new(ClockTrack::new()));
    if stamp {
        let clock = clock.clone();
        let (host, port) = live_address();
        std::thread::spawn(move || {
            let c = AbletonConnection::new(host, port);
            if c.subscribe(&["clock"], Some(0.0)).is_err() {
                eprintln!("WARN: could not subscribe to Live's clock");
                return;
            }
            let mut send = SendTime::default();
            while start.elapsed().as_secs_f64() < seconds + 1.0 {
                for ev in c.wait_events(Duration::from_millis(50)).unwrap_or_default() {
                    let at = send.sent(ev.get("t").and_then(Value::as_f64), start.elapsed().as_secs_f64());
                    if let Some(e) = ClockEvent::from_live(&ev, at) {
                        clock.lock().unwrap().push(e);
                    }
                }
            }
        });
    }
    let lag_s = if rate > 0.0 { buffer as f64 / rate } else { 0.0 };
    // Onsets: 5 ms steps of energy against the 50 ms before them.
    let step = ((rate * 0.005) as usize).max(1);
    let mut acc = (0.0f64, 0usize);
    let mut history: std::collections::VecDeque<f64> = std::collections::VecDeque::new();
    let mut quiet_until = 0.0f64;
    let mut offsets_ms: Vec<f64> = Vec::new();
    let mut analyzer = Analyzer::new(rate);
    let mut l = vec![0.0f32; 8192];
    let mut r = vec![0.0f32; 8192];
    let mut next = Duration::from_millis(250);
    let mut loudest_peak = -120.0f32;
    let mut frames = 0u64;

    while start.elapsed().as_secs_f64() < seconds {
        let now = start.elapsed();
        let now_ms = now.as_secs_f64() * 1000.0;
        let n = tap::drain(&mut l, &mut r);
        if n > 0 {
            analyzer.push(&l[..n], &r[..n], now_ms);
        }
        if stamp && n > 0 {
            let end_s = now.as_secs_f64() - lag_s;
            for i in 0..n {
                acc.0 += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
                acc.1 += 2;
                if acc.1 < step * 2 {
                    continue;
                }
                let e = acc.0 / acc.1 as f64;
                acc = (0.0, 0);
                let before = if history.is_empty() { 0.0 } else { history.iter().sum::<f64>() / history.len() as f64 };
                history.push_back(e);
                if history.len() > 10 {
                    history.pop_front();
                }
                let t = end_s - (n - i) as f64 / rate;
                let db = 10.0 * e.max(1e-12).log10();
                // A hit: 9 dB over the last 50 ms, above −40 dBFS, and not
                // inside the 100 ms after the last one.
                if db > -40.0 && e > before * 8.0 && t > quiet_until {
                    quiet_until = t + 0.1;
                    if let Some(p) = clock.lock().unwrap().at(t) {
                        if p.playing && p.beats_per_s > 0.0 {
                            let off_beats = p.beat - p.beat.round();
                            let ms = off_beats / p.beats_per_s * 1000.0;
                            if ms.abs() < 200.0 {
                                offsets_ms.push(ms);
                            }
                        }
                    }
                }
            }
        }
        if now >= next {
            next = now + Duration::from_millis(250);
            let f = analyzer.frame(now_ms);
            frames += 1;
            loudest_peak = loudest_peak.max(f.peak[0].max(f.peak[1]));
            let loudest_band = f
                .bands
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, v)| (analysis::band_edge(i), *v))
                .unwrap_or((0.0, -80.0));
            println!(
                "{:4.1}s  peak {:6.1} / {:6.1}  rms {:6.1}  corr {:+.2}  loudest {:6.0} Hz at {:6.1} dBFS  {}",
                now.as_secs_f64(),
                f.peak[0],
                f.peak[1],
                f.rms[0].max(f.rms[1]),
                f.correlation,
                loudest_band.0,
                loudest_band.1,
                if f.clip { "CLIP" } else { "" }
            );
        }
        std::thread::sleep(Duration::from_millis(4));
    }

    let seen = tap::frames_seen();
    let devices_while_listening = tap::audio_device_count();
    tap::stop();
    std::thread::sleep(Duration::from_millis(400));
    let devices_after = tap::audio_device_count();
    restore(&conn, started_playback);

    println!("---");
    println!("audio frames the tap delivered: {seen}");
    println!("analysis frames: {frames}");
    println!("loudest peak over the run: {loudest_peak:.1} dBFS");
    let live_db = live_peak.load(Ordering::Relaxed) as f32 / 100.0;
    println!(
        "Live's own master meter peaked at: {:.3} of full scale (its own curve, not dBFS)",
        10f32.powf(live_db / 20.0)
    );
    println!("audio devices while listening: {devices_while_listening}, after stopping: {devices_after} (baseline {baseline})");
    if seen == 0 {
        eprintln!(
            "FAIL: the tap delivered no audio at all. macOS is handing us silence: allow \
             \"Ableton Music Maker\" under System Settings › Privacy & Security › Screen & \
             System Audio Recording, or the binary is unsigned and was never offered the prompt."
        );
        std::process::exit(2);
    }
    let expected = (rate * seconds * 0.5) as u64;
    if seen < expected {
        eprintln!("WARN: only {seen} frames in {seconds}s at {rate} Hz — expected around {expected}");
    }
    if loudest_peak <= analysis::SILENCE_DB {
        eprintln!("WARN: the tap ran but Live was silent. Play something and run it again.");
        std::process::exit(3);
    }
    if devices_after != baseline {
        eprintln!(
            "FAIL: {} audio device(s) left behind after stopping — the aggregate device leaked.",
            devices_after as i64 - baseline as i64
        );
        std::process::exit(4);
    }
    if live_db <= -119.0 {
        eprintln!("WARN: Live's own meter read nothing, so the two paths were not cross-checked");
    } else {
        println!("cross-check: Live's own meter moved too, so both paths heard the same audio");
    }
    println!("PASS: the tap is receiving Live's output and leaves nothing behind.");
    if stamp {
        offsets_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if offsets_ms.len() < 8 {
            println!("stamp: only {} hits near a beat — play a set with a hit on every beat (or the metronome) and run longer.", offsets_ms.len());
            return;
        }
        let at = |p: f64| offsets_ms[((offsets_ms.len() - 1) as f64 * p).round() as usize];
        let median = at(0.5);
        let spread = offsets_ms.iter().map(|o| (o - median).abs()).fold(0.0, f64::max);
        println!(
            "stamp: {} hits; the tap's audio lands {median:+.1} ms from Live's beat (p10 {:+.1}, p90 {:+.1}); worst {spread:.1} ms from the median",
            offsets_ms.len(),
            at(0.1),
            at(0.9)
        );
        println!("stamp: set clock_offset_ms to {median:.0} in the app's settings.json; with it, ±50 ms {}.", if spread <= 50.0 { "holds" } else { "does NOT hold" });
    }
}

fn restore(conn: &AbletonConnection, started_playback: bool) {
    if started_playback {
        let _ = conn.send_command("stop_playback", None);
        println!("transport put back where it was");
    }
}
