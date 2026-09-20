# Architecture overview

| | |
|---|---|
| **Verified** | 2026-09-20 against `src/`, `app/src-tauri/src/`, the Remote Script, `Dockerfile`, `docker/verify-image.sh` and `.github/workflows/ci.yml` |

How the system actually works. This is the one document with a hard freshness duty: it is
read while someone changes the code it describes, so **a change in `src/` or the Remote
Script updates this note in the same PR.** For what the product does see the
[feature-matrix](../technical/feature-matrix.md); for every number see
[source-of-truth](../facts/source-of-truth.md).

---

## Two processes, one socket

The socket speaks **protocol 2** ([decision 0010](../decisions/0010-the-scripts-contract-is-a-duplex-protocol.md)):
a request may carry an `id` and its reply carries it back, documents are
newline-delimited or concatenated, and the script pushes events nobody asked
for on the same socket. The script's clients are read **on Live's main-thread
tick**, which is 100.0 ms — jitter 9.6 ms, measured over 600 samples by the
script itself and reported in `get_script_info` and `--check`. That is the
whole latency budget: a round trip is one tick, whether or not it touches
Live's API. Before the reader moved onto the tick it was 200 ms, or 400 ms
with an API touch, because a message waited once to be read by a Python thread
and again to reach the main thread.

A server from before protocol 2 sends no ids; the script answers those in
order, without ids, exactly as it always did — and gets the faster round trip
anyway, because the speed is the script's, not the server's.

The socket speaks **protocol 2** ([decision 0010](../decisions/0010-the-scripts-contract-is-a-duplex-protocol.md)):
a request may carry an `id` and its reply carries it back, documents are
newline-delimited or concatenated, and the script pushes events nobody asked
for on the same socket. The script's clients are read **on Live's main-thread
tick**, which is 100.0 ms (jitter 9.6 ms, measured over 600 samples and
reported in `get_script_info`). That is the whole latency budget: a round trip
is one tick, 100 ms, whether or not it touches Live's API. Before the reader
moved onto the tick it was 200 ms, or 400 ms with an API touch, because a
message waited once to be read by a Python thread and again to reach the main
thread.

```
MCP client ──stdio (JSON-RPC)──▶ ableton-music-maker ──TCP 9877 (JSON)──▶ Ableton Live
(Claude Desktop, Claude Code,    Rust binary, runs anywhere          AbletonMusicMaker Remote Script,
 Cursor)                          that can reach the port             Python, runs inside Live
```

| Process | Where it must run | Why |
|---|---|---|
| **The Remote Script** — `AbletonMusicMaker_Remote_Script/__init__.py` | Inside Live, on the producer's machine | It is a Live control surface. Live loads control surfaces only through its embedded Python (2.7 on Live 10, 3.x on 11+), so this file stays Python with no f-strings, type hints or third-party imports. It opens a TCP server on port 9877 and executes JSON commands against the Live API on Live's main thread. |
| **The server** — `ableton-music-maker` | Anywhere that can reach that port: the same machine, or a container on it | Speaks MCP over **stdio only** to the client; there is no HTTP transport and no listening port. Talks to Live over one TCP socket. |

The installer, `ableton-music-maker-install-script`, is a second binary in the same crate.
The Remote Script is embedded into both with `include_str!`; the installer copies it into
Live's User Library under `Remote Scripts/AbletonMusicMaker/`, keeping a `.bak` of anything
it replaces (`src/install.rs`, Library.cfg discovery for the library path).

## Startup (`src/app.rs`)

1. Builds a `LiveState` around a `RealBridge` configured from `ABLETON_HOST` / `ABLETON_PORT`;
   the `LiveState` also owns the [activity log](#the-activity-log-srcactivityrs).
2. Runs the **handshake** on the blocking pool: `get_script_info` to Live. If Live is not
   reachable it logs a warning and continues — every tool retries the handshake on use, so
   starting the server before Live is fine.
3. Writes a **heartbeat** (`<state_dir>/sessions/<pid>.json`) so the Mac app can see the
   process; the MCP `initialize` request rewrites it with the client's name and version.
4. Serves MCP over stdio until the client disconnects, then removes the heartbeat and
   disconnects the socket.

`ableton-music-maker --status` prints versions, paths and the activity switches as JSON;
`--check` runs the handshake plus `get_session_info` against Live and exits 0 only when the
script is loaded and up to date. Both are for CI and the Mac app.

## A tool call, end to end (`src/tools.rs`)

```
#[tool] method ── binds a ToolSpec to a body ──▶ Server::run
   └─ spawn_blocking ▶ run_blocking(live, spec, params, body)
         ├─ require(live, command)   capability check against the cached script info
         ├─ body(&LiveState, &Params) -> Result<String, String>
         │     └─ live.send_command(cmd, params)  ──▶ AbletonConnection ──▶ Live
         │           (each command and its Live time are noted in a thread-local trace)
         └─ live.activity.record(...)   one JSON line, local only
   └─ Ok(text)  → CallToolResult::success
      Err(text) → CallToolResult::error          (never a JSON-RPC error)
```

- **Tool bodies are plain functions** so tests call them through `run()` with a
  `FakeBridge`. The `#[tool]` method does nothing but bind.
- **Compact note forms** (`src/notes.rs`) — csv lines, step strings, patterns and tiling —
  expand into plain note objects *before* any command is sent, so a bad pattern touches
  nothing and the Remote Script only ever sees `add_notes_to_clip` with a note array.
- **Multi-command bodies** (`create_clip` with a name and notes, `duplicate_to_arrangement`
  with many placements) send their commands in order and stop at the first failure; the
  activity line lists every command that went out.
- **Orchestration bodies** (`batch`, `build_song`, `play_and_measure`) call other bodies
  through `run_named` (a name → params → body table) or directly; they never bypass
  `require`, and the activity line for the one tool call lists every command that went out.
  `batch` answers with a summary — steps grouped by tool and outcome, with the counts each
  group reported added up — then every failure and every line a step skipped, then each
  successful step's own text only when `verbose: true` or the batch is ten steps or fewer.
  A step that half-applied is counted in the header, so it cannot hide in a wall of ✓.
- **Capture** (`capture_mix`) is the only path out of Live: the script records the master
  through a Resampling track as a fixed-length Session clip, the server polls until the clip
  has a file, stops the transport (a `Drop` guard stops it on any early exit), and
  `src/audio.rs` reads the WAV or AIFF Live wrote and measures it. Nothing is copied.
  - **Capture needs the server on Live's machine.** The file is read off the server's own
    filesystem, and the recording folder belongs to the set, on the host. The native binary
    and the Mac app share that filesystem; the hardened container does not — it mounts
    nothing but `/state` and by the image's contract should not casually mount more. So
    capture is native-only today, and a read that fails says that instead of "no such file"
    (`unreachable_recording` in `src/audio.rs`). The route that would make it work in the
    container is the script sending the bytes over the socket it already has — it is on
    Live's machine, and the duplex protocol has streams — into a file under `state_dir()`;
    not a new mount and not a new socket. Not built ([#49](https://github.com/defoAI/mcp-ableton-music-maker/issues/49)).
    Nothing in the measurements needs the file to leave the server: peak, RMS per bar,
    crest, octave bands, LF/HF and stereo correlation are all in the reply.
  - **The playhead is confirmed before the fire.** The seek and the transport are
    asynchronous, so the script seeks to a whole bar before the asked bar, starts playing,
    and only fires once its own tick reads a playhead inside that preroll and still before
    the start — with bar quantization, so the take begins exactly on the bar. It reports
    `confirmed_at`.
  - **And the take is checked afterwards.** The server measures the leading silence
    (`audio::measure`); more than an eighth of a bar means the playhead had not arrived, so
    the clip is deleted and the take recorded again. A second bad take is an error with no
    measurements: a capture that starts early is not a mix observation.
  - **What it measures** is peak, RMS, crest factor, RMS per bar, ten octave bands
    (31 Hz–16 kHz), the low-to-high ratio, clipping and stereo correlation — one FFT pass
    shared by `band_balance` and the octave shares.
- **Performance** (`start_performance`, `cue`, `get_performance_state`, …) is the one place
  the script acts on its own clock. Claude plans, Live executes: `cue` resolves bar numbers to
  beats and names to indices against a fresh `get_performance_state` (`src/performance.rs`,
  pure logic, unit-tested), refuses a step in the past or a plan that leaves a bar silent, and
  hands the steps to `schedule_cue`. The script stores them and re-arms `schedule_message`
  every tick while work remains: launch steps are issued inside the bar before their target so
  Live's global quantization places them on the bar, sets and ramp steps land within a tick of
  their beat, and every step fired is queued as an event the next state read reports. A stopped
  transport or a time-signature change cancels pending cues. While a performance runs
  (`LiveState::performance`, in memory only) the transport-touching tools refuse with the
  on-the-bar alternative in the message.
- **The clock rides on the envelope.** While performance mode is on, the Remote Script
  attaches `clock` (bar, beat, seconds to the next bar, phrase, next cue) to every response
  it sends; `AbletonConnection::exchange` keeps it in the per-call trace and `run_blocking`
  renders it as the last line of every tool result. Zero round trips. Launch commands read
  the transport *after* they fire and report the bar the launch lands on.
- **Sections and songs** (`src/song.rs`, `src/sections.rs`) put a vocabulary on the performance
  layer without a state machine in the script. A section is a scene row named
  `<name> · <bars>`; the script parses the names into its phrase table on every state read and
  on every clock, so a Live restart loses nothing. The song is the setlist written into the
  name of the `Setlist:` scene. `play_song` fires the first entry and hands every counted jump
  to the script as one cue; each steering verb (`go`, `next_section`, `previous_section`,
  `back`, `jump_to`) is one state read and one `schedule_cue` whose `replaces` field retires
  the old plan in the same round trip, and `hold_section` is a `cancel_cue`. The server keeps
  only the cursor (which entry, the plan cue id, the jump history) in the performance record
  and re-derives the position from the row that plays. A move lands at the end of the playing
  section's phrase (`phrase.ends_bar`, counted from the bar the row was fired on) unless told
  `next_bar`, and then the reply says what it cuts. `make_section` is Live's own
  capture-and-insert-scene (`capture_scene`, which launches the copy and carries the phrase
  count over), `duplicate_scene` plus per-track rewrites, or `create_scene` plus clips.
  Transitions (`src/transition.rs`) are composed on the server into the cue primitives above
  (ramps, sets, fires, stops) and note rewrites written into the target row before the jump.
- **A performance is kept as a take.** Live writes Session launches into the Arrangement only
  while `Song.record_mode` is on, so `start_performance` and `play_song` arm it
  (`start_arrangement_record`) and `end_performance` disarms it (`stop_arrangement_record`,
  which also calls Back to Arrangement and reports the bars). `record` decides what happens to
  an Arrangement that already holds something: the default `ask` reads it
  (`arrangement_summary`, one pass over the tracks) and, when it is not empty, **returns the
  question having sent nothing else** — the producer answers `after` (the next bar line past
  the last clip), `replace` (delete it, record from bar 1) or `off`. The answer is remembered
  for the session; `replace` never is. There is no `undo` command, so `replace` names Cmd+Z.
  Live 10 has no `track.arrangement_clips`, so it cannot be asked about and is not recorded.
  A failure between arming and the first fire disarms Live again, and the script's tick clears
  `record_mode` when the transport stops, so a take never outlives its performance.
- **Levels ride with the clock.** The script's tick keeps the master and per-track meter peaks
  for the current and last bar and the master's peak per scene row; they travel as `levels` on
  the `clock` envelope and the state, `run_blocking` renders the 🔊 line under the ⏱ line, and
  a jump into a row that ran more than 3 dB hotter warns before the mix clips.
- **A meter reading is dB, and the curve was measured, not guessed.**
  `output_meter_left/right/level` is Live's *meter scale*: **linear in dB**, 0.0 reading −70 dB
  and 1.0 reading +6 dB, so 0 dBFS sits at 0.921. It is neither linear amplitude (20·log10 of
  it is not dB — a 12 dB fader move used to read as about 2 dB) nor the fader taper (which
  would read it as 6.3 dB). Measured on Live 12.4.6 against a −12.0 dBFS file at seven fader
  positions over 42 dB: a straight-line fit gave `dB = 76·v − 70` with zero residuals, and the
  reading now matches the file to 0.1 dB. `get_meter_scale` hands the law to the server, which
  caches it per session (`song::MeterScale`) and converts every raw meter it renders; the same
  law is `song::meter_db`, so a script that predates the command still reads in real dB. Every
  meter readout says post-fader.
- **A device is addressed the way the loader takes it** (`kind: track | return | master`).
  `tools::resolve_track_target` turns a name, an index, `"master"` or a return's name or
  letter into that pair, `resolve_device_index` turns a device name into its chain index, and
  every device-facing command (`get_device_parameters`, `set_device_parameter(s)`,
  `get_track_info`, `get_drum_rack_pads`, `delete_device`, `move_device`) carries the `kind`
  the script resolves with `_resolve_track`. So anything that can be loaded can be read,
  corrected and removed.
- **So is everything else that takes a track.** `resolve_track_target` is the one resolver:
  every core tool that addresses a track runs through it, and `tests/artist.rs` fails the
  build when a core tool takes `track_index` without `track`. Beside it,
  `tools::resolve_clip_slot` turns a Session clip's name into its slot (read off
  `get_track_info`) and `tools::resolve_bar` turns a locator's name into a bar — the locators
  are read through the generic ops layer (`song.cue_points`), so no command was added for it.
  That read needs a collection to come back as a collection, which is why script 1.35.0
  renders Live's `Base.Vector` as a list: it is neither a `list` nor a `tuple`, and rendering
  it as its repr gave the count 0, so every locator name was refused as "the set has none"
  while Live held four. A read that fails now says so instead, rather than reading as an
  empty set.
  Two rules hold all three: an **ambiguous** name lists every candidate and changes nothing,
  and a name that matches nothing is an error listing what does exist — it never falls back
  to an index, because reading "Drop" as slot 0 is how the wrong clip gets overwritten. The
  index forms stay, because `build_song` documents them and `batch` payloads send them.
- **A parameter carries what Live shows.** The Live Object Model has no `value_string`; it has
  `str_for_value(value)` and, for a quantized parameter, `value_items`. The script reads both
  in `_serialize_parameter` — the display string, the range as display strings, the labels of
  a chooser — and the server renders them as a table. A value may be set by its display
  string (`"200 Hz"`, `"Low Cut 48 dB"`): the script bisects `str_for_value` for a continuous
  parameter and matches the label exactly for a quantized one, so a wrong label is refused
  with the list rather than silently written. The strings cost Live three calls per parameter,
  so they are read for the device a person asked about and left out of the snapshot and the
  rack-chain walk.
- **The song remembers itself** (`src/memory.rs`). What Live can hold, Live holds — measured,
  not assumed: a `describe` sweep of a real Live 12.4.6 shows the only writable text in Live's
  object model is a `name`. So a track's **role** is a suffix on its name (`Sitar [lead]`) and a
  parked idea is a clip in a `Stash:` scene row — the same trick as the `Setlist:` scene, kept by
  Live's own Save, travelling in the `.als`, working with no server at all. `is_reserved_scene`
  is what keeps both rows out of the sections: a `Stash:` row is never listed, launched, counted
  or played. Only what has nowhere to go in Live is written down: the **overview** (the agent's
  model of the track, capped at 8 KB), the **notes**, and a per-session **digest**, in
  `state_dir()/songs/<key>.json`, keyed on `song.file_path` read through the generic ops layer —
  no command, no `SCRIPT_VERSION` bump. A set that was never saved is filed provisionally and
  the first Cmd+S renames the file and says so once — **looked for on disk, not only in the
  running process**, because the session that wrote the notes is usually not the one running
  when Live finally reports a path: the producer builds, closes the client, saves, and comes
  back tomorrow. One provisional file serves the machine, so it is adopted only into a song
  that has no memory of its own, and the adoption is announced rather than silent.
  **Retrieval is not a call**: the whole overview rides back in the `get_context` header, the
  call an agent makes first anyway, because a "load my memory" call is one an agent can fail to
  make and the turn it skips it on is the first turn of a session. That header is derived from
  the `get_context` payload already in hand — `get_context` stays one round trip for the set,
  plus the single op that reads the set's identity. Full on the first call of a session and
  after any change, one line after that: a server rule, so the agent decides nothing and a
  repeat call stops spending the cap on something unchanged. The staleness check cannot ride
  there alone for the same reason, so the drift line also travels on `capture_mix` and
  `play_song` — and it costs nothing when there is no overview to be stale.
  `reset_set` forgets the file: the plan it held described tracks and clips that call deletes.
  Notes are **never repointed by guess** — a rename is reattached only when exactly one unspoken
  track is named like the missing subject, and anything less certain is reported and left alone.
- **The sound vocabulary** (`src/sound.rs`) is a table, not a guess: a word is resolved at call
  time against the device's rack macros by name, then candidate parameter names per Live
  instrument, then aliases and the word itself; `shape_sound` writes several words through one
  `set_device_parameters`, and a cue ramp takes a word the same way.
- **What that table cannot reach, the server learns** (`src/devices.rs`). `sound::vocabulary`
  keys on the device *class*, and a preset is not a class: "Vinyl Drawbs" is a rack whose
  macros were named by whoever made it, and no compiled table can know it has no cutoff. But
  the failure already produces the answer — `shape_sound` reads the real parameter names off
  the device to print them — and every write is read back with Live's display string. That is
  kept, keyed on the **device** (name and class) and the **Live version**, never on a song,
  under `state_dir()/devices/<live-version>.json` beside the browser index and behind the same
  `ABLETON_MCP_LIBRARY_INDEX=false`. Nothing extra is asked of Live: these are the reads that
  already happened. Two rules make it safe to keep. Every row is **stamped** — Live version,
  device name and class, the day — and a Live whose version is not known yet stays in memory,
  because an unstamped measurement does not ship. And a stored value is a **hint, never
  truth**: the names are re-read on every use, and an observed value → display pair is only
  ever reported. A macro that ran the other way round (`devices::backwards`) is *said*, with
  both measurements, rather than silently inverted — a server that quietly flips a number is
  one the producer cannot reconcile with what Live shows them.
  `adv_device_vocabulary` shows all of it and deletes it, and asks Live nothing.
- **A write is read back.** The script re-reads the parameter after setting it and sends
  `asked`, `landed`, `is_enabled`, `is_quantized` and `automation_state` beside the value
  (`_landing`). `landing` in `src/tools.rs` turns that into one of three answers: a clean
  landing, a note that a quantized parameter snapped to its nearest step, or an error naming
  why nothing moved. A parameter Live ignored is never counted as a step that happened —
  in `adv_set_device_parameter` or in one of `shape_sound`'s per-word lines. A script that
  sends no `landed` gets the plain before-and-after echo, since nothing may be assumed of it.
- **Set memory is on request.** `export_set` (`src/sets.rs`) reads one session snapshot and one
  context into a rebuildable document under `state_dir()/sets/` and nothing else creates that
  folder; `import_set` rebuilds through `build_song`, `set_scale` and `set_song`. The Live set
  (its scene names) stays the memory.
- **The surface is one list, prefixed** (decision 0006). `Server::tool_router` builds every
  tool, then re-keys each one outside `CORE_TOOLS` as `adv_<name>` and sets the MCP
  annotations from the name; `run_named` strips the prefix, so `batch` takes either spelling. Nothing is gated: the client's user chooses. The
  artist tools in `src/arrange.rs` compose the raw bodies (`feel` runs the note rewrites in
  order and keeps one undo; `arrange` converts bars to beats once and sends one
  `place_clips` / `delete_arrangement_clips` / `duplicate_arrangement_clip` per track, the
  script looping on Live's main thread so a 200-clip change is one round trip). Faders in dB
  are the script's doing: it bisects Live's own fader curve with `str_for_value`, so the
  number the artist reads in Live is the number they said.
- **The library index.** `src/library.rs` pages the script's browser walk in the background
  after the handshake (one-second pages so tool calls interleave on the shared socket),
  keeps it under `state_dir()/library/` and answers `search_browser` and every internal
  lookup locally once complete. **Complete means the walk finished *and* every walked item
  was fetched**: the script's `index_complete` reports only the first of those, and reading
  it as both stopped the paging after one page — on a Live 12 Suite that left 1,000 of
  12,036 items on disk marked complete, so "Boom Bap Kit" and "reverb" read as absent from a
  library that had them. The file carries a format number so a truncated one written by an
  older build is re-walked rather than believed.
- **The sample index.** `src/samples.rs` asks the script once where samples live
  (`list_sample_folders`: the Core Library inside the application, the Packs and the User
  Library around the script's own folder, the open set's folder from `Song.file_path`, and
  Live's Places when Live exposes a path for them) and then walks those folders itself,
  reading each WAV or AIFF header for a length. The walk is on this side because file I/O in
  Live's embedded interpreter measured about two files a second against Live 12.4.6, against
  ~5,000 natively: 9,904 files took 2.0 s here and would have taken over an hour there. A
  server that cannot see the producer's disk — the Docker variant — finds nothing, says so,
  and Live's browser answers `search_browser` instead. `add_sample` then sends one
  `place_sample`, which creates the clip and fits it inside a single task so Live records one
  undo step; the script reads `looping`, `loop_end`, `start_time` and `end_time` back off the
  clip and the reply states what Live actually did, because Live's own Auto-Warp decides how
  the material is heard and an Arrangement clip's `end_time` cannot be set at all.
- **Orientation and instructions.** `get_context` is one Remote Script round trip that returns
  the set, every track, the returns, the scenes and the performance clock; `src/context.rs`
  renders it and holds the MCP `instructions` string the server sends at `initialize` (the
  `get_info` override in `tools.rs`), so a client's model knows the workflow before its first
  call.
- **Whole sections in few round trips.** `create_tracks` and `write_clips` take the document
  `build_song` (and `make_section`) validated on the server and do every create, name,
  instrument load, fader and note write in a main-thread task; a copy in another row is
  made inside Live from the first clip (`copy_of`), so the notes cross the socket once. With
  `place_clips` and `delete_arrangement_clips` this is the answer to the 200 ms floor below:
  the count of round trips, not the size of any one, is what a producer waits for.
  Tracks are the exception, and the one place the server deliberately spends a round trip.
  Every track reinitialises Live's audio graph and most load a device from disk, and ten of
  those in one command took Live down twice on a producer's machine — so `build_song` sends
  them `TRACKS_PER_GROUP` at a time, looks at Live between groups, and writes each track into
  the reply as its group lands. A death then costs one group instead of the document, and the
  reply names the tracks that are really in the set and how to finish the build.
- **A round trip costs about 200 ms, whatever it does.** Measured on Live 12.4.6 over one
  persistent socket: an unknown command, a tiny read and `get_context` all answer in the same
  200 ms, with or without the per-command log line and with `TCP_NODELAY` on both ends. The
  floor is Live scheduling the script's socket thread, not the command. Design consequence:
  fewer round trips beat smaller payloads; `get_context` is one call where three used to be,
  and `batch` / `build_song` exist for the same reason.
- **`require(live, cmd)`** is the capability check: the command must be in the script's
  advertised `SCRIPT_CAPABILITIES`. A missing or outdated script produces one clear "run the
  installer, then restart Live" error instead of a half-working session. A unit test
  cross-checks `tools::ALL_REMOTE_COMMANDS` against the script's list.
- **The socket is synchronous.** Bodies run on the blocking pool; nothing calls the bridge
  from async code.

## The handshake and capabilities (`src/handshake.rs`)

The server only talks to the Remote Script it ships with. `expected_remote_script_version`
reads `SCRIPT_VERSION` out of the embedded source at startup — the constant in the script is
the single source of truth. `ScriptInfoCache` holds the last handshake result (version,
protocol version, capabilities, up-to-date flag) and is refreshed when a tool finds it empty.

Adding a Remote Script command therefore means: handler in the script → name in
`SCRIPT_CAPABILITIES` → bump `SCRIPT_VERSION` → add to `ALL_REMOTE_COMMANDS` → tool body.

## The connection (`src/connection.rs`)

`AbletonConnection` owns one `TcpStream` behind a mutex. A request and its response are one
indivisible exchange, so the whole round-trip — including reconnect — runs under the lock;
tool calls and the passive poller share the socket. `RealBridge` wraps it with reconnection.
Timeouts are per command (`command_timeout`): importing audio can hold Live's main thread
far longer than anything else, modifying commands get a wider budget than reads, and the
connect itself has its own. `LiveError` is the error type every body sees.

`LiveBridge` is the trait; tests substitute a `FakeBridge` (`tests/common/`).

## The activity log (`src/activity.rs`)

One JSON line per tool call, appended to `<state_dir>/activity/<session-id>.jsonl` where
the session id is the start time plus the pid: `ts`, `tool`, `commands` (what was sent to
Live), `ok`, `error`, `duration_ms`, `live_ms`, `in_chars`, `out_chars`. `params` and
`result` are added only under `ABLETON_MCP_ACTIVITY_PAYLOADS=true`; `ABLETON_MCP_ACTIVITY=false`
turns the file off. A write failure is logged to stderr once and never fails the tool.
`tests/activity.rs` pins the defaults.

## Where the server writes (`src/state.rs`)

`ABLETON_MCP_STATE_DIR`, else `~/.ableton-music-maker/`, with `activity/`, `sessions/`,
`library/` (the browser index and the sample index), `sample_folders.json` only after an
explicit `adv_sample_folders add`, and, only after an explicit `export_set`, `sets/` under it. Nothing else. There is no upload path: no HTTP client in the dependency tree (CI
fails if one appears), no telemetry, no dataset — `tests/local_only.rs` is the policy and
[decision 0004](../decisions/0004-who-publishes-and-holds-the-data.md) the reason.

## The Mac app (`app/`)

A Tauri 2 menu bar app whose Rust core depends on this crate by path and calls `install`,
`handshake`, `connection`, `state` and `app::check` as functions. It never runs the server
Claude talks to: it carries the server binary as a sidecar at `Contents/MacOS/ableton-music-maker`,
writes that path into Claude Desktop's config (backup first) or shows the command for Claude
Code and Cursor, and derives everything it shows from files the server writes:

| Screen | Reads |
|---|---|
| Overview: the chain client → server → Live | live heartbeats (a dead pid is deleted on sight) and one `check` against Live every 10 s while open |
| Delete all local data | removes the activity and session files, the library index and the set exports |
| Activity | the session's `.jsonl`, re-read every 2 s; tokens are `ceil(chars / 4)`, labelled *est.* |
| Prompts | nothing at run time. `app/src-tauri/src/prompts.rs` embeds the four markdown files in `prompts/` at the repository root with `include_str!` and splits each into title, when-to-use and body; the screen shows the body and copies it to the clipboard. The banner reuses the chain state the Overview already computed, so opening the screen probes nothing. Copying writes no activity line and no file |
| Setup | Library.cfg discovery for the script path and its `SCRIPT_VERSION`; `check`; the client config |
| Settings | its own `settings.json`; the switches that concern the server are written into the client config's `env` block, because only the environment reaches a client-started server |
| Listen | a Core Audio process tap of Live's process ([0008](../decisions/0008-app-hears-live-through-a-process-tap.md)) (`src/listen/tap.m`, compiled by `cc`; every 14.2 symbol weak-imported behind `@available`, so the app still launches on macOS 12), drained by an analysis thread into 72 log-spaced bands, master peak / RMS / correlation, six range shares and a 256-point waveform, sent to every window as a `listen:frame` event about thirty times a second. Nothing is written. Silence longer than three seconds asks Live over the existing socket whether its transport runs, so the screen can tell "quiet" from "the permission is off" |
| Float, Visual | two more windows fed by the same events: a small always-on-top spectrum (`float.html`) and a WebGL feedback visualiser with eight presets and full screen (`visual.html`). Listening stops when no window is showing it |

The menu bar item shows the same three states and the last call, refreshed in the background
while the window is closed.

## The Docker image (`Dockerfile`, `docker/verify-image.sh`)

Three stages: `builder` (release build, dependency layer cached separately), `test`
(`cargo test` inside the image with both disable variables set), `runtime`
(`distroless/cc-debian12:nonroot` plus the two binaries). The runtime image bakes in
`ABLETON_HOST=host.docker.internal`, both disable variables, and points every writable path
(`ABLETON_MCP_STATE_DIR`, `ABLETON_MCP_DATA_DIR`, `HOME`) at `/state`, the only volume, so the
root filesystem runs read-only. No `HEALTHCHECK` and no exposed port: the server speaks stdio
and only makes an outbound connection.

The contract is checked, not assumed: `verify-image.sh` asserts no shell, non-root, both
disable variables, every privacy gate off with no credentials, a working `initialize`
handshake over stdio with nothing but JSON-RPC on stdout, the size budget, and a working
installer binary. Changing the Dockerfile means re-running it.

What cannot be containerised: the Remote Script. `docker compose --profile install` runs the
installer with the User Library bind-mounted; Live still has to be restarted and the control
surface selected by hand.

## The fake Live (`tests/remote_script/fake_live.py`, `scripts/fake-live.py`)

The test suite has a Live in it. Not a stub that returns canned JSON — a **Live Object
Model** the real Remote Script runs against, so a green suite means the set was really
built, not that the right commands were sent to nobody.

**The model.** `tests/remote_script/fake_live.py` is Song, Track, ClipSlot, Clip, Device,
RackDevice/Chain/DrumPad, DeviceParameter, MixerDevice, Scene, CuePoint, AutomationEnvelope,
MidiNote, Groove/GroovePool, both Views, Browser/BrowserItem and Application, with Live's
semantics where the script depends on them: sequences are `Vector` (not a list and not a
tuple, which is why `describe` types them `Vector`; `run … get` renders one as a list from
script 1.35.0, and the model stays sequence-shaped without becoming a list so it keeps
telling `describe` the truth),
`set_notes` adds,
`get_notes` and `get_notes_extended` take their arguments in different orders, every track
has one clip slot per scene, a return gives every track a send, a parameter outside its
range raises, `set_or_delete_cue` toggles at the play position. An unnamed track's name is
**derived from its position** — delete the first of four and the rest renumber — and the
volume fader shows Live's measured taper: a straight 40 dB per unit above 0.4 and a quadratic
below (−18 dB at 0.4, 0 dB at 0.85, +6 at 1.0), printed to three decimals trimmed.
`default_set()` is what a new Live set is: `1-MIDI`, `2-MIDI`, `3-Audio`, `4-Audio`, the
returns `A-Reverb` and `B-Delay`, eight scenes, 120 BPM.

**Starting from the same place.** `reset_set` (Remote Script 1.34.1, served as
`adv_reset_set`) empties the open set back to that: every clip, locator and scene name gone,
the tracks and scenes and tempo back to a new set's. It is a generator, because deleting
tracks holds Live's main thread. The Live API has no File > New — nothing here opens a
document — so it clears the set that is open, and the tool says so rather than implying
otherwise. `song_length` is the one thing it cannot move: Live keeps it at the furthest the
set has ever reached. Both differentials below open with it, which is what makes two runs
comparable at all.

**The script, not a copy of it.** `harness.py` puts the model behind the real script —
`song()` and `application()` are the model, `sys.modules["Live"]` is `live_module()` — and
`scripts/fake-live.py` adds the two things Live adds: the script's **own** socket server
(nothing here reimplements the protocol) and a main thread that calls back every 100 ms.

**The tick is Live's.** 100.0 ms, measured 2026-09-20 on Live 12.4.6 through
`ableton-music-maker --check`: `period_ms 100.0, jitter_ms 0.24` over 211 samples, the same
period decision 0010 recorded over 600. The script's `_TickSampler` then reports Live-like
figures, so `get_script_info.tick` and the clock channel's rate are testable with no Live
open. `--tick-ms` overrides it for a fast run and the reply says so.

**A call costs what it costs in Live.** `LATENCY_12_4_6` is a per-call table, and every row
is a measurement with its source stamped on it: `Browser.load_item` 839 ms typical /
1411 ms worst (#43's activity log, #45's device loads under stress), `Song.create_scene`
68 ms, `Song.delete_track` 75–110 ms, `ClipSlot.create_clip` 5.4 ms, `Clip.set_notes`
0.5 ms. The executor's slice budget then spreads work over ticks as in Live — four tracks
with instruments come back in four slices — `main_ms` is real, and a slice over 25 ms lands
in the log as it would in `Log.txt`. **Calls nobody has measured charge nothing and are
named** in `LATENCY_UNMEASURED` (`Song.create_midi_track` among them: #43's 0.25 s figure is
a round trip, not main-thread time), so the gap is visible in
`scripts/fake-live.py --latency-report` rather than quietly filled with a guess.

**What it is not.** No audio — meters read what a test put there. No rendering. No real
browser index: a couple of dozen items, not Live's library. No Max for Live. No plug-ins.
Anything that depends on sound is a real-Live check, always. `TheModelIsNotLive` in
`tests/remote_script/test_live_semantics.py` pins each of those as a test, so the limits are
executable rather than a paragraph nobody reads.

**Isolation: one process, one set, per test.** Each Rust test starts its own
`scripts/fake-live.py` with `--shared-set` — one set behind every connection, which is what
Live is — and kills it when the test's bridge is dropped. Nothing is shared, so nothing has
to be reset, and there is no divergence from Live to remember. It costs about 110 ms to
start one and eight start in 131 ms because they overlap, so the suite pays a few seconds
for perfect isolation. `--exit-with-pid` stops any process that outlives a crashed test
binary.

`--set-per-connection` also exists, and gives each connecting client its own
`default_set()` inside one process. That one **is** a divergence: Live has one set and one
main thread, and the event channels and scheduled cues run on that thread rather than on a
socket, so those channels read the first connection's set. A `subscribe` from any other
connection is therefore **refused**, naming `--shared-set`, rather than answered about a set
it is not looking at. Nothing in the Rust suite uses that mode; it is there for a Python
test that wants two clients in one process.

**Debugging, and failing on purpose.** `--record FILE` writes the wire as it went, one JSON
line per request and per reply; `--script-log FILE` collects the script's own log lines
(Live's `Log.txt`, slow slices included); `--die-on COMMAND` and `--die-after N` make Live
go away mid-session, which is #45 as it actually happened, so the resume path is testable;
`--slow CALL=MS` gives a call a cost, marked `NOT MEASURED` everywhere it is reported. On
exit it prints a readout — commands, worst `main_ms`, slices, errors, what each Live call was
charged — to put beside `scripts/live-latency.sh` against a real Live.

**Keeping it honest.** The fake is checked against the real Live four ways.

1. **The conformance test.** `scripts/live-api-surface.py` reads every Live member the script
   touches out of it by AST — 157 members over 15 classes — and the model must have each with
   the right shape. That is the floor: it proves the member exists, not that it behaves.
2. **The transcript differential.** `scripts/live-differential.py` sends one fixed script of
   commands to a real Live and to the fake and compares every field of every reply, with an
   allow-list for what legitimately varies (`main_ms`, ids, Live's colour choice) and for
   library content — the fake's browser is a miniature and a real Live has whatever that
   producer installed, so comparing the contents would be comparing two hard disks.
   `scripts/live-transcript.sh` records the real half into `tests/fixtures/`, and
   `test_transcript_differential.py` replays it with no Live open. **0 differences over 34
   steps** as of 2026-09-20 against Live 12.4.6.
3. **The object-model sweep.** `scripts/live-lom-sweep.py` asks `describe` of both for 25
   paths and compares attributes, methods, `readonly`, types and the value of every readable
   attribute — as **sets**, because `describe` returns sorted lists and comparing them by
   index turns one missing member into a hundred false differences. It splits the answer:
   **in scope** is what the script actually uses plus anything the model invents, and must be
   zero; **out of scope** is the rest of Live's model, counted and left alone. Live's Track
   carries about 150 methods and the script calls 22 — completing the other 128 would be work
   with no reader, and a member that comes into use moves into scope by itself.
   `test_lom_conformance.py` runs it against `tests/fixtures/live-lom-<version>.json` with no
   Live open. **In scope: nothing differs.** Out of scope: 1115.
4. **The suite itself.** `ABLETON_TARGET=live cargo test -- --test-threads=1` runs every
   converted suite against a real Live instead of the fake.

What the differentials found is the argument for having them: the fader taper was wrong
(−30 dB at 0.4 where Live has −18), track names are derived and not stored, a send is named
after the return it feeds, loading an instrument arms the track and `exclusive_arm` disarms
the rest, note ids run from 1 within a clip, `gain` and `pitch_coarse` are audio-clip
members, and Live's master track has no `arm`, `mute` or `solo` at all. None of that was
guessed; each was measured against a running Live and is stamped where it is encoded. The
rules for keeping the two in step are in `CLAUDE.md`.

## CI (`.github/workflows/ci.yml`)

Two jobs, both on macOS, because that is the only platform the product ships for
([decision 0009](../decisions/0009-ci-builds-the-mac-app-only.md)). `rust` job:
`cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test`, the dependency
gate that fails on an HTTP client, and `scripts/check-docs-facts.sh`. `mac-app` job, on an
Apple Silicon runner: `tauri build --target aarch64-apple-darwin`, then
`app/scripts/verify-dmg.sh` on what it produced, then the `.dmg` as a build artifact — ad-hoc
signed, so it opens only on a machine it was not downloaded to.

The Docker image is not built here. `docker build --target test .` and
`docker/verify-image.sh` still work and are still the contract; they are run by hand now.

Caching: `Swatinem/rust-cache` keeps each job's dependency artifacts (the `rust` job under
its own key, the `mac-app` one under `shared-key: mac-dmg`, which the tag build reads so a
release does not start cold) and `setup-node` keeps the npm download cache. Only `main`
writes the macOS cache — the
repository has 10 GB for all of it, and a 600 MB cache per branch would evict what matters.
Workspace crates are deliberately not cached: they change with every commit, so the two
crates always recompile and that is the floor on the `mac-app` job.

`.github/workflows/release.yml` is the same build on a `v*` tag, with Tauri signing against
the Developer ID certificate and notarising from six repository secrets, verified with
`EXPECT_SIGNED=1` (Developer ID authority, stapled ticket, Gatekeeper accepts) and attached
to the GitHub release. It stops before building when a secret is missing: an unsigned
download is not something to publish.

## Boundaries worth knowing

- **One server instance at a time**, across all clients — the Remote Script serves one
  socket and the server holds one connection. The app warns when two heartbeats are alive.
- **Live's main thread is the bottleneck, and the only thread** (decision 0007). The
  script's socket thread parses JSON and waits; `_run_on_main` schedules one task per
  command on Live's main thread and `_dispatch` runs it there — reads, writes, the clock
  stamp. A handler with many units of work is a generator the executor slices: 8 ms per tick
  while the transport runs, 40 ms while stopped, re-armed with `schedule_message(1)`. Each
  mutating command is one undo step. Every reply carries `main_ms` and `slices`; the
  activity line carries their sum, so what a tool cost Live is a number in the log. A slice
  over 25 ms is written to Live's log with the command name. The timeouts exist because a
  big audio import still blocks everything else.
- **The Remote Script binds loopback by default** and `bind_host.txt` beside it overrides
  that — [0003](../decisions/0003-remote-script-bind-address.md). `get_script_info` reports
  `bind_host`, and `--check` prints it.
- **Arrangement commands are Live 11+**; the script has Python 2 branches for Live 10 but no
  test matrix across versions.
