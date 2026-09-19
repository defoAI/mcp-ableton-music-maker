# Prototype transcript — song-writing feel: expressive notes, groove, quantize, capture, record, sample edits, undo

Fake data, real tool names and parameter names. Written 2026-09-19, not yet reviewed.

The producer here is writing a track, not performing one. They play parts in, tidy them,
make loops breathe, tune samples, and want "undo that" to mean what it means in Live. The
performance layer is not running in any of these scenes unless said so.

Bar numbers are Live's own (1-based). Beats inside a clip are 0-based, as Live's API counts.

---

## 1. A hat pattern that breathes — chance and velocity range in the step string

**Producer:** hats on the 16ths on Drums, the off-beats ghosted and only played two times
out of three, and don't let every hit be the same velocity.

**Claude calls** `add_notes_to_clip`
```json
{"track_index": 0, "clip_index": 0, "clear": true,
 "steps": {"42": "x?x?x?x?x?x?x?x?"}, "step": 0.25,
 "velocity": 96, "chance_velocity": 58, "chance": 0.66, "velocity_deviation": -18}
```

**Result:**
```
Added 16 notes to clip at track 0, slot 0 (cleared 16 first) — last note ends at beat 4.
8 notes play with a 66% chance at velocity 58; every note may land up to 18 below its
velocity (Live picks per play). Note ids 1–16.
```

In Live the Chance lane shows 66% on the off-beats and the velocity markers carry a
shaded range below them.

### 1a. The same call on Live 10

**Result:**
```
Added 16 notes to clip at track 0, slot 0 (cleared 16 first) — last note ends at beat 4.
This Live writes plain notes: chance and velocity_deviation were dropped (Live 11 or newer
keeps them). Note ids are not available here, so edit_notes works by pitch and time only.
```

## 2. Change three notes without rewriting the clip

**Producer:** the snare on the 4 is too loud, take it down 20, and push the off-beat hats
a hair late.

**Claude calls** `edit_notes`
```json
{"track_index": 0, "clip_index": 0,
 "select": {"pitch": 38, "from_beat": 3.0, "to_beat": 3.25},
 "set": {"velocity": "-20"}}
```

**Result:**
```
Edited 1 of 24 notes in 'Drums 1' (track 0, slot 0): D1 at beat 3 — velocity 110 → 90.
```

**Claude calls** `edit_notes`
```json
{"track_index": 0, "clip_index": 0,
 "select": {"pitch": 42, "off_grid": 0.5},
 "set": {"shift_beats": 0.02}}
```

**Result:**
```
Edited 8 of 24 notes in 'Drums 1' (track 0, slot 0): F#1 on the off-beat eighths —
moved 0.02 beats later (about 10 ms at 126 BPM). Nothing crossed the clip end.
```

### 2a. A selection that matches nothing

**Result (error):**
```
No notes match: pitch 39 between beat 0 and 4 in 'Drums 1'. The clip holds D1 (38) ×4,
F#1 (42) ×16, C1 (36) ×4. get_clip_notes lists them.
```

### 2b. Deleting by selection

**Producer:** drop every second kick.

**Claude calls** `edit_notes`
`{"track_index": 0, "clip_index": 0, "select": {"pitch": 36, "every": 2, "offset": 1}, "delete": true}`

**Result:**
```
Deleted 2 of 24 notes in 'Drums 1' (track 0, slot 0): C1 at beats 1 and 3. 22 remain.
undo restores them.
```

## 3. Tighten a played take, but not all the way

**Producer:** I played the keys in loosely. Quantize to 16ths but only about 80%, and give
it a bit of swing.

**Claude calls** `quantize_clip`
`{"track_index": 2, "clip_index": 1, "grid": "1/16", "amount": 0.8, "swing": 0.57}`

**Result:**
```
Quantized 'Keys take 2' (track 2, slot 1) to 1/16 at 80% with swing 57% (Live's global
swing, now 57% for record quantize too). 41 notes moved, the largest by 0.11 beats
(about a 32nd); 6 were already on the grid.
```

### 3a. Only the kick

`{"track_index": 0, "clip_index": 0, "grid": "1/8", "amount": 1.0, "pitch": 36}`

**Result:**
```
Quantized C1 (36) in 'Drums 1' to 1/8 at 100%; the other pitches were left alone. 4 notes
moved, the largest by 0.06 beats.
```

### 3b. A grid Live does not have

`{"grid": "1/12"}` → **error:**
```
grid must be one of 1/4, 1/8, 1/8T, 1/8+T, 1/16, 1/16T, 1/16+T, 1/32 (Live's record
quantization grids).
```

## 4. Groove from Live's pool

**Producer:** what grooves do I have? Put an MPC swing on the hats.

**Claude calls** `set_groove` with `{"list": true}`

**Result:**
```
Groove pool (3): 0 'MPC 16 Swing-59' (base 1/16, timing 100%, random 0%, velocity 0%),
1 'Swing 8-62' (base 1/8), 2 'Logic 16 Swing 60'. Global groove amount 100%.
Clips using a groove: Drums 1 slot 0 → none.
```

**Claude calls** `set_groove`
`{"track_index": 0, "clip_index": 0, "groove": "MPC 16 Swing-59", "timing": 0.7}`

**Result:**
```
'Drums 1' (track 0, slot 0) now uses groove 'MPC 16 Swing-59' with timing 70% (that
groove's timing applies to every clip using it). Listen for the off-beat 16ths landing
late and slightly softer.
```

### 4a. A groove that is not in the pool

`{"track_index": 0, "clip_index": 0, "groove": "Swing 16-65"}` → **error:**
```
'Swing 16-65' is not in this set's groove pool (MPC 16 Swing-59, Swing 8-62, Logic 16
Swing 60). The API can only assign grooves already in the pool: drag one from the browser's
Grooves folder (or Core Library → Grooves) onto the pool, then ask again.
```

### 4b. An empty pool

**Result (error):**
```
The groove pool is empty. Drag a groove from the browser's Grooves folder onto a clip or
the pool, then ask again; the API cannot add one.
```

### 4c. Removing it

`{"track_index": 0, "clip_index": 0, "groove": "none"}` → `Removed the groove from 'Drums 1' slot 0; it plays straight again.`

## 5. Undo that

**Producer:** no, undo that.

**Claude calls** `undo` with `{}`

**Result:**
```
Undid 1 step in Live's history. The set overview did not change (the step was inside a
clip or a device; Live's Edit menu names it). Live can undo 41 more steps and redo 1.
```

**Producer:** undo the whole last thing you did — the three edits.

**Claude calls** `undo` with `{"steps": 3}`

**Result:**
```
Undid 3 steps in Live's history. Changed: 'Drums 1' slot 0 notes 22 → 24. Live can undo
38 more steps and redo 4.
Note: a server tool can be several Live steps (build_song was 61 modifying commands); the
activity log lists the commands the last tool sent.
```

### 5a. During a performance

**Result (error):**
```
A performance is running (bar 33, Groove looping, one cue pending). Undo can remove a clip
a cue depends on; end_performance first, or force: true to undo anyway.
```

### 5b. Redo, and nothing to redo

`{"redo": true}` → `Redid 1 step in Live's history. Changed: 'Drums 1' slot 0 notes 24 → 22.`

`{"redo": true}` again → **error:** `Nothing to redo in Live's history.`

## 6. I just played something good

**Producer:** (plays a bass line for a while, transport stopped) keep that.

**Claude calls** `capture_midi` with `{"destination": "session", "name": "Bass idea"}`

**Result:**
```
Captured 'Bass idea' on 'Bass' (track 1, slot 2): 2 bars, 14 notes, E minor likely.
Live set the tempo to 93.4 BPM from your playing (the transport was stopped); set_tempo
128 to go back, or keep_tempo: true next time to restore it automatically.
```

### 6a. Nothing to capture

**Result (error):**
```
Live has nothing to capture. Play some MIDI on a track that is armed or set to monitor In
(Live remembers what you played recently); then ask again.
```

### 6b. Two tracks were playing

**Result:**
```
Captured 2 clips: 'Keys' (track 2, slot 4) 4 bars 31 notes; 'Bass' (track 1, slot 4)
4 bars 12 notes. Tempo unchanged (the transport was running at 126 BPM).
```

## 7. Record a real take into the Arrangement

**Producer:** click on, count-in, record 8 bars of keys from bar 17.

**Claude calls** `set_recording_options`
`{"metronome": true, "record_quantize": "none", "arrangement_overdub": false}`

**Result:**
```
Metronome on. Record quantize none (was 1/16). MIDI Arrangement Overdub off. Count-in
is 1 bar (Live's metronome menu sets it; the API only reads it). Swing 57%. Punch in/out off.
```

**Claude calls** `record_arrangement`
`{"tracks": ["Keys"], "from_bar": 17, "bars": 8}`

**Result:**
```
Recording 'Keys' into the Arrangement from bar 17 for 8 bars (to bar 25) after a 1-bar
count-in. The other armed track 'Bass' was disarmed. The script stops the recording at bar
25 and playback continues; get_arrangement_clips on 'Keys' shows the take.
```

**Producer (bar 21):** stop, I messed up.

**Claude calls** `record_arrangement` with `{"stop": true}`

**Result:**
```
Stopped recording at bar 21.3; playback stopped. 'Keys' has a new Arrangement clip
17.1–21.3 (4 bars 2 beats); delete_arrangement_clip removes it, undo also works.
```

### 7a. A track that cannot record

`{"tracks": ["Master"]}` → **error:** `'Master' cannot be armed (group, return or master tracks cannot record). Name an audio or MIDI track.`

## 8. Layer a hat pattern onto the playing clip

**Producer:** loop the drums and let me overdub hats for four bars.

**Claude calls** `overdub_clip`
`{"track_index": 0, "clip_index": 0, "bars": 4, "quantize": "1/16"}`

**Result:**
```
Session overdub on 'Drums 1' (track 0, slot 0) for 4 bars from the next bar (bar 9):
the clip keeps playing, what you play is added to it, record quantize 1/16. The script
turns overdub off at bar 13 and disarms 'Drums'. undo removes the take.
```

## 9. Sample edits

**Producer:** the vocal chop is a bit sharp and too loud; take it down two semitones and
about 3 dB, and put it on Complex Pro.

**Claude calls** `set_audio_clip`
`{"track_index": 3, "clip_index": 0, "transpose": -2, "gain_db": -3, "warp_mode": "complex_pro"}`

**Result:**
```
'Vox chop' (track 3, slot 0): transpose 0 → -2 st, gain 0.0 → -3.0 dB (Live's 0–1 gain,
matched to 0.1 dB in 6 steps), warp mode Beats → Complex Pro. Warp on.
```

### 9a. A MIDI clip

**Result (error):** `'Keys take 2' (track 2, slot 1) is a MIDI clip; gain, transpose and warp are audio-clip settings.`

### 9b. A warp mode this clip does not offer

`{"warp_mode": "rex"}` → **error:** `'Vox chop' offers warp modes beats, tones, texture, repitch, complex, complex_pro (not rex).`

### 9c. Warp markers and crop

**Producer:** put a warp marker at beat 4 and crop it to the loop.

**Claude calls** `set_warp_markers`
`{"track_index": 3, "clip_index": 0, "add": [{"beat": 4.0}]}`

**Result:** `'Vox chop' now has 3 warp markers: 0.0, 4.0, 8.0 beats (Live keeps a hidden one at the end).`

**Claude calls** `set_audio_clip` `{"track_index": 3, "clip_index": 0, "crop": true}`

**Result:** `Cropped 'Vox chop' to its loop: 4 → 4 beats long, start marker now 0.0.`

### 9d. Double the loop of a MIDI clip

**Claude calls** `set_clip_loop` `{"track_index": 2, "clip_index": 1, "double": true}`

**Result:** `'Keys take 2' loop doubled: 8 → 16 beats, notes and envelopes copied into the second half.`

---

## What the review should decide

- In scene 1, is `?` in the step string the right spelling for "a note with chance"? The
  alternative is a per-pitch `chance` map; `?` keeps the drum grammar one string.
- In scene 2, `select` filters plus `set` deltas (`"-20"` as a string means relative). Is
  the string-for-relative convention clear enough, or should it be `velocity_delta`?
- In scene 5, is diffing the set overview enough, or should `undo` say nothing about what
  changed and just count? The overview diff costs one extra state read per call.
- In scene 6, keeping Live's detected tempo by default (and saying so) vs. restoring it.
- In scene 7, whether the script or the server should stop the recording at `bars`. The
  script's clock is the only thing that can land it on the bar; the server cannot.
