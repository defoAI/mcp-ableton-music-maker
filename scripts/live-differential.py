#!/usr/bin/env python3
"""Run every call against a real Live and against the fake, and log what differs.

    scripts/live-differential.py                    # a real Live on localhost:9877
    scripts/live-differential.py --port 9877 --out target/live-differential
    scripts/live-differential.py --record-fixture   # also keep the real transcript

The same fixed script of commands (`scripts/live-transcript.py`) goes to both
sides, step by step: a real Ableton Live on `--host`/`--port`, and a fresh
`scripts/fake-live.py` started here. Every reply is compared field by field
with the allow-list for what legitimately varies (`main_ms`, ids, timings,
absolute paths, Live's choice of colour).

**It logs everything and keeps going.** A difference does not stop the run —
the point is one pass that finds all of them, written down so they can be
worked through afterwards. Three files land in `--out`:

    report.md       every difference, grouped by command, newest run wins
    report.json     both transcripts and the diff, for a tool to read
    real.json / fake.json   the two transcripts as recorded

Exit code is 0 when the two agree, 1 when they do not, 2 when no Live
answered — so CI can tell "they differ" from "there was nothing to compare".

**It builds in the set it is pointed at**: tracks, clips, notes, placements,
a locator and a scene, and it does not clean up. Point it at a scratch set.
"""
from __future__ import print_function

import argparse
import importlib.util
import json
import os
import socket
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FAKE_LIVE = os.path.join(ROOT, "scripts", "fake-live.py")


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, os.path.join(ROOT, "scripts", filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


transcript = load("live_transcript", "live-transcript.py")


def live_is_available(host, port, timeout=2.0):
    """True when something answers on the socket and speaks the handshake."""
    try:
        sock = socket.create_connection((host, port), timeout=timeout)
    except (OSError, socket.error):
        return False
    try:
        sock.settimeout(timeout + 8.0)
        sock.sendall((json.dumps({"type": "get_script_info", "params": {}, "id": 1}) + "\n").encode())
        buf = b""
        while b"\n" not in buf:
            chunk = sock.recv(65536)
            if not chunk:
                return False
            buf += chunk
        reply = json.loads(buf.split(b"\n")[0].decode("utf-8"))
        return reply.get("status") == "success"
    except Exception:
        return False
    finally:
        sock.close()


class FakeLive(object):
    """The fake in its own process; the port comes off stdout."""

    def __init__(self, extra=()):
        self.proc = subprocess.Popen(
            [sys.executable, "-u", FAKE_LIVE, "--quiet", "--exit-with-pid", str(os.getpid())]
            + list(extra),
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        line = self.proc.stdout.readline().decode("utf-8").strip()
        if not line.isdigit():
            err = self.proc.stderr.read().decode("utf-8")[:2000]
            raise RuntimeError("the fake did not start: %r\n%s" % (line, err))
        self.port = int(line)

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=10)

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.stop()


# How a difference is classified, so a long report is worked through by
# kind rather than line by line. First match wins.
CATEGORIES = [
    ("baseline",  "the two sets did not start alike — reset Live, or the fake's default_set is wrong",
     lambda l: l.split(" ")[0] in ("get_session_info", "get_returns", "get_performance_state",
                                   "arrangement_summary", "get_context", "get_session_snapshot")
               and ("track_count" in l or "song_length" in l or "current_song_time" in l
                    or "result.tempo" in l or "loop_start" in l)),
    ("lom-surface", "describe's view of the Live Object Model: attributes and methods Live has",
     lambda l: ".methods." in l or ".attrs." in l or "_has_listener" in l or "_listener" in l),
    ("status", "Live succeeded where the fake failed, or the other way round",
     lambda l: "Live said" in l),
    ("missing-field", "the fake left out a field Live returns",
     lambda l: "fake '<missing>'" in l),
    ("extra-field", "the fake returned a field Live does not",
     lambda l: "Live '<missing>'" in l),
    ("value", "both answered, with different values", lambda l: True),
]


def categorise(line):
    for name, _, test in CATEGORIES:
        try:
            if test(line):
                return name
        except Exception:
            continue
    return "value"


def group(lines):
    """Differences by command, in the order the script runs them."""
    out, order = {}, []
    for line in lines:
        command = line.split(" ", 1)[0].rstrip(":")
        if command not in out:
            out[command] = []
            order.append(command)
        out[command].append(line)
    return [(c, out[c]) for c in order]


def report_markdown(real, fake, lines, elapsed):
    when = time.strftime("%Y-%m-%d %H:%M:%SZ", time.gmtime())
    head = [
        "# Live differential — the fake against a real Live",
        "",
        "| | |",
        "|---|---|",
        "| Run | %s |" % when,
        "| Real Live | %s, script %s, protocol %s |" % (
            real.get("live_version"), real.get("script_version"), real.get("protocol_version")),
        "| Fake | script %s, protocol %s |" % (
            fake.get("script_version"), fake.get("protocol_version")),
        "| Steps compared | %d |" % len(real.get("steps", [])),
        "| Differences | **%d** |" % len(lines),
        "| Took | %.1f s |" % elapsed,
        "",
        "Every difference below is a field where the fake answered something other than",
        "Live did, outside `ALLOWED_TO_VARY` in `scripts/live-transcript.py`. Each line is",
        "`command (step) path: Live <what Live said>, fake <what the fake said>`.",
        "",
    ]
    if not lines:
        head += ["## No differences", "",
                 "The fake answered like Live on every field of every step.", ""]
        return "\n".join(head)
    buckets = {}
    for line in lines:
        buckets.setdefault(categorise(line), []).append(line)
    head += ["## What differs, by kind", "",
             "| kind | count | what it means |", "|---|---:|---|"]
    for name, meaning, _ in CATEGORIES:
        if name in buckets:
            head.append("| `%s` | %d | %s |" % (name, len(buckets[name]), meaning))
    head.append("")
    if "baseline" in buckets:
        head += ["> **The baseline did not match.** The two sets did not start the same, so most",
                 "> of what follows is leftover state in Live rather than the fake answering",
                 "> differently. Make a fresh set in Live (Cmd+N) and run again before reading",
                 "> the rest as drift.", ""]
    head += ["## Differences, by kind then command", ""]
    for name, meaning, _ in CATEGORIES:
        entries = buckets.get(name)
        if not entries:
            continue
        head.append("## `%s` — %d" % (name, len(entries)))
        head.append("")
        head.append("*%s*" % meaning)
        head.append("")
        for command, rows in group(entries):
            head.append("### `%s` — %d" % (command, len(rows)))
            head.append("")
            shown = rows[:40]
            for line in shown:
                head.append("- %s" % line)
            if len(rows) > len(shown):
                head.append("- … and %d more (all of them in `report.json`)" % (len(rows) - len(shown)))
            head.append("")
    # A per-step round-trip comparison: the timing side of "behaves the same".
    head += ["## Round trip, Live against the fake", "",
             "| step | command | Live ms | fake ms |", "|---|---|---:|---:|"]
    for r, f in zip(real.get("steps", []), fake.get("steps", [])):
        head.append("| %s | `%s` | %.0f | %.0f |" % (
            r["label"], r["command"], r.get("round_trip_ms", 0), f.get("round_trip_ms", 0)))
    head.append("")
    return "\n".join(head)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", default=os.environ.get("ABLETON_HOST", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=int(os.environ.get("ABLETON_PORT", "9877")))
    parser.add_argument("--out", default=os.path.join(ROOT, "target", "live-differential"),
                        help="where the report and the transcripts land")
    parser.add_argument("--prefix", default="Diff", help="prefix for the tracks it creates")
    parser.add_argument("--record-fixture", action="store_true",
                        help="also write the real transcript to tests/fixtures/")
    parser.add_argument("--require-live", action="store_true",
                        help="exit 2 when no Live answers (the default is to say so and exit 2)")
    args = parser.parse_args(argv)

    if not live_is_available(args.host, args.port):
        sys.stderr.write(
            "no Live answered on %s:%d — nothing to compare.\n"
            "Open Ableton Live with the AbletonMusicMaker control surface selected, "
            "or point --host/--port at one.\n" % (args.host, args.port))
        return 2

    if not os.path.isdir(args.out):
        os.makedirs(args.out)

    started = time.time()
    sys.stderr.write("recording against the real Live on %s:%d ...\n" % (args.host, args.port))
    real = transcript.record(args.host, args.port, args.prefix)

    sys.stderr.write("starting the fake and recording the same script ...\n")
    with FakeLive() as fake_proc:
        fake = transcript.record("127.0.0.1", fake_proc.port, args.prefix)
    elapsed = time.time() - started

    lines = transcript.diff(real, fake)

    for name, payload in (("real.json", real), ("fake.json", fake)):
        with open(os.path.join(args.out, name), "w") as handle:
            handle.write(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    with open(os.path.join(args.out, "report.json"), "w") as handle:
        handle.write(json.dumps({
            "recorded_at": real.get("recorded_at"),
            "live_version": real.get("live_version"),
            "script_version": real.get("script_version"),
            "steps": len(real.get("steps", [])),
            "difference_count": len(lines),
            "differences": lines,
            "by_category": dict(
                (name, [l for l in lines if categorise(l) == name])
                for name in set(categorise(l) for l in lines)),
        }, indent=2, sort_keys=True) + "\n")
    text = report_markdown(real, fake, lines, elapsed)
    report = os.path.join(args.out, "report.md")
    with open(report, "w") as handle:
        handle.write(text + "\n")

    if args.record_fixture:
        fixtures = os.path.join(ROOT, "tests", "fixtures")
        if not os.path.isdir(fixtures):
            os.makedirs(fixtures)
        target = os.path.join(fixtures, "live-transcript-%s.json" % real.get("live_version"))
        with open(target, "w") as handle:
            handle.write(json.dumps(real, indent=2, sort_keys=True) + "\n")
        sys.stderr.write("wrote the fixture %s\n" % target)

    counts = {}
    for line in lines:
        counts[categorise(line)] = counts.get(categorise(line), 0) + 1
    for name, _, _ in CATEGORIES:
        if name in counts:
            print("%-14s %d" % (name, counts[name]))
    print("")
    print("%d difference(s) outside the allow-list over %d steps; report: %s"
          % (len(lines), len(real.get("steps", [])), report))
    return 1 if lines else 0


if __name__ == "__main__":
    sys.exit(main())
