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
└── scripts/          build-sidecar.sh (the server, for one triple) and verify-dmg.sh (the
                      assertions CI runs on the built image)
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

Apple Silicon only — the sidecar, the app and the disk image are all `aarch64-apple-darwin`.

```bash
cd app && npm ci && CI=true APPLE_SIGNING_IDENTITY=- npm run tauri -- build --target aarch64-apple-darwin
app/scripts/verify-dmg.sh "app/src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/"*.dmg
```

Both variables are what CI has anyway, and a local build wants them too. `CI=true` makes
Tauri pass `--skip-jenkins` to `bundle_dmg.sh`, which skips the AppleScript that arranges the
window: a terminal that has not been granted Apple Events control of Finder cannot run it, and
`bundle_dmg.sh` then exits 64 with "Failed running AppleScript" after the `.app` is already
built. `APPLE_SIGNING_IDENTITY=-` signs ad-hoc, which is what seals the bundle — without any
identity Tauri skips signing entirely and the app is left with nothing but the linker's own
signature and no `_CodeSignature`, which `codesign --verify` refuses.

`build-sidecar.sh release` runs first, from `beforeBuildCommand`, and builds the server for
the triple Tauri passes it; the bundler looks the sidecar up by that exact name. The two
commands above are what CI runs, so a green `mac-app` job means this works.

`verify-dmg.sh` mounts the image and checks what a download needs: the app and the
`/Applications` shortcut, both binaries arm64 and signed, `--status` actually running out of
the mounted image, and the Info.plist keys the app depends on — the bundle identifier and
`NSAudioCaptureUsageDescription`, without which the Listen screen can never ask.

A build with no certificate is **unsigned**: fine on the machine that built it, refused by
Gatekeeper on any Mac that downloads it. Signing and notarising happens on a tag, in
`.github/workflows/release.yml`, from six repository secrets — `APPLE_CERTIFICATE` (base64
of the Developer ID `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`. Tauri reads
them itself. No certificate or key is in the repository.

## What the server writes, and where the app reads it

| | Path |
|---|---|
| Activity log | `~/.ableton-music-maker/activity/<session>.jsonl` |
| Heartbeats | `~/.ableton-music-maker/sessions/<pid>.json` |
| App settings | `~/Library/Application Support/com.defoai.ableton-music-maker/settings.json` |
| Claude Desktop config it edits | `~/Library/Application Support/Claude/claude_desktop_config.json` (backup kept beside it) |
