"""The Live Object Model the Remote Script uses matches a real Live's.

`scripts/live-lom-sweep.py` asks `describe` of a real Ableton Live and of
the fake for each of 25 paths, and compares attributes, methods, `readonly`,
types and the value of every readable attribute. This replays the real
Live's half — recorded into `tests/fixtures/live-lom-<version>.json` — so
the check runs in CI with no Live open.

**Only what the script touches has to match.** Live's Track carries about
150 methods and the Remote Script calls 22 of them;
`scripts/live-api-surface.py` reads that set out of the script, and a
difference on a member outside it is counted, not failed. Completing Live's
object model would be work with no reader, and the moment the script starts
using a member, it moves into scope here on its own — there is no list to
keep.

A model that *invents* a member is always in scope, whatever the script
uses: `describe` is a tool, and a member Live does not have is a lie told
to whoever reads it.

**With no fixture checked in this skips and says so.** It does not pass.
"""
import glob
import io
import contextlib
import importlib.util
import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)

FIXTURES = os.path.join(ROOT, "tests", "fixtures")
FAKE_LIVE = os.path.join(ROOT, "scripts", "fake-live.py")


def load_sweep():
    spec = importlib.util.spec_from_file_location(
        "live_lom_sweep", os.path.join(ROOT, "scripts", "live-lom-sweep.py"))
    module = importlib.util.module_from_spec(spec)
    with contextlib.redirect_stdout(io.StringIO()):
        spec.loader.exec_module(module)
    return module


def fixtures():
    return sorted(glob.glob(os.path.join(FIXTURES, "live-lom-*.json")))


class LomConformance(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls):
        cls.sweep = load_sweep()

    def fake_sweep(self):
        """The model's half, over a real socket to a real fake Live."""
        proc = subprocess.Popen(
            [sys.executable, "-u", FAKE_LIVE, "--quiet",
             "--exit-with-pid", str(os.getpid())],
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            line = proc.stdout.readline().decode("utf-8").strip()
            self.assertTrue(line.isdigit(), "the fake did not start: %r" % line)
            client = self.sweep.transcript.Client("127.0.0.1", int(line))
            try:
                return self.sweep.sweep(client, self.sweep.PATHS)
            finally:
                client.close()
        finally:
            proc.terminate()
            proc.wait(timeout=10)
            for pipe in (proc.stdout, proc.stderr):
                try:
                    pipe.close()
                except Exception:
                    pass

    def test_the_model_matches_live_on_everything_the_script_touches(self):
        found = fixtures()
        if not found:
            self.skipTest(
                "no tests/fixtures/live-lom-*.json — record one against a real Live with "
                "`scripts/live-lom-sweep.py --record-fixture`. Until then this is not a check.")
        used = self.sweep.used_surface()
        fake = self.fake_sweep()
        for path in found:
            with self.subTest(fixture=os.path.basename(path)):
                with open(path) as handle:
                    recorded = json.load(handle)
                rows = self.sweep.compare(recorded["paths"], fake, used)
                inside = [r for r in rows if r[3]]
                self.assertEqual(
                    [r[2] for r in inside], [],
                    "the model differs from Live %s on %d member(s) the Remote Script "
                    "uses:\n  %s\n\nFix the model in tests/remote_script/fake_live.py, or "
                    "if Live really changed, re-record with "
                    "`scripts/live-lom-sweep.py --record-fixture`."
                    % (recorded.get("live_version"), len(inside),
                       "\n  ".join(r[2] for r in inside)))

    def test_the_out_of_scope_gap_is_reported_not_hidden(self):
        """The rest of Live's model is counted, so the gap stays visible."""
        found = fixtures()
        if not found:
            self.skipTest("no LOM fixture recorded")
        used = self.sweep.used_surface()
        fake = self.fake_sweep()
        with open(found[0]) as handle:
            recorded = json.load(handle)
        rows = self.sweep.compare(recorded["paths"], fake, used)
        outside = [r for r in rows if not r[3]]
        self.assertGreater(
            len(outside), 0,
            "Live has more object model than the script asks for; if this is ever zero "
            "the scoping has broken, not the model")

    def test_every_described_path_is_classified(self):
        """A path with no class in CLASS_OF would have every difference on
        it treated as in scope, which is the safe way round but hides why."""
        for raw in self.sweep.PATHS:
            path = raw[:-1] if raw.endswith("?") else raw
            self.assertIn(path, self.sweep.CLASS_OF,
                          "%s has no Live class in CLASS_OF" % path)

    def test_the_inventory_is_the_scripts_own(self):
        """The scope comes from the script, not from a list kept by hand."""
        used = self.sweep.used_surface()
        self.assertIn("Track", used)
        self.assertIn("name", used["Track"])
        self.assertIn("clip_slots", used["Track"])
        # Something Live has that the script never touches stays out.
        self.assertNotIn("is_frozen", used.get("Track", set()))


if __name__ == "__main__":
    unittest.main()
