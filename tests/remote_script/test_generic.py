"""Phase 3: describe and run reach Live's objects and nothing else.

The whitelist is the security model — the socket has no authentication and
loopback is the boundary — so every refusal below is a test, not a promise.
"""
import unittest

import harness


class FakeParam(object):
    def __init__(self, name, value):
        self.name = name
        self._value = value
        self.min = 0.0
        self.max = 1.0

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, v):
        self._value = v

    def str_for_value(self, v):
        return "%.1f" % v


class FakeMixer(object):
    def __init__(self):
        self.volume = FakeParam("Volume", 0.85)


class FakeSlot(object):
    def __init__(self):
        self.has_clip = False
        self.fired = 0

    def fire(self):
        self.fired += 1
        return None


class FakeTrack(object):
    def __init__(self, name):
        self.name = name
        self.mute = False
        self.solo = False
        self.arm = False
        self.playing_slot_index = -1
        self.mixer_device = FakeMixer()
        self.clip_slots = [FakeSlot(), FakeSlot()]

    @property
    def is_visible(self):        # read-only on purpose
        return True


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
    def setUp(self):
        self.ns = harness.load()
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.song.tracks = [FakeTrack("Kick"), FakeTrack("Bass")]
        self.script._live_version = "12.4.6"

    def test_it_names_the_class_attributes_and_methods(self):
        d = self.script._describe("song.tracks[0]")
        self.assertEqual(d["class"], "FakeTrack")
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


class Run(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()
        self.Done = self.ns["Done"]
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.song.tracks = [FakeTrack("Kick"), FakeTrack("Bass")]
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
        self.assertEqual(self.song.tracks[1].clip_slots[0].fired, 1)

    def test_only_named_results_come_back(self):
        out = self.run_ops([{"op": "get", "path": "song.tempo"}])
        self.assertEqual(out, {"ops": 1})

    def test_wait_tick_yields_without_touching_live(self):
        gen = self.script._run_ops({"ops": [{"op": "wait_tick", "ticks": 3},
                                            {"op": "get", "path": "song.tempo", "as": "t"}]})
        for _ in range(3):
            self.assertIsNone(next(gen))
        next(gen)                      # the get
        self.assertEqual(next(gen).result["t"], 120.0)

    def test_a_list_comes_back_as_values_not_objects(self):
        out = self.run_ops([{"op": "get", "path": "song.tracks", "as": "tracks"}])
        self.assertEqual(len(out["tracks"]), 2)
        self.assertTrue(all(isinstance(x, str) for x in out["tracks"]))

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
    would need, whatever a future edit adds."""

    def test_the_source_has_no_eval_exec_import_or_open_on_the_request_path(self):
        import io
        source = io.open(harness.SCRIPT, encoding="utf-8").read()
        body = source.split("class AbletonMCP(", 1)[1]
        for bad in ("eval(", "exec(", "compile(", "__import__", "importlib", "subprocess"):
            self.assertNotIn(bad, body, "%s is reachable from a request" % bad)
        # `open(` exists only in the two config readers, which run at import.
        self.assertNotIn("open(", body.split("def _drain_clients", 1)[1])


if __name__ == "__main__":
    unittest.main()
