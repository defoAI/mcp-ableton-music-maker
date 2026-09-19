# Remove the telemetry and dataset tiers — local data only

## Story
**As a** producer letting an AI assistant into my open Live set,
**I want** the server to have no upload path at all — not off, not opt-in, absent,
**So that** "nothing leaves my machine" is a property of the binary, not a setting I have to trust.

## Details
| Field | Value |
|-------|-------|
| Status | `Done` — shipped 2026-09-19 |
| Priority | P1 — the Mac app is built on this posture, and every public claim waits on it |
| Size | M |
| Tracker | — |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |
| Decision | [0004](../../decisions/0004-who-publishes-and-holds-the-data.md), superseding [0001](../../decisions/0001-telemetry-opt-in-by-default.md) |

## Context

### The Problem
Two upload tiers exist in the code, both off by default: anonymous telemetry and dataset
recording (prompts, MIDI, device parameters) to a Supabase project. Off-by-default is a
default; a default can be flipped by an environment variable, a stored consent file, or a
future commit. The owner decided on 2026-09-19 that this fork keeps nothing that can upload.

### Current State
- `src/telemetry.rs` — the anonymous tier: `TelemetryConfig::from_env`, the event queue,
  the `TelemetrySink`, the install ID in `customer_uuid.txt`.
- `src/dataset/` — nine files: `consent.rs` (the gate and the consent notice), `recorder.rs`,
  `trajectory.rs`, `passive_poller.rs`, `snapshot.rs`, `hierarchy.rs`, `schema.rs`,
  `supabase.rs` (the only user of `ureq`), `mod.rs`.
- `src/tools.rs` — six dataset tools (`set_dataset_consent`, `submit_intent`,
  `rate_last_action`, `prefer_candidate`, `reject_last_action`, `record_audition`), the
  `Telemetry` enum on `ToolSpec`, and the consent branch in `Server::run` (elicitation or
  appended notice).
- `src/app.rs` — `privacy_status()`, the dataset wiring in `serve_stdio`, the recorder
  flush in `shutdown`.
- `tests/privacy_defaults.rs` — proves credentials alone send nothing.
- `docker/verify-image.sh` check 4 reads four privacy keys from `--privacy-status`; check 3
  asserts the two disable variables in the image environment. The `Dockerfile` bakes them.
- `TERMS.md`, the README "Telemetry" section, CLAUDE.md's opt-in rule.
- `Cargo.toml`: `ureq`, `uuid`, `sha2` and rmcp's `elicitation` feature exist for these tiers
  (confirm each before removing; `uuid` may be used elsewhere).

### Root Cause
The fork inherited a data-collection project. The owner's product is a bridge.

## Open Questions
1. **Keep `--privacy-status`?** → Replace with `--status`: version, expected Remote Script
   version, state directory, activity-log settings. The four privacy keys disappear with the
   tiers; the image check asserts absence instead.
2. **Does the local activity log the Mac app needs land here or in the app story?** → In the
   app story. This story leaves `Server::run` with one hook point (`run_blocking`'s
   outcome) and no side effects.
3. **What replaces the privacy suite?** → `tests/local_only.rs`: the binary reads no
   `ABLETON_MCP_SUPABASE_*` or `*_TELEMETRY*` variable, the tool router lists no dataset
   tool, and CI adds a dependency-tree gate: `cargo tree -e normal` contains none of `ureq`,
   `reqwest`, `hyper`, `curl`.

## Prototype
N/A — no conversational surface changes except six tools disappearing.

## Acceptance Criteria

### Removal
- [x] **AC1:** `src/telemetry.rs` and `src/dataset/` are deleted; `src/lib.rs` no longer
      declares them; no `Telemetry` variant remains on `ToolSpec`.
- [x] **AC2:** The six dataset tools are gone; the tool count test in `src/tools.rs` asserts
      the new count and the README table and `docs/facts/source-of-truth.md` snapshot are
      updated in the same PR (`scripts/check-docs-facts.sh` green).
- [x] **AC3:** `Server::run` has no consent branch: a body's `Ok` is a success result, its
      `Err` an error result, nothing else. rmcp's `elicitation` feature is dropped if
      nothing else uses it.
- [x] **AC4:** No `ABLETON_MCP_SUPABASE_*`, `*_TELEMETRY*`, `*_DATASET*`, `*_CONSENT*`,
      `*_PASSIVE_*`, `*_SNAPSHOT_*` or `*_IMPLICIT_*` variable is read anywhere
      (`grep -rhoE 'ABLETON_MCP_[A-Z_]+' src | sort -u` shows only host/port, state and data
      directories, and whatever the app story adds).
- [x] **AC5:** `ureq` is gone from `Cargo.toml`; `sha2` and `uuid` go too unless another
      use is found and named in the PR.

### Replacement surfaces
- [x] **AC6:** `ableton-music-maker --status` prints JSON: `version`,
      `expected_remote_script_version`, `state_dir`, `data_dir`. `--privacy-status` is
      removed (not aliased).
- [x] **AC7:** `docker/verify-image.sh` check 3 and 4 are replaced by: `--status` parses and
      carries no `telemetry_*` key; the server binary, copied out of the image with
      `docker create` + `docker cp`, contains none of the strings `supabase`,
      `ABLETON_MCP_SUPABASE`, `ENABLE_TELEMETRY`. The `Dockerfile` no longer sets the two
      disable variables.
- [x] **AC8:** `.github/workflows/ci.yml` gains a step that fails if `cargo tree -e normal`
      lists `ureq`, `reqwest`, `hyper` or `curl`.

### Documents describe now
- [x] **AC9:** `TERMS.md` is rewritten to describe only what the software stores locally and
      how to delete it; no data controller, no deletion-request address, no license grant
      for recordings. Re-dated.
- [x] **AC10:** README: the "Telemetry" section becomes "Your data" (three sentences: what is
      stored, where, that nothing is uploaded), the "What the image guarantees" list drops
      the disable-variable line, and the disclaimer credits the upstream author and names
      DefoAI UG as maintainer of this fork.
- [x] **AC11:** CLAUDE.md's "Telemetry and dataset recording are opt-in" rule becomes "The
      server opens no socket except the one to Live; CI fails on an HTTP client in the
      dependency tree." The layout block loses `telemetry.rs` and `dataset/`.
- [x] **AC12:** `docs/architecture/overview.md`, `docs/technical/feature-matrix.md`,
      `docs/marketing/brand-and-claims.md` (approved claim "nothing leaves your machine,
      ever"; banned claim list loses the opt-in qualifier) and `docs/facts/source-of-truth.md`
      (privacy rows) are updated in the same PR.

### No Regressions
- [x] **AC13:** Every Live-facing tool behaves identically; `tests/clip_notes.rs` and
      `tests/stdio_integration.rs` pass unchanged except for the tool count.
- [x] **AC14:** stdout stays pure JSON-RPC (`verify-image.sh` check 5).
- [x] **AC15:** The image still runs read-only with `/state` the only writable path; the
      state directory remains the home of anything the app story adds.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `src/lib.rs`, `src/app.rs`, `src/tools.rs`, `src/bin/ableton-music-maker.rs` | remove the tiers, the tools, the consent branch; `--status` |
| `Cargo.toml`, `Cargo.lock` | drop `ureq` (+ `sha2`, `uuid` if unused), rmcp `elicitation` |
| `Dockerfile`, `docker/verify-image.sh`, `.github/workflows/ci.yml` | AC7, AC8 |
| `TERMS.md`, `README.md`, `CLAUDE.md`, `docs/…` | AC9–AC12 |
| `tests/stdio_integration.rs` | tool count |

### New
| File | Description |
|------|-------------|
| `tests/local_only.rs` | the policy test (Open Question 3) |

### Deleted
| File | Reason |
|------|--------|
| `src/telemetry.rs`, `src/dataset/*` | the tiers |
| `tests/privacy_defaults.rs` | replaced by `tests/local_only.rs` |

## Remote Script compatibility
No command changes. `drain_passive_events` stays in the script and in `SCRIPT_CAPABILITIES`
(removing it would force a `SCRIPT_VERSION` bump for no user benefit); it leaves
`tools::ALL_REMOTE_COMMANDS` since nothing calls it. `SCRIPT_VERSION` unchanged.

## Privacy
Removes data capture. After this story the server writes only `state_dir` and `data_dir`
contents that later stories define; today, nothing.

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `src/tools.rs` unit tests | tool count; `ALL_REMOTE_COMMANDS` cross-check unchanged | AC2 |
| `tests/local_only.rs` (new) | no dataset tools listed; no removed variable read; `--status` shape | AC4, AC6 |
| `tests/stdio_integration.rs` | tool count | AC13 |
| `docker/verify-image.sh` | checks 3/4 replaced | AC7 |
| `.github/workflows/ci.yml` | dependency-tree gate | AC8 |

## Implementation Notes

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| JSON status command | `app::privacy_status` | `--status` |
| Binary extraction from a distroless image | — | `docker create` + `docker cp` in `verify-image.sh` |

### Design Decisions
- **Absence, not a default.** A disable variable in an image proves the image; a missing
  HTTP client in the dependency tree proves every build. The CI gate is the guarantee.
- **`--privacy-status` is not aliased** so no script keeps reading keys that no longer mean
  anything.

## Verification
1. `cargo test`, `cargo clippy --all-targets -- -D warnings`, `scripts/check-docs-facts.sh`.
2. `docker compose build && docker/verify-image.sh mcp-ableton-music-maker:local`.
3. With Live open: `get_session_info`, `create_midi_track`, `add_notes_to_clip` through
   Claude Desktop behave as before; the tool list in the client shows no dataset tools.

## Out of Scope
- The local activity log, heartbeat and `--check` — the app story.
- Renaming `AbletonMCP` inside the Remote Script.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| [0004](../../decisions/0004-who-publishes-and-holds-the-data.md) | Decided | |

## Related Stories
- `mac-app-installs-runs-and-watches-the-server` — depends on this one.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created |
| 2026-09-19 | Done. `regex` went with the tiers too; `--check` and the activity log landed the same day under the Mac app story |
