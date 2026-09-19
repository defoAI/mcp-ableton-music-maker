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
| Which tools exist and what they do | The `#[tool]` doc comments in `src/tools.rs` — they *are* the descriptions the client shows | `cargo test` lists them; the README table is a copy |
| Which Remote Script commands exist | `SCRIPT_CAPABILITIES` in `AbletonMusicMaker_Remote_Script/__init__.py`; `tools::ALL_REMOTE_COMMANDS` is the server's cross-check and a test fails if the two disagree | `grep -A40 'SCRIPT_CAPABILITIES = \[' AbletonMusicMaker_Remote_Script/__init__.py` |
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
| Bind address | `HOST` in the Remote Script — currently `0.0.0.0`, which is [decision 0003](../decisions/0003-remote-script-bind-address.md), still open | |
| Per-command socket timeouts | `command_timeout` in `src/connection.rs` and the `MODIFYING_COMMANDS` list beside it | The README's troubleshooting row copies these |
| Connect timeout | `TcpStream::connect_timeout` in `src/connection.rs` | |
| Where Live is, from inside Docker | `ENV ABLETON_HOST=host.docker.internal` in the `Dockerfile` | |

### Local data

| Fact | Authoritative source | How to check |
|---|---|---|
| The server has no upload path | `tests/local_only.rs` (no removed variable or the word Supabase in `src/`; no dataset tool served) plus the CI step "No HTTP client in the dependency tree" | `cargo tree -e normal \| grep -E 'ureq\|reqwest\|hyper\|curl'` prints nothing |
| What the activity log writes, and that payloads are off by default | `src/activity.rs`; `tests/activity.rs` pins the defaults | `cargo test --test activity` |
| Every environment variable the server reads | the code | `grep -rhoE 'ABLETON_MCP_[A-Z_]+' src \| sort -u` |
| Where the server writes | `src/state.rs` — `ABLETON_MCP_STATE_DIR`, else `~/.ableton-music-maker/` with `activity/` and `sessions/` under it | `ableton-music-maker --status` |
| What the heartbeat contains | `app::write_heartbeat` in `src/app.rs` | one `<pid>.json` per running server |
| Plain-language description of the above, and how to delete it | `TERMS.md` — must match the code facts in this table | |
| Who publishes the product | DefoAI UG — [decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md); nobody holds data because none leaves the machine | |

### The image and CI

| Fact | Authoritative source | How to check |
|---|---|---|
| What the runtime image guarantees | `docker/verify-image.sh` — each check is one promise: no shell, non-root, `--status` says `uploads: none` and `/state`, the binary carries no upload tier, stdout is pure JSON-RPC, size budget, installer present | `docker/verify-image.sh mcp-ableton-music-maker:local` |
| Image size limit | `MAX_SIZE_MB` default in `docker/verify-image.sh` | |
| Base image and user | `FROM gcr.io/distroless/cc-debian12:nonroot` in the `Dockerfile` | |
| Where the published image lives | `.github/workflows/ci.yml` — `ghcr.io/defoAI/mcp-ableton-music-maker:<sha>` and `:latest`, pushed on `main` only, linux/amd64 + linux/arm64 | |
| What CI runs | `.github/workflows/ci.yml`: fmt, clippy `-D warnings`, `cargo test`, the dependency-tree gate, the docs drift check, image test stage, verify-image, trivy on CRITICAL | |
| Where the Remote Script is installed | `src/install.rs` — Live's User Library `Remote Scripts/AbletonMusicMaker/`, with a `.bak` of anything replaced | `ableton-music-maker-install-script --list-targets` |

### Community and lineage

| Fact | Authoritative source |
|---|---|
| Discord invite, setup video, demo videos | `README.md` header — links, no member counts |
| Upstream project and licence holder | `LICENSE` — MIT, copyright Siddharth Ahuja 2025; the fork point is commit `8731a47` (`ahujasid/ableton-mcp` v1.4.0) |
| Directory listing configuration | `smithery.yaml` |

---

## Verified snapshot — 2026-09-19

Read the sources above rather than trusting this block. It is dated so a reader can see at a
glance whether it has gone stale, and `scripts/check-docs-facts.sh` fails CI when the bold
values below stop matching the code. Keep the row labels exactly as they are — the script
matches on them.

| | Verified value | Source |
|---|---|---|
| MCP tools | **49** | `src/tools.rs` |
| Remote Script commands | **48** | `SCRIPT_CAPABILITIES` |
| Remote Script version | **1.10.1** | `SCRIPT_VERSION` |
| Server version | **2.0.0** | `Cargo.toml` |
| Image size limit | **50** MB | `docker/verify-image.sh` |
| Port | **9877** | `DEFAULT_PORT` |
| Timeouts | 65 s `create_audio_clip` · 25 s `search_browser` · 15 s modifying commands · 10 s reads · 5 s connect | `src/connection.rs` |
| Uploads | **none** — no code path exists | `tests/local_only.rs`, CI dependency gate |
| Activity log | on by default, payloads off; `ABLETON_MCP_ACTIVITY=false` / `ABLETON_MCP_ACTIVITY_PAYLOADS=true` | `tests/activity.rs` |
| Transport | stdio only | `src/app.rs` |
| Live versions | 11 and 12 fully; 10 without the arrangement tools; untested beyond the script's own branches | the Remote Script |

### Surfaces known to carry stale facts today

| Surface | Problem | Tracking |
|---|---|---|
| The Remote Script's class and log lines | Still say `AbletonMCP`; the control surface Live shows is `AbletonMusicMaker` | vocabulary, see [brand-and-claims](../marketing/brand-and-claims.md) |

---

*When you correct a fact anywhere in this repository, update the snapshot above and re-date it.*
