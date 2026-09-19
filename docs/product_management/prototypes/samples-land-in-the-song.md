# Prototype transcript — samples land in the song

Real tool names, real parameter names, and **real replies**: every result below is the text
that came back from a run against Ableton Live 12.4.6 with Remote Script 1.26.0, on a machine
whose Live folders hold 9,904 audio files. The producer's words are invented, and the track
indices are the test set's; the replies are verbatim.

The producer has a set and wants recorded audio in it: a break under a section, a crash on a
downbeat, a riser into the drop. Before this, Claude could only put audio in a Session slot if
the producer typed an absolute path, and could not find a sample at all.

Bar numbers are Live's own (1-based). A sample is **referenced in place**, exactly as dragging
the file into Live would do it; nothing is copied, moved or uploaded. **Fitting is on by
default**, and what it means depends on where the sample goes: a Session row plays through its
section, so it loops; a bar in the Arrangement is one hit, so it does not.

---

## 1. Finding a sample — the first search builds the index

**Producer:** find me a drum break around 90 bpm

**Claude calls** `search_browser`
```json
{"query": "break 90", "category": "samples", "limit": 3}
```

**Result:**
```
From the sample index: 9904 files in 3 folders (Core Library, Factory Packs, User Library), indexed in 2.0 s.
2 matches for "break 90":
  Break 90s Vinyl 88 bpm — Samples/Loops/Drums/Full    aif · 5.4 s
    path: /Applications/Ableton Live 12 Suite.app/Contents/App-Resources/Core Library/Samples/Loops/Drums/Full/Break 90s Vinyl 88 bpm.aif
  Break Ghosts 90 bpm — Samples/Loops/Drums/Full    wav · 5.3 s
    path: /Applications/Ableton Live 12 Suite.app/Contents/App-Resources/Core Library/Samples/Loops/Drums/Full/Break Ghosts 90 bpm.wav
Put one in the song with add_sample(sample, section or at_bar); words, a path or a uri all work.
```

**Claude:** Two, both about 2 bars: the vinyl one is dustier, Ghosts is drier. Which in the
Verse?

> Live is asked once where samples live (`list_sample_folders`); the walk itself is the
> server's own work and took 2.0 s for those 9,904 files — 1.3 ms of Live's main thread.
> Every later search is 8 ms and no round trip. The lengths come from each WAV or AIFF
> header, 4 KB a file, never from decoding one.

---

## 2. Into a section — creates the track, warps, loops

**Producer:** the vinyl one, in the Verse, on its own track

**Claude calls** `add_sample`
```json
{"sample": "Break 90s Vinyl 88 bpm", "section": "Verse"}
```

**Result:**
```
Added 'Break 90s Vinyl 88 bpm' to Verse on Break 90s Vinyl 88 bpm (new audio track 'Break 90s Vinyl 88 bpm', index 5).
5.4 s of audio, warped and looping 2 bars (Live heard 2 bars).
The file is referenced where it is: /Applications/Ableton Live 12 Suite.app/Contents/App-Resources/Core Library/Samples/Loops/Drums/Full/Break 90s Vinyl 88 bpm.aif — nothing copied.
```

**Claude:** It's in the Verse, warped and looping over 2 bars. Play the Verse to hear it
against the drums?

> One Remote Script command (`place_sample`) creates the clip and fits it in a single task,
> so it is **one undo step** in Live: Cmd+Z removes the clip, not the warp setting. Live's
> own Auto-Warp does the tempo detection — it heard 88 bpm here and landed on exactly 2 bars
> — and the reply states what was read back off the clip afterwards, never what was asked
> for. Measured cost: 35 ms of Live's main thread, plus ~100 ms because Live had to make the
> track.
>
> `track` was left out, so the track is named after the sample — which is why the track here
> is called "Break 90s Vinyl 88 bpm". Naming it is one word more:
> `{"sample": "…", "section": "Verse", "track": "Break"}`.

---

## 3. A one-shot at a bar — one hit, not a loop

**Producer:** put a crash at bar 5 on that track

**Claude calls** `add_sample`
```json
{"sample": "crash", "track": "Break 90s Vinyl 88 bpm", "at_bar": 5}
```

**Result:**
```
Added 'Crash LD' to the Arrangement on Break 90s Vinyl 88 bpm at bar 5. Covers bar 5 to bar 5.65.
1.3 s of audio, unwarped, as Live loaded it and not looping — one hit at that bar. Give `bars` to make it a loop of a set length.
The file is referenced where it is: /Applications/Ableton Live 12 Suite.app/Contents/App-Resources/Core Library/Samples/One Shots/Drums/Cymbal/Crash LD.aif — nothing copied.
Also matched: Crash 505, Crash 707, Crash 909 — say one to swap it.
```

**Claude:** Crash on the downbeat of 5. There are three others if you want a longer tail.

> A bar in the Arrangement is an event, so nothing is looped and Live's own warp decision
> stands. Ambiguity is answered by placing the best hit and naming the runners-up: undo is
> one keystroke, and a question would cost a round trip.

---

## 4. A loop at a bar, and what the timeline can actually hold

**Producer:** drop Ghosts at bar 9 and make it 4 bars

**Claude calls** `add_sample`
```json
{"sample": "Break Ghosts 90 bpm", "track": "Break 90s Vinyl 88 bpm", "at_bar": 9, "bars": 4}
```

**Result:**
```
Added 'Break Ghosts 90 bpm' to the Arrangement on Break 90s Vinyl 88 bpm at bar 9. Covers bar 9 to bar 11.
5.3 s of audio, warped and looping 4 bars as asked (Live heard 2 bars).
On the timeline it is 2 bars long, not 4 bars: Live gives an Arrangement clip the length of its material and the API cannot stretch that. `arrange repeat` fills the rest.
The file is referenced where it is: …/Core Library/Samples/Loops/Drums/Full/Break Ghosts 90 bpm.wav — nothing copied.
```

**Claude:** The clip loops 4 bars but Live only draws the 2 bars of material it has — want me
to repeat it to bar 13?

> `Clip.end_time` is read-only in Live's API, so an Arrangement clip is as long as its
> material. The reply says so rather than let "4 bars" read as four bars of timeline.

---

## 5. Transposed, and named

**Producer:** same break in the Intro, up three, call it "chop up 3"

**Claude calls** `add_sample`
```json
{"sample": "Break Ghosts 90 bpm", "track": "Break 90s Vinyl 88 bpm", "section": "Intro",
 "transpose": 3, "name": "chop up 3"}
```

**Result:**
```
Added 'chop up 3' to Intro on Break 90s Vinyl 88 bpm.
5.3 s of audio, warped and looping 2 bars (Live heard 2 bars). Transposed +3 semitones.
The file is referenced where it is: …/Core Library/Samples/Loops/Drums/Full/Break Ghosts 90 bpm.wav — nothing copied.
```

> `transpose` is Live's clip Transpose (`pitch_coarse`, −48…48). Nothing detects the sample's
> key: the producer said up three.

---

## 6. Pointing Claude at a folder

**Producer:** use my Splice folder too, it's in ~/Splice/sounds

**Claude calls** `adv_sample_folders`
```json
{"action": "add", "path": "/Users/nick/Splice/sounds"}
```

**Result:**
```
Added /Users/nick/Splice/sounds — 2318 audio files.
11222 files in 4 folders (Core Library, Factory Packs, User Library, sounds), indexed in 2.4 s
  Core Library       3567 files  /Applications/Ableton Live 12 Suite.app/Contents/App-Resources/Core Library/Samples
  Factory Packs      6337 files  /Users/nick/Music/Ableton/Factory Packs
  User Library          0 files  /Users/nick/Music/Ableton/User Library/Samples
  sounds             2318 files  /Users/nick/Splice/sounds  (you added this)
Live's own folders need no adding. The list of yours is kept in ~/.ableton-music-maker/sample_folders.json — paths only, never audio.
```

**Claude:** Splice is in. Searches cover it from now on.

> Live has the last word on whether the folder is there: the server adds it, asks Live for
> the folder list again, and only keeps it if Live names it back. A folder Live cannot see is
> refused and forgotten.

---

## 7. Refusals

Each of these came back from the real Live run.

### 7a. The track is a MIDI track
```
Could not put the sample in the set: '1-MIDI' is a MIDI track; a sample needs an audio track
```

### 7b. The row is taken
```
Could not put the sample in the set: slot 0 on 'Samples Test' already holds 'Break 90s Vinyl 88 bpm'
```

### 7c. Live cannot read the file — Live's own words
```
Could not put the sample in the set: The provided path does not appear to point to a valid audio file
```

### 7d. No such section
```
No section called "Chorus". Sections: Intro, Verse, scene 4. Make one with make_section, or give at_bar to place the sample in the Arrangement.
```

### 7e. Nowhere to put it
```
Where should it go? `section` puts it in that row so it plays with the song; `at_bar` puts it in the Arrangement.
```

### 7f. Both places at once
```
Give one place: `section` (a Session row) or `at_bar` (the Arrangement), not both.
```

### 7g. Nothing matches
```
No sample matches "tabla in seven eight". 9904 files in 3 folders (Core Library, Factory Packs, User Library), indexed in 2.0 s. Try fewer words, give the file's path, or add a folder with adv_sample_folders.
```

### 7h. A sample known only to Live's browser, asked for at a bar
```
'query:Samples#FileId_19408' is in Live's browser, and Live tells a client an item's file only once it is a clip — which the Arrangement needs. Put it in a section first (add_sample with `section`), then arrange it; or add its folder with adv_sample_folders.
```

Into a section that same sample works, and Live then says where its file is:

**Claude calls** `add_sample`
```json
{"sample": "query:Samples#FileId_19408", "track": "Break 90s Vinyl 88 bpm", "slot": 4}
```
```
Added '000_808KICK4' to slot 4 on Break 90s Vinyl 88 bpm.
0.3 s of audio: a one-shot, left as Live loaded it — unwarped, not looping.
The file is referenced where it is: /Users/nick/Music/Ableton/User Library/Samples/Imported/Dirt-Samples/future/000_808KICK4.wav — nothing copied.
```

> The script highlights the target slot (`Song.View.highlighted_clip_slot`) and calls
> `browser.load_item`, then reads the new clip's `file_path` back — so the sample Claude could
> not place at a bar a moment ago now has a path, and the name in the reply is the one Live
> gave the clip, not the URI.

### 7i. Live 11
```
Could not put the sample in the set: ClipSlot.create_audio_clip is unavailable in this Ableton Live version. Requires Live 12.0.5 or newer.
```

Finding samples still works on Live 11; only placing needs Live 12.

---

## What the review settled

- **Ambiguity**: place the best hit and name the runners-up (scene 3), rather than asking.
- **A missing `track`**: make an audio track named after the sample (scene 2) — and say so in
  the reply, since a new track is a visible thing.
- **A browser-only sample at a bar** (7h): refuse with the way round. A temporary Session clip
  that appears and vanishes to learn the file path was considered and rejected.
