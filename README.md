<div align="center">

# MCP Ableton Music Maker

**Connect Ableton Live to Claude AI**

Prompt-assisted music production, end-to-end track creation, and Live session and arrangement manipulation — driven by AI.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/JK4hNKGprW)

[**Setup Video**](https://youtu.be/iJWJqyVuPS8) · [**Discord**](https://discord.gg/JK4hNKGprW) · [**Issues**](https://github.com/defoAI/mcp-ableton-music-maker/issues)

</div>

---

## What it is

Two pieces:

1. **`ableton-music-maker`**, a single Rust binary that speaks the [Model Context Protocol](https://modelcontextprotocol.io) over stdio to Claude Desktop, Claude Code or Cursor. It exposes 48 tools for reading and editing the Live set, and talks to Live over a TCP socket on port 9877.
2. **The AbletonMusicMaker Remote Script**, a control surface that runs inside Live and executes the commands. Live only loads control surfaces through its embedded Python interpreter, so this one file stays Python. It is embedded in the binary and installed with `ableton-music-maker-install-script`.

```
Claude ──stdio──▶ ableton-music-maker ──TCP 9877──▶ Ableton Live (AbletonMusicMaker Remote Script)
```

**Nothing leaves your machine.** The server has no upload path at all — no telemetry, no analytics, no dataset — and CI fails if one appears. What it keeps locally, and how to delete it, is in [Your data](#your-data).

## Quickstart (Docker)

Docker Desktop must be running whenever the MCP client starts the server.

**1. Build the image**

```bash
git clone https://github.com/defoAI/mcp-ableton-music-maker.git
cd mcp-ableton-music-maker
docker compose build
docker/verify-image.sh mcp-ableton-music-maker:local   # optional: the checks CI runs
```

**2. Install the Remote Script into Live's User Library**

```bash
docker compose --profile install run --rm install-script
# User Library somewhere else? Point at it:
ABLETON_USER_LIBRARY="/Volumes/Work/Ableton/User Library" docker compose --profile install run --rm install-script
```

On a Linux Docker engine (not Docker Desktop) add `ABLETON_INSTALL_USER="$(id -u):$(id -g)"` in front of the command so the container can write to your library.

Then restart Live, open **Settings → Link, Tempo & MIDI**, choose **AbletonMusicMaker** in a **Control Surface** slot, and set its Input and Output to **None**.

**3. Point your MCP client at the container**

<details open>
<summary><b>Claude Desktop</b> — Settings → Developer → Edit Config</summary>

```json
{
  "mcpServers": {
    "AbletonMusicMaker": {
      "command": "docker",
      "args": [
        "run", "--rm", "-i",
        "--read-only",
        "--security-opt", "no-new-privileges:true",
        "--cap-drop", "ALL",
        "-v", "ableton-music-maker-state:/state",
        "mcp-ableton-music-maker:local"
      ]
    }
  }
}
```
</details>

<details>
<summary><b>Claude Code</b></summary>

```bash
claude mcp add AbletonMusicMaker -- docker run --rm -i --read-only --security-opt no-new-privileges:true --cap-drop ALL -v ableton-music-maker-state:/state mcp-ableton-music-maker:local
```

Opening this repository in Claude Code also offers the server automatically through the project's [`.mcp.json`](.mcp.json).
</details>

<details>
<summary><b>Cursor</b> — Settings → MCP</summary>

Paste this as a command:

```
docker run --rm -i --read-only --security-opt no-new-privileges:true --cap-drop ALL -v ableton-music-maker-state:/state mcp-ableton-music-maker:local
```
</details>

Do not add `-t` (it breaks the stdio transport) or `-p` (the container listens on nothing). Run only one instance of the server at a time across all clients.

That's it — ask Claude to build something.

## Running the binary directly

If you would rather not use Docker, build the binaries with a Rust toolchain (1.85 or newer):

```bash
cargo install --path . --locked
ableton-music-maker-install-script                  # copies the Remote Script into Live's User Library
ableton-music-maker-install-script --list-targets   # preview the folders it would use
```

Then register `ableton-music-maker` as the command in your MCP client, for example `claude mcp add AbletonMusicMaker ableton-music-maker`. The server reads Live's address from `ABLETON_HOST` (default `localhost`) and `ABLETON_PORT` (default `9877`).

## What the image guarantees

Checked by `docker/verify-image.sh` and by CI on every build:

- distroless runtime: no shell, no package manager, runs as a non-root user
- read-only root filesystem; the only writable path is the `/state` volume
- the server binary carries no upload tier and reports `"uploads": "none"`; everything it writes sits under `/state`
- the binary completes the MCP handshake over stdio with nothing but JSON-RPC on stdout
- image size under 50 MB

## Tools

| Area | Tools |
|---|---|
| Session | `get_session_info`, `get_session_snapshot`, `set_tempo`, `start_playback`, `stop_playback` |
| Tracks | `get_track_info`, `create_midi_track`, `create_audio_track`, `set_track_name`, `set_track_mixer`, `set_send`, `get_returns`, `set_color` |
| Clips | `create_clip`, `create_audio_clip`, `get_clip_notes`, `add_notes_to_clip`, `clear_notes_from_clip`, `set_clip_name`, `delete_clip`, `fire_clip`, `stop_clip`, `get_clip_info`, `set_clip_loop`, `set_clip_launch`, `set_clip_automation`, `get_clip_automation` |
| Devices | `get_device_parameters`, `set_device_parameter`, `load_instrument_or_effect`, `load_drum_kit`, `get_drum_rack_pads` |
| Browser | `search_browser`, `get_browser_tree`, `get_browser_items_at_path` |
| Arrangement | `switch_to_arrangement_view`, `set_arrangement_time`, `get_arrangement_clips`, `duplicate_to_arrangement`, `set_arrangement_clip_name`, `delete_arrangement_clip`, `create_locator`, `delete_locator` |
| Listening | `get_track_meters`, `play_and_measure` |
| Orchestration | `batch`, `build_song` |
| Bridge | `get_remote_script_info` |

Notes can be written compactly: a step string per pitch (`{"36": "x...x...x...x..."}`), a repeating pattern, `notes_csv` lines, and `loop_every`/`until` to tile a bar across a clip; `create_clip` names and fills a clip in one call; `duplicate_to_arrangement` places a clip across a whole range at once.

**Upgrading from the original AbletonMCP?** Remove its `AbletonMCP` entry from your client config; both servers talk to the same Remote Script and Claude would see two overlapping tool sets. The Mac app offers to do this in Setup.

Every tool checks that the loaded Remote Script advertises the command it needs. A missing or outdated script produces a clear "run `ableton-music-maker-install-script`, then restart Live" error instead of a half-working session. The server also retries the handshake on the first tool call, so starting it before Live is fine.

### Example prompts

| Prompt | Demo |
|---|---|
| *"Create an 80s synthwave track"* | [Watch](https://youtu.be/VH9g66e42XA) |
| *"Create a Metro Boomin style hip-hop beat"* | |
| *"Create a full arrangement with an intro, buildup, drop, breakdown, and outro"* | |
| *"Add a jazz chord progression to the clip in track 1"* | |
| *"Load a 808 drum rack into the selected track"* | |
| *"Set the tempo to 120 BPM"* | |

## Troubleshooting

| Problem | Fix |
|---|---|
| Tools report the Remote Script cannot run a command | Run the installer again, restart Live, and re-select AbletonMusicMaker as a Control Surface. `get_remote_script_info` shows the loaded and expected versions. |
| "could not connect to Ableton" | Live is not running, or the AbletonMusicMaker control surface is not selected. From Docker, Live must be reachable at `host.docker.internal:9877`. |
| The client says the server failed to start | With Docker, Docker Desktop must be running before the client launches the server. |
| Timeout errors | Break the request into smaller steps. Importing large audio files is given 65 seconds; everything else 10 to 15. |

Diagnostics go to stderr. Set `RUST_LOG=debug` for the full command trace. `ableton-music-maker --check` asks Live for the loaded Remote Script and the current session and prints the answer as JSON; `--status` prints versions and paths.

## Your data

Nothing is uploaded, by the server or by the app; there is no code that could. What the server keeps on your machine is an **activity log** — one line per tool call with the tool's name, the Live commands it sent, timings, sizes and the result — under `~/.ableton-music-maker/activity/`. Parameters and results (your MIDI and names) are written only if you set `ABLETON_MCP_ACTIVITY_PAYLOADS=true`; `ABLETON_MCP_ACTIVITY=false` turns the log off. Delete the folder to delete the data. Details: [TERMS.md](TERMS.md).

## Mac app (developer preview)

`app/` holds a Tauri 2 menu bar app that installs the Remote Script into Live, connects Claude Desktop, Claude Code or Cursor to the bundled server, shows whether Claude, the server and Live are talking, and lists every call with its timing and an estimated token cost. It runs from source today:

```bash
cd app && npm install && npm run dev      # needs Rust 1.85+, Xcode command line tools
```

A signed, notarised download follows once the release pipeline exists.

## Documentation

What the product is, what it does today, the decisions behind it, and how a feature is
designed before it is built: [docs/](docs/README.md). Numbers in prose are copies; the
originals are listed in [docs/facts/source-of-truth.md](docs/facts/source-of-truth.md).

## Development

```bash
cargo test                      # unit, clip-notes, local-only, activity and end-to-end stdio tests
cargo clippy --all-targets      # lints
scripts/check-docs-facts.sh     # docs snapshot vs code (CI runs this)
docker build --target test .    # the same suite inside the image
```

The Remote Script lives in `AbletonMusicMaker_Remote_Script/__init__.py` and is embedded into the binary at build time; its `SCRIPT_VERSION` is the version the server expects. Edit the script, bump that constant when the command surface changes, and rebuild.

## Join the Community

Give feedback, get inspired, and build on top of the MCP: [**Discord**](https://discord.gg/JK4hNKGprW)

## Disclaimer

This is a third-party integration and not made by Ableton. Derived from AbletonMCP by [Siddharth Ahuja](https://x.com/sidahuj) (MIT); this fork is maintained by DefoAI UG.
