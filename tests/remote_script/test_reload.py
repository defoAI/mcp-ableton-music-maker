"""The body is replaced while the script runs, and nothing else moves.

The script is two files. The loader is what Live holds — the socket, the
accept thread, the framing, the tick, the executor — and the body is every
handler. `reload_body` re-reads the body from beside the loader, on a worker
thread, and then reassigns `instance.__class__` on Live's own thread. The
instance keeps its identity, so everything it was holding is the same object
afterwards.

These tests write real bodies to a real directory and load them through the
loader's own `_load_body_module`, so what is under test is the mechanism as
it runs inside Live and not a stand-in for it. What they cannot test is
whether Live's interpreter allows the rebase at all: that is a fact about
Live's Python and it is measured by `scripts/live-reload-probe.py` against a
real Live (`TheModelIsNotLive`).

Covers AC6–AC10 and AC15 of
`docs/product_management/stories/remote-script-reloads-its-body-without-restarting-live.md`.
"""
import io
import os
import shutil
import sys
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import fake_live  # noqa: E402
import harness  # noqa: E402


class Sink(object):
    """Where the reload's answer lands: what `_ClientSink` and the
    background reader's queue both are, reduced to the one method."""

    def __init__(self):
        self.payload = None

    def put(self, payload):
        self.payload = payload


class QuietSocket(object):
    """A socket with nothing to say: the drain reads it every tick and must
    find a client that is still there afterwards, not one it dropped."""

    def recv(self, _n):
        import errno
        import socket
        raise socket.error(errno.EAGAIN, "nothing to read")

    def send(self, data):
        return len(data)

    def close(self):
        pass


class ReloadCase(unittest.TestCase):
    """A loader and a body in a directory of the test's own making.

    The real body is 6,800 lines and a copy of it per test would make this
    suite slow for no gain, so the bodies written here are small ones that
    derive from the same `Loader` — which is exactly what the mechanism
    requires and all it requires.
    """

    BODY = '''
class Body(Loader):
    VERSION_MARK = "%(mark)s"

    @staticmethod
    def _body_defaults():
        return %(defaults)s

    def _dispatch(self, command_type, params, response_queue):
        if command_type == "what_am_i":
            return {"mark": self.VERSION_MARK}
        raise Exception("Unknown command: " + command_type)
'''

    def setUp(self):
        self.ns = harness.load(song=fake_live.default_set())
        self.dir = tempfile.mkdtemp(prefix="ableton-reload-")
        self.addCleanup(shutil.rmtree, self.dir, True)
        # The loader looks for `body.py` beside its own `__file__`, so point
        # its idea of where it lives at the scratch directory.
        self.ns["__file__"] = os.path.join(self.dir, "__init__.py")
        self.ns["_body_path"].__globals__["__file__"] = self.ns["__file__"]
        self.write_body("first")
        module = self.ns["_load_body_module"]()
        self.ns["_body_module"] = module
        self.ns["Body"] = self.ns["_body_class"](module)
        self.script = harness.instance(self.ns)

    def tearDown(self):
        self.ns["_body_path"].__globals__["__file__"] = harness.SCRIPT

    def write_body(self, mark, defaults="{}", version="1.0.0", needs="1.0.0",
                   source=None, name="body.py"):
        text = source
        if text is None:
            text = ('SCRIPT_VERSION = "%s"\nNEEDS_LOADER = "%s"\n' % (version, needs)
                    + self.BODY % {"mark": mark, "defaults": defaults})
        with io.open(os.path.join(self.dir, name), "w", encoding="utf-8") as handle:
            handle.write(text)

    def reload(self, peer="127.0.0.1", params=None, limit=400):
        """A reload the way `_client_start` sends one, driven to its answer."""
        sink = Sink()
        self.script._begin_reload(sink, peer, params or {})
        for _ in range(limit):
            if sink.payload is not None:
                break
            time.sleep(0.005)
            self.script.tick()
        self.assertIsNotNone(sink.payload, "the reload never answered")
        return sink.payload

    def mark(self):
        return self.script._dispatch("what_am_i", {}, None)["mark"]


class TheSwap(ReloadCase):
    def test_a_new_body_serves_the_next_request(self):
        self.assertEqual(self.mark(), "first")
        self.write_body("second", version="1.1.0")
        reply = self.reload()
        self.assertEqual(reply["status"], "success")
        self.assertEqual(reply["result"]["was"], "1.0.0")
        self.assertEqual(reply["result"]["now"], "1.1.0")
        self.assertEqual(self.mark(), "second")

    def test_the_instance_is_the_same_object(self):
        """Nothing is handed over, so there is no second copy of the state."""
        before = id(self.script)
        self.script.marker_the_test_put_here = 41
        self.write_body("second")
        self.reload()
        self.assertEqual(id(self.script), before)
        self.assertEqual(self.script.marker_the_test_put_here, 41)

    def test_a_body_written_twice_in_the_same_second_is_still_picked_up(self):
        """AC10: a fresh module name each time, and no .pyc to go stale on a
        timestamp whose granularity is a second."""
        self.write_body("second")
        self.reload()
        self.assertEqual(self.mark(), "second")
        self.write_body("third")          # same second, same file size
        self.reload()
        self.assertEqual(self.mark(), "third")
        self.assertFalse(os.path.isdir(os.path.join(self.dir, "__pycache__")),
                         "the reload wrote bytecode it could later read back stale")

    def test_each_reload_is_a_fresh_module(self):
        self.write_body("second")
        first = self.reload()["result"]["generation"]
        self.write_body("third")
        second = self.reload()["result"]["generation"]
        self.assertGreater(second, first)
        self.assertNotIn("AbletonMusicMaker_body_%d" % second, sys.modules,
                         "a generation was left in sys.modules and can never be collected")

    def test_the_count_and_the_cost_are_reported(self):
        self.write_body("second")
        reply = self.reload()
        self.assertEqual(reply["result"]["count"], 1)
        self.assertEqual(self.script._reload_count, 1)
        self.assertEqual(self.script._reload_last_ms, reply["main_ms"])
        info = self.script._get_script_info()
        self.assertEqual(info["reload"]["count"], 1)
        self.assertEqual(info["reload"]["last_ms"], reply["main_ms"])
        self.assertEqual(info["script_version"], "1.0.0")
        self.assertEqual(info["loader_version"], self.ns["LOADER_VERSION"])


class WhatIsKept(ReloadCase):
    """AC8: the socket, the clients, the subscriptions, the cues, the
    performance mode, the scene tables, the peaks and the snapshots."""

    def loaded_state(self):
        client = self.ns["_Client"](QuietSocket(), ("127.0.0.1", 5000))
        client.subscriptions = {"clock": {}, "levels": {}}
        with self.script._clients_lock:
            self.script._clients.append(client)
        self.script._cues = [{"id": 1, "steps": []}]
        self.script._performance_mode = True
        self.script._scene_phrase = {0: 8, 1: 16}
        self.script._scene_started = {0: 1}
        self.script._bar_peaks = {"master": -6.0}
        self.script._mix_snapshots = {1: {"tracks": []}}
        self.script._library_key_cache = "a key"
        return client

    def test_every_count_survives_and_the_reply_says_so(self):
        client = self.loaded_state()
        server = object()
        self.script.server = server
        self.write_body("second")
        kept = self.reload()["result"]["kept"]
        self.assertEqual(kept["clients"], 1)
        self.assertEqual(kept["subscriptions"], 2)
        self.assertEqual(kept["cues"], 1)
        self.assertEqual(kept["performance_mode"], True)
        self.assertEqual(kept["scene_phrases"], 2)
        self.assertEqual(kept["bar_peaks"], True)
        self.assertEqual(kept["mix_snapshots"], 1)
        self.assertTrue(kept["unchanged"])
        # And the objects themselves, not only the counts.
        self.assertIs(self.script.server, server)
        self.assertIs(self.script._clients[0], client)
        self.assertEqual(self.script._clients[0].subscriptions, {"clock": {}, "levels": {}})
        self.assertEqual(self.script._scene_started, {0: 1})
        self.assertEqual(self.script._library_key_cache, "a key")

    def test_a_body_that_needs_a_new_attribute_gets_one_and_only_one(self):
        self.script._performance_mode = True
        self.write_body("second", defaults='{"_performance_mode": False, "_brand_new": 7}')
        result = self.reload()["result"]
        self.assertEqual(result["attributes_added"], ["_brand_new"])
        self.assertEqual(self.script._brand_new, 7)
        self.assertTrue(self.script._performance_mode,
                        "a default overwrote state the running body was holding")

    def test_the_tick_keeps_running_across_the_swap(self):
        self.write_body("second")
        before = self.script._tick_sampler.stats()["samples"]
        self.reload()
        for _ in range(5):
            time.sleep(0.005)
            self.script.tick()
        self.assertGreater(self.script._tick_sampler.stats()["samples"], before)
        self.assertTrue(self.script._clock_tick_armed or self.script.scheduled,
                        "the clock tick stopped re-arming")


class EveryRefusal(ReloadCase):
    """AC7: on any failure the running body keeps serving, and the message
    names both the reason and the version that is still there."""

    def refused(self, contains, **kwargs):
        reply = self.reload(**kwargs)
        self.assertEqual(reply["status"], "error")
        self.assertIn(contains, reply["message"])
        self.assertIn("Still running body v1.0.0", reply["message"])
        self.assertEqual(self.mark(), "first", "the old body stopped serving")
        self.assertEqual(self.script._reload_count, 0)
        return reply

    def test_a_missing_file(self):
        os.remove(os.path.join(self.dir, "body.py"))
        self.refused("is not beside the loader")

    def test_a_syntax_error(self):
        self.write_body("second", source="def (:\n")
        reply = self.refused("SyntaxError")
        self.assertIn("body.py", reply["traceback"])

    def test_a_body_that_raises_at_import(self):
        self.write_body("second", source="raise ValueError('not today')\n")
        self.refused("not today")

    def test_no_body_class(self):
        self.write_body("second", source="SCRIPT_VERSION = '2.0.0'\nX = 1\n")
        self.refused("defines no Body class")

    def test_a_body_that_does_not_derive_from_the_loader(self):
        self.write_body("second", source="SCRIPT_VERSION = '2.0.0'\n"
                                         "class Body(object):\n    pass\n")
        self.refused("does not derive from the loader")

    def test_a_body_that_needs_a_newer_loader(self):
        self.write_body("second", needs="99.0.0")
        reply = self.refused("needs loader v99.0.0")
        self.assertIn("restart Live", reply["message"])

    def test_a_body_that_needs_a_newer_loader_and_reaches_for_what_it_hands(self):
        """The real shape of a loader bump, and the reason the version is
        read out of the source rather than off the loaded module.

        A body written for a newer loader does not merely declare a newer
        `NEEDS_LOADER`: it *uses* what that loader hands over, so executing it
        first fails at the body's own import check — "Tick is missing" —
        which tells a producer nothing they can act on. Measured against a
        running Live 12.4.6 on 2026-09-21, before this check moved: the
        reload refused with exactly that, and the version the body declared
        never came into it.
        """
        self.write_body("second", needs="99.0.0", source=(
            'SCRIPT_VERSION = "1.0.0"\nNEEDS_LOADER = "99.0.0"\n'
            'SOMETHING_ONLY_A_NEWER_LOADER_HANDS_OVER\n'))
        reply = self.refused("needs loader v99.0.0")
        self.assertIn("restart Live", reply["message"])
        self.assertNotIn("NameError", reply["message"])

    def test_parameters_are_refused_because_nothing_crosses_the_socket(self):
        self.refused("takes no parameters", params={"path": "/tmp/evil.py"})

    def test_a_request_from_off_the_machine_is_refused(self):
        self.refused("loopback only", peer="10.0.0.7")

    def test_a_second_reload_while_one_is_running(self):
        first = Sink()
        self.write_body("second")
        self.script._begin_reload(first, "127.0.0.1", {})
        second = Sink()
        self.script._begin_reload(second, "127.0.0.1", {})
        self.assertEqual(second.payload["status"], "error")
        self.assertIn("already running", second.payload["message"])
        for _ in range(400):
            if first.payload is not None:
                break
            time.sleep(0.005)
            self.script.tick()
        self.assertEqual(first.payload["status"], "success")

    def test_a_failed_reload_leaves_the_next_one_free_to_run(self):
        self.write_body("second", source="def (:\n")
        self.reload()
        self.assertIsNone(self.script._reload)
        self.write_body("third")
        self.assertEqual(self.reload()["status"], "success")
        self.assertEqual(self.mark(), "third")


class TheReservedPath(unittest.TestCase):
    """AC9: the loader answers the reload before the body is consulted, so a
    body that cannot answer anything can still be replaced."""

    def setUp(self):
        self.ns = harness.load()

    def test_the_body_defines_neither_reserved_method(self):
        reserved = self.ns["Loader"].RESERVED_METHODS
        self.assertEqual(sorted(reserved), ["_client_start", "_process_command"])
        for name in reserved:
            self.assertNotIn(name, self.ns["Body"].__dict__,
                             "%s is the loader's: a body that defines it can make itself "
                             "unreplaceable" % name)

    def test_a_body_whose_dispatch_raises_can_still_be_replaced(self):
        script = harness.instance(self.ns)

        def always_raises(*args, **kwargs):
            raise RuntimeError("this body is broken")

        script._dispatch = always_raises
        sink = Sink()
        script._begin_reload(sink, "127.0.0.1", {})
        for _ in range(400):
            if sink.payload is not None:
                break
            time.sleep(0.005)
            script.tick()
        self.assertEqual(sink.payload["status"], "success", sink.payload)

    def test_a_loader_with_no_body_at_all_still_answers_and_still_reloads(self):
        script = harness.instance(self.ns, body=False)
        info = script._get_script_info()
        self.assertIsNone(info["script_version"])
        self.assertEqual(info["loader_version"], self.ns["LOADER_VERSION"])
        self.assertFalse(info["reload"]["body_loaded"])
        self.assertTrue(info["reload"]["supported"])
        self.assertIn("reload_body", info["capabilities"])
        # And every other command says why, rather than an AttributeError.
        sink = Sink()
        script._run_on_main("set_tempo", {"tempo": 130.0}, sink)
        self.assertEqual(sink.payload["status"], "error")
        self.assertIn("did not load", sink.payload["message"])


class WhatTheBodyIsHanded(unittest.TestCase):
    """The loader hands the body a short, named list, not its own namespace.

    A body that reaches for anything else has to say so in `BODY_EXPORTS` —
    which lives in the file that costs a restart, so the cost of widening the
    contract is paid where it is decided.
    """

    def setUp(self):
        self.ns = harness.load()

    def test_the_list_is_exactly_what_the_body_uses(self):
        self.assertEqual(
            sorted(self.ns["BODY_EXPORTS"]),
            ["DEFERRED", "Done", "LOG_EVERY_COMMAND", "Loader", "Tick",
             "_served_commands"])

    def test_the_body_checks_for_them_itself(self):
        """So a body loaded any other way fails at import, not at the first
        request — which is where it would be a producer's problem."""
        with io.open(harness.BODY, encoding="utf-8") as handle:
            source = handle.read()
        for name in self.ns["BODY_EXPORTS"]:
            self.assertIn('"%s"' % name, source.split("class Body(", 1)[0],
                          "%s is handed over but the body never checks for it" % name)

    def test_nothing_else_of_the_loaders_leaks_into_the_body(self):
        module = self.ns["_body_module"]
        for name in ("_Client", "_ClientSink", "_TickSampler", "_load_body_module",
                     "_body_path", "_body_class", "create_instance", "HOST",
                     "DEFAULT_PORT", "LOADER_VERSION", "LOADER_CAPABILITIES",
                     "_split_documents", "BODY_EXPORTS", "_PENDING_BODY"):
            self.assertNotIn(
                name, vars(module),
                "%s reached the body without being in BODY_EXPORTS" % name)


class TheCostAndTheTiming(ReloadCase):
    """AC12 and AC15: the file read is off Live's thread, the swap is on it
    and is one assignment, and the tick after it runs the new body."""

    def test_the_import_happens_off_the_main_thread(self):
        threads = []
        real = self.ns["_load_body_module"]

        def watched():
            threads.append(threading.current_thread())
            return real()

        self.ns["_load_body_module"].__globals__["_load_body_module"] = watched
        try:
            self.write_body("second")
            self.reload()
        finally:
            self.ns["_load_body_module"].__globals__["_load_body_module"] = real
        self.assertEqual(len(threads), 1)
        self.assertIsNot(threads[0], threading.current_thread(),
                         "the file was read on the thread that drives the tick")

    def test_the_main_thread_pays_for_the_swap_and_nothing_else(self):
        self.write_body("second")
        reply = self.reload()
        self.assertLess(reply["main_ms"], 1.0,
                        "the rebase should be one assignment: %s ms" % reply["main_ms"])
        self.assertLess(reply["main_ms"], self.script.SLOW_SLICE_MS)
        self.assertGreaterEqual(reply["result"]["import_ms"], 0.0)

    def test_at_most_one_already_armed_callback_runs_the_old_body(self):
        """A `schedule_message` closure holds the function it was armed with,
        so a callback armed before the swap runs the old body once; every
        tick after the swap looks its callback up on the instance and gets
        the new one. (Cycling '74's LOM reference makes no promise either
        way; this is what CPython does with a bound method, and
        `test_live_semantics.py` records it.)"""
        ran = []
        self.script.schedule_message(1, lambda: ran.append(self.mark()))
        self.write_body("second")
        self.reload()
        self.script.tick()
        self.assertEqual(self.mark(), "second")
        self.assertLessEqual(len(ran), 1)


class WhatAReloadCosts(ReloadCase):
    """AC16: what twenty reloads retain, measured rather than asserted about.

    A reload is a developer and update action, not something on a timer, and
    no file watcher was added: polling the body's mtime on a 100 ms tick would
    spend Live's main thread on a `stat` forever to save a command that is
    already free. The number this prints is the one
    `docs/architecture/overview.md` carries; the real-Live figure comes from
    `scripts/live-reload-probe.py`.
    """

    RELOADS = 20

    def live_generations(self):
        """Every body module still reachable, counted rather than assumed.

        A generation is kept alive by whatever still points at it: the class
        the instance wears, a generator still running on the old body, an
        armed callback. Nothing puts one in `sys.modules`, so when the last
        of those goes, so does the module.
        """
        import gc
        gc.collect()
        return [o for o in gc.get_objects()
                if isinstance(o, type(sys)) and getattr(o, "__name__", "").startswith(
                    "AbletonMusicMaker_body_")]

    def test_twenty_reloads_retain_one_generation_not_twenty(self):
        first = self.ns["_BODY_GENERATION"][0] + 1
        for i in range(self.RELOADS):
            self.write_body("gen%d" % i)
            self.assertEqual(self.reload()["status"], "success")
        # Only the generations this test made: other tests in this process
        # hold bodies of their own, and they are not what is being measured.
        mine = [m for m in self.live_generations()
                if int(m.__name__.rsplit("_", 1)[1]) >= first]
        sys.stderr.write(
            "\n  %d reloads: %d of the %d body modules are still reachable "
            "(python %s, measured %s)\n"
            % (self.RELOADS, len(mine), self.RELOADS, sys.version.split()[0],
               time.strftime("%Y-%m-%d")))
        # The one the instance is wearing, and no more: a generation nothing
        # points at is collected, so the cost of a reload does not accumulate.
        self.assertEqual(len(mine), 1, [m.__name__ for m in mine])


if __name__ == "__main__":
    unittest.main()
