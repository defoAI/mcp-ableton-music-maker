# Prototype transcript — bar awareness on every response, gestures as cue steps

Fake data, real tool names and parameter names. Written 2026-09-19, not yet reviewed.

Two facts shape everything below. A socket round trip to Live costs about 200 ms whatever
the command does (measured 2026-09-19 on Live 12.4.6; see the architecture note), so a bar
at 126 BPM is nine round trips long and the assistant's own reply is several more. And the
Remote Script always knows the bar at zero cost. So the clock rides on every response the
script already sends, and a live move is one step the script expands, never a sequence of
calls that race the bar line.

Bar numbers are Live's own (1-based). `39.2` is bar 39, beat 2.

---

## 1. The clock is on every reply

**Producer:** open the hats a bit.

**Claude calls** `set_send` `{"track_index": 1, "send_name": "Reverb", "value": 0.35}`

**Result:**
```
Set send A (Reverb) on track 1 'Hats+Perc' to 0.35.
⏱ bar 39.2 · next bar in 2.2 s · phrase ends bar 48 · next cue: bar 51 fire scene 'Break' (cue 4)
```

**Claude:** Hats have more room now. We're at bar 39 in the second phrase of the groove; the
break is cued for 51.

No extra call was needed: the clock line came with the reply. The same line ends every
result while a performance runs, including errors, and `get_performance_state` carries it
too.

## 2. A launch says which bar it made

**Producer:** fire the lead now.

**Claude calls** `fire_clip` `{"track_index": 4, "clip_index": 2}`

**Result (comfortably before the bar line):**
```
Fired Lead/'Lead A' — lands on bar 40 (issued at 39.3, 1-bar quantization).
⏱ bar 39.3 · next bar in 1.4 s · phrase ends bar 48 · next cue: bar 51 fire scene 'Break' (cue 4)
```

**Result (the bar line passed while the call was in flight):**
```
Fired Lead/'Lead A' — lands on bar 41, not 40: the call arrived at 40.1.02, after the bar
line (round trip 210 ms, the bar was 0.4 s away when I sent it). For certainty, cue it.
⏱ bar 40.1 · next bar in 1.8 s · …
```

The script computes the landing bar *after* it fires, from where the transport actually is.
The assistant never guesses.

**Result (the server refuses to gamble, with `no_later_than`):**

`fire_clip` `{"track_index": 4, "clip_index": 2, "no_later_than": 40}` issued with 0.3 s to
the bar and a measured round trip of 0.21 s:
```
Not fired: bar 40 is 0.3 s away and the round trip is 0.21 s, so the launch might land on
41. Scheduled as cue 5 for bar 41 instead (the earliest certain bar); cancel_cue 5 to drop
it. ⏱ bar 39.4 · next bar in 0.3 s · …
```

## 3. Phrases, and the bar map

**Producer:** what's coming up?

**Claude calls** `get_performance_state` `{"bar_map": true}`

**Result:**
```
bar 40.1 · 126 BPM · 4/4 · playing since bar 1 (1 min 14 s) · quantization 1 bar · key F minor
Track      Playing                  Queued
Kick       Groove/Kick              –
Hats+Perc  Groove/Hats              –
Bass       Groove/Bass              –
Pad        –                        –
Lead       Lead A                   –
Scenes: Intro · [Groove] (phrase 16) · Groove+Pad · Break (phrase 8) · Breakbeat
Cues: 4 'into breaks' — next step bar 51 fire scene 'Break'; 5 'lead' — bar 41 fire Lead/2
Bars 40–71:
  41   cue 5: fire Lead/slot 2
  48 | phrase ends (Groove, 16 bars from 33)
  49 | phrase starts
  51   cue 4: fire scene 'Break'
  51–59 cue 4: ramp tempo 126 → 134
  57 | phrase ends (Break, 8 bars from 51 — set when the scene fires)
  59   cue 4: fire scene 'Breakbeat'
  65 | phrase (Breakbeat, default 16 bars)
⏱ bar 40.1 · next bar in 1.9 s · phrase ends bar 48 · next cue: bar 41 fire Lead/slot 2 (cue 5)
```

Phrase length is per scene (`create_scene` and `set_scene` take `phrase_bars`), default 16,
counted from the bar the scene was fired. A scene fired from Live's own UI counts from the
bar the script saw it start.

## 4. Cue times that speak in phrases

**Producer:** bring the pad in at the next phrase, and the break two phrases after that.

**Claude calls** `cue`
```json
{"name": "pad then break",
 "steps": [
   {"at": "next_phrase", "fire_clip": {"track": "Pad", "clip": 2}},
   {"at": {"phrases_after": 2}, "fire_scene": "Break"}
 ]}
```

**Result:**
```
Cue 'pad then break' (id 6) scheduled — it is bar 40.2 now.
  bar 49       fire Pad/slot 2           (next phrase)
  bar 81       fire scene 'Break' (1 clip: Pad); Kick, Hats+Perc, Bass out   (2 phrases after: 49 + 2×16)
Check: every bar from the first step has at least one clip playing.
The Remote Script runs this on its own clock. Cancel with cancel_cue {"id": 6}.
⏱ bar 40.2 · next bar in 1.6 s · phrase ends bar 48 · next cue: bar 41 fire Lead/slot 2 (cue 5)
```

Sub-bar placement, when the quantization is finer than a bar:

`{"at": {"bar": 49, "beat": 3}, "fire_clip": {"track": "FX", "clip": 0}}` → `bar 49.3  fire FX/slot 0`
(refused with "quantization is 1 bar; a beat inside the bar cannot land — set_launch_quantization \"1/4\" first" when it is not).

## 5. A breakdown is one step

**Producer:** breakdown for 8 bars at the phrase, keep the pad and the lead, then everything
back.

**Claude calls** `cue`
```json
{"name": "breakdown",
 "steps": [
   {"at": "next_phrase", "gesture": {"breakdown": {"keep": ["Pad", "Lead"], "bars": 8}}}
 ]}
```

**Result:**
```
Cue 'breakdown' (id 7) scheduled — it is bar 42.1 now.
  bar 49       breakdown: Kick, Hats+Perc, Bass out (stop clips), Pad, Lead stay
  bar 57       breakdown ends: Kick, Hats+Perc, Bass back (fire the slots they were playing)
  = 6 primitive steps; get_performance_state lists them under cue 7
⏱ bar 42.1 · …
```

The gesture is expanded on the server into the primitive steps the script already runs
(`stop_clip` ×3 at 49, `fire_clip` ×3 at 57, each with the slot that was playing when the
cue was made). If a kept track has no clip playing, the result says so before anything is
scheduled.

Other gestures, same shape:

| Gesture | Parameters | Expands to |
|---|---|---|
| `drop` | `bars` (default 1), `keep` | stop everything but `keep` at the bar, refire the playing slots `bars` later |
| `mute_except` | `keep`, `bars` (optional: restore after) | `set mute` on every other track, and back |
| `sweep` | `track`, `device_index`, `parameter_index`, `from`, `to`, `bars` | one `ramp` step on the device parameter |
| `build` | `track` (the riser), `send`, `to`, `bars`, optional `hats` (a track whose clip is swapped for a denser slot at the last bar) | send `ramp` + one `fire_clip` |
| `panic` | `keep`, `bars` (default 1) | volume ramps to 0 on everything but `keep`, then stops |

Half-time and double-time on a drum track, and hat density from note editing, are not
gestures: they need a new clip, which is `vary_clip` in the variation story.

## 6. Listening without stopping the set

**Producer:** is the bass fighting the kick?

**Claude calls** `listen` `{"bars": 1}`

**Result:**
```
Listened for 1 bar (bar 44, 1.9 s, 8 meter readings):
  Kick       peak -3.1 dBFS-ish  avg 0.61
  Bass       peak -4.0           avg 0.58
  Master     peak -0.4           avg 0.72   ← within 0.5 dB of clipping
Reading: the master is riding the ceiling; Kick and Bass peak within 1 dB of each other.
Levels are Live's output meters, not audio: use listen {"capture": true, "bars": 2} for a
recorded bar with low/mid/high balance (records through the Capture track without touching
the transport).
⏱ bar 45.1 · …
```

`listen` never stops or moves the transport; `capture: true` fires a fixed-length recording
on the Capture track on the next bar, exactly like `record_clip`, and measures the file.

## 7. Safety nets

**Producer:** try a wider stereo on the pad and pull the bass down 3 dB — if it's worse, put
it back on the bar.

**Claude calls** `snapshot_mix` → `Mix snapshot 1 taken at bar 46.1 (5 tracks, 2 returns, master).` then the
mixer calls, then after listening:

**Claude calls** `cue` `{"steps": [{"at": "next_bar", "restore_mix": 1}]}`

**Result:**
```
Cue (id 8) scheduled — bar 48: restore mix snapshot 1 (volume, pan, sends, mute on 5 tracks,
2 returns, master).
⏱ …
```

## 8. Finding sounds without a round trip

**Producer:** I want an analog bass, a techno kit and an evolving pad.

**Claude calls** `search_browser` `{"queries": ["analog bass", "techno kit", "evolving pad"], "best": true}`

**Result (index complete, no command sent to Live):**
```
From the library index (3,412 items, complete, walked 14:12):
  analog bass   → 'Analog Bass Warm'   query:Sounds#Bass:Analog%20Bass%20Warm      (Sounds/Bass)
  techno kit    → 'Techno Kit 909'     query:Drums#Kits:Techno%20Kit%20909         (Drums/Kits)
  evolving pad  → 'Evolving Pad'       query:Sounds#Pad:Evolving%20Pad             (Sounds/Pad)
Use the URIs in build_song `instrument` or load_instrument_or_effect; or give build_song the
words and it resolves them the same way.
```

**Result (index still walking):**
```
From the library index (1,200 items so far, walking) plus Live: …
  evolving pad  → nothing yet; 2 more results may appear when the walk completes.
```

**A cached URI that no longer loads** (a pack was removed):
```
Loaded 'Evolving Pad' on track 3 ('Pad') after re-finding it: the indexed URI was stale,
so the index entry was replaced. Devices on the track now: Evolving Pad
```

## 9. Failure cases

**A phrase time when nothing is playing**
```
cue: step 1 "next_phrase" — no scene has been fired, so there is no phrase to count from.
Use "next_bar" or {"bar": N}.
```

**A sub-bar time with bar quantization**
```
cue: step 2 is at bar 49 beat 3 but launch quantization is 1 bar, so it would land on bar 50.
set_launch_quantization "1/4" first, or use {"bar": 49}.
```

**A gesture whose kept track is silent**
```
cue: breakdown keeps Pad, Lead — Lead is not playing anything at bar 49 (nothing queued
either). Fire it first, or keep Pad only.
```

**The clock line when the transport stopped in Live**
```
⏱ stopped at bar 88.3 (transport stopped outside this server) · 1 cue cancelled · guards on
```

---

## What the transcript exposes

1. The clock costs nothing because the script already answers every command; the server
   only renders a field that is already there.
2. "Lands on bar N" is computed after the fact by the script, from the transport, so a launch
   is reported, not predicted. `no_later_than` is the only place the server predicts, and it
   errs on the side of a cue.
3. Gestures are a server-side expansion into steps the script already knows. The script
   gains nothing new for phase 2; the state readout shows the expansion so nothing is hidden.
4. `listen` is meters unless asked to record; the recording path reuses `record_clip`'s
   fixed-length fire, so it never touches the transport and the guard on `capture_mix` stays.
5. Phrases need one new fact the script must keep: the bar each scene was last fired on.
6. Search is instant because the index lives on the server (and on disk); the script's walk
   is paged in the background so a tool call never waits behind it for more than a second.
