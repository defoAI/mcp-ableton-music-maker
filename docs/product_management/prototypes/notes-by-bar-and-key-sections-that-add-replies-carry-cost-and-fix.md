# Prototype transcript — notes by bar and in the key, sections that add, replies that carry the cost and the fix

Fake data, real tool names and parameter names. Written 2026-09-19 from the field notes of
the third real set (a three-track techno set built in an empty Live set and steered live,
with two mid-performance rewrites). Not yet reviewed.

The performer here builds while the room listens. Every scene below is a moment from those
notes where the tool cost a round trip, a wrong note or a bar of real time, replayed with the
behaviour this story proposes. Bar numbers are Live's own (1-based). Note names are Live's
(`C3` = 60, the kick pad `C1` = 36). Where a reply says "held Live N ms", the number is the
`main_ms` the Remote Script already reports on every reply.

---

## 1. A bassline by the bar, not by the character

**Producer:** eight bars of that off-beat bass on F, with a turnaround in the last bar.

Today this is one 128-character step string, or `loop_every: 4, until: 32` plus a second
call for bar 8. Proposed: the span is `bars`, a one-bar string repeats to fill it, and a bar
that differs is named.

**Claude calls** `add_notes_to_clip`
```json
{"track_index": 2, "clip_index": 0, "clear": true,
 "bars": 8,
 "steps": {"F1": "..8...8...8...8."},
 "bar_steps": {"8": {"F1": "..8...8...8.9.9."}}}
```

**Result:**
```
Bass slot 1 'Bass/Groove': 34 notes over 8 bars (bar 1 repeated ×7, bar 8 its own).
F1 = 41. Velocities: 8 → 112, 9 → 126.
Empty in this clip: nothing (one pitch).
```

The velocity line is the mapping made visible: `1–9` is `14 × digit`, `x` is `velocity`
(100), `X` is `accent_velocity` (120). It also lives in the tool description, so the choice
between `8` and `X` is deliberate.

## 2. Mixed lengths are refused, with the answer

**Producer:** (Claude generates a kick, a hat and a clap line; the clap comes out at 128
characters because the generator tiled it.)

**Claude calls** `add_notes_to_clip`
```json
{"track_index": 0, "clip_index": 0, "bars": 1,
 "steps": {"C1": "x...x...x...x...", "F#1": "..x...x...x...x.",
           "D1": "....X.......X.......X.......X.......X.......X.......X.......X.......X.......X.......X.......X.......X.......X.......X.......X..."}}
```

**Result (error):**
```
steps: the lines do not describe the same span. C1 and F#1 are 16 steps (1 bar at step
0.25); D1 is 128 steps (8 bars). Make D1 16 steps, or set bars: 8 so the 16-step lines
repeat under it.
```

Today (`src/notes.rs`, 2026-09-19) this call is accepted and the clap is written across
eight bars of a one-bar clip, silently.

## 3. The piano break, in the key

**Producer:** give me a piano break — Fm9, Db maj7, Bbm7, Ab6, two bars each, spread
voicing with the bass note.

Today: twenty-two `notes_csv` rows with MIDI numbers worked out by hand. Proposed: chord
symbols expanded on the server against the set's key.

**Claude calls** `make_section`
```json
{"name": "Piano Break", "phrase_bars": 8,
 "clips": {"Piano": {"bars": 8,
   "chords": ["Fm9", "Db△7", "Bbm7", "Ab6"], "each": 2,
   "octave": 3, "voicing": "spread", "bass": true}}}
```

**Result:**
```
Section 'Piano Break' created as scene 9 (8-bar phrase) with clips on: Piano.
  Piano 'Piano Break/Piano' 8 bars, 4 chords in F minor:
    bar 1  Fm9   F2 · F3 Ab3 C4 Eb4 G4
    bar 3  Db△7  Db2 · Db3 F3 Ab3 C4
    bar 5  Bbm7  Bb1 · Bb2 Db3 F3 Ab3
    bar 7  Ab6   Ab1 · Ab2 C3 Eb3 F3
  All four chords sit in F minor.
Empty in this section (they stop when it fires): Kick, Hats, Clap, Bass, Perc, Stab.
Held Live 241 ms (a new row is one Live call; write it between sections or before the show).
Not in the setlist: jump_to it or add_to_song {"section": "Piano Break"}.
```

The resolved notes are in the reply so the caller sees what it got. A chord whose root is
outside the set's key is written and flagged: `Ab6 is outside F minor (Ab is in it; the 6th
F is) — fine` / `E7: E is not in F minor`.

## 4. A degree-written bassline moves with the key

**Producer:** same bass in the new key.

**Claude calls** `add_notes_to_clip`
```json
{"track_index": 2, "clip_index": 3, "clear": true, "bars": 4, "octave": 1,
 "steps": {"^1": "x...x...x...x...", "^b7": "......x.........", "^5": "..........x....."}}
```

**Result:**
```
Bass slot 4: 24 notes over 4 bars in F minor: ^1 = F1 (41), ^b7 = Eb1 (39), ^5 = C1 (36).
```

Degrees carry a caret so `"5"` stays MIDI note 5 and `"^5"` is the fifth of the key. The
octave is the call's `octave` (default 3, Live's middle-C octave); `^5+1` is the fifth an
octave up.

## 5. A copy of a section, plus a part it did not have

**Producer:** the peak is the build plus a stab. Put it right after the build.

**Claude calls** `make_section`
```json
{"name": "W3 Peak", "from": {"section": "W2 Build"}, "after": "W2 Build",
 "changes": {"Stab": {"bars": 1, "steps": {"^1": "x..x..x...x.x..."}, "octave": 2}}}
```

**Result:**
```
Section 'W3 Peak' created as scene 7, a copy of 'W2 Build · 8', with: Stab added (7 notes,
1 bar; W2 Build has no Stab clip). Kick, Hats, Clap, Bass, Perc are identical to W2 Build.
Placed after 'W2 Build' (scene 6).
Empty in this section (they stop when it fires): Piano.
Held Live 236 ms.
Not in the setlist: jump_to it or add_to_song {"section": "W3 Peak"}.
```

Three things that today either fail or are undocumented: a track absent from the source
appears in `changes` with notes and is *added* (this already works in the code and says
nothing about it); `W2 Build` was itself a copy (chained sources work today, undocumented);
`after` names any section (today refused for a copy, because Live's `duplicate_scene` only
inserts below its source — the copy is now `create_scene` at the place plus
`duplicate_clip_to` per track, one Remote Script command).

## 6. Eleven abandoned sections

**Producer:** forget the whole W set, take those rows out.

**Claude calls** `delete_section`
```json
{"sections": ["W1 Intro", "W2 Build", "W3 Peak", "W4 Break", "W5 Acid", "W6 Drop",
              "W7 Hats", "W8 Perc", "W9 Roll", "W10 Out", "W11 Tail"]}
```

**Result:**
```
Deleted 10 sections (scenes 3–12): W1 Intro, W2 Build, W3 Peak, W4 Break, W5 Acid, W6 Drop,
W7 Hats, W8 Perc, W9 Roll, W10 Out — 47 clips went with them.
Kept 'W11 Tail' (scene 13): it is playing. Jump elsewhere first, then delete it.
The song had W2 Build and W6 Drop in it: removed from the setlist as well.
Song: A1 Intro → [A2 Acid: looping] → A3 Break → A4 Drop.
Held Live 118 ms.
```

The playing or queued section is refused the way `delete_clip` refuses a playing clip; the
rest go. Sections in the setlist are removed from it in the same call (the alternative,
refusing, would cost a round trip to `remove_from_song` and back).

## 7. The drop that names its bar

**Producer:** piano break now, with a one-bar drop into it.

It is bar 62.3 of an 8-bar phrase that ends at bar 65.

**Claude calls** `jump_to`
```json
{"section": "Piano Break", "transition": {"drop": {"bars": 1, "keep": []}}}
```

**Result today** (three round trips of it, each a bar later):
```
transition.drop: the drop would start at bar 62, before the next bar (63); jump later or shorten it
```

**Result proposed** (the default `at` is the end of the phrase, so the server moves to the
first phrase end the drop fits before):
```
Jumping to 'Piano Break' at bar 65 (end of this phrase); it loops until go.
  bar 64      drop: Kick, Hats, Clap, Bass, Perc, Stab out (silence) for the last bar before the jump
  bar 65      fire scene 9 'Piano Break · 8'
'Piano Break' is not in the setlist: it holds until you say go; next_section resumes the
song at 'A2 Acid'. add_to_song {"section": "Piano Break", "after": "A1 Intro"} splices it in.
Cue 14 replaces cue 12.
```

Had the phrase ended at bar 63 (the drop could not fit before it), the reply would say:
```
Jumping to 'Piano Break' at bar 71 (end of the next phrase: a 1-bar drop needs the jump at
bar 64 or later, and bar 63 is this phrase's end) …
```

And with an explicit bar that cannot fit, the refusal carries the answer:
```
transition.drop: a 1-bar drop before bar 63 would start at bar 62, which has passed (it is
bar 62.3). Earliest: at: 64. Or shorten: no drop fits before 63.
```

## 8. Hearing it before the room does

**Producer:** is the peak too busy?

Live's API cannot render a scene that is not playing, and resampling records the master; a
per-track spectrum is not available without playing it (the story says so once, in
`capture_mix`). What the server *does* have is every note it wrote.

**Claude calls** `get_context`
```json
{"section": "W3 Peak"}
```

**Result:**
```
Section 'W3 Peak · 8' (scene 7), 7 tracks with clips, 1 empty (Piano).
  onsets/bar   range        track     clip
  4            C1           Kick      W3 Peak/Kick
  16           F#1          Hats      W3 Peak/Hats
  8            F#1–A#1      Perc      W3 Peak/Perc
  2            D1           Clap      W3 Peak/Clap
  8            F1–Ab1       Bass      W3 Peak/Bass
  7            F2–C3        Stab      W3 Peak/Stab
  45 onsets per bar across 7 tracks; 24 of them on 16ths from Hats and Perc, which share
  F#1–A#1. The last section that ran (W2 Build) had 29 per bar.
Levels the last time it played: master −3.1 dB peak (2 passes, bar 41–56).
```

Onsets per bar and the shared pitch ranges say "two hi-hat layers and a percussion loop in
the same register" before anyone hears it. The peak level (−3 dB, healthy) never would.

## 9. The name the guidance gave is the name the tool has

**Producer:** how loud is it?

**Claude calls** `listen` `{"bars": 2}` — the artist's name, no prefix, because it is in
`CORE_TOOLS` after this story. Today the reply of `get_track_meters` and the text of
`clear_captures` say `listen`, the served name is `adv_listen`, and the call fails with
tool-not-found mid-set. A test now reads every tool name out of the instructions, the footer
and every description and requires it to be a served name.

**Result:**
```
Listened 2 bars (57–58) at 130 BPM:
  master  −3.4 dB peak · −9.8 dB average
  Kick    −6.0 · Hats −11.2 · Bass −7.4 · Perc −12.0 · Clap −9.1 · Stab −10.5
  silent: Piano
Peaks in dB from Live's meters (0 dB is the top). listen {"capture": true} records the bars
instead and adds RMS and low/mid/high balance.
⏱ bar 59.1 · next bar in 1.8 s · phrase ends bar 65 · next jump: bar 65 fire scene 9 (cue 14)
🔊 master −3.4 dB peak this bar · Kick −6 · Bass −7 · Stab −10
```

## 10. What a call costs, said before it runs

From `load_instrument_or_effect`'s description after this story:

> Loads in one Live call the audio engine waits for: 0.5–1.5 s on Live 12 for an
> instrument (measured; the reply says what this one held). While a set plays, load onto a
> spare track between sections or before the show; there is no background load in Live's
> API.

And the reply when it was called mid-set anyway:
```
Loaded Grand Piano onto Piano (device 1). Held Live 1501 ms — longer than a beat at 130 BPM
(462 ms); during a performance this is audible. Load between sections, or keep a spare
track with the instrument ready.
```

---

## What the review should decide

1. Bar-oriented entry: `bars` + `bar_steps` (proposed) against the notes' `{"bar", "repeat",
   "except"}` object. Both say the same thing; the proposal keeps `steps` as the one grammar.
2. Degrees with a caret (`^b3`) against a call-level `pitches: "degrees"` switch.
3. Roman-numeral chords (`i9`, `VI△7`) as the "cannot be wrong" form beside chord symbols.
4. `delete_section` as a tool (housekeeping, beside `delete_clip`) rather than an action
   on `make_section`.
5. Clamp forward for a phrase-timed transition that does not fit; refuse-with-answer for an
   explicit bar.
6. Promote `listen`, `get_drum_rack_pads` and `get_clip_notes` into `CORE_TOOLS` under their
   current names, and leave `arrange` where it is.
