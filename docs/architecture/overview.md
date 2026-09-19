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
- **Orchestration bodies** (`batch`, `build_song`, `play_and_measure`) call other bodies
  through `run_named` (a name → params → body table) or directly; they never bypass
  `require`, and the activity line for the one tool call lists every command that went out.
- **Capture** (`capture_mix`) is the only path out of Live: the script records the master
  through a Resampling track as a fixed-length Session clip (fired on the tick after the
  playhead lands, the same two-phase pattern as locators), the server polls until the clip
  has a file, stops the transport (a `Drop` guard stops it on any early exit), and
  `src/audio.rs` reads the WAV or AIFF Live wrote and measures it. Nothing is copied.
- **Performance** (`start_performance`, `cue`, `get_performance_state`, …) is the one place
  the script acts on its own clock. Claude plans, Live executes: `cue` resolves bar numbers to
  beats and names to indices against a fresh `get_performance_state` (`src/performance.rs`,
  pure logic, unit-tested), refuses a step in the past or a plan that leaves a bar silent, and
  hands the steps to `schedule_cue`. The script stores them and re-arms `schedule_message`
  every tick while work remains: launch steps are issued inside the bar before their target so
  Live's global quantization places them on the bar, sets and ramp steps land within a tick of
  their beat, and every step fired is queued as an event the next state read reports. A stopped
  transport or a time-signature change cancels pending cues. While a performance runs
  (`LiveState::performance`, in memory only) the transport-touching tools refuse with the
  on-the-bar alternative in the message.
- **The clock rides on the envelope.** While performance mode is on, the Remote Script
  attaches `clock` (bar, beat, seconds to the next bar, phrase, next cue) to every response
  it sends; `AbletonConnection::exchange` keeps it in the per-call trace and `run_blocking`
  renders it as the last line of every tool result. Zero round trips. Launch commands read
  the transport *after* they fire and report the bar the launch lands on.
- **Sections and songs** (`src/song.rs`, `src/sections.rs`) put a vocabulary on the performance
  layer without a state machine in the script. A section is a scene row named
  `<name> · <bars>`; the script parses the names into its phrase table on every state read and
  on every clock, so a Live restart loses nothing. The song is the setlist written into the
  name of the `Setlist:` scene. `play_song` fires the first entry and hands every counted jump
  to the script as one cue; each steering verb (`go`, `next_section`, `previous_section`,
  `back`, `jump_to`) is one state read and one `schedule_cue` whose `replaces` field retires
  the old plan in the same round trip, and `hold_section` is a `cancel_cue`. The server keeps
  only the cursor (which entry, the plan cue id, the jump history) in the performance record
  and re-derives the position from the row that plays. A move lands at the end of the playing
  section's phrase (`phrase.ends_bar`, counted from the bar the row was fired on) unless told
  `next_bar`, and then the reply says what it cuts. `make_section` is Live's own
  capture-and-insert-scene (`capture_scene`, which launches the copy and carries the phrase
  count over), `duplicate_scene` plus per-track rewrites, or `create_scene` plus clips.
  Transitions (`src/transition.rs`) are composed on the server into the cue primitives above
  (ramps, sets, fires, stops) and note rewrites written into the target row before the jump.
- **Levels ride with the clock.** The script's tick keeps the master and per-track meter peaks
  for the current and last bar and the master's peak per scene row; they travel as `levels` on
  the `clock` envelope and the state, `run_blocking` renders the 🔊 line under the ⏱ line, and
  a jump into a row that ran more than 3 dB hotter warns before the mix clips.
- **The sound vocabulary** (`src/sound.rs`) is a table, not a guess: a word is resolved at call
  time against the device's rack macros by name, then candidate parameter names per Live
  instrument, then aliases and the word itself; `shape_sound` writes several words through one
  `set_device_parameters`, and a cue ramp takes a word the same way.
- **Set memory is on request.** `export_set` (`src/sets.rs`) reads one session snapshot and one
  context into a rebuildable document under `state_dir()/sets/` and nothing else creates that
  folder; `import_set` rebuilds through `build_song`, `set_scale` and `set_song`. The Live set
  (its scene names) stays the memory.
- **The surface is one list, prefixed** (decision 0006). `Server::tool_router` builds every
  tool, then re-keys each one outside `CORE_TOOLS` as `adv_<name>` and sets the MCP
  annotations from the name; `run_named` strips the prefix, so `batch` takes either spelling. Nothing is gated: the client's user chooses. The
  artist tools in `src/arrange.rs` compose the raw bodies (`feel` runs the note rewrites in
  order and keeps one undo; `arrange` converts bars to beats once and sends one
  `place_clips` / `delete_arrangement_clips` / `duplicate_arrangement_clip` per track, the
  script looping on Live's main thread so a 200-clip change is one round trip). Faders in dB
  are the script's doing: it bisects Live's own fader curve with `str_for_value`, so the
  number the artist reads in Live is the number they said.
- **The library index.** `src/library.rs` pages the script's browser walk in the background
  after the handshake (one-second pages so tool calls interleave on the shared socket),
  keeps it under `state_dir()/library/` and answers `search_browser` and every internal
  lookup locally once complete.
- **Orientation and instructions.** `get_context` is one Remote Script round trip that returns
  the set, every track, the returns, the scenes and the performance clock; `src/context.rs`
  renders it and holds the MCP `instructions` string the server sends at `initialize` (the
  `get_info` override in `tools.rs`), so a client's model knows the workflow before its first
  call.
- **Whole sections in one round trip.** `create_tracks` and `write_clips` take the document
  `build_song` (and `make_section`) validated on the server and do every create, name,
  instrument load, fader and note write in one main-thread task; a copy in another row is
  made inside Live from the first clip (`copy_of`), so the notes cross the socket once. With
  `place_clips` and `delete_arrangement_clips` this is the answer to the 200 ms floor below:
  the count of round trips, not the size of any one, is what a producer waits for.
- **A round trip costs about 200 ms, whatever it does.** Measured on Live 12.4.6 over one
  persistent socket: an unknown command, a tiny read and `get_context` all answer in the same
  200 ms, with or without the per-command log line and with `TCP_NODELAY` on both ends. The
  floor is Live scheduling the script's socket thread, not the command. Design consequence:
  fewer round trips beat smaller payloads; `get_context` is one call where three used to be,
  and `batch` / `build_song` exist for the same reason.
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

`ABLETON_MCP_STATE_DIR`, else `~/.ableton-music-maker/`, with `activity/`, `sessions/`,
`library/` and, only after an explicit `export_set`, `sets/` under it. Nothing else. There is no upload path: no HTTP client in the dependency tree (CI
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
| Delete all local data | removes the activity and session files, the library index and the set exports |
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
