# Product overview

| | |
|---|---|
| **Status** | Current |
| **Verified** | 2026-09-19 against `README.md`, `TERMS.md`, `LICENSE`, `Cargo.toml` and the decision log |
| **Product** | MCP Ableton Music Maker — `github.com/defoAI/mcp-ableton-music-maker` |
| **Publisher** | Contested — [decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md) |

The one-page answer to "what is this, who is it for, and why should it exist". For claims and
vocabulary see [brand-and-claims](../marketing/brand-and-claims.md); for what ships today see
the [feature-matrix](../technical/feature-matrix.md).

---

## What it is

A bridge that lets an AI assistant work inside a producer's open Ableton Live set. The
producer talks to Claude in Claude Desktop, Claude Code or Cursor; Claude reads and edits the
set through MCP tools; a control surface running inside Live executes each command against
the Live API. Session view, Arrangement view, tracks, clips, MIDI notes, devices and the
browser are all reachable.

It is free and MIT-licensed. There is no account, no subscription, no hosted component. The
only network connection the product makes by default is the one from the server to Live on
the same machine.

## Who it is for

- **Producers who already use Live** and want to describe what they hear instead of clicking
  it into place — "add a jazz progression to the clip in track 1", "build an intro, drop,
  breakdown and outro". They are not developers; they install once and talk.
- **Developers building on Live** who want a tested, versioned command surface into Live
  rather than writing their own control surface. The Rust crate and the capability handshake
  are for them.
- **The dataset tier's contributors** — producers who opt in, explicitly, to have their
  sessions recorded for an open music-production dataset. This audience only exists if
  [decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md) resolves.

## Why it should exist

Live has no official language-model interface, and its Python API is reachable only from a
control surface loaded by Live itself. Everything that wants to drive Live from outside has
to solve the same two problems — get a script inside Live, and get a protocol out of it —
and solve them safely. This project solves them once: one Remote Script, versioned and
capability-checked, and one hardened binary that speaks the protocol every AI client already
understands.

## Lineage

Forked from `ahujasid/ableton-mcp` (MIT, Siddharth Ahuja) at v1.4.0, commit `8731a47`. On
2026-09-19 the Python server was rewritten as a Rust crate, telemetry was made opt-in in the
code, and the project was renamed — [decisions 0001](../decisions/0001-telemetry-opt-in-by-default.md)
and [0002](../decisions/0002-rust-server-remote-script-stays-python.md). The Remote Script is
the surviving upstream artifact and still ports from upstream one file at a time.

## Principles

1. **Off by default.** Nothing leaves the machine except the connection to Live unless the
   producer says yes in their own words. This is enforced by tests, not by prose.
2. **A tool either works or says exactly what to do.** A missing or outdated Remote Script
   yields "run the installer, restart Live", never a half-working session.
3. **The tool description is the documentation.** If a producer needs a manual to use a
   tool, the description is wrong. Design happens in the transcript before the code.
4. **Live's Python is the boundary; everything else is Rust.** One Python file, constrained
   by Live's interpreter. All logic that can live outside Live does.
5. **We use AI but are not used by AI.** The producer directs; Claude executes; the set is
   theirs. The product never claims to make music on its own.
