"""Starting the transport where it is asked, captures that record the
Arrangement, and clip envelopes read back as written.

From a studio report (2026-10-15, Live 12.4.6), every one of them reproduced
against a real Live before it was fixed:

  1. set_arrangement_time to bar 49, then start_playback, played bar 1:
     `start_playing()` plays from the start marker.
  2. capture_mix from a mid-record bar played the intro first: the seek was
     made while stopped and `continue_playing()` ignores it.
  3. after a capture, Live's Back to Arrangement stayed lit: the Capture
     track's fired slot took it off the Arrangement and nothing put it back.
  4. a full Capture track ended a run of captures.
  5. a clip envelope written from 0.0 read the fader's value at time 0:
     `value_at_time` answers the value before a breakpoint.

The model carries what was measured (fake_live: Song.start_playing,
continue_playing, the quantized seek, Track.back_to_arranger,
AutomationEnvelope.value_at_time); these hold the script to it.
"""
import unittest

import harness
import fake_live
from test_duplex import connect
from test_every_command import Sink

BAR = 4.0


def long_set(scenes=8):
    """Three tracks and a locator at bar 69, so the Arrangement reaches past
    bar 49 the way a record does."""
    song = fake_live.Song(scenes=scenes)
    for name in ("Kick", "Pad", "Lead"):
        song.create_midi_track()
        song.tracks[-1].name = name
    song.create_return_track()
    song._cue_points.append(fake_live.CuePoint(song, "Outro", 68 * BAR))
    for t in song.tracks:
        t.clip_slots[0].create_clip(16.0)
    return song


class Base(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load(song=long_set())
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.sock, self.client = connect(self.script, self.ns)

    def command(self, name, params=None, limit=400):
        sink = Sink()
        self.script._run_on_main(name, params or {}, sink)
        for _ in range(limit):
            if sink.payload is not None:
                break
            self.script.tick()
        self.assertIsNotNone(sink.payload, "%s never answered" % name)
        return sink.payload

    def ok(self, name, params=None):
        reply = self.command(name, params)
        self.assertEqual(reply.get("status"), "success", reply)
        return reply["result"]

    def stop_somewhere_else(self, beat):
        """The transport stopped at `beat`: where continue_playing resumes."""
        self.ok("play_from", {"time": beat})
        self.script.tick(3)
        self.ok("stop_playback")


class StartPlayback(Base):
    """Issue 1: the playhead is where the artist hears from."""

    def assert_plays_from(self, beat, reply):
        self.assertTrue(self.song.is_playing)
        self.assertAlmostEqual(reply["at"], beat, delta=0.3)
        self.script.tick(2)  # 200 ms on Live's tick
        now = self.song.current_song_time
        self.assertGreaterEqual(now, beat)
        self.assertLess(now, beat + 1.0)

    def test_set_time_then_start_plays_from_that_bar_when_stopped(self):
        for bar in (1, 49, 70):  # 70 is past the last locator
            with self.subTest(bar=bar):
                self.stop_somewhere_else(17.0)
                beat = (bar - 1) * BAR
                self.ok("set_current_song_time", {"time": beat})
                self.assert_plays_from(beat, self.ok("start_playback"))
                self.ok("stop_playback")

    def test_set_time_while_playing_then_start_keeps_playing_from_the_new_bar(self):
        for bar in (1, 49, 70):
            with self.subTest(bar=bar):
                self.ok("play_from", {"time": 40.0})
                self.script.tick(3)
                beat = (bar - 1) * BAR
                # While playing, the move goes through play_from's primitive:
                # a plain seek would snap to the 1-bar grid, as Live does.
                reply = self.ok("play_from", {"time": beat})
                self.assert_plays_from(beat, reply)
                started = self.ok("start_playback")
                self.assertTrue(started["already_playing"])
                self.ok("stop_playback")

    def test_off_the_bar_lands_exactly_and_quantization_is_put_back(self):
        self.song.clip_trigger_quantization = fake_live.Q_BAR
        self.stop_somewhere_else(17.0)
        reply = self.ok("play_from", {"time": 194.5})
        self.assertAlmostEqual(reply["at"], 194.5, places=3)
        self.assertEqual(int(self.song.clip_trigger_quantization), fake_live.Q_BAR)

    def test_past_the_end_fails_says_why_and_changes_nothing(self):
        self.song.clip_trigger_quantization = fake_live.Q_BAR
        reply = self.command("play_from", {"time": 4000.0})
        self.assertEqual(reply["status"], "error")
        self.assertIn("past the end", reply["message"])
        self.assertFalse(self.song.is_playing)
        self.assertEqual(int(self.song.clip_trigger_quantization), fake_live.Q_BAR)

    def test_a_start_that_never_lands_is_stopped_and_an_error(self):
        # A Live that ignores the seek altogether: the transport plays from
        # the start marker. The call must not report the wrong start as the
        # right one, and must not leave it playing.
        self.song._pending_song_time = None
        original = type(self.song).current_song_time.fset

        def ignore(song, value):
            pass
        type(self.song).current_song_time = property(
            type(self.song).current_song_time.fget, ignore)
        try:
            reply = self.command("play_from", {"time": 192.0})
        finally:
            type(self.song).current_song_time = property(
                type(self.song).current_song_time.fget, original)
        self.assertEqual(reply["status"], "error")
        self.assertIn("did not start at beat 192", reply["message"])
        self.assertFalse(self.song.is_playing)


class Capture(Base):
    """Issues 2, 3 and 4."""

    def setUp(self):
        Base.setUp(self)
        self.ok("ensure_capture_track")
        self.capture = [t.name for t in self.song.tracks].index("Capture")

    def take(self, bar, bars=1):
        start = (bar - 1) * BAR
        r = self.ok("start_capture", {"start": start, "bars": bars, "name": "t"})
        slot = r["slot"]
        status = self.ok("capture_status", {"slot": slot})
        self.ok("stop_capture", {"slot": slot})
        return start, r, status

    def test_ten_captures_from_mid_record_bars_all_fire_inside_the_preroll(self):
        self.stop_somewhere_else(17.0)
        for i in range(10):
            bar = 49 + 4 * (i % 4)
            with self.subTest(take=i, bar=bar):
                start, r, status = self.take(bar)
                self.assertGreaterEqual(r["confirmed_at"], r["begin_beat"] - 0.05)
                self.assertLess(r["confirmed_at"], start)
                self.assertEqual(r["launch_quantization"], "q_bar")
                self.assertGreaterEqual(status["song_time"], r["begin_beat"] - 0.05)
                self.assertEqual(status["off_arrangement"], [])

    def test_a_capture_from_bar_one_fires_with_the_transport(self):
        self.stop_somewhere_else(40.0)
        start, r, status = self.take(1)
        self.assertEqual(r["preroll_beats"], 0.0)
        self.assertAlmostEqual(r["confirmed_at"], 0.0, places=3)

    def test_after_a_capture_every_track_follows_the_arrangement(self):
        self.take(49)
        self.assertFalse(self.song.back_to_arranger)
        for t in self.song.tracks:
            self.assertFalse(t.back_to_arranger, t.name)
            self.assertEqual(t.playing_slot_index, -1, t.name)

    def test_tracks_left_on_session_clips_are_put_back_and_named(self):
        self.song.tracks[0].clip_slots[0].fire()
        self.ok("stop_playback")
        self.assertTrue(self.song.tracks[0].back_to_arranger)
        start, r, status = self.take(49)
        self.assertEqual(r["returned_to_arrangement"], ["Kick"])
        self.assertEqual(status["off_arrangement"], [])
        self.assertFalse(self.song.back_to_arranger)

    def test_status_names_a_track_that_leaves_the_arrangement_mid_take(self):
        r = self.ok("start_capture", {"start": 48 * BAR, "bars": 2, "name": "t"})
        self.song.tracks[1].clip_slots[0].fire()
        status = self.ok("capture_status", {"slot": r["slot"]})
        self.assertEqual(status["off_arrangement"], ["Pad"])
        self.ok("stop_capture", {"slot": r["slot"]})

    def test_twenty_captures_on_eight_slots_rotate_the_oldest_and_never_stop(self):
        seen = []
        for i in range(20):
            with self.subTest(take=i):
                start, r, status = self.take(49)
                seen.append(r["slot"])
                if i >= 8:
                    self.assertIn("rotated", r)
                    self.assertEqual(r["rotated"]["slot"], seen[i - 8])
                else:
                    self.assertNotIn("rotated", r)
        self.assertEqual(seen[:8], list(range(8)))
        self.assertEqual(seen[8:16], list(range(8)))


class ClipEnvelope(Base):
    """Issue 5: a read at time 0 is the value written at time 0."""

    def write_and_read(self, target, points, parameter):
        parameter.value = 0.55 if parameter.max == 1.0 else parameter.value
        self.ok("set_clip_automation", {
            "track_index": 0, "clip_index": 0, "arrangement": False, "target": target,
            "points": points, "mode": "linear", "resolution": 0.5, "clear": True})
        return self.ok("get_clip_automation", {
            "track_index": 0, "clip_index": 0, "arrangement": False, "target": target,
            "resolution": 4})

    def test_volume_ramp_reads_its_written_values(self):
        vol = self.song.tracks[0].mixer_device.volume
        r = self.write_and_read({"mixer": "volume"},
                                [{"time": 0, "value": 0.0}, {"time": 16, "value": 1.0}], vol)
        values = [s["value"] for s in r["samples"]]
        self.assertAlmostEqual(values[0], 0.0, delta=0.01)
        self.assertAlmostEqual(values[1], 0.25, delta=0.01)   # beat 4 of 16
        self.assertAlmostEqual(values[2], 0.5, delta=0.01)
        self.assertEqual(r["semantics"], "absolute")
        self.assertIn("db", r["samples"][0])

    def test_a_step_at_one_reads_one_at_time_zero(self):
        vol = self.song.tracks[0].mixer_device.volume
        r = self.write_and_read({"mixer": "volume"}, [{"time": 0, "value": 1.0}], vol)
        self.assertAlmostEqual(r["samples"][0]["value"], 1.0, delta=0.01)

    def test_a_send_reads_its_written_value_at_time_zero(self):
        sends = list(self.song.tracks[0].mixer_device.sends)
        if not sends:
            self.skipTest("the set has no return")
        r = self.write_and_read({"mixer": "send", "send_index": 0},
                                [{"time": 0, "value": 0.0}, {"time": 16, "value": 1.0}], sends[0])
        self.assertAlmostEqual(r["samples"][0]["value"], 0.0, delta=0.01)

    def test_a_device_parameter_reads_its_written_value_at_time_zero(self):
        track = self.song.tracks[0]
        if not list(track.devices):
            self.ok("load_browser_item", {"track_index": 0, "item_uri": "query:AudioFx#Reverb"})
        params = list(track.devices[0].parameters)
        p = params[-1]
        lo, hi = float(p.min), float(p.max)
        r = self.write_and_read({"device_index": 0, "parameter_index": len(params) - 1},
                                [{"time": 0, "value": lo}, {"time": 16, "value": hi}], p)
        self.assertAlmostEqual(r["samples"][0]["value"], lo, delta=0.01 * (hi - lo))

    def test_the_write_reads_its_points_back(self):
        r = self.ok("set_clip_automation", {
            "track_index": 0, "clip_index": 0, "target": {"mixer": "volume"},
            "points": [{"time": 0, "value": 0.0}, {"time": 8, "value": 0.85}],
            "mode": "linear", "resolution": 0.5, "clear": True})
        self.assertEqual(r["semantics"], "absolute")
        first = r["readback"][0]
        self.assertAlmostEqual(first["reads"], 0.0, delta=0.01)

    def test_a_point_in_db_is_the_faders_own_value(self):
        vol = self.song.tracks[0].mixer_device.volume
        r = self.ok("set_clip_automation", {
            "track_index": 0, "clip_index": 0, "target": {"mixer": "volume"},
            "points": [{"time": 0, "db": 0.0}], "mode": "step", "resolution": 0.5,
            "clear": True})
        got = r["readback"][0]
        self.assertAlmostEqual(got["db"], 0.0, delta=0.2)
        self.assertAlmostEqual(got["reads"], self.script._value_for_db(vol, 0.0), places=4)

    def test_db_is_refused_for_anything_but_volume(self):
        reply = self.command("set_clip_automation", {
            "track_index": 0, "clip_index": 0, "target": {"mixer": "pan"},
            "points": [{"time": 0, "db": 0.0}]})
        self.assertEqual(reply["status"], "error")
        self.assertIn("db is for the volume target", reply["message"])


if __name__ == "__main__":
    unittest.main()
