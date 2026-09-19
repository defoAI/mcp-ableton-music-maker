# 0007 — How does the Mac app hear what Live is playing?

| | |
|---|---|
| **Status** | **Decided — a Core Audio process tap of Live's process only; the app's minimum macOS stays 12** |
| **Raised** | 2026-09-19 (the Listen screen story) |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | The Listen screen, its float window and the visual · the Listening section of `TERMS.md` |

## Context

Live's API exposes no audio, only one 0–1 meter value per track read on Live's main thread.
A spectrum, a waveform or a visual needs the samples. On a Mac there are three ways to get
another app's audio: a virtual audio driver the producer routes Live through (BlackHole,
Loopback), ScreenCaptureKit under the Screen Recording permission (macOS 13), or a Core Audio
process tap under its own audio-capture permission (API macOS 14.2, prompt from 14.4).

The parts that cost real work to reverse are the permission the producer is asked for, which
process is tapped, and the app's minimum macOS.

## Options

- **A. A virtual audio driver** — works on every macOS. Against: changes Live's output
  device and the producer's monitoring chain, and is a system extension to install and sign.
- **B. ScreenCaptureKit** — macOS 13. Against: asks for Screen Recording, the wrong prompt
  for a meter and a hard one to explain in the Privacy section.
- **C. A process tap of Live's process** — the decision. The narrowest permission macOS
  offers ("record audio from other apps"), nothing routed, Live keeps playing through its
  own output, unmuted. Against: macOS 14.4 for the prompt.
- **Raise the app's minimum to 14.4** — refused: the app's first job (install, connect,
  watch) must not lose a producer on macOS 12 or 13 over a visualiser.

## Decision

**C, of Live's process only, with the app's minimum unchanged.** The tap is compiled with a
12.0 deployment target and every 14.2 symbol is weak-imported behind `@available`, so the
binary launches on 12 and 13 and the Listen screen says what it needs there. Only the
process whose executable lives in an `Ableton Live*.app` bundle is tapped; the microphone is
never opened and other apps' audio never arrives — measured: a −3 dBFS tone from another
process while tapping Live read −∞. The audio lives in a ring buffer in memory and is turned
into about ninety numbers thirty times a second; nothing is written.

macOS gives no public way to ask whether the permission is on; a denied tap delivers silence.
The screen therefore combines silence with Live's transport state over the existing socket
and says "Live is playing, but nothing arrives" after three seconds. No private API.

## Consequences

- The app links `CoreAudio`, `AudioToolbox` and `Foundation`, and `cc` compiles one
  Objective-C file (`app/src-tauri/src/listen/tap.m`). The bundle's Info.plist carries
  `NSAudioCaptureUsageDescription`; Tauri embeds it in development builds too, so the prompt
  appears under `cargo tauri dev`.
- New data in flight, none at rest: `TERMS.md` has a Listening section, and a test in the
  listen module fails on any file or network API in its source.
- The frames the tap produces are the app's, not the server's. Feeding them to Claude is a
  separate story with its own Privacy section ([0006](0006-one-artist-surface-raw-layer-marked-advanced.md)).
