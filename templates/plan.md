# <Plan title>

| | |
|---|---|
| **Status** | Draft \| Active |
| **Verified** | YYYY-MM-DD against <sources> |
| **Owner** | <name> |
| **Horizon** | <period, with absolute dates> |

One paragraph: what this plan is for. Remove completed work rather than ticking it — what
remains should be what is still true and still to do.

**No changelog.** This document describes the current state of affairs. No version-history
table, no "supersedes" row, no "what changed" section. Git holds the history.

---

## Before you write a number

Read [docs/facts/source-of-truth.md](../docs/facts/source-of-truth.md). Tool counts, versions,
ports and defaults must be **linked or stamped**: `37 tools (verified YYYY-MM-DD against
src/tools.rs)`. A bare number with neither is a bug.

---

## 1. What this plan is for

Who the plan serves and what "done" looks like, in one paragraph.

## 2. Where we are now

**Done and closed** — removed from the plan, listed here only if it unblocks something.
**Still open** — the short list.
**Blocking everything downstream** — usually one thing. Name it and link its decision record.

## 3. The bets

One subsection each. For every bet: what it is, why the product rests on it, the open risk.

## 4. The timeline

Phases with absolute dates, not "week 3".

## 5. Checkpoints and tripwires

Dated checkpoints with concrete conditions, and standing tripwires that fire without a
meeting.

## 6. Open questions

Numbered, each with an owner. Anything blocking gets a
[decision record](../docs/decisions/).

## 7. Rules of engagement

What this plan will not do, however tempting. Privacy defaults, claims discipline, the
Remote Script's Python constraints.

## 8. Risks worth naming

Including the ones with no mitigation — say so rather than inventing one.
