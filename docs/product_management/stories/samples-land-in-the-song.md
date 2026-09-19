<!-- Required sections: Story · Details · Context · Open Questions · Prototype ·
     Acceptance Criteria (+ No Regressions) · Affected Files · Remote Script compatibility ·
     Privacy · Test Coverage · Implementation Notes · Verification · Out of Scope ·
     Dependencies · Related Stories · Changelog. -->

# Samples Land in the Song

## Story
**As a** producer using Claude Desktop / Claude Code / Cursor with Live open,
**I want** Claude to find the audio I already own and put it into my song — a break into the
Verse, a riser at bar 25, a crash on the downbeat,
**So that** a set is not limited to what a MIDI instrument can play, and I do not have to
stop, find the file myself and drag it in.

## Details
| Field | Value |
|-------|-------|
| Status | `Done` |
| Priority | P1 — the set is half a set without audio; every producer's own library is the part Claude could not see |
| Size | L |
| Tracker | — |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |

## Context

### The Problem
Claude can write MIDI into a Live set and cannot bring in a single sample. A producer who
says "put a break under the verse" gets a refusal or a drum-rack approximation, and the
6,350 audio files sitting in `~/Music/Ableton` on the machine this was written on are
invisible to the server. The one tool that places audio,
`adv_create_audio_clip`, needs an absolute path typed by hand, only writes into a Session
slot, and leaves the clip unwarped at whatever tempo the file was recorded at.

### Current State
- `adv_create_audio_clip` sends `create_audio_clip` with `{track_index, clip_index, path}`
  ([`src/tools.rs:1531`](../../../src/tools.rs)), which calls Live's
  `ClipSlot.create_audio_clip` ([Remote Script `_create_audio_clip`, `__init__.py:1084`](../../../AbletonMusicMaker_Remote_Script/__init__.py)).
  It is an `adv_` tool: not in `CORE_TOOLS` ([`src/tools.rs:151`](../../../src/tools.rs)).
- The library index walks five categories only — `WALKED_CATEGORIES` is
  `all, instruments, sounds, drums, audio_effects, midi_effects`
  ([`src/library.rs:243`](../../../src/library.rs)) — and the Remote Script's `"all"` covers
  the same five ([`__init__.py:2279`](../../../AbletonMusicMaker_Remote_Script/__init__.py)).
  `search_browser` accepts `category: "samples"` already
  ([`src/tools.rs:623`](../../../src/tools.rs)) and falls through to a live browser walk,
  which returns **names, browser paths and URIs — never a filesystem path**
  ([`_browser_indexer`, `__init__.py:2282`](../../../AbletonMusicMaker_Remote_Script/__init__.py)).
  So the model can search for a sample and still not have what
  `create_audio_clip` requires.
- Nothing in the server or the script reads a directory. The script's only filesystem call is
  one `os.path.isfile` ([`__init__.py:43`](../../../AbletonMusicMaker_Remote_Script/__init__.py)).
- `import_set` prints "Not rebuilt: N audio clips (…); create_audio_clip places them from
  their files" ([`src/sets.rs:505`](../../../src/sets.rs)) — an instruction to the model,
  because the tool cannot do it itself.
- `Track.create_audio_clip(file_path, position)` — audio straight into the Arrangement at a
  beat position — is in the Live Object Model
  ([`docs/reference/ableton/live-object-model.md:1005`](../../reference/ableton/live-object-model.md))
  and in the installed Live's own LOM tables, and **is not used anywhere in this repo**.

### Root Cause
Discovery, not placement. The browser is the only library the server knows, and Live's
browser does not tell a client where a file is on disk.

## Open Questions

**1. Which samples should the MCP be able to reach?** → **Answered (producer, 2026-09-19):
both.** Live's browser *and* folders the producer points at. Browser hits are findable and
can be placed into a section; folder hits carry a path and can go anywhere.

**2. Where does a sample land?** → **Answered: both, chosen by parameter.** `section` puts it
in that section's Session row so it plays as part of the song; `at_bar` puts it straight into
the Arrangement.

**3. Fitted to the song, or placed raw?** → **Answered: fit by default.** Warping on so the
clip follows the set tempo, loop set to a whole number of bars, named after the file, one
undo step. `fit: false` places it exactly as Live's own drag-and-drop would.

**4. Playable one-shots (a kick on a Drum Rack pad, then MIDI)?** → **Answered: out of scope
for this story.** Clips only. See Out of Scope.

**5. Do Live's Places (`browser.user_folders`) expose a filesystem path?** → **Still open,
and it does not block.** Probed against Live 12.4.6 on 2026-09-19: the set under test had no
Places, so `user_folders` was empty and neither answer was observed. The probe ships:
`_user_folder_path` reads `path`, `file_path`, `absolute_path`, `folder`, `directory` and a
`file://`-shaped `uri` off each item and believes only an existing directory; a Place whose
folder Live will not name is listed under `places_without_path` and the producer is told to
add it by path. Worth re-probing on a set with Places in the browser sidebar.

**5a. Where does the walk run — the Remote Script or the server?** → **Answered by
measurement, after the first build was wrong.** The first implementation walked the folders
inside the Remote Script, off Live's main thread, so a scan cost the music nothing. Against
real Live it managed **about two files a second** (9,904 files would have taken over an
hour; `main_ms` was 0.3, so the cost was the interpreter, not Live's main thread). The same
code in a normal Python process did 3,900 a second. The walk moved to the server, which did
the same 9,904 files in **2.0 s**. The Remote Script keeps only `list_sample_folders`, which
is what Live alone knows. Consequence, stated in the tool text and in TERMS: sample *search*
needs the server on the machine Live runs on, which is how the Mac app ships; in a container
the folders come back unreadable, the reply says so, and Live's browser answers instead.

**6. Where is the Core Library on disk?** → **Answered by inspection (2026-09-19):**
`/Applications/Ableton Live 12 Suite.app/Contents/App-Resources/Core Library/Samples`,
4,914 WAV/AIFF files, derived in the script from `sys.executable`. Packs are
`~/Music/Ableton/Factory Packs/<pack>/Samples`, derived from the User Library, which is the
script's own grandparent directory.

**7. A sample that exists only in Live's browser, asked for at a bar?** → **Refuse with the
way round** (place it in a section first, which yields `Clip.file_path`, then `arrange`).
Rejected: silently creating and deleting a temporary Session clip to learn the path — a
clip appearing and vanishing in the producer's set is worse than a sentence.

**8. Its own index file, or the existing library index?** → **Its own file, the same off
switch.** `library/samples-<key>.json` beside `library/<key>.json`, both governed by
`ABLETON_MCP_LIBRARY_INDEX`. One switch to explain, one TERMS section to change.

**9. May `add_sample` create the track?** → **Yes**, an audio track named after the sample
when `track` is not given or names nothing — the same courtesy `build_song` does. A `track`
that names an existing *MIDI* track is refused, not silently replaced.

**9a. Does a sample loop?** → **The target decides, not a guess about the audio.** The first
build looped anything a bar or longer, which warped a crash one-shot into a 1-bar loop at bar
5 (seen on real Live). A Session row plays through its section, so a row loops; a bar in the
Arrangement is one hit, so it does not, unless `bars` asks for a loop of a set length.

**10. Which file types?** → `.wav .wave .aif .aiff .aifc .flac .mp3 .m4a .ogg` for the
index. The tool does not validate: Live's own error text is returned when Live will not read
a file, because Live is the authority on what it can decode.

## Prototype

[prototypes/samples-land-in-the-song.md](../prototypes/samples-land-in-the-song.md) — a
transcript: finding a break, into a section, at a bar, a forced length, adding a folder,
transposing, and the seven refusals.

What the review changed: the transcript's own "what the review should decide" list became
questions 7 and 9 above, and the reply text lost a "round trips" line (decision 0006: the
description says what the artist gets).

## Tool description

`add_sample` (core, Build):

```text
Put audio into the song: a sample from your folders or Live's browser becomes a clip in a
section, or lands in the Arrangement at a bar. `sample` is plain words to search for
("break 90", "vinyl crackle"), an absolute file path, or a browser URI. Give `section` (a
Session row, so it plays with the song) or `at_bar` (Live's 1-based bar in the
Arrangement), not both. `track` is an audio track by name or index; leave it out and an
audio track is made, named after the sample. By default the clip is fitted to the song:
warping on so it follows the tempo, the loop set to a whole number of bars when Live heard
one, named after the file — one undo step in Live. `bars` forces a loop length, `transpose`
shifts it in semitones, `fit: false` places it exactly as dragging the file in would.
The file is referenced where it is: nothing is copied, moved or uploaded. Needs Live 12
(Live's own API for placing audio from a file); on Live 11 it says so. Refuses a MIDI
track, both targets at once, a section that does not exist, and a browser sample asked for
at a bar (Live only reveals a browser item's file once it is a clip — put it in a section
first). Find samples with search_browser category "samples"; add a folder with
adv_sample_folders.
```

`adv_sample_folders` (advanced):

```text
The folders Claude looks in for samples. action "list" (default) shows them with the file
count; "add" with a path adds one and indexes it; "remove" takes one out; "refresh"
re-scans. Live's own folders — the Core Library, your Packs, the User Library, this
project — are always included and need no adding. The list is kept in
~/.ableton-music-maker/sample_folders.json and holds paths only, never audio.
```

## Acceptance Criteria

### Finding
- [x] **AC1:** `search_browser` with `category: "samples"` searches an index of audio files
      under Live's own folders plus any the producer added, and prints each hit's absolute
      path and duration. The index is built on first use, never at start-up (9,904 files in
      2.0 s on Live 12.4.6; 8 ms a search afterwards).
- [x] **AC2:** When the file index has no hit (or no root exists), the search falls back to
      Live's browser walk as it does today, and those hits print a `uri:` instead of a path.
- [x] **AC3:** `adv_sample_folders` lists, adds, removes and refreshes; `add` reports the
      file count it found; a path that is not a directory is refused.
- [x] **AC4:** A duration is read from the WAV/AIFF header (`audio::header_seconds`, at most
      4 KB), never by decoding the file, and is absent rather than wrong for other formats.

### Landing
- [x] **AC5:** `add_sample` with `section` writes the clip into that section's row on the
      named track, creating an audio track named after the sample when `track` is absent.
- [x] **AC6:** `add_sample` with `at_bar` places the clip in the Arrangement at that bar
      through `Track.create_audio_clip`, and the reply says which bars it covers.
- [x] **AC7:** `sample` resolves plain words (best hit, runners-up named in the reply), an
      absolute path (used as given, no index needed) and a browser URI (loaded through
      Live's browser into the chosen slot, then its file path read back and remembered).
- [x] **AC8:** The whole placement — create, warp, loop, transpose, name — is **one** Remote
      Script command, so Cmd+Z in Live undoes the sample, not the warp setting.

### Fitting
- [x] **AC9:** A sample about a bar long or longer is warped, and its loop is set to the
      nearest whole bar count when Live's own warp landed within 10% of one; `bars` forces a
      length; the reply states what Live ended up with, read back from the clip.
- [x] **AC10:** A sample shorter than about a bar in a row, and **anything placed at a bar**,
      is left as Live loaded it and does not loop; the reply says it is one hit and how to ask
      for a loop instead.
- [x] **AC11:** When the warped length is nowhere near a whole bar and no `bars` was given,
      the markers are left alone and the reply says so rather than trimming the material.
- [x] **AC12:** `transpose` sets Live's clip Transpose in semitones (−48…48); out of range
      is refused.

### Refusals and compatibility
- [x] **AC13:** Both `section` and `at_bar` → refused naming both. Neither → refused naming
      both ways round.
- [x] **AC14:** A named section that does not exist → refused, listing the sections there are.
- [x] **AC15:** A `track` that is a MIDI track → refused, naming it, suggesting another.
- [x] **AC16:** No hit for the words → refused, saying how many files are indexed and in how
      many folders, and how to add one.
- [x] **AC17:** A browser URI with `at_bar` → refused with the way round (section first).
- [x] **AC18:** On a Live without the API the script says which Live version is needed; the
      tool passes Live's own words through for a file Live cannot read (verified: "The
      provided path does not appear to point to a valid audio file").
- [x] **AC18a:** A folder the server cannot read is named as such, and Live's browser answers
      the search instead — the container case.

### No Regressions
- [x] **AC19:** `search_browser` for instruments, sounds, drums and effects behaves exactly
      as before — same walk, same ranking, same text, no sample scan triggered.
- [x] **AC20:** `adv_create_audio_clip` keeps working unchanged.
- [x] **AC21:** The activity log, the upload gate and the library index defaults are
      untouched; `tests/local_only.rs` and `tests/activity.rs` pass unchanged but for the
      served-tool count.
- [x] **AC22:** Nothing new is written unless the producer adds a folder or a sample search
      happens; `export_set` is still the only tool that creates `sets/`.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | `place_sample` and `list_sample_folders`; `SCRIPT_VERSION` 1.26.0; two capabilities; `place_sample` in `MUTATING_COMMANDS` and its 60 s timeout |
| `src/audio.rs` | `header_seconds`: a length from the first 4 KB of a WAV or AIFF, no decoding |
| `src/tools.rs` | `add_sample` / `adv_sample_folders` bindings; `samples` in `ALL_REMOTE_COMMANDS`; `add_sample` in `CORE_TOOLS`; `search_browser` body searches the sample index for `category: "samples"` |
| `src/connection.rs` | timeouts for the three commands; `place_sample` modifying |
| `src/library.rs` | `WALKED_CATEGORIES` unchanged; `search` reused by the sample index |
| `src/state.rs` | `sample_folders_file()` |
| `src/context.rs` | one clause in `INSTRUCTIONS` and the `FOOTER` |
| `src/lib.rs` | `pub mod samples` and its module-map line |
| `TERMS.md` | the library-index section covers sample names and paths; a row for `sample_folders.json` |
| `docs/architecture/overview.md`, `docs/technical/feature-matrix.md`, `docs/facts/source-of-truth.md`, `README.md`, `CLAUDE.md` | the new commands, tools, counts and script version |

### New
| File | Description |
|------|-------------|
| `src/samples.rs` | the sample index (Live names the folders, this walks them; on disk beside the library index, searched locally), the folder list, and the `add_sample` / `sample_folders` bodies |
| `tests/samples.rs` | the suite for this story |

## Remote Script compatibility

- [x] Handlers added — no f-strings, no type hints, no third-party imports (`os`, `struct`,
      `sys`, `time` only, all already imported or stdlib)
- [x] `place_sample`, `list_sample_folders` in `SCRIPT_CAPABILITIES` (91 commands)
- [x] `SCRIPT_VERSION` 1.25.0 → 1.26.0
- [x] All three in `tools::ALL_REMOTE_COMMANDS`
- [x] Every body calls `require(live, …)` first
- [x] Live version floor: placing audio from a file needs Live 12 —
      `ClipSlot.create_audio_clip` (the script's existing docstring says 12.0.5 or newer) and
      `Track.create_audio_clip` (documented in the Live 12.1 LOM, present in the installed
      Live 12.4.6). The script checks `hasattr` on the object it is about to use and names
      the version; finding samples needs no Live version at all
- [x] Timeouts: `place_sample` 60 s in the script / 65 s in `connection.rs` (an import of a
      large file is Live's own work), `list_sample_folders` the 10 s read default
- [x] `place_sample` is in `MUTATING_COMMANDS`, so Live opens one undo step around the whole
      placement
- [x] No command walks the filesystem: the server does that (see open question 5a), so
      nothing about finding a sample can be heard

## Privacy

Two new things are stored, both paths, never audio:

| What | Where | Default | Off switch |
|---|---|---|---|
| **Sample index** — the name, folder, absolute path, extension and duration of each audio file in the folders Claude looks in | `~/.ableton-music-maker/library/samples-<key>.json` | Built on the first sample search, not at start-up | `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory, as for the browser index |
| **Sample folders** — the paths the producer added | `~/.ableton-music-maker/sample_folders.json` | **Only when you ask** (`adv_sample_folders add`) | Delete the file, or "Delete all local data" |

- No audio is read by the server. The scan runs in the Remote Script; the server never opens
  a sample. (`src/audio.rs` still reads captures only — that is unchanged.)
- No audio is copied or moved. A placed clip references the file where it lies, exactly as
  Live's own drag-and-drop does.
- Nothing uploads; `tests/local_only.rs` is unchanged and still the policy.
- `TERMS.md` gains the two rows above and a sentence in **Library index**.
- Pinned by `tests/samples.rs`: `ABLETON_MCP_LIBRARY_INDEX=false` writes no sample index, and
  no file is written until a folder is added.

## Test Coverage

| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/samples.rs` (new) | the whole story, 12 tests: both targets, the three `sample` forms, the walk over a real tree of real WAV/AIFF headers, fitting and its six outcomes, every refusal, the folder actions, the container case, the index on disk and its off switch | AC1–AC18a, AC22 |
| `src/tools.rs` unit tests | tool count 99 → 101; `ALL_REMOTE_COMMANDS` cross-check picks up the two commands | AC21 |
| `src/samples.rs` unit tests | 7 tests on the pure logic: the ranking, a length only when the header gave one, bar wording, path/URI/words, the index key, the status line, and the walk over a real tree (audio only, hidden and non-audio skipped, an unreadable folder named) | AC1, AC4, AC7, AC9 |
| `tests/artist.rs` | `add_sample` is served under its own name, `sample_folders` as `adv_sample_folders` | AC5 |
| `tests/local_only.rs` | served-tool count 99 → 101 | AC21 |
| `tests/orchestration.rs` | `search_browser` for instruments is unchanged and triggers no scan | AC19 |
| `scripts/check-docs-facts.sh` | snapshot rows for tools, commands and the script version | — |

## Implementation Notes

### Patterns to Follow
- `src/library.rs` is the model for the index: page from the script, merge in memory, one
  JSON file per library key under the state dir, search locally. `samples::search` reuses
  `library::rank`-shaped ranking (every word must appear in the name or the folder).
- `find_items` (`src/tools.rs:2657`) is the model for resolution: "plain words are a search;
  a URI carries a ':'" — extended with "a path starts with a separator".
- `_place_clips` (`__init__.py:1865`) is the model for the script handler: resolve, act,
  `yield Done({...})` with everything the reply needs.

### Design Decisions
- **The script is the filesystem's eyes.** The server never scans, for two reasons: the
  Docker variant's filesystem is not the producer's, and the paths Live needs are host paths.
  One consequence worth keeping: the scan costs Live nothing, because it never touches Live.
- **Read back, then report.** Live's warp behaviour on import depends on the file and on the
  producer's Auto-Warp preference, and the API cannot time-stretch without rewriting warp
  markers. So `place_sample` sets what it can, reads `looping`, `loop_end`, `start_time` and
  `end_time` back off the clip, and the reply states what Live actually did — never what was
  asked for.
- **Measure with `sample_length / sample_rate`.** `Clip.length` "makes no sense for unwarped
  audio clips" (LOM), and an unwarped clip's loop markers are in seconds. Frames over rate is
  true in both states, so the one-shot decision is made on it.
- **Lazily, in the foreground, once.** The first sample search builds the index with a
  4 s-per-page budget and says how long it took; later searches are instant. A cold cache
  never delays a call that has nothing to do with samples.

## Verification

### What was tested, and how
`cargo test` (17 suites, 12 of them this story's), `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --all`, `scripts/check-docs-facts.sh`, `python3 -m py_compile` on the Remote Script
— and **the whole thing end to end against Ableton Live 12.4.6** with Remote Script 1.26.0
loaded, driven through the real server over stdio by a throwaway MCP client.

What real Live showed, from the activity log in a scratch state dir:

| | Measured |
|---|---|
| The folders Live named | Core Library (inside the application, from `sys.executable`), Factory Packs and the User Library's Samples (from the script's own folder). `places_without_path` empty — that set had no Places |
| The walk | **9,904 files in 2.0 s**, `main_ms` 1.3 — and 8 ms for every search after it, with no round trip at all |
| A loop into a Session row | `Break 90s Vinyl 88 bpm` (5.4 s, an AIFF): warped, looping 2 bars. Read back: `warping: true, looping: true, loop_start: 0, loop_end: 8, end_marker: 8`, and Live's own warp markers at 0 and 5.449 s — Live heard 88 bpm and the snap kept its 2 bars |
| A one-shot at a bar | `Crash LD` (1.3 s): unwarped, not looping, covering bar 5 to bar 5.65 — `start_time` 16, `end_time` 18.59 |
| `bars: 4` at bar 9 | the clip's loop is 4 bars and the timeline shows 2, because Live gives an Arrangement clip the length of its material and `Clip.end_time` is read-only. The reply says exactly that |
| `transpose: 3` | `pitch_coarse: 3` on the clip; `name` overrode the file's name |
| Live's main thread | 1–46 ms for a placement (median ~5 ms), ~100 ms when `add_sample` also makes the track, which is Live's own work and already true of `create_tracks`. Well inside the 50 ms the reply warns above while music plays |
| Refusals, from Live itself | `'1-MIDI' is a MIDI track; a sample needs an audio track` · `slot 0 on 'Samples Test' already holds 'Break 90s Vinyl 88 bpm'` · `The provided path does not appear to point to a valid audio file` (Live's own words for an `.opus` file) |
| `batch` | two `add_sample` steps in one call, 40.5 ms of main thread over four slices |

Three defects were found by that run and fixed: the crash looped at a bar (the loop rule now
follows the target), "1 bars", and a search for "crash" answering with "Kick Crash Combo"
(the ranking now prefers a name that leads with the word). The test track was deleted
afterwards; the set is as it was.

### Not covered by the suite
Whether `browser.user_folders` exposes a path (open question 5 — no Places in the set under
test). Whether Live's Auto-Warp lands a given file on a whole bar count: it is read back
rather than assumed, which is the point.

### Manual verification steps
1. Live 12 open with Remote Script 1.26.0 loaded (`ableton-music-maker --check` says
   `up_to_date: true`).
2. "find me a drum break" → paths and lengths, and how many files in how many folders.
3. "put it in the Verse" → a clip in that row, warped, looping whole bars; Cmd+Z removes it
   in one step.
4. "crash at bar 33 on FX" → one hit at bar 33, not looping.
5. `adv_sample_folders add` a folder of your own → its files appear in the next search.
6. The activity log: a placement's `main_ms` in single figures.

## Out of Scope
- **Samples on Drum Rack pads and in Simpler** (open question 4) — a follow-up story. It
  needs Live's browser hotswap target, not a file path, and should be verified against a
  running Live first.
- **Time-stretching a loop onto the grid** by rewriting warp markers (`move_warp_marker`).
  The story warps and reports; it never claims to stretch.
- **Making an Arrangement clip longer than its material.** `Clip.end_time` is read-only
  (LOM: "get, observe"), verified on 12.4.6; `bars` sets the clip's loop and the reply says
  what the timeline shows. `arrange repeat` fills the rest.
- **Detecting a sample's key or tempo.** `src/variation.rs` already finds the key of a
  capture; pointing that at a sample is a separate, small story.
- **Clip gain, warp modes and slicing.** `shape_sound` is the place for sound, and neither is
  needed to get audio into a song.
- **`import_set` rebuilding audio clips** now that a one-call placement exists — a follow-up,
  small: the export already stores each audio clip's `file_path` (`src/sets.rs:70`).
- **Copying samples into the project.** Live's API has no Collect All and Save; the clip
  references the file where it is, and the reply says so.

## Dependencies
- Live 12 for placing (both `create_audio_clip` functions). Finding needs nothing.
- Remote Script 1.26.0.

## Related Stories
- [performance-records-itself-as-an-arrangement-take](performance-records-itself-as-an-arrangement-take.md)
  — the other writer into the Arrangement; `add_sample` with `at_bar` uses the same bar
  vocabulary.
- [notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix](notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md)
  — the reply conventions this story follows.

## Changelog
- 2026-09-19 — Written after the API investigation (Live 12.4.6's own LOM tables and the
  local LOM copy), the four producer answers, and the prototype transcript. Implemented and
  verified against real Live the same day: Remote Script 1.26.0, `add_sample`,
  `adv_sample_folders`, `src/samples.rs`, `audio::header_seconds`, `tests/samples.rs`. The
  walk moved from the Remote Script to the server after measuring two files a second inside
  Live (question 5a), and the loop rule moved from guessing at the audio to following the
  target (question 9a).
