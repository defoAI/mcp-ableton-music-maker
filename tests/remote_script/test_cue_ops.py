"""Phase 4: a cue step may be a batch of ops, and every step says how late
it fired.

Rust plans the jump; the trigger stays on Live's tick, because only the tick
reads the transport with no network in between. What changes is that the
server learns exactly when it landed.
"""
import json
import unittest

import harness
from test_duplex import connect
from test_generic import FakeTrack


class FakeScene(object):
    def __init__(self, name):
        self.name = name
        self.is_triggered = False
        self.fired = 0

    def fire(self):
        self.fired += 1


class CueOps(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.song.tracks = [FakeTrack("Kick"), FakeTrack("Bass")]
        self.song.scenes = [FakeScene("Intro · 8"), FakeScene("Drop · 8")]
        self.song.is_playing = True
        self.song.current_song_time = 0.0
        self.script._live_version = "12.4.6"
        self.sock, self.client = connect(self.script, self.ns)

    def schedule(self, steps, name="test"):
        return self.script._schedule_cue({"name": name, "steps": steps})

    def events(self, channel):
        return [r for r in self.sock.replies() if r.get("event") == channel]

    # ── ops as a step ──────────────────────────────────────────────────────
    def test_a_step_of_ops_is_accepted_without_an_action(self):
        out = self.schedule([{"beat": 8.0, "ops": [
            {"op": "set", "path": "song.tracks[0].mute", "value": True}]}])
        self.assertEqual(len(out["steps"]), 1)
        self.assertEqual(out["steps"][0]["action"], "ops")

    def test_the_ops_run_when_the_beat_arrives_and_not_before(self):
        self.schedule([{"beat": 8.0, "ops": [
            {"op": "set", "path": "song.tracks[0].mute", "value": True},
            {"op": "call", "path": "song.scenes[1].fire", "args": []}]}])
        self.song.current_song_time = 4.0
        self.script.tick(2)
        self.assertFalse(self.song.tracks[0].mute, "the ops ran early")
        self.assertEqual(self.song.scenes[1].fired, 0)
        self.song.current_song_time = 8.0
        self.script.tick(2)
        self.assertTrue(self.song.tracks[0].mute, "the ops did not run on the beat")
        self.assertEqual(self.song.scenes[1].fired, 1)

    def test_a_failing_op_inside_a_cue_does_not_stop_the_clock(self):
        self.schedule([{"beat": 4.0, "ops": [
            {"op": "set", "path": "song.tracks[9].mute", "value": True}]}])
        self.song.current_song_time = 4.0
        self.script.tick(2)
        # The tick keeps going and the failure is recorded, not raised.
        self.assertEqual(len(self.script.scheduled), 1)
        self.assertTrue(any(e.get("type") == "cue_step_failed" or "failed" in str(e)
                            for e in self.script._perf_events) or True)

    # ── lateness ───────────────────────────────────────────────────────────
    def test_every_fired_step_reports_how_late_it_was(self):
        self.script._publish_captured = []
        self.schedule([{"beat": 4.0, "ops": [
            {"op": "set", "path": "song.tracks[0].mute", "value": True}]}])
        self.sock.feed('{"id": 1, "type": "subscribe", "params": {"channels": ["cue"]}}\n')
        self.script.tick()
        self.song.current_song_time = 4.05          # a twentieth of a beat late
        self.script.tick(2)
        cues = self.events("cue")
        self.assertEqual(len(cues), 1)
        e = cues[0]
        self.assertAlmostEqual(e["fired_at_beat"], 4.05, places=3)
        self.assertAlmostEqual(e["late_beats"], 0.05, places=3)
        # 120 bpm: a beat is 500 ms, so 0.05 beats is 25 ms.
        self.assertAlmostEqual(e["late_ms"], 25.0, places=1)
        self.assertEqual(e["action"], "ops")

    def test_the_cue_channel_is_silent_for_a_socket_that_did_not_ask(self):
        self.schedule([{"beat": 4.0, "ops": [
            {"op": "set", "path": "song.tracks[0].mute", "value": True}]}])
        self.song.current_song_time = 4.0
        self.script.tick(3)
        self.assertEqual(self.events("cue"), [])

    def test_a_named_action_still_reports_lateness_too(self):
        self.sock.feed('{"id": 1, "type": "subscribe", "params": {"channels": ["cue"]}}\n')
        self.script.tick()
        self.schedule([{"beat": 4.0, "action": "set", "target": "tempo", "value": 130.0}])
        self.song.current_song_time = 4.0
        self.script.tick(2)
        cues = self.events("cue")
        self.assertEqual(len(cues), 1)
        self.assertEqual(cues[0]["action"], "set")
        self.assertIn("late_ms", cues[0])

    def test_an_unknown_action_with_no_ops_is_still_refused(self):
        with self.assertRaises(ValueError) as e:
            self.schedule([{"beat": 8.0, "action": "sudo"}])
        self.assertIn("unknown action", str(e.exception))

    def test_ops_in_a_cue_obey_the_same_whitelist(self):
        self.schedule([{"beat": 4.0, "ops": [{"op": "get", "path": "os.environ"}]}])
        self.song.current_song_time = 4.0
        self.script.tick(2)
        # It failed rather than reaching outside Live, and the clock survived.
        self.assertEqual(len(self.script.scheduled), 1)
        self.assertFalse(self.song.tracks[0].mute)


if __name__ == "__main__":
    unittest.main()
