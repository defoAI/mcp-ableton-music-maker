<!-- Required sections: Story · Details · Context · Open Questions · Prototype ·
     Acceptance Criteria (+ No Regressions) · Affected Files · Remote Script compatibility ·
     Privacy · Test Coverage · Implementation Notes · Verification · Out of Scope ·
     Dependencies · Related Stories · Changelog.
     File name is the slug: docs/product_management/stories/<slug>.md -->

# {Title}

## Story
**As a** {producer using Claude Desktop / Claude Code / Cursor with Live open},
**I want** {goal, in Live's own vocabulary},
**So that** {benefit}.

## Details
| Field | Value |
|-------|-------|
| Status | `Draft` / `Ready` / `In Progress` / `Done` |
| Priority | P0 / P1 / P2 / P3 — {one-line reason} |
| Size | S / M / L |
| Tracker | {GitHub issue link, if execution is tracked} |
| Created | YYYY-MM-DD |
| Updated | YYYY-MM-DD |

## Context

### The Problem
{What the producer cannot do, or does badly, today}

### Current State
{How things work today — anchor every claim to a `file:line` link in `src/` or the Remote Script}

### Root Cause
<!-- Optional — primarily for bug fix stories -->
{Why the problem exists}

## Open Questions

<!-- Answer these BEFORE the acceptance criteria are final. A story that begins with a tool
     signature instead of questions is a story nobody has interrogated yet. "None — the
     problem is fully specified" is a valid answer, stated explicitly. -->

{Every clarifying question, and its answer once you have one}

## Prototype

<!-- REQUIRED for anything the producer will talk to. A transcript in
     docs/product_management/prototypes/<slug>.md: the real prompt, the real tool calls and
     parameter names, the returned text including the failure cases, Claude's reply.
     Internal-only work (installer, CI, image): write "N/A — no conversational surface". -->

{Link to the transcript, and what the review changed}

## Tool description

<!-- The #[tool] doc comment, written in full as the model will read it. What the tool does,
     what it refuses, what error text it returns. This IS the documentation. -->

```text
{description}
```

## Acceptance Criteria

- [ ] **AC1:** {Criterion}
- [ ] **AC2:** {Criterion}

### No Regressions
- [ ] **AC{N}:** {Existing behaviour that must be preserved}

## Affected Files

### Modified
| File | Change |
|------|--------|
| `src/tools.rs` | {tool body + `#[tool]` binding} |

### New
| File | Description |
|------|-------------|
| | |

## Remote Script compatibility

<!-- Delete this section only if the story touches no Remote Script command. -->

- [ ] Handler added to `AbletonMusicMaker_Remote_Script/__init__.py` — no f-strings, no type
      hints, no third-party imports; Python 2.7 branches kept where the file already has them
- [ ] Command name added to `ALL_REMOTE_COMMANDS` (the script derives its own list)
- [ ] `SCRIPT_VERSION` bumped
- [ ] Command added to `tools::ALL_REMOTE_COMMANDS`
- [ ] Tool body calls `require(live, "<command>")` before the bridge
- [ ] Live version floor stated: {works on Live 10 / needs 11+}, and what the tool says on an
      older Live
- [ ] Timeout class: read (10 s) / modifying (15 s, add to `MODIFYING_COMMANDS`) / special

## Privacy

<!-- Does this story store anything new on the machine? If no: say "No new local data."
     If yes: the change to TERMS.md, the default, and the test in tests/activity.rs or
     tests/local_only.rs that pins it. Nothing uploads; a story cannot propose that. -->

{Statement}

## Test Coverage

<!-- The DEFAULT is to extend an existing suite. cargo test runs six:
       unit (src/tools.rs, src/notes.rs) — tool bodies through run() with FakeBridge; note forms
       tests/clip_notes.rs        — note handling
       tests/arrangement.rs       — clips, placements, snapshot, track creation
       tests/mixer.rs             — mixer, sends, colours, drum pads, deletion
       tests/orchestration.rs     — search, clip settings, meters, automation, batch, build_song
       tests/capture.rs           — capture_mix flow, timeout guard, measure_capture
       tests/local_only.rs        — no upload path
       tests/activity.rs          — the activity log defaults
       tests/stdio_integration.rs — end-to-end over stdio
     Dockerfile changed? docker/verify-image.sh in the same PR. -->

| Suite / script | Change | AC |
|----------------|--------|----|
| `src/tools.rs` unit tests | {what assertion changes; tool count if a tool is added} | AC{N} |

## Implementation Notes

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| {Pattern name} | `{path}` | {How to apply here} |

### Design Decisions
{"Why not X?" decisions so future readers understand the rationale}

## Verification

### Manual verification steps
<!-- With Live open and the Remote Script reinstalled and Live restarted. -->
1. {Prompt to type, and what Live should show}
2. {Edge case}
3. {No regression}

## Out of Scope
- {Feature or concern intentionally excluded and why}

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| | | |

## Related Stories
- `{related story slug}` — {Relationship}

---

## Changelog
| Date | Change |
|------|--------|
| YYYY-MM-DD | Created |
