"""Live's semantics the Remote Script depends on, one case each.

These are not the model's choices. Each is something the script would be
wrong about if Live behaved otherwise, and each is named in #53's table of
"Live API facts this is built on", from Cycling '74's LOM reference for
Live 11+ (`docs/reference/ableton/live-api-overview.md`) or from a
measurement this repo took. The model encodes them; this pins them, so a
convenient simplification in `fake_live.py` fails here rather than letting
a Rust suite pass against a Live that does not exist.

Where the reference is silent — the fader taper is not documented there —
the case follows the measurement and says which one.
"""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import fake_live  # noqa: E402
import harness  # noqa: E402


class Sequences(unittest.TestCase):
    """`_step` does `list(obj)` and `describe` types them `list[...]`."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_every_live_sequence_is_a_tuple_not_a_list(self):
        for seq in (self.song.tracks, self.song.return_tracks, self.song.scenes,
                    self.song.cue_points, self.song.visible_tracks,
                    self.song.tracks[0].clip_slots, self.song.tracks[0].devices,
                    self.song.tracks[0].arrangement_clips,
                    self.song.tracks[0].mixer_device.sends,
                    self.song.return_tracks[0].devices[0].parameters,
                    self.song.master_track.mixer_device.sends):
            self.assertIsInstance(seq, tuple)

    def test_a_sequence_cannot_be_assigned_to(self):
        """Live's `tracks` is a read-only property. A test that wants a set
        of its own builds it through the API, as a producer would."""
        for attr in ("tracks", "scenes", "return_tracks"):
            with self.assertRaises(AttributeError):
                setattr(self.song, attr, [])


class Notes(unittest.TestCase):
    """`_add_notes_to_clip` reports `len(notes)` because `set_notes` ADDS;
    the server's replace loop is read → `clear_notes_from_clip` → add."""

    def setUp(self):
        self.song = fake_live.default_set()
        self.song.tracks[0].clip_slots[0].create_clip(4.0)
        self.clip = self.song.tracks[0].clip_slots[0].clip

    def test_set_notes_adds_it_does_not_replace(self):
        self.clip.set_notes(((60, 0.0, 1.0, 100, False),))
        self.clip.set_notes(((64, 1.0, 1.0, 100, False),))
        self.assertEqual(sorted(n.pitch for n in self.clip._notes), [60, 64])

    def test_the_two_getters_take_their_arguments_in_different_orders(self):
        """`get_notes(from_time, from_pitch, time_span, pitch_span)` against
        `get_notes_extended(from_pitch, pitch_span, from_time, time_span)` —
        `_notes_from_clip` and `_clear_notes_from_clip` each use one."""
        self.clip.set_notes(((60, 0.0, 1.0, 100, False), (72, 3.0, 1.0, 100, False)))
        # The same window, written both ways: beats 0-2, pitches 0-64.
        plain = self.clip.get_notes(0.0, 0, 2.0, 65)
        extended = self.clip.get_notes_extended(0, 65, 0.0, 2.0)
        self.assertEqual([n[0] for n in plain], [60])
        self.assertEqual([n.pitch for n in extended], [60])
        # The four numbers the extended call takes, handed to the plain one,
        # select nothing: from_time 0, from_pitch 65, time_span 0. Getting
        # the order wrong is silent, which is why it is pinned here.
        self.assertEqual(self.clip.get_notes(0, 65, 0.0, 2.0), ())

    def test_remove_notes_takes_lives_order_too(self):
        self.clip.set_notes(((60, 0.0, 1.0, 100, False), (72, 3.0, 1.0, 100, False)))
        self.clip.remove_notes(0.0, 0, 2.0, 128)
        self.assertEqual([n.pitch for n in self.clip._notes], [72])


class Grid(unittest.TestCase):
    """One clip slot per scene, always — `_create_clip` bounds-checks
    against `clip_slots`, so a model where the two could differ would let a
    clip be written where Live has no cell."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_every_track_has_one_slot_per_scene(self):
        for t in self.song.tracks:
            self.assertEqual(len(t.clip_slots), len(self.song.scenes))

    def test_create_scene_gives_every_track_a_slot(self):
        before = len(self.song.scenes)
        self.song.create_scene()
        self.assertEqual(len(self.song.scenes), before + 1)
        for t in self.song.tracks:
            self.assertEqual(len(t.clip_slots), before + 1)

    def test_a_new_track_gets_one_slot_per_existing_scene(self):
        self.song.create_scene()
        self.song.create_midi_track()
        self.assertEqual(len(self.song.tracks[-1].clip_slots), len(self.song.scenes))

    def test_delete_scene_takes_the_row_off_every_track(self):
        before = len(self.song.scenes)
        self.song.delete_scene(0)
        for t in self.song.tracks:
            self.assertEqual(len(t.clip_slots), before - 1)

    def test_create_midi_track_appends_and_the_new_one_is_the_last(self):
        before = len(self.song.tracks)
        self.song.create_midi_track(-1)
        self.assertEqual(len(self.song.tracks), before + 1)
        self.assertTrue(self.song.tracks[-1].has_midi_input)


class Sends(unittest.TestCase):
    """`create_return_track` adds a send to every track — `_set_send`
    resolves a send by index or by the return's name."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_every_track_has_one_send_per_return(self):
        for t in self.song.tracks:
            self.assertEqual(len(t.mixer_device.sends), len(self.song.return_tracks))

    def test_a_new_return_gives_every_track_another_send(self):
        before = len(self.song.tracks[0].mixer_device.sends)
        self.song.create_return_track()
        for t in self.song.tracks:
            self.assertEqual(len(t.mixer_device.sends), before + 1)

    def test_a_track_created_after_the_return_has_the_send_too(self):
        self.song.create_return_track()
        self.song.create_midi_track()
        self.assertEqual(len(self.song.tracks[-1].mixer_device.sends),
                         len(self.song.return_tracks))


class Parameters(unittest.TestCase):
    """What #47's "did it land" check reads, and what the display bisection
    needs to converge."""

    def setUp(self):
        self.song = fake_live.default_set()
        self.reverb = self.song.return_tracks[0].devices[0]

    def test_a_value_outside_the_range_raises(self):
        p = self.reverb.parameters[0]
        for bad in (p.max + 0.01, p.min - 0.01):
            with self.assertRaises(RuntimeError):
                p.value = bad

    def test_str_for_value_is_monotonic_so_the_bisection_converges(self):
        """`_value_from_display` bisects over `str_for_value`. A curve that
        doubled back would make it land anywhere."""
        for p in self.reverb.parameters:
            if p.is_quantized:
                continue
            lo, hi = float(p.min), float(p.max)
            seen = []
            for i in range(64):
                v = lo + (hi - lo) * i / 63.0
                seen.append(self._number_in(p.str_for_value(v)))
            self.assertEqual(seen, sorted(seen),
                             "%s: display is not monotonic" % p.name)

    @staticmethod
    def _number_in(text):
        import re
        m = re.search(r"-?\d+(?:\.\d+)?", text)
        return float(m.group(0)) if m else 0.0

    def test_a_quantized_parameter_snaps_to_its_nearest_step(self):
        activator = self.song.tracks[0].mixer_device.track_activator
        self.assertTrue(activator.is_quantized)
        activator.value = 0.25
        self.assertEqual(activator.value, 0.0)
        self.assertEqual(activator.str_for_value(activator.value), "Off")

    def test_value_string_and_str_for_value_agree_on_the_current_value(self):
        p = self.reverb.parameters[0]
        p.value = 0.5
        self.assertEqual(p.value_string, p.str_for_value(0.5))


class Fader(unittest.TestCase):
    """The volume fader's taper. Cycling '74's reference does not document
    it; these three points were measured on Live 12.4.6 and are the curve
    `src/song.rs` converts with (`[0,-80] [0.4,-30] [0.7,-12] [0.85,0]
    [1.0,6]`). `get_meter_scale` hands the server the same points."""

    def setUp(self):
        self.song = fake_live.default_set()
        self.volume = self.song.tracks[0].mixer_device.volume

    def test_unity_is_0_85_and_the_top_is_plus_six(self):
        self.assertEqual(self.volume.str_for_value(0.85), "0.00 dB")
        self.assertEqual(self.volume.str_for_value(1.0), "6.00 dB")

    def test_the_bottom_reads_minus_infinity_not_a_number(self):
        self.assertEqual(self.volume.str_for_value(0.0), "-inf dB")

    def test_the_curve_matches_the_one_src_song_rs_converts_with(self):
        for value, db in ((0.0, -80.0), (0.4, -30.0), (0.7, -12.0),
                          (0.85, 0.0), (1.0, 6.0)):
            self.assertAlmostEqual(fake_live.fader_to_db(value), db, places=9)

    def test_db_to_fader_is_the_inverse(self):
        for db in (-30.0, -12.0, -6.0, 0.0, 3.0, 6.0):
            self.assertAlmostEqual(fake_live.fader_to_db(fake_live.db_to_fader(db)),
                                   db, places=6)


class Arrangement(unittest.TestCase):
    """`duplicate_clip_to_arrangement` trims what it overlaps — Live does
    not stack two clips on one track at one time."""

    def setUp(self):
        self.song = fake_live.default_set()
        self.track = self.song.tracks[0]
        self.track.clip_slots[0].create_clip(4.0)
        self.source = self.track.clip_slots[0].clip

    def place(self, at):
        self.track.duplicate_clip_to_arrangement(self.source, at)

    def test_a_clip_dropped_over_another_trims_it(self):
        self.place(0.0)
        self.place(2.0)
        clips = self.track.arrangement_clips
        self.assertEqual(len(clips), 2)
        self.assertAlmostEqual(clips[0].end_time, 2.0, msg="the first was not trimmed")
        self.assertAlmostEqual(clips[1].start_time, 2.0)

    def test_a_clip_dropped_exactly_over_another_replaces_it(self):
        self.place(0.0)
        self.place(0.0)
        self.assertEqual(len(self.track.arrangement_clips), 1)

    def test_clips_that_do_not_touch_both_stay(self):
        self.place(0.0)
        self.place(8.0)
        self.assertEqual(len(self.track.arrangement_clips), 2)


class Exceptions(unittest.TestCase):
    """Live's own exception types. The script catches by type in places and
    the server turns the message into what the artist reads."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_a_bad_index_is_an_index_error(self):
        for call in (lambda: self.song.delete_track(99),
                     lambda: self.song.delete_scene(99),
                     lambda: self.song.duplicate_scene(99),
                     lambda: self.song.create_scene(99)):
            with self.assertRaises(IndexError):
                call()

    def test_an_impossible_operation_is_a_runtime_error(self):
        slot = self.song.tracks[0].clip_slots[0]
        slot.create_clip(4.0)
        with self.assertRaises(RuntimeError):
            slot.create_clip(4.0)                       # already has a clip
        with self.assertRaises(RuntimeError):
            self.song.tracks[2].clip_slots[0].create_clip(4.0)   # MIDI clip, audio track
        with self.assertRaises(RuntimeError):
            self.song.tempo = 5.0                       # outside 20..999

    def test_a_write_to_a_read_only_member_is_an_attribute_error(self):
        for obj, attr in ((self.song, "is_playing"), (self.song, "can_undo"),
                          (self.song.tracks[0], "clip_slots"),
                          (self.song.tracks[0], "has_midi_input"),
                          (self.song.tracks[0].mixer_device.volume, "min")):
            with self.assertRaises(AttributeError):
                setattr(obj, attr, 1)

    def test_a_track_that_cannot_be_armed_refuses_the_arm(self):
        """`ensure_capture_track` depends on this: Live raises where
        `can_be_armed` is False."""
        master = self.song.master_track
        self.assertFalse(master.can_be_armed)
        # Live refuses the write rather than ignoring it. The model raises
        # RuntimeError; which exception Live 12.4.6 actually raises is one
        # of the things the transcript differential (#53 section E) settles.
        with self.assertRaises(RuntimeError):
            master.arm = True


class Cues(unittest.TestCase):
    """`set_or_delete_cue()` toggles a cue AT THE PLAY POSITION, which is
    why `_create_locator` is two-phase (#25): the playhead move is applied
    asynchronously and the second step must see it."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_the_cue_lands_where_the_playhead_is(self):
        self.song.current_song_time = 16.0
        self.song.set_or_delete_cue()
        self.assertEqual([c.time for c in self.song.cue_points], [16.0])

    def test_calling_it_again_at_the_same_place_removes_it(self):
        self.song.current_song_time = 16.0
        self.song.set_or_delete_cue()
        self.song.set_or_delete_cue()
        self.assertEqual(self.song.cue_points, ())

    def test_cue_points_come_back_in_time_order(self):
        for t in (32.0, 8.0, 16.0):
            self.song.current_song_time = t
            self.song.set_or_delete_cue()
        self.assertEqual([c.time for c in self.song.cue_points], [8.0, 16.0, 32.0])


class Browser(unittest.TestCase):
    """`Browser.load_item` loads onto the SELECTED track (the highlighted
    slot for a sample) and replaces the instrument that is there — which is
    why `_load_browser_item` sets `view.selected_track` first."""

    def setUp(self):
        self.song = fake_live.default_set()
        self.browser = self.song._browser

    def load(self, uri, track):
        self.song.view.selected_track = track
        self.browser.load_item(self.browser._find(uri))

    def test_an_instrument_lands_on_the_selected_track(self):
        self.load("query:Synths#Analog", self.song.tracks[1])
        self.assertEqual([d.name for d in self.song.tracks[1].devices], ["Analog"])
        self.assertEqual(self.song.tracks[0].devices, ())

    def test_a_second_instrument_replaces_the_first(self):
        self.load("query:Synths#Analog", self.song.tracks[0])
        self.load("query:Synths#Operator", self.song.tracks[0])
        self.assertEqual([d.name for d in self.song.tracks[0].devices], ["Operator"])

    def test_an_audio_effect_is_appended_not_swapped(self):
        self.load("query:Synths#Analog", self.song.tracks[0])
        self.load("query:AudioFx#Reverb", self.song.tracks[0])
        self.load("query:AudioFx#Delay", self.song.tracks[0])
        self.assertEqual([d.name for d in self.song.tracks[0].devices],
                         ["Analog", "Reverb", "Delay"])

    def test_an_instrument_on_an_audio_track_is_refused(self):
        with self.assertRaises(RuntimeError):
            self.load("query:Synths#Analog", self.song.tracks[2])

    def test_a_midi_effect_stays_before_the_instrument(self):
        self.load("query:MidiFx#Arpeggiator", self.song.tracks[0])
        self.load("query:Synths#Analog", self.song.tracks[0])
        self.assertEqual([d.name for d in self.song.tracks[0].devices],
                         ["Arpeggiator", "Analog"])


class Undo(unittest.TestCase):
    """One undo step per mutating command (decision 0007): the executor
    opens one before the first slice and closes it after the last."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_begin_and_end_nest_into_one_step(self):
        self.song.begin_undo_step()
        self.song.begin_undo_step()
        self.song.create_midi_track()
        self.song.end_undo_step()
        self.assertEqual(self.song._undo_steps, 0, "closed too early")
        self.song.end_undo_step()
        self.assertEqual(self.song._undo_steps, 1)

    def test_ending_a_step_that_never_began_raises(self):
        with self.assertRaises(RuntimeError):
            self.song.end_undo_step()


class TheModelIsNotLive(unittest.TestCase):
    """What the model is NOT, stated as a test so nobody reads a green
    suite as proof of something it cannot prove. The architecture note says
    the same in prose."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_there_is_no_audio(self):
        """Meters read what a test put there, not what is playing. Anything
        about sound — `capture_mix`'s measurements, the levels channel's
        real numbers — is a real-Live check, always."""
        self.song.start_playing()
        self.assertEqual(self.song.master_track.output_meter_level, 0.0)

    def test_the_browser_is_a_handful_of_items_not_lives_library(self):
        names = [i.name for i in self.song._browser.instruments.children]
        self.assertLess(len(names), 20, "the model's browser is small on purpose")
        self.assertIn("Analog", names)

    def test_calls_are_free_unless_a_measured_table_is_attached(self):
        self.assertIsNone(self.song._latency)
        self.song._latency = fake_live.LatencyTable.measured(
            scale=0.0, clock=fake_live.DrivenClock())
        self.song.create_scene()
        self.assertEqual(self.song._latency.charged["Song.create_scene"][0], 1)

    def test_the_unmeasured_calls_are_named_rather_than_guessed(self):
        table = fake_live.LatencyTable.measured()
        self.assertIn("Song.create_midi_track", table.unmeasured())
        self.assertNotIn("Song.create_midi_track", table.table)


if __name__ == "__main__":
    unittest.main()
