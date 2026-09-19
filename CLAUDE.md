# mcp-ableton-music-maker

Rust MCP server that lets Claude drive Ableton Live. Two processes:

- `AbletonMusicMaker_Remote_Script/__init__.py`: a Live control surface. Runs *inside* Ableton on the
  user's machine, listens on TCP 9877, executes JSON commands against the Live API. Live loads
  control surfaces only through its embedded Python interpreter, so this is the one Python file
  in the repo. It must stay compatible with Live's bundled Python (2.7 on Live 10, 3.x on 11+):
  no f-strings, no type hints, no third-party imports. The binary embeds it with `include_str!`
  and installs it; `SCRIPT_VERSION` inside the script is the version the server expects.
- The `ableton-music-maker` crate: the MCP server over **stdio** (`src/bin/ableton-music-maker.rs`) and the
  installer (`src/bin/ableton-music-maker-install-script.rs`). Built on `rmcp` 3.

## Commands

```bash
cargo test                                   # 4 suites: unit, clip-notes, privacy, stdio end-to-end
cargo clippy --all-targets -- -D warnings    # CI runs this
cargo fmt --all
docker compose build                         # runtime image mcp-ableton-music-maker:local
docker build --target test .                 # the suite inside the image
docker/verify-image.sh mcp-ableton-music-maker:local     # the assertions CI runs on the image
docker compose --profile install run --rm install-script   # Remote Script -> Live User Library
```

No Rust toolchain on the machine? Build inside `rust:1-slim-bookworm` with the repo bind-mounted.

## Layout

```
src/connection.rs      LiveBridge trait, AbletonConnection (TCP), RealBridge (reconnecting), LiveError
src/handshake.rs       get_script_info handshake, ScriptInfoCache, per-command capability check
src/tools.rs           Server, ToolSpec, the 37 tool bodies and their #[tool] bindings, run() wrapper
src/telemetry.rs       anonymous tier: opt-in gates, bounded queue, TelemetrySink
src/dataset/           trajectory tier: consent.rs (the gate), recorder.rs, trajectory.rs, poller
src/install.rs         installer logic (Library.cfg discovery, install with .bak)
src/app.rs             startup handshake, dataset wiring, stdio serve, shutdown, --privacy-status
tests/                 clip_notes.rs, privacy_defaults.rs, stdio_integration.rs, common/
docker/                verify-image.sh, Claude Desktop example config
.github/workflows/ci.yml   fmt, clippy, test; image build, verify, trivy, push to GHCR on main
```

## Rules that are easy to break

- **stdout is the MCP transport.** Never print from the server; use `tracing` (stderr). One
  stray line breaks every client. `verify-image.sh` checks stdout is pure JSON-RPC.
- **Telemetry and dataset recording are opt-in and off by default.** Anonymous telemetry needs
  `ABLETON_MCP_ENABLE_TELEMETRY=true` plus credentials in the environment; dataset recording
  additionally needs an explicit user yes (or `ABLETON_MCP_ENABLE_DATASET`). `*_DISABLE_*`
  variables override everything. `tests/privacy_defaults.rs` pins this; do not flip it.
- **No credentials in the repo or image.** Supabase URL and anon key come only from the
  environment (`ABLETON_MCP_SUPABASE_URL`, `ABLETON_MCP_SUPABASE_ANON_KEY`).
- **Tool bodies are plain functions** `fn(&LiveState, &Params) -> Result<String, String>`; the
  `#[tool]` method only binds a body to its `ToolSpec` and calls `Server::run`, which adds
  telemetry, trajectory recording and the consent prompt. Keep it that way so tests can call
  bodies through `run()` with a `FakeBridge`. Failures are `CallToolResult::error`, never
  JSON-RPC errors.
- **Every tool checks its command against the script's capabilities** via `require(live, cmd)`.
  Adding a Remote Script command means: handler in the script, name in `SCRIPT_CAPABILITIES`,
  bump `SCRIPT_VERSION`, add it to `tools::ALL_REMOTE_COMMANDS` (a test cross-checks the list),
  then the tool body.
- **The Live socket is synchronous.** Bodies run on the blocking pool through `spawn_blocking`;
  do not call the bridge from async code directly.
- **The Docker image is hardened by contract**: distroless, non-root, read-only root, `/state`
  the only writable path, both disable variables baked in. Changing the Dockerfile means
  re-running `docker/verify-image.sh`; anything new the server writes must go under
  `ABLETON_MCP_STATE_DIR` / `ABLETON_MCP_DATA_DIR`.

## Working with Claude Code in this repo

`.mcp.json` registers the AbletonMusicMaker server (Docker variant). Build the image first, have Docker
Desktop and Live running with the Remote Script loaded, then approve the project server when
Claude Code asks. Only run one instance of the server at a time across all clients.
