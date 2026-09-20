# Prompts screen: one copy starts the song or the set

## Story
**As a** producer who has just connected Claude to my Live set and has no idea what to type,
**I want** a screen of finished prompts I can copy and paste,
**So that** Claude arrives already knowing the tools, the order they go in, the rules Live
imposes, and that its first job is to ask me what I want to make.

## Details
| Field | Value |
|-------|-------|
| Status | `Draft` |
| Surface | [Decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md): adds **no tool**. It is app UI plus four markdown files; the prompts name only tools that already exist, in the artist's spelling, and say `adv_` where they mean the raw layer |
| Priority | P1 — the gap between "Setup complete" and the first note is where a new producer gives up; Setup already ends with nothing to do next |
| Size | M — four prompt files (the real work is the wording), one screen, one Tauri command, one conformance test |
| Tracker | [#59](https://github.com/defoAI/mcp-ableton-music-maker/issues/59) — the full task list |
| Created | 2026-09-20 |
| Updated | 2026-09-20 |
| Prototype | [prototypes/prompts-screen-one-copy-starts-song-or-set.html](../prototypes/prompts-screen-one-copy-starts-song-or-set.html) · [review link](https://claude.ai/artifact/XZKNS4TCVd9uAWDrLiHpPa) |

## Context

### The Problem
A producer finishes Setup, opens Claude, and faces an empty message box with 105 tools
(verified 2026-09-20 against `src/tools.rs`) behind it. What they type is usually "make me a
techno track", which gets a reply that writes four clips into an empty set, never asks what
they actually want, never sets the key, never listens to the result, and stops the moment
something is on screen. The product's whole workflow — key before notes, `dry_run` before
building, `capture_mix` before claiming it sounds right, sections before a setlist, the
performance loop that keeps a set alive — is knowledge the server hands to the *model*, not
to the *person*, and only some clients surface it before the first message.

Two things in particular never happen by themselves:

- **A song is declared finished at the draft.** The model builds, reports, and stops.
  Nobody told it that the producer expects to sit there changing things until the mix is
  right.
- **A performance is prepared and then runs out.** The model makes sections, fires them, and
  waits for instructions while the same eight bars loop. Nobody told it that the job during a
  set is to keep writing the next section while the current one plays.

### Current State
- The server's `INSTRUCTIONS` (`src/context.rs:13`) are handed to the client at `initialize`
  and are written for the model: units, workflow, refusals. `FOOTER` (`src/context.rs:34`)
  repeats the short form on every `get_context`. Neither is addressed to the producer, and
  neither can ask them what they want to make.
- `CORE_TOOLS` (`src/tools.rs:162`) is the artist's set; everything else is served as
  `adv_<name>`. A prompt that wants the raw layer must say so in that spelling.
- The Mac app has four screens — Overview, Activity, Listen, Setup, Settings
  (`app/src/index.html:13`) — and Setup's last step is "your client is configured". Nothing
  follows it.
- The app already copies text to the clipboard with a fallback and a toast
  (`app/src/app.js:422`), and screen switching is `go(screen)` (`app/src/app.js:433`), so the
  mechanics of a new screen are in place.
- The app's Rust side already reads the chain's state (`app/src-tauri/src/status.rs`) and the
  client config (`app/src-tauri/src/clients.rs`), so a banner can say truthfully whether a
  pasted prompt will work.

### Root Cause
The product's interface is a conversation, and nothing in the product teaches a producer how
to open one. The knowledge exists — in `INSTRUCTIONS`, in the tool descriptions, in this
repository's stories — everywhere except in front of the person holding the mouse.

## Open Questions

1. **Does the app fill the prompt in (genre, BPM, length) before copying?** → **No.** The
   prompt is finished text and tells Claude to run the interview in chat, offering a default
   for every question so "go" is a valid answer. A form in the app would ask for the one
   thing a producer is worst at typing and best at describing loosely.
2. **Which prompts ship?** → **Four**, matching the four states a producer is in when they
   open the app: *Make a song from nothing*, *Prepare a set I can perform*, *Finish what I
   have already got*, *Build around my sample or idea*.
3. **Where does the text live?** → **One markdown file per prompt in `prompts/` at the
   repository root**, embedded in the app binary with `include_str!`. A PR reviews the
   wording; a Claude Code or Cursor user reads the same file without the Mac app; the
   conformance test reads the same file.
4. **Does the "make a song" prompt stop at a built set?** → **No.** It carries an explicit
   refinement loop: play a section, change what the producer reacts to, `capture_mix` again,
   report what actually moved, ask again — and it is told in as many words never to declare
   the mix finished, because that is the producer's to say.
5. **Does the performance prompt stop once the set is playing?** → **No.** Most of its text
   is about what happens *while the music runs*: build the next section out of the one
   looping now, `add_to_song` it ahead of the playhead, announce it in one line before it
   sounds, never let the set fall silent, never edit a clip that is sounding this phrase.
6. **Can the app send the prompt to Claude for me?** → **No, and it must not pretend to.**
   The client starts the server over stdio (`src/app.rs`); the app never holds the
   conversation. Copy is the only honest action on the screen.
7. **Does copying a prompt get logged?** → **No.** The activity log is the record of what
   reached Live (`src/activity.rs`). Logging an in-app click would widen what is stored and
   needs a `TERMS.md` line for no benefit.
8. **What stops a prompt going stale when a tool changes?** → A test that reads
   `prompts/*.md` and fails when a tool name in the text is not served (AC7). Wording drift
   is a review problem; a named tool that does not exist is a build problem.
9. **Do the prompts name the client ("paste into Claude Desktop")?** → **No.** The same text
   is pasted into Claude Code and Cursor. The screen says "into Claude"; the prompt names no
   client.

## Prototype
[prompts-screen-one-copy-starts-song-or-set.html](../prototypes/prompts-screen-one-copy-starts-song-or-set.html),
published at https://claude.ai/artifact/XZKNS4TCVd9uAWDrLiHpPa. The screen with all four
prompts in full, the card rail, the "what happens after you paste" strip, and the three chain
states (connected · setup unfinished · Live not running).

What the review changed: the performance prompt was rewritten from "prepare a set, then
steer it" to "prepare the minimum, then keep composing while it plays" — most of its body is
now the keep-it-going loop; and the song prompt gained the refine-until-I-say-stop loop and a
second card step, so the screen no longer implies the work ends when the set is built.

## Tool description
N/A — this story adds no MCP tool and changes no tool description. The prompts are text that
uses the existing surface.

## Acceptance Criteria

### The prompts
- [ ] **AC1 — four files.** `prompts/make-a-song.md`,
      `prompts/prepare-a-performance-set.md`, `prompts/finish-what-i-have.md`,
      `prompts/build-around-my-idea.md`, each with a title line, a one-line "when to use
      this", and the prompt body, verbatim as it appears in the prototype.
- [ ] **AC2 — every prompt opens with the interview.** Each begins with `get_context`, a
      one-line readout of what is already in the set, and a short round of questions with a
      default offered for each. No prompt writes a note before the producer has answered.
- [ ] **AC3 — the song prompt refines until the producer stops it.** `make-a-song.md`
      contains the loop: play the section, change, `capture_mix` again, report what moved,
      ask; and the instruction never to call the mix finished. It ends at Cmd+S, not at
      "done".
- [ ] **AC4 — the performance prompt keeps composing while it plays.**
      `prepare-a-performance-set.md` instructs: prepare three or four sections only; build
      the next section from the row looping now and `add_to_song` it ahead of the playhead;
      announce it in one line before it sounds; always keep one unplayed section ready;
      `hold_section` rather than let the set grind round; never edit a clip sounding this
      phrase; never let it go silent.
- [ ] **AC5 — the rules Live imposes are in the text**, once each and where they bite:
      `set_key` before notes (a fresh Live 12 set is C Major), launches land on the bar,
      plan two bars ahead, the Arrangement-take question is the producer's to answer, and
      Cmd+S is the producer's because the Live API cannot save.
- [ ] **AC6 — artist spelling.** Prompts use `CORE_TOOLS` names and write `adv_` explicitly
      where they mean the raw layer (`adv_cue`, `adv_keep_track_playing`,
      `adv_sample_folders`, `adv_get_library_status`).
- [ ] **AC7 — every named tool exists.** A test extracts every tool-shaped name from
      `prompts/*.md` and fails if one is not served by `Server::tool_router()`, in the
      spelling used.

### The screen
- [ ] **AC8 — a Prompts item in the app's navigation**, between Listen and Setup, showing
      the four prompts as a card rail with the selected prompt's full text below it.
- [ ] **AC9 — copy.** One "Copy prompt" button per prompt, using the existing `copy()` path
      (`app/src/app.js:422`) with its fallback, and a toast that says what to do next
      ("Copied. Paste it into Claude and answer its questions."). The prompt text is
      selectable for a manual copy.
- [ ] **AC10 — honest state.** When no client is configured, or Live is not running, a
      banner says which call will fail first and links to Setup. Copying stays enabled.
- [ ] **AC11 — no send button.** Nothing on the screen claims to talk to Claude.
- [ ] **AC12 — the text shown is the file.** The UI renders what
      `app/src-tauri` returns from `include_str!`, not a second copy of the prompt in JS.
- [ ] **AC13 — the screen works with Live closed and no client configured**, because a
      producer may be reading it before installing anything.

### No Regressions
- [ ] **AC14:** The server is untouched: no new tool, no changed description, no change to
      `INSTRUCTIONS`, `SCRIPT_VERSION` unchanged, `CORE_TOOLS` unchanged.
- [ ] **AC15:** Nothing new is written to disk or to the activity log when a prompt is
      copied; `tests/activity.rs` and `tests/local_only.rs` pass unchanged.
- [ ] **AC16:** The other five screens and the menu-bar popover behave as before; `go()`
      still switches screens and the Listen screen still starts and stops.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `app/src/index.html` | Nav item + `<section class="screen" id="s-prompts">`: banner, card rail, detail panel |
| `app/src/app.js` | `renderPrompts()`, selection state, `go('prompts')` case; reuse `copy()` and `toast()` |
| `app/src/app.css` | Card rail, prompt panel (monospace, scrollable, `user-select: text`), chips |
| `app/src-tauri/src/lib.rs` | Register the `prompts` command |
| `CLAUDE.md` · `docs/facts/source-of-truth.md` | Suite count and the snapshot re-dated in the same PR, per the facts rule |
| `docs/technical/feature-matrix.md` · `docs/architecture/overview.md` | The app gains a screen; the server does not change |
| `docs/product_management/prototypes/README.md` | Prototype row + review link |

### New
| File | Description |
|------|-------------|
| `prompts/make-a-song.md` | Empty set → finished mix, with the refinement loop |
| `prompts/prepare-a-performance-set.md` | Minimum to start, then compose while it plays |
| `prompts/finish-what-i-have.md` | Honest assessment, fill, arrange, measure, say what was left alone |
| `prompts/build-around-my-idea.md` | Sample or recording first, everything else serves it |
| `prompts/README.md` | What these are, that the app embeds them, how to use them without the app |
| `app/src-tauri/src/prompts.rs` | `include_str!` the four files; one command returning id, title, when-to-use, body |
| `tests/prompts.rs` | AC7, plus: every prompt starts with `get_context`, and the two loop prompts contain their loop instructions |

## Remote Script compatibility
**None.** No command is added or changed; `SCRIPT_VERSION` and `ALL_REMOTE_COMMANDS` are
untouched, and `tests/remote_script/` and `fake_live.py` need no change. The prompts run
against whatever the installed script already serves; a producer on an older script gets the
server's existing capability error from the first tool that is missing, which is the
behaviour today.

## Privacy
**No new local data.** Copying a prompt writes nothing: no activity line, no state file, no
preference beyond the selected card in memory. The prompt files are read-only text compiled
into the app binary. `TERMS.md` needs no change, and `tests/activity.rs` and
`tests/local_only.rs` are unchanged — AC15 pins that.

## Test Coverage

| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/prompts.rs` (new) | Reads `prompts/*.md` from `CARGO_MANIFEST_DIR`: every tool-shaped token that matches a served name's stem must be served in the spelling used (an artist tool unprefixed, a raw tool as `adv_`); every file starts with `get_context`; `make-a-song.md` contains the never-declare-it-finished instruction and `capture_mix` after a change; `prepare-a-performance-set.md` contains `add_to_song` and the never-silent instruction | AC1, AC2, AC3, AC4, AC6, AC7 |
| `tests/activity.rs`, `tests/local_only.rs` | Unchanged — run to prove they still pass | AC15 |
| `app/src/prompts.test.mjs` (new, `node --test`) | `renderPrompts()` against the four fixtures: card rail renders four, selecting swaps the body, the copy button carries the selected body, the banner appears only in the two unhealthy states | AC8, AC9, AC10, AC12 |
| Manual, in the app | AC11, AC13, AC16 |

## Implementation Notes

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| `copy()` with clipboard + `execCommand` fallback and toast | `app/src/app.js:422` | The copy button — do not write a second copy path |
| `go(screen)` + `hidden` sections | `app/src/app.js:433`, `app/src/index.html:13` | Adding the screen |
| Chain state already computed for Overview | `app/src-tauri/src/status.rs` | The banner; no new probe |
| `include_str!` of a text asset compiled into the binary | the Remote Script in `src/install.rs` | The four prompt files |

### Design Decisions
- **Why not MCP prompts?** The protocol has a prompts capability, but client support is
  uneven and a prompt served over MCP is invisible until the producer goes looking in a menu.
  The problem being solved is "the producer does not know what to type", which is a problem
  in front of the screen, not in the protocol. Revisit if clients converge.
- **Why markdown files rather than strings in the UI?** Three readers: the app, a
  non-Mac user reading the repository, and the conformance test. One source for all three.
- **Why four, not one big prompt?** The four states a producer opens the app in want
  different first questions. One prompt that covered all four would open by asking which of
  the four this is — which is the card rail, done worse.
- **Why does the performance prompt cap preparation at three or four sections?** Because the
  set has to start. The composing happens while the music runs; preparing ten sections first
  is the failure mode this prompt exists to prevent.

## Verification

### Manual verification steps
1. With Live open, the script installed and Claude Desktop configured: open Prompts, copy
   *Make a song from nothing*, paste it into a new conversation. Claude calls `get_context`,
   reports the set in one line, and asks what to make with defaults offered — before
   creating anything.
2. Answer with one line ("something like early Boards of Canada, 3 minutes"). It sets the
   key, previews `build_song` with `dry_run`, builds, then plays a section back and asks
   "keep or change?". Say "bass too loud" — it changes it, captures again, reports the
   numbers, and asks again. It does not announce the track is finished; it asks.
3. Copy *Prepare a set I can perform* into a set that has material. It prepares three or
   four sections, starts, and while the first section loops it announces the next one it is
   building and adds it to the setlist. Leave it running for ten minutes without typing: the
   set keeps changing and never falls silent.
4. With Live closed: the screen still opens, the banner says `get_context` will fail, and
   copy still works.
5. With no client configured: the banner links to Setup and the Setup badge shows.
6. Change a tool name in a prompt file to one that does not exist and run `cargo test` — the
   prompts suite fails and names the file and the token.

## Out of Scope
- Producer-written or edited prompts saved in the app. Ships as four read-only files; an
  editor is a separate story if anyone asks for it.
- Serving the prompts over MCP (Open Question 9's alternative).
- A "send to Claude" button, deep links into a client, or any path that opens a
  conversation for the producer.
- Localisation. English only, matching the rest of the app.
- Prompts for the `adv_` layer as a group. The four prompts reach for a raw tool by name
  where the artist's set has no word for it; a raw-layer prompt is not a producer's need.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| `mac-app-installs-runs-and-watches-the-server` | In Progress | This screen lives in that app and uses its chain state and `copy()` |
| Nothing in the server or the Remote Script | — | The story is deliberately additive-in-the-app only |

## Related Stories
- `mac-app-installs-runs-and-watches-the-server` — the app this screen joins; Setup's missing
  next step is what this story fills.
- `song-writing-feel-notes-groove-quantize-record-undo` — the tools the song prompt drives.
- `performance-records-itself-as-an-arrangement-take` — the Arrangement-take question the
  performance prompt hands back to the producer rather than answering.
- `samples-land-in-the-song` — `add_sample` and the sample folders the fourth prompt uses.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-20 | Created, after the prototype review: the performance prompt became a keep-composing loop and the song prompt a refine-until-the-producer-stops loop |
