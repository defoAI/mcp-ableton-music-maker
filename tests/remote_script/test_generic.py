"""Phase 3: describe and run reach Live's objects and nothing else.

The whitelist is the security model — the socket has no authentication and
loopback is the boundary — so every refusal below is a test, not a promise.
"""
import io
import re
import unittest

import harness
from harness import two_track_set


class Paths(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()
        self.parse = self.ns["_parse_path"]

    def test_a_path_is_names_and_indexes(self):
        self.assertEqual(self.parse("song.tracks[3].mixer_device.volume.value"),
                         [("attr", "song"), ("attr", "tracks"), ("index", 3),
                          ("attr", "mixer_device"), ("attr", "volume"), ("attr", "value")])

    def test_the_three_roots_and_no_others(self):
        for root in ("song", "application", "browser"):
            self.parse(root + ".x")
        for bad in ("os.environ", "sys.modules", "__builtins__", "self.song", "Live.Song"):
            with self.assertRaises(ValueError) as e:
                self.parse(bad)
            self.assertIn("must start with", str(e.exception))

    def test_underscore_names_are_refused_which_covers_every_dunder(self):
        for bad in ("song.__class__", "song.tracks[0].__dict__", "song._private",
                    "song.__class__.__mro__", "song.tracks[0].__class__.__init__.__globals__"):
            with self.assertRaises(ValueError) as e:
                self.parse(bad)
            self.assertIn("not reachable", str(e.exception))

    def test_a_malformed_path_is_refused_not_guessed(self):
        for bad, why in [("", "required"), ("song..tracks", "empty name"),
                         ("song.tracks[", "unclosed"), ("song.tracks[x]", "whole number"),
                         ("[0].song", "starts with a name")]:
            with self.assertRaises(ValueError) as e:
                self.parse(bad)
            self.assertIn(why, str(e.exception))

    def test_a_path_cannot_be_unbounded(self):
        with self.assertRaises(ValueError) as e:
            self.parse("song" + ".a" * 40)
        self.assertIn("at most", str(e.exception))


class Describe(unittest.TestCase):
    """`readonly` is exactly Live's, because the model's members are real
    `property` descriptors and `_describe` derives it from `fset`."""

    def setUp(self):
        self.ns = harness.load(song=two_track_set(clips=True))
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.script._live_version = "12.4.6"

    def test_it_names_the_class_attributes_and_methods(self):
        d = self.script._describe("song.tracks[0]")
        self.assertEqual(d["class"], "Track")
        self.assertEqual(d["name"], "Kick")
        self.assertEqual(d["live_version"], "12.4.6")
        self.assertEqual(d["attrs"]["name"]["type"], "str")
        self.assertEqual(d["attrs"]["mute"]["type"], "bool")
        self.assertIn("mixer_device", d["attrs"])
        self.assertIn("clip_slots", d["attrs"])

    def test_read_only_is_reported_so_rust_need_not_guess(self):
        d = self.script._describe("song.tracks[0]")
        self.assertTrue(d["attrs"]["is_visible"]["readonly"], "a property with no setter")
        d = self.script._describe("song.tracks[0].mixer_device.volume")
        self.assertFalse(d["attrs"]["value"]["readonly"], "a property with a setter")

    def test_methods_are_listed_apart_from_attributes(self):
        d = self.script._describe("song.tracks[0].clip_slots[0]")
        self.assertIn("fire", d["methods"])
        self.assertNotIn("fire", d["attrs"])

    def test_no_underscore_name_is_ever_listed(self):
        d = self.script._describe("song.tracks[0]")
        self.assertFalse([k for k in d["attrs"] if k.startswith("_")])
        self.assertFalse([m for m in d["methods"] if m.startswith("_")])

    def test_an_index_out_of_range_says_so(self):
        with self.assertRaises(ValueError) as e:
            self.script._describe("song.tracks[9]")
        self.assertIn("out of range", str(e.exception))

    def test_an_attribute_this_live_lacks_names_the_version(self):
        with self.assertRaises(ValueError) as e:
            self.script._describe("song.tracks[0].no_such_thing")
        self.assertIn("12.4.6", str(e.exception))


class _Sink(object):
    """Where a reply lands when the executor answers, as a client's outbound
    buffer would."""

    def __init__(self):
        self.payload = None

    def put(self, payload):
        self.payload = payload


class Run(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load(song=two_track_set(clips=True))
        self.Done = self.ns["Done"]
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.script._live_version = "12.4.6"

    def run_ops(self, ops):
        """Drive the generator the way the executor does; returns the result
        or raises what the executor would surface."""
        gen = self.script._run_ops({"ops": ops})
        while True:
            item = next(gen)
            if isinstance(item, self.Done):
                return item.result

    def test_get_set_and_call_in_one_batch(self):
        out = self.run_ops([
            {"op": "get", "path": "song.tempo", "as": "tempo"},
            {"op": "set", "path": "song.tracks[1].mixer_device.volume.value", "value": 0.5, "as": "vol"},
            {"op": "call", "path": "song.tracks[1].clip_slots[0].fire", "args": []},
            {"op": "get", "path": "song.tracks[1].name", "as": "who"},
        ])
        self.assertEqual(out["tempo"], 120.0)
        self.assertEqual(out["vol"], 0.5)
        self.assertEqual(out["who"], "Bass")
        self.assertEqual(out["ops"], 4)
        self.assertEqual(self.song.tracks[1].mixer_device.volume.value, 0.5)
        # The slot really fired: Bass is playing its first clip.
        self.assertEqual(self.song.tracks[1].playing_slot_index, 0)
        self.assertTrue(self.song.tracks[1].clip_slots[0].clip.is_playing)

    def test_only_named_results_come_back(self):
        out = self.run_ops([{"op": "get", "path": "song.tempo"}])
        self.assertEqual(out, {"ops": 1})

    def test_wait_tick_asks_the_executor_for_a_real_wait(self):
        """Not a bare yield. A bare `yield None` is a *permission* to slice,
        and the executor takes it only when the slice budget is spent — so a
        `wait_tick` that cost nothing was resumed on the very next line,
        inside the same tick, and waited for nothing at all (#87). Measured
        against Live 12.4.6 before the fix: `wait_tick 8`, which is 800 ms of
        Live, returned in 199 ms — one round trip — reporting one slice."""
        Tick = self.ns["Tick"]
        gen = self.script._run_ops({"ops": [{"op": "wait_tick", "ticks": 3},
                                            {"op": "get", "path": "song.tempo", "as": "t"}]})
        waited = next(gen)
        self.assertIsInstance(waited, Tick)
        self.assertEqual(waited.ticks, 3)
        next(gen)                      # the get
        self.assertEqual(next(gen).result["t"], 120.0)

    def test_the_wait_really_crosses_lives_ticks(self):
        """Through the executor, not the generator: the task comes back on a
        later tick, and a playhead move asked for before the wait is readable
        after it. That pair is the whole point of the op."""
        sink = _Sink()
        ticks_before = self.script.now_tick
        self.script._run_on_main("run", {"ops": [
            {"op": "set", "path": "song.current_song_time", "value": 16.0},
            {"op": "get", "path": "song.current_song_time", "as": "straight_back"},
            {"op": "wait_tick", "ticks": 2},
            {"op": "get", "path": "song.current_song_time", "as": "after_the_wait"},
        ]}, sink)
        for _ in range(20):
            if sink.payload is not None:
                break
            self.script.tick()
        self.assertIsNotNone(sink.payload, "the batch never answered")
        self.assertEqual(sink.payload["status"], "success", sink.payload.get("message"))
        out = sink.payload["result"]
        # Live has not granted the move yet, so the read in the same slice
        # still answers where the playhead was.
        self.assertEqual(out["straight_back"], 0.0)
        # After the wait it has.
        self.assertEqual(out["after_the_wait"], 16.0)
        self.assertGreaterEqual(self.script.now_tick - ticks_before, 2,
                                "the batch answered without Live's frames passing")
        self.assertGreaterEqual(sink.payload["slices"], 2,
                                "a wait that did not cost a slice did not happen")

    def test_a_batch_cannot_wait_without_bound(self):
        gen = self.script._run_ops({"ops": [{"op": "wait_tick", "ticks": 32},
                                            {"op": "wait_tick", "ticks": 32}]})
        next(gen)
        with self.assertRaises(ValueError) as e:
            next(gen)
        self.assertIn("at most 32 ticks of waiting", str(e.exception))

    def test_getting_a_whole_sequence_gives_its_contents(self):
        """A Live collection reads as a sequence, so it can be counted.

        `song.tracks` is a `Base.Vector` — neither a `list` nor a `tuple`.
        Until script 1.35.0 `_jsonable` tested only for those two, so a
        whole-collection read came back as the object's repr: verified
        against Live 12.4.6 on 2026-09-20, `run get song.tracks` returned
        `"<Base.Vector object at 0x1656f6fc0>"`. The caller could read a
        member by index but never learn how many there were, which is what
        made every locator name unresolvable (#66). A Vector is a sequence
        and is now rendered as one, capped like any other at 256."""
        out = self.run_ops([{"op": "get", "path": "song.tracks", "as": "tracks"}])
        self.assertIsInstance(out["tracks"], list)
        self.assertEqual(len(out["tracks"]), len(list(self.song.tracks)))

    def test_an_empty_collection_is_an_empty_list_not_a_repr(self):
        """The distinction the locator lookup turns on: a set with no
        locators and a read that failed must not look the same."""
        out = self.run_ops([{"op": "get", "path": "song.cue_points", "as": "cues"}])
        self.assertEqual(out["cues"], [])

    def test_a_string_is_not_split_into_characters(self):
        """A string is a sequence too, and must stay whole."""
        self.song._file_path = "/Users/p/Music/Smoke Project/Smoke.als"
        out = self.run_ops([{"op": "get", "path": "song.file_path", "as": "set"}])
        self.assertEqual(out["set"], "/Users/p/Music/Smoke Project/Smoke.als")

    def test_a_member_of_a_sequence_comes_back_as_a_value(self):
        out = self.run_ops([{"op": "get", "path": "song.tracks[0].name", "as": "first"},
                            {"op": "get", "path": "song.tracks[1].name", "as": "second"}])
        self.assertTrue(all(isinstance(v, str) for v in (out["first"], out["second"])))

    def test_the_failing_op_is_named(self):
        with self.assertRaises(ValueError) as e:
            self.run_ops([{"op": "get", "path": "song.tempo", "as": "a"},
                          {"op": "get", "path": "song.tracks[7].name", "as": "b"}])
        self.assertIn("op 1", str(e.exception))
        self.assertIn("out of range", str(e.exception))

    def test_a_read_only_set_says_so_with_the_live_version(self):
        with self.assertRaises(ValueError) as e:
            self.run_ops([{"op": "set", "path": "song.tracks[0].is_visible", "value": False}])
        self.assertIn("read-only", str(e.exception))
        self.assertIn("12.4.6", str(e.exception))

    def test_calling_something_that_is_not_a_method_is_refused(self):
        with self.assertRaises(ValueError) as e:
            self.run_ops([{"op": "call", "path": "song.tracks[0].name", "args": []}])
        self.assertIn("not a method", str(e.exception))

    def test_every_way_out_of_live_is_refused(self):
        for path in ("os.environ", "song.__class__", "song.tracks[0].__dict__",
                     "song.tracks[0].__class__.__mro__", "sys.exit", "__builtins__.eval"):
            with self.assertRaises(ValueError):
                self.run_ops([{"op": "get", "path": path}])

    def test_an_unknown_op_is_refused_with_the_known_ones(self):
        with self.assertRaises(ValueError) as e:
            self.run_ops([{"op": "exec", "path": "song.tempo"}])
        self.assertIn("unknown op", str(e.exception))
        self.assertIn("wait_tick", str(e.exception))

    def test_an_empty_or_oversized_batch_is_refused(self):
        with self.assertRaises(ValueError):
            self.run_ops([])
        with self.assertRaises(ValueError) as e:
            self.run_ops([{"op": "get", "path": "song.tempo"}] * 600)
        self.assertIn("at most", str(e.exception))

    def test_ops_before_a_failure_have_already_happened(self):
        # The batch is not a transaction: it stops, and says where.
        with self.assertRaises(ValueError):
            self.run_ops([
                {"op": "set", "path": "song.tracks[0].mixer_device.volume.value", "value": 0.25},
                {"op": "set", "path": "song.tracks[0].is_visible", "value": False},
            ])
        self.assertEqual(self.song.tracks[0].mixer_device.volume.value, 0.25)


class NoEscapeHatches(unittest.TestCase):
    """The script's request path must not contain the tools an attacker
    would need, whatever a future edit adds.

    Both files are scanned. The reload (decision 0011) did not widen this:
    `reload_body` carries no path and no source, and the one function that
    reads a file and imports by path is named below and nowhere else. The
    body — every handler a request can reach — has none of them at all.
    """

    # The loader's one importer. It takes no argument, reads `body.py` beside
    # itself and is reached only through `_begin_reload`, which refuses
    # parameters and refuses anything but loopback.
    THE_ONE_LOADER_FUNCTION = "_load_body_module"

    def read(self, path):
        with io.open(path, encoding="utf-8") as handle:
            return handle.read()

    def test_the_body_has_no_eval_exec_import_or_open_anywhere(self):
        body = self.read(harness.BODY)
        for bad in ("eval(", "exec(", "compile(", "__import__", "importlib",
                    "subprocess", "open("):
            self.assertNotIn(bad, body, "%s is reachable from a request" % bad)

    def test_the_loader_has_them_only_where_it_loads_the_body(self):
        source = self.read(harness.SCRIPT)
        allowed = ("_configured_host", "_configured_reader", "_served_commands",
                   self.THE_ONE_LOADER_FUNCTION)
        current, offenders = "", []
        for i, line in enumerate(source.split("\n"), 1):
            code = line.split("#")[0]
            stripped = code.strip()
            if stripped.startswith("def "):
                current = stripped[4:].split("(")[0]
            for bad in ("eval(", "exec(", "compile(", "__import__", "subprocess"):
                if bad in code and not re.search(r"[\w.]" + re.escape(bad), code):
                    offenders.append("line %d: %s in %s" % (i, bad, current))
            if "open(" in code and current not in allowed:
                offenders.append("line %d: open( in %s" % (i, current))
            if "importlib" in code and current not in ("", self.THE_ONE_LOADER_FUNCTION):
                offenders.append("line %d: importlib in %s" % (i, current))
        self.assertEqual(offenders, [], "the loader reaches Live and one file beside it")

    def test_the_request_path_never_reaches_the_importer(self):
        """`reload_body` is the only way in, and it takes no arguments."""
        script = harness.instance(harness.load())
        answers = []

        class Sink(object):
            def put(self, payload):
                answers.append(payload)

        script._begin_reload(Sink(), "127.0.0.1", {"path": "/etc/passwd"})
        self.assertEqual(answers[0]["status"], "error")
        self.assertIn("takes no parameters", answers[0]["message"])
        del answers[:]
        script._begin_reload(Sink(), "10.0.0.7", {})
        self.assertEqual(answers[0]["status"], "error")
        self.assertIn("loopback only", answers[0]["message"])


if __name__ == "__main__":
    unittest.main()
