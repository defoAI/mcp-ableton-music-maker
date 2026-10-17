//! The producer answers in the app: comments pinned to a bar, picks between
//! options Claude offered, replies in the same thread, and the away switch
//! ("Claude may play the set").
//!
//! Everything is a file under `state_dir()/feedback/`, so neither process
//! needs a socket to the other. Each file has one writer: the app writes
//! `comments.jsonl`, `picks.jsonl` and `away.json`; the server writes
//! `offers.jsonl`, `replies.jsonl` and `claude_playing.json`. Append-only
//! lines with ids, so a reader never needs a lock.

use crate::connection::LiveState;
use crate::listening::{self, Range};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

pub type ToolResult = Result<String, String>;

/// The away switch's default time limit.
pub const AWAY_MINUTES: u32 = 20;

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

fn song_dir(song_key: &str) -> PathBuf {
    crate::state::feedback_dir().join(
        listening::song_dir(song_key)
            .file_name()
            .unwrap_or_default(),
    )
}

fn read_lines<T: for<'de> Deserialize<'de>>(path: PathBuf) -> Vec<T> {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn append<T: Serialize>(path: PathBuf, item: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writeln!(
        f,
        "{}",
        serde_json::to_string(item).map_err(|e| e.to_string())?
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

// ── what is kept ────────────────────────────────────────────────────────────

/// A comment the producer pinned to a bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub at: String,
    pub at_ms: i64,
    #[serde(default)]
    pub bar: Option<i64>,
    #[serde(default)]
    pub section: String,
    pub text: String,
    /// The set's revision when it was written
    #[serde(default)]
    pub revision: u64,
}

/// Claude's answer in a comment's thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub at: String,
    pub at_ms: i64,
    pub comment: String,
    pub text: String,
}

/// How to hear an option: one of these.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct Hear {
    /// Bars in the Arrangement: 33, "33-40" or [33, 40]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<Value>,
    /// A locator by name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// A section (a scene row) by name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// A scene by name or index
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<Value>,
    /// A Session clip: {"track": "Bass", "slot": 3}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<Value>,
}

impl Hear {
    fn ways(&self) -> usize {
        [
            self.bars.is_some(),
            self.locator.is_some(),
            self.section.is_some(),
            self.scene.is_some(),
            self.clip.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count()
    }

    fn describe(&self) -> String {
        if let Some(b) = &self.bars {
            return match listening::parse_bars(b) {
                Ok((a, z)) if a == z => format!("bar {a}"),
                Ok((a, z)) => format!("bars {a}–{z}"),
                Err(_) => format!("bars {b}"),
            };
        }
        if let Some(l) = &self.locator {
            return format!("locator {l}");
        }
        if let Some(s) = &self.section {
            return format!("section {s}");
        }
        if let Some(s) = &self.scene {
            return format!(
                "scene {}",
                s.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| s.to_string())
            );
        }
        if let Some(c) = &self.clip {
            return format!(
                "clip {} slot {}",
                c.get("track")
                    .map(|t| t
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| t.to_string()))
                    .unwrap_or_default(),
                c.get("slot").map(|s| s.to_string()).unwrap_or_default()
            );
        }
        "—".into()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct OptionItem {
    /// A short name: "Breakdown C"
    pub name: String,
    /// One line on what it is
    #[serde(default)]
    pub what: String,
    /// How the producer hears it
    #[serde(default)]
    pub hear: Hear,
}

/// A choice Claude put in front of the producer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub at: String,
    pub at_ms: i64,
    pub question: String,
    pub options: Vec<OptionItem>,
}

/// The producer's pick: "This one".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pick {
    pub id: String,
    pub at: String,
    pub at_ms: i64,
    pub offer: String,
    /// 1-based, as the producer sees the rows
    pub option: usize,
    pub name: String,
    #[serde(default)]
    pub note: String,
}

/// The away switch: while on, Claude may play the set to measure it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Away {
    pub on: bool,
    pub since_ms: i64,
    pub until_ms: i64,
    /// Why it went off: "producer", "played", "time"
    #[serde(default)]
    pub off_reason: Option<String>,
    #[serde(default)]
    pub off_ms: Option<i64>,
}

// ── files ───────────────────────────────────────────────────────────────────

pub fn comments(song_key: &str) -> Vec<Comment> {
    read_lines(song_dir(song_key).join("comments.jsonl"))
}

pub fn replies(song_key: &str) -> Vec<Reply> {
    read_lines(song_dir(song_key).join("replies.jsonl"))
}

pub fn offers(song_key: &str) -> Vec<Offer> {
    read_lines(song_dir(song_key).join("offers.jsonl"))
}

pub fn picks(song_key: &str) -> Vec<Pick> {
    read_lines(song_dir(song_key).join("picks.jsonl"))
}

fn away_path() -> PathBuf {
    crate::state::feedback_dir().join("away.json")
}

fn playing_path() -> PathBuf {
    crate::state::feedback_dir().join("claude_playing.json")
}

pub fn away() -> Away {
    std::fs::read(away_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// On, and not past its time limit.
pub fn away_on() -> bool {
    let a = away();
    a.on && now_ms() < a.until_ms
}

fn write_json<T: Serialize>(path: PathBuf, v: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(v).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

// ── the app's side ──────────────────────────────────────────────────────────

/// The producer's switch. `minutes` is the time limit; off records why.
pub fn set_away(on: bool, minutes: u32, reason: &str) -> Result<Away, String> {
    let now = now_ms();
    let a = if on {
        Away {
            on: true,
            since_ms: now,
            until_ms: now + i64::from(minutes.clamp(1, 24 * 60)) * 60_000,
            off_reason: None,
            off_ms: None,
        }
    } else {
        let mut a = away();
        if !a.on {
            return Ok(a);
        }
        a.on = false;
        a.off_reason = Some(reason.to_string());
        a.off_ms = Some(now);
        a
    };
    write_json(away_path(), &a)?;
    Ok(a)
}

/// Claude is playing the set until `until_ms`: the app must not read a
/// transport start in that window as the producer pressing play.
pub fn claude_playing() -> bool {
    std::fs::read(playing_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("until_ms").and_then(Value::as_i64))
        .is_some_and(|u| now_ms() < u)
}

/// Called by the app when the transport starts or stops: a start or stop
/// that was not Claude's is the producer back at the controls, and the
/// switch goes off. A switch past its time limit is turned off here too.
pub fn transport_moved_by_someone() -> Option<Away> {
    let a = away();
    if !a.on {
        return None;
    }
    if now_ms() >= a.until_ms {
        return set_away(false, 0, "time").ok();
    }
    if claude_playing() {
        return None;
    }
    set_away(false, 0, "played").ok()
}

pub fn add_comment(
    song_key: &str,
    bar: Option<i64>,
    section: &str,
    text: &str,
) -> Result<Comment, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("a comment needs some text".into());
    }
    let now = now_ms();
    let c = Comment {
        id: format!("c-{now}"),
        at: crate::recorder::now_rfc3339(),
        at_ms: now,
        bar,
        section: section.to_string(),
        text: text.chars().take(2000).collect(),
        revision: listening::revision(song_key),
    };
    append(song_dir(song_key).join("comments.jsonl"), &c)?;
    Ok(c)
}

/// The producer picks option `option` (1-based) of `offer`.
pub fn pick_option(song_key: &str, offer: &str, option: usize, note: &str) -> Result<Pick, String> {
    let o = offers(song_key)
        .into_iter()
        .find(|o| o.id == offer)
        .ok_or_else(|| format!("no question {offer}"))?;
    let item = o
        .options
        .get(option.wrapping_sub(1))
        .ok_or_else(|| format!("question {offer} has {} options", o.options.len()))?;
    let now = now_ms();
    let p = Pick {
        id: format!("p-{now}"),
        at: crate::recorder::now_rfc3339(),
        at_ms: now,
        offer: offer.to_string(),
        option,
        name: item.name.clone(),
        note: note.trim().chars().take(2000).collect(),
    };
    append(song_dir(song_key).join("picks.jsonl"), &p)?;
    Ok(p)
}

/// Everything the Feedback screen shows for a song: threads, questions with
/// their picks, the away switch.
pub fn screen(song_key: &str) -> Value {
    let rs = replies(song_key);
    let ps = picks(song_key);
    let threads: Vec<Value> = comments(song_key)
        .into_iter()
        .rev()
        .map(|c| {
            let answers: Vec<&Reply> = rs.iter().filter(|r| r.comment == c.id).collect();
            json!({"comment": c, "replies": answers})
        })
        .collect();
    let questions: Vec<Value> = offers(song_key)
        .into_iter()
        .rev()
        .map(|o| {
            let picked = ps.iter().rfind(|p| p.offer == o.id);
            let rows: Vec<Value> = o
                .options
                .iter()
                .enumerate()
                .map(|(i, it)| json!({"n": i + 1, "name": it.name, "what": it.what, "hear": it.hear.describe()}))
                .collect();
            json!({"id": o.id, "question": o.question, "at": o.at, "options": rows, "picked": picked})
        })
        .collect();
    json!({
        "threads": threads,
        "questions": questions,
        "away": away(),
        "away_on": away_on(),
    })
}

// ── playing what an option names ────────────────────────────────────────────

/// Where a range starts and how many beats it lasts, in the Arrangement.
fn arrangement_span(live: &LiveState, hear: &Hear) -> Result<Option<(f64, f64)>, String> {
    let (_, bpb) = crate::tools::tempo_and_meter(live);
    if let Some(b) = &hear.bars {
        let (a, z) = listening::parse_bars(b)?;
        return Ok(Some(((a - 1) as f64 * bpb, (z - a + 1) as f64 * bpb)));
    }
    let name = match (&hear.locator, &hear.section) {
        (Some(l), _) => l.clone(),
        (None, Some(s)) => s.clone(),
        _ => return Ok(None),
    };
    let locs = crate::tools::locators(live)?;
    let Some(i) = locs
        .iter()
        .position(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
    else {
        return if hear.locator.is_some() {
            Err(format!("no locator named '{name}'"))
        } else {
            Ok(None)
        };
    };
    let start = locs[i].1;
    let next = locs
        .iter()
        .map(|(_, t)| *t)
        .filter(|t| *t > start + 1e-6)
        .fold(f64::INFINITY, f64::min);
    let len = if next.is_finite() {
        next - start
    } else {
        8.0 * bpb
    };
    Ok(Some((start, len)))
}

/// Start playing what an option names: the producer's own ▶, so it may move
/// the transport. Bars and locators play from their first beat; a section or
/// a scene fires its row; a clip fires its slot.
pub fn hear(live: &LiveState, hear: &Hear) -> Result<String, String> {
    if hear.ways() == 0 {
        return Err("this option says no way to hear it".into());
    }
    if let Some((start, _)) = arrangement_span(live, hear)? {
        crate::tools::require(live, "play_from")?;
        live.send_command("play_from", Some(json!({"time": start})))
            .map_err(|e| format!("could not play from beat {start}: {e}"))?;
        return Ok(format!(
            "Playing from bar {}",
            (start / crate::tools::tempo_and_meter(live).1).floor() as i64 + 1
        ));
    }
    if let Some(scene) = hear
        .scene
        .clone()
        .or_else(|| hear.section.clone().map(Value::String))
    {
        return crate::tools::run_named(live, "fire_scene", json!({"scene": scene}));
    }
    if let Some(c) = &hear.clip {
        return crate::tools::run_named(
            live,
            "fire_clip",
            json!({"track": c.get("track"), "clip": c.get("slot")}),
        );
    }
    Err("nothing to play".into())
}

/// Part D: Claude plays a range while the room is empty, and the listening
/// memory measures it like any other playback. Refused unless the producer
/// switched on "Claude may play the set" and the app is remembering; stops
/// at once if the producer takes the transport back or turns the switch off.
pub fn play_to_listen(live: &LiveState, range: &Range) -> Result<String, String> {
    if !away_on() {
        return Err("Claude may play the set is off, so nothing was played. Measuring by playback needs the producer's switch in the app (Feedback › Claude may play the set); until then, ask them to play it.".into());
    }
    if listening::app_state() != listening::AppState::Remembering {
        return Err(format!(
            "Nothing was played: {}",
            listening::nothing_heard(listening::app_state())
        ));
    }
    if crate::tools::performance_running(live).is_some() {
        return Err("A performance is running; listen {play} never moves its transport.".into());
    }
    let (tempo, bpb) = crate::tools::tempo_and_meter(live);
    let hear_as = match range {
        Range::Bars(a, z) => Hear {
            bars: Some(json!([a, z])),
            ..Hear::default()
        },
        Range::Section(s) => Hear {
            section: Some(s.clone()),
            ..Hear::default()
        },
        Range::Last => return Err("play needs a section or bars".into()),
    };
    let span = arrangement_span(live, &hear_as)?;
    let beats = match span {
        Some((_, len)) => len,
        None => {
            // A scene row: its phrase, from the name ("Drop · 8").
            let state = crate::tools::read_perf_state(live)?;
            let name = match range {
                Range::Section(s) => s.clone(),
                _ => String::new(),
            };
            let section = crate::song::sections(&state)
                .into_iter()
                .find(|s| s.name.eq_ignore_ascii_case(&name))
                .ok_or_else(|| format!("no section or locator named '{name}'"))?;
            section.bars.max(1) as f64 * bpb
        }
    };
    let seconds = beats * 60.0 / tempo.max(1.0);
    if seconds > 600.0 {
        return Err(format!(
            "that is {:.0} s of playback; play at most ten minutes at a time",
            seconds
        ));
    }
    let until = now_ms() + ((seconds + 3.0) * 1000.0) as i64;
    write_json(playing_path(), &json!({"until_ms": until}))?;
    let started = hear(live, &hear_as);
    if let Err(e) = started {
        let _ = std::fs::remove_file(playing_path());
        return Err(e);
    }
    // One bar's lead-in for a quantized launch, then the range.
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs_f64(seconds + 60.0 / tempo.max(1.0) * bpb * 0.5);
    let mut why = None;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if !away_on() {
            why = Some("the producer turned \"Claude may play the set\" off");
            break;
        }
        let playing = live
            .send_command("get_session_info", None)
            .ok()
            .and_then(|v| v.get("is_playing").and_then(Value::as_bool));
        if playing == Some(false) {
            why = Some("the transport was stopped in Live");
            break;
        }
    }
    let _ = live.send_command("stop_playback", None);
    let _ = std::fs::remove_file(playing_path());
    // Give the app a moment to close the take.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    match why {
        Some(w) => Err(format!("Claude's playback stopped at once: {w}. What played before that is in the listening memory.")),
        None => Ok(format!("Played {:.0} s while the room was empty.", seconds)),
    }
}

// ── the tools ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct OfferOptionsParams {
    /// The question, as the producer will read it: "Which breakdown?"
    pub question: String,
    /// Two to ten options, each with a name, one line on what it is, and how
    /// to hear it (bars, locator, section, scene or clip)
    pub options: Vec<OptionItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GetFeedbackParams {
    /// The cursor the last get_feedback returned: only what is newer
    #[serde(default)]
    pub since: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReplyParams {
    /// The comment's id (from get_feedback)
    pub comment: String,
    /// The answer, as the producer will read it in the thread
    pub text: String,
}

pub fn offer_options_body(live: &LiveState, p: &OfferOptionsParams) -> ToolResult {
    let question = p.question.trim();
    if question.is_empty() {
        return Err("question is the line the producer reads above the options".into());
    }
    if !(2..=10).contains(&p.options.len()) {
        return Err(format!("give two to ten options, not {}", p.options.len()));
    }
    for (i, o) in p.options.iter().enumerate() {
        if o.name.trim().is_empty() {
            return Err(format!("option {} has no name", i + 1));
        }
        match o.hear.ways() {
            1 => {}
            0 => {
                return Err(format!(
                "option {} ({}) needs one way to hear it: bars, locator, section, scene or clip",
                i + 1,
                o.name
            ))
            }
            _ => {
                return Err(format!(
                    "option {} ({}) gives more than one way to hear it; give one",
                    i + 1,
                    o.name
                ))
            }
        }
        if let Some(b) = &o.hear.bars {
            listening::parse_bars(b).map_err(|e| format!("option {}: {e}", i + 1))?;
        }
    }
    let (key, _) = listening::song_key(live);
    let now = now_ms();
    let offer = Offer {
        id: format!("o-{now}"),
        at: crate::recorder::now_rfc3339(),
        at_ms: now,
        question: question.to_string(),
        options: p.options.clone(),
    };
    append(song_dir(&key).join("offers.jsonl"), &offer)?;
    let rows: Vec<String> = offer
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            format!(
                "  {}. {} — {} (▶ {})",
                i + 1,
                o.name,
                o.what,
                o.hear.describe()
            )
        })
        .collect();
    let app = match listening::app_state() {
        listening::AppState::NotRunning => " The Mac app is not running, so the producer cannot see it there yet: ask them to open it, or ask the question in the chat.",
        _ => "",
    };
    Ok(format!(
        "Question {} is in the app's Feedback screen: \"{question}\"\n{}\nEach row has ▶ and This one. The pick arrives on get_feedback.{app}",
        offer.id,
        rows.join("\n")
    ))
}

fn cursor_of(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

pub fn get_feedback_body(live: &LiveState, p: &GetFeedbackParams) -> ToolResult {
    let (key, note) = listening::song_key(live);
    Ok(feedback_text(
        &key,
        cursor_of(p.since.as_ref()),
        note.as_deref(),
    ))
}

pub fn feedback_text(song_key: &str, since: i64, note: Option<&str>) -> String {
    let cs: Vec<Comment> = comments(song_key)
        .into_iter()
        .filter(|c| c.at_ms > since)
        .collect();
    let all_replies = replies(song_key);
    let os = offers(song_key);
    let ps: Vec<Pick> = picks(song_key)
        .into_iter()
        .filter(|p| p.at_ms > since)
        .collect();
    let mut out = String::new();
    if let Some(n) = note {
        out.push_str(n);
        out.push('\n');
    }
    let mut newest = since;
    if cs.is_empty() {
        out.push_str("No new comments.\n");
    } else {
        out.push_str("Comments:\n");
        for c in &cs {
            newest = newest.max(c.at_ms);
            let replied = all_replies.iter().any(|r| r.comment == c.id);
            out.push_str(&format!(
                "- {} {}{} at {}: \"{}\"{}\n",
                c.id,
                c.bar
                    .map(|b| format!("bar {b}"))
                    .unwrap_or_else(|| "no bar".into()),
                if c.section.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", c.section)
                },
                listening::when(&c.at),
                c.text,
                if replied { " — answered" } else { "" }
            ));
        }
    }
    if !ps.is_empty() {
        out.push_str("Picks:\n");
        for p in &ps {
            newest = newest.max(p.at_ms);
            let q = os
                .iter()
                .find(|o| o.id == p.offer)
                .map(|o| o.question.as_str())
                .unwrap_or("?");
            out.push_str(&format!(
                "- {} \"{q}\": option {} \"{}\"{} at {}\n",
                p.offer,
                p.option,
                p.name,
                if p.note.is_empty() {
                    String::new()
                } else {
                    format!(" — note: \"{}\"", p.note)
                },
                listening::when(&p.at)
            ));
        }
    }
    let all_picks = picks(song_key);
    let open: Vec<&Offer> = os
        .iter()
        .filter(|o| !all_picks.iter().any(|p| p.offer == o.id))
        .collect();
    if !open.is_empty() {
        out.push_str(&format!(
            "Waiting for a pick: {}.\n",
            open.iter()
                .map(|o| format!("{} \"{}\"", o.id, o.question))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let a = away();
    out.push_str(&if away_on() {
        format!(
            "Claude may play the set: on until {}. Measuring by playback is allowed (listen {{play: true}}).\n",
            chrono::DateTime::from_timestamp_millis(a.until_ms)
                .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_default()
        )
    } else {
        format!(
            "Claude may play the set: off{}. Do not start the transport to measure.\n",
            match a.off_reason.as_deref() {
                Some("played") => " (the producer pressed play)",
                Some("time") => " (its time limit ran out)",
                _ => "",
            }
        )
    });
    if listening::app_state() == listening::AppState::NotRunning {
        out.push_str("The Mac app is not running, so new comments and picks cannot arrive until it is open.\n");
    }
    out.push_str(&format!("cursor: {}", newest.max(since)));
    out
}

pub fn reply_body(live: &LiveState, p: &ReplyParams) -> ToolResult {
    let (key, _) = listening::song_key(live);
    reply_to(&key, &p.comment, &p.text)
}

pub fn reply_to(song_key: &str, comment: &str, text: &str) -> ToolResult {
    let text = text.trim();
    if text.is_empty() {
        return Err("a reply needs some text".into());
    }
    let c = comments(song_key)
        .into_iter()
        .find(|c| c.id == comment.trim())
        .ok_or_else(|| format!("no comment {comment}; get_feedback lists them with their ids"))?;
    let now = now_ms();
    let r = Reply {
        id: format!("r-{now}"),
        at: crate::recorder::now_rfc3339(),
        at_ms: now,
        comment: c.id.clone(),
        text: text.chars().take(2000).collect(),
    };
    append(song_dir(song_key).join("replies.jsonl"), &r)?;
    Ok(format!(
        "Answered {} ({}\"{}\") in the app's Feedback screen.",
        c.id,
        c.bar.map(|b| format!("bar {b}: ")).unwrap_or_default(),
        c.text
    ))
}
