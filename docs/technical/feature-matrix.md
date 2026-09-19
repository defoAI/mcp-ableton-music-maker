# Feature matrix

| | |
|---|---|
| **Verified** | 2026-09-19 against `src/tools.rs`, `src/connection.rs`, the Remote Script's `SCRIPT_CAPABILITIES`, `Dockerfile` and `docker/verify-image.sh` |

What the product does, at capability level. For *why* see
[product-overview](../strategy/product-overview.md); for what is next see
[product-plan](product-plan.md); for the numbers see [source-of-truth](../facts/source-of-truth.md).
The tool descriptions themselves are the `#[tool]` doc comments in `src/tools.rs` — this
table is a copy for orientation.

---

## Tools, by area

The tools without a mark are the artist's set (`CORE_TOOLS`, decision 0006); a tool marked
*advanced* here is served as `adv_<name>`. Faders and levels are in dB, Arrangement positions
in Live's 1-based bars.

| Area | Tool | Remote Script command(s) | Notes |
|---|---|---|---|
| Arrange | `arrange` | `place_clips`, `duplicate_arrangement_clip`, `delete_arrangement_clips`, `get_arrangement_clips` | place a Session clip at a bar or every N bars up to a bar, repeat an Arrangement clip after itself, move one to a bar, delete the clips starting in a bar range, shorten the whole arrangement to end at a bar (clips that run past it are named: Live's API cannot trim), list; one round trip per track however many clips |
| Shape | `feel` | `get_clip_notes` + `add_notes_to_clip`, `get_grooves` + `set_clip_groove` | swing, humanize_ms, a Groove Pool groove, groove_amount, retime, a variation, any mix in one call, seeded; `undo: true` restores the clip as it was before the call |
| | `set_key` | `set_scale` | "F minor", "D dorian": Live 12's scale settings; `build_song` takes `key` and sets it first |
| | `create_return` | `create_return_track` + `load_browser_item` | a return track, optionally with an effect by words or URI; `load_instrument_or_effect` takes `kind` return / master |
| | `set_track_mixer` | `set_track_mixer` (the script bisects Live's own fader curve with `str_for_value`) | `volume` is dB (`volume_db` too), `fader` is Live's raw 0–1; dB in and out, `get_context` and `get_returns` show dB |
| Build | `add_sample` | `list_sample_folders` + `place_sample` (+ `create_tracks` when the track is new) | audio into the song: words, an absolute path or a browser URI, into a section's row (`section`/`slot`) or the Arrangement (`at_bar`). One `place_sample` creates the clip and fits it, so Live undoes it in one step: warping on, the loop snapped to a whole number of bars when Live's own warp landed within 10% of one, `bars` to force a length, `transpose` in semitones, `fit: false` for exactly what dragging the file in gives. A row loops, a bar is one hit. Measured on Live 12.4.6: 1–46 ms of Live's main thread for the placement, ~100 ms when it makes the track (Live's own work). **Needs Live 12** — `ClipSlot.create_audio_clip` (12.0.5+) and `Track.create_audio_clip`; the script says so on an older Live. A browser item has no file path until it is a clip, so it can go in a row but not at a bar. An Arrangement clip is as long as its material: Live's `end_time` is read-only, so a longer `bars` sets the clip's loop and the reply says what the timeline shows |
| | `sample_folders` *advanced* | `list_sample_folders` | list, add, remove, refresh the folders Claude looks in. Live's own (Core Library inside the application, Packs, User Library, the open set's folder, Live's Places) always count and need no adding; a path Live does not report back is refused, not kept |
| Play | `clear_captures` | `get_context` + `delete_track` / `delete_clip` | removes the Capture track (or only its clips); the audio files stay in the project |
| Session | `get_context` | `get_context` | **start here**: one round trip for the set, every track (kind, arm/mute/solo, mixer, devices, clips by slot, play state, Arrangement count), returns, scenes, the performance clock, optional library summary; ends with the workflow reminder |
| | `get_session_info` *advanced* | `get_session_info` | tempo, tracks, view |
| | `get_session_snapshot` | `get_session_snapshot` | compact by default: empty slots and scenes dropped, counts kept; `compact: false` for the raw dump |
| | `set_tempo` | `set_tempo` | |
| | `start_playback`, `stop_playback` | same names | |
| Tracks | `get_track_info` | `get_track_info` | |
| | `create_midi_track`, `create_audio_track` | same names | result names the new track's index |
| | `set_track_name` | `set_track_name` | |
| | `set_track_mixer` | `set_track_mixer` | volume (0.85 = 0 dB), pan, mute, solo, arm; tracks, returns, master |
| | `set_send`, `get_returns` | `set_send`, `get_returns` | sends by return name, letter or index |
| | `set_color` | `set_track_color` / `set_clip_color` | Live palette index 0-69 |
| Clips | `create_clip` | `create_clip` (+ `set_clip_name`, `add_notes_to_clip` when `name` / notes are given) | Session slot; names and fills the clip in one call |
| | `create_audio_clip` | `create_audio_clip` | 65 s socket budget — large imports hold Live's main thread |
| | `get_clip_notes`, `add_notes_to_clip`, `clear_notes_from_clip` | same names | MIDI in, MIDI out. `add_notes_to_clip` takes compact forms — `steps` strings per pitch, `patterns`, `notes_csv`, `loop_every`/`until` tiling, note names — expanded server-side (`src/notes.rs`); `clear: true` replaces instead of appending. `tests/clip_notes.rs`, `tests/arrangement.rs` |
| | `set_clip_name`, `delete_clip` | same names | |
| | `fire_clip`, `stop_clip` | same names | |
| Devices | `get_device_parameters`, `set_device_parameter` | same names | racks and nested chains readable |
| | `load_instrument_or_effect` | `load_browser_item` | by browser URI |
| | `load_drum_kit` | `get_browser_items_at_path` + `load_browser_item` | |
| | `get_drum_rack_pads` | `get_drum_rack_pads` | pitch, Live note name (C1 = 36), pad name per pad with a sound; finds a Drum Rack nested inside an Instrument Rack |
| Browser | `search_browser` category `samples` | `list_sample_folders`, then the server's own walk (`src/samples.rs`) | files, not browser items: each hit carries its absolute path and, for a WAV or AIFF, its length from the header. Built on the first sample search, never at start-up; 9,904 files in 2.0 s on Live 12.4.6, then 8 ms a search with no round trip. The walk is here and not in the Remote Script because file I/O in Live's embedded interpreter measured ~2 files a second (2026-09-19). A server that cannot see those folders (the Docker variant) finds nothing, says so, and Live's browser answers instead |
| | `search_browser` | the server's library index (`get_browser_index` pages, `src/library.rs`); `search_browser` while the walk is incomplete | words matched against name and folder path; `queries: [...]` many per call, `best: true` one URI each; answered with no round trip once the index is complete; the index is walked in the background after the handshake (one-second pages), kept on disk under the state dir, `refresh: true` re-walks |
| | `get_library_status` | `get_library_status` | Live version, instruments/effects present, Suite instruments missing, packs installed; undownloaded packs are invisible to the API and the result says so |
| | `get_browser_tree`, `get_browser_items_at_path` | same names | the tree now recurses two folder levels |
| | `load_instrument_or_effect` result | `load_browser_item` | reports the loaded device's name and index |
| Clips | `get_clip_info`, `set_clip_loop`, `set_clip_launch` | same names | loop points, markers, launch mode/quantization, legato |
| Automation | `set_clip_automation`, `get_clip_automation` *advanced* | same names | device parameter or mixer target; points or a ramp; steps at `resolution`; **Session clips only** — Live's API answers "Not a session clip" for an Arrangement clip (verified on 12.4.6), so the tool refuses before Live and says to automate the Session clip and place it |
| Listening | `get_track_meters` *advanced* | `get_track_meters` (on Live's main thread; off it the meters read as zero) | one reading of every output meter, in dB |
| | `play_and_measure` | `set_current_song_time` + `start_playback` + `get_track_meters`×N + `stop_playback` | peak per track over a played stretch; names silent tracks |
| Capture | `capture_mix` | `ensure_capture_track` + `start_capture` + `capture_status`×N + `stop_capture` + `set_clip_name` | records `bars` bars of the master into the Capture track (Resampling, muted, armed), reads the WAV/AIFF Live wrote, reports peak, RMS per bar, silent bars, clipping, stereo correlation. **Live 11+** |
| | `list_captures`, `measure_capture` | `list_captures`, `capture_status` | the captures on the Capture track; re-measure one without playing |
| Performance | `start_performance` | `get_performance_state` + `set_launch_quantization` + `set_track_mixer`×armed + `set_scale` + `fire_scene` / `start_playback` | sets the global launch quantization (default 1 bar), disarms every armed track (Live arms new MIDI tracks by itself; an armed track records on a scene launch), sets the key in Live 12's scale settings when given (the parameter wins over the set), fires the first scene, turns on the guards: `stop_playback`, `set_tempo`, `set_arrangement_time`, `switch_to_arrangement_view`, `back_to_arrangement`, `play_and_measure`, `capture_mix` and `delete_track`/`delete_clip` on a playing or queued target are refused until `end_performance` |
| | *every result while performing* | the `clock` field on every response envelope | the Remote Script attaches bar.beat, seconds to the next bar, the phrase end and the next cue step to every response while performance mode is on; `run_blocking` renders it as the closing ⏱ line of every tool result. No extra round trip |
| | `get_performance_state` | `get_performance_state` | bar.beat, tempo, quantization, key; playing and queued clip per track; scenes with phrase lengths; pending cues; seconds to the next bar; what the script's clock did since the last call; `bar_map: true` adds the next 32 bars; the measured round trip |
| | `fire_clip`, `fire_scene`, `record_clip` landing | `lands_on_bar` read after the fire | the reply says which bar the launch made, from the transport, and says so when the bar line passed in flight; `no_later_than` schedules a cue for the next certain bar instead of gambling |
| | `cue`, `cancel_cue` | `schedule_cue`, `cancel_cue` | steps at Live bar numbers (`next_bar`, `next_phrase`, `{bar}`, `{bar, beat}`, `{bars_after}`, `{phrases_after}`): fire scene/clip, stop clip, stop all, set, ramp (tempo, crossfader, volume, send, device parameter), or a `gesture` (breakdown, drop, mute_except, sweep, build, panic, restore_mix) expanded on the server into those primitives. Resolved to beats and checked (past, unknown names, silence) before the script stores it; the script runs it on its own tick, so it happens even if the server is gone. `tests/performance.rs` |
| | `fire_scene`, `create_scene`, `set_scene` | same names | scene by name or index; `phrase_bars` per scene (default 16) is what phrase cue times count in, from the bar the scene was fired on (UI launches are noticed by the tick) |
| | `listen` | `get_track_meters`×N, or `start_live_capture` + `capture_status` | meters over N bars from the next bar line (never touches the transport); `capture: true` records through the Capture track while the set plays and adds RMS and low/mid/high balance |
| | `vary_clip`, `undo_vary`, `follow_key` | `get_clip_notes`, `get_clip_info`, `add_notes_to_clip`, `set_scale` | seeded note variations (`src/variation.rs`) in place or into a slot, one undo; the key of a recording and re-keying of the other clips (`follow_key: true` on start_performance automates it) |
| | `snapshot_mix`, `restore_mix`, `panic` | `snapshot_mix`, `restore_mix`, `schedule_cue` | one-round-trip mixer snapshots (restorable on the bar as a cue action); fade everything but `keep` to silence over a bar |
| | `record_clip` | `record_clip` | arms the track, fixed-length Session recording on the next bar, loops when done; the script names the clip and disarms. **Live 11+** |
| | `set_launch_quantization`, `set_crossfader` | same names | the transport-bar quantization; crossfader position and A/B assignment |
| | `keep_track_playing` | `set_slot_stop_buttons` | removes the stop buttons from a track's empty slots so scene launches leave it playing (Live's own mechanism); the cue silence check reads `no_stop_slots` |
| | `end_performance` | `cancel_cue` + `schedule_cue` / `stop_all_clips` + `stop_playback` + `stop_arrangement_record` | on the bar, faded over N bars (master volume restored after the stop), or now; lifts the guards; stops the Arrangement take, reports the bars it covers and the tracks it touched, and calls Back to Arrangement |
| | `record` on `start_performance` / `play_song` | `arrangement_summary` + `start_arrangement_record` | keeps the performance as a take in the Arrangement. Default `ask`: records from bar 1 when the Arrangement is empty, and when it is not, starts nothing and returns what is there with `after` (the next bar line past the last clip), `replace` (delete it and record from bar 1, naming Cmd+Z since there is no undo) and `off`. The answer is remembered for the session, except `replace`. **Live 11+** — Live 10 cannot report Arrangement clips, so nothing is recorded and the reply says why. `tests/performance.rs` |
| Sections | `make_section` | `capture_scene` (from what plays: Live's capture-and-insert-scene, which launches the copy seamlessly and carries the phrase count over), or `duplicate_scene` + `get_clip_notes`/`add_notes_to_clip`/`delete_clip` per changed track, or `create_scene` + `create_clip`×tracks | a section is a scene row named `<name> · <bars>`; the script rebuilds its phrase table from the names on every state read, so phrase lengths survive a Live restart; a duplicate name is refused, `replace: true` rewrites a `clips` section in place. **Live 11+** for the capture |
| | `set_song`, `add_to_song`, `remove_from_song` | `set_scene` / `create_scene` on the `Setlist:` scene | the song is the setlist in the name of one empty scene at the bottom (`Setlist: Intro×2 → Groove → …`); an entry without a count loops until `go`; every section is validated and a section that would leave the set silent is refused; a running song is re-planned |
| | `play_song` | `start_performance`'s commands (when none runs) or `fire_scene`, then one `schedule_cue` | fires the first entry now and cues every counted jump at `started + Σ repeats × phrase_bars`, waiting at the first uncounted entry |
| | `go`, `hold_section`, `next_section`, `previous_section`, `back`, `jump_to` | `get_performance_state` + `schedule_cue` (`replaces` the old plan cue in the same call), or `cancel_cue` for hold | one state read and one cue each (`tests/song.rs` asserts the command lists); a move lands at the end of the *playing* section's phrase (the next multiple of its phrase length from the bar it started on) or on the next bar, and then the reply says "cutting N bars off …"; `back` is the jump history, `previous_section` the setlist; a jump into a section whose stored master peak is more than 3 dB above the current one warns (`force: true` skips) |
| | `jump_to` `transition` | the same cue, plus `get_clip_notes`/`create_clip`/`add_notes_to_clip` for retime and fill, `get_context` once for a crossfade's faders | `tempo` (a ramp under the outgoing phrase), `retime` (half/double-time copies written into the target row before the jump), `crossfade` (volume ramps out and in), `fill` (a one-bar fill clip before the boundary), `drop` (everything but `keep` out for the last bars), `sweep`; composed on the server (`src/transition.rs`), nothing rebuilt |
| | *the level line* | `levels` on the `clock` envelope and the state | the script's tick keeps the master and per-track meter peaks per bar and the master's peak per scene row; `run_blocking` renders `🔊 master −3.8 dB peak this bar · Kick −6 · Bass −7` under the ⏱ line (Live's 0–1 meters as dB) |
| | `get_context`, `get_performance_state` | same | sections listed from the scene names, the song from the `Setlist:` scene (an unreadable entry is named), the song position (`Song: Intro×2 → [Groove: looping] → …`) and "next jump" for the plan cue — readable after a Live restart with no server file |
| Feel | `groove_clip`, `groove_amount` | `get_grooves`, `set_clip_groove` | a Groove Pool groove on a Session clip (`Clip.groove`, **Live 11+**), by name or index, with the groove's timing/random/velocity amounts, or the set's global amount; the API cannot add a groove to the pool and the reply says what to drag |
| | `humanize`, `swing_notes`, `retime_clip` | `get_clip_notes`, `add_notes_to_clip` | seeded note rewrites through the vary path with `undo_vary`: timing drift in ms at the current tempo (never across a bar line) and velocity variation, off-beat notes most; off-beat 16ths or 8ths delayed by a fraction of the step; half/double time. Replies describe the change in note values, never note counts |
| Sound | `shape_sound` | `get_track_info` + `get_device_parameters` + `set_device_parameters` | cutoff, resonance, attack, decay, sustain, release, drive, detune, width, lfo_rate, reverb, delay as a fraction of the range or `"±N%"`; resolved against a rack's macros by name first, then a table per Live instrument, then parameter names (`src/sound.rs`); several words in one round trip; the reply shows before/after in Live's display units and the words the device answers to; an unknown device lists its parameters |
| | `set_device_parameter` by name, `cue` `ramp {"sound": …}` | `get_device_parameters` + `set_device_parameter` / `schedule_cue` | `parameter: "<name substring>"`; a ramp step resolves a word to the device parameter it means |
| Set memory | `export_set`, `import_set` | `get_session_snapshot` + `get_context`; `build_song`'s commands + `set_scale` + `set_song`'s | a rebuildable document (tracks with devices by name and instruments by URI, clips with notes, mixer and sends, sections, setlist, tempo, signature, key) under `state_dir()/sets/<name>.json`, written only on request; import refuses an occupied set unless `merge: true`, audio clips are named, not rebuilt |
| Orchestration | `batch` | any | ordered steps, stop at first failure, `$last_track` and `$last_clip` |
| | `build_song` | `set_scale`, `set_tempo`, `create_scene`/`set_scene`, `create_tracks` (every track in one round trip: create, name, instrument by URI, fader in dB, pan, colour, sends), `write_clips` (every clip in one round trip; copies in other rows made inside Live from the first, so the notes travel once), `place_clips`, `create_locator` | one document → key, tempo, sections, tracks, instruments (plain words resolved through the index, or a URI), clips (any compact note form, `slots` for copies), placements, locators; validated before the first command; `dry_run`. A 7-track, 22-clip set builds in a few seconds instead of forty |
| Arrangement | `switch_to_arrangement_view` | `switch_to_arrangement_view` | |
| | `set_arrangement_time` | `set_current_song_time` | |
| | `get_arrangement_clips` | `get_arrangement_clips` | **Live 11+** |
| | `duplicate_to_arrangement` *advanced* | `place_clips` (every copy in one round trip) | **Live 11+**; `at_bar` / `until_bar` / `every_bars`, or a beat, a list, or `start`/`end`/`step`; the script reports the copies Live refused |
| | `set_arrangement_clip_name`, `create_locator` | same names | |
| | `delete_arrangement_clip`, `delete_locator` *advanced* | `delete_arrangement_clip`, `delete_arrangement_clips` (several or all in one round trip), `delete_locator` | **Live 11+**; one, several or `all`; the undo the Arrangement lacked |
| | `delete_track`, `back_to_arrangement`, `set_arrangement_loop` | same names | orphan tracks, the Back to Arrangement button, the loop brace for auditioning a section |
| | `play_and_measure`, `capture_mix` positioning | `play_from` | plays from the asked position (`continue_playing`); `start_playing` jumps to the start marker |
| | `add_notes_to_clip` `propagate_to_arrangement` | `get_clip_info` + `get_arrangement_clips` + delete/duplicate | refreshes Arrangement copies of an edited Session clip |
| | `set_arrangement_time` | `set_current_song_time` | reports the requested position and where the playhead was |
| Bridge | `get_remote_script_info` | `get_script_info` | loaded vs expected version, capabilities |

Count the tools before quoting a total; the snapshot in
[source-of-truth](../facts/source-of-truth.md) is checked by CI.

## Behaviour every tool shares

| Capability | Notes |
|---|---|
| Instructions at `initialize` | The server's `ServerConfig.instructions` (`src/context.rs`) teach the model the workflow: get_context → build_song → capture_mix → start_performance / cue. Pinned by `tests/stdio_integration.rs` |
| Version and capability handshake | Every tool checks the loaded Remote Script advertises the command it needs; a missing or outdated script gets one clear "reinstall, restart Live" error |
| Retry on first use | Starting the server before Live is fine; the handshake is retried when a tool runs |
| Failures are results, not protocol errors | `CallToolResult::error` with the message; the client keeps working |
| Per-command timeouts | 10 s reads, 15 s modifying commands, 65 s audio import, 5 s connect — `src/connection.rs` |
| Diagnostics | stderr only; `RUST_LOG=debug` for the command trace. stdout is pure JSON-RPC |
| Activity line | one local JSON line per call: tool, commands, timings, sizes, result — `src/activity.rs` |
| Heartbeat | one `<pid>.json` per running server naming the client that started it |
| `--check` / `--status` | handshake plus one read as JSON with an exit code; versions and paths |

## Clients and transports

| | Status |
|---|---|
| Claude Desktop, Claude Code, Cursor | Supported — any stdio MCP client |
| Transport | **stdio only**. No HTTP, no SSE, no listening port |
| Docker | The default install path: `docker compose build`, `docker run … -i` from the client config, `.mcp.json` for Claude Code |
| Native binary | `cargo install --path . --locked`; `ABLETON_HOST` / `ABLETON_PORT` |
| Directory listings | `smithery.yaml` names the command inside the image |

## The Live side

| | Status |
|---|---|
| Installer | Copies the embedded script into Live's User Library, `.bak` of anything replaced, `--list-targets` to preview |
| Live versions | 11 and 12; 10 without the arrangement tools; no automated matrix |
| Port | 9877, bound to `0.0.0.0` — [decision 0003](../decisions/0003-remote-script-bind-address.md) |
| Authentication on the socket | None |

## Local data

| What | Default | Switch | Where |
|---|---|---|---|
| Activity log: tool, commands, timings, sizes, result | **On** | `ABLETON_MCP_ACTIVITY=false` | `~/.ableton-music-maker/activity/` |
| Payloads: parameters and results (MIDI, names) | **Off** | `ABLETON_MCP_ACTIVITY_PAYLOADS=true` | same files |
| Heartbeat: pid, client, versions | On | removed on exit | `~/.ableton-music-maker/sessions/` |
| Library index: browser names, paths, URIs | On | `ABLETON_MCP_LIBRARY_INDEX=false` | `~/.ableton-music-maker/library/` |
| Sample index: audio file names, folders, paths, lengths (WAV/AIFF headers, never the audio) | Built on the first sample search, not at start-up | `ABLETON_MCP_LIBRARY_INDEX=false` | `~/.ableton-music-maker/library/samples-*.json` |
| Sample folders you added | **Only on an `adv_sample_folders add` call** | delete the file | `~/.ableton-music-maker/sample_folders.json` |
| Set exports: your tracks, notes, names, mixer, sections, setlist | **Only on an `export_set` call** | delete the file or the folder | `~/.ableton-music-maker/sets/` |
| Uploads | **None** — no code path; CI fails on an HTTP client in the dependency tree | | |

## The Mac app

| | Status |
|---|---|
| Menu bar app, Tauri 2, in `app/` | **Runs from source** (`cd app && npm run dev`); linked against this crate |
| Setup: install the Remote Script, check Live, connect Claude Desktop / Claude Code / Cursor, test | Built |
| Delete all local data: activity, sessions, the library index and the set exports | Built |
| Activity: per-session table, detail, estimated tokens, filters, retention | Built |
| Listen: spectrum, master meters, six ranges from a process tap of Live (macOS 14.4+; the app itself still runs on 12) | Built — `cargo test` in `app/src-tauri` (19 unit, 5 integration of which 2 need Live), `node --test app/src/listen.test.mjs` (16), `cargo run --example listen_probe` against Live |
| Float window (always on top) and the full-screen visual (WebGL, eight presets) | Built |
| The visual never repeats and never cuts: each visit re-rolls the preset into a variant (palette, warp mode, shape, fold, swirl, ripple, chroma), wanders at random, and cross-dissolves — both warp modes mixed per pixel, both folds crossfaded, the shapes dissolving on one feedback buffer | Built — `node --test app/src/visual.test.mjs` (18), including a simulated twenty minutes in which no field moves more than 6% of its own range in a frame |
| Signed, notarised download | Not yet |

## The image contract

Checked by `docker/verify-image.sh`, run by hand since CI builds only the Mac app
([decision 0009](../decisions/0009-ci-builds-the-mac-app-only.md)): distroless with no shell · non-root ·
`--status` reports no uploads and `/state` · the binary carries no upload tier ·
`initialize` handshake works over stdio with only JSON-RPC on stdout · size under the limit ·
installer binary present. Not published: the image is built where it is used.
