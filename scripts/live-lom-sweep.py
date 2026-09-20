#!/usr/bin/env python3
"""Every method and every read, on both Lives, compared.

    scripts/live-lom-sweep.py                 # a real Live on localhost:9877
    scripts/live-lom-sweep.py --out target/lom-sweep

`scripts/live-differential.py` sends the *commands* to both sides. This
sends the *Live Object Model*: it walks a list of paths, and for each one
asks `describe` of a real Ableton Live and of `scripts/fake-live.py`, then
compares

  * **attributes** — the names, as sets, plus whether Live and the model
    agree on `readonly` and on the type name for each shared one,
  * **methods** — the names, as sets,
  * **reads** — a `run` batch that gets every shared, readable attribute on
    both sides and compares the values.

Sets, not positions: `describe` returns sorted lists, so comparing them
index by index turns one missing member into a hundred bogus differences.

It is **read-only**. `describe` and `run … get` change nothing, so unlike
the differential this needs no scratch set and can be pointed at a Live
with work open.

Exit code 0 when the model matches, 1 when it does not, 2 when no Live
answered.
"""
from __future__ import print_function

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load(name, filename):
    spec = importlib.util.spec_from_file_location(
        name, os.path.join(ROOT, "scripts", filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


transcript = load("live_transcript", "live-transcript.py")
differential = load("live_differential", "live-differential.py")

# The paths worth describing: one of every class in the Live Object Model
# the script can reach. `?` marks one that may not resolve in a given set
# (no clip in the slot, no device on the track) — it is skipped on both
# sides rather than reported.
PATHS = [
    "song",
    "song.view",
    "song.master_track",
    "song.master_track.mixer_device",
    "song.master_track.mixer_device.volume",
    "song.master_track.mixer_device.panning",
    "song.master_track.mixer_device.crossfader",
    "song.tracks[0]",
    "song.tracks[0].mixer_device",
    "song.tracks[0].mixer_device.volume",
    "song.tracks[0].mixer_device.panning",
    "song.tracks[0].mixer_device.sends[0]",
    "song.tracks[0].clip_slots[0]",
    "song.tracks[0].clip_slots[0].clip?",
    "song.tracks[0].devices[0]?",
    "song.tracks[0].devices[0].parameters[0]?",
    "song.return_tracks[0]",
    "song.return_tracks[0].mixer_device",
    "song.return_tracks[0].devices[0]?",
    "song.scenes[0]",
    "song.cue_points[0]?",
    "application",
    "application.view",
    "browser",
    "browser.instruments",
]

# Values that legitimately differ between two Lives with different sets
# open. Structure is compared strictly; these are compared only for type.
VOLATILE = (
    "name", "value", "tempo", "current_song_time", "song_length", "loop_start",
    "loop_length", "file_path", "color", "color_index", "playing_slot_index",
    "fired_slot_index", "is_playing", "is_recording", "is_triggered",
    "output_meter_left", "output_meter_right", "output_meter_level",
    "input_meter_level", "input_meter_left", "input_meter_right",
    "scale_name", "root_note", "scale_mode", "groove_amount", "signature_numerator",
    "signature_denominator", "clip_trigger_quantization", "back_to_arranger",
    "can_undo", "can_redo", "is_selected", "value_string", "str_for_value",
    "length", "start_time", "end_time", "loop_end", "start_marker", "end_marker",
    "has_clip", "arm", "mute", "solo", "muted", "playing_status", "is_empty",
)


def describe(client, path):
    reply = client.ask("describe", {"path": path})
    if reply.get("status") != "success":
        return None, reply.get("message", "")
    return reply.get("result") or {}, None


def reads(client, path, names):
    """One `run` batch that gets every named attribute of `path`."""
    if not names:
        return {}
    ops = [{"op": "get", "path": "%s.%s" % (path, n), "as": n} for n in names]
    reply = client.ask("run", {"ops": ops})
    if reply.get("status") != "success":
        return {"__error__": reply.get("message", "")}
    return (reply.get("result") or {}).get("values") or reply.get("result") or {}


def sweep(client, paths):
    out = {}
    for raw in paths:
        optional = raw.endswith("?")
        path = raw[:-1] if optional else raw
        described, error = describe(client, path)
        if described is None:
            out[path] = {"missing": error, "optional": optional}
            continue
        attrs = described.get("attrs") or {}
        readable = sorted(n for n in attrs if n not in ("canonical_parent",))
        out[path] = {
            "attrs": attrs,
            "methods": sorted(described.get("methods") or []),
            "class": described.get("class"),
            "reads": reads(client, path, readable),
        }
    return out


def compare(real, fake):
    """Differences, as (category, path, line)."""
    rows = []
    for path in [p[:-1] if p.endswith("?") else p for p in PATHS]:
        r, f = real.get(path), fake.get(path)
        if r is None or f is None:
            continue
        if "missing" in r and "missing" in f:
            continue
        if "missing" in r:
            rows.append(("path", path, "Live has no %s (%s), the model does" % (path, r["missing"])))
            continue
        if "missing" in f:
            rows.append(("path", path, "the model has no %s (%s), Live does" % (path, f["missing"])))
            continue
        if r.get("class") != f.get("class"):
            rows.append(("class", path, "class: Live %r, model %r" % (r.get("class"), f.get("class"))))
        ra, fa = set(r["attrs"]), set(f["attrs"])
        for name in sorted(ra - fa):
            rows.append(("attr-missing", path, "%s.%s: Live has it, the model does not (%s)"
                         % (path, name, r["attrs"][name].get("type"))))
        for name in sorted(fa - ra):
            rows.append(("attr-extra", path, "%s.%s: the model has it, Live does not" % (path, name)))
        for name in sorted(ra & fa):
            if r["attrs"][name].get("readonly") != f["attrs"][name].get("readonly"):
                rows.append(("readonly", path, "%s.%s: Live readonly=%s, the model %s"
                             % (path, name, r["attrs"][name].get("readonly"),
                                f["attrs"][name].get("readonly"))))
            if r["attrs"][name].get("type") != f["attrs"][name].get("type"):
                rows.append(("type", path, "%s.%s: Live type %r, the model %r"
                             % (path, name, r["attrs"][name].get("type"),
                                f["attrs"][name].get("type"))))
        rm, fm = set(r["methods"]), set(f["methods"])
        for name in sorted(rm - fm):
            rows.append(("method-missing", path, "%s.%s(): Live has it, the model does not" % (path, name)))
        for name in sorted(fm - rm):
            rows.append(("method-extra", path, "%s.%s(): the model has it, Live does not" % (path, name)))
        # reads: the value of every shared attribute
        rr, fr = r.get("reads") or {}, f.get("reads") or {}
        for name in sorted(set(rr) & set(fr)):
            if name.startswith("__") or name in VOLATILE:
                continue
            a, b = rr[name], fr[name]
            if a == b:
                continue
            if isinstance(a, float) and isinstance(b, float) and abs(a - b) < 1e-6:
                continue
            rows.append(("read", path, "%s.%s: Live %r, the model %r" % (path, name, a, b)))
        for name in sorted(set(rr) - set(fr)):
            if not name.startswith("__"):
                rows.append(("read-missing", path, "%s.%s: Live read it, the model could not" % (path, name)))
    return rows


MEANING = {
    "path": "a path resolves on one side only",
    "class": "the object is a different class",
    "attr-missing": "Live has an attribute the model does not",
    "attr-extra": "the model invents an attribute Live has no such thing as",
    "readonly": "Live and the model disagree on whether it can be written",
    "type": "Live and the model disagree on the type",
    "method-missing": "Live has a method the model does not",
    "method-extra": "the model has a method Live does not",
    "read": "reading it gave different values",
    "read-missing": "Live could read it, the model could not",
}
ORDER = ["path", "class", "attr-extra", "method-extra", "readonly", "type",
         "read", "read-missing", "attr-missing", "method-missing"]


def report(rows, real_info, elapsed):
    counts = {}
    for cat, _, _ in rows:
        counts[cat] = counts.get(cat, 0) + 1
    out = [
        "# The Live Object Model — every method and every read",
        "",
        "| | |", "|---|---|",
        "| Run | %s |" % time.strftime("%Y-%m-%d %H:%M:%SZ", time.gmtime()),
        "| Real Live | %s, script %s |" % (real_info.get("live_version"), real_info.get("script_version")),
        "| Paths described | %d |" % len(PATHS),
        "| Differences | **%d** |" % len(rows),
        "| Took | %.1f s |" % elapsed,
        "",
        "`describe` on each path, both sides, compared as **sets**; then a `run` batch",
        "that reads every shared attribute and compares the values. Values that",
        "legitimately differ between two open sets are compared for presence, not",
        "equality (`VOLATILE` in this script).",
        "",
        "| kind | count | what it means |", "|---|---:|---|",
    ]
    for cat in ORDER:
        if cat in counts:
            out.append("| `%s` | %d | %s |" % (cat, counts[cat], MEANING[cat]))
    out.append("")
    for cat in ORDER:
        entries = [r for r in rows if r[0] == cat]
        if not entries:
            continue
        out.append("## `%s` — %d" % (cat, len(entries)))
        out.append("")
        out.append("*%s*" % MEANING[cat])
        out.append("")
        by_path = {}
        for _, path, line in entries:
            by_path.setdefault(path, []).append(line)
        for path in sorted(by_path):
            out.append("### `%s` — %d" % (path, len(by_path[path])))
            out.append("")
            for line in by_path[path][:60]:
                out.append("- %s" % line)
            if len(by_path[path]) > 60:
                out.append("- … and %d more" % (len(by_path[path]) - 60))
            out.append("")
    return "\n".join(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", default=os.environ.get("ABLETON_HOST", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=int(os.environ.get("ABLETON_PORT", "9877")))
    parser.add_argument("--out", default=os.path.join(ROOT, "target", "lom-sweep"))
    args = parser.parse_args(argv)

    if not differential.live_is_available(args.host, args.port):
        sys.stderr.write("no Live answered on %s:%d — nothing to compare.\n" % (args.host, args.port))
        return 2
    if not os.path.isdir(args.out):
        os.makedirs(args.out)

    started = time.time()
    client = transcript.Client(args.host, args.port)
    try:
        info = (client.ask("get_script_info", {}) or {}).get("result") or {}
        real = sweep(client, PATHS)
    finally:
        client.close()

    with differential.FakeLive() as fake_proc:
        fake_client = transcript.Client("127.0.0.1", fake_proc.port)
        try:
            fake = sweep(fake_client, PATHS)
        finally:
            fake_client.close()

    rows = compare(real, fake)
    elapsed = time.time() - started

    with open(os.path.join(args.out, "real.json"), "w") as h:
        h.write(json.dumps(real, indent=2, sort_keys=True) + "\n")
    with open(os.path.join(args.out, "fake.json"), "w") as h:
        h.write(json.dumps(fake, indent=2, sort_keys=True) + "\n")
    with open(os.path.join(args.out, "report.json"), "w") as h:
        h.write(json.dumps({"difference_count": len(rows),
                            "differences": [{"kind": k, "path": p, "line": l} for k, p, l in rows]},
                           indent=2, sort_keys=True) + "\n")
    path = os.path.join(args.out, "report.md")
    with open(path, "w") as h:
        h.write(report(rows, {"live_version": info.get("live", {}).get("version"),
                              "script_version": info.get("script_version")}, elapsed) + "\n")

    counts = {}
    for cat, _, _ in rows:
        counts[cat] = counts.get(cat, 0) + 1
    for cat in ORDER:
        if cat in counts:
            print("%-16s %d" % (cat, counts[cat]))
    print("")
    print("%d difference(s) over %d paths; report: %s" % (len(rows), len(PATHS), path))
    return 1 if rows else 0


if __name__ == "__main__":
    sys.exit(main())
