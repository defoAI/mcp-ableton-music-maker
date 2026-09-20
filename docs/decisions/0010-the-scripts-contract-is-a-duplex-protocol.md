# 0010 — What is the Remote Script's contract with the server?

| | |
|---|---|
| **Status** | **Decided — a duplex protocol: ids and events on one socket, read on Live's tick, with a generic op surface beside the named commands** |
| **Raised** | 2026-09-20 |
| **Decided** | 2026-09-20 |
| **Owner** | Nick |
| **Unblocked** | A capability that ships without a script release · the clock reaching the server without being asked · [0007](0007-live-main-thread-only-in-slices.md)'s executor being reachable generically |

## Context

The script answered one request at a time on a socket read by a Python thread,
and spoke only when spoken to. Three costs followed, all measured on Live 12.4.6
on 2026-09-20 (`scripts/live-latency.sh`):

- **A round trip cost 200 ms whatever it did**, and 400 ms if it touched Live's
  API — the second hop being the move onto the main thread. `get_context`, which
  returns over two hundred values, cost the same 400 ms as `get_session_info`,
  which returns three. The cost was per message, not per unit of work.
- **Every capability was a script release**: a Python handler, a capability
  entry, a version bump, and the producer reinstalling and restarting Live. Nine
  such releases happened on 2026-09-19 alone.
- **The server only knew what it asked for.** The clock and the levels rode on
  replies because that was the only direction the socket spoke.

The open question was whether the 200 ms was Live's main-thread tick or the
Python thread waiting to be rescheduled. Nobody had measured it. **The tick is
100.0 ms** (jitter 9.6 ms, worst 111 ms, over 600 samples), so it was the
thread.

## Options

- **A. Keep the request-only protocol and optimise around it** — more composite
  commands, more batching in Rust. Against: it does nothing for the 200 ms floor
  and nothing for the release tax, and composite commands are what there were
  already ninety-eight of.
- **B. A WebSocket** — proper framing, both directions. Against: the latency is
  *where the socket is read*, not how it is framed. A WebSocket is still read by
  a Python thread and pays the same quantum; on loopback, in Python-2.7-
  compatible code, it is two hundred lines that buy nothing.
- **C. Ids and events on the existing newline JSON, read on Live's tick, with a
  generic op surface** — the decision.
- **D. Replace the named commands with the generic surface** — refused. A
  generic batch and a native handler both cost one round trip, but the native
  one reads a track in one go; `get_context` stays native while it wins.

## Decision

**C.** The protocol is `protocol_version: 2`:

- A request may carry an `id` and its reply carries it back. Requests without
  one are answered in order, one at a time, so a server from before ids sees
  exactly what it always saw. Documents are newline-delimited or concatenated.
- Client sockets are non-blocking and drained from the same tick the clock runs
  on. A message waits one tick, not one turn of Live's thread scheduler.
- `subscribe` / `unsubscribe` per socket, with four channels the tick fills:
  `clock`, `levels`, `changes` and `cue`. The changes channel is a diff of a
  small watched set, computed from reads on the main thread. **No listener is
  ever registered on a Live object**, which is what the first capture's deadlock
  taught us ([0007](0007-live-main-thread-only-in-slices.md)).
- `describe(path)` answers what a class has on *this* Live; `run(ops)` takes
  get, set, call and wait_tick in one round trip under the executor's slices.
  Both are rooted at `song`, `application` or `browser`, and no name may start
  with an underscore — which closes every dunder, and so every climb out to the
  interpreter, the file system and the network. The socket has no
  authentication and loopback is the boundary ([0003](0003-remote-script-bind-address.md));
  the generic surface is exactly as dangerous as the fixed list and not one step
  more. Sending Python source was refused for the same reason.
- A cue step may be a batch of ops. **The trigger stays on Live's tick**: only
  the tick reads the transport with no network in between. Rust plans, and every
  fired step reports `late_ms` so Rust learns when it landed.

## Consequences

Measured after, on the same Live:

| | before | after |
|---|---|---|
| Round trip, no API touch | 200 ms | 100.3 ms p50 |
| Round trip, touching Live's API | 400 ms | 100.1 ms p50 |
| `get_context`, 209 values | 400 ms | 102.3 ms |
| The same values as a generic batch | not possible | 102.0 ms, 30 ops, one round trip |
| Cue lateness | not measured | 34.7 ms p50, 64.7 ms p95 over 12 cues |

- **A new capability need not be a script release.** `examples/new_capability_no_reload.rs`
  is the standing proof: track output routing and crossfade assignment, neither
  of which the script has a handler for, read against an unmodified script.
- The clock's rate is floored by the tick: asking for 20 events a second yields
  about 10, because the tick is 100 ms. That is a property of Live, not a fault.
- The script has automated tests for the first time (`tests/remote_script/`,
  Live stubbed, 77 of them), because a protocol can be tested without Live.
- `SCRIPT_VERSION` 1.31.0. An old server on a new script and a new server on an
  old script both degrade to today's behaviour, never to an error, and both are
  tested.
- Reversing this is not cheap: the protocol is the script's contract now. What
  is cheap is ignoring it — every named command still works exactly as it did.
