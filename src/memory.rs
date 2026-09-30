//! The song's memory: what Claude knew that Live cannot hold.
//!
//! A producer's session ends and everything not readable from the set goes
//! with it — that the Sitar is the lead, that the Smoke Bass is theirs and
//! must never be regenerated, why the sitar ended where it did, and the
//! biggest loss of all, **the plan**. "The Drop is thin, the next move is the
//! answer phrase an octave up" is the most valuable sentence of the afternoon
//! and the one most certain to be gone tomorrow.
//!
//! **What Live can hold, Live holds** — measured, not assumed. A `describe`
//! sweep of a real Live 12.4.6 (`tests/fixtures/live-lom-12.4.6.json`,
//! 2026-09-20, script 1.34.2) shows the only writable text in Live's object
//! model is a `name`. There is no info text, no comment field, no data store.
//! So a track's **role** lives in its name (`Sitar [lead]`) and a parked idea
//! lives as a clip in a `Stash:` scene row — both travel in the `.als` and
//! survive with no server at all. Only what has nowhere to go in Live is
//! here: the overview, the notes and the per-session digest.
//!
//! **Retrieval is not a call.** The whole overview rides back in the
//! `get_context` header, the call the agent makes first anyway. A "load my
//! memory" call is one an agent can fail to make, and the turn it skips it on
//! is the first turn of a session — exactly the turn that needed it.
//!
//! Privacy: this is the most content-bearing thing the server writes —
//! prose about unreleased music, written by the agent rather than typed by
//! the producer. On by default is defensible only because it is local,
//! capped, inspectable in one call and deletable in one call, and named in
//! `TERMS.md`. `ABLETON_MCP_SONG_MEMORY=false` stops every write.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// The overview's ceiling. Over it the write is refused and nothing is
/// stored: an overview that silently lost its plan is worse than one that
/// said it was too big.
pub const OVERVIEW_CAP: usize = 8 * 1024;

/// The keys the header renders first, in this order. Anything else the agent
/// writes is kept verbatim and rendered after them — the shape is a
/// suggestion, not a schema.
pub const KNOWN_KEYS: &[&str] = &["what_it_is", "plan", "tracks", "decided", "next"];

/// The key a set that has never been saved is filed under. One Live has one
/// set open, so one name is enough; the first Cmd+S renames the file.
pub const PROVISIONAL_KEY: &str = "unsaved-set";

/// `ABLETON_MCP_SONG_MEMORY` — anything but "false"/"0"/"off"/"no" keeps it on.
pub fn enabled() -> bool {
    !matches!(
        crate::env_str("ABLETON_MCP_SONG_MEMORY")
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "off" | "no"
    )
}

// ── What is stored ──────────────────────────────────────────────────────────

/// What Live said about the set when the overview was last written. The
/// server writes it; an agent's value is ignored, because the whole point is
/// to be able to say later that the set has moved and the overview has not.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AsOf {
    #[serde(default)]
    pub tempo: f64,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub sections: usize,
    #[serde(default)]
    pub tracks: usize,
    #[serde(default)]
    pub at: String,
}

/// One thing said about the song, a track or a section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// `"song"`, a track name, or `"section:<name>"`.
    pub about: String,
    pub note: String,
    pub at: String,
}

/// What one session did, written whether or not anything was remembered on
/// purpose — so a session that never called `remember` still leaves a trace.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Digest {
    pub at: String,
    #[serde(default)]
    pub tracks: Vec<String>,
    #[serde(default)]
    pub sections: Vec<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, u64>,
}

impl Digest {
    /// "2026-09-20 — 14 calls, tracks Sitar, Smoke Bass; sections Drop".
    pub fn line(&self) -> String {
        let calls: u64 = self.tools.values().sum();
        let mut out = format!(
            "{} — {calls} call{}",
            self.at,
            if calls == 1 { "" } else { "s" }
        );
        if !self.tracks.is_empty() {
            out.push_str(&format!("; tracks {}", self.tracks.join(", ")));
        }
        if !self.sections.is_empty() {
            out.push_str(&format!("; sections {}", self.sections.join(", ")));
        }
        out
    }
}

/// One song's file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SongMemory {
    /// The file's own name, derived from the set's path.
    pub key: String,
    /// The set this belongs to. **The only path this file ever holds.**
    #[serde(default)]
    pub set_path: String,
    /// What to call it: the `.als` file's stem.
    #[serde(default)]
    pub set_name: String,
    /// True while the set has never been saved and the key is provisional.
    #[serde(default)]
    pub provisional: bool,
    #[serde(default)]
    pub overview: Map<String, Value>,
    #[serde(default)]
    pub as_of: AsOf,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub sessions: Vec<Digest>,
    #[serde(default)]
    pub updated_at: String,
}

impl SongMemory {
    pub fn is_empty(&self) -> bool {
        self.overview.is_empty() && self.notes.is_empty() && self.sessions.is_empty()
    }

    /// The most recent note, for the header.
    pub fn last_note(&self) -> Option<&Note> {
        self.notes.last()
    }

    /// Every note about one subject, oldest first.
    pub fn notes_about(&self, about: &str) -> Vec<&Note> {
        let want = about.trim().to_lowercase();
        self.notes
            .iter()
            .filter(|n| n.about.to_lowercase() == want)
            .collect()
    }
}

// ── Identity ────────────────────────────────────────────────────────────────

/// The file name for a set, from the path Live gave. Two sets called
/// `Smoke.als` in different folders are different songs, so the folder is in
/// the key — as a short digest, not as a readable path.
pub fn key_for(set_path: &str) -> String {
    let path = set_path.trim();
    if path.is_empty() {
        return PROVISIONAL_KEY.to_string();
    }
    let stem = set_name_of(path);
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let safe: String = safe
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let safe = if safe.is_empty() {
        "set".to_string()
    } else {
        safe.chars().take(48).collect()
    };
    format!("{safe}-{}", digest_of(path))
}

/// What to call the set: the `.als` file's stem.
pub fn set_name_of(set_path: &str) -> String {
    std::path::Path::new(set_path.trim())
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// A short, stable digest of the whole path. FNV-1a: this is a file name, not
/// a secret, and it must be the same on every run.
fn digest_of(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:08x}")
}

// ── The overview ────────────────────────────────────────────────────────────

/// Merge a patch into the overview, or rewrite it. Returns the keys that
/// actually changed, so the reply says what moved rather than "ok".
pub fn merge(
    into: &mut Map<String, Value>,
    patch: &Map<String, Value>,
    replace: bool,
) -> Vec<String> {
    if replace {
        let mut changed: Vec<String> = into
            .keys()
            .filter(|k| !patch.contains_key(*k))
            .cloned()
            .collect();
        for (k, v) in patch {
            if into.get(k) != Some(v) {
                changed.push(k.clone());
            }
        }
        *into = patch.clone();
        changed.sort();
        changed.dedup();
        return changed;
    }
    let mut changed = Vec::new();
    for (k, v) in patch {
        if into.get(k) != Some(v) {
            changed.push(k.clone());
        }
        into.insert(k.clone(), v.clone());
    }
    // Sorted, so the reply reads the same way twice for the same patch.
    changed.sort();
    changed
}

/// How big the overview is once written, and which key is the largest — the
/// question the producer actually has when a write is refused.
pub fn size_of(overview: &Map<String, Value>) -> (usize, String, usize) {
    let total = serde_json::to_string(overview)
        .map(|s| s.len())
        .unwrap_or(0);
    let (name, size) = overview
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                serde_json::to_string(v).map(|s| s.len()).unwrap_or(0),
            )
        })
        .max_by_key(|(_, n)| *n)
        .unwrap_or_default();
    (total, name, size)
}

/// `Err` when the overview would not fit. Nothing is written on an `Err`.
pub fn check_cap(overview: &Map<String, Value>) -> Result<usize, String> {
    let (total, largest, largest_size) = size_of(overview);
    if total <= OVERVIEW_CAP {
        return Ok(total);
    }
    Err(format!(
        "the overview would be {total} bytes and the cap is {OVERVIEW_CAP}; nothing was written. \
         The largest key is '{largest}' at {largest_size} bytes — shorten it, or move the detail \
         into a note with remember(about, note)."
    ))
}

/// The overview as the header shows it: the known keys in their own order,
/// then everything else the agent chose to keep, verbatim.
pub fn render_overview(overview: &Map<String, Value>) -> String {
    if overview.is_empty() {
        return String::new();
    }
    let mut ordered = Map::new();
    for k in KNOWN_KEYS {
        if let Some(v) = overview.get(*k) {
            ordered.insert((*k).to_string(), v.clone());
        }
    }
    for (k, v) in overview {
        if !KNOWN_KEYS.contains(&k.as_str()) {
            ordered.insert(k.clone(), v.clone());
        }
    }
    serde_json::to_string_pretty(&Value::Object(ordered)).unwrap_or_default()
}

/// One line instead of the whole thing, for the second `get_context` of a
/// session and every one after it.
pub fn overview_digest(memory: &SongMemory) -> String {
    format!(
        "Overview: {} (last changed {}). It is rendered in full on the first get_context of a \
         session and after any change; remember(overview: …) to add to it.",
        memory
            .overview
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        if memory.updated_at.is_empty() {
            "—"
        } else {
            &memory.updated_at
        }
    )
}

/// Where the set has moved since the overview was written. The set is the
/// truth; the overview is what somebody thought last time.
pub fn drift(as_of: &AsOf, now: &AsOf) -> Vec<String> {
    let mut out = Vec::new();
    if as_of.at.is_empty() {
        return out;
    }
    if (as_of.tempo - now.tempo).abs() > 0.01 && as_of.tempo > 0.0 {
        out.push(format!("tempo {} → {}", num(as_of.tempo), num(now.tempo)));
    }
    if !as_of.key.is_empty() && as_of.key != now.key && !now.key.is_empty() {
        out.push(format!("key {} → {}", as_of.key, now.key));
    }
    if as_of.sections != now.sections {
        out.push(format!("sections {} → {}", as_of.sections, now.sections));
    }
    if as_of.tracks != now.tracks {
        out.push(format!("tracks {} → {}", as_of.tracks, now.tracks));
    }
    out
}

/// The one line that travels: it rides on `get_context`, and also on
/// `capture_mix` and `play_song`, because `get_context` is called once per
/// session and a set the producer nudges mid-session would otherwise go
/// unnoticed.
pub fn drift_line(as_of: &AsOf, now: &AsOf) -> Option<String> {
    let moved = drift(as_of, now);
    if moved.is_empty() {
        return None;
    }
    Some(format!(
        "The set has moved since the overview was written ({}): {}. The set is the truth; \
         remember(overview: …) to bring it up to date.",
        as_of.at,
        moved.join(", ")
    ))
}

fn num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

// ── Roles, in the track name ────────────────────────────────────────────────

/// `"Sitar [lead]"` → `("Sitar", Some("lead"))`.
pub fn split_role(track_name: &str) -> (String, Option<String>) {
    let text = track_name.trim();
    if let Some(open) = text.rfind(" [") {
        if text.ends_with(']') {
            let role = &text[open + 2..text.len() - 1];
            if !role.trim().is_empty() && !role.contains('[') {
                return (
                    text[..open].trim().to_string(),
                    Some(role.trim().to_string()),
                );
            }
        }
    }
    (text.to_string(), None)
}

/// `("Sitar", "lead")` → `"Sitar [lead]"`. A second role **replaces** the
/// suffix rather than stacking on it.
pub fn with_role(track_name: &str, role: &str) -> String {
    let (base, _) = split_role(track_name);
    let role = role.trim();
    if role.is_empty() {
        return base;
    }
    format!("{base} [{role}]")
}

/// Every role in the set, as `(role, track name)`.
pub fn roles(track_names: &[String]) -> Vec<(String, String)> {
    track_names
        .iter()
        .filter_map(|n| split_role(n).1.map(|r| (r, n.clone())))
        .collect()
}

// ── The header ──────────────────────────────────────────────────────────────

/// What rides back on `get_context`. Empty when there is nothing to say —
/// a header that always appears is a header nobody reads.
///
/// `full` renders the whole overview (the first call of a session, and after
/// any change); otherwise one line. That rule is the server's: the agent
/// decides nothing, and a repeat call stops spending the cap on something
/// unchanged.
pub struct Header<'a> {
    pub memory: Option<&'a SongMemory>,
    pub full: bool,
    pub now: &'a AsOf,
    pub roles: Vec<(String, String)>,
    pub stashed: usize,
    /// Set once, when a provisional file was renamed to the set's own key.
    pub renamed: Option<String>,
}

pub fn header_text(h: &Header) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(m) = h.memory {
        if !m.is_empty() || !m.set_name.is_empty() {
            let mut first = format!(
                "Song: {}",
                if m.set_name.is_empty() {
                    "this set (not saved yet, so the notes are provisional)"
                } else {
                    &m.set_name
                }
            );
            first.push_str(&format!(
                " · {} section{}",
                h.now.sections,
                if h.now.sections == 1 { "" } else { "s" }
            ));
            if let Some(last) = m.sessions.last() {
                first.push_str(&format!(" · last worked on {}", last.at));
            }
            lines.push(first);
        }
    }
    if !h.roles.is_empty() {
        lines.push(format!(
            "Roles: {}",
            h.roles
                .iter()
                .map(|(r, t)| format!("{r} = {t}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(note) = h.memory.and_then(SongMemory::last_note) {
        lines.push(format!(
            "Last note ({}, {}): {}",
            note.about, note.at, note.note
        ));
    }
    if h.stashed > 0 {
        lines.push(format!(
            "Stash: {} idea{} parked — stash(action: \"list\") shows them.",
            h.stashed,
            if h.stashed == 1 { "" } else { "s" }
        ));
    }
    if let Some(m) = h.memory {
        if !m.overview.is_empty() {
            if h.full {
                lines.push(format!("Overview:\n{}", render_overview(&m.overview)));
            } else {
                lines.push(overview_digest(m));
            }
        }
        if let Some(said) = drift_line(&m.as_of, h.now) {
            lines.push(said);
        }
    }
    if let Some(said) = &h.renamed {
        lines.push(said.clone());
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("{}\n", lines.join("\n"))
}

// ── Reconciliation ──────────────────────────────────────────────────────────

/// A note whose track no longer exists, and what became of it. **Never
/// repointed by guess**: where it is not certain the note is left unattached
/// and the reply says so.
#[derive(Debug, Clone, PartialEq)]
pub enum Reattached {
    /// One track was renamed and nothing else claims the note.
    To { from: String, to: String },
    /// Several tracks could be meant, or none: the note keeps its old
    /// subject and the producer is told.
    Unattached { about: String, why: String },
}

impl Reattached {
    pub fn line(&self) -> String {
        match self {
            Reattached::To { from, to } => {
                format!("The notes about '{from}' now read as '{to}' (one track, renamed).")
            }
            Reattached::Unattached { about, why } => format!(
                "The notes about '{about}' are not attached to a track any more: {why}. \
                 Nothing was repointed — say which track they belong to and they will be."
            ),
        }
    }
}

/// Match notes whose subject is gone against the tracks that are there.
///
/// The one case that is certain is a **rename**: exactly one track, itself
/// unspoken for, whose name still contains the missing subject or is
/// contained by it — "Sitar" becoming "Sitar Lead". Anything else is a
/// guess, and a guess here silently moves the producer's words onto the
/// wrong part of their song, so it is reported and left alone. A set is full
/// of tracks nobody wrote a note about; "the only one without notes" would
/// not be evidence of anything.
pub fn reconcile(subjects: &[String], track_names: &[String]) -> Vec<Reattached> {
    let base_of = |n: &String| split_role(n).0.to_lowercase();
    let present = |want: &str| {
        track_names
            .iter()
            .any(|n| base_of(n) == want || n.to_lowercase() == want)
    };
    let missing: Vec<&String> = subjects
        .iter()
        .filter(|s| !present(&s.to_lowercase()))
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    // Tracks nobody already speaks for: a track that has its own notes is
    // not a candidate for somebody else's.
    let unspoken: Vec<&String> = track_names
        .iter()
        .filter(|n| !subjects.iter().any(|s| s.to_lowercase() == base_of(n)))
        .collect();
    let mut out = Vec::new();
    for about in missing {
        let want = about.to_lowercase();
        let like: Vec<&&String> = unspoken
            .iter()
            .filter(|n| {
                let base = base_of(n);
                base.contains(&want) || want.contains(&base)
            })
            .collect();
        match like.len() {
            1 => out.push(Reattached::To {
                from: about.clone(),
                to: split_role(like[0]).0,
            }),
            _ => out.push(Reattached::Unattached {
                about: about.clone(),
                why: if like.is_empty() {
                    format!(
                        "no track in the set is named anything like it ({})",
                        if unspoken.is_empty() {
                            "every track already has notes of its own".to_string()
                        } else {
                            unspoken
                                .iter()
                                .map(|n| n.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    )
                } else {
                    format!(
                        "{} tracks could be meant ({})",
                        like.len(),
                        like.iter()
                            .map(|n| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
            }),
        }
    }
    out
}

// ── On disk ─────────────────────────────────────────────────────────────────

pub fn file_for(key: &str) -> PathBuf {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    crate::state::songs_dir().join(format!("{safe}.json"))
}

pub fn load(key: &str) -> Option<SongMemory> {
    let bytes = std::fs::read(file_for(key)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether this song already has a memory of its own on disk. A set that
/// has one is not waiting to be told what it is, so the provisional file is
/// left alone rather than merged into it.
pub fn exists(key: &str) -> bool {
    file_for(key).is_file()
}

/// Write it, unless the memory is switched off. Every write goes through
/// here, so one check is the whole off switch.
pub fn save(memory: &SongMemory) {
    if !enabled() {
        return;
    }
    let path = file_for(&memory.key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec_pretty(memory) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!("could not write the song memory {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("could not serialise the song memory: {e}"),
    }
}

/// Every song file on this machine, newest first — what `adv_song_memory
/// list` shows.
pub fn all() -> Vec<SongMemory> {
    let mut out: Vec<SongMemory> = std::fs::read_dir(crate::state::songs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<SongMemory>(&b).ok())
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

// ── What the server process holds ───────────────────────────────────────────

/// The song memory this server process has open, plus the two things that
/// are session-scoped: whether the overview has been rendered in full yet,
/// and the digest being accumulated.
#[derive(Default)]
pub struct Songs {
    open: Mutex<Option<SongMemory>>,
    shown: Mutex<bool>,
    digest: Mutex<Digest>,
    /// Said once, on the `get_context` after a provisional file was renamed.
    pub renamed: Mutex<Option<String>>,
}

impl Songs {
    pub fn snapshot(&self) -> Option<SongMemory> {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Open the memory for this key, loading it from disk. Switching to a
    /// different set switches the notes and starts the overview rendering
    /// over, because it is a different song.
    pub fn open(&self, key: &str, set_path: &str) -> SongMemory {
        let mut guard = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = guard.as_ref() {
            if m.key == key {
                return m.clone();
            }
        }
        *self.shown.lock().unwrap_or_else(|e| e.into_inner()) = false;
        let memory = load(key).unwrap_or(SongMemory {
            key: key.to_string(),
            set_path: set_path.to_string(),
            set_name: set_name_of(set_path),
            provisional: key == PROVISIONAL_KEY,
            ..Default::default()
        });
        *guard = Some(memory.clone());
        memory
    }

    /// Make this memory the open one, so `rename_to` can carry it to the
    /// song's own key. Used when the provisional file was written by an
    /// earlier session and this process has nothing open yet.
    pub fn adopt(&self, memory: SongMemory) {
        *self.shown.lock().unwrap_or_else(|e| e.into_inner()) = false;
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = Some(memory);
    }

    /// Change it and write it. The closure gets the open memory; a `false`
    /// return means nothing changed and nothing is written.
    ///
    /// A change here is a change to what the header *renders*, so the whole
    /// overview goes out again on the next `get_context`.
    pub fn update(&self, f: impl FnOnce(&mut SongMemory) -> bool) -> Option<SongMemory> {
        let changed = self.write(f);
        if changed.is_some() {
            *self.shown.lock().unwrap_or_else(|e| e.into_inner()) = false;
        }
        changed
    }

    /// The same, without saying the header changed — for the session digest,
    /// which is written on every `get_context` and is not a reason to spend
    /// the cap rendering the overview again.
    fn write(&self, f: impl FnOnce(&mut SongMemory) -> bool) -> Option<SongMemory> {
        let mut guard = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let memory = guard.as_mut()?;
        if !f(memory) {
            return None;
        }
        memory.updated_at = now();
        save(memory);
        Some(memory.clone())
    }

    /// Whether this `get_context` renders the whole overview. True the first
    /// time in a session and after any change; one line after that.
    pub fn take_full(&self) -> bool {
        let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        let full = !*shown;
        *shown = true;
        full
    }

    /// Note a tool call for this session's digest. Cheap, and it is what
    /// makes a session that never called `remember` still leave a trace.
    pub fn note_call(&self, tool: &str) {
        let mut d = self.digest.lock().unwrap_or_else(|e| e.into_inner());
        if d.at.is_empty() {
            d.at = today();
        }
        *d.tools.entry(tool.to_string()).or_insert(0) += 1;
    }

    pub fn note_track(&self, name: &str) {
        let name = split_role(name).0;
        if name.trim().is_empty() {
            return;
        }
        let mut d = self.digest.lock().unwrap_or_else(|e| e.into_inner());
        if !d.tracks.iter().any(|t| t == &name) && d.tracks.len() < 32 {
            d.tracks.push(name);
        }
    }

    pub fn note_section(&self, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let mut d = self.digest.lock().unwrap_or_else(|e| e.into_inner());
        if !d.sections.iter().any(|s| s == &name) && d.sections.len() < 32 {
            d.sections.push(name);
        }
    }

    pub fn digest(&self) -> Digest {
        self.digest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Fold this session's digest into the open file. One row per day, so a
    /// long afternoon is one line rather than a hundred.
    pub fn flush_digest(&self) {
        let digest = self.digest();
        if digest.tools.is_empty() {
            return;
        }
        self.write(|m| {
            match m.sessions.iter_mut().find(|s| s.at == digest.at) {
                Some(row) => *row = digest.clone(),
                None => m.sessions.push(digest.clone()),
            }
            while m.sessions.len() > 30 {
                m.sessions.remove(0);
            }
            true
        });
    }

    /// Forget this song: the file goes, and nothing in Live is touched.
    pub fn forget(&self) -> String {
        let mut guard = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let Some(memory) = guard.take() else {
            return "No song memory is open, so there was nothing to forget.".to_string();
        };
        let path = file_for(&memory.key);
        let removed = std::fs::remove_file(&path).is_ok();
        format!(
            "Forgot the memory of '{}': {} note{}, {} overview key{}, {} session{}. {} \
             Nothing in Live changed — the roles are in your track names and the stash is a scene \
             row in your set, and both are yours to keep or rename.",
            if memory.set_name.is_empty() {
                "this set"
            } else {
                &memory.set_name
            },
            memory.notes.len(),
            if memory.notes.len() == 1 { "" } else { "s" },
            memory.overview.len(),
            if memory.overview.len() == 1 { "" } else { "s" },
            memory.sessions.len(),
            if memory.sessions.len() == 1 { "" } else { "s" },
            if removed {
                format!("{} deleted.", path.display())
            } else {
                "There was no file to delete.".to_string()
            }
        )
    }

    /// Start the session's memory over: the digest, and the rule that the
    /// overview renders in full on the first `get_context`. `reset_set`
    /// calls it, because what follows is a new song in the same window.
    pub fn start_over(&self) {
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.shown.lock().unwrap_or_else(|e| e.into_inner()) = false;
        *self.digest.lock().unwrap_or_else(|e| e.into_inner()) = Digest::default();
        *self.renamed.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Point the open memory at a new key, carrying what was written against
    /// the provisional one. Returns the sentence to say once.
    pub fn rename_to(&self, key: &str, set_path: &str) -> Option<String> {
        let mut guard = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let memory = guard.as_mut()?;
        if memory.key == key {
            return None;
        }
        let was = memory.key.clone();
        let old_file = file_for(&was);
        memory.key = key.to_string();
        memory.set_path = set_path.to_string();
        memory.set_name = set_name_of(set_path);
        memory.provisional = false;
        memory.updated_at = now();
        save(memory);
        let _ = std::fs::remove_file(old_file);
        Some(format!(
            "This set has been saved as '{}', so the notes written against it before it had a \
             name now belong to that song and will come back with it.",
            memory.set_name
        ))
    }
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn a_patch_merges_and_replace_rewrites() {
        let mut overview = map(json!({"what_it_is": "a dub track", "next": "the answer phrase"}));
        let changed = merge(&mut overview, &map(json!({"next": "the bridge"})), false);
        assert_eq!(changed, vec!["next"]);
        assert_eq!(
            overview["what_it_is"], "a dub track",
            "the rest is untouched"
        );
        // Writing the same value back is not a change.
        assert!(merge(&mut overview, &map(json!({"next": "the bridge"})), false).is_empty());

        let mut changed = merge(&mut overview, &map(json!({"plan": "8 bars"})), true);
        changed.sort();
        assert_eq!(changed, vec!["next", "plan", "what_it_is"]);
        assert_eq!(overview.keys().collect::<Vec<_>>(), vec!["plan"]);
    }

    #[test]
    fn the_cap_refuses_and_names_the_largest_key() {
        let mut overview = map(json!({"plan": "short"}));
        overview.insert("tracks".into(), json!("x".repeat(OVERVIEW_CAP)));
        let err = check_cap(&overview).expect_err("over the cap");
        assert!(err.contains("the cap is 8192"), "{err}");
        assert!(err.contains("The largest key is 'tracks'"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
        assert!(check_cap(&map(json!({"plan": "short"}))).is_ok());
    }

    #[test]
    fn known_keys_render_first_and_unknown_ones_survive_verbatim() {
        let overview = map(json!({
            "mixdown_notes": "the sub is 3 dB hot",
            "next": "the answer phrase an octave up",
            "what_it_is": "a dub track at 140"
        }));
        let text = render_overview(&overview);
        let at = |k: &str| text.find(k).unwrap_or(usize::MAX);
        assert!(at("what_it_is") < at("next"), "{text}");
        assert!(at("next") < at("mixdown_notes"), "unknown keys come after");
        assert!(
            text.contains("the sub is 3 dB hot"),
            "kept verbatim: {text}"
        );
    }

    #[test]
    fn drift_names_what_moved_and_says_nothing_when_it_did_not() {
        let as_of = AsOf {
            tempo: 140.0,
            key: "D Dorian".into(),
            sections: 5,
            tracks: 7,
            at: "2026-09-20 16:04".into(),
        };
        assert!(drift(&as_of, &as_of).is_empty());
        assert!(drift_line(&as_of, &as_of).is_none());
        let now = AsOf {
            tempo: 142.0,
            sections: 6,
            ..as_of.clone()
        };
        let said = drift_line(&as_of, &now).expect("it moved");
        assert!(said.contains("tempo 140 → 142"), "{said}");
        assert!(said.contains("sections 5 → 6"), "{said}");
        assert!(said.contains("The set is the truth"), "{said}");
    }

    /// An `as_of` that was never written is not a drift of everything from
    /// zero.
    #[test]
    fn an_overview_with_no_as_of_reports_no_drift() {
        let now = AsOf {
            tempo: 140.0,
            sections: 5,
            ..Default::default()
        };
        assert!(drift(&AsOf::default(), &now).is_empty());
    }

    #[test]
    fn a_role_lives_in_the_track_name_and_a_second_one_replaces_it() {
        assert_eq!(with_role("Sitar", "lead"), "Sitar [lead]");
        assert_eq!(
            with_role("Sitar [lead]", "counter"),
            "Sitar [counter]",
            "a second role replaces the suffix, never stacks"
        );
        assert_eq!(
            split_role("Sitar [lead]"),
            ("Sitar".into(), Some("lead".into()))
        );
        assert_eq!(split_role("Sitar"), ("Sitar".into(), None));
        // Not a role: a name that merely ends in a bracket.
        assert_eq!(split_role("Bass [] "), ("Bass []".into(), None));
        assert_eq!(with_role("Sitar [lead]", ""), "Sitar");
        assert_eq!(
            roles(&["Sitar [lead]".into(), "Sub".into()]),
            vec![("lead".to_string(), "Sitar [lead]".to_string())]
        );
    }

    #[test]
    fn two_sets_of_the_same_name_in_different_folders_are_different_songs() {
        let a = key_for("/Users/p/Music/A/Smoke.als");
        let b = key_for("/Users/p/Music/B/Smoke.als");
        assert_ne!(a, b);
        assert!(a.starts_with("smoke-"), "{a}");
        assert_eq!(
            a,
            key_for("/Users/p/Music/A/Smoke.als"),
            "stable across runs"
        );
        assert_eq!(key_for(""), PROVISIONAL_KEY);
        assert_eq!(set_name_of("/Users/p/Music/A/Smoke.als"), "Smoke");
    }

    #[test]
    fn one_renamed_track_is_reattached_and_anything_less_certain_is_not() {
        // One subject gone, one track nobody speaks for: certain.
        let moved = reconcile(
            &["Sitar".into(), "Sub".into()],
            &["Sub".into(), "Sitar Lead [lead]".into()],
        );
        assert_eq!(
            moved,
            vec![Reattached::To {
                from: "Sitar".into(),
                to: "Sitar Lead".into()
            }]
        );
        assert!(moved[0].line().contains("now read as 'Sitar Lead'"));

        // Two candidates: never a guess.
        let moved = reconcile(&["Sitar".into()], &["Bells".into(), "Texture".into()]);
        assert_eq!(moved.len(), 1);
        let line = moved[0].line();
        assert!(line.contains("not attached to a track any more"), "{line}");
        assert!(line.contains("Bells, Texture"), "{line}");
        assert!(line.contains("Nothing was repointed"), "{line}");

        // A role does not make a track a stranger to its own notes.
        assert!(reconcile(&["Sitar".into()], &["Sitar [lead]".into()]).is_empty());
    }

    #[test]
    fn the_header_says_nothing_when_there_is_nothing_to_say() {
        let now = AsOf::default();
        assert_eq!(
            header_text(&Header {
                memory: None,
                full: true,
                now: &now,
                roles: vec![],
                stashed: 0,
                renamed: None,
            }),
            ""
        );
    }

    #[test]
    fn the_header_carries_the_song_the_roles_the_last_note_and_the_stash() {
        let memory = SongMemory {
            key: "smoke-1".into(),
            set_name: "Smoke".into(),
            overview: map(json!({"next": "the answer phrase an octave up"})),
            notes: vec![Note {
                about: "Smoke Bass".into(),
                note: "mine, never regenerate".into(),
                at: "2026-09-20".into(),
            }],
            sessions: vec![Digest {
                at: "2026-09-20".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let now = AsOf {
            sections: 5,
            ..Default::default()
        };
        let text = header_text(&Header {
            memory: Some(&memory),
            full: true,
            now: &now,
            roles: vec![("lead".into(), "Sitar [lead]".into())],
            stashed: 2,
            renamed: None,
        });
        assert!(
            text.contains("Song: Smoke · 5 sections · last worked on 2026-09-20"),
            "{text}"
        );
        assert!(text.contains("Roles: lead = Sitar [lead]"), "{text}");
        assert!(
            text.contains("Last note (Smoke Bass, 2026-09-20): mine, never regenerate"),
            "{text}"
        );
        assert!(text.contains("Stash: 2 ideas parked"), "{text}");
        assert!(text.contains("the answer phrase an octave up"), "{text}");

        // Not full: one line instead of the whole thing.
        let short = header_text(&Header {
            memory: Some(&memory),
            full: false,
            now: &now,
            roles: vec![],
            stashed: 0,
            renamed: None,
        });
        assert!(short.contains("Overview: next (last changed"), "{short}");
        assert!(
            !short.contains("the answer phrase an octave up"),
            "the second call of a session does not spend the cap again: {short}"
        );
    }

    #[test]
    fn a_digest_reads_as_one_line_per_day() {
        let d = Digest {
            at: "2026-09-20".into(),
            tracks: vec!["Sitar".into(), "Smoke Bass".into()],
            sections: vec!["Drop".into()],
            tools: BTreeMap::from([("shape_sound".to_string(), 9), ("feel".to_string(), 5)]),
        };
        assert_eq!(
            d.line(),
            "2026-09-20 — 14 calls; tracks Sitar, Smoke Bass; sections Drop"
        );
    }
}

// ── The tools ───────────────────────────────────────────────────────────────

use crate::connection::LiveState;
use crate::tools::{live_err, read_perf_state, require, ToolResult};
use schemars::JsonSchema;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RememberParams {
    /// The model of the song, as an object: what it is trying to be
    /// (`what_it_is`), the plan section by section (`plan`), what each track
    /// is for (`tracks`), what was decided and why (`decided`), what is next
    /// (`next`). Any other key you want is kept as you wrote it. Named keys
    /// merge; the whole thing comes back on the next get_context.
    #[serde(default)]
    pub overview: Option<Value>,
    /// Rewrite the overview instead of merging into it (default false)
    #[serde(default)]
    pub replace: bool,
    /// What the note is about: "song", a track's name, or "section:<name>"
    #[serde(default)]
    pub about: Option<String>,
    /// The note. Leave it out to read back what is remembered about `about`.
    #[serde(default)]
    pub note: Option<String>,
    /// Give the track a role — "lead", "mine", "sub". It is written into the
    /// track's name (`Sitar [lead]`), so Live's own Save keeps it and the
    /// role then addresses the track in every tool.
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StashParams {
    /// "save" an idea, "list" what is parked, "place" one into the song, or
    /// "drop" one
    #[serde(default)]
    pub action: String,
    /// save: the track whose clip is parked, or the track an audio idea
    /// lands on. place/drop: the track the stashed idea sits on.
    #[serde(default)]
    pub track: Option<Value>,
    /// save: the clip to park, by name or slot. place/drop: the stashed idea,
    /// by name.
    #[serde(default)]
    pub clip: Option<Value>,
    /// save: audio instead of a clip — words ("break 90"), an absolute path,
    /// or a browser URI, parked the way add_sample would place it
    #[serde(default)]
    pub sample: Option<String>,
    /// save: what to call it (default: the clip's or the sample's own name)
    #[serde(default)]
    pub name: Option<String>,
    /// save: a word or two about why it is worth keeping
    #[serde(default)]
    pub tags: Option<String>,
    /// place: the section whose row it goes into, by name
    #[serde(default)]
    pub section: Option<Value>,
    /// place: or a Session slot by index
    #[serde(default)]
    pub slot: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SongMemoryParams {
    /// "show" what is remembered about this song (and what is not), "list"
    /// every song on this machine, "attach" a note whose track was renamed,
    /// or "forget" this song's file
    #[serde(default)]
    pub action: String,
    /// attach: the note's current subject
    #[serde(default)]
    pub about: Option<String>,
    /// attach: the track it should belong to
    #[serde(default)]
    pub to: Option<String>,
}

/// Read the set's identity through the generic ops layer and open its
/// memory. No new Remote Script command, so `SCRIPT_VERSION` does not move.
///
/// A set that has never been saved gets the provisional key, and the first
/// time Live reports a path the file is renamed to the song's own key and
/// that is said once.
pub fn open_for_set(live: &LiveState) -> SongMemory {
    let path = set_path_of(live);
    let key = key_for(&path);
    // The provisional file becomes the song's own on the first save.
    //
    // It is looked for on disk, not only in this process. The session that
    // wrote the notes is usually not the session that is running when Live
    // finally reports a path: the producer builds, closes the client, saves,
    // and comes back tomorrow. Reading only the open memory meant that the
    // common case — save at the end of the night — silently orphaned the
    // provisional file and started an empty one under the saved key, losing
    // the overview at the exact moment the set was committed (#63).
    let open = live
        .songs
        .snapshot()
        .filter(|m| m.provisional)
        .or_else(|| load(PROVISIONAL_KEY));
    if let Some(m) = open.as_ref() {
        if m.provisional && !path.is_empty() && m.key != key && !exists(&key) {
            // Adopt only into a song that has nothing of its own yet. A set
            // that already has a memory is not waiting to be told what it is,
            // and merging two would be a guess.
            live.songs.adopt(m.clone());
            if let Some(said) = live.songs.rename_to(&key, &path) {
                *live.songs.renamed.lock().unwrap_or_else(|e| e.into_inner()) = Some(said);
            }
            return live.songs.snapshot().unwrap_or_default();
        }
    }
    live.songs.open(&key, &path)
}

/// The open set's path, for tests that must tell "the set's own path" from
/// any other. Not part of the tool surface.
pub fn set_path_of_for_test(live: &LiveState) -> String {
    set_path_of(live)
}

/// `song.file_path` — empty when the set has never been saved, and empty
/// when this script is too old for the generic layer.
fn set_path_of(live: &LiveState) -> String {
    use crate::lom::{Batch, Op, Path};
    if crate::lom::require(live).is_err() {
        return String::new();
    }
    Batch::new()
        .push(Op::get(&Path::song().attr("file_path"), "set"))
        .run(live)
        .ok()
        .and_then(|out| out.get("set").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

/// What Live says about the set right now — the four numbers `as_of` holds.
pub fn as_of_now(live: &LiveState) -> AsOf {
    let Ok(state) = read_perf_state(live) else {
        return AsOf::default();
    };
    as_of_from(&state)
}

pub fn as_of_from(state: &crate::performance::PerfState) -> AsOf {
    AsOf {
        tempo: state.tempo,
        key: match (state.root_note_name.as_deref(), state.scale_name.as_deref()) {
            (Some(r), Some(s)) if !r.is_empty() && !s.is_empty() => format!("{r} {s}"),
            _ => String::new(),
        },
        sections: crate::song::sections(state).len(),
        tracks: state.tracks.len(),
        at: now(),
    }
}

pub fn remember_body(live: &LiveState, p: &RememberParams) -> ToolResult {
    let asked = [
        p.overview.is_some(),
        p.note.is_some(),
        p.role.is_some(),
        p.about.is_some(),
    ];
    if !asked.iter().any(|a| *a) {
        return Err("Give overview (the model of the song), about + note (something to keep about the song, a track or a section), or about + role (write a role into the track's name).".into());
    }
    let mut out: Vec<String> = Vec::new();
    if let Some(role) = p.role.as_deref() {
        out.push(set_role(live, p.about.as_deref(), role)?);
    }
    if let Some(overview) = p.overview.as_ref() {
        out.push(write_overview(live, overview, p.replace)?);
    }
    match (p.about.as_deref(), p.note.as_deref()) {
        (Some(about), Some(note)) => out.push(write_note(live, about, note)?),
        (Some(about), None) if p.role.is_none() => out.push(read_notes(live, about)),
        _ => {}
    }
    Ok(out.join("\n"))
}

fn write_overview(live: &LiveState, overview: &Value, replace: bool) -> Result<String, String> {
    let Some(patch) = overview.as_object() else {
        return Err(format!(
            "overview is an object, not {overview}. The keys the header renders first are: {}. \
             Any other key you want is kept as you wrote it.",
            KNOWN_KEYS.join(", ")
        ));
    };
    // The cap is checked against what the file *would* become, so an
    // overview that would not fit writes nothing at all.
    let existing = open_for_set(live);
    let mut would_be = existing.overview.clone();
    merge(&mut would_be, patch, replace);
    let size = check_cap(&would_be)?;
    if !enabled() {
        return Ok("Nothing was written: the song memory is off \
                   (ABLETON_MCP_SONG_MEMORY=false). The roles in your track names and the ideas \
                   in the Stash: row still work — they are in your Live set, not here."
            .to_string());
    }
    // `as_of` is the server's: an agent value is ignored, because the point
    // of it is to be able to say later that the set moved and this did not.
    let now = as_of_now(live);
    let mut changed = Vec::new();
    live.songs.update(|m| {
        changed = merge(&mut m.overview, patch, replace);
        m.overview.remove("as_of");
        m.as_of = now.clone();
        true
    });
    Ok(format!(
        "Remembered. {} · {size} of {OVERVIEW_CAP} bytes used. It comes back in the get_context \
         header, in full on the first call of a session and after any change.",
        if changed.is_empty() {
            "Nothing changed".to_string()
        } else {
            format!("Keys changed: {}", changed.join(", "))
        }
    ))
}

fn write_note(live: &LiveState, about: &str, note: &str) -> Result<String, String> {
    let about = about.trim();
    let note = note.trim();
    if about.is_empty() || note.is_empty() {
        return Err(
            "A note needs `about` (\"song\", a track's name, or \"section:<name>\") and `note`."
                .into(),
        );
    }
    if !enabled() {
        return Ok(
            "Nothing was written: the song memory is off (ABLETON_MCP_SONG_MEMORY=false)."
                .to_string(),
        );
    }
    open_for_set(live);
    let mut kept = 0;
    live.songs.update(|m| {
        m.notes.push(Note {
            about: about.to_string(),
            note: note.to_string(),
            at: today(),
        });
        while m.notes.len() > 200 {
            m.notes.remove(0);
        }
        kept = m.notes.len();
        true
    });
    Ok(format!(
        "Noted about '{about}' ({kept} note{} for this song). The most recent one rides back on \
         get_context; remember(about: \"{about}\") reads them all.",
        if kept == 1 { "" } else { "s" }
    ))
}

fn read_notes(live: &LiveState, about: &str) -> String {
    let memory = open_for_set(live);
    let found = memory.notes_about(about);
    if found.is_empty() {
        return format!(
            "Nothing remembered about '{}' yet. Subjects that have notes: {}.",
            about.trim(),
            if memory.notes.is_empty() {
                "none".to_string()
            } else {
                let mut subjects: Vec<&str> =
                    memory.notes.iter().map(|n| n.about.as_str()).collect();
                subjects.sort_unstable();
                subjects.dedup();
                subjects.join(", ")
            }
        );
    }
    format!(
        "About '{}':\n{}",
        about.trim(),
        found
            .iter()
            .map(|n| format!("  {} — {}", n.at, n.note))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// A role is a suffix on the track's name, so Live's Save keeps it and it
/// works with no server at all.
fn set_role(live: &LiveState, about: Option<&str>, role: &str) -> Result<String, String> {
    let about = about
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .ok_or("A role needs `about` — the track it belongs to.")?;
    require(live, "set_track_name")?;
    let state = read_perf_state(live)?;
    let want = split_role(about).0.to_lowercase();
    let hits: Vec<&crate::performance::TrackState> = state
        .tracks
        .iter()
        .filter(|t| split_role(&t.name).0.to_lowercase() == want)
        .collect();
    let track = match hits.len() {
        1 => hits[0],
        0 => {
            return Err(format!(
                "No track called '{about}'. Tracks: {}.",
                state
                    .tracks
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
        _ => {
            return Err(format!(
                "'{about}' is two tracks ({}), so nothing was renamed. Say which one.",
                hits.iter()
                    .map(|t| format!("track {}", t.index))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    };
    // A role that already belongs to another track would make it ambiguous
    // for every tool that resolves one, so it is refused rather than made.
    let role_trimmed = role.trim().to_lowercase();
    if let Some(other) = state.tracks.iter().find(|t| {
        t.index != track.index
            && split_role(&t.name)
                .1
                .is_some_and(|r| r.to_lowercase() == role_trimmed)
    }) {
        return Err(format!(
            "'{}' is already the role of '{}', and a role that names two tracks addresses \
             neither. Nothing was renamed — pick another word, or move it off '{}' first.",
            role.trim(),
            other.name,
            other.name
        ));
    }
    let was = track.name.clone();
    let now = with_role(&was, role);
    if now == was {
        return Ok(format!("'{was}' already carries that role."));
    }
    live.send_command(
        "set_track_name",
        Some(serde_json::json!({"track_index": track.index, "name": now})),
    )
    .map_err(|e| live_err("write the role into the track's name", e))?;
    live.songs.note_track(&now);
    Ok(format!(
        "'{was}' is now '{now}'. The role is in the track's name, so Live's Save keeps it and it \
         travels in the .als — and '{}' addresses this track in every tool from here.",
        role.trim()
    ))
}

pub fn song_memory_body(live: &LiveState, p: &SongMemoryParams) -> ToolResult {
    let action = p.action.trim().to_lowercase();
    match action.as_str() {
        "forget" => Ok(live.songs.forget()),
        "list" => {
            let all = all();
            if all.is_empty() {
                return Ok(format!(
                    "No songs are remembered on this machine. They would be in {}.",
                    crate::state::songs_dir().display()
                ));
            }
            Ok(format!(
                "{} song{} remembered in {}:\n{}",
                all.len(),
                if all.len() == 1 { "" } else { "s" },
                crate::state::songs_dir().display(),
                all.iter()
                    .map(|m| format!(
                        "  {} — {} overview key{}, {} note{}, last {}",
                        if m.set_name.is_empty() {
                            m.key.as_str()
                        } else {
                            m.set_name.as_str()
                        },
                        m.overview.len(),
                        if m.overview.len() == 1 { "" } else { "s" },
                        m.notes.len(),
                        if m.notes.len() == 1 { "" } else { "s" },
                        if m.updated_at.is_empty() {
                            "—"
                        } else {
                            &m.updated_at
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))
        }
        "attach" => {
            let about = p
                .about
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .ok_or("attach needs `about` (the note's current subject) and `to` (the track).")?;
            let to =
                p.to.as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or("attach needs `to` — the track the notes belong to.")?;
            open_for_set(live);
            let mut moved = 0;
            live.songs.update(|m| {
                for n in m.notes.iter_mut() {
                    if n.about.to_lowercase() == about.to_lowercase() {
                        n.about = to.to_string();
                        moved += 1;
                    }
                }
                moved > 0
            });
            if moved == 0 {
                return Ok(format!("No notes about '{about}' to attach."));
            }
            Ok(format!(
                "{moved} note{} about '{about}' now belong to '{to}'.",
                if moved == 1 { "" } else { "s" }
            ))
        }
        "" | "show" => Ok(show_memory(live)),
        other => Err(format!(
            "action must be show, list, attach or forget, not '{other}'"
        )),
    }
}

fn show_memory(live: &LiveState) -> String {
    let memory = open_for_set(live);
    let path = file_for(&memory.key);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut out = format!(
        "Song memory for '{}'{}.\n",
        if memory.set_name.is_empty() {
            "this set"
        } else {
            &memory.set_name
        },
        if memory.provisional {
            " (not saved yet, so the notes are filed provisionally and move to the song's own \
             file the first time you press Cmd+S)"
                .to_string()
        } else {
            format!(" — {} ({size} bytes)", path.display())
        }
    );
    if memory.overview.is_empty() {
        out.push_str("Overview: nothing yet.\n");
    } else {
        out.push_str(&format!(
            "Overview:\n{}\n",
            render_overview(&memory.overview)
        ));
    }
    out.push_str(&format!(
        "Notes: {}.\n",
        if memory.notes.is_empty() {
            "none".to_string()
        } else {
            format!(
                "{}\n{}",
                memory.notes.len(),
                memory
                    .notes
                    .iter()
                    .map(|n| format!("  {} · {} — {}", n.at, n.about, n.note))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    ));
    out.push_str(&format!(
        "Sessions: {}.\n",
        if memory.sessions.is_empty() {
            "none".to_string()
        } else {
            memory
                .sessions
                .iter()
                .rev()
                .take(5)
                .map(|d| d.line())
                .collect::<Vec<_>>()
                .join(" | ")
        }
    ));
    out.push_str(
        "What is NOT in this file: no MIDI note, no audio, no path outside the set's own. The \
         roles in your track names and the ideas in the Stash: row are in your Live set, not \
         here, so they survive this file being deleted. On by default, like the activity log; \
         ABLETON_MCP_SONG_MEMORY=false stops every write, and action: \"forget\" deletes it.",
    );
    if !enabled() {
        out.push_str(" It is currently OFF, so nothing new is being written.");
    }
    out
}

// ── The stash ───────────────────────────────────────────────────────────────

/// One parked idea, as the set holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Idea {
    pub scene: i64,
    pub track: i64,
    pub track_name: String,
    pub name: String,
    pub tags: String,
}

/// `"answer phrase (octave up)"` → `("answer phrase", "octave up")`. The
/// tags ride in the clip's own name, because the clip is in the producer's
/// `.als` and a note beside it in the server's file would not be.
pub fn split_tags(clip_name: &str) -> (String, String) {
    let text = clip_name.trim();
    if text.ends_with(')') {
        if let Some(open) = text.rfind(" (") {
            return (
                text[..open].trim().to_string(),
                text[open + 2..text.len() - 1].trim().to_string(),
            );
        }
    }
    (text.to_string(), String::new())
}

pub fn stash_name(name: &str, tags: &str) -> String {
    let name = name.trim();
    let tags = tags.trim();
    if tags.is_empty() {
        name.to_string()
    } else {
        format!("{name} ({tags})")
    }
}

/// The `Stash:` rows, in scene order.
fn stash_rows(state: &crate::performance::PerfState) -> Vec<i64> {
    state
        .scenes
        .iter()
        .filter(|s| crate::song::is_stash_scene(&s.name))
        .map(|s| s.index)
        .collect()
}

/// Everything parked, read off the set.
pub fn stashed(live: &LiveState) -> Result<Vec<Idea>, String> {
    let state = read_perf_state(live)?;
    let rows = stash_rows(&state);
    let mut out = Vec::new();
    for scene in rows {
        for track in &state.tracks {
            if !track.slots_with_clips.contains(&scene) {
                continue;
            }
            let info = live
                .send_command(
                    "get_clip_info",
                    serde_json::json!({"track_index": track.index, "clip_index": scene}).into(),
                )
                .map_err(|e| live_err("read a stashed idea", e))?;
            let (name, tags) =
                split_tags(crate::tools::get_display(&info, "name", "idea").as_str());
            out.push(Idea {
                scene,
                track: track.index,
                track_name: split_role(&track.name).0,
                name,
                tags,
            });
        }
    }
    Ok(out)
}

/// The row to park in: the first `Stash:` row where this track's slot is
/// free, else a new one. A track whose stash slots are all taken gets
/// another row rather than losing the idea it already had there.
fn free_stash_row(live: &LiveState, track: i64) -> Result<(i64, bool), String> {
    let state = read_perf_state(live)?;
    let Some(t) = state.tracks.iter().find(|t| t.index == track) else {
        return Err(format!("no track {track}"));
    };
    for scene in stash_rows(&state) {
        if !t.slots_with_clips.contains(&scene) {
            return Ok((scene, false));
        }
    }
    require(live, "create_scene")?;
    let r = live
        .send_command(
            "create_scene",
            serde_json::json!({"index": -1, "name": crate::song::STASH_PREFIX}).into(),
        )
        .map_err(|e| live_err("make the Stash: row", e))?;
    let index = r
        .get("index")
        .and_then(Value::as_i64)
        .ok_or("Live made the Stash: row but did not say where")?;
    Ok((index, true))
}

/// A track's name as Live has it now, through the generic ops layer — one
/// op, no command, and `None` when this script is too old for it.
fn read_track_name(live: &LiveState, track: i64) -> Option<String> {
    use crate::lom::{Batch, Op, Path};
    if crate::lom::require(live).is_err() {
        return None;
    }
    Batch::new()
        .push(Op::get(&Path::track(track).attr("name"), "name"))
        .run(live)
        .ok()?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

pub fn stash_body(live: &LiveState, p: &StashParams) -> ToolResult {
    match p.action.trim().to_lowercase().as_str() {
        "save" | "" if p.sample.is_some() || p.clip.is_some() => stash_save(live, p),
        "save" => Err("stash save needs either `clip` (a Session clip on `track` to park) or `sample` (words, a path or a browser URI to park as audio).".into()),
        "list" => stash_list(live),
        "place" => stash_place(live, p),
        "drop" => stash_drop(live, p),
        other => Err(format!(
            "action must be save, list, place or drop, not '{other}'"
        )),
    }
}

fn stash_save(live: &LiveState, p: &StashParams) -> ToolResult {
    let state = read_perf_state(live)?;
    // An audio idea: the same placement `add_sample` does, into the stash
    // row instead of a section. Choosing between three vocal chops from a
    // listing is choosing blind; this way they can be heard against each
    // other before one is committed to the song.
    if let Some(sample) = p.sample.as_deref().filter(|s| !s.trim().is_empty()) {
        // The name the producer's track had before the idea was parked.
        // Putting a sample in a slot can rename the track it lands on, and
        // parking is the one action that is explicitly *not* a commitment —
        // it is also where roles live (`Sitar [lead]`), so a stash save was
        // able to eat a role suffix (#69). A track the producer named keeps
        // its name; a track `add_sample` creates for the occasion does not
        // have one to keep.
        let (track, was_named) = match p.track.as_ref() {
            Some(t) => {
                let t = state.track_by(t)?;
                (t.index, Some(t.name.clone()))
            }
            None => (-1, None),
        };
        let (scene, made_row) = if track >= 0 {
            free_stash_row(live, track)?
        } else {
            // No track yet: `add_sample` will make one, so the row only has
            // to be a Stash: row that exists.
            match stash_rows(&state).first() {
                Some(s) => (*s, false),
                None => {
                    require(live, "create_scene")?;
                    let r = live
                        .send_command(
                            "create_scene",
                            serde_json::json!({"index": -1, "name": crate::song::STASH_PREFIX})
                                .into(),
                        )
                        .map_err(|e| live_err("make the Stash: row", e))?;
                    (
                        r.get("index")
                            .and_then(Value::as_i64)
                            .ok_or("Live made the Stash: row but did not say where")?,
                        true,
                    )
                }
            }
        };
        let placed = crate::samples::add_sample_body(
            live,
            &crate::samples::AddSampleParams {
                sample: sample.to_string(),
                track: p.track.clone(),
                slot: Some(scene),
                name: Some(stash_name(
                    p.name.as_deref().unwrap_or(sample),
                    p.tags.as_deref().unwrap_or_default(),
                )),
                ..Default::default()
            },
        )?;
        if let Some(name) = was_named.filter(|_| track >= 0) {
            let now = read_track_name(live, track);
            if now.is_some_and(|n| n != name) {
                require(live, "set_track_name")?;
                live.send_command(
                    "set_track_name",
                    serde_json::json!({"track_index": track, "name": name}).into(),
                )
                .map_err(|e| live_err("put the track's name back", e))?;
            }
        }
        return Ok(format!(
            "Parked in the Stash: row{}. {placed}\nIt is an audio clip in your set, so you can \
             fire it and hear it against the song before committing it; stash(action: \"place\") \
             puts it into a section, stash(action: \"drop\") removes it.",
            if made_row { " (made just now)" } else { "" },
        ));
    }
    // A MIDI idea: the clip's notes, copied into the stash row on the same
    // track so it plays with the right instrument.
    let track = state.track_by(
        p.track
            .as_ref()
            .ok_or("stash save needs `track` — the track the clip is on.")?,
    )?;
    let target = crate::tools::TrackTarget {
        index: track.index,
        kind: "track".into(),
        name: track.name.clone(),
    };
    let slot = crate::tools::resolve_clip_slot(live, &target, p.clip.as_ref(), None)?;
    let info = live
        .send_command(
            "get_clip_info",
            serde_json::json!({"track_index": track.index, "clip_index": slot}).into(),
        )
        .map_err(|e| live_err("read the clip to park", e))?;
    let source_name = crate::tools::get_display(&info, "name", "idea");
    let length = info.get("length").and_then(Value::as_f64).unwrap_or(4.0);
    let notes = crate::tools::clip_notes(live, track.index, slot)?;
    let (scene, made_row) = free_stash_row(live, track.index)?;
    let name = stash_name(
        p.name.as_deref().unwrap_or(&source_name),
        p.tags.as_deref().unwrap_or_default(),
    );
    require(live, "create_clip")?;
    live.send_command(
        "create_clip",
        serde_json::json!({"track_index": track.index, "clip_index": scene, "length": length})
            .into(),
    )
    .map_err(|e| live_err("make the stashed clip", e))?;
    require(live, "set_clip_name")?;
    live.send_command(
        "set_clip_name",
        serde_json::json!({"track_index": track.index, "clip_index": scene, "name": name}).into(),
    )
    .map_err(|e| live_err("name the stashed clip", e))?;
    if !notes.is_empty() {
        crate::tools::write_notes(live, track.index, scene, &notes)?;
    }
    live.songs.note_track(&track.name);
    Ok(format!(
        "Parked '{name}' from {} in the Stash: row{} ({} notes, {} bars' worth). It is a clip in \
         your set, kept by Live's own Save — not in any file of the server's. stash(action: \
         \"list\") shows what is parked.",
        split_role(&track.name).0,
        if made_row { " (made just now)" } else { "" },
        notes.len(),
        crate::memory::bars_of(length, &state)
    ))
}

/// A clip's length in whole bars, for a readout.
pub fn bars_of(length: f64, state: &crate::performance::PerfState) -> String {
    let bars = length / state.beats_per_bar();
    if (bars - bars.round()).abs() < 1e-6 {
        format!("{}", bars.round() as i64)
    } else {
        format!("{bars:.2}")
    }
}

fn stash_list(live: &LiveState) -> ToolResult {
    let ideas = stashed(live)?;
    if ideas.is_empty() {
        return Ok("Nothing is parked. stash(action: \"save\", track: …, clip: …) parks a clip, and stash(action: \"save\", sample: \"break 90\") parks audio you can hear before you commit it.".into());
    }
    let state = read_perf_state(live)?;
    let mut lines = Vec::new();
    for idea in &ideas {
        let info = live
            .send_command(
                "get_clip_info",
                serde_json::json!({"track_index": idea.track, "clip_index": idea.scene}).into(),
            )
            .map_err(|e| live_err("read a stashed idea", e))?;
        let length = info.get("length").and_then(Value::as_f64).unwrap_or(0.0);
        let midi = info
            .get("is_midi_clip")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let what = if midi {
            let notes = crate::tools::clip_notes(live, idea.track, idea.scene)?;
            format!("{} notes", notes.len())
        } else {
            let file = crate::tools::get_display(&info, "file_path", "");
            format!(
                "audio{}",
                std::path::Path::new(&file)
                    .file_name()
                    .map(|f| format!(", {}", f.to_string_lossy()))
                    .unwrap_or_default()
            )
        };
        lines.push(format!(
            "  '{}' on {} — {} bars, {what}{}",
            idea.name,
            idea.track_name,
            bars_of(length, &state),
            if idea.tags.is_empty() {
                String::new()
            } else {
                format!(" · {}", idea.tags)
            }
        ));
    }
    Ok(format!(
        "{} idea{} parked in the Stash: row{}:\n{}\nstash(action: \"place\", clip: \"<name>\", \
         section: \"<section>\") puts one into the song and leaves the parked copy; \
         stash(action: \"drop\", clip: \"<name>\") removes it.",
        ideas.len(),
        if ideas.len() == 1 { "" } else { "s" },
        if stash_rows(&state).len() == 1 {
            ""
        } else {
            "s"
        },
        lines.join("\n")
    ))
}

/// The one idea a call means, by name. Two with one name is the producer's
/// business: both are named and nothing moves.
fn one_idea(live: &LiveState, p: &StashParams) -> Result<Idea, String> {
    let ideas = stashed(live)?;
    if ideas.is_empty() {
        return Err("Nothing is parked.".into());
    }
    let want = match p.clip.as_ref() {
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(other) => other.to_string(),
        None => return Err("Say which idea: clip: \"<name>\", as stash list shows it.".into()),
    };
    let on_track = p
        .track
        .as_ref()
        .map(|t| read_perf_state(live).and_then(|s| s.track_by(t).map(|t| t.index)))
        .transpose()?;
    let hits: Vec<&Idea> = ideas
        .iter()
        .filter(|i| {
            on_track.is_none_or(|t| t == i.track)
                && (i.name.to_lowercase() == want || i.name.to_lowercase().contains(&want))
        })
        .collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err(format!(
            "No parked idea called '{want}'. Parked: {}.",
            ideas
                .iter()
                .map(|i| format!("'{}' on {}", i.name, i.track_name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!(
            "'{want}' is {} parked ideas ({}), so nothing moved. Say the track too.",
            hits.len(),
            hits.iter()
                .map(|i| format!("'{}' on {}", i.name, i.track_name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn stash_place(live: &LiveState, p: &StashParams) -> ToolResult {
    let idea = one_idea(live, p)?;
    let state = read_perf_state(live)?;
    let slot = match (p.section.as_ref(), p.slot) {
        (Some(section), _) => {
            let sections = crate::song::sections(&state);
            let which = match section {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            crate::song::find_section(&sections, &which)
                .ok_or_else(|| {
                    format!(
                        "No section called \"{}\". Sections: {}.",
                        which.trim(),
                        if sections.is_empty() {
                            "none yet".to_string()
                        } else {
                            crate::song::section_names(&sections)
                        }
                    )
                })?
                .index
        }
        (None, Some(slot)) => slot,
        (None, None) => {
            return Err(
                "Where should it go? `section` puts it in that row so it plays with the song."
                    .into(),
            )
        }
    };
    if crate::song::is_stash_scene(
        state
            .scenes
            .iter()
            .find(|s| s.index == slot)
            .map(|s| s.name.as_str())
            .unwrap_or_default(),
    ) {
        return Err("That row is the stash itself; give a section to place it into.".into());
    }
    let info = live
        .send_command(
            "get_clip_info",
            serde_json::json!({"track_index": idea.track, "clip_index": idea.scene}).into(),
        )
        .map_err(|e| live_err("read the parked idea", e))?;
    let length = info.get("length").and_then(Value::as_f64).unwrap_or(4.0);
    let midi = info
        .get("is_midi_clip")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !midi {
        // Audio: the file is placed again where it lies, exactly as
        // `add_sample` would; nothing is copied or moved.
        let file = crate::tools::get_display(&info, "file_path", "");
        if file.is_empty() {
            return Err(format!(
                "'{}' is an audio idea and Live did not say where its file is, so it cannot be \
                 placed again. Drag it in Live instead.",
                idea.name
            ));
        }
        let placed = crate::samples::add_sample_body(
            live,
            &crate::samples::AddSampleParams {
                sample: file,
                track: Some(serde_json::json!(idea.track)),
                slot: Some(slot),
                name: Some(idea.name.clone()),
                ..Default::default()
            },
        )?;
        return Ok(format!(
            "{placed}\nThe parked copy is still in the Stash: row."
        ));
    }
    let notes = crate::tools::clip_notes(live, idea.track, idea.scene)?;
    require(live, "create_clip")?;
    live.send_command(
        "create_clip",
        serde_json::json!({"track_index": idea.track, "clip_index": slot, "length": length}).into(),
    )
    .map_err(|e| live_err("place the parked idea", e))?;
    require(live, "set_clip_name")?;
    live.send_command(
        "set_clip_name",
        serde_json::json!({"track_index": idea.track, "clip_index": slot, "name": idea.name})
            .into(),
    )
    .map_err(|e| live_err("name the placed clip", e))?;
    if !notes.is_empty() {
        crate::tools::write_notes(live, idea.track, slot, &notes)?;
    }
    // An unnamed scene parses to an empty section name, and "into  " told
    // the producer nothing about where the idea went (#69). A row the
    // producer has not named is a slot, and saying so is the honest answer.
    let where_ = state
        .scenes
        .iter()
        .find(|s| s.index == slot)
        .map(|s| crate::song::parse_section_name(&s.name).0)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("slot {slot}"));
    live.songs.note_section(&where_);
    Ok(format!(
        "Placed '{}' on {} into {where_} ({} notes). The parked copy is still in the Stash: row.",
        idea.name,
        idea.track_name,
        notes.len()
    ))
}

fn stash_drop(live: &LiveState, p: &StashParams) -> ToolResult {
    let idea = one_idea(live, p)?;
    require(live, "delete_clip")?;
    live.send_command(
        "delete_clip",
        serde_json::json!({"track_index": idea.track, "clip_index": idea.scene}).into(),
    )
    .map_err(|e| live_err("drop the parked idea", e))?;
    Ok(format!(
        "Dropped '{}' from the Stash: row on {}. Nothing else changed — the row stays for the \
         other ideas.",
        idea.name, idea.track_name
    ))
}

// ── A section that changed its name, or went ────────────────────────────────

/// The subject a note about a section is filed under.
pub fn section_subject(name: &str) -> String {
    format!("section:{}", name.trim())
}

/// A section renamed: every note the producer filed about it follows, and so
/// does the session digest. Their own words are never left pointing at a
/// section that no longer exists — the same care [`reconcile`] takes when a
/// track is renamed. Returns how many notes moved.
pub fn rename_section(live: &LiveState, old: &str, new: &str) -> usize {
    let (from, to) = (section_subject(old), section_subject(new));
    let moved = std::cell::Cell::new(0usize);
    live.songs.update(|m| {
        let mut n = 0;
        for note in m.notes.iter_mut() {
            if note.about.eq_ignore_ascii_case(&from) {
                note.about = to.clone();
                n += 1;
            }
        }
        let mut touched = n > 0;
        for d in m.sessions.iter_mut() {
            for s in d.sections.iter_mut() {
                if s.eq_ignore_ascii_case(old.trim()) {
                    *s = new.trim().to_string();
                    touched = true;
                }
            }
        }
        moved.set(n);
        touched
    });
    moved.get()
}

/// How many notes are about a section, for a reply that is about to delete
/// it. Nothing is removed: a structural edit never deletes what someone
/// wrote.
pub fn notes_about_section(live: &LiveState, name: &str) -> usize {
    let subject = section_subject(name);
    live.songs
        .snapshot()
        .map(|m| {
            m.notes
                .iter()
                .filter(|n| n.about.eq_ignore_ascii_case(&subject))
                .count()
        })
        .unwrap_or(0)
}
