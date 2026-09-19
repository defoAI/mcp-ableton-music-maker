# Notes by bar and in the key, sections that add, replies that carry the cost and the fix

## Story
**As a** performer building a set with Claude while the room listens,
**I want** to write a part as bars and chords in the set's key rather than as MIDI numbers
and 128-character strings, to grow a section from another by adding a part, to place it
and delete it, to know what a call will cost Live before I make it, to read the fix in every
refusal, to see whether a section is too busy before it plays, and to be told when a jump
leaves the plan,
**So that** the effort of a set goes into deciding *when* things happen — which the tool
already does well — and not into producing and validating note data, which is where every
mistake and most of the time went in the third real set.

## Details
| Field | Value |
|-------|-------|
| Status | `Draft` — the open questions carry proposed answers; the prototype transcript is not yet reviewed |
| Surface | [Decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md): no tool is added except `delete_section` (housekeeping, beside `delete_clip`); everything else extends `add_notes_to_clip` / `create_clip` / `make_section` / `build_song` (the note forms), `make_section` (add, place), `jump_to` (the reply), `get_context` (`section`), the descriptions and the instructions. Three raw tools are promoted into `CORE_TOOLS` under their current names: `listen`, `get_drum_rack_pads`, `get_clip_notes`. Units stay the artist's: bars, note names, dB |
| Priority | P1 — the performer's field notes of 2026-09-19 after the third real set: "excellent at deciding when things happen and weak at expressing what the notes are"; every item below is something that went wrong or took a bar of real time in front of a room |
| Size | L — six phases below, each shippable alone; phase 1 (notes) and phase 3 (the cost and the fix) are the ask and ship first; phase 6 (names) is a day and closes a tool-not-found mid-set |
| Tracker | to be filed — one issue, a checkbox per phase |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Prototype | [prototypes/notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md](../prototypes/notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md) |
| Baseline | 99 tools, 86 Remote Script commands, Remote Script 1.23.0 on `main`, server 2.0.0 (the snapshot in [source-of-truth](../../facts/source-of-truth.md), verified 2026-09-19); the working tree on `fix/43-main-thread-executor` is at Remote Script 1.24.0, so this story bumps from whatever `main` holds when it lands |
| Decisions | [0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md) (the surface; its group list changes with phase 6) · [0007](../../decisions/0007-live-main-thread-only-in-slices.md) (`main_ms` on every reply is what phase 3 puts into words) |

## Context

### The Problem
Three real sets have been played with this server on 2026-09-19. The third was built in an
empty Live set and steered live, and the operator changed direction twice — once asking for
completely different material, once for a piano break *now*. The performance layer held:
cues ran on Live's clock, every jump landed on a phrase, the clock and the plan were on
every reply, the transition compositor did in one cue what an operator would assemble by
hand. Almost all of the effort, and nearly all of the mistakes, were in saying **what the
notes are**:

- A two-bar bassline is a 32-character step string; the four-bar ones were 64. They cannot
  be read, counted or edited by eye, and one misplaced character shifts everything after
  it. The performer stopped writing them and wrote a Python script to generate and
  length-check them. A clip whose kick and hat lines were 16 characters and whose clap line
  was 128 was accepted without a word.
- `set_key` writes the scale into Live, and then every pitch is still a MIDI number. The
  piano break was twenty-two hand-computed `notes_csv` rows for four chords. The register
  trap the guidance warns about (29 is inaudible, F1 is not) exists only because a number
  can be an octave off and a name cannot.
- `make_section` from a section can thin, transpose, empty or replace a track the source
  has — the performer believed it could not *add* one and re-sent whole sections. The copy
  must sit below its source, so by the end of the set the Session view no longer read as the
  performance. Eleven abandoned sections stayed in the set: there is no `delete_section`.
- The guidance says to build before the transport starts; the room asked for something else
  twice. Every `make_section` held Live about a quarter of a second, the piano's
  `load_browser_item` 1.5 s, and nothing said so *before* the call.
- A drop that did not fit was refused with "jump later" three times, each a bar later, when
  the server knew the earliest bar the first time.
- Seven tracks of new material had no way to be heard before they were fired at the room;
  the meters read a healthy −3 dB and it sounded like noise.
- A jump to a section outside the setlist printed the old material queued behind it in the
  same voice as a routine plan.
- The reply text says `listen`; the tool is served as `adv_listen`. Tool-not-found, mid-set.

### Current State
Verified 2026-09-19 against the working tree on `fix/43-main-thread-executor`.

**Notes (`src/notes.rs`).** Four forms, freely mixed: `notes`, `notes_csv`, `steps`,
`patterns`, plus `loop_every` + `until` to tile (`src/notes.rs:5-13`). Pitches are numbers
or Live's note names in `steps` keys, `notes_csv` and `patterns` (`parse_note_name`,
`src/notes.rs:138-187`; used at `:234`, `:258`, `:299`); `notes` objects take `pitch: i64`
only. Step characters: `x` = `velocity` (default 100), `X` = `accent_velocity` (default
120), `1`–`9` = `digit × 14` (so `5` is 70, `9` is 126), `_` ties, `.`/`-` rest
(`src/notes.rs:274-279`). The mapping appears nowhere the model reads: the description says
"1-9 = velocity level" (`src/tools.rs:6155-6165`). **Nothing compares the lengths of the
step strings in one call** (`parse_steps`, `src/notes.rs:250-293`): each line is walked on
its own. `loop_every` without `until` is refused; `until` is the clip span in beats
(`src/notes.rs:336-341`). No form knows the key: `PerfState` carries `root_note`,
`scale_name` (`src/performance.rs:189-195`) and `set_key` writes them (`src/arrange.rs:206`),
but no pitch is ever resolved against them.

**Sections (`src/sections.rs`).** `make_section` has three sources: `"playing"`
(`capture_scene`), `{"section": …}` (`duplicate_scene` + `changes`), or `clips`
(`create_scene` + `create_clip` per track) (`src/sections.rs:24-47`, `:196-253`). `changes`
per track: a variation name, `{"transpose": n}`, `"empty"`, or notes (`Change::parse`,
`:401-427`). **A notes change on a track the source leaves empty already creates the clip**
(`apply_change`, `Change::Notes`, `:512-535`) — the parameter doc says "replacement notes"
and the reply says "rewritten", so nobody knows. Chained sources work: `from` is looked up
by name among all sections (`:305`). `after` with a copy is refused: "a copy is inserted
right below its source … `after` cannot move it" (`:313-319`), because Live's
`Song.duplicate_scene` inserts below (LOM, `docs/reference/ableton/live-object-model.md:494`).
`replace: true` rewrites a `clips` section in place (`:44-46`, `:217-230`). There is no
scene deletion in `SCRIPT_CAPABILITIES`; the LOM has `Song.delete_scene(index)` (`:479`),
`Song.create_scene(index)` (`:473`) and `ClipSlot.duplicate_clip_to(target)` (`:1149`,
copies audio or MIDI with every clip setting). `build_song` takes a `scenes` block that
names and creates empty rows (`src/tools.rs:833-836`).

**Cost.** Since decision 0007 every reply carries `main_ms` and the activity line records
it. From this machine's activity log (`~/.ableton-music-maker/activity/*.jsonl`,
2026-09-19, Live 12.4.6): `make_section` from `clips` (`create_scene` + `write_clips`)
median 241 ms over 15 calls, max 291; from a section (`duplicate_scene` + changes) 213–236;
`create_scene` alone 68; `load_browser_item` median 564, max 1501 over 5; `delete_track`
51–110; `create_clip` 5.4; `add_notes_to_clip` 0.6; `jump_to` 4.6–7.7; `get_context` 1.9;
`listen` 3–8. Note writes are sliced at 8 ms per tick while the transport runs
(`SLICE_MS_PLAYING`, decision 0007); a scene, track or device call is one Live call and
cannot be sliced. No description states any of this; the cost is known after the call.

**Refusals.** `transition.drop` refuses with "the drop would start at bar {drop_bar},
before the next bar ({next}); jump later or shorten it" (`src/transition.rs:626-630`);
`transition.fill` the same (`:495-499`). Both know `bar`, `bars` and `state.next_bar()`, so
both know the earliest bar. `fill` on a silent track says "plays nothing to make a fill
from" (`:490-493`) and does not name what does play. `make_section` from `clips` names the
tracks left empty; a jump does not name the tracks the target row stops.

**Hearing.** `capture_mix` and `listen {capture: true}` record the master through a
Resampling input (`__init__.py:2469-2488`); `band_balance` reports low (< 200 Hz), mid, high
(> 4 kHz) of that one signal (`src/audio.rs:511-521`). `listen` reads every track's meter
peak (`src/tools.rs:3941-3965`). Nothing measures a row that is not playing, and nothing
reads density — the notes the server itself wrote are never counted.

**The setlist.** A `jump_to` target outside the setlist keeps the entry the song left as
its position (`src/sections.rs:1224-1243`); without `repeats` the plan waits there
(`song::plan`, `src/song.rs:392-415`) — **the default already holds**. With `repeats`, the
plan continues from the entry after the one it left, and the reply prints it as "then the
song continues: A2 Acid at bar 117 …" (`:1394-1407`), the same words as a routine plan.
The song line does say "now in 'X' (not in the setlist)" (`src/song.rs:520-526`); the
jump's own reply does not.

**Names.** `INSTRUCTIONS` (`src/context.rs:12-29`) names `start_performance`, `cue`,
`get_performance_state`, `keep_track_playing`, `stop_playback`, `fire_scene`, `fire_clip`
and `stop_clip` without the `adv_` prefix they are served under; reply text names `listen`
bare at `src/tools.rs:4087`, `:6447-6448` and `:6933`. `tests/artist.rs:538-575` pins that
core tools are served bare and the rest as `adv_`; nothing pins that the words in the
instructions and descriptions are served names. `get_drum_rack_pads`'s description says
"call this before writing drums" (`src/tools.rs:6371-6373`) and it is `adv_`.
`annotations_for` special-cases `listen` as read-only (`src/tools.rs:117-122`).

### Root Cause
The note forms were designed for a 16-step drum bar and never revisited when parts became
eight bars long and harmonic. The section tools were designed for the setlist as the
running order and the Session view as storage, so placement and deletion were not needed
until the set was improvised. The cost of a call became measurable in 1.23.0 and has not
yet been written into the descriptions. The instructions were written before the `adv_`
prefix existed and were never tested against the served names.

## Open Questions

Proposed answers are marked *(proposed)*; they become decisions when the performer confirms
or the review of the transcript changes them.

1. **Bar-oriented entry: which shape?** The notes propose
   `{"bar": "…", "repeat": "all", "except": {"8": "…"}}`. *(proposed)* Keep `steps` as the
   one step grammar and add two fields beside it: `bars` (the clip's span in bars) and
   `bar_steps` (`{"8": {"F1": "…"}}`, whole bars that differ). A step line shorter than the
   span repeats to fill it; a line exactly the span long plays once. This makes the two
   common calls two short strings each, and one grammar rather than two. `loop_every` and
   `until` stay accepted for generated material and drop out of the description's first
   paragraph.
2. **Mixed lengths: refuse all, or allow divisors?** *(proposed)* A line's length must be
   the span or divide it evenly (a two-bar bass under a one-bar hat is music); anything else
   is refused naming the pitch, its length in steps and bars, and the expected length. With
   no `bars` given, every line must be the same length — the current silent acceptance is
   the defect (prototype scene 2).
3. **Scale degrees: how are they marked?** `"5"` already means MIDI note 5 in every pitch
   position, so bare digits cannot become degrees. *(proposed)* A caret: `^1`, `^b3`, `^5`,
   `^b7`, `^9`; the octave is the call's `octave` (default 3, Live's middle-C octave), and
   `^5+1` / `^5-1` shift it. Against: a call-level `pitches: "degrees"` switch, which makes
   `"36"` mean degree 36 and fail loudly — one more thing to set. The caret needs nothing set.
4. **Chords: symbols, Roman numerals, or both?** *(proposed)* Both. `Fm9`, `Db△7` (also
   `Dbmaj7`), `Bbm7`, `Ab6`, `Eb7`, `Gdim`, `Csus4` are what a performer says; `i9`, `VI△7`,
   `iv7`, `III6` resolve against the key and cannot be in the wrong one. A symbol whose root
   is not in the set's key is written and flagged in the reply, not refused.
5. **Where does `voicing` live and what are its words?** *(proposed)* On the same notes
   input as `chords`: `close` (stacked within an octave above the root), `spread` (root, then
   the upper voices spaced a third–fourth apart across an octave and a half), `rootless`
   (third, fifth, seventh, extensions); `bass: true` adds the root an octave below;
   `each` is bars per chord (default: the span divided evenly); `rhythm` is an optional step
   line applied to every chord (default: one hit held for the chord's bars).
6. **Placing a copy anywhere: a new Remote Script command or an extension?** *(proposed)*
   Extend `duplicate_scene` with `at` (a scene index): when given, the script does
   `create_scene(at)` + `duplicate_clip_to` per track from the source row, names the row,
   shifts its scene tables. Without `at` the existing `Song.duplicate_scene` path stays
   (one call, keeps clip colours and names — `duplicate_clip_to` keeps them too, but the
   fast path costs less). A cue that refers to a scene index above `at` is re-pointed by the
   script the way `_shift_scene_tables` already does for the phrase table.
7. **`delete_section`: a tool, or an action?** *(proposed)* A tool, in the housekeeping
   group beside `delete_clip` and `delete_track`; its name earns the destructive annotation
   from `annotations_for`. Takes `sections: [names]` (one or many — the abandoned set was
   eleven). Refuses only the sections that are playing or queued, deletes the rest, and
   removes any of them from the setlist in the same call (prototype scene 6). Refused
   outright during a performance? No — the guard is per section, like `delete_clip`'s.
8. **Pre-allocated rows: a new feature or documentation?** *(proposed)* Documentation plus
   one gap. `build_song`'s `scenes` block already creates named empty rows and
   `make_section {clips, replace: true}` already fills one in place; the story documents
   that as *the* way to improvise ("make eight spare sections before the show; fill them
   with `replace: true`"). The gap: `make_section` with neither `from` nor `clips` should
   create an empty named row so a spare can be made mid-set in one call (~68 ms).
9. **The cost in the description: figures or classes?** *(proposed)* Both, stamped: "a new
   row or track is one Live call, ~70–250 ms on Live 12 (measured 2026-09-19); an instrument
   load 0.5–1.5 s; note writes are sliced under the audio". The real number is in the reply
   already (`main_ms`); the description sets the expectation. Re-verify the figures when the
   Remote Script changes.
10. **Warn or refuse a call that will hold Live longer than a beat while performing?**
    *(proposed)* Warn, in the reply, when `main_ms` exceeded a beat at the current tempo:
    "held Live 1501 ms — longer than a beat at 130 BPM (462 ms); during a performance this
    is audible. Load between sections or keep a spare track ready." Refusing would have
    blocked the piano break the room asked for. Background loading does not exist in Live's
    API; the story says so in `load_instrument_or_effect` once.
11. **Transition does not fit: clamp or refuse-with-answer?** *(proposed)* When `at` is a
    phrase (the default), move to the first phrase end the transition fits before and say
    why; when `at` is an explicit bar, refuse and attach the earliest bar and the longest
    fitting length (prototype scene 7). Clamping an explicit bar would land the jump
    somewhere the performer did not name.
12. **Hearing before the room: what is possible?** Live's API cannot render a scene that
    is not playing; Resampling records the master. *(proposed)* Say that once, in
    `capture_mix`'s description, and give the performer what the server does have: the
    notes. `get_context {section}` reports onsets per bar, pitch range and shared registers
    per track, the total against the section that last ran, and the row's last peak from
    the level table (prototype scene 8). Every section-writing reply (`make_section`,
    `build_song`) ends with the same density line. Per-track band balance would need
    per-track routing into the Capture track; out of scope, named as such.
13. **Off-setlist jump: a new parameter?** *(proposed)* No. Omitting `repeats` already
    holds; the reply gains one line naming that the target is outside the setlist, what
    `next_section` would resume, and the `add_to_song` call that splices it in. With
    `repeats` on an off-setlist target, the line says "after N passes the song resumes at
    'A2 Acid' (bar 117)" ahead of the plan.
14. **Promote or rename?** The notes propose `listen`, `drum_pads`, `read_clip`.
    *(proposed)* Promote `listen`, `get_drum_rack_pads` and `get_clip_notes` into
    `CORE_TOOLS` under their current names: renaming breaks callers and the `get_` prefix is
    what gives the two readers their read-only annotation. `arrange` stays core: 0006's
    criterion is the artist's set, not the tools a show touches, and the Arrangement is the
    artist's when composing. `fire_clip`/`fire_scene`/`start_playback`/`stop_playback` stay
    advanced, as the notes say.
15. **Which words does the names test read?** *(proposed)* Every `snake_case` token of
    `INSTRUCTIONS`, `FOOTER`, every tool description and the fixed reply strings must be a
    served tool name, a parameter name of the tool it appears in, or a Remote Script command
    named in an `adv_` description. The three bare `listen` replies and the eight bare
    performance-layer names in the instructions are the failures it starts with.

Assumed without asking:

16. The velocity mapping does not change (`x` 100, `X` 120, digit × 14); it is documented,
    and the reply of every notes write prints the mapping actually used when digits appear.
17. Degrees and chords follow the *set's* key at write time (read from the state in the
    same call); `set_key` afterwards does not rewrite clips — that is `adv_follow_key`.
18. `bars` in a notes input and `length` on `create_clip` / `SectionClip` agree or `bars`
    wins with a note in the reply; `bars` alone sets the clip length.

## Prototype

[prototypes/notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md](../prototypes/notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md)
— ten scenes from the field notes replayed: the bass by the bar, mixed lengths refused with
the answer, the piano break as four chord symbols with the resolved notes in the reply, a
degree-written bass, a copy plus a part placed after its source, eleven sections deleted in
one call, the drop that names its bar, density before the room hears it, `listen` under its
served name, and the cost said in the description and the reply. The review points are
listed at its end; the transcript is written against the proposed answers above.

## Tool descriptions

Written in full as the model reads them. Descriptions say what the artist gets; the cost
figures are stamped in the story, not restated elsewhere.

### `add_notes_to_clip` (Build; `create_clip`, `make_section` `clips`, `build_song` clips share the note forms)
```text
Write MIDI notes into a Session clip. Think in bars: `bars` is the clip's span; `steps` is
one line per pitch (`{"C1": "x...x...x...x...", "F#1": "..x...x...x...x."}`, one character
per `step` beat, default a 16th), and a one-bar line repeats to fill `bars`; `bar_steps`
names whole bars that differ (`{"8": {"F1": "..8...8...8.9.9."}}`). Every line must span
the clip or divide it evenly — mixed lengths are refused, naming the pitch and the lengths.
Characters: x = `velocity` (100), X = `accent_velocity` (120), 1-9 = velocity 14…126
(digit × 14), _ = hold, . or - = rest; spaces and | are ignored.
Pitches are names (C1, F#2, Bb3 — Live's, C3 = 60), MIDI numbers, or scale degrees in the
set's key (^1, ^b3, ^5, ^b7; `octave` sets the register, ^5+1 an octave up). `chords`
writes harmony: ["Fm9", "Db△7", "Bbm7", "Ab6"] or Roman numerals in the key ("i9", "VI△7"),
`each` bars per chord, `voicing` close / spread / rootless, `bass: true` for the root an
octave below, `rhythm` a step line every chord follows. `notes_csv`
(pitch,start,duration,velocity per line), `patterns` (a repeating note) and full `notes`
objects still work, as do loop_every/until for generated material.
The reply lists what was written per pitch and bar, the velocities the digits meant, and
every chord's resolved notes, and flags a chord root outside the key. `clear: true`
replaces the clip's notes instead of appending.
```

### `make_section` (Build)
```text
A new section: a scene row named "<name> · <bars>" (its phrase length, kept by Live's Save).
Three sources. from: "playing" copies what plays into a row below the playing one (the set
keeps playing). from: {"section": "Build"} copies that row — a copy of a copy is fine — and
applies `changes` per track: a variation ("thin", "half_time", "double_time",
"fill_last_bar", "ghost_notes", "invert_chords"), {"transpose": -12}, "empty", or notes in
any form; notes on a track the source leaves empty ADD that part, so "the build plus a
stab" is one call. `clips` writes it from notes per track (bars, steps, chords …); tracks
not named stay empty and stop when the section fires. No source makes an empty named row to
fill later with replace: true — make spares before the show; filling one mid-set is a clip
write, not a new row. `after` puts the row after any section (default: at the end).
The reply names the tracks with clips, the tracks that stop, the density per track (onsets
per bar, pitch range) against the section that last ran, and what the call held Live for.
A new row is one Live call (~70–250 ms on Live 12); make rows between sections. A duplicate
name is refused; replace: true rewrites a `clips` section in place so the setlist keeps its
name.
```

### `delete_section` (housekeeping, new)
```text
Delete sections by name, and their clips, in one call: {"sections": ["W1 Intro", "W2
Build"]}. A section that is playing or queued is kept and named — jump elsewhere first. A
deleted section that was in the setlist is removed from the song in the same call and the
new song line is shown. Live's undo (Cmd+Z) restores a deleted row.
```

### `jump_to` (Play) — the amended tail
```text
… Warns when the target last ran more than 3 dB hotter (force: true skips it). A section
outside the setlist holds until go, and the reply says so, names what next_section would
resume, and gives the add_to_song call that splices it in; with `repeats` it says where the
song resumes and when. A transition that cannot fit before a phrase end moves to the first
phrase end it fits and says why; before an explicit bar it is refused with the earliest bar
that fits attached.
```

### `get_context` (Look) — the new parameter
```text
… `section: "<name>"` reads one section instead: its tracks with clips and without, per
track the onsets per bar and pitch range, the registers two tracks share, the total against
the section that last ran, and the row's last master peak. Use it on a section you have
not fired yet.
```

### `capture_mix` (Play) — one sentence added
```text
… Live's API cannot render a section that is not playing: capture_mix and listen measure
what the room hears; get_context {section} reads a section's density from its notes before
it plays.
```

### `load_instrument_or_effect` (Build) — one sentence added
```text
… Loading is one Live call the audio waits for, 0.5–1.5 s for an instrument on Live 12
(measured; the reply says what this one held). While a set plays, load between sections or
onto a spare track before the show; Live's API has no background load.
```

### `listen` (Play, promoted; text unchanged)
```text
Listen without stopping the set: the peak and average level of every track, return and the
master in dB over the next `bars` bars, from the next bar line (the master is flagged
within 0.5 dB of clipping). capture: true records the bars through the Capture track
instead and adds RMS per bar and low/mid/high balance. Allowed during a performance.
```

### `INSTRUCTIONS` (`src/context.rs`) — the paragraphs that change
```text
MAKING MUSIC: … notes in bars: `bars` and one step line per pitch that repeats, `bar_steps`
for the bars that differ, pitches as names (F1) or degrees in the key (^1, ^b3), `chords`
as symbols or Roman numerals with a voicing; the reply shows the notes it resolved. Call
get_drum_rack_pads before writing drums: kits differ. …

PERFORMING: … the raw layer underneath is adv_start_performance, adv_cue,
adv_get_performance_state, adv_keep_track_playing, adv_fire_scene, adv_fire_clip,
adv_stop_clip; adv_stop_playback is refused while a performance runs. listen reads the
levels over the next bars. …

COST: a reply ends with what the call held Live for. Note writes are sliced under the
audio; a new row, track or instrument is one Live call (a row ~70–250 ms, an instrument
0.5–1.5 s on Live 12) — make those between sections or before the show, and keep spare
sections and tracks to fill. A hold longer than a beat is flagged in the reply.
```

## Acceptance Criteria

### Phase 1 — notes by bar and in the key (`src/notes.rs`; every tool that takes notes)
- [ ] **AC1 — `bars`.** A notes input with `bars: N` spans N bars at the current signature;
  a step line shorter than the span whose length divides it repeats to fill; a line the
  span long plays once; `bars` sets the clip length when the tool creates the clip.
- [ ] **AC2 — mixed lengths refused.** Step lines of lengths that do not all equal the
  span, or divide it, are refused before anything reaches Live, and the error names each
  offending pitch, its length in steps and bars, and the expected length (prototype
  scene 2). With no `bars`, unequal lines are refused the same way.
- [ ] **AC3 — `bar_steps`.** `{"8": {"F1": "…"}}` replaces bar 8 of pitch F1; a bar
  number outside the span, or a line not one bar long, is refused by name.
- [ ] **AC4 — the velocity mapping is stated.** The description carries `x` 100, `X` 120,
  `1-9` = digit × 14; a write that used digits prints the velocities they became.
- [ ] **AC5 — note names everywhere.** Already true for `steps` keys, `notes_csv` and
  `patterns`; `notes` objects accept `pitch` as a name too; the description says so first.
- [ ] **AC6 — degrees.** `^1`, `^b3`, `^#4`, `^5`, `^b7`, `^9` (and `^5+1`, `^5-1`) resolve
  against the set's key read in the same call, with `octave` (default 3); a degree with no
  key set is refused with "set_key first"; the reply prints each degree's note name and
  number.
- [ ] **AC7 — chords.** `chords` with `each`, `octave`, `voicing`, `bass`, `rhythm` expands
  server-side into notes; symbols (`Fm9`, `Db△7`/`Dbmaj7`, `Bbm7`, `Ab6`, `Eb7`, `Gdim`,
  `Csus4`, `F`, `Fm`) and Roman numerals (`i9`, `VI△7`, `iv7`, `III6`, `V7`) both parse; a
  symbol root outside the key is written and flagged; the reply lists every chord's
  resolved notes (prototype scene 3).
- [ ] **AC8 — one code path.** `add_notes_to_clip`, `create_clip`, `make_section`
  (`clips` and `changes`) and `build_song` clips all accept the new fields through
  `NotesInput`; `notes::expand` is the only place they are resolved; `expand` takes the
  key and signature as arguments so it stays pure.

### Phase 2 — sections that add, sit where you want, and go away (`src/sections.rs`, the Remote Script)
- [ ] **AC9 — addition is documented and said.** A notes change on a track the source
  leaves empty creates the clip (already true), the parameter doc and the tool description
  say so, and the reply says "added" rather than "rewritten" (prototype scene 5).
- [ ] **AC10 — chained sources are documented.** "A copy of a copy is fine" in the
  description; a test copies from a copy.
- [ ] **AC11 — `after` places a copy anywhere.** `make_section {from: {section}, after}`
  is accepted; the Remote Script's `duplicate_scene` with `at` does `create_scene(at)` +
  `duplicate_clip_to` per track; the row's name, phrase and tempo are set; the phrase and
  level tables and any pending cue's scene indices shift; without `after` the old path is
  used unchanged.
- [ ] **AC12 — empty spares.** `make_section {name, phrase_bars}` with neither `from` nor
  `clips` creates an empty named row; the reply says it is empty and how to fill it
  (`replace: true`).
- [ ] **AC13 — `delete_section`.** New tool and Remote Script command `delete_scene`; many
  names per call; playing or queued sections are kept and named; the rest are deleted with
  their clips; deleted sections are removed from the `Setlist:` scene in the same call and
  the new song line is printed; the `Setlist:` scene itself cannot be deleted this way
  (`set_song` with an empty list does that); the tool carries the destructive annotation.

### Phase 3 — the cost said before, the fix said in the refusal
- [ ] **AC14 — the cost in the descriptions.** `make_section`, `build_song`,
  `load_instrument_or_effect`, `create_return`, `delete_track` and the `INSTRUCTIONS` COST
  paragraph state the cost class and the measured range, stamped with the date and Live
  version in this story; `adv_create_scene`, `adv_create_midi_track` and
  `adv_create_audio_track` say "one Live call" in one clause.
- [ ] **AC15 — the beat warning.** While a performance runs, any reply whose `main_ms`
  exceeded one beat at the current tempo ends with the warning line of prototype scene 10,
  naming the tempo, the beat length and the alternative; below a beat, only the existing
  "held Live N ms" wording.
- [ ] **AC16 — transitions carry the answer.** `fill` and `drop` that do not fit before a
  phrase-timed jump move to the first phrase end they fit before and say why; before an
  explicit `at` bar they are refused with the earliest fitting bar and the longest fitting
  length in the message (prototype scene 7). No refusal in `src/transition.rs` says "jump
  later" without a bar.
- [ ] **AC17 — refusals name the alternative.** `transition.fill` on a track that plays
  nothing names the tracks that do; a `set_song` or `add_to_song` silence refusal names the
  tracks that would stop (already names the section — add the tracks); `jump_to`'s reply
  names the tracks the target row stops, as `make_section` already names the empty ones.

### Phase 4 — hearing before the room does
- [ ] **AC18 — `get_context {section}`.** Reads the row's clips (`get_clip_notes` per
  track, sliced) and reports per track: onsets per bar, pitch range as note names, and the
  registers two tracks share; the total onsets per bar against the section that last ran;
  the row's last master peak from the level table when it has one (prototype scene 8).
- [ ] **AC19 — density in every section reply.** `make_section` and `build_song` end with
  the same per-track density lines for the rows they wrote, computed from the notes just
  written (no read-back).
- [ ] **AC20 — the limit said once.** `capture_mix`'s description states that Live's API
  cannot render a section that is not playing and points to `get_context {section}`; no
  other description repeats it.

### Phase 5 — the setlist says when you left it
- [ ] **AC21 — the off-setlist line.** A `jump_to` to a section outside the setlist ends
  its first paragraph with: it is not in the setlist, it holds until go, what
  `next_section` resumes, and the `add_to_song {section, after}` call that splices it in
  after the entry the song left. With `repeats`, the line says where and at which bar the
  song resumes, ahead of the plan lines.
- [ ] **AC22 — `back` from a detour.** `back` after an off-setlist jump keeps working as
  today (the jump history), and its reply carries the same line when the destination is
  outside the setlist.

### Phase 6 — one name per tool
- [ ] **AC23 — the names test.** A test reads every `snake_case` token of `INSTRUCTIONS`,
  `FOOTER`, every served description and the fixed reply strings named in Current State,
  and requires each to be a served tool name (bare for core, `adv_` for the rest), a
  parameter of the tool it appears in, or a Remote Script command inside an `adv_`
  description. The eight bare performance-layer names in the instructions and the three
  bare `listen` replies fail it before the fix.
- [ ] **AC24 — promotions.** `listen`, `get_drum_rack_pads` and `get_clip_notes` join
  `CORE_TOOLS` (Play, Build, Build) under their current names; `tests/artist.rs` pins the
  new list; decision 0006's group list and `docs/technical/feature-matrix.md` are updated in
  the same PR; `arrange` stays.
- [ ] **AC25 — `annotations_for`.** `listen` stays read-only; `delete_section` is
  destructive by its prefix; a test asserts both.

### No Regressions
- [ ] **AC26 — `get_context` stays one call**: tempo, key, tracks with instruments, clips
  by slot, sections, setlist, levels and the clock in one round trip (`src/context.rs`);
  `section` is a parameter of the same tool, not a second tool.
- [ ] **AC27 — the section is the row.** A section launch stops tracks without a clip in
  that row unless `adv_keep_track_playing`; nothing here changes it.
- [ ] **AC28 — the clock and plan lines** ride on every reply while performing, never
  opt-in (`run_blocking`, `performance::clock_lines`).
- [ ] **AC29 — the transition compositor** (`src/transition.rs`) keeps composing tempo,
  retime, crossfade, fill, drop and sweep into one cue; AC16 changes its refusals and
  landing, not its steps.
- [ ] **AC30 — the old note forms keep working**: every test in `src/notes.rs`,
  `tests/clip_notes.rs`, `tests/song.rs` and `tests/orchestration.rs` that writes `steps`,
  `notes_csv`, `patterns`, `loop_every`/`until` passes unchanged; `batch` steps written with
  either spelling of a tool keep working.
- [ ] **AC31 — the Remote Script touches Live only on its main thread**, in slices
  (decision 0007): `delete_scene` and `duplicate_scene {at}` run through `_dispatch`; the
  clip copies in `duplicate_scene {at}` yield per track; every reply carries `main_ms`.
- [ ] **AC32 — the tool count and command count** in the source-of-truth snapshot are
  updated and re-dated in the same PR (`scripts/check-docs-facts.sh`).

## Affected Files

### Modified
| File | Change |
|------|--------|
| `src/notes.rs` | `bars`, `bar_steps`, `octave`, `chords`, `each`, `voicing`, `bass`, `rhythm` on `NotesInput`; length check across step lines; `^` degrees; chord symbol and Roman-numeral parsing and voicing; `expand(input, key, beats_per_bar)`; the write summary (per pitch and bar, digit velocities, resolved chords) |
| `src/tools.rs` | `add_notes_to_clip`, `create_clip`, `build_song` pass the key and signature into `expand` and print its summary; `get_context {section}`; `delete_section` body and `#[tool]`; `CORE_TOOLS` + three; descriptions per the section above; the beat warning in `run_blocking`'s tail; the three bare `listen` strings; `ALL_REMOTE_COMMANDS` + `delete_scene` |
| `src/sections.rs` | `changes` notes on an empty slot say "added"; `after` for a copy; empty spare rows; density lines in the reply; the off-setlist line in `steer`'s reply; the tracks a jump stops; `delete_section` body (guards, setlist edit) |
| `src/transition.rs` | `fill` and `drop`: fit-before-phrase-end, clamp or refuse-with-answer; the playing tracks named in the silent-track refusal |
| `src/context.rs` | `INSTRUCTIONS` MAKING MUSIC and PERFORMING paragraphs, the COST paragraph; `FOOTER` |
| `src/performance.rs` | `PerfState` exposes `key()` as (root, scale) for `expand`; onset and range helpers for the density readout (pure) |
| `src/song.rs` | `remove_sections(entries, names)` for `delete_section`; the off-setlist words |
| `AbletonMusicMaker_Remote_Script/__init__.py` | `delete_scene` handler (shifts the phrase and level tables, cancels or re-points cues on the row); `duplicate_scene {at}` (create + `duplicate_clip_to` per track, a generator); `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` |
| `tests/artist.rs` | the names test; the new `CORE_TOOLS`; annotations for `delete_section` |
| `tests/clip_notes.rs` | `bars` tiling, mixed-length refusals, `bar_steps`, degrees, chords through `add_notes_to_clip` and `create_clip` with a `FakeBridge` state that carries a key |
| `tests/song.rs` | `make_section` add / chain / `after` / empty spare; `delete_section` guards and setlist edit; the off-setlist reply line; transition fit and refusal text |
| `tests/performance.rs` | the beat warning line from a reply with `main_ms` over a beat |
| `tests/orchestration.rs` | `get_context {section}` readout; `build_song` density lines |
| `docs/decisions/0006-one-artist-surface-raw-layer-marked-advanced.md` | the group list gains `listen`, `get_drum_rack_pads`, `get_clip_notes` (dated) |
| `docs/architecture/overview.md`, `docs/technical/feature-matrix.md` | the note forms, `delete_section`, `duplicate_scene {at}`, `get_context {section}`, the names test |
| `docs/facts/source-of-truth.md` | tool count, command count, script version; re-dated |
| `README.md`, `CLAUDE.md` | the tool count where they repeat it (the check script) |

### New
| File | Description |
|------|-------------|
| `src/harmony.rs` | Pure: key → pitch classes; degree and chord-symbol / Roman-numeral parsing; voicings; note-name rendering. Unit-tested on known chords (Fm9 in F minor, VI△7 → Db△7) |

## Remote Script compatibility

- [ ] `delete_scene {index}` handler in `AbletonMusicMaker_Remote_Script/__init__.py`: main thread via
  `_dispatch`; deletes the row, shifts the phrase table and the per-scene level table, cancels or
  re-points pending cues that fire the row (says which in the result); no f-strings, no type
  hints, no third-party imports
- [ ] `duplicate_scene {at}`: `create_scene(at)`, then `duplicate_clip_to` per track as a
  generator (`yield None` per track), name and phrase set, tables shifted; without `at` the
  existing path
- [ ] Names in `SCRIPT_CAPABILITIES`: `delete_scene` (new); `duplicate_scene` unchanged
- [ ] `SCRIPT_VERSION` bumped (from whatever `main` holds when this lands)
- [ ] `delete_scene` added to `tools::ALL_REMOTE_COMMANDS`
- [ ] `delete_section_body` calls `require(live, "delete_scene")`; `make_section` with
  `after` on a copy checks the script version for `at` support and otherwise refuses with
  today's message plus "update the Remote Script"
- [ ] Live version floor: `Song.delete_scene`, `Song.create_scene(index)` and
  `ClipSlot.duplicate_clip_to` are in the LOM for Live 10 and 11+ (local copy); works on
  Live 10; `duplicate_clip_to` on an audio clip verified on 12.4.6 in the manual pass
- [ ] Timeout class: `delete_scene` modifying (15 s, `MODIFYING_COMMANDS`); `duplicate_scene`
  stays modifying

## Privacy

No new local data. The density readout is computed from notes the server reads or writes in
the call and is not stored; the activity line keeps the tool name, commands, `main_ms` and
sizes as today (`tests/activity.rs` unchanged). The measured cost figures in the descriptions
are constants in the source, not something the server records or reports about the machine.
Nothing uploads.

## Test Coverage

| Suite / script | Change | AC |
|----------------|--------|----|
| `src/notes.rs` unit tests | `bars` tiling; a line that divides the span; mixed lengths refused with pitch, length, expected; `bar_steps`; digit velocities in the summary; `notes` objects with named pitches; degrees in F minor and D dorian with `octave` and `+1`; chord symbols and Roman numerals to notes per voicing; out-of-key root flagged | AC1–AC8 |
| `src/harmony.rs` unit tests | pitch classes per scale name Live uses; `Db△7` = `Dbmaj7`; `VI△7` in F minor = Db△7; `rootless` drops the root; `bass` adds it an octave down | AC6, AC7 |
| `tests/clip_notes.rs` | `add_notes_to_clip` and `create_clip` with the new fields through `run()` and a `FakeBridge` state carrying `root_note`/`scale_name`; the refusal when no key is set; the reply summary | AC1–AC8 |
| `tests/song.rs` | `make_section` `changes` on an empty slot creates and says "added"; from a copy; `after` sends `duplicate_scene {at}`; empty spare row; `delete_section` keeps the playing one, deletes the rest, rewrites the `Setlist:` scene; transition clamp and refusal texts (fits at the next phrase end; explicit bar refused with earliest); `jump_to` off-setlist line with and without `repeats`; the tracks a jump stops | AC9–AC13, AC16, AC17, AC21, AC22 |
| `tests/performance.rs` | a reply with `main_ms` above one beat at 130 BPM carries the warning line; below it does not | AC15 |
| `tests/orchestration.rs` | `get_context {section}` readout on a synthetic row; `build_song` reply ends with density lines; `tool_count_and_schema_defaults` count + 1 | AC18, AC19, AC32 |
| `tests/artist.rs` | the names test (fails on the current instructions before the fix, passes after); `CORE_TOOLS` + 3 served bare and not as `adv_`; `delete_section` destructive, `listen` read-only | AC23–AC25 |
| `scripts/check-docs-facts.sh` | snapshot rows for tools, commands, script version | AC32 |
| `docker/verify-image.sh` | unchanged — no Dockerfile change | — |

## Implementation Notes

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| Compact forms expand to plain notes in one pure function | `notes::expand`, `src/notes.rs:325-363` | `bars`, `bar_steps`, degrees and chords: still one `expand`, now given the key and signature |
| Word vocabularies resolved against a table, unknowns listed | `src/sound.rs` | chord-symbol and Roman-numeral parsing: a table of qualities, the error lists what parses |
| A change validated per track before the first Live command | `make_from_section`, `src/sections.rs:320-329` | `delete_section` checks every name and the playing/queued guard before deleting any |
| The playing-or-queued guard | `delete_clip`, `src/tools.rs:3850-3860` | `delete_section` per section |
| Generator handlers, one unit per slice | `_write_clips`, `__init__.py:1983` | `duplicate_scene {at}`: one `duplicate_clip_to` per track per slice |
| Scene tables shifted on insert | `_shift_scene_tables`, `__init__.py:3610` | the delete: the inverse shift, and the cue re-point |
| Landing resolved once, phrase end known | `resolve_landing`, `src/sections.rs` | the transition fit: `phrase_end - bars >= next_bar`, else the next phrase end (`ends_bar + bars_of_phrase`) |
| One reply tail for every tool | `run_blocking` renders the clock and level lines | the beat warning from `main_ms` and the tempo already in the clock |
| Every served name pinned by a test | `tests/artist.rs:538-575` | the names test over the instruction and description text |

### Design Decisions
- **`bars` + `bar_steps` rather than a second grammar.** The notes' `{"bar", "repeat",
  "except"}` object and `steps` would be two ways to write a step line. One grammar, one
  field for the span, one for the exceptions; `loop_every`/`until` stay for generated
  material and leave the first paragraph of the description.
- **Refuse lengths that do not divide the span, not all unequal lengths.** A two-bar bass
  under a one-bar hat is the common case in this music; refusing it would make `bars`
  useless for it. The defect the notes name — a 128-step line accepted under 16-step lines
  in a one-bar clip — is refused either way.
- **Degrees carry a marker.** `"5"` is already MIDI note 5 wherever a pitch is read;
  changing that silently would rewrite every existing call. The caret is the theory
  notation for a scale degree and costs one character.
- **Chords expand on the server.** The key lives there (`set_key`, the state read); the
  caller cannot be in the wrong key by construction with Roman numerals, and sees what it
  got with the resolved notes in the reply. Expansion in the caller is what produced
  twenty-two hand-computed rows.
- **`delete_section` is a tool.** Decision 0006 admits a tool for a new intent; "remove this
  row" is one, it has no home on `make_section`, and its name gives it the destructive
  annotation. It joins `delete_clip` and `delete_track` in housekeeping.
- **Placement without a move.** Live's API has no scene move; `create_scene(index)` +
  `duplicate_clip_to` per track is the copy the API does allow anywhere. The fast
  `Song.duplicate_scene` path stays for the default because it is one call.
- **Warn, do not refuse, a long hold mid-set.** The piano break the room asked for was the
  case. The reply says what it cost and what to do next time; the description said what it
  would cost. Refusing is for calls that break the performance model (stopping the
  transport), not for ones that cost time.
- **Clamp a phrase-timed transition, refuse an explicit bar.** "At the end of the phrase"
  is an intent the server can honour at the next phrase; "at bar 64" is a fact the
  performer stated and the server should not move.
- **Density from notes, not audio.** The API cannot render an unplayed row; the notes the
  server wrote are exact. Onsets per bar and shared registers are what "too busy" means
  before it is heard. Per-track spectra are named as out of scope rather than implied.
- **No `after: "hold"` on `jump_to`.** Omitting `repeats` already holds. Adding a word for
  the default would make two spellings of one thing; the missing piece was the sentence.
- **Promote under current names.** Renaming `get_drum_rack_pads` to `drum_pads` breaks
  `batch` callers and the `get_` annotation rule for one shorter word. `arrange` stays core
  because 0006's criterion is the artist's set, and the Arrangement is the artist's when
  composing; "used during a show" is a different cut and would move `build_song` too.

## Verification

### Manual verification steps
With Live 12 open, the Remote Script reinstalled and Live restarted.
1. Empty set, "set the key to F minor", then `add_notes_to_clip` with `bars: 8`, one 16-step
   line and a `bar_steps` bar 8 (prototype scene 1). Live's clip editor shows an 8-bar clip;
   bar 8 differs; the reply prints the digit velocities.
2. Send the mixed-length call of scene 2: refused, nothing written; the error names D1, 128
   steps, 8 bars, expected 16.
3. `make_section` with the four chord symbols of scene 3: the clip editor shows the voicing,
   in F minor, bass note an octave below; the reply lists the notes. Change one symbol to
   `E7`: written and flagged.
4. Copy a section with `after` naming a section two rows up: the row appears there, the
   clips keep their colours and names; the source row is untouched. Copy again from the copy.
5. Make three empty spares before starting; `play_song`; fill one with `replace: true`
   during the set: the reply says what it held Live for and the output does not click.
6. Create eleven sections, put two in the setlist, `delete_section` all eleven while one
   plays: ten go, the playing one is named, the song line no longer shows the two.
7. At bar x.3 of a phrase, `jump_to` with a 1-bar drop: it lands at the phrase end with the
   drop the bar before, or at the next phrase end with the reason. Repeat with `at` a bar
   that cannot fit: refused with the earliest bar.
8. `get_context {section}` on an unfired section with two hat layers in the same register:
   the shared range is named; fire it; compare with what is heard.
9. Load an instrument mid-set: the reply carries the beat warning; `main_ms` in the
   activity line matches.
10. `jump_to` a section not in the setlist: the reply's line names the hold and the
    `add_to_song` call; `next_section` resumes where it said.
11. Start a fresh client: the instructions name `adv_cue` and `listen`; call `listen` bare.

## Out of Scope
- Per-track spectral balance or offline rendering of a section: Live's API cannot render a
  row that is not playing and Resampling is the master; per-track routing into the Capture
  track is a different story.
- Background instrument loading or a second instrument swapped at a bar line: no such call
  in Live's API; the answer is spare tracks made before the show, said in the description.
- A move-scene primitive: not in the API; placement is a copy at an index.
- Renaming served tools (`drum_pads`, `read_clip`): promotion under current names instead.
- Demoting `arrange` or any other core tool.
- Chord-scale suggestion, harmonisation of a melody, or a voicing designer: `voicing` is
  three words.
- Rewriting existing clips when the key changes: `adv_follow_key`.
- The step grammar's expressive characters (chance, velocity range): the
  `song-writing-feel-notes-groove-quantize-record-undo` story.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| Decision 0007 (`main_ms` on every reply, sliced handlers) | Shipped (Remote Script 1.23.0) | phase 3's warning and the sliced clip copy stand on it |
| `Song.delete_scene`, `Song.create_scene(index)`, `ClipSlot.duplicate_clip_to` | LOM (local copy) | `duplicate_clip_to` on audio clips to verify on 12.4.6 |
| The key in `PerfState` (`root_note`, `scale_name`) | Shipped | degrees and chords read it in the same call |
| `build_song` `scenes`, `make_section replace: true` | Shipped | pre-allocation is documented, plus the empty-spare gap (AC12) |
| Open questions 1–15 | Waiting on the review of the transcript | ACs may move |

## Related Stories
- `song-writing-feel-notes-groove-quantize-record-undo` — the per-note expressive fields
  (chance, velocity range) and `edit_notes` extend the same `NotesInput`; land this story's
  `bars`/degrees/chords first so both read one grammar.
- `app-taps-live-output-spectrum-and-meters` — the app's spectrum is the audio answer to
  "is it too busy" *while it plays*; phase 4 here is the notes answer *before* it plays.
- `mac-app-installs-runs-and-watches-the-server` — the activity view shows `main_ms`; the
  descriptions' cost figures come from the same numbers.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from the performer's field notes after the third real set (eight items and five things not to regress), verified against the code; every item mapped to the artist tool it extends, one new tool (`delete_section`), three promotions |
