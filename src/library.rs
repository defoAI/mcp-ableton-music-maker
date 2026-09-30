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
/// Format 2 files know nothing of plugins or Max for Live (#73): a producer
/// reading one would keep being told they own no Serum. They are re-walked too.
pub const INDEX_FORMAT: u32 = 3;

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
    /// Third-party plugins and Max for Live devices (#73). They are kept
    /// apart from `items` because the Remote Script pages `items` by offset
    /// and these are walked here, through `run`, against every script that
    /// already has the generic layer — no new command, no reinstall.
    #[serde(default)]
    pub plugins: Vec<Item>,
    /// Whether that walk finished. Until it has, a plugin search answers
    /// with what is known so far and says so.
    #[serde(default)]
    pub plugins_walked: bool,
}

/// The in-memory index plus whether a warm-up thread is running.
#[derive(Default)]
pub struct Library {
    pub index: std::sync::Mutex<Option<Index>>,
    pub warming: std::sync::atomic::AtomicBool,
    /// Whether the plugin roots have been walked *at all* this session, which
    /// is not the same as walked to the end: a walk that ran out of budget is
    /// not repeated on every search.
    pub plugins_attempted: std::sync::atomic::AtomicBool,
}

impl Library {
    pub fn snapshot(&self) -> Option<Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// "library index: 3,412 items, complete (walked 14:12)" or "walking, 1,200 so far".
    pub fn status_line(&self) -> String {
        match self.snapshot() {
            Some(ix) if ix.complete => format!(
                "library index: {} items{}, complete (walked {})",
                ix.items.len(),
                plugin_note(&ix),
                ix.walked_at
            ),
            // The plugin count belongs here too: the two walks are separate,
            // and a line that said "walking, 0 items so far" above a search
            // that had just listed three Max for Live devices was counting
            // only half of what it holds.
            Some(ix) => format!(
                "library index: walking, {} items so far{}",
                ix.items.len(),
                plugin_note(&ix)
            ),
            None => "library index: not started (searches go to Live)".to_string(),
        }
    }

    /// Whether every walk is done — Live's own categories and the plugins.
    pub fn is_complete(&self) -> bool {
        self.snapshot()
            .map(|ix| ix.complete && ix.plugins_walked)
            .unwrap_or(false)
    }

    pub fn forget_uri(&self, uri: &str) {
        if let Some(ix) = self
            .index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            ix.items.retain(|i| i.uri != uri);
            ix.plugins.retain(|i| i.uri != uri);
        }
    }

    pub fn clear(&self) {
        *self.index.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.plugins_attempted
            .store(false, std::sync::atomic::Ordering::SeqCst);
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
            let mut walked_live_categories = false;
            loop {
                match warm_up_step(&live, 1.0) {
                    Ok(true) => {
                        walked_live_categories = true;
                        break;
                    }
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
            // Plugins and Max for Live come last, and only once Live's own
            // categories are in: they are walked from here rather than by the
            // script (#73), and a producer asking for "reverb" should not wait
            // behind someone else's VST folder.
            if walked_live_categories {
                match walk_plugins(&live, PLUGIN_WALK_BUDGET) {
                    Ok(n) => tracing::info!("plugin walk: {n} item(s)"),
                    Err(e) => tracing::debug!("plugin walk: {e}"),
                }
            }
            live.library.warming.store(false, Ordering::SeqCst);
            tracing::info!("{}", live.library.status_line());
        })
        .ok();
}

/// The categories a search can answer from the index: the six the Remote
/// Script walks, and the two walked from here (#73).
pub const WALKED_CATEGORIES: &[&str] = &[
    "all",
    "instruments",
    "sounds",
    "drums",
    "audio_effects",
    "midi_effects",
    "plugins",
    "max_for_live",
];

/// The two browser roots the Remote Script's walk never had: third-party
/// plugins and Max for Live.
///
/// Live exposes them as `browser.plugins` and `browser.max_for_live`
/// (recorded off a real Live 12.4.6 in `tests/fixtures/live-lom-12.4.6.json`),
/// and `_walk_browser_for_uri` in the script already loads by their URIs — so
/// the only thing missing was a way to produce one. Walking them from here,
/// through `run`, means it works against every script that already has the
/// generic layer: no new command, no `SCRIPT_VERSION` bump, no reinstall
/// (decision 0010).
pub const PLUGIN_CATEGORIES: &[&str] = &["plugins", "max_for_live"];

/// How deep the plugin tree is followed. A VST folder is vendor/product;
/// Max for Live is category/device. Six is slack, not a target.
const PLUGIN_DEPTH: usize = 6;
/// Children read per round trip. Five reads each (name, uri, loadable,
/// device, folder) against the script's 512-op ceiling.
const PLUGIN_CHUNK: usize = 100;
/// A plugin folder the size of a sample library is a mistake somewhere;
/// stopping says so rather than walking for ever.
const PLUGIN_MAX_ITEMS: usize = 20_000;

pub fn is_plugin_category(category: &str) -> bool {
    PLUGIN_CATEGORIES.contains(&category.trim().to_lowercase().as_str())
}

/// The script renders a Live sequence as a list of at most this many entries
/// (`_jsonable`), so a longer one has to be counted by asking for its members.
const JSONABLE_CAP: usize = 256;

/// How many children a browser item has.
///
/// A read of `children` answers with the members, but only the first 256 of
/// them: a vendor folder with 400 plug-ins would be walked as 256 and the
/// rest would read as not installed. So a full-looking list is followed by
/// probing single indices — each one is an op that either answers or is
/// refused as out of range — doubling and then bisecting, about ten round
/// trips for a folder of thousands and none at all for the usual folder.
fn children_count(live: &LiveState, node: &crate::lom::Path) -> Result<usize, String> {
    use crate::lom::{Batch, Op};
    let children = node.clone().attr("children");
    let kids = Batch::new().push(Op::get(&children, "kids")).run(live)?;
    let seen = match kids.get("kids") {
        Some(Value::Array(a)) => a.len(),
        // A root this Live does not have, or a script too old to render a
        // Live vector as a list (#66): nothing to walk.
        _ => return Ok(0),
    };
    if seen < JSONABLE_CAP {
        return Ok(seen);
    }
    let exists = |i: usize| -> bool {
        Batch::new()
            .push(Op::get(&children.clone().at(i as i64).attr("name"), "n"))
            .run(live)
            .is_ok()
    };
    let (mut low, mut high) = (seen, seen * 2);
    while exists(high - 1) && high < PLUGIN_MAX_ITEMS {
        low = high;
        high *= 2;
    }
    // `low` exists, `high - 1` does not: the count is somewhere between.
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if exists(mid - 1) {
            low = mid;
        } else {
            high = mid;
        }
    }
    Ok(low)
}

/// Walk `browser.<attr>`: every loadable item under it, and whether the walk
/// reached the end rather than the budget.
fn walk_browser_root(
    live: &LiveState,
    attr: &str,
    category: &str,
    deadline: std::time::Instant,
) -> Result<(Vec<Item>, bool), String> {
    use crate::lom::{Batch, Op, Path};
    let mut out: Vec<Item> = Vec::new();
    let mut stack: Vec<(Path, Vec<String>, usize)> = vec![(Path::browser().attr(attr), vec![], 0)];
    while let Some((node, trail, depth)) = stack.pop() {
        if out.len() >= PLUGIN_MAX_ITEMS || std::time::Instant::now() >= deadline {
            return Ok((out, false));
        }
        let count = children_count(live, &node)?;
        for start in (0..count).step_by(PLUGIN_CHUNK) {
            let end = (start + PLUGIN_CHUNK).min(count);
            let mut batch = Batch::new();
            for i in start..end {
                let kid = node.clone().attr("children").at(i as i64);
                batch = batch
                    .push(Op::get(&kid.clone().attr("name"), &format!("n{i}")))
                    .push(Op::get(&kid.clone().attr("uri"), &format!("u{i}")))
                    .push(Op::get(&kid.clone().attr("is_loadable"), &format!("l{i}")))
                    .push(Op::get(&kid.clone().attr("is_device"), &format!("d{i}")))
                    .push(Op::get(&kid.attr("is_folder"), &format!("f{i}")));
            }
            let answer = batch.run(live)?;
            for i in start..end {
                let name = answer
                    .get(&format!("n{i}"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut here = trail.clone();
                if !name.is_empty() {
                    here.push(name.clone());
                }
                let uri = answer
                    .get(&format!("u{i}"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let loadable = answer
                    .get(&format!("l{i}"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if loadable && !uri.is_empty() {
                    out.push(Item {
                        name: name.clone(),
                        path: here.join("/"),
                        uri,
                        category: category.to_string(),
                        is_device: answer
                            .get(&format!("d{i}"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                }
                let folder = answer
                    .get(&format!("f{i}"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if folder && depth + 1 < PLUGIN_DEPTH {
                    stack.push((node.clone().attr("children").at(i as i64), here, depth + 1));
                }
            }
        }
    }
    Ok((out, true))
}

/// How long a plugin walk may run before it stops with what it has. The
/// background pass gets the whole of it; a search that triggers the walk
/// itself gets less, because a producer is waiting on the reply.
pub const PLUGIN_WALK_BUDGET: Duration = Duration::from_secs(60);
pub const PLUGIN_WALK_BUDGET_INLINE: Duration = Duration::from_secs(15);

/// Walk both plugin roots into the index. Returns how many were found.
///
/// A producer with no plugins installed gets zero and a walk marked done —
/// which is the answer, and a different one from "this server cannot see
/// them". `adv_get_library_status` says which.
pub fn walk_plugins(live: &LiveState, budget: Duration) -> Result<usize, String> {
    use std::sync::atomic::Ordering;
    live.library.plugins_attempted.store(true, Ordering::SeqCst);
    if !crate::lom::available(live) {
        return Err("the Remote Script has no run/describe".into());
    }
    let deadline = std::time::Instant::now() + budget;
    let mut found: Vec<Item> = Vec::new();
    let mut whole = true;
    for (attr, category) in [("plugins", "plugins"), ("max_for_live", "max_for_live")] {
        let (items, complete) = walk_browser_root(live, attr, category, deadline)?;
        found.extend(items);
        whole = whole && complete;
    }
    let total = found.len();
    let mut guard = live.library.index.lock().unwrap_or_else(|e| e.into_inner());
    let ix = guard.get_or_insert_with(|| Index {
        format: INDEX_FORMAT,
        ..Default::default()
    });
    ix.plugins = found;
    // Half a walk is not a walk: saying it finished would tell a producer
    // their Serum is not installed when it is simply further down the folder.
    ix.plugins_walked = whole;
    ix.format = INDEX_FORMAT;
    if ix.complete && ix.plugins_walked {
        save_to_disk(ix);
    }
    Ok(total)
}

/// ", 12 plugins" — said only when there are some, so a producer with none
/// is not told about a category they do not use.
fn plugin_note(ix: &Index) -> String {
    if ix.plugins.is_empty() {
        return String::new();
    }
    let m4l = ix
        .plugins
        .iter()
        .filter(|i| i.category == "max_for_live")
        .count();
    match (ix.plugins.len() - m4l, m4l) {
        (0, m) => format!(", {m} Max for Live"),
        (p, 0) => format!(", {p} plugin(s)"),
        (p, m) => format!(", {p} plugin(s) and {m} Max for Live"),
    }
}

/// Every word must appear in the name or the folder path; hits with more words
/// in the name rank first, then shorter paths, then alphabetical.
pub fn search<'a>(ix: &'a Index, query: &str, category: &str, limit: usize) -> Vec<&'a Item> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let cat = category.trim().to_lowercase();
    // "all" means all of it: a producer typing "Serum" expects it found, and
    // the plugin half is indexed beside Live's own (#73).
    let mut hits: Vec<(i64, usize, &Item)> = ix
        .items
        .iter()
        .chain(ix.plugins.iter())
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
            plugins: vec![
                Item {
                    name: "Serum".into(),
                    path: "VST3/Xfer Records/Serum".into(),
                    uri: "vst3:instr:Serum".into(),
                    category: "plugins".into(),
                    is_device: true,
                },
                Item {
                    name: "Poli.amxd".into(),
                    path: "Max Instrument/Poli.amxd".into(),
                    uri: "query:MaxForLive:MaxInstrument#Poli.amxd".into(),
                    category: "max_for_live".into(),
                    is_device: true,
                },
            ],
            plugins_walked: true,
        }
    }

    #[test]
    fn a_plugin_is_found_by_its_own_category_and_under_all() {
        let i = ix();
        assert_eq!(
            search(&i, "serum", "plugins", 10)[0].uri,
            "vst3:instr:Serum"
        );
        assert_eq!(
            search(&i, "serum", "all", 10)[0].uri,
            "vst3:instr:Serum",
            "a producer typing one word expects it found (#73)"
        );
        assert_eq!(search(&i, "poli", "max_for_live", 10).len(), 1);
        assert!(
            search(&i, "serum", "instruments", 10).is_empty(),
            "a category still filters"
        );
        assert!(is_plugin_category("plugins") && is_plugin_category("max_for_live"));
        assert!(!is_plugin_category("instruments"));
        for c in PLUGIN_CATEGORIES {
            assert!(
                WALKED_CATEGORIES.contains(c),
                "{c} is answered from the index, never sent to the script"
            );
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
