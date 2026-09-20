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
cargo test                                   # 17 suites: unit, clip-notes, arrangement, mixer, orchestration, capture, performance, song, feel, sound, sets, artist, library, samples, local-only, activity, stdio
python3 scripts/check-script-helpers.py      # the Remote Script's pure helpers, against a stub Live
scripts/test-remote-script.sh                # the Remote Script's own suite: tick, duplex, sockets, ops, streams, cues, every command, the fake Live
scripts/fake-live.py                         # a Live that is not Live: the real script + the model on a real socket, port on stdout
scripts/fake-live.py --latency-report        # what a Live call costs, and which calls nobody has measured
scripts/live-latency.sh                      # against a running Live: measures the tick, the round trip and every phase
scripts/live-transcript.sh                   # against a running Live: records tests/fixtures/live-transcript-<version>.json
python3 scripts/live-transcript.py --diff REAL.json FAKE.json   # the field-by-field differential
ABLETON_TARGET=live cargo test -- --test-threads=1   # the whole suite against a real Live (builds in the open set)
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
src/lom.rs             the Live Object Model in Rust: Path (typed, validated), Op, Batch, describe cache — how a capability is written without touching the script
src/tools.rs           Server, ToolSpec, CORE_TOOLS, the 104 tool bodies and their #[tool] bindings, run() wrapper
src/activity.rs        the local activity log: one JSON line per tool call, payloads off by default
src/state.rs           state_dir / activity_dir / sessions_dir — the only places the server writes
src/install.rs         installer logic (Library.cfg discovery, install with .bak)
src/app.rs             startup handshake, heartbeat, stdio serve, shutdown, --status, --check
app/                   the Mac companion app (Tauri 2): src-tauri/ links this crate, src/ is the UI; src-tauri/src/listen/ is the process tap of Live (Objective-C + Rust, decision 0008)
src/notes.rs           compact note forms (csv, step strings, patterns, tiling) → plain Note objects
src/audio.rs           WAV/AIFF reader and the capture measurements (peak, RMS per bar, crest, octave bands, leading silence, clipping)
src/performance.rs     performance state, bar arithmetic, cue resolution (bars → beats, silence check), readout text
src/context.rs         get_context readout and the MCP instructions every client receives at initialize
src/library.rs         the server's copy of Live's browser: paged from the script, on disk under state_dir, searched locally
src/samples.rs         the sample index (Live names the folders, the server walks them), add_sample, adv_sample_folders
src/variation.rs       clip variations (fill, ghosts, inversions, thinning, half/double time), humanize, swing, and the key of a recording
src/song.rs            sections (scene names "<name> · <bars>") and songs (the Setlist: scene): parsing, the plan, the cursor — pure
src/sections.rs        make_section, set_song and its edits, play_song, the steering verbs (one state read + one cue each)
src/transition.rs      a jump's transition (tempo, retime, crossfade, fill, drop, sweep) composed into cue primitives
src/sound.rs           the sound vocabulary: words → rack macros, the per-instrument table, or a parameter name — pure
src/sets.rs            export_set / import_set: a rebuildable document under state_dir()/sets, written only on request
src/arrange.rs         the artist-facing tools: arrange (bars), feel (one tool, one undo), set_key, create_return, clear_captures
tests/remote_script/   the Remote Script's suite (Python, no Live): fake_live.py is the Live Object Model, harness.py runs the script against it, FakeSurface.tick() drives schedule_message
scripts/fake-live.py   the model + the real script on the script's own socket, on Live's 100 ms tick — what the Rust suites run against
scripts/live-api-surface.py  every Live API member the script touches, read out of it by AST
tests/                 clip_notes.rs, arrangement.rs, mixer.rs, orchestration.rs, capture.rs, performance.rs, song.rs, feel.rs, sound.rs, sets.rs, artist.rs, library.rs, samples.rs, local_only.rs, activity.rs, stdio_integration.rs, common/
docker/                verify-image.sh, Claude Desktop example config
.github/workflows/ci.yml   fmt, clippy, test, docs facts; the Mac app and its .dmg — both jobs on macOS, nothing on Linux
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
  keeps browser names, paths and URIs on disk, and the sample index (`src/samples.rs`) the
  names, folders and paths of audio files in the folders Live names; `ABLETON_MCP_LIBRARY_INDEX=false`
  keeps both in memory; `tests/library.rs` and `tests/samples.rs` pin that. The list of
  sample folders the producer added is written only by `adv_sample_folders add`. Set exports (`src/sets.rs`) are written only by an
  explicit `export_set` call; `tests/sets.rs` pins that. Anything new the server writes goes
  under `state::state_dir()` and into `TERMS.md`.
- **Tool bodies are plain functions** `fn(&LiveState, &Params) -> Result<String, String>`; the
  `#[tool]` method only binds a body to its `ToolSpec` and calls `Server::run`, which writes
  the activity line. Keep it that way so tests can call bodies through `run()` with a
  `FakeBridge`. Failures are `CallToolResult::error`, never JSON-RPC errors.
- **Reach Live through `run`/`describe` before you add a command.** The script exposes a
  generic surface (protocol 2, [decision 0010](docs/decisions/0010-the-scripts-contract-is-a-duplex-protocol.md)):
  `describe(path)` says what this Live has, and `run(ops)` gets, sets and calls along a
  whitelisted path in one round trip. Build them in `src/lom.rs` with `Path` and `Batch`, and
  the capability ships in the binary alone — no handler, no `SCRIPT_VERSION` bump, no reinstall,
  no reselecting the control surface, and it works against every script that already has the
  generic layer. `cargo run --example new_capability_no_reload` is the proof and the pattern.
  A new *command* is for what ops cannot express: work that must loop inside one tick, or that
  needs Live's main thread held across steps.
- **The command list has one home: `tools::ALL_REMOTE_COMMANDS` in `src/tools.rs`.** The
  Remote Script declares nothing by hand — `_served_commands` reads its own dispatch back at
  import — so there is no second list to keep in sync. Adding a Remote Script command means:
  handler in the script, name in `ALL_REMOTE_COMMANDS`, bump `SCRIPT_VERSION`, then the tool
  body. `the_servers_command_list_and_the_scripts_dispatch_are_the_same_set` fails the build
  if a name here has no handler, or a handler has no name here. Every tool still checks its
  command via `require(live, cmd)`; at handshake the server uses its own list when Live runs
  the script version the binary embeds, and the script's derived list only when they differ.
- **The fake Live moves with the real one, always.** `tests/remote_script/fake_live.py` is
  the Live Object Model the whole test suite runs against, and a fake that drifts is worse
  than no fake: it makes a green suite a lie. So every change to the Remote Script, and
  every new thing learned about Live, updates the model **in the same PR**:
  - A handler that touches a Live member the model lacks — a new property, a new method, a
    new argument such as `ClipSlot.fire(record_length=…)` — is a **model gap**. Fix it in
    `fake_live.py`. Never give the test an easier parameter, never stub the member out, and
    never add it to `DELIBERATELY_ABSENT` unless a test needs the *fallback path* to run.
  - A new Remote Script command means a case in
    `tests/remote_script/test_every_command.py` with the parameters a real session sends and
    an assertion about **the set**, not the reply. The suite fails on a name in
    `ALL_REMOTE_COMMANDS` with no case, in both directions.
  - A Live behaviour the script now depends on — an argument order, what raises, what a write
    snaps to — gets a case in `tests/remote_script/test_live_semantics.py`, with where the
    fact came from (Cycling '74's LOM reference, or a measurement in this repo).
  - `scripts/live-api-surface.py` reads every Live member the script touches out of it by
    AST, and `test_live_api_conformance.py` fails when one is missing from the model or has
    the wrong shape. That check is the floor, not the ceiling: it proves the member exists,
    not that it behaves.
  - **Numbers are measured, never invented.** `fake_live.LATENCY_12_4_6` carries what a Live
    call costs, and every row is stamped with the run it came from. A call nobody has timed
    goes in `LATENCY_UNMEASURED` and charges nothing, so the gap is visible
    (`scripts/fake-live.py --latency-report`) instead of quietly filled in. The same rule as
    `brand-and-claims.md`: a figure without a source does not ship.
  - **When the real Live is available, check the fake against it.**
    `ABLETON_TARGET=live cargo test -- --test-threads=1` runs the whole suite against Live
    instead of the fake — the same tests, the same assertions — and
    `scripts/live-transcript.sh` re-records `tests/fixtures/live-transcript-<version>.json`,
    which the differential replays against the fake and diffs field by field. Refresh that
    fixture whenever `SCRIPT_VERSION` changes, and say in the PR that it was refreshed or
    that it was not and why.
  - What the fake is **not** is written down, in `docs/architecture/overview.md` and as tests
    in `TheModelIsNotLive`: no audio, no rendering, no real browser index, no Max for Live,
    and `--set-per-connection` is not Live (the Rust suite does not use it: every test gets
    its own process with `--shared-set`, one set, as Live is). Anything that depends on
    those is a real-Live check, always, and the issue says so rather than the suite
    pretending.
- **The Remote Script touches Live only from Live's main thread** (decision 0007). The
  socket is read *on* that thread, on Live's own 100 ms tick, not from a Python thread
  (`SOCKET_READER`, decision 0010); `_run_on_main` → `_dispatch` runs every command there. A
  handler that loops over tracks, clips or browser items is a generator (`yield None`
  between units, `yield Done(result)` last) so the executor can slice it per tick; never
  call the Live API from the socket thread, never loop for seconds in one task. Every reply
  carries `main_ms`; the activity line carries it; a slice over 25 ms lands in Live's log.
- **The Live socket is synchronous.** Bodies run on the blocking pool through `spawn_blocking`;
  do not call the bridge from async code directly.
- **The Docker image is hardened by contract**: distroless, non-root, read-only root, `/state`
  the only writable path, no upload tier in the binary. CI no longer builds it
  ([decision 0009](docs/decisions/0009-ci-builds-the-mac-app-only.md)), so changing the
  Dockerfile means running `docker/verify-image.sh` yourself before you push; anything new
  the server writes must go under `ABLETON_MCP_STATE_DIR`.
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
  `search_browser`, `add_sample`, `set_key`, `set_tempo`), Shape (`shape_sound`, `feel`, `set_track_mixer`,
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
