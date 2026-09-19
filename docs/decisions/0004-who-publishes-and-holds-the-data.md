# 0004 — Who publishes this product, and who holds the data?

| | |
|---|---|
| **Status** | **Decided — nobody holds data, because no data leaves the machine; DefoAI UG publishes the fork** |
| **Raised** | 2026-09-19 |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | `TERMS.md` rewrite · the README disclaimer · directory listings · the Mac app's "local data only" posture |

## Context

After the fork and rename the repository gives three answers to the same question:

- `LICENSE`: MIT, copyright Siddharth Ahuja 2025 — correct, and stays; MIT requires the
  notice to be preserved.
- `README.md`: "This is a third-party integration and not made by Ableton. Made by
  Siddharth." — the disclaimer half is correct; the attribution half predates the fork.
- `TERMS.md`: "an independent open-source project … There's no company behind it", data
  contact `ahujasid@gmail.com`, rows written to "a Supabase project" whose owner is not
  named. The repository is under `defoAI` and commits are authored by DefoAI UG.

Whoever owns the Supabase project the credentials point at is the data controller for
anything a user opts into. That is a legal fact, and the document must state it correctly
before anyone is invited to opt in.

## Options

- **A. DefoAI UG publishes the product and holds the data** — `TERMS.md` names DefoAI UG as
  controller with a DefoAI contact, the README credits the upstream author and names DefoAI
  as maintainer, listings are filed by DefoAI. Against: DefoAI takes on GDPR obligations for
  a dataset tier that captures prompts and MIDI.
- **B. The project stays independent; DefoAI is a contributor** — `TERMS.md` stays as it is
  and the upstream author keeps the Supabase project. Against: the repo, the name and the
  image registry are all DefoAI's; "no company behind it" is not true of this fork.
- **C. Remove both tiers from this fork** — no Supabase client, no `TERMS.md` beyond the
  MIT notice, the strongest possible privacy claim, and [0001](0001-telemetry-opt-in-by-default.md)
  option C revisited. Against: loses the dataset the upstream project was built to collect,
  and removes `tests/privacy_defaults.rs`'s reason to exist.

## Recommendation

**A** if DefoAI holds the Supabase project; otherwise **C**. A telemetry tier whose
controller cannot be named must not be switch-on-able, however far off by default it is.

## Cheapest available evidence

Who can log into the Supabase project the anon key belongs to. One question, one answer.

## Decision

**C.** Both tiers — anonymous telemetry and dataset recording — are removed from this fork.
The server keeps no upload path of any kind; the only socket it ever opens is the one to
Live. Whatever it records stays on the producer's machine, under their control, and the
[Mac app](../product_management/stories/mac-app-installs-runs-and-watches-the-server.md)
is built on that posture from the start.

The publisher half follows: DefoAI UG, as the repository owner, publishes and signs the
fork; the README credits the upstream author and keeps the MIT notice. There is no data
controller to name because there is no data.

Decided by the owner on 2026-09-19 ("remove telemetry and only local data"). It gives up
the training dataset the upstream project was built to collect.

## Consequences

- The removal is a story:
  [remove-telemetry-and-dataset-tiers-local-data-only](../product_management/stories/remove-telemetry-and-dataset-tiers-local-data-only.md).
  Until it ships, the code still carries both tiers, off by default, and every document
  keeps describing the code as it is.
- [0001](0001-telemetry-opt-in-by-default.md) is superseded.
- `TERMS.md` shrinks to what is stored locally and how to delete it; the deletion-request
  contact goes away with the data.
- The strongest claim in [brand-and-claims](../marketing/brand-and-claims.md) becomes
  printable: "nothing leaves your machine, ever" — once the removal story's CI gate (no
  HTTP client in the dependency tree) is green.
- The [source-of-truth](../facts/source-of-truth.md) "contested facts" row closes when the
  README disclaimer names the maintainer.
