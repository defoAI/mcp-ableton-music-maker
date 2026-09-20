"""The fake answers like Live, or this fails.

A fake nobody checks is a fake that drifts, and a drifted fake makes a green
suite a lie. `scripts/live-transcript.sh` runs a fixed script of commands
against a **real** Ableton Live and keeps every reply in
`tests/fixtures/live-transcript-<version>.json`. This replays the same
script against the fake and diffs the replies field by field, with an
explicit allow-list for what legitimately varies (`main_ms`, ids, timings,
absolute paths, the library's own uris, Live's choice of track colour).

A difference outside that list fails here.

**With no fixture checked in, `the_fake_answers_like_live` skips and says
so.** It does not pass. The fixture has to be recorded against a real Live,
and until someone does, this check is not a check — which is better said out
loud than hidden behind a green tick.

The rest of the file tests the machinery itself, against the fake on both
sides, so a broken differential cannot masquerade as agreement.
"""
import glob
import io
import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "scripts"))

from test_fake_live_server import Fake  # noqa: E402

FIXTURES = os.path.join(ROOT, "tests", "fixtures")


def load_transcript_module():
    import importlib.util
    spec = importlib.util.spec_from_file_location(
        "live_transcript", os.path.join(ROOT, "scripts", "live-transcript.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def real_live_fixtures():
    return sorted(glob.glob(os.path.join(FIXTURES, "live-transcript-*.json")))


def record_against_the_fake(prefix="T", args=("--shared-set", "--latency", "none", "--quiet")):
    transcript = load_transcript_module()
    with Fake(*args) as fake:
        return transcript.record("127.0.0.1", fake.port, prefix)


class TheDifferential(unittest.TestCase):
    """The machinery, checked before it is trusted."""

    def setUp(self):
        self.transcript = load_transcript_module()

    def test_the_script_covers_the_commands_a_session_really_uses(self):
        names = [c for _, c, _ in self.transcript.the_script(("K", "B", "V"))]
        for wanted in ("get_script_info", "create_tracks", "write_clips", "get_clip_notes",
                       "place_clips", "set_track_mixer", "set_device_parameter",
                       "get_session_snapshot", "describe", "run"):
            self.assertIn(wanted, names)
        # And the failures, because how Live refuses is part of the contract.
        refusals = [(l, c, p) for l, c, p in self.transcript.the_script(("K", "B", "V"))
                    if "cannot do" in l]
        self.assertTrue(refusals, "the script never asks Live for something it will refuse")

    def test_a_transcript_of_the_fake_against_itself_is_clean(self):
        a = record_against_the_fake("A")
        b = record_against_the_fake("A")
        lines = self.transcript.diff(a, b)
        self.assertEqual(lines, [], "the fake disagrees with itself:\n  " + "\n  ".join(lines))

    def test_main_ms_and_ids_are_allowed_to_vary_and_a_value_is_not(self):
        a = record_against_the_fake("A")
        b = json.loads(json.dumps(a))
        b["steps"][1]["reply"]["main_ms"] = 999.0
        b["steps"][1]["reply"]["id"] = 4242
        self.assertEqual(self.transcript.diff(a, b), [],
                         "main_ms and id must be allowed to vary")
        b["steps"][1]["reply"]["result"]["tempo"] = 77.0
        lines = self.transcript.diff(a, b)
        self.assertTrue(any("tempo" in line for line in lines), lines)

    def test_a_status_that_differs_is_reported_first_and_plainly(self):
        a = record_against_the_fake("A")
        b = json.loads(json.dumps(a))
        b["steps"][1]["reply"] = {"status": "error", "message": "Live went away"}
        lines = self.transcript.diff(a, b)
        self.assertTrue(any("Live said success, the fake said error" in line for line in lines),
                        lines)

    def test_a_missing_field_is_a_difference(self):
        a = record_against_the_fake("A")
        b = json.loads(json.dumps(a))
        del b["steps"][1]["reply"]["result"]["track_count"]
        lines = self.transcript.diff(a, b)
        self.assertTrue(any("track_count" in line for line in lines), lines)

    def test_the_fake_builds_what_the_script_asks_for(self):
        """The transcript is only worth diffing if the script did its work."""
        t = record_against_the_fake("A")
        by_command = {}
        for step in t["steps"]:
            by_command.setdefault(step["command"], []).append(step)
        created = by_command["create_tracks"][0]["reply"]["result"]["created"]
        self.assertEqual([c["name"] for c in created], ["A Kick", "A Bass", "A Vox"])
        notes = by_command["get_clip_notes"][0]["reply"]["result"]["notes"]
        self.assertEqual(sorted(n["pitch"] for n in notes), [36, 36, 36, 36])
        clips = by_command["get_arrangement_clips"][0]["reply"]["result"]["clips"]
        self.assertEqual([c["start_time"] for c in clips], [0.0, 4.0, 8.0, 12.0])
        # And the two refusals really were refused.
        refused = [s for s in t["steps"] if s["reply"].get("status") == "error"]
        self.assertEqual(len(refused), 2, [s["command"] for s in refused])


class AgainstRealLive(unittest.TestCase):
    def test_the_fake_answers_like_live(self):
        fixtures = real_live_fixtures()
        if not fixtures:
            self.skipTest(
                "NO REAL-LIVE TRANSCRIPT CHECKED IN. This check cannot run until someone "
                "records one: open Live with the Remote Script loaded, on a scratch set, "
                "and run `scripts/live-transcript.sh`. Until then the fake is unverified "
                "against Live — see #53 section E.")
        transcript = load_transcript_module()
        for path in fixtures:
            with io.open(path, encoding="utf-8") as handle:
                real = json.load(handle)
            with self.subTest(fixture=os.path.basename(path)):
                fake = record_against_the_fake(real.get("track_prefix", "T"))
                lines = transcript.diff(real, fake)
                self.assertEqual(
                    lines, [],
                    "the fake answers differently from Live %s (script %s):\n  %s"
                    % (real.get("live_version"), real.get("script_version"),
                       "\n  ".join(lines)))

    def test_a_fixture_is_stamped_with_the_live_and_script_it_came_from(self):
        for path in real_live_fixtures():
            with io.open(path, encoding="utf-8") as handle:
                real = json.load(handle)
            with self.subTest(fixture=os.path.basename(path)):
                for field in ("recorded_at", "live_version", "script_version",
                              "protocol_version", "socket_reader"):
                    self.assertTrue(real.get(field), "%s has no %s" % (path, field))

    def test_the_fixture_matches_the_script_version_the_binary_embeds(self):
        """A transcript recorded against an older script is a transcript of a
        different contract. It does not silently keep passing."""
        script = os.path.join(ROOT, "AbletonMusicMaker_Remote_Script", "__init__.py")
        with io.open(script, encoding="utf-8") as handle:
            for line in handle:
                if line.startswith("SCRIPT_VERSION"):
                    current = line.split('"')[1]
                    break
        for path in real_live_fixtures():
            with io.open(path, encoding="utf-8") as handle:
                real = json.load(handle)
            with self.subTest(fixture=os.path.basename(path)):
                self.assertEqual(
                    real.get("script_version"), current,
                    "%s was recorded against script %s; the binary embeds %s. Re-record it "
                    "with scripts/live-transcript.sh against a real Live."
                    % (os.path.basename(path), real.get("script_version"), current))


class ReadAfterWrite(unittest.TestCase):
    """Two readings of the same state agree.

    For every command that both writes and reads, the read reflects the
    write. On the fake here; in the transcript against a real Live.
    """

    def setUp(self):
        self.fake = Fake("--shared-set", "--latency", "none", "--quiet")
        self.client = self.fake.connect()
        created = self.ask("create_tracks", {"tracks": [
            {"name": "Kick", "kind": "midi", "instrument_uri": "query:Synths#Analog"},
        ]})["result"]["created"]
        self.track = created[0]["index"]

    def tearDown(self):
        self.client.close()
        self.fake.stop()

    def ask(self, command, params=None):
        reply = self.client.ask(command, params or {})
        self.assertEqual(reply.get("status"), "success",
                         "%s: %s" % (command, reply.get("message")))
        return reply

    def test_set_tempo_is_what_get_session_info_says(self):
        self.ask("set_tempo", {"tempo": 137.5})
        self.assertEqual(self.ask("get_session_info")["result"]["tempo"], 137.5)

    def test_set_track_mixer_is_what_get_track_info_says(self):
        self.ask("set_track_mixer", {"track_index": self.track, "volume_db": -6.0})
        info = self.ask("get_track_info", {"track_index": self.track})["result"]
        db = info.get("volume_db")
        if db is None:
            db = (info.get("mixer") or {}).get("volume_db")
        self.assertIsNotNone(db, info)
        self.assertAlmostEqual(db, -6.0, places=1)

    def test_add_notes_to_clip_is_what_get_clip_notes_says(self):
        self.ask("create_clip", {"track_index": self.track, "clip_index": 0, "length": 4.0})
        written = [{"pitch": 36, "start_time": 0.0, "duration": 0.25, "velocity": 100},
                   {"pitch": 38, "start_time": 1.5, "duration": 0.5, "velocity": 80}]
        self.ask("add_notes_to_clip",
                 {"track_index": self.track, "clip_index": 0, "notes": written})
        read = self.ask("get_clip_notes",
                        {"track_index": self.track, "clip_index": 0})["result"]["notes"]
        self.assertEqual(sorted((n["pitch"], n["start_time"], n["duration"]) for n in read),
                         sorted((n["pitch"], n["start_time"], n["duration"]) for n in written))

    def test_place_clips_is_what_arrangement_summary_says(self):
        self.ask("create_clip", {"track_index": self.track, "clip_index": 0, "length": 4.0})
        self.ask("place_clips",
                 {"track_index": self.track, "clip_index": 0, "times": [0.0, 4.0, 8.0]})
        clips = self.ask("get_arrangement_clips",
                         {"track_index": self.track})["result"]["clips"]
        self.assertEqual([c["start_time"] for c in clips], [0.0, 4.0, 8.0])
        summary = self.ask("arrangement_summary")["result"]
        self.assertEqual(summary["clips"], 3)
        self.assertEqual(summary["end_beat"], 12.0)

    def test_set_device_parameter_is_what_get_device_parameters_says(self):
        self.ask("set_device_parameter", {"track_index": self.track, "device_index": 0,
                                          "parameter_index": 1, "value": 0.25})
        device = self.ask("get_device_parameters",
                          {"track_index": self.track, "device_index": 0})["result"]["device"]
        self.assertAlmostEqual(device["parameters"][1]["value"], 0.25)

    def test_set_clip_loop_is_what_get_clip_info_says(self):
        self.ask("create_clip", {"track_index": self.track, "clip_index": 0, "length": 4.0})
        self.ask("set_clip_loop", {"track_index": self.track, "clip_index": 0,
                                   "looping": True, "loop_end": 8.0})
        info = self.ask("get_clip_info",
                        {"track_index": self.track, "clip_index": 0})["result"]
        self.assertEqual((info["looping"], info["loop_end"]), (True, 8.0))

    def test_create_scene_is_what_get_performance_state_says(self):
        self.ask("create_scene", {"index": -1, "name": "Drop", "phrase_bars": 8})
        scenes = self.ask("get_performance_state")["result"]["scenes"]
        self.assertTrue(any(s["name"].startswith("Drop") for s in scenes),
                        [s["name"] for s in scenes])

    def test_set_scale_is_what_get_performance_state_says(self):
        self.ask("set_scale", {"root_note": 2, "scale_name": "Minor"})
        state = self.ask("get_performance_state")["result"]
        self.assertEqual((state["root_note"], state["scale_name"]), (2, "Minor"))


if __name__ == "__main__":
    unittest.main()
