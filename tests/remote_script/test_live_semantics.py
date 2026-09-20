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
    """Live's sequences are its own `Vector`: not a list, not a tuple.

    `describe song.tracks` on Live 12.4.6 reports `class: Vector`, and
    `run … get song.tracks` hands back `"<Base.Vector object at 0x…>"` —
    `_jsonable` tests `isinstance(value, (list, tuple))` and a Vector is
    neither. `_step` still does `list(obj)`, which works on any sequence.
    Verified against a real Live on 2026-09-20."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_every_live_sequence_is_a_vector_not_a_list_or_tuple(self):
        for seq in (self.song.tracks, self.song.return_tracks, self.song.scenes,
                    self.song.cue_points, self.song.visible_tracks,
                    self.song.tracks[0].clip_slots, self.song.tracks[0].devices,
                    self.song.tracks[0].arrangement_clips,
                    self.song.tracks[0].mixer_device.sends,
                    self.song.return_tracks[0].devices[0].parameters,
                    self.song.master_track.mixer_device.sends):
            self.assertIsInstance(seq, fake_live.Vector)
            self.assertNotIsInstance(seq, (list, tuple))
            self.assertEqual(list(seq), list(iter(seq)))

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

    def test_live_has_no_value_string_so_the_model_has_none_either(self):
        """The script reads `param.value_string` as a fallback and says in
        its own comment that it "is not in the LOM". A real Live 12.4.6
        agrees: `describe` on a parameter lists `str_for_value` and no
        `value_string`. The model matches Live, so the fallback is exercised
        here exactly as it is there."""
        volume = self.song.tracks[0].mixer_device.volume
        with self.assertRaises(AttributeError):
            volume.value_string
        # What Live does give: the display for any value.
        self.assertEqual(volume.str_for_value(0.85), "0.0 dB")
        # And printing the parameter gives that display, which is what
        # `run … get` on it hands back.
        self.assertEqual(str(volume), volume.str_for_value(volume.value))


class Fader(unittest.TestCase):
    """The volume fader's taper, as Live really draws it.

    Cycling '74's reference does not document it, so it was measured on
    2026-09-20 against Live 12.4.6 by asking
    `song.tracks[0].mixer_device.volume.str_for_value` at twenty-one fader
    positions over a `run` batch. Above 0.4 it is a straight 40 dB per
    unit; below it a quadratic. Both reproduce every measured point to the
    decimal Live prints, and Live prints one — except where a value needs
    more, as 0.6999869 does ("-6.001 dB").

    The five points in `src/song.rs`'s
    `lives_own_curve_converts_a_meter_reading` (`[0.4,-30] [0.7,-12]`) are
    **not** this curve: they are synthetic fixture data for `MeterScale`,
    which takes its real points from `get_meter_scale` at run time."""

    MEASURED = ((0.05, -57.2), (0.10, -48.6), (0.15, -41.0), (0.20, -34.4),
                (0.25, -28.8), (0.30, -24.2), (0.35, -20.6), (0.40, -18.0),
                (0.45, -16.0), (0.50, -14.0), (0.55, -12.0), (0.60, -10.0),
                (0.65, -8.0), (0.70, -6.0), (0.75, -4.0), (0.80, -2.0),
                (0.85, 0.0), (0.90, 2.0), (0.95, 4.0), (1.00, 6.0))

    def setUp(self):
        self.song = fake_live.default_set()
        self.volume = self.song.tracks[0].mixer_device.volume

    def test_unity_is_0_85_and_the_top_is_plus_six(self):
        self.assertEqual(self.volume.str_for_value(0.85), "0.0 dB")
        self.assertEqual(self.volume.str_for_value(1.0), "6.0 dB")

    def test_the_bottom_reads_minus_infinity_not_a_number(self):
        self.assertEqual(self.volume.str_for_value(0.0), "-inf dB")

    def test_the_taper_reproduces_every_position_live_was_asked_for(self):
        for value, db in self.MEASURED:
            self.assertAlmostEqual(fake_live.fader_to_db(value), db, places=1,
                                   msg="fader %.2f" % value)
            self.assertEqual(self.volume.str_for_value(value), "%.1f dB" % db)

    def test_above_the_knee_it_is_forty_dB_per_unit(self):
        """0.4 to 1.0 is a straight line: 0.85 is unity, 1.0 is +6."""
        for a, b in zip(self.MEASURED[7:], self.MEASURED[8:]):
            slope = (b[1] - a[1]) / (b[0] - a[0])
            self.assertAlmostEqual(slope, 40.0, places=6)

    def test_db_to_fader_is_the_inverse(self):
        for db in (-48.6, -30.0, -18.0, -12.0, -6.0, 0.0, 3.0, 6.0):
            self.assertAlmostEqual(fake_live.fader_to_db(fake_live.db_to_fader(db)),
                                   db, places=4)

    def test_minus_six_lands_where_live_landed(self):
        """The differential caught this: asking Live for -6 dB left the
        fader at 0.6999, and a model on the old curve put it at 0.7749."""
        self.assertAlmostEqual(fake_live.db_to_fader(-6.0), 0.70, places=3)


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
        # Not is_playing: Live reports it writable (`describe song` gives
        # readonly=False) and starts the transport when it is set.
        for obj, attr in ((self.song, "exclusive_arm"), (self.song, "can_undo"),
                          (self.song.tracks[0], "clip_slots"),
                          (self.song.tracks[0], "has_midi_input"),
                          (self.song.tracks[0].mixer_device.volume, "min")):
            with self.assertRaises(AttributeError):
                setattr(obj, attr, 1)

    def test_the_master_and_the_returns_have_no_arm_at_all(self):
        """`ensure_capture_track` checks `can_be_armed` before touching
        `arm`, and this is why: on Live 12.4.6 neither the master nor a
        return has an `arm` member — `describe song.master_track` lists
        none, and reading it raises. The check is not belt-and-braces, it
        is the only thing standing between the script and an
        AttributeError."""
        for track in (self.song.master_track, self.song.return_tracks[0]):
            self.assertFalse(track.can_be_armed)
            with self.assertRaises(AttributeError):
                track.arm
            with self.assertRaises(AttributeError):
                track.arm = True

    def test_a_midi_track_arms_and_arming_one_disarms_the_rest(self):
        """exclusive_arm is on in a new set."""
        first, second = self.song.tracks[0], self.song.tracks[1]
        first.arm = True
        self.assertTrue(first.arm)
        second.arm = True
        self.assertTrue(second.arm)
        self.assertFalse(first.arm, "exclusive_arm disarms the other track")


class Cues(unittest.TestCase):
    """`set_or_delete_cue` toggles a locator at the play position — Live
    gives no way to put one anywhere else, which is what `create_locator`'s
    two-phase move is for (#25)."""

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


class BrowserCategories(unittest.TestCase):
    """#58: a category is a place to browse, not any member of Browser.

    Live's Browser carries `load_item`, `preview_item`,
    `relation_to_hotswap_target`, `filter_type`, `colors` and three listener
    methods per observable property. `dir(browser)` returns all of it, and
    `available_categories` used to be exactly that — so a client was told it
    could ask for `load_item`."""

    def setUp(self):
        self.ns = harness.load()
        self.script = harness.instance(self.ns)
        self.browser = self.script.application().browser

    def test_only_browsable_roots_are_offered(self):
        roots = self.script._browser_roots(self.browser)
        self.assertIn("instruments", roots)
        self.assertIn("drums", roots)
        self.assertIn("user_folders", roots, "a vector of places is a category too")
        for name in roots:
            self.assertFalse(name.endswith("_listener"))
            value = getattr(self.browser, name)
            self.assertFalse(callable(value), "%s is a method" % name)

    def test_the_methods_and_the_enums_are_left_out(self):
        roots = self.script._browser_roots(self.browser)
        for name in ("load_item", "stop_preview", "relation_to_hotswap_target",
                     "hotswap_target", "filter_type", "colors"):
            self.assertNotIn(name, roots)

    def test_every_root_offered_can_actually_be_walked(self):
        """The promise the field makes: ask for one of these and get items."""
        for name in self.script._browser_roots(self.browser):
            value = getattr(self.browser, name)
            items = [value] if hasattr(value, "name") else list(value)
            for item in items:
                self.assertTrue(hasattr(item, "name"))


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


class SongFilePath(unittest.TestCase):
    """`Song.file_path` is the path to the open Set, and it is empty until
    the Set has been saved.

    Cycling '74's LOM reference, Song > file_path
    (`docs/reference/ableton/live-object-model.md:223`): "Type: symbol —
    Access: get. The path to the current Live Set, in OS-native format. If
    the Live Set hasn't been saved, the path is empty."

    The server identifies a song by this, through a generic `run` op, so
    three things have to hold: reading it never raises, an unsaved Set reads
    empty rather than None, and it cannot be written (Live's API cannot save
    or name a Set, which is why the producer presses Cmd+S). The model had
    the Clip helper `_audio_only` on it, whose getter reads
    `self._is_midi_clip`; every read raised `AttributeError: 'Song' object
    has no attribute '_is_midi_clip'` (measured 2026-09-20)."""

    def setUp(self):
        self.song = fake_live.default_set()

    def test_an_unsaved_set_reads_an_empty_path_rather_than_raising(self):
        self.assertEqual(self.song.file_path, "")

    def test_a_saved_set_reads_the_path_live_would_give(self):
        self.song._file_path = "/Users/p/Music/Smoke Project/Smoke.als"
        self.assertEqual(self.song.file_path,
                         "/Users/p/Music/Smoke Project/Smoke.als")

    def test_it_cannot_be_written_because_lives_api_cannot_save_a_set(self):
        with self.assertRaises(AttributeError):
            self.song.file_path = "/tmp/anything.als"

    def test_it_reads_back_through_the_generic_ops_layer(self):
        """The server asks for it with `run`, so no command is added and
        SCRIPT_VERSION does not move."""
        ns = harness.load(song=self.song)
        done = ns["Done"]
        script = harness.instance(ns)

        def read():
            gen = script._run_ops(
                {"ops": [{"op": "get", "path": "song.file_path", "as": "set"}]})
            while True:
                item = next(gen)
                if isinstance(item, done):
                    return item.result["set"]

        self.assertEqual(read(), "")
        self.song._file_path = "/Users/p/Music/Smoke Project/Smoke.als"
        self.assertEqual(read(), "/Users/p/Music/Smoke Project/Smoke.als")


if __name__ == "__main__":
    unittest.main()
