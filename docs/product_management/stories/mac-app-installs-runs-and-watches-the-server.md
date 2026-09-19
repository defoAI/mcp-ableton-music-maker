# Mac app installs, runs and watches the server

## Story
**As a** producer on a Mac who wants Claude inside my Live set,
**I want** one app that puts the Remote Script into Live, connects my client, shows me at a glance whether Claude, the server and Live are talking, and lets me see every call and what it cost,
**So that** I never open a terminal, never install Docker, and when something breaks I know which half broke and what to click.

## Details
| Field | Value |
|-------|-------|
| Status | `In Progress` |
| Surface | [Decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md): the app adds no tool; its activity view sees every tool name, the `adv_` ones included, and "Delete all local data" removes the set exports and the library index too |
| Priority | P1 — the first thing a producer sees; today's first thing is Docker Desktop |
| Size | L — split into the three phases in Implementation Notes; each ships on its own |
| Tracker | — |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Decisions | [0004](../../decisions/0004-who-publishes-and-holds-the-data.md) local data only · [0005](../../decisions/0005-mac-app-is-tauri-and-bundles-the-server.md) Tauri 2, bundled server |
| Prototype | [prototypes/mac-app-installs-runs-and-watches-the-server.html](../prototypes/mac-app-installs-runs-and-watches-the-server.html) · [review link](https://claude.ai/artifact/D2fY7k4dPf8DqEHtYc6QGh) |

## Context

### The Problem
Installing today means Docker Desktop, `docker compose build`, a compose profile to install
the Remote Script, a hand-edited `claude_desktop_config.json`, and a restart of Live and of
the client — with nothing on screen that says whether any of it worked
(`README.md` "Quickstart (Docker)"). When a tool fails, the producer sees one error line in
the chat and cannot tell whether Live is closed, the control surface is not selected, the
script is outdated, or the client never started the server. Nothing shows what Claude did
to the set, how long Live took, or how much each call adds to the conversation.

### Current State
- The server is started by the client over stdio and exits when the client disconnects
  (`src/app.rs` `serve_stdio`); nothing else can see it run. Diagnostics go to stderr only.
- The crate already holds every capability the app needs as library code: Library.cfg
  discovery and install with `.bak` (`src/install.rs`), the handshake and capability cache
  (`src/handshake.rs`), the synchronous connection with per-command timeouts
  (`src/connection.rs` `command_timeout`), and the version constants (`src/lib.rs`
  `MCP_VERSION`, `handshake::expected_remote_script_version`).
- Every tool call passes through one place, `run_blocking` inside `Server::run`
  (`src/tools.rs`), which knows the tool, its command, the duration, the outcome and the
  sizes of what went in and out.
- The state directory is `ABLETON_MCP_STATE_DIR`, else `~/.ableton-music-maker/`
  (`src/dataset/consent.rs` `state_dir` today; it survives the removal story as the crate's
  one writable location).
- A bundled sidecar in a Tauri 2 app lands at
  `/Applications/<App>.app/Contents/MacOS/<name>`, a stable path a client config can point at.

### Root Cause
The product was built as a developer tool: the install is a build, and the process is
invisible by design of the transport.

## Open Questions
1. **Does the app bundle the native binary and become the Mac install path?** → Yes
   ([0005](../../decisions/0005-mac-app-is-tauri-and-bundles-the-server.md)). Docker stays
   documented for CI and hardened setups, after the app in the README.
2. **Tauri 2 or SwiftUI?** → Tauri 2; the core links the crate directly.
3. **Telemetry?** → Removed entirely
   ([0004](../../decisions/0004-who-publishes-and-holds-the-data.md)). The app has no
   privacy toggles for upload tiers because there are none; it shows what is stored locally
   and where.
4. **Is an activity log on by default consistent with the product's posture?** → Yes, as
   *local data only*: tool names, timings, sizes and results are kept on the Mac by default;
   parameters and results (which contain MIDI and names) are kept only when the producer
   turns "Keep the payloads too" on. Nothing is uploaded by anything.
5. **May the app edit `claude_desktop_config.json`?** → Yes, after showing the exact entry
   and keeping a timestamped backup beside the file. Assumed from the prototype review;
   the Setup step makes the write explicit and reversible.
6. **What are "tokens"?** → An estimate from payload sizes, about four characters per
   token, labelled *est.* everywhere. The server cannot see the model's context. Per call
   and per session only; a per-day view is out of scope.
7. **Who signs the app?** → DefoAI UG's Developer ID
   ([0005](../../decisions/0005-mac-app-is-tauri-and-bundles-the-server.md)); certificate
   and notary key are CI secrets. **Assumed from repository ownership — confirm the
   Developer ID exists before Phase 3.**
8. **What does "run it" mean when the client owns the process?** → The app never hosts the
   server Claude talks to. "Run a test call" starts the app's own short-lived server,
   performs the handshake and one read, and stops. The menu bar reflects servers the
   clients started, discovered through heartbeat files.

## Prototype
[mac-app-installs-runs-and-watches-the-server.html](../prototypes/mac-app-installs-runs-and-watches-the-server.html),
published at https://claude.ai/artifact/D2fY7k4dPf8DqEHtYc6QGh. Four screens (Overview,
Activity, Setup, Settings), a menu bar popover, and five simulated states.

What the review changed: the prototype's Settings "Privacy" group (four read-only rows
mirroring `--privacy-status`) is replaced by a "Your data" group listing the two things the
software stores locally and a "Delete all local data" button; the telemetry references in
the design notes are moot.

## Tool description
N/A — the app adds no MCP tool. The server gains one command-line flag, `--check`:

```text
--check   Ask Live for the loaded Remote Script and the current session, print the
          result as JSON, and exit 0 if the script is loaded and up to date, 1 otherwise.
          Changes nothing in the set.
```

## Acceptance Criteria

### Server: what the app reads (Phase 1, in the crate)
- [x] **AC1 — activity stream.** For every tool call, `run_blocking` appends one JSON line to
      `<state_dir>/activity/<session-id>.jsonl`: `ts`, `tool`, `commands` (the Remote
      Script commands it sent), `ok`, `error` (the returned text, if any), `duration_ms`,
      `live_ms` (time inside `send_command`), `in_chars`, `out_chars`. Written on the
      blocking pool after the body returns; a write failure is logged to stderr and never
      fails the tool.
- [x] **AC2 — payloads off by default.** `params` and `result` are added to the line only
      when `ABLETON_MCP_ACTIVITY_PAYLOADS=true`. `ABLETON_MCP_ACTIVITY=false` disables the
      stream entirely. A test pins both defaults.
- [x] **AC3 — heartbeat.** On `initialize`, the server writes
      `<state_dir>/sessions/<pid>.json`: `pid`, `started`, `client` (`clientInfo` name and
      version from the MCP `initialize` request), `server_version`, `script_version` (from
      the handshake, may be null), `activity_file`. Removed on clean shutdown; a reader
      treats a file whose pid is not alive as stale and deletes it.
- [x] **AC4 — `--check`.** As described above. Output: `{"live_reachable", "script_version",
      "expected_version", "up_to_date", "capabilities", "session": {tempo, track_count,
      view} | null, "error"}`.
- [x] **AC5 — library surface.** `install`, `handshake`, `connection` and the state-dir
      function are `pub` and documented as the app's API in `src/lib.rs`; the Docker image,
      stdio purity and the existing suites are unchanged.

### App shell (Phase 2)
- [x] **AC6:** `app/` holds a Tauri 2 project: `app/src-tauri` (Rust, depends on the crate
      by path, own `Cargo.lock`, not a workspace member) and `app/src` (static HTML, CSS and
      JS — no framework, no bundler). The server binary is declared as a sidecar and ships
      inside the `.app` at `Contents/MacOS/ableton-music-maker`.
- [x] **AC7:** The app runs as a menu bar item with a menu (three chain lines, the last
      call, "Open", "Show Activity", "Fix the connection…" enabled when something needs the
      producer, "Quit") and one window with the four screens. Closing the window keeps the
      menu bar item. *Built as a native menu rather than the prototype's popover: same
      content, no custom window positioning.*
- [x] **AC8 — the chain.** Overview shows three cells — the client, `ableton-music-maker`,
      Ableton Live — each with one state line and, when needed, one fix button that opens
      the Setup step that fixes it. States are derived, not typed in: the client and server
      cells from live heartbeats (none → "No client has connected yet"; a stale pid → "not
      running"), the Live cell from a `--check` run every 10 s while the window is open
      (unreachable / outdated script with both versions / loaded with command count).
- [x] **AC9 — session tiles.** Calls, errors, estimated tokens in and out, Live round-trip
      median and slowest call, changes to the set (count of modifying commands) — for the
      selected session, read from its activity file.
- [x] **AC10 — two clients.** When two live heartbeats exist the popover and Overview show
      "Two clients are connected to Live; only one should be" with both client names.

### Setup (Phase 2)
- [x] **AC11 — step 1, Remote Script.** Detects the User Library through the crate's
      discovery, shows the path, and shows one of: installed *version* / found *old*, app
      ships *new* / not found / installing. "Install into Live" or "Update Remote Script"
      calls the crate's installer; the `.bak` behaviour is unchanged. "Change library…" opens
      a folder picker and remembers the choice. On success: "Installed. Restart Live to
      load it."
- [x] **AC12 — step 2, select it in Live.** The exact Live menu path as copy; "Check" runs
      `--check` and shows the answer (version, command count) or the failure with the two
      likely causes, and reminds that Live must be restarted after an install.
- [x] **AC13 — step 3, connect a client.** Segmented Claude Desktop / Claude Code / Cursor.
      Claude Desktop: shows the exact JSON entry, then "Add to Claude Desktop" merges
      `mcpServers.AbletonMusicMaker.command` (the sidecar path) into
      `~/Library/Application Support/Claude/claude_desktop_config.json`, creating the file if
      absent, writing `claude_desktop_config.json.bak-<timestamp>` first, preserving every
      other key and the file's formatting as far as a JSON round-trip allows; then says
      "Restart Claude Desktop". Claude Code: the `claude mcp add …` command with a Copy
      button. Cursor: the sidecar path with Copy.
- [x] **AC14 — step 4, test.** "Run a test call" runs `--check` and shows tempo, track count
      and view, or the failure. Steps mark themselves done from real state, never from
      having been clicked.

### Activity (Phase 2)
- [x] **AC15:** A table of calls for the selected session (sessions listed newest first,
      named by client and start time): time, tool, what it did (a one-line summary derived
      per tool from the commands and, when kept, the params), Live round-trip, result,
      tokens *est.* in → out. Filters: all, changes to the set, reads, errors. New lines
      appear while the file grows.
- [x] **AC16:** Clicking a row opens the detail: round-trip and time spent in Live, chars
      and estimated tokens each way, the error text if any, and the parameters and result —
      or, when payloads are not kept, a sentence saying so and how to turn it on.
- [x] **AC17:** The footer sentence about estimation is present verbatim from the prototype.
- [x] **AC18 — retention.** The app deletes activity files older than the configured
      retention (default 7 days) on launch and once a day; "Clear log" deletes the selected
      session's file after a confirm.

### Settings and local data (Phase 2)
- [x] **AC19:** Live host and port (written to the app's own settings and passed to the
      sidecar through the client config as `env`, so the client-started server uses them).
- [x] **AC20:** "Keep a log of tool calls" (default on) and "Keep the payloads too" (default
      off) — stored in the app's settings **and** applied to the client-started server by
      writing `ABLETON_MCP_ACTIVITY` / `ABLETON_MCP_ACTIVITY_PAYLOADS` into the client
      config's `env` block, so the server and the app agree.
- [x] **AC21 — "Your data".** Lists exactly what exists on disk and where: the activity
      files and the heartbeat directory under `~/.ableton-music-maker/`, the app's settings
      file, and the Remote Script folder in Live's library. "Delete all local data" removes
      the first two after a confirm and leaves the Remote Script and the client config alone.
- [~] **AC22:** Check for updates (opens the GitHub releases page; no auto-updater), About
      with app, server and Remote Script versions, and the line "Third-party integration,
      not made by Ableton." *Built. "Show in menu bar" and "open at login" are not wired
      yet — Phase 3, with the signed bundle, since login items need a bundle identifier.*

### Packaging (Phase 3)
- [ ] **AC23:** `cargo tauri build` on macOS produces a universal `.dmg`; CI builds it
      unsigned on every PR (`macos-latest` job) and signed + notarised on a tag, with the
      Developer ID certificate and notary key from secrets. No secret is in the repository.
- [ ] **AC24:** README's quickstart leads with "Download the app, open it, click Install";
      the Docker section follows unchanged. `docs/technical/feature-matrix.md`,
      `docs/architecture/overview.md` (a section for the app and the activity stream) and
      `docs/facts/source-of-truth.md` (activity variables, state layout, app version source)
      are updated in the same PR.

### No Regressions
- [x] **AC25:** stdout stays pure JSON-RPC; the activity writer and heartbeat never print.
- [x] **AC26:** The Docker image and `verify-image.sh` pass unchanged apart from the state
      directory now containing `activity/` and `sessions/` — both under `/state`.
- [x] **AC27:** Every existing tool behaves identically; the tool count is unchanged by
      this story.
- [x] **AC28:** A server started without the app (Docker, `cargo install`) works exactly as
      before; the app is optional.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `src/tools.rs` | activity line from `run_blocking`; `commands` and `live_ms` captured via the bridge |
| `src/connection.rs` | expose per-call Live time to the caller |
| `src/app.rs` | heartbeat on initialize and removal on shutdown; `--check` |
| `src/bin/ableton-music-maker.rs` | `--check` flag |
| `src/lib.rs` | `pub mod activity`, `pub mod state` (state-dir function moved out of the removed `dataset::consent`), doc comment naming the app API |
| `README.md`, `CLAUDE.md`, `docs/…` | AC24; CLAUDE.md layout block gains `app/` and `src/activity.rs` |
| `.github/workflows/ci.yml` | `mac-app` job |
| `.gitignore` | `app/src-tauri/target/`, `app/src-tauri/gen/`, `app/src-tauri/binaries/` |

### New
| File | Description |
|------|-------------|
| `src/activity.rs` | the JSONL writer and its gates |
| `src/state.rs` | `state_dir()`, `activity_dir()`, `sessions_dir()` |
| `app/src-tauri/Cargo.toml`, `tauri.conf.json`, `src/main.rs`, `src/commands/*.rs` | status, install, check, configure client, activity, settings, delete data |
| `app/src/index.html`, `app/src/app.css`, `app/src/app.js` | the four screens and the popover, from the prototype |
| `app/README.md` | build, sign, run |
| `tests/activity.rs` | AC1–AC3 through `run()` with `FakeBridge` and a temp state dir |

## Remote Script compatibility
No command changes; `--check` uses `get_script_info` and `get_session_info`, both in
`SCRIPT_CAPABILITIES` today. `SCRIPT_VERSION` unchanged.

## Privacy
New data at rest, local only:

| What | Where | Default | Off switch |
|---|---|---|---|
| Activity lines: tool, commands, timings, sizes, error text | `~/.ableton-music-maker/activity/` | on | `ABLETON_MCP_ACTIVITY=false`, or the app's switch |
| Payloads (parameters and results, which contain MIDI and names) | same files | **off** | `ABLETON_MCP_ACTIVITY_PAYLOADS` (default unset) |
| Heartbeats: pid, client name, versions | `~/.ableton-music-maker/sessions/` | on | none — removed on exit, no content beyond the above |
| App settings | Tauri app-data dir | — | "Delete all local data" |

Nothing is uploaded by the server or the app; there is no code path that could
([0004](../../decisions/0004-who-publishes-and-holds-the-data.md)). `TERMS.md` gains the
table above in plain language and the path to delete it. `tests/activity.rs` pins the
payload default as off; `tests/local_only.rs` (from the removal story) keeps proving no
HTTP client exists.

Error text can contain a track or clip name the producer typed; that is why the activity
log is deletable from the app and retained for seven days by default.

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/activity.rs` (new) | one line per call with the documented fields; payloads absent by default and present with the variable; `ABLETON_MCP_ACTIVITY=false` writes nothing; heartbeat written on initialize and gone after shutdown | AC1–AC3 |
| `tests/stdio_integration.rs` | `--check` against the fake bridge prints the documented shape and exit codes | AC4 |
| `src/tools.rs` unit tests | unchanged count | AC27 |
| `app/src-tauri` unit tests | config merge: creates file, preserves other keys, writes backup, idempotent; stale-heartbeat detection; retention pruning | AC13, AC8, AC18 |
| `.github/workflows/ci.yml` | `mac-app` job builds the app unsigned; `verify-image.sh` still green | AC23, AC26 |

Manual, on a Mac with Live 12 and Claude Desktop — the verification steps below.

## Implementation Notes

### Phases
1. **Crate** — activity stream, heartbeat, `--check`, `state.rs`. Ships alone; useful for
   debugging without the app. Depends on the removal story landing first so `run_blocking`
   has one hook point and `state_dir` has a home.
2. **App** — shell, Setup, Activity, Settings, unsigned local builds.
3. **Packaging** — signing, notarisation, README lead, release.

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| One wrapper, no side effects in bodies | `Server::run` / `run_blocking` | the activity hook |
| Blocking pool for anything that touches Live | `spawn_blocking` in `Server::run` | `--check`, the app's status poll |
| JSON status flag | `--privacy-status` → `--status` (removal story) | `--check` |
| Library.cfg discovery, `.bak` on replace | `src/install.rs` | the Setup step, unchanged |
| Cross-checked constants | `ALL_REMOTE_COMMANDS` vs `SCRIPT_CAPABILITIES` test | the activity field list vs `docs/facts/source-of-truth.md` |

### Design Decisions
- **Files, not a socket.** The server is started by the client, possibly before the app is
  open; a JSONL file per session and a heartbeat file per pid need no coordination, survive
  either side restarting, and are readable by a human with `cat`. A Unix socket would need
  the app to be the listener and would lose calls made while it was closed.
- **The app reads what the server writes; it never instruments the client.** Tokens are
  therefore an estimate of payload size, and the UI says so on every screen that shows one.
- **Settings reach the client-started server through the client config's `env` block**,
  not through a file the server polls; the server keeps reading only the environment.
- **No framework in the WebView.** The prototype is plain HTML, CSS and JS; the app keeps it
  that way so a change to a screen is a change to one file and the bundle stays small.
- **Not a workspace member**, so the Dockerfile's dependency-layer trick and the root
  `Cargo.lock` are untouched; the app's `Cargo.lock` lives in `app/src-tauri/`.

## Verification
1. Fresh user account on a Mac with Live 12 installed, Remote Script absent, no client
   config. Open the app: menu bar item appears; Overview shows three attention cells.
2. Setup step 1 → Install. `~/Music/Ableton/User Library/Remote Scripts/AbletonMusicMaker/__init__.py`
   exists. Restart Live, select the control surface. Step 2 → Check reports the version and
   command count.
3. Step 3 → Add to Claude Desktop. The config contains the sidecar path, a `.bak-` file sits
   beside it, other entries untouched. Restart Claude Desktop.
4. Ask Claude "set the tempo to 96 and add a MIDI track called Bass". Overview shows the
   client cell connected with Claude Desktop's name, the server cell running, Activity
   lists `set_tempo`, `create_midi_track`, `set_track_name` with timings and estimates.
5. Quit Live. Overview's Live cell turns red with the two causes; a tool call in Claude
   returns the reinstall/restart error and Activity shows it as an error row.
6. Settings → payloads on → repeat step 4 → detail shows parameters. Payloads off → new
   rows show sizes only. "Delete all local data" empties `activity/` and `sessions/` and
   leaves the Remote Script and the config alone.
7. Open Claude Code with the server added as well: the two-clients warning appears.

## Out of Scope
- Windows and Linux builds.
- A per-day or per-week token view.
- Hosting the server for the client (a daemon, a launch agent, an HTTP transport).
- Auto-update (Tauri's updater) — a later story once releases are signed.
- Renaming `AbletonMCP` inside the Remote Script.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| [remove-telemetry-and-dataset-tiers-local-data-only](remove-telemetry-and-dataset-tiers-local-data-only.md) | Ready | Phase 1 lands after it |
| Tauri 2 toolchain on the build machine and in CI | Ready | `cargo tauri` CLI, Xcode command line tools |
| Apple Developer ID for DefoAI UG | **Unknown** | Phase 3 only; confirm before starting it |
| [0003](../../decisions/0003-remote-script-bind-address.md) bind address | Open | Not blocking; the app's Check step is the cheapest place to run its test |

## Related Stories
- `remove-telemetry-and-dataset-tiers-local-data-only` — the dependency.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created from the prototype review; open questions answered by the owner (bundle, Tauri, no telemetry, local data only) |
| 2026-09-19 | Phases 1 and 2 built and run locally against a simulated client; Phase 3 (signing, notarisation, README lead) open. The menu bar item is a native menu, not a popover |
