# Prototype transcript — sections and songs: build, jump, loop, remember

Fake data, real tool names and parameter names. Written 2026-09-19, not yet reviewed.

The performer here is loop-based: they think in sections (an 8-bar groove, a 16-bar
break), not in clips and slots. A section is a scene row with a name and a phrase length;
a song is a setlist of sections, each looping until "go" unless it carries a repeat count;
both live in the Live set itself (scene names), so Live's own Save keeps them. Every jump
lands at the end of the *playing* section's phrase unless told otherwise, cutting a phrase
short is said out loud, and the default transition is a straight cut. The level rides
under the clock on every reply.

Bar numbers are Live's own (1-based).

---

## 1. Before the gig: a set with sections

**Producer:** build me a techno set in F minor at 126 with an intro, a groove, a groove with
pad, a break and a drop. 8-bar phrases, the break 16.

**Claude calls** `build_song` with a `scenes` block
`[{"name": "Intro", "phrase_bars": 8}, {"name": "Groove", "phrase_bars": 8}, {"name": "Groove+Pad", "phrase_bars": 8}, {"name": "Break", "phrase_bars": 16}, {"name": "Drop", "phrase_bars": 8}]`
and clips per track and row.

**Result (last lines):**
```
Scenes: 0 Intro · 8, 1 Groove · 8, 2 Groove+Pad · 8, 3 Break · 16, 4 Drop · 8.
…
Sections are the scene rows; their names carry the phrase length, so Live's Save keeps
them. Say "play the song" once you have a setlist, or "jump to Break".
```

In Live the scenes read `Intro · 8`, `Groove · 8`, `Groove+Pad · 8`, `Break · 16`,
`Drop · 8`. That naming is the whole storage format for a section.

## 2. A song is a setlist

**Producer:** the song goes intro twice, then groove, groove with pad, break, drop, and
groove to end — I'll say go each time.

**Claude calls** `set_song`
```json
{"setlist": [
  {"section": "Intro", "repeats": 2}, {"section": "Groove"}, {"section": "Groove+Pad"},
  {"section": "Break"}, {"section": "Drop"}, {"section": "Groove"}
]}
```

**Result:**
```
Song: Intro×2 → Groove → Groove+Pad → Break → Drop → Groove. Intro is counted (16 bars);
the other five loop until you say go.
Stored as the scene 'Setlist: Intro×2 → Groove → Groove+Pad → Break → Drop → Groove' (an
empty row at the bottom of the set); edit the name in Live to change the order.
play_song starts it; go, next, back and jump_to steer it. Give an entry a count
("Drop×4") to pre-plan it.
```

## 3. Playing the song, steering it

**Producer:** play the song.

**Claude calls** `play_song` `{"from": "Intro"}`

**Result:**
```
Performance started at 21:04:11 (1-bar quantization, key F Minor, guards on).
Song from 'Intro' (1 of 6): Intro×2 from bar 1, then Groove at bar 17 — Groove loops until
you say go. Cue 1 holds that one jump. Say go to continue, next, back or jump to <section>
at any time; hold stops a counted section where it is.
⏱ bar 1.1 · next bar in 1.9 s · phrase ends bar 9 · next cue: bar 17 fire scene 'Groove · 8' (cue 1)
🔊 master −4.0 dB peak this bar · Kick −6 · Hats+Perc −12
```

**Producer (bar 30, Groove looping since 17):** go.

**Claude calls** `go` (default: at the end of the playing section's phrase)

**Result:**
```
Continuing at the end of this phrase: Groove+Pad at bar 33 (Groove is 8 bars from 17: 25,
33). Groove+Pad loops until you say go. Cue 2.
⏱ bar 30.3 · next bar in 0.9 s · phrase ends bar 33 · next cue: bar 33 fire scene 'Groove+Pad · 8' (cue 2)
🔊 master −3.8 dB · Kick −6 · Bass −7
```

The phrase rule, stated once: "next phrase" is counted in the phrase of the section that
is playing now, from the bar it started on, whatever is queued. Break is 16 bars, but at
bar 30 the phrase that ends is Groove's, at 33.

**Producer:** jump to the drop at the next phrase and loop it eight times.

**Claude calls** `jump_to` `{"section": "Drop", "repeats": 8}`

**Result:**
```
Warning: 'Drop' last peaked at −0.8 dB, 3.0 dB hotter than Groove+Pad (−3.8). Jumping anyway
(force: true to silence this; pull the master 3 dB with set_track_mixer master, or cue a
ramp, before bar 41).
Jumping to 'Drop' at bar 41 (end of this phrase), 8 passes, then the song continues from
after Drop: Groove at bar 105 (loops). Cue 3 replaces cue 2.
⏱ bar 38.2 · … · next cue: bar 41 fire scene 'Drop · 8' (cue 3)
🔊 master −3.8 dB · …
```

**Producer:** back.

**Claude calls** `back` → `Back to where you were: 'Groove+Pad' at bar 49 (end of this phrase); it loops until go. Drop is out of the plan; previous_section walks the setlist instead.`

**Producer:** next, on the bar.

**Claude calls** `next_section` `{"at": "next_bar"}` → `'Break' at bar 51 (next bar) — cutting 6 bars off Groove+Pad's phrase, as asked. Break loops until go.`

Every jump is one `cue` under the hood: the server computes the bars from the playing
section's phrase length, the script fires the scenes on its clock, and the clock line shows
the next one. A counted section (`Intro×2`) is a hold that ends by itself; `hold` turns a
count back into a loop.

## 4. Sections made on stage

**Producer (tweaking, layering):** save this as a section called Peak, 8 bars.

**Claude calls** `make_section` `{"name": "Peak", "phrase_bars": 8, "from": "playing"}`

**Result:**
```
Section 'Peak · 8' saved as scene 5: the playing clips (Kick, Hats+Perc, Bass, Pad, Lead)
captured into a new row below the current one (Live's capture-and-insert-scene). The set
keeps playing; the song's setlist is unchanged — say "add Peak after Drop" or jump to it.
⏱ …
```

**Producer:** make a darker 8-bar section after the break: half-time drums, the pad down an
octave, no lead. Call it Dark.

**Claude calls** `make_section`
```json
{"name": "Dark", "phrase_bars": 8, "after": "Break",
 "clips": {"Kick": {"steps": {"C1": "x.......x......."}}, "Hats+Perc": {"steps": {"F#1": "..x...x...x...x."}},
           "Bass": {"notes_csv": "41,0,4,90\n41,4,4,90"}, "Pad": {"notes_csv": "41,0,8,70\n44,0,8,70\n48,0,8,70"}}}
```

**Result:**
```
Section 'Dark · 8' created as scene 4 (after Break; later rows moved down): Kick, Hats+Perc,
Bass, Pad have clips, Lead is empty (Lead stops when Dark fires; keep_track_playing Lead to
carry it). Not in the setlist yet: add_to_song {"section": "Dark", "after": "Break", "repeats": 2}
or jump_to it.
⏱ …
```

**Producer:** put it in after the break, twice.

**Claude calls** `add_to_song` `{"section": "Dark", "after": "Break", "repeats": 2}` →
`Song: … → Break → Dark×2 → Drop → …; the setlist scene renamed; the running plan re-planned from bar 65.`

**Producer:** make one like Groove but with the hats thinned out and the bass an octave
down. Call it Groove Low.

**Claude calls** `make_section`
```json
{"name": "Groove Low", "from": {"section": "Groove"},
 "changes": {"Hats+Perc": "thin", "Bass": {"transpose": -12}, "Lead": "empty"}}
```

**Result:**
```
Section 'Groove Low · 8' created as scene 7, a copy of 'Groove · 8' with: Hats+Perc thinned
(every other off-beat dropped, seed 1), Bass transposed −12, Lead emptied. Kick and Pad are
identical to Groove. Not in the setlist; jump_to it or add_to_song.
⏱ …
```

## 5. Transitions that mix instead of rebuild

**Producer:** after the break go into organic house at 122, drums half-time, and cross the
pad group into the organic pads over 8 bars.

**Claude calls** `jump_to`
```json
{"section": "Organic", "transition": {"tempo": 122, "retime": {"tracks": ["Kick", "Hats+Perc"], "to": "half_time"},
 "crossfade": {"out": ["Pad", "Lead"], "in": ["Organic Pad", "Keys"], "bars": 8}}}
```

**Result:**
```
Jumping to 'Organic · 16' at bar 97 (end of this phrase):
  bars 89–97   ramp tempo 126 → 122
  bar 97       fire scene 'Organic · 16'; Kick, Hats+Perc retimed to half time in Organic's row
               (clips 'Kick (half_time)', 'Hats (half_time)' written now, seed 1)
  bars 97–105  crossfade: Pad, Lead → 0 · Organic Pad, Keys → their current level
Cue 4 (11 primitive steps). Nothing is rebuilt; every clip stays where it was.
⏱ …
```

`retime` is `vary_clip half_time/double_time` written into the target section's row before
the jump; `crossfade` is a pair of volume ramps; `tempo` is the existing ramp under the
outgoing phrase. All three are the `cue` primitives the script already runs.

## 6. Feel

**Producer:** the congas sound like a drum machine. Swing them and loosen them up.

**Claude calls** `groove_clip` `{"track": "Congas", "clip": 1, "groove": "Swing 16", "amount": 0.6}`

**Result (Live groove):**
```
Congas slot 1: groove 'Swing 16' from the Groove Pool at 60% (non-destructive; Live's own
groove). Set the pool's global amount with groove_amount.
```

**Claude calls** `humanize` `{"track": "Congas", "clip": 1, "timing_ms": 12, "velocity": 15, "seed": 3}`

**Result:**
```
Congas slot 1 humanized (seed 3): hits now land up to 12 ms early or late — about a 64th
note at 126 BPM — so the congas drift around the hats instead of sitting on them; velocities
vary ±15, the off-beat 16ths most. undo_vary puts them back on the grid.
```

**Result (a Live without that groove in its pool):**
```
No groove named 'Swing 16' in this set's Groove Pool and the API cannot add one; drag one in
from the browser (Grooves), or use humanize / swing_notes (note rewrite, undoable).
```

## 7. Sound without leaving the flow

**Producer:** darken the pad and give the bass more drive.

**Claude calls** `shape_sound` `{"track": "Pad", "cutoff": "-25%", "resonance": "+10%"}` and
`shape_sound` `{"track": "Bass", "drive": "+30%"}`

**Result:**
```
Pad (Instrument Rack 'Evolving Pad'): macro 'Cutoff' 62% → 37%, macro 'Res' 20% → 30%.
Bass (Analog): Drive 0% → 30%.
Racks are asked first: their macros are how the preset's maker meant it to be shaped. Words
this track answers to: cutoff, resonance (rack macros), attack, decay, sustain, release,
drive, detune, width, lfo_rate, reverb, delay (Analog). Sweep one in a cue:
{"ramp": {"sound": "cutoff", "track": "Pad", "to": 0.8}}.
```

**Result (a device the vocabulary does not know):**
```
Pad ('Serum' on this track) is not in the vocabulary; its parameters by name are: Cutoff,
Res, Env1 Atk, … — set_device_parameter takes a name substring now: {"parameter": "Cutoff", "value": 0.4}.
```

## 8. Remembering the set

**Producer (next day, opening the set in Live):** what's in here?

**Claude calls** `get_context`

**Result (excerpt):**
```
Sections (scenes): 0 Intro · 8 (2) · 1 Groove · 8 (4) · 2 Groove+Pad · 8 (4) · 3 Break · 16 (2) · 4 Dark · 8 (4) · 5 Drop · 8 (5) · 6 Peak · 8 (5)
Song: Intro×2 → Groove×4 → Groove+Pad×4 → Break×1 → Dark×2 → Drop×4 → Groove×2 (from the 'Setlist:' scene)
Key F Minor · 126 BPM · phrase lengths from the scene names
```

Nothing was read from a server file: the scene names are the memory. The producer saved
the set in Live with Cmd+S (the API has no Save).

**Producer:** export this set so you can rebuild it anywhere.

**Claude calls** `export_set` `{"name": "techno-friday"}`

**Result:**
```
Exported to ~/.ableton-music-maker/sets/techno-friday.json: 5 tracks (instruments by URI
and by name), 7 sections with 27 clips and their notes, mixer and sends, the setlist, tempo
and key. import_set {"name": "techno-friday"} rebuilds it into an empty set through
build_song. The file holds your notes and names; delete it from the sets folder or with
"Delete all local data".
```

## 9. Failure cases

**A section name that is not unique**
```
make_section: a scene named 'Groove · 8' exists (scene 1). Names are how sections are
remembered; call this one 'Groove 2' or replace: true.
```

**A setlist naming an unknown section**
```
set_song: 'Outro' is not a section. Sections: Intro, Groove, Groove+Pad, Break, Dark, Drop,
Peak. make_section first.
```

**"go" with nothing held**
```
Nothing is held: the song is running (Groove+Pad, pass 2 of 4, next jump bar 65).
```

**A song without a performance**
```
play_song starts a performance; the transport is stopped, so it fires 'Intro' now and counts
from bar 1.
```

**A Setlist scene edited by hand into something unreadable**
```
The 'Setlist:' scene reads "Setlist: intro x2, grove x4"; 'grove' is not a section (did you
mean Groove?). Fix the scene name in Live or set_song again.
```

---

## What the transcript exposes

0. Holding is the state, going is the action: a section without a count loops until the
   performer says so, and the reply never assumes how long they will want it.
1. Sections and songs need no new storage: scene names and one Setlist scene are the
   format, and Live's Save is the persistence. `export_set` is a backup, not the truth.
2. A song is one cue; steering it is cancel-and-re-plan from the end of the *playing*
   section's phrase, counted from the bar it started on. The server keeps the cursor
   (which section, which pass) and the jump history in the performance record; `back` is
   the history, `previous_section` is the setlist.
2a. Cutting a phrase short is always said ("cutting 6 bars off Groove+Pad"); loudness is
   always shown (the 🔊 line), and a jump into a hotter section warns before the mix clips.
3. `make_section` from what is playing is Live's own `capture_and_insert_scene`; from a
   description it is `create_scene` plus one `create_clip` per track — both already exist.
4. Genre transitions are compositions of existing primitives: tempo ramp, `vary_clip`
   half/double time into the target row, volume ramps. No rebuild.
5. Groove has two paths on purpose: Live's Groove Pool when the API can assign one, note
   rewriting otherwise; the reply always says which happened.
6. The sound vocabulary is a table, not intelligence: words → parameter names per Live
   instrument; anything else falls back to a parameter name substring.
