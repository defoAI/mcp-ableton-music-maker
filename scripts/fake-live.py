#!/usr/bin/env python3
"""A Live that is not Live, on a real socket, on Live's clock.

Runs the **real** Remote Script (`AbletonMusicMaker_Remote_Script/__init__.py`)
against the **real** model (`tests/remote_script/fake_live.py`), listening on
the script's **own** socket server. Nothing here reimplements the protocol:
the accept thread, the tick reader, the framing, the ids, the streams and the
cues are all the script's code, exactly as they run inside Ableton. What this
file supplies is the two things Live supplies — a Song to talk to, and a main
thread that calls back every 100 ms.

    scripts/fake-live.py                       # ephemeral port, printed on stdout
    scripts/fake-live.py --port 9878
    ABLETON_PORT=$(scripts/fake-live.py --print-port-only & …)

    cargo run --bin ableton-music-maker -- --check      # with ABLETON_PORT set

stdout carries the port and nothing else, so a parent process can read it;
every other line goes to stderr.

What it is not
--------------
No audio, no rendering, no real browser index, no Max for Live. Meters read
what nothing put there. `docs/architecture/overview.md` says the same at
length; `tests/remote_script/test_live_semantics.py` pins it as tests.

The tick
--------
Live's main-thread tick is **100 ms** — measured 2026-09-20 on Live 12.4.6
(macOS 26.6) through `ableton-music-maker --check`: `period_ms 100.0,
jitter_ms 0.24` over 211 samples; decision 0010 has the same period from a
600-sample run. A ticker thread calls the surface's due callbacks at that
rate, so the script's `_TickSampler` reports Live-like figures and the clock
channel's rate (#51) is testable with no Live open. `--tick-ms` overrides it
for a fast run, and the reply to `get_script_info` then says so.

What a call costs
-----------------
On by default: the measured table in `fake_live.LATENCY_12_4_6`, every row
stamped with the run it came from (#43's `main_ms` log, #45's device loads).
The executor's slice budget then spreads work over ticks as in Live, `main_ms`
is real, and a slice over 25 ms lands in the log as it would in `Log.txt`.
`--latency none` turns it off, `--latency worst` charges the worst case, and
`--latency-scale` shortens every charge by one factor — reported in
`--latency-report`, so a scaled run cannot be read as a real one. Calls nobody
has measured charge nothing and are listed by `--latency-report`.

Debugging, and using it under stress
------------------------------------
`--record FILE` writes one JSON line per request and per reply — the wire,
exactly as it went — which is what a failing end-to-end run is read from, and
what `scripts/live-transcript.sh` records against a real Live so the two can
be diffed. `--script-log FILE` collects the script's own `log_message` lines,
which is Live's `Log.txt`: the "slow slice" warnings land there.

A fake that always answers cannot reproduce the failures that matter, so it
can be told to fail on purpose:

    --die-after N        stop answering after N replies, socket closed
    --die-on COMMAND     go away the moment that command is asked for
    --slow CALL=MS       make one Live call cost MS (marked NOT measured)

`--die-on create_tracks` is #45 as it actually happened: the socket died
mid-command and the producer was left with half a set. `--slow
Song.create_midi_track=700` is the device load nobody has timed yet, asserted
rather than assumed — the reply and the report both say the row is invented.

On exit it prints a readout to stderr (commands served, worst `main_ms`,
slices, slow slices, what each call was charged), so a stress run can be put
beside `scripts/live-latency.sh` against a real Live.

Isolation
---------
`--set-per-connection` (the default) gives each connecting client its own
`default_set()`, so tests running in parallel inside one binary cannot collide.
**That is not Live.** Live has one set and one main thread, and the event
channels and scheduled cues run on that thread, not on a socket — so in this
mode they read the first connection's set, and a `subscribe` from any other
connection is refused rather than quietly answered about the wrong set.
`--shared-set` is one set for every client, which is what Live is, and is what
the end-to-end and latency runs use.
"""
from __future__ import print_function

import argparse
import io
import json
import os
import signal
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "tests", "remote_script"))

import fake_live  # noqa: E402
import harness  # noqa: E402


def log(message):
    sys.stderr.write("fake-live: %s\n" % message)
    sys.stderr.flush()


class Surface(harness.FakeSurface):
    """The harness surface with one addition: which set `song()` answers
    with. With `--shared-set` that is always the same one. With
    `--set-per-connection` it is the set of the client whose command is
    running — tracked through `_run_on_main` and carried across the ticks a
    sliced handler resumes on, so a generator sees the same set on its last
    slice as on its first."""

    def __init__(self, c_instance=None):
        harness.FakeSurface.__init__(self, c_instance)
        self.primary = self._song_obj
        self.per_connection = False
        self.current = None          # the client being served, or None
        self.sets = {}               # id(client) -> Song

    # ── which set ──
    def song(self):
        if not self.per_connection or self.current is None:
            return self.primary
        key = id(self.current)
        if key not in self.sets:
            self.sets[key] = self.new_set()
        return self.sets[key]

    def application(self):
        """Per connection too: `Application.browser.load_item` loads onto the
        SELECTED track of the set the Application belongs to, so an
        Application from another set would build in the wrong one."""
        return self.song()._application

    def new_set(self):
        song = fake_live.default_set()
        song._clock = self.primary._clock
        song._latency = self.primary._latency
        return song

    def forget(self, client):
        self.sets.pop(id(client), None)

    # ── carrying the client across ticks ──
    def schedule_message(self, ticks, callback):
        client = self.current
        if client is not None:
            inner = callback

            def carried():
                before = self.current
                self.current = client
                try:
                    inner()
                finally:
                    self.current = before
            callback = carried
        harness.FakeSurface.schedule_message(self, ticks, callback)


def install_isolation(script, surface, wire=None):
    """`_run_on_main` is the one place a command's client is known: the sink
    it is given carries it. Wrapping it there, rather than editing the
    script, keeps the script the thing under test."""
    original = script._run_on_main

    def run_on_main(command_type, params, sink=None):
        client = getattr(sink, "client", None)
        if client is not None and wire is not None:
            wire.expect(client, getattr(sink, "request_id", None), command_type)
        before = surface.current
        surface.current = client
        try:
            return original(command_type, params, sink)
        finally:
            surface.current = before

    script._run_on_main = run_on_main

    subscribe = script._subscribe

    def guarded_subscribe(client, params):
        # The tick belongs to the application, not to a socket: the clock,
        # levels and changes channels read the primary set. Answering a
        # second connection about a set it is not looking at would be a
        # wrong number that reads like a right one.
        if surface.per_connection and client is not first_client(script):
            raise ValueError(
                "this fake Live runs --set-per-connection: the event channels run on "
                "the tick and read the first connection's set, so subscribing from "
                "another connection would answer about a set you are not looking at. "
                "Start it with --shared-set, which is what Live is.")
        return subscribe(client, params)

    script._subscribe = guarded_subscribe


def first_client(script):
    return script._clients[0] if script._clients else None


class Wire(object):
    """Every request and every reply, and what they cost.

    The two hooks are the script's own: `_split_documents`, which is where a
    request becomes a document, and `_client_write`, which is where a reply
    becomes bytes. Neither is reimplemented — both are wrapped, so what is
    recorded is what went over the socket.
    """

    def __init__(self, path=None, die_after=None, die_on=None):
        self.path = path
        self.handle = io.open(path, "w", encoding="utf-8") if path else None
        self.die_after = die_after
        self.die_on = die_on
        self.dead = False
        self.requests = 0
        self.replies = 0
        self.by_command = {}          # command -> [count, worst_main_ms, slices, errors]
        # Which command a reply belongs to. `_ClientSink.put` has already
        # taken the request out of `c.inflight` by the time the reply is
        # written, so the pairing is kept here: authoritative from the
        # executor, and from the request itself for the two commands that
        # never reach it (`get_script_info`, `subscribe`).
        self.by_sink = {}             # (id(client), request_id) -> command
        self.by_id = {}               # request_id -> command
        self.started_at = time.time()
        self.lock = threading.Lock()

    def note_request(self, doc):
        command = doc.get("type", "?")
        with self.lock:
            self.requests += 1
            row = self.by_command.setdefault(command, [0, 0.0, 0, 0])
            row[0] += 1
            self.by_id[doc.get("id")] = command
            if self.die_on and command == self.die_on:
                self.dead = True
        self.write({"at": round(time.time() - self.started_at, 4), "dir": "in",
                    "id": doc.get("id"), "type": command, "params": doc.get("params")})

    def expect(self, client, request_id, command):
        self.by_sink[(id(client), request_id)] = command

    def command_for(self, client, request_id):
        return (self.by_sink.pop((id(client), request_id), None)
                or self.by_id.pop(request_id, None) or "?")

    def note_reply(self, command, payload):
        with self.lock:
            self.replies += 1
            row = self.by_command.setdefault(command or "?", [0, 0.0, 0, 0])
            row[1] = max(row[1], float(payload.get("main_ms") or 0.0))
            row[2] += int(payload.get("slices") or 0)
            if payload.get("status") == "error":
                row[3] += 1
            if self.die_after is not None and self.replies > self.die_after:
                self.dead = True
        self.write({"at": round(time.time() - self.started_at, 4), "dir": "out",
                    "id": payload.get("id"), "type": command,
                    "status": payload.get("status"),
                    "main_ms": payload.get("main_ms"), "slices": payload.get("slices"),
                    "result": payload.get("result"), "message": payload.get("message")})

    def write(self, row):
        if self.handle is None:
            return
        with self.lock:
            self.handle.write(json.dumps(row, sort_keys=True, default=str) + "\n")
            self.handle.flush()

    def close(self):
        if self.handle is not None:
            self.handle.close()

    def report(self, latency):
        rows = sorted(("%-32s %5d calls  worst main_ms %8.2f  slices %5d  errors %d"
                       % (c, v[0], v[1], v[2], v[3]))
                      for c, v in self.by_command.items())
        out = ["", "fake-live readout — %.1f s, %d requests, %d replies"
               % (time.time() - self.started_at, self.requests, self.replies)]
        out.extend("  " + r for r in rows)
        if latency is not None and latency.charged:
            out.append("  what Live was charged for:")
            for call in sorted(latency.charged):
                n, total = latency.charged[call]
                row = latency.table.get(call)
                mark = ""
                if row is not None and row.source.startswith("NOT MEASURED"):
                    mark = "   <- NOT MEASURED, given with --slow"
                out.append("    %-30s %5d x  %9.1f ms total%s" % (call, n, total, mark))
            if latency.scale != 1.0:
                out.append("  NOTE: every charge was scaled by %g. Not a real figure."
                           % latency.scale)
        return "\n".join(out)


def install_wire(ns, script, wire):
    """Record the wire and, when asked, break it. Both through the script's
    own seams."""
    split = ns["_split_documents"]

    def recording_split(buf):
        docs, rest = split(buf)
        for doc in docs:
            if isinstance(doc, dict):
                wire.note_request(doc)
        return docs, rest

    ns["_split_documents"] = recording_split

    write = script._client_write

    def recording_write(c, request_id, payload):
        wire.note_reply(wire.command_for(c, request_id), payload)
        if wire.dead:
            # Live went away: the socket closes with the reply unsent, which
            # is the "connection closed before any data arrived" #45 saw.
            log("dying on purpose: closing the socket with a reply unsent")
            try:
                script._client_close(c, "fake-live was told to die")
            except Exception:
                pass
            return
        return write(c, request_id, payload)

    script._client_write = recording_write


class Ticker(object):
    """Live's main thread: one callback round every `period_s`.

    Live re-arms `schedule_message(1, ...)` from inside the callback, so the
    rate is the rate the surface is driven at. Nothing else here is a
    thread: every command still runs on this one, as decision 0007 requires.
    """

    def __init__(self, surface, period_s):
        self.surface = surface
        self.period = period_s
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.run)
        self.thread.daemon = True

    def run(self):
        next_at = time.time()
        while not self.stop.is_set():
            next_at += self.period
            try:
                self.surface.tick()
            except Exception as e:      # a tick that raises must not end the clock
                log("tick error: %s" % e)
            wait = next_at - time.time()
            if wait > 0:
                self.stop.wait(wait)
            else:
                next_at = time.time()   # a slow slice: clamp forward, do not burst

    def start(self):
        self.thread.start()

    def shutdown(self):
        self.stop.set()
        self.thread.join(2.0)


def slow_rows(specs):
    """`--slow Call=MS`: a row that nobody measured, marked as such so it can
    never be mistaken for one that was."""
    rows = {}
    for spec in specs or []:
        if "=" not in spec:
            raise SystemExit("--slow takes CALL=MS, got %r" % spec)
        call, ms = spec.rsplit("=", 1)
        rows[call] = fake_live.Measured(
            float(ms), float(ms), "main_ms", "NOT MEASURED — given on the command line",
            "invented for this run with --slow")
    return rows


def build(args):
    latency = None
    if args.latency != "none":
        latency = fake_live.LatencyTable.measured(
            worst=(args.latency == "worst"), scale=args.latency_scale)
        latency.table.update(slow_rows(args.slow))
    elif args.slow:
        latency = fake_live.LatencyTable(slow_rows(args.slow), scale=args.latency_scale)
    song = fake_live.default_set()
    # `AbletonMCP` binds its base class when the script is exec'd, so the
    # surface has to be in place before `load` runs, not after.
    ns = harness.load(song=song, clock=fake_live.WallClock(), latency=latency,
                      surface_class=Surface)
    ns["HOST"] = "127.0.0.1"
    ns["DEFAULT_PORT"] = args.port
    ns["SOCKET_READER"] = "main_thread_tick"

    harness._NEXT_SONG[:] = [song]
    script = ns["AbletonMCP"](None)          # AbletonMCP IS the surface
    surface = script
    surface.primary = song
    surface.per_connection = not args.shared_set
    surface.current = None
    surface.sets = {}
    return ns, script, surface


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--port", type=int, default=0,
                        help="TCP port on 127.0.0.1; 0 (the default) is ephemeral. "
                             "Live owns 9877, so nothing here takes it by default.")
    parser.add_argument("--tick-ms", type=float, default=fake_live.TICK_PERIOD_MS,
                        help="the main-thread tick, in ms. The default, %.1f, is what "
                             "Live 12.4.6 was measured at." % fake_live.TICK_PERIOD_MS)
    parser.add_argument("--latency", choices=("measured", "worst", "none"), default="measured",
                        help="what a Live call costs: the measured table (default), its "
                             "worst cases, or nothing")
    parser.add_argument("--latency-scale", type=float, default=1.0,
                        help="shorten every charge by this factor for a fast run; "
                             "reported, so a scaled run cannot be read as a real one")
    parser.add_argument("--shared-set", action="store_true",
                        help="one set for every connection, which is what Live is. "
                             "The default gives each connection its own.")
    parser.add_argument("--latency-report", action="store_true",
                        help="print the latency table, including the calls nobody has "
                             "measured, and exit")
    parser.add_argument("--record", metavar="FILE",
                        help="one JSON line per request and per reply: the wire, for "
                             "reading a failure back or diffing against a real Live")
    parser.add_argument("--script-log", metavar="FILE",
                        help="the script's own log_message lines — Live's Log.txt, "
                             "where the slow-slice warnings go")
    parser.add_argument("--die-after", type=int, metavar="N",
                        help="stop answering after N replies, socket closed: Live "
                             "going away mid-session")
    parser.add_argument("--die-on", metavar="COMMAND",
                        help="go away the moment this command is asked for (#45 was "
                             "--die-on create_tracks)")
    parser.add_argument("--slow", action="append", metavar="CALL=MS",
                        help="make one Live call cost MS. Marked NOT MEASURED "
                             "everywhere it is reported.")
    parser.add_argument("--print-port-only", action="store_true",
                        help="print the port and nothing else on stdout (the default)")
    args = parser.parse_args(argv)

    if args.latency_report:
        table = fake_live.LatencyTable.measured()
        json.dump(table.report(), sys.stdout, indent=2, sort_keys=True)
        sys.stdout.write("\n")
        return 0

    ns, script, surface = build(args)
    wire = Wire(args.record, args.die_after, args.die_on)
    install_isolation(script, surface, wire)
    install_wire(ns, script, wire)
    if args.script_log:
        handle = io.open(args.script_log, "w", encoding="utf-8")

        def log_message(message, _handle=handle, _script=script):
            line = "%s %s" % (time.strftime("%H:%M:%S"), message)
            _handle.write(line + "\n")
            _handle.flush()
            _script.logged.append("%s" % message)
        script.log_message = log_message
    if not script.running:
        log("the script's server did not start: %s" % "; ".join(script.logged[-3:]))
        return 1
    port = script.server.getsockname()[1]

    ticker = Ticker(surface, args.tick_ms / 1000.0)
    ticker.start()

    sys.stdout.write("%d\n" % port)
    sys.stdout.flush()
    log("listening on 127.0.0.1:%d, script %s, tick %.1f ms, latency %s x%g, %s"
        % (port, ns["SCRIPT_VERSION"], args.tick_ms, args.latency, args.latency_scale,
           "one shared set" if args.shared_set else "one set per connection"))

    stop = threading.Event()

    def bye(signum, frame):
        stop.set()
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            signal.signal(sig, bye)
        except ValueError:
            pass

    try:
        while not stop.is_set():
            stop.wait(0.25)
            # A parent that went away takes the fake with it, so a killed
            # test run never leaves one of these listening.
            if os.getppid() == 1:
                log("parent gone; stopping")
                break
    finally:
        ticker.shutdown()
        try:
            script.disconnect()
        except Exception as e:
            log("disconnect: %s" % e)
        slow = [line for line in script.logged if "slow slice" in line]
        sys.stderr.write(wire.report(surface.primary._latency) + "\n")
        sys.stderr.write("  slow slices (over %s ms, as Live's Log.txt records them): %d\n"
                         % (script.SLOW_SLICE_MS, len(slow)))
        for line in slow[:10]:
            sys.stderr.write("    " + line + "\n")
        sys.stderr.flush()
        wire.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
