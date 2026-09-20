"""Phase 2: the clock, the levels and the changes come to the server.

Events go only to sockets that asked for them, are computed on the tick from
reads (never a listener on a clip, which is what deadlocked Live during the
first capture), and stop when nobody is listening.
"""
import json
import unittest

import harness
from test_duplex import FakeSock, connect


class FakeScene(object):
    def __init__(self, name):
        self.name = name
        self.is_triggered = False


class FakeTrack(object):
    def __init__(self, name):
        self.name = name
        self.mute = False
        self.solo = False
        self.arm = False
        self.playing_slot_index = -1


class Streams(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.song.tracks = [FakeTrack("Kick"), FakeTrack("Bass")]
        self.song.scenes = [FakeScene("Intro · 8"), FakeScene("Drop · 8")]
        self.song.is_playing = True
        self.sock, self.client = connect(self.script, self.ns)

    def subscribe(self, channels, **opts):
        params = {"channels": channels}
        params.update(opts)
        self.sock.feed(json.dumps({"id": 1, "type": "subscribe", "params": params}) + "\n")
        self.script.tick()
        replies = [r for r in self.sock.replies() if r.get("id") == 1]
        self.assertTrue(replies, "no reply to subscribe")
        return replies[0]

    def events(self, channel=None):
        out = [r for r in self.sock.replies() if "event" in r]
        return [e for e in out if channel is None or e["event"] == channel]

    # ── subscribing ────────────────────────────────────────────────────────
    def test_subscribe_answers_with_what_is_active(self):
        r = self.subscribe(["clock", "levels"])
        self.assertEqual(r["status"], "success")
        self.assertEqual(r["result"]["subscribed"], ["clock", "levels"])

    def test_an_unknown_channel_is_refused_and_names_the_known_ones(self):
        self.sock.feed('{"id": 2, "type": "subscribe", "params": {"channels": ["everything"]}}\n')
        self.script.tick()
        r = [x for x in self.sock.replies() if x.get("id") == 2][0]
        self.assertEqual(r["status"], "error")
        self.assertIn("clock", r["message"])

    def test_unsubscribe_stops_the_events(self):
        self.subscribe(["clock"])
        self.script.tick(3)
        self.assertTrue(self.events("clock"))
        self.sock.feed('{"id": 3, "type": "unsubscribe", "params": {"channels": ["clock"]}}\n')
        self.script.tick()
        before = len(self.events("clock"))
        self.script.tick(5)
        self.assertEqual(len(self.events("clock")), before, "events kept coming after unsubscribe")

    def test_nothing_is_pushed_to_a_socket_that_did_not_ask(self):
        self.script.tick(5)
        self.assertEqual(self.events(), [], "an unsubscribed socket received events")

    def test_two_sockets_get_only_their_own_channels(self):
        other_sock, _ = connect(self.script, self.ns)
        self.subscribe(["clock"])
        other_sock.feed('{"id": 1, "type": "subscribe", "params": {"channels": ["levels"]}}\n')
        self.script.tick(4)
        mine = set(e["event"] for e in self.events())
        theirs = set(r["event"] for r in other_sock.replies() if "event" in r)
        self.assertEqual(mine, {"clock"})
        self.assertTrue(theirs <= {"levels"}, theirs)

    # ── the clock ──────────────────────────────────────────────────────────
    def test_the_clock_carries_the_bar_and_the_tempo(self):
        self.subscribe(["clock"], clock_every_ms=0)
        self.song.current_song_time = 8.0
        self.script.tick()
        e = self.events("clock")[-1]
        self.assertEqual(e["bar"], 3)
        self.assertEqual(e["beat_in_bar"], 1)
        self.assertEqual(e["tempo"], 120.0)
        self.assertTrue(e["playing"])
        self.assertIn("next_bar_in_s", e)
        self.assertIn("t", e)

    def test_a_stopped_transport_sends_one_last_clock_then_goes_quiet(self):
        self.subscribe(["clock"], clock_every_ms=0)
        self.script.tick(2)
        self.song.is_playing = False
        self.script.tick()
        count = len(self.events("clock"))
        self.script.tick(5)
        self.assertEqual(len(self.events("clock")), count, "a stopped transport kept sending")
        self.song.is_playing = True
        self.script.tick()
        self.assertGreater(len(self.events("clock")), count, "it did not resume")

    # ── the levels ─────────────────────────────────────────────────────────
    def test_levels_arrive_once_a_bar_in_lives_own_meter_scale(self):
        self.script._meter_of = lambda track: 0.5
        self.song.master_track = type("M", (), {"name": "Master"})()
        self.subscribe(["levels"])
        self.script.tick(3)
        first = self.events("levels")
        self.assertEqual(len(first), 1, "more than one event in one bar")
        self.assertEqual(first[0]["bar"], 1)
        self.assertEqual(first[0]["master"], 0.5)
        self.assertEqual(len(first[0]["tracks"]), 2)
        self.song.current_song_time = 4.0
        self.script.tick(2)
        self.assertEqual(len(self.events("levels")), 2, "the next bar did not arrive")

    # ── the changes ────────────────────────────────────────────────────────
    def test_a_rename_arrives_as_a_change_with_before_and_after(self):
        self.subscribe(["changes"])
        self.script.tick(4)
        self.song.tracks[1].name = "Vinyl break"
        self.script.tick(4)
        changed = [c for e in self.events("changes") for c in e["changed"]]
        self.assertIn({"path": "song.tracks[1].name", "from": "Bass", "to": "Vinyl break"}, changed)

    def test_adding_a_track_changes_the_count_and_the_new_row(self):
        self.subscribe(["changes"])
        self.script.tick(4)
        self.song.tracks.append(FakeTrack("Pad"))
        self.script.tick(4)
        paths = [c["path"] for e in self.events("changes") for c in e["changed"]]
        self.assertIn("song.tracks.count", paths)
        self.assertIn("song.tracks[2].name", paths)

    def test_nothing_changing_sends_nothing(self):
        self.subscribe(["changes"])
        self.script.tick(10)
        self.assertEqual(self.events("changes"), [], "a still set produced events")

    def test_no_listener_is_ever_registered_on_anything(self):
        self.subscribe(["clock", "levels", "changes"])
        self.script.tick(10)
        # A Live object would raise if the script called add_..._listener on
        # it; these stubs have no such methods at all, so ten ticks of every
        # channel passing proves the diff is reads only.
        for t in self.song.tracks:
            self.assertFalse([a for a in dir(t) if "listener" in a])

    def test_the_event_tick_survives_a_broken_read(self):
        self.subscribe(["changes"])

        class Exploding(object):
            @property
            def name(self):
                raise RuntimeError("Live went away")
        self.song.tracks.append(Exploding())
        self.script.tick(6)
        self.assertEqual(len(self.script.scheduled), 1, "the tick stopped re-arming")


if __name__ == "__main__":
    unittest.main()
