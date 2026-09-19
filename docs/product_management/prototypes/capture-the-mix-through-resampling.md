# Prototype transcript — capture the mix through resampling

Fake data, real tool names and parameter names. Reviewed 2026-09-19.

## 1. The happy path: capture the drop, read it, fix it, capture again

**Producer:** capture the drop (bars 33 to 40) and tell me how it sits.

**Claude calls** `capture_mix` `{"start": 128, "bars": 8, "name": "drop"}`

**Result:**
```
Captured 'drop @ 128' (Capture track, slot 0) — 8 bars at 128 BPM, 15.0 s, stereo WAV
/Users/nick/Music/Ableton/Techno Sketch Project/Samples/Recorded/Capture 0001 [2026-09-19 141203].wav
peak -0.9 dBFS · RMS -11.4 dBFS · stereo correlation 0.62
RMS per bar: -13.8 -13.6 -13.5 -13.7 -9.9 -9.8 -9.7 -9.8
Reading: bars 5–8 are 3.8 dB louder than bars 1–4; no clipping; no silent bars.
Play it in Live by firing Capture slot 0. Nothing else in the set changed.
```

**Claude:** The second half of the drop is 3.8 dB louder than the first, which is the bass
entering at bar 37 rather than the drums. Peak is at −0.9 dBFS with no clipping, so there
is headroom to spare but not much. Want the bass to come in at bar 33 instead, or keep the
lift and pull it back 2 dB?

**Producer:** pull the bass back 2 dB.

**Claude calls** `set_track_mixer` `{"track_index": 2, "volume": 0.79}` then
`capture_mix` `{"start": 128, "bars": 8, "name": "drop v2"}`

**Result (second capture):**
```
Captured 'drop v2 @ 128' (Capture track, slot 1) — 8 bars at 128 BPM, 15.0 s, stereo WAV
… peak -2.7 dBFS · RMS -12.6 dBFS
RMS per bar: -13.8 -13.6 -13.5 -13.7 -11.7 -11.6 -11.5 -11.6
Reading: bars 5–8 are 2.0 dB louder than bars 1–4 (was 3.8); no clipping.
```

**Claude:** Now the lift is 2 dB. Both captures are in the Capture track, slots 0 and 1,
if you want to A/B them.

## 2. The set is not saved yet

**Result:**
```
Captured 'intro @ 0' (Capture track, slot 0) — 8 bars … 
This set is unsaved, so Live recorded into a temporary project folder; the file moves when
you save the set. Measurements are unaffected.
```

## 3. Someone deleted the Capture track by hand

`capture_mix` recreates it (`ensure_capture_track` is idempotent) and says so:
```
Created the Capture track at index 7 (audio, input Resampling, monitoring off, muted, armed).
Captured 'verse @ 64' …
```

## 4. A second capture while one is recording

```
A capture is already recording ('drop @ 128', slot 0, 6 s left). Wait for it, or call
stop_capture.
```

## 5. Live Intro, no audio track left

```
Could not create the Capture track: Live refused to add an audio track (track limit
reached in this edition). Free a track and try again; nothing in the set changed.
```

## 6. The server dies mid-capture

Live finishes the fixed-length recording on its own. The next call from any server runs
`stop_capture`, which stops the transport; `list_captures` shows the clip with its file.

## 7. `list_captures` and `measure_capture`

```
Captures on the Capture track (2):
  slot 0  drop @ 128     8 bars  peak -0.9 dBFS  RMS -11.4  …/Capture 0001 […].wav
  slot 1  drop v2 @ 128  8 bars  peak -2.7 dBFS  RMS -12.6  …/Capture 0002 […].wav
```

`measure_capture` `{"slot": 0}` re-reads slot 0's file and prints the same block as a
capture, without playing anything.

## What the review changed
- Per-bar RMS replaced a single RMS: the sentence the loop needs is "bars 5–8 are louder".
- The Capture track is muted so firing a capture by hand does not double the mix.
- The unsaved-set note is a sentence in the result, not an error.
