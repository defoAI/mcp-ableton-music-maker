#!/usr/bin/env python3
"""The Remote Script's pure helpers, exercised with a stub Live.

Live's own interpreter is the only place the script normally runs, so the
display-string logic, the meter curve and track convergence are checked here
against stand-ins for Live's objects. Run it after touching the script:

    python3 scripts/check-script-helpers.py

Importing the file with `_Framework` and `Live` stubbed also proves it still
parses under a plain interpreter.
"""
import io
import sys
import types

SCRIPT = __file__.rsplit("/scripts/", 1)[0] + "/AbletonMusicMaker_Remote_Script/__init__.py"


def load():
    """The script's namespace, with Live's own modules stubbed out."""
    framework = types.ModuleType("_Framework")
    surface = types.ModuleType("_Framework.ControlSurface")

    class ControlSurface(object):
        def __init__(self, c_instance=None):
            pass

        def log_message(self, message):
            pass

        def schedule_message(self, ticks, callback):
            pass

    surface.ControlSurface = ControlSurface
    framework.ControlSurface = surface
    sys.modules["_Framework"] = framework
    sys.modules["_Framework.ControlSurface"] = surface
    live = types.ModuleType("Live")
    live.Song = types.SimpleNamespace(
        Quantization=types.SimpleNamespace(q_bar=1, q_quarter=2, q_no_q=0)
    )
    sys.modules["Live"] = live
    namespace = {"__name__": "ableton_remote_script", "__file__": SCRIPT}
    exec(compile(io.open(SCRIPT, encoding="utf-8").read(), SCRIPT, "exec"), namespace)
    return namespace


class Param(object):
    """A parameter that behaves like Live's: a display curve and labels."""

    def __init__(self, name, value, low, high, show, items=None):
        self.name, self.value, self.min, self.max = name, value, low, high
        self._show, self.is_quantized = show, items is not None
        self.value_items = items or []

    def str_for_value(self, value):
        return self._show(value)


def freq_show(v):
    """Live's own shape for a filter frequency: 10 Hz to 22 kHz, logarithmic."""
    hz = 10.0 * (2200.0 ** v)
    return ("%.2f kHz" % (hz / 1000.0)) if hz >= 1000 else ("%.1f Hz" % hz)


FILTER_TYPES = [
    "High Pass 48dB", "High Pass 12dB", "Low Shelf", "Bell",
    "Notch", "High Shelf", "Low Pass 12dB", "Low Pass 48dB",
]


def check_display_strings(script):
    for text, number, unit in [
        ("1.20 kHz", 1200.0, "hz"), ("-6.0 dB", -6.0, "db"),
        ("12.0 ms", 0.012, "s"), ("100 %", 100.0, "%"),
        ("0.71", 0.71, ""), ("-inf dB", -80.0, "db"),
    ]:
        got = script._display_parts(text)
        assert abs(got[0] - number) < 1e-9 and got[1] == unit, (text, got)
    assert script._display_parts("Low Cut 48 dB")[0] is None
    print("display strings parse")

    freq = Param("1 Frequency A", 0.336, 0.0, 1.0, freq_show)
    for asked, expected in [("200 Hz", "200.0 Hz"), ("1.5 kHz", "1.50 kHz"), ("40 Hz", "40.0 Hz")]:
        value = script._value_from_display(freq, asked)
        assert freq_show(value) == expected, (asked, freq_show(value))
    print("a frequency is set by what Live shows")

    try:
        script._value_from_display(freq, "40 kHz")
        raise AssertionError("out of range must be refused")
    except ValueError as e:
        assert "outside that" in str(e), e

    drive = Param("Drive", 0.5, -36.0, 36.0, lambda v: "%.2f dB" % v)
    assert abs(script._value_from_display(drive, "3 dB") - 3.0) < 0.01

    chooser = Param("1 Filter Type A", 1.0, 0.0, 7.0,
                    lambda v: FILTER_TYPES[int(round(v))], FILTER_TYPES)
    assert script._value_from_display(chooser, "High Pass 48dB") == 0.0
    assert script._value_from_display(chooser, "low pass 12db") == 6.0
    try:
        script._value_from_display(chooser, "High Pass")
        raise AssertionError("a label the parameter does not have must be refused")
    except ValueError as e:
        assert "takes one of" in str(e) and "High Pass 48dB" in str(e), e
    print("a chooser takes its own labels, and refuses anything else")

    entry = script._serialize_parameter(chooser, 5)
    assert entry["display"] == "High Pass 12dB" and entry["items"] == FILTER_TYPES, entry
    entry = script._serialize_parameter(freq, 6)
    assert entry["display_min"] == "10.0 Hz" and entry["display_max"].endswith("kHz"), entry
    assert "display" not in script._serialize_parameter(freq, 6, displays=False)
    print("a parameter carries its display, its range and its labels")


def check_meter_curve(script):
    # Measured on Live 12.4.6: a -12.0 dBFS file read 0.76314, and every 12 dB
    # of fader moved the value by 0.15789 — the meter is linear in dB.
    for value, decibels in [(0.76314, -12.0), (0.60524, -24.0),
                            (0.84209, -6.0), (0.28945, -48.0), (1.0, 6.0)]:
        got = script._db(value)
        assert abs(got - decibels) < 0.05, (value, got, decibels)
    assert script._db(0.0) == -80.0
    assert abs((script._db(0.76314) - script._db(0.60524)) - 12.0) < 0.05
    scale = script._get_meter_scale()
    assert scale["reference"] == "post_fader" and scale["points"][0] == [0.0, -70.0], scale
    print("a meter reading is dB on the measured curve")


class Track(object):
    def __init__(self, name):
        self.name = name
        self.devices = []


def check_create_tracks(script, done_class):
    song = types.SimpleNamespace(tracks=[Track("Kick")])
    type(script)._song = property(lambda self: song)
    script._create_midi_track = lambda index: song.tracks.append(
        Track("%d-MIDI" % len(song.tracks)))
    script._create_audio_track = script._create_midi_track

    def run(specs, on_existing):
        generator = script._create_tracks(specs, on_existing)
        while True:
            try:
                item = next(generator)
            except StopIteration:
                return None
            if isinstance(item, done_class):
                return item.result

    made = run([{"name": "Kick", "kind": "midi"}, {"name": "Bass", "kind": "midi"}], "converge")
    assert made["created"][0] == {"index": 0, "name": "Kick", "reused": True}, made
    assert "reused" not in made["created"][1], made
    print("a name the set already has is reused, not built again")

    try:
        run([{"name": "Kick", "kind": "midi"}], "fail")
        raise AssertionError("fail must refuse")
    except (ValueError, RuntimeError) as e:
        assert "already has a track named 'Kick'" in str(e) and "after 0 of" not in str(e), e

    before = len(song.tracks)
    run([{"name": "Kick", "kind": "midi"}], "add")
    assert len(song.tracks) == before + 1
    print("add builds another copy on purpose")

    def refuse(index, uri, kind):
        raise ValueError("Browser item with URI '%s' not found" % uri)

    script._load_browser_item = refuse
    song.tracks = []
    try:
        run([{"name": "Lead", "kind": "midi", "instrument_uri": "query:Drums#Kit"}], "converge")
        raise AssertionError("the device load must raise")
    except RuntimeError as e:
        assert "after 1 of 1 tracks (Lead)" in str(e), e
    print("a failure half-way names the tracks that are really in the set")


def main():
    namespace = load()
    script = namespace["AbletonMCP"].__new__(namespace["AbletonMCP"])
    check_display_strings(script)
    check_meter_curve(script)
    check_create_tracks(script, namespace["Done"])
    print("\nall Remote Script helper checks passed (script %s)" % namespace["SCRIPT_VERSION"])


if __name__ == "__main__":
    main()
