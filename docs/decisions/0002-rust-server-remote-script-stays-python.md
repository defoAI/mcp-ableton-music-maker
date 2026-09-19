# 0002 — Is the server Python or a compiled binary?

| | |
|---|---|
| **Status** | **Decided — a Rust crate; the Remote Script stays Python** |
| **Raised** | 2026-09-19 |
| **Decided** | 2026-09-19 — commit `2d9db19`, "Rewrite the MCP server in Rust and rename to mcp-ableton-music-maker" |
| **Owner** | Nick |
| **Unblocked** | The distroless image · the 50 MB size budget · the bundled installer |

## Context

The product is two processes. The Remote Script runs *inside* Live, and Live loads control
surfaces only through its embedded Python interpreter (2.7 on Live 10, 3.x on 11 and 12), so
that file has no choice of language. The MCP server has every choice: it speaks JSON-RPC over
stdio to the client and JSON over TCP to Live.

The Python server was containerised first, on the same day. The result worked but carried an
interpreter, `uv`, the Supabase client and roughly forty transitive packages into a 73 MB
image with a shell in it, and the Smithery-generated Dockerfile it replaced had been building
C extensions on Alpine.

## Options

- **A. Keep the Python server, harden the image** — the least work; upstream merges stay
  mechanical. Against: an interpreter and a package tree in the runtime image, a shell to
  disable, a slower start, and a server whose privacy posture rests on which packages are
  installed.
- **B. Rewrite the server in Rust on `rmcp`, embed the Remote Script** — the decision. One
  static binary per role (`ableton-music-maker`, `ableton-music-maker-install-script`), the
  script compiled in with `include_str!`, a distroless runtime with no shell.
- **C. Rewrite the Remote Script too** — impossible; see Context.

## Decision

**B.** The server and installer are one Rust crate. The Remote Script remains the one Python
file in the repository and is embedded into the binary at build time; `SCRIPT_VERSION` inside
it is the version the server expects, read out of the embedded source at startup.

The rename to `mcp-ableton-music-maker` landed in the same commit, with the control surface
shown in Live as `AbletonMusicMaker`.

## Consequences

- A Rust toolchain (1.85+) is needed to build; without one, build inside
  `rust:1-slim-bookworm` with the repo bind-mounted. `cargo install --path . --locked` is the
  non-Docker path.
- **Upstream server changes no longer port mechanically.** Remote Script changes from
  `ahujasid/ableton-mcp` still do, one file, and each one is a `SCRIPT_VERSION` bump.
- The image contract became checkable: distroless, non-root, read-only root, under 50 MB,
  no shell — `docker/verify-image.sh`.
- The Remote Script's Python constraints are now the *only* Python constraints in the repo,
  and they are absolute: no f-strings, no type hints, no third-party imports.
- The class and log lines inside the script still say `AbletonMCP`; only the folder and the
  name Live shows were renamed. Cosmetic, tracked as vocabulary in
  [brand-and-claims](../marketing/brand-and-claims.md).
