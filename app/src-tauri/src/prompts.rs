//! The Prompts screen's text. The four files in `prompts/` at the repository
//! root are compiled into the app, the way the Remote Script is, so the screen
//! shows exactly what a PR reviewed and what a Claude Code user reads without
//! the app. Nothing here touches Live, the network or the disk: copying a
//! prompt is a clipboard write and nothing else.
//!
//! The screen's own copy — the card blurb, the meta line, and the three or
//! four things that happen after a paste — lives here beside the file it
//! describes, so the UI renders what Rust returns and there is no second copy
//! of a prompt in JavaScript.

use serde_json::{json, Value};

/// One card: the file it shows, and the screen's words for it.
struct Card {
    id: &'static str,
    file: &'static str,
    /// The whole markdown file, embedded at build time.
    source: &'static str,
    /// One line on the card, under the title.
    card: &'static str,
    /// The small print on the card.
    meta: &'static str,
    /// "What happens after you paste": a name, a sentence, and the tools it
    /// reaches for, in the spelling the model will see.
    steps: &'static [(&'static str, &'static str, &'static [&'static str])],
}

const CARDS: &[Card] = &[
    Card {
        id: "song",
        file: "make-a-song.md",
        source: include_str!("../../../prompts/make-a-song.md"),
        card: "An empty set, and a track at the end of it.",
        meta: "4 questions, then it keeps refining · make-a-song.md",
        steps: &[
            (
                "It looks",
                "Reads the set and the instruments this Live actually has, and says in one line what is already there.",
                &["get_context", "adv_get_library_status"],
            ),
            (
                "It asks you",
                "Style or a reference, mood, length, anything that must be in it — with a default for each, so “go” is a valid answer.",
                &[],
            ),
            (
                "It builds",
                "Key and tempo first, then the whole set in one validated document — shown to you as a plan before a single command reaches Live.",
                &["set_key", "set_tempo", "build_song"],
            ),
            (
                "It refines until you stop it",
                "Section by section: change, play it, measure it again, ask you. It never declares the mix finished — you do.",
                &["shape_sound", "feel", "set_track_mixer", "capture_mix"],
            ),
        ],
    },
    Card {
        id: "perform",
        file: "prepare-a-performance-set.md",
        source: include_str!("../../../prompts/prepare-a-performance-set.md"),
        card: "Sections and a setlist, then it drives — writing the next part while this one plays.",
        meta: "Never goes silent · prepare-a-performance-set.md",
        steps: &[
            (
                "It asks, then starts",
                "Length, material, order, whether you are playing — then three or four sections, a setlist, and the set is running.",
                &["get_context", "make_section", "set_song", "play_song"],
            ),
            (
                "It writes while it plays",
                "The next section is built out of the one looping now and queued ahead of you, announced in a line before it ever sounds.",
                &["make_section", "add_to_song", "feel"],
            ),
            (
                "It steers on the bar",
                "Phrase ends, holds, jumps back — and it never lets the set fall silent or grind round the same loop.",
                &["go", "hold_section", "jump_to", "end_performance"],
            ),
        ],
    },
    Card {
        id: "finish",
        file: "finish-what-i-have.md",
        source: include_str!("../../../prompts/finish-what-i-have.md"),
        card: "Half-finished material, honestly assessed and taken to done.",
        meta: "Touches nothing without asking · finish-what-i-have.md",
        steps: &[
            (
                "It looks",
                "Every track, clip, section and the key and tempo — then says what is finished, what is a sketch, and what clashes.",
                &["get_context"],
            ),
            (
                "It asks you",
                "What “finished” means for this one, and what is untouchable.",
                &[],
            ),
            (
                "It fills, arranges, measures",
                "Holes first, then the timeline in bars, then a mix fixed against numbers — and it says what it did not touch.",
                &[
                    "create_clip",
                    "feel",
                    "arrange",
                    "create_locator",
                    "capture_mix",
                    "set_track_mixer",
                ],
            ),
        ],
    },
    Card {
        id: "sample",
        file: "build-around-my-idea.md",
        source: include_str!("../../../prompts/build-around-my-idea.md"),
        card: "One loop, one recording, one idea — and a track that serves it.",
        meta: "Nothing is copied or moved · build-around-my-idea.md",
        steps: &[
            (
                "It looks, then asks",
                "Reads the set, then offers the three ways in: a folder on this Mac, Live’s browser, or you playing it.",
                &[
                    "get_context",
                    "adv_sample_folders",
                    "search_browser",
                    "record_clip",
                ],
            ),
            (
                "It places the source",
                "Warped and looped to whole bars, in a section or at a bar. The file is referenced where it lives.",
                &["add_sample"],
            ),
            (
                "It builds around it",
                "Matches key and tempo to the source, adds only parts that earn their place, and checks the two play together.",
                &["set_key", "build_song", "capture_mix"],
            ),
        ],
    },
];

/// Title, when-to-use and body, read out of the embedded file. The shape is
/// the one `prompts/README.md` documents: a `# ` title, one line saying when
/// to use it, a `---` rule, and then the prompt. A file that does not parse is
/// a build-time mistake, so the fallbacks keep the screen usable and say so
/// rather than hiding it.
fn split(source: &'static str) -> (&'static str, &'static str, &'static str) {
    let (head, body) = match source.split_once("\n---\n") {
        Some(parts) => parts,
        None => return ("", "", source.trim()),
    };
    let mut lines = head.lines().filter(|l| !l.trim().is_empty());
    let title = lines.next().unwrap_or("").trim_start_matches("# ").trim();
    let when = lines.next().unwrap_or("").trim();
    (title, when, body.trim())
}

/// Everything the Prompts screen draws, in the order it draws it.
pub fn list() -> Value {
    let prompts: Vec<Value> = CARDS
        .iter()
        .map(|c| {
            let (title, when, body) = split(c.source);
            json!({
                "id": c.id,
                "file": format!("prompts/{}", c.file),
                "title": title,
                "when": when,
                "body": body,
                "words": body.split_whitespace().count(),
                "card": c.card,
                "meta": c.meta,
                "steps": c.steps.iter().map(|(name, what, tools)| json!({
                    "name": name,
                    "what": what,
                    "tools": tools,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "prompts": prompts })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_card_carries_a_title_a_when_and_a_body() {
        let v = list();
        let prompts = v["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 4);
        for p in prompts {
            assert!(!p["title"].as_str().unwrap().is_empty(), "{p}");
            assert!(!p["when"].as_str().unwrap().is_empty(), "{p}");
            let body = p["body"].as_str().unwrap();
            assert!(body.starts_with("You are "), "{}", p["file"]);
            assert!(body.contains("get_context"), "{}", p["file"]);
            assert!(
                !body.contains("\n# "),
                "{}: the header leaked into the body",
                p["file"]
            );
            assert!(p["words"].as_u64().unwrap() > 300, "{}", p["file"]);
            assert!(!p["steps"].as_array().unwrap().is_empty(), "{}", p["file"]);
        }
    }

    #[test]
    fn the_ids_are_the_four_states_a_producer_opens_the_app_in() {
        let v = list();
        let ids: Vec<&str> = v["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["song", "perform", "finish", "sample"]);
    }
}
