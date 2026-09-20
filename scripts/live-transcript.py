#!/usr/bin/env python3
"""Record a fixed script of commands against a Live, and keep the replies.

Run it against a **real** Ableton Live to make the fixture:

    scripts/live-transcript.sh                      # localhost:9877, a real Live
    scripts/live-transcript.sh --out tests/fixtures/live-transcript-12.4.6.json

Run it against the fake to make the file the differential compares:

    scripts/live-transcript.py --port $PORT --out /tmp/fake.json

The same script of commands either way — that is the point. A test replays
the checked-in real-Live transcript against the fake and diffs the replies
field by field, with an explicit allow-list for what legitimately varies. A
difference outside that list means the fake answers differently from Live,
which is the one thing a fake must never do quietly.

**It builds.** The script creates tracks, clips, notes, placements, a
locator and a section, and it does not clean up — point it at a scratch set,
not at work you care about. Every step is named so the transcript reads as a
story rather than a list of opcodes.
"""
from __future__ import print_function

import argparse
import json
import os
import socket
import sys
import time

# What legitimately varies between two Lives, or between two runs of one.
# Anything else differing is a real difference. Paths are dotted, with `*`
# for any list index or any key.
ALLOWED_TO_VARY = [
    "main_ms",                 # what the call cost Live's main thread
    "slices",                  # how the executor split it
    "id",                      # the request id
    "*.seconds",               # how long a walk took
    "*.time",                  # wall-clock stamps
    "*.t",
    "result.tick.*",           # the measured tick
    "result.live.python",      # Live's bundled interpreter
    "result.port",
    "result.bind_host",
    "result.bind_is_loopback",
    "result.script_version",
    "result.snapshot_schema",
    "result.indexed_items",    # how much of the browser had been walked
    "result.index_complete",
    "result.truncated_walk",
    "result.items.*.uri",      # a library's own uris
    "result.items.*.path",
    "result.results.*.uri",
    "result.color",            # Live picks a track colour
    "result.color_index",
    "*.color",
    "*.color_index",
    "result.song_length",      # read-only, and Live keeps the high-water mark

    # ── The library is the machine's, not the model's ──────────────────────
    # The fake's browser is a miniature: a handful of instruments, effects
    # and kits, enough to load one of each. A real Live has whatever that
    # producer installed — 23 instruments, 47 audio effects, seven Packs,
    # 240 drum kits on the machine this was recorded against. Comparing the
    # *contents* would be comparing two people's hard disks. What is
    # compared is the shape of the reply and the behaviour around it: that a
    # search returns hits, that a load puts a device on the track, that a
    # tree comes back with categories at all.
    "result.categories.*",         # get_browser_tree: what is installed
    "result.available_categories", # dir(browser); see the bug filed on this
    "result.available_categories.*",
    "result.results.*",            # search_browser: what matched
    "result.parameters.*",         # get_device_parameters: a real Analog has ~100
    "result.device.parameters.*",
    "result.devices.*.parameters.*",
    "result.tracks.*.devices.*",   # get_session_snapshot: what is loaded
    "result.items.*",              # search_browser: what matched
    "result.returned",
    "result.total_matches",
    "result.grooves.*",            # the Groove Pool is the producer's
    "result.pool_functions.*",
    "result.groove_amount",
    # describe's attrs and methods are compared properly — as sets, with
    # readonly and type — by scripts/live-lom-sweep.py. Comparing them here
    # would be positional, where one extra member shifts every index.
    "result.attrs.*",
    "result.methods.*",
    # Live's own counters and choices, which say nothing about behaviour.
    "*.color_index",               # Live picks a colour per track and clip
    "*.clip.color",
    # The fader bisection: Live's taper is a table with float error, the
    # model's is the analytic curve, so -6 dB lands on 0.6999869 there and
    # 0.70 here. Both read back as -6 dB to a decimal.
    "result.volume_db",
    "*.volume_db",
    "result.clips.*.color",        # Live picks a colour per track and clip
    "result.tracks.*.color",
    "result.tracks.*.arrangement_clips.*.color",
    "result.tracks.*.clips.*.color",
    "result.session.current_song_time",   # the transport moved during the run
    "result.session.song_length",
    "result.bar",
    "result.beat",
    "*.bar",                       # the transport moved during the run
    "*.beat",
    "result.device.*",             # which device the fake's browser loaded
    "result.name",
    "result.display",
    "result.old_display",
    "result.value",
    "result.old_value",
    "result.value_string",
    "result.is_quantized",
    "result.landed",
    "result.max",
    "result.items.*",
    "result.instruments.*",        # get_library_status: what Live ships with
    "result.audio_effects.*",
    "result.midi_effects.*",
    "result.packs.*",
    "result.drums_folders.*",
    "result.sounds_folders.*",
    "result.child_count",
    "*.child_count",
    "*.is_folder",
    "*.is_device",
    "*.is_loadable",
    "*.file_path",             # absolute paths
    "*.path",
]


class Client(object):
    def __init__(self, host, port, timeout=90.0):
        self.sock = socket.create_connection((host, port), timeout=10)
        self.sock.settimeout(timeout)
        self.buf = b""
        self.next_id = 1

    def ask(self, command, params=None):
        rid = self.next_id
        self.next_id += 1
        doc = {"id": rid, "type": command, "params": params or {}}
        self.sock.sendall((json.dumps(doc) + "\n").encode("utf-8"))
        while True:
            nl = self.buf.find(b"\n")
            if nl >= 0:
                line, self.buf = self.buf[:nl], self.buf[nl + 1:]
                if not line.strip():
                    continue
                reply = json.loads(line.decode("utf-8"))
                if reply.get("id") == rid:
                    return reply
                continue  # an event, or a reply to something else
            data = self.sock.recv(65536)
            if not data:
                raise AssertionError("the socket closed waiting for %s" % command)
            self.buf += data

    def close(self):
        self.sock.close()


def the_script(names):
    """The fixed script, as (label, command, params).

    Reads first, so a Live that refuses to build still produces a useful
    transcript; then the build, in the order a session does it.
    """
    kick, bass, vox = names
    return [
        ("the handshake", "get_script_info", {}),
        # Both sides start from the same set, or the rest is a diff of
        # leftovers. reset_set empties the open set back to a new one; it is
        # the reason a transcript can be compared at all.
        ("back to a new set", "reset_set", {}),
        ("the set as found", "get_session_info", {}),
        ("the returns", "get_returns", {}),
        ("the meter curve", "get_meter_scale", {}),
        ("the grooves", "get_grooves", {}),
        ("the arrangement, empty", "arrangement_summary", {}),
        ("the performance state", "get_performance_state", {}),
        ("what the browser has", "get_browser_tree", {"category_type": "all"}),
        ("a browser search", "search_browser", {"query": "analog", "limit": 5}),

        ("the tempo", "set_tempo", {"tempo": 124.0}),
        ("the key", "set_scale", {"root_note": 2, "scale_name": "Minor"}),
        ("three tracks, two with instruments", "create_tracks", {"tracks": [
            {"name": kick, "kind": "midi", "instrument_uri": "query:Drums#Drum%20Rack"},
            {"name": bass, "kind": "midi", "instrument_uri": "query:Synths#Analog"},
            {"name": vox, "kind": "audio"},
        ], "on_existing": "converge"}),
        ("what the tracks look like now", "get_session_info", {}),

        ("two clips, with notes", "write_clips", {"clips": [
            {"track_index": "$kick", "clip_index": 0, "length": 4.0, "name": "Kick 1",
             "notes": [{"pitch": 36, "start_time": b, "duration": 0.25, "velocity": 100}
                       for b in (0.0, 1.0, 2.0, 3.0)]},
            {"track_index": "$bass", "clip_index": 0, "length": 4.0, "name": "Bass 1",
             "notes": [{"pitch": 41, "start_time": 0.0, "duration": 4.0, "velocity": 90}]},
        ]}),
        ("the notes, read back", "get_clip_notes", {"track_index": "$kick", "clip_index": 0}),
        ("the clip, read back", "get_clip_info", {"track_index": "$kick", "clip_index": 0}),

        ("the fader, in Live's own units", "set_track_mixer",
         {"track_index": "$bass", "volume_db": -6.0}),
        ("a send", "set_send", {"track_index": "$bass", "send_index": 0, "value": 0.25}),
        ("the track, read back", "get_track_info", {"track_index": "$bass"}),

        ("a device parameter", "set_device_parameter",
         {"track_index": "$bass", "device_index": 0, "parameter_index": 1, "value": 0.25}),
        ("the device, read back", "get_device_parameters",
         {"track_index": "$bass", "device_index": 0}),

        ("four bars of kick in the Arrangement", "place_clips",
         {"track_index": "$kick", "clip_index": 0, "times": [0.0, 4.0, 8.0, 12.0]}),
        ("two bars of bass", "place_clips",
         {"track_index": "$bass", "clip_index": 0, "times": [8.0, 12.0]}),
        ("the arrangement, read back", "get_arrangement_clips", {"track_index": "$kick"}),
        ("the arrangement summary", "arrangement_summary", {}),

        ("a section", "create_scene", {"index": -1, "name": "Drop", "phrase_bars": 8}),
        ("the scenes, read back", "get_performance_state", {}),
        ("the whole set", "get_session_snapshot", {"include_notes": True, "include_params": False}),
        ("the context the artist sees", "get_context", {}),

        ("what Live cannot do: an index that is not there", "get_track_info",
         {"track_index": 999}),
        ("what Live cannot do: a clip that is not there", "get_clip_notes",
         {"track_index": "$kick", "clip_index": 7}),
        ("describe, protocol 2", "describe", {"path": "song.tracks[0]"}),
        # Not `song.tracks.count`: `tracks` is a tuple in Live and in the
        # model, so `.count` is the tuple method, and `get` hands back its
        # repr — which carries a memory address and so can never match
        # between two runs. A path that names a value, not a method.
        ("run, protocol 2", "run", {"ops": [
            {"op": "get", "path": "song.tempo", "as": "tempo"},
            {"op": "get", "path": "song.tracks[0].name", "as": "first_track"},
            {"op": "get", "path": "song.tracks[0].mixer_device.volume.value", "as": "fader"},
        ]}),
    ]


def resolve(params, indexes):
    """`$kick` and `$bass` become the indexes the tracks really got."""
    if isinstance(params, dict):
        return dict((k, resolve(v, indexes)) for k, v in params.items())
    if isinstance(params, list):
        return [resolve(v, indexes) for v in params]
    if isinstance(params, str) and params.startswith("$"):
        return indexes[params[1:]]
    return params


def record(host, port, prefix):
    client = Client(host, port)
    names = ["%s Kick" % prefix, "%s Bass" % prefix, "%s Vox" % prefix]
    indexes = {}
    steps = []
    try:
        for label, command, params in the_script(names):
            sent = resolve(params, indexes)
            started = time.time()
            reply = client.ask(command, sent)
            steps.append({
                "label": label,
                "command": command,
                "params": sent,
                "reply": reply,
                "round_trip_ms": round((time.time() - started) * 1000.0, 2),
            })
            if command == "create_tracks":
                created = (reply.get("result") or {}).get("created") or []
                for entry, key in zip(created, ("kick", "bass", "vox")):
                    indexes[key] = entry.get("index")
        handshake = steps[0]["reply"].get("result") or {}
        return {
            "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "live_version": ".".join(
                str(x) for x in (handshake.get("live") or {}).get("version", "unknown").split(".")),
            "script_version": handshake.get("script_version"),
            "protocol_version": handshake.get("protocol_version"),
            "socket_reader": handshake.get("socket_reader"),
            "track_prefix": prefix,
            "steps": steps,
        }
    finally:
        client.close()


# ── the differential ────────────────────────────────────────────────────────

def flatten(value, path="", out=None):
    out = {} if out is None else out
    if isinstance(value, dict):
        for k, v in value.items():
            flatten(v, "%s.%s" % (path, k) if path else k, out)
    elif isinstance(value, list):
        for i, v in enumerate(value):
            flatten(v, "%s.%d" % (path, i) if path else str(i), out)
    else:
        out[path] = value
    return out


def matches(pattern, path):
    p, q = pattern.split("."), path.split(".")
    if "*" not in pattern:
        return p == q or (len(q) > len(p) and q[:len(p)] == p)
    # A pattern that opens with `*.` names a member at any depth:
    # `*.color_index` covers `result.tracks.3.color_index`. Without this it
    # would only ever match a path exactly two deep.
    if p[0] == "*" and len(p) > 1:
        tail = p[1:]
        if len(q) >= len(tail):
            for i, part in enumerate(tail):
                if part != "*" and q[len(q) - len(tail) + i] != part:
                    break
            else:
                return True
    if len(p) > len(q):
        return False
    for i, part in enumerate(p):
        if part == "*":
            continue
        if q[i] != part:
            return False
    return True


def allowed(path):
    # A list index anywhere becomes `*` for matching, so `result.items.3.uri`
    # is covered by `result.items.*.uri`.
    generic = ".".join("*" if part.isdigit() else part for part in path.split("."))
    return any(matches(pattern, path) or matches(pattern, generic)
               for pattern in ALLOWED_TO_VARY)


def diff(real, fake):
    """Field-by-field differences between two transcripts, outside the
    allow-list. Returns a list of readable lines."""
    out = []
    real_steps, fake_steps = real["steps"], fake["steps"]
    if len(real_steps) != len(fake_steps):
        out.append("different number of steps: %d against %d"
                   % (len(real_steps), len(fake_steps)))
    for r, f in zip(real_steps, fake_steps):
        if r["command"] != f["command"]:
            out.append("step order differs: %s against %s" % (r["command"], f["command"]))
            continue
        if r["reply"].get("status") != f["reply"].get("status"):
            out.append("%s (%s): Live said %s, the fake said %s — %s / %s" % (
                r["command"], r["label"], r["reply"].get("status"), f["reply"].get("status"),
                r["reply"].get("message", ""), f["reply"].get("message", "")))
            continue
        rf, ff = flatten(r["reply"]), flatten(f["reply"])
        for path in sorted(set(rf) | set(ff)):
            if allowed(path):
                continue
            a, b = rf.get(path, "<missing>"), ff.get(path, "<missing>")
            if a == b:
                continue
            if isinstance(a, float) and isinstance(b, float) and abs(a - b) < 1e-6:
                continue
            out.append("%s (%s) %s: Live %r, fake %r" % (r["command"], r["label"], path, a, b))
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", default=os.environ.get("ABLETON_HOST", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=int(os.environ.get("ABLETON_PORT", "9877")))
    parser.add_argument("--out", help="where to write the transcript (default: stdout)")
    parser.add_argument("--prefix", default="T", help="prefix for the track names it creates")
    parser.add_argument("--diff", nargs=2, metavar=("REAL", "FAKE"),
                        help="diff two transcripts instead of recording one")
    args = parser.parse_args(argv)

    if args.diff:
        with open(args.diff[0]) as handle:
            real = json.load(handle)
        with open(args.diff[1]) as handle:
            fake = json.load(handle)
        lines = diff(real, fake)
        for line in lines:
            print(line)
        print("%d difference(s) outside the allow-list" % len(lines))
        return 1 if lines else 0

    transcript = record(args.host, args.port, args.prefix)
    text = json.dumps(transcript, indent=2, sort_keys=True) + "\n"
    if args.out:
        with open(args.out, "w") as handle:
            handle.write(text)
        sys.stderr.write("wrote %s: %d steps, Live %s, script %s\n" % (
            args.out, len(transcript["steps"]), transcript["live_version"],
            transcript["script_version"]))
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
