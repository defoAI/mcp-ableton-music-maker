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
cargo test                                   # 9 suites: unit, clip-notes, arrangement, mixer, orchestration, capture, local-only, activity, stdio
cd app && npm run dev                        # the Mac app against this checkout (Tauri 2)
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
src/tools.rs           Server, ToolSpec, the 55 tool bodies and their #[tool] bindings, run() wrapper
src/activity.rs        the local activity log: one JSON line per tool call, payloads off by default
src/state.rs           state_dir / activity_dir / sessions_dir — the only places the server writes
src/install.rs         installer logic (Library.cfg discovery, install with .bak)
src/app.rs             startup handshake, heartbeat, stdio serve, shutdown, --status, --check
app/                   the Mac companion app (Tauri 2): src-tauri/ links this crate, src/ is the UI
src/notes.rs           compact note forms (csv, step strings, patterns, tiling) → plain Note objects
src/audio.rs           WAV/AIFF reader and the capture measurements (peak, RMS per bar, silence, clipping)
tests/                 clip_notes.rs, arrangement.rs, mixer.rs, orchestration.rs, capture.rs, local_only.rs, activity.rs, stdio_integration.rs, common/
docker/                verify-image.sh, Claude Desktop example config
.github/workflows/ci.yml   fmt, clippy, test; image build, verify, trivy, push to GHCR on main
```

## Rules that are easy to break

- **stdout is the MCP transport.** Never print from the server; use `tracing` (stderr). One
  stray line breaks every client. `verify-image.sh` checks stdout is pure JSON-RPC.
- **The server opens no socket except the one to Live.** There is no upload path: no
  telemetry, no dataset, no HTTP client. CI fails on `ureq`/`reqwest`/`hyper`/`curl` in the
  dependency tree and `tests/local_only.rs` fails on any removed variable or the word
  Supabase in `src/`. Do not add one.
- **Local data is on by default, payloads off.** The activity log (`src/activity.rs`) writes
  tool names, commands, timings and sizes; `ABLETON_MCP_ACTIVITY_PAYLOADS` adds parameters
  and results. `tests/activity.rs` pins the defaults. Anything new the server writes goes
  under `state::state_dir()` and into `TERMS.md`.
- **Tool bodies are plain functions** `fn(&LiveState, &Params) -> Result<String, String>`; the
  `#[tool]` method only binds a body to its `ToolSpec` and calls `Server::run`, which writes
  the activity line. Keep it that way so tests can call bodies through `run()` with a
  `FakeBridge`. Failures are `CallToolResult::error`, never JSON-RPC errors.
- **Every tool checks its command against the script's capabilities** via `require(live, cmd)`.
  Adding a Remote Script command means: handler in the script, name in `SCRIPT_CAPABILITIES`,
  bump `SCRIPT_VERSION`, add it to `tools::ALL_REMOTE_COMMANDS` (a test cross-checks the list),
  then the tool body.
- **The Live socket is synchronous.** Bodies run on the blocking pool through `spawn_blocking`;
  do not call the bridge from async code directly.
- **The Docker image is hardened by contract**: distroless, non-root, read-only root, `/state`
  the only writable path, no upload tier in the binary. Changing the Dockerfile means
  re-running `docker/verify-image.sh`; anything new the server writes must go under
  `ABLETON_MCP_STATE_DIR`.
- **The app links the crate.** `install`, `handshake`, `connection`, `state` and `app::check`
  are the Mac app's API; a signature change there breaks `app/src-tauri`. The app is not a
  workspace member — it has its own `Cargo.lock` so the Dockerfile's dependency layer stays
  untouched.

## Product management — `docs/`

The product layer lives in `docs/` (index: `docs/README.md`), shaped after
`defoAI/growomat-management`. Three layers, do not mix them:

| Layer | Lives in |
|---|---|
| Strategy & product | `docs/` — product-overview, feature-matrix, product-plan, brand-and-claims, decisions |
| Feature design | `docs/product_management/stories/<slug>.md` (+ `<slug>-impl.md`); prototype transcript in `prototypes/` |
| Execution & defects | GitHub issues in this repo |

- **Never restate a fact you do not own.** Before writing a tool count, version, port,
  timeout or privacy default anywhere — a doc, the README, an answer to the user — read the
  source named in `docs/facts/source-of-truth.md` and link or stamp it
  (`37 tools (verified 2026-09-19 against src/tools.rs)`). `scripts/check-docs-facts.sh`
  runs in CI and fails when the snapshot there drifts from the code: when you bump
  `SCRIPT_VERSION` or the crate version, add a tool or a command, or change the image size
  limit, update the snapshot and re-date it in the same PR.
- **The architecture note moves with the code.** A change in `src/` or the Remote Script
  updates `docs/architecture/overview.md` and `docs/technical/feature-matrix.md` in the same
  PR. Nobody else will.
- **Stories start with questions and a prototype, not a tool signature.** Named by
  kebab-case slug, no numbers; never referenced from code or tests; deleted once shipped.
  Rules and template: `docs/product_management/naming-and-pm-guide.md`.
- **Any new data capture is a story with a Privacy section** naming the `TERMS.md` change
  and the test in `tests/privacy_defaults.rs` that pins the default as off.
- **Decisions that cost real work to reverse go in `docs/decisions/`**, numbered, never
  renumbered or deleted (`templates/decision.md`). Two are open — the Remote Script bind
  address (0003) and who publishes the product and holds the data (0004). Do not resolve
  them in prose; do not claim "listens only on localhost" or name a data controller until
  they are decided.
- **Documents describe now.** No changelogs, no "previously this said", no superseded plan
  left beside a live one — git holds the history. The only date in a document is the
  `Verified` row in its header; move it only when you actually re-verified.
- **Claims discipline.** Anything public — README, listings, release notes — obeys
  `docs/marketing/brand-and-claims.md`: "third-party, not made by Ableton" always; never
  "official"; never a version claim wider than "Live 11 and 12"; never invented numbers.
- Issues: commits use the leading scope `fix(#N): …`; a trailing `(#N)` is a PR number.
  Close on evidence, not age.

## Working with Claude Code in this repo

`.mcp.json` registers the AbletonMusicMaker server (Docker variant). Build the image first, have Docker
Desktop and Live running with the Remote Script loaded, then approve the project server when
Claude Code asks. Only run one instance of the server at a time across all clients.
