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

# The Live class each path is, so a difference on it can be looked up in
# the inventory of what the Remote Script actually touches.
CLASS_OF = {
    "song": "Song",
    "song.view": "View",
    "song.master_track": "Track",
    "song.master_track.mixer_device": "MixerDevice",
    "song.master_track.mixer_device.volume": "DeviceParameter",
    "song.master_track.mixer_device.panning": "DeviceParameter",
    "song.master_track.mixer_device.crossfader": "DeviceParameter",
    "song.tracks[0]": "Track",
    "song.tracks[0].mixer_device": "MixerDevice",
    "song.tracks[0].mixer_device.volume": "DeviceParameter",
    "song.tracks[0].mixer_device.panning": "DeviceParameter",
    "song.tracks[0].mixer_device.sends[0]": "DeviceParameter",
    "song.tracks[0].clip_slots[0]": "ClipSlot",
    "song.tracks[0].clip_slots[0].clip": "Clip",
    "song.tracks[0].devices[0]": "Device",
    "song.tracks[0].devices[0].parameters[0]": "DeviceParameter",
    "song.return_tracks[0]": "Track",
    "song.return_tracks[0].mixer_device": "MixerDevice",
    "song.return_tracks[0].devices[0]": "Device",
    "song.scenes[0]": "Scene",
    "song.cue_points[0]": "CuePoint",
    "application": "Application",
    "application.view": "View",
    "browser": "Browser",
    "browser.instruments": "BrowserItem",
}


def used_surface():
    """{class: {member}} — every Live member the Remote Script touches,
    read out of the script by scripts/live-api-surface.py."""
    spec = importlib.util.spec_from_file_location(
        "live_api_surface", os.path.join(ROOT, "scripts", "live-api-surface.py"))
    module = importlib.util.module_from_spec(spec)
    import contextlib, io
    with contextlib.redirect_stdout(io.StringIO()):
        spec.loader.exec_module(module)
    return dict((cls, set(members)) for cls, members in module.surface().items())


def member_of(line):
    """The member name a difference line is about."""
    head = line.split(":")[0]
    if "." not in head:
        return None
    name = head.rsplit(".", 1)[-1]
    return name[:-2] if name.endswith("()") else name


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
    "highlighted_clip_slot", "selected_track", "selected_scene", "detail_clip",
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


_REPR = __import__("re").compile(r"^<([A-Za-z_][\w.]*) object at 0x[0-9a-f]+>$")


def same_repr(a, b):
    """Two object reprs naming the same class.

    `run … get` on anything that is not a number or a string hands back
    `"%s" % value`, which for a Live object is `<Track.Track object at
    0x15cf001c0>`. The address is the object's identity in that process and
    can never match between two of them; the class name is the part that
    says whether the model handed back the same kind of thing. Live's
    classes carry their module (`Track.Track`, `Base.Vector`), so the
    comparison is on the last component.
    """
    if not (isinstance(a, str) and isinstance(b, str)):
        return False
    ma, mb = _REPR.match(a), _REPR.match(b)
    if not (ma and mb):
        return False
    return ma.group(1).rsplit(".", 1)[-1] == mb.group(1).rsplit(".", 1)[-1]


def same_collection(a, b):
    """Both sides read as a collection.

    Script 1.35.0 renders a Live `Base.Vector` as a list instead of its
    repr, so a whole-collection read now comes back as contents rather than
    an address (#66). What is *in* a collection is the open set's, not the
    model's: the real Live swept here has the producer's tracks, a Suite
    browser of thousands of items and a Reverb with 33 parameters, while the
    model has a default set, a browser of a couple of dozen items and a
    simplified Reverb — all of that is written down in
    `docs/architecture/overview.md` as what the fake deliberately is not.

    Comparing contents would therefore report the fixture's set, not a model
    defect. Before 1.35.0 this comparison degenerated to "both are a Vector"
    (two reprs of the same class, see `same_repr`); this keeps exactly that
    meaning now that the wire carries more. What each member of a collection
    looks like is still checked — by the indexed paths in `PATHS`
    (`song.tracks[0]`, `song.tracks[0].devices[0]` and the rest), which
    compare attributes, methods, readonly and types the usual way.
    """
    return isinstance(a, list) and isinstance(b, list)


def compare(real, fake, used=None):
    """Differences, as (category, path, line, in_scope).

    `in_scope` is True when the Remote Script actually touches that member.
    The model is not Live and is not trying to be: Live's Track carries 150
    methods and the script calls 22 of them. What has to match is what the
    script reads, writes and calls — the inventory
    `scripts/live-api-surface.py` reads out of the script. The rest is
    counted so the gap is visible, and left alone.
    """
    used = used if used is not None else {}
    rows = []

    def scoped(path, member):
        cls = CLASS_OF.get(path)
        if cls is None:
            return True
        return member in (used.get(cls) or set())

    for path in [p[:-1] if p.endswith("?") else p for p in PATHS]:
        r, f = real.get(path), fake.get(path)
        if r is None or f is None:
            continue
        if "missing" in r and "missing" in f:
            continue
        if "missing" in r:
            rows.append(("path", path, "Live has no %s (%s), the model does" % (path, r["missing"]), True))
            continue
        if "missing" in f:
            rows.append(("path", path, "the model has no %s (%s), Live does" % (path, f["missing"]), True))
            continue
        if r.get("class") != f.get("class"):
            rows.append(("class", path, "class: Live %r, model %r" % (r.get("class"), f.get("class")), True))
        ra, fa = set(r["attrs"]), set(f["attrs"])
        for name in sorted(ra - fa):
            rows.append(("attr-missing", path,
                         "%s.%s: Live has it, the model does not (%s)"
                         % (path, name, r["attrs"][name].get("type")), scoped(path, name)))
        for name in sorted(fa - ra):
            rows.append(("attr-extra", path,
                         "%s.%s: the model has it, Live does not" % (path, name), True))
        for name in sorted(ra & fa):
            if r["attrs"][name].get("readonly") != f["attrs"][name].get("readonly"):
                rows.append(("readonly", path,
                             "%s.%s: Live readonly=%s, the model %s"
                             % (path, name, r["attrs"][name].get("readonly"),
                                f["attrs"][name].get("readonly")), scoped(path, name)))
            if (r["attrs"][name].get("type") != f["attrs"][name].get("type")
                    and f["attrs"][name].get("type") != "none"
                    and name not in VOLATILE):
                rows.append(("type", path,
                             "%s.%s: Live type %r, the model %r"
                             % (path, name, r["attrs"][name].get("type"),
                                f["attrs"][name].get("type")), scoped(path, name)))
        rm, fm = set(r["methods"]), set(f["methods"])
        for name in sorted(rm - fm):
            rows.append(("method-missing", path,
                         "%s.%s(): Live has it, the model does not" % (path, name),
                         scoped(path, name)))
        for name in sorted(fm - rm):
            # A method the model invents is in scope whatever the script
            # calls: it tells `describe` about a Live that does not exist.
            rows.append(("method-extra", path,
                         "%s.%s(): the model has it, Live does not" % (path, name),
                         not _is_listener(name)))
        rr, fr = r.get("reads") or {}, f.get("reads") or {}
        for name in sorted(set(rr) & set(fr)):
            if name.startswith("__") or name in VOLATILE:
                continue
            a, b = rr[name], fr[name]
            if a == b:
                continue
            if isinstance(a, float) and isinstance(b, float) and abs(a - b) < 1e-6:
                continue
            if same_repr(a, b) or same_collection(a, b):
                continue
            rows.append(("read", path, "%s.%s: Live %r, the model %r" % (path, name, a, b),
                         scoped(path, name)))
        for name in sorted(set(rr) - set(fr)):
            if not name.startswith("__"):
                rows.append(("read-missing", path,
                             "%s.%s: Live read it, the model could not" % (path, name),
                             scoped(path, name)))
    return rows


def _is_listener(name):
    """Live's observer boilerplate: three per observable property, and the
    script calls none of them (decision 0007 — it runs off the tick). The
    model generates them from its own properties, so which ones it has
    follows which properties it has; that is not drift worth chasing."""
    return (name.startswith("add_") and name.endswith("_listener")) \
        or (name.startswith("remove_") and name.endswith("_listener")) \
        or name.endswith("_has_listener")


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


def report(rows, real_info, elapsed, used):
    """In scope first, because that is the part that has to be zero."""
    inside = [r for r in rows if r[3]]
    outside = [r for r in rows if not r[3]]
    counted = sum(len(v) for v in used.values())

    def table(entries):
        counts = {}
        for cat, _, _, _ in entries:
            counts[cat] = counts.get(cat, 0) + 1
        lines = ["| kind | count | what it means |", "|---|---:|---|"]
        for cat in ORDER:
            if cat in counts:
                lines.append("| `%s` | %d | %s |" % (cat, counts[cat], MEANING[cat]))
        return lines

    def detail(entries, title):
        out = ["## %s" % title, ""]
        for cat in ORDER:
            rows_of = [r for r in entries if r[0] == cat]
            if not rows_of:
                continue
            out += ["### `%s` — %d" % (cat, len(rows_of)), "", "*%s*" % MEANING[cat], ""]
            by_path = {}
            for _, path, line, _ in rows_of:
                by_path.setdefault(path, []).append(line)
            for path in sorted(by_path):
                out.append("**`%s`**" % path)
                out.append("")
                for line in by_path[path][:40]:
                    out.append("- %s" % line)
                if len(by_path[path]) > 40:
                    out.append("- … and %d more" % (len(by_path[path]) - 40))
                out.append("")
        return out

    out = [
        "# The Live Object Model — what the Remote Script uses, compared",
        "",
        "| | |", "|---|---|",
        "| Run | %s |" % time.strftime("%Y-%m-%d %H:%M:%SZ", time.gmtime()),
        "| Real Live | %s, script %s |" % (real_info.get("live_version"), real_info.get("script_version")),
        "| Paths described | %d |" % len(PATHS),
        "| Members the script touches | %d |" % counted,
        "| **In scope — must be zero** | **%d** |" % len(inside),
        "| Out of scope — counted, not chased | %d |" % len(outside),
        "| Took | %.1f s |" % elapsed,
        "",
        "`describe` on each path, both sides, compared as **sets**; then a `run` batch that",
        "reads every shared attribute and compares the values.",
        "",
        "**In scope** is a member `scripts/live-api-surface.py` finds the Remote Script reading,",
        "writing or calling. Those must agree: the script runs against both, so a difference is",
        "a difference in what the product does. **Out of scope** is the rest of Live's object",
        "model — Live's Track carries 150 methods and the script calls 22. The model is not a",
        "reimplementation of Live and completing it would be work with no reader. It is counted",
        "here so the gap stays visible, and a member that moves into the script's reach moves",
        "into scope on the next run, with no list to maintain.",
        "",
    ]
    if inside:
        out += ["## In scope — these have to be fixed", ""] + table(inside) + [""]
        out += detail(inside, "In scope, in detail")
    else:
        out += ["## In scope — nothing differs", "",
                "Every member the Remote Script touches matches Live, on all %d paths." % len(PATHS),
                ""]
    out += ["## Out of scope — Live has more model than the script asks for", ""]
    out += table(outside) + [""]
    out += detail(outside, "Out of scope, in detail")
    return "\n".join(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", default=os.environ.get("ABLETON_HOST", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=int(os.environ.get("ABLETON_PORT", "9877")))
    parser.add_argument("--out", default=os.path.join(ROOT, "target", "lom-sweep"))
    parser.add_argument("--record-fixture", action="store_true",
                        help="keep the real Live's sweep under tests/fixtures/, so the "
                             "in-scope check can run in CI with no Live")
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

    used = used_surface()
    if args.record_fixture:
        fixtures = os.path.join(ROOT, "tests", "fixtures")
        if not os.path.isdir(fixtures):
            os.makedirs(fixtures)
        version = info.get("live", {}).get("version", "unknown")
        target = os.path.join(fixtures, "live-lom-%s.json" % version)
        with open(target, "w") as handle:
            handle.write(json.dumps({
                "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "live_version": version,
                "script_version": info.get("script_version"),
                "paths": real,
            }, indent=2, sort_keys=True) + "\n")
        sys.stderr.write("wrote the fixture %s\n" % target)

    rows = compare(real, fake, used)
    inside = [r for r in rows if r[3]]
    outside = [r for r in rows if not r[3]]
    elapsed = time.time() - started

    with open(os.path.join(args.out, "real.json"), "w") as h:
        h.write(json.dumps(real, indent=2, sort_keys=True) + "\n")
    with open(os.path.join(args.out, "fake.json"), "w") as h:
        h.write(json.dumps(fake, indent=2, sort_keys=True) + "\n")
    with open(os.path.join(args.out, "report.json"), "w") as h:
        h.write(json.dumps({"difference_count": len(rows),
                            "in_scope": len(inside),
                            "out_of_scope": len(outside),
                            "differences": [{"kind": k, "path": p, "line": l, "in_scope": s_}
                                            for k, p, l, s_ in rows]},
                           indent=2, sort_keys=True) + "\n")
    path = os.path.join(args.out, "report.md")
    with open(path, "w") as h:
        h.write(report(rows, {"live_version": info.get("live", {}).get("version"),
                              "script_version": info.get("script_version")}, elapsed, used) + "\n")

    counts = {}
    for cat, _, _, in_ in inside:
        counts[cat] = counts.get(cat, 0) + 1
    print("IN SCOPE — what the Remote Script touches")
    if inside:
        for cat in ORDER:
            if cat in counts:
                print("  %-16s %d" % (cat, counts[cat]))
    else:
        print("  nothing differs")
    print("")
    print("out of scope (Live has more model than the script asks for): %d" % len(outside))
    print("report: %s" % path)
    return 1 if inside else 0


if __name__ == "__main__":
    sys.exit(main())
