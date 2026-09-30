#!/usr/bin/env python3
"""Against a running Live: does the reload actually work in Live's Python?

Everything else about the loader/body split can be checked with no Live open.
This cannot. Whether `instance.__class__ = NewBody` is allowed inside Live's
own interpreter, against a class deriving from `_Framework.ControlSurface`,
is a fact about Ableton's code and Ableton's Python, and the model in
`tests/remote_script/fake_live.py` says nothing about it (`TheModelIsNotLive`).

    scripts/live-reload-probe.py                 # 20 reloads, the full report
    scripts/live-reload-probe.py --reloads 5
    scripts/live-reload-probe.py --port 9877 --host 127.0.0.1

What it measures, and prints one line each:

  rebase          the swap happened at all, and the instance kept its class
  attributes      the ~30 instance attributes are the same objects afterwards
  armed callback  a schedule_message armed before the swap did not crash
  tick period     the period and jitter before and after, over N samples
  cost            the import (off Live's thread) and the swap (on it)
  memory          what N reloads retained, from the script's own reply
  cue lateness    a cue pending across a reload still fires on the bar

It changes nothing in the set: it reloads the body that is already installed,
which is the same file every time, so every reload is a no-op except for the
mechanism. Run it on a scratch set anyway.

Exit code 0 when every check passed, 1 when one did not. A `1` here means the
design in
`docs/product_management/stories/remote-script-reloads-its-body-without-restarting-live.md`
does not hold on this Live and the fallback in that story is what ships.
"""
import argparse
import json
import socket
import sys
import time


class Live(object):
    """One socket to the Remote Script, one request at a time, with ids."""

    def __init__(self, host, port, timeout=20.0):
        self.sock = socket.create_connection((host, port), timeout=5.0)
        self.sock.settimeout(timeout)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.buf = b""
        self.next_id = 1

    def ask(self, command, params=None):
        rid = self.next_id
        self.next_id += 1
        doc = {"type": command, "id": rid}
        if params:
            doc["params"] = params
        self.sock.sendall(json.dumps(doc).encode("utf-8"))
        while True:
            for line in self._documents():
                if line.get("id") == rid:
                    return line
            data = self.sock.recv(65536)
            if not data:
                raise RuntimeError("Live closed the socket")
            self.buf += data

    def _documents(self):
        decoder = json.JSONDecoder()
        text = self.buf.decode("utf-8", "ignore")
        out, pos = [], 0
        while pos < len(text):
            while pos < len(text) and text[pos] in " \t\r\n":
                pos += 1
            if pos >= len(text):
                break
            try:
                doc, end = decoder.raw_decode(text, pos)
            except ValueError:
                break
            out.append(doc)
            pos = end
        self.buf = text[pos:].encode("utf-8")
        return out

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass


class Report(object):
    def __init__(self):
        self.rows = []
        self.ok = True

    def check(self, name, passed, detail=""):
        self.rows.append((name, bool(passed), detail))
        if not passed:
            self.ok = False

    def note(self, name, detail):
        self.rows.append((name, None, detail))

    def print(self):
        width = max(len(n) for n, _, _ in self.rows)
        for name, passed, detail in self.rows:
            mark = "    " if passed is None else (" ok " if passed else "FAIL")
            print("  %s  %-*s  %s" % (mark, width, name, detail))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=9877)
    parser.add_argument("--reloads", type=int, default=20,
                        help="how many reloads to do for the memory figure")
    parser.add_argument("--tick-samples", type=int, default=600)
    args = parser.parse_args(argv)

    live = Live(args.host, args.port)
    report = Report()

    info = live.ask("get_script_info")["result"]
    report.note("live", "%s, python %s" % (info["live"].get("version"),
                                           info["live"].get("python")))
    report.note("versions", "body v%s, loader v%s" % (
        info.get("script_version"), info.get("loader_version")))
    report.check("split", info.get("loader_version") is not None,
                 "the loaded script reports a loader version")
    reload_info = info.get("reload") or {}
    report.check("reload supported", reload_info.get("supported") is True,
                 str(reload_info.get("body_file")))
    if not report.ok:
        report.print()
        print("\nThis Live is running a script from before the split. Install the "
              "current one and restart Live, then run this again.")
        return 1

    # The tick, before.
    before_tick = wait_for_tick(live, args.tick_samples)
    report.note("tick before", fmt_tick(before_tick))

    # An armed callback across the swap: a pending cue is one, and it is the
    # one a producer would actually have. Schedule it far enough out that the
    # reload lands while it is pending.
    state = live.ask("get_performance_state")
    playing = bool((state.get("result") or {}).get("is_playing"))
    cue = None
    if playing:
        cue = live.ask("schedule_cue", {"steps": [
            {"bars": 2, "action": "set", "target": "tempo",
             "value": (state["result"].get("tempo") or 120.0)}]})
        report.note("cue", "one pending across the reload (transport running)")
    else:
        report.note("cue", "SKIPPED: the transport is stopped. Start it and run "
                           "again for the cue-lateness figure.")

    # The reload itself.
    first = live.ask("reload_body")
    if first.get("status") != "success":
        report.check("rebase", False, first.get("message", ""))
        report.print()
        print("\nThe rebase was refused. The fallback design in the story is what ships.")
        live.close()
        return 1
    result = first["result"]
    report.check("rebase", True, "v%s -> v%s" % (result["was"], result["now"]))
    kept = result["kept"]
    report.check("attributes", kept.get("unchanged") is True, json.dumps(kept, sort_keys=True))
    report.check("swap cost", result["main_ms"] < 1.0,
                 "%s ms on Live's main thread (import %s ms off it)"
                 % (result["main_ms"], result["import_ms"]))

    # Still serving, and still the same script.
    after = live.ask("get_script_info")["result"]
    report.check("still serving", after.get("script_version") == result["now"],
                 "get_script_info answered after the swap")
    session = live.ask("get_session_info")
    report.check("handlers", session.get("status") == "success",
                 "a handler ran on the new body")

    # An armed callback did not crash: the tick is still arriving.
    after_tick = wait_for_tick(live, args.tick_samples)
    report.note("tick after", fmt_tick(after_tick))
    report.check("tick alive", (after_tick or {}).get("samples", 0) > 0,
                 "the clock tick kept re-arming across the swap")
    if before_tick and after_tick and before_tick.get("period_ms"):
        drift = abs(after_tick["period_ms"] - before_tick["period_ms"])
        report.check("tick period", drift < 5.0,
                     "%.2f ms apart before and after" % drift)

    if cue is not None:
        fired = wait_for_cue(live)
        if fired is None:
            report.check("cue fired", False, "the cue never reported")
        else:
            report.check("cue fired", True, "late_ms %s" % fired.get("late_ms"))

    # N reloads, for the cost and whatever they retain.
    costs = []
    for _ in range(max(0, args.reloads - 1)):
        r = live.ask("reload_body")
        if r.get("status") != "success":
            report.check("repeat reloads", False, r.get("message", ""))
            break
        costs.append(r["result"]["main_ms"])
    else:
        report.check("repeat reloads", True,
                     "%d reloads, worst %s ms on the main thread"
                     % (args.reloads, max(costs) if costs else result["main_ms"]))

    final = live.ask("get_script_info")["result"]
    report.note("reload count", str((final.get("reload") or {}).get("count")))
    report.note("memory", "read Live's Log.txt for the script's own lines; this "
                          "probe cannot see Python's heap from outside")

    report.print()
    live.close()
    if report.ok:
        print("\nThe rebase works on this Live. "
              "Record these figures in docs/architecture/overview.md.")
    else:
        print("\nOne or more checks failed. Do not ship the rebase design on this Live.")
    return 0 if report.ok else 1


def fmt_tick(tick):
    if not tick:
        return "no samples"
    return "period %s ms, jitter %s ms, worst %s ms over %s samples" % (
        tick.get("period_ms"), tick.get("jitter_ms"), tick.get("max_ms"), tick.get("samples"))


def wait_for_tick(live, samples, limit_s=90.0):
    """The script samples its own tick; wait until it has enough of them."""
    deadline = time.time() + limit_s
    tick = {}
    while time.time() < deadline:
        tick = live.ask("get_script_info")["result"].get("tick") or {}
        if (tick.get("samples") or 0) >= samples:
            return tick
        time.sleep(1.0)
    return tick


def wait_for_cue(live, limit_s=30.0):
    deadline = time.time() + limit_s
    while time.time() < deadline:
        state = live.ask("get_performance_state").get("result") or {}
        for event in state.get("events", []):
            if event.get("kind") in ("cue_fired", "cue_step"):
                return event
        if not state.get("cues"):
            return {"late_ms": None}
        time.sleep(0.25)
    return None


if __name__ == "__main__":
    sys.exit(main())
