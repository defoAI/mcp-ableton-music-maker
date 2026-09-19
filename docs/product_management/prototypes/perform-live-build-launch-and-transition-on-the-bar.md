# Prototype transcript — perform live: build, launch and transition on the bar

Fake data, real tool names and parameter names. Written 2026-09-19, not yet reviewed.

The rule the whole transcript obeys: **Claude plans, Live executes.** Claude's replies take
seconds; a bar at 126 BPM lasts 1.9 s. So nothing musically timed happens at the moment a
tool call arrives. Every launch is quantized by Live to the next bar, and every multi-step
move is handed to the Remote Script as a cue that the script executes on its own clock,
whether or not the server is still connected.

Bar numbers are Live's own (1-based, as shown in Live's transport), so the producer can
read them off the screen.

---

## 1. Going live: a deep pumping techno track from nothing

**Producer:** we're going live. start with a deep pumping techno track, 126, and keep it
rolling until I say otherwise.

**Claude calls** `build_song` (existing tool) with a `dry_run: false` document: five tracks
(Kick, Hats+Perc, Bass, Pad, Lead) with instruments by search words, three Session scenes
`Intro`, `Groove`, `Groove+Pad`, clips in step-string form, sends, colours — then:

**Claude calls** `start_performance`
`{"scene": "Intro", "quantization": "1_bar", "key": "F minor", "tempo": 126}`

**Result:**
```
Performance started at 2026-09-19 21:04:11.
Playing scene 'Intro' (2 clips) from bar 1 · 126 BPM · 4/4 · key F minor (from your set)
Launch quantization is 1 bar: everything you or I fire lands on the next bar.
While the performance runs I will not stop the transport, move the playhead, jump the
tempo, or delete anything that is playing; those calls are refused until end_performance.
State: get_performance_state. Timed moves: cue.
```

**Claude:** Rolling. Intro is kick and hats only; I'll bring the bass in at bar 9 and the
pad at 17 unless you want it sooner.

**Claude calls** `cue`
```json
{"name": "open up",
 "steps": [
   {"at": {"bar": 9},  "fire_scene": "Groove"},
   {"at": {"bar": 17}, "fire_scene": "Groove+Pad"}
 ]}
```

**Result:**
```
Cue 'open up' (id 1) scheduled — it is bar 3.2 now.
  bar 9   fire scene 'Groove' (4 clips: Kick, Hats+Perc, Bass, Pad muted)
  bar 17  fire scene 'Groove+Pad' (4 clips)
The Remote Script runs this on its own clock; it happens even if I go quiet.
Cancel with cancel_cue {"id": 1}.
```

## 2. Reading the room

**Producer:** where are we?

**Claude calls** `get_performance_state`

**Result:**
```
bar 14.3 · 126 BPM · 4/4 · playing since bar 1 (26 s) · quantization 1 bar · key F minor
Track      Playing                  Queued
Kick       Groove/Kick (loop 4)     –
Hats+Perc  Groove/Hats (loop 2)     –
Bass       Groove/Bass (loop 8)     –
Pad        –                        –
Lead       –                        –
Scenes: Intro · [Groove] · Groove+Pad · (3 empty)
Cues: 1 'open up' — next step bar 17 fire 'Groove+Pad'; 1 step done
Next bar in 1.1 s.
```

**Claude:** Bar 14, groove running, pad comes in at 17.

## 3. "Give a melody" — typed

**Producer:** give it a melody now, something like F Ab C Bb Ab F, slow, over two bars, on
the lead.

**Claude calls** `create_clip` (existing) `{"track_index": 4, "clip_index": 2, "length": 8,
"name": "Lead A", "notes": "f4:2 g#4:1 c5:2 a#4:1 g#4:1 f4:1"}` — slot 2 is the row of the
scene that is playing, so the clip belongs to the current section — then
`fire_clip` `{"track_index": 4, "clip_index": 2}`

**Result (fire_clip):**
```
Fired Lead/'Lead A' — starts at bar 19 (1-bar quantization; it is bar 18.2).
```

**Claude:** Lead comes in at 19, in F minor. I've also copied it into the next scene so it
survives the next section change; say "drop the lead" to take it out.

**Claude calls** `create_clip` again with `clip_index: 3` (the `Groove+Pad` row, actually
the next empty row — see the story's open question 5 about copying clips between scenes).

## 4. "Give a melody" — played

**Producer:** actually let me play it. arm the lead, record me for four bars.

**Claude calls** `record_clip` `{"track_index": 4, "bars": 4, "name": "Lead live"}`

**Result:**
```
Recording Lead slot 3 'Lead live' — starts at bar 21 (next bar), 4 bars, then loops.
Play from bar 21. The clip keeps looping when the recording ends; nothing else changes.
```

**Claude:** Armed and counting in. Recording starts at bar 21 and loops from bar 25.

**Producer (after):** that was awful, throw it away and keep yours.

**Claude calls** `stop_clip` `{"track_index": 4, "clip_index": 3}` then `fire_clip`
`{"track_index": 4, "clip_index": 2}` (both land on the next bar) then
`delete_clip` `{"track_index": 4, "clip_index": 3}` — allowed, because the clip is no longer
playing.

## 5. Mixing into a breakbeat track

**Producer:** ok, now mix us into a breakbeat track, about 8 bars from now. make it a
proper transition, not a jump.

**Claude** builds the next material without touching what plays — `create_scene` twice
(`Break`, `Breakbeat`), a second drum track `Breaks` with a kit loaded (`create_midi_track`
+ `load_instrument_or_effect`; the track is silent, so loading cannot interrupt the mix),
`create_clip` for the break (pad + sparse perc, no kick) and the breakbeat (Breaks track:
step strings for a 2-bar amen-style pattern, Bass: the same F minor bassline re-voiced
in 8ths), and `set_clip_automation` on the `Break` pad clip: a filter cutoff ramp so the
sweep is sample-accurate, written into the clip rather than performed over the socket.

**Claude calls** `get_performance_state` → `bar 41.1` — then `cue`
```json
{"name": "into breaks",
 "steps": [
   {"at": {"bar": 49}, "fire_scene": "Break"},
   {"from": {"bar": 49}, "bars": 8, "ramp": {"tempo": 134}},
   {"at": {"bar": 57}, "fire_scene": "Breakbeat"}
 ]}
```

**Result:**
```
Cue 'into breaks' (id 2) scheduled — it is bar 41.2 now.
  bar 49     fire scene 'Break' (Pad, Hats+Perc; kick out)
  bars 49–57 ramp tempo 126 → 134 (while no kick is playing)
  bar 57     fire scene 'Breakbeat' (Breaks, Bass, Pad, Lead)
Check: every bar from 49 has at least one clip playing. Nothing is silent.
```

**Claude:** At bar 49 the kick drops out and the pad opens up; the tempo climbs to 134
under the break so it never sounds like a speed-up; at 57 the breaks come in. If you want
it earlier say "go" and I'll pull the cue forward to the next bar.

**Producer:** go.

**Claude calls** `cancel_cue` `{"id": 2}` then `cue`
```json
{"name": "into breaks now",
 "steps": [
   {"at": "next_bar", "fire_scene": "Break"},
   {"from": "next_bar", "bars": 8, "ramp": {"tempo": 134}},
   {"at": {"bars_after": 8}, "fire_scene": "Breakbeat"}
 ]}
```

**Result:**
```
Cue 'into breaks now' (id 3) scheduled — next bar is 44 (in 0.7 s).
  bar 44     fire scene 'Break'
  bars 44–52 ramp tempo 126 → 134
  bar 52     fire scene 'Breakbeat'
```

## 6. A DJ-style crossfade between two audio tracks

**Producer:** blend the second track in over 16 bars, don't cut.

**Claude** has `create_audio_clip` (existing, Live 12.0.5+) the second track's audio in a
Session slot on track `Deck B`, warped, and:

**Claude calls** `set_crossfader` `{"assign": [{"track": "Deck A", "side": "A"},
{"track": "Deck B", "side": "B"}]}` then `cue`
```json
{"steps": [
   {"at": "next_bar", "fire_clip": {"track": "Deck B", "clip": 0}},
   {"from": "next_bar", "bars": 16, "ramp": {"crossfader": 1.0}}
 ]}
```

**Result:**
```
Cue (id 4) scheduled — next bar is 61.
  bar 61      fire Deck B/'Second track'
  bars 61–77  ramp crossfader A → B
```

## 7. What is refused while performing

**Producer:** stop everything.

**Claude calls** `stop_playback`

**Result (error):**
```
A performance is running (since bar 1, now bar 78). stop_playback would cut the audio
mid-bar. Use end_performance {"at": "next_bar"} to stop on the bar, or
end_performance {"fade_bars": 8} to fade the master and stop, or
end_performance {"now": true} if you really mean now.
```

**Claude:** Do you want it on the bar, faded over 8 bars, or right now?

**Producer:** on the bar.

**Claude calls** `end_performance` `{"at": "next_bar"}`

**Result:**
```
Performance ended at bar 79: all clips stopped, transport stopped. 2 min 32 s, 4 cues,
0 cancelled, 0 missed. Guards lifted.
```

The same refusal covers `play_from`, `set_arrangement_time`, `set_tempo` (a jump; use a
cue ramp), `capture_mix` (it moves the transport), `switch_to_arrangement_view`,
`delete_track` and `delete_clip` on anything playing or queued, and `stop_all_clips`
outside a cue.

## 8. Failure cases

**A cue in the past**
```
cue: step 1 is at bar 40 but it is bar 41.2. Use "next_bar" or a bar ahead of 42.
```

**A cue that would go silent**
```
cue: at bar 65 nothing would be playing (scene 'Outro' has no clips on any playing
track). Add "allow_silence": true if that is the plan.
```

**A step less than a bar ahead**
```
Cue (id 5) scheduled. Warning: step 1 is at bar 42 and it is 41.4 — 0.4 s away. The
script fires it during bar 41 and Live quantizes it to 42, but if the socket is slow it
lands at 43. For certainty, cue two bars ahead.
```

**Connection lost mid-performance, then back**
`get_performance_state` after reconnect:
```
bar 96.1 · 134 BPM · playing · quantization 1 bar
While I was away: cue 3 step 3 fired at bar 52 (scene 'Breakbeat'). No cue missed.
```

**The producer pressed Stop in Live**
```
bar 0 · stopped by Live at bar 88.3 (transport stopped outside this server). Pending cue 6
cancelled: its bars no longer mean anything. Say "start again from scene 'Breakbeat'" to
resume; the guards are still on.
```

**Live 10, or the Remote Script is older than the server**
```
start_performance needs Remote Script 1.13.0 (this Live has 1.12.0): fire_scene, cue and
get_performance_state are not available. Reinstall the script and restart Live.
```

**Time signature change mid-set**
```
cue: the set changed to 7/8 at bar 100; cue 7's bars were counted in 4/4. Cancelled.
Re-cue in the new signature.
```

---

## What the transcript exposes

1. The producer never sees a tool name in a reply; they see bars and scene names.
2. `cue` is the only new concept the producer has to hold. Everything else is Live's own
   vocabulary: scene, clip, quantization, crossfader.
3. Three places the tool must be honest about timing: the "less than a bar ahead"
   warning, the refusal list, and "while I was away".
4. The melody path splits: typed → `create_clip` (existing); played → `record_clip` (new,
   built on the fixed-length recording the capture story already uses).
5. Loading an instrument while the set plays is the one thing this transcript cannot
   prove from the API; it is open question 7 in the story.
