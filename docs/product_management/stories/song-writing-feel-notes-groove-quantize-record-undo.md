# Song-writing feel: expressive notes, groove, quantize, capture, record, sample edits and undo

## Story
**As a** producer writing a track with Live open and Claude at my side,
**I want** the notes Claude writes to breathe (chance, velocity range, a groove from my
pool, a partial quantize), the parts I play in to be kept and tidied (Capture MIDI, real
Arrangement takes, overdubbing on a looping clip), my samples tuned and warped from the
conversation, and "undo that" to mean Live's own undo,
**So that** what comes out of the conversation sounds played rather than programmed, and
I can let Claude touch my set because every move is one undo away.

## Details
| Field | Value |
|-------|-------|
| Status | `Draft` — open questions 1, 2 and 6 want a real-Live probe or the producer's word; the prototype transcript is not yet reviewed |
| Priority | P1 — the gap analysis of 2026-09-19 ranked these eight surfaces first: every other song-writing feature (variations, sections, transitions) writes straight-grid, fixed-velocity notes until this lands, and no tool today can undo anything but its own last variation |
| Size | L — three phases; phase 1 (feel and undo) is the ask and ships first; phases 2 and 3 can ship in either order |
| Tracker | to be filed — one issue, a checkbox per phase; [#30](https://github.com/defoAI/mcp-ableton-music-maker/issues/30) (groove and swing per clip) is closed by phase 1 |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Prototype | [prototypes/song-writing-feel-notes-groove-quantize-record-undo.md](../prototypes/song-writing-feel-notes-groove-quantize-record-undo.md) |
| Baseline | 75 tools, 75 Remote Script commands (verified 2026-09-19 against `src/tools.rs` and `SCRIPT_CAPABILITIES`); Remote Script 1.16.0 on `main` (#38); the working tree on `feat/39-sections-and-songs` was at 86 tools and Remote Script 1.17.0 and still moving on 2026-09-19, so this story bumps from whatever `main` holds when it lands |

## Context

### The Problem
Everything the server writes into a MIDI clip is a plain note: pitch, start, length,
velocity, mute. A producer hears that immediately. Live 11 gave every note a chance and a
velocity range, Live has had a groove pool and a partial quantize for a decade, and a
producer's first instinct after a loose take is Capture MIDI or a real take with the click
on. None of that is reachable from the conversation. Nor is Live's undo: after Claude edits
a clip, the only way back is `undo_vary`, which knows one variation on one clip.

The result is that the assistant is trusted with sketches, not songs. The producer plays
the part themselves, tidies it in Live, and comes back to Claude for arrangement work.

### Current State
Line numbers are `main` at 541d701 (the links pin that commit); the working tree on
`feat/39-sections-and-songs` is moving under the sections work.

- `add_notes_to_clip` builds `(pitch, start, duration, velocity, mute)` tuples and calls
  `clip.set_notes`
  ([`__init__.py:1026-1037`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/AbletonMusicMaker_Remote_Script/__init__.py#L1026-L1037)).
  Anything else in a note object reaches the script (the `Note` struct flattens unknown
  keys into `extra`, [`src/tools.rs:320-334`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/src/tools.rs#L320-L334)) and is
  dropped there in silence.
- Reading is already extended: `_notes_from_clip` uses `get_notes_extended` and returns
  `probability`, `velocity_deviation`, `release_velocity` and `note_id` when Live has them
  ([`__init__.py:4290-4342`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/AbletonMusicMaker_Remote_Script/__init__.py#L4290-L4342)).
  The ids are read and never used.
- Changing one note means `clear_notes_from_clip` plus a full rewrite: `vary_clip` and
  `undo_vary` do exactly that through `write_notes`
  ([`src/tools.rs:3391-3403`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/src/tools.rs#L3391-L3403)). Every such rewrite
  gives every note a new id and drops any per-note expression the producer set by hand.
- No command touches `Song.groove_pool`, `Clip.groove`, `Song.groove_amount`,
  `Song.swing_amount`, `Clip.quantize` or `Song.midi_recording_quantization`; the grep
  for those names in the script is empty. Issue #30 records the ask.
- Recording exists in two shapes only: a fixed-length Session clip (`record_clip`,
  [`__init__.py:3488-3537`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/AbletonMusicMaker_Remote_Script/__init__.py#L3488-L3537))
  and resampling the master (`capture_mix`). `Song.record_mode`, `session_record`,
  `overdub`, `arrangement_overdub`, `metronome`, `punch_in/out` and `capture_midi` are
  not sent by any command. Input routing and monitoring are set in one place, the Capture
  track ([`__init__.py:2092-2123`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/AbletonMusicMaker_Remote_Script/__init__.py#L2092-L2123)).
- Audio clip properties are read-only: `_serialize_clip_common` reports `gain`,
  `pitch_coarse`, `pitch_fine`, `warp_mode`, `warping` and the warp markers
  ([`__init__.py:4486-4507`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/AbletonMusicMaker_Remote_Script/__init__.py#L4486-L4507));
  nothing sets them.
- Undo is the server's own one-level map keyed by (track, slot), filled only by an
  in-place `vary_clip` ([`src/tools.rs:3489-3510`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/src/tools.rs#L3489-L3510)).
  `Song.undo` is never called.

### Root Cause
The Remote Script was written against Live 10's note API and the tool layer grew
arrangement, mixing and performance features on top of it without revisiting the note
write path or the transport's recording controls. Nothing forced the question until the
performance layer made the assistant's output audible in real time.

## Open Questions

Proposed answers are marked *(proposed)*; they become decisions when the producer
confirms or the real-Live probe answers.

1. **Follow actions — are they in the API at all?** The gap analysis listed them; the
   local LOM copy
   ([live-object-model.md](../../reference/ableton/live-object-model.md)) has no
   `follow_action` member on `Clip` or `Scene`, only the manual chapter describes them.
   *(proposed)* A five-minute probe on real Live (`[a for a in dir(clip) if "follow" in a]`
   in the script's log) decides. If absent, this story does not mention them again and the
   `sections-and-songs` cue plan is the way sections chain; if present, a follow-up story.
   **Not in scope of any AC below until the probe says otherwise.**
2. **Can the API add a groove to the pool?** The LOM exposes `Song.groove_pool.grooves`
   and `Clip.groove` (get/set) but no "add"; a groove reaches the pool when a clip uses it
   or the producer drags it there. *(proposed)* Probe `load_browser_item` with a groove
   file from the browser's Grooves folder while a clip is selected. Until the probe passes,
   `set_groove` assigns pool grooves only and its empty-pool error says what to drag. This
   is the same question as open question 6 of `sections-and-songs`.
3. **Extended notes on Live 10 — refuse or degrade?** *(proposed)* Degrade and say so: write
   plain notes and list what was dropped (prototype scene 1a). Refusing would make the
   default drum grammar fail on Live 10 for a feature the producer may not have asked for.
4. **How does the producer say which notes to edit?** *(proposed)* Both a `note_ids` list
   (for the model, which has just read the clip) and a `select` filter (pitch or pitches,
   beat range, velocity range, `off_grid` step, `every`/`offset`), applied server-side to
   a fresh `get_clip_notes` read. Filters are first-class because the producer speaks in
   filters ("the snare on the 4", "every second kick").
5. **Relative values in `edit_notes.set`?** *(proposed)* A number is absolute, a string
   with a sign (`"-20"`, `"+0.02"`) is relative. The review may prefer separate
   `velocity_delta` fields; the transcript uses the string form.
6. **Capture MIDI changed my tempo — keep it?** Live sets the song tempo from the played
   material when the transport was stopped. *(proposed)* Keep Live's behaviour and say so
   in the reply; `keep_tempo: true` restores the previous tempo after the capture. Needs
   the producer's word: some want the detected tempo every time.
7. **Who stops an Arrangement recording at `bars`?** *(proposed)* The script's clock, which
   already lands `record_clip` and cues on the bar; the server cannot. Without `bars` the
   recording runs until `record_arrangement {stop: true}`.
8. **Gain in dB.** `Clip.gain` is 0–1 with no documented curve; `gain_display_string`
   reports dB after a set. *(proposed)* The script searches: set, read the display string,
   bisect until within 0.1 dB, at most 8 steps, inside one command. Absolute `gain` 0–1
   is also accepted for callers who know the curve.
9. **Undo during a performance?** *(proposed)* Refused with the reason unless `force: true`
   (a cue may depend on the clip being undone); same shape as the other performance guards.
10. **Should `undo` know how many Live steps the last server tool made?** *(proposed)* No.
    Live's history is not one step per command (some commands make several steps, reads
    make none). The reply says how many modifying commands the last tool sent (from the
    activity record already kept per call) and lets the producer choose `steps`.
11. **`undo_vary` — keep it?** *(proposed)* Keep. It restores a clip's notes by content,
    which survives Live's history and a reconnect; `undo` is Live's own. The instructions
    say which is which.

Assumed without asking:

12. Chance in the step grammar is `?` (a note played with `chance` at `chance_velocity`);
    per-note fields in `notes` objects and as a sixth and seventh csv column
    (`pitch,start,duration,velocity,mute,probability,velocity_deviation`).
13. `quantize_clip` grids are Live's record-quantization grids, spelled as Live's labels
    (`1/4 … 1/32`, `1/8T`, `1/16+T`); `swing` on the same call sets `Song.swing_amount`,
    which Live applies to `Clip.quantize` and to record quantize.
14. `set_recording_options` reports `count_in_duration` and never sets it: the LOM marks it
    get/observe only.

## Prototype

[prototypes/song-writing-feel-notes-groove-quantize-record-undo.md](../prototypes/song-writing-feel-notes-groove-quantize-record-undo.md)
— nine scenes: chance and velocity range in a step string (and the Live 10 fallback),
editing three notes by selection, partial quantize with swing, the groove pool (assign,
unknown name, empty pool, remove), undo with the set-overview diff and the performance
guard, Capture MIDI with the tempo change, recording options and an Arrangement take
stopped early, overdubbing a looping clip, and sample edits (transpose, gain in dB, warp
mode, warp markers, crop, doubling a MIDI loop). Not yet reviewed; the five review
questions are at its end.

## Tool descriptions

The `#[tool]` doc comments, as the model will read them.

```text
add_notes_to_clip (changed):
Write MIDI notes into a Session clip; appends unless `clear: true`. Notes in compact forms:
`steps` strings per pitch (x = note, X = accent, 1-9 = velocity level, ? = a note played
with `chance` at `chance_velocity`, _ = hold, . = rest), `notes_csv` lines
("pitch,start,duration,velocity[,mute[,probability[,velocity_deviation]]]"), `patterns`,
or full `notes` objects (which may carry probability 0-1, velocity_deviation -127..127 and
release_velocity 0-127). `probability` and `velocity_deviation` at the top level apply to
every note that does not set its own. Live 11 or newer keeps chance and velocity range and
returns note ids; on Live 10 they are dropped and the reply says so. Fails with the reason
when the slot holds no clip or a value is out of range; nothing is written then.

edit_notes:
Change or delete some notes of a Session MIDI clip without rewriting it. Choose the notes
with `note_ids` (from get_clip_notes) or `select`: `pitch` (number, note name or list),
`from_beat`/`to_beat` (start times, 0-based clip beats), `min_velocity`/`max_velocity`,
`off_grid: 0.5` (notes not on that grid, e.g. off-beat eighths), `every: N` with
`offset` (every Nth match). Then `set`: velocity, probability, velocity_deviation,
release_velocity, mute, duration, transpose (semitones), shift_beats; a number is
absolute, a signed string ("-20", "+0.02") is relative; or `delete: true`. The reply names
the notes touched and the before → after values. Needs Live 11 or newer (note ids); on
Live 10 it fails and names add_notes_to_clip with clear as the way. Notes never move past
the clip's end; a shift that would is clamped and reported. Fails when nothing matches,
listing the pitches the clip holds.

quantize_clip:
Quantize a Session MIDI clip with Live's own Quantize: `grid` one of 1/4, 1/8, 1/8T,
1/8+T, 1/16, 1/16T, 1/16+T, 1/32; `amount` 0-1 (1 = fully on the grid, 0.5 = halfway);
`pitch` limits it to one pitch (a kick, a hat); `swing` 0-1 sets Live's global swing
first, which Live applies to this quantize and to record quantize from then on. The reply
says how many notes moved and by how much at most, in beats and as a note value. Notes
keep their ids and expression. Fails on an audio clip or an unknown grid, naming the grids.

set_groove:
Give a Session clip a groove from this set's groove pool, or list the pool. `list: true`
returns every groove (name, base, timing/random/velocity amounts) and which clips use one;
`groove` is a pool groove's name or index, or "none" to remove it; `timing`, `random`,
`velocity` 0-1 set that groove's amounts (they apply to every clip using it);
`global_amount` sets the set's groove amount. The API cannot add a groove to the pool:
when the name is unknown or the pool is empty the error says to drag one from the
browser's Grooves folder onto the pool. Needs Live 11 or newer for the clip's groove;
older Lives can still set the global amount.

set_recording_options:
Live's transport settings for recording, in one call; unspecified ones stay: `metronome`,
`record_quantize` (none, 1/4, 1/8, 1/8T, 1/8+T, 1/16, 1/16T, 1/16+T, 1/32), `swing` 0-1,
`session_overdub`, `arrangement_overdub`, `punch_in`, `punch_out`. The reply lists every
value, including the count-in length, which the API reads but cannot set (Live's
metronome menu does).

capture_midi:
Live's Capture MIDI: keep what was just played on armed or monitored MIDI tracks, even
when nothing was recording. `destination` auto (the focused view), session or
arrangement; `name` names the new clip(s). The reply lists each new clip (track, slot or
bar, length, note count, likely key) and says when Live changed the tempo from the playing
(it does when the transport was stopped); `keep_tempo: true` restores the previous tempo.
Fails with "nothing to capture" when Live has no recent MIDI, and says what to do.

record_arrangement:
Record a real take into the Arrangement: arms `tracks` (names or indices; every other
armed track is disarmed unless `keep_armed: true`), turns Arrangement Record on, and
starts playback at `from_bar` (default: the current position) after Live's count-in. With
`bars`, the Remote Script's clock ends the recording on that bar and playback continues;
without it, call again with `stop: true`. `overdub: true` layers MIDI onto existing
Arrangement clips instead of replacing them. Fails for tracks that cannot be armed
(group, return, master), while a performance is running, or while capture_mix records.
The reply names the new Arrangement clips after a stop; undo removes a take.

overdub_clip:
Session overdub on a looping MIDI clip: arms the track, turns Session Record on for `bars`
bars from the next bar line (the clip keeps playing and what you play is added to it),
then turns it off and disarms; `quantize` sets record quantize for the take. Needs the
clip to be playing or fires it. Fails on audio clips and on tracks that cannot be armed.
undo removes the take.

set_audio_clip:
Audio-clip settings of a Session or Arrangement clip; unspecified ones stay: `transpose`
(-48..48 semitones), `detune` (-50..49 cents), `gain` 0-1 or `gain_db` (matched through
Live's display value to 0.1 dB), `warp` on/off, `warp_mode` (beats, tones, texture,
repitch, complex, complex_pro, rex — only the modes the clip offers), `crop: true`
(removes what lies outside the loop, or outside the start and end markers when not
looping). The reply shows before → after. Fails on a MIDI clip and on a warp mode the clip
does not offer, naming the ones it does.

set_warp_markers:
Add, move or remove warp markers on a warped audio clip: `add` [{beat, sample_time?}],
`move` [{beat, by}], `remove` [beat]. The reply lists the markers afterwards; Live keeps a
hidden last marker, which is reported and cannot be removed. Fails when the clip is not
warped or is MIDI.

set_clip_loop (changed): gains `double: true` — Live's Duplicate Loop: doubles the loop
(or the clip when not looped), copying notes and envelopes into the new half. MIDI clips
only.

undo:
Live's own Undo (Edit menu), `steps` times (default 1); `redo: true` redoes instead. The
reply says how many steps Live undid, what changed in the set overview (tempo, tracks,
clips and note counts) or that the change was inside a clip or device, and how many steps
remain. Refused while a performance runs unless `force: true`. A server tool can be
several Live steps; the reply says how many modifying commands the last tool sent. To
restore a clip's notes by content after vary_clip, undo_vary is the other way.
```

## Acceptance Criteria

### Phase 1 — feel and undo (the ask)
- [ ] **AC1 — Extended note writes.** The script's `add_notes_to_clip` handler calls
      `Clip.add_new_notes` with `probability`, `velocity_deviation`, `release_velocity` and
      `mute` per note when the method exists, and returns `{"note_count", "note_ids",
      "extended": true}`; without it (Live 10) it falls back to `set_notes`, returns
      `"extended": false` and `"dropped": [...]` naming the fields it could not write, and
      the tool text says so. `src/notes.rs` expands `?` in steps (with `chance`,
      `chance_velocity`), the csv columns six and seven, the top-level `probability` and
      `velocity_deviation` defaults, and validates ranges before any command is sent.
- [ ] **AC2 — `edit_notes`.** Reads the clip (`get_clip_notes`), selects by `note_ids` or
      `select`, computes the new note dictionaries, sends one `modify_notes` command
      (`Clip.apply_note_modifications` with the note dicts, ids included) or one
      `remove_notes` command (`Clip.remove_notes_by_id`). Relative values, transpose and
      shift clamp to 0–127 and to the clip's length and the reply says when they did.
      Refuses with the clip's pitch inventory when nothing matches; refuses on Live 10
      (no `note_id` in the read) naming the rewrite path.
- [ ] **AC3 — `quantize_clip`.** Script command `quantize_clip(track, clip, grid, amount,
      pitch?)` maps `grid` to a `Live.Song.RecordingQuantization` member
      ([live-python-enum-members.md](../../reference/ableton/live-python-enum-members.md))
      and calls `Clip.quantize` or `Clip.quantize_pitch`; the tool reads the notes before
      and after and reports moved count and the largest move (beats and note value);
      `swing` sets `Song.swing_amount` first through `set_recording_options`' command.
- [ ] **AC4 — `set_groove`.** Script commands `get_grooves` (pool with name, base,
      amounts, and per clip `has_groove`/groove name) and `set_clip_groove(track, clip,
      groove_index | none, timing?, random?, velocity?, global_amount?)`. Name resolution
      happens server-side against `get_grooves`; unknown names and the empty pool produce
      the two error texts of the prototype. Live 10 (no `Clip.groove`) can still set the
      global amount and says the clip groove needs Live 11.
- [ ] **AC5 — `set_recording_options`.** Script command `set_recording_options` setting
      `metronome`, `midi_recording_quantization` (enum member), `swing_amount`,
      `session_record`, `arrangement_overdub`, `punch_in`, `punch_out` when given, and
      returning all of them plus `count_in_duration` as a label. The tool text lists every
      value and marks count-in read-only.
- [ ] **AC6 — `undo`.** Script command `undo(steps, redo)` calling `Song.undo`/`Song.redo`
      while `can_undo`/`can_redo` holds, returning the steps done and the remaining counts.
      The tool reads `get_performance_state` before and after and reports the differences
      in tempo, track names and count, and clips-with-notes per track, or "no visible
      change"; refuses during a performance unless `force`; reports the modifying-command
      count of the previous tool call. "Nothing to undo/redo" is an error with that text.
- [ ] **AC7 — The instructions.** `src/context.rs` MAKING MUSIC gains one sentence: notes
      may carry chance and velocity range, `edit_notes` changes some notes, `quantize_clip`
      and `set_groove` add feel, `undo` is Live's undo; the FOOTER stays one line.

### Phase 2 — play it in
- [ ] **AC8 — `capture_midi`.** Script command `capture_midi(destination, name,
      keep_tempo)`: refuses when `can_capture_midi` is false with the prototype's text;
      snapshots every track's clip slots (or arrangement clip count) and the tempo, calls
      `Song.capture_midi(destination)`, then on the script's tick (up to 2 s) finds the
      new clips, names them, and returns them with length and note count plus
      `tempo_before`/`tempo_after`; `keep_tempo` restores `tempo_before`. The tool adds the
      likely key through `variation::estimate_key`.
- [ ] **AC9 — `record_arrangement`.** Script commands `start_arrangement_record(track_indices,
      from_beat, bars?, overdub, keep_armed)` and `stop_arrangement_record()`: arms,
      disarms the others unless kept, sets `arrangement_overdub`, `record_mode = True`,
      `current_song_time`, `start_playing`; with `bars` the tick sets `record_mode = False`
      at the end bar and leaves playback running; the stop command returns the new
      Arrangement clips per track (`arrangement_clips` diff against the start snapshot).
      Refused during a performance, during `capture_mix`, and for tracks with
      `can_be_armed` false. **`start_arrangement_record` / `stop_arrangement_record` are
      shared with
      [performance-records-itself-as-an-arrangement-take](performance-records-itself-as-an-arrangement-take.md),
      which arms the same switch for the performance the server drives. One script command,
      one signature: whichever story lands first defines it, and the other conforms. The
      difference is who chooses the bar — here the producer passes `from_beat`, there the
      producer is asked what to do with what is already in the Arrangement.**
- [ ] **AC10 — `overdub_clip`.** Script command `session_overdub(track, clip, bars,
      quantize?)`: arms the track, fires the clip if not playing, sets `session_record =
      True` on the next bar line and back to False after `bars` bars (tick), disarms, and
      returns the note count before and after. Refused on audio clips.

### Phase 3 — samples
- [ ] **AC11 — `set_audio_clip`.** Script command `set_audio_clip(track, clip, arrangement,
      transpose?, detune?, gain?, gain_db?, warp?, warp_mode?, crop?)`: refuses MIDI clips;
      maps warp-mode words to the LOM ints and checks `available_warp_modes`; `gain_db`
      bisects `gain` against `gain_display_string` to within 0.1 dB in ≤ 8 steps; `crop`
      calls `Clip.crop`; returns before/after for every field touched.
- [ ] **AC12 — `set_warp_markers`.** Script command `edit_warp_markers(track, clip,
      arrangement, add, move, remove)` calling `add_warp_marker`, `move_warp_marker`,
      `remove_warp_marker`, returning the marker list; refuses unwarped and MIDI clips.
- [ ] **AC13 — `set_clip_loop {double: true}`.** Calls `Clip.duplicate_loop` through the
      existing `set_clip_loop` command (a new optional field) and reports the old and new
      loop length.

### No Regressions
- [ ] **AC14:** `add_notes_to_clip` without any new field sends exactly the note objects it
      sends today (pinned by `tests/clip_notes.rs`), and a script without the new commands
      makes the new tools fail with the reinstall message, never a half-write.
- [ ] **AC15:** `vary_clip`, `undo_vary`, `follow_key` and `build_song` keep working through
      the changed write path; their suites stay green. In-place `vary_clip` preserves each
      note's `probability`, `velocity_deviation` and `release_velocity` when the variation
      keeps the note (today they are lost with the rewrite).
- [ ] **AC16:** The performance guards apply: `undo`, `record_arrangement`, `overdub_clip`
      are refused during a performance (undo with `force` excepted); `cue`, `record_clip`
      and `capture_mix` behave as before.
- [ ] **AC17:** No new socket, no new file, no new environment variable; `tests/local_only.rs`
      and `tests/activity.rs` unchanged and green.
- [ ] **AC18:** The docs facts snapshot and the tool count tests move together in the same
      PR (`tool_count_and_schema_defaults`, `tests/stdio_integration.rs`,
      `docs/facts/source-of-truth.md`, `scripts/check-docs-facts.sh` green).

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | `add_notes_to_clip` handler → `add_new_notes` with fallback; new handlers `modify_notes`, `remove_notes`, `quantize_clip`, `get_grooves`, `set_clip_groove`, `set_recording_options`, `undo` (phase 1); `capture_midi`, `start_arrangement_record`, `stop_arrangement_record`, `session_overdub` (phase 2); `set_audio_clip`, `edit_warp_markers`, `duplicate_loop` inside `set_clip_loop` (phase 3); tick work for the recording ends; `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` |
| `src/notes.rs` | `?` steps, `chance`, `chance_velocity`, csv columns 6–7, top-level `probability`/`velocity_deviation`, range validation; `Note` gains the three optional fields explicitly (no longer via `extra`) |
| `src/tools.rs` | new param structs, bodies and `#[tool]` bindings for the ten tools; `add_notes_to_clip` and `set_clip_loop` params; `ALL_REMOTE_COMMANDS`; the `vary_clip` write path preserving expression; tool count test |
| `src/connection.rs` | the new modifying commands in `MODIFYING_COMMANDS`; `capture_midi` and the two record starts in the modifying class (the script's own wait stays under 15 s) |
| `src/context.rs` | one sentence in MAKING MUSIC (AC7) |
| `src/variation.rs` | `VNote` carries the optional expression fields through `from_value`/`to_value` (AC15) |
| `docs/architecture/overview.md`, `docs/technical/feature-matrix.md` | the note write path, the recording tools, undo; same PR |
| `docs/facts/source-of-truth.md` | snapshot: tools, commands, script version, re-dated |
| `docs/reference/ableton/live-object-model.md` | `GroovePool` and `Groove` promoted from one-line summaries to full classes (properties `grooves`; `name`, `base`, `quantization_amount`, `timing_amount`, `random_amount`, `velocity_amount`), re-fetched per the reference README, so AC4 is anchored |
| `README.md` | tool table rows |

### New
| File | Description |
|------|-------------|
| `tests/feel.rs` | phase 1 suite (see Test Coverage) |
| `tests/recording.rs` | phase 2 suite |
| `tests/audio_clip.rs` | phase 3 suite |

## Remote Script compatibility

- [ ] Handlers added to `AbletonMusicMaker_Remote_Script/__init__.py` — no f-strings, no
      type hints, no third-party imports; every Live 11+ member behind `hasattr`/`getattr`
      with the Live 10 branch kept where the file already has one
- [ ] Command names added to `SCRIPT_CAPABILITIES`: `modify_notes`, `remove_notes`,
      `quantize_clip`, `get_grooves`, `set_clip_groove`, `set_recording_options`, `undo`,
      `capture_midi`, `start_arrangement_record`, `stop_arrangement_record`,
      `session_overdub`, `set_audio_clip`, `edit_warp_markers`
- [ ] `SCRIPT_VERSION` bumped (minor) from the version on `main` at merge time; the changed
      `add_notes_to_clip` response (`extended`, `note_ids`, `dropped`) is additive, so an
      older server against the new script keeps working
- [ ] Commands added to `tools::ALL_REMOTE_COMMANDS` (the cross-check test fails otherwise)
- [ ] Every tool body calls `require(live, "<command>")` for each command it may send,
      before the first send
- [ ] Live version floor: `add_new_notes`, `apply_note_modifications`, `remove_notes_by_id`,
      `Clip.groove`, `warp_markers` and the warp-marker functions need Live 11+; `quantize`,
      `groove_amount`, `swing_amount`, `metronome`, `record_mode`, `session_record`,
      `capture_midi`, `undo`, `gain`, `pitch_coarse`, `warp_mode`, `crop` are older. On
      Live 10: `add_notes_to_clip` degrades (AC1), `edit_notes` and `set_warp_markers`
      refuse with "needs Live 11 or newer", `set_groove` sets only the global amount
- [ ] Timeout class: `get_grooves` read (10 s); everything else modifying (15 s); the
      script's own waits (`capture_midi` clip discovery ≤ 2 s, gain bisection ≤ 8 sets)
      stay well inside it

## Privacy

No new local data. Live's undo history lives in Live; the server keeps nothing new in
memory beyond the per-call activity record it already writes (tool name, commands,
timings, sizes — payloads off by default, pinned by `tests/activity.rs`). No new file
under `state_dir()`, no new environment variable, no change to `TERMS.md`. Nothing
uploads.

## Test Coverage

The Remote Script has no test harness (it runs only inside Live), so every script handler
is specified by the response shape the Rust tests script into `FakeBridge`, and the manual
pass in Verification is the only place the Live side is exercised. That is why each
response field a tool depends on is named here.

| Suite / script | Change | AC |
|----------------|--------|----|
| `src/notes.rs` unit tests | `?` expands to a note with `probability = chance` and `velocity = chance_velocity`; `x` next to `?` keeps probability 1 when `probability` is unset; top-level `probability`/`velocity_deviation` apply only to notes without their own; csv with 5, 6 and 7 columns; out-of-range probability 1.5, deviation 200, release 300, negative chance → error before any command; `_` hold after `?` keeps the chance | AC1 |
| `tests/clip_notes.rs` | the sent `add_notes_to_clip` payload carries the three fields only when given (AC14 pin: a plain call's JSON equals today's); response `extended: false, dropped: [...]` → the Live 10 sentence in the text; response `note_ids` → "Note ids 1–16" | AC1, AC14 |
| `tests/feel.rs` (new) | **edit_notes:** with a scripted `get_clip_notes` (24 notes with ids, three pitches) — `select.pitch 38 + beat range` sends one `modify_notes` with exactly one note dict, id kept, velocity 110→90, text names D1; `off_grid 0.5` selects the 8 off-beat hats; `every 2, offset 1` picks kicks at beats 1 and 3 and `delete: true` sends `remove_notes` with those two ids; relative `"+0.02"` shift on a note at beat 3.98 of a 4-beat clip clamps and the text says so; transpose beyond 127 clamps; a response without `note_id` → the Live 10 refusal; nothing matches → the pitch inventory error; note names ("F#1") accepted in `select.pitch`. **quantize_clip:** grid labels → `rec_q_*` member names in the payload, `1/12` refused with the list; `amount` 1.2 refused; `pitch` → `quantize_pitch` shape; scripted before/after reads → "41 notes moved, largest 0.11 beats (about a 32nd)"; `swing` sends `set_recording_options {swing_amount}` first (command order asserted). **set_groove:** `list` renders the pool and clip usage from a scripted `get_grooves`; a name resolves to its index in `set_clip_groove`; case-insensitive and unique-prefix matching; unknown name → error listing the pool; empty pool → the drag text; `"none"` → `groove_index: null`; amounts outside 0–1 refused; Live 10 response (`clip_groove_supported: false`) → global amount only. **set_recording_options:** payload holds only the given keys; `record_quantize` labels → member names; the reply lists all values including count-in as read-only. **undo:** command list is `get_performance_state, undo, get_performance_state`; scripted states differing in track count and a clip's note count → the diff text; identical states → "no visible change"; response `steps_done: 0, can_undo: false` → "Nothing to undo"; `redo` flag; the performance guard (a scripted running performance) refuses and `force` passes; the previous tool's modifying-command count appears in the text | AC2–AC6, AC16 |
| `tests/performance.rs` | in-place `vary_clip` keeps `probability`/`velocity_deviation`/`release_velocity` on surviving notes; `undo_vary` unchanged | AC15 |
| `tests/orchestration.rs` | `build_song` clips with `?` steps reach `add_notes_to_clip` with probability set; `batch` runs `edit_notes` | AC1, AC15 |
| `tests/recording.rs` (new) | **capture_midi:** scripted `can_capture_midi: false` → the nothing-to-capture error and no `capture_midi` command; a response with two new clips and `tempo_before 126 / tempo_after 93.4` → both clips listed and the tempo sentence; `keep_tempo` in the payload; the key from the returned notes. **record_arrangement:** `tracks` by name resolve through the state read; the payload's `track_indices`, `from_beat` from `from_bar` at the state's beats per bar, `bars`, `overdub`, `keep_armed`; `Master` refused before any send; a running performance refuses; `capture_mix` in flight refuses; `stop: true` sends only `stop_arrangement_record` and lists the returned clips. **overdub_clip:** audio clip (from `get_clip_info`) refused; payload and text | AC8–AC10, AC16 |
| `tests/audio_clip.rs` (new) | MIDI clip refused from `get_clip_info` before any set; warp-mode words → ints and `available_warp_modes` from the info gate the error text; `gain_db` sent as is (the bisection is the script's) and the response's `gain_steps`/`gain_db_after` render as "matched to 0.1 dB in 6 steps"; `transpose` 49 refused; `crop` alone sends `crop: true` only; **set_warp_markers:** payload shape for add/move/remove, unwarped clip refused, the hidden last marker sentence; **set_clip_loop double:** sends `double: true` and renders 8 → 16 | AC11–AC13 |
| `src/tools.rs` unit tests | `tool_count_and_schema_defaults` (+10), `ALL_REMOTE_COMMANDS` ⇄ `SCRIPT_CAPABILITIES` cross-check (+13), every new tool's schema has the documented defaults (`amount` 1.0, `steps` 1, `destination` "auto") | AC18 |
| `tests/stdio_integration.rs` | tool count; the ten names present; instructions contain "undo" and "edit_notes" | AC7, AC18 |
| `tests/activity.rs`, `tests/local_only.rs` | unchanged, green | AC17 |
| `python3 -m py_compile AbletonMusicMaker_Remote_Script/__init__.py` | still compiles; `grep -n 'f"' ` on the script prints nothing (no f-strings) | compatibility |
| `scripts/check-docs-facts.sh` | green after the snapshot update | AC18 |

## Implementation Notes

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| Expand and validate before the first command | `crate::notes::expand` in `add_notes_to_clip_body` ([`src/tools.rs:1056`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/src/tools.rs#L1056)) | every new field in `notes.rs`; `edit_notes` selection and clamping; grid and warp-mode word tables |
| Read, compute, write once | `vary_clip_body` (`clip_notes` → `variation::vary` → `write_notes`) | `edit_notes` (`get_clip_notes` → select/apply → `modify_notes`), `quantize_clip`'s before/after report |
| Resolve names server-side against a read | `state.track_by(&p.track)` in the performance tools | `set_groove` names against `get_grooves`; `record_arrangement` tracks |
| Performance guards | `stop_playback`, `set_tempo`, `delete_track` refusals while a performance runs (feature-matrix, Performance rows) | `undo` (with `force`), `record_arrangement`, `overdub_clip` |
| Script-side waits on the tick | `_pending_record` and `_arm_perf_tick` in `_record_clip` ([`__init__.py:3488`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/AbletonMusicMaker_Remote_Script/__init__.py#L3488)) | ending an Arrangement recording or a Session overdub on the bar; discovering the clips Capture MIDI made |
| `hasattr` version branches | `_notes_from_clip` ([`__init__.py:4296`](https://github.com/defoAI/mcp-ableton-music-maker/blob/541d701/AbletonMusicMaker_Remote_Script/__init__.py#L4296)), `_set_scale` | `add_new_notes` vs `set_notes`; `Clip.groove`; warp-marker functions |
| Enum members from the reference | `Live.Song.Quantization` use in `set_launch_quantization` | `Live.Song.RecordingQuantization` for `quantize_clip` and record quantize |
| Compact result text with before → after | `set_track_mixer`, `set_clip_loop` replies | `set_audio_clip`, `set_recording_options`, `edit_notes` |

### Design Decisions
- **Why change `add_notes_to_clip` instead of adding `add_notes_extended`?** Every existing
  caller (`create_clip`, `build_song`, `vary_clip`, `undo_vary`, `follow_key`) goes through
  the same command; a second command would leave them writing plain notes and would lose
  expression on every rewrite. The response is additive, so version skew is safe both ways.
- **Why `edit_notes` with server-side selection rather than exposing
  `apply_note_modifications` raw?** The model would otherwise read the clip, rewrite the
  note dictionaries by hand and send them back, which is where transcription errors live.
  Selection plus deltas is what the producer says, and the server can clamp and report.
- **Why is quantize Live's, not the server's?** Live's Quantize honours the global swing
  and keeps note ids and expression; a server-side move would need `apply_note_modifications`
  anyway and would still not match what Cmd-U does. The server only reports the change.
- **Why does the script bisect gain?** The 0–1 gain curve is undocumented; guessing a
  formula gives a reply that lies by a dB. Eight sets and reads inside one command cost
  nothing the producer can hear.
- **Why is `undo` allowed to be blunt?** Live's history is the producer's mental model; a
  server-side "undo the last tool" would be a second, diverging history. The overview diff
  is cheap (one state read the server already has after the call) and tells the producer
  whether the step was the one they meant.
- **Why keep `undo_vary`?** It is content-based and survives Live's history (a later edit
  does not push it out of reach) and a reconnect. The two are described side by side.
- **Why no follow-action tool?** The local LOM copy has none; see open question 1. The
  story does not design against a member it cannot point at.
- **Why not count-in, freeze, export, grouping, arrangement automation?** Not in the LOM,
  or read-only. The feature-matrix's "Capability" table names them as boundaries.

## Verification

### Manual verification steps
With Live 12 open, the Remote Script reinstalled and Live restarted, the activity log on
(default), payloads off (default).

1. *Feel.* "hats on the 16ths, off-beats ghosted, two times out of three, vary the
   velocity": the clip's Chance lane shows 66% on eight notes and shaded velocity ranges.
   `get_clip_notes` returns `probability` 0.66 on those notes and stable `note_id`s.
2. *Edit.* "snare on the 4 down by 20": only that note's velocity changes in Live's editor,
   every other note keeps its id (read before and after). Then "undo that": the velocity is
   back; the reply says "no visible change" (the step was inside the clip).
3. *Quantize.* Play a loose keys take with `record_clip`; "quantize to 16ths, 80%, some
   swing": Live's swing field shows 57%; the notes move part-way; a second call at 100%
   lands them on the grid.
4. *Groove.* Drag a groove from the browser onto the pool; `set_groove list` shows it;
   assign it to the hats; the clip's Groove chooser shows the name; "none" clears it. Ask
   for a groove not in the pool and read the error.
5. *Undo the big one.* Run `build_song` for a small set, then `undo {steps: 5}`: the
   reply's diff matches what disappears in Live; `redo` brings it back.
6. *Capture.* Transport stopped, arm a MIDI track, play four bars, "keep that": a clip
   appears where Live's focused view is; the reply names the detected tempo; with
   `keep_tempo: true` the old tempo is back.
7. *Arrangement take.* Metronome on, "record 8 bars of keys from bar 17": Live counts in,
   records, the recording light goes off at bar 25, playback continues; `get_arrangement_clips`
   shows the take at 17–25. Repeat and stop early at bar 21 with `stop: true`.
8. *Overdub.* Loop a drum clip; "let me overdub hats for four bars": Session Record lights
   for four bars, the new notes are in the clip, the track is disarmed afterwards.
9. *Samples.* On an audio clip: transpose −2, gain −3 dB, Complex Pro: Clip View shows
   −2 st, −3.0 dB (±0.1), Complex Pro. Ask for REX on a clip that lacks it and read the
   error. Add a warp marker at beat 4; crop; double a MIDI loop.
10. *Live 11 and, if available, Live 10.* Steps 1 and 2 on Live 11 behave as on 12; on
    Live 10 step 1 writes plain notes and says what was dropped, step 2 refuses.
11. *No regression.* `start_performance`, a `cue` with a fill, `vary_clip` in place, then
    `undo_vary`: unchanged. `undo` during the performance is refused; `end_performance`,
    then `undo` works.
12. *Local only.* `ableton-music-maker --status` shows the same files as before the
    story; the activity log lines for the new tools carry names, commands and sizes and no
    note payloads.

## Out of Scope
- Follow actions (open question 1) until the probe finds them in the API.
- Adding a groove to the pool from the API (open question 2); `humanize`/`swing_notes` as
  note rewrites stay in `sections-and-songs` phase 3 (AC13 there) and are the Live 10
  fallback for feel.
- Time-signature changes, scene tempo and scene time signature (the `sections-and-songs`
  story owns scene tempo), tuning systems.
- Track routing and monitoring as tools (`input_routing_type`, `current_monitoring_state`);
  `record_arrangement` and `overdub_clip` arm only. A "set up this track to record from
  my keyboard" story can follow once the recording verbs exist.
- Device housekeeping (delete, reorder, macro variations), Simpler slicing.
- Anything the LOM does not offer: rendering and export, freeze and flatten, grouping
  tracks, arrangement-level track automation, tempo and time-signature markers, sidechain
  routing, setting the count-in length.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| Remote Script `main` version at merge time | 1.16.0 merged (#38); 1.17.0 in flight on `feat/39-sections-and-songs` | rebase onto whichever lands first; both bump the minor |
| Real-Live probe for open questions 1 and 2 | not done | ten minutes in the script's log on Live 12 |
| `docs/reference/ableton/live-object-model.md` `Groove`/`GroovePool` full classes | to do in this PR | re-fetch per the reference README; no paraphrase |
| Issue for tracking | to file | one issue, three checkboxes; closes #30 |

## Related Stories
- `performance-records-itself-as-an-arrangement-take` — records the *performance*; this story
  records the *producer*. They share `start_arrangement_record` / `stop_arrangement_record`
  (AC9) and must not specify them differently.
- `notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix` — the note grammar
  this story's per-note fields extend (`bars`, `bar_steps`, degrees, chords); land that
  first so `NotesInput` has one shape.
- Shipped code this story builds on, no longer a story: `feel` (groove, humanize, swing,
  retime — `src/variation.rs`, `src/arrange.rs`), the script tick and the performance
  guards (decision 0007, `AbletonMusicMaker_Remote_Script/__init__.py`), and the Capture
  track `capture_mix` and `record_arrangement` both want (`src/audio.rs`,
  `src/tools.rs`).

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from the song-writing gap analysis; prototype transcript written, not yet reviewed |
