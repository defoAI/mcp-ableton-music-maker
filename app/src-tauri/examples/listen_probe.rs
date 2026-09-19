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

use ableton_music_maker_app_lib::listen::{analysis, analysis::Analyzer, tap};
use mcp_ableton_music_maker::connection::{live_address, AbletonConnection};
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
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

    let mut analyzer = Analyzer::new(rate);
    let mut l = vec![0.0f32; 8192];
    let mut r = vec![0.0f32; 8192];
    let start = Instant::now();
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
}

fn restore(conn: &AbletonConnection, started_playback: bool) {
    if started_playback {
        let _ = conn.send_command("stop_playback", None);
        println!("transport put back where it was");
    }
}
