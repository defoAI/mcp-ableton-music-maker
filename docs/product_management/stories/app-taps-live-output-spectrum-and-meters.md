# The app taps Live's output for a spectrum and meters

## Story
**As a** producer with Live and the Mac app open,
**I want** a Listen screen that shows what Live is putting out right now, as a spectrum from 20 Hz to 20 kHz and master meters in dBFS, in a small window I can keep beside Live,
**So that** I see the low end, the peaks and the width of the mix while Claude and I change it, without routing Live's output through anything or installing a driver.

## Details
| Field | Value |
|-------|-------|
| Status | `In Progress` — built and tested against Live 12 on macOS 26; the signed-build check (question 10) and the macOS 12/13 launch check (AC5) remain manual |
| Surface | [Decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md): the app adds no tool and the server crate is untouched. Units are the artist's: dBFS with 0 at the top, frequencies in Hz, the six ranges by name |
| Priority | P2 — the artist's set is the priority; this is the first screen that shows the *result* of it |
| Size | L — three phases in Implementation Notes; the first is a spike that proves the tap on a signed build |
| Tracker | — |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Decisions | [0004](../../decisions/0004-who-publishes-and-holds-the-data.md) local data only · [0005](../../decisions/0005-mac-app-is-tauri-and-bundles-the-server.md) Tauri 2 · [0008](../../decisions/0008-app-hears-live-through-a-process-tap.md) a process tap of Live only, minimum macOS unchanged |
| Prototype | [prototypes/app-taps-live-output-spectrum-and-meters.html](../prototypes/app-taps-live-output-spectrum-and-meters.html) · [review link](https://claude.ai/artifact/L5mpwmGRgwoFE5PzK9XHUS) |

## Context

### The Problem
While Claude builds and shapes a set, the producer hears the mix but sees nothing of it
outside Live's own small meters. Live's Spectrum device is a device on a track; putting one
on the master and keeping its window open is a change to the set and a window Live owns.
Every visualiser app on the Mac either asks the producer to route Live's output through a
virtual audio device (BlackHole, Loopback) and rebuild a multi-output device for
monitoring, or lives inside the DAW as a plugin. Neither is a thing to ask of someone in
the middle of a session.

### Current State
- Live's API exposes no audio. What it exposes is one 0–1 meter value per track and for
  the master, read on Live's main thread
  (`AbletonMusicMaker_Remote_Script/__init__.py:2721` `_get_track_meters`; the reads
  are forced onto the main thread at `__init__.py:429-430` because they read as zero off it).
  The performance tick keeps per-bar peaks from the same values (`__init__.py:3776`
  `_level_tick`). That is enough for level bars, not for a spectrum.
- The server measures audio only after the fact: `capture_mix` records the master through
  Live's Resampling input, polls every 250 ms until the clip's file exists
  (`src/tools.rs:3362-3385`), then reads the file once for peak, RMS per bar, silence,
  clipping and stereo correlation (`src/audio.rs:235` `measure`). The module's contract is
  that it never writes, copies or sends audio (`src/audio.rs:6`).
- The Mac app is installer, status and activity log
  ([mac-app-installs-runs-and-watches-the-server](mac-app-installs-runs-and-watches-the-server.md)).
  Its Rust core links the crate and exposes commands to a plain HTML window
  (`app/src-tauri/src/lib.rs:179` `run`); the window polls status every 10 s
  (`app/src/app.js:482`). It declares macOS 12.0 as its minimum
  (`app/src-tauri/tauri.conf.json:34-36`) and one capability, `core:default`, for the
  `main` window (`app/src-tauri/capabilities/default.json`).
- macOS 14.2 added Core Audio process taps: an app can receive the audio another process
  sends to an output device, without a driver and without changing that process's routing.
  From macOS 14.4 the first tap surfaces a permission prompt whose text comes from
  `NSAudioCaptureUsageDescription` in the app's Info.plist; the permission is its own
  category, separate from the microphone and from screen recording. The prompt appears only
  for a signed binary. Rust bindings exist in `objc2-core-audio` (`CATapDescription`,
  `AudioHardwareCreateProcessTap`, `AudioHardwareDestroyProcessTap`). Sources:
  [Apple, Capturing system audio with Core Audio taps](https://developer.apple.com/documentation/CoreAudio/capturing-system-audio-with-core-audio-taps),
  [Apple, NSAudioCaptureUsageDescription](https://developer.apple.com/documentation/bundleresources/information-property-list/nsaudiocaptureusagedescription),
  [AudioCap sample, macOS 14.4+](https://github.com/insidegui/AudioCap),
  [objc2-core-audio](https://docs.rs/objc2-core-audio/latest/objc2_core_audio/).
- The other Apple route, ScreenCaptureKit (macOS 13), also captures one app's audio but
  under the Screen Recording permission, which is the wrong thing to ask for a meter.

### Root Cause
The product has never had a live view of the sound: the transport is a conversation, the
server is headless, and the app was designed to show *calls*, not *audio*.

## Open Questions

Proposed answers are marked; none is confirmed until the prototype review.

1. **Tap Live's process only, or the whole system output?** → *Proposed: Live only.* The
   spectrum then shows the set and nothing else, and the permission asked for is the
   narrowest macOS offers. A "system" option is a one-line change to the tap description if
   ever wanted.
2. **Which permission route?** → *Proposed: Core Audio process taps, not ScreenCaptureKit.*
   Taps ask for "audio from other apps"; ScreenCaptureKit asks for screen recording. A
   virtual audio driver is refused: it changes the producer's monitoring.
3. **Minimum macOS.** The tap API is 14.2, the prompt 14.4; the app declares 12.0. →
   *Proposed: keep 12.0 for the app, gate the screen at 14.4 at runtime.* The app's first job
   (install, connect, watch) must not lose a producer on macOS 13 over a visualiser. Cost:
   the three tap functions are resolved at runtime (`dlsym`) so the binary launches on
   older systems; the Listen screen says what it needs there.
4. **How does the app know the permission was denied?** macOS offers no public query for
   this category; a tap made without permission delivers silence. → *Proposed: one
   combined state.* After three seconds of silence while Live reports its transport running
   (over the existing Live socket, the same one `--check` uses), the screen says "Live is
   playing, but nothing arrives" and offers System Settings. With the transport stopped it
   says Live is quiet. No private TCC API.
5. **Should Claude see these numbers?** → *Proposed: not in this story.* Captures already
   give Claude a measured mix. A live feed would make the app a data source for the server
   (a file under `state_dir` the server polls), which is a new architecture and new data at
   rest; it gets its own story if wanted.
6. **A float window in the first release?** → *Proposed: yes.* A small always-on-top
   window is what lets the spectrum sit beside Live during a session; without it the screen
   is only visible when the app is in front.
7. **Waveform / scope view?** → *Proposed: later,* as a second view on the same screen. The
   prototype shows the control disabled.
8. **Per-track level bars beside the spectrum?** → *Proposed: separate story.* That data
   comes from the Remote Script's meters over port 9877, not from the tap, and polling it
   competes with Claude's commands for Live's main thread; it needs its own design.
9. **Should the six range names (sub, bass, low mids, mids, presence, air) join the sound
   vocabulary** so "more sub" works in `shape_sound`? → *Proposed: separate story.* Today
   the vocabulary is parameter words (`src/sound.rs:64-75`: cutoff, resonance, attack,
   drive, reverb …), not frequency ranges. The Listen screen names the ranges; teaching the
   server to act on them is a different change.
10. **Does the prompt appear on an unsigned dev build?** Unknown. The sources say a
    properly signed binary; Tauri's dev build is ad-hoc signed. → Answered by the Phase 1
    spike, before anything else is built.

## Prototype
[app-taps-live-output-spectrum-and-meters.html](../prototypes/app-taps-live-output-spectrum-and-meters.html):
the Listen screen inside the app's window, a menu bar item, and seven simulated states
(listening, not started, permission not given yet, silence with the permission off, Live not
running, macOS 13, Live open and quiet) plus the float window. Fake audio: a 126 BPM loop in
F minor synthesised in the page so the spectrum, the peak hold, the meters, the correlation
and the six range readouts move the way the real ones would. Real copy, real units.

Published for review at https://claude.ai/artifact/L5mpwmGRgwoFE5PzK9XHUS.

What the build changed: the analysis window is 4096 points (85 ms at 48 kHz) rather than 2048, so a tone reads its own level in its own band from about 300 Hz up; the state line therefore reports two numbers, the meters' lag and the spectrum's window. Question 7's "scope view later" became a third window instead: a full-screen WebGL visual (`visual.html`, eight MilkDrop-style presets, F / esc / space, auto-cycle) fed by the same frames, which now carry a 256-point waveform. The permission string is embedded by Tauri itself, so the prompt appears in development builds without a link-time trick. A measurement made along the way: Live's own 0–1 output meter is its fader scale, not linear amplitude — an 18 dB swing in true level moved it from 0.57 to 0.76 — so the tap's dBFS and the server's `meter_db` readouts are different scales; that is filed for the server, not changed here.

## Tool description
N/A — no MCP tool. The server crate is untouched. The app gains commands the window calls:

```text
listen_start     Start listening to Live: find Live's process, create the tap, start
                 analysis. Returns {sample_rate, buffer_frames, live_pid} or an error:
                 "Live is not running", "This screen needs macOS 14.4 (this Mac runs
                 13.6)", or the tap's own failure text.
listen_stop      Stop the tap and free everything. Never fails.
listen_status    {listening, sample_rate, live_pid, silent_for_ms, live_transport}
                 for the menu bar and the state line.
```

and one event the core emits about 30 times a second while listening:

```text
listen:frame     {bands: [72 × dBFS], hold: [72 × dBFS], peak: [L, R], rms: [L, R],
                  peak_hold: dBFS, correlation: -1…1, clip: bool, ranges: [6 × dB]}
```

## Acceptance Criteria

### The tap (Phase 1, a spike on a signed build first)
- [x] **AC1 — Live only.** The tap description names exactly one process: the running
      process whose bundle identifier is Live's. Nothing else on the system is tapped; the
      microphone is never opened. Live keeps playing through its own output, unmuted.
- [x] **AC2 — the prompt.** The bundle's Info.plist carries `NSAudioCaptureUsageDescription`
      with the text from the prototype's system prompt. The first "Start listening" on a
      signed build shows macOS's prompt once; a later start does not.
- [x] **AC3 — audio stays in memory.** Samples go from the tap's callback into a ring
      buffer and are consumed by the analysis thread; no code path in the listen module
      writes to a file, a socket or the clipboard. A test walks `app/src-tauri/src/listen/`
      and fails on any file or network API name (the pattern of `tests/local_only.rs:46`).
- [x] **AC4 — stops cleanly.** Stopping, leaving the Listen screen, hiding the window with
      no float window open, quitting, or Live exiting all destroy the tap and the aggregate
      device within one second. A tap that errors mid-stream flips the screen to the matching
      state rather than freezing.
- [~] **AC5 — older macOS.** *Built (12.0 deployment target, weak imports, the gate state); not yet run on a 12 or 13 Mac.* On macOS 12 and 13 the app launches, every other screen works,
      and the Listen screen shows the "needs macOS 14.4" state with the running version.
      The tap symbols are resolved at runtime; the build does not link them.

### The screen (Phase 2)
- [x] **AC6 — spectrum.** 72 log-spaced bands from 20 Hz to 20 kHz, in dBFS with 0 at the
      top and a 72 dB range, a 2 s peak-hold line, grid labels at 20, 50, 100, 200, 500 Hz
      and 1, 2, 5, 10, 20 kHz. Fast attack and slower release; a "Fast / Slow" switch sets
      the release. Redrawn on every frame event, about 30 a second; the page never polls.
- [x] **AC7 — meters.** Master left and right: sample peak and RMS as two bars on one
      scale (0 to −60 dBFS), a peak-hold line, numeric peak L / R, RMS, correlation (−1 to
      +1) and peak hold. A clip LED lights when a sample reaches ±1.0 and stays on for 2 s.
      Numbers use the same dB formatting as the server's readouts (`src/song.rs:528`
      `meter_db`: −80 is the floor).
- [x] **AC8 — the six ranges.** Under the spectrum: sub (20–60 Hz), bass (60–200),
      low mids (200–500), mids (500–2 k), presence (2–6 k), air (6–20 k), each as its share
      of the mix's energy over the last second, in dB relative to the whole. Copy as in the
      prototype.
- [x] **AC9 — states.** Exactly the prototype's: not started (explanation and Start);
      permission not given yet (what macOS will ask and why); listening (state line with
      pid, sample rate, buffer size and lag); Live open and quiet; Live playing but silent
      (System Settings button); Live not running (Start disabled); macOS too old. Each is
      derived from real state, never from a click.
- [x] **AC10 — listening is visible.** A pulsing LED on the Listen item in the sidebar and
      a "Listening" item in the menu bar while the tap runs; both gone when it stops.
- [x] **AC11 — float.** "Float" opens a second, always-on-top, 340 × 150 px window with the
      spectrum and the peak value; closing it stops nothing while the main Listen screen is
      open, and stops the tap if the main window is hidden. It remembers its position.
- [x] **AC12 — lag.** *Measured 43.7 ms at 48 kHz with Live's 512-frame buffer; the spectrum's 85 ms window is reported beside it.* The state line's "about N ms behind the speakers" is measured, not
      typed: tap buffer duration plus the analysis window, reported per session. Target
      under 60 ms at 48 kHz with a 256-frame buffer.

### Settings and data (Phase 2)
- [x] **AC13 — "Your data" gains one row:** "Listening — nothing is stored. When you start
      listening, Live's audio is analysed in memory about 30 times a second and discarded."
      `TERMS.md` gains the Listening section below.
- [x] **AC14 — no new files.** The listen feature writes nothing under
      `~/.ableton-music-maker/` and nothing in the app's settings except the float window's
      position and the Fast / Slow choice.

### The visual (added during the build)
- [x] **AC19 — visual window.** "Visual" and "Full screen" on the Listen screen open a third window drawn in WebGL from the same frames: a feedback warp with eight presets (each a warp mode, a wave shape, a fold and a palette), the waveform and spectrum drawn on top with trails, beat-reactive zoom and brightness. Space / → next, ← previous, 1–8 by number, A auto-cycle, F full screen, esc leaves it, H help; the cursor hides in full screen. Listening keeps running while it is open, and it goes dark with "Waiting for Live" when frames stop.

### Packaging (Phase 3)
- [ ] **AC15:** The Info.plist merge is part of `cargo tauri build`; the CI `mac-app` job
      still builds unsigned; the signed release build shows the prompt. The README's app
      section gains one sentence and a screenshot of the Listen screen; the
      [feature-matrix](../../technical/feature-matrix.md) and
      [architecture overview](../../architecture/overview.md) gain the app's listen module.

### No Regressions
- [x] **AC16:** The server crate, its tool count, the Docker image and every `cargo test`
      suite are unchanged; `tests/local_only.rs` still passes as is.
- [x] **AC17:** The app opens no socket it did not open before; the tap is not a socket.
      The Overview, Activity, Setup and Settings screens behave as before, and the 10 s
      status poll is unaffected by listening.
- [x] **AC18:** Live's audio settings, output device and routing are untouched before,
      during and after listening; nothing is muted.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `app/src-tauri/Cargo.toml` | `objc2-core-audio`, `objc2-core-foundation` (tap description, aggregate device), `realfft` (or `rustfft`) |
| `app/src-tauri/tauri.conf.json` | no version change; `bundle.macOS` keeps 12.0 |
| `app/src-tauri/capabilities/default.json` | `windows: ["main", "float"]` |
| `app/src-tauri/src/lib.rs` | `listen_start`, `listen_stop`, `listen_status` commands; the float window builder |
| `app/src-tauri/src/tray.rs` | the "Listening" item |
| `app/src-tauri/src/status.rs` | `live_transport` from the existing check, for question 4 |
| `app/src-tauri/src/settings.rs` | float position, Fast / Slow |
| `app/src/index.html`, `app/src/app.css`, `app/src/app.js` | the Listen screen and the sidebar item, from the prototype; the frame event listener and the two canvases |
| `app/README.md` | the listen module, the signing note for the prompt |
| `TERMS.md` | the Listening section |
| `docs/technical/feature-matrix.md`, `docs/architecture/overview.md` | the app's listen module |

### New
| File | Description |
|------|-------------|
| `app/src-tauri/Info.plist` | `NSAudioCaptureUsageDescription`; merged by Tauri into the bundle |
| `app/src-tauri/src/listen/mod.rs` | the state machine: idle → starting → listening → stopped, and the error states |
| `app/src-tauri/src/listen/tap.rs` | find Live's process object, `CATapDescription` (Live only, stereo, unmuted), private aggregate device, IOProc into the ring buffer; symbols resolved at runtime |
| `app/src-tauri/src/listen/analysis.rs` | Hann window, 2048-point FFT, 72 log bands, peak / RMS / correlation, peak hold, the six ranges — pure functions over `&[f32]`, tested |
| `app/src/listen.js` | the drawing, split out because the canvases are the one place the app animates |
| `app/src/float.html` | the float window's page |

## Remote Script compatibility
No command changes. Question 4 uses `get_session_info` as `--check` already does
(`src/app.rs:40`). `SCRIPT_VERSION` unchanged.

## Privacy
New data *in flight*, none at rest:

| What | Where | Default | Off switch |
|---|---|---|---|
| Live's output audio, stereo, at the device rate | memory only: a ring buffer of under one second, consumed by the analysis thread | **off** — only while "Start listening" is active and the screen or float window is open | Stop, leave the screen, close the app; macOS's own switch under Privacy & Security |
| Analysis frames: 72 band levels, meters, six range levels | memory only, sent to the window as an event | same | same |

Nothing is written under `~/.ableton-music-maker/`, nothing is uploaded, and the server has
no part in it ([0004](../../decisions/0004-who-publishes-and-holds-the-data.md)). Only Live's
process is tapped; the microphone is never opened; other apps' audio is never received.

`TERMS.md` gains a **Listening** section saying the above in plain words, and its sentence
about the Mac app ("reads files the server writes on your Mac and never connects to anything
but Live") gains "and, when you ask it to, listens to Live's audio in memory". The test
named in AC3 pins that the listen module has no file or network API; `tests/local_only.rs`
is unchanged because the server is.

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `app/src-tauri` unit tests: `listen::analysis` | band edges cover 20 Hz–20 kHz with no gap; a 1 kHz sine at −6 dBFS lands in the right band at −6 ±0.5 dB; silence gives −80 everywhere and correlation 0; identical channels give +1, inverted −1; a sample at 1.0 sets clip; the six ranges sum to 0 dB | AC6–AC8 |
| `app/src-tauri` unit test: listen module source walk | no `std::fs`, `File`, `TcpStream`, `UdpSocket`, `Command` in `src/listen/` | AC3 |
| `app/src-tauri` unit test: state machine | every transition in AC9 from a fake tap that yields frames, silence, or an error | AC4, AC9 |
| `cargo test` (root) | unchanged | AC16 |
| Manual, on a signed build | the Verification steps | AC1, AC2, AC5, AC11, AC12, AC18 |

## Implementation Notes

### Phases
1. **Spike** — a `cargo run` in `app/src-tauri` that taps Live, prints peak L / R per
   second to stderr, on a Developer-ID-signed build. Answers question 10 and confirms the
   tap chain. Nothing ships.
2. **Screen** — the module, the analysis, the Listen screen, the float window, TERMS.
3. **Packaging** — Info.plist in the release, README, matrix, overview.

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| Pure analysis over decoded samples, no I/O | `src/audio.rs` `measure` | `listen::analysis` |
| dB from linear with a −80 floor | `src/song.rs:528` `meter_db` | every number on the screen |
| One state derived from real checks, never from clicks | Setup steps in `app/src/app.js` | the Listen states |
| Source-walk test that fails on a forbidden API name | `tests/local_only.rs:46` | the listen module's no-I/O test |
| Plain HTML, no framework, one file per screen concern | `app/src/*` | `listen.js`, `float.html` |

### Design Decisions
- **A tap of Live, not the system output.** The narrowest permission, and the spectrum
  is the set's. Muting is off: the producer keeps hearing Live through its own output.
- **Not a virtual audio driver.** It would change Live's output device and the producer's
  monitoring chain, and it is a kernel or system extension to install and sign.
- **Not ScreenCaptureKit.** It works from macOS 13, but asks for Screen Recording, which is
  the wrong prompt for a meter and a hard one to justify in the Privacy section.
- **Symbols at runtime, minimum macOS unchanged.** The tap functions are looked up with
  `dlsym` against the already-loaded CoreAudio framework; on macOS 12 and 13 the lookup
  fails and the screen says so. Raising the app's minimum to 14.4 for one screen would drop
  producers whose Live runs fine on 13.
- **Analysis in Rust, drawing in the WebView.** The FFT runs on a thread in the core; the
  window only receives about 90 numbers per frame and draws two canvases. The audio thread
  itself does nothing but copy into the ring buffer.
- **Silence is ambiguous, and the screen says so.** macOS gives no public way to ask
  whether the permission is on. Combining the tap's silence with Live's transport state over
  the existing socket gives the producer the one sentence they need.
- **Claude does not see this.** The server stays headless and the app stays a viewer
  ([0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md)). Feeding
  the spectrum to Claude is a separate story with its own Privacy section.

## Verification

With Live 12 open and playing, on a Developer-ID-signed build of the app, on macOS 14.4 or later.
1. Open Listen. The "not started" state explains what happens. Click Start listening: the
   permission explanation appears, then macOS's prompt with the exact text from
   Info.plist. Allow. The spectrum moves with the music; Live's sound is unchanged; Live's
   audio preferences show the same output device as before.
2. The state line names Live's pid, the sample rate and buffer size Live is set to
   (compare with Live's Audio preferences), and a lag under 60 ms.
3. Play a 1 kHz test tone at −6 dB from a Live utility: the 1 kHz band reads −6 ±0.5 dBFS,
   L and R peak read −6.0, correlation +1.00. Pan hard left: correlation drops toward 0.
   Push the master into clipping: the clip LED lights for 2 s.
4. Stop Live's transport: the "Live is open and quiet" state. Start it again: listening
   resumes without a new prompt.
5. Turn the permission off in System Settings › Privacy & Security › Screen & System
   Audio Recording, restart the app, start listening with Live playing: after 3 s the
   "Live is playing, but nothing arrives" state with the System Settings button.
6. Float: the small window stays above Live's window; close the main window, the float
   keeps drawing; close the float, the tap stops within a second (Activity Monitor shows
   no aggregate device left behind in `coreaudiod`; the menu bar "Listening" item is gone).
7. Quit Live while listening: the "Live is not running" state within a second.
8. On a macOS 13 Mac: the app opens, the Listen screen shows the "needs 14.4" state, Setup
   and Activity work as before.
9. `ls ~/.ableton-music-maker/` before and after a session of listening: identical.

## Out of Scope
- Per-track meters (the Remote Script's meters over port 9877): separate story.
- Feeding the analysis to Claude or to the server: separate story, new Privacy section.
- A waveform or stereo scope view; a spectrogram.
- Recording or exporting anything from the tap. Captures remain the way audio is kept,
  inside Live's project.
- Tapping the whole system, or any process other than Live.
- Windows and Linux (the app is Mac only).
- Adding the six range names to `shape_sound`.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| [mac-app-installs-runs-and-watches-the-server](mac-app-installs-runs-and-watches-the-server.md) Phase 3 (signing) | Open | the prompt appears only on a signed build; the spike needs the Developer ID |
| macOS 14.4 or later on the review Mac | — | for the prototype review of the real prompt |
| `objc2-core-audio` exposing the tap functions | Ready | verified in the crate's item list |
| A decision record for the tap (Live only, taps not ScreenCaptureKit, minimum macOS kept) | Written | [0008](../../decisions/0008-app-hears-live-through-a-process-tap.md) |

## Related Stories
- `mac-app-installs-runs-and-watches-the-server` — the app this screen joins; its signing phase is the dependency.
- `capture_mix` and `listen {capture: true}` (shipped, `src/audio.rs`) — the after-the-fact measurement of the same signal; the numbers here use their formatting and their floor.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from the question "can the app show a visualiser / equaliser of Ableton's output"; prototype built, not yet reviewed |
| 2026-09-19 | Built end to end: the tap, the analysis, the screen, the float window, the visual; tested against Live 12 Suite on macOS 26.6 (frames, levels, Live-only, no leaked device); Remote Script 1.24.0 installed with the loopback bind (decision 0003) in the same pass |
