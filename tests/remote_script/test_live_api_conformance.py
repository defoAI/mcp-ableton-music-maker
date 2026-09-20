"""The fake Live Object Model covers what the Remote Script asks of Live.

`scripts/live-api-surface.py` reads every Live API member the script
touches out of the script by AST. This test asserts each one exists on the
matching class in `fake_live` with the right shape: a method where the
script calls it, a writable property where the script assigns it, and any
member where it only reads. A member the script starts using that the model
lacks fails here, not in a Rust suite that runs against the model.
"""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "scripts"))

import fake_live  # noqa: E402

# What live-api-surface.py calls a receiver -> the class in fake_live that is it.
CLASSES = {
    "Song": fake_live.Song,
    "Track": fake_live.Track,
    "ClipSlot": fake_live.ClipSlot,
    "Clip": fake_live.Clip,
    "Device": fake_live.Device,
    "DeviceParameter": fake_live.DeviceParameter,
    "MixerDevice": fake_live.MixerDevice,
    "Scene": fake_live.Scene,
    "CuePoint": fake_live.CuePoint,
    "Browser": fake_live.Browser,
    "BrowserItem": fake_live.BrowserItem,
    "Application": fake_live.Application,
    "AutomationEnvelope": fake_live.AutomationEnvelope,
    "Note": fake_live.MidiNote,
    "View": None,  # the script uses both Song.View and Application.View
}
VIEW_CLASSES = (fake_live.SongView, fake_live.ApplicationView)

# Live's note objects carry their fields as plain attributes, not
# descriptors, so those are checked on an instance.
INSTANCES = {"Note": lambda: fake_live.MidiNote(60, 0.0, 1.0)}

# Members the script reaches for that a real Live does not have. The script
# probes each behind `getattr`/`try`, and the model matches Live rather than
# the probe — so the fallback path is the one that runs, here as in Live.
# Verified against Live 12.4.6 on 2026-09-20 with
# `describe song.tracks[0].mixer_device.volume`: neither is in `attrs`.
DELIBERATELY_ABSENT = {
    # The script's own comment says so: "value_string is not in the LOM, so
    # it is only a fallback for the current value."
    ("DeviceParameter", "value_string"),
    # Live has `automation_state`; `is_automated` is not on DeviceParameter.
    ("DeviceParameter", "is_automated"),
}


def load_surface():
    import importlib.util
    spec = importlib.util.spec_from_file_location(
        "live_api_surface", os.path.join(ROOT, "scripts", "live-api-surface.py"))
    saved = sys.argv
    sys.argv = ["live-api-surface.py", "--quiet"]
    try:
        mod = importlib.util.module_from_spec(spec)
        import io, contextlib
        with contextlib.redirect_stdout(io.StringIO()):
            spec.loader.exec_module(mod)
    finally:
        sys.argv = saved
    return mod.surface()


def shape_of(cls, member, instance=None):
    """(exists, callable, writable) for a member on a class, or on an
    instance when the class keeps its fields as plain attributes."""
    if instance is not None:
        if not hasattr(instance, member):
            return (False, False, False)
        return (True, callable(getattr(instance, member)), True)
    attr = getattr(cls, member, None)
    if attr is None and member not in dir(cls):
        return (False, False, False)
    if isinstance(attr, property):
        return (True, False, attr.fset is not None)
    return (True, callable(attr), True)


class Conformance(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        self.surface = load_surface()

    def classes_for(self, name):
        if name == "View":
            return VIEW_CLASSES
        return (CLASSES[name],)

    def test_every_member_the_script_touches_is_on_the_model(self):
        missing, wrong = [], []
        total = 0
        for cls_name, members in sorted(self.surface.items()):
            for member, kinds in sorted(members.items()):
                total += 1
                if (cls_name, member) in DELIBERATELY_ABSENT:
                    continue
                instance = INSTANCES[cls_name]() if cls_name in INSTANCES else None
                shapes = [shape_of(c, member, instance) for c in self.classes_for(cls_name)]
                present = [s for s in shapes if s[0]]
                if not present:
                    missing.append("%s.%s (%s)" % (cls_name, member, ",".join(kinds)))
                    continue
                exists, is_callable, writable = present[0]
                if "call" in kinds and not is_callable:
                    wrong.append("%s.%s is called but is not a method" % (cls_name, member))
                if "set" in kinds and not writable:
                    wrong.append("%s.%s is assigned but is read-only" % (cls_name, member))
        covered = total - len(missing)
        report = "%d of %d Live API members covered" % (covered, total)
        self.assertEqual(missing, [], "%s; missing:\n  %s" % (report, "\n  ".join(missing)))
        self.assertEqual(wrong, [], "%s; wrong shape:\n  %s" % (report, "\n  ".join(wrong)))

    def test_sequences_are_vectors_as_in_live(self):
        """Live's sequence type is its own `Vector` — `describe song.tracks`
        on 12.4.6 reports `class: Vector`, and it is neither a list nor a
        tuple, which is why `run … get` hands back its repr."""
        s = fake_live.default_set()
        for seq in (s.tracks, s.return_tracks, s.scenes, s.cue_points,
                    s.tracks[0].clip_slots, s.tracks[0].devices, s.tracks[0].arrangement_clips,
                    s.tracks[0].mixer_device.sends, s.return_tracks[0].devices[0].parameters,
                    s.master_track.mixer_device.sends):
            self.assertIsInstance(seq, fake_live.Vector)
            self.assertNotIsInstance(seq, (list, tuple))

    def test_describe_sees_writable_and_read_only_as_live_has_them(self):
        """What `describe` derives from `fset`, on members the script sets."""
        writable = [(fake_live.Song, "tempo"), (fake_live.Track, "name"), (fake_live.Track, "mute"),
                    # Live reports these writable; verified 2026-09-20.
                    (fake_live.Song, "is_playing"), (fake_live.Track, "color"),
                    (fake_live.Clip, "name"), (fake_live.Clip, "loop_end"), (fake_live.Scene, "name"),
                    (fake_live.DeviceParameter, "value"), (fake_live.MixerDevice, "crossfade_assign")]
        read_only = [(fake_live.Song, "exclusive_arm"), (fake_live.Song, "tracks"), (fake_live.Clip, "length"),
                     (fake_live.Clip, "is_midi_clip"), (fake_live.Track, "clip_slots"),
                     (fake_live.DeviceParameter, "min"), (fake_live.DeviceParameter, "name")]
        for cls, m in writable:
            self.assertIsNotNone(getattr(cls, m).fset, "%s.%s should be writable" % (cls.__name__, m))
        for cls, m in read_only:
            self.assertIsNone(getattr(cls, m).fset, "%s.%s should be read-only" % (cls.__name__, m))


if __name__ == "__main__":
    unittest.main()
