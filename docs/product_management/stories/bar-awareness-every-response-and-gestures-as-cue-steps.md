# Bar awareness on every response, live moves as single cue steps, and instant browser search

## Story
**As a** producer performing a set with Claude at my side,
**I want** Claude to know where we are in musical time from every reply it gets, to place
anything at a phrase or a bar without a second call, and to make a breakdown, a drop or a
sweep as one move,
**So that** it never misses a bar line, every layer change lands on a phrase boundary, a
live gesture is one cue step rather than three to six calls racing the bar, and finding a
sound never costs a round trip once the library has been seen once.

## Details
| Field | Value |
|-------|-------|
| Status | `In Progress` — all seven phases built against the assumed answers (Remote Script 1.16.0); real-Live pass pending; open questions 1, 3, 6, 9 and 11 still want the producer's word |
| Surface | [Decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md): the clock and level lines ride on every reply of every tool; `vary_clip`, `undo_vary`, `listen`, `snapshot_mix`, `restore_mix`, `follow_key`, `cue` gestures and `set_scene` are the raw layer (served as `adv_…`). The artist reaches them through `feel` (variations, one undo), `capture_mix`, and the song verbs; `listen` reports dB |
| Priority | P1 — the first real set (2026-09-19) named this as the remaining gap after `get_context`, `slots` and `keep_track_playing`; item 1 is the foundation the other items stand on |
| Size | L — seven phases below; phase 1 alone delivers the core ask and must ship first; phase 6 (browser search) is independent of the others and can ship in any order. Each phase gets its own `-impl` plan |
| Tracker | none yet — an issue per phase when the story is `Ready` |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Prototype | [prototypes/bar-awareness-every-response-and-gestures-as-cue-steps.md](../prototypes/bar-awareness-every-response-and-gestures-as-cue-steps.md) |
| Depends on | `perform-live-build-launch-and-transition-on-the-bar` (Remote Script 1.15.0, verified 2026-09-19 against the working tree: the cue clock, `get_performance_state`, `get_context`, `keep_track_playing`) |

## Context

### The Problem
The performance layer works: cues run on Live's clock, launches land on the bar, the state
readout says what happened. What the first real set exposed is that the assistant is blind
between calls. A `fire_clip` or `set_send` reply says nothing about time, so knowing whether
a move made the bar costs another round trip, and each round trip costs about 200 ms
whatever it asks (measured 2026-09-19 on Live 12.4.6: an unknown command, a small read and
`get_context` all answer in the same 200 ms; `docs/architecture/overview.md`). A bar at
126 BPM is 1.9 s: nine round trips, and the assistant's own reply is several more. So the
assistant plans from stale information, cannot say what a "phrase" is, and turns a
breakdown into a sequence of calls that may or may not straddle the bar line.

The producer's own success test: a full set where the assistant never misses a bar line,
every layer change lands on a phrase boundary without a follow-up state call, and a
breakdown or a drop is one cue step.

### Current State
- Every tool result is the body's text and nothing else: `run_blocking`
  ([`src/tools.rs:3879`](../../../src/tools.rs#L3879)) writes the activity line and returns
  the text as it is. The clock exists only in `get_performance_state`
  ([`state_text`](../../../src/performance.rs#L930)) and `get_context`.
- The Remote Script answers every command from a thread that can read the transport at zero
  cost, but the response envelope carries only `status` and `result`
  ([`_handle_client`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L250)); the server's
  `exchange` ([`src/connection.rs:350`](../../../src/connection.rs#L350)) returns `result` and
  drops the rest. `CallTrace` ([`src/connection.rs:53`](../../../src/connection.rs#L53))
  already measures the round trip of every command per call, but only the activity line sees it.
- `fire_clip` ([`src/tools.rs:1224`](../../../src/tools.rs#L1224)) reports "Started playing
  clip at track 4, slot 2" and nothing about the bar it lands on; `fire_scene` predicts the
  landing bar from the state read *before* the fire, so it is wrong whenever the bar line
  passes in flight.
- Cue times are `"next_bar"`, `{"bar": N}`, `{"bars_after": k}`
  ([`CueTime`](../../../src/performance.rs#L381), [`resolve_time`](../../../src/performance.rs#L479)).
  There is no notion of a phrase anywhere: no per-scene length, no record of the bar a scene
  was fired on. Sub-bar times do not exist.
- The script fires a cued launch inside the bar before its target
  (`LAUNCH_LEAD_MARGIN`, [`__init__.py:2929`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L2929);
  [`_launch_lead`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L3485)); cued moves
  are therefore bar-exact, immediate ones are not.
- Cue actions are primitives only — `fire_scene`, `fire_clip`, `stop_clip`, `stop_all_clips`,
  `set`, `ramp`, `stop_playback` ([`_run_cue_action`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L3530)).
  A breakdown is three `stop_clip` steps and three `fire_clip` steps the assistant must
  work out by hand, after a state read to learn which slots play.
- `play_and_measure` and `capture_mix` are refused during a performance because they stop
  and move the transport (`guard_performance`); `get_track_meters`
  ([`src/tools.rs:2165`](../../../src/tools.rs#L2165)) is allowed but is one reading, and
  nothing reads audio without stopping the set. `_start_capture`
  ([`__init__.py:2093`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L2093)) stops
  the transport before it fires, by design for the Arrangement; the fixed-length fire itself
  needs no transport move — `record_clip` proves it.
- `batch` substitutes only `$last_track` ([`substitute`](../../../src/tools.rs#L3507)).
  `build_song`'s `SongClip.slot` defaults to 0 even when `slots` is given
  ([`src/tools.rs:613`](../../../src/tools.rs#L613)), so a clip meant for rows 1–2 also lands
  in row 0. `build_song` has no scenes block: rows are unnamed unless the producer names them.
- Browser search is a round trip per query. The script walks the browser once per category
  per Live session and keeps the index in memory
  ([`_browser_index`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L1757), 8 s budget
  per call, continued across calls), so the first search pays the walk and later ones are
  instant *on the Live side* — but each still costs the 200 ms round trip, the index dies
  with Live, and one query per call means the assistant sends four to six searches to
  resolve a set's instruments (the activity logs of 2026-09-19 show five and four searches in
  single sessions). `search_browser_body` ([`src/tools.rs:1903`](../../../src/tools.rs#L1903))
  forwards one query and renders the hits; nothing is cached on the server.
- Already shipped from the request's item 6, 2026-09-19: every nested schema type is inlined
  (a unit test fails on any `$ref`), `get_drum_rack_pads` walks into a Drum Rack nested in an
  Instrument Rack, plain instrument words are searched.

### Root Cause
Time is a side channel today: the script knows the bar on every command and throws it away;
the server measures the round trip on every command and files it. Everything the assistant
needs to stay on the bar is already produced, just not returned.

## Open Questions

1. **What is a phrase?** The request says "based on the playing clips' loop lengths"; a
   producer says 8 or 16 bars per section. Loop lengths disagree with each other (a 1-bar
   hat loop under a 16-bar pad) and Live has no phrase concept. → **Assumed: a phrase is a
   property of the scene (`phrase_bars`, default 16, set on `create_scene`, `set_scene` or
   by name in `build_song`'s scenes block), counted from the bar the scene was fired on.
   The longest playing clip loop is shown in the bar map as a hint, never used as the
   phrase.** *Needs your answer.*
2. **Counted from where when the producer fires a scene from Live's UI?** The script does
   not see UI launches. → **The clock's tick records the bar on which any track's
   `playing_slot_index` changes to a new scene row; that bar becomes the scene's start.
   Precision is one tick (about 100 ms), which is inside the bar.**
3. **Where does the clock line go?** Every result, or only while a performance runs?
   Outside a performance the transport is usually stopped and the line is noise. →
   **Assumed: every result while a performance runs (started by `start_performance`, or
   adopted when the script reports performance mode after a server restart), including
   error results; never otherwise.** *Needs your answer.*
4. **Does the clock cost a round trip?** No: the script attaches it to the response
   envelope of every command while performance mode is on; the server renders a field it
   already received. `get_performance_state` remains the full readout.
5. **Latency compensation: refuse, or shift?** → **Report, never guess: the script computes
   the landing bar after the fire, from the transport. `no_later_than: N` on `fire_clip`,
   `fire_scene` and `record_clip` makes the server predict: if the bar is closer than the
   measured round trip plus 150 ms, it schedules a cue for the next certain bar instead of
   gambling, and says so. Without `no_later_than` an immediate launch is issued as today
   and the reply says which bar it made.**
6. **Which gestures, and where are they expanded?** → **Server-side, into the primitive
   steps the script already runs, so the script changes nothing for gestures and the state
   readout shows every primitive. Phase 2 ships `breakdown`, `drop`, `mute_except`, `sweep`,
   `build` (send ramp plus one clip swap) and `panic`. Half-time, double-time and hat
   density need a new clip and belong to `vary_clip` (phase 4).** *Needs your answer on the
   list.*
7. **Numeric listening: meters or audio?** Meters are free and live-safe but coarse (Live's
   output meter, not dBFS). Audio needs a recording. → **`listen` reads meters over a bar
   by default; `capture: true` fires a fixed-length recording on the Capture track on the
   next bar, exactly as `record_clip` does, without touching the transport, and measures
   the file (peak, RMS, low/mid/high balance — the band split is a small FFT in
   `src/audio.rs`, no new dependency). The `capture_mix` guard stays: it moves the playhead.**
8. **Following the performer: MIDI input or recorded clip?** A control surface only sees
   MIDI on its own input port, and this script has none. → **From the recorded clip:
   `record_clip` produces notes; `follow_key` reads them (`get_clip_notes`), estimates the
   key from the pitch-class histogram, and transposes the assistant's own clips.
   The "incoming MIDI summary" becomes a summary of the last recorded clip.**
9. **A limiter on the master: whose?** Live's own Limiter (Suite, Standard) is the safe
   choice; Intro lacks it. → **`start_performance {"limiter": true}` loads Live's Limiter
   on the master through `load_browser_item` with `kind: "master"`, ceiling −0.3 dB, and
   says so; if the browser has no Limiter it says that and continues.** *Needs your answer.*
10. **`$last_clip` in `cue`?** A cue step does not create clips (creation is immediate, not
    timed), so `$last_clip` is a `batch` feature: the slot of the last `create_clip`. In
    `cue`, `{"fire_clip": "$last_clip"}` resolves against the same batch. → **Assumed as
    stated; a cue outside a batch has no `$last_clip`.**

11. **Should the browser index live on disk?** In memory it dies with the server; on disk
    it is a new kind of local data (the names, folder paths and URIs of the producer's
    library — no audio, no notes). → **Assumed: on disk under `state_dir()/library/`, on by
    default like the activity log, `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory
    only, "Delete all local data" removes it. `TERMS.md` gains a paragraph.** *Needs your
    answer.*
12. **When is the index stale?** Packs install, the User Library changes. → **Keyed by Live
    version plus the pack list from `get_library_status`; refreshed on `search_browser
    {"refresh": true}`, whenever a `load_browser_item` by a cached URI fails, and in the
    background after every handshake (cheap: the walk resumes where the key matches).**
## Prototype
[prototypes/bar-awareness-every-response-and-gestures-as-cue-steps.md](../prototypes/bar-awareness-every-response-and-gestures-as-cue-steps.md)
— the clock line on an ordinary reply, a launch that reports the bar it made (and one that
missed, and one refused with `no_later_than`), the bar map, phrase-based cue times, a
breakdown as one step, `listen`, the mix snapshot, and the failure cases.

Not yet reviewed. What writing it changed: the landing bar is *computed after the fire* by
the script rather than predicted by the server, because the prediction was exactly the
guess the request asks to remove; and gestures expand on the server so the readout can show
every primitive step.

## Tool description

```text
Every result while a performance runs ends with one line:
⏱ bar 39.2 · next bar in 2.2 s · phrase ends bar 48 · next cue: bar 51 fire scene 'Break' (cue 4)
(or "⏱ stopped at bar 88.3 (transport stopped outside this server)"). It is the Remote
Script's own reading at the moment it answered; no extra call is made.

fire_clip / fire_scene / record_clip: … The reply says which bar the launch lands on,
computed by the Remote Script after the fire from where the transport is. no_later_than: N
refuses to gamble: if bar N is closer than the measured round trip plus 150 ms, the launch is
scheduled as a cue for the next certain bar instead, and the reply says so.

cue times: "next_bar", {"bar": N}, {"bars_after": k}, "next_phrase", {"phrases_after": n},
and {"bar": N, "beat": b} for a beat inside the bar (only with a launch quantization finer
than a bar; refused otherwise with the quantization to set).

cue gesture step: {"gesture": {"breakdown": {"keep": [tracks], "bars": 8}}} — also drop
{bars, keep}, mute_except {keep, bars}, sweep {track, device_index, parameter_index, from,
to, bars}, build {track, send, to, bars, hats?}, panic {keep, bars}, restore_mix {snapshot}.
A gesture expands into the primitive steps the script runs (stop/fire the slots that are
playing when the cue is made, sets and ramps); the plan lists them. Refused when a kept
track is silent or a track has nothing to restore.

get_performance_state {"bar_map": true}: adds the next 32 bars with every cue step and
phrase boundary. Scenes show their phrase length.

create_scene / set_scene: phrase_bars (default 16). build_song "scenes": [{name, tempo,
phrase_bars}] names the rows.

listen {"bars": 1, "capture": false}: per-track and master peak and average over the bar
from Live's meters (never touches the transport); capture: true records the bar through the
Capture track on the next bar and adds low/mid/high balance and RMS from the file.

vary_clip {track, clip, "variation": "fill_last_bar" | "ghost_notes" | "invert_chords" |
"thin" | "half_time" | "double_time", "seed": n, "to_slot": m}: writes a varied copy into
to_slot (or in place); undo_vary restores the previous notes. follow_key {"from": {track,
clip}}: estimates the key of a recorded clip and transposes the assistant's clips to it.

snapshot_mix / restore_mix {id}: volume, pan, sends, mute of every track, return and master
in one round trip; restore_mix is also a cue action.

start_performance {"limiter": true}: Live's Limiter on the master at −0.3 dB.

batch: $last_clip is the slot of the last create_clip; cue steps inside a batch may fire it.

search_browser {"query": …} or {"queries": ["analog bass", "techno kit", "evolving pad"]},
category, limit, best: true (one URI per query, ready for load_instrument_or_effect or
build_song): answered from the server's own index of the browser — no round trip to Live once
the index is complete — with the words matched against name and folder path (so "pad",
"sub", "break" work as tags). The index is walked in the background after the handshake,
kept on disk under the state dir, refreshed with refresh: true. get_context says how many
items are indexed and whether the walk is complete.
```

## Acceptance Criteria

### Phase 1 — the clock on every response (the core ask)
- [ ] **AC1 — Performance mode in the script.** `set_performance_mode(on)` is called by
      `start_performance` / `end_performance`; while on, every command's response envelope
      carries `clock: {beat, bar, beat_in_bar, tempo, beats_per_bar, is_playing,
      seconds_to_next_bar, phrase: {scene_index, started_bar, bars, ends_bar} | null,
      next_cue: {cue_id, bar, label} | null, round_trip_hint: null}`. `get_script_info`
      reports `performance_mode` so a restarted server adopts it.
- [ ] **AC2 — The envelope reaches the tool.** `exchange` keeps `clock` in the thread-local
      trace next to the round-trip time; `run_blocking` appends the rendered line to every
      result text, success or error, when a clock was received. No tool body changes for this.
- [ ] **AC3 — The line.** `⏱ bar B.b · next bar in S s · phrase ends bar P · next cue: bar N
      <label> (cue id)`; "phrase ends" omitted when no scene has been fired; "next cue"
      omitted when none is pending; `⏱ stopped at bar B.b (transport stopped outside this
      server)` when not playing. Under 120 characters.
- [ ] **AC4 — Landing bar after the fire.** `fire_clip`, `fire_scene`, `record_clip` in the
      script read the transport after the fire and return `lands_on_bar`, computed from the
      global quantization grid (for 1 bar: the next bar; for n bars: the next multiple; for
      finer: the current bar). The tool text says "lands on bar N (issued at B.b)"; when the
      launch arrived after the bar the caller aimed at (the beat read at issue is more than
      half a beat into a new bar compared with the state the server last read), it says so
      and gives the round trip.
- [ ] **AC5 — `no_later_than`.** On the three launch tools: if `seconds_to_next_bar` from the
      last clock is less than the trailing-average round trip (from `CallTrace`) plus
      0.15 s, the server schedules a one-step cue for the next certain bar and returns the
      cue id; otherwise it fires. The reply names the bar either way.
- [ ] **AC6 — Phrases.** `phrase_bars` on `create_scene` and a new `set_scene` (name, tempo,
      phrase_bars); default 16; stored in the script per scene index (in memory, per set —
      lost on reload, and the readout says "default" then). The script records
      `scene_started_bar` when it fires a scene, when a cue fires one, and when the tick sees
      a row change it did not cause. `phrase.ends_bar = started_bar + k × bars` for the
      smallest k that is ahead.
- [ ] **AC7 — Phrase cue times.** `"next_phrase"` and `{"phrases_after": n}` resolve from the
      current phrase; refused with the prototype's text when no scene has been fired.
      `{"bar": N, "beat": b}` resolves to `(N-1)·bpb + (b-1)`; refused when the global
      quantization is a bar or coarser, naming the quantization to set.
- [ ] **AC8 — Bar map.** `get_performance_state {"bar_map": true}` renders the next 32 bars:
      one line per bar that has a cue step or a phrase boundary, ramps as ranges; the
      longest playing clip loop is shown once as a hint.
- [ ] **AC9 — Round trip in the open.** `get_performance_state` and `get_context` report the
      trailing-average round trip ("round trip 0.21 s"); the clock line does not (length).

### Phase 2 — gestures
- [ ] **AC10 — `gesture` step.** Exactly one gesture per step; expanded in
      `performance::resolve_cue` into primitive steps using the fresh state: which slot each
      track plays (or has queued) at cue time. `breakdown {keep, bars}`: `stop_clip` on every
      playing track not in `keep` at the step's bar, `fire_clip` of the same slots `bars`
      later. `drop {bars=1, keep}`: same with the default. `mute_except {keep, bars?}`:
      `set mute` and back. `sweep`: one `ramp` on a device parameter. `build {track, send,
      to, bars, hats?}`: a send `ramp` plus, when `hats` is given, a `fire_clip` of the next
      slot with a clip on that track at the last bar. `panic {keep, bars=1}`: volume ramps to
      0 on every other track then `stop_clip`.
- [ ] **AC11 — Refusals before Live.** A kept track that plays nothing (and has nothing
      queued); a gesture with nothing to stop; `sweep` on a missing parameter; `hats` on a
      track with one clip. Each names the fix.
- [ ] **AC12 — Readout.** The cue result and the state readout show the gesture name and its
      primitive steps ("= 6 primitive steps"); the silence check runs over the expansion.

### Phase 3 — listening without stopping
- [ ] **AC13 — `listen {bars, capture}`.** Meters: `get_track_meters` every 250 ms for
      `bars` bars (from the next bar boundary, so a bar is a bar), peak and mean per track,
      return and master, master's distance to 1.0 flagged under 0.5 dB. Never stops or
      moves the transport; allowed during a performance.
- [ ] **AC14 — `capture: true`.** A new script command `start_live_capture(bars, name)`:
      the Capture track's next free slot fired with `record_length` on the global
      quantization while the transport keeps running (the `record_clip` path, on the Capture
      track), polled with `capture_status`, measured by `src/audio.rs` plus a three-band
      split (low < 200 Hz, mid, high > 4 kHz, energy share of each). The `capture_mix` guard
      is unchanged.

### Phase 4 — variation and following
- [ ] **AC15 — `vary_clip`.** Server-side note transforms on `get_clip_notes` →
      `add_notes_to_clip {clear: true}` (or `create_clip` into `to_slot`): `fill_last_bar`
      (16th-note repeats of the clip's own pitches across the last bar), `ghost_notes`
      (velocity-30 notes on empty 16ths of the drum pitches, density 0.2), `invert_chords`
      (each simultaneous group up one inversion), `thin` (drop every other off-beat note),
      `half_time` / `double_time` (time-stretch the note grid, clip length kept). Seeded
      (`seed`), so the same call is reproducible; the previous notes are kept in server
      memory per clip for `undo_vary` (one level).
- [ ] **AC16 — `follow_key`.** Pitch-class histogram of the recorded clip → best of 24
      keys (Krumhansl profiles); transposes every clip the assistant created in this
      performance (tracked in `Performance`) by the interval to the new root, and sets Live's
      scale. `start_performance {"follow_key": true}` runs it after every `recording_done`
      event. The state readout gains "last recording: 4 bars, 37 notes, D minor likely".

### Phase 5 — safety nets
- [ ] **AC17 — `snapshot_mix` / `restore_mix`.** One script command each: every track's,
      return's and master's volume, pan, sends, mute (arm and solo excluded) into a numbered
      snapshot in script memory; `restore_mix` is also a cue action, so it lands on a bar.
- [ ] **AC18 — `panic`** as a gesture (AC10) and as a tool for "now".
- [ ] **AC19 — `limiter: true`** on `start_performance` (open question 9).

### Phase 6 — instant browser search (independent of the others)
- [ ] **AC27 — `get_browser_index` in the script.** `{category, offset, limit, budget_s}`
      returns a page of the walked items (`name, path, uri, category, is_device`) plus
      `total_walked`, `index_complete` and a `library_key` (Live version + pack names hash);
      the walk resumes across calls with the existing `_browser_index`; `budget_s` defaults
      to 1.0 so a page never holds the socket long.
- [ ] **AC28 — The server-side index.** `src/library.rs`: an in-memory index per
      `library_key`, filled by paging `get_browser_index` and persisted to
      `state_dir()/library/<key>.json` (skipped when `ABLETON_MCP_LIBRARY_INDEX=false`).
      Loaded at startup; a matching key means search is instant from the first call of a new
      server process.
- [ ] **AC29 — Background warm-up.** After the handshake the server pages the index on a
      background thread, one page (1 s budget) per exchange so tool calls interleave on the
      shared socket, until `index_complete`; a running warm-up never delays a tool call by
      more than one page. `get_context` and `get_library_status` report "library index:
      3,412 items, complete (walked 14:12)" or "walking, 1,200 so far".
- [ ] **AC30 — `search_browser` from the index.** With a complete index: no command is sent;
      words are matched against name and folder path as today, case-insensitive, all words
      must match; results ranked by number of matches in the name over the path, then
      alphabetical. With an incomplete index: the local hits are returned and the script is
      asked for the remainder (one round trip, as today), merged and de-duplicated by URI.
      `queries: [...]` answers many in one call; `best: true` returns one URI per query;
      `refresh: true` drops the index and starts the walk again.
- [ ] **AC31 — Callers use it.** `build_song`'s word resolution, `load_drum_kit`'s kit
      lookup and the `instrument_query` path all go through the index, so a whole set's
      instruments resolve with zero browser round trips once warm.
- [ ] **AC32 — Stale URI recovery.** A `load_browser_item` failure on a URI that came from
      the index invalidates that entry, re-searches by the item's name, and retries once
      before reporting.
- [ ] **AC33 — Privacy.** `TERMS.md` gains the "Library index" paragraph; `tests/activity.rs`
      (or a new `tests/library.rs`) pins: the file lives under the state dir only, is off
      with `ABLETON_MCP_LIBRARY_INDEX=false`, contains names, paths and URIs and nothing
      else; `tests/local_only.rs` unchanged and green.

### Phase 0 — small fixes (ship with phase 1)
- [ ] **AC20 — `slots` without `slot`.** When `slots` is given and `slot` is absent, the clip
      goes into `slots` only (`slot` becomes `Option<i64>`).
- [ ] **AC21 — `scenes` block in `build_song`.** `[{name, tempo?, phrase_bars?}]` creates or
      names rows 0…n before clips are written; the readout's scene column shows the names.
- [ ] **AC22 — `$last_clip`.** `batch` records the slot of the last `create_clip` per track;
      `{"track": "$last_track", "clip": "$last_clip"}` and the bare `"$last_clip"` resolve
      in `fire_clip`, `stop_clip`, `set_clip_launch`, `set_clip_automation` and in `cue`
      steps inside the batch.

### No Regressions
- [ ] **AC23:** Outside a performance no result changes by one character;
      `tests/orchestration.rs`, `tests/capture.rs`, `tests/arrangement.rs`, `tests/mixer.rs`
      unchanged and green.
- [ ] **AC24:** The clock never adds a socket round trip: `tests/performance.rs` asserts the
      command list of every tool is unchanged by performance mode.
- [ ] **AC25:** A cue's primitive expansion is what the script runs; a gesture never adds a
      script command in phase 2.
- [ ] **AC26:** `capture_mix` and `play_and_measure` stay refused during a performance.
- [ ] **AC34:** `search_browser` returns the same hits for the same query from the index as
      from the script (a test walks a fake browser both ways and compares).

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | `set_performance_mode`, the `clock` envelope field, `lands_on_bar` on the three fires, scene phrase store and `scene_started_bar`, `set_scene`, `start_live_capture`, `snapshot_mix`/`restore_mix` (+ cue action), `performance_mode` in `get_script_info`; `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` per phase |
| `src/connection.rs` | `CallTrace` gains `clock: Option<Value>` and a trailing round-trip average; `exchange` keeps the envelope's `clock` |
| `src/tools.rs` | `run_blocking` appends the clock line; `no_later_than` on the launch tools; `set_scene`, `listen`, `vary_clip`, `undo_vary`, `follow_key`, `snapshot_mix`, `restore_mix`, `panic` bodies; `build_song` scenes block and `slot: Option<i64>`; `batch` `$last_clip`; `start_performance` `limiter`, `follow_key` |
| `src/performance.rs` | `CueTime::{NextPhrase, PhrasesAfter, BarBeat}`; `gesture` expansion; bar map; clock line renderer; phrase arithmetic |
| `src/audio.rs` | three-band energy split |
| `src/context.rs` | round trip and the library index state in the readout; instructions mention the clock line, phrases, gestures, `listen`, and "search once with several queries" |
| `src/state.rs` | `library_dir()` under the state dir |
| `TERMS.md` | the "Library index" paragraph (AC33) |
| `docs/…`, `README.md` | feature matrix, architecture note (the envelope), source-of-truth snapshot per phase |

### New
| File | Description |
|------|-------------|
| `src/variation.rs` | the note transforms and the key estimate (pure, unit-tested) |
| `src/library.rs` | the browser index: paging, matching, ranking, disk copy, staleness key (pure where possible, unit-tested) |
| `tests/library.rs` | index vs script parity (AC34), disk defaults and the off switch (AC33), warm-up interleaving, multi-query and `best` |
| `tests/performance.rs` (extended) | clock line, landing bar, `no_later_than`, phrase times, gesture expansions, `listen` sequences, snapshots |

## Remote Script compatibility
- [ ] Handlers added (Live-Python compatible; no f-strings, no type hints, no third-party imports)
- [ ] Command names in `SCRIPT_CAPABILITIES`
- [ ] `SCRIPT_VERSION` bumped once per phase (1.16.0 for phase 1)
- [ ] Commands in `tools::ALL_REMOTE_COMMANDS`
- [ ] Tool bodies call `require(live, "<command>")` first
- [ ] Live version floor stated: **Live 11+** as before; `start_live_capture` needs
      `ClipSlot.fire(record_length)` (Live 11+); the Limiter needs an edition that has it
- [ ] Timeout class: `set_performance_mode`, `set_scene`, `start_live_capture`,
      `snapshot_mix`, `restore_mix` modifying (15 s); `get_browser_index` a read whose
      `budget_s` is its own bound (1 s default, 10 s socket); the envelope `clock` adds no time
- [ ] The `clock` field is additive: a server that does not know it ignores it (the current
      `exchange` already drops unknown envelope fields)

## Privacy
**One new kind of local data: the library index (phase 6).** A JSON file per library key
under `state_dir()/library/` holding the names, folder paths and URIs of the browser items
Live showed the script — no audio, no notes, no project content. On by default like the
activity log; `ABLETON_MCP_LIBRARY_INDEX=false` keeps the index in memory only; "Delete all
local data" removes the folder. `TERMS.md` gains a "Library index" paragraph saying exactly
this, and `tests/library.rs` pins the default, the switch and the location (AC33).

Everything else is in memory: the clock, phrases, snapshots and the one-level vary undo live
in the script and the server. `listen {capture: true}` records into Live's own
project exactly as `capture_mix` does, already described in `TERMS.md`; the server reads the
file and writes nothing. `follow_key` reads notes the producer recorded, in memory, and
transposes clips; nothing is stored. The activity log gains the new tool names only;
payloads stay off by default (`tests/activity.rs` unchanged). Nothing uploads
(`tests/local_only.rs` unchanged).

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/performance.rs` | clock line appended to a success and an error result during a performance, absent outside one; command lists unchanged (AC24); landing-bar text from a scripted `lands_on_bar`; `no_later_than` schedules a cue when the bar is too close; `next_phrase`, `phrases_after`, `bar+beat` resolution and their refusals; each gesture's expansion and refusals; `listen` command sequence; `snapshot_mix`/`restore_mix` sequence | AC2–AC5, AC7, AC10–AC13, AC17 |
| `src/performance.rs` unit tests | phrase arithmetic, clock line renderer, bar map text, gesture expansion on synthetic states | AC3, AC6, AC8, AC10 |
| `src/connection.rs` unit tests | `exchange` keeps `clock` and the trailing round-trip average | AC2, AC9 |
| `src/variation.rs` unit tests | every transform on fixed note sets with a fixed seed; key estimate on scales | AC15, AC16 |
| `src/audio.rs` unit tests | band split on synthetic sines | AC14 |
| `tests/orchestration.rs` | `slots` without `slot`; scenes block; `$last_clip` | AC20–AC22 |
| `tests/stdio_integration.rs` | instructions mention the clock line and gestures | — |
| `tests/library.rs` (new) | parity with the script's matching, `queries` and `best`, disk copy on by default and off by env, warm-up pages interleave with a tool call, stale-URI retry | AC28–AC34 |

The script's own behaviour (landing bar after the fire, phrase start on a UI launch) needs
the manual pass below, recorded in the tracker issue with the Live version.

## Implementation Notes

### Phases
0. Small fixes (AC20–AC22) — with phase 1.
1. The clock: envelope, line, landing bar, `no_later_than`, phrases, bar map — the core ask.
2. Gestures — server-side expansion only.
3. `listen` — meters, then the live-safe capture with bands.
4. `vary_clip`, `follow_key`.
5. Snapshots, `panic`, limiter.
6. Instant browser search — independent; can ship first if the producer wants it first.

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| Thread-local trace around a body | `CallTrace` / `begin_trace` / `end_trace` | carrying `clock` from `exchange` to `run_blocking` |
| The wrapper owns the activity line | `run_blocking` | the wrapper also owns the clock line |
| Resolve names and times before the first command | `performance::resolve_cue` | gesture expansion, phrase times |
| Fixed-length slot fire without touching the transport | `record_clip` | `start_live_capture` |
| Polling loop with a budget and a `Drop` guard | `capture_mix_body` | `listen {capture: true}` |
| Pure note transforms | `src/notes.rs` | `src/variation.rs` |

### Design Decisions
- **The clock rides on the envelope, never on a call.** The one lever against a 200 ms round
  trip is fewer round trips; a clock that cost one would defeat the ask.
- **The landing bar is computed after the fire, by the script.** Predicting it on the server
  is the guess the producer asked to remove. `no_later_than` is the only prediction, and it
  prefers a cue to a gamble.
- **Gestures expand on the server.** The script stays a small, dumb executor of primitives;
  the readout shows exactly what will run; tests cover expansions without Live.
- **Phrase is a scene property, not a loop-length inference.** Loop lengths contradict each
  other; the producer's intent is per section.
- **`listen` is meters by default.** Free and live-safe; audio is opt-in because it writes a
  file into the producer's project.
- **The browser index lives on the server, not only in the script.** The script's own index
  already makes the walk cheap; what costs is the round trip and the Live restart. A server
  copy answers in microseconds and survives Live; the disk copy survives the server.
- **Warm-up pages are small on purpose.** The socket is one exchange at a time; a 1 s page
  is the most a tool call may wait behind the walk.
- **`follow_key` reads a recording, not a MIDI port.** A control surface without an input
  port cannot see the performer's MIDI; the recorded clip is the same notes, one bar later.

## Verification
With Live 12 open, the Remote Script reinstalled, Live restarted, a set built by
`build_song` with a scenes block.
1. Start a performance; call `set_send`: the reply ends with the clock line and the state
   readout's bar agrees with Live's display within one beat.
2. `fire_clip` at beat 1 of a bar: "lands on bar N+1". `fire_clip` timed 0.2 s before the
   bar line (script a delay): the reply says which bar it made, and it is right when checked
   against the clip's blinking in Live.
3. `fire_clip {"no_later_than": N}` 0.2 s before the bar: a cue for N+1 is scheduled, no
   launch happens on N+1's predecessor.
4. Fire a scene from Live's own UI: the next state read shows the phrase counted from that
   bar.
5. `cue` with `"next_phrase"`: lands on the phrase boundary shown in the bar map.
6. Breakdown gesture, keep the pad: the kick, hats and bass stop on the bar, come back 8
   bars later on their own slots, and the readout listed the six primitives.
7. `listen {"bars": 1}` during the drop: master within 0.5 dB of the ceiling is flagged;
   `listen {"capture": true}`: a clip appears on the Capture track, the transport never
   stopped, bands are reported.
8. `vary_clip fill_last_bar` on the drum clip with `to_slot`: the varied clip differs in the
   last bar only; `undo_vary` restores it.
9. `record_clip` four bars in D minor on the lead, `follow_key`: the assistant's clips are
   transposed and Live's scale shows D minor.
10. `snapshot_mix`, change three faders, `cue restore_mix` on the next bar: faders return on
    the bar.
11. Outside a performance: no clock line on any result.
12. Fresh server, Live open: `get_context` says "walking"; within about a minute it says
    "complete"; `search_browser {"queries": ["analog bass", "techno kit", "pad"], "best":
    true}` answers with three URIs and the activity line lists no Live command; restart the
    server: the first search is instant and the activity line still lists no command;
    `ABLETON_MCP_LIBRARY_INDEX=false`: no file appears under the state dir.

## Out of Scope
- Reading the performer's MIDI port live (a control surface sees only its own input).
- Beat-matching or warping audio the producer drops in.
- Any phrase inference from audio.
- A second undo level for `vary_clip`, or an undo for gestures (a gesture is a cue: cancel
  it before it fires, or fire the previous scene).
- Sending audio or MIDI anywhere.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| `perform-live-build-launch-and-transition-on-the-bar` | Built, real-Live pass pending | script 1.15.0 |
| The 200 ms round-trip floor | Measured 2026-09-19 | the reason for the envelope design |
| Open questions 1, 3, 6, 9, 11 | Waiting on the producer | ACs may move |

## Related Stories
- `perform-live-build-launch-and-transition-on-the-bar` — the cue clock and guards this
  story rides on.
- `capture-the-mix-through-resampling` — the file reader and measurements `listen` reuses.
- `measure-a-capture-by-band-and-section` — not written; the band split lands here first.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from the first real set's feature request ("make AbletonMusicMaker truly playable live, with proper bar awareness at all times") |
| 2026-09-19 | Phase 6 added: instant browser search (server-side index, disk copy, background warm-up, many queries per call) after the same set showed four to six searches per session |
| 2026-09-19 | Built end to end: the clock on every response, landing bars, `no_later_than`, phrases and the bar map, gestures, `listen`, `vary_clip`/`undo_vary`/`follow_key`, mix snapshots, `panic`, the limiter, the library index, `$last_clip`, scenes and `slots`-only in `build_song`; `tests/library.rs` and new performance tests |
