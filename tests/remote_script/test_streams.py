"""Phase 2: the clock, the levels and the changes come to the server.

Events go only to sockets that asked for them, are computed on the tick from
reads (never a listener on a clip, which is what deadlocked Live during the
first capture), and stop when nobody is listening.
"""
import json
import time
import unittest

import harness
from harness import two_track_set
from test_duplex import FakeSock, connect


class Streams(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load(song=two_track_set())
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.song.start_playing()
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
        self.song.stop_playing()
        self.script.tick()
        count = len(self.events("clock"))
        self.script.tick(5)
        self.assertEqual(len(self.events("clock")), count, "a stopped transport kept sending")
        self.song.start_playing()
        self.script.tick()
        self.assertGreater(len(self.events("clock")), count, "it did not resume")

    # ── the clock's rate ───────────────────────────────────────────────────
    def live_ticks(self, count, period=0.10005, jitter=0.01):
        """Live's own tick as it was measured: about 100.05 ms apart with
        roughly 10 ms of jitter either way, so half the ticks land early.
        It runs from now, because subscribing already put the schedule on
        the real clock."""
        out, t = [], self.later()
        for i in range(count):
            out.append(t + (-jitter if i % 2 else jitter))
            t += period
        return out

    def later(self, seconds=1.0):
        """An instant past anything the harness's own ticks scheduled."""
        return time.time() + seconds

    def clock_at(self, times):
        """Drive the clock channel over those wall-clock instants and count
        the events it published."""
        before = len(self.events("clock"))
        for t in times:
            self.script._clock_event_tick(t)
        self.script._flush_clients()
        return len(self.events("clock")) - before

    def test_a_100_ms_subscription_gets_an_event_on_every_tick(self):
        # An early tick used to fail a hard floor at the interval, and a skip
        # costs a whole tick: 100 ms asked for arrived every 199 ms (#51).
        self.subscribe(["clock"], clock_every_ms=100)
        self.assertEqual(self.clock_at(self.live_ticks(20)), 20)

    def test_a_faster_tick_still_delivers_the_rate_that_was_asked_for(self):
        # Live's tick is not a constant: on a busy set it has been seen well
        # under 100 ms. The schedule advances by the interval, so the rate
        # holds whatever the tick is -- 50 ticks 60 ms apart span 2.945 s and
        # carry 30 events, a shade over the 10/s asked for. Measuring from
        # the last send instead rounds every interval up to the next tick and
        # delivers 25.
        self.subscribe(["clock"], clock_every_ms=100)
        self.assertEqual(self.clock_at(self.live_ticks(50, period=0.06, jitter=0.005)), 30)

    def test_a_long_gap_does_not_produce_a_burst_to_catch_up(self):
        self.subscribe(["clock"], clock_every_ms=100)
        t0 = self.later()
        self.clock_at([t0])
        # Live stalled for five seconds: the schedule is clamped forward, so
        # the next ticks are one event each, not fifty at once.
        self.assertEqual(self.clock_at([t0 + 5.0, t0 + 5.1, t0 + 5.2]), 3)

    def test_a_rate_change_takes_effect_at_once(self):
        # The fastest subscriber sets the rate. When one arrives asking for
        # every tick, the schedule built for the slower rate must not hold
        # the next event back.
        self.subscribe(["clock"], clock_every_ms=100)
        t0 = self.later()
        self.clock_at([t0])
        self.assertEqual(self.clock_at([t0 + 0.03]), 0, "100 ms was not held")
        other_sock, _ = connect(self.script, self.ns)
        other_sock.feed('{"id": 1, "type": "subscribe", "params": '
                        '{"channels": ["clock"], "clock_every_ms": 0}}\n')
        self.script.tick()
        self.assertEqual(self.clock_at([t0 + 0.06, t0 + 0.09, t0 + 0.12]), 3)

    # ── the levels ─────────────────────────────────────────────────────────
    def test_levels_arrive_once_a_bar_in_lives_own_meter_scale(self):
        # Real meters on the real set: the master and both tracks at 0.5.
        for t in (self.song.master_track,) + self.song.tracks:
            t._output_meter_level = 0.5
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
        self.song.create_midi_track()
        self.song.tracks[-1].name = "Pad"
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
        # The model has no add_..._listener anywhere, so ten ticks of every
        # channel passing proves the diff is reads only. Registering one in
        # Live is what deadlocked it during the first capture (#26).
        for t in self.song.tracks:
            self.assertFalse([a for a in dir(t) if "listener" in a])

    def test_the_event_tick_survives_a_broken_read(self):
        self.subscribe(["changes"])

        class Exploding(object):
            @property
            def name(self):
                raise RuntimeError("Live went away")
        # Straight into the private list: Live's `tracks` is a tuple and
        # nothing in the API can put a broken object there. The point is the
        # tick surviving a read that raises, whatever raised it.
        self.song._tracks.append(Exploding())
        self.script.tick(6)
        self.assertEqual(len(self.script.scheduled), 1, "the tick stopped re-arming")


if __name__ == "__main__":
    unittest.main()
