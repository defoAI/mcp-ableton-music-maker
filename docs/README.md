# Product management for mcp-ableton-music-maker

**The product layer of this repository** — what the product is, what it does today, where it
is going, the decisions that shaped it, and how a feature is designed before it is built. The
shape is borrowed from [`defoAI/growomat-management`](https://github.com/defoAI/growomat-management)
and trimmed to what a free, MIT-licensed developer tool needs: there is no commercial layer
because there is no price.

Code lives in `src/` and in the Remote Script. The *why* and the *what next* live here.

---

## Start here

| If you want… | Read |
|---|---|
| What the product is, who it is for, why it exists | [product-overview](strategy/product-overview.md) |
| What the product actually does today | [feature-matrix](technical/feature-matrix.md) |
| How the two processes actually work | [architecture/overview](architecture/overview.md) |
| Where the product is going | [product-plan](technical/product-plan.md) |
| What we may say about it in public | [brand-and-claims](marketing/brand-and-claims.md) |
| How to design a feature before building it | [naming-and-pm-guide](product_management/naming-and-pm-guide.md) |
| What is decided, and what is still open | [decisions/](decisions/) |
| **The real value of any number** | **[source-of-truth](facts/source-of-truth.md)** |

## The one rule

**No document here is the source of a fact it does not own.**

Tool counts, versions, ports, timeouts and privacy defaults live in code and tests. Read
[source-of-truth](facts/source-of-truth.md) before quoting any of them — into a document, a
README, a directory listing, or an answer to a user.

Prose goes stale silently, because nothing fails when it does. So: link the fact, or stamp it —
`37 tools (verified 2026-09-19 against src/tools.rs)`. `scripts/check-docs-facts.sh` fails CI
when the verified snapshot in source-of-truth disagrees with the code.

## Three layers of planning — do not mix them

| Layer | Lives in | Granularity | Example |
|---|---|---|---|
| **Strategy & product** | **`docs/`** | Quarters, audiences, principles | "The Docker path is the default install" |
| **Feature design** | **`docs/product_management/stories/`** | One shippable feature, `<slug>.md` + `<slug>-impl.md` | "Arrangement automation envelopes readable and writable" |
| **Execution & defects** | **GitHub issues**, [`defoAI/mcp-ableton-music-maker`](https://github.com/defoAI/mcp-ableton-music-maker/issues) | One task, bug or verification | "create_audio_clip times out on files over 100 MB" |

Remediation plans do not belong here — those become issues. A document here should still be
broadly true in three months; if it will be stale in three weeks, it is an issue. Stories are
the exception: they describe a moment in time and are pruned once shipped.

## Layout

```
docs/
├── README.md                        This index
├── architecture/overview.md         How the system actually works — updated with the code
├── facts/source-of-truth.md         Where every number actually lives, plus a dated snapshot
├── strategy/product-overview.md     What it is, who for, why, lineage, principles
├── marketing/brand-and-claims.md    Names, vocabulary, approved and banned claims
├── technical/
│   ├── feature-matrix.md            What the product does today
│   └── product-plan.md              Where it is going (horizon level, no feature specs)
├── product_management/
│   ├── naming-and-pm-guide.md       Slug rules, status/priority/size, story shape, lifecycle
│   ├── stories/                     Feature-level user stories (pruned once shipped)
│   ├── prototypes/                  Throwaway transcripts and mock-ups reviewed before code
│   └── templates/                   user_story.md · implementation_plan.md
├── reference/ableton/               Local copies of the Live manual chapters and the Live Object Model, each stamped with source and fetch date
└── decisions/                       Dated decision log, ADR-style, never renumbered
templates/                           plan.md · decision.md
scripts/check-docs-facts.sh          The drift check CI runs
```

## Issues

This repository is its own tracker.

```bash
gh issue list   --repo defoAI/mcp-ableton-music-maker
gh issue create --repo defoAI/mcp-ableton-music-maker --title "..." --body "..."
gh issue close  <n> --repo defoAI/mcp-ableton-music-maker --reason completed --comment "..."
```

Commits trace a fix by the **leading scope** convention — `fix(#42): …` and branches
`fix/42-short-description`. A **trailing** `(#N)` in a commit subject is a pull-request
number, not an issue number; the two collide constantly.

Close on evidence, not age. "Old, and the area was rewritten" is a legitimate reason to close
as stale — say so in the comment rather than marking it completed.

---

*Conventions for AI assistants working here: [CLAUDE.md](../CLAUDE.md).*
