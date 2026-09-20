# The song remembers itself: an overview the agent keeps, roles and ideas in the set

## Story
**As a** producer who worked on this track with Claude for an afternoon and came back a week later,
**I want** Claude to pick the song back up with the model of it it had when we stopped — the plan,
what each track is for, what we decided and why, what is mine and must not be touched, and the ideas
we parked —
**So that** the next session starts where the last one ended instead of with `get_context` and a guess.

## Details
| Field | Value |
|-------|-------|
| Status | `Ready` |
| Priority | P1 — the producer's session of 2026-09-20 (#50) lost every non-Live fact at the end of the conversation; #46 is the same loss by crash. Nothing else on the list makes a second session cheaper |
| Size | L — three tools, four core tools' addressing, the instructions, four prompts, a new suite |
| Tracker | [#63](https://github.com/defoAI/mcp-ableton-music-maker/issues/63) |
| Created | 2026-09-20 |
| Updated | 2026-09-20 |

## Context

### The Problem

Everything Claude knows about a track that is not readable from Live dies with the conversation:
that it is in D Dorian (Live holds this), that the drums are track 4 (Live holds this), but also
that **"Sitar" is the lead**, that **"Smoke Bass" is the producer's own playing and must never be
regenerated**, that **"harsh" meant the sitar and then the bass**, that the 7-step pluck was an idea
worth keeping. Next session begins with `get_context` and inference.

And the largest loss is not any single fact but the **model**: the plan for the form, what was
tried and rejected, what the next move was going to be. A fresh session can read the set and still
have no idea what the track is *for*.

Four separate costs in the same session (#50):

1. **Orientation.** The producer must re-explain the track every time.
2. **Addressing.** "Track 8 slot 1" all afternoon: four **core** tools still take raw indices while
   the instructions promise names.
3. **Ideas.** Material that is not in the song yet has nowhere to be. A sketch either goes into a
   slot immediately or is lost.
5. **The plan.** "The Drop is thin, next move is the answer phrase an octave up" is the most
   valuable sentence of the afternoon and the one most certain to be gone tomorrow.

### Current State

| Fact | Anchor |
|---|---|
| `get_context` is the one orientation call and reads only Live | `src/context.rs` |
| Sections are scene names `"<name> · <bars>"`, kept by Live's Save | `src/song.rs` |
| The setlist is a magic scene named `Setlist:`, never parsed as a section | `SETLIST_PREFIX`, `src/song.rs:17` |
| `export_set` writes a rebuildable document, only on an explicit call | `src/sets.rs`, `tests/sets.rs` |
| `build_song` can write that snapshot itself with `snapshot: true` | `src/tools.rs:905` |
| Tracks resolve by name, return letter or index | `resolve_track_name`, `src/tools.rs:1215` |
| **Seven core tools take a track by index only** — `set_track_mixer` (`src/tools.rs:614`), `set_send` (`:633`), `load_instrument_or_effect` (`:560`), `delete_track` (`TrackParams`, `:348`), `create_clip` (`:471`), `add_notes_to_clip` (`:532`), `delete_clip` (`ClipParams`, `:370`) — while five take it by name: `shape_sound` (`:426`), `feel`, `add_sample`, `record_clip`, `arrange` | audit of `CORE_TOOLS`, 2026-09-20 |
| `arrange` takes the track by name and the clip by integer, disagreeing with itself in one call | `src/arrange.rs:358` |
| Locators can be created and deleted, never listed | `create_locator` / `delete_locator`, `__init__.py:2022`, `:2767` |
| The server writes only under `state_dir()` | `src/state.rs` |
| The activity log is on by default, payloads off | `src/activity.rs`, `tests/activity.rs` |
| Live's API cannot save the set | stated once in `src/context.rs` |

**What Live can hold, measured not assumed** — `describe` of a real Live 12.4.6, recorded 2026-09-20
with script 1.34.2 (`tests/fixtures/live-lom-12.4.6.json`, via `scripts/live-lom-sweep.py`):

| Class | Writable text members |
|---|---|
| `Song` | `scale_name` |
| `Track` | `name`, routing strings |
| `Scene` | `name` |
| `Clip` | `name` |
| `CuePoint` | `name` |

**The only writable text in Live's object model is a name.** No info text, no comment field, no
arbitrary data store. Cycling '74's reference agrees (`docs/reference/ableton/live-object-model.md`).

### Root Cause

Two rules were followed correctly and left a gap between them. *Live holds the set* (so nothing
outside Live was built) and *the server writes only when asked* (so `export_set` is never called at
the moment it matters). Neither rule covers the facts that are **about** the set but have no field
in it: a role, a reason, a parked idea.

## Open Questions

| # | Question | Answer |
|---|---|---|
| 1 | Can all of it live inside Live's own objects, so there is no separate storage? | **Partly, and that is the design.** The sweep above shows the only writable text is a name, so: the stash is real clips in a `Stash:` scene row, and a role is a suffix on the track name. The **note log cannot** — it is dated paragraphs with history, and, decisively, anything inside Live is only as durable as Cmd+S, which the API cannot press. #46's crash happens before the save |
| 2 | One file or two? | **Two.** A small file per song under `state_dir()/songs/` holding the overview and the notes, and `export_set`'s heavy rebuildable snapshot unchanged. A note must not rewrite every MIDI note in the set |
| 3 | Beside the `.als`, so it travels with the project? | **No.** `src/state.rs` and the Docker read-only contract say the server writes under `state_dir()` only, and an unsaved set has nowhere to put it. What travels with the project is what is *in* the set — the roles and the stash — which is most of it |
| 4 | Written automatically, or only when asked? | **Automatically, on by default**, with `ABLETON_MCP_SONG_MEMORY=false` to turn it off. This is a privacy-default change and carries its `TERMS.md` row and a test that pins both the default and the off switch |
| 5 | Should "never touch this track" be enforced? | **No — every track works the same.** A note is text Claude reads, not a lock. Enforcement that holds only most of the time is worse than none, because it gets trusted. If it is wanted it is its own story, wired into `delete_track` and `delete_clip` |
| 6 | An `alias` tool for "the lead" and "the drop"? | **No.** A role is in the track name and a named bar is a locator — both in Live, both visible, both kept by Save. A third naming system would drift from the two real ones |
| 7 | Does a stash outside the grid beat a scratch row inside it? | **No** — inside wins. An idea in a `Stash:` row can be *fired to hear it*, which is the whole point of a sketch. Accepted cost: a clip belongs to a track, so a stashed idea cannot travel to another song |
| 8 | Is `remember` the name, over `note`? | `remember` — it covers a role and a note, and reads as an artist verb |
| 9 | What happens if the agent never calls anything? | **It still works.** The orientation header is built from Live (roles, stash) plus a session digest the *server* writes on its own. See the Zero-thought contract |
| 10 | Free-form JSON for the overview, or a schema? | **A light schema with an escape hatch.** `intent`, `form`, `tracks`, `decided`, `next`, `open` are rendered by the header and named in the instructions; any other key is kept verbatim. Free-form alone means every session invents a shape and the next cannot rely on it; a closed schema means the agent cannot say the thing the schema forgot |
| 11 | How does the agent get the overview back? | **It does not ask.** The whole overview is in the `get_context` header, the call the agent makes first anyway. A second "load my memory" call is a thing an agent can forget to make |
| 12 | What stops the overview becoming a dumping ground? | **An 8 KB cap**, enforced on write with an error saying what to trim. It is served in full on every `get_context`, so the cap is also the budget |
| 13 | The overview repeats facts Live owns (tempo, section count) — does that not break rule one? | It is the one allowed exception, and it is **stamped**: the server writes `as_of` (tempo, key, section and track counts as Live reports them at that moment). On load the header compares and reports a drift. Live is the truth; the overview is a memory of it |
| 14 | Patch or replace? | **Merge the named keys by default**, `replace: true` to rewrite. A one-key update must not silently erase the form |

## Prototype

[prototypes/song-remembers-itself-names-notes-idea-bin.md](../prototypes/song-remembers-itself-names-notes-idea-bin.md)
— the same afternoon end to end: the first unsaved hour, the next morning's orientation, "make the
lead louder from the drop", a note written mid-work, six sketches parked and one placed, a Save and
a rename, and what each failure says.

Review of the first draft changed three things: the stash moved out of the server and into a
`Stash:` scene row (an idea you cannot hear is not an idea), roles moved into the track name, and
the file shrank to notes and identity. The `alias` tool was cut.

## The Zero-thought contract

The agent must not have to *remember to remember*. Split by who acts:

| Happens with no tool call at all | Needs one deliberate call |
|---|---|
| **Roles** are read out of the track names already in the set | |
| **The stash** is read out of the `Stash:` rows already in the set | |
| **The session digest** — date, which tracks and sections were touched, which tools ran — is written by the server from the calls it is already logging (`src/activity.rs`), not by the agent | |
| **The overview** — the agent's own model of the song — comes back *in full* in the header, so there is nothing to fetch and no decision about whether to fetch it | |
| **`as_of`** is stamped by the server, and a drift between it and the set is reported on load — the agent cannot forget to record it or fail to notice it went stale | |
| **The header** is a few lines on top of every `get_context`, the call the agent makes first anyway | |
| **Identity** resolves itself: a provisional name before the first save, renamed to the set's own the moment `song.file_path` appears | |
| **Reconciliation** after a rename or reorder runs on connect and is reported in the header | |
| | **`remember(overview: …)`** — the plan and the model, patched as the work moves: what this is, what each track is for, what was decided, what is next |
| | **`remember(note: …)`** — the *why*, which only exists in the conversation ("harsh meant the 2–4 kHz"), and a role the producer states |
| | **`stash`** — parking material the producer has not chosen |

So a session in which the agent calls neither tool still leaves the next session oriented. The two
verbs only add what no machine can observe.

## Tool description

```text
remember — Keep what you learn about this song, so the next session starts where this one ended.

  remember(about, note, role, overview, replace)

  overview: a JSON object — your own model of this song, handed back to you in full at the top of
         every get_context. Nothing to load: it is already there. Keep it current as you work —
         after a build, after a round of changes, and before you stop.
         Keys the readout renders, all optional:
           intent   what this track is trying to be; references; who it is for
           form     the plan, section by section ("Breakdown": "everything out but drone + sitar")
           tracks   what each track is FOR, and what has been learned about it
           decided  choices already made, and what was rejected, so they are not re-litigated
           next     the move you were about to make
           open     what the producer has not decided yet
         Any other key is kept exactly as you write it. The named keys are merged, so a patch of
         one key leaves the rest alone; replace: true rewrites the whole object.
         The cap is 8 KB, because the whole of it is read back on every get_context. Over it, the
         write is refused and the error says what is biggest.
         The server stamps as_of (tempo, key, section and track counts) itself. When the set has
         moved since, the header says so — Live is the truth, this is a memory of it.

  about: "song" (default), a track name, or "section:<name>".
  note:  free text — why a change was made, what the producer called it, what not to do.
  role:  one or two words for what a track IS ("lead", "drone", "nick's own take"). A role is
         written into the track name itself (Sitar -> "Sitar [lead]") so the Live set keeps it
         without this server: Cmd+S saves it and it travels inside the .als. From then on the role
         addresses the track — set_track_mixer(track: "lead") works.

  With no note, role or overview, returns what is remembered about that subject.

  The overview and the notes are kept on this machine only, under the server's state folder, one
  small file per song
  (~/.ableton-music-maker/songs/). They are read back to you at the top of every get_context, so
  you never have to ask for them. Nothing is uploaded. ABLETON_MCP_SONG_MEMORY=false turns the
  notes off; a role still goes into the track name, because that is the producer's set, not ours.

  Write a note whenever the producer tells you what something is, what never to touch, or why a
  change was made and what it cost. A session that ends with nothing remembered is a session the
  next one starts blind.
```

```text
stash — Park an idea that is not in the song yet, where it can still be heard.

  stash(action: "save" | "list" | "place" | "drop", name, track, tags, section, slot, + note forms)

  save:  puts the material in a clip in this set's "Stash:" scene row, on the track you name
         (default: the track it came from). Notes in any of the compact forms, or a sample the way
         add_sample takes one - a browser URI, a path, or words. Tags ride in the clip name:
         "answer B [breakdown]". Park the alternatives you have not chosen: three vocal chops in the
         stash can be HEARD against each other, where a search listing can only be read.
         The row is a normal Live scene row, so the producer can fire the clip to hear it — but it
         is not a section, the setlist ignores it, and no section launch touches it.
  list:  every stashed idea with its track, length, note count and tags.
  place: copies one into a section (or a slot) on its track. The stashed copy stays.
  drop:  deletes the stashed clip.

  The stash lives in the Live set, not on this machine: Cmd+S keeps it and it travels inside the
  .als. Because a clip belongs to a track, a stashed idea belongs to its track, and cannot move to
  a different song.
```

```text
adv_song_memory — What is remembered about this song, and the producer's control over it.

  adv_song_memory(action: "show" | "list" | "attach" | "forget", name)

  show:   the file, its size, the overview's size and keys, how many notes, what is NOT in it, how
          to turn it off and delete it.
  list:   every song this machine has notes for, including notes from sets that were never saved.
  attach: point this set at one of those (after a Save As, or a set that was renamed on disk).
  forget: delete this song's overview and notes. The Live set is untouched: the roles in the track
          names and the Stash: row are the producer's, and stay.
```

## Acceptance Criteria

### It works without being thought about
- [ ] **AC1:** `get_context` begins with up to four lines for this song — its name, section count and
      when it was last worked on; the roles read out of the track names; the most recent note; and
      what is in the stash — and adds nothing when there is nothing to say.
- [ ] **AC2:** A session in which neither `remember` nor `stash` is called still writes a digest
      (date, tracks and sections touched, tools run), and the next `get_context` reports it.
- [ ] **AC3:** No new call is needed to read the memory: it rides on `get_context`, and there is no
      `notes()` tool.

### The overview — the agent's model, returned without being asked
- [ ] **AC4:** `remember(overview: {...})` stores a JSON object in this song's file and returns which
      keys changed and the size used.
- [ ] **AC5:** The **whole** overview comes back in the `get_context` header, rendered as JSON — no
      second call, no flag, no truncation below the cap.
- [ ] **AC6:** Named keys merge: a patch of one key leaves every other key untouched.
- [ ] **AC7:** `replace: true` rewrites the whole object.
- [ ] **AC8:** Unknown keys are kept verbatim and rendered after the known ones.
- [ ] **AC9:** An overview over 8 KB is refused, nothing is written, and the error names the largest
      key.
- [ ] **AC10:** An `overview` that is not a JSON object is refused with the list of keys the header
      renders.
- [ ] **AC11:** The server writes `as_of` (tempo, key, section count, track count as Live reports
      them) on every overview write; a value supplied by the agent is ignored.
- [ ] **AC12:** When Live has moved away from `as_of`, the header states each drift and says the set
      is the truth.
- [ ] **AC13:** When the set matches `as_of`, no drift line appears.
- [ ] **AC14:** The overview survives a Live restart, a server restart and the set being closed and
      reopened, and comes back attached to the same song.
- [ ] **AC15:** **Full once, then a digest.** The first `get_context` of a session renders the whole
      overview; every later call in the same session renders one line — which keys it holds and when
      it last changed — and says how to get it in full. The rule is the server's; no parameter, no
      decision for the agent.
- [ ] **AC16:** A `get_context` after the overview changed in this session renders it in full again.
- [ ] **AC17:** **The drift line travels.** When Live has moved away from `as_of`, the one-line drift
      also rides on `capture_mix` and `play_song` replies, beside the clock line they already carry —
      so a set the producer nudges mid-session is noticed without a second `get_context`.
- [ ] **AC18:** With the memory off, `remember(overview: …)` writes nothing and says so.

### Identity
- [ ] **AC19:** The set is identified by `song.file_path`, read with a generic `run` op — no new
      Remote Script command and no `SCRIPT_VERSION` bump.
- [ ] **AC20:** A set that has never been saved gets a provisional key, and notes written against it
      are kept.
- [ ] **AC21:** When `song.file_path` first appears, the provisional file is renamed to the set's key
      and the rename is reported once.
- [ ] **AC22:** Opening a different set in the same server process switches to that set's notes.
- [ ] **AC23:** A renamed or reordered track keeps its notes; every re-attachment is reported; where
      it is not certain the notes are left unattached and said so — **never repointed by guess**.

### Roles, in the set
- [ ] **AC24:** `remember(about: "Sitar", role: "lead")` renames the track to `Sitar [lead]` and says so.
- [ ] **AC25:** Any tool that takes a track resolves `"lead"` to that track (`resolve_track_name`).
- [ ] **AC26:** A role matching two tracks errors and names both; it never picks one.
- [ ] **AC27:** Setting a role twice replaces the suffix instead of stacking it.
- [ ] **AC28:** Roles survive with the memory deleted or switched off — they are in the `.als`.

### The stash, in the set
- [ ] **AC29:** `stash save` creates the clip in a scene whose name starts with `Stash:`, creating
      the row when there is none and another row when every slot on that track is taken.
- [ ] **AC30:** **`stash save` takes a sample** the way `add_sample` does — a browser URI, a path, or
      words — and parks it as an audio clip in the `Stash:` row on an audio track, warped and looped
      like `add_sample` does, so three candidates can be **heard against each other before one is
      committed to the song**.
- [ ] **AC31:** `stash list` shows an audio idea with its file name and length, and `stash place`
      puts it into a section the way `add_sample` would.
- [ ] **AC32:** A `Stash:` row is not a section: `get_context`'s sections, `make_section`, `set_song`
      and `play_song` ignore it, and a section launch never fires it.
- [ ] **AC33:** `stash list` reports every idea with track, bars, note count and tags.
- [ ] **AC34:** `stash place` copies into the section or slot named and leaves the stashed clip.
- [ ] **AC35:** `stash drop` deletes only that clip.
- [ ] **AC36:** The stash survives the memory being deleted or switched off.

### The agent is told
- [ ] **AC37:** `src/context.rs` `INSTRUCTIONS` says to read the memory back before the first change
      and to write to it while working; the `FOOTER` carries the short form.
- [ ] **AC38:** All four `prompts/*.md` tell the producer's agent to read it and keep it up to date.
- [ ] **AC39:** `tests/prompts.rs` passes — every tool-shaped name in the prompts is served in that
      spelling.

### Privacy
- [ ] **AC40:** `ABLETON_MCP_SONG_MEMORY=false` stops every write to `songs/`; roles and the stash
      still work, because they are in the set.
- [ ] **AC41:** `adv_song_memory(action: "show")` names the file, its size, what is in it, **what is
      not**, the off switch and how to delete it.
- [ ] **AC42:** `adv_song_memory(action: "forget")` deletes the file and touches nothing in Live.
- [ ] **AC43:** The notes file holds no MIDI note, no audio and no path outside the set's own.

### No Regressions
- [ ] **AC44:** `export_set` and `import_set` are unchanged and still write only when called
      (`tests/sets.rs` untouched and passing).
- [ ] **AC45:** The activity log's defaults are unchanged: payloads still off (`tests/activity.rs`).
- [ ] **AC46:** No upload path (`tests/local_only.rs`), no HTTP client in the tree.
- [ ] **AC47:** `SCRIPT_VERSION` is unchanged and an existing installed script needs no reinstall.
- [ ] **AC48:** Every existing suite passes untouched except where an AC above names it.
- [ ] **AC49:** `CORE_TOOLS` gains exactly `remember` and `stash`; `adv_song_memory` is advanced
      (`tests/artist.rs`).

## Affected Files

### Modified
| File | Change |
|------|--------|
| `src/tools.rs` | `remember`, `stash`, `adv_song_memory` bodies and `#[tool]` bindings; `CORE_TOOLS`; `resolve_track_name` matches a `[role]` token |
| `src/context.rs` | the memory header; `INSTRUCTIONS` and `FOOTER` |
| `src/song.rs` | a `Stash:` scene is not a section (beside the `Setlist:` rule) |
| `src/sections.rs` | `make_section` / `set_song` / `play_song` skip `Stash:` rows |
| `src/state.rs` | `songs_dir()` |
| `src/activity.rs` | the per-song digest counters |
| `src/samples.rs` | `add_sample`'s placement reused by `stash save` for an audio idea |
| `src/capture.rs` / `src/sections.rs` | the drift line beside the clock line on `capture_mix` and `play_song` |
| `src/lom.rs` | `Path::song().attr("file_path")`, the `cue_points` read helper |
| `prompts/*.md` (4) | read the memory first, keep it up to date |
| `TERMS.md` | one row: song notes, on by default, where, how to delete |
| `docs/facts/source-of-truth.md` | tools 105 → 108, suites 19 → 20, a Local data row, re-dated |
| `docs/architecture/overview.md`, `docs/technical/feature-matrix.md` | same PR, per CLAUDE.md |
| `CLAUDE.md` | suite count, `src/memory.rs` in the layout |
| `tests/remote_script/fake_live.py` | **model gap, see below** |

### New
| File | Description |
|------|-------------|
| `src/memory.rs` | the song file: the overview (merge, cap, `as_of`, drift), the notes, identity from `song.file_path`, reconciliation, the session digest, and the header text — pure where it can be |
| `tests/song_memory.rs` | the suite for this story |

## Remote Script compatibility

**No command is added and `SCRIPT_VERSION` does not move.** Everything reaches Live through
commands already served (`set_track_name`, `create_scene`, `set_scene`, `create_clip`,
`set_clip_name`, `get_clip_notes`, `add_notes_to_clip`, `delete_clip`) or through the generic ops
layer (decision 0010): `song.file_path`, and `song.cue_points` for the locator list. An installed
script needs no reinstall and no reselection of the control surface — `cargo run --example
new_capability_no_reload` is the pattern.

**One model gap must be fixed in the same PR.** `Song.file_path` in `tests/remote_script/fake_live.py`
is declared with `_audio_only` (the Clip helper), whose getter reads `self._is_midi_clip`. Measured
against the model on 2026-09-20:

```
>>> fake_live.Song().file_path
AttributeError: 'Song' object has no attribute '_is_midi_clip'
```

A real Live answers with the path, or `""` when the set has never been saved
(`docs/reference/ableton/live-object-model.md:223`). Fix it to a readable property with a settable
backing so a test can be a saved set or an unsaved one — not by giving the test an easier member,
and not with `DELIBERATELY_ABSENT`.

## Privacy

**This story stores something new, and it is on by default.** It is a privacy-default change and
carries all of it:

- **What:** the **song overview** — Claude's structured model of the track: what it is trying to be,
  the plan section by section, what each track is for, what was decided, what is next (up to 8 KB,
  written by the agent, so it holds the producer's ideas in the producer's words); free-text
  **notes** with their dates and subjects; the set's path, used as the identity; the per-session
  **digest** (date, tracks and sections touched, tool names). **Not** MIDI notes, **not** audio,
  **not** any path outside the set's own, **not** the roles or the stash — those are in the Live
  set, not here.
- **Where:** `state_dir()/songs/<key>.json`, one file per song. Under `/state` in the image.
- **Default:** on, like the activity log — off is `ABLETON_MCP_SONG_MEMORY=false`, which stops every
  write to `songs/`. Deletion is `adv_song_memory(action: "forget")`, the Mac app's "Delete all
  local data", or deleting the folder.
- **`TERMS.md`:** a new row in *What is stored on your machine* — "**Song memory** — Claude's
  written model of a song: the plan, what each track is for, what you decided and why, what not to
  touch, and a per-session digest of which tracks were worked on | `~/.ableton-music-maker/songs/<song>.json` |
  On | `ABLETON_MCP_SONG_MEMORY=false`, `adv_song_memory(action: forget)`, or 'Delete all local
  data'" — plus a sentence in *Captures*-style plain language that the roles and stashed ideas are
  in the producer's own Live set and are not copied here.
- **Pinned by:** `tests/song_memory.rs` (the default, the off switch, the shape of the file, the
  8 KB cap, that no MIDI note or audio path enters it) and `tests/local_only.rs` unchanged and
  passing.
- **Worth stating plainly at review:** the overview is the most content-bearing thing this server
  has ever written to disk — prose about the producer's unreleased music, written without them
  typing it. On by default is defensible only because it is local, capped, inspectable with one
  call, deletable with one call, and named in TERMS.
- **Nothing uploads**, and this story does not propose a way to.

## Test Coverage

Every AC has a test. New suite `tests/song_memory.rs` unless the row says otherwise.

| Suite / script | Test | AC |
|---|---|---|
| `tests/song_memory.rs` | `context_header_lists_roles_last_note_and_stash` | AC1 |
| | `overview_round_trips_and_reports_the_keys_that_changed` | AC4 |
| | `the_whole_overview_is_in_the_context_header` | AC5 |
| | `a_patch_merges_and_leaves_other_keys_alone` | AC6 |
| | `replace_rewrites_the_object` | AC7 |
| | `unknown_keys_are_kept_verbatim` | AC8 |
| | `an_overview_over_the_cap_is_refused_and_names_the_largest_key` | AC9 |
| | `an_overview_that_is_not_an_object_is_refused` | AC10 |
| | `as_of_is_written_by_the_server_and_an_agent_value_is_ignored` | AC11 |
| | `a_drift_from_as_of_is_reported_and_live_is_called_the_truth` | AC12 |
| | `no_drift_line_when_the_set_matches` | AC13 |
| | `the_overview_survives_a_restart_and_reattaches` | AC14 |
| | `full_on_the_first_context_of_a_session_then_a_digest` | AC15 |
| | `a_change_makes_the_next_context_render_it_in_full_again` | AC16 |
| | `the_drift_line_rides_on_capture_mix_and_play_song` | AC17 |
| | `stash_save_takes_a_sample_and_parks_it_as_an_audio_clip` | AC30 |
| | `stash_list_and_place_handle_an_audio_idea` | AC31 |
| | `the_off_switch_stops_the_overview_too` | AC18, AC40 |
| | `context_header_is_absent_when_there_is_nothing_to_say` | AC1 |
| | `a_session_with_no_memory_calls_still_writes_a_digest` | AC2 |
| | `no_notes_tool_is_served` | AC3, AC49 |
| | `identity_comes_from_song_file_path_through_one_op` | AC19 |
| | `an_unsaved_set_gets_a_provisional_key_and_keeps_its_notes` | AC20 |
| | `the_first_save_renames_the_file_and_says_so_once` | AC21 |
| | `opening_another_set_switches_the_notes` | AC22 |
| | `a_renamed_track_keeps_its_notes_and_the_move_is_reported` | AC23 |
| | `an_uncertain_match_leaves_the_notes_unattached_and_says_so` | AC23 |
| | `a_role_is_written_into_the_track_name` | AC24 |
| | `a_role_addresses_the_track` | AC25 |
| | `an_ambiguous_role_names_both_tracks_and_changes_nothing` | AC26 |
| | `a_second_role_replaces_the_suffix` | AC27 |
| | `roles_and_stash_survive_forget_and_the_off_switch` | AC28, AC36, AC40 |
| | `stash_save_makes_the_row_then_a_second_row_when_full` | AC29 |
| | `stash_list_reports_track_bars_notes_and_tags` | AC33 |
| | `stash_place_copies_and_keeps_the_original` | AC34 |
| | `stash_drop_deletes_only_that_clip` | AC35 |
| | `show_names_the_file_what_is_in_it_and_what_is_not` | AC41 |
| | `forget_deletes_the_file_and_sends_nothing_to_live` | AC42 |
| | `the_file_holds_no_midi_note_no_audio_and_no_foreign_path` | AC43 |
| `tests/song.rs` | `a_stash_scene_is_not_a_section` | AC32 |
| `tests/capture.rs` | `capture_mix_carries_the_drift_line` | AC17 |
| `tests/samples.rs` | `a_stashed_sample_is_referenced_not_copied` | AC30 |
| `tests/artist.rs` | the core/advanced split and the served spellings | AC49 |
| `tests/orchestration.rs` | `play_song_and_make_section_skip_the_stash_row` | AC32 |
| `tests/prompts.rs` | passes with the new names in the prompts | AC38, AC39 |
| `src/context.rs` unit tests | the instructions name `remember` and `stash`; the header renders | AC37, AC1 |
| `src/tools.rs` unit tests | `tool_count_and_schema_defaults` → 108 | AC49 |
| `tests/activity.rs` | unchanged and passing | AC45 |
| `tests/sets.rs` | unchanged and passing | AC44 |
| `tests/local_only.rs` | unchanged and passing | AC46 |
| `tests/remote_script/test_live_semantics.py` | `song_file_path_is_empty_until_the_set_is_saved` — with the LOM reference cited | model gap |
| `tests/remote_script/test_live_api_conformance.py` | passes with `Song.file_path` fixed | model gap |
| `src/tools.rs` unit tests | `the_servers_command_list_and_the_scripts_dispatch_are_the_same_set` passes with `SCRIPT_VERSION` untouched | AC47 |
| `scripts/check-docs-facts.sh` | passes with the re-dated snapshot; the `SCRIPT_VERSION` row is unchanged | AC47 |
| the whole suite | `cargo test` green, and `ABLETON_TARGET=live cargo test -- --test-threads=1` green against Live 12 | AC48 |
| `tests/stdio_integration.rs` | the three tools appear in `tools/list` with their annotations | AC49 |

`ABLETON_TARGET=live cargo test -- --test-threads=1` must pass against a real Live before this is
called done, and `scripts/live-lom-sweep.py` re-run so `song.file_path` is in scope rather than out
of it.

## Implementation Notes

### Patterns to Follow
| Pattern | Where used | Reuse for |
|---|---|---|
| A magic scene the song logic ignores | `SETLIST_PREFIX`, `src/song.rs:17` | a `STASH_PREFIX` beside it, for the `Stash:` rows |
| A convention inside a name, parsed back | `"<name> · <bars>"` sections | `Track [role]`, `clip [tags]` |
| Name-or-index parameter as `Option<Value>` | `track` on `arrange`, `record_clip` | `clip`, and the locator form of `at_bar` |
| Capability through ops, no script change | `src/lom.rs`, `examples/new_capability_no_reload.rs` | `song.file_path`, `song.cue_points` |
| A per-key file under the state dir | `src/library.rs`, `src/sets.rs` | `src/memory.rs` |
| Pure module + thin tool body | `src/song.rs`, `src/performance.rs` | `src/memory.rs` |

### Design Decisions

- **Why a name suffix and not a file, for roles.** The set is the thing the producer keeps, backs up
  and sends to a collaborator. A role in the track name is saved by Cmd+S, travels in the `.als`,
  works on another machine with no server, and can be edited by the producer in Live. The cost is
  real and accepted: it edits the producer's own track names, which appear in the mixer, on clips
  and in exported stem filenames.
- **Why the stash is in Live.** A sketch you cannot hear is not a sketch. In a `Stash:` row the
  producer fires it with one click. The accepted cost is that an idea belongs to its track and
  cannot move to another song — a cross-song library is a different object, out of scope.
- **Why the notes are not in Live.** There is no writable text field for them (measured), and Live's
  memory is only as durable as a save the API cannot perform. #46's loss happened before a save.
- **Why on by default.** The same reasoning as the activity log: local, small, deletable, and
  worthless if it only exists when someone remembered to ask for it. The off switch and the TERMS
  row are the price.
- **Why no enforcement on "never touch".** A refusal that holds most of the time gets trusted all of
  the time. Either it is wired into the destructive tools as its own story, or it is honest prose.
- **Why no `alias`.** Two naming systems exist already and Live keeps both. A third would drift.
- **Why the overview is returned whole, not on request.** A "load my memory" call is a call an
  agent can fail to make, and the one it will skip is the cheap-looking first turn of a session —
  exactly the turn that needed it. Riding on `get_context` makes retrieval unskippable. The 8 KB cap
  is the price of that, and is also what keeps the overview a model rather than a diary.
- **Why `as_of` belongs to the server.** The overview is the one place a fact Live owns may be
  repeated. An agent-written stamp would be wrong precisely when it matters — after the producer
  changed something by hand — so the server writes the numbers it is already reading.
- **Why merge is the default.** The common write is one key ("next"). Replace-by-default would
  quietly delete the form and the track model the first time an agent patched a single field.
- **Why full once, then a digest.** The overview is only worth reading again when it has changed,
  and at the 8 KB cap a repeat render would put roughly two thousand tokens into every later
  `get_context` for something the agent itself last wrote. Serving it in full on the first call of a
  session and after every change keeps retrieval unskippable without paying for it twice; making it
  a server rule rather than a parameter keeps the no-branch property that made this work.
- **Why the drift line travels beyond `get_context`.** `get_context` is called once, at the top of a
  session — that is exactly why the overview rides on it, and exactly why the staleness check cannot
  ride there alone. A producer who nudges the tempo mid-session would otherwise be invisible until
  the next session. `capture_mix` and `play_song` are the calls an agent makes repeatedly and both
  already return a clock line, so the drift costs a line beside something already being read.
- **Why the stash takes audio, not just MIDI.** Audio is the material that cannot be judged from its
  name — choosing between three vocal chops from a search listing is choosing blind, because
  `add_sample` can only put one into a section or a bar, so auditioning means committing. Parking
  all three in the `Stash:` row makes A/B the default instead of the exception. The stash still
  stores no audio: it references the file, as `add_sample` does.
- **Why the digest is the server's job.** The requirement is that this works without the agent
  thinking about it. Anything that depends on the agent remembering to call a tool will be missing
  from exactly the sessions that most needed it.

## Verification

### Manual verification steps
With Live open, the current Remote Script installed (no reinstall required — that is AC47):

1. New empty set, never saved. Build something small. Confirm the reply names a provisional memory.
   Kill Live. Reopen. `get_context` — the notes are offered, and the stash and roles are gone with
   the unsaved set, as they must be.
2. Save the set. `get_context` — the rename to the set's key is reported once, and not again.
3. `remember(overview: {"intent": "…", "next": "…"})`, then `get_context` — the object comes back in
   full at the top. Call `get_context` again: one line, not the object. Patch one key: full again,
   and the other keys are still there.
4. Change the tempo in Live by hand, then `capture_mix` — the drift rides beside the clock line,
   without a second `get_context`.
5. `remember(about: "<a track>", role: "lead")` — the track is renamed in Live's mixer in front of
   you. `set_track_mixer(track: "lead", …)` moves that fader (needs the naming issue shipped).
6. `stash save` three samples onto an audio track straight from a `search_browser` listing, fire
   each in turn, `stash place` the one that wins — the A/B that is impossible today.
7. `stash save` two MIDI ideas on one track — a `Stash:` scene row appears at the bottom of the
   Session view. Fire one clip: it plays. Fire a section: it does not.
8. Rename the track in Live and drag it two places up. `get_context` — the notes followed it and the
   move is reported.
9. `ABLETON_MCP_SONG_MEMORY=false` and restart the server: `remember` says the overview and notes
   are off, the role still lands in the track name, `songs/` gains no file.
10. `adv_song_memory(action: "forget")` — the file is gone; the `Stash:` row and the `[lead]` suffix
    are still in the set.

## Out of Scope
- **A cross-song idea library** ("the bassline from the last track"). A stashed clip belongs to its
  track and its set. A library is a different object with its own privacy story.
- **An `alias` tool.** Roles and locators cover it (Open Question 6).
- **Enforcing "never touch".** Open Question 5 — its own story if wanted.
- **Audio in the stash.** The server copies no audio; an audio idea can only be a reference to a
  file that exists.
- **Storing the notes beside the `.als`.** Open Question 3 — it would need a numbered decision.
- **Names instead of numbers.** Split out and shipping **first** — it needs none of this story's
  machinery: no file, no identity, no reconciliation, no cap, no `as_of`, no privacy section. Seven
  core tools take a track by index only while five take it by name, and the two halves meet inside a
  single mixing move. That is a defect in the artist surface (decision 0006), not a design question,
  so it is an issue with an audit table rather than a story.
- **A device vocabulary the server learns and keeps** — that "Vinyl Drawbs" answers to Vinyl Drive,
  Release and Rotation Amount rather than a cutoff, and that VHS Dreams' Release macro runs
  backwards. Those are facts about Ableton's factory content, true in every set anyone ever builds,
  and song-scoped memory throws them away. They belong in a cache keyed on the device, not in this
  file — its own issue, and it has no privacy story to write because it is not about the producer's
  music.
- **A profile of the producer across songs** (taste, habits, house style). This file is per song and
  keyed to a set; a profile is a different object with a different privacy story.
- **Making `export_set` automatic.** #46 owns that; this story only points at the last snapshot.
- **Saving the Live set.** The API cannot, and this story does not pretend otherwise.

## Dependencies
| Dependency | Status | Notes |
|---|---|---|
| `song.file_path` through ops | Available | `PATH_ROOTS` and `Path::validate` already allow it; no script change |
| The `Stash:` clip commands | Available | all in `ALL_REMOTE_COMMANDS` |
| `Song.file_path` in the fake Live | **Broken** | must be fixed in this PR (Remote Script compatibility) |
| #46 (the snapshot at the moment of risk) | Open | independent; this story links to the last snapshot, it does not replace it |
| #45 (`build_song` crash) | Open | the reason a memory that predates the first save matters |

## Related Stories
- `performance-records-itself-as-an-arrangement-take` — the same principle: the thing the producer
  keeps is the Live set, and the server adds only what Live has no field for.
- `prompts-screen-one-copy-starts-song-or-set` — the four prompts this story edits.

---

## Changelog
| Date | Change |
|------|--------|
| 2026-09-20 | Created |
| 2026-09-20 | Song overview added: a structured model the agent patches and gets back in full on every `get_context` (AC4–AC18) |
