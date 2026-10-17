//! The Feedback screen's side of Claude listens: the song as a strip from
//! the listening memory, comments pinned to a bar, Claude's questions with
//! ▶ and This one, replies, and the away switch. Everything is the crate's
//! `feedback` and `listening` stores; this file only finds the open set and
//! the current bar.

use crate::listen::remember::MEMORY;
use mcp_ableton_music_maker::{feedback, listening};
use serde_json::{json, Value};

fn song() -> Result<(String, String), String> {
    MEMORY
        .song()
        .or_else(|| listening::newest_song().map(|k| (k, String::new())))
        .ok_or_else(|| "Live is not reachable, and nothing has been heard yet.".to_string())
}

/// Everything the screen draws, in one read.
pub fn state() -> Value {
    let (key, name) = match song() {
        Ok(s) => s,
        Err(e) => {
            return json!({"error": e, "away": feedback::away(), "away_on": feedback::away_on()})
        }
    };
    let position = MEMORY
        .position()
        .map(|(bar, section, playing)| json!({"bar": bar, "section": section, "playing": playing}));
    json!({
        "song_key": key,
        "set_name": name,
        "strip": listening::strip(&key),
        "screen": feedback::screen(&key),
        "position": position,
        "memory": MEMORY.status(),
        "app": format!("{:?}", listening::app_state()),
    })
}

/// A comment on `bar`, or on the bar playing now when none is given.
pub fn comment(text: &str, bar: Option<i64>) -> Result<Value, String> {
    let (key, _) = song()?;
    let (bar, section) = match bar {
        Some(b) => {
            let section = listening::takes(&key)
                .iter()
                .rev()
                .flat_map(|t| t.bars.iter())
                .find(|r| r.bar == b)
                .map(|r| r.section.clone())
                .unwrap_or_default();
            (Some(b), section)
        }
        None => match MEMORY.position() {
            Some((b, s, _)) => (Some(b), s),
            None => (None, String::new()),
        },
    };
    let c = feedback::add_comment(&key, bar, &section, text)?;
    Ok(serde_json::to_value(c).unwrap_or(Value::Null))
}

pub fn pick(offer: &str, option: usize, note: &str) -> Result<Value, String> {
    let (key, _) = song()?;
    let p = feedback::pick_option(&key, offer, option, note)?;
    Ok(serde_json::to_value(p).unwrap_or(Value::Null))
}

/// ▶ on an option: the producer's own click, so it may move the transport.
pub fn play(offer: &str, option: usize) -> Result<Value, String> {
    let (key, _) = song()?;
    let o = feedback::offers(&key)
        .into_iter()
        .find(|o| o.id == offer)
        .ok_or_else(|| format!("no question {offer}"))?;
    let item = o
        .options
        .get(option.wrapping_sub(1))
        .ok_or_else(|| format!("question {offer} has {} options", o.options.len()))?;
    let said = feedback::hear(&MEMORY.live(), &item.hear)?;
    Ok(json!({"playing": said}))
}

pub fn away(on: bool, minutes: u32) -> Result<Value, String> {
    let a = feedback::set_away(on, minutes, "producer")?;
    Ok(json!({"away": a, "away_on": feedback::away_on()}))
}
