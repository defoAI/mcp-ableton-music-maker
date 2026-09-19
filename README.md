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

1. **`ableton-music-maker`**, a single Rust binary that speaks the [Model Context Protocol](https://modelcontextprotocol.io) over stdio to Claude Desktop, Claude Code or Cursor. It exposes 37 tools for reading and editing the Live set, and talks to Live over a TCP socket on port 9877.
2. **The AbletonMusicMaker Remote Script**, a control surface that runs inside Live and executes the commands. Live only loads control surfaces through its embedded Python interpreter, so this one file stays Python. It is embedded in the binary and installed with `ableton-music-maker-install-script`.

```
Claude ──stdio──▶ ableton-music-maker ──TCP 9877──▶ Ableton Live (AbletonMusicMaker Remote Script)
```

**All telemetry is off by default.** Nothing leaves your machine except the connection to Live unless you explicitly opt in. See [Telemetry](#telemetry).

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
- telemetry and dataset recording are hard-off via environment variables baked into the image, and off by default in the binary as well
- no Supabase credentials in the image
- the binary completes the MCP handshake over stdio with nothing but JSON-RPC on stdout
- image size under 50 MB

## Tools

| Area | Tools |
|---|---|
| Session | `get_session_info`, `get_session_snapshot`, `set_tempo`, `start_playback`, `stop_playback` |
| Tracks | `get_track_info`, `create_midi_track`, `create_audio_track`, `set_track_name` |
| Clips | `create_clip`, `create_audio_clip`, `get_clip_notes`, `add_notes_to_clip`, `clear_notes_from_clip`, `set_clip_name`, `delete_clip`, `fire_clip`, `stop_clip` |
| Devices | `get_device_parameters`, `set_device_parameter`, `load_instrument_or_effect`, `load_drum_kit` |
| Browser | `get_browser_tree`, `get_browser_items_at_path` |
| Arrangement | `switch_to_arrangement_view`, `set_arrangement_time`, `get_arrangement_clips`, `duplicate_to_arrangement`, `set_arrangement_clip_name`, `create_locator` |
| Bridge | `get_remote_script_info` |
| Dataset (opt-in) | `set_dataset_consent`, `submit_intent`, `rate_last_action`, `prefer_candidate`, `reject_last_action`, `record_audition` |

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

Diagnostics go to stderr. Set `RUST_LOG=debug` for the full command trace.

## Telemetry

**All telemetry is off by default.** There are two tiers, both opt-in, and both require Supabase credentials in the environment (`ABLETON_MCP_SUPABASE_URL`, `ABLETON_MCP_SUPABASE_ANON_KEY`) that are not shipped with the binary or the image. See the [Terms & Data Use](TERMS.md) for exactly what each tier collects.

| Tier | What it collects | Off by default | Turn on |
|---|---|---|---|
| Anonymous telemetry | Tool names, success/duration, versions, install ID | Yes | `ABLETON_MCP_ENABLE_TELEMETRY=true` |
| Dataset recording | Prompts, MIDI notes, track and clip names, device settings | Yes | Telemetry on **and** your explicit yes to the consent question, or `ABLETON_MCP_ENABLE_DATASET=true` |

With telemetry off the dataset consent question is never asked. With telemetry on, you are asked once (as a dialog if your client supports it, otherwise in the chat), and an unanswered or dismissed question means recording stays off.

The disable variables override any opt-in, stored answer or enable variable, and are what the Docker image sets: `ABLETON_MCP_DISABLE_TELEMETRY=true` (also `DISABLE_TELEMETRY`, `MCP_DISABLE_TELEMETRY`) and `ABLETON_MCP_DISABLE_DATASET=true`.

`ableton-music-maker --privacy-status` prints the state of every gate as JSON.

## Development

```bash
cargo test                      # unit, tool, privacy and end-to-end stdio tests
cargo clippy --all-targets      # lints
docker build --target test .    # the same suite inside the image
```

The Remote Script lives in `AbletonMusicMaker_Remote_Script/__init__.py` and is embedded into the binary at build time; its `SCRIPT_VERSION` is the version the server expects. Edit the script, bump that constant when the command surface changes, and rebuild.

## Join the Community

Give feedback, get inspired, and build on top of the MCP: [**Discord**](https://discord.gg/JK4hNKGprW)

## Disclaimer

This is a third-party integration and not made by Ableton. Made by [Siddharth](https://x.com/sidahuj).
