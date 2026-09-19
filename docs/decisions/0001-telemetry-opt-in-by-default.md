# 0001 — Is telemetry on or off by default?

| | |
|---|---|
| **Status** | **Superseded by [0004](0004-who-publishes-and-holds-the-data.md)** — both tiers are being removed; until that ships, this decision describes the code |
| **Raised** | 2026-09-19 (the dockerisation work) |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | The Docker image contract · the rewritten `TERMS.md` · every "off by default" claim in the README |

## Context

The upstream project this repository forked from (`ahujasid/ableton-mcp`, commit `8731a47`)
turned anonymous telemetry on whenever a `config.py` with Supabase credentials was present,
and treated an *unanswered* dataset-consent question as permission to record. Credentials
were expected to be checked into the running install. A containerised server that phones
home by default is not something a producer should have to discover by reading source.

The maintainer's instruction on 2026-09-19 was "disable all telemetry by default". The
question was how far to take it.

## Options

- **A. On by default, credentials shipped** — the upstream posture. Maximises data
  collection; every install is a data source. Against: nothing in the product's own
  description tells the user, and the dataset tier captures prompts and MIDI.
- **B. Opt-in in the code, not only in the image** — the decision. Anonymous telemetry needs
  `ABLETON_MCP_ENABLE_TELEMETRY=true` *and* credentials in the environment; dataset recording
  additionally needs an explicit yes to the consent question (or
  `ABLETON_MCP_ENABLE_DATASET`); `ABLETON_MCP_DISABLE_TELEMETRY` and
  `ABLETON_MCP_DISABLE_DATASET` override every opt-in and stored answer.
- **C. Strip both tiers from the fork entirely** — removes the whole class of risk and the
  Supabase client. Against, at the time: it made merging upstream changes harder. Deferred,
  not refused; it comes back under [0004](0004-who-publishes-and-holds-the-data.md).

## Decision

**B.** Both tiers are off by default in the binary, and the Docker image additionally bakes
in both disable variables. An unanswered or dismissed consent question means recording stays
off. With telemetry off the question is never asked, because a yes could not start anything.

## Consequences

- **Enforced by tests, not prose.** `tests/privacy_defaults.rs` places credentials in the
  environment and proves that nothing is sent and no request is built unless the user opts
  in. Reversing this decision means changing that test in the same PR, so the reversal is a
  visible diff rather than a default that quietly moved.
- `docker/verify-image.sh` checks 3 and 4 pin the image side: both disable variables in the
  image environment, and `--privacy-status` reporting every gate off with no credentials.
- `TERMS.md` and the README describe the opt-in defaults; a change to either must keep
  matching the tests.
- The consent question is phrased so the model relays it verbatim and the user's own words
  are stored (`dataset::consent::record_consent`).
- Every story that adds data capture carries a **Privacy** section naming its default (off)
  and its test — see the [user story template](../product_management/templates/user_story.md).
