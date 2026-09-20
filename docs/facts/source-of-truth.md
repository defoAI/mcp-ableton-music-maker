# Source of truth

**Every number in this repository's prose is a copy. This file says where the original lives.**

Documentation goes stale silently — a confident sentence about the tool count keeps reading as
confident long after a tool is added, and nothing fails when it does. This file, and the check
script that reads its snapshot, is how that gets caught.

---

## The rule

**A document may not be the source of a fact it does not own.**

When a document needs a count, a version, a port, a timeout or a default, it does one of two
things:

1. **Links here**, and states the fact as "see [source-of-truth](../facts/source-of-truth.md)"; or
2. **States the fact and stamps it** — `37 tools (verified 2026-09-19 against src/tools.rs)`.

A bare number with no stamp and no link is a bug. When you find one, either verify and stamp
it or delete it.

**Before writing any count, version, port, default or path into any document, README,
listing or answer — read the file in the table below.** Not this file. Not memory. The file.

---

## Where the originals live

Paths are relative to the repository root.

### The product surface

| Fact | Authoritative source | How to check |
|---|---|---|
| How many MCP tools there are | `src/tools.rs` — one `#[tool(name = …)]` per tool; the unit test `tool_count_and_schema_defaults` pins the count | `grep -c '#\[tool(name = ' src/tools.rs` |
| Which tools are the artist's surface and which are advanced | `CORE_TOOLS` in `src/tools.rs` (decision 0006); every other tool is served as `adv_<name>`; `tests/artist.rs` pins it | `sed -n '/^pub const CORE_TOOLS/,/^\];/p' src/tools.rs` |
| Which tools exist and what they do | The `#[tool]` doc comments in `src/tools.rs` — they *are* the descriptions the client shows | `cargo test` lists them; the README table is a copy |
| Which Remote Script commands exist | `tools::ALL_REMOTE_COMMANDS` in `src/tools.rs` — the one place the list is written. The script declares nothing by hand: it reads its own dispatch back at import (`_served_commands`), and the test `the_servers_command_list_and_the_scripts_dispatch_are_the_same_set` fails the build if a name here has no handler, or a handler has no name here | `sed -n '/^pub const ALL_REMOTE_COMMANDS/,/^\];/p' src/tools.rs` |
| The Remote Script version the server expects | `SCRIPT_VERSION` in the same file — the binary reads it out of the embedded source at startup (`handshake::expected_remote_script_version`) | `grep '^SCRIPT_VERSION' AbletonMusicMaker_Remote_Script/__init__.py` |
| The wire protocol version | `PROTOCOL_VERSION` in the same file | |
| The server version | `version` in `Cargo.toml`, surfaced as `MCP_VERSION` and in `--privacy-status` | `grep '^version' Cargo.toml` |
| Minimum Rust | `rust-version` in `Cargo.toml` | |
| Which MCP transport is served | `app::serve_stdio` — stdio only, no HTTP, no port | |
| Which Live versions work | The Remote Script's own branches: Python 2 fallbacks for Live 10; `track.arrangement_clips` and the arrangement commands are Live 11+ (comments in the script near `get_arrangement_clips` and `duplicate_session_clip_to_arrangement`). There is no test matrix across Live versions — say "Live 11 and 12; Live 10 without the arrangement tools", not "every version" | `grep -n 'Live 1[012]' AbletonMusicMaker_Remote_Script/__init__.py` |

### The connection

| Fact | Authoritative source | How to check |
|---|---|---|
| Port | `DEFAULT_PORT` in the Remote Script; `ABLETON_PORT` on the server side (`RealBridge::from_env`) | |
| Bind address | `DEFAULT_HOST` in the Remote Script (`127.0.0.1`), overridden by the first non-comment line of `bind_host.txt` beside it; reported as `bind_host` in `get_script_info` — [decision 0003](../decisions/0003-remote-script-bind-address.md) | `grep -n '^DEFAULT_HOST' AbletonMusicMaker_Remote_Script/__init__.py`; `ableton-music-maker --check` prints what the loaded script bound |
| Per-command socket timeouts | `command_timeout` in `src/connection.rs` and the `MODIFYING_COMMANDS` list beside it | The README's troubleshooting row copies these |
| Connect timeout | `TcpStream::connect_timeout` in `src/connection.rs` | |
| Where Live is, from inside Docker | `ENV ABLETON_HOST=host.docker.internal` in the `Dockerfile` | |
| What the Mac app hears | `app/src-tauri/src/listen/tap.m`: a Core Audio process tap of the process whose executable is inside an `Ableton Live*.app` bundle, unmuted; in memory only — [decision 0008](../decisions/0008-app-hears-live-through-a-process-tap.md) | the no-I/O test in `app/src-tauri/src/listen/mod.rs`; `cargo run --example listen_probe` in `app/src-tauri` with Live playing |

### Local data

| Fact | Authoritative source | How to check |
|---|---|---|
| The server has no upload path | `tests/local_only.rs` (no removed variable or the word Supabase in `src/`; no dataset tool served) plus the CI step "No HTTP client in the dependency tree" | `cargo tree -e normal \| grep -E 'ureq\|reqwest\|hyper\|curl'` prints nothing |
| What the activity log writes, and that payloads are off by default | `src/activity.rs`; `tests/activity.rs` pins the defaults | `cargo test --test activity` |
| Set exports are written only on an explicit call, and where | `src/sets.rs`; `tests/sets.rs` pins the folder and that no other tool creates it | `cargo test --test sets` |
| Every environment variable the server reads | the code | `grep -rhoE 'ABLETON_MCP_[A-Z_]+' src \| sort -u` |
| Where the server writes | `src/state.rs` — `ABLETON_MCP_STATE_DIR`, else `~/.ableton-music-maker/` with `activity/`, `sessions/`, `library/` and (only after an `export_set` call) `sets/` under it | `ableton-music-maker --status` |
| What the heartbeat contains | `app::write_heartbeat` in `src/app.rs` | one `<pid>.json` per running server |
| Plain-language description of the above, and how to delete it | `TERMS.md` — must match the code facts in this table | |
| Who publishes the product | DefoAI UG — [decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md); nobody holds data because none leaves the machine | |

### The image and CI

| Fact | Authoritative source | How to check |
|---|---|---|
| What the runtime image guarantees | `docker/verify-image.sh` — each check is one promise: no shell, non-root, `--status` says `uploads: none` and `/state`, the binary carries no upload tier, stdout is pure JSON-RPC, size budget, installer present | `docker/verify-image.sh mcp-ableton-music-maker:local` |
| Image size limit | `MAX_SIZE_MB` default in `docker/verify-image.sh` | |
| Base image and user | `FROM gcr.io/distroless/cc-debian12:nonroot` in the `Dockerfile` | |
| Whether the image is published | It is not — CI builds only the Mac app ([decision 0009](../decisions/0009-ci-builds-the-mac-app-only.md)). `docker compose build` produces `mcp-ableton-music-maker:local` where it is used | `grep -c ubuntu .github/workflows/*.yml` is 0 |
| What CI runs | `.github/workflows/ci.yml`: fmt, clippy `-D warnings`, `cargo test`, the dependency-tree gate, the docs drift check, image test stage, verify-image, trivy on CRITICAL, and the `mac-app` job — `tauri build` then `verify-dmg.sh` then the `.dmg` as an artifact | |
| What the Mac disk image guarantees | `app/scripts/verify-dmg.sh` — each check is one promise: it mounts, carries the app and the `/Applications` shortcut, both binaries arm64 and validly signed, the sidecar's `--status` runs out of the mounted image, the Info.plist keys the app needs. `EXPECT_SIGNED=1` adds Developer ID, a stapled ticket and Gatekeeper | `app/scripts/verify-dmg.sh <dmg>` |
| Which Macs the app is built for | the `--target` in the `mac-app` job and in `release.yml` (`aarch64-apple-darwin` — Apple Silicon), and `minimumSystemVersion` in `app/src-tauri/tauri.conf.json` | `lipo -archs` on the binaries inside the bundle |
| How the app is signed and published | `.github/workflows/release.yml` — a `v*` tag, six `APPLE_*` repository secrets read by Tauri itself, attached to the GitHub release. No certificate or key is in the repository, and the workflow stops before building when a secret is missing | `gh secret list` |
| Where the Remote Script is installed | `src/install.rs` — Live's User Library `Remote Scripts/AbletonMusicMaker/`, with a `.bak` of anything replaced | `ableton-music-maker-install-script --list-targets` |

### Community and lineage

| Fact | Authoritative source |
|---|---|
| Discord invite, setup video, demo videos | `README.md` header — links, no member counts |
| Upstream project and licence holder | `LICENSE` — MIT, copyright Siddharth Ahuja 2025; the fork point is commit `8731a47` (`ahujasid/ableton-mcp` v1.4.0) |
| Directory listing configuration | `smithery.yaml` |

---

## Verified snapshot — 2026-09-20

Read the sources above rather than trusting this block. It is dated so a reader can see at a
glance whether it has gone stale, and `scripts/check-docs-facts.sh` fails CI when the bold
values below stop matching the code. Keep the row labels exactly as they are — the script
matches on them.

| | Verified value | Source |
|---|---|---|
| MCP tools | **104** | `src/tools.rs` |
| Remote Script commands | **97** | `ALL_REMOTE_COMMANDS` |
| Remote Script version | **1.33.0** | `SCRIPT_VERSION` |
| Server version | **2.0.0** | `Cargo.toml` |
| Image size limit | **50** MB | `docker/verify-image.sh` |
| Port | **9877** | `DEFAULT_PORT` |
| Timeouts | 190 s `create_tracks` · 65 s `create_audio_clip` and `place_sample` and `write_clips` · 25 s `search_browser` · 15 s modifying commands (`delete_device` and `move_device` among them) · 10 s reads · 5 s connect | `src/connection.rs` |
| Uploads | **none** — no code path exists | `tests/local_only.rs`, CI dependency gate |
| Activity log | on by default, payloads off; `ABLETON_MCP_ACTIVITY=false` / `ABLETON_MCP_ACTIVITY_PAYLOADS=true` | `tests/activity.rs` |
| Transport | stdio only | `src/app.rs` |
| Bind address | **127.0.0.1** by default; `bind_host.txt` overrides | `DEFAULT_HOST` |
| Live versions | 11 and 12 fully; 10 without the arrangement tools; untested beyond the script's own branches | the Remote Script |

### Surfaces known to carry stale facts today

| Surface | Problem | Tracking |
|---|---|---|
| The Remote Script's class and log lines | Still say `AbletonMCP`; the control surface Live shows is `AbletonMusicMaker` | vocabulary, see [brand-and-claims](../marketing/brand-and-claims.md) |

---

*When you correct a fact anywhere in this repository, update the snapshot above and re-date it.*
