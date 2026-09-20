# Prototype — Devices and Measurements Claude Can Read Back

Story: [devices-and-measurements-claude-can-read-back](../stories/devices-and-measurements-claude-can-read-back.md)

**The replies below are real.** Every one was taken from a run against Live 12.4.6 with Remote
Script 1.27.2 on 2026-09-20, through the server over stdio. Where a scene changed what was
built, the note says so.

---

## Scene 1 — an effect on the master, and getting back out again

> **Producer:** the mix is flat. put an EQ on the master and high-pass it.

**Claude calls** `load_instrument_or_effect`

```json
{"track_index": 0, "kind": "master", "uri": "query:AudioFx#EQ%20Eight"}
```

```text
Loaded 'EQ Eight' as device 0 on track 0 ('Main'). Devices on the track now: EQ Eight
```

**Claude calls** `adv_get_device_parameters`

```json
{"track": "master", "device": "EQ Eight"}
```

```text
Main · device 0 'EQ Eight' (Eq8) — 84 parameters
  idx  name             now             range
    0  Device On        On              [Off, On]
    1  Output           0.00 dB         -12.0 dB … 12.0 dB
    2  Scale            100 %           -200 % … 200 %
    3  Adaptive Q       On              [Off, On]
    4  1 Filter On A    On              [Off, On]
    5  1 Filter Type A  Low Shelf       [High Pass 48dB, High Pass 12dB, Low Shelf, Bell, Notch, High Shelf, Low Pass 12dB, Low Pass 48dB]
    6  1 Frequency A    30.0 Hz         10.0 Hz … 22.0 kHz
    7  1 Gain A         0.00 dB         -15.0 dB … 15.0 dB
    8  1 Q A            0.71            0.10 … 18.0
    … 84 rows in all
Set one by what it reads: adv_set_device_parameter {"track": "master", "device": "EQ Eight", "parameter": "Device On", "value": "On"} — the raw number works too.
```

The master used to answer `Track index out of range` here, and the labels in the last column
did not exist at all.

---

## Scene 2 — the band that used to be a guess

The failure this replaces: `{"name": "1 Filter Type A", "value": 1, "min": 0, "max": 7}`, a
guess at what `1` means, and a silenced track.

**Claude calls** `adv_set_device_parameter`

```json
{"track": "master", "device": "EQ Eight", "parameter": "1 Filter Type A", "value": "High Pass 48dB"}
```

```text
Main · EQ Eight · 1 Filter Type A: Low Shelf → High Pass 48dB (raw 2 → 0 of 0 … 7).
```

```json
{"track": "master", "device": "EQ Eight", "parameter": "1 Frequency A", "value": "200 Hz"}
```

```text
Main · EQ Eight · 1 Frequency A: 30.0 Hz → 200 Hz (raw 0.143 → 0.389 of 0 … 1).
```

**The refusals.** A label the parameter does not have:

```json
{"track": "master", "device": "EQ Eight", "parameter": "1 Filter Type A", "value": "High Pass"}
```

```text
Could not set device parameter: '1 Filter Type A' takes one of: High Pass 48dB, High Pass 12dB, Low Shelf, Bell, Notch, High Shelf, Low Pass 12dB, Low Pass 48dB
```

…and a value outside the range:

```text
Could not set device parameter: 'Output' runs -12.0 dB to 12.0 dB; '-99 dB' is outside that
```

Nothing is written in either case. **What the run changed:** Live's own labels are
`High Pass 48dB`, not the `Low Cut` this transcript first guessed — which is the whole point
of reading them out of Live rather than writing them down.

---

## Scene 3 — the faders came down; the meters agree

> **Producer:** everything's slamming. pull it down and tell me where we are.

A file that peaks at exactly −12.0 dBFS, played on a track at unity, then cut:

```text
  fader    0 dB (true -12.0 dBFS) -> 4 'ZZ Calib': −12.0 dB
  fader  -12 dB (true -24.0 dBFS) -> 4 'ZZ Calib': −24.0 dB
  fader  -24 dB (true -36.0 dBFS) -> 4 'ZZ Calib': −36.0 dB
```

```text
Peaks are post-fader, in dB on Live's meter scale (−70 dB at the bottom, 0 dB is full scale, +6 dB the top).
```

> **Producer:** what was wrong with the old numbers?

> **Claude:** They were 20·log10 of Live's meter value, and that value is not amplitude — it is
> linear in dB. A 12 dB cut moved the old reading by about 2 dB, which is why you went looking
> for a limiter that was not there.

**What the run changed:** the first implementation converted through Live's *fader taper*,
which read the same 12 dB cut as 6.3 dB. The law was then measured at seven fader positions
over 42 dB — `dB = 76·v − 70`, zero residuals — and that is what ships.

---

## Scene 4 — a capture is the bars you asked for

**Claude calls** `capture_mix`

```json
{"start_bar": 3, "bars": 2, "name": "ZZ tone"}
```

```text
Playhead confirmed at beat 4.18 before recording began.
Captured 'ZZ tone @ bar 3' (Capture track, slot 0) — 2 bars at 120 BPM, 4.0 s, stereo
/Users/…/test Project/Samples/Recorded/Capture 0001 [2026-09-20 083247].wav
peak -12.0 dBFS · RMS -15.0 dBFS · crest 3.0 dB · LF/HF +37.7 dB · stereo correlation 1.00
bands     31    63   125   250   500    1k    2k    4k    8k   16k
          0%    0%    0%  100%    0%    0%    0%    0%    0%    0%
RMS per bar: -15.0 -15.0
Reading: 100 % of the energy sits in one octave around 250 Hz; crest 3.0 dB — squashed, little dynamic range left; level is even across the bars; no clipping; no silent bars; effectively mono.
Play it in Live by firing Capture slot 0; the folder may need its own access grant before the file can be opened. Nothing else in the set changed.
```

Every number is checkable: the source is a 220 Hz sine at −12.0 dBFS, so peak −12.0, RMS 3 dB
under it, crest 3.0 dB, and all the energy in the octave around 250 Hz.

**When the stretch is empty**, the reply says which thing is true:

```text
Nothing plays from 1 for 2 bars: the take is silent end to end. That is the set, not a failed capture. Check that something is placed there.
…
Reading: silent from end to end — nothing was playing.
```

**What the run changed, twice.** The first build reported an empty set as a playhead race
("both takes began before the playhead reached it"), and the analysis described the crest
factor and spectrum of silence. Then the run exposed the real defect under the original bug:
**Live resumes from where it last stopped, so a seek made while the transport is stopped is
ignored.** The capture now seeks again while rolling until Live confirms the playhead is
inside the preroll bar, and only then fires.

---

## Scene 5 — take it back off

```json
{"track": "master", "device": "EQ Eight", "action": "move", "to_index": 1}
```

```text
Moved 'EQ Eight' on Main from 0 to 1. Chain: Limiter, EQ Eight.
```

```json
{"track": "master", "device": "EQ Eight", "action": "bypass"}
```

```text
Bypassed 'EQ Eight' on Main (device 0) — still in the chain, not processing. Turn it back on with {"track": "master", "device": "EQ Eight", "action": "enable"}.
```

```json
{"track": "master", "device": "EQ Eight", "action": "remove"}
```

```text
Removed 'EQ Eight' (was device 0) from Main. Chain: Limiter. Cmd-Z in Live puts it back.
```

And when there is nothing left to name:

```text
Main has no devices
```

**What the run changed:** `move` first reported success without moving anything — Live inserts
*before* the position it is given, counting the device that is still in the chain, so moving
later needs one more. `to_index` now means the index it ends up at.

---

## Scene 6 — the build that died half-way

The first run failed on a bad instrument URI, after the first track existed:

```text
Plan: 2 track(s), 2 clip(s) with 6 notes, 4 placement(s), 0 locator(s), tempo 120.
Tempo 120.
Stopped: Could not create the tracks: after 1 of 2 tracks (ZZ Kick): Browser item with URI 'query:Drums#Drum Rack' not found
What is above is in the set; the rest is not. Re-run the same document — build_song converges: a track whose name already exists is reused, and a slot that already holds the named clip is left alone.
```

The same document, run again:

```text
Track 6 'ZZ Kick' — reused
Track 7 'ZZ Bass'
Clip 'ZZ Four' on track 6 slot 0 (4 notes).
Clip 'ZZ Line' on track 7 slot 0 (2 notes).
2 clips written in one round trip.
Placed track 6 slot 0 at 4 position(s).
Converged: 1 track(s) reused, 0 clip(s) and 0 placement(s) were already there — nothing was duplicated.
```

And a third time, with everything already in place:

```text
Track 6 'ZZ Kick' — reused
Track 7 'ZZ Bass' — reused
Every clip was already in its slot; nothing was written.
Converged: 2 track(s) reused, 2 clip(s) and 4 placement(s) were already there — nothing was duplicated.
```

---

## Scene 7 — the performance nobody is playing

A performance was started, the transport was then stopped from outside the server, and a
minute passed:

```json
{"tempo": 124}
```

```text
A performance was left running from 1 min 5 s ago and Live's transport has been stopped since — I ended it. Set tempo to 124 BPM
```

While it really is playing, the guard still holds:

```text
A performance is running (since bar 1, started 08:35:59, transport playing). stop_playback would cut the audio mid-bar. Use end_performance {"at": "next_bar"} to stop on the bar, end_performance {"fade_bars": 8} to fade the master and stop, or end_performance {"now": true} if you really mean now.
```

---

## Scene 8 — shape_sound says what it did and what it did not

```json
{"track": "master", "device": "EQ Eight", "cutoff": 0.6, "width": 0.3}
```

```text
Main ('EQ Eight', Eq8) — 1 of 2 applied:
  applied  '1 Frequency A' 200 Hz → 1.01 kHz  (cutoff)
  skipped  width — no parameter on this device says it
For width: adv_set_device_parameter takes a name substring; the parameters are: Device On, Output, Scale, Adaptive Q, 1 Filter On A, … (84 in all — adv_get_device_parameters lists them).
Words this device answers to: cutoff (1 Frequency A), resonance (Adaptive Q). Sweep one in a cue: {"ramp": {"sound": "cutoff", "track": "Main", "to": 0.8}, "bars": 8}.
```

The line that matters: **`skipped` is in the reply, and the reply does not open with an
unqualified success.** **What the run changed:** the first build printed all 84 parameter
names; a reply nobody can read is its own kind of silence.

---

## Scene 9 — a return, by its letter

```json
{"track": "A"}
```

```text
A-Reverb · device 0 'Reverb' (Reverb) — 33 parameters
  idx  name            now       range
   11  Diff. Hi Type   Shelf     [Shelf, Lowpass]
   20  Decay Time      2.50 s    200 ms … 60.0 s
   27  Size Smoothing  None      [None, Slow, Fast]
   29  Density         High      [Sparse, Low, Mid, High]
   30  Reflect Level   0.0 dB    -30 dB … 6.0 dB
```

---

## What this transcript decided

1. **One addressing vocabulary.** `track` (name, index, `"master"`, a return's name or letter)
   plus optional `kind` on every device-facing call. Verified on a track, a return and the
   master.
2. **A parameter reads as Live shows it**, and is set the same way — including a chooser's own
   labels, which nobody can guess.
3. **A dB is a dB, because it was measured.** Not derived, not assumed: a file of known level,
   seven fader positions, zero residuals.
4. **A capture is the bars asked for, or an honest statement about the set.** Never a musical
   observation about silence the tool itself caused.
5. **Chains are editable**, so a wrong load is a moment rather than the rest of the session.
6. **A half-applied build names what exists and converges when re-run.**
7. **Refusals say why**, and stale state reconciles itself instead of blocking.
