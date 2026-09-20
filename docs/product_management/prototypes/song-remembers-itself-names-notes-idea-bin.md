# Prototype transcript — the song remembers itself

**Nothing below runs yet.** Unlike [samples-land-in-the-song](samples-land-in-the-song.md),
whose replies are verbatim from a real Live, every reply here is *proposed* text, written to
be read as text and argued with before any of it is built. Lines that state a fact about
today's system are anchored to a `file:line`.

The producer is mid-track: ten tracks, six sections, a sitar lead, a bass line they played
themselves. Everything Claude knows about it lives in the conversation and dies with it.

---

## The rule the whole design hangs on

**Anything Live can hold, Live holds.** What Live can hold was measured, not assumed — a
`describe` sweep of a real Live 12.4.6 (`tests/fixtures/live-lom-12.4.6.json`, recorded
2026-09-20 with script 1.34.2, produced by `scripts/live-lom-sweep.py`):

| Class | Writable text |
|---|---|
| `Song` | `scale_name` |
| `Track` | `name` (and routing strings) |
| `Scene` | `name` |
| `Clip` | `name` |
| `CuePoint` | `name` |

**The only writable text in Live's object model is a name.** There is no info-text member, no
comment field and no arbitrary data store — Cycling '74's reference
(`docs/reference/ableton/live-object-model.md`) says the same.

So the memory splits three ways, and the split is the design:

| What | Where it lives | Why there |
|---|---|---|
| Parked ideas (the stash) | **Live** — clips in a `Stash:` scene row | An idea *is* a clip. Cmd+S keeps it, it travels inside the `.als`, and the producer can fire it to hear it. Follows the `Setlist:` scene `set_song` already writes (`src/song.rs`) |
| A track's role | **Live** — a suffix on the track name, `Sitar [lead]` | The only in-Live home for a word about a track. Follows the section convention `"<name> · <bars>"` |
| Section lengths, key, tempo, locators, clip names | **Live**, as today | Already true; `get_context` reads them every call (`src/context.rs`) |
| The note log **and the song overview** | **The server's file**, `state_dir()/songs/<key>.json` | Dated paragraphs, and the agent's own structured model of the track. Live has no field for either, and — decisive — **anything inside Live is only as durable as Cmd+S, which the API cannot press.** The crash in #46 happens before the save |

The file therefore holds **the overview, the notes, the set's identity and the reconciliation map.
Nothing else.** One copy of every fact — and where the overview repeats something Live owns, it is
stamped `as_of` and checked against Live on load rather than believed.

---

## 0. The first hour — an unsaved set, and the memory starts anyway

**Producer:** let's start something in D Dorian at 88

**Claude calls** `set_key`, then `build_song` … *(unchanged)*

**Result** (last two lines new):
```
Built 'Vienna Raga': 6 sections, 10 tracks, 34 clips, 1,208 notes. Key D Dorian, 88 BPM.
Notes for this set are being kept under the provisional name "unsaved-2026-09-20-14:22" — this Live
set has never been saved, so they move to the set's own name the moment you press Cmd+S (the API
cannot press it).
```

> Identity is `song.file_path`, documented as *"the path to the current Live Set… if the Live
> Set hasn't been saved, the path is empty"*
> (`docs/reference/ableton/live-object-model.md:223`). The server reads it with a generic
> `run` op: `song.file_path` passes `Path::validate` (`src/lom.rs:90`) and the script's
> `PATH_ROOTS` (`AbletonMusicMaker_Remote_Script/__init__.py:236`), so **no new Remote Script
> command and no `SCRIPT_VERSION` bump**. The script already reads the same member for
> `_project_dir` (`__init__.py:1393`).

---

## 1. The next morning — one call, and the producer is oriented

**Producer:** where were we

**Claude calls** `get_context` — *the same one call as today, no new tool*

**Result** (everything above `Live 12.4.6` is new):
```
Vienna Raga · 6 sections · last worked on 2026-09-20 · overview written 2026-09-20
Roles (from the track names): Sitar [lead] · Tanpura [drone] · Smoke Bass [nick's own take]
Last note: "harsh at first — cutoff to 78, reopened to 90 after the spectral check" (Sitar, yesterday)
Stash: 3 ideas in the Stash: row — rhodes lick, sitar answer, 7-step pluck
Overview (as you left it):
{
  "intent": "Raga-leaning downtempo, 88 BPM, D Dorian. Reference: Talvin Singh 'Traveller' — organic,
             not a club track. Long-form: nothing repeats more than twice without changing.",
  "form": {
    "Intro":      "tanpura drone alone, 8 bars, no drums",
    "Verse":      "tabla + bass enter, sitar answers every 4 bars",
    "Breakdown":  "everything out but drone + one sitar phrase — this is the emotional low",
    "Drop":       "full kit, the answer phrase doubled an octave up",
    "Verse 2":    "as Verse, bass plays the D-D-C-A line an octave down",
    "Outro":      "reverse of the intro"
  },
  "tracks": {
    "Sitar":      "the lead. Brightness comes from the EQ, NOT the filter — 2-4 kHz is where 'harsh' lives.",
    "Smoke Bass": "Nick played this himself. Never regenerate, requantize or replace. D-D-C-A.",
    "Tanpura":    "the drone. Never mute it — it is the only thing holding the key through the breakdown.",
    "Tabla":      "pad 40 is 'Snare Dubplate'; the tihai goes in the bar before each drop"
  },
  "decided": [
    "88 BPM, tried 92 and it lost the sway",
    "no sidechain — it made the drone pump and Nick hated it"
  ],
  "next": "the Drop is thin. Try the answer phrase doubled an octave up before touching the drums.",
  "open": ["does the Outro need the tabla or not?"],
  "as_of": {"tempo": 88, "key": "D Dorian", "sections": 6, "tracks": 10}
}
Live 12.4.6 · script 1.34.2 · 88 BPM 4/4 · stopped at bar 1.1 · launch quantization 1 bar · key D Dorian
Tracks (10):
  …
```

**Claude:** Vienna Raga, D Dorian at 88, six sections, and I have the plan back: the Drop is
thin and we said the next move is the answer phrase doubled an octave up, before touching the
drums. Smoke Bass stays untouched. Start there?

> **This is the retrieval, and it costs the agent nothing** — the overview rides on the
> `get_context` it makes first anyway. No second call, no "do I have a memory?" decision.
> Roles and the stash come out of the set Live just handed over; only the note line and the
> overview are read from disk.
>
> `as_of` is what Live said when the overview was written. The header compares it with Live
> now, and a drift is stated rather than believed — see scene 4.

---

## 2. "Make the lead louder from the drop" — no lookup, no numbers

**Producer:** make the lead louder from the drop

**Claude calls** `set_track_mixer`
```json
{"track": "lead", "volume_db": -4.5}
```

**Result:**
```
'lead' is Sitar [lead]. Volume -6.0 → -4.5 dB.
```

**Producer:** and bring the answer phrase in at the drop instead of bar 33

**Claude calls** `arrange`
```json
{"action": "move", "track": "Sitar", "clip": "sitar answer", "at_bar": "drop"}
```

**Result:**
```
Moved 'sitar answer' on Sitar [lead] from bar 33 to bar 41 ("drop" — the locator at 41).
```

> Three changes, all inside tools that exist:
>
> **A clip can be named.** Today `arrange` takes `clip: Option<i64>` (`src/arrange.rs:358`),
> and `create_clip`, `add_notes_to_clip` and `delete_clip` take raw `track_index` and
> `clip_index` (`src/tools.rs:471`, `:532`, `:370`) — four **core** tools still speaking in
> indices while the instructions promise "tracks and sections are addressed by name"
> (`src/context.rs:19`). This is the producer's "I referred to track 8 slot 1 all afternoon".
>
> **A bar can be named by its locator.** `create_locator` already makes them and Live saves
> them. Reading them back needs no new command: one `run` op gets `song.cue_points` — the
> script's `_jsonable` renders each as a string, so the list gives the *count*
> (`__init__.py:5567`) — then one batch of `name` and `time` gets by index.
>
> **A role resolves like a name.** `resolve_track_name` (`src/tools.rs:1215`) also matches
> the token inside `[…]`, so "the lead" needs no alias table. **There is no `alias` tool**: a
> role is in Live's track name and a named bar is a locator, both visible to the producer and
> both kept by Cmd+S. A third naming system would drift from the two real ones.

---

## 3. The notebook — written while working

**Producer:** that's still harsh

**Claude calls** `shape_sound` … then `remember`
```json
{"about": "Sitar", "note": "'harsh' twice now — it is the 2–4 kHz, not the cutoff. Fixed with the EQ, not the filter."}
```

**Result:**
```
Noted about Sitar [lead]. 4 notes on this track.
```

**Producer:** and don't ever touch the Smoke Bass, that's me playing

**Claude calls** `remember`
```json
{"about": "Smoke Bass", "role": "nick's own take", "note": "recorded by hand — do not regenerate, requantize or replace"}
```

**Result:**
```
Renamed the track to 'Smoke Bass [nick's own take]' so the role is in the set itself, and noted why.
```

**Claude:** It's on the track now, so it survives without me — and the note is in front of me
next session.

> Honest about what it does: the note is **text Claude reads**, not a lock. No tool refuses
> anything because of it — every track works the same. A story that promised enforcement here
> would be promising something the producer could not rely on; if a protected flag is wanted
> later it is its own story, with the refusal wired into `delete_track` and `delete_clip`
> where it can be kept.
>
> Reading back is `remember` with no `note`: `{"about": "Sitar"}` returns the four notes with
> their dates. There is no second `notes()` tool — `get_context` carries the latest already.
> `about` takes `"song"`, a track name, or `"section:Breakdown"`.

---

## 4. The overview — the agent's own model, left behind on purpose

**Producer:** ok that's the breakdown sorted, I'm going to get lunch

**Claude calls** `remember`
```json
{"overview": {
   "next": "Breakdown is done. The Drop is still thin — answer phrase an octave up, then re-measure.",
   "decided": ["88 BPM, tried 92 and it lost the sway",
               "no sidechain — it made the drone pump and Nick hated it",
               "breakdown: drone + one sitar phrase only, no tabla"]
 }}
```

**Result:**
```
Overview updated (2 of 8 keys changed: next, decided). 2.1 KB of 8 KB. It is read back to you at the
top of every get_context.
```

**Claude:** Saved where we are. When you're back, I'll pick up at the Drop.

> **Merge, not replace.** A patch touches the keys it names and leaves the rest; `replace:
> true` is there for a rewrite. Otherwise a one-key update would silently erase the form and
> the track model.
>
> **A cap, deliberately.** 8 KB stored, and the whole of it is served in every `get_context`
> — one rule, no "is it small enough today?" branch. Over the cap, the write is refused with
> what to trim; a mental model that does not fit in a header was going to be ignored anyway.
>
> **A light schema, with an escape hatch.** `intent`, `form`, `tracks`, `decided`, `next`,
> `open`, `as_of` are the keys the header knows how to render and the instructions name; any
> other key is kept verbatim. Free-form JSON alone would mean every session invents a shape
> and the next one cannot rely on it.

---

## 4b. When the overview and the set disagree

**Producer:** *(over lunch, adds two sections and drops the tempo to 84 by hand)*

**Claude calls** `get_context`

**Result** (the new line is the last one before Live's own readout):
```
Vienna Raga · 8 sections · last worked on today · overview written 2 hours ago
…
Overview (as you left it): { … "as_of": {"tempo": 88, "key": "D Dorian", "sections": 6, "tracks": 10} }
The set has moved since: 84 BPM (was 88) and 8 sections (was 6). Trust Live, not the overview, and
update it.
Live 12.4.6 · script 1.34.2 · 84 BPM 4/4 · …
```

**Claude:** You changed the tempo to 84 and added two sections — I'll work from the set, not
from my notes. What are the two new ones for?

> The overview is the only place in this design where a fact Live owns may be repeated, and it
> is repeated **stamped**. The rule the header enforces: Live is the truth, the overview is a
> memory of it, and a disagreement is reported rather than resolved silently. `as_of` is
> written by the server, not the agent — the agent cannot get it wrong or forget it.

---

## 5. The idea bin — ten sketches, in Live, audible

**Producer:** try a few answer phrases for the breakdown, don't put them in the song yet

**Claude calls** `stash` three times
```json
{"action": "save", "name": "answer A", "track": "Sitar", "tags": ["breakdown"], "bars": 2,
 "steps": {"D4": "x..x..x.", "A4": "....x..x"}}
```

**Result:**
```
Stashed 'answer A' on Sitar [lead] — clip 'answer A [breakdown]' in the Stash: row (scene 8, slot 0).
2 bars, 6 notes. Fire it in Live to hear it; it is not in any section and the setlist ignores it.
```

**Producer:** what have we got

**Claude calls** `stash` `{"action": "list"}`

**Result:**
```
Stash: 6 ideas in 2 rows (scenes 8–9), on 3 tracks:
  answer A      Sitar [lead]   2 bars · 6 notes    breakdown
  answer B      Sitar [lead]   2 bars · 9 notes    breakdown
  answer C      Sitar [lead]   4 bars · 11 notes   breakdown
  rhodes lick   Rhodes         4 bars · 24 notes   keys
  sitar answer  Sitar [lead]   2 bars · 7 notes    (also in Breakdown)
  7-step pluck  Pluck          2 bars · 14 notes   odd
```

**Producer:** play me B … yeah, put that in the breakdown

**Claude calls** `stash`
```json
{"action": "place", "name": "answer B", "section": "Breakdown"}
```

**Result:**
```
Copied 'answer B' into Breakdown on Sitar [lead] (slot 3). The stash copy stays where it is.
```

> Nothing the server stores. A stash row is any scene whose name starts with `Stash:` — it
> has no `·`, so `src/song.rs` never reads it as a section and the setlist never plays it,
> exactly as the `Setlist:` scene works today. `save` fills the first `Stash:` row with a free
> slot on that track and makes another row when they are full. Tags ride in the clip name in
> brackets, the same convention as a role.
>
> The trade this accepts: **a clip must live on a track**, so a stashed idea belongs to its
> track and cannot travel to another song. "The bassline from the last track" is a
> cross-song library — a different object, out of scope here and named as such.
>
> Under the hood: `create_scene`, `set_scene`, `create_clip`, `set_clip_name`,
> `get_clip_notes`, `add_notes_to_clip` — **every one already served** (`ALL_REMOTE_COMMANDS`).

---

## 6. Cmd+S, a rename, a reorder — the memory survives the producer

**Producer:** *(saves as `Vienna Raga.als`, renames `Sitar [lead]` to `Main Sitar [lead]`, drags it above the drums)*

**Claude calls** `get_context`

**Result:**
```
Vienna Raga · 6 sections · notes last written today
This set was saved since the notes were written: "unsaved-2026-09-20-14:22" is now Vienna Raga
(~/.ableton-music-maker/songs/vienna-raga-3f2a91c4.json).
One track was renamed and moved: "Sitar" (track 4) is now "Main Sitar [lead]" (track 2) — its 4
notes moved with it. Its role came along in the name, as it always does.
Live 12.4.6 · script 1.34.2 · 88 BPM 4/4 · …
```

> Reconciliation only ever has to find *notes* a home — roles and the stash are in the set and
> cannot be orphaned, which is most of the reason to put them there. The rule: match by name,
> then by position and device chain, and when it is not certain **say so and leave the notes
> unattached rather than guess**. Every re-attachment is reported. A track the notes know and
> the set no longer has is kept, not deleted — the producer may be mid-undo.

---

## 7. The producer's side of it

**Producer:** what are you keeping about my tracks?

**Claude calls** `adv_song_memory` `{"action": "show"}`

**Result:**
```
Vienna Raga — ~/.ableton-music-maker/songs/vienna-raga-3f2a91c4.json, 6 KB, written 3 minutes ago.
The song overview I keep as we work (2.1 KB, 8 keys), 9 notes (5 tracks, 2 sections, the song), the
set's path, and which track each note belongs to.
Not in it: your roles and stashed ideas (those are in the Live set itself), tempo, key, section
names, track names, clip names, locators, notes you played, any audio.
On this machine only. Nothing is uploaded and no code in this server can.
Off: ABLETON_MCP_SONG_MEMORY=false. Delete: adv_song_memory(action: forget).
```

**Producer:** forget this one

**Result:**
```
Deleted ~/.ableton-music-maker/songs/vienna-raga-3f2a91c4.json — the overview and 9 notes are gone.
The Live set is untouched: the roles in your track names and the Stash: row are still there, because
they are yours.
```

---

## 8. What it says when it cannot do the thing

| Situation | Reply |
|---|---|
| Two clips called "answer" on one track | `arrange: 'answer' matches 2 clips on Sitar [lead] — 'answer A' (slot 3) and 'answer B' (slot 5). Name one, or give the slot.` |
| A locator that is not there | `arrange: no locator called "drop". Locators in this set: "intro" (1), "verse" (9), "break" (25). Give a bar number, or make one with create_locator.` |
| A role that matches two tracks | `set_track_mixer: 'lead' matches Sitar [lead] and Vox [lead vox]. Name the track.` |
| `stash place` with no free slot in the section | `stash: Breakdown is full on Sitar [lead] (slot 3 holds 'sitar answer'). Replace it, or name another section.` |
| Notes switched off | `remember: song memory is off (ABLETON_MCP_SONG_MEMORY=false), so the note and the overview were not written. The role went into the track name anyway — that is in your set, not mine.` |
| An overview over the cap | `remember: that overview is 11.4 KB and the cap is 8 KB, so nothing was written. It is read back in full on every get_context, which is why it stays small — trim "form" (6.2 KB) or move the detail into notes on the tracks it is about.` |
| An overview that is not an object | `remember: overview must be a JSON object of keys, not a string. The keys the header renders are intent, form, tracks, decided, next, open; any other key is kept as you write it.` |
| An unsaved set, reopened after a restart | `Two sets of notes have no set: "unsaved-2026-09-20-14:22" (9 notes) and "unsaved-2026-09-19-11:03" (2). If this is one of them, attach it with adv_song_memory(action: attach, name: …).` |

---

## 9. The scene that decides whether any of it is used

The tools are the easy half. #46 was not that `export_set` was missing — it existed
(`src/sets.rs`) — but that nobody called it. So the instructions and the four copy-and-paste
prompts carry the memory, or the memory is furniture.

**`src/context.rs` `INSTRUCTIONS`, START paragraph, gains:**

> …and this song's memory: the **overview** you left last time (the plan, what each track is
> for, what was decided and what comes next), the **roles in the track names**, the **ideas in
> the `Stash:` row**, and the **notes**. It is already in front of you — there is nothing to
> fetch. Where the overview disagrees with the set, the set is right and the header says so.
> Write as you work: `remember(overview: …)` to keep the plan current — after a build, after a
> round of changes, and before you stop, patching only the keys that moved; `remember(note: …)`
> for why a change was made or what never to touch; `stash` for material that is not in the
> song yet. A session that ends with nothing remembered is a session the next one starts
> blind.

**`prompts/make-a-song.md`**, in "How I want you to work":

> - Remember as you go. Keep a short `remember(overview: …)` of this song current — what we are
>   making, what each track is for, what we decided and what is next — and update it after each
>   round rather than at the end. When I tell you what a track is, put it in the role so it
>   lands in the track name and my set keeps it. Park ideas I have not chosen with `stash`
>   instead of dropping them into the song.

**`prompts/finish-what-i-have.md`**, in its opening steps:

> 2. Read this set's memory back to me in one line before you change anything — the overview,
>    the roles, the last note, what is in the stash. If the overview disagrees with what is
>    actually in the set, say so and believe the set.

`tests/prompts.rs` (new on main, 2026-09-20) already fails the build when a tool-shaped name
in a prompt is not served in the spelling used, so `remember` and `stash` are checked the
moment they are written into a prompt.
