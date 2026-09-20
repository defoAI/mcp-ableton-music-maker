<div align="center">

# MCP Ableton Music Maker

**Talk to your Live set.**

Claude builds the track, shapes the sound, arranges the song, performs it on the bar and hears the result — inside the Ableton Live set you already have open. Nothing leaves your machine.

[![CI](https://github.com/defoAI/mcp-ableton-music-maker/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/defoAI/mcp-ableton-music-maker/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/JK4hNKGprW)

[**Setup video**](https://youtu.be/iJWJqyVuPS8) · [**Demo: an 80s synthwave track**](https://youtu.be/VH9g66e42XA) · [**Discord**](https://discord.gg/JK4hNKGprW) · [**Issues**](https://github.com/defoAI/mcp-ableton-music-maker/issues)

*Third-party integration, not made by Ableton.*

</div>

---

## What it feels like

> **You:** build me a techno set in F minor at 126 with an intro, a groove, a groove with pad, a break and a drop. 8-bar phrases, the break 16.
>
> **Claude** builds five sections as scene rows, a kick, a bass that follows the key, a pad, the clips filled with notes — one validated call, one undo step in Live.
>
> **You:** the low end is muddy. Tighten the bass, more attack, and capture the drop so I can hear it.
>
> **Claude** turns the words into the bass instrument's own parameters, records eight bars of the master through Live's Resampling, and tells you the peak, the RMS per bar and where the energy sits.
>
> **You:** play the song. Jump to the break on the next phrase, with a crossfade.
>
> **Claude** hands the jump to the Remote Script's clock, and every reply carries the bar you are on and the level under it.

The producer talks in bars, dB and the words a producer uses. Claude talks to Live.

## How it works

```
Claude Desktop / Claude Code / Cursor
        │  MCP over stdio
        ▼
ableton-music-maker            one Rust binary, 101 tools
        │  TCP 9877, this machine only by default
        ▼
Ableton Live  ◀── AbletonMusicMaker Remote Script (a control surface inside Live)
```

Two pieces, and both are in this repository.

- **`ableton-music-maker`** speaks the [Model Context Protocol](https://modelcontextprotocol.io) to your client and drives Live over a local socket. It checks every call against the loaded Remote Script's version and capabilities before it runs, validates a whole song before the first command reaches Live, and turns Live's raw numbers into the producer's units on the way back.
- **The AbletonMusicMaker Remote Script** runs inside Live, because Live loads control surfaces only through its own Python. It executes each command on Live's main thread in bounded slices, so Live keeps playing while Claude works, and it keeps the performance clock so a cue lands on the bar even if the server is busy. The binary embeds the script and installs it for you.

**Nothing is uploaded.** There is no telemetry, no analytics, no dataset and no HTTP client in the dependency tree — CI fails if one appears, and `docker/verify-image.sh` checks the image carries none. What is kept on your machine, and how to delete it, is in [Your data](#your-data).

Works with **Live 11 and 12**; Live 10 without the Arrangement tools. Placing samples needs Live 12.

## Get started on a Mac

**The app** installs the Remote Script into Live, connects your client to the bundled server, shows at a glance whether Claude, the server and Live are talking, and lists every call with its timing and an estimated token cost.

```bash
git clone https://github.com/defoAI/mcp-ableton-music-maker.git
cd mcp-ableton-music-maker/app
npm install && npm run dev          # Rust 1.85+, Xcode command line tools
```

Then in the app: **Setup → Install into Live**, restart Live, pick **AbletonMusicMaker** in a **Control Surface** slot under **Settings → Link, Tempo & MIDI** (Input and Output **None**), and **Add to Claude Desktop**. Claude Code and Cursor get the command to paste. Four steps, each checked against what is really there.

Every push to `main` builds and verifies an Apple Silicon disk image, kept as the `mac-app` artifact of the [CI run](https://github.com/defoAI/mcp-ableton-music-maker/actions/workflows/ci.yml). It is ad-hoc signed, so a Mac that downloads it refuses it until you clear the quarantine flag:

```bash
xattr -dr com.apple.quarantine "/Applications/Ableton Music Maker.app"
```

A signed, notarised download follows once the Developer ID certificate is in the release pipeline.

### Or just the binary

```bash
cargo install --path . --locked
ableton-music-maker-install-script                 # copies the Remote Script into Live's User Library
ableton-music-maker-install-script --list-targets  # shows the folders it would use
```

Restart Live and select the control surface as above, then register the server with your client:

```bash
claude mcp add AbletonMusicMaker ableton-music-maker
```

For Claude Desktop, add `{"command": "ableton-music-maker"}` under `mcpServers.AbletonMusicMaker` in **Settings → Developer → Edit Config**; for Cursor, paste the command under **Settings → MCP**. The server reads Live's address from `ABLETON_HOST` (default `localhost`) and `ABLETON_PORT` (default `9877`). Run one instance of the server at a time across all clients.

### Or Docker

The image is built where it is used — it is not published — and is hardened by contract: distroless, non-root, read-only root filesystem, `/state` the only writable path, no upload code in the binary, and under 50 MB. `docker/verify-image.sh` checks all of that against an image you built.

```bash
docker compose build
docker/verify-image.sh mcp-ableton-music-maker:local
docker compose --profile install run --rm install-script     # the Remote Script into Live's User Library
```

<details>
<summary>Client configuration for the container</summary>

Claude Desktop:

```json
{
  "mcpServers": {
    "AbletonMusicMaker": {
      "command": "docker",
      "args": ["run", "--rm", "-i", "--read-only", "--security-opt", "no-new-privileges:true",
               "--cap-drop", "ALL", "-v", "ableton-music-maker-state:/state",
               "mcp-ableton-music-maker:local"]
    }
  }
}
```

Claude Code:

```bash
claude mcp add AbletonMusicMaker -- docker run --rm -i --read-only --security-opt no-new-privileges:true --cap-drop ALL -v ableton-music-maker-state:/state mcp-ableton-music-maker:local
```

Do not add `-t` (it breaks the stdio transport) or `-p` (the container listens on nothing). Docker Desktop must be running whenever the client starts the server. Opening this repository in Claude Code offers the container through [`.mcp.json`](.mcp.json).
</details>

## What Claude can do

The server presents **the artist's set** — 37 tools, verified 2026-09-20 against `src/tools.rs` — and serves the raw layer underneath it as `adv_…`, never hidden. Faders and levels are in dB, Arrangement positions in Live's 1-based bars, note times inside a clip in beats. Tracks and sections are addressed by name.

| | Tools | What the producer gets |
|---|---|---|
| **Look** | `get_context` | The whole set in one call: every track with its mixer, devices and clips, the returns, the sections, the clock, and the workflow reminder. Start here. |
| **Build** | `build_song` `make_section` `create_clip` `add_notes_to_clip` `load_instrument_or_effect` `search_browser` `add_sample` `set_key` `set_tempo` | A whole set from one document — key, tempo, sections as scene rows, tracks with instruments found by words, clips with notes — validated before the first command reaches Live, `dry_run` to preview. Notes in compact forms: a step string per pitch (`{"36": "x...x...x...x..."}`), patterns, `notes_csv`, a bar tiled across a clip. Instruments and effects by plain words or a browser URI, onto a track, a return or the master. Audio you already own placed into a section's row or at a bar, warped and fitted, one undo step. |
| **Shape** | `shape_sound` `feel` `set_track_mixer` `set_send` `create_return` | Cutoff, resonance, attack, drive, reverb … as words, resolved against a rack's macros first, then the instrument's own parameters, then any parameter by name. Swing, humanize, a Groove Pool groove, retime, a variation — one call, one undo. Faders in dB. |
| **Arrange** | `set_song` `add_to_song` `remove_from_song` `arrange` `create_locator` | A song is a setlist of sections, kept in the set itself as scene names, so Live's own Save keeps it. Place, repeat, move, delete and shorten in the Arrangement in bars, one round trip per track however many clips. |
| **Play** | `play_song` `go` `jump_to` `back` `hold_section` `next_section` `previous_section` `record_clip` `capture_mix` `clear_captures` `end_performance` | Every jump lands at the end of the playing section's phrase unless told otherwise; a transition can be a tempo ramp, a retime, a crossfade, a fill, a drop or a sweep. Every reply carries the bar you are on and the master level under it. Record the producer playing; capture the master through Resampling and get it measured. |
| **And** | `delete_track` `delete_clip` `export_set` `import_set` `batch` | Set memory as a rebuildable document, written only when asked. Several calls in one round trip. |

The raw layer — scenes, clips, cues with gestures (breakdown, drop, sweep, build, panic), meters, snapshots, automation, the browser tree, sample folders — is there for the moment the artist's set has no word for what you want, served as `adv_<name>` with "(advanced)" in front of its description.

Every client receives a short workflow at `initialize`: `get_context`, then `build_song`, hear it with `capture_mix`, perform it with `play_song` and the steering verbs. A missing or outdated Remote Script produces a plain "run the installer, then restart Live" instead of a half-working session.

### Prompts that work

- *"Create an 80s synthwave track"* — [watch it happen](https://youtu.be/VH9g66e42XA)
- *"Create a Metro Boomin style hip-hop beat"*
- *"Build a full arrangement with an intro, buildup, drop, breakdown and outro"*
- *"Add a jazz chord progression to the clip in track 1"*
- *"Put the kick from my samples folder on a new track, one hit per bar"*
- *"Capture the drop and tell me if it clips"*
- *"Play the song — I'll say go each time"*

## Hear it, see it

**Claude hears the set.** `capture_mix` records bars of the master inside Live, exactly as if you had pressed record, and reads the file once: peak, RMS per bar, silent bars, clipping, stereo correlation. The audio stays in your project; nothing is copied.

**You see it.** The Mac app's **Listen** screen taps the audio Live sends to your speakers through a macOS process tap of Live's process alone — no virtual audio driver, no routing change, Live keeps playing through its own output — and draws a spectrum from 20 Hz to 20 kHz in dBFS, the master meters with peak hold and a clip light, and the six ranges of a mix as their share of the whole. A small always-on-top window keeps it beside Live. **Visual** opens a full-screen feedback visual in the MilkDrop tradition, drawn from the same audio: eight presets that never repeat, because every visit re-rolls the palette, the warp, the shape and the fold, and never cut, because one becomes the next over several seconds on the same trails. Space wanders, F is full screen.

The audio is analysed in memory about thirty times a second and thrown away; it is never written and never sent. The Listen screen needs macOS 14.4 or later; the rest of the app runs on macOS 12.

## Your data

Nothing is uploaded, by the server or by the app; there is no code that could. What is kept on your machine:

| What | Default | Where |
|---|---|---|
| Activity log — tool names, the Live commands sent, timings, sizes, results | on | `~/.ableton-music-maker/activity/` |
| The parameters and results themselves, which contain your MIDI and names | **off** — `ABLETON_MCP_ACTIVITY_PAYLOADS=true` turns it on | same files |
| The server's copy of Live's browser: names, paths, URIs | on — `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory | `~/.ableton-music-maker/library/` |
| A set exported as a document | **only on `export_set`** | `~/.ableton-music-maker/sets/` |

Captures are your project's own recordings and are never copied. The Remote Script accepts connections from this machine only by default. The app's **Delete all local data** removes everything in the table; so does deleting the folder. The whole story: [TERMS.md](TERMS.md).

## Troubleshooting

| Problem | Fix |
|---|---|
| Tools say the Remote Script cannot run a command | Run the installer again (or **Update Remote Script** in the app), restart Live, re-select AbletonMusicMaker as a control surface. `ableton-music-maker --check` prints the loaded and expected versions. |
| "could not connect to Ableton" | Live is not running, or the control surface is not selected. From Docker, Live must be reachable at `host.docker.internal:9877`. |
| The server runs on another machine, or a container cannot reach the host's loopback | The script binds `127.0.0.1` by default. Put the address to bind — `0.0.0.0` for any interface — on the first line of `bind_host.txt` beside the script's `__init__.py` and restart Live. `--check` prints what the loaded script bound. |
| Timeouts | Break the request into smaller steps. Creating many tracks is given 190 s, importing audio 65 s, a browser search 25 s, other changes 15 s, reads 10 s. |
| Two servers are talking to Live | Only one should. The app warns when it sees two; remove the extra client entry. Upgrading from the original AbletonMCP? Remove its entry — the app offers to. |
| Nothing arrives on the Listen screen while Live plays | macOS delivers silence when the audio-capture permission is off. Allow **Ableton Music Maker** under **System Settings → Privacy & Security → Screen & System Audio Recording**. |

Diagnostics go to stderr; `RUST_LOG=debug` gives the full command trace. `ableton-music-maker --status` prints versions and paths.

## Development

```bash
cargo test                                   # the server's suites, FakeBridge in place of Live
cargo clippy --all-targets -- -D warnings    # CI runs this
scripts/check-docs-facts.sh                  # every number in the docs against the code
docker build --target test .                 # the same suite inside the image
cd app/src-tauri && cargo test               # the app; --ignored adds two that need Live open
cd app/src && node --test listen.test.mjs visual.test.mjs
cd app/src-tauri && cargo run --example listen_probe   # taps Live for a few seconds and prints what arrived
```

The Remote Script is `AbletonMusicMaker_Remote_Script/__init__.py`, embedded into the binary at build time; its `SCRIPT_VERSION` is what the server expects, and it stays compatible with the Python Live bundles. Adding a command means a handler there, its name in `SCRIPT_CAPABILITIES`, a version bump, and a test cross-checks the list against the server.

CI runs two jobs, both on macOS: the Rust gate, and the app with its verified disk image. A `v*` tag builds the signed, notarised image.

The product layer — what this is, what it does today, the decisions behind it, and how a feature is designed before it is built — lives in [docs/](docs/README.md). Every number in prose is a copy; the originals are in [docs/facts/source-of-truth.md](docs/facts/source-of-truth.md).

## Community

Feedback, ideas, and what people are building with it: [**Discord**](https://discord.gg/JK4hNKGprW) · [Issues](https://github.com/defoAI/mcp-ableton-music-maker/issues)

## Disclaimer

This is a third-party integration and not made by Ableton. Derived from AbletonMCP by [Siddharth Ahuja](https://x.com/sidahuj) (MIT); this fork is maintained by DefoAI UG. Licensed under [MIT](LICENSE).
