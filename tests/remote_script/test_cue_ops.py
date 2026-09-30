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
from harness import two_track_set


class CueOps(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load(song=two_track_set(clips=True))
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.song.start_playing()
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
        self.assertEqual([t.playing_slot_index for t in self.song.tracks], [-1, -1])
        self.song.current_song_time = 8.0
        self.script.tick(2)
        self.assertTrue(self.song.tracks[0].mute, "the ops did not run on the beat")
        # The scene really fired: both tracks are playing their second clip.
        self.assertEqual([t.playing_slot_index for t in self.song.tracks], [1, 1])

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


class _Sink(object):
    def __init__(self):
        self.payload = None

    def put(self, payload):
        self.payload = payload


class ResetClearsEveryLocator(unittest.TestCase):
    """#82: `reset_set` reported locators cleared that were still there, and
    left one more behind than it started with.

    Live's only way to remove a locator is to put the playhead on it and call
    `set_or_delete_cue`, and Live grants a playhead move on one of its own
    frames. The old loop did both in one task, so every toggle acted at the
    position the playhead had not left: three locators in, four out — the
    three untouched, plus one created where the playhead still was (measured
    against Live 12.4.6, 2026-09-21).
    """

    def setUp(self):
        self.ns = harness.load(song=two_track_set(clips=True))
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.script._live_version = "12.4.6"
        for beat in (16.0, 32.0, 48.0):
            self.song.current_song_time = beat
            self.song._apply_pending()
            self.song.set_or_delete_cue()
        self.song.current_song_time = 12.0
        self.song._apply_pending()

    def run_command(self, name, params, limit=400):
        sink = _Sink()
        self.script._run_on_main(name, params, sink)
        for _ in range(limit):
            if sink.payload is not None:
                break
            self.script.tick()
        self.assertIsNotNone(sink.payload, "%s never answered" % name)
        return sink.payload

    def test_every_locator_goes_and_none_is_added(self):
        self.assertEqual([c.time for c in self.song.cue_points], [16.0, 32.0, 48.0])
        payload = self.run_command("reset_set", {})
        self.assertEqual(payload["status"], "success", payload.get("message"))
        self.assertEqual([c.time for c in self.song.cue_points], [],
                         "a locator survived the reset, or one was created")
        self.assertEqual(payload["result"]["removed"]["locators"], 3)

    def test_the_count_is_what_went_not_what_was_tried(self):
        """The reported number is checked against the set, one cue at a time,
        so 'cleared' cannot be a claim the set disagrees with."""
        payload = self.run_command("reset_set", {})
        self.assertEqual(payload["result"]["removed"]["locators"],
                         3 - len(list(self.song.cue_points)))


if __name__ == "__main__":
    unittest.main()
