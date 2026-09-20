"""Every command the server can ask for, through the script's real dispatch.

`tools::ALL_REMOTE_COMMANDS` in `src/tools.rs` is the one home for the
command list. This suite runs each of those names through the script's real
`_run_on_main` → `_dispatch` against the fake Live, with parameters a
producer's session would actually send, and asserts two things:

1. the reply is `{"status": "success", ...}` in the shape the server parses;
2. where the command changes the set, the set changed — asked of the model
   afterwards, not of the reply.

A name in `ALL_REMOTE_COMMANDS` with no case here fails
`test_every_command_in_the_servers_list_has_a_case`, the same one-home rule
`the_servers_command_list_and_the_scripts_dispatch_are_the_same_set` keeps
on the Rust side. An `AttributeError` out of one of these is a gap in the
model and is fixed in `fake_live.py` — never by giving the case an easier
parameter.
"""
import os
import re
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)

import fake_live  # noqa: E402
import harness  # noqa: E402
from test_duplex import connect  # noqa: E402

TOOLS_RS = os.path.join(ROOT, "src", "tools.rs")


def all_remote_commands():
    """`tools::ALL_REMOTE_COMMANDS`, read out of the Rust source. The list
    has one home; this reads it rather than keeping a copy."""
    import io
    with io.open(TOOLS_RS, encoding="utf-8") as handle:
        source = handle.read()
    start = source.index("pub const ALL_REMOTE_COMMANDS")
    body = source[start:source.index("];", start)]
    return [m for m in re.findall(r'"([a-z_]+)"', body)]


class Sink(object):
    """Where a reply lands. Stands in for the client's outbound buffer, so
    the executor path that a socket request takes is the one under test."""

    def __init__(self):
        self.payload = None

    def put(self, payload):
        self.payload = payload


def command_set():
    """A set with something in it: two named MIDI tracks (the first with an
    instrument and two clips, one of them with notes), two audio tracks, a
    Drum Rack, a return, a locator, and an Arrangement clip."""
    song = fake_live.default_set()
    song.tracks[0].name = "Kick"
    song.tracks[1].name = "Bass"
    song.tracks[2].name = "Vox"
    song.tracks[3].name = "Texture"

    browser = song._browser
    song.view.selected_track = song.tracks[0]
    browser.load_item(browser._find("query:Synths#Analog"))
    song.view.selected_track = song.tracks[1]
    browser.load_item(browser._find("query:Drums#Drum%20Rack"))
    song.view.selected_track = song.tracks[0]

    slot = song.tracks[0].clip_slots[0]
    slot.create_clip(4.0)
    slot.clip.name = "Kick 1"
    slot.clip.set_notes(((36, 0.0, 0.25, 100, False), (36, 1.0, 0.25, 100, False),
                         (36, 2.0, 0.25, 100, False), (36, 3.0, 0.25, 100, False)))
    song.tracks[0].clip_slots[1].create_clip(4.0)
    song.tracks[0].clip_slots[1].clip.name = "Kick 2"
    song.tracks[1].clip_slots[0].create_clip(4.0)
    song.tracks[1].clip_slots[0].clip.name = "Bass 1"

    # One Arrangement clip on Kick, at bar 1.
    song.tracks[0].duplicate_clip_to_arrangement(slot.clip, 0.0)

    song.current_song_time = 0.0
    song.set_or_delete_cue()          # a locator at bar 1
    song.current_song_time = 0.0
    song.scenes[0].name = "Intro · 8"
    song.scenes[1].name = "Drop · 8"
    return song


# ── The cases ───────────────────────────────────────────────────────────────
#
# name -> (params, check). `check(test, result, song)` runs after the reply;
# `None` means the reply shape is the whole assertion (a pure read).

def no_check(test, result, song):
    pass


CASES = {}


def case(name, params=None, check=no_check, playing=False):
    CASES[name] = {"params": params or {}, "check": check, "playing": playing}


# ── reads ───────────────────────────────────────────────────────────────────
case("get_session_info", {}, lambda t, r, s: t.assertEqual(r["track_count"], len(s.tracks)))
case("get_track_info", {"track_index": 0}, lambda t, r, s: t.assertEqual(r["name"], "Kick"))
case("get_script_info", {}, lambda t, r, s: t.assertEqual(r["protocol_version"], 2))
case("get_clip_notes", {"track_index": 0, "clip_index": 0},
     lambda t, r, s: t.assertEqual(len(r["notes"]), 4))
case("get_device_parameters", {"track_index": 0, "device_index": 0},
     lambda t, r, s: t.assertEqual(r["device"]["name"], "Analog"))
case("get_session_snapshot", {},
     lambda t, r, s: t.assertEqual(len(r["tracks"]), len(s.tracks)))
case("get_arrangement_clips", {"track_index": 0},
     lambda t, r, s: t.assertEqual(len(r["clips"]), 1))
case("get_returns", {}, lambda t, r, s: t.assertEqual(len(r["returns"]), 2))
case("get_clip_info", {"track_index": 0, "clip_index": 0},
     lambda t, r, s: t.assertEqual(r["name"], "Kick 1"))
case("get_track_meters", {}, lambda t, r, s: t.assertEqual(len(r["tracks"]), len(s.tracks)))
case("get_meter_scale", {}, lambda t, r, s: t.assertIn("points", r))
case("get_library_status", {},
     lambda t, r, s: t.assertIn("Analog", [i["name"] for i in r["instruments"]]))
case("get_performance_state", {},
     lambda t, r, s: t.assertEqual(len(r["tracks"]), len(s.tracks)))
case("get_grooves", {}, lambda t, r, s: t.assertIn("can_add", r))
case("get_context", {}, lambda t, r, s: t.assertIn("tracks", r))
case("get_drum_rack_pads", {"track_index": 1},
     lambda t, r, s: t.assertEqual(
         [p["pad_name"] for p in r["pads"]][:4], ["Kick", "Snare", "Hat Closed", "Hat Open"]))
case("get_clip_automation", {"track_index": 0, "clip_index": 0,
                             "target": {"mixer": "volume"}},
     lambda t, r, s: t.assertIn("has_envelope", r))
def _categories_are_browsable(t, r, s):
    """#58: `available_categories` is what you may ask for, so every name in
    it has to be a place to browse. It used to be `dir(browser)`, which
    offered `load_item`, `preview_item` and the listener boilerplate."""
    t.assertIn("categories", r)
    offered = r["available_categories"]
    t.assertTrue(offered, "no categories offered")
    for name in offered:
        t.assertFalse(name.endswith("_listener"), "%s is a listener, not a category" % name)
        t.assertNotIn(name, ("load_item", "preview_item", "stop_preview",
                             "relation_to_hotswap_target", "hotswap_target",
                             "filter_type", "colors"),
                      "%s is not a place to browse" % name)
    t.assertIn("instruments", offered)
    t.assertIn("drums", offered)


case("get_browser_tree", {"category_type": "all"}, _categories_are_browsable)
case("get_browser_items_at_path", {"path": "instruments"},
     lambda t, r, s: t.assertTrue(r["items"]))
case("get_browser_index", {"category": "instruments", "limit": 50},
     lambda t, r, s: t.assertIn("Analog", [i["name"] for i in r["items"]]))
case("search_browser", {"query": "analog", "limit": 5},
     lambda t, r, s: t.assertEqual([i["name"] for i in r["items"]], ["Analog"]))
case("arrangement_summary", {}, lambda t, r, s: t.assertIn("tracks", r))
case("list_captures", {}, lambda t, r, s: t.assertIn("captures", r))
case("list_sample_folders", {}, lambda t, r, s: t.assertIn("folders", r))
case("drain_passive_events", {}, lambda t, r, s: t.assertIn("events", r))
case("capture_status", {"slot": 0}, lambda t, r, s: t.assertIn("is_recording", r))
case("describe", {"path": "song.tracks[0]"},
     lambda t, r, s: t.assertEqual(r["class"], "Track"))
case("run", {"ops": [{"op": "get", "path": "song.tempo", "as": "tempo"}]},
     lambda t, r, s: t.assertEqual(r["tempo"], 120.0))


# ── writes: the set is asked afterwards ─────────────────────────────────────
def _tempo(t, r, s):
    t.assertEqual(s.tempo, 128.0)


case("set_tempo", {"tempo": 128.0}, _tempo)


def _midi_track(t, r, s):
    t.assertEqual(len(s.tracks), 5)
    t.assertTrue(s.tracks[-1].has_midi_input)


case("create_midi_track", {"index": -1}, _midi_track)


def _audio_track(t, r, s):
    t.assertEqual(len(s.tracks), 5)
    t.assertTrue(s.tracks[-1].has_audio_input)


case("create_audio_track", {"index": -1}, _audio_track)
case("set_track_name", {"track_index": 0, "name": "Kick Drum"},
     lambda t, r, s: t.assertEqual(s.tracks[0].name, "Kick Drum"))
case("create_clip", {"track_index": 1, "clip_index": 1, "length": 8.0},
     lambda t, r, s: t.assertEqual(s.tracks[1].clip_slots[1].clip.length, 8.0))
case("create_audio_clip", {"track_index": 2, "clip_index": 0, "path": "/Samples/kick.wav"},
     lambda t, r, s: t.assertEqual(s.tracks[2].clip_slots[0].clip.file_path, "/Samples/kick.wav"))
case("add_notes_to_clip",
     {"track_index": 0, "clip_index": 1,
      "notes": [{"pitch": 60, "start_time": 0.0, "duration": 1.0, "velocity": 100}]},
     lambda t, r, s: t.assertEqual(len(s.tracks[0].clip_slots[1].clip._notes), 1))
case("clear_notes_from_clip", {"track_index": 0, "clip_index": 0},
     lambda t, r, s: t.assertEqual(s.tracks[0].clip_slots[0].clip._notes, []))
case("set_clip_name", {"track_index": 0, "clip_index": 0, "name": "Four"},
     lambda t, r, s: t.assertEqual(s.tracks[0].clip_slots[0].clip.name, "Four"))
case("set_arrangement_clip_name", {"track_index": 0, "clip_index": 0, "name": "Take"},
     lambda t, r, s: t.assertEqual(s.tracks[0].arrangement_clips[0].name, "Take"))
case("delete_clip", {"track_index": 0, "clip_index": 1},
     lambda t, r, s: t.assertFalse(s.tracks[0].clip_slots[1].has_clip))
case("fire_clip", {"track_index": 0, "clip_index": 0},
     lambda t, r, s: t.assertEqual(s.tracks[0].playing_slot_index, 0))
case("stop_clip", {"track_index": 0, "clip_index": 0},
     lambda t, r, s: t.assertEqual(s.tracks[0].playing_slot_index, -1))
case("start_playback", {}, lambda t, r, s: t.assertTrue(s.is_playing))
case("stop_playback", {}, lambda t, r, s: t.assertFalse(s.is_playing), playing=True)
case("load_browser_item", {"track_index": 1, "item_uri": "query:AudioFx#Reverb"},
     lambda t, r, s: t.assertIn("Reverb", [d.name for d in s.tracks[1].devices]))
case("switch_to_arrangement_view", {},
     lambda t, r, s: t.assertTrue(s._application.view.is_view_visible("Arranger")))
case("set_current_song_time", {"time": 8.0},
     lambda t, r, s: t.assertAlmostEqual(s.current_song_time, 8.0))
case("duplicate_session_clip_to_arrangement",
     {"track_index": 0, "clip_index": 1, "destination_time": 16.0},
     lambda t, r, s: t.assertEqual(len(s.tracks[0].arrangement_clips), 2))
case("create_locator", {"name": "Drop", "time": 16.0},
     lambda t, r, s: t.assertIn("Drop", [c.name for c in s.cue_points]))
case("delete_locator", {"time": 0.0},
     lambda t, r, s: t.assertEqual(len(s.cue_points), 0))
case("set_track_mixer", {"track_index": 0, "volume_db": -6.0},
     lambda t, r, s: t.assertAlmostEqual(
         fake_live.fader_to_db(s.tracks[0].mixer_device.volume.value), -6.0, places=1))
case("set_send", {"track_index": 0, "send_index": 0, "value": 0.5},
     lambda t, r, s: t.assertAlmostEqual(s.tracks[0].mixer_device.sends[0].value, 0.5))
case("set_track_color", {"track_index": 0, "color_index": 7},
     lambda t, r, s: t.assertEqual(s.tracks[0].color_index, 7))
case("set_clip_color", {"track_index": 0, "clip_index": 0, "color_index": 9},
     lambda t, r, s: t.assertEqual(s.tracks[0].clip_slots[0].clip.color_index, 9))
case("delete_arrangement_clip", {"track_index": 0, "clip_index": 0},
     lambda t, r, s: t.assertEqual(len(s.tracks[0].arrangement_clips), 0))
case("set_clip_loop", {"track_index": 0, "clip_index": 0, "loop_end": 8.0},
     lambda t, r, s: t.assertAlmostEqual(s.tracks[0].clip_slots[0].clip.loop_end, 8.0))
case("set_clip_launch", {"track_index": 0, "clip_index": 0, "launch_mode": "gate"},
     lambda t, r, s: t.assertEqual(s.tracks[0].clip_slots[0].clip.launch_mode, 1))
case("set_clip_automation",
     {"track_index": 0, "clip_index": 0, "target": {"mixer": "volume"},
      "points": [{"time": 0.0, "value": 0.0}, {"time": 4.0, "value": 1.0}]},
     lambda t, r, s: t.assertTrue(
         s.tracks[0].clip_slots[0].clip.automation_envelope(
             s.tracks[0].mixer_device.volume) is not None))
case("play_from", {"time": 4.0},
     lambda t, r, s: t.assertTrue(s.is_playing))
case("delete_track", {"track_index": 3},
     lambda t, r, s: t.assertEqual(len(s.tracks), 3))
case("reset_set", {},
     lambda t, r, s: (t.assertEqual([x.name for x in s.tracks],
                                    ["1-MIDI", "2-MIDI", "3-Audio", "4-Audio"]),
                      t.assertEqual(len(s.scenes), 8),
                      t.assertEqual(s.tempo, 120.0),
                      t.assertEqual(s.loop_start, 0.0),
                      t.assertFalse(any(sl.has_clip for tr in s.tracks for sl in tr.clip_slots)),
                      t.assertFalse(any(tr.arrangement_clips for tr in s.tracks)),
                      t.assertEqual(len(s.cue_points), 0),
                      t.assertEqual(len(s.return_tracks), 2)))
case("back_to_arrangement", {}, lambda t, r, s: t.assertFalse(s.back_to_arranger))
case("set_arrangement_loop", {"start": 0.0, "length": 16.0, "enabled": True},
     lambda t, r, s: t.assertTrue(s.loop) or t.assertAlmostEqual(s.loop_length, 16.0))
case("set_launch_quantization", {"name": "1_bar"},
     lambda t, r, s: t.assertEqual(s.clip_trigger_quantization, fake_live.Q_BAR))
case("create_scene", {"index": -1, "name": "Outro", "phrase_bars": 8},
     lambda t, r, s: t.assertEqual(len(s.scenes), 9))
case("set_scene", {"index": 0, "name": "Verse", "phrase_bars": 16},
     lambda t, r, s: t.assertIn("Verse", s.scenes[0].name))
case("fire_scene", {"scene_index": 0},
     lambda t, r, s: t.assertEqual(s.tracks[0].playing_slot_index, 0))
case("stop_all_clips", {},
     lambda t, r, s: t.assertEqual([x.playing_slot_index for x in s.tracks], [-1] * len(s.tracks)))
case("capture_scene", {"name": "Held"}, lambda t, r, s: t.assertEqual(len(s.scenes), 9))
case("duplicate_scene", {"index": 0, "name": "Intro 2"},
     lambda t, r, s: t.assertEqual(len(s.scenes), 9))
# The artist's 0..1 (A..B) onto Live's raw -1..1: 0.75 is halfway to B.
case("set_crossfader", {"value": 0.75},
     lambda t, r, s: t.assertAlmostEqual(s.master_track.mixer_device.crossfader.value, 0.5))
case("set_scale", {"root_note": 2, "scale_name": "Minor"},
     lambda t, r, s: t.assertEqual((s.root_note, s.scale_name), (2, "Minor")))
case("set_slot_stop_buttons", {"track_index": 0, "has_stop_button": False},
     lambda t, r, s: t.assertFalse(s.tracks[0].clip_slots[2].has_stop_button))
case("set_performance_mode", {"on": True}, no_check)
# Parameter 0 is Device On, which is quantized; 1 is Filter Freq.
case("set_device_parameter",
     {"track_index": 0, "device_index": 0, "parameter_index": 1, "value": 0.25},
     lambda t, r, s: t.assertAlmostEqual(s.tracks[0].devices[0].parameters[1].value, 0.25))
case("set_device_parameters",
     {"track_index": 0, "device_index": 0, "values": [{"index": 2, "value": 0.75}]},
     lambda t, r, s: t.assertAlmostEqual(s.tracks[0].devices[0].parameters[2].value, 0.75))
case("delete_device", {"track_index": 0, "device_index": 0},
     lambda t, r, s: t.assertEqual(len(s.tracks[0].devices), 0))
case("move_device", {"track_index": 1, "device_index": 0, "to_index": 0},
     lambda t, r, s: t.assertEqual(len(s.tracks[1].devices), 1))
case("place_clips", {"track_index": 0, "clip_index": 0, "times": [16.0, 32.0]},
     lambda t, r, s: t.assertEqual(len(s.tracks[0].arrangement_clips), 3))
case("delete_arrangement_clips", {"track_index": 0, "all": True},
     lambda t, r, s: t.assertEqual(len(s.tracks[0].arrangement_clips), 0))
case("duplicate_arrangement_clip", {"track_index": 0, "clip_index": 0, "times": [16.0]},
     lambda t, r, s: t.assertEqual(len(s.tracks[0].arrangement_clips), 2))
case("create_return_track", {"name": "C Delay"},
     lambda t, r, s: t.assertEqual(s.return_tracks[-1].name, "C Delay"))
case("create_tracks",
     {"tracks": [{"name": "Pad", "kind": "midi", "instrument_uri": "query:Synths#Operator"},
                 {"name": "FX", "kind": "audio"}]},
     lambda t, r, s: t.assertEqual([x.name for x in s.tracks[-2:]], ["Pad", "FX"]))
case("write_clips",
     {"clips": [{"track_index": 1, "clip_index": 1, "length": 4.0, "name": "Bass 2",
                 "notes": [{"pitch": 38, "start_time": 0.0, "duration": 1.0, "velocity": 90}]}]},
     lambda t, r, s: t.assertEqual(s.tracks[1].clip_slots[1].clip.name, "Bass 2"))
case("set_clip_groove", {"global_amount": 0.5},
     lambda t, r, s: t.assertAlmostEqual(s.groove_amount, 0.5))
case("start_arrangement_record", {"from_beat": 0.0},
     lambda t, r, s: t.assertTrue(s.record_mode))
case("stop_arrangement_record", {}, lambda t, r, s: t.assertFalse(s.record_mode))
case("place_sample",
     {"track_index": 2, "path": "/Samples/kick.wav", "slot": 0, "name": "Kick sample"},
     lambda t, r, s: t.assertEqual(s.tracks[2].clip_slots[0].clip.name, "Kick sample"))
case("record_clip", {"track_index": 0, "bars": 4},
     lambda t, r, s: t.assertTrue(s.tracks[0].arm))
case("snapshot_mix", {}, lambda t, r, s: t.assertIn("id", r))
case("restore_mix", {"id": 1}, no_check)
case("ensure_capture_track", {},
     lambda t, r, s: t.assertIn("Capture", [x.name for x in s.tracks]))
case("start_capture", {"start": 0.0, "bars": 2, "name": "take"},
     lambda t, r, s: t.assertIn("Capture", [x.name for x in s.tracks]))
case("stop_capture", {}, no_check)
case("start_live_capture", {"bars": 2, "name": "live take"},
     lambda t, r, s: t.assertIn("Capture", [x.name for x in s.tracks]))
case("schedule_cue",
     {"steps": [{"beat": 4.0, "action": "set", "target": "tempo", "value": 130.0}]},
     lambda t, r, s: t.assertTrue(r["steps"]), playing=True)
case("cancel_cue", {"reason": "test"}, no_check, playing=True)
case("subscribe", {"channels": ["clock"]},
     lambda t, r, s: t.assertEqual(r["subscribed"], ["clock"]))
case("unsubscribe", {"channels": ["clock"]}, no_check)


# ── The suite ───────────────────────────────────────────────────────────────

class EveryCommand(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        self.ns = harness.load(song=command_set())
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.sock, self.client = connect(self.script, self.ns)

    # Commands the script answers without going near the executor, because
    # they touch nothing in Live: the handshake, and the per-socket
    # subscription bookkeeping. `_client_start` routes them, not `_dispatch`.
    OFF_THE_EXECUTOR = ("get_script_info", "subscribe", "unsubscribe")

    # Handlers that answer from a LATER tick (`DEFERRED`): Live applies the
    # first step asynchronously, so the reply is written by `_answer`, which
    # does not go through the executor's `finish()` and so carries no
    # `main_ms`. Recorded here as what the script does today rather than
    # asserted away; the server treats `main_ms` as optional.
    NO_MAIN_MS = ("create_locator", "delete_locator", "play_from", "start_capture")

    def run_command(self, name, params, limit=400):
        """One command the whole way a socket request goes: `_run_on_main`,
        the executor, its slices, the undo step, `main_ms`."""
        if name == "get_script_info":
            return {"status": "success", "result": self.script._get_script_info()}
        if name in ("subscribe", "unsubscribe"):
            handler = self.script._subscribe if name == "subscribe" else self.script._unsubscribe
            return {"status": "success", "result": handler(self.client, params)}
        sink = Sink()
        self.script._run_on_main(name, params, sink)
        for _ in range(limit):
            if sink.payload is not None:
                break
            self.script.tick()
        self.assertIsNotNone(sink.payload, "%s never answered in %d ticks" % (name, limit))
        return sink.payload

    # ── the one-home rule ──────────────────────────────────────────────────
    def test_every_command_in_the_servers_list_has_a_case(self):
        listed = set(all_remote_commands())
        self.assertGreater(len(listed), 90, "ALL_REMOTE_COMMANDS did not parse")
        missing = sorted(listed - set(CASES))
        extra = sorted(set(CASES) - listed)
        self.assertEqual(missing, [], "commands the server asks for with no case here: %s" % missing)
        self.assertEqual(extra, [], "cases for commands the server does not ask for: %s" % extra)

    # ── every one of them, for real ────────────────────────────────────────
    def test_every_command_runs_against_the_model_and_says_success(self):
        failures = []
        for name in sorted(CASES):
            spec = CASES[name]
            with self.subTest(command=name):
                self.setUp()
                if spec["playing"]:
                    self.song.start_playing()
                if name == "restore_mix":
                    self.run_command("snapshot_mix", {})
                if name == "cancel_cue":
                    self.run_command("schedule_cue", {"steps": [
                        {"beat": 8.0, "action": "set", "target": "tempo", "value": 130.0}]})
                if name == "stop_capture":
                    self.run_command("start_capture", {"start": 0.0, "bars": 2, "name": "t"})
                if name == "unsubscribe":
                    self.run_command("subscribe", {"channels": ["clock"]})
                if name in ("capture_status", "start_capture", "stop_capture"):
                    self.run_command("ensure_capture_track", {})
                payload = self.run_command(name, spec["params"])
                if payload.get("status") != "success":
                    failures.append("%s: %s" % (name, payload.get("message")))
                    continue
                if name in self.OFF_THE_EXECUTOR or name in self.NO_MAIN_MS:
                    self.assertNotIn("main_ms", payload,
                                     "%s grew a main_ms; move it out of the list" % name)
                else:
                    self.assertIn("main_ms", payload, "%s: no main_ms on the reply" % name)
                try:
                    spec["check"](self, payload.get("result"), self.song)
                except AssertionError as e:
                    failures.append("%s: the set did not change as expected: %s" % (name, e))
        self.assertEqual(failures, [], "commands that failed against the model:\n  " +
                         "\n  ".join(failures))

    def test_a_generator_command_is_spread_over_ticks_not_run_in_one(self):
        """`create_tracks` yields per track; with Live's measured cost the
        executor's 40 ms stopped budget cannot take twenty in one slice."""
        self.ns = harness.load(song=command_set(), clock=fake_live.WallClock(),
                               latency=fake_live.LatencyTable.measured(scale=0.06))
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        payload = self.run_command("create_tracks", {"tracks": [
            {"name": "T%d" % i, "kind": "midi", "instrument_uri": "query:Synths#Analog"}
            for i in range(20)]})
        self.assertEqual(payload["status"], "success")
        self.assertGreater(payload["slices"], 1, "twenty tracks came back in one slice")
        self.assertEqual([t.name for t in self.song.tracks[-20:]],
                         ["T%d" % i for i in range(20)])


if __name__ == "__main__":
    unittest.main()
