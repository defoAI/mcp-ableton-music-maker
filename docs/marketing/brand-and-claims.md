# Brand, vocabulary and claims

| | |
|---|---|
| **Verified** | 2026-09-19 against `README.md`, `TERMS.md`, `src/`, the Remote Script and `docker/verify-image.sh` |

What may be said about this product in public — the README, directory listings, release
notes, a Discord reply. Every approved claim below names the code or test that makes it true;
when that source changes, the claim is re-checked or withdrawn.

---

## 01 Names

| Thing | Name | Where it is fixed |
|---|---|---|
| The product | **MCP Ableton Music Maker** | README title |
| The repository and crate | `mcp-ableton-music-maker` | `Cargo.toml`, GitHub |
| The server binary | `ableton-music-maker` | `Cargo.toml` `[[bin]]` |
| The installer binary | `ableton-music-maker-install-script` | `Cargo.toml` `[[bin]]` |
| The control surface, as Live shows it | **AbletonMusicMaker** | the Remote Script folder name |
| The MCP server name a client sees | `AbletonMusicMaker` | `#[tool_handler(name = …)]` in `src/tools.rs` |
| The Docker image | `mcp-ableton-music-maker:local`, `ghcr.io/defoAI/mcp-ableton-music-maker` | `docker-compose.yml`, `ci.yml` |

**Vocabulary debt:** the class and log lines inside the Remote Script still say `AbletonMCP`.
Do not introduce that name anywhere new; it is the upstream name.

## 02 Vocabulary

| Say | Not | Why |
|---|---|---|
| **Live** for the application, **Ableton** for the company | "Ableton" for the app | Ableton's own usage; also keeps "not made by Ableton" unambiguous |
| **Remote Script** (the file), **control surface** (what Live calls it in Settings) | "plugin", "extension" | Ableton's terms; users find them in Live's own settings |
| **tool** — an MCP tool the client sees | | 37 of them; counted in code |
| **command** — a Remote Script command over TCP | "tool" for these | The two lists differ; a tool may use several commands |
| **Session view**, **Arrangement view** | "session mode" | Live's names, capitalised as Live does |
| **set** for the open document | "project", "song" | Live's term |
| **producer** for the user | "musician", "artist" | Neutral across genres and skill |
| **opt in** | "enable tracking" | The mechanism is consent, and the default is off |

## 03 Approved claims — each with its source

| Claim | Made true by |
|---|---|
| "Nothing leaves your machine, ever" | no upload path exists: `tests/local_only.rs`, the CI dependency-tree gate, `verify-image.sh` check 4; [decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md) |
| "No telemetry, no analytics, no dataset" | same |
| "The image is distroless, runs as a non-root user, with a read-only root filesystem" | `verify-image.sh` checks 1, 2 and the `--read-only` run in check 5 |
| "The image is under 50 MB" | `verify-image.sh` check 6 — quote the limit, not a measured size |
| "31 tools" | `grep -c '#\[tool(name = ' src/tools.rs` — count before printing |
| "Every tool checks the Remote Script's version and capabilities before running" | `src/handshake.rs`; `require()` in every body |
| "Works with Claude Desktop, Claude Code and Cursor" | README quickstart; all three are stdio clients |
| "Third-party integration, not made by Ableton" | **Mandatory** wherever the product is listed |
| "Derived from AbletonMCP by Siddharth Ahuja, MIT" | `LICENSE` — the notice must be preserved |

## 04 Banned claims

- **"Official", "Ableton-approved", "partner", "verified by Ableton"** — never, on any surface.
- **"Works with every version of Live"** — the arrangement tools are Live 11+, and there is
  no test matrix. Say "Live 11 and 12; Live 10 without the arrangement tools".
- **"Listens only on localhost"** — not until [decision 0003](../decisions/0003-remote-script-bind-address.md) is made.
- **"No data is ever stored"** — the activity log is stored, locally. Say "nothing is
  uploaded" and point at `TERMS.md` for what is kept on the machine.
- **"Independent project with no company behind it"** — this fork is maintained by DefoAI UG,
  per [decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md).
- **Any user, download, star or Discord member count** that is not read from the source at
  the moment of writing. Silence is not a claim; an invented number is.
- **"Makes music for you", "fully autonomous production"** — the producer directs and the set
  is theirs. Describe the mechanism: "describe the change, Claude makes it in Live".

## 05 Voice

Plain, specific, in Live's vocabulary. Lead with what happens in the set, not with the
technology. A troubleshooting row says what to do, not why it is hard. Example prompts are
things a producer would type, and every one in the README either has a demo link or is
short enough to try in a minute.
