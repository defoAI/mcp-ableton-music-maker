# Perform live: build, launch and transition a set on the fly, always on the bar

## Story
**As a** producer performing with Live open and Claude at my side,
**I want** to say "start with a deep pumping techno track", later "give it a melody", later
"mix us into a breakbeat track", and have every launch, tempo move and section change land
on the bar without the music ever stopping,
**So that** I can run a set by talking, and trust it in front of people.

## Details
| Field | Value |
|-------|-------|
| Status | `In Progress` — phases 1–3 built against the assumed answers below; awaiting the real-Live pass and the producer's answers to open questions 1, 2, 4 and 7 |
| Surface | [Decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md): `start_performance`, `cue`, `cancel_cue`, `fire_scene`, `create_scene`, `set_launch_quantization`, `set_crossfader` and `get_performance_state` are the raw layer (served as `adv_…`) under the artist's `play_song` / `go` / `jump_to` / `back` / `hold_section` / `next_section` / `previous_section` / `end_performance`; `jump_to` starts a performance when none runs, so the artist never needs `start_performance` |
| Priority | P1 — the first use of the product that is a performance rather than a session; it is what "drive Live" means to a performer |
| Size | L — three phases below; phase 1 alone makes a bar-accurate performance possible |
| Tracker | none yet — an issue is opened when the story is `Ready` |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Prototype | [prototypes/perform-live-build-launch-and-transition-on-the-bar.md](../prototypes/perform-live-build-launch-and-transition-on-the-bar.md) |
| Depends on | Remote Script 1.12.0 (verified 2026-09-19 against `AbletonMusicMaker_Remote_Script/__init__.py` at `364f06d`); the two-phase `DEFERRED` handler pattern and the fixed-length slot recording from `capture-the-mix-through-resampling` |

## Context

### The Problem
Everything the product does today assumes the producer is composing: build tracks, write
clips, place them in the Arrangement, capture and measure. A performance is different in
one way that changes everything: **the music is already playing, and it must not stop.**

Claude's replies take seconds. A bar at 126 BPM lasts 1.9 s. A tool call that fires a clip
arrives at an arbitrary moment inside a bar; a transition made of five calls spreads over
ten seconds; a tempo change is a jump. There is no way today to say "at bar 49 drop the
kick, climb to 134 over the break, and bring the breaks in at 57" and have Live do exactly
that, on the bar, regardless of when the words arrive. And nothing stops a well-meaning
call from stopping the transport mid-bar.

"Flawless" therefore has a precise meaning in this story:

1. **Nothing lands off the bar.** Every launch and every section change happens on a bar
   boundary chosen in advance.
2. **Nothing goes silent unplanned.** At every bar something is playing, unless the
   producer asked for silence.
3. **Nothing depends on the round trip.** A move that has been cued happens even if the
   server is slow, disconnected, or gone.
4. **Nothing is destroyed while it plays.** No tool stops the transport, moves the
   playhead, jumps the tempo, or deletes a playing clip while a performance runs.
5. **The producer can always see where they are and what is about to happen.**

### Current State
- `fire_clip` calls `clip_slot.fire()` with no arguments
  ([`__init__.py:1010`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L1010)), so a
  clip starts on Live's global launch quantization — which the product neither reads nor
  sets. `get_session_info` reports tempo, signature, `is_playing` and `current_song_time`
  ([`__init__.py:666`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L666)) but not
  the quantization, the bar, or which slot is playing or queued on each track.
- Scenes are serialized with name, tempo and `is_triggered`
  ([`__init__.py:3355`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L3355)) and nothing
  more: there is no `fire_scene`, no `create_scene`, and `create_clip` refuses a slot index
  beyond the scenes that exist ("Clip index out of range",
  [`__init__.py:826`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L826)). Firing a
  section today means one `fire_clip` per track, each a separate socket round trip
  ([`batch_body`](../../../src/tools.rs#L2680) runs steps sequentially with no notion of
  time).
- `set_tempo` assigns `song.tempo` once
  ([`__init__.py:997`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L997)): a jump,
  never a ramp. The master crossfader and per-track crossfade assignment are not exposed.
- Per-clip launch settings exist: `set_clip_launch` writes launch mode, launch
  quantization (the `LAUNCH_QUANTIZATIONS` table at
  [`__init__.py:2180`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L2180)), legato
  and velocity amount. Clip automation envelopes exist (`set_clip_automation`), so a sweep
  can be written into a clip before it is fired and Live plays it sample-accurately.
- The script can answer a command from a later tick (`DEFERRED`,
  [`__init__.py:36`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L36), used by
  `_play_from` at
  [`__init__.py:1874`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L1874) and
  `_start_capture` at
  [`__init__.py:1999`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L1999)), and it
  has a passive event queue with `playback_changed`, `tempo_changed`,
  `time_signature_changed` and `tracks_changed`
  ([`_setup_passive_listeners`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L2850)).
  There is no periodic tick and no beat-position listener; nothing in the script acts on
  its own clock.
- Several existing tools stop or move the transport as a side effect: `play_from` stops
  playback before moving the playhead
  ([`__init__.py:1874`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L1874)),
  `capture_mix` stops playback and moves the playhead
  ([`__init__.py:1999`](../../../AbletonMusicMaker_Remote_Script/__init__.py#L1999)),
  `play_and_measure` and `stop_playback` stop it outright. Nothing prevents any of them
  during a performance.
- `create_audio_clip` (Live 12.0.5+) can put a warped audio file in a Session slot, so an
  audio "track" can be played as well as Claude-written MIDI.
- Socket budgets: reads 10 s, modifying commands 15 s
  ([`command_timeout`](../../../src/connection.rs#L164)). One socket, one exchange at a
  time — a second call waits for the first.

### Root Cause
The bridge is request–response: Live does what the last message said, when it arrives.
Music is scheduled: things happen at bars. The gap is a scheduler on the Live side and a
guard on the server side.

## Open Questions

1. **What is "a track" in "play songs"?** Three readings: (a) material Claude writes on the
   spot (MIDI clips on instrument tracks), (b) Session clips the producer prepared in the
   set, (c) audio files, DJ-style. → **Assumed: (a) first, (b) always (the tools work on
   whatever the set holds), (c) supported through `create_audio_clip` and the crossfader
   (phase 3) but not the first path.** *Needs your answer.*
2. **What happens to the tempo when the genre changes?** Techno at 126, breakbeat commonly
   130–140 or half-time at ~85. Options: ramp under a drum-less break (a DJ never ramps
   under a kick), hold the tempo and let the pattern carry the change, or jump at the
   drop. → **Assumed default: ramp under a break, at most 12 BPM per 8 bars; the producer
   can say "hold the tempo" or "jump".** *Needs your answer.*
3. **How far ahead may Claude commit?** With 1-bar quantization, a fire issued during bar
   N lands at N+1. Claude's own latency plus the socket is 2–8 s, i.e. 1–4 bars. →
   **Cues default to at least 2 bars ahead; less than 1 bar ahead is allowed with a
   warning in the result (prototype §8).**
4. **Gig or jam?** In front of people, an unplanned silence is a failure; in the studio it
   is a pause. → **Assumed: gig. Silence needs `allow_silence: true`.** *Needs your answer.*
5. **Where does new material go while a scene is playing?** A clip written into the
   playing scene's row and fired joins the section on the next bar (prototype §3); a
   section change then fires that row's other clips and stops this one unless it is also
   in the next row. → **The result of `create_clip` during a performance says which scene
   row it landed in; `cue` steps may `fire_clip` individual slots alongside `fire_scene`,
   so a lead can be carried across a section change without copying. A Session
   clip-to-slot copy is a separate small story if it turns out to be needed every set.**
6. **"Give a melody" — typed or played?** Both happen on stage. → **Typed: `create_clip`
   (existing). Played: `record_clip`, a fixed-length Session recording on an armed track,
   quantized to the next bar, looping when done — the same `ClipSlot.fire(record_length,
   launch_quantization)` the capture story uses.**
7. **Does loading an instrument on a silent track interrupt the audio?** Live loads
   devices while playing; whether a sample-heavy instrument causes a dropout depends on
   the machine. → **Not provable from the API. Verification step 6 measures it on the
   producer's machine with `get_track_meters` on the master during a load. Until then the
   tool result says "loaded while playing" and the README recommends preparing the
   instrument tracks before going live (`build_song` does it in one call).**
   *Needs a real-Live pass.*
8. **What is the clock for cues?** The script has no tick. Options: a `current_song_time`
   listener (fires whenever the playhead advances; the Live Object Model exposes
   `Song.current_song_time` as get/set/observe — local copy
   `docs/reference/ableton/live-object-model.md`), or a re-armed `schedule_message(1, …)`
   loop (about 100 ms per tick; the tick rate is not documented and must be measured on
   a real Live). → **Song-time listener for cue steps, active only while a
   performance runs; a scheduled-message loop as the fallback if the listener proves to
   fire too often for the main thread. Either way, launches are fired during the bar
   before their target and Live's quantization places them exactly; only non-quantized
   actions (parameter sets, ramp steps) carry the clock's granularity.**
9. **Time signature changes mid-set?** → **Cues are counted in the signature at scheduling
   time; a `time_signature_changed` event cancels pending cues and the next state readout
   says so (prototype §8).**
10. **Which Live versions?** Scenes, `fire`, `clip_trigger_quantization`, the crossfader
    and `schedule_message` exist on Live 10 and 11; the key readout (`Song.scale_name`,
    `Song.root_note`) is Live 12; `record_clip` needs `ClipSlot.fire(record_length, …)`,
    Live 11+. → **Floor Live 11 for the whole story; the key comes from the producer on
    Live 11 and from the set on Live 12.**

## Prototype
[prototypes/perform-live-build-launch-and-transition-on-the-bar.md](../prototypes/perform-live-build-launch-and-transition-on-the-bar.md)
— going live from nothing, reading the state, a typed melody, a played melody, the
techno-to-breakbeat transition as one cue, a DJ-style crossfade, what is refused, and the
failure cases.

Not yet reviewed. What writing it changed: `cue` became a small timeline (`at`, `from` +
`bars`, `ramp`) rather than one action per call, because the breakbeat transition is three
things that belong together; and `end_performance` grew `at`/`fade_bars`/`now` because
"stop" on stage has three meanings.

## Tool description

```text
start_performance: Go live. Sets the global launch quantization (default 1 bar), records
the key (from the set on Live 12, else from `key`), fires the named scene if given and
starts the transport if it is stopped, and turns on the performance guards: until
end_performance, stop_playback, play_from, set_arrangement_time, set_tempo, capture_mix,
play_and_measure, switch_to_arrangement_view, and delete_track/delete_clip on anything
playing or queued are refused with a message that names the cue to use instead. Nothing in
the set changes except the quantization. Refused if a performance is already running.

get_performance_state: Where the set is: bar.beat, tempo, signature, seconds since the
performance started, quantization, key; per track the playing clip and the queued clip;
the scenes with the playing one marked; pending cues with their next step; seconds to the
next bar; and anything that happened since the last call (cue steps fired, transport
stopped in Live, cues cancelled). Read-only.

cue: Schedule steps on Live's clock. Each step has a time — {"bar": N} in Live's bar
numbers, "next_bar", or {"bars_after": k} — and one action: fire_scene, fire_clip,
stop_clip, stop_all_clips, set (a device parameter, track volume, send or mute), or a
ramp over `bars` of tempo, crossfader, track volume or a device parameter. The Remote
Script executes the cue on its own clock, so it happens even if this server is slow or
gone. Launches are placed by Live's quantization and land exactly on the bar; other
actions land within one script tick of the beat. Refuses a step in the past, an unknown
scene or clip, a ramp longer than 64 bars, and a plan that leaves a bar with nothing
playing unless allow_silence is true. Returns the cue id and the plan as it will run.

cancel_cue: Cancel a pending cue by id; steps already fired stay fired.

fire_scene: Fire a scene by index or name, quantized by Live to the next bar (or the
current launch quantization). Says when it will start. For timed moves use cue.

create_scene: Add a scene (row of slots) at an index or the end, optionally named and with
a scene tempo. Returns its index so create_clip can fill it.

record_clip: Record the producer playing into a Session clip: arms the track, fires an
empty slot at the next bar for `bars` bars, and lets the clip loop when the recording
ends. Live 11+. Refused while the track is already recording.

set_launch_quantization: The global launch quantization: none, 8_bars … 1_bar, 1/2 …
1/32. This is what quantizes fire_clip, fire_scene and record_clip.

set_crossfader: Move the master crossfader (0 = A, 1 = B) and/or assign tracks to A, B or
neither. For a timed blend, ramp the crossfader in a cue.

end_performance: Stop on the bar ({"at": "next_bar"} or a bar), fade the master over
`fade_bars` and stop, or stop now. Lifts the guards, cancels pending cues, restores the
master volume after a fade, and reports the set: duration, cues run, cancelled, missed.
```

The transport-touching tools listed under `start_performance` gain one sentence in their
own descriptions: "Refused while a performance runs; see end_performance."

## Acceptance Criteria

### Remote Script — phase 1: the primitives
- [ ] **AC1 — `get_performance_state`.** Returns `bar`, `beat`, `tempo`,
      `signature_numerator/denominator`, `is_playing`, `clip_trigger_quantization` (index
      and name), per track `{playing_slot_index, fired_slot_index, playing_clip_name,
      is_recording}` (Live's `playing_slot_index` is -2 when the Clip Stop slot fired and
      -1 when nothing plays; both read as "–"), the scenes (`index, name, tempo,
      is_triggered, is_playing`), the
      script's pending cues, and the key (`scale_name`, `root_note`) where Live exposes
      it, else `null`. Bar numbers are Live's `get_current_beats_song_time()`.
- [ ] **AC2 — `set_launch_quantization(name)`** writes `song.clip_trigger_quantization`.
      Its integer table is **not** the clip table already in the script: the global one
      runs 0 None, 1 8 Bars, 2 4 Bars, 3 2 Bars, 4 1 Bar, 5 1/2 … 13 1/32 and has no
      "global" entry, while a clip's `launch_quantization` runs 0 Global, 1 None, 2 8 Bars
      … 14 1/32 (`docs/reference/ableton/live-object-model.md`, `Song` and `Clip`). A second
      table, `GLOBAL_QUANTIZATIONS`, with the same names minus `global`; unknown name errors
      with the list.
- [ ] **AC3 — `create_scene(index, name, tempo)`** and **`fire_scene(index)`**; firing an
      out-of-range scene errors with the range. `Scene.fire` takes no quantization of its
      own — each clip follows its launch quantization, "Global" by default — so `fire_scene`
      is on the bar only while every clip in the row is on Global or a bar value; the result
      names any clip whose setting is finer. `stop_all_clips` calls `song.stop_all_clips()`.
- [ ] **AC4 — `set_crossfader(value, assign)`**: `master_track.mixer_device.crossfader`
      and per-track `mixer_device.crossfade_assign`.
- [ ] **AC5 — `record_clip(track_index, bars, name)`**: arms the track (error if it cannot
      be armed), finds the first empty slot, fires it with `record_length = bars ×
      beats per bar` and the global quantization, names the clip; `DEFERRED` until the
      slot reports `is_recording`; error if the track is already recording. Live 11+.
- [ ] **AC6 — Script hygiene.** No f-strings, type hints or imports beyond Live's; the
      commands added to `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` bumped;
      `tools::ALL_REMOTE_COMMANDS` updated; the cross-check test passes.

### Remote Script — phase 2: the clock
- [ ] **AC7 — `schedule_cue(cue)`** stores the cue with its steps resolved to absolute
      beats at scheduling time and returns `{id, steps: [{beat, bar, action}]}`. Refuses a
      step whose beat is at or before the current beat.
- [ ] **AC8 — Execution.** While a performance runs, a song-time listener (or the fallback
      tick, open question 8) walks the pending steps. A launch step (`fire_scene`,
      `fire_clip`, `stop_clip`, `stop_all_clips`) is issued once the clock is inside the
      bar before its target, so Live's quantization lands it on the target bar. A `set`
      step is applied on the first tick at or after its beat. A `ramp` step writes a
      linear interpolation from the value at its first tick to `to` on every tick between
      `from` and `from + bars`, and writes `to` exactly at the end. Each fired step
      appends a `cue_step_fired` passive event with the beat it was issued at.
- [ ] **AC9 — Cancellation and interruption.** `cancel_cue(id)` removes the cue's
      remaining steps. A `playback_changed` to stopped, or a `time_signature_changed`,
      cancels every pending cue and enqueues `cues_cancelled` with the reason. The
      listener is removed when no performance runs.
- [ ] **AC10 — Idempotent handshake.** Restarting the server does not lose the script's
      pending cues; `get_performance_state` lists them.

### Server — phase 1
- [ ] **AC11 — Performance mode.** `LiveState` gains `performance: Mutex<Option<Performance>>`
      (`started_at`, `start_bar`, `key`, `quantization`, counters). `start_performance`
      runs `set_launch_quantization` → optional `fire_scene` → `start_playback` if
      stopped → `get_performance_state`, and reports as in prototype §1. Refused if
      already running.
- [ ] **AC12 — Guards.** With a performance running, the bodies of `stop_playback`,
      `play_from`, `set_arrangement_time`, `set_tempo`, `capture_mix`, `play_and_measure`,
      `switch_to_arrangement_view`, `stop_all_clips` (outside a cue) return the error in
      prototype §7 before any command reaches Live; `delete_track` and `delete_clip`
      first read the state and refuse only if the target is playing or queued. The guard
      is one function, `guard_performance(live, tool)`, called first in each body.
- [ ] **AC13 — `get_performance_state` text** as in prototype §2: the header line, the
      per-track table, scenes with the playing one bracketed, cues, "next bar in N s"
      computed from tempo and beat, and the since-last-call events drained from the
      passive queue. Under 1,200 characters for eight tracks.
- [ ] **AC14 — `fire_scene`, `create_scene`, `set_launch_quantization`,
      `set_crossfader`, `record_clip` tools** as described, each resolving a scene or
      track by name or index. `fire_scene` and `fire_clip` results say which bar the launch
      lands on, computed from the state and the quantization.

### Server — phase 2
- [ ] **AC15 — `cue` validation before Live.** Times resolve against a fresh state
      (`bar`, `next_bar`, `bars_after`); a step in the past errors; less than one bar
      ahead warns; unknown scene or clip errors; ramps capped at 64 bars; the tempo ramp
      target must be 20–999 BPM. The **silence check**: simulate the cue over the playing
      slots (a `fire_scene` replaces each track's playing slot by the scene's slot or
      empties it; `stop_*` empties) and error if any bar between the first and last step
      has no playing clip on any track, unless `allow_silence`.
- [ ] **AC16 — `cue` result** as in prototype §5: id, "it is bar N now", one line per
      step in Live's bar numbers, the silence check's verdict, and the cancel hint.
- [ ] **AC17 — `end_performance`**: `at` schedules a cue `stop_all_clips` then
      `stop_playback` on the bar; `fade_bars` schedules a master-volume ramp to 0 and the
      stop, then restores the volume after the stop; `now` stops immediately. All three
      cancel other cues, lift the guards, and report duration and counters.

### Phase 3
- [ ] **AC18 — Crossfade transition** (prototype §6) works end to end with an audio clip
      from `create_audio_clip` on Live 12.0.5+.
- [ ] **AC19 — README "Performing live"**: the five meanings of flawless, the prepare-then-
      go-live recommendation, the refusal list, and three example prompts.

### Documents
- [ ] **AC20:** feature matrix (new "Performance" section), architecture overview (the
      script's clock and the guard), source-of-truth snapshot re-dated in the same PR as
      each phase.

### No Regressions
- [ ] **AC21:** With no performance running, every existing tool behaves exactly as
      before; `tests/capture.rs`, `tests/orchestration.rs`, `tests/arrangement.rs`,
      `tests/mixer.rs` unchanged and green.
- [ ] **AC22:** The global launch quantization is written only by `start_performance`
      and `set_launch_quantization`; `end_performance` leaves it as it is (the producer's
      set keeps the performance setting rather than silently reverting).
- [ ] **AC23:** A killed server never leaves a cue half-run in an inconsistent state: a
      cue's steps are independent and each is fired at most once; the script keeps
      executing; `get_performance_state` after reconnect reports what fired.
- [ ] **AC24:** `record_clip` never leaves a track armed: the recording has a fixed
      length, and the script disarms the track on the tick after `is_recording` goes
      false. This matters because Live's Scene launch "starts recording of armed and empty
      tracks" when the *Start Recording on Scene Launch* preference is on (Live Object
      Model, `Scene.fire`, local copy in `docs/reference/ableton/live-object-model.md`):
      an armed Lead with an empty slot in the next scene would record instead of
      stopping. `fire_scene` and the `cue` silence check also treat an armed track with an
      empty slot in the target scene as "would record" and say so.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | commands `get_performance_state`, `set_launch_quantization`, `create_scene`, `fire_scene`, `stop_all_clips`, `set_crossfader`, `record_clip`, `schedule_cue`, `cancel_cue`; the cue store and its clock; `cue_step_fired` / `cues_cancelled` passive events; `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` |
| `src/tools.rs` | the nine tool bodies, params, specs, methods; `guard_performance`; `ALL_REMOTE_COMMANDS`; `run_named` entries; the guard call at the top of the listed transport-touching bodies; tool count in `tool_count_and_schema_defaults` |
| `src/connection.rs` | `Performance` in `LiveState`; the modifying commands in `MODIFYING_COMMANDS` |
| `README.md`, `docs/technical/feature-matrix.md`, `docs/architecture/overview.md`, `docs/facts/source-of-truth.md` | AC19, AC20 |

### New
| File | Description |
|------|-------------|
| `tests/performance.rs` | the tenth suite: start/end sequences, guards, cue validation and text, silence check, reconnect readout — all through `run()` with `FakeBridge` |

## Remote Script compatibility
- [ ] Handlers added (Live-Python compatible; no f-strings, no type hints, no third-party imports)
- [ ] Command names in `SCRIPT_CAPABILITIES`
- [ ] `SCRIPT_VERSION` bumped (once per phase)
- [ ] Commands in `tools::ALL_REMOTE_COMMANDS`
- [ ] Tool bodies call `require(live, "<command>")` first
- [ ] Live version floor stated: **Live 11+** for the story; on Live 10 the capability
      list omits `record_clip` and the tools refuse with the standard message; the key
      readout is `null` below Live 12 and `start_performance` then needs `key`
- [ ] Timeout class: `schedule_cue`, `cancel_cue`, `fire_scene`, `create_scene`,
      `stop_all_clips`, `set_launch_quantization`, `set_crossfader`, `record_clip`
      modifying (15 s); `get_performance_state` read (10 s)

## Privacy
**No new local data.** The performance state and the cues live in memory, in the server and
in the script, and vanish with the process. The activity log gains nothing but the new tool
names and the usual timings and sizes; payloads stay off by default (`tests/activity.rs`
unchanged). `record_clip` records into Live's own project, exactly as a producer pressing
record would, and the server never reads or copies that audio. `tests/local_only.rs`
unchanged and green. No `TERMS.md` change.

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/performance.rs` (new) | `start_performance` command sequence; every guarded tool refused with the §7 text while running and allowed when not; `delete_clip` allowed on a non-playing clip; `cue` past-step error, one-bar warning, unknown scene, silence check with and without `allow_silence`; `cue` plan text; `end_performance` three modes; reconnect readout drains `cue_step_fired` | AC11–AC17, AC21, AC23 |
| `src/tools.rs` unit tests | tool count; `ALL_REMOTE_COMMANDS` cross-check; bar arithmetic (`next_bar_in_s`, `bars_after`) on synthetic states | AC6, AC13, AC15 |
| `tests/stdio_integration.rs` | the new tools listed over stdio | AC14 |
| `tests/activity.rs`, `tests/local_only.rs` | unchanged, must stay green | Privacy |

The script's clock cannot be tested without Live; the verification steps below are the
test, recorded in the tracker issue with the Live version.

## Implementation Notes

### Phases
1. **Primitives and guards** — state, scene fire, quantization, record, the guard. A bar-
   accurate performance is possible with `fire_scene` alone; transitions are by hand.
2. **The clock** — `schedule_cue`, ramps, `end_performance`'s timed modes.
3. **Crossfade, README, polish.**

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| Two-phase handler answering from a later tick (`DEFERRED`) | `_play_from`, `_start_capture` | `record_clip`: fire, then answer once `is_recording` |
| Fixed-length slot recording | `_start_capture` | `record_clip` |
| Passive event queue | `_enqueue_passive` | `cue_step_fired`, `cues_cancelled` |
| Validate everything before the first command reaches Live | `build_song_body` | `cue`'s validation and silence check |
| Resolve a track by name or index | `build_song` | scene and track names in `cue`, `fire_scene`, `set_crossfader` |
| Per-command socket budget | `command_timeout` | the modifying list above |

### Design Decisions
- **Claude plans, Live executes.** Nothing musically timed crosses the socket at the moment
  it must happen. Launches use Live's own quantization; multi-step moves are cues run by
  the script. This is what makes point 3 of "flawless" true rather than hoped for.
- **Cues live in the script, not the server.** The server can be slow or dead; the music
  cannot. The price is that a Live restart loses cues, which is the right failure.
- **Sweeps are clip automation, not cue ramps.** A filter sweep written into the clip with
  `set_clip_automation` is sample-accurate; a ramp on the script's clock is stepped at the
  tick. Cue ramps are for the things Live cannot automate in a clip: tempo and the
  crossfader (and, second choice, track volume).
- **Section changes are scene fires, not mutes.** A mute is not quantized; a scene fire is.
  The tool descriptions say so.
- **A tempo ramp writes `Song.tempo` every tick.** The Live Object Model says the tempo
  "may be automated, so it can change depending on the current song time": if the set has
  arrangement tempo automation, the ramp and the automation fight. `cue` refuses a tempo
  ramp when the set is following the arrangement (`back_to_arranger` false with tempo
  automation present) and says why.
- **Guards are refusals with the alternative in the message**, not silent no-ops. The
  producer asked for something; the reply says how to get it on the bar.
- **Silence is opt-in.** A cue that would empty every track errors unless `allow_silence`.
  On stage, silence you did not plan is the one thing everyone hears.
- **The global quantization is left as set by `end_performance`.** Restoring it would
  change the producer's set behind their back; leaving it is visible in Live's transport bar.
- **Why not a "performance mode" flag in the script?** The script already has the cues to
  know whether a performance runs; the guard is a server concern because the tools being
  refused are server tools.

## Verification
With Live 12 open, the Remote Script reinstalled, Live restarted, an empty set.
1. "We're going live, deep techno at 126" — expect `build_song` then
   `start_performance`; Live's transport shows 1-bar quantization; the Intro scene plays.
2. "Bring the bass in at bar 9 and the pad at 17" — expect one cue; watch Live's scene
   launch buttons blink at bar 8 and 16 and the clips start at 9 and 17 exactly.
3. "Where are we?" — the bar in the reply matches Live's transport within one beat.
4. "Give it a melody, F Ab C Bb Ab F" — a clip appears in the playing scene's row on Lead
   and starts on the next bar; the notes are in F minor.
5. "Let me play it, four bars" — Lead arms, the slot records from the next bar for four
   bars, then loops; the arm is still on; `end_performance` clears it.
6. During the groove: "load a piano on Lead" — read `get_track_meters` on the master ten
   times across the load; record whether any reading drops to silence (open question 7).
7. "Mix us into a breakbeat track, 8 bars from now" — one cue with three steps; the kick
   drops at the first bar, the tempo display climbs smoothly to the target over 8 bars,
   the breaks start on the third step's bar; nothing is silent in between.
8. Kill the server (the app's heartbeat pid) between steps 1 and 3 of that cue: Live still
   runs the remaining steps; restart the server and "where are we?" lists them as fired.
9. Press Stop in Live: the next reply says the transport was stopped in Live and the
   pending cue is cancelled.
10. "Stop everything" — refused with the three options; "on the bar" ends it on the bar.
11. Repeat step 1 on Live 11 without a scale set: `start_performance` asks for `key`.

## Out of Scope
- Arrangement recording of the performance (Live's own arrangement record button does it).
- Beat-matching or warping audio the producer drops in (Live warps on import).
- MIDI controller mapping (the producer's controller keeps working alongside).
- Follow actions and clip launch modes beyond what `set_clip_launch` already writes.
- Scale correction of typed melodies (the key is reported; notes are not moved).
- Anything that sends audio or MIDI off the machine.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| Remote Script 1.12.0, `DEFERRED` pattern and `_start_capture` | Merged (`364f06d`) | |
| Live Object Model: `Song.clip_trigger_quantization`, `Scene.fire`, `Song.create_scene`, `Song.stop_all_clips`, `MixerDevice.crossfader`, `Track.mixer_device.crossfade_assign`, `Song.add_current_song_time_listener`, `Song.get_current_beats_song_time` | Documented; local copy in `docs/reference/ableton/live-object-model.md` (fetched 2026-09-19) | verify the listener's rate on a real Live before choosing the clock |
| `ClipSlot.fire(record_length, launch_quantization)` | Live 11+ | already used by `_start_capture` |
| `Song.scale_name`, `Song.root_note` | Live 12 | `null` below |
| Open questions 1, 2, 4, 7 | Waiting on the producer | ACs may move |

## Related Stories
- `capture-the-mix-through-resampling` — the fixed-length recording and the two-phase
  handler this story reuses; its `capture_mix` is guarded during a performance.
- `mac-app-installs-runs-and-watches-the-server` — the app could show the performance
  state; not in this story.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from "go live with Ableton via the MCP and play songs and make arrangements on the fly" |
| 2026-09-19 | Built end to end (Remote Script 1.13.0: nine commands and the tick clock; `src/performance.rs`; ten tools; guards; `tests/performance.rs`); real-Live verification pending |
| 2026-09-19 | First real-Live session's feedback folded in (Remote Script 1.14.0): nested schemas inlined so the model sees note and cue forms; `build_song` searches plain instrument words and copies clips into `slots`; `start_performance` disarms and sets the key in Live; `keep_track_playing` (stop buttons off) replaces clip copies; nested Drum Racks found |
| 2026-09-19 | `get_context` (one round trip, Remote Script 1.15.0) and MCP instructions at `initialize`, so an agent knows the build → hear → perform workflow out of the box |
