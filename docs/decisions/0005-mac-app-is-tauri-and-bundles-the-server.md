# 0005 — Is the Mac app native Swift or Tauri, and does it bundle the server?

| | |
|---|---|
| **Status** | **Decided — Tauri 2, bundling the server binary** |
| **Raised** | 2026-09-19 (the Mac app prototype review) |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | The Mac app story · the app's place in the repository · the CI job that builds it |

## Context

The Mac app has to do four things the crate already knows how to do: find Live's User
Library and install the Remote Script (`src/install.rs`), run the handshake
(`src/handshake.rs`), talk to Live (`src/connection.rs`), and read the server's state
directory. It also has to give producers a one-click install on a Mac, where today the path
is Docker Desktop plus a compose profile plus a hand-edited client config.

The clickable prototype
([review link](https://claude.ai/artifact/D2fY7k4dPf8DqEHtYc6QGh)) was reviewed on
2026-09-19 with two questions open: the UI stack, and whether the app carries the binary.

## Options

- **A. SwiftUI menu bar app** — the most native feel and the smallest binary. Against: it
  cannot link the Rust crate; every capability above becomes a shell-out to the bundled
  binaries and a re-parse of their output, and the installer's library discovery would be
  reimplemented or proxied.
- **B. Tauri 2** — the decision. The app's core is Rust and depends on the crate as a
  library, so install, handshake and status are function calls. The UI is HTML and CSS, one
  step from the prototype. Against: a WebView, a larger app, and Tauri's own toolchain in CI.
- **C. Electron** — refused: the heaviest option with no advantage over B.

On bundling: **the app carries `ableton-music-maker` as a sidecar** so a client's config can
point at a stable path inside the app bundle. The alternative — the app installs a binary
into `~/.local/bin` or similar — leaves a file behind when the app is deleted and a second
thing to update.

## Decision

**B, bundling the server.** The app lives in `app/` in this repository, its Rust core
depends on the crate by path, and the release build ships the server binary inside the
`.app`. Docker stays the hardened path and the CI path; it is no longer the first thing a Mac
producer sees.

## Consequences

- The crate's `install`, `handshake` and `connection` modules are a library surface now;
  changes to their signatures are breaking for the app.
- Signing and notarisation need a Developer ID, held by DefoAI UG per
  [0004](0004-who-publishes-and-holds-the-data.md). Certificates and the notary key are CI
  secrets, never in the repository.
- The README's quickstart leads with the app; the Docker section moves after it.
- The app is not a Cargo workspace member: it keeps its own `Cargo.lock` so the Dockerfile's
  dependency layer and the root lock file stay untouched.
