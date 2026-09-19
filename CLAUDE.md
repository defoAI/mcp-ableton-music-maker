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
cargo test                                   # 16 suites: unit, clip-notes, arrangement, mixer, orchestration, capture, performance, song, feel, sound, sets, artist, library, local-only, activity, stdio
cd app && npm run dev                        # the Mac app against this checkout (Tauri 2)
cargo clippy --all-targets -- -D warnings    # CI runs this
cargo fmt --all
docker compose build                         # runtime image mcp-ableton-music-maker:local
docker build --target test .                 # the suite inside the image
docker/verify-image.sh mcp-ableton-music-maker:local     # the assertions CI runs on the image
cd app && npm ci && npm run tauri -- build --target aarch64-apple-darwin   # the app + .dmg (Apple Silicon)
app/scripts/verify-dmg.sh <dmg>              # the assertions CI runs on the disk image
docker compose --profile install run --rm install-script   # Remote Script -> Live User Library
```

No Rust toolchain on the machine? Build inside `rust:1-slim-bookworm` with the repo bind-mounted.

## Layout

```
src/connection.rs      LiveBridge trait, AbletonConnection (TCP), RealBridge (reconnecting), LiveError
src/handshake.rs       get_script_info handshake, ScriptInfoCache, per-command capability check
src/tools.rs           Server, ToolSpec, CORE_TOOLS, the 99 tool bodies and their #[tool] bindings, run() wrapper
src/activity.rs        the local activity log: one JSON line per tool call, payloads off by default
src/state.rs           state_dir / activity_dir / sessions_dir — the only places the server writes
src/install.rs         installer logic (Library.cfg discovery, install with .bak)
src/app.rs             startup handshake, heartbeat, stdio serve, shutdown, --status, --check
app/                   the Mac companion app (Tauri 2): src-tauri/ links this crate, src/ is the UI; src-tauri/src/listen/ is the process tap of Live (Objective-C + Rust, decision 0008)
src/notes.rs           compact note forms (csv, step strings, patterns, tiling) → plain Note objects
src/audio.rs           WAV/AIFF reader and the capture measurements (peak, RMS per bar, silence, clipping)
src/performance.rs     performance state, bar arithmetic, cue resolution (bars → beats, silence check), readout text
src/context.rs         get_context readout and the MCP instructions every client receives at initialize
src/library.rs         the server's copy of Live's browser: paged from the script, on disk under state_dir, searched locally
src/variation.rs       clip variations (fill, ghosts, inversions, thinning, half/double time), humanize, swing, and the key of a recording
src/song.rs            sections (scene names "<name> · <bars>") and songs (the Setlist: scene): parsing, the plan, the cursor — pure
src/sections.rs        make_section, set_song and its edits, play_song, the steering verbs (one state read + one cue each)
src/transition.rs      a jump's transition (tempo, retime, crossfade, fill, drop, sweep) composed into cue primitives
src/sound.rs           the sound vocabulary: words → rack macros, the per-instrument table, or a parameter name — pure
src/sets.rs            export_set / import_set: a rebuildable document under state_dir()/sets, written only on request
src/arrange.rs         the artist-facing tools: arrange (bars), feel (one tool, one undo), set_key, create_return, clear_captures
tests/                 clip_notes.rs, arrangement.rs, mixer.rs, orchestration.rs, capture.rs, performance.rs, song.rs, feel.rs, sound.rs, sets.rs, artist.rs, library.rs, local_only.rs, activity.rs, stdio_integration.rs, common/
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
  and results. `tests/activity.rs` pins the defaults. The library index (`src/library.rs`)
  keeps browser names, paths and URIs on disk; `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in
  memory; `tests/library.rs` pins that. Set exports (`src/sets.rs`) are written only by an
  explicit `export_set` call; `tests/sets.rs` pins that. Anything new the server writes goes
  under `state::state_dir()` and into `TERMS.md`.
- **Tool bodies are plain functions** `fn(&LiveState, &Params) -> Result<String, String>`; the
  `#[tool]` method only binds a body to its `ToolSpec` and calls `Server::run`, which writes
  the activity line. Keep it that way so tests can call bodies through `run()` with a
  `FakeBridge`. Failures are `CallToolResult::error`, never JSON-RPC errors.
- **Every tool checks its command against the script's capabilities** via `require(live, cmd)`.
  Adding a Remote Script command means: handler in the script, name in `SCRIPT_CAPABILITIES`,
  bump `SCRIPT_VERSION`, add it to `tools::ALL_REMOTE_COMMANDS` (a test cross-checks the list),
  then the tool body.
- **The Remote Script touches Live only from Live's main thread** (decision 0007). The
  socket thread parses and waits; `_run_on_main` → `_dispatch` runs every command there. A
  handler that loops over tracks, clips or browser items is a generator (`yield None`
  between units, `yield Done(result)` last) so the executor can slice it per tick; never
  call the Live API from the socket thread, never loop for seconds in one task. Every reply
  carries `main_ms`; the activity line carries it; a slice over 25 ms lands in Live's log.
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

## The tool surface — decision 0006, do not drift from it

The server presents **the artist's set** and keeps the raw layer served but marked. The
rule set lives in [`docs/decisions/0006`](docs/decisions/0006-one-artist-surface-raw-layer-marked-advanced.md);
the short form:

- **`CORE_TOOLS` in `src/tools.rs` is the surface**: Look (`get_context`), Build
  (`build_song`, `make_section`, `create_clip`, `add_notes_to_clip`, `load_instrument_or_effect`,
  `search_browser`, `set_key`, `set_tempo`), Shape (`shape_sound`, `feel`, `set_track_mixer`,
  `set_send`, `create_return`), Arrange (`set_song`, `add_to_song`, `remove_from_song`,
  `arrange`, `create_locator`), Play (`play_song`, `go`, `jump_to`, `back`, `hold_section`,
  `next_section`, `previous_section`, `record_clip`, `capture_mix`, `clear_captures`,
  `end_performance`), plus `delete_track`, `delete_clip`, `export_set`, `import_set`, `batch`.
  Every other tool is served as `adv_<name>` (`adv_fire_scene`, `adv_cue`) — never hidden,
  never gated by an environment variable (that was tried and rejected: the person at the
  client decides). The prefix keeps the two sets from competing in a client's list and in a
  tool search; `batch` accepts either spelling. `tests/artist.rs` pins both.
- **Before adding a tool, extend an artist tool.** Feel is `feel` (modes, one undo); sound is
  `shape_sound` (words, then any parameter by name); the Arrangement is `arrange` (actions);
  sections and songs are `make_section` / `set_song` / `play_song` and the steering verbs. A
  new capability becomes a mode or an action there first; only a new *intent* becomes a tool,
  and the PR says which group it joins or that it is advanced.
- **Units are the artist's**: faders and levels in dB — `volume` *is* dB everywhere it is
  written or read, `fader` is Live's raw 0–1 parameter, meters read dB with 0 dB the top —
  Arrangement positions as Live's 1-based bars (`at_bar`, `from_bar`, `start_bar`), note
  times inside a clip in beats. A beat form may stay as the optional second parameter. Say
  "section", not "scene", outside the `adv_` tools.
- **Descriptions say what the artist gets.** No round-trip counts, no "the Remote Script
  executes on its own clock" — that belongs in `docs/architecture/overview.md`. What the
  Live API cannot do (save the set, automate an Arrangement clip) is said once: in the
  instructions and in the one tool concerned.
- **Every tool carries the MCP annotations** (`readOnlyHint`, `destructiveHint`,
  `openWorldHint: false`), set from its name in `annotations_for`; a new `get_*`, `list_*`,
  `delete_*` or `clear_*` name gets them for free.
- **Stories and issues are read against this list.** A proposal that says "new tool X" is
  answered with the artist tool it extends, or with "advanced", before any code.

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
  renumbered or deleted (`templates/decision.md`). One is open — who publishes the product
  and holds the data (0004). Do not resolve it in prose; do not name a data controller until
  it is decided. The bind address is decided (0003: loopback by default, `bind_host.txt`
  overrides), so say "listens on this machine only by default", never without "by default".
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
