# Ableton Music Maker for Mac

The menu bar companion to `mcp-ableton-music-maker`. It installs the Remote Script into
Live, connects Claude Desktop, Claude Code or Cursor to the bundled server, shows whether
Claude, the server and Live are talking, and lists every tool call with its timing and an
estimated token cost. Its Listen screen shows what Live is putting out, through a macOS
process tap of Live alone, with a float window and a full-screen visual. It never runs the
server Claude talks to — the client does — and it opens no socket except the one to Live.

Design: `docs/product_management/stories/mac-app-installs-runs-and-watches-the-server.md`
and `app-taps-live-output-spectrum-and-meters.md`, with the prototypes beside them.
Decisions: `docs/decisions/0005-…` (Tauri, bundled server) and `0008-…` (the tap).

## Layout

```
app/
├── src/                 The UI: index.html, app.css, app.js — plain HTML, no framework
│   ├── listen.js        the Listen screen; spectrum.js draws for it and for float.html
│   ├── float.html       the small always-on-top spectrum
│   ├── visual.html      the full-screen visual: WebGL feedback, eight presets, F / esc / space
│   └── listen.test.mjs  node --test: the screen's states and the drawing scale, headless
├── src-tauri/           The Rust core (Tauri 2); depends on the crate at ../.. by path
│   ├── src/lib.rs       commands the UI calls
│   ├── src/status.rs    the chain: heartbeats + one check against Live
│   ├── src/clients.rs   Claude Desktop config merge (backup first), Code/Cursor commands
│   ├── src/activity.rs  reading the server's activity files; retention; delete
│   ├── src/settings.rs  the app's settings; mirrored into the client config's env block
│   ├── src/tray.rs      the menu bar item
│   ├── src/listen/      the tap (tap.m, Objective-C, compiled by cc), its FFI (tap.rs), the analysis (pure, tested), the session
│   ├── Info.plist       NSAudioCaptureUsageDescription — merged into the bundle and embedded in dev builds by Tauri
│   ├── examples/listen_probe.rs   the live check: taps Live for a few seconds and prints what arrived
│   ├── tests/listen_integration.rs  the commands on Tauri's mock runtime; two tests need Live (--ignored)
│   ├── binaries/        the sidecar, produced by scripts/build-sidecar.sh (gitignored)
│   └── icons/
└── scripts/build-sidecar.sh
```

Not a Cargo workspace member on purpose: it keeps its own `Cargo.lock`, so the root crate's
lock file and the Dockerfile's dependency layer stay untouched.

## Run from source

Needs Rust 1.85+, the Xcode command line tools, and either the Tauri CLI
(`cargo install tauri-cli --version '^2'`) or Node for `npx @tauri-apps/cli`.

```bash
cd app
./scripts/build-sidecar.sh debug      # builds ../../ and places the server binary as the sidecar
cargo tauri dev                        # or: npm install && npm run dev
```

Without the CLI, `cd src-tauri && cargo run` also works after the sidecar script has run.

## Tests

```bash
cd app/src-tauri && cargo test                                   # unit + integration without Live
cd app/src-tauri && cargo test --test listen_integration -- --ignored --test-threads=1   # with Live open and playing
cd app/src-tauri && cargo run --example listen_probe -- 4         # taps Live for 4 s, prints levels, checks nothing leaks
cd app/src && node --test listen.test.mjs                        # the Listen screen, headless
```

The probe asks Live to play if it is stopped and puts the transport back; set
`AMM_PROBE_NO_TRANSPORT=1` to leave the transport alone.

## Build the bundle

```bash
cd app && cargo tauri build            # runs build-sidecar.sh release, then bundles .app and .dmg
```

Unsigned unless `APPLE_SIGNING_IDENTITY` and the notarisation variables are set; see the
Tauri docs for the exact names. Certificates and keys are CI secrets, never in the repo.

## What the server writes, and where the app reads it

| | Path |
|---|---|
| Activity log | `~/.ableton-music-maker/activity/<session>.jsonl` |
| Heartbeats | `~/.ableton-music-maker/sessions/<pid>.json` |
| App settings | `~/Library/Application Support/com.defoai.ableton-music-maker/settings.json` |
| Claude Desktop config it edits | `~/Library/Application Support/Claude/claude_desktop_config.json` (backup kept beside it) |
