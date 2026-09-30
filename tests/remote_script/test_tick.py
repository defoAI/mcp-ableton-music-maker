"""Phase 0 of the streams story: the tick period is measured and reported."""
import unittest

import harness


class TickSampler(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()
        self.Sampler = self.ns["_TickSampler"]

    def test_empty_sampler_reports_nothing_rather_than_a_guess(self):
        st = self.Sampler().stats()
        self.assertEqual(st, {"period_ms": None, "jitter_ms": None, "max_ms": None, "samples": 0})

    def test_period_is_the_median_and_jitter_the_spread(self):
        s = self.Sampler()
        # 16.7 ms ticks, one late one at 40 ms: the median holds, the max tells.
        t = 100.0
        s.note(t)
        for i in range(20):
            t += 0.040 if i == 7 else 0.0167
            s.note(t)
        st = s.stats()
        self.assertEqual(st["samples"], 20)
        self.assertAlmostEqual(st["period_ms"], 16.7, places=1)
        self.assertAlmostEqual(st["max_ms"], 40.0, places=1)
        self.assertGreaterEqual(st["jitter_ms"], 0.0)
        self.assertLess(st["jitter_ms"], 24.0)

    def test_a_flat_two_hundred_ms_tick_is_reported_as_such(self):
        s = self.Sampler()
        t = 0.0
        s.note(t)
        for _ in range(50):
            t += 0.2
            s.note(t)
        st = s.stats()
        self.assertAlmostEqual(st["period_ms"], 200.0, places=1)
        self.assertAlmostEqual(st["jitter_ms"], 0.0, places=1)

    def test_the_ring_forgets_the_oldest(self):
        s = self.Sampler(capacity=10)
        t = 0.0
        s.note(t)
        for _ in range(5):
            t += 0.5
            s.note(t)
        for _ in range(10):
            t += 0.01
            s.note(t)
        st = s.stats()
        self.assertEqual(st["samples"], 10)
        self.assertAlmostEqual(st["period_ms"], 10.0, places=1)
        self.assertAlmostEqual(st["max_ms"], 10.0, places=1, msg="the 500 ms intervals fell out")


class ClockTick(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()
        self.script = harness.instance(self.ns)

    def test_the_tick_is_armed_at_start_and_rearms_itself(self):
        # After construction one callback is pending; each tick runs it and
        # leaves exactly one pending again, for the life of the script.
        self.assertEqual(len(self.script.scheduled), 1)
        for _ in range(5):
            ran = self.script.tick()
            self.assertEqual(ran, 1)
            self.assertEqual(len(self.script.scheduled), 1)
        self.assertEqual(self.script._tick_sampler.stats()["samples"], 4)

    def test_disconnect_lets_the_tick_lapse(self):
        self.script.running = False
        self.script.tick()
        self.assertEqual(len(self.script.scheduled), 0, "a stopped script must not keep Live calling back")

    def test_live_version_is_read_once_on_the_main_thread(self):
        self.assertIsNone(self.script._live_version)
        self.script.tick()
        self.assertEqual(self.script._live_version, "12.4.6")

    def test_the_handshake_reports_the_tick_without_touching_live(self):
        self.script.tick(3)
        info = self.script._get_script_info()
        self.assertIn(info["socket_reader"], ("main_thread_tick", "background_thread"))
        self.assertEqual(info["protocol_version"], 2)
        self.assertEqual(info["live"]["version"], "12.4.6")
        self.assertIn("python", info["live"])
        tick = info["tick"]
        for key in ("period_ms", "jitter_ms", "max_ms", "samples", "playing"):
            self.assertIn(key, tick)
        self.assertEqual(tick["samples"], 2)
        self.assertIs(tick["playing"], False)

    def test_the_tick_survives_a_failing_callback(self):
        self.script.application = lambda: (_ for _ in ()).throw(RuntimeError("no app"))
        self.script.tick(2)
        self.assertEqual(self.script._live_version, "unknown")
        self.assertEqual(len(self.script.scheduled), 1)


if __name__ == "__main__":
    unittest.main()
