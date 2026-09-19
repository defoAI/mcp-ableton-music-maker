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

| Area | Tool | Remote Script command(s) | Notes |
|---|---|---|---|
| Session | `get_context` | `get_context` | **start here**: one round trip for the set, every track (kind, arm/mute/solo, mixer, devices, clips by slot, play state, Arrangement count), returns, scenes, the performance clock, optional library summary; ends with the workflow reminder |
| | `get_session_info` | `get_session_info` | tempo, tracks, view |
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
| Browser | `search_browser` | the server's library index (`get_browser_index` pages, `src/library.rs`); `search_browser` while the walk is incomplete | words matched against name and folder path; `queries: [...]` many per call, `best: true` one URI each; answered with no round trip once the index is complete; the index is walked in the background after the handshake (one-second pages), kept on disk under the state dir, `refresh: true` re-walks |
| | `get_library_status` | `get_library_status` | Live version, instruments/effects present, Suite instruments missing, packs installed; undownloaded packs are invisible to the API and the result says so |
| | `get_browser_tree`, `get_browser_items_at_path` | same names | the tree now recurses two folder levels |
| | `load_instrument_or_effect` result | `load_browser_item` | reports the loaded device's name and index |
| Clips | `get_clip_info`, `set_clip_loop`, `set_clip_launch` | same names | loop points, markers, launch mode/quantization, legato |
| Automation | `set_clip_automation`, `get_clip_automation` | same names | device parameter or mixer target; points or a ramp; steps at `resolution`; Session and Arrangement clips |
| Listening | `get_track_meters` | `get_track_meters` | one reading of every output meter |
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
| | `end_performance` | `cancel_cue` + `schedule_cue` / `stop_all_clips` + `stop_playback` | on the bar, faded over N bars (master volume restored after the stop), or now; lifts the guards |
| Orchestration | `batch` | any | ordered steps, stop at first failure, `$last_track` and `$last_clip` |
| | `build_song` | many | one document → scenes (names, tempos, phrase lengths), tracks, instruments (URI, or plain words resolved through the index), clips (any compact note form, `slots` for copies in several rows), placements, locators; validated before the first command; `dry_run` |
| Arrangement | `switch_to_arrangement_view` | `switch_to_arrangement_view` | |
| | `set_arrangement_time` | `set_current_song_time` | |
| | `get_arrangement_clips` | `get_arrangement_clips` | **Live 11+** |
| | `duplicate_to_arrangement` | `duplicate_session_clip_to_arrangement` (once per placement) | **Live 11+**; one time, a list, or `start`/`end`/`step`; stops at the first failure and says how far it got |
| | `set_arrangement_clip_name`, `create_locator` | same names | |
| | `delete_arrangement_clip`, `delete_locator` | same names | **Live 11+**; one, several or `all`; the undo the Arrangement lacked |
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
| Uploads | **None** — no code path; CI fails on an HTTP client in the dependency tree | | |

## The Mac app

| | Status |
|---|---|
| Menu bar app, Tauri 2, in `app/` | **Runs from source** (`cd app && npm run dev`); linked against this crate |
| Setup: install the Remote Script, check Live, connect Claude Desktop / Claude Code / Cursor, test | Built |
| Activity: per-session table, detail, estimated tokens, filters, retention | Built |
| Signed, notarised download | Not yet |

## The image contract

Checked by `docker/verify-image.sh` on every CI build: distroless with no shell · non-root ·
`--status` reports no uploads and `/state` · the binary carries no upload tier ·
`initialize` handshake works over stdio with only JSON-RPC on stdout · size under the limit ·
installer binary present. Published to GHCR on `main` for amd64 and arm64.
