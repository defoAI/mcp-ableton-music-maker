<!-- Required sections: Story · Details · Context · Open Questions · Prototype ·
     Acceptance Criteria (+ No Regressions) · Affected Files · Remote Script compatibility ·
     Privacy · Test Coverage · Implementation Notes · Verification · Out of Scope ·
     Dependencies · Related Stories · Changelog. -->

# Devices and Measurements Claude Can Read Back

## Story
**As a** producer using Claude Desktop / Claude Code / Cursor with Live open,
**I want** everything Claude changes in my set to be something Claude can then read, correct
and remove — and every number it reports back to me to be true,
**So that** a wrong move is a moment rather than the rest of the session, and the mix
decisions we make together are made on real measurements.

## Details
| Field | Value |
|-------|-------|
| Status | `Done` |
| Priority | P0 — the loop the whole product rests on is open: Claude writes, then cannot read back. Two of these (master devices, parameter units) make an agent destructive by accident |
| Size | L — ten groups; see **Shipping order**, each group is one GitHub issue |
| Tracker | — (to be filed: one issue per group, `fix(#N):` commits) |
| Created | 2026-09-20 |
| Updated | 2026-09-20 |

## Context

### The Problem
A producer ran a full session through the MCP and came back with nine defects. They share one
spine: **the server can act on the set but cannot faithfully read it back.** A device goes on
the master and can never be touched again. A parameter is a bare float, so setting an EQ band
is a coin flip — one wrong guess silenced a track. Meters are labelled dB and are not dB, so
a 12 dB fader move reads as 2 dB and sends the producer hunting a limiter that does not
exist. A capture returns digital silence and the tool reports it as a musical observation,
invalidating an A/B test. A half-finished `build_song` leaves the set in a state nothing can
resume from.

Every one of those is the same failure in a different place: an agent that cannot verify its
own work will confidently report success it did not achieve.

Two patterns were called out as right, and both are the model for the rest:
`adv_get_drum_rack_pads` returns pad names *and* the pitch numbers needed to act on them
([`__init__.py:2125`](../../../AbletonMusicMaker_Remote_Script/__init__.py)), so writing a
drum pattern that hits the intended sounds is one call; and `shape_sound`'s macro-first
resolution with its "words this device answers to" reply
([`src/tools.rs:1152`](../../../src/tools.rs)).

### Current State
Every claim here was read in the code on 2026-09-20; the producer's session supplied the
symptoms.

**1 — Devices on the master and returns are write-only.**
`load_instrument_or_effect` takes `kind: "track" | "return" | "master"`
([`src/tools.rs:499`](../../../src/tools.rs)) and the script resolves it through
`_resolve_track(track_index, kind)`, which already handles the master
([`__init__.py:1904`](../../../AbletonMusicMaker_Remote_Script/__init__.py),
used by `_load_browser_item` at [`__init__.py:3496`](../../../AbletonMusicMaker_Remote_Script/__init__.py)).
The readers and writers do not: `DeviceParams` and `SetDeviceParameterParams` are
`track_index: i64` only ([`src/tools.rs:349`](../../../src/tools.rs),
[`src/tools.rs:355`](../../../src/tools.rs)), and the handlers index `self._song.tracks`
directly — `_get_device_parameters`
([`__init__.py:5832`](../../../AbletonMusicMaker_Remote_Script/__init__.py)),
`_set_device_parameter` ([`__init__.py:5924`](../../../AbletonMusicMaker_Remote_Script/__init__.py)),
`_set_device_parameters` ([`__init__.py:5951`](../../../AbletonMusicMaker_Remote_Script/__init__.py)),
`_get_track_info` ([`__init__.py:935`](../../../AbletonMusicMaker_Remote_Script/__init__.py)) —
so the master raises `IndexError("Track index out of range")`. `shape_sound` resolves its
track through `PerfState::track_by` ([`src/performance.rs:273`](../../../src/performance.rs)),
which searches `self.tracks` only — hence "no track named 'Master'". `_get_drum_rack_pads`
calls `_resolve_track` but never passes a `kind`
([`__init__.py:2128`](../../../AbletonMusicMaker_Remote_Script/__init__.py)).

**2 — Parameter values have no units, no display strings, no enum labels.**
`_serialize_device` emits `value_string` only `if hasattr(param, "value_string")`
([`__init__.py:5568`](../../../AbletonMusicMaker_Remote_Script/__init__.py)). The Live Object
Model lists no `value_string` property on `DeviceParameter` — it lists `str_for_value(value)`
and, for quantized parameters, `value_items`
([`live-object-model.md:1830`](../../reference/ableton/live-object-model.md),
[`:1909`](../../reference/ableton/live-object-model.md),
[`:1922`](../../reference/ableton/live-object-model.md)). So on Live 12 the `hasattr` is
false and the reply is `{index, name, value, min, max, is_enabled, is_quantized}` — exactly
what the producer saw. `value_items` is never read at all, which is why `Filter Type = 1` had
to be guessed. The server side is already waiting for the string: `sound::Param` carries
`value_string` and `display()` falls back to a percentage without it
([`src/sound.rs:20`](../../../src/sound.rs), [`:54`](../../../src/sound.rs)) — and
`_volume_db` proves `str_for_value` works in this script
([`__init__.py:1920`](../../../AbletonMusicMaker_Remote_Script/__init__.py)).
`adv_get_device_parameters` pretty-prints the raw JSON with no rendering at all
([`src/tools.rs:1047`](../../../src/tools.rs)).

**3 — "dB" from the meters is not dB.**
Live's `output_meter_left/right/level` is documented as "smoothed momentary peak value …
0.0 to 1.0 … corresponds to the meters shown in Live"
([`live-object-model.md:947`](../../reference/ableton/live-object-model.md)) — Live's *meter
scale*, which is the fader taper, not linear amplitude. Both sides convert it with
`20·log10`: `song::meter_db` ([`src/song.rs:528`](../../../src/song.rs)) and the script's
`_db` ([`__init__.py:4293`](../../../AbletonMusicMaker_Remote_Script/__init__.py)). That
conversion is used in the `listen` readout ([`src/tools.rs:4105`](../../../src/tools.rs)),
`play_and_measure` ([`src/tools.rs:3202`](../../../src/tools.rs)), the meters readout
([`src/tools.rs:3093`](../../../src/tools.rs)), the performance level line
([`src/performance.rs:633`](../../../src/performance.rs)) and the section readout
([`src/sections.rs:1322`](../../../src/sections.rs)). Nothing anywhere says pre- or
post-fader. The arithmetic matches the producer's report: on Live's taper 0.85 reads 0.0 dB
and roughly 0.65 reads −12.0 dB, and `20·log10(0.65/0.85) = −2.3 dB` — the ~2 dB they saw
after a 12 dB cut. The same reasoning says the meters *are* post-fader, which is the thing
to confirm first.

**4 — `capture_mix` has a transport race.**
`_start_capture` sets `song.current_song_time = start − preroll` (1 beat under 160 BPM, else
2), then fires the slot from a `schedule_message(2, finish)` callback with
`launch_quantization = q_quarter`
([`__init__.py:2972`](../../../AbletonMusicMaker_Remote_Script/__init__.py)). Nothing waits
for Live to confirm the playhead arrived, and the quantized fire can land on a beat before
`start`. When it does, the WAV opens with silence, and `audio::reading` narrates it as music:
"bars 3–4 are 49.8 dB louder than bars 1–2" ([`src/audio.rs:378`](../../../src/audio.rs)),
because `capture_mix_body` measures whatever came back without ever asking whether the take is
valid ([`src/tools.rs:3344`](../../../src/tools.rs)).

**5 — The analysis reports the wrong things, and the file is hard to reach.**
`Measurements` is peak, RMS, RMS per bar, silent bars, clipped samples, stereo correlation
([`src/audio.rs:274`](../../../src/audio.rs)) — none of which catches a mix with 78 % of its
energy under 120 Hz. `band_balance` (low/mid/high) already exists
([`src/audio.rs:585`](../../../src/audio.rs)) and is wired into `listen` only
([`src/tools.rs:4024`](../../../src/tools.rs)), not into `capture_mix` or `measure_capture`.
There is no crest factor and no LF/HF ratio. The WAV lands wherever Live recorded it, and
`capture_text` only remarks on it when the set is unsaved
([`src/tools.rs:3335`](../../../src/tools.rs)) — in a client sandbox that folder needs its own
access grant before the file can be opened.

**6 — No device delete, bypass or reorder; effects only append.**
`SCRIPT_CAPABILITIES` has no device-chain command beyond load and parameter writes
([`__init__.py:88`](../../../AbletonMusicMaker_Remote_Script/__init__.py)), and
`_load_browser_item` selects the *last* device before loading so an effect always appends
([`__init__.py:3510`](../../../AbletonMusicMaker_Remote_Script/__init__.py)). Live offers both
missing verbs: `Track.delete_device(index)`
([`live-object-model.md:1019`](../../reference/ableton/live-object-model.md)) and
`Song.move_device(device, target, position)`
([`live-object-model.md:562`](../../reference/ableton/live-object-model.md)). Bypass is
already reachable as a parameter — every device's parameter 0 is `Device On` — but only on a
track the parameter tools can address, which is defect 1 again.

**7 — `build_song` half-applies.**
It validates the entire document before sending anything ([`src/tools.rs:5839`](../../../src/tools.rs)),
then executes "stopping at the first failure" with no rollback: the failure message is the
transcript so far plus `Stopped: …`. `create_tracks` always creates, so re-running the same
document duplicates every track that was made before the failure.

**8 — Stale performance state blocks work.**
`Performance` lives in a mutex in the server process
([`src/connection.rs:148`](../../../src/connection.rs)); `guard_performance` refuses
transport-touching tools while it is `Some`, whatever Live's transport is actually doing
([`src/tools.rs:3865`](../../../src/tools.rs), callers at
[`:1720`](../../../src/tools.rs) `set_tempo`, [`:1917`](../../../src/tools.rs)
`stop_playback`, [`:3345`](../../../src/tools.rs) `capture_mix` and three more). An MCP server
started by Claude Desktop outlives conversations, so a performance from this morning is still
"running" tonight — the producer's "running since bar 1, 36698 s".

**9 — Errors do not say why.**
`delete_track_body` maps any Live exception through `live_err`
([`src/tools.rs:3528`](../../../src/tools.rs), [`:999`](../../../src/tools.rs)) and the script
re-raises Live's own text ([`__init__.py:2871`](../../../AbletonMusicMaker_Remote_Script/__init__.py)),
so deleting the only track in a set produces Live's bare "Couldn't delete track" with nothing
about the rule that caused it.

**10 — `shape_sound` reports partial success as success.**
Unresolved words are appended as prose after the success sentence
([`src/tools.rs:1275`](../../../src/tools.rs)); the reply's first line reads as if everything
asked for was applied.

### Root Cause
The surface was built write-first. Each tool was added at the moment a producer needed to
*change* something, and the read path was whatever the Live API returned with no rendering
(`pretty(&r)`) — or, for meters, a plausible formula nobody calibrated. Where Live's API needed
a second call to be legible (`str_for_value`, `value_items`) or a second verb to be reversible
(`delete_device`, `move_device`), the work stopped at the first call.

## Open Questions

Answers marked **Proposed** are the story author's recommendation and need the producer's yes
before the acceptance criteria are final.

**1. One addressing vocabulary, or `kind` everywhere?** → **Proposed: both, and they are the
same thing.** Every device-facing tool takes `track` (a name, an index, `"master"`, or a
return's name/letter) the way `shape_sound` already does, *and* keeps `track_index` + `kind`
for callers that have indices. `kind` stays the disambiguator when a return and a track share
a name. Rejected: a separate `adv_master_device_parameters` family — two vocabularies for one
concept is how this hole opened.

**2. Does a parameter value get set by its display string?** → **Proposed: yes, and the raw
number keeps working.** `"3 dB"`, `"200 Hz"`, `"Low Cut 48 dB"` and `0.542` all set
`1 Frequency A`. Strings resolve by asking Live (`str_for_value` bisection for continuous
parameters — the script already does this for volume at
[`__init__.py:1937`](../../../AbletonMusicMaker_Remote_Script/__init__.py) — and exact label
match for quantized ones). Every reply states both forms.

**3. Meters: fix the curve, or return the raw value?** → **Built: fixed, through Live's own
curve, with the raw value as the honest fallback.** `get_meter_scale` asks the master volume
parameter for `str_for_value` at 101 points and hands the taper to the server, which caches it
for the session (`song::MeterScale`); the script converts its own `peak_db` through the same
table. Where Live will not answer, the readouts say "meter units" and call nothing dB
(`tests/orchestration.rs::without_lives_curve_a_meter_reading_is_not_called_db`). Originally
proposed as:**fix it, through Live's own curve, and never invent one.** Convert a meter value with `str_for_value` on that track's
volume parameter (meters and faders share Live's scale), fall back to the raw 0–1 labelled
"Live meter units" when Live will not answer. **This must be measured before it is shipped** —
see Verification step 3. If the measurement says meters and faders do *not* share a taper, the
fallback becomes the behaviour: raw value, named as such, curve documented, and no "dB" in the
text.

**4. Pre- or post-fader?** → **Answered by measurement: post-fader.** A −12.0 dBFS file read
−12.0 dB with the fader at unity and −24.0 dB with the fader at −12, so the meter follows the
fader. Said in every meter readout.

**5. What does `capture_mix` do when a take starts early?** → **Proposed: discard and retry
once, then fail.** Never measure a take with leading silence; never let `audio::reading`
describe it. The retry and the reason are named in the reply.

**6. Where do captures land?** → **Proposed: leave Live's file where Live put it, and make
the analysis rich enough that the file is usually unnecessary.** The path stays in the reply.
A copy under `state_dir()/captures/` is a *new local data capture* and would need a TERMS
change, so it ships only if the producer asks, behind an explicit `export_capture`, on the
`export_set` pattern ([`src/sets.rs`](../../../src/sets.rs)).

**7. Which band set?** → **Proposed: ten octave bands from 31 Hz to 16 kHz**, plus crest
factor (peak − RMS) and one LF/HF ratio (< 200 Hz vs > 4 kHz, in dB). Ten numbers a producer
reads at a glance; the existing three-band `band_balance` becomes a sum over them so `listen`
keeps its shape.

**8. Do chain edits belong on the artist surface?** → **Proposed: advanced, one tool,
actions.** `adv_edit_devices {track, kind?, device, action: "remove" | "move" | "bypass" |
"enable", to_index?}` — following `arrange`'s action shape rather than four verbs. Per
[decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md) a new
*intent* may become a tool; "undo what you loaded" is one. If producers reach for it by voice
("take the reverb off the master") it joins Shape later, and the PR that does that says so.

**9. Inserting before an existing device?** → **Proposed: yes, as `load_instrument_or_effect
{at_index}`,** implemented as load-then-`move_device`, because Live's browser always loads at
the selection. One undo step in the tool's reply, two in Live's history — say so.

**10. How does `build_song` converge?** → **Built: idempotent by name, not transactional, and
the reuse decision is Live's.** `create_tracks` takes `on_existing`, so the set's own state is
read inside the same round trip — convergence costs nothing when there is nothing to converge
on. Clip slots and Arrangement beats are checked only for the tracks Live reports as reused.
Originally proposed as: **idempotent by name, not transactional.**
A track whose name already exists with the document's instrument is reused; a clip slot that
already holds the named clip of the right length is left alone; placements are re-placed only
where absent. `on_existing: "converge" | "add" | "fail"` with `converge` the default. Rejected:
rolling back on failure — deleting a producer's tracks to clean up after a dropped socket is
worse than the half-built state.

**11. When is a performance stale?** → **Proposed: reconcile, do not time out.** On every
`guard_performance` call, if Live's transport is stopped *and* no cue is pending *and* the
performance has been running longer than one minute, end it and say so in the reply of the
tool that was being called. A clock-only timeout would end a real performance that is between
sections.

**12. Should every Live error be translated?** → **Proposed: no — the known ones, by rule.**
A table of pre-checks in the tool body (last track, frozen track, no audio input, master
cannot be deleted), each saying the rule and the way round. Unknown Live text still passes
through verbatim, prefixed with the command that produced it.

**13. Structured or prose replies?** → **Proposed: prose that names every outcome, not JSON.**
Tool replies are text the model reads; the defect is not the format but the missing
`skipped` line. `shape_sound` gains `applied` / `skipped` lines per word, and a reply with any
`skipped` line never opens with an unqualified success sentence.

## Prototype

[`prototypes/devices-and-measurements-claude-can-read-back.md`](../prototypes/devices-and-measurements-claude-can-read-back.md)
— nine scenes: an effect on the master and back off again, the EQ band that silenced a track,
the 12 dB fader move the meters agree with, a capture that is the bars asked for (and the
retry, and the failure), chain edits, a build that converges on re-run, a stale performance, a
refusal that says why, and `shape_sound` naming what it skipped.

**Every reply in the transcript is real**, taken from a run against Live 12.4.6 with Remote
Script 1.27.2 on 2026-09-20.

What the transcript changed before the build: `adv_get_device_parameters` was going to return
JSON with a `display` field added. Written out as a transcript it was unreadable — a model
reading 84 EQ Eight parameters as JSON objects is worse off than one reading an aligned table.
The readout became a table with the chooser's items inline. And the capture failure text
originally kept the measurement "for information"; it now refuses to measure at all, because a
number offered alongside "this take is invalid" is a number that ends up in a conclusion.

What the *run* changed is in **Verification** below: three defects that only appeared against
Live, including the real cause of the capture race.

## Tool description

The `#[tool]` doc comments as the model will read them. Unchanged tools are omitted.

```text
adv_get_device_parameters
Every parameter of one device, as Live shows it: the display string ("200 Hz", "Low Cut
48 dB"), the range, and for a switch or a chooser the list of values it accepts. `track` is a
name, an index, "master", or a return's name or letter; `device` is a name (a substring) or an
index. Set one with adv_set_device_parameter, which takes the same addressing and accepts
either the display string or the raw number.
```

```text
adv_set_device_parameter
Set one device parameter. The value may be what Live displays ("3 dB", "200 Hz", "Low Cut
48 dB") or the raw number between min and max; the reply gives both, before and after. `track`
is a name, an index, "master", or a return's name or letter. A chooser whose label you give is
matched exactly: a label the parameter does not have is refused with the list of the ones it
has, and nothing changes.
```

```text
adv_edit_devices
Remove, move, bypass or re-enable a device in a track's chain — including the master's and a
return's. action: "remove" takes it out (Cmd-Z in Live puts it back), "move" needs to_index,
"bypass" leaves it in the chain and stops it processing, "enable" turns it back on. `device`
is a name (a substring) or an index; the reply lists the chain afterwards.
```

```text
capture_mix
Record a stretch of the arrangement through the master and measure it: peak, RMS, crest
factor, ten octave bands, the low-to-high ratio and RMS per bar, with a reading you can act
on. The playhead is confirmed at the starting bar before recording begins; a take that starts
early is discarded and recorded again rather than measured. Live saves the audio in the set's
own Samples/Recorded folder and the reply gives the path. The set is not saved — Live's API
cannot save it.
```

```text
shape_sound
… (existing text) … The reply names every word applied and every word skipped, with the
parameter each one moved and its value before and after.
```

## Acceptance Criteria

### A — Address every device the way the loader already does
- [x] **AC1:** `adv_get_device_parameters`, `adv_set_device_parameter`, the batched
  `set_device_parameters`, `adv_get_track_info` and `adv_get_drum_rack_pads` accept
  `track` (name, index, `"master"`, return name or letter) and `track_index` + `kind`, and
  reach the master and every return.
- [x] **AC2:** `shape_sound {"track": "master"}` and `{"track": "A"}` resolve; the "no track
  named X" error lists the returns and the master alongside the tracks.
- [x] **AC3:** A device loaded with `load_instrument_or_effect {"kind": "master"}` is readable
  and settable by the same call that the reply suggests, in the same session, with no further
  parameters.

### B — A parameter reads like Live shows it
- [x] **AC4:** Every parameter in `adv_get_device_parameters` carries Live's display string for
  its current value, its range as display strings, and `value_items` when it is quantized.
- [x] **AC5:** The readout is an aligned table (index, name, value, range or items) headed by
  the track, device, class and parameter count — not pretty-printed JSON.
- [x] **AC6:** `adv_set_device_parameter` accepts a display string or a raw number, and the
  reply states both, before and after.
- [x] **AC7:** A display string the parameter cannot take is refused with the list of accepted
  values, and nothing is written.
- [x] **AC8:** A Live version that answers neither `str_for_value` nor `value_items` degrades
  to today's numbers with one line saying the display strings are unavailable — no tool fails.

### C — dB means dB, and says where it was measured
- [x] **AC9:** One conversion, in one place, from Live's meter units to dB, through Live's own
  curve; `song::meter_db`'s 20·log10 is gone from the server and `_db` from the script.
- [x] **AC10:** A fader move of N dB moves the reported peak of that track by N ± 0.5 dB,
  measured on a real Live (Verification step 3).
- [x] **AC11:** Every meter readout says post-fader or pre-fader — `listen`, `play_and_measure`,
  the meters readout, the performance level line, the section readout.
- [x] **AC12:** Superseded by the measurement: the law is known (`dB = 76·v − 70`), so a
  reading is always real dB — including against a Remote Script that predates
  `get_meter_scale`, which uses the same law in `song::meter_db`. No readout prints "meter
  units"; every one says post-fader.

### D — A capture is the bars you asked for
- [x] **AC13:** Recording starts only after Live confirms the playhead is at or after the
  requested bar; the reply states the confirmed position.
- [x] **AC14:** A take whose head is silent while the rest is not is discarded and re-recorded
  once; the reply says it happened. A take that is silent *end to end* is the set's own state,
  reported as such with no retry (found by the run).
- [x] **AC15:** Two bad takes produce an *error*, with no measurements and no reading, and the
  slot left clean.
- [x] **AC16:** `audio::reading` never describes leading silence as a musical difference
  between halves.

### E — A capture is measured like a mix
- [x] **AC17:** `capture_mix` and `adv_measure_capture` report crest factor, ten octave bands
  (31 Hz … 16 kHz) and the LF/HF ratio in dB, alongside today's numbers.
- [x] **AC18:** The reading names a spectral problem when there is one ("78 % of the energy is
  below 125 Hz") rather than only "level is even".
- [x] **AC19:** `listen`'s existing low/mid/high line still reads the same, computed from the
  new bands.
- [x] **AC20:** The reply always gives the WAV's path, and says in one line that the folder may
  need an access grant in the client.

### F — A device can be removed, moved and bypassed
- [x] **AC21:** `adv_edit_devices` removes, moves, bypasses and re-enables a device on a track,
  a return or the master, and lists the chain afterwards.
- [ ] **AC22:** `load_instrument_or_effect {"at_index": N}` — **not built.** Live's browser
  loads at the selection, so this is load-then-`move_device`; with `adv_edit_devices move`
  shipped and verified, inserting before an existing device is two calls rather than one
  parameter. Left for the producer to ask for: see Out of Scope.
- [x] **AC23:** Removing a device Live will not remove (a frozen track's) says so, and nothing
  changes.

### G — A build converges
- [x] **AC24:** `build_song` run twice with the same document produces one set of tracks, clips,
  placements and locators — nothing duplicated.
- [x] **AC25:** A failure names what exists and what does not, and says the document can be
  re-run.
- [x] **AC26:** `on_existing: "fail"` restores today's behaviour for callers that want it.

### H — Stale performance state does not block work
- [x] **AC27:** A performance whose transport has been stopped for over a minute with no pending
  cue is ended by the next tool that would have been blocked, and that tool then runs; the
  reply says the performance was ended and why.
- [x] **AC28:** A performance with the transport playing, or with a cue pending, still blocks —
  the existing refusal text is unchanged.
- [x] **AC29:** `adv_get_performance_state` says how long a stale performance has been running
  and that the next tool needing the transport will end it.

### I — Errors say why
- [x] **AC30:** `delete_track` on the last track refuses with Live's rule in words, and nothing
  changes. (Built in the Remote Script's own pre-check, so it costs no extra state read; the
  server also translates an older script's bare "Couldn't delete track".)
- [x] **AC31:** Pre-checks and their messages exist for the last track and a frozen track
  (`_delete_track`), and for a frozen track's devices (`_delete_device`, `_move_device`). The
  master cannot be reached by `delete_track` at all — it takes a song-track index — and arming
  an audio track with no input is Live's own refusal, passed through.
- [x] **AC32:** An error Live raises that is not in the table is still passed through verbatim,
  prefixed with the command that produced it.

### J — Partial success is not success
- [x] **AC33:** `shape_sound` names every word applied and every word skipped on its own line.
- [x] **AC34:** A reply containing a skipped word does not open with an unqualified success
  sentence.

### No Regressions
- [x] **AC35:** Every `#[tool]` doc comment stays in the artist's units — dB for levels, Live's
  1-based bars for the Arrangement, beats inside a clip — and says "section", not "scene",
  outside `adv_` tools ([decision 0006](../../decisions/0006-one-artist-surface-raw-layer-marked-advanced.md)).
- [x] **AC36:** `CORE_TOOLS` gains nothing; `adv_edit_devices` is served as `adv_`, and
  `tests/artist.rs` is updated to pin it there.
- [x] **AC37:** No tool writes anything new to disk (see Privacy), and `tests/local_only.rs`
  and `tests/activity.rs` pass untouched.
- [x] **AC38:** No command loops over tracks, clips or devices in one main-thread slice: the
  chain walks are generators yielding between units, and every new command's `main_ms` stays
  under 25 ms ([decision 0007](../../decisions/0007-live-main-thread-only-in-slices.md)).
- [x] **AC39:** The Remote Script stays Live-Python compatible: no f-strings, no type hints, no
  third-party imports.

## Affected Files

### Modified
| File | Change |
|------|--------|
| `AbletonMusicMaker_Remote_Script/__init__.py` | `kind` through `_get_device_parameters`, `_set_device_parameter`, `_set_device_parameters`, `_get_track_info`, `_get_drum_rack_pads`; `_serialize_device` gains display strings via `str_for_value`, `value_items`, `default_value`; new `delete_device`, `move_device`, `meter_db_of` handlers; `_start_capture` confirms the playhead before firing; `_db` removed in favour of Live's curve; `SCRIPT_CAPABILITIES`, `SCRIPT_VERSION` |
| `src/tools.rs` | `DeviceParams` / `SetDeviceParameterParams` gain `track` + `kind`; device readout renderer; `adv_edit_devices` body + `#[tool]`; `capture_mix_body` validity check and retry; `build_song_body` convergence; `guard_performance` reconciliation; `delete_track_body` pre-check; `shape_sound_body` applied/skipped; `ALL_REMOTE_COMMANDS` |
| `src/sound.rs` | `Param` gains `value_items`, `display_range()`; value-by-label resolution |
| `src/audio.rs` | `Measurements` gains `crest_db`, `octaves`, `lf_hf_db`; `band_balance` computed from the octave bands; `reading()` names spectral problems and never narrates leading silence |
| `src/song.rs` | `meter_db` replaced by the calibrated conversion (or removed if the script converts) |
| `src/performance.rs` | `PerfState` carries returns and the master for `track_by`; transport-stopped duration; level line labelled post-fader |
| `src/connection.rs` | `delete_device`, `move_device` in `MODIFYING_COMMANDS` |
| `src/sections.rs` | section readout meter labels |
| `docs/architecture/overview.md` | the meter conversion and where it is done; the capture handshake |
| `docs/technical/feature-matrix.md` | device chain editing; the capture analysis |
| `docs/facts/source-of-truth.md` | tool count, command count, `SCRIPT_VERSION`, re-dated snapshot |
| `tests/…` | see Test Coverage |

### New
| File | Description |
|------|-------------|
| — | No new source file; every change extends an existing module |

## Remote Script compatibility

- [ ] Handlers added to `AbletonMusicMaker_Remote_Script/__init__.py`: `delete_device`,
      `move_device` — no f-strings, no type hints, no third-party imports; Python 2.7 branches
      kept where the file already has them
- [ ] `kind` accepted (defaulting to `"track"`) by `get_device_parameters`,
      `set_device_parameter`, `set_device_parameters`, `get_track_info`, `get_drum_rack_pads`
      — additive, so an older server still works against a newer script
- [ ] Command names added to `SCRIPT_CAPABILITIES`
- [x] `SCRIPT_VERSION` bumped to `1.27.1` (from `1.26.0` — source-of-truth snapshot, verified
      2026-09-20). The installer embeds the script with `include_str!`, so the binary is rebuilt
      before installing; the version bump is what makes a stale loaded copy visible in `--check`.
- [ ] Commands added to `tools::ALL_REMOTE_COMMANDS` (a test cross-checks the two lists)
- [ ] Tool bodies call `require(live, "<command>")` before the bridge; the device readout
      degrades gracefully when a script predates the display strings
- [ ] Live version floor: `delete_device` and `move_device` are Live 9+ in the LOM and need no
      branch; `str_for_value` and `value_items` are in every supported Live. A parameter that
      raises on either is reported with numbers only
- [ ] Timeout class: `delete_device`, `move_device` are modifying (15 s, add to
      `MODIFYING_COMMANDS`); the enriched `get_device_parameters` stays a read (10 s) and must
      stay under it for a 54-parameter device — measure, and page if it does not

## Privacy

**No new local data.** Nothing here records, stores or uploads anything the server does not
already write. Captures stay where Live writes them; the server does not copy them. The
activity log gains nothing beyond the tool names and timings it already records
(payloads remain off by default, [`src/activity.rs`](../../../src/activity.rs), pinned by
[`tests/activity.rs`](../../../tests/activity.rs)).

If Open Question 6 is answered the other way and captures are copied under
`state_dir()/captures/`, that is a new data capture and this section must gain: the `TERMS.md`
change, the default (written only on an explicit `export_capture` call), and the test in
`tests/local_only.rs` that pins it — on the `export_set` pattern in
[`src/sets.rs`](../../../src/sets.rs) / [`tests/sets.rs`](../../../tests/sets.rs).

## Test Coverage

| Suite / script | Change | AC |
|----------------|--------|----|
| `src/tools.rs` unit tests | tool count and schema defaults for `adv_edit_devices`; `track`/`kind` parsing on device params; `delete_track` pre-check message | AC1, AC21, AC30, AC36 |
| `src/sound.rs` unit tests | value-by-label resolution; `display_range`; refusal listing the items | AC6, AC7 |
| `src/audio.rs` unit tests | crest factor on a known signal; octave bands on a 60 Hz tone (energy in band 1) and a 10 kHz tone; `reading()` on a take with a silent first bar | AC16, AC17, AC18 |
| `tests/orchestration.rs` | device parameters through `run()` with `FakeBridge` for track, return and master; the readout table; `build_song` run twice converges; `on_existing: "fail"` | AC1, AC4, AC5, AC24, AC26 |
| `tests/mixer.rs` | `adv_edit_devices` remove / move / bypass / enable; chain listed afterwards; meter conversion against a fixture table | AC9, AC21, AC22 |
| `tests/capture.rs` | playhead confirmation before firing; one retry on a leading-silence take; error after two; no measurements in the error | AC13, AC14, AC15 |
| `tests/sound.rs` | `shape_sound` applied/skipped lines; no unqualified success with a skipped word; `track: "master"` | AC2, AC33, AC34 |
| `tests/performance.rs` | stale performance ended by a blocked tool; a playing performance still blocks; transport-stopped duration in the state readout | AC27, AC28, AC29 |
| `tests/artist.rs` | `adv_edit_devices` served as `adv_`, absent from `CORE_TOOLS`; `batch` accepts either spelling | AC36 |
| `tests/activity.rs`, `tests/local_only.rs` | unchanged and passing | AC37 |
| `scripts/check-docs-facts.sh` | snapshot re-dated with the new tool count, command count and `SCRIPT_VERSION` | — |

## Implementation Notes

### Patterns to Follow
| Pattern | Where Used | Reuse For |
|---------|-----------|-----------|
| `_resolve_track(track_index, kind)` | [`__init__.py:1904`](../../../AbletonMusicMaker_Remote_Script/__init__.py) | Every device handler — the helper already exists; this is mostly deletion of duplicated indexing |
| `str_for_value` bisection | `_value_for_db` [`__init__.py:1937`](../../../AbletonMusicMaker_Remote_Script/__init__.py) | Setting a continuous parameter from a display string, and converting a meter value to dB |
| Pads return names *and* pitches | `_get_drum_rack_pads` [`__init__.py:2125`](../../../AbletonMusicMaker_Remote_Script/__init__.py) | The device readout: return what is needed to act, not what the API happened to hold |
| Actions on one tool | `arrange` [`src/arrange.rs`](../../../src/arrange.rs) | `adv_edit_devices` |
| Generator handlers, `yield None` between units | decision 0007, `_list_captures` [`__init__.py:3116`](../../../AbletonMusicMaker_Remote_Script/__init__.py) | Walking a 54-parameter device and a chain |
| Written only on an explicit call | `export_set` [`src/sets.rs`](../../../src/sets.rs) | `export_capture`, if Open Question 6 goes that way |

### Design Decisions
**Why not roll back a failed `build_song`?** Deleting tracks a producer can see, to tidy up
after a dropped socket, risks destroying work that is not ours. Converging on re-run gets the
same end state with no destructive step.

**Why not a `get_master_device_parameters`?** Decision 0006: one vocabulary. The master is a
track you address as `"master"`, everywhere or nowhere.

**Why keep prose replies instead of returning JSON?** The reader is a model reading text in a
tool result, and the repo's replies are written for that reader. The defect was a missing
line, not the format. A structured field would still have been summarised as "done".

**Why measure the meter curve rather than derive it?** Because the current bug is exactly a
derived curve nobody measured. The fix is a number taken from a running Live with a known
signal, and a test fixture that pins it.

**Why `value_items` instead of a table of Live's enums?** The repo has
`docs/reference/ableton/live-python-enum-members.md`, but a table goes stale per Live version
and per device; `value_items` is what the installed Live says today.

## Verification

Run against **Live 12.4.6, Remote Script 1.27.2, macOS**, on 2026-09-20, driving the real
server over stdio. Work was done in tracks created for the test and deleted afterwards; the
set was left as it was found (four empty tracks, empty master chain, tempo 120).

### What was verified

| # | Check | Result |
|---|---|---|
| 1 | The master is no longer write-only | `adv_get_track_info {track: "master"}` reads the Main track; EQ Eight loaded, read back (84 parameters), set, bypassed, moved, removed |
| 2 | A parameter reads as Live shows it | `1 Frequency A 30.0 Hz (10.0 Hz … 22.0 kHz)`, `1 Filter Type A Low Shelf [High Pass 48dB, High Pass 12dB, Low Shelf, Bell, Notch, High Shelf, Low Pass 12dB, Low Pass 48dB]` — Live's labels, not ours |
| 3 | A value is set by its display string | `"High Pass 48dB"` → raw 0; `"200 Hz"` → raw 0.389; `"1.5 kHz"` → reads back `1.50 kHz` |
| 4 | A wrong label changes nothing | `"High Pass"` → refused with the eight labels; `"-99 dB"` → refused with `-12.0 dB to 12.0 dB` |
| 5 | **The meter law** | see the table below |
| 6 | A capture is the bars asked for | `Playhead confirmed at beat 4.18 before recording began`, then a clean take of bars 3–4 |
| 7 | The analysis is right | a 220 Hz sine at −12.0 dBFS measured `peak -12.0 dBFS · RMS -15.0 dBFS · crest 3.0 dB`, 100 % of the energy in the 250 Hz octave |
| 8 | A silent stretch is not narrated as music | `silent from end to end — nothing was playing` |
| 9 | Chain edits | move 0→1 and back, bypass, enable, remove, and `Main has no devices` when the chain is empty |
| 10 | `build_song` converges | a real half-applied failure, then two re-runs: `2 track(s) reused, 2 clip(s) and 4 placement(s) were already there — nothing was duplicated` |
| 11 | A stale performance | ended by `set_tempo` after the transport had been stopped for a minute; a playing performance still refuses |
| 12 | Returns by letter | `adv_get_device_parameters {track: "A"}` reads A-Reverb's Reverb |
| 13 | Cost on Live's main thread | see the table below |

### The meter measurement — the number this story rests on

A file peaking at exactly −12.0 dBFS, played on one track, read at seven fader positions:

| Fader | True level | Live's meter value | `(dB + 70) / 76` |
|---|---|---|---|
| +6 dB | −6.0 dBFS | 0.84209 | 0.84211 |
| 0 dB | −12.0 dBFS | 0.76314 | 0.76316 |
| −6 dB | −18.0 dBFS | 0.68419 | 0.68421 |
| −12 dB | −24.0 dBFS | 0.60524 | 0.60526 |
| −18 dB | −30.0 dBFS | 0.52630 | 0.52632 |
| −24 dB | −36.0 dBFS | 0.44735 | 0.44737 |
| −36 dB | −48.0 dBFS | 0.28945 | 0.28947 |

A straight-line fit gives **dB = 76 · value − 70** with zero residuals over 42 dB: Live's meter
is **linear in dB**, −70 dB at 0.0 and +6 dB at 1.0, so 0 dBFS sits at 0.921. It is neither
amplitude (20·log10, the old code: a 12 dB cut read as ~2 dB) nor the fader taper (the first
implementation here: the same cut read as 6.3 dB). After the fix the readings are exact:

```text
  fader    0 dB (true -12.0 dBFS) -> 4 'ZZ Calib': −12.0 dB
  fader  -12 dB (true -24.0 dBFS) -> 4 'ZZ Calib': −24.0 dB
  fader  -24 dB (true -36.0 dBFS) -> 4 'ZZ Calib': −36.0 dB
```

### What it costs Live's main thread (decision 0007)

`main_ms` per command, read from the Remote Script's own reply:

| Command | main_ms |
|---|---|
| `get_device_parameters` — 84 parameters with every display string, range and label | **2.51** |
| `set_device_parameter` — raw number | **0.64** |
| `set_device_parameter` — display string (`"200 Hz"`, a 48-step bisection over `str_for_value`) | **0.68** |
| `set_device_parameter` — chooser label (`"Bell"`) | **0.73** |

Live answers `str_for_value` in about ten microseconds, so reading what Live shows is free at
this scale. Nothing here approaches the 25 ms slow-slice threshold, and the parameter walk
yields between parameters anyway.

### The three defects only the run could find

1. **The capture race had a deeper cause.** Live resumes from where it last *stopped*: a seek
   made while the transport is stopped is discarded by `continue_playing`. Confirming the
   playhead was not enough — the script now seeks again while rolling until Live reports the
   playhead inside the preroll bar. `_play_from` had the same latent bug and got the same fix.
2. **`move_device` did not move.** Live inserts *before* the position it is given, counting the
   device that is still in the chain, so `to_index` one later than the device's own index was a
   no-op that reported success. `to_index` now means the index it ends up at.
3. **A silent stretch was reported as a playhead race**, and then analysed as music (crest
   factor and spectral balance of digital silence). A take that is silent end to end is now
   named as the set's own state, with no retry.

Two smaller ones: a reply listing all 84 parameter names, and Live padding some display
strings (`"0.50  "`), both fixed.

### How to re-run it

- `python3 scripts/check-script-helpers.py` — the script's display-string, meter and
  convergence logic against a stub Live, with no Live needed.
- `cargo test` — 20 suites.
- Against Live: install (`cargo build --bins` **first** — the installer embeds the script with
  `include_str!`), restart Live, check `--check` says `up_to_date: true`, then drive the server
  over stdio. The meter calibration needs a file of known level; generate a −12 dBFS sine and
  read `adv_get_track_meters` at several fader positions.

## Out of Scope
- **`load_instrument_or_effect {at_index}`** (AC22). Live's browser loads at the selection, so
  inserting before an existing device is load-then-move; `adv_edit_devices {"action": "move"}`
  ships and does the second half. One parameter can join the loader when a producer asks for
  it.
- **The library index caps at 2,000 items and reports itself complete.** Found while running
  this story: `search_browser "saturator"` answers "nothing matches" from the index while
  `adv_get_library_status` lists Saturator as installed, because the index stops at 2,000 and
  shadows Live's own browser. Nothing to do with this story's defects — it deserves its own
  issue against `src/library.rs`.
- **Automating an Arrangement clip's device parameters.** Live's API cannot; it is said once,
  in the instructions and in the one tool concerned, and stays that way.
- **Saving the set.** Same reason.
- **Rack chain devices addressed individually** (the Operator inside a drum pad). The snapshot
  already walks them ([`_serialize_chains`](../../../AbletonMusicMaker_Remote_Script/__init__.py));
  addressing one for a write is its own story.
- **A spectrum over time.** The Mac app's process tap already draws that
  ([`app-taps-live-output-spectrum-and-meters`](app-taps-live-output-spectrum-and-meters.md));
  this story measures a captured file, not a stream.
- **Loudness (LUFS) and true-peak.** Worth having, not needed to answer "why is the low end
  heavy"; a later story if a producer asks.
- **Undo across tools.** `feel` owns the one-undo pattern; chain edits rely on Live's own undo.

## Dependencies
| Dependency | Status | Notes |
|------------|--------|-------|
| A running Live 12 with the script installed | Available | Verification steps 1–9 cannot be done against `FakeBridge` |
| Answers to Open Questions 3, 4 (the meter curve) | **Blocking group C only** | Groups A, B, D–J can ship without them |
| Producer's yes on Open Questions 1, 2, 8, 10 | **Blocking the ACs** | Story stays `Draft` until then |

## Related Stories
- [`song-writing-feel-notes-groove-quantize-record-undo`](song-writing-feel-notes-groove-quantize-record-undo.md)
  — owns `shape_sound` and the one-undo pattern this story extends.
- [`app-taps-live-output-spectrum-and-meters`](app-taps-live-output-spectrum-and-meters.md)
  — the app's own metering; the dB question is the same question and the two must agree.
- [`performance-records-itself-as-an-arrangement-take`](performance-records-itself-as-an-arrangement-take.md)
  — owns the performance state this story reconciles.

## Shipping order

Ten groups, each one GitHub issue, `fix(#N):` commits, in this order. A, B and F are one
coherent change to the device layer and should land together; the rest are independent.

| Order | Group | Issue title |
|---|---|---|
| 1 | A | Device tools cannot address the master or the returns |
| 2 | B | Device parameters have no units, display strings or enum labels |
| 3 | F | No device delete, bypass or reorder; effects only append |
| 4 | D | capture_mix records before the playhead arrives |
| 5 | E | Capture analysis misses spectral balance and crest factor |
| 6 | C | Meter readings labelled dB are not dB, and do not say pre- or post-fader |
| 7 | H | A stale performance blocks work for the rest of the session |
| 8 | G | build_song half-applies and cannot be resumed |
| 9 | I | Errors do not say why (delete_track on the last track) |
| 10 | J | shape_sound reports partial success as success |

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-20 | Created from a producer's session report (nine defects); prototype transcript written |
| 2026-09-20 | Built and verified against Live 12.4.6 (Remote Script 1.27.2). All ten groups ship except `load_instrument_or_effect {at_index}` (AC22, see Out of Scope). The meter curve was measured rather than derived — `dB = 76·v − 70`, zero residuals over 42 dB — after the first implementation (Live's fader taper) proved wrong on the bench. The run also found the real cause of the capture race (Live discards a seek made while the transport is stopped), a `move_device` that reported success without moving, and a silent stretch reported as a playhead race. Status `Done` |
