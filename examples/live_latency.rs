//! The numbers the streams story is decided by, measured against a running
//! Live. `scripts/live-latency.sh` is the front door.
//!
//! Prints the tick period the Remote Script reports (phase 0), the round-trip
//! cost with and without a touch of Live's API as p50 and p95 (phase 1's
//! before-and-after), and, for the phases that exist on the loaded script,
//! the event rate, the generic batch and cue lateness. A line whose phase is
//! not on the script says so instead of failing. Nothing in the set changes.

use mcp_ableton_music_maker::connection::{live_address, AbletonConnection};
use mcp_ableton_music_maker::handshake::ScriptInfoCache;
use mcp_ableton_music_maker::lom::{Batch, Op, Path};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn pct(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted_ms.len() - 1) as f64 * p).round() as usize;
    sorted_ms[i.min(sorted_ms.len() - 1)]
}

fn time_command(conn: &AbletonConnection, cmd: &str, params: Option<Value>, n: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        if conn.send_command(cmd, params.clone()).is_ok() {
            out.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

fn line(label: &str, body: String, phase: &str) {
    println!("{label:<24}{body:<44}← {phase}");
}

fn main() {
    let samples: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let (host, port) = live_address();
    let conn = AbletonConnection::new(host.clone(), port);
    let cache = ScriptInfoCache::default();
    let info = cache.handshake(&conn);
    let Some(version) = info.script_version.as_deref() else {
        eprintln!(
            "FAIL: Live at {host}:{port} did not answer the handshake ({})",
            info.error.unwrap_or_default()
        );
        std::process::exit(1);
    };
    let has = |c: &str| info.capabilities.iter().any(|x| x == c);
    println!(
        "Remote Script {version} · protocol {} · reader {}",
        info.protocol_version
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".into()),
        info.extra
            .get("socket_reader")
            .and_then(Value::as_str)
            .unwrap_or("? (script before 1.28)")
    );
    println!();

    // Phase 0: the tick period, as the script measured it.
    match info.extra.get("tick") {
        Some(t) if t.get("period_ms").and_then(Value::as_f64).is_some() => line(
            "tick period",
            format!(
                "{:.1} ms  ({} samples, jitter {:.1} ms, worst {:.1} ms)",
                t["period_ms"].as_f64().unwrap(),
                t["samples"].as_u64().unwrap_or(0),
                t["jitter_ms"].as_f64().unwrap_or(0.0),
                t["max_ms"].as_f64().unwrap_or(0.0)
            ),
            "phase 0",
        ),
        Some(t) => line("tick period", format!("no samples yet ({})", t), "phase 0"),
        None => line(
            "tick period",
            "not reported: reinstall the script and restart Live".into(),
            "phase 0",
        ),
    }

    // Phase 1's before-and-after: the round trip with and without Live's API.
    let no_api = time_command(&conn, "get_script_info", None, samples);
    line(
        "round trip, no API",
        format!(
            "p50 {:.1} ms  p95 {:.1} ms  (n={})",
            pct(&no_api, 0.5),
            pct(&no_api, 0.95),
            no_api.len()
        ),
        "phase 1",
    );
    let api = time_command(&conn, "get_session_info", None, samples);
    line(
        "round trip, with API",
        format!(
            "p50 {:.1} ms  p95 {:.1} ms  (n={})",
            pct(&api, 0.5),
            pct(&api, 0.95),
            api.len()
        ),
        "phase 1",
    );

    // Phase 3: the whole set as one native handler and as one generic batch.
    let t = Instant::now();
    let ctx = conn.send_command("get_context", Some(json!({"include_library": false})));
    let ctx_ms = t.elapsed().as_secs_f64() * 1000.0;
    fn leaves(v: &Value) -> usize {
        match v {
            Value::Object(m) => m.values().map(leaves).sum(),
            Value::Array(a) => a.iter().map(leaves).sum(),
            _ => 1,
        }
    }
    let n_values = ctx.as_ref().map(leaves).unwrap_or(0);
    line(
        "get_context, native",
        format!("{ctx_ms:.1} ms for {n_values} values, 1 round trip"),
        "phase 3",
    );
    if has("run") {
        let tracks = conn
            .send_command("get_session_info", None)
            .ok()
            .and_then(|v| v.get("track_count").and_then(Value::as_i64))
            .unwrap_or(0);
        let mut batch = Batch::new();
        for i in 0..tracks {
            let t = Path::track(i);
            for attr in ["name", "mute", "solo", "is_visible"] {
                batch = batch.push(Op::get(&t.clone().attr(attr), &format!("t{i}_{attr}")));
            }
            batch = batch.push(Op::get(&Path::track_volume(i), &format!("t{i}_vol")));
        }
        let ops = batch.len();
        let t = Instant::now();
        match conn.send_command("run", Some(batch.params())) {
            Ok(v) => {
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                line(
                    "run: a generic batch",
                    format!(
                        "{ms:.1} ms for {ops} ops, 1 round trip (main thread {} ms)",
                        v.get("main_ms").and_then(Value::as_f64).unwrap_or(0.0)
                    ),
                    "phase 3",
                );
            }
            Err(e) => line("run: a generic batch", format!("failed: {e}"), "phase 3"),
        }
    } else {
        line(
            "run: a generic batch",
            "not on this script".into(),
            "phase 3",
        );
    }

    // The clock and the cues only mean anything while the transport runs.
    // Start it if it is stopped and put it back after, the way listen_probe
    // does; --no-transport leaves the set entirely alone.
    let leave_alone = std::env::args().any(|a| a == "--no-transport");
    let was_playing = conn
        .send_command("get_session_info", None)
        .ok()
        .and_then(|v| v.get("is_playing").and_then(Value::as_bool))
        .unwrap_or(false);
    let started = if !was_playing && !leave_alone {
        let ok = conn.send_command("start_playback", None).is_ok();
        std::thread::sleep(Duration::from_millis(400));
        ok
    } else {
        false
    };

    if has("subscribe") {
        let _ = conn.subscribe(&["clock", "levels"], Some(50.0));
        let t = Instant::now();
        let (mut clocks, mut levels) = (0usize, 0usize);
        // Events share the socket with replies, so a cheap read pulls them.
        while t.elapsed() < Duration::from_secs(2) {
            let _ = conn.send_command("get_script_info", None);
            for e in conn.take_events() {
                match e.get("event").and_then(Value::as_str) {
                    Some("clock") => clocks += 1,
                    Some("levels") => levels += 1,
                    _ => {}
                }
            }
        }
        let secs = t.elapsed().as_secs_f64();
        let playing_now = conn
            .send_command("get_session_info", None)
            .ok()
            .and_then(|v| v.get("is_playing").and_then(Value::as_bool))
            .unwrap_or(false);
        line(
            "clock events / s",
            if playing_now {
                format!(
                    "{:.1}/s · levels {:.1}/s (asked 20/s; the tick is the floor)",
                    clocks as f64 / secs,
                    levels as f64 / secs
                )
            } else {
                "the transport is stopped, so the clock is quiet by design".into()
            },
            "phase 2",
        );
        let _ = conn.send_command("unsubscribe", Some(json!({})));
    } else {
        line("clock events / s", "not on this script".into(), "phase 2");
    }

    // Phase 4: how late a cue step fires, over harmless steps that only read.
    let playing = conn
        .send_command("get_session_info", None)
        .ok()
        .and_then(|v| v.get("is_playing").and_then(Value::as_bool))
        .unwrap_or(false);
    if has("run") && playing {
        let _ = conn.subscribe(&["cue"], None);
        let tempo = conn
            .send_command("get_session_info", None)
            .ok()
            .and_then(|v| v.get("tempo").and_then(Value::as_f64))
            .unwrap_or(120.0);
        let mut late: Vec<f64> = Vec::new();
        for _ in 0..12 {
            let now = conn
                .send_command("get_performance_state", None)
                .ok()
                .and_then(|v| v.get("beat").and_then(Value::as_f64))
                .unwrap_or(0.0);
            let at = (now + 2.0).ceil();
            let cue = json!({"cue": {"name": "latency", "steps": [
                {"beat": at, "ops": [{"op": "get", "path": "song.tempo", "as": "t"}]}]}});
            if conn.send_command("schedule_cue", Some(cue)).is_err() {
                break;
            }
            let wait = Duration::from_secs_f64((at - now + 1.0) * 60.0 / tempo);
            let deadline = Instant::now() + wait;
            let before = late.len();
            while Instant::now() < deadline && late.len() == before {
                let _ = conn.send_command("get_script_info", None);
                for e in conn.take_events() {
                    if e.get("event").and_then(Value::as_str) == Some("cue") {
                        if let Some(ms) = e.get("late_ms").and_then(Value::as_f64) {
                            late.push(ms);
                        }
                    }
                }
            }
        }
        late.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if late.is_empty() {
            line("cue lateness p50/p95", "no cue fired".into(), "phase 4");
        } else {
            line(
                "cue lateness p50/p95",
                format!(
                    "{:.1} / {:.1} ms over {} cues",
                    pct(&late, 0.5),
                    pct(&late, 0.95),
                    late.len()
                ),
                "phase 4",
            );
        }
        let _ = conn.send_command("unsubscribe", Some(json!({})));
    } else if has("run") {
        line(
            "cue lateness p50/p95",
            "transport stopped: press play".into(),
            "phase 4",
        );
    } else {
        line(
            "cue lateness p50/p95",
            "not on this script".into(),
            "phase 4",
        );
    }
    if started {
        let _ = conn.send_command("stop_playback", None);
        println!("\n  (the transport was stopped; it was started for the clock and put back)");
    }
    conn.disconnect();
}
