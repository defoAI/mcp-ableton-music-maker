# Ableton Music Maker for Mac

The menu bar companion to `mcp-ableton-music-maker`. It installs the Remote Script into
Live, connects Claude Desktop, Claude Code or Cursor to the bundled server, shows whether
Claude, the server and Live are talking, and lists every tool call with its timing and an
estimated token cost. It never runs the server Claude talks to — the client does — and it
opens no socket except the one to Live.

Design: `docs/product_management/stories/mac-app-installs-runs-and-watches-the-server.md`
and the prototype beside it. Decisions: `docs/decisions/0005-…`.

## Layout

```
app/
├── src/                 The UI: index.html, app.css, app.js — plain HTML, no framework
├── src-tauri/           The Rust core (Tauri 2); depends on the crate at ../.. by path
│   ├── src/lib.rs       commands the UI calls
│   ├── src/status.rs    the chain: heartbeats + one check against Live
│   ├── src/clients.rs   Claude Desktop config merge (backup first), Code/Cursor commands
│   ├── src/activity.rs  reading the server's activity files; retention; delete
│   ├── src/settings.rs  the app's settings; mirrored into the client config's env block
│   ├── src/tray.rs      the menu bar item
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
