"""The second studio report (2026-10-15, after 1.39.0): changing a clip's notes
in place, the locators an agent can see and delete, the set as data, and a
fade that ends where it was written to.

Every Live behaviour these lean on was measured on Live 12.4.6 the same day
and is in the model with its source: an Arrangement copy keeps its Session
clip's name and takes remove_notes_extended / set_notes; an Arrangement clip
refuses create_automation_envelope.
"""
import unittest

import harness
import fake_live
from test_duplex import connect
from test_every_command import Sink


def a_record():
    """A Lead with a Theme placed three times, a different clip placed once,
    an audio track, and three locators — two of them sharing a name."""
    song = fake_live.Song(scenes=4)
    song.create_midi_track()
    song.tracks[0].name = "Lead"
    song.create_audio_track()
    song.tracks[1].name = "Atmo"
    lead = song.tracks[0]
    lead.clip_slots[0].create_clip(4.0)
    lead.clip_slots[0].clip.name = "Theme"
    lead.clip_slots[0].clip.set_notes(((60, 0.0, 1.0, 100, False), (62, 1.0, 1.0, 100, False)))
    lead.clip_slots[1].create_clip(4.0)
    lead.clip_slots[1].clip.name = "Call"
    lead.clip_slots[1].clip.set_notes(((67, 0.0, 2.0, 100, False),))
    for t in (0.0, 16.0, 32.0):
        lead.duplicate_clip_to_arrangement(lead.clip_slots[0].clip, t)
    lead.duplicate_clip_to_arrangement(lead.clip_slots[1].clip, 48.0)
    song.tracks[1].clip_slots[0]._clip = fake_live.Clip(
        song, song.tracks[1], 4.0, midi=False, name="Air", file_path="/a.wav")
    for name, time in (("Drop", 64.0), ("Intro", 0.0), ("drop", 96.0)):
        song._cue_points.append(fake_live.CuePoint(song, name, time))
    return song


def pitches(clip):
    return [(n.pitch, n.start_time) for n in
            sorted(clip.get_notes_extended(0, 128, 0.0, 64.0), key=lambda n: (n.start_time, n.pitch))]


class Base(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load(song=a_record())
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


NEW_THEME = [{"pitch": 60, "start_time": 0.0, "duration": 1.0},
             {"pitch": 64, "start_time": 1.0, "duration": 1.0}]  # 62 became 64


class ReplaceClipNotes(Base):
    """R1: a melody change without a rebuild."""

    def test_one_changed_note_reaches_the_clip_and_all_three_placements(self):
        r = self.ok("replace_clip_notes", {"clips": [
            {"track_index": 0, "clip_index": 0, "notes": NEW_THEME, "arrangement": True}]})
        lead = self.song.tracks[0]
        want = [(60, 0.0), (64, 1.0)]
        self.assertEqual(pitches(lead.clip_slots[0].clip), want)
        themes = [c for c in lead.arrangement_clips if c.name == "Theme"]
        self.assertEqual(len(themes), 3)
        for c in themes:
            self.assertEqual(pitches(c), want, "the copy at beat %s" % c.start_time)
        self.assertEqual([c["start_time"] for c in r["replaced"][0]["arrangement_copies"]],
                         [0.0, 16.0, 32.0])

    def test_a_clip_of_another_name_is_not_touched(self):
        self.ok("replace_clip_notes", {"clips": [
            {"track_index": 0, "clip_index": 0, "notes": NEW_THEME, "arrangement": True}]})
        lead = self.song.tracks[0]
        call = [c for c in lead.arrangement_clips if c.name == "Call"][0]
        self.assertEqual(pitches(call), [(67, 0.0)])
        self.assertEqual(pitches(lead.clip_slots[1].clip), [(67, 0.0)])

    def test_without_arrangement_the_placements_keep_their_notes(self):
        r = self.ok("replace_clip_notes", {"clips": [
            {"track_index": 0, "clip_index": 0, "notes": NEW_THEME}]})
        self.assertEqual(r["replaced"][0]["arrangement_copies"], [])
        lead = self.song.tracks[0]
        for c in lead.arrangement_clips:
            if c.name == "Theme":
                self.assertEqual(pitches(c), [(60, 0.0), (62, 1.0)])

    def test_the_whole_edit_is_one_undo_step(self):
        before = self.song._undo_steps
        self.ok("replace_clip_notes", {"clips": [
            {"track_index": 0, "clip_index": 0, "notes": NEW_THEME, "arrangement": True},
            {"track_index": 0, "clip_index": 1, "notes": [{"pitch": 69, "start_time": 0.0}],
             "arrangement": True}]})
        self.assertEqual(self.song._undo_steps - before, 1)

    def test_a_new_length_resizes_the_session_clip_and_not_the_placements(self):
        self.ok("replace_clip_notes", {"clips": [
            {"track_index": 0, "clip_index": 0, "notes": NEW_THEME, "length": 8.0,
             "arrangement": True}]})
        lead = self.song.tracks[0]
        self.assertAlmostEqual(lead.clip_slots[0].clip.length, 8.0)
        self.assertAlmostEqual(lead.clip_slots[0].clip.end_marker, 8.0)
        for c in lead.arrangement_clips:
            self.assertAlmostEqual(c.length, 4.0)
        self.ok("replace_clip_notes", {"clips": [
            {"track_index": 0, "clip_index": 0, "notes": NEW_THEME, "length": 2.0}]})
        self.assertAlmostEqual(lead.clip_slots[0].clip.length, 2.0)

    def test_every_clip_is_checked_before_the_first_is_changed(self):
        reply = self.command("replace_clip_notes", {"clips": [
            {"track_index": 0, "clip_index": 0, "notes": NEW_THEME, "arrangement": True},
            {"track_index": 0, "clip_index": 3, "notes": NEW_THEME}]})
        self.assertEqual(reply["status"], "error")
        self.assertIn("slot 3 on 'Lead' holds no clip", reply["message"])
        self.assertEqual(pitches(self.song.tracks[0].clip_slots[0].clip), [(60, 0.0), (62, 1.0)])

    def test_an_audio_clip_is_refused(self):
        reply = self.command("replace_clip_notes", {"clips": [
            {"track_index": 1, "clip_index": 0, "notes": NEW_THEME}]})
        self.assertEqual(reply["status"], "error")
        self.assertIn("audio clip", reply["message"])


class Locators(Base):
    """R3 and R5: the locators are in get_context, and one can be deleted
    by the name it is listed under."""

    def test_get_context_lists_every_locator_by_time(self):
        r = self.ok("get_context")
        self.assertEqual([(l["name"], l["time"]) for l in r["locators"]],
                         [("Intro", 0.0), ("Drop", 64.0), ("drop", 96.0)])

    def test_get_context_counts_each_tracks_slots(self):
        r = self.ok("get_context")
        self.assertEqual([t["slots"] for t in r["tracks"]], [4, 4])

    def test_a_name_two_locators_share_is_refused_with_their_beats(self):
        reply = self.command("delete_locator", {"name": "DROP"})
        self.assertEqual(reply["status"], "error")
        self.assertIn("2 locators are called 'DROP' (at beats 64, 96)", reply["message"])
        self.assertEqual(len(self.song.cue_points), 3)

    def test_a_name_is_matched_without_case_or_space(self):
        r = self.ok("delete_locator", {"name": "  intro "})
        self.assertEqual(r["deleted"], "Intro")
        self.assertEqual([c.name for c in self.song.cue_points], ["Drop", "drop"])

    def test_the_time_settles_a_shared_name(self):
        r = self.ok("delete_locator", {"time": 96.0})
        self.assertEqual(r["deleted"], "drop")


class Envelopes(Base):
    """A fade that runs to the clip's end reaches its target; the Arrangement
    is refused, as Live refuses it."""

    def test_a_fade_to_the_clips_end_reaches_its_target(self):
        r = self.ok("set_clip_automation", {
            "track_index": 0, "clip_index": 0, "target": {"mixer": "volume"},
            "points": [{"time": 0, "value": 0.0}, {"time": 4, "value": 0.85}],
            "mode": "linear", "resolution": 0.5, "clear": True})
        reads = dict((row["time"], row["reads"]) for row in r["readback"])
        self.assertAlmostEqual(reads[0.0], 0.0, places=4)
        self.assertAlmostEqual(reads[4.0], 0.85, places=4)
        got = self.ok("get_clip_automation", {
            "track_index": 0, "clip_index": 0, "target": {"mixer": "volume"},
            "resolution": 0.5})
        values = dict((s["time"], s["value"]) for s in got["samples"])
        # Every step still holds the ramp's own value at its start.
        self.assertAlmostEqual(values[2.0], 0.425, places=4)
        self.assertAlmostEqual(values[3.5], 0.85 * 3.5 / 4, places=4)
        self.assertAlmostEqual(values[4.0], 0.85, places=4)

    def test_a_fade_that_ends_before_the_clip_holds_its_target(self):
        r = self.ok("set_clip_automation", {
            "track_index": 0, "clip_index": 0, "target": {"mixer": "volume"},
            "points": [{"time": 0, "value": 0.0}, {"time": 2, "value": 0.85}],
            "mode": "linear", "resolution": 0.5, "clear": True})
        self.assertEqual(r["steps_written"], 5)  # four ramp steps and the hold
        got = self.ok("get_clip_automation", {
            "track_index": 0, "clip_index": 0, "target": {"mixer": "volume"},
            "resolution": 1})
        values = dict((s["time"], s["value"]) for s in got["samples"])
        self.assertAlmostEqual(values[1.0], 0.425, places=4)
        self.assertAlmostEqual(values[3.0], 0.85, places=4)

    def test_an_arrangement_clip_is_refused_as_live_refuses_it(self):
        reply = self.command("set_clip_automation", {
            "track_index": 0, "clip_index": 0, "arrangement": True,
            "target": {"mixer": "volume"}, "points": [{"time": 0, "value": 0.5}]})
        self.assertEqual(reply["status"], "error")
        self.assertIn("Not a session clip", reply["message"])


if __name__ == "__main__":
    unittest.main()
