# Architecture overview

| | |
|---|---|
| **Verified** | 2026-09-19 against `src/`, `app/src-tauri/src/`, the Remote Script, `Dockerfile`, `docker/verify-image.sh` and `.github/workflows/ci.yml` |

How the system actually works. This is the one document with a hard freshness duty: it is
read while someone changes the code it describes, so **a change in `src/` or the Remote
Script updates this note in the same PR.** For what the product does see the
[feature-matrix](../technical/feature-matrix.md); for every number see
[source-of-truth](../facts/source-of-truth.md).

---

## Two processes, one socket

```
MCP client ──stdio (JSON-RPC)──▶ ableton-music-maker ──TCP 9877 (JSON)──▶ Ableton Live
(Claude Desktop, Claude Code,    Rust binary, runs anywhere          AbletonMusicMaker Remote Script,
 Cursor)                          that can reach the port             Python, runs inside Live
```

| Process | Where it must run | Why |
|---|---|---|
| **The Remote Script** — `AbletonMusicMaker_Remote_Script/__init__.py` | Inside Live, on the producer's machine | It is a Live control surface. Live loads control surfaces only through its embedded Python (2.7 on Live 10, 3.x on 11+), so this file stays Python with no f-strings, type hints or third-party imports. It opens a TCP server on port 9877 and executes JSON commands against the Live API on Live's main thread. |
| **The server** — `ableton-music-maker` | Anywhere that can reach that port: the same machine, or a container on it | Speaks MCP over **stdio only** to the client; there is no HTTP transport and no listening port. Talks to Live over one TCP socket. |

The installer, `ableton-music-maker-install-script`, is a second binary in the same crate.
The Remote Script is embedded into both with `include_str!`; the installer copies it into
Live's User Library under `Remote Scripts/AbletonMusicMaker/`, keeping a `.bak` of anything
it replaces (`src/install.rs`, Library.cfg discovery for the library path).

## Startup (`src/app.rs`)

1. Builds a `LiveState` around a `RealBridge` configured from `ABLETON_HOST` / `ABLETON_PORT`;
   the `LiveState` also owns the [activity log](#the-activity-log-srcactivityrs).
2. Runs the **handshake** on the blocking pool: `get_script_info` to Live. If Live is not
   reachable it logs a warning and continues — every tool retries the handshake on use, so
   starting the server before Live is fine.
3. Writes a **heartbeat** (`<state_dir>/sessions/<pid>.json`) so the Mac app can see the
   process; the MCP `initialize` request rewrites it with the client's name and version.
4. Serves MCP over stdio until the client disconnects, then removes the heartbeat and
   disconnects the socket.

`ableton-music-maker --status` prints versions, paths and the activity switches as JSON;
`--check` runs the handshake plus `get_session_info` against Live and exits 0 only when the
script is loaded and up to date. Both are for CI and the Mac app.

## A tool call, end to end (`src/tools.rs`)

```
#[tool] method ── binds a ToolSpec to a body ──▶ Server::run
   └─ spawn_blocking ▶ run_blocking(live, spec, params, body)
         ├─ require(live, command)   capability check against the cached script info
         ├─ body(&LiveState, &Params) -> Result<String, String>
         │     └─ live.send_command(cmd, params)  ──▶ AbletonConnection ──▶ Live
         │           (each command and its Live time are noted in a thread-local trace)
         └─ live.activity.record(...)   one JSON line, local only
   └─ Ok(text)  → CallToolResult::success
      Err(text) → CallToolResult::error          (never a JSON-RPC error)
```

- **Tool bodies are plain functions** so tests call them through `run()` with a
  `FakeBridge`. The `#[tool]` method does nothing but bind.
- **Compact note forms** (`src/notes.rs`) — csv lines, step strings, patterns and tiling —
  expand into plain note objects *before* any command is sent, so a bad pattern touches
  nothing and the Remote Script only ever sees `add_notes_to_clip` with a note array.
- **Multi-command bodies** (`create_clip` with a name and notes, `duplicate_to_arrangement`
  with many placements) send their commands in order and stop at the first failure; the
  activity line lists every command that went out.
- **`require(live, cmd)`** is the capability check: the command must be in the script's
  advertised `SCRIPT_CAPABILITIES`. A missing or outdated script produces one clear "run the
  installer, then restart Live" error instead of a half-working session. A unit test
  cross-checks `tools::ALL_REMOTE_COMMANDS` against the script's list.
- **The socket is synchronous.** Bodies run on the blocking pool; nothing calls the bridge
  from async code.

## The handshake and capabilities (`src/handshake.rs`)

The server only talks to the Remote Script it ships with. `expected_remote_script_version`
reads `SCRIPT_VERSION` out of the embedded source at startup — the constant in the script is
the single source of truth. `ScriptInfoCache` holds the last handshake result (version,
protocol version, capabilities, up-to-date flag) and is refreshed when a tool finds it empty.

Adding a Remote Script command therefore means: handler in the script → name in
`SCRIPT_CAPABILITIES` → bump `SCRIPT_VERSION` → add to `ALL_REMOTE_COMMANDS` → tool body.

## The connection (`src/connection.rs`)

`AbletonConnection` owns one `TcpStream` behind a mutex. A request and its response are one
indivisible exchange, so the whole round-trip — including reconnect — runs under the lock;
tool calls and the passive poller share the socket. `RealBridge` wraps it with reconnection.
Timeouts are per command (`command_timeout`): importing audio can hold Live's main thread
far longer than anything else, modifying commands get a wider budget than reads, and the
connect itself has its own. `LiveError` is the error type every body sees.

`LiveBridge` is the trait; tests substitute a `FakeBridge` (`tests/common/`).

## The activity log (`src/activity.rs`)

One JSON line per tool call, appended to `<state_dir>/activity/<session-id>.jsonl` where
the session id is the start time plus the pid: `ts`, `tool`, `commands` (what was sent to
Live), `ok`, `error`, `duration_ms`, `live_ms`, `in_chars`, `out_chars`. `params` and
`result` are added only under `ABLETON_MCP_ACTIVITY_PAYLOADS=true`; `ABLETON_MCP_ACTIVITY=false`
turns the file off. A write failure is logged to stderr once and never fails the tool.
`tests/activity.rs` pins the defaults.

## Where the server writes (`src/state.rs`)

`ABLETON_MCP_STATE_DIR`, else `~/.ableton-music-maker/`, with `activity/` and `sessions/`
under it. Nothing else. There is no upload path: no HTTP client in the dependency tree (CI
fails if one appears), no telemetry, no dataset — `tests/local_only.rs` is the policy and
[decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md) the reason.

## The Mac app (`app/`)

A Tauri 2 menu bar app whose Rust core depends on this crate by path and calls `install`,
`handshake`, `connection`, `state` and `app::check` as functions. It never runs the server
Claude talks to: it carries the server binary as a sidecar at `Contents/MacOS/ableton-music-maker`,
writes that path into Claude Desktop's config (backup first) or shows the command for Claude
Code and Cursor, and derives everything it shows from files the server writes:

| Screen | Reads |
|---|---|
| Overview: the chain client → server → Live | live heartbeats (a dead pid is deleted on sight) and one `check` against Live every 10 s while open |
| Activity | the session's `.jsonl`, re-read every 2 s; tokens are `ceil(chars / 4)`, labelled *est.* |
| Setup | Library.cfg discovery for the script path and its `SCRIPT_VERSION`; `check`; the client config |
| Settings | its own `settings.json`; the switches that concern the server are written into the client config's `env` block, because only the environment reaches a client-started server |

The menu bar item shows the same three states and the last call, refreshed in the background
while the window is closed.

## The Docker image (`Dockerfile`, `docker/verify-image.sh`)

Three stages: `builder` (release build, dependency layer cached separately), `test`
(`cargo test` inside the image with both disable variables set), `runtime`
(`distroless/cc-debian12:nonroot` plus the two binaries). The runtime image bakes in
`ABLETON_HOST=host.docker.internal`, both disable variables, and points every writable path
(`ABLETON_MCP_STATE_DIR`, `ABLETON_MCP_DATA_DIR`, `HOME`) at `/state`, the only volume, so the
root filesystem runs read-only. No `HEALTHCHECK` and no exposed port: the server speaks stdio
and only makes an outbound connection.

The contract is checked, not assumed: `verify-image.sh` asserts no shell, non-root, both
disable variables, every privacy gate off with no credentials, a working `initialize`
handshake over stdio with nothing but JSON-RPC on stdout, the size budget, and a working
installer binary. Changing the Dockerfile means re-running it.

What cannot be containerised: the Remote Script. `docker compose --profile install` runs the
installer with the User Library bind-mounted; Live still has to be restarted and the control
surface selected by hand.

## CI (`.github/workflows/ci.yml`)

`rust` job: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test`, and
`scripts/check-docs-facts.sh`. `image` job: build the test stage, build and load the runtime
stage, `verify-image.sh`, trivy on CRITICAL, and on `main` push to
`ghcr.io/defoAI/mcp-ableton-music-maker` for amd64 and arm64.

## Boundaries worth knowing

- **One server instance at a time**, across all clients — the Remote Script serves one
  socket and the server holds one connection. The app warns when two heartbeats are alive.
- **Live's main thread is the bottleneck.** Every command runs there; the timeouts exist
  because a big audio import blocks everything else.
- **The bind address is an open decision** —
  [0003](../decisions/0003-remote-script-bind-address.md).
- **Arrangement commands are Live 11+**; the script has Python 2 branches for Live 10 but no
  test matrix across versions.
