<div align="center">

# MCP Ableton Music Maker

### Your Live set, in conversation.

[![CI](https://github.com/defoAI/mcp-ableton-music-maker/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/defoAI/mcp-ableton-music-maker/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/JK4hNKGprW)

Claude Desktop, Claude Code or Cursor, working inside the Ableton Live set you have open.<br>
Build, shape, arrange, perform, and hear the result. Nothing leaves your machine.

[Install](#install) · [What you can say](#what-you-can-say) · [The Mac app](#the-mac-app) · [Your data](#your-data) · [How it is built](#how-it-is-built) · [Discord](https://discord.gg/JK4hNKGprW)

<sub>A third-party integration, not made by Ableton.</sub>

</div>

---

## A session

These are lines producers actually typed while this was being built, and the shape of what came back.

> **eight bars of that off-beat bass on F, with a turnaround in the last bar.**
>
> A clip on Bass, notes written from a step string per pitch, one undo step in Live.

> **find me a drum break around 90 bpm — the vinyl one, in the Verse, on its own track.**
>
> The server searches your sample folders and Live's browser, makes an audio track named after the file, and places the break in the Verse row: warping on, the loop snapped to whole bars, ready to play with the section.

> **the snare on the 4 is too loud, take it down 20, and push the off-beat hats.**
>
> Velocities, not faders: the notes on the beat come down, the off-beats come up, and the reply shows before and after.

> **how loud is it?**
>
> Eight bars of the master recorded through Live's own Resampling, then: peak −3.8 dBFS, RMS per bar, no clipping, bars 5–8 are 5.3 dB louder than 1–4.

> **play the song. go to the drop.**
>
> ```
> ⏱ bar 33.1 · next bar in 1.9 s
> 🔊 master −4.0 dB peak this bar · Kick −6 · Bass −7
> ```
> Every reply while performing carries the clock and the level. The jump lands at the end of the playing phrase, and the Remote Script's own clock makes it land there even if the server is busy.

Bars, dB, semitones, section names. The producer never sees a clip index.

## What makes it different

- **The set is the memory.** A section is a scene row named `Groove · 8`; the song is a `Setlist:` scene. Live's own Save keeps them, so nothing about your song lives in a sidecar file, and every set you already have can become one.
- **The producer's units, both ways.** Faders and meters in dB, Arrangement positions in Live's 1-based bars, note times in beats. Live's raw 0–1 fader curve and its beat times are the server's problem.
- **Validated before Live sees it.** A whole set from one document is checked end to end before the first command reaches Live, and `dry_run` shows the plan. Whatever changes, Live undoes in one step.
- **It hears what it made.** `capture_mix` records the master inside Live and reports what a mastering engineer would: peak, RMS per bar, silence, clipping, stereo correlation. A performance is kept as a take in the Arrangement, after what is already there, never over it unless you say so.
- **It performs on Live's clock.** A cue is handed to the Remote Script, which runs it on Live's own tick. A jump into a section that last ran hotter warns before it fires.
- **It stays on your machine.** One socket, to Live, on this machine only by default. No telemetry, no analytics, no dataset, no HTTP client in the dependency tree, and CI fails if one appears.

Works with **Live 11 and 12**; Live 10 without the Arrangement tools. Placing samples needs Live 12.

## Install

Two pieces ship together: the server, `ableton-music-maker`, a single Rust binary that speaks the [Model Context Protocol](https://modelcontextprotocol.io) to your client; and the **AbletonMusicMaker** Remote Script, a control surface that runs inside Live because Live loads control surfaces only through its own Python. The binary carries the script and installs it.

```
Claude client ──stdio──▶ ableton-music-maker ──TCP 9877, loopback──▶ Live + AbletonMusicMaker
```

### The Mac app

Installs the script, connects your client, and shows you whether the three are talking.

```bash
git clone https://github.com/defoAI/mcp-ableton-music-maker.git
cd mcp-ableton-music-maker/app && npm install && npm run dev      # Rust 1.85+, Xcode command line tools
```

**Setup** walks four steps and checks each against what is really there: install into Live's User Library; restart Live and pick **AbletonMusicMaker** in a **Control Surface** slot under **Settings → Link, Tempo & MIDI**, Input and Output **None**; add the server to Claude Desktop (the app edits the config with a backup beside it) or copy the command for Claude Code and Cursor; run a test call.

Every push to `main` builds and verifies an Apple Silicon disk image, kept as the `mac-app` artifact of the [CI run](https://github.com/defoAI/mcp-ableton-music-maker/actions/workflows/ci.yml). It is ad-hoc signed: a Mac that downloads it refuses it until `xattr -dr com.apple.quarantine "/Applications/Ableton Music Maker.app"`. A signed, notarised download follows once the Developer ID certificate is in the release pipeline.

### The binary alone

```bash
cargo install --path . --locked
ableton-music-maker-install-script          # the Remote Script into Live's User Library, with a .bak of what was there
claude mcp add AbletonMusicMaker ableton-music-maker
```

Restart Live and select the control surface as above. Claude Desktop takes `{"command": "ableton-music-maker"}` under `mcpServers.AbletonMusicMaker`; Cursor takes the same command under **Settings → MCP**. `ABLETON_HOST` and `ABLETON_PORT` override where Live is. Run one server at a time across all clients.

<details>
<summary><b>Docker</b> — built where it is used, not published</summary>

The image is hardened by contract and `docker/verify-image.sh` proves it against an image you built: distroless, non-root, read-only root filesystem, `/state` the only writable path, no upload code in the binary, under 50 MB.

```bash
docker compose build && docker/verify-image.sh mcp-ableton-music-maker:local
docker compose --profile install run --rm install-script
claude mcp add AbletonMusicMaker -- docker run --rm -i --read-only --security-opt no-new-privileges:true --cap-drop ALL -v ableton-music-maker-state:/state mcp-ableton-music-maker:local
```

For Claude Desktop, the same `docker run …` line becomes `command` and `args`. Never add `-t` (it breaks stdio) or `-p` (the container listens on nothing). Docker Desktop must be running whenever the client starts the server; opening this repository in Claude Code offers the container through [`.mcp.json`](.mcp.json).
</details>

## What you can say

The server presents **the artist's set** — 37 tools, verified 2026-09-20 against `src/tools.rs` — and serves the raw layer under it as `adv_…`, never hidden. Tracks and sections are addressed by name.

| You say | What happens | Tool |
|---|---|---|
| *build me a techno set in F minor at 126 with an intro, a groove, a break and a drop, 8-bar phrases* | One document becomes the set: key, tempo, sections as scene rows, tracks with instruments found by words, clips with notes. Validated first. | `build_song` |
| *give me a piano break, Fm9 Dbmaj7 Bbm7 Ab6, two bars each* | A clip named and filled in one call. Notes as step strings, patterns, `notes_csv`, or a bar tiled across the clip. | `create_clip` `add_notes_to_clip` |
| *same break in the Intro, up three, call it "chop up 3"* | A section copied from another with per-track changes: a transposition, a variation, empty, or new notes. | `make_section` |
| *the vinyl one, on its own track; put a crash at bar 5* | Audio from your folders or Live's browser into a section's row, or at a bar. Warped, looped to whole bars, one undo. | `add_sample` |
| *put an Echo on the pad; find me an analog bass* | Instruments and effects by plain words, onto a track, a return or the master. Answered from the server's own index of your library once it has walked it. | `load_instrument_or_effect` `search_browser` |
| *warmer. more attack on the bass. less reverb.* | Words resolved against the rack's macros first, then the instrument's own parameters, then any parameter by name; before and after in Live's display units. | `shape_sound` |
| *put an MPC swing on the hats, and quantize the keys to 16ths but only 80%* | Swing, humanize, a Groove Pool groove, retime, a variation, in one call. `undo: true` puts the clip back. | `feel` |
| *drop Ghosts at bar 9 and make it 4 bars* | The Arrangement in bars: place, repeat, move, delete, shorten, list. One round trip per track however many clips. | `arrange` `create_locator` |
| *intro twice, then groove, groove with pad, break, drop, and groove to end* | The setlist, written into the set. An entry without a count loops until you say go. | `set_song` `add_to_song` `remove_from_song` |
| *play the song. go. bring it down. again from the drop.* | Fires the first section and cues the counted jumps; then steer. A transition can ramp the tempo, retime, crossfade, fill, drop or sweep. | `play_song` `go` `jump_to` `back` `hold_section` `next_section` `previous_section` |
| *click on, count-in, record 8 bars of keys from bar 17* | The producer playing, into a Session clip on the next bar; the script names it and disarms the track. | `record_clip` |
| *how loud is it? is the drop too busy?* | Eight bars of the master measured. The Capture track stays until you clear it. | `capture_mix` `clear_captures` `end_performance` |
| *keep a copy of this set* | The whole set as a rebuildable document, written only when asked. | `export_set` `import_set` |

Plus `get_context`, which every session starts with: the set, every track with its devices and clips, the sections, the song and the clock, in one call. `delete_track`, `delete_clip` and `batch` complete the set. Every client receives this workflow at `initialize`, so the model knows it before you say anything.

Behind the artist's set are 105 tools in all: scenes and clips by index, cues with gestures (breakdown, drop, sweep, build, panic), meters, device chains (read a parameter as Live shows it, set it by that string, remove or bypass a device — on a track, a return or the master), mix snapshots, clip automation, the browser tree, sample folders. The one thing the Live API cannot do is save the set; you press Cmd+S.

## The Mac app

A menu bar app, Tauri 2, linked against the server's own crate.

- **The chain.** Client, server, Live: one state each, one fix each. The app never runs the server your client talks to; it makes that invisible process visible, and warns when two are talking to Live.
- **Prompts.** Four finished prompts for the four states you open the app in: make a song from nothing, prepare a set you can perform, finish what you already have, build around your sample. Copy one, paste it into Claude, and it asks you what to make before it writes a note — then keeps refining until you say the mix is done, or keeps writing the next section while the current one plays. They are plain markdown in [`prompts/`](prompts/), so you can read and paste them without the app; nothing is sent anywhere when you copy.
- **Activity.** Every tool call with its Live round-trip, the commands it sent, the result, and an estimated token cost, from the server's local log. Payloads (your MIDI and names) are shown only if you turned them on.
- **Listen.** What Live is putting out right now, through a macOS process tap of Live's process alone: no virtual audio driver, no routing change, Live keeps playing through its own output. A spectrum from 20 Hz to 20 kHz in dBFS with peak hold, master meters with a clip light, correlation, and the six ranges of a mix as their share of the whole. A small always-on-top window keeps it beside Live. The audio is analysed in memory about thirty times a second and thrown away. macOS 14.4 or later for this screen; the rest of the app runs on macOS 12.
- **Visual.** A full-screen feedback visual in the MilkDrop tradition, drawn from the same audio. Eight presets that never show you the same thing twice, because every visit re-rolls the palette, the warp, the shape and the fold, and never cut, because one becomes the next over several seconds on the same trails. Space wanders, 1–8 picks one, F is full screen.

## Your data

Nothing is uploaded, by the server or the app; there is no code that could. What is kept, all under `~/.ableton-music-maker/`:

| | Default | Off switch |
|---|---|---|
| Activity log: tool names, the Live commands sent, timings, sizes, results | on | `ABLETON_MCP_ACTIVITY=false`, or the app |
| Parameters and results, which contain your MIDI and names | **off** | `ABLETON_MCP_ACTIVITY_PAYLOADS=true` turns it on |
| The server's copy of Live's browser: names, paths, URIs | on | `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory |
| A set exported as a document | **only on `export_set`** | delete the file |
| Listening in the app | off until you start it | stops the moment no window shows it; never written |

Captures and takes are your project's own recordings and are never copied. **Delete all local data** in the app removes everything above. The whole story, in plain words: [TERMS.md](TERMS.md).

## When it goes wrong

| | |
|---|---|
| **"the Remote Script cannot run this command"** | The script in Live is older than the server expects. Install again (the app's Setup offers **Update Remote Script**), restart Live, re-select the control surface. `ableton-music-maker --check` prints the loaded and expected versions. |
| **"could not connect to Ableton"** | Live is not running, or AbletonMusicMaker is not selected as a control surface. |
| **The server is on another machine, or a container cannot reach the host** | The script binds `127.0.0.1` by default. Put the address to bind on the first line of `bind_host.txt` beside the script's `__init__.py` and restart Live. `--check` prints what it bound. |
| **Two servers** | Only one should talk to Live. Remove the extra client entry; if it is the original AbletonMCP, the app offers to. |
| **A timeout** | Ask for less at once. Making many tracks is allowed 190 s, importing audio 65 s, a browser search 25 s, other changes 15 s, reads 10 s. |
| **Listen shows nothing while Live plays** | macOS hands over silence when the audio-capture permission is off. Allow the app under **System Settings → Privacy & Security → Screen & System Audio Recording**. |

Diagnostics go to stderr; `RUST_LOG=debug` traces every command. `ableton-music-maker --status` prints versions and paths.

## How it is built

- **`src/`** is the server: tool bodies are plain functions, `fn(&LiveState, &Params) -> Result<String, String>`, so the whole suite runs against a fake Live. Compact note forms, the sound vocabulary, sections and songs, transitions, capture measurement and the library index are each a module of pure functions with their own tests.
- **The Remote Script** is one Python file, compatible with the Python Live bundles, embedded into the binary. It touches Live only from Live's main thread, in bounded slices, and reports what each command cost. The list of commands lives in the server, in one place; the script reads its own handlers back to say what it serves, the server checks that before every call, and a test fails the build if the two ever disagree.
- **`app/`** is the Mac app. Its Rust core links the crate by path; the tap of Live's audio is one Objective-C file compiled by `cc`, with everything newer than macOS 12 weak-imported behind `@available`.
- **Decisions that cost real work to reverse are written down**, numbered, never renumbered: [docs/decisions](docs/decisions/). Nine so far, all decided, from "the server is Rust and the script stays Python" to "CI builds the Mac app only".
- **Every number in the documentation is a copy** of one in the code, and `scripts/check-docs-facts.sh` fails CI when a copy drifts. The originals are listed in [docs/facts/source-of-truth.md](docs/facts/source-of-truth.md). Features start as questions and a prototype transcript, not a tool signature: [docs/](docs/README.md).

```bash
cargo test                                          # the server, against a fake Live
cargo clippy --all-targets -- -D warnings
scripts/check-docs-facts.sh
cd app/src-tauri && cargo test                      # add -- --ignored with Live open: two tests drive a real tap
cd app/src && node --test listen.test.mjs visual.test.mjs
cd app/src-tauri && cargo run --example listen_probe  # taps Live for a few seconds and prints what arrived
docker build --target test .                        # the same suite inside the image
```

CI is two jobs, both on macOS: the Rust gate, and the app with its verified disk image. A `v*` tag builds the signed, notarised one.

---

<div align="center">

[**Discord**](https://discord.gg/JK4hNKGprW) · [Issues](https://github.com/defoAI/mcp-ableton-music-maker/issues) · [Docs](docs/README.md)

MIT licensed. Derived from AbletonMCP by [Siddharth Ahuja](https://x.com/sidahuj) (MIT); this fork is maintained by DefoAI UG.<br>
This is a third-party integration and not made by Ableton.

</div>
