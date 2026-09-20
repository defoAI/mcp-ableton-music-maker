"""`scripts/fake-live.py` as a client sees it: a real socket, a real tick.

Everything here goes over TCP to a separate process, so what is under test is
the whole thing the Rust side will talk to — the script's accept thread, its
tick reader, its framing and ids, its streams and cues, and the model behind
them. Nothing in this file knows the protocol: it writes newline-delimited
JSON and reads newline-delimited JSON.
"""
import io
import json
import os
import socket
import subprocess
import sys
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
FAKE_LIVE = os.path.join(ROOT, "scripts", "fake-live.py")


class Fake(object):
    """The fake Live in its own process. The port comes off stdout, which
    carries nothing else."""

    def __init__(self, *args):
        self.proc = subprocess.Popen(
            [sys.executable, "-u", FAKE_LIVE] + list(args),
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        line = self.proc.stdout.readline().decode("utf-8").strip()
        if not line.isdigit():
            raise AssertionError("no port on stdout; got %r, stderr: %s"
                                 % (line, self.proc.stderr.read().decode("utf-8")[:2000]))
        self.port = int(line)

    def connect(self, timeout=10):
        return Client(self.port, timeout)

    def stop(self, timeout=10):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=timeout)
        return self.proc.stderr.read().decode("utf-8")

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.stderr = self.stop()


class Client(object):
    def __init__(self, port, timeout=10):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        self.buf = b""
        self.next_id = 1
        # Events that arrived while `ask` was waiting for a reply. Without
        # this they are dropped, and a test that counts events against
        # ticks undercounts the events while counting every tick.
        self.stray_events = []

    def send(self, command, params=None):
        rid = self.next_id
        self.next_id += 1
        self.sock.sendall((json.dumps({"id": rid, "type": command,
                                       "params": params or {}}) + "\n").encode("utf-8"))
        return rid

    def lines(self, seconds, want=None):
        """Every complete document that arrives inside `seconds`, or until
        `want` of them have."""
        out, end = [], time.time() + seconds
        while time.time() < end:
            nl = self.buf.find(b"\n")
            if nl >= 0:
                line, self.buf = self.buf[:nl], self.buf[nl + 1:]
                if line.strip():
                    out.append(json.loads(line.decode("utf-8")))
                    if want is not None and len(out) >= want:
                        return out
                continue
            self.sock.settimeout(max(0.01, end - time.time()))
            try:
                data = self.sock.recv(65536)
            except socket.timeout:
                continue
            if not data:
                break
            self.buf += data
        return out

    def ask(self, command, params=None, seconds=30):
        """One request, its reply, and no waiting past it — a slow `ask`
        would let the transport run while the test thinks it is idle.

        Anything else that arrives meanwhile is kept in `stray_events`, not
        thrown away: on a subscribed socket a reply and an event race each
        other, and dropping the event makes the stream look slower than it
        is."""
        rid = self.send(command, params)
        end = time.time() + seconds
        while time.time() < end:
            docs = self.lines(max(0.01, end - time.time()), want=1)
            if not docs:
                break
            for doc in docs:
                if doc.get("id") == rid:
                    return doc
                if "event" in doc:
                    self.stray_events.append(doc)
        raise AssertionError("no reply to %s in %ss" % (command, seconds))

    def close(self):
        self.sock.close()


def tracks(n, instrument="query:Synths#Analog"):
    return {"tracks": [{"name": "T%d" % i, "kind": "midi", "instrument_uri": instrument}
                       for i in range(n)]}


class TheWire(unittest.TestCase):
    def test_the_port_is_on_stdout_and_nothing_else_is(self):
        with Fake("--latency", "none") as fake:
            self.assertGreater(fake.port, 1024)
            self.assertNotEqual(fake.port, 9877, "Live owns 9877; the fake must not take it")
        self.assertIn("listening on 127.0.0.1", fake.stderr)

    def test_the_handshake_is_the_scripts_own(self):
        with Fake("--latency", "none") as fake:
            c = fake.connect()
            r = c.ask("get_script_info")
            info = r["result"]
            self.assertEqual(info["protocol_version"], 2)
            self.assertEqual(info["socket_reader"], "main_thread_tick")
            self.assertEqual(len(info["capabilities"]), 98)
            self.assertEqual(info["live"]["version"], "12.4.6")
            self.assertTrue(info["bind_is_loopback"])
            c.close()

    def test_the_tick_reports_lives_measured_period(self):
        with Fake("--latency", "none") as fake:
            c = fake.connect()
            c.ask("get_script_info")
            time.sleep(1.5)
            tick = c.ask("get_script_info")["result"]["tick"]
            self.assertGreater(tick["samples"], 4, tick)
            # The fake aims at Live's measured 100.0 ms (jitter 0.24 ms on
            # 12.4.6), and its ticker never runs faster than that. A shared
            # runner can stretch it a long way, so only the floor is really
            # a statement about the fake; the ceiling is there to catch a
            # clock that has stopped, not to measure the machine. What Live's
            # tick actually is, is a real-Live check.
            self.assertGreater(tick["period_ms"], 50.0, tick)
            self.assertLess(tick["period_ms"], 500.0, tick)
            c.close()

    def test_a_faster_tick_can_be_asked_for_and_is_reported_as_such(self):
        """`--tick-ms` is the fast path: asking for 10 ms must not be
        ignored.

        Counted, not timed. In 0.6 s the 100 ms default gives about 6
        samples; 10 ms gives about 60. A loaded runner shrinks both, but a
        ticker that was actually sped up still produces far more than the
        default would — so the count says what a period cannot.
        """
        with Fake("--latency", "none", "--tick-ms", "10") as fake:
            c = fake.connect()
            before = c.ask("get_script_info")["result"]["tick"]["samples"]
            time.sleep(0.6)
            tick = c.ask("get_script_info")["result"]["tick"]
            self.assertGreater(
                tick["samples"] - before, 12,
                "0.6 s gave %d ticks; the 100 ms default would give about 6"
                % (tick["samples"] - before))
            c.close()

    def test_an_id_is_echoed_and_an_error_keeps_the_socket_open(self):
        with Fake("--latency", "none") as fake:
            c = fake.connect()
            r = c.ask("get_track_info", {"track_index": 99})
            self.assertEqual(r["status"], "error")
            self.assertIn("range", r["message"].lower())
            self.assertEqual(c.ask("get_session_info")["result"]["track_count"], 4)
            c.close()


class TheSet(unittest.TestCase):
    def test_a_build_over_the_wire_is_really_in_the_set(self):
        with Fake("--latency", "none") as fake:
            c = fake.connect()
            r = c.ask("create_tracks", tracks(3))
            self.assertEqual(r["status"], "success")
            names = [t["name"] for t in c.ask("get_context")["result"]["tracks"]]
            for i in range(3):
                self.assertIn("T%d" % i, names)
            # And the notes read back through the script, not the reply.
            c.ask("create_clip", {"track_index": 4, "clip_index": 0, "length": 4.0})
            c.ask("add_notes_to_clip", {"track_index": 4, "clip_index": 0, "notes": [
                {"pitch": 60, "start_time": 0.0, "duration": 1.0, "velocity": 100},
                {"pitch": 64, "start_time": 1.0, "duration": 1.0, "velocity": 90}]})
            got = c.ask("get_clip_notes", {"track_index": 4, "clip_index": 0})["result"]
            self.assertEqual(sorted(n["pitch"] for n in got["notes"]), [60, 64])
            c.close()

    def test_each_connection_gets_its_own_set_by_default(self):
        with Fake("--latency", "none") as fake:
            a, b = fake.connect(), fake.connect()
            a.ask("create_tracks", tracks(2))
            self.assertEqual(a.ask("get_session_info")["result"]["track_count"], 6)
            self.assertEqual(b.ask("get_session_info")["result"]["track_count"], 4,
                             "the second connection saw the first one's tracks")
            a.close(); b.close()

    def test_a_shared_set_is_what_live_is_and_both_connections_see_it(self):
        with Fake("--latency", "none", "--shared-set") as fake:
            a, b = fake.connect(), fake.connect()
            a.ask("create_tracks", tracks(2))
            self.assertEqual(b.ask("get_session_info")["result"]["track_count"], 6)
            a.close(); b.close()

    def test_subscribing_from_a_second_connection_is_refused_not_answered_wrongly(self):
        """The event channels run on the tick, which belongs to the
        application. In per-connection mode a second socket would be told
        about a set it is not looking at, so it is refused instead."""
        with Fake("--latency", "none") as fake:
            a, b = fake.connect(), fake.connect()
            a.ask("get_session_info")
            b.ask("get_session_info")
            self.assertEqual(a.ask("subscribe", {"channels": ["clock"]})["status"], "success")
            refused = b.ask("subscribe", {"channels": ["clock"]})
            self.assertEqual(refused["status"], "error")
            self.assertIn("--shared-set", refused["message"])
            a.close(); b.close()


class TheClock(unittest.TestCase):
    def test_the_clock_channel_reaches_a_real_socket_while_the_transport_runs(self):
        """What only this test can say: the events get onto the wire.

        **The rate is not asserted here, on purpose.** #51 — 100 ms asked
        for arriving every 199 ms — is owned by
        `test_streams.test_a_100_ms_subscription_gets_an_event_on_every_tick`,
        which drives `_clock_event_tick` with explicit timestamps, jitter
        included, and asserts exactly 20 events for 20 ticks. That test is
        deterministic and machine-independent.

        Asserting a rate here measures the machine instead. On a runner
        whose ticks are jittery — CI saw a 100 ms median with enough spread
        to skip 8 windows in 20 — a healthy clock and #51's clock produce
        the same count, so no threshold can tell them apart. Two attempts at
        one taught that; the third is to let the deterministic test own the
        rate and let this one own the wire.
        """
        with Fake("--latency", "none", "--shared-set") as fake:
            c = fake.connect()
            c.ask("subscribe", {"channels": ["clock"], "clock_every_ms": 100})
            c.ask("start_playback")
            c.stray_events = []
            docs = c.lines(2.0)
            events = [d for d in docs + c.stray_events if d.get("event") == "clock"]

            self.assertTrue(events, "no clock event reached the socket at all")
            bars = [e["bar"] for e in events]
            self.assertEqual(bars, sorted(bars), "bars went backwards")
            self.assertTrue(all(e["playing"] for e in events))
            for field in ("bar", "beat_in_bar", "tempo", "next_bar_in_s", "t"):
                self.assertIn(field, events[0], events[0])
            self.assertEqual(events[0]["tempo"], 120.0)
            # A sanity bound either side, wide enough that only something
            # badly wrong trips it: silence, or a burst that ignores the
            # rate entirely. Two seconds at the 10/s asked for is ~20.
            self.assertGreater(len(events), 2, "%d events in two seconds" % len(events))
            self.assertLess(len(events), 60, "%d events is a burst" % len(events))
            c.close()

    def test_a_cue_fires_on_the_beat_and_says_how_late_it_was(self):
        with Fake("--latency", "none", "--shared-set") as fake:
            c = fake.connect()
            c.ask("subscribe", {"channels": ["cue"]})
            c.ask("start_playback")
            r = c.ask("schedule_cue", {"steps": [
                {"beat": 8.0, "action": "set", "target": "tempo", "value": 130.0}]})
            self.assertEqual(r["status"], "success")
            fired = [d for d in c.lines(8.0) if d.get("event") == "cue"]
            self.assertTrue(fired, "the cue never fired")
            # The cue is triggered on the tick, so it lands inside one —
            # decision 0010 measured 34.7 ms p50 / 64.7 ms p95 against a
            # real Live. The band here is wide because a shared runner can
            # stretch the ticker thread; what is under test is that the
            # lateness is reported and is a tick's worth, not a bar's.
            self.assertLess(fired[0]["late_ms"], 1000.0, fired[0])
            self.assertGreaterEqual(fired[0]["late_ms"], 0.0, fired[0])
            self.assertAlmostEqual(c.ask("get_session_info")["result"]["tempo"], 130.0)
            c.close()


class WhatACallCosts(unittest.TestCase):
    def test_a_track_with_an_instrument_costs_what_it_costs_in_live(self):
        """#43 measured one device load at 839 ms of Live's main thread.
        Four of them is four slices, one per track, because the executor
        cannot fit two in one budget — which is #45's shape."""
        with Fake("--shared-set") as fake:
            c = fake.connect()
            r = c.ask("create_tracks", tracks(4), seconds=60)
            self.assertEqual(r["status"], "success")
            self.assertEqual(r["slices"], 4, "one track per slice, as in Live")
            self.assertGreater(r["main_ms"], 4 * 800.0, r["main_ms"])
            c.close()

    def test_turning_the_table_off_makes_the_same_build_instant(self):
        with Fake("--latency", "none", "--shared-set") as fake:
            c = fake.connect()
            r = c.ask("create_tracks", tracks(4))
            # With no table to charge from, the work itself is microseconds;
            # the bound is loose because `main_ms` is wall-clock inside the
            # task and a loaded runner stretches it. The point is the
            # contrast with the 3358 ms above, not the figure.
            self.assertLess(r["main_ms"], 500.0, r["main_ms"])
            c.close()

    def test_an_invented_cost_is_marked_as_invented_everywhere(self):
        with Fake("--latency", "none", "--shared-set",
                  "--slow", "Song.create_midi_track=250") as fake:
            c = fake.connect()
            r = c.ask("create_tracks", {"tracks": [{"name": "X", "kind": "midi"}]}, seconds=30)
            self.assertGreater(r["main_ms"], 200.0)
            c.close()
        self.assertIn("NOT MEASURED", fake.stderr)

    def test_the_latency_report_names_the_calls_nobody_has_measured(self):
        out = subprocess.check_output([sys.executable, FAKE_LIVE, "--latency-report"],
                                      cwd=ROOT).decode("utf-8")
        report = json.loads(out)
        self.assertIn("Song.create_midi_track", report["unmeasured"])
        calls = [row["call"] for row in report["rows"]]
        self.assertIn("Browser.load_item", calls)
        for row in report["rows"]:
            self.assertTrue(row["source"], "%s has no source" % row["call"])
        self.assertEqual(report["tick_ms"], 100.0)
        self.assertIn("2026-09-20", report["tick_measured"])

    def test_a_slow_slice_lands_in_the_log_as_it_would_in_lives(self):
        path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "fake-live-test.log")
        with Fake("--shared-set", "--script-log", path) as fake:
            c = fake.connect()
            c.ask("create_tracks", tracks(2), seconds=60)
            c.close()
        with io.open(path, encoding="utf-8") as handle:
            text = handle.read()
        self.assertIn("slow slice", text, "a 839 ms device load is a slow slice")
        self.assertIn("create_tracks", text)
        self.assertIn("slow slices", fake.stderr)
        os.unlink(path)


class WhenLiveGoesAway(unittest.TestCase):
    """#45 as it happened: the socket died mid-command and the producer was
    left with half a set. A fake that always answers cannot test the resume
    path; this one can be told to go away."""

    def test_die_on_a_command_closes_the_socket_with_the_reply_unsent(self):
        with Fake("--latency", "none", "--shared-set", "--die-on", "create_tracks") as fake:
            c = fake.connect()
            self.assertEqual(c.ask("get_session_info")["status"], "success")
            c.send("create_tracks", tracks(3))
            self.assertEqual(c.lines(5.0), [], "it answered after being told to die")
            c.close()

    def test_die_after_n_replies_serves_n_and_then_stops(self):
        with Fake("--latency", "none", "--shared-set", "--die-after", "2") as fake:
            c = fake.connect()
            self.assertEqual(c.ask("get_session_info")["status"], "success")
            self.assertEqual(c.ask("get_session_info")["status"], "success")
            c.send("get_session_info")
            self.assertEqual(c.lines(4.0), [])
            c.close()


class TheRecording(unittest.TestCase):
    def test_every_request_and_reply_is_recorded_as_one_json_line_each(self):
        path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "fake-live-wire.jsonl")
        with Fake("--latency", "none", "--shared-set", "--record", path) as fake:
            c = fake.connect()
            c.ask("get_script_info")
            c.ask("set_tempo", {"tempo": 126.0})
            c.ask("get_session_info")
            c.close()
        with io.open(path, encoding="utf-8") as handle:
            rows = [json.loads(line) for line in handle if line.strip()]
        os.unlink(path)
        ins = [r for r in rows if r["dir"] == "in"]
        outs = [r for r in rows if r["dir"] == "out"]
        self.assertEqual([r["type"] for r in ins],
                         ["get_script_info", "set_tempo", "get_session_info"])
        self.assertEqual(len(outs), 3)
        self.assertEqual([r["type"] for r in outs],
                         ["get_script_info", "set_tempo", "get_session_info"],
                         "a reply was not paired with its command")
        self.assertEqual(outs[-1]["result"]["tempo"], 126.0)
        self.assertTrue(all("at" in r for r in rows), "no timing on the wire")


if __name__ == "__main__":
    unittest.main()
