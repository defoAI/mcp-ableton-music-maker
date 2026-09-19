# The Performance Records Itself as an Arrangement Take

## Story
**As a** producer performing a set through Claude with Live open,
**I want** every performance kept as a take in the Arrangement, and to be asked what to do
when something is already there,
**So that** a set I just played is something I can listen back to, edit and keep — instead of
something that existed only while it was playing, and without the server quietly deciding the
fate of work I already had.

## Details
| Field | Value |
|-------|-------|
| Status | `Draft` |
| Priority | P1 — the performance layer shipped without it, so every set played through this product so far has been lost the moment it ended |
| Size | M |
| Tracker | — |
| Created | 2026-09-19 |
| Updated | 2026-09-19 |

## Context

### The Problem
A producer plays a whole set through `play_song`, steers it with `go` and `jump_to`, ends it
with `end_performance` — and Live is exactly as it was before. The performance left no trace.
There is nothing to listen back to, nothing to edit, nothing to keep. The one thing a
performance is *for* is the thing the product throws away.

The moment it *is* recorded, a second problem appears: the Arrangement usually already has
something in it. Whatever the server does with that material — append after it, record over
it — is a decision about the producer's work, and the server does not get to make it quietly.

### Current State
The performance path launches Session clips and nothing else. `play_song_body`
(`src/sections.rs:985`) fires the first entry's scene (`src/sections.rs:1028`) and hands the
counted jumps to the script as one cue (`src/sections.rs:970`); each steering verb is one
state read and one `schedule_cue` (`src/sections.rs:1179`). `start_performance_body`
(`src/tools.rs:4671`) sets the launch quantization, turns on performance mode and fires.

Live writes Session launches into the Arrangement only while its global Arrangement Record is
on — `Song.record_mode`. The Remote Script never touches it: `record_mode`, `session_record`
and `back_to_arranger` appear nowhere in `AbletonMusicMaker_Remote_Script/__init__.py`, and no
`start_arrangement_record` is in `SCRIPT_CAPABILITIES`. So no tool can reach the switch.

What the server keeps of a performance is only its own cursor — which setlist entry, the plan
cue id, the jump history — and even that is re-derived from the row that plays
(`docs/architecture/overview.md:103`). Nothing is written to the set.

Three facts make this cheaper and safer to fix than it looks:

- **Tracks are already disarmed.** `disarm` defaults to yes (`src/tools.rs:3595`) and
  `play_song` passes it (`src/tools.rs:4721`). Arrangement Record's other behaviour —
  recording armed tracks' *inputs* — is therefore already neutralised for the default path.
- **The server can already see what the Arrangement holds.** `_get_arrangement_clips` returns
  `start_time`, `end_time` and `length` per clip
  (`AbletonMusicMaker_Remote_Script/__init__.py:1348`), used today by `arrange`
  (`src/arrange.rs:400`), and `delete_arrangement_clips` already exists
  (`src/arrange.rs:524`).
- **The playhead and Back to Arrangement are already commands.** `set_current_song_time`
  (`:116`, handler `:1334`) and `back_to_arrangement` (`:143`, handler `:2427`).

### Root Cause
The performance layer was designed around "Claude plans, Live executes" — cues on Live's own
clock — and recording was never part of that loop. It was written down separately, as a
producer-facing take tool (`record_arrangement`, AC9 of
[song-writing-feel-notes-groove-quantize-record-undo](song-writing-feel-notes-groove-quantize-record-undo.md)),
which is a different feature: that one records *the producer playing*, at a bar they choose.
Nobody joined the two, so performing and recording stayed unrelated.

## Open Questions

1. **What happens when the Arrangement already has material?** — **Answered: ask the
   producer.** The server states what is there and offers `after`, `replace` or `off`, and
   starts nothing until it has an answer. It never picks for them, and never overwrites
   except on an explicit `replace`.
2. **Is recording the default?** — **Answered: yes**, as `record: "ask"`. An empty
   Arrangement records from bar 1 with no question, because there is nothing to decide.
3. **Where does an appended take start?** — **Answered:** the next bar line at or after the
   largest `end_time` across *all* tracks — one shared start bar, so the take is aligned and
   reads as one performance.
4. **How long is the answer remembered?** — **Proposed:** `after` and `off` for the rest of
   the session, so the second performance does not ask again; `replace` never, because the
   material it would delete is different every time. Needs agreement.
5. **Is the question worth a round trip before any sound?** — Open (prototype Q6). The
   alternative is to append quietly the first time and only ask when the producer has already
   recorded a take into this set.
6. **Should `replace` delete before playing, or at `end_performance`?** — Open (prototype Q7).
   Deleting first means a performance stopped in its first bar has destroyed the old
   arrangement and replaced it with one bar.
7. **A locator per section as it fires?** — Open. It makes a take navigable, but it is one
   `create_locator` per section launch on Live's main thread mid-performance (decision 0007,
   issue #43). Alternatives: one locator at the take's start, or all of them written at
   `end_performance` from the jump history the server already holds.
8. **A blank bar between takes?** — Open. The prototype appends with no gap.
9. **Does `end_performance` always call `back_to_arrangement`?** — Open. Proposed: only when
   it recorded, so a dry run leaves the producer's override state alone.
10. **Do the cue ramps belong in the take?** — Open. Live writes tempo, crossfader and volume
    ramps as automation while recording. Proposed: keep them; they are part of the
    performance.
11. **Should `undo` ship first?** — **Answered: no.** `undo` is not in `SCRIPT_CAPABILITIES`
    today. It was a blocker only while the design overwrote by default; with `replace` being
    something the producer says out loud, after being told what it deletes, Live's own Cmd+Z
    is the honest answer — and the reply says so.
12. **One command for what the Arrangement holds, or the existing per-track loop?** —
    **Answered:** one command. See Design Decisions.

## Prototype

[performance-records-itself-as-an-arrangement-take.md](../prototypes/performance-records-itself-as-an-arrangement-take.md)
— nine scenes: the question when the Arrangement is not empty and the producer's answer,
steering mid-take, what `end_performance` reports, a second take that does not ask again,
`replace` and what it says about undo, opting out, an empty Arrangement, Live 10, and an
armed track the producer kept. Not yet reviewed.

Writing the replies out is what produced open questions 5–10: the cost of the question before
any sound, the locator cost, the take-to-take gap, and the fact that a `replace` which deletes
before playing can be stopped in its first bar. None of those were visible from the tool
signature.

## Tool description

No new tool — decision 0006: a new capability becomes a parameter on the artist tool that owns
the intent. `play_song` and `start_performance` gain one parameter each, described to the
model as:

```text
record: What to do with the Arrangement while this performance plays (default: "ask").
"ask" records the take from bar 1 when the Arrangement is empty, and when it is not, plays
nothing and returns what is there with the choices below, for the producer to pick.
"after" records the take after everything already in the Arrangement, starting at the next
bar line, touching nothing that exists. "replace" deletes the existing Arrangement clips and
records the take from bar 1 — only ever on the producer's explicit word; there is no undo in
this server, so the reply names how many bars it deleted and says Cmd+Z in Live is the way
back. "off" plays without recording. The answer is remembered for the rest of the session,
except "replace", which is asked every time. Live's API cannot save the set — the producer
presses Cmd+S. Needs Live 11 or newer; on Live 10 the server cannot see what the Arrangement
holds, so it does not record and the reply says so.
```

`end_performance`'s description gains:

```text
Stops Arrangement Record, reports the take (the bars it covers and the tracks it touched),
and puts the tracks back on the timeline with Back to Arrangement.
```

## Acceptance Criteria

### Asking, when there is something to ask about
- [ ] **AC1:** With `record: "ask"` (the default) and a non-empty Arrangement, **no command
      that starts or changes anything is sent**: the tool returns the question, naming how
      many bars are there and on how many tracks, with `after`, `replace` and `off` as the
      choices. The performance does not start.
- [ ] **AC2:** With `record: "ask"` and an empty Arrangement, the take records from bar 1 with
      no question, and the reply says the Arrangement was empty and how to opt out.
- [ ] **AC3:** `after`, `replace` and `off` each do what they say, and `off` sends no recording
      command at all.
- [ ] **AC4:** The answer is remembered for the rest of the session, so a second performance
      does not ask again — except `replace`, which is never remembered (Q4).
- [ ] **AC5:** The question is the same whether it came from `play_song` or
      `start_performance`.

### Where the take lands
- [ ] **AC6:** `after` starts at the next bar line at or after the largest `end_time` across
      all tracks, at the set's beats per bar; every track records from that same bar.
- [ ] **AC7:** With `after`, no existing Arrangement clip on any track is shortened, moved or
      deleted. This is the criterion the tests are hardest on.
- [ ] **AC8:** `replace` deletes the existing Arrangement clips and records from bar 1, and the
      reply says how many bars on how many tracks it deleted and that Cmd+Z in Live is the only
      way back. It is reachable **only** by the producer answering it — never a default, never
      inferred.
- [ ] **AC9:** Before the first scene fires, the server sets the playhead to the take's start
      bar and turns on `record_mode`, in that order, so the first bar of the take is the first
      bar played.

### What the producer is told
- [ ] **AC10:** The reply that starts a performance says whether it is recording and from which
      bar, and that the set still has to be saved by hand.
- [ ] **AC11:** `end_performance` turns `record_mode` off on every path it has (on the bar,
      faded, `now` — `src/tools.rs:5231`) and reports the bars the take covers and how many
      tracks it touched.
- [ ] **AC12:** `get_performance_state`'s readout says a take is recording and from which bar,
      so a fresh conversation can tell.

### Refusals and compatibility
- [ ] **AC13:** On a Live without `track.arrangement_clips` (Live 10), nothing is recorded, no
      question is asked, and the reply says why. The performance runs exactly as it does today.
- [ ] **AC14:** With `disarm: false`, the reply names each armed track and says Arrangement
      Record captures its input as audio — the one case where a take contains something other
      than what the clips played.
- [ ] **AC15:** If the Arrangement read fails, the performance still starts, without recording,
      and says so. A performance is never blocked by the take.
- [ ] **AC16:** `record_mode` is off whenever no performance is running — including after a
      failure between arming and firing.

### No Regressions
- [ ] **AC17:** With `record: "off"` every existing performance test passes unchanged: the same
      commands, in the same order, from every performance tool.
- [ ] **AC18:** The guards still hold (`src/tools.rs:4872`) — a running performance still
      refuses `stop_playback`, playhead moves, tempo jumps and deleting what plays. The take's
      playhead move happens before the performance starts, which is why it is allowed.
- [ ] **AC19:** Steering verbs send no extra recording command; the take keeps rolling.
- [ ] **AC20:** No new tool is served; the tool count is unchanged (decision 0006).
- [ ] **AC21:** `capture_mix` and `record_clip` behave exactly as they do today.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | new handlers `arrangement_summary`, `start_arrangement_record`, `stop_arrangement_record`; the performance tick turns `record_mode` off when the transport stops; `SCRIPT_CAPABILITIES`; `SCRIPT_VERSION` 1.24.0 → 1.25.0 |
| `src/tools.rs` | `record` on `start_performance`; `end_performance` stops the take and reports it; `ALL_REMOTE_COMMANDS` |
| `src/sections.rs` | `record` on `play_song`; the question when the Arrangement is not empty; the take's start computed before the first fire; the reply lines |
| `src/performance.rs` | the take in the performance record (start bar, whether recording) and the session's remembered answer; the start-bar arithmetic — pure, so unit-tested |
| `src/context.rs` | the PERFORMING paragraph (`:25`) says a set is kept as a take, and that the producer is asked when the Arrangement is not empty |
| `docs/architecture/overview.md`, `docs/technical/feature-matrix.md` | the performance section gains the take |
| `docs/facts/source-of-truth.md` | `SCRIPT_VERSION` and the command count in the snapshot |

### New
| File | Description |
|------|-------------|
| — | no new source file; the logic belongs in the performance modules that already exist |

## Remote Script compatibility

- [ ] Handlers added to `AbletonMusicMaker_Remote_Script/__init__.py` — no f-strings, no type
      hints, no third-party imports
- [ ] `arrangement_summary`, `start_arrangement_record`, `stop_arrangement_record` added to
      `SCRIPT_CAPABILITIES`
- [ ] `SCRIPT_VERSION` bumped 1.24.0 → 1.25.0 (`AbletonMusicMaker_Remote_Script/__init__.py:63`)
- [ ] All three added to `tools::ALL_REMOTE_COMMANDS`
- [ ] Every call guarded by `require(live, …)`, so an older script degrades to today's
      behaviour rather than erroring
- [ ] Live version floor: `arrangement_summary` needs `track.arrangement_clips` (Live 11+).
      `record_mode` itself is older, but without knowing what the Arrangement holds there is
      neither a safe start bar nor an honest question, so the whole feature is Live 11+ and
      AC13 is what Live 10 gets
- [ ] `arrangement_summary` iterates every track: a generator that yields between tracks
      (decision 0007), read timeout (10 s). The two record commands are modifying (15 s,
      `MODIFYING_COMMANDS`)

## Privacy

No new local data. The take is written into the producer's own Live set, not to disk by the
server, and the set still has to be saved by hand (`src/context.rs:17`). No new file, no new
state directory entry, nothing leaves the machine. `TERMS.md` is unchanged — but note this is
the first feature that **writes to the producer's set as a side effect of another verb**,
which is why AC1 asks before anything happens and why `replace` is reachable only by name.

## Test Coverage

| Suite / script | Change | AC |
|----------------|--------|----|
| `tests/performance.rs` | a non-empty Arrangement with `record: "ask"` sends **no** command after the read and returns the question; an empty one records from bar 1; each of `after`, `replace`, `off`; the remembered answer on a second performance and `replace` not remembered; the command order (read → playhead → record on → fire); `end_performance` stopping on all three paths; the Live 10 path; the armed-track sentence; a failure between arming and firing leaves `record_mode` off | AC1–AC5, AC9, AC11, AC13–AC16 |
| `src/performance.rs` unit tests | the start bar from a scripted set of arrangement ends: empty → bar 1, ragged ends → the largest, a non-bar-aligned end → the next bar line, several beats-per-bar | AC6 |
| `tests/arrangement.rs` | `after` sends no command that shortens, moves or deletes an existing Arrangement clip; `replace` sends `delete_arrangement_clips` and nothing else destructive | AC7, AC8 |
| `src/tools.rs` unit tests | the tool count is unchanged; `record` appears on both tools' schemas with its four values | AC20 |

## Implementation Notes

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| A refusal that names the alternative in the message | the performance guards, `src/tools.rs:3830` | the question: it is a reply the model can act on, not an error |
| One state read, then one command | the steering verbs, `src/sections.rs:1179` | the read and the arm, before the first fire |
| Pure bar arithmetic, unit-tested on synthetic state | `src/performance.rs` | the take's start bar |
| `require(live, cmd)` before the bridge | every tool body | degrade to today's behaviour on an older script |
| Generator handlers that yield per track | `_get_arrangement_clips` | `arrangement_summary` |
| The producer's units | decision 0006 | the take is reported in 1-based bars, never beats |

### Design Decisions
**Why ask instead of choosing.** Appending is the safe choice, and the first draft of this
story made it the silent default. But "safe" is not the same as "what the producer wanted":
a producer who has a rough arrangement and plays a better take usually wants the take to
*be* the arrangement. Guessing either way is the server deciding the fate of their work.
Asking costs one round trip before any sound — the performance has not started, so nothing
is interrupted — and it is the only point in the product where the server is about to write
into material the producer made by hand.

**Why `replace` is offered at all.** Hiding it would not stop anyone wanting it; it would
make them do it by hand, or make Claude improvise a `delete` before performing. Offering it
by name, with the bar count in the question and Cmd+Z named in the reply, is informed
consent. It is never a default and is never remembered.

**Why a new `arrangement_summary` command instead of the existing per-track loop.** `arrange`
reads one track at a time (`src/arrange.rs:400`). Doing that for every track before a
performance starts is N round trips and N passes over Live's main thread, right when the
producer is waiting to hear the first bar — and issue #43 is open about exactly that cost.
One command that walks the tracks in a generator and returns the end bar, the bar count and
the track count is cheaper, and it is what the question needs anyway.

**Why not put it in `get_performance_state`.** That is read by every steering verb, many times
a performance. Making it iterate every track's Arrangement clips would put the cost on every
`go`. It is needed once, before the first fire.

**Why the default is on.** A performance that leaves no trace is the problem this story exists
for; a default of off would leave it unsolved for everyone who does not know the parameter
exists.

## Verification

### Manual verification steps
With Live 12 open, the Remote Script reinstalled, Live restarted, and a set whose Arrangement
already has material:

1. "play the song" → **nothing plays**; the reply names the bars that are there and the three
   choices. Live is untouched.
2. "after them" → the take records from the bar after the existing material; clips appear on
   the timeline as each section fires. **The material before the take is byte-identical**
   (compare clip start/end times before and after).
3. "that's it" → `end_performance` reports the bars; Live's record button goes out; the tracks
   follow the timeline again.
4. Straight away, "play it again" → no question, and the second take starts after the first.
5. "play it and replace the arrangement" → the reply says how many bars it deleted; the take
   is at bar 1; Cmd+Z in Live restores the old material.
6. A set with an empty Arrangement → no question, take from bar 1.
7. "play it but don't record" → nothing is added.
8. Stop Live's transport by hand mid-take → Live's record button goes out.
9. A set with an armed track and `disarm: false` → the reply names the track; that track's
   take is the audio input, as it says.

## Out of Scope
- **Recording the producer playing.** That is `record_arrangement` / `overdub_clip` in
  [song-writing-feel-notes-groove-quantize-record-undo](song-writing-feel-notes-groove-quantize-record-undo.md)
  — a take the producer plays, at a bar they choose. This story is only about the performance
  the server itself drives.
- **`undo`.** It stays in the other story (Q11).
- **Recording at a bar the producer names.** The choices are `after`, `replace` and `off`; a
  chosen bar is the other story's feature.
- **Consolidating or exporting a take**, and anything that saves the set — the Live API cannot
  save.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| Live 11+ `track.arrangement_clips` | available | the floor for the whole feature; Live 10 gets AC13 |
| `set_current_song_time`, `back_to_arrangement`, `delete_arrangement_clips` | shipped | `AbletonMusicMaker_Remote_Script/__init__.py:116`, `:143`; `src/arrange.rs:524` |
| Issue #43 (main-thread cost) | open | the reason the read is one command, and the reason Q7's locators are a question |

## Related Stories
- `song-writing-feel-notes-groove-quantize-record-undo` — its AC9/AC10 record the *producer*;
  this records the *performance*. They share the script's record commands and must not specify
  them differently.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-19 | Created |
