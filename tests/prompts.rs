//! The four prompts a producer copies into Claude, held against the tool
//! surface they name.
//!
//! `prompts/*.md` is text, so nothing about it fails on its own: a tool can be
//! renamed, moved behind `adv_`, or removed, and the prompt keeps reading as
//! confident prose while the first call it asks for no longer exists. This
//! suite is what makes that a build problem. It reads the same files the Mac
//! app embeds and the same files a Claude Code user opens, and it fails when a
//! tool-shaped name in the text is not served **in the spelling used** — an
//! artist tool unprefixed, a raw tool as `adv_`.
//!
//! It also pins the four things the story's review asked for, because they are
//! the reason the prompts exist rather than decoration: every prompt opens by
//! asking the producer what to make, the song prompt refines until the
//! producer stops it, and the performance prompt keeps composing while the set
//! plays. Wording drift is a review problem; these are not.

use mcp_ableton_music_maker::tools::Server;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The four files, in the order the screen shows them.
const PROMPT_FILES: &[&str] = &[
    "make-a-song.md",
    "prepare-a-performance-set.md",
    "finish-what-i-have.md",
    "build-around-my-idea.md",
];

fn prompts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompts")
}

fn read(file: &str) -> String {
    let path = prompts_dir().join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A prompt file is a title line, a one-line "when to use this", a rule, and
/// then the body — which is the whole of what the producer copies.
struct Prompt {
    file: &'static str,
    title: String,
    when: String,
    body: String,
}

fn parse(file: &'static str) -> Prompt {
    let text = read(file);
    let (head, body) = text
        .split_once("\n---\n")
        .unwrap_or_else(|| panic!("{file}: no `---` rule between the header and the body"));
    let mut lines = head.lines().filter(|l| !l.trim().is_empty());
    let title = lines
        .next()
        .and_then(|l| l.strip_prefix("# "))
        .unwrap_or_else(|| panic!("{file}: the first line is not a `# ` title"))
        .trim()
        .to_string();
    let when = lines
        .next()
        .unwrap_or_else(|| panic!("{file}: no one-line “when to use this” under the title"))
        .trim()
        .to_string();
    assert!(
        lines.next().is_none(),
        "{file}: the header is a title and one line, nothing else"
    );
    Prompt {
        file,
        title,
        when,
        body: body.trim().to_string(),
    }
}

fn all() -> Vec<Prompt> {
    PROMPT_FILES.iter().map(|f| parse(f)).collect()
}

/// Every name the server actually serves, keyed by its stem — the name with
/// `adv_` taken off. `get_library_status` is served as `adv_get_library_status`
/// and `capture_mix` as itself, so the stem is what a writer is reaching for
/// and the value is the only spelling that works.
fn served_by_stem() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for tool in Server::tool_router().list_all() {
        let served = tool.name.to_string();
        let stem = served.strip_prefix("adv_").unwrap_or(&served).to_string();
        map.insert(stem, served);
    }
    map
}

/// Tokens in the text that look like a tool: lower case, underscored. Single
/// word tools (`go`, `back`, `feel`, `arrange`, `batch`) are deliberately out
/// of reach of a scanner — they are ordinary English and matching them would
/// turn "go back" into a tool reference — so an underscore is the shape.
fn tool_shaped(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut token = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' {
            token.push(ch);
        } else {
            if token.contains('_') && token.starts_with(|c: char| c.is_ascii_lowercase()) {
                out.push(std::mem::take(&mut token));
            } else {
                token.clear();
            }
        }
    }
    out
}

// ── AC7: every named tool exists, in the spelling used ──────────────────────

/// Tokens in a prompt that are shaped like a tool and are not one: a
/// parameter, a value of one, a compact clip form. Nothing else in a prompt
/// may look like a tool, so this list is the whole of the exception — and
/// `the_not_a_tool_list_is_still_true` holds each entry against `src/`, so it
/// cannot quietly keep a name the code has dropped either.
const NOT_TOOLS: &[(&str, &str)] = &[
    ("dry_run", "build_song's preview parameter"),
    ("notes_csv", "one of the compact clip forms"),
    ("next_bar", "a value of `at:` on the steering verbs"),
    // The overview's own keys, as `remember(overview: {…})` takes them.
    // `memory::KNOWN_KEYS` is where they are defined.
    ("what_it_is", "a key of remember's overview object"),
    ("decided", "a key of remember's overview object"),
];

#[test]
fn every_tool_named_in_a_prompt_is_served_in_that_spelling() {
    let served = served_by_stem();
    let mut checked = 0;
    for p in all() {
        for token in tool_shaped(&p.body) {
            if NOT_TOOLS.iter().any(|(name, _)| *name == token) {
                continue;
            }
            let stem = token.strip_prefix("adv_").unwrap_or(&token);
            let correct = served.get(stem).unwrap_or_else(|| {
                panic!(
                    "prompts/{}: names `{token}`, and the server serves no such tool. \
                     Either the tool was renamed and the prompt is now a lie, or `{token}` is \
                     a parameter — in which case add it to NOT_TOOLS with what it is.",
                    p.file
                )
            });
            assert_eq!(
                &token, correct,
                "prompts/{}: writes `{token}`, but the server serves it as `{correct}`. \
                 The artist's tools are unprefixed and the raw layer is `adv_` (decision 0006); \
                 fix the prompt, not this test.",
                p.file
            );
            checked += 1;
        }
    }
    assert!(
        checked > 40,
        "only {checked} tool names found across the four prompts — the scanner has stopped seeing them"
    );
}

#[test]
fn the_not_a_tool_list_is_still_true() {
    let served = served_by_stem();
    let src: String = std::fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"))
        .expect("src/ is missing")
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect();
    let bodies: String = all().into_iter().map(|p| p.body).collect();
    for (name, what) in NOT_TOOLS {
        assert!(
            !served.contains_key(*name),
            "`{name}` is a tool now ({what} was the reason it was excused); take it out of NOT_TOOLS"
        );
        assert!(
            src.contains(name),
            "`{name}` ({what}) is in no source file — the prompts name something the server dropped"
        );
        assert!(
            bodies.contains(name),
            "no prompt says `{name}` any more; take it out of NOT_TOOLS"
        );
    }
}

#[test]
fn a_prompt_that_names_a_tool_that_does_not_exist_is_caught() {
    // The scanner has to see a plausible-but-wrong name as a name, not prose.
    let served = served_by_stem();
    assert_eq!(
        tool_shaped("Then capture_mixdown over the last eight bars, then clear_capture."),
        vec!["capture_mixdown", "clear_capture"]
    );
    for lie in ["capture_mixdown", "clear_capture", "render_master"] {
        assert!(
            !served.contains_key(lie) && !NOT_TOOLS.iter().any(|(n, _)| *n == lie),
            "{lie} is real now — pick another name for this check"
        );
    }
}

// ── AC1: four files, each with a title and a line saying when to use it ─────

#[test]
fn four_prompts_each_with_a_title_and_a_when_to_use_line() {
    let prompts = all();
    assert_eq!(prompts.len(), 4);
    let titles = [
        "Make a song from nothing",
        "Prepare a set I can perform",
        "Finish what I have already got",
        "Build around my sample or idea",
    ];
    for (p, title) in prompts.iter().zip(titles) {
        assert_eq!(p.title, title, "prompts/{}", p.file);
        assert!(
            !p.when.is_empty() && p.when.len() < 120 && !p.when.starts_with('#'),
            "prompts/{}: “{}” is not a one-line when-to-use",
            p.file,
            p.when
        );
        assert!(
            p.body.len() > 1500,
            "prompts/{}: the body is too short to be the reviewed prompt",
            p.file
        );
    }
}

#[test]
fn the_folder_holds_the_four_prompts_and_its_readme_and_nothing_else() {
    let mut found: Vec<String> = std::fs::read_dir(prompts_dir())
        .expect("prompts/ is missing")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    found.sort();
    let mut expected: Vec<String> = PROMPT_FILES.iter().map(|f| f.to_string()).collect();
    expected.push("README.md".into());
    expected.sort();
    assert_eq!(
        found, expected,
        "a file in prompts/ that the app does not embed and this suite does not read"
    );
}

// ── AC2: every prompt opens with the interview ──────────────────────────────

#[test]
fn every_prompt_starts_by_reading_the_set_and_asking() {
    for p in all() {
        let first = tool_shaped(&p.body)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("prompts/{}: names no tool at all", p.file));
        assert_eq!(
            first, "get_context",
            "prompts/{}: the first tool it reaches for is `{first}`, not `get_context`",
            p.file
        );
        assert!(
            p.body.contains("ask me") || p.body.contains("Ask me"),
            "prompts/{}: never asks the producer anything",
            p.file
        );
        // The interview is the opening, not an afterthought: the first ask
        // comes before anything is written into the set.
        let ask = p
            .body
            .find("ask me")
            .into_iter()
            .chain(p.body.find("Ask me"))
            .min()
            .unwrap();
        let first_write = ["build_song", "create_clip", "make_section", "add_sample"]
            .iter()
            .filter_map(|t| p.body.find(t))
            .min();
        if let Some(write) = first_write {
            assert!(
                ask < write,
                "prompts/{}: it writes into the set before it asks",
                p.file
            );
        }
    }
}

// ── AC3: the song prompt refines until the producer stops it ────────────────

#[test]
fn the_song_prompt_never_declares_the_mix_finished() {
    let p = parse("make-a-song.md");
    for phrase in [
        "Do not tell me it is finished",
        "that is mine to say",
        "keep going until I say it is done",
    ] {
        assert!(
            p.body.contains(phrase),
            "prompts/{}: lost “{phrase}” — the refinement loop is the point of this prompt",
            p.file
        );
    }
    // A change is proven by measuring it again, not by claiming it.
    assert!(
        p.body.contains("capture_mix that section again"),
        "prompts/{}: no re-measurement after a change",
        p.file
    );
    // And it ends at Cmd+S, because the Live API cannot save the set.
    let cmd_s = p.body.rfind("Cmd+S").expect("make-a-song.md: no Cmd+S");
    assert!(
        cmd_s > p.body.len() / 2,
        "make-a-song.md: Cmd+S is not where the prompt ends"
    );
}

/// The song prompt lays the song into the Arrangement, and says where to look.
///
/// This is not style. A producer watched a whole song get built as Session
/// clips and never placed on the timeline, and then could not see a note of
/// it: the prompt offered `set_song` **or** `arrange`, the model took the
/// first, and Live's Arrangement stayed empty. Nothing the server calls moves
/// the producer's eyes either — `song.view.detail_clip` was `null`, so the
/// clip editor showed nothing however much music was in the set. Both halves
/// are pinned here so neither can drift back.
#[test]
fn the_song_prompt_lays_the_song_down_the_timeline_and_says_where_to_look() {
    let p = parse("make-a-song.md");
    for phrase in [
        // The timeline is required, not one of two options.
        "this is not optional",
        "Session clips are not the song",
        "as well as the timeline, never instead of it",
        // And it is read back rather than assumed, like every other claim.
        "prove the timeline, do not assume it",
        // The producer is left looking at the work.
        "adv_switch_to_arrangement_view",
        "tell me the track and the bar to click",
    ] {
        assert!(
            p.body.contains(phrase),
            "prompts/{}: lost “{phrase}” — a song left in Session clips is the defect this wording exists to stop",
            p.file
        );
    }
    // The arrange step comes before the refine loop, not after it: the loop is
    // where a model stops reading.
    let arrange = p
        .body
        .find("Lay it down the timeline")
        .expect("no arrange step");
    let refine = p
        .body
        .find("Then refine it with me")
        .expect("no refine loop");
    assert!(
        arrange < refine,
        "make-a-song.md: the timeline step now sits after the refine loop, where it gets skipped"
    );
    // And the Arrangement is checked once more before the producer saves,
    // because the refine loop edits Session clips.
    let recheck = p
        .body
        .find("read the Arrangement back one last time")
        .expect("make-a-song.md: nothing re-checks the Arrangement before Cmd+S");
    assert!(
        recheck < p.body.rfind("Cmd+S").unwrap(),
        "make-a-song.md: the final Arrangement check is not before Cmd+S"
    );
}

// ── AC4: the performance prompt keeps composing while it plays ──────────────

#[test]
fn the_performance_prompt_keeps_writing_and_never_goes_silent() {
    let p = parse("prepare-a-performance-set.md");
    for phrase in [
        "add_to_song",
        "Never let it go silent",
        "never edit a clip that is sounding this phrase",
        "Always have at least one unplayed section ready",
        "hold_section",
        "three or four sections",
    ] {
        assert!(
            p.body.contains(phrase),
            "prompts/{}: lost “{phrase}” — keeping the set alive is the point of this prompt",
            p.file
        );
    }
    // The Arrangement-take question goes to the producer; `replace` is never
    // chosen for them.
    assert!(
        p.body.contains("never pass replace unless I said replace"),
        "prompts/{}: it no longer hands back the Arrangement-take question",
        p.file
    );
    // Launches land on the bar, so it plans ahead rather than chasing a beat.
    assert!(
        p.body.contains("Plan two bars ahead"),
        "prompts/{}: the on-the-bar rule is gone",
        p.file
    );
}

// ── AC5: the rules Live imposes, where they bite ────────────────────────────

#[test]
fn the_rules_live_imposes_are_in_the_prompts_that_hit_them() {
    // A fresh Live 12 set is C Major, and it bends every note written into it,
    // so every prompt that writes notes says so.
    for file in [
        "make-a-song.md",
        "finish-what-i-have.md",
        "build-around-my-idea.md",
    ] {
        let p = parse(file);
        assert!(
            p.body.contains("set_key"),
            "prompts/{file}: writes notes without setting the key"
        );
        assert!(
            p.body.contains("C Major") || p.body.contains("match it"),
            "prompts/{file}: does not say why the key has to be set"
        );
        // The Live API cannot save the set: Cmd+S is the producer's.
        assert!(
            p.body.contains("Cmd+S"),
            "prompts/{file}: never tells the producer to save"
        );
    }
}

// ── AC12 / the app and the folder stay one thing ────────────────────────────

#[test]
fn the_mac_app_embeds_these_four_files_and_no_copy_of_the_text() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if !root.join("app").is_dir() {
        // The Docker test stage builds with `app/` out of the context
        // (`.dockerignore`): the Mac app is not in the image. There is nothing
        // to hold the prompts against here; every other test in this file still
        // runs, and a checkout always has `app/`.
        return;
    }
    let src = std::fs::read_to_string(root.join("app/src-tauri/src/prompts.rs"))
        .expect("app/src-tauri/src/prompts.rs is missing");
    for file in PROMPT_FILES {
        assert!(
            src.contains(&format!("include_str!(\"../../../prompts/{file}\")")),
            "the app does not embed prompts/{file}"
        );
    }
    let js =
        std::fs::read_to_string(root.join("app/src/app.js")).expect("app/src/app.js is missing");
    assert!(
        !js.contains("You are producing in my open Ableton Live set"),
        "the prompt text has been copied into the UI; the screen renders what Rust returns"
    );
}
