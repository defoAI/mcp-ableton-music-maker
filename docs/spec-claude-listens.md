# Feature spec: Claude listens

*Product spec, 2026-10-17. Status: built (Parts A–D); the real-Live gates below are open.*

**Owners:** the server (`src/`), the Mac app (`app/`) and the Remote Script (`body.py`).

---

## 1. In one paragraph

Today Claude can change a song but cannot hear it. To learn how something sounds, it has two options:
- **Play the song itself.** `capture_mix` starts the transport, plays out loud in the producer's room,
  takes real time, and fails on long takes.
- **Ask the producer.**

The Mac app already hears everything Live plays: the Listen screen taps Live's process. It then throws it
all away.

**Claude listens** makes the app remember what it heard, as measurements stamped with bars, sections and
the set's revision. It hands those measurements to Claude through a few new tools. It also gives the
producer a place to answer Claude without typing:
- a comment pinned to a bar;
- a pick between options Claude offered;
- a "go ahead, I'm away" switch.

The producer works exactly as before. Every time they press play, Claude learns how the song sounds.

---

## 2. The problem, from both chairs

**The producer** says things like "the drop feels small", "the pad goes on forever", "option 3". They
point with words because there is nothing to point with. Feedback rarely says which bar, and choosing
between Claude's options means hunting through locators and then typing the choice back.

**Claude** reasons from notes and settings, not from sound. In practice:
- **It cannot measure while the producer listens.** `capture_mix` plays out loud and takes the transport,
  and the rule is never to start the transport while the producer is listening. So almost nothing gets
  measured after the first build.
- **Every "this should sound better now" is a guess.** It is checked only when the producer happens to
  say so.
- **It cannot tell what has been heard.** It doesn't know which parts of the song the producer has heard
  since the last change, so it doesn't know what still needs a listen.

**Evidence.** These come from the heaviest user of the server, a studio that makes whole records through
it:
- Its engineer's wishlist ranks "let the engineer hear the record without playing it into the room" first
  of twelve items.
- "A listening page for the artist" is sixth, and "options as a first-class verb" is fifth.
- Our own feature request F6, "Measuring without the room hearing it", asks for the same thing.

## 3. Who it is for

| Who | What they get |
|---|---|
| **A producer working with Claude** (every user) | Claude's answers about the mix cite what they just played ("the drop you played at 14:02 peaked at −0.4 dBFS, and 61 % of it is below 250 Hz"). They give feedback by clicking a bar, not by describing where it is. |
| **A producer comparing ideas** | Options from Claude show up as rows in the app, each with a play button and a **This one** button. |
| **A producer who steps away** | A switch says "Claude may play the set now". Claude can then measure the whole song while the room is empty. |
| **Studios with their own tooling** on top of the server (theVoid's `void`) | A stable, documented reading of what was heard, by bar and by section, to build checks and reports on. |

---

## 4. What we are building

Four parts, shipped in this order. Each part is useful on its own.

### Part A: Listening memory (the core)

The Listen tap keeps running while the app is open and Live is playing, even with no window showing. The
setting is **Remember what Live plays**, and it is off until the producer turns it on.

For every stretch of playback, which we call a **take**, the app keeps:
- **When it was played** (wall clock), and **where**: the bar and beat at every frame, from the Remote
  Script's clock.
- **The set's revision** at that moment (`song.rs` already tracks it). This tells Claude whether the
  producer heard the current version or one from before the last change.
- **Measurements per bar:**
  - peak and true peak, RMS, crest;
  - short-term loudness (LUFS);
  - the six ranges (sub, bass, low mids, mids, presence, air) and the octave bands;
  - correlation;
  - clipping;
  - silence.
- **Measurements per 16th note:** RMS. This is what the duck reading in `audio.rs` needs, so sidechain
  depth can be read without a capture.
- **Measurements per frame**, about 30 per second: the spectrum and a 12-note pitch-class profile
  (chroma). Spectrograms and chord readings are drawn from these.
- **Which section** each bar belonged to, from the scene and locator names at the time.

**No audio is written.** The tap's audio is turned into numbers and dropped, as it is today. Only the
numbers are stored. That keeps the promise in [your-data.md](your-data.md) close to its current wording.

**Size.** About 3 KB a second (quantised frames plus per-bar rows), so about 10 MB per hour of playback.

**Retention.**
- Kept per set (identity from `song.file_path`, as song memory does) for 30 days. That period is a
  setting.
- **Delete all local data** removes it.
- The tray icon shows a dot while the app is remembering.

**What it cannot hear:**
- **Single tracks.** The tap hears Live's whole output, so per-track readings still need `capture_mix`
  with `tracks`.
- **Playback the producer did not do.** If nobody plays it, nothing is heard. That is what Part D is
  for.

### Part B: Tools for Claude

These are new tools, and each reads only from the listening memory. **None of them starts the
transport.**

| Tool | What it answers | Example reply (abridged) |
|---|---|---|
| `heard` | What has been heard of this set, and how current it is. | "Heard at the current revision: Intro, Build. Heard before the last change (pad −3 dB at 14:10): Drop, Break. Never heard: Outro." |
| `listen` | How a range sounded the last time it was played. Takes `section`, `bars`, or `last`; plus `revision: current or any`, `resolution: bar, beat or 16th`. | Per section: LUFS, peak, crest, the six ranges, low end per bar, duck depth if any. It names the take it used ("played 14:02, revision 41 of 43: the pad change is not in it"). |
| `compare` | The difference between two takes of the same bars: before and after a change, or option A against option B. | "Drop, revision 41 against 43: −1.2 dB below 250 Hz, crest +1.8 dB, loudness −0.6 LU." |
| `listen_picture` | A picture of a range: a spectrogram, a loudness curve, and a chroma strip, with section names. Returned as MCP image content. | A PNG Claude can look at. |

`get_context` gains one line under its clock line: **🎧 heard: 3 of 6 sections current; Drop not heard
since pad −3 dB.** That way every session starts knowing what is stale.

**Words matter here.** A reading is labelled as **measured from playback at <time>**, never as "how it
sounds". Chords read from chroma are labelled **heard as Am (chroma)** next to what the clips say, and
never replace them.

### Part C: The producer answers in the app

A new screen, **Feedback**, next to Listen, plus a small float like the spectrum float.

1. **The song as a strip.** Sections in order, under a loudness curve built from the listening memory.
   Unheard sections are greyed. Sections heard before the last change are marked.
2. **Comments on a bar.** Click the strip (or press a hotkey while Live plays, which stamps the current
   bar) and type or dictate: "pad too long here". Claude reads it with `get_feedback` and answers in the
   same thread.
3. **Options.** Claude calls `offer_options` with a question and two to ten options. For each option:
   - a name and one line on what it is;
   - **how to hear it**: a bar range or locator, a scene, or a clip.

   Each option appears as a row:
   - **▶** plays it. This is the producer's own click, so it is allowed to move the transport. It uses the
     existing `jump_to` and `fire_scene` paths.
   - **This one** picks it, with an optional note.

   Claude gets the pick on its next `get_feedback`. Clients that support MCP elicitation can also show
   the question inline; the app is the surface that always works.
4. **The away switch.** **Claude may play the set** stays on until the producer turns it off. It also
   turns off when they press play themselves, or after a time limit they choose (default 20 minutes). Its
   state is in `get_context` and in `get_feedback`.

| New tool | Purpose |
|---|---|
| `offer_options` | Put a choice in front of the producer: the question, the options, and how to hear each one. Returns an ID. |
| `get_feedback` | Everything since a cursor: new comments (bar, section, text, time), picks, the away switch. |
| `reply` | Answer a comment thread in the app ("pad shortened to 8 bars; play from the Break to hear it"). |

### Part D: Claude plays when the room is empty

This part is only allowed while **Claude may play the set** is on:
- **`capture_mix` with `whole_song: true` (or a range) runs as a sequence of takes.** The server clears
  the Capture track between takes, without the 30-second confirmation, and reports progress.
- **Or Claude simply plays the range.** The listening memory measures it like any other playback, with
  no Capture track at all. This is simpler and is the default.
- **Live's own File → Export** is driven through macOS UI scripting, needs Accessibility permission, and
  is exact and faster than real time. **It is a stretch goal:** Live's dialogs are custom windows and the
  script will be fragile. It needs a spike before we promise it.

If the producer presses play or stop, or turns the switch off, Claude's playback stops at once and the
reply says so.

---

## 5. Producer journeys

**1. "The drop feels small."** The producer has just played the drop. Claude calls
`listen {section: "Drop"}`:
- The drop is 2.1 LU quieter than the build.
- Its crest is 14 dB, against 9 dB in the build.
- The kick and bass share the 63 Hz band.

Claude proposes three fixes, applies one, and says "play the drop when you're ready". When the producer
plays it, `compare` shows the change, and Claude reports it in one line. Claude never touched the
transport.

**2. Picking a breakdown.**
1. Claude builds four breakdowns after the song's end, as it does today, and calls `offer_options`.
2. The producer clicks ▶ on each, then **This one** on the third, with the note "but shorter".
3. Claude reads the pick, keeps option 3, removes the others, and shortens it by 4 bars.

What used to be about eight steps on the producer's side is now two clicks.

**3. Notes while listening.** The producer plays the whole song with headphones on and presses the hotkey
four times: "too long", "hats harsh", "love this", "ending abrupt". Each note lands on its bar. Claude
answers all four in one pass and lists what changed, by bar.

**4. Lunch.** The producer turns on **Claude may play the set** and leaves. Claude plays the song once,
measures every section at the current revision, fixes a clipping peak in the last chorus, and plays that
section again to confirm. When the producer comes back, a summary is waiting in the Feedback screen.

---

## 6. Privacy and trust

This feature changes a promise, so the change is spelled out:

| | Today | With Claude listens |
|---|---|---|
| Listening in the app | Off until started; stops when no window shows it; never written. | **Remember what Live plays** is off until turned on. When on, it runs while the app is open and Live plays. **Measurements are written; audio is not.** |
| Comments and picks | none | Stored locally with the set's memory. Deleted by **Delete all local data**. |
| Claude starting the transport | Any time (`capture_mix`, `play_song`). | Measuring by playback needs the away switch on. Performance tools are unchanged. |

**Updates:**
- `your-data.md` and `TERMS.md` gain one row each.
- The tray shows a dot while the app is remembering.
- Nothing leaves the machine. There is still no code that could send it anywhere.

---

## 7. Requirements

**Must, for the first release (Parts A and B):**
1. Listening memory runs headless in the app while Live plays, with no window open, after a one-time
   opt-in.
2. Every bar of a take is stamped with its bar number, section name and set revision. The bar stamp must
   be within ±50 ms of Live's own position after a jump, a loop and a tempo change.
3. `heard`, `listen` and `compare` work from the memory alone, and answer in under 300 ms for a 10-minute
   song.
4. A reading never presents a stale take as current: the revision and the time are always in the reply.
5. Per-bar values match `capture_mix` on the same bars within 0.5 dB (peak, RMS, bands). This is tested
   against a fixture render.
6. The tap's CPU cost stays under 3 % of one M-series core while remembering.
7. The privacy table above ships with it.

**Should, for the second release (Part C, plus `listen_picture`):**
- the Feedback screen, comments with replies, `offer_options`, `get_feedback`, the hotkey, the away
  switch;
- `listen_picture` with a spectrogram, loudness curve and chroma.

**Could, later (Part D):**
- whole-song capture under the away switch;
- the Live Export spike;
- chord reading from chroma as a `listen` field.

**Won't, in this feature:**
- per-track listening without solo or capture;
- storing audio;
- any cloud analysis;
- automatic mixing from readings. Claude proposes, and the producer hears it.

---

## 8. How it fits what exists

| Piece | Today | Change |
|---|---|---|
| `app/src-tauri/src/listen/` | Process tap; `Analyzer` builds about 90 numbers per frame at 30 fps for the screen. | Add chroma and short-term LUFS to `Frame`. Add a **recorder** that keeps frames and per-bar summaries, and runs when **Remember** is on, with no window needed. |
| Remote Script clock (`_clock`, `subscribe`) | Bar and beat position for performance and the clock line. | The app subscribes to the clock while remembering. Interpolate between ticks with the tempo. Mark jumps, loops and tempo changes. |
| `src/song.rs` revision; `src/memory.rs` song identity | Track set changes and keep per-song memory. | Stamp takes with the revision. Store takes beside the song memory under `state_dir`. |
| `src/audio.rs` measurements | Measure captured WAVs. | Shared code: the same functions measure per-bar summaries from the tap's frames, so both paths agree by construction. |
| `capture_mix` | Real-time capture through a Capture track. | Unchanged. It stays for per-track readings. Part D adds the away-switch path. |
| Tools | 111. | +4 in Part B (`heard`, `listen`, `compare`, `listen_picture`), +3 in Part C (`offer_options`, `get_feedback`, `reply`). All are core tools, not `adv_`. |
| The server and the app talking | The app reads the server's heartbeat and activity files. | Takes, comments and options live as files under `state_dir`, so neither process needs a socket to the other. The server reads them, the app writes takes and comments, and the server writes options and replies. |

**One design rule:** the server is short-lived (every client starts its own), so **the app does the
listening and the server only reads what it wrote**. If the app is not running, the tools say so ("the
Mac app is not running, so nothing has been heard") rather than returning empty readings.

---

## 9. Measuring success

| Signal | Today | Target, after release 2 |
|---|---|---|
| Edits followed by a measured listen before the next edit | Near 0 % outside the first build (estimated from the studio's sessions) | > 60 % |
| Transport starts by Claude just to measure, while the producer is present | Common | 0 |
| Producer actions to pick an option | About 8 (find locator, play, compare, type) | 2 clicks |
| Feedback that names a bar | Rare | > 80 % of comments |
| Readings that disagree with `capture_mix` by more than 0.5 dB | n/a | 0 in the fixture suite |

All of these are counted from the local activity log. Nothing is sent anywhere.

---

## 10. Risks and open questions

1. **Bar accuracy.** The Remote Script ticks at Live's UI rate (tens of milliseconds, and uneven).
   Stamping the tap's audio by interpolating between ticks needs a probe across jumps, loops, tempo
   automation and warped playback. **This is the first spike** (`examples/listen_probe.rs`, extended).
2. **The process tap hears everything Live outputs**, including cue and preview. Previewing a sample in
   the browser while stopped would be stamped with no bar. Rule: frames are kept only while the transport
   runs; anything else is dropped.
3. **Arrangement against Session.** Playing Session clips over the arrangement makes "bar 33" mean
   something else. Takes record whether any track was following Session clips (the script already
   knows), and `listen` warns.
4. **macOS 14.4+.** Listening needs it, and the rest of the app runs on 12. Part B tools must answer
   clearly on older systems.
5. **Comments without the app open.** Is a comment hotkey worth a global shortcut permission, or is the
   float window enough? Decide after the first producers use it.
6. **Elicitation.** Which clients show MCP elicitation today, and do we use it at all, or keep the app as
   the one surface? Check Claude Desktop and Claude Code before Part C.
7. **The Export spike** (Part D) may fail. The feature must not depend on it.

---

## 11. Release plan

| Release | Contents | Size | Gate to ship |
|---|---|---|---|
| **0: spike** | Bar-stamping probe against a real Live: jumps, loops, tempo change. | days | ±50 ms holds, or we know what breaks it. |
| **1: Claude hears** | Part A, plus `heard`, `listen`, `compare`, the `get_context` line, and the privacy docs. | ~2 weeks | Requirements 1–7. One real session where Claude answers "how does the drop sound now" without touching the transport. |
| **2: the producer answers** | Part C, plus `listen_picture`. | ~2 weeks | A full options round done only by clicks. Comments answered in thread. |
| **3: the empty room** | Part D: away-switch playback; the Export spike. | ~1 week + spike | Claude measures a whole song alone and leaves a summary. |

The sizes are rough, for one engineer who knows the codebase.

---

## Appendix: what theVoid's tooling does with it

The `voidmusic` toolkit on top of the server would use it like this:
- **`void mix`** reads `listen` by section instead of capturing. It captures only for per-track readings.
- **The option rounds of `make-a-record`** go through `offer_options`. That removes the locator jumping
  and lets a sub-agent wait for a pick.
- **The section bars in `song.json`** become the `bars` and `section` arguments, and a take's revision is
  checked against the document's version.

None of this needs anything from the server beyond the tools above.

---

## Implementation status (2026-10-17)

| Part | What shipped | Where |
|---|---|---|
| A | The recorder (clock → playhead with jumps, loops and tempo; a stamper that waits for the clock; a row per bar measured with `audio::measure` + `meter`; RMS per 16th; ~30 spectrum + chroma frames a second), takes under `state_dir/listening/<song>/takes`, the change log as the revision, retention, the app heartbeat. In the app: **Remember what Live plays**, the headless tap, the clock feed, the tray dot, Delete all local data. Chroma and short-term LUFS added to the Listen screen's `Frame`. | `src/recorder.rs`, `src/meter.rs`, `src/listening.rs`, `app/src-tauri/src/listen/remember.rs` |
| B | `heard`, `listen` (section / bars / last, revision current or any, resolution bar / beat / 16th), `compare` (before/after, or A against B), `listen_picture` (PNG), the 🎧 line under get_context's clock line. The old raw `listen` (track meters) is now `adv_listen_meters`. | `src/listening.rs`, `src/picture.rs`, `src/tools.rs` |
| C | `offer_options`, `get_feedback`, `reply`; the Feedback screen (strip, comments on a bar, M to stamp the bar playing, questions with ▶ and This one, threads, the away switch) and the Note float. Elicitation is not used: the app is the one surface (open question 6). The hotkey is in-app (M) and the float, not a global shortcut (open question 5). | `src/feedback.rs`, `app/src/feedback.js`, `app/src/feedback-float.html` |
| D | `listen {play: true}` and `capture_mix {whole_song: true}`, both refused unless the away switch is on; Claude's playback stops when the producer stops Live or turns the switch off. The Live Export spike is not built. | `src/feedback.rs`, `src/tools.rs` |

**Measured here, without Live:** per-bar peak, RMS and bands agree with `capture_mix` within 0.5 dB on the fixture render (`tests/listening.rs`) — after `audio::octave_energy` moved to quarter-hop windows so a band's energy no longer depends on where the windows fall (the old non-overlapping windows read a bar 1.6 dB apart below 44 Hz depending on alignment). Stamping holds ±50 ms across a quantized jump, a loop and a tempo change against a simulated Live with Live's tick jitter (`recorder` tests). heard / listen / compare / listen_picture answer a ten-minute song in under 300 ms (debug build). Remembering costs 1.1 % of one M-series core and writes 2.9 KB a second (`cargo run --release --example listen_cost`).

**Open, needs a real Live:** release 0's gate — run `cargo run --example clock_probe -- --drive` and `cd app/src-tauri && cargo run --example listen_probe -- 20 --stamp` on a scratch set with a hit on every beat; set `clock_offset_ms` to the median it prints. Requirement 1's one real session ("how does the drop sound now" answered without touching the transport), the tap's own CPU on top of the analysis, and macOS 14.4 permission flows are checked in the app, not in the suite.
