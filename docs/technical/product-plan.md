# Product plan

| | |
|---|---|
| **Status** | Draft — the open items are sourced from the repository; the horizon and bets are the owner's to confirm |
| **Verified** | 2026-09-19 against the repository state at commit `64f8484` |
| **Owner** | Nick |
| **Horizon level** | Releases. Feature design lives in stories; defects live in issues |

**This document does not contain feature specifications.** Features are designed as stories
in [`docs/product_management/stories/`](../product_management/stories/) and tracked as GitHub
issues. This is the layer above both: what the engineering effort is *for*, and which bets
the product rests on.

---

## Where the product actually is

| Capability | Status | Notes |
|---|---|---|
| Rust server on `rmcp`, stdio | **Live** | [decision 0002](../decisions/0002-rust-server-remote-script-stays-python.md) |
| The artist's tool set, with the raw layer served as `adv_` | **Live** | [decision 0006](../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md); the count lives in `src/tools.rs`, see [source-of-truth](../facts/source-of-truth.md) |
| Capability handshake with the Remote Script | **Live** | `src/handshake.rs` |
| Hardened Docker image, verified by `verify-image.sh` | **Live, built locally** | CI builds only the Mac app ([decision 0009](../decisions/0009-ci-builds-the-mac-app-only.md)) |
| Telemetry and dataset tiers | **Removed** | [decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md); no upload path exists, pinned by `tests/local_only.rs` |
| Mac app: install, status, activity, and listening to Live | **Runs from source** | [0005](../decisions/0005-mac-app-is-tauri-and-bundles-the-server.md) Tauri and the bundled server, [0008](../decisions/0008-app-hears-live-through-a-process-tap.md) the process tap; signing and notarisation are the open phase |
| Remote Script bound to loopback | **Live** | [decision 0003](../decisions/0003-remote-script-bind-address.md); `bind_host.txt` beside the script overrides it |
| Verified end-to-end on a Mac with Live | **Recorded** | Live 12 Suite on macOS 26.6, 2026-09-19: the capture through resampling, and the app's process tap of Live (frames at the device rate, other apps inaudible, no audio device left behind). Docker Desktop against a loopback bind is still unverified — [0003](../decisions/0003-remote-script-bind-address.md) |

## What is open before the first release under the new name

1. **Remove the upload tiers** — decided in [0004](../decisions/0004-who-publishes-and-holds-the-data.md); the story is the work. Blocks `TERMS.md`, the README disclaimer and every listing.
2. **Decide the bind address** — [0003](../decisions/0003-remote-script-bind-address.md). One test on a Mac.
3. **A recorded pass against real Live**: install the script from the image, restart Live, run the README's example prompts through Claude Desktop, and note the Live version. Becomes an issue with a checklist.
4. **Directory listings under the new name** — Smithery, and any MCP connector directory — once 1 has shipped.
5. **The Mac app** — the first thing a producer sees; story `mac-app-installs-runs-and-watches-the-server`.
6. **The tool surface, measured against a real session** — the tracking issue
   [#18](https://github.com/defoAI/mcp-ableton-music-maker/issues/18) orders seventeen
   findings from the 2026-09-19 session: compact note entry and batching first, then the
   mixer, automation and arrangement gaps, then hearing the result.

## The bets the product rests on

### 1. The Remote Script is the moat and the constraint

Every alternative has to write one. Ours is versioned, capability-advertised, and installed
by a binary that finds Live's library itself. The constraint is the same fact from the other
side: it is Python inside Live's interpreter, it runs on Live's main thread, and no test can
exercise it without Live. **Open risk:** no automated test touches the script; a Live update
that changes the API is invisible until a producer hits it.

### 2. Off by default is the product's reputation

A tool that sits inside a producer's session and can read their MIDI must be trusted. The
privacy suite and the image contract make "off by default" a build failure to break, not a
promise. **Open risk:** none once the removal ships — the posture becomes "no upload path exists",
enforced by a dependency-tree gate in CI rather than by defaults.

### 3. The conversation is the interface

There are no screens to design; there are tool descriptions and returned text. Design
quality lives in the transcript prototype and the `#[tool]` doc comment. **Open risk:** the
tool descriptions were ported, not rewritten, in the Rust rewrite; nobody has yet reviewed
them as the documentation they are.

## Checkpoints

| When | Condition |
|---|---|
| Before the first tagged release under the new name | Items 1–3 above closed; `SCRIPT_VERSION`, `Cargo.toml` and the snapshot in [source-of-truth](../facts/source-of-truth.md) agree |
| Every Remote Script change | `SCRIPT_VERSION` bumped, release notes tell users to reinstall and restart Live |

## Rules of engagement

- Nothing widens what is collected without a story carrying a Privacy section and a test.
- No claim in the README that [brand-and-claims](../marketing/brand-and-claims.md) bans.
- The Remote Script stays compatible with Live's bundled Python.
- One `main` branch; CI green including `verify-image.sh` before anything is tagged.

## Risks worth naming

- **Live API drift** — no mitigation beyond the handshake; a version matrix would need a
  machine with several Live versions.
- **Upstream divergence** — server changes upstream no longer port; the script still does,
  by hand.
- **Single maintainer** — the decision log has one owner.
