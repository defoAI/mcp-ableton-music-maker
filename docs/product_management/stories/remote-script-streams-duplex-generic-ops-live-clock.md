# The Remote Script streams: one duplex socket, generic ops, the clock pushed to Rust

## Story
**As a** producer performing and building through Claude with Live open,
**I want** the server to hear Live's clock and changes as they happen and to reach any part of Live's API in one round trip, without a Remote Script reinstall every time a capability is added,
**So that** a jump lands where I asked, a reply never waits on a read that could have been streamed, a new capability is a server release rather than a "restart Live", and any server version works with any script that speaks the protocol.

## Details
| Field | Value |
|-------|-------|
| Status | `Draft` — the open questions carry proposed answers; question 1 is answered by a measurement, not a review |
| Surface | [Decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md): no new tool. The artist's set is unchanged; what changes is how fast and how generally the tools reach Live. Units stay the artist's |
| Priority | P1 — every reply today pays a 200 ms scheduling quantum per round trip, and every new capability costs the producer a Live restart |
| Size | L — six phases in Implementation Notes; each ships alone and each is measured by the same script before and after |
| Tracker | — (file the issue once question 1 is answered) |
| Created | 2026-09-20 |
| Updated | 2026-09-20 |
| Decisions | [0002](../../decisions/0002-rust-server-remote-script-stays-python.md) Rust server, Python script · [0003](../../decisions/0003-remote-script-bind-address.md) loopback by default · [0007](../../decisions/0007-live-main-thread-only-in-slices.md) main thread only, in slices · a new record is needed when questions 1–4 are answered: *the script's contract becomes a duplex protocol with generic ops and streams; the trigger stays on Live's tick* |
| Prototype | [prototypes/remote-script-streams-duplex-generic-ops-live-clock.md](../prototypes/remote-script-streams-duplex-generic-ops-live-clock.md) — the wire, both directions, failures included |

## Context

### The Problem
Three things, all measured against Live 12 Suite on macOS 26.6 on 2026-09-20 with one open socket:

- **A round trip costs 200 ms whatever it does.** Six consecutive `get_script_info` calls — a command that never touches Live's API — returned in 199.4, 210.6, 199.7, 200.4, 199.6 and 199.8 ms. A command that does touch the API costs 400 ms. `get_context`, which returns 413 values, costs the same 400 ms as `get_session_info`, which returns three. The cost is per message, not per unit of work. The script's own header names the cause: Live scheduling the script's socket thread.
- **Every capability is a script release.** Adding one means a handler in Python, its name in `SCRIPT_CAPABILITIES`, a `SCRIPT_VERSION` bump, and the producer reinstalling and restarting Live. The script carries 89 `hasattr` / `except AttributeError` branches — Live 10, 11 and 12 knowledge living where no test can see it.
- **The server only knows what it asked for.** The clock and the levels ride on replies because that is the only direction the socket speaks. A jump is planned with a clock that is one round trip stale; the Mac app's visual guesses the beat from audio because nothing tells it the bar.

### Current State
- The script (`AbletonMusicMaker_Remote_Script/__init__.py`) accepts on a background thread (`start_server`, `_server_thread`) and reads each client on its own background thread (`_handle_client`): one JSON object per line, one request in flight, replies in order, no message ids. The command is handed to Live's main thread through `schedule_message` and run in slices (`_run_on_main`, `SLICE_MS_PLAYING`, `SLICE_MS_STOPPED`, `SLOW_SLICE_MS`); every reply carries `main_ms`.
- The cue engine (`_schedule_cue`, `_perf_tick`, `_arm_perf_tick`) and the per-bar level peaks (`_level_tick`) run on the main-thread tick, re-armed with `schedule_message(1, …)`. **The tick's period is 100.0 ms** — measured by phase 0 against Live 12.4.6 on 2026-09-20: jitter 9.0 ms, worst 112.0 ms, over 181 consecutive samples. This is the number phase 0 existed to find, and it decided phase 1: at 100 ms the tick is twice as fast as the 200 ms thread quantum, so moving the reader onto it wins, and the round trip's floor becomes one tick rather than one turn of Live's scheduler.
- There is no generic path: no read-a-property, no call-a-method, no describe. `batch` exists only in Rust (`tools::batch_body`) and each tool inside it pays its own round trips.
- The server's connection (`connection::AbletonConnection`) is persistent and reconnecting, newline-framed, one request at a time under a lock; `RealBridge` wraps it. The handshake (`handshake::ScriptInfo`) passes unknown fields through `extra`, so a new `tick` block reaches the server without a struct change.
- Passive listeners on the Live Object Model were removed: attaching one to a clip while it recorded, from inside a Live notification, deadlocked Live ([0007](../../decisions/0007-live-main-thread-only-in-slices.md) and the note in the script's constructor).
- The activity log (`src/activity.rs`) records the commands each tool sent; payloads are off by default (`tests/activity.rs`). The script binds loopback by default and has no authentication ([0003](../../decisions/0003-remote-script-bind-address.md)).

### Root Cause
The socket is read by a Python thread that Live schedules on its own terms, and the protocol is request-only, so latency is bounded below by Live's thread quantum and information flows only when asked. The command list is fixed in Python because the only way to reach the API is a hand-written handler.

## Open Questions
Proposed answers are marked. **Question 1 is not a review question**: it is answered by phase 0's measurement, and the story's shape depends on the number.

1. **What is the period of Live's main-thread tick, and how much does it jitter?** → *Unknown. Phase 0 adds `tick` to `get_script_info` and `scripts/live-latency.sh` prints it.* If it is near 200 ms, phase 1 (reading the socket on the tick) buys nothing and is dropped; the streams and the generic ops still ship, at that latency. If it is 10–20 ms, phase 1 is a 10–20× cut on every round trip.
2. **WebSocket or newline JSON on TCP?** → *Proposed: newline JSON, with `id` and `event` fields.* The latency is Python thread scheduling, not framing; a WebSocket is still read by a Python thread and pays the same quantum. A hand-written WebSocket in Python-2.7-compatible code is two hundred lines that buy nothing on loopback. The one thing a duplex protocol needs — pairing replies to requests and telling events apart — is a field, not a transport.
3. **Where does the trigger live?** → *Proposed: in Live, on the tick, as today.* Rust plans a cue and learns how late it fired; it never sends "fire now" over a link with jitter. The clock is *streamed* to Rust so planning is never stale.
4. **How wide is `run`?** → *Proposed: get, set, call and wait_tick on paths rooted at `song`, `application` and `browser`; no dunder attributes; no `eval`, imports, files or sockets; a per-op error that names the op.* The socket has no authentication, so the generic path must be exactly as dangerous as today — "anything on loopback can drive Live" — and not one step more. Sending Python source is refused for that reason.
5. **What streams?** → *Proposed: three channels.* `clock` at a requested interval while the transport runs; `levels` once a bar from the peaks the tick already keeps; `changes` as a diff of a small watched set (track and scene names and count, transport, tempo, the playing and triggered scene, arm/mute/solo) computed on the main thread every N ticks. Never a listener on a clip.
6. **Does an old server keep working on a new script, and the reverse?** → *Yes, both.* A request without `id` is answered in order without one and never receives events; a new server refuses `run`, `describe` and `subscribe` through `require()` on an old script and falls back to a read per reply for the readouts.
7. **Which native commands stay?** → *All of them, until the numbers say otherwise.* `get_context`, `write_clips`, `create_tracks`, `place_clips`, `place_sample`, the capture and the cue engine are hot or timing-bound. New capabilities ship as Rust-side op sequences first; a sequence is promoted to a native handler when it is on a hot path or needs a continuation the ops cannot express.
8. **Where does discovery live?** → *Rust, cached under the state dir per Live version.* `describe` at handshake for the classes the server uses; the 89 Python branches migrate to one table in `src/lom.rs` with tests, one branch per commit.
9. **Does the Mac app subscribe?** → *Yes, on its own connection.* The script already serves several clients (the browser lock exists for that). The app opens no socket it did not already open; it uses the one to Live for the Listen screen's bar and the visual's beat.
10. **Is a protocol version needed?** → *Yes: `protocol_version: 2` in the handshake.* Framing is compatible both ways, but the meaning of a message with an `id` is new, and the server should say which protocol it speaks in `--check`.

## Prototype
[remote-script-streams-duplex-generic-ops-live-clock.md](../prototypes/remote-script-streams-duplex-generic-ops-live-clock.md):
the handshake with the tick block, an overlapped pair of requests, subscribing and the three
event shapes, `describe` on a device and on a refused path, a `run` batch with a `wait_tick`
and two refused ops, a cue as ops with the `cue` event that reports lateness, old-on-new and
new-on-old, and the eight-line output of the measurement script that decides the story.

What the review changed: nothing yet; not reviewed.

## Tool description
N/A — no MCP tool. The Remote Script gains four commands and one handshake block, and the server gains one flag and one script:

```text
describe {path}            The class, attributes (type, read-only) and methods of the object at
                           a path rooted at song, application or browser, for this Live.
run {ops}                  An ordered batch of {op: get|set|call|wait_tick, path, value|args|
                           ticks, as}, run on Live's main thread under the executor's slices,
                           one round trip. Results keyed by `as`; the first failing op stops
                           the batch and the error names it. Same roots and refusals as describe.
subscribe {channels, clock_every_ms}
unsubscribe {channels}     Events on this socket: clock, levels, changes. Never sent to a socket
                           that did not subscribe.
get_script_info            gains tick {period_ms, jitter_ms, samples, playing},
                           socket_reader, protocol_version 2, live {version, python}.

ableton-music-maker --check    also prints protocol_version, tick and socket_reader.
scripts/live-latency.sh        the eight measurements in the prototype, against a running Live.
```

## Acceptance Criteria

### Phase 0 — the number (ships alone)
- [x] **AC1 — the tick is measured.** The script samples the interval between consecutive `schedule_message(1, …)` callbacks over a rolling window and reports `tick {period_ms, jitter_ms, samples, playing}` in `get_script_info`; `--check` prints it. `scripts/live-latency.sh` prints the tick period and the round-trip cost with and without an API touch, each over at least 20 samples, as p50 and p95.
- [x] **AC2 — the story is re-dated on the number.** The measured period is written into this story's Context, and phase 1 is kept or struck accordingly, before phase 1 is started.

### Phase 1 — one duplex socket
- [x] **AC3 — message ids.** A request may carry `id`; its reply carries the same `id`. Requests without `id` are answered in order without one. The script never reorders replies to id-less requests.
- [x] **AC4 — the reader moves to the tick.** Client sockets are non-blocking and drained from the main-thread tick; the executor and slices are unchanged. A client that stops reading cannot stall the tick: writes are bounded and a full outbound buffer drops that client with a log line, never blocks.
- [x] **AC5 — the round trip is bounded by the tick.** With phase 1 in, `live-latency.sh` shows p95 round trip ≤ 2 × tick period for both the no-API and the API command. If AC1's period made this impossible, this AC is struck with the number beside it.
- [x] **AC6 — the server can overlap.** `AbletonConnection` gains a reader task and a map of pending ids; two requests in flight on one socket are paired correctly under `tests/connection` with a fake script that answers out of order. `LiveState` and every tool body are unchanged.

### Phase 2 — streams out
- [x] **AC7 — subscribe.** `subscribe` / `unsubscribe` per socket; the reply lists the channels now active. Events are JSON lines with `event` and `t` (the script's monotonic seconds) and are never sent to a socket that did not subscribe.
- [ ] **AC8 — clock.** While the transport runs, a `clock` event at the requested interval (floor: one tick) with bar, beat, beat time, tempo, playing, scene and phrase bar; when it stops, one final event and silence. `live-latency.sh` shows the achieved rate against the requested one.
- [x] **AC9 — levels.** Once a bar, the master and per-track peaks the tick already keeps, in Live's 0–1 meter scale with the scale named (`get_meter_scale`), so the server keeps converting exactly as it does today.
- [x] **AC10 — changes.** A diff of the watched set every N ticks, computed on the main thread by reading properties, with `from` and `to` per path. No listener is registered on any clip, slot or device; `tests/remote_script` proves the watched-set diff and that registering a listener is not in the code path.
- [ ] **AC11 — the readouts come from the stream.** With a subscription active, the `⏱` and `🔊` lines on a reply are built from the latest `clock` and `levels` events and the reply does not perform the reads it performs today; without one, the current path is used. `tests/performance.rs` pins both.
- [ ] **AC12 — the app's beat.** The Mac app subscribes on its Live connection; the Listen screen shows the bar, and the visual's beat comes from `clock` when available and from onset detection otherwise. `app/src/listen.test.mjs` pins the fallback.

### Phase 3 — generic in
- [x] **AC13 — describe.** Any path rooted at `song`, `application` or `browser`, with indexes; the reply is the class, each attribute with its type and read-only flag, the methods, and the Live version. A path off those roots, or through a dunder, is refused with the reason.
- [x] **AC14 — run.** get / set / call / wait_tick in one round trip under the slicer, one undo step per slice, results keyed by `as`, `ops`, `slices` and `main_ms` in the reply. The first failing op stops the batch; the error names the op index and, for a read-only set, says so with the Live version. Same roots and refusals as `describe`.
- [x] **AC15 — the whitelist is a test, not a promise.** `tests/remote_script` drives `run` and `describe` with paths through `os`, `sys`, `__class__`, `__dict__`, `builtins`, a callable that is not a method of a Live object, and an index out of range, and asserts each refusal. The script contains no `eval`, `exec`, `compile`, `__import__` or `open(` on the request path; `tests/local_only.rs` walks the script source for those, the way it walks `src/` for HTTP clients.
- [ ] **AC16 — discovery in Rust.** `src/lom.rs`: a typed path builder, `describe` at handshake for the classes the server uses, a per-Live-version cache under the state dir, and a table of the differences the script's branches encode today. One test per migrated branch; the Python branch is removed in the same commit as the Rust row that replaces it.
- [ ] **AC17 — a capability without a script release.** One real capability the server does not have today is shipped as a `run` sequence with no script change, as proof: proposed `set_clip_color` for Arrangement clips by bar, or the next thing the sample story needs. Its tool checks `require(live, "run")`.

### Phase 4 — cues as ops
- [x] **AC18 — a cue step may be ops.** `schedule_cue` accepts steps as `{at_beat, ops}` alongside today's step forms; the trigger is unchanged and stays on the tick. A `cue` event reports `fired_at_beat` and `late_ms` for every step.
- [ ] **AC19 — lateness is measured and bounded.** `live-latency.sh` schedules 50 harmless cues (a `set` of the master volume to its current value at the next bar, 126 BPM) and prints p50/p95 lateness; target p95 ≤ 1 tick period. `src/transition.rs` composes its primitives into ops when `run` is available and into today's steps otherwise, under `tests/performance.rs`.

### Phase 5 — the migration
- [ ] **AC20 — new things are generic first.** The PR template's checklist gains "shipped as ops, or says why native". The 89 version branches are down to the ones that need a continuation the ops cannot express, each with a comment naming why.
- [ ] **AC21 — the hot paths are measured, not assumed.** `live-latency.sh` compares `get_context` native against the same 413 values as one `run` batch, and the native handler stays while it wins.

### No Regressions
- [x] **AC22:** Every existing tool, test suite and the Docker image behave identically with an old script (protocol 1) loaded; `tests/stdio_integration.rs` runs the handshake against a fake script that lacks the new commands.
- [x] **AC23:** stdout stays pure JSON-RPC; events are consumed by the reader task and never printed. The activity log records command names as before and never event payloads; `tests/activity.rs` pins that an event stream leaves the log unchanged.
- [x] **AC24:** The script binds loopback by default and opens no listener it did not open before; the app opens no socket it did not open before.
- [x] **AC25:** The Remote Script stays compatible with the Python Live bundles: no f-strings, no type hints, no third-party imports, and it loads on a Live 11 with Python 3 and, for the paths it has today, a Live 10 with Python 2.7. A Rust test walks the script source for f-strings and annotations.
- [x] **AC26:** A slice never exceeds today's budgets because of streaming: the diff, the clock event and the socket drain are counted inside the tick's budget and `main_ms` still reports what a command cost.

## Where this stands

Phases 0 to 4 are in (Remote Script 1.31.0, protocol 2). Eighteen of the twenty-six
criteria are ticked above and carry a test; what is **not** done is named here rather than
left to be inferred from an unticked box.

**Measured against Live 12.4.6, macOS 26.6, 2026-09-20** (`scripts/live-latency.sh`):

| | |
|---|---|
| Tick period | 100.0 ms, jitter 9.0 ms, worst 112.0 ms, 181 samples |
| Round trip, no API touch | p50 100.1 ms, p95 109.8 ms (n=40) — was 200 ms |
| Round trip, with an API touch | p50 100.2 ms, p95 110.0 ms (n=40) — was 400 ms |
| An old client (no id, no newline) | p50 100.2 ms — served correctly by 1.31.0 |
| `get_context`, native | 103.4 ms for 209 values, one round trip |
| A 30-op batch across every track | one round trip, 0 ms of Live's main thread |

**What remains.**

- **AC8, AC19 — the rates were measured with the transport stopped.** The clock goes quiet
  when Live is not playing, which is the designed behaviour, so the run reported 0.5 events
  a second against 20 requested and no cue lateness at all. Both numbers need one run with
  the transport rolling before they mean anything. The behaviour itself is covered by
  `tests/remote_script/test_streams.py` and `test_cue_ops.py`.
- **AC11, AC12 — nothing consumes the events yet.** The script publishes on four channels
  and `AbletonConnection` collects them, both under test, but no caller in `src/` or in the
  Mac app calls `subscribe` or `take_events`. The readouts still perform their own reads and
  the app's visual still guesses the beat from audio. The stream is built and proven; it is
  not yet plumbed to anything a producer sees.
- **AC16, AC20 — the 89 version branches are still in Python.** `src/lom.rs` has the typed
  path, the batch and the per-version describe cache, which is the half that had to exist
  first; no branch has moved yet, and the PR checklist has not gained its line.
- **AC17, AC21 — proven, not yet spent.** `cargo run --example new_capability_no_reload`
  reads track output routing and crossfade assignment, neither of which has a handler,
  against an unmodified 1.31.0. No artist-facing capability has been shipped that way yet,
  and the `get_context`-as-ops comparison that would justify replacing a native handler has
  not been run — so every native handler stays.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | tick sampling; `id`/`event` framing; non-blocking reads drained on the tick; `subscribe`/`unsubscribe` and the three channels; `describe`/`run` with the whitelist; `schedule_cue` steps as ops and the `cue` event; `protocol_version` 2; `SCRIPT_CAPABILITIES` and `SCRIPT_VERSION` |
| `src/connection.rs` | reader task, pending-id map, event subscribers, `subscribe`; `RealBridge` unchanged for callers |
| `src/handshake.rs` | `tick`, `socket_reader`, `protocol_version` surfaced; `require()` unchanged |
| `src/performance.rs` | readouts from the latest events when subscribed |
| `src/transition.rs`, `src/sections.rs` | compose into ops when `run` is available |
| `src/app.rs` | `--check` prints protocol, tick, reader; `scripts/live-latency.sh` drives it |
| `src/tools.rs` | `ALL_REMOTE_COMMANDS` gains the four; the one capability of AC17 |
| `app/src-tauri/src/listen/mod.rs`, `app/src/listen.js`, `app/src/visual.html` | the bar from `clock`; the beat from `clock` with onset fallback |
| `docs/architecture/overview.md`, `docs/technical/feature-matrix.md`, `docs/facts/source-of-truth.md` | the protocol, the measured tick, the reader location; the fact snapshot's script version and command count |
| `CLAUDE.md` | the "adding a Remote Script command" rule gains "or ship it as ops" |
| `TERMS.md` | one sentence: events are in memory only, never logged |

### New
| File | Description |
|------|-------------|
| `src/lom.rs` | typed paths, discovery cache, the Live-version table |
| `scripts/live-latency.sh` | the eight measurements, against a running Live, printed as in the prototype |
| `tests/remote_script/` | Python tests that load the script with `_Framework` and `Live` stubbed and a synchronous `schedule_message`: framing, ids, the drain, the whitelist, the diff, the tick sampler; run by `scripts/test-remote-script.sh` and CI's `rust` job |
| `tests/lom.rs` | the discovery cache and the version table |
| `docs/decisions/00NN-…` | the protocol as the script's contract; the trigger stays in Live |

## Remote Script compatibility
Four commands added, one changed (`schedule_cue` accepts a new step form), the handshake extended; checklist per command: handler, name in `SCRIPT_CAPABILITIES`, `SCRIPT_VERSION` bump (major: the protocol changes meaning), entry in `tools::ALL_REMOTE_COMMANDS`, `require()` in every body that uses one. Live floor: the framing and streams work on any Live the script loads on; `describe` and `run` work wherever the path exists and say which Live refused otherwise. Timeout class: `run` is *special* — its budget is the sum of its ops' classes, capped at the longest today (190 s), and the reply reports what ran. Python 2.7 branches kept where the file already has them.

## Privacy
No new data at rest. Events are held in memory by the server's reader task and by the app's Listen session and are discarded as they are consumed; nothing new is written under `state_dir()`. The activity log keeps recording command names and sizes and never records events (AC23). The generic path widens what a client on the loopback socket can reach *inside Live* — to the whole of `song`, `application` and `browser` — and nothing outside it (AC15); the socket is loopback by default ([0003](../../decisions/0003-remote-script-bind-address.md)) and that does not change (AC24). `TERMS.md` gains one sentence saying events are in memory and never stored. Nothing uploads; `tests/local_only.rs` keeps proving no HTTP client exists and gains the walk for `eval`/`exec`/`open(` on the script's request path.

## Test Coverage
| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/remote_script/` (new, Python, stubs for `_Framework` and `Live`) | ids and in-order replies; the drain under a blocked writer; subscribe/unsubscribe and event routing per socket; the watched-set diff with no listeners; every whitelist refusal; the tick sampler; `run` slicing and the per-op error; `wait_tick` | AC3, AC4, AC7, AC10, AC13–15, AC18 |
| `tests/local_only.rs` | the script's request path contains no `eval`/`exec`/`compile`/`__import__`/`open(`; no f-strings or annotations anywhere in the script | AC15, AC25 |
| `src/connection.rs` unit tests + `tests/stdio_integration.rs` | out-of-order replies paired by id; events routed to subscribers; an old script (no ids, no events) served unchanged; the handshake with and without `tick` | AC6, AC22 |
| `tests/performance.rs` | readouts from events when subscribed, from reads when not; transitions as ops when `run` exists, as steps when not | AC11, AC19 |
| `tests/lom.rs` (new) | path builder; cache round-trip; one test per migrated branch | AC16 |
| `tests/activity.rs` | an event stream leaves the log unchanged | AC23 |
| `app/src/listen.test.mjs` | the bar from `clock`; the onset fallback | AC12 |
| `scripts/live-latency.sh` (new) | the eight numbers, before and after each phase, against Live | AC1, AC5, AC8, AC19, AC21 |
| `scripts/check-docs-facts.sh` | the tick period, if quoted anywhere, must match `--check` | AC2 |

## Implementation Notes

### Phases
0. **Measure.** The tick sampler and `live-latency.sh`. Land it, run it, write the number into this story, decide phase 1.
1. **Duplex.** Ids, the reader on the tick, the server's reader task. Nothing else changes; `live-latency.sh` shows the cut.
2. **Streams.** Subscribe, the three channels, readouts from events, the app's bar.
3. **Generic.** `describe`, `run`, the whitelist tests, `src/lom.rs`, one capability shipped without a script release.
4. **Cues as ops.** The new step form, the `cue` event, lateness measured.
5. **Migration.** New things generic first; branches out of Python one at a time; the hot-path comparison.

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| Work on the main thread in slices, `main_ms` on every reply | `_run_on_main`, `Done`, generators | `run` batches, the diff, the drain |
| Deferred completion after Live applies something a tick later | `_create_locator`'s two-tick finish, `DEFERRED` | `wait_tick` |
| Per-bar peaks kept by the tick | `_level_tick`, `_bar_peaks` | the `levels` channel |
| Capability check before every call | `require()` in every body | `run`, `describe`, `subscribe` |
| Unknown handshake fields passed through | `ScriptInfo.extra` | `tick`, `socket_reader` before the struct catches up |
| Source-walk tests for forbidden APIs | `tests/local_only.rs` | `eval`/`exec`/`open(` in the script; f-strings and annotations |
| A live check as a script with a PASS line | `app/src-tauri/examples/listen_probe.rs`, `docker/verify-image.sh` | `scripts/live-latency.sh` |
| Fake bridge answering scripted responses | `tests/common::FakeBridge` | out-of-order ids and events |

### Design Decisions
- **Measure before building.** The 200 ms is Live's thread quantum, measured; the tick period is not. Phase 1 is conditional on a number, and the story says which number.
- **Not a WebSocket.** The latency is where the socket is read, not how it is framed. On loopback, in Python-2.7-compatible code, a WebSocket is cost without benefit. Ids and an `event` field are the whole difference between request-only and duplex.
- **The trigger stays in Live.** Rust plans; the tick fires; the `cue` event closes the loop with a measured lateness. Sending "fire now" over any link is strictly worse than reading the transport on the tick.
- **Generic alongside native, not instead.** A 413-value read as one native handler and as one `run` batch both cost one round trip, but the native one reads a track in one go; keep it while it wins, and say so with the number.
- **The whitelist is the security model.** The socket has no authentication and loopback is the boundary. `run` reaches Live's objects and nothing else, and the refusals are tests.
- **No listeners.** Every observed change is a diff computed on the main thread from reads. The deadlock that removed the passive listeners is the reason.
- **Old clients keep working.** Ids are optional, events are opt-in, capabilities are advertised. A mixed pair degrades to today's behaviour, never to an error.
- **Python tests for the script, at last.** Stubbing `_Framework` and `Live` and making `schedule_message` synchronous turns the protocol, the whitelist and the diff into unit tests that run in CI, which the script has never had.

## Verification

On a Mac with Live 12 open and a set playing at 126 BPM, the script reinstalled and Live restarted, after each phase:

1. `scripts/live-latency.sh` — all eight lines print; compare with the previous phase's output kept beside the story.
2. `ableton-music-maker --check` — `protocol_version`, `tick` and `socket_reader` are present and match the script.
3. Ask Claude *"play the song, go to the drop"* — the `⏱` and `🔊` lines move between replies without a read (watch the activity log: no `get_performance_state` between two consecutive replies while subscribed).
4. Open the Mac app's Listen screen — the bar shown matches Live's; open the visual — its pulse is on Live's bar, and unplugging the subscription (old script) falls back to onset without a visible change in behaviour.
5. Ask for the capability of AC17 — it works, and the activity log shows `run` and no new native command.
6. Load an old script (protocol 1) — every tool works, the readouts appear, `run` is refused with the installer message.
7. Point `run` at `os.environ`, `song.__class__`, and a read-only property — three refusals, each naming the op.
8. Leave the subscription running for an hour with the transport playing — no growth in Live's memory, no slice over budget in Live's log, the tick period unchanged in `--check`.

## Out of Scope
- Replacing the native composite commands with ops (measured, not assumed; AC21).
- A WebSocket, HTTP, or any transport other than newline JSON on the loopback TCP socket.
- Authentication on the socket (a separate decision; loopback is the boundary today).
- Listeners on the Live Object Model of any kind.
- Feeding events to Claude as they happen (the readouts change; the tools' shapes do not).
- Windows.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| The samples story's script changes landing | In flight | this story's phase 0 edits the same file; land after |
| Live 12 on a Mac for `live-latency.sh` | Ready | the numbers in Context were taken on 12.4.6 |
| Python 3 on the CI runner for `tests/remote_script` | Ready | macOS runners carry it; the tests stub Live |
| A decision record for the protocol | Not written | write it when questions 1–4 are answered, with the measured tick |

## Related Stories
- `samples-land-in-the-song` — the next capability to ship as ops instead of a handler (AC17 candidate).
- `app-taps-live-output-spectrum-and-meters` — the visual's beat moves from onset detection to the `clock` stream (AC12).
- `perform-live-build-launch-and-transition-on-the-bar` (shipped, pruned) — the cue engine this story turns into ops with a beat.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-20 | Created from the round-trip measurement (200 ms flat on an open socket, 400 with an API touch, `get_context` 413 values in 400 ms) and the question "surely the timing can be in Rust"; prototype is the wire transcript |
