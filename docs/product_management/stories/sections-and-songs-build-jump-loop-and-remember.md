# Sections and songs for a loop-based performer: build, jump, loop, remember

## Story
**As a** loop-based performer with Live open and Claude at my side,
**I want** to make sections of music quickly (from a description, from what is playing, as
a variation of another, or prepared before the gig), let each one loop until I say go,
chain them into a song when I want one, and jump, return and steer between them by name so
that every move lands on a phrase,
**So that** a set is sections I steer, not clips I manage, and the set I built tonight is
still my set next week.

## Details
| Field | Value |
|-------|-------|
| Status | `Draft` — the producer's answers of 2026-09-19 and the performer's review of the first draft are folded in (below); open questions 6 and 8 still want a word; the prototype transcript is not yet reviewed |
| Priority | P1 — the performer's own framing of what the performance layer is for; everything shipped so far (cues, clock, gestures) is the machinery this story puts a vocabulary on |
| Size | L — five phases; phase 1 (sections, songs, steering) is the ask and ships first; the other four are the second set's feedback and can ship in any order |
| Tracker | [#39](https://github.com/defoAI/mcp-ableton-music-maker/issues/39) — one issue, a checkbox per phase |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Prototype | [prototypes/sections-and-songs-build-jump-loop-and-remember.md](../prototypes/sections-and-songs-build-jump-loop-and-remember.md) |
| Depends on | `bar-awareness-every-response-and-gestures-as-cue-steps` (Remote Script 1.16.0, merged in #38: phrases per scene, cue gestures, `vary_clip`, `listen`, the clock on every response) |

## Context

### The Problem
The performance layer now does the hard part: cues run on Live's clock, every launch lands
on the bar, the clock is on every reply. But the performer still speaks in clips, slots and
scene indices, and the assistant still turns "go to the drop" into a state read, a scene
lookup and a cue. Two real sets (2026-09-19) said what is missing, in the performer's words:

- *Sections and songs.* A loop-based performer builds an 8-bar groove, a 16-bar break, a
  drop, and then plays them: jump, hold, loop four times, back, next. Today a section has no
  name that survives, a song has no shape, and "hold here" has no meaning.
- *Transitions between tempos and feels.* "Every time I had to rebuild rather than mix."
  A tempo ramp exists; nothing re-times a double-time drum clip when the tempo halves, and
  nothing crosses one group of tracks into another over N bars.
- *Rhythmic humanity.* "The congas sound like a drum machine." Everything written is on the
  grid; there is no groove and no humanize.
- *Sound design without leaving the flow.* Shaping a sound is many `set_device_parameter`
  calls by index; "darken the pad" is one thought.
- *Set memory.* "After end_performance the set was empty on the next session." Phrase
  lengths and the plan live in the script's memory and die with Live.

### Current State
- **Sections.** A scene has a name and, since #38, a phrase length (`set_scene`,
  `create_scene`, the `scenes` block of `build_song`), but the phrase length lives in the
  Remote Script's memory (`_scene_phrase`) and is lost when Live restarts
  ([`__init__.py`](../../../AbletonMusicMaker_Remote_Script/__init__.py), `_set_scene`). Nothing
  captures what is playing into a new row; the Live Object Model has
  `Song.capture_and_insert_scene()` ("capture the currently playing clips and insert them as
  a new scene below the selected scene", `docs/reference/ableton/live-object-model.md`),
  `duplicate_scene`, `delete_scene`. `get_context` lists scenes with clip counts.
- **Songs.** No notion of an ordered setlist or repeats anywhere. A whole song can be
  expressed as one `cue` today (fire scene X at bar N …), but the assistant must compute
  the bars by hand and nothing holds, resumes or re-plans it.
- **Steering.** `cue`, `cancel_cue`, `fire_scene {no_later_than}` and phrase times exist
  (`next_phrase`, `phrases_after`); "hold", "go", "next", "back", "loop this x4" do not.
- **Transitions.** Tempo ramps and gestures exist; `vary_clip` has `half_time` and
  `double_time` but is a tool, not a transition step; there is no crossfade between track
  groups (only the master crossfader, `set_crossfader`).
- **Groove.** The Live Object Model exposes `Song.groove_pool` (Live 11+), `Clip.groove`
  (get/set) and `Song.groove_amount`; nothing in the script or server touches them. Whether
  a groove can be *added* to the pool through the API is unknown (GroovePool is a summary
  class in the reference; grooves normally arrive by dragging from the browser).
- **Sound.** `get_device_parameters` returns every parameter's name, value, min and max;
  `set_device_parameter` takes an index. There is no vocabulary across instruments.
- **Set memory.** The server writes nothing about the set (`src/state.rs`: activity,
  sessions, library). The Live Object Model has no Save; the producer saves with Cmd+S.
  `build_song` is a one-way document → set; nothing exports a set as a document.

### Root Cause
The layer speaks Live's vocabulary (scene, slot, clip) and the performer speaks music's
(section, song, phrase). Everything a section or a song needs already exists as a primitive;
what is missing is the nouns, and a place for them to survive that is not the script's
memory.

### The model, stated once
Three rules every tool in this story obeys. They came out of the first draft's review and
are the document's spine:

1. **A section loops until told otherwise.** Holding is the default state, not an action;
   `go` is the action. Repeats are an optional pre-plan (`repeats` on a setlist entry or a
   jump); a section with no repeat count loops until `go`, `next`, `back` or `jump_to`.
2. **"Next phrase" is counted in the phrase of the section that is playing now**, from the
   bar it started on — never in the target's phrase. At bar 30 with Groove (8 bars, started
   at 17) playing and Break (16 bars) queued, "next phrase" is bar 33; Break's 16 bars start
   counting when Break fires. `next_phrase`, `phrases_after`, `go`, `next`, `back` and
   `jump_to` all resolve time this way; the reply always says the bar.
3. **Cutting a phrase short is said out loud.** Any move with `at: "next_bar"` (or a
   sub-bar time) that lands before the phrase end replies with what it cuts: "cutting 6 bars
   off Groove+Pad".

## Open Questions

Answered by the producer on 2026-09-19 (the four choices below are decided):

1. **Where does a section's material come from?** → **Three sources, all first-class:**
   Claude writes it from a description while the current section plays; it is captured from
   what is playing (Live's capture-and-insert-scene); it was prepared before the gig with
   `build_song`. Recording the performer's playing into a section stays `record_clip`
   (already shipped) and is not a fourth source in this story.
2. **How is a song defined and advanced?** → **A setlist Claude cues**: an ordered list of
   sections, each with an optional repeat count; the whole plan is one cue on the script's
   clock. Reviewed and inverted: an entry *without* `repeats` loops until `go`, so on stage
   the song is a path, not a timetable; `repeats` is the pre-written case. `hold` stops a
   running count where it is; `go` continues; `next`/`previous`/`jump_to` re-plan from
   the end of the current phrase.
3. **When does "go" land?** → **End of the current phrase** by default: the next multiple
   of the playing section's `phrase_bars` from the bar it started on, however many passes
   it has looped (rule 2 above). `at: "next_bar"` cuts the phrase short and the reply
   says by how much (rule 3).
4. **Default transition?** → **A straight cut** on the boundary; fills, drops, sweeps,
   retimes and crossfades only when asked (a `transition` object on the jump).
5. **Where are songs and sections remembered?** → **In the Live set itself.** Scene names
   carry the section name and phrase length (`Groove · 8`); the setlist is the name of one
   empty scene at the bottom (`Setlist: Intro×2 → Groove×4 → …`); Live's Save keeps it
   all and `get_context` reads it back with no server file. `export_set` writes a
   rebuildable JSON backup under the state dir on request; `import_set` rebuilds through
   `build_song`.
6. **Groove: Live's pool or note rewriting?** → **Live groove first, notes as fallback.**
   `groove_clip` assigns a pool groove to a clip where the API allows and says so;
   `humanize` and `swing_notes` rewrite notes (seeded, one undo). *Still wanted: whether a
   groove can be added to the pool by the API (`load_browser_item` of a `.agr` onto a
   clip?) needs a real-Live check before the AC is final.*
7. **The sound vocabulary?** → **A shared vocabulary for Live's own instruments** (Analog,
   Wavetable, Drift, Operator, Simpler/Sampler, Meld, and rack macros), with parameter-name
   substrings as the fallback for anything else; the same words usable in cue sweeps.
8. **Story shape?** → **One story with phases.** *Still wanted: the order of phases 2–5
   after phase 1 — the assumption below is transitions, then feel, then sound, then export.*

From the performer's review of the first draft (decided):

12. **Loudness belongs next to the clock.** Every result during a performance carries the
    master's level under the clock line (`🔊 master −3.1 dB peak this bar · Kick −6, Bass −7`
    from the meters the script already reads); the script keeps a per-section peak so a
    jump can warn when the target ran hotter than the current section by more than 3 dB.
13. **`back` undoes the last jump; `previous_section` walks the setlist.** Two verbs, not
    one: the jump history is the performer's "where was I", the setlist is the song's.
14. **A section "like Groove but…"** is a first-class source: `make_section {from:
    {"section": "Groove"}, changes: {…}}` duplicates the row (`duplicate_scene`) and applies
    per-track overrides — a `vary_clip` variation, replacement notes, or `empty`.
15. **Rack macros are a first-class sound path**, before the instrument table: most Live
    presets are racks and expose their intent as eight named macros.
16. **Groove replies describe what to listen for**, not note counts.

Assumed without asking:

9. **Section names must be unique** (they are the memory); `make_section` refuses a
   duplicate unless `replace: true`.
10. **The scene-name convention is `<name> · <bars>`** with the middle dot; a scene without
    the suffix is a section with the default 16 bars; the `Setlist:` scene is never fired
    and is skipped by "next".
11. **Scene tempo is part of a section** when set; the jump ramps to it over the outgoing
    phrase only when the `transition` says `tempo`, otherwise Live's scene tempo applies at
    the fire (a jump).

## Prototype
[prototypes/sections-and-songs-build-jump-loop-and-remember.md](../prototypes/sections-and-songs-build-jump-loop-and-remember.md)
— a set with sections, a setlist, playing and steering it (hold, go, jump, loop, back,
next), sections made on stage from what plays and from a description, a genre transition
as one jump, groove and humanize, the sound vocabulary, set memory the next day, and the
failure cases.

Not yet reviewed. What writing it changed: the setlist became a scene name rather than a
server file, because the producer wanted the set to be the memory; and steering became five
verbs on one cue (cancel and re-plan) rather than a state machine in the script.

## Tool description

```text
Sections and songs. A section is a scene row named "<name> · <bars>" (its phrase length);
a song is the setlist in the name of the 'Setlist:' scene. Both are kept by Live's own
Save; get_context lists them.

make_section {name, phrase_bars, from: "playing" | {"section": name} | clips: {track:
notes…}, changes?, after?, tempo?, replace?}: a new section — captured from what is playing
(Live's capture-and-insert-scene); a variation of another section (duplicate_scene, then
`changes` per track: a vary_clip variation name, replacement notes in any compact form, or
"empty"); or written from a clips document (create_scene + one create_clip per track).
At the end or after a named section. Refuses a duplicate name. The set keeps playing.

set_song {setlist: [{section, repeats?}]}: the song. An entry without repeats loops until
you say go. Writes the 'Setlist:' scene; reports the bars that are fixed and which
sections wait for go. add_to_song {section, after|before, repeats?}, remove_from_song
{section}.

play_song {from?}: starts the performance if needed, fires the first section, and
schedules every counted jump as one cue at phrase boundaries; at an uncounted section the
plan waits for go. go {at?}: continues from the end of the current phrase (next multiple
of the playing section's phrase length from its start bar) or on the next bar. hold: stops
a running count on the current section. next_section / previous_section walk the setlist;
back returns to the section before the last jump (the jump history); jump_to {section,
repeats?, at?, transition?} goes anywhere; the song continues after the target. Every move
replies with the plan, "cutting N bars off <section>" when a phrase is cut short, the
clock line, and the level line; a jump warns when the target section last ran more than
3 dB hotter than this one.

transition (on a jump): {tempo?, retime: {tracks, to: half_time|double_time}, crossfade:
{out, in, bars}, fill?, drop?, sweep?} — composed into cue primitives; nothing is rebuilt.

groove_clip {track, clip, groove, amount?}: a Groove Pool groove on a clip (Live's own,
non-destructive); groove_amount {value}. humanize {track, clip, timing_ms, velocity, seed}
and swing_notes {track, clip, amount, seed}: note rewrites, undo_vary restores. Replies
say what to listen for: "off-beat hats now lag by about a 64th; velocities vary ±15".

shape_sound {track, device?, cutoff, resonance, attack, decay, sustain, release, drive,
detune, width, lfo_rate, reverb, delay: value 0–1 or "±N%"}: the words resolved first
against the device's rack macros by their names (most Live presets are racks), then the
instrument table; the reply names what moved and, for a rack, which macro. Unknown device:
its parameters by name, and set_device_parameter takes parameter: "<name substring>". A cue
ramp takes {"sound": "cutoff", "track": …, "to": …}.

The level line (under the clock line on every result while performing): 🔊 master −3.1 dB
peak this bar · Kick −6 · Bass −7 — Live's meters, read by the script's tick, not audio.

export_set {name} / import_set {name}: a build_song document of the whole set (tracks,
devices by URI and name, sections with clips and notes, mixer, setlist, tempo, key) under
the state dir; import rebuilds it into an empty set.
```

## Acceptance Criteria

### Phase 1 — sections, songs, steering (the ask)
- [ ] **AC1 — Section naming.** `create_scene`, `set_scene` and `build_song`'s scenes block
      write the name as `<name> · <bars>` when a phrase length is given; the script's
      `_scene_phrase` is seeded from scene names on every state read, so phrase lengths
      survive a Live restart. `get_context`/`get_performance_state` list sections as
      `name · bars (clip count)` and never show the suffix twice.
- [ ] **AC2 — `make_section {from: "playing"}`.** New script command `capture_scene(name,
      phrase_bars, after)`: selects the row to insert below (the current scene, or `after`),
      calls `Song.capture_and_insert_scene()`, names the new scene, returns its index and
      the captured clips per track. The performance keeps playing; the new row is not
      fired.
- [ ] **AC3 — `make_section {clips}`.** `create_scene` at the end or after a section, then
      one `create_clip` per named track with any compact note form; tracks not named stay
      empty and the reply says which tracks will stop when the section fires (with the
      `keep_track_playing` hint).
- [ ] **AC4 — Unique names.** A duplicate section name is refused with the existing index;
      `replace: true` deletes the old row's clips and rewrites it in place (`delete_clip`
      per slot) so the setlist keeps pointing at the same name.
- [ ] **AC5 — `set_song`.** Validates every section exists, writes (or creates) the
      `Setlist:` scene as the last row (`Intro×2 → Groove → Break×1 …`, an entry without a
      count meaning "until go"), reports the fixed bars and the sections that wait for go.
      `add_to_song` / `remove_from_song` edit it; a running song is re-planned from the
      current position.
- [ ] **AC6 — `play_song`.** Starts a performance if none runs (with `start_performance`'s
      defaults), fires `from` (default the first entry) now, and schedules one cue whose
      steps fire each counted section at `started + Σ(repeats × phrase_bars)` bars up to
      the first uncounted entry, where the plan waits for `go`. The performance record
      gains `song: {entries, position, pass, plan_cue_id, jump_history}`.
- [ ] **AC7 — Steering.** `hold` cancels the plan cue (the section loops); `go {at}`
      continues from the end of the current phrase (rule 2) or the next bar; `next_section`
      / `previous_section` move along the setlist; `back` returns to the section before the
      last jump (from `jump_history`, so a mistaken `jump_to Drop` returns to where the
      performer was, not to the entry before Drop); `jump_to {section, repeats, at,
      transition}` goes anywhere; each cancels and re-plans with the target first, and the
      song continues after the target. Every reply lists the new plan; every launch is on
      a phrase boundary unless `at: "next_bar"`, in which case the reply says "cutting N
      bars off <section>" (rule 3).
- [ ] **AC7a — Level under the clock.** The script's tick keeps the master and per-track
      meter peaks for the current bar and per section (max while that row played); the
      `clock` envelope carries `levels: {master_peak_db, tracks: [...], section_peaks}`,
      and `run_blocking` renders the 🔊 line under the ⏱ line. `jump_to`, `next_section`,
      `go` warn ("Drop last peaked −0.8 dB, 4.2 dB hotter than Groove: pull the master or
      the drop's kick before the jump") when the target section's stored peak exceeds the
      current one by more than 3 dB; `force: true` skips the warning. Meter values are
      Live's 0–1 output meters converted to dB, labelled as such.
- [ ] **AC7b — `make_section {from: {"section": …}, changes}`.** `duplicate_scene`, then
      per track: a `vary_clip` variation name (applied to the copy), replacement notes
      (`clear` + `add_notes_to_clip`), or `"empty"` (`delete_clip`); the new row is named
      and the reply lists what differs from the source.
- [ ] **AC8 — Reading the song back.** After a Live restart, `get_context` shows the
      sections (from scene names), the setlist (parsed from the `Setlist:` scene name, with
      a clear error naming the first unreadable token), the key and tempo; `play_song`
      works with no server memory.
- [ ] **AC9 — The state readout.** `get_performance_state` and the clock line show the song
      position: `Song: Intro×2 → [Groove ×4: pass 2] → Groove+Pad×4 …`, and "next jump bar
      N" replaces the generic next-cue label.

### Phase 2 — transitions that mix
- [ ] **AC10 — `transition` on a jump.** `tempo` becomes a ramp under the outgoing phrase
      (ending at the boundary); `retime {tracks, to}` writes `vary_clip half_time |
      double_time` copies into the target section's row for the named tracks before the
      jump (seeded, named `<clip> (half_time)`), so the target fires the retimed clips;
      `crossfade {out, in, bars}` is volume ramps to 0 on `out` and from 0 to their current
      level on `in` (the `in` tracks' faders are set to 0 one tick before the boundary and
      restored after); `fill`, `drop`, `sweep` map to the existing gestures. The reply lists
      the primitives, as gestures do.
- [ ] **AC11 — `retime_clip`** as a standalone tool for the same rewrite outside a jump.

### Phase 3 — feel
- [ ] **AC12 — `groove_clip`.** Script command `set_clip_groove(track, clip, groove_name |
      index, amount)`: finds the groove in `Song.groove_pool.grooves` by name, assigns
      `Clip.groove`, sets the groove's amount if the API allows; errors name the pool's
      grooves when the name is unknown, and say that the API cannot add one (drag from the
      browser) — unless the real-Live check (open question 6) finds a way, in which case
      `groove_clip` loads it. `groove_amount` sets `Song.groove_amount`.
- [ ] **AC13 — `humanize` and `swing_notes`.** `src/variation.rs` gains `humanize
      (timing_ms, velocity, seed)` (per-note offsets from a seeded normal-ish distribution,
      timing converted to beats at the set's tempo, never moving a note across a bar line)
      and `swing (amount, seed)` (off-beat 8ths/16ths delayed by `amount` of the grid
      step); both through the `vary_clip` write path with `undo_vary`. The reply describes
      the audible change in musical units: the largest offset as a note value ("about a
      64th"), the velocity range, which beats moved ("off-beat 16ths"), never a note count.

### Phase 4 — sound
- [ ] **AC14 — The vocabulary.** `src/sound.rs`: resolution in two steps. First the
      device's rack macros by name (an Instrument or Audio Effect Rack, or a rack wrapping
      the instrument as most presets do): a macro whose name contains the word, or a known
      alias ("Cutoff"/"Filter"/"Freq" for cutoff, "Res" for resonance, "Drive"/"Sat" for
      drive, "Space"/"Verb" for reverb …), wins. Then a table `word → [candidate parameter
      names]` per device class name (Analog, Wavetable, Drift, Operator, Simpler, Sampler,
      Meld, Drum Rack pads' Simpler), resolved against `get_device_parameters` at call
      time; `shape_sound` applies several
      words in one round trip per device (the script gains `set_device_parameters(track,
      device, [{index, value}])`), accepting absolute 0–1 or relative `"±N%"` of the range.
      The reply names each parameter and its before/after in display units.
- [ ] **AC15 — Fallbacks.** An unknown device lists its parameter names; `set_device_parameter`
      accepts `parameter: "<substring>"`; a cue `ramp {"sound": word, …}` resolves the word
      the same way.

### Phase 5 — set memory
- [ ] **AC16 — `export_set`.** Reads the set (tracks, devices by name and URI where the
      library index knows them, every Session clip's notes, mixer, sends, colours,
      sections, setlist, tempo, signature, key) into a `build_song` document plus a
      `sections`/`setlist` block, written to `state_dir()/sets/<name>.json`; the reply
      says what the file holds and how to delete it.
- [ ] **AC17 — `import_set`.** Validates the document, refuses a set that is not empty
      unless `merge: true`, and rebuilds through `build_song` plus `set_song`.
- [ ] **AC18 — Privacy.** `TERMS.md` gains a "Set exports" paragraph: written only when
      asked, holds the producer's notes and names, lives under the state dir, removed by
      "Delete all local data"; `tests/library.rs` (or a new `tests/sets.rs`) pins the
      location and that nothing is written without an explicit call.

### No Regressions
- [ ] **AC19:** A set whose scene names carry no suffix behaves exactly as today (default
      16-bar phrases); every existing suite green.
- [ ] **AC20:** `cue`'s silence check and `keep_track_playing` rules apply to every jump the
      song makes; a setlist that would go silent at a boundary is refused at `set_song`
      time with the section named.
- [ ] **AC21:** Steering never adds a socket round trip beyond one state read and one cue:
      `tests/performance.rs` asserts the command lists.
- [ ] **AC22:** `export_set` never runs on its own; the activity log shows it only when
      called.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | `capture_scene`, `duplicate_section`, section-name parsing into `_scene_phrase`, meter peaks per bar and per section in the tick and in the `clock` envelope, `set_clip_groove`, `groove_amount`, `set_device_parameters` (many in one call), `Setlist:` scene handling; `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` per phase |
| `src/performance.rs` | `Song` planning (counted entries → cue steps at phrase boundaries, uncounted wait for go), steering re-plans, the jump history, the phrase rule, the "cutting N bars" line, the song and level lines in the state and clock texts, the level guard on jumps, `transition` composition |
| `src/variation.rs` | `humanize`, `swing`, `retime` |
| `src/tools.rs` | `make_section`, `set_song`, `add_to_song`, `remove_from_song`, `play_song`, `hold_section`, `go`, `next_section`, `previous_section`, `jump_to`, `retime_clip`, `groove_clip`, `groove_amount`, `humanize`, `swing_notes`, `shape_sound`, `export_set`, `import_set`; `set_device_parameter` by name; `build_song` scene-name suffix |
| `src/context.rs` | sections and the song in the readout; instructions gain the section vocabulary |
| `src/connection.rs` | `Performance.song` |
| `src/state.rs` | `sets_dir()` |
| `TERMS.md`, `README.md`, `docs/…` | AC18 and the usual matrix, architecture, snapshot |

### New
| File | Description |
|------|-------------|
| `src/song.rs` | setlist parsing and rendering (`Setlist:` scene name ⇄ entries), the plan, the cursor |
| `src/sound.rs` | the word → parameter table per instrument and the resolver |
| `tests/song.rs` | sections, setlist parsing round trip, plan bars, steering command lists, transitions' primitives |

## Remote Script compatibility
- [ ] Handlers added (Live-Python compatible; no f-strings, no type hints, no third-party imports)
- [ ] Command names in `SCRIPT_CAPABILITIES`
- [ ] `SCRIPT_VERSION` bumped once per phase (1.17.0 for phase 1)
- [ ] Commands in `tools::ALL_REMOTE_COMMANDS`
- [ ] Tool bodies call `require(live, "<command>")` first
- [ ] Live version floor stated: **Live 11+** (`capture_and_insert_scene`, `Clip.groove`,
      `Song.groove_pool` are Live 11+; the key readout Live 12)
- [ ] Timeout class: `capture_scene`, `set_clip_groove`, `set_device_parameters` modifying
      (15 s); reads unchanged

## Privacy
**One new kind of local data, only on request: set exports (phase 5).** `export_set` writes
`state_dir()/sets/<name>.json` holding the producer's tracks, clip notes, names, mixer
values and setlist — content, not just metadata — and only when the producer asks for it;
nothing exports on its own. `TERMS.md` gains a "Set exports" paragraph; "Delete all local
data" removes the folder; a test pins that no set file appears without an explicit call.
Everything else this story adds lives in the Live set (scene names) or in memory. Nothing
uploads (`tests/local_only.rs` unchanged).

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/song.rs` (new) | scene-name convention round trip; setlist ⇄ `Setlist:` scene name incl. uncounted entries and unreadable tokens; plan bars from repeats and phrase lengths, waiting at the first uncounted entry; the phrase rule (Groove 8 from 17, Break 16 queued, "next phrase" at bar 30 = 33); `play_song` command list (state, start, fire, one cue); `hold`/`go`/`next`/`previous`/`back`(history)/`jump_to` re-plans and their cue steps; "cutting N bars" on `at: next_bar`; the level guard (warn above 3 dB, `force`); `make_section` from a section with changes; `transition` expansion (tempo ramp, retime copies, crossfade ramps); duplicate-name refusal; setlist silence refusal | AC1–AC11, AC20, AC21 |
| `src/variation.rs` unit tests | humanize never crosses a bar line, is seeded; swing delays off-beats only | AC13 |
| `src/sound.rs` unit tests | word resolution against synthetic parameter lists for each instrument; relative values; unknown device fallback | AC14, AC15 |
| `tests/performance.rs` | the song line in state and clock text | AC9 |
| `tests/sets.rs` (new) | export document shape from a synthetic context; import validation; nothing written without a call; off-switch location | AC16–AC18, AC22 |
| `tests/stdio_integration.rs` | instructions mention sections and songs | — |

The script's behaviour (`capture_and_insert_scene` inserting below the *selected* scene,
groove assignment, restored phrase lengths after a Live restart) needs the manual pass
below.

## Implementation Notes

### Phases
1. Sections, songs, steering — with the naming convention and `capture_scene`.
2. Transitions: `transition` on jumps, `retime_clip`.
3. Feel: `groove_clip`, `groove_amount`, `humanize`, `swing_notes`.
4. Sound: the vocabulary, `shape_sound`, parameter by name, cue sweeps by word.
5. Set memory: `export_set`, `import_set`, `TERMS.md`.

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| One cue for a whole plan, cancel-and-re-plan to steer | `cue` / `cancel_cue`, `end_performance` | `play_song`, `hold`, `go`, `jump_to` |
| Gesture expansion into primitives on the server | `performance::expand_gesture` | `transition` |
| Seeded note transforms with one undo | `src/variation.rs`, `vary_clip` | `humanize`, `swing`, `retime` |
| Name-or-index resolution against a fresh state | `PerfState::scene_by` | sections by name; the `Setlist:` scene |
| Read parameters, then write by index | `get_device_parameters` → `set_device_parameter` | `shape_sound` |
| A document validated before the first command | `build_song` | `import_set` |

### Design Decisions
- **The set is the memory.** Scene names are visible, editable in Live, and saved by Live;
  a server file would drift from the set the moment the producer touched it in Live.
  `export_set` is a backup the producer asks for.
- **A song is one cue; steering is re-planning.** No state machine in the script: the
  script keeps running primitives it already knows, the server keeps the cursor, and a hold
  is a cancel. A Live restart loses only the cursor, which `get_context` rebuilds from the
  names.
- **Loop until told otherwise.** A performer does not know they want Groove four times
  until they are in it; counting is the pre-written case, holding is the stage.
- **"go" lands at the end of the phrase** because that is what a phrase length is for; the
  performer can always say "on the bar", and then is told what was cut.
- **Loudness travels with the clock** because transitions are where mixes clip, and a
  level the performer did not ask for is the one they needed.
- **Transitions compose primitives** (ramp, vary_clip copies, volume ramps) so a genre
  change is a cue, and every clip stays where it was.
- **Groove prefers Live's own** because it is non-destructive and audible in Live's UI;
  note rewriting is honest about being a rewrite and is undoable.
- **The sound vocabulary is a table** the producer can read in the reply, not a model
  guessing parameter indices.

## Verification
With Live 12 open, the Remote Script reinstalled, Live restarted, an empty set.
1. Build the set of prototype §1: scene names in Live read `Intro · 8` … `Drop · 8`.
2. `set_song` as in §2: a `Setlist:` scene appears at the bottom; its name is the setlist.
3. `play_song`: the intro fires; the clock line names the next jump; each section changes
   at the computed bar and on a phrase boundary; the total ends where the plan said.
4. Groove has no count in the setlist: it loops; `go` at bar 30 lands Groove+Pad at 33
   (rule 2). `jump_to Drop repeats 8`: lands at the phrase end, eight passes, then the song
   continues; `back`: returns to Groove+Pad, not to Break; `next_section {at: next_bar}`:
   lands on the next bar and the reply says how many bars it cut. Every reply carries the
   🔊 line; a jump into a section that ran 4 dB hotter warns.
5. While Groove plays, tweak a fader and mute a track, then `make_section from playing`:
   a new row appears below the current one with the playing clips; the set never stops.
6. `make_section` from a clips document after Break: the row is inserted, later scenes move
   down, the setlist still names the right sections.
7. Save the set in Live (Cmd+S), quit Live, reopen: `get_context` lists the sections with
   their phrase lengths and the song; `play_song` works.
8. A jump with `transition {tempo: 122, retime: half_time on the drums, crossfade over 8}`:
   the tempo ramps under the outgoing phrase, the target's drum clips are the half-time
   copies, the pads cross over eight bars; nothing was rebuilt.
9. `groove_clip` with a groove that is in the pool: Live's clip view shows it assigned;
   with one that is not: the reply names the pool's grooves. `humanize` on the congas: the
   notes move off the grid in Live's editor; `undo_vary` puts them back.
10. `shape_sound {cutoff: "-25%"}` on a Wavetable pad and on an Analog bass: the named
    parameters move by a quarter of their range; an unknown device lists its parameters.
11. `export_set`, empty the set, `import_set`: the set is back with sections and setlist.
12. Delete all local data in the app: the sets folder is gone; the Live set is untouched.

## Out of Scope
- Recording the performer into a section (that is `record_clip`, shipped).
- Live's own scene follow actions (the plan lives in one cue instead, so re-planning is one
  call).
- Arrangement-view songs (this is a Session-view performer).
- Saving the Live set from the API (there is no such call; Live's Save does it).
- Automatic transitions the performer did not ask for.
- A groove designer; only assignment of pool grooves and note-level swing/humanize.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| `bar-awareness-every-response-and-gestures-as-cue-steps` (#38) | Merged | phrases, gestures, `vary_clip`, the clock |
| `Song.capture_and_insert_scene`, `Clip.groove`, `Song.groove_pool`, `Song.groove_amount` | Live 11+ (local LOM copy) | verify groove assignment and pool additions on Live 12.4.6 |
| Open questions 3, 6, 8 | Waiting on the producer | ACs may move |

## Related Stories
- `perform-live-build-launch-and-transition-on-the-bar` — the performance layer.
- `bar-awareness-every-response-and-gestures-as-cue-steps` — phrases, gestures, the clock.
- `capture-the-mix-through-resampling` — `listen`'s recording path.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from the performer's ask ("quickly create sections of music, jump to them, loop") and the second set's feedback (transitions between feels, rhythmic humanity, sound design in the flow, set memory); shaped by the producer's answers on sources, setlist, "go" timing, transitions, memory, groove, vocabulary and story shape |
| 2026-09-19 | Reviewed by the performer: the loop model inverted (hold is the state, go the action, repeats optional), the phrase rule stated once, "cutting N bars" confirmations, the level line under the clock and a 3 dB guard on jumps, `back` as jump-history undo, "like Groove but…" sections, rack macros first, audible groove replies |
