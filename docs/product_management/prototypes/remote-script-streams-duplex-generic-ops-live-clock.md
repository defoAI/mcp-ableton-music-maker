# Prototype transcript — the Remote Script streams: one duplex socket, generic ops, the clock pushed to Rust

Fake data, real message names. Written 2026-09-20, not yet reviewed.

The interface here is not a conversation but a socket, so the transcript is the wire: each
message the server sends, each message the script answers or pushes, in order, including the
failures. Times are the ones measured against Live 12 Suite on macOS 26.6 on 2026-09-20
(`scripts/live-latency.sh`, see the story); the ones marked *target* are what the story commits
to and are verified by the same script after the change.

Framing stays newline-delimited JSON on TCP 9877, loopback. Three kinds of message share one
socket: a **request** (`id`, `type`, `params`), its **reply** (`id`, `status`, `result` or
`error`), and an **event** (`event`, `t`, payload) the script pushes without being asked. A
request without an `id` is answered in order, as today, so an old server keeps working.

---

## 0. The handshake tells the truth about timing

**Server →**
```json
{"id": 1, "type": "get_script_info"}
```
**Script →**
```json
{"id": 1, "status": "success", "result": {
  "script_version": "2.0.0", "protocol_version": 2,
  "capabilities": ["…", "describe", "run", "subscribe", "unsubscribe"],
  "bind_host": "127.0.0.1", "bind_is_loopback": true,
  "tick": {"period_ms": 16.7, "samples": 600, "jitter_ms": 2.1, "playing": true},
  "socket_reader": "main_thread_tick",
  "live": {"version": "12.4.6", "python": "3.11.6"}
}}
```
The `tick` block is the number the whole design rests on and it is reported, not assumed.
Before this story it does not exist; measuring it is phase 0 and lands on its own.

## 1. A round trip no longer costs a scheduling quantum

**Server →** `{"id": 2, "type": "get_session_info"}`
**Script →** `{"id": 2, "status": "success", "result": {"tempo": 126.0, "is_playing": true, "track_count": 12, "main_ms": 0.9}}`

Measured today, background-thread reader: 200 ms for a request that never touches Live's API,
400 ms for one that does. *Target*, main-thread-tick reader: under 2 tick periods for either.

## 2. Requests overlap, and each reply finds its request

**Server →** `{"id": 3, "type": "get_track_info", "params": {"track_index": 0}}`
**Server →** `{"id": 4, "type": "get_track_info", "params": {"track_index": 1}}`
**Script →** `{"id": 4, "status": "success", "result": {"name": "Bass", "…": "…"}}`
**Script →** `{"id": 3, "status": "success", "result": {"name": "Kick", "…": "…"}}`

Replies may arrive out of order; the `id` is the only thing that pairs them. The executor still
runs the handlers one slice at a time on the main thread; concurrency is on the wire, not in Live.

## 3. Subscribe, and the clock comes to you

**Server →**
```json
{"id": 5, "type": "subscribe", "params": {"channels": ["clock", "levels", "changes"], "clock_every_ms": 50}}
```
**Script →** `{"id": 5, "status": "success", "result": {"subscribed": ["clock", "levels", "changes"], "clock_every_ms": 50}}`

Then, unasked, every 50 ms while the transport runs (every tick that crosses a bar when it is
stopped nothing is sent):
```json
{"event": "clock", "t": 12.345, "bar": 33, "beat": 1, "beat_time": 128.0, "tempo": 126.0,
 "playing": true, "scene": 3, "phrase_bar": 1, "next_bar_in_s": 1.9}
```
Once a bar, the peaks the tick already keeps:
```json
{"event": "levels", "t": 12.902, "bar": 33, "master": 0.71, "tracks": [0.62, 0.58, 0.0, 0.31]}
```
And when something the server cares about changes — diffed on the main thread, never a
listener on a clip:
```json
{"event": "changes", "t": 13.410, "changed": [
  {"path": "song.tracks[3].name", "from": "Audio", "to": "Vinyl break"},
  {"path": "song.scenes[3].is_triggered", "from": false, "to": true}
]}
```

**Server →** `{"id": 6, "type": "unsubscribe", "params": {"channels": ["levels"]}}`
**Script →** `{"id": 6, "status": "success", "result": {"subscribed": ["clock", "changes"]}}`

The server's readouts (`⏱`, `🔊`) come from this stream instead of a read per reply; the Mac app
subscribes on its own connection for the Listen screen's bar and the visual's beat.

## 4. Describe: what this Live actually has

**Server →** `{"id": 7, "type": "describe", "params": {"path": "song.tracks[0].devices[0]"}}`
**Script →**
```json
{"id": 7, "status": "success", "result": {
  "class": "Device", "name": "Analog",
  "attrs": {"name": {"type": "str", "readonly": false}, "parameters": {"type": "list[DeviceParameter]", "readonly": true},
            "is_active": {"type": "bool", "readonly": false}},
  "methods": ["store_chosen_bank"],
  "live_version": "12.4.6"
}}
```
Rust asks this once per class per Live version at handshake time and caches it under the
state dir, so the 89 `hasattr`/`except AttributeError` branches in the script today become one
table in Rust that the test suite can see.

**Server →** `{"id": 8, "type": "describe", "params": {"path": "os.environ"}}`
**Script →** `{"id": 8, "status": "error", "error": "describe: path must start with song, application or browser"}`

## 5. Run: a batch of ops, one round trip

**Server →**
```json
{"id": 9, "type": "run", "params": {"ops": [
  {"op": "get",  "path": "song.tempo",                      "as": "tempo"},
  {"op": "set",  "path": "song.tracks[3].mixer_device.volume.value", "value": 0.72},
  {"op": "call", "path": "song.tracks[3].clip_slots[2].fire", "args": []},
  {"op": "wait_tick", "ticks": 2},
  {"op": "get",  "path": "song.tracks[3].clip_slots[2].is_triggered", "as": "queued"}
]}}
```
**Script →**
```json
{"id": 9, "status": "success", "result": {"tempo": 126.0, "queued": true, "ops": 5, "slices": 2, "main_ms": 3.4}}
```
One round trip, run under the same slicer as every native command, one undo step per slice.
`wait_tick` is the two-tick dance `_create_locator` does by hand today, made a verb.

**Server →** `{"id": 10, "type": "run", "params": {"ops": [{"op": "call", "path": "song.tracks[0].__class__.__mro__", "args": []}]}}`
**Script →** `{"id": 10, "status": "error", "error": "run: op 0: dunder attributes are not reachable"}`

**Server →** `{"id": 11, "type": "run", "params": {"ops": [{"op": "set", "path": "song.tracks[0].playing_slot_index", "value": 1}]}}`
**Script →** `{"id": 11, "status": "error", "error": "run: op 0: song.tracks[0].playing_slot_index is read-only on Live 12.4.6"}`

The reply names the op, so Rust can say which step of a plan failed and what was already done.

## 6. A cue is ops with a beat

**Server →**
```json
{"id": 12, "type": "schedule_cue", "params": {"cue": {"label": "into the break",
  "steps": [{"at_beat": 160.0, "ops": [
     {"op": "call", "path": "song.scenes[4].fire", "args": []},
     {"op": "set",  "path": "song.tracks[1].mixer_device.volume.value", "value": 0.0}]}]}}}
```
**Script →** `{"id": 12, "status": "success", "result": {"cue_id": 7, "steps": 1, "first_at_beat": 160.0}}`

Then, when the tick crosses beat 160:
```json
{"event": "cue", "t": 27.004, "cue_id": 7, "step": 0, "at_beat": 160.0, "fired_at_beat": 160.02, "late_ms": 9.5}
```
The trigger stays on Live's tick; Rust planned it and learns exactly how late it landed.
*Target*: `late_ms` under one tick period at 126 BPM, measured over 50 cues.

## 7. An old server on a new script, and the reverse

An old server sends `{"type": "get_session_info"}` with no `id`. The script answers in order,
without an `id`, and pushes no events to a socket that never subscribed. Nothing changes for it.

A new server on an old script: the handshake lacks `run`, `describe` and `subscribe` in
`capabilities`, so `require()` refuses those and every native command keeps working; the
readouts fall back to a read per reply, as today. The error a tool returns is the existing one:
run the installer, restart Live.

## 8. The measurement that decides it

`scripts/live-latency.sh` prints, against a running Live:

```
tick period            16.7 ms  (600 samples, jitter 2.1 ms)       ← phase 0
round trip, no API    200.1 ms  → target ≤ 2 ticks                 ← phase 1
round trip, with API  400.3 ms  → target ≤ 2 ticks
clock events / s        20.0    (asked for 50 ms)                   ← phase 2
cue lateness p50/p95   9.5 / 15.8 ms over 50 cues, 126 BPM          ← phase 4
run: 413 ops in one batch   38 ms main thread, 1 round trip         ← phase 3
```

If the tick period comes back near 200 ms, phase 1 buys nothing and the story stops there,
with the generic ops and the streams still worth having at that latency. That is a fine outcome
and the story says so.
