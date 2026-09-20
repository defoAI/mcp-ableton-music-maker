"""The script's own TCP server, for real, with Live stubbed.

The accept thread and the tick reader run exactly as they do inside Live;
only `schedule_message` is driven by a thread here instead of Live's
scheduler. A plain client speaks protocol 2 over loopback and checks ids,
ordering, concurrency and the handshake. This is the end-to-end test that
does not need Live open.
"""
import json
import socket
import threading
import time
import unittest

import harness


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
        self.ns = harness.load()
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.ns["DEFAULT_PORT"] = 0            # an ephemeral port: Live owns 9877
        self.script = harness.instance(self.ns, open_socket=True)
        self.assertTrue(self.script.running, "the server did not start: %s" % self.script.logged)
        self.port = self.script.server.getsockname()[1]
        # Handlers the stub Song can serve.
        self.script._dispatch = self.dispatch

    def dispatch(self, command_type, params, response_queue):
        Done = self.ns["Done"]
        if command_type == "echo":
            return {"echo": params}
        if command_type == "slow":
            # Real work takes time; the executor's slice budget (40 ms with
            # the transport stopped) then spreads it over several ticks.
            def gen():
                for _ in range(int(params.get("ticks", 3))):
                    time.sleep(0.01)
                    yield None
                yield Done({"slow": True})
            return gen()
        if command_type == "boom":
            raise ValueError("boom")
        raise ValueError("Unknown command: %s" % command_type)

    def tearDown(self):
        self.script.disconnect()

    def test_handshake_and_a_command_over_a_real_socket(self):
        with Ticker(self.script):
            c = Client(self.port)
            rid = c.request("get_script_info")
            r = c.read()
            self.assertEqual(r["id"], rid)
            self.assertEqual(r["result"]["protocol_version"], 2)
            self.assertEqual(r["result"]["socket_reader"], "main_thread_tick")
            self.assertGreater(r["result"]["tick"]["samples"], 0, "the tick sampler ran")
            rid = c.request("echo", {"x": 1})
            r = c.read()
            self.assertEqual((r["id"], r["status"], r["result"]), (rid, "success", {"echo": {"x": 1}}))
            c.close()

    def test_a_reply_arrives_within_a_few_ticks_not_a_scheduling_quantum(self):
        with Ticker(self.script, period_s=0.005):
            c = Client(self.port)
            c.request("get_script_info"); c.read()
            t = time.time()
            for _ in range(20):
                c.request("echo", {})
                c.read()
            per = (time.time() - t) / 20 * 1000.0
            # 5 ms ticks: a round trip is a tick or two, not hundreds of ms.
            self.assertLess(per, 60.0, "round trip averaged %.1f ms" % per)
            c.close()

    def test_overlapping_requests_come_back_by_id(self):
        with Ticker(self.script):
            c = Client(self.port)
            slow = c.request("slow", {"ticks": 20})
            fast = c.request("echo", {"n": 2})
            first = c.read()
            second = c.read()
            self.assertEqual(first["id"], fast, "the fast one did not wait behind the slow one")
            self.assertEqual(second["id"], slow)
            self.assertEqual(second["result"], {"slow": True})
            c.close()

    def test_a_client_without_ids_is_served_in_order_like_before(self):
        with Ticker(self.script):
            c = Client(self.port)
            c.request("slow", {"ticks": 10}, with_id=False)
            c.request("echo", {"n": 2}, with_id=False)
            first = c.read()
            second = c.read()
            self.assertNotIn("id", first)
            self.assertEqual(first["result"], {"slow": True})
            self.assertEqual(second["result"], {"echo": {"n": 2}})
            c.close()

    def test_two_clients_at_once_do_not_see_each_others_replies(self):
        with Ticker(self.script):
            a = Client(self.port)
            b = Client(self.port)
            ra = a.request("echo", {"who": "a"})
            rb = b.request("echo", {"who": "b"})
            self.assertEqual(a.read()["result"], {"echo": {"who": "a"}})
            self.assertEqual(b.read()["result"], {"echo": {"who": "b"}})
            self.assertEqual((ra, rb), (1, 1), "ids are per socket")
            a.close(); b.close()

    def test_an_error_keeps_the_socket_open(self):
        with Ticker(self.script):
            c = Client(self.port)
            c.request("boom")
            r = c.read()
            self.assertEqual(r["status"], "error")
            self.assertIn("boom", r["message"])
            c.request("echo", {"still": "here"})
            self.assertEqual(c.read()["result"], {"echo": {"still": "here"}})
            c.close()

    def test_a_closed_client_is_forgotten_by_the_script(self):
        with Ticker(self.script):
            c = Client(self.port)
            c.request("echo", {}); c.read()
            self.assertEqual(len(self.script._clients), 1)
            c.close()
            end = time.time() + 2.0
            while time.time() < end and self.script._clients:
                time.sleep(0.01)
            self.assertEqual(self.script._clients, [])


if __name__ == "__main__":
    unittest.main()
