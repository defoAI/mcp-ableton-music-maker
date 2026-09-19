# 0007 — The Remote Script touches Live only from Live's main thread, in bounded slices, and reports the cost

| | |
|---|---|
| **Status** | **Decided — one executor on Live's main thread; long work sliced across ticks; one undo step per mutating command; `main_ms` on every reply** |
| **Raised** | 2026-09-19 (clicks and pops in the output while Claude worked during a performance, issue #43) |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | Every Remote Script handler: where it runs, how long it may hold Live, and what it reports |

## Context

During a live set the output clicked whenever Claude was working and stopped when the
commands stopped. The activity log showed nothing long: batches of twenty to forty short
writes, a quarter second each. The script at that point ran writes on Live's main thread
through `schedule_message(0)` and reads — session state, clip notes, the performance state,
the browser walk, the clock stamp on every reply — on its own socket thread, straight into
the Live API. The browser walk held that thread for up to eight seconds in one block.

Live's Python API is not thread-safe, and Live's audio engine takes the document lock that
every write takes. Nothing in the code said which of these produced the clicks, and nothing
measured what a command cost Live: the log had the round trip, not the time inside Live.

This is a product for live acts. The bar is that nothing Claude does can be heard.

## Options

- **A. Keep the split; tune the offenders.** Cheap. Against: the off-thread reads stay a race
  with Live's own thread (a crash on stage), and without a measurement every fix is a guess.
- **B. One executor on the main thread, sliced, measured.** The decision. Every command runs
  inside one task scheduled on Live's main thread; a handler with a lot to do is a generator
  the executor resumes on the next tick once a slice budget is spent; every mutating command
  is one undo step; every reply carries the main-thread time and the slice count, and the
  server writes them to the activity line.
- **C. Move the work out of the script.** The server would do more and the script less.
  Against: the Live API is only reachable from inside Live; the work that clicks is the Live
  API work itself.

## Decision

- **One thread.** The socket thread parses JSON and waits on a queue. Everything that reads
  or writes Live runs in `_run_on_main` → `_dispatch` on Live's main thread. The handshake
  (`get_script_info`) is the only command answered off-thread: it touches nothing in Live.
- **Bounded slices.** A handler that has many units of work is a generator (`yield None`
  between units, `yield Done(result)` last): `write_clips`, `create_tracks`, `place_clips`,
  `delete_arrangement_clips`, `duplicate_arrangement_clip`, `list_captures`, `search_browser`
  and the browser paging. The executor runs it for `SLICE_MS_PLAYING` (8 ms) per tick while
  the transport runs, `SLICE_MS_STOPPED` (40 ms) otherwise, and re-arms with
  `schedule_message(1)`. The browser walk's wall budget is capped at two seconds while
  playing; the server keeps what each call returned.
- **One undo step per mutating command** (`begin_undo_step` / `end_undo_step` around each
  slice), from `MUTATING_COMMANDS` in the script.
- **Cost is reported.** Every reply carries `main_ms` and `slices`; the server's trace sums
  them into the activity line (`main_ms`, `slices`). A slice over `SLOW_SLICE_MS` (25 ms) is
  written to Live's log with the command name. Nothing else is logged per command.
- **The clock stamp** on every reply is computed inside the task, on the main thread.

## Consequences

- The activity line answers "what did this call cost Live" per tool; issue #43 is closed on
  that evidence, not on a listening test alone. A regression is a number.
- A sliced command takes longer in wall time while the music plays (twenty clips ≈ two
  seconds) and holds Live for at most 8 ms at a time. The timeouts in
  `src/connection.rs` and the script's `COMMAND_TIMEOUTS` cover it.
- A new handler that loops over tracks, clips or browser items is written as a generator
  from the start; a plain function is for one unit of work.
- What remains after this is Live's own cost of a mutation (a device load, a new track): the
  tool says so in its reply when the transport was running, and the instructions say to
  build before the show.
