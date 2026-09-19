# Capture the mix through resampling, so Claude can measure what it made

## Story
**As a** producer directing Claude in Live,
**I want** Claude to record a stretch of the arrangement to an audio clip in Live and read it back as measurements — and I want to play that same clip myself —
**So that** Claude judges the section it just built from the sound rather than from meter peaks, and we hear the same thing.

## Details
| Field | Value |
|-------|-------|
| Status | `Ready` |
| Priority | P1 — the artist's session ended with "I never heard the track"; `play_and_measure` reports levels, not sound |
| Size | L — three phases below; the Live-side capture ships first and is useful alone |
| Tracker | [#26](https://github.com/defoAI/mcp-ableton-music-maker/issues/26) |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Prototype | [prototypes/capture-the-mix-through-resampling.md](../prototypes/capture-the-mix-through-resampling.md) |
| Depends on | Remote Script ≥ 1.10.2 (the two-phase playhead handling from #25 is the pattern reused here) |

## Context

### The Problem
Nothing in the product lets Claude, or the producer through Claude, hear a result. Live's
control-surface API has no export and no audio capture, which is why #11 (save and export)
closed as not possible. What shipped instead, `play_and_measure` (`src/tools.rs`), plays a
stretch and reports each track's meter peak. That catches silence and clipping and nothing
else: a clashing chord, a kick with no weight, a drop with less low end than the intro are
invisible to it.

### Current State
- `play_and_measure_body` in `src/tools.rs` already runs the shape this story needs on the
  server side: move the playhead, start playback, poll a read command, stop. It never
  touches audio.
- The Remote Script (`AbletonMusicMaker_Remote_Script/__init__.py`) has no routing, arming,
  monitoring or recording commands. `_serialize_clip_common` reads `file_path` off audio
  clips, so a recorded clip's file is reachable once a clip exists.
- Live can record its own master into a track: an audio track whose input is
  **Resampling** records whatever the master outputs. The Live Object Model exposes
  `Track.available_input_routing_types`, `Track.input_routing_type`, `Track.arm`,
  `Track.current_monitoring_state`, and `ClipSlot.fire(record_length, launch_quantization)`
  (Live 11+), which records a Session clip of a fixed length in beats.
- Recorded audio lands in the project's `Samples/Recorded/` folder as WAV or AIFF, per Live's
  "Record File Type" preference; the clip's `file_path` names it.
- The server writes nothing outside `state_dir` (`src/state.rs`) and opens no socket except
  the one to Live (`tests/local_only.rs`). Reading an audio file Live wrote is a new kind of
  read, not a new write.
- The Mac app (`app/`) reads files under the state dir and can list and play local files.

### Root Cause
The bridge was built for editing, not for listening; every path out of Live was assumed to
be an export, which the API does not have. Resampling is the path the API does have.

## Open Questions
1. **Session recording or Arrangement recording?** → Session. Firing a Session clip on one
   track takes over that track only; every other track keeps playing the Arrangement, so a
   fixed-length Session recording on a dedicated capture track records the arrangement
   section without adding a clip to the Arrangement. Arrangement recording (`record_mode`)
   would leave a clip on the timeline and interact with overdub and punch settings.
2. **Whose track?** → A dedicated audio track named **Capture** that the tool creates once
   if absent (last position), sets to input Resampling, monitoring Off, and arms. It stays
   in the set on purpose: the producer can see and play every capture in its slots. Its
   volume does not matter for the recording; it is muted after creation so it never doubles
   the mix when a captured clip is fired by hand.
3. **How long can a capture be?** → 1 to 64 bars (4 to 256 beats), default 8 bars. Longer
   than that is a bounce, and the measurements the loop needs come from sections.
4. **WAV or AIFF?** → Both, PCM 16/24/32-bit, read by a small parser in the crate
   (`hound` covers WAV; AIFF is a 60-line chunk reader — no third-party decoder, no
   network dependency). Anything else (FLAC recording is not a Live option) is an error
   that names the file type.
5. **Does the capture tool analyse, or only capture?** → It returns the basic time-domain
   readout (peak dBFS, integrated RMS, per-bar RMS, silent bars, clipping, stereo
   correlation) so the loop closes in one tool. Spectral measurements — low/mid/high energy,
   transient density, section comparison — are the next story,
   `measure-a-capture-by-band-and-section`, which reads the same files.
6. **Where do the files live, and are they ours?** → They are Live's project files. The
   server records the path in the tool result and never copies or deletes audio. The app
   plays them in place. `TERMS.md` states that captures are Live's own recordings in the
   project folder, not part of the app's data, and that "Delete all local data" leaves them.
7. **What if the artist's set has no project folder yet (untitled)?** → Live records to a
   temporary project folder; `file_path` still resolves. The result says the set is
   unsaved so the capture will move when it is saved.
8. **Latency and pre-roll?** → Resampling records the master as heard, so there is no
   input latency; `launch_quantization` "none" starts recording the moment the transport
   is at `start`. The tool starts the transport one beat before `start` and fires the clip
   at `start`, so the first beat is not clipped by transport start.
9. **Can two captures run at once?** → No. One capture at a time per set; a second call
   while `is_recording` is true is refused with the running capture's name.

## Prototype
[prototypes/capture-the-mix-through-resampling.md](../prototypes/capture-the-mix-through-resampling.md)
— the transcript of Claude capturing the drop, reading the numbers, fixing the bass level,
and capturing again; plus the failure cases (unsaved set, capture track deleted by hand,
a set with no audio tracks allowed).

What the review changed: the tool returns per-bar RMS rather than one number, because "the
drop is quieter than the build" is the sentence the loop needs; and the capture track is
muted so a producer auditioning a capture by firing it does not hear it twice.

## Tool description

```text
capture_mix: Record a stretch of the arrangement as an audio clip and measure it.
Plays from `start` for `bars` bars (default 8, max 64), records the master through a
Capture track (created once, input Resampling), then reports the clip and its levels:
peak dBFS, RMS, RMS per bar, silent bars, clipped samples and stereo correlation. The
clip stays in the Capture track's Session slot named "<name> @ <start>" so you can play
it; its file path is returned for the app. Nothing else in the set changes. One capture
at a time; up to 64 bars.

list_captures: The captures in the Capture track: name, start, bars, file path, and the
levels already measured.

measure_capture: Re-read the levels of an existing capture (slot index) without playing.
```

## Acceptance Criteria

### Remote Script (Phase 1)
- [ ] **AC1 — `ensure_capture_track`.** Returns the index of the track named `Capture`,
      creating it as an audio track at the end if absent; sets its input routing type to
      the one whose display name is `Resampling` (error naming the available types if none
      matches), monitoring Off, mute on, arm on. Idempotent: a second call changes nothing
      and returns the same index.
- [ ] **AC2 — `start_capture(start, bars, name)`.** Refuses if a capture is already
      recording. Finds the first empty slot on the Capture track (error if none of the
      slots is free and says how many captures to delete). Moves the playhead to
      `start - 1` beat, starts the transport, and on the tick after the playhead has
      landed (the #25 pattern) fires the slot with `record_length = bars × beats per bar`
      and `launch_quantization` none, then names the clip `"<name> @ <start>"`. Answers
      the socket from that tick with `{slot, started_at, record_length}`.
- [ ] **AC3 — `capture_status(slot)`.** Returns `{is_recording, has_clip, file_path,
      length, name}` for the slot; `file_path` is present once recording has finished.
- [ ] **AC4 — `stop_capture(slot)`.** Stops the transport and the slot if still recording;
      used by the server on timeout so a failed capture never leaves Live recording.
- [ ] **AC5 — `list_captures`.** Every clip on the Capture track: slot, name, length in
      beats, file path, plus the levels stored in the clip name's suffix when measured
      (`… | -6.1 dBFS`). Empty list, not an error, when there is no Capture track.
- [ ] **AC6 — Live compatibility.** `ClipSlot.fire(record_length, launch_quantization)`
      needs Live 11+; on Live 10 the command errors with "captures need Live 11 or newer"
      and the capability list omits the capture commands so the tools refuse cleanly.
- [ ] **AC7 — Script hygiene.** No f-strings, type hints or imports beyond Live's; the
      four commands added to `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` bumped;
      `tools::ALL_REMOTE_COMMANDS` updated; the cross-check test passes.

### Server (Phase 1)
- [ ] **AC8 — `capture_mix` body.** `ensure_capture_track` → `start_capture` → poll
      `capture_status` every 250 ms until `has_clip && !is_recording` or the budget
      (`bars` at the set's tempo plus 5 s, max 90 s) → on timeout `stop_capture` and an
      error that says the capture was stopped → read the file → measure → name the clip
      with the peak → result text. Socket budget for `start_capture` and `stop_capture`
      is the modifying default; `capture_status` is a read.
- [ ] **AC9 — Reader.** `src/audio.rs` reads PCM WAV (via `hound`) and AIFF (own chunk
      reader) at 16, 24 and 32-bit integer and 32-bit float, mono or stereo, into `f32`
      channels; unsupported formats error with the format name; a file under 0.1 s errors
      as "empty capture".
- [ ] **AC10 — Measurements.** `peak_dbfs`, `rms_dbfs` (integrated), `rms_per_bar`
      (from the set's tempo and the clip's length, bars of `signature_numerator` beats),
      `silent_bars` (RMS below −60 dBFS), `clipped_samples` (|x| ≥ 0.999), `stereo
      correlation` (−1…1; "mono" below 0.1 width note), `duration_s`. Golden tests on
      synthetic buffers: a 0 dBFS square wave, silence, a −20 dBFS sine, a left-only
      signal.
- [ ] **AC11 — Result text.** One block: the clip name and slot, the file path, then the
      numbers with a one-line reading (e.g. "bars 5–8 are 4 dB quieter than 1–4", "left
      only", "3 clipped samples"). Under 900 characters for 8 bars.
- [ ] **AC12 — `list_captures` and `measure_capture` tools** as described.
- [ ] **AC13 — No new network, no new writes.** `tests/local_only.rs` still passes; the
      server reads Live's file and writes nothing but the activity line.

### App (Phase 2)
- [ ] **AC14 — Captures screen.** A "Captures" section on the Activity screen (or its own
      screen): every capture from `list_captures` via the sidecar's `--captures` flag, with
      name, start, bars, peak/RMS, a Play button (plays the file in place through the
      WebView audio element or `afplay`), and Reveal in Finder.
- [ ] **AC15 — The app never deletes audio.** "Delete all local data" leaves captures alone
      and the Your Data screen says so.

### Documents (Phase 1)
- [ ] **AC16:** `TERMS.md` gains a "Captures" paragraph (Live's own recordings, in the
      project folder, not touched by the app's delete). README tool table, feature matrix,
      architecture note (a "capture" paragraph next to `play_and_measure`) and the
      source-of-truth snapshot updated in the same PR.

### No Regressions
- [ ] **AC17:** `play_and_measure` unchanged; every existing tool behaves identically.
- [ ] **AC18:** A set with no Capture track and no free audio tracks (Live Intro's track
      limit) gets a clear error, and nothing in the set changes.
- [ ] **AC19:** A capture never leaves the transport running or the slot recording, even
      when the server is killed mid-capture: `stop_capture` runs from a `Drop` guard in the
      body, and the script's `start_capture` uses a fixed `record_length`, so Live stops
      the recording by itself even if nothing else arrives.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | five commands: `ensure_capture_track`, `start_capture`, `capture_status`, `stop_capture`, `list_captures`; `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` |
| `src/tools.rs` | `capture_mix`, `list_captures`, `measure_capture` bodies, params, specs, methods; `ALL_REMOTE_COMMANDS`; `run_named` entries |
| `src/connection.rs` | `start_capture`, `stop_capture`, `ensure_capture_track` in `MODIFYING_COMMANDS` |
| `src/bin/ableton-music-maker.rs` | `--captures` flag printing `list_captures` as JSON for the app |
| `Cargo.toml` | `hound` (WAV; no network, no C) |
| `app/src-tauri/src/*.rs`, `app/src/*` | Captures section, play, reveal |
| `TERMS.md`, `README.md`, `docs/…` | AC16 |

### New
| File | Description |
|------|-------------|
| `src/audio.rs` | WAV/AIFF PCM reader and the measurements |
| `tests/capture.rs` | the capture flow through `run()` with `FakeBridge` (command sequence, polling, timeout guard) and the measurement golden tests |

## Remote Script compatibility
- [ ] Handlers added (Live-Python compatible)
- [ ] Command names in `SCRIPT_CAPABILITIES`
- [ ] `SCRIPT_VERSION` bumped
- [ ] Commands in `tools::ALL_REMOTE_COMMANDS`
- [ ] Tool bodies call `require(live, "<command>")` first
- [ ] Live version floor stated: **Live 11+** for `ClipSlot.fire(record_length, …)`; the commands are omitted from the capability list on Live 10 so the tools refuse with the standard message
- [ ] Timeout class: `start_capture`, `stop_capture`, `ensure_capture_track` modifying (15 s); `capture_status`, `list_captures` reads (10 s)

## Privacy
New local data: **none written by us**. Captures are audio files Live records into its own
project folder, exactly as if the producer had pressed record. The server reads them to
measure, writes nothing, uploads nothing; the activity line records the tool call and the
file path (which can contain the project's name, hence the producer's naming — the same
class of text the log already holds). `TERMS.md` gains one paragraph saying so. The app's
"Delete all local data" does not touch captures. `tests/local_only.rs` unchanged and green.

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/capture.rs` (new) | command sequence `ensure_capture_track → start_capture → capture_status×N → (stop_capture on timeout)`; result text; timeout guard; refusal when already recording | AC8, AC11, AC19 |
| `src/audio.rs` unit tests | WAV and AIFF readers on generated files; measurement golden values | AC9, AC10 |
| `src/tools.rs` unit tests | tool count; `ALL_REMOTE_COMMANDS` cross-check | AC7 |
| `tests/local_only.rs` | unchanged, must stay green | AC13 |
| `tests/stdio_integration.rs` | `--captures` prints JSON | AC14 |

Manual, on Live 12 with a saved set: the verification steps below.

## Implementation Notes

### Phases
1. **Script + server capture and time-domain readout** — the loop closes here.
2. **App** — list, play, reveal.
3. **Spectral measurements** — separate story, same files.

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| Two-phase handler answering from a later tick (`DEFERRED`) | `_create_locator` (#25) | `start_capture`: move playhead, then fire on the next tick |
| Server-side polling loop with a budget and cleanup | `play_and_measure_body` | `capture_mix_body` |
| Per-command socket budget | `command_timeout` in `src/connection.rs` | reads vs modifying commands above |
| Fake-bridge command sequence assertions | `tests/orchestration.rs` | `tests/capture.rs` |

### Design Decisions
- **A named Capture track rather than a hidden one.** The producer sees exactly what Claude
  recorded and can play it in Live; hiding it would make the feature feel like surveillance
  of their own set.
- **Fixed-length recording rather than start/stop.** `record_length` makes Live end the
  recording by itself, so a crash on our side cannot leave Live recording.
- **Time-domain first.** Peak, RMS per bar and silence answer the questions the artist's
  session actually asked ("is the drop louder?", "is the bass there?"); spectral work is
  worth its own review.
- **No copying of audio.** The file stays where Live put it; the app plays it in place.

## Verification
1. Saved set open in Live 12, Remote Script reinstalled, Live restarted, Claude Desktop
   restarted.
2. Ask Claude: "capture bars 33–40 of the arrangement and tell me the levels". Expect a
   Capture track to appear at the end, muted and armed, a clip named `capture @ 128`, and a
   result with peak, RMS per bar and no clipping.
3. Play the clip in Live by firing its slot: it is the drop, once, with no doubling.
4. Ask for the same bars again: a second slot, no second Capture track.
5. Mute the bass track and capture again: the per-bar RMS drops and the reading says so.
6. Kill the server mid-capture (`kill` the pid from the app's heartbeat): Live stops
   recording on its own at `record_length`; the transport stops when the next server call
   runs `stop_capture`.
7. In the app, the capture appears with its levels; Play plays it; Reveal opens
   `Samples/Recorded`.

## Out of Scope
- Spectral and rhythmic measurements (next story).
- Exporting or rendering the whole set (impossible from a control surface, #11).
- Sending audio anywhere, including to a model.
- Arrangement-record captures and captures longer than 64 bars.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| Remote Script 1.10.2 (#25 two-phase pattern) | Merged, installed, loads at the next Live restart | |
| `ClipSlot.fire(record_length, launch_quantization)` | Live 11+ API | verify on Live 12.4.6 before writing the server side |
| `hound` crate | Ready | pure Rust, no network |

## Related Stories
- `measure-a-capture-by-band-and-section` — the spectral half; not written yet.
- `mac-app-installs-runs-and-watches-the-server` — the app that will list and play captures.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from the "I never heard the track" feedback and the resampling route |
