"""Phase 1 of the streams story: one duplex socket read on the tick.

A request with an id is answered with that id whenever its handler is done;
one without is answered in order, one at a time. Writes are buffered and
bounded. A handler that never answers is timed out on the tick. All of it
runs here with a fake socket and the tick driven by hand.
"""
import errno
import json
import socket
import unittest

import harness


class FakeSock(object):
    """A non-blocking socket as the tick reader sees it."""

    def __init__(self):
        self.to_read = b""
        self.sent = b""
        self.closed = False
        self.block_writes = False
        self.write_limit = None   # bytes accepted per tick, None = all
        self.sent_this_tick = False

    def feed(self, text):
        self.to_read += text.encode("utf-8") if not isinstance(text, bytes) else text

    def recv(self, n):
        if not self.to_read:
            raise socket.error(errno.EAGAIN, "would block")
        data, self.to_read = self.to_read[:n], self.to_read[n:]
        return data

    def send(self, data):
        if self.block_writes:
            raise socket.error(errno.EAGAIN, "would block")
        if self.write_limit is None:
            self.sent += data
            return len(data)
        # A slow reader: a few bytes go, then the kernel says "would block"
        # until the test lets the next tick happen.
        if self.sent_this_tick:
            raise socket.error(errno.EAGAIN, "would block")
        take = min(self.write_limit, len(data))
        self.sent += data[:take]
        self.sent_this_tick = True
        return take

    def close(self):
        self.closed = True

    def setblocking(self, flag):
        pass

    def replies(self):
        """Every complete reply written so far; a partial last line is not
        a reply yet (a slow reader sees exactly that)."""
        text = self.sent.decode("utf-8")
        lines = text.split("\n")
        if not text.endswith("\n"):
            lines = lines[:-1]
        return [json.loads(line) for line in lines if line.strip()]


def connect(script, ns):
    sock = FakeSock()
    client = ns["_Client"](sock, ("127.0.0.1", 50000))
    script._clients.append(client)
    return sock, client


class Framing(unittest.TestCase):
    def setUp(self):
        self.split = harness.load()["_split_documents"]

    def test_newline_delimited_and_concatenated_both_parse(self):
        docs, rest = self.split(b'{"a":1}\n{"b":2}{"c":3}\n')
        self.assertEqual(docs, [{"a": 1}, {"b": 2}, {"c": 3}])
        self.assertEqual(rest, b"")

    def test_a_partial_document_waits(self):
        docs, rest = self.split(b'{"a":1}\n{"b":')
        self.assertEqual(docs, [{"a": 1}])
        self.assertEqual(rest, b'{"b":')
        docs, rest = self.split(rest + b"2}")
        self.assertEqual(docs, [{"b": 2}])
        self.assertEqual(rest, b"")

    def test_a_split_multibyte_character_waits_too(self):
        text = json.dumps({"name": u"Groove \u00b7 8"}, ensure_ascii=False).encode("utf-8")
        cut = text.index(b"\xc2")  # first byte of the middle dot
        docs, rest = self.split(text[:cut + 1])
        self.assertEqual(docs, [])
        self.assertEqual(rest, text[:cut + 1])
        docs, rest = self.split(rest + text[cut + 1:])
        self.assertEqual(docs, [{"name": "Groove · 8"}])


class Duplex(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.sock, self.client = connect(self.script, self.ns)

    def test_a_request_with_an_id_is_answered_with_that_id(self):
        # The stub Song has no mixer, so the handler is a stand-in; what is
        # under test is the path from bytes in to a reply with the id.
        self.script._dispatch = lambda ct, p, q: {"ok": True}
        self.sock.feed('{"id": 7, "type": "get_session_info", "params": {}}\n')
        self.script.tick()
        r = self.sock.replies()
        self.assertEqual(len(r), 1)
        self.assertEqual(r[0]["id"], 7)
        self.assertEqual(r[0]["status"], "success")
        self.assertIn("main_ms", r[0])

    def test_the_handshake_is_answered_on_the_tick_without_the_executor(self):
        self.sock.feed('{"id": 1, "type": "get_script_info"}')
        self.script.tick()
        r = self.sock.replies()
        self.assertEqual(r[0]["id"], 1)
        self.assertEqual(r[0]["result"]["socket_reader"], "main_thread_tick")
        self.assertEqual(r[0]["result"]["protocol_version"], 2)

    def test_two_requests_in_one_read_get_their_own_ids(self):
        self.script._dispatch = lambda ct, p, q: {"ok": True}
        self.sock.feed('{"id": 1, "type": "get_session_info"}\n{"id": 2, "type": "get_session_info"}\n')
        self.script.tick()
        ids = sorted(x["id"] for x in self.sock.replies())
        self.assertEqual(ids, [1, 2])

    def test_requests_without_ids_are_answered_in_order_one_at_a_time(self):
        # A handler that takes two ticks: the second request must not start
        # until it has answered, and neither reply carries an id.
        Done = self.ns["Done"]
        order = []

        def slow_then_fast(command_type, params, response_queue):
            if command_type == "slow":
                def gen():
                    order.append("slow-start")
                    yield None
                    order.append("slow-end")
                    yield Done({"which": "slow"})
                return gen()
            order.append("fast")
            return {"which": "fast"}
        self.script._dispatch = slow_then_fast
        self.script.COMMAND_TIMEOUTS = {}
        # A generous slice budget would finish the generator in one tick;
        # force a slice boundary so the order is observable.
        self.script._slice_budget = lambda: -1.0
        self.sock.feed('{"type": "slow"}\n{"type": "fast"}\n')
        self.script.tick()
        self.assertEqual(order, ["slow-start"])
        self.assertEqual(self.sock.replies(), [])
        self.script.tick()
        self.script.tick()
        replies = self.sock.replies()
        self.assertEqual([r["result"]["which"] for r in replies], ["slow", "fast"])
        self.assertTrue(all("id" not in r for r in replies))
        self.assertEqual(order, ["slow-start", "slow-end", "fast"])

    def test_requests_with_ids_overlap(self):
        Done = self.ns["Done"]

        def dispatch(command_type, params, response_queue):
            if command_type == "slow":
                def gen():
                    yield None
                    yield Done({"which": "slow"})
                return gen()
            return {"which": "fast"}
        self.script._dispatch = dispatch
        self.script._slice_budget = lambda: -1.0
        self.sock.feed('{"id": 1, "type": "slow"}\n{"id": 2, "type": "fast"}\n')
        self.script.tick()
        first = self.sock.replies()
        self.assertEqual([(r["id"], r["result"]["which"]) for r in first], [(2, "fast")], "fast did not wait")
        self.script.tick()
        self.script.tick()
        self.assertEqual([(r["id"], r["result"]["which"]) for r in self.sock.replies()], [(2, "fast"), (1, "slow")])

    def test_a_handler_that_never_answers_is_timed_out_on_the_tick(self):
        DEFERRED = self.ns["DEFERRED"]
        self.script._dispatch = lambda *a: DEFERRED
        self.script.COMMAND_TIMEOUTS = {"never": 0.0}
        self.sock.feed('{"id": 9, "type": "never"}\n')
        self.script.tick()
        # The deadline is `now + 0`; the next tick is after it.
        self.script.tick()
        r = self.sock.replies()
        self.assertEqual(r[0]["id"], 9)
        self.assertEqual(r[0]["status"], "error")
        self.assertIn("Timeout", r[0]["message"])
        self.assertEqual(self.client.inflight, {})

    def test_a_late_answer_after_a_timeout_is_dropped_not_sent_twice(self):
        DEFERRED = self.ns["DEFERRED"]
        sinks = []

        def dispatch(command_type, params, response_queue):
            sinks.append(response_queue)
            return DEFERRED
        self.script._dispatch = dispatch
        self.script.COMMAND_TIMEOUTS = {"never": 0.0}
        self.sock.feed('{"id": 3, "type": "never"}\n')
        self.script.tick()
        self.script.tick()
        sinks[0].put({"status": "success", "result": {"late": True}})
        self.script.tick()
        r = self.sock.replies()
        self.assertEqual(len(r), 1)
        self.assertEqual(r[0]["status"], "error")

    def test_a_client_that_stops_reading_is_dropped_not_waited_for(self):
        self.sock.block_writes = True
        self.client.MAX_OUTBOUND = 200
        for i in range(20):
            self.sock.feed('{"id": %d, "type": "get_session_info"}\n' % i)
        self.script.tick()
        self.assertTrue(self.client.closed)
        self.assertTrue(self.sock.closed)
        self.assertNotIn(self.client, self.script._clients)
        self.assertTrue(any("not reading" in m for m in self.script.logged))

    def test_a_partial_write_is_continued_on_the_next_tick(self):
        self.script._dispatch = lambda ct, p, q: {"ok": True}
        self.sock.write_limit = 10
        self.sock.feed('{"id": 1, "type": "get_session_info"}\n')
        self.script.tick()
        self.assertEqual(self.sock.replies(), [], "only ten bytes went out")
        self.assertEqual(len(self.sock.sent), 10)
        for _ in range(40):
            self.sock.sent_this_tick = False
            self.script.tick()
        self.assertEqual(self.sock.replies()[0]["id"], 1)
        self.assertFalse(self.client.closed, "a slow reader is not a dead one")

    def test_a_closed_socket_is_forgotten(self):
        self.sock.recv = lambda n: b""
        self.script.tick()
        self.assertTrue(self.client.closed)
        self.assertEqual(self.script._clients, [])

    def test_a_non_object_request_is_refused_and_the_socket_survives(self):
        self.script._dispatch = lambda ct, p, q: {"ok": True}
        self.sock.feed('[1,2]\n{"id": 5, "type": "get_session_info"}\n')
        self.script.tick()
        r = self.sock.replies()
        self.assertEqual(r[0]["status"], "error")
        self.assertEqual(r[1]["id"], 5)

    def test_an_exception_in_a_handler_is_an_error_reply_with_the_id(self):
        def boom(*a):
            raise ValueError("no such track")
        self.script._dispatch = boom
        self.sock.feed('{"id": 4, "type": "anything"}\n')
        self.script.tick()
        r = self.sock.replies()
        self.assertEqual((r[0]["id"], r[0]["status"]), (4, "error"))
        self.assertIn("no such track", r[0]["message"])

    def test_disconnect_closes_tick_clients(self):
        self.script.disconnect()
        self.assertTrue(self.sock.closed)
        self.assertEqual(self.script._clients, [])


class ReaderChoice(unittest.TestCase):
    def test_default_is_the_tick_and_the_file_can_choose_the_thread(self):
        ns = harness.load()
        self.assertEqual(ns["SOCKET_READER"], "main_thread_tick")
        self.assertEqual(ns["READER_FILE_NAME"], "socket_reader.txt")


if __name__ == "__main__":
    unittest.main()
