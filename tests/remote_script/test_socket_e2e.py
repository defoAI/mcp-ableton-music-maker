"""The script's own TCP server, for real, against the fake Live.

The accept thread and the tick reader run exactly as they do inside Live;
only `schedule_message` is driven by a thread here instead of Live's
scheduler, and `Live` is `fake_live`. A plain client speaks protocol 2 over
loopback and checks ids, ordering, concurrency and the handshake — and the
handlers are the script's real ones, so a command that says it built
something built it, and the set is asked afterwards. This is the end-to-end
test that does not need Live open.

The three commands it drives:

- `get_session_info` — a read, cheap, one slice.
- `create_tracks` — a real generator: one track per slice, so the executor
  spreads it over ticks exactly as in Live.
- `get_track_info` with an index the set does not have — the error path,
  raised by the model the way Live raises it.
"""
import json
import socket
import threading
import time
import unittest

import fake_live
import harness

# A real socket and a real ticker thread run on real time, so this suite
# uses the wall clock and gives Live's calls their measured cost -- scaled
# down, because the point here is the ORDER work comes back in, not the
# wall-clock figure. 839 ms of device load becomes 50 ms; twenty of them
# still cannot fit in one 40 ms slice, which is what the executor is for.
LATENCY_SCALE = 0.06


class Ticker(object):
    """Runs the harness tick every few milliseconds on a thread, the way
    Live's scheduler would, until stopped."""

    def __init__(self, script, period_s=0.005):
        self.script = script
        self.period = period_s
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.run)
        self.thread.daemon = True

    def run(self):
        while not self.stop.is_set():
            self.script.tick()
            time.sleep(self.period)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *a):
        self.stop.set()
        self.thread.join(2.0)


class Client(object):
    def __init__(self, port):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=5)
        self.buf = b""
        self.next_id = 1

    def send(self, doc):
        self.sock.sendall((json.dumps(doc) + "\n").encode("utf-8"))

    def request(self, command, params=None, with_id=True):
        doc = {"type": command, "params": params or {}}
        if with_id:
            doc["id"] = self.next_id
            self.next_id += 1
        self.send(doc)
        return doc.get("id")

    def read(self, timeout=5.0):
        end = time.time() + timeout
        while time.time() < end:
            nl = self.buf.find(b"\n")
            if nl >= 0:
                line, self.buf = self.buf[:nl], self.buf[nl + 1:]
                if line.strip():
                    return json.loads(line.decode("utf-8"))
                continue
            self.sock.settimeout(max(0.01, end - time.time()))
            try:
                data = self.sock.recv(65536)
            except socket.timeout:
                continue
            if not data:
                raise AssertionError("server closed the socket")
            self.buf += data
        raise AssertionError("no reply within %ss" % timeout)

    def close(self):
        self.sock.close()


class SocketEndToEnd(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load(
            clock=fake_live.WallClock(),
            latency=fake_live.LatencyTable.measured(scale=LATENCY_SCALE))
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.ns["DEFAULT_PORT"] = 0            # an ephemeral port: Live owns 9877
        self.script = harness.instance(self.ns, open_socket=True)
        self.assertTrue(self.script.running, "the server did not start: %s" % self.script.logged)
        self.port = self.script.server.getsockname()[1]
        self.song = self.script.song()

    def tearDown(self):
        self.script.disconnect()

    @staticmethod
    def tracks_named(prefix, n):
        """Params for `create_tracks`: n MIDI tracks each loading an
        instrument, which is #45's workload and the one Live charges for."""
        return {"tracks": [{"name": "%s %d" % (prefix, i + 1), "kind": "midi",
                            "instrument_uri": "query:Synths#Analog"}
                           for i in range(n)]}

    def test_handshake_and_a_command_over_a_real_socket(self):
        with Ticker(self.script):
            c = Client(self.port)
            rid = c.request("get_script_info")
            r = c.read()
            self.assertEqual(r["id"], rid)
            self.assertEqual(r["result"]["protocol_version"], 2)
            self.assertEqual(r["result"]["socket_reader"], "main_thread_tick")
            self.assertGreater(r["result"]["tick"]["samples"], 0, "the tick sampler ran")
            rid = c.request("get_session_info")
            r = c.read()
            self.assertEqual((r["id"], r["status"]), (rid, "success"))
            self.assertEqual(r["result"]["track_count"], len(self.song.tracks))
            self.assertEqual(r["result"]["tempo"], 120.0)
            c.close()

    def test_a_reply_arrives_within_a_few_ticks_not_a_scheduling_quantum(self):
        with Ticker(self.script, period_s=0.005):
            c = Client(self.port)
            c.request("get_script_info"); c.read()
            t = time.time()
            for _ in range(20):
                c.request("get_session_info")
                c.read()
            per = (time.time() - t) / 20 * 1000.0
            # 5 ms ticks: a round trip is a tick or two. The point is that
            # it is nothing like the 200 ms scheduling quantum the
            # background-thread reader cost (decision 0010), so the band is
            # wide enough that a loaded runner cannot fail it while still
            # being far below what it is contrasted with.
            self.assertLess(per, 150.0, "round trip averaged %.1f ms" % per)
            c.close()

    def test_overlapping_requests_come_back_by_id(self):
        with Ticker(self.script):
            c = Client(self.port)
            slow = c.request("create_tracks", self.tracks_named("Slow", 20))
            fast = c.request("get_session_info")
            first = c.read()
            second = c.read()
            self.assertEqual(first["id"], fast, "the fast one did not wait behind the slow one")
            self.assertEqual(second["id"], slow)
            self.assertEqual(len(second["result"]["created"]), 20)
            self.assertGreater(second["slices"], 1, "20 tracks came back in one slice")
            # And the twenty tracks are really in the set, in order, each
            # with the instrument that was asked for.
            built = self.song.tracks[-20:]
            self.assertEqual([t.name for t in built],
                             ["Slow %d" % (i + 1) for i in range(20)])
            self.assertEqual(set(t.devices[0].name for t in built), {"Analog"})
            c.close()

    def test_a_client_without_ids_is_served_in_order_like_before(self):
        with Ticker(self.script):
            c = Client(self.port)
            c.request("create_tracks", self.tracks_named("Ordered", 10), with_id=False)
            c.request("get_session_info", with_id=False)
            first = c.read()
            second = c.read()
            self.assertNotIn("id", first)
            self.assertEqual(len(first["result"]["created"]), 10)
            # The read that queued behind it sees the ten tracks, so it
            # really did run second.
            self.assertEqual(second["result"]["track_count"], len(self.song.tracks))
            self.assertEqual(second["result"]["track_count"], 14)
            c.close()

    def test_two_clients_at_once_do_not_see_each_others_replies(self):
        with Ticker(self.script):
            a = Client(self.port)
            b = Client(self.port)
            ra = a.request("create_tracks", {"tracks": [{"name": "From A"}]})
            rb = b.request("create_tracks", {"tracks": [{"name": "From B"}]})
            self.assertEqual(a.read()["result"]["created"][0]["name"], "From A")
            self.assertEqual(b.read()["result"]["created"][0]["name"], "From B")
            self.assertEqual((ra, rb), (1, 1), "ids are per socket")
            # One set behind both sockets, as Live is: both tracks are in it.
            names = [t.name for t in self.song.tracks]
            self.assertIn("From A", names)
            self.assertIn("From B", names)
            a.close(); b.close()

    def test_an_error_keeps_the_socket_open(self):
        with Ticker(self.script):
            c = Client(self.port)
            c.request("get_track_info", {"track_index": 99})
            r = c.read()
            self.assertEqual(r["status"], "error")
            self.assertIn("range", r["message"].lower())
            c.request("get_session_info")
            self.assertEqual(c.read()["result"]["track_count"], 4)
            c.close()

    def test_a_closed_client_is_forgotten_by_the_script(self):
        with Ticker(self.script):
            c = Client(self.port)
            c.request("get_session_info"); c.read()
            self.assertEqual(len(self.script._clients), 1)
            c.close()
            end = time.time() + 2.0
            while time.time() < end and self.script._clients:
                time.sleep(0.01)
            self.assertEqual(self.script._clients, [])


if __name__ == "__main__":
    unittest.main()
