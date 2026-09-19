# Decision log

Product decisions that shaped this repository, and the ones still open. Numbered
sequentially, never renumbered, never deleted.

A decision belongs here when reversing it would cost real work — what the product collects by
default, which language the server is written in, what it binds to, who stands behind it.
Routine calls do not.

| # | Decision | Status |
|---|---|---|
| [0001](0001-telemetry-opt-in-by-default.md) | Is telemetry on or off by default? | **Superseded by 0004** — was: both tiers off, opt-in only (2026-09-19). Still describes the code until the removal ships |
| [0002](0002-rust-server-remote-script-stays-python.md) | Is the server Python or a compiled binary? | **Decided** — a Rust crate on `rmcp`; the Remote Script stays the one Python file because Live loads control surfaces only through its own interpreter (2026-09-19) |
| [0003](0003-remote-script-bind-address.md) | Which address does the Remote Script bind? | **Open** — `0.0.0.0` today; `127.0.0.1` untested from Docker Desktop |
| [0004](0004-who-publishes-and-holds-the-data.md) | Who publishes this product and who holds the data? | **Decided** — both tiers are removed; nothing leaves the machine, so nobody holds data. DefoAI UG publishes the fork (2026-09-19) |
| [0005](0005-mac-app-is-tauri-and-bundles-the-server.md) | Is the Mac app native Swift or Tauri, and does it bundle the server? | **Decided** — Tauri 2 in `app/`, depending on the crate as a library, shipping the server binary as a sidecar (2026-09-19) |
| [0006](0006-one-artist-surface-raw-layer-marked-advanced.md) | Which tools does the server present, and in what units? | **Decided** — the artist's set (`CORE_TOOLS`) in five groups, dB and bars; the raw layer is served too, as `adv_<name>`, never hidden (2026-09-19) |
| [0007](0007-live-main-thread-only-in-slices.md) | Where does the Remote Script run its work, and how long may it hold Live? | **Decided** — one executor on Live's main thread, long work sliced per tick (8 ms while playing), one undo step per mutating command, `main_ms` on every reply and activity line (2026-09-19) |

## Already settled, recorded where they bind

Some decisions are load-bearing enough that they live in the file they govern rather than
here. Listed so they are findable:

| Decision | Recorded in |
|---|---|
| stdout is the MCP transport; the server never prints, only `tracing` to stderr | [CLAUDE.md](../../CLAUDE.md), enforced by `docker/verify-image.sh` check 5 |
| Tool bodies are plain functions; `#[tool]` only binds; failures are `CallToolResult::error` | [CLAUDE.md](../../CLAUDE.md), `Server::run` in `src/tools.rs` |
| Every tool checks its command against the script's advertised capabilities before calling Live | `src/handshake.rs` module doc |
| The image is hardened by contract, not by convention | `docker/verify-image.sh` — each check is one promise |
| No credentials in the repository or the image; only the two environment variables | `tests/privacy_defaults.rs`, `verify-image.sh` check 4 |
| The Remote Script installs into Live's User Library, not the legacy User Remote Scripts folder | commit `423855e`, `src/install.rs` |
| Consent is stored with the user's own words, because consent relayed through a model is only as good as the relay | `dataset::consent::record_consent` |
| The Live socket is synchronous and one request/response is indivisible; the poller and tool calls share it under a mutex | `AbletonConnection` in `src/connection.rs` |

Use [templates/decision.md](../../templates/decision.md) for a new one.
