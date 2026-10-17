//! What remembering costs: a minute of stereo audio at 48 kHz played through
//! the Mac app's path (`recorder::Remembering`: the stamper, a row per bar
//! measured like a capture, an RMS per 16th, thirty spectrum + chroma frames
//! a second, the take written to a scratch state dir), timed on this machine.
//!
//!   cargo run --release --example listen_cost
//!
//! The spec's bar is under 3 % of one core while remembering. This measures
//! the analysis and the writes; the process tap itself (Core Audio handing
//! the app its buffers) is outside it and is measured in the app.

use mcp_ableton_music_maker::listening;
use mcp_ableton_music_maker::recorder::sim::{Move, Playhead};
use mcp_ableton_music_maker::recorder::{FrameRow, Remembering, SetView, Take, TakeSink};
use std::time::Instant;

struct Sink;

impl TakeSink for Sink {
    fn set_info(&mut self) -> (String, String, SetView) {
        (
            "listen-cost".into(),
            "Listen cost".into(),
            SetView::default(),
        )
    }
    fn revision(&mut self, key: &str) -> u64 {
        listening::revision(key)
    }
    fn write(&mut self, take: &Take, frames: &[FrameRow]) {
        listening::write_take(take, frames).expect("write");
    }
}

fn main() {
    let dir = std::env::temp_dir().join(format!("amm-listen-cost-{}", std::process::id()));
    std::env::set_var("ABLETON_MCP_STATE_DIR", &dir);
    let rate = 48_000.0;
    let seconds = 60.0;
    let block = 512usize;
    let ph = Playhead::new(
        128.0,
        4.0,
        vec![Move::Play {
            at_s: 0.0,
            from_beat: 0.0,
        }],
        seconds + 2.0,
    );
    let events = ph.events(seconds + 2.0, 0.1, 0.01);
    // Audio is generated before the clock starts so only the recorder is timed.
    let n = (seconds * rate) as usize;
    let mut seed: u32 = 1;
    let (l, r): (Vec<f32>, Vec<f32>) = (0..n)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.1;
            let t = i as f32 / rate as f32;
            let tone = 0.3 * (2.0 * std::f32::consts::PI * 110.0 * t).sin();
            (tone + noise, tone - noise)
        })
        .unzip();
    let mut rem = Remembering::new(rate);
    let mut sink = Sink;
    let mut next = 0;
    let started = Instant::now();
    let mut at = 0usize;
    while at < n {
        let end = (at + block).min(n);
        let now = end as f64 / rate;
        while next < events.len() && events[next].at_s <= now {
            rem.clock_event(events[next].clone());
            next += 1;
        }
        rem.audio(&l[at..end], &r[at..end], now);
        rem.tick(now, &mut sink);
        at = end;
    }
    rem.finish(&mut sink);
    let spent = started.elapsed().as_secs_f64();
    let bytes: u64 = std::fs::read_dir(listening::song_dir("listen-cost").join("takes"))
        .map(|d| {
            d.flatten()
                .filter_map(|f| f.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    println!(
        "{seconds:.0} s of playback remembered in {:.3} s of one core: {:.2} % while playing; {:.1} KB a second written ({} bytes).",
        spent,
        spent / seconds * 100.0,
        bytes as f64 / seconds / 1000.0,
        bytes
    );
    let _ = std::fs::remove_dir_all(&dir);
}
