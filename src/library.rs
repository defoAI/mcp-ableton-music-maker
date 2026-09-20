//! The server's own copy of Live's browser: names, folder paths and URIs of
//! every loadable item, paged out of the Remote Script's walk
//! (`get_browser_index`) in the background and kept on disk under the state
//! dir. Once complete, `search_browser` and every internal lookup answer
//! from here with no round trip to Live.
//!
//! Privacy: the file holds names, paths and URIs — no audio, no notes, no
//! project content. `ABLETON_MCP_LIBRARY_INDEX=false` keeps the index in
//! memory only; "Delete all local data" removes the folder.

use crate::connection::LiveState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub name: String,
    #[serde(default)]
    pub path: String,
    pub uri: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub is_device: bool,
}

/// Bumped when a disk copy written by an older build cannot be trusted.
/// Format 1 files were written by a build that stopped paging after the
/// first 1000 items and stamped them `complete` (#68); they are indexes of
/// a library that is mostly missing, so they are re-walked rather than read.
pub const INDEX_FORMAT: u32 = 2;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    /// Live version plus the pack list, from the Remote Script
    pub key: String,
    pub items: Vec<Item>,
    pub complete: bool,
    /// When the walk finished (or the last page arrived), local time
    pub walked_at: String,
    /// What wrote it. Absent (0) in files from before the format existed.
    #[serde(default)]
    pub format: u32,
}

/// The in-memory index plus whether a warm-up thread is running.
#[derive(Default)]
pub struct Library {
    pub index: std::sync::Mutex<Option<Index>>,
    pub warming: std::sync::atomic::AtomicBool,
}

impl Library {
    pub fn snapshot(&self) -> Option<Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// "library index: 3,412 items, complete (walked 14:12)" or "walking, 1,200 so far".
    pub fn status_line(&self) -> String {
        match self.snapshot() {
            Some(ix) if ix.complete => format!(
                "library index: {} items, complete (walked {})",
                ix.items.len(),
                ix.walked_at
            ),
            Some(ix) => format!("library index: walking, {} items so far", ix.items.len()),
            None => "library index: not started (searches go to Live)".to_string(),
        }
    }

    pub fn forget_uri(&self, uri: &str) {
        if let Some(ix) = self
            .index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            ix.items.retain(|i| i.uri != uri);
        }
    }

    pub fn clear(&self) {
        *self.index.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `ABLETON_MCP_LIBRARY_INDEX` — anything but "false"/"0"/"off"/"no" keeps the disk copy on.
pub fn disk_enabled() -> bool {
    !matches!(
        crate::env_str("ABLETON_MCP_LIBRARY_INDEX")
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "off" | "no"
    )
}

fn file_for(key: &str) -> PathBuf {
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
    crate::state::library_dir().join(format!("{safe}.json"))
}

pub fn load_from_disk(key: &str) -> Option<Index> {
    if !disk_enabled() {
        return None;
    }
    let bytes = std::fs::read(file_for(key)).ok()?;
    let ix: Index = serde_json::from_slice(&bytes).ok()?;
    if ix.format < INDEX_FORMAT {
        tracing::info!(
            "the library index on disk was written by an older build ({} items, format {}); \
             walking Live's browser again",
            ix.items.len(),
            ix.format
        );
        return None;
    }
    (ix.key == key).then_some(ix)
}

pub fn save_to_disk(ix: &Index) {
    if !disk_enabled() {
        return;
    }
    let path = file_for(&ix.key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec(ix) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!("could not write the library index {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("could not serialise the library index: {e}"),
    }
}

fn now_hhmm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// How many items one `get_browser_index` exchange asks for. The script
/// caps its own limit at 2000.
const PAGE: usize = 1000;

/// One page of the walk: merges into the in-memory index (loading the disk
/// copy first when the key matches) and returns whether the index is complete.
pub fn warm_up_step(live: &LiveState, budget_s: f64) -> Result<bool, String> {
    if !live.script.has_capability("get_browser_index") {
        return Err("the Remote Script has no get_browser_index".into());
    }
    let offset = live
        .library
        .snapshot()
        .map(|ix| {
            if ix.complete {
                usize::MAX
            } else {
                ix.items.len()
            }
        })
        .unwrap_or(0);
    if offset == usize::MAX {
        return Ok(true);
    }
    let page = live
        .send_command(
            "get_browser_index",
            Some(json!({"category": "all", "offset": offset, "limit": PAGE, "budget_s": budget_s})),
        )
        .map_err(|e| e.to_string())?;
    let key = page
        .get("library_key")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    // `index_complete` says the browser *walk* finished — not that this page
    // is the whole of it. The script walks everything, then answers with
    // `items[offset:offset + limit]` and reports the full count as
    // `total_walked`. Reading the flag as "done" stopped the paging after
    // the first page, wrote 1000 items to disk marked complete, and every
    // later session trusted that file — so "Boom Bap Kit" and "reverb" read
    // as absent from a full Suite library (#68). The index is complete when
    // the walk finished *and* every walked item has been fetched.
    let walk_finished = page
        .get("index_complete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let total_walked = page
        .get("total_walked")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let items: Vec<Item> = page
        .get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let mut guard = live.library.index.lock().unwrap_or_else(|e| e.into_inner());
    let ix = match guard.as_mut() {
        Some(ix) if ix.key == key => ix,
        _ => {
            // First page for this key: a matching disk copy is complete already.
            if let Some(disk) = load_from_disk(&key) {
                if disk.complete {
                    *guard = Some(disk);
                    return Ok(true);
                }
            }
            *guard = Some(Index {
                key: key.clone(),
                format: INDEX_FORMAT,
                ..Default::default()
            });
            guard.as_mut().unwrap()
        }
    };
    let page_was_empty = items.is_empty();
    if offset == ix.items.len() {
        ix.items.extend(items);
    }
    // Complete when the walk finished and nothing is left to fetch. A script
    // too old to report `total_walked` has only the flag to go on, and a
    // short page from such a script is the end of the walk.
    let fetched_everything = match total_walked {
        Some(total) => ix.items.len() >= total,
        None => page_was_empty || ix.items.len() < offset + PAGE,
    };
    ix.complete = walk_finished && fetched_everything;
    ix.format = INDEX_FORMAT;
    ix.walked_at = now_hhmm();
    if ix.complete {
        save_to_disk(ix);
    }
    Ok(ix.complete)
}

/// Walk the index on a background thread, one small page per exchange so
/// tool calls interleave on the shared socket. Idempotent.
pub fn start_warm_up(live: Arc<LiveState>) {
    use std::sync::atomic::Ordering;
    if live.library.warming.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("library-warm-up".into())
        .spawn(move || {
            let mut failures = 0;
            loop {
                match warm_up_step(&live, 1.0) {
                    Ok(true) => break,
                    Ok(false) => failures = 0,
                    Err(e) => {
                        // Live not up yet, an older script, or another client
                        // mid-walk: be patient (about ten minutes), then give up.
                        failures += 1;
                        tracing::debug!("library warm-up: {e}");
                        if failures > 200 {
                            break;
                        }
                        std::thread::sleep(Duration::from_secs(3));
                    }
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            live.library.warming.store(false, Ordering::SeqCst);
            tracing::info!("{}", live.library.status_line());
        })
        .ok();
}

/// The categories the walk covers ("all" in the Remote Script).
pub const WALKED_CATEGORIES: &[&str] = &[
    "all",
    "instruments",
    "sounds",
    "drums",
    "audio_effects",
    "midi_effects",
];

/// Every word must appear in the name or the folder path; hits with more words
/// in the name rank first, then shorter paths, then alphabetical.
pub fn search<'a>(ix: &'a Index, query: &str, category: &str, limit: usize) -> Vec<&'a Item> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let cat = category.trim().to_lowercase();
    let mut hits: Vec<(i64, usize, &Item)> = ix
        .items
        .iter()
        .filter(|i| cat == "all" || cat.is_empty() || i.category == cat)
        .filter_map(|i| {
            let name = i.name.to_lowercase();
            let path = i.path.to_lowercase();
            let mut in_name = 0i64;
            for w in &words {
                if name.contains(w.as_str()) {
                    in_name += 1;
                } else if !path.contains(w.as_str()) {
                    return None;
                }
            }
            Some((-in_name, i.path.len(), i))
        })
        .collect();
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.name.cmp(&b.2.name))
    });
    hits.into_iter().take(limit).map(|h| h.2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix() -> Index {
        Index {
            key: "k".into(),
            complete: true,
            walked_at: "14:12".into(),
            format: INDEX_FORMAT,
            items: vec![
                Item {
                    name: "Analog Bass Warm".into(),
                    path: "Sounds/Bass/Analog Bass Warm".into(),
                    uri: "u1".into(),
                    category: "sounds".into(),
                    is_device: false,
                },
                Item {
                    name: "Bass Station".into(),
                    path: "Sounds/Bass/Analog/Bass Station".into(),
                    uri: "u2".into(),
                    category: "sounds".into(),
                    is_device: false,
                },
                Item {
                    name: "Analog".into(),
                    path: "Instruments/Analog".into(),
                    uri: "u3".into(),
                    category: "instruments".into(),
                    is_device: true,
                },
                Item {
                    name: "Techno Kit 909".into(),
                    path: "Drums/Kits/Techno Kit 909".into(),
                    uri: "u4".into(),
                    category: "drums".into(),
                    is_device: false,
                },
            ],
        }
    }

    #[test]
    fn search_requires_every_word_and_ranks_names_first() {
        let i = ix();
        let hits: Vec<&str> = search(&i, "analog bass", "all", 10)
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        assert_eq!(
            hits,
            vec!["Analog Bass Warm", "Bass Station"],
            "both words in the name wins; a word in the path still matches"
        );
        assert!(search(&i, "analog", "instruments", 10)
            .iter()
            .all(|h| h.category == "instruments"));
        assert_eq!(search(&i, "techno kit", "all", 1)[0].uri, "u4");
        assert!(search(&i, "piano", "all", 10).is_empty());
        assert!(search(&i, "", "all", 10).is_empty());
    }

    #[test]
    fn disk_copy_round_trips_under_the_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ABLETON_MCP_STATE_DIR", dir.path());
        std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
        let i = ix();
        save_to_disk(&i);
        let path = dir.path().join("library");
        assert!(path.join("k.json").exists(), "file under <state>/library/");
        let back = load_from_disk("k").unwrap();
        assert_eq!(back.items, i.items);
        assert!(load_from_disk("other").is_none());
        std::env::set_var("ABLETON_MCP_LIBRARY_INDEX", "false");
        assert!(!disk_enabled());
        assert!(
            load_from_disk("k").is_none(),
            "off switch also stops reading"
        );
        std::env::remove_var("ABLETON_MCP_LIBRARY_INDEX");
        std::env::remove_var("ABLETON_MCP_STATE_DIR");
    }
}
