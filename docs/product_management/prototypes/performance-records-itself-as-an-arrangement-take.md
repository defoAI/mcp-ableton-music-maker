# Prototype transcript — the performance records itself as an Arrangement take

Fake data, real tool names and parameter names. Written 2026-09-19, not yet reviewed.

The producer here is performing, not writing. They have a set with sections in the Session
View and they want to play it. Today the performance is gone the moment it ends: the tools
fire scenes and schedule cues, and nothing is written down. These scenes assume it is.

Bar numbers are Live's own (1-based). **When the Arrangement already holds something, the
server asks what to do with it before a note plays** — it never decides on its own, and it
never overwrites without being told to in so many words.

---

## 1. The Arrangement already has something in it — so it asks

The Arrangement holds an earlier version of the track, ending at bar 128 on the longest
track.

**Producer:** play the song

**Claude calls** `play_song`
```json
{}
```

**Result:**
```
Not started — your Arrangement already has 128 bars on 5 tracks, and I keep performances as
a take in the Arrangement. What should I do with what is there?

  after    record the take from bar 129, leaving those 128 bars alone
  replace  delete those 128 bars and record the take from bar 1
  off      play without recording; the Arrangement is untouched

Say which, or "play it again without recording" any time. I will remember your answer for
the rest of this session.
```

**Claude:** Your Arrangement already has 128 bars in it. Want me to record this take after
them (from bar 129), replace them, or just play without recording?

### 1a. The producer answers

**Producer:** after them

**Claude calls** `play_song`
```json
{"record": "after"}
```

**Result:**
```
Playing "Night Drive" — 6 sections, 96 bars.
Intro · 8 → Verse · 16 → Build · 8 → Drop · 32 → Break · 16 → Outro · 16

Recording this take from bar 129. The 128 bars before it are untouched.
Live still has to be saved by hand when you like it.

Now: Intro, bar 1 of 8. Next: Verse at bar 9.
```

**Claude:** Rolling — Intro's playing and the take is recording from bar 129.

---

## 2. Steering mid-performance — the take keeps rolling

**Producer:** go to the drop

**Claude calls** `jump_to`
```json
{"section": "Drop"}
```

**Result:**
```
Drop lands at bar 33, at the end of Verse's phrase — cuts 6 bars of it.
Recording continues; the take is at bar 161 in the Arrangement.

Now: Verse, bar 10 of 16. Next: Drop at bar 33.
```

---

## 3. Ending it — what the take became

**Producer:** that's it, bring it down

**Claude calls** `end_performance`
```json
{"fade": true}
```

**Result:**
```
Fading over 4 bars, stopping on the bar.

Take recorded: bars 129–214 of the Arrangement, 86 bars on 5 tracks.
Back to Arrangement is on, so the tracks follow the timeline again.
Press Cmd+S in Live to keep it.
```

**Claude:** Done — 86 bars from bar 129. Nothing before bar 129 was touched. Save in Live if
you want to keep it.

---

## 4. A second performance — it remembers the answer

Same session, straight afterwards. It does not ask again.

**Producer:** again, but start from the drop

**Claude calls** `play_song`
```json
{"from_section": "Drop", "record": "after"}
```

**Result:**
```
Playing "Night Drive" from Drop — 3 sections, 64 bars.
Drop · 32 → Break · 16 → Outro · 16

Recording this take from bar 215, after the last one.

Now: Drop, bar 1 of 32. Next: Break at bar 33.
```

---

## 5. Replacing what is there — only when asked in so many words

A later session. The producer has decided the old arrangement is not worth keeping.

**Producer:** play it and replace that old arrangement with the take

**Claude calls** `play_song`
```json
{"record": "replace"}
```

**Result:**
```
Playing "Night Drive" — 6 sections, 96 bars.

Deleted 214 bars on 5 tracks and recording this take from bar 1, as you asked. There is no
undo in this server — Cmd+Z in Live is the way back, and only before you save.

Now: Intro, bar 1 of 8. Next: Verse at bar 9.
```

**Claude:** Old arrangement cleared, recording from bar 1. If that was not what you meant,
Cmd+Z in Live now — before you save.

---

## 6. Opting out

**Producer:** play it again but don't record this one

**Claude calls** `play_song`
```json
{"record": "off"}
```

**Result:**
```
Playing "Night Drive" — 6 sections, 96 bars. Not recording; the Arrangement is untouched.

Now: Intro, bar 1 of 8. Next: Verse at bar 9.
```

---

## 7. An empty Arrangement — nothing to ask about

A fresh set: sections in the Session View, nothing in the Arrangement.

**Claude calls** `play_song`
```json
{}
```

**Result:**
```
Playing "Night Drive" — 6 sections, 96 bars.
Recording this take from bar 1 — the Arrangement was empty, so there was nothing to ask
about. Say "don't record" if you would rather I did not.

Now: Intro, bar 1 of 8. Next: Verse at bar 9.
```

---

## 8. Live 10 — the take cannot be placed, so it is not recorded

Live 10 has no `track.arrangement_clips`, so the server cannot see what is in the
Arrangement and cannot ask a question it knows the answer to.

**Result:**
```
Playing "Night Drive" — 6 sections, 96 bars.
Not recording: this Live (10) does not report Arrangement clips, so I cannot tell what is
already there, and I will not record over it blind. Live 11 or newer records the take.

Now: Intro, bar 1 of 8. Next: Verse at bar 9.
```

---

## 9. The producer left a track armed

`play_song` disarms by default (`src/tools.rs:4721`), which is also what keeps Arrangement
Record from capturing the audio interface. With `disarm: false` the producer has said they
want it armed, and the take captures that track's input as a new recording.

**Claude calls** `play_song`
```json
{"record": "after", "disarm": false}
```

**Result:**
```
Playing "Night Drive" — 6 sections, 96 bars.
Recording this take from bar 129.
'Vocals' stays armed at your request: Arrangement Record captures its input as audio, so
that track records what is coming into it, not the clips.

Now: Intro, bar 1 of 8. Next: Verse at bar 9.
```

---

## Open questions the transcript raised

1. **How long is the answer remembered?** Scene 4 remembers "after" for the session. Should
   `replace` ever be remembered, or always be said again? (Proposed: never remembered — the
   Arrangement it would delete is different material each time.)
2. **A locator per section** would make a take navigable, but it is one `create_locator` per
   section launch on Live's main thread mid-performance (decision 0007, issue #43). Not
   built for that reason — worth the cost, or written at `end_performance` from the jump
   history the server already holds?
3. Should there be a **blank bar between takes**? Scene 4 uses none.
4. Scene 3 says "Back to Arrangement is on". Should `end_performance` always do that, or
   only when it recorded?
5. The cue ramps (tempo, crossfader, volume) land as **automation** in the take because Live
   writes them. Keep, or is a take meant to be clips only?
6. Scene 1 costs the producer a round trip before any sound. Is the question worth it every
   first performance of a session, or should a set whose Arrangement has never been recorded
   into by this server just append quietly?
7. Scene 5 deletes before playing. If the producer then says "stop" in the first bar, the old
   arrangement is gone and the take is one bar. Should `replace` instead record after, and
   delete the old material only at `end_performance`?
