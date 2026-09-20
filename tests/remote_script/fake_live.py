"""A Live Object Model that is not Live.

The Remote Script runs against this in place of `Live`: the same classes
(Song, Track, ClipSlot, Clip, Device, DeviceParameter, MixerDevice, Scene,
CuePoint, Browser, Application, the two Views), the same members, the same
semantics where the script depends on them. Nothing here is a stub that
returns a canned answer: a track created is a track that is there, a note
added is a note read back, a fader set is a dB reading.

The rules it keeps, because the script depends on each:

- Live's sequences (`tracks`, `clip_slots`, `devices`, `parameters`, ...)
  come back as tuples: read-only from outside, mutable only through the
  methods Live gives (`create_midi_track`, `delete_clip`, ...).
- Writable members are real `property` descriptors with a setter, read-only
  ones a `property` without: `describe` derives `readonly` from `fset`.
- Every track has one clip slot per scene, and creating a scene gives every
  track a slot, as Live does.
- `set_notes` adds notes; it does not replace them. `remove_notes` and the
  `_extended` pair take Live's argument orders, which differ.
- A `DeviceParameter.value` outside `[min, max]` raises, as Live does, and
  `str_for_value` is monotonic so the script's bisection over it converges.
  The volume fader displays the dB curve the server assumes
  (`src/song.rs`: 0.85 is 0 dB, 1.0 is +6 dB).
- Live's checks raise Live's exceptions: `IndexError` on a bad index,
  `RuntimeError` on an impossible operation ("Clip slot already has a
  clip"), `AttributeError` on a read-only write.

`tests/remote_script/test_live_api_conformance.py` reads the Live API surface
out of the script by AST and fails when a member the script touches is
missing here or has the wrong shape, so the model cannot drift from what
the script needs.
"""
import bisect
import time
import types

# Live's main-thread tick, the rate `schedule_message(1, ...)` comes back at.
# Measured 2026-09-20 on Live 12.4.6 (macOS 26.6) through
# `ableton-music-maker --check`: period_ms 100.0, jitter_ms 0.24, over 211
# samples. Decision 0010 records the same period from a longer run (jitter
# 9.6 ms, 600 samples). Nothing here invents it.
TICK_S = 0.1
TICK_PERIOD_MS = 100.0
TICK_JITTER_MS = 0.24
TICK_MEASURED = "2026-09-20, Live 12.4.6, ableton-music-maker --check, 211 samples"


# ── The clock ───────────────────────────────────────────────────────────────

class WallClock(object):
    """Now is now. What `scripts/fake-live.py` runs on, because a real
    socket and a real tick pass real time."""

    driven = False

    def now(self):
        return time.time()

    def sleep(self, seconds):
        if seconds > 0:
            time.sleep(seconds)


class DrivenClock(object):
    """Now is what the test has advanced to. What the harness runs on, so
    the transport, the tick sampler and the latency table are deterministic
    and cost no wall-clock time."""

    driven = True

    def __init__(self, start=1_600_000_000.0):
        self._t = float(start)

    def now(self):
        return self._t

    def advance(self, seconds):
        self._t += float(seconds)

    def sleep(self, seconds):
        self.advance(seconds)


# ── Descriptors ─────────────────────────────────────────────────────────────


class Vector(object):
    """Live's sequence type.

    `song.tracks` is not a list and not a tuple in Live — `describe` reports
    its class as `Vector`. A model that used a tuple here would tell
    `describe` a different story, so this is a sequence that is neither.
    Verified against Live 12.4.6 on 2026-09-20 with `describe song.tracks`.

    Until script 1.35.0 `_jsonable` tested `isinstance(value, (list, tuple))`
    and so handed a `run … get` of one of these its repr — the address of the
    object rather than its contents — which is why a locator could be read by
    index but never counted (#66). It now renders any non-string sequence as
    a list, so this class must stay sequence-shaped (`__len__` and
    `__getitem__`) without becoming a list.
    """

    __slots__ = ("_items",)

    def __init__(self, items=()):
        self._items = list(items)

    def __len__(self):
        return len(self._items)

    def __getitem__(self, index):
        return self._items[index]

    def __iter__(self):
        return iter(self._items)

    def __contains__(self, item):
        return item in self._items

    def __eq__(self, other):
        if isinstance(other, Vector):
            return self._items == other._items
        if isinstance(other, (list, tuple)):
            return self._items == list(other)
        return NotImplemented

    def __ne__(self, other):
        result = self.__eq__(other)
        return result if result is NotImplemented else not result

    def __repr__(self):
        return "<Base.%s object at 0x%012x>" % (type(self).__name__, id(self))

    def count(self, item):
        return self._items.count(item)

    def index(self, item):
        return self._items.index(item)


class RoutingTypeVector(Vector):
    """Live's own name for the vector of routing types."""


class ATimeableValueVector(Vector):
    """What Live calls a device's `parameters`."""


class BrowserItemVector(Vector):
    """What Live calls a browser item's `children` and `user_folders`."""


# Live's Quantization enum values. Module level on purpose: Live's Song has
# no `Q_BAR` attribute, so a model that put them on the class would describe
# a Live that does not exist. `live_module()` hands these to the script as
# `Live.Song.Quantization`.
class Quantization(int):
    """Live's Quantization enum: an int that `describe` types
    `Quantization`, which is what `song.clip_trigger_quantization` is."""


class RecordingQuantization(int):
    """Live's MIDI recording quantization enum."""


Q_NONE, Q_8_BARS, Q_4_BARS, Q_2_BARS = 0, 1, 2, 3
Q_BAR, Q_HALF, Q_HALF_TRIPLET, Q_QUARTER = 4, 5, 6, 7
Q_QUARTER_TRIPLET, Q_EIGHTH, Q_EIGHTH_TRIPLET = 8, 9, 10
Q_SIXTEENTH, Q_SIXTEENTH_TRIPLET, Q_THIRTYSECOND = 11, 12, 13

MONITOR_IN, MONITOR_AUTO, MONITOR_OFF = 0, 1, 2
AUTOMATION_NONE, AUTOMATION_PLAYING, AUTOMATION_OVERRIDDEN = 0, 1, 2
TYPE_AUDIO_EFFECT, TYPE_INSTRUMENT, TYPE_MIDI_EFFECT = 0, 1, 2
LAUNCH_MODE_TRIGGER, LAUNCH_MODE_GATE, LAUNCH_MODE_TOGGLE, LAUNCH_MODE_REPEAT = 0, 1, 2, 3

def live_prop(attr, readonly=False, cast=None, doc=None):
    """A Live member: stored under `_<attr>`, exposed as a real property so
    `describe` sees the right `readonly`."""
    storage = "_" + attr

    def getter(self):
        return getattr(self, storage)

    if readonly:
        return property(getter, doc=doc)

    def setter(self, value):
        setattr(self, storage, cast(value) if cast is not None else value)

    return property(getter, setter, doc=doc)


def _vector_seq(attr, kind, doc=None):
    """A Vector under the subclass name Live gives it."""
    storage = "_" + attr

    def getter(self):
        return kind(getattr(self, storage))

    return property(getter, doc=doc)


def _routing_seq(attr, doc=None):
    """A Vector Live names `RoutingTypeVector`."""
    storage = "_" + attr

    def getter(self):
        return RoutingTypeVector(getattr(self, storage))

    return property(getter, doc=doc)


def seq_prop(attr, doc=None):
    """A Live sequence: a tuple over the private list."""
    storage = "_" + attr

    def getter(self):
        return Vector(getattr(self, storage))

    return property(getter, doc=doc)



def with_listeners(cls):
    """Give a class Live's listener boilerplate.

    Every observable property in the Live Object Model carries three
    methods — `add_<name>_listener`, `remove_<name>_listener` and
    `<name>_has_listener`. Nothing in the Remote Script calls them (it runs
    off Live's tick, not off notifications), but `describe` lists what an
    object has, so a model without them tells Claude about a Live that does
    not exist. Generated from the class's own properties, so a member added
    to the model gets its three for free.
    """
    names = [n for n, v in vars(cls).items()
             if isinstance(v, property) and not n.startswith("_")]
    for name in names:
        def make(name):
            def add(self, callback):
                self._listeners.setdefault(name, []).append(callback)

            def remove(self, callback):
                try:
                    self._listeners.get(name, []).remove(callback)
                except ValueError:
                    raise RuntimeError("listener was not connected")

            def has(self, callback=None):
                registered = self._listeners.get(name, [])
                return bool(registered) if callback is None else callback in registered
            return add, remove, has
        add, remove, has = make(name)
        setattr(cls, "add_%s_listener" % name, add)
        setattr(cls, "remove_%s_listener" % name, remove)
        setattr(cls, "%s_has_listener" % name, has)
    return cls

class LiveObject(object):
    """What every Live object has: a name for messages, an id for `==`."""
    _next_id = 1

    def __init__(self):
        self._id = LiveObject._next_id
        LiveObject._next_id += 1
        self._listeners = {}

    def __eq__(self, other):
        return isinstance(other, LiveObject) and other._id == self._id

    def __ne__(self, other):
        return not self.__eq__(other)

    def __hash__(self):
        return self._id


# ── What a call costs Live ──────────────────────────────────────────────────
#
# Live's API is not free, and the calls that are expensive are the ones the
# product keeps tripping over: a track create and a device load reinitialise
# the audio graph inside a single uninterruptible Live call, which is what
# #43 (the main thread held for seconds) and #45 (ten tracks took Live down)
# both ride on. A fake Live that answers instantly cannot reproduce either.
#
# Every number below was MEASURED against a real Live and is stamped with
# where it came from. Nothing here is invented, and a call nobody has
# measured gets no row rather than a guess — `LatencyTable.unmeasured()`
# lists those, and `scripts/fake-live.py --latency-report` prints them, so
# the gap is visible instead of silently filled in.

class Measured(object):
    """One measurement: what it cost, where it was taken, and what the
    number IS (`main_ms` inside Live's main thread, or a whole round trip
    including the tick)."""

    __slots__ = ("typical_ms", "worst_ms", "kind", "source", "note")

    def __init__(self, typical_ms, worst_ms, kind, source, note=""):
        self.typical_ms = float(typical_ms)
        self.worst_ms = float(worst_ms)
        self.kind = kind            # "main_ms" or "round_trip"
        self.source = source
        self.note = note

    def ms(self, worst=False):
        return self.worst_ms if worst else self.typical_ms

    def as_dict(self):
        return {"typical_ms": self.typical_ms, "worst_ms": self.worst_ms,
                "kind": self.kind, "source": self.source, "note": self.note}


# Issue #43, second comment: single commands while the transport ran,
# `main_ms` straight from the activity log. Live 12.4.6, macOS 26.6,
# Remote Script 1.23.0, 2026-09-19.
_I43 = "issue #43, activity log main_ms, Live 12.4.6, script 1.23.0, 2026-09-19"
# Issue #45, both comments: the ten-track build, before and after the fix.
# Live 12.4.6, macOS 26.6, Remote Script 1.32.0, 2026-09-20. The worst
# figures are one track's device load, which Live runs in a single
# uninterruptible call ("the 709 ms and the 1411 ms seen under stress are
# one track's device load").
_I45 = "issue #45, Live's log and cargo run --example ten_track_build, Live 12.4.6, script 1.32.0, 2026-09-20"

LATENCY_12_4_6 = {
    # The device load is the expensive one, and it is measured directly:
    # `load_instrument_or_effect` (one Reverb) was 839 ms of main thread.
    # #45's worst single-track slices — 1411 ms under stress, 2154 ms
    # before the grouping fix — are the same call.
    "Browser.load_item": Measured(839.0, 1411.0, "main_ms", _I43 + "; worst from " + _I45,
                                  "one Reverb; the worst is one track's device load under stress"),
    "Song.create_scene": Measured(68.0, 68.0, "main_ms", _I43, "create_scene, one call"),
    "Song.duplicate_scene": Measured(68.0, 68.0, "main_ms", _I43, "make_section, two calls at 68 ms"),
    "Song.delete_track": Measured(75.0, 110.0, "main_ms", _I43, "measured as a 40-110 ms range"),
    "ClipSlot.create_clip": Measured(5.4, 5.4, "main_ms", _I43, "create_clip, one call"),
    "Clip.set_notes": Measured(0.5, 0.6, "main_ms", _I43,
                               "add_notes_to_clip: 0.5 ms into a stopped slot, 0.6 ms into a playing clip"),
    "Clip.get_notes_extended": Measured(0.5, 0.5, "main_ms", _I43, "get_clip_notes, one call"),
    "MixerDevice.write": Measured(0.7, 0.7, "main_ms", _I43, "set_track_mixer in dB, one call"),
    "Track.meter_read": Measured(0.5, 0.5, "main_ms", _I43, "listen, 5 meter reads over 6 calls at 3.0 ms"),
}

# Calls the model can charge for but that nobody has measured yet. They are
# named rather than guessed: each is a line in the PR and a row a real-Live
# run can fill. `Song.create_midi_track` is the sharp one — #43's first
# table has it at 0.25 s median / 2.0 s worst, but those are ROUND TRIPS
# from before `main_ms` existed ("a 200-300 ms figure is the round trip
# itself"), so the main-thread share of it is not known.
LATENCY_UNMEASURED = {
    "Song.create_midi_track": "#43's 0.25 s median / 2.0 s worst are round trips, not main_ms",
    "Song.create_audio_track": "not separated from create_midi_track in any run",
    "Song.create_return_track": "named in #43 as a graph reinitialisation; never timed on its own",
    "Track.duplicate_clip_to_arrangement": "#53 asks for it; no run has timed it",
    "ClipSlot.create_audio_clip": "sample read from disk; never timed on its own",
    "Track.delete_device": "never timed",
    "Song.move_device": "never timed",
}


class LatencyTable(object):
    """What the model charges for a Live call, and how.

    `LatencyTable.measured()` is the table above; `off()` charges nothing,
    which is what the harness uses so the script's 94 tests stay fast.
    `scale` shortens every charge by the same factor for a fast run and is
    reported, so a scaled run can never be read as a real one.
    """

    def __init__(self, table=None, worst=False, scale=1.0, clock=None):
        self.table = dict(table or {})
        self.worst = bool(worst)
        self.scale = float(scale)
        self.clock = clock or WallClock()
        self.charged = {}       # call -> (count, total_ms) actually charged

    @classmethod
    def measured(cls, **kw):
        return cls(LATENCY_12_4_6, **kw)

    @classmethod
    def off(cls, **kw):
        return cls({}, **kw)

    def charge(self, call):
        """Spend what this call costs Live. Real elapsed time on a wall
        clock, so the executor's slice budget spreads work over ticks and
        `main_ms` is real; virtual time on a driven clock."""
        m = self.table.get(call)
        if m is None:
            return 0.0
        ms = m.ms(self.worst) * self.scale
        count, total = self.charged.get(call, (0, 0.0))
        self.charged[call] = (count + 1, total + ms)
        self.clock.sleep(ms / 1000.0)
        return ms

    def unmeasured(self):
        """Calls the model would charge for if anyone had measured them."""
        return dict(LATENCY_UNMEASURED)

    def report(self):
        rows = []
        for call in sorted(self.table):
            m = self.table[call]
            d = m.as_dict()
            d["call"] = call
            d["charged_ms"] = round(self.charged.get(call, (0, 0.0))[1], 2)
            d["charged_calls"] = self.charged.get(call, (0, 0.0))[0]
            rows.append(d)
        return {"scale": self.scale, "worst": self.worst, "rows": rows,
                "unmeasured": self.unmeasured(), "tick_ms": TICK_PERIOD_MS,
                "tick_measured": TICK_MEASURED}


# ── Display curves ──────────────────────────────────────────────────────────

# Live's volume fader taper, measured on Live 12.4.6 on 2026-09-20 by
# asking `volume.str_for_value` at twenty-one fader positions:
#
#   0.05 -57.2   0.20 -34.4   0.40 -18.0   0.70  -6.0   0.90  +2.0
#   0.10 -48.6   0.25 -28.8   0.50 -14.0   0.85   0.0   1.00  +6.0
#
# Above 0.4 it is a straight 40 dB per unit; below it a quadratic, and both
# reproduce every measured point to the decimal Live prints. 0.0 is -inf.
# This is not the curve in src/song.rs's test fixture, which puts -30 dB at
# 0.4 and -12 dB at 0.7 where Live puts -18 and -6.
VOLUME_UNITY = 0.85            # 0 dB
VOLUME_TOP_DB = 6.0            # at 1.0
_TAPER_KNEE = 0.4              # where the straight part starts
_TAPER_SLOPE, _TAPER_OFFSET = 40.0, -34.0        # dB = 40v - 34 above the knee
_TAPER_A, _TAPER_B, _TAPER_C = -200.0, 202.0, -66.8   # below it


def fader_to_db(value):
    """The dB Live shows for a fader position."""
    v = max(0.0, min(1.0, float(value)))
    if v <= 0.0:
        return float("-inf")
    if v >= _TAPER_KNEE:
        return _TAPER_SLOPE * v + _TAPER_OFFSET
    return _TAPER_A * v * v + _TAPER_B * v + _TAPER_C


def db_to_fader(db):
    """The fader position that shows this dB: the taper, inverted."""
    db = float(db)
    if db <= fader_to_db(0.001):
        return 0.0
    if db >= VOLUME_TOP_DB:
        return 1.0
    if db >= _TAPER_A * _TAPER_KNEE ** 2 + _TAPER_B * _TAPER_KNEE + _TAPER_C:
        return (db - _TAPER_OFFSET) / _TAPER_SLOPE
    # -200v^2 + 202v - 66.8 = db, the root inside [0, 0.4]
    a, b, c = _TAPER_A, _TAPER_B, _TAPER_C - db
    disc = b * b - 4 * a * c
    if disc < 0:
        return 0.0
    root = (-b + disc ** 0.5) / (2 * a)
    return max(0.0, min(_TAPER_KNEE, root))


def display_db(value):
    """Live's volume readout.

    Three decimals, trailing zeros trimmed but never below one: Live prints
    "-6.0 dB" at fader 0.7 and "-6.001 dB" at 0.6999869. Verified on
    12.4.6, 2026-09-20 — the precision matters because
    `_value_from_display` bisects over this string, so a coarser readout
    lands the fader somewhere Live would not."""
    db = fader_to_db(value)
    if db == float("-inf"):
        return "-inf dB"
    text = "%.3f" % db
    if "." in text:
        text = text.rstrip("0")
        if text.endswith("."):
            text += "0"
    return "%s dB" % text


def display_pan(value):
    v = float(value)
    if abs(v) < 0.005:
        return "C"
    return "%d%s" % (int(round(abs(v) * 50)), "L" if v < 0 else "R")


def display_send(value):
    return display_db(value)


def display_hz(lo, hi):
    """A log-frequency knob, 0-1 -> lo..hi Hz, shown as Live shows it."""
    import math
    def show(value):
        f = lo * (hi / lo) ** max(0.0, min(1.0, float(value)))
        if f >= 1000.0:
            return "%.2f kHz" % (f / 1000.0)
        return "%.1f Hz" % f
    return show


def display_percent(value):
    return "%d %%" % int(round(float(value) * 100))


def display_plain(value):
    return "%.2f" % float(value)


# ── DeviceParameter ─────────────────────────────────────────────────────────

class DeviceParameter(LiveObject):
    """Live.DeviceParameter.DeviceParameter."""

            
    def __init__(self, name, value=0.0, minimum=0.0, maximum=1.0,
                 display=None, value_items=None, original_name=None):
        LiveObject.__init__(self)
        self._name = name
        self._original_name = original_name or name
        self._min = float(minimum)
        self._max = float(maximum)
        self._value_items = tuple(value_items or ())
        self._is_quantized = bool(self._value_items)
        self._display = display or (self._quantized_display if self._is_quantized else display_plain)
        self._value = float(value)
        self._default_value = float(value)
        self._automation_state = AUTOMATION_NONE
        self._is_enabled = True
        # Set for a mixer strip's parameters: the only write anyone has
        # timed (`set_track_mixer`, 0.7 ms) is a mixer write.
        self._song = None
        self._charge_as = None

    name = live_prop("name", readonly=True)
    original_name = live_prop("original_name", readonly=True)
    min = live_prop("min", readonly=True)
    max = live_prop("max", readonly=True)
    default_value = live_prop("default_value", readonly=True)
    is_quantized = live_prop("is_quantized", readonly=True)
    automation_state = live_prop("automation_state", readonly=True)
    is_enabled = live_prop("is_enabled", readonly=True)

    @property
    def value_items(self):
        """Only a quantized parameter has them, as in Live."""
        if not self._is_quantized:
            raise AttributeError("'value_items' is only on a quantized parameter")
        return Vector(self._value_items)

    def __str__(self):
        """`run … get` on a parameter gives its display, because that is
        what Live's DeviceParameter prints: "0.0 dB", "C", "Mono"."""
        return self._display(self._value)

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, v):
        v = float(v)
        # Live: "RuntimeError: Invalid value" outside the range.
        if v < self._min - 1e-9 or v > self._max + 1e-9:
            raise RuntimeError("Invalid value %r for parameter '%s' (%s..%s)" % (
                v, self._name, self._min, self._max))
        if self._song is not None and self._charge_as:
            self._song._charge(self._charge_as)
        self._value = float(int(round(v))) if self._is_quantized else v
        # A manual move while automation plays: Live shows it overridden.
        if self._automation_state == AUTOMATION_PLAYING:
            self._automation_state = AUTOMATION_OVERRIDDEN

    def _quantized_display(self, value):
        i = int(round(float(value))) - int(round(self._min))
        if 0 <= i < len(self._value_items):
            return self._value_items[i]
        return "%d" % int(round(float(value)))

    def str_for_value(self, value):
        return self._display(value)

    def re_enable_automation(self):
        if self._automation_state == AUTOMATION_OVERRIDDEN:
            self._automation_state = AUTOMATION_PLAYING


# ── MixerDevice ─────────────────────────────────────────────────────────────

class MixerDevice(LiveObject):
    """Live.MixerDevice.MixerDevice: the fader strip every track has."""

    def __init__(self, song, sends=0, is_master=False):
        LiveObject.__init__(self)
        self._song = song
        self._is_master = is_master
        self._volume = DeviceParameter("Volume", 0.85, 0.0, 1.0, display_db, original_name="Track Volume")
        self._panning = DeviceParameter("Panning", 0.0, -1.0, 1.0, display_pan, original_name="Track Panning")
        # Live shows the crossfader as a number, not as L/C/R.
        self._crossfader = DeviceParameter(
            "Crossfader", 0.0, -1.0, 1.0, lambda v: "%d" % int(round(float(v) * 50)))
        self._track_activator = DeviceParameter("Track Activator", 1.0, 0.0, 1.0,
                                                value_items=("Off", "On"))
        self._sends = [self._new_send(i) for i in range(sends)]
        for p in (self._volume, self._panning, self._crossfader, self._track_activator):
            p._song, p._charge_as = song, "MixerDevice.write"
        self._crossfade_assign = 1  # 0 A, 1 none, 2 B

    @staticmethod
    def _new_send(i, return_track=None):
        """Live names a send after the return it feeds — the send to
        "A-Reverb" is called "A-Reverb", not "Send A". Verified against Live
        12.4.6 on 2026-09-20."""
        name = return_track.name if return_track is not None else "%s-Return" % chr(ord("A") + i)
        return DeviceParameter(name, 0.0, 0.0, 1.0, display_send)

    volume = live_prop("volume", readonly=True)
    panning = live_prop("panning", readonly=True)
    @property
    def crossfader(self):
        """Live puts the crossfader on the master mixer and nowhere else."""
        if not self._is_master:
            raise AttributeError("the crossfader is on the master mixer only")
        return self._crossfader
    track_activator = live_prop("track_activator", readonly=True)
    crossfade_assign = live_prop("crossfade_assign", cast=int)

    @property
    def sends(self):
        """Each send carries the name of the return it feeds, so renaming a
        return renames the sends, as it does in Live."""
        returns = list(self._song._return_tracks)
        for i, send in enumerate(self._sends):
            if i < len(returns):
                send._name = returns[i].name
        return Vector(self._sends)


# ── Devices ─────────────────────────────────────────────────────────────────

class Device(LiveObject):
    """Live.Device.Device. `class_name` is Live's internal name
    ("OriginalSimpler", "Eq8"), `class_display_name` the one in the browser."""

            
    def __init__(self, name, class_name=None, parameters=None, device_type=None,
                 class_display_name=None, chains=None, can_have_drum_pads=False):
        LiveObject.__init__(self)
        self._name = name
        self._class_name = class_name or name.replace(" ", "")
        self._class_display_name = class_display_name or name
        self._type = TYPE_INSTRUMENT if device_type is None else device_type
        self._is_active = True
        # Every Live device leads with "Device On".
        on = DeviceParameter("Device On", 1.0, 0.0, 1.0, value_items=("Off", "On"))
        self._parameters = [on] + list(parameters or [])
        self._chains = list(chains or [])
        self._can_have_chains = chains is not None
        self._can_have_drum_pads = bool(can_have_drum_pads)
        self._canonical_parent = None

    name = live_prop("name", cast=str)
    class_name = live_prop("class_name", readonly=True)
    class_display_name = live_prop("class_display_name", readonly=True)
    type = live_prop("type", readonly=True)
    is_active = live_prop("is_active", readonly=True)
    parameters = _vector_seq("parameters", ATimeableValueVector)
    can_have_chains = live_prop("can_have_chains", readonly=True)

    @property
    def chains(self):
        """Live gives `chains` to a rack; a plain device has no such member."""
        if not self._can_have_chains:
            raise AttributeError("'%s' is not a rack" % self._name)
        return Vector(self._chains)
    can_have_drum_pads = live_prop("can_have_drum_pads", readonly=True)
    canonical_parent = live_prop("canonical_parent", readonly=True)

    def _find_parameter(self, name):
        for p in self._parameters:
            if p.name == name:
                return p
        return None


class Chain(LiveObject):
    """A rack chain: its own devices and its own mixer."""

    def __init__(self, name, song, devices=None):
        LiveObject.__init__(self)
        self._name = name
        self._devices = list(devices or [])
        self._mixer_device = MixerDevice(song)
        self._mute = False
        self._solo = False

    name = live_prop("name", cast=str)
    devices = seq_prop("devices")
    mixer_device = live_prop("mixer_device", readonly=True)
    mute = live_prop("mute", cast=bool)
    solo = live_prop("solo", cast=bool)


class DrumPad(LiveObject):
    def __init__(self, note, name="", chains=None):
        LiveObject.__init__(self)
        self._note = note
        self._name = name
        self._chains = list(chains or [])
        self._mute = False
        self._solo = False

    note = live_prop("note", readonly=True)
    name = live_prop("name", readonly=True)
    chains = seq_prop("chains")
    mute = live_prop("mute", cast=bool)
    solo = live_prop("solo", cast=bool)


class RackDevice(Device):
    """An Instrument, Drum or Effect Rack: chains, macros, and for a Drum
    Rack the 128 pads."""

    def __init__(self, name, song, class_name="InstrumentGroupDevice", chains=None,
                 drum=False, macros=8, device_type=None):
        macro_params = [DeviceParameter("Macro %d" % (i + 1), 0.0, 0.0, 127.0, display_plain)
                        for i in range(macros)]
        Device.__init__(self, name, class_name=class_name, parameters=macro_params,
                        device_type=device_type, chains=chains if chains is not None else [],
                        can_have_drum_pads=drum)
        self._song = song
        self._drum_pads = [DrumPad(n) for n in range(128)] if drum else []
        self._visible_drum_pads = []
        self._macros_mapped = tuple(False for _ in range(macros))
        self._is_showing_chains = False

    drum_pads = seq_prop("drum_pads")
    visible_drum_pads = seq_prop("visible_drum_pads")
    macros_mapped = live_prop("macros_mapped", readonly=True)
    is_showing_chains = live_prop("is_showing_chains", cast=bool)

    def _fill_pad(self, note, name, devices=None):
        """Put a sample on a pad: Live makes a chain named after it."""
        pad = self._drum_pads[note]
        chain = Chain(name, self._song, devices or [Device(name, "OriginalSimpler")])
        pad._name = name
        pad._chains = [chain]
        self._chains.append(chain)
        self._visible_drum_pads = [p for p in self._drum_pads if p._chains]
        return chain


# ── Notes and automation ────────────────────────────────────────────────────

class MidiNote(object):
    """Live.Clip.MidiNote (Live 11+): what `get_notes_extended` returns."""
    def __init__(self, pitch, start_time, duration, velocity=100.0, mute=False,
                 probability=1.0, velocity_deviation=0.0, release_velocity=0.0):
        self.pitch = int(pitch)
        self.start_time = float(start_time)
        self.duration = float(duration)
        self.velocity = float(velocity)
        self.mute = bool(mute)
        self.probability = float(probability)
        self.velocity_deviation = float(velocity_deviation)
        # Live reports 0.0 unless the note carries one.
        self.release_velocity = float(release_velocity)
        # Live numbers a clip's notes from 1, per clip. The clip sets this
        # when the note joins it; until then it is unnumbered, as in Live.
        self.note_id = 0

    def as_tuple(self):
        return (self.pitch, self.start_time, self.duration, self.velocity, self.mute)


class AutomationEnvelope(LiveObject):
    """Live.Clip.AutomationEnvelope: steps, read back by time."""

    def __init__(self, parameter):
        LiveObject.__init__(self)
        self._parameter = parameter
        self._steps = []  # (time, length, value), kept sorted

    def insert_step(self, time, length, value):
        self._steps.append((float(time), float(length), float(value)))
        self._steps.sort()

    def value_at_time(self, time):
        t = float(time)
        current = self._parameter.value
        for start, length, value in self._steps:
            if start <= t:
                current = value
        return current


# ── Clip ────────────────────────────────────────────────────────────────────


def _audio_only(attr, readonly=False, cast=None):
    """A member Live puts on audio clips and not on MIDI ones.

    Reading `gain` or `pitch_coarse` on a MIDI clip raises in Live, which is
    why `get_clip_info` leaves them out of a MIDI clip's reply — a model
    that answered would put fields in the reply that Live never sends."""
    storage = "_" + attr

    def getter(self):
        if self._is_midi_clip:
            raise AttributeError("'%s' is only on audio clips" % attr)
        return getattr(self, storage)

    if readonly:
        return property(getter)

    def setter(self, value):
        if self._is_midi_clip:
            raise AttributeError("'%s' is only on audio clips" % attr)
        setattr(self, storage, cast(value) if cast is not None else value)

    return property(getter, setter)


class Clip(LiveObject):
    """Live.Clip.Clip. One class for Session and Arrangement clips, MIDI and
    audio, as in Live; `start_time` is where it sits in the Arrangement."""

                
    def __init__(self, song, track, length, midi=True, name="", file_path=None,
                 start_time=0.0, in_arrangement=False):
        LiveObject.__init__(self)
        self._song = song
        self._track = track
        self._name = name
        self._is_midi_clip = bool(midi)
        self._is_audio_clip = not midi
        self._file_path = file_path
        self._in_arrangement = in_arrangement
        self._start_time = float(start_time)
        self._start_marker = 0.0
        self._end_marker = float(length)
        self._loop_start = 0.0
        self._loop_end = float(length)
        self._looping = True
        self._notes = []
        self._envelopes = {}
        self._color_index = track._color_index if track is not None else 0
        self._groove = None
        self._launch_mode = LAUNCH_MODE_TRIGGER
        self._launch_quantization = 0  # q_global
        self._legato = False
        self._pitch_coarse = 0
        self._pitch_fine = 0.0
        self._velocity_amount = 0.0
        self._is_playing = False
        self._is_recording = False
        self._is_triggered = False
        self._muted = False
        self._warp_markers = ()
        self._warping = not midi
        self._gain = 1.0
        self._signature_numerator = 4
        self._signature_denominator = 4

    name = live_prop("name", cast=str)
    is_midi_clip = live_prop("is_midi_clip", readonly=True)
    is_audio_clip = live_prop("is_audio_clip", readonly=True)
    file_path = _audio_only("file_path", readonly=True)
    is_arrangement_clip = live_prop("in_arrangement", readonly=True)
    start_marker = live_prop("start_marker", cast=float)
    end_marker = live_prop("end_marker", cast=float)
    loop_start = live_prop("loop_start", cast=float)
    loop_end = live_prop("loop_end", cast=float)
    looping = live_prop("looping", cast=bool)
    color_index = live_prop("color_index", cast=int)
    groove = live_prop("groove")
    launch_mode = live_prop("launch_mode", cast=int)
    launch_quantization = live_prop("launch_quantization", cast=int)
    legato = live_prop("legato", cast=bool)
    # gain, the pitch pair and the warp members exist on audio clips only:
    # Live raises for them on a MIDI clip, and `get_clip_info` leaves them
    # out of a MIDI clip's reply because of it.
    pitch_coarse = _audio_only("pitch_coarse", cast=int)
    pitch_fine = _audio_only("pitch_fine", cast=float)
    velocity_amount = live_prop("velocity_amount", cast=float)
    is_playing = live_prop("is_playing", readonly=True)
    is_recording = live_prop("is_recording", readonly=True)
    is_triggered = live_prop("is_triggered", readonly=True)
    muted = live_prop("muted", cast=bool)
    warp_markers = _audio_only("warp_markers", readonly=True)
    warping = _audio_only("warping", cast=bool)
    gain = _audio_only("gain", cast=float)
    signature_numerator = live_prop("signature_numerator", cast=int)
    signature_denominator = live_prop("signature_denominator", cast=int)
    canonical_parent = live_prop("track", readonly=True)

    @property
    def color(self):
        return _COLOR_TABLE[self._color_index % len(_COLOR_TABLE)]

    @color.setter
    def color(self, value):
        self._color_index = _nearest_color_index(value)

    @property
    def length(self):
        """Live: the loop's length when looping, else the markers'."""
        if self._looping:
            return self._loop_end - self._loop_start
        return self._end_marker - self._start_marker

    @property
    def start_time(self):
        return self._start_time if self._in_arrangement else self._loop_start

    @property
    def end_time(self):
        return self.start_time + self.length

    # Notes: the old API (time first) and Live 11's extended one (pitch first).
    def _midi_only(self, what):
        if not self._is_midi_clip:
            raise RuntimeError("%s is only available on MIDI clips" % what)

    def _select(self, from_time, from_pitch, time_span, pitch_span):
        t0, t1 = float(from_time), float(from_time) + float(time_span)
        p0, p1 = int(from_pitch), int(from_pitch) + int(pitch_span)
        return [n for n in self._notes
                if t0 <= n.start_time < t1 and p0 <= n.pitch < p1]

    def get_notes(self, from_time, from_pitch, time_span, pitch_span):
        self._midi_only("get_notes")
        return tuple(n.as_tuple() for n in self._select(from_time, from_pitch, time_span, pitch_span))

    def get_notes_extended(self, from_pitch, pitch_span, from_time, time_span):
        self._song._charge("Clip.get_notes_extended")
        self._midi_only("get_notes_extended")
        return tuple(self._select(from_time, from_pitch, time_span, pitch_span))

    def set_notes(self, notes):
        self._song._charge("Clip.set_notes")
        """Adds. Live's set_notes does not replace what is there."""
        self._midi_only("set_notes")
        for note in notes:
            pitch, start, duration, velocity = note[0], note[1], note[2], note[3]
            mute = note[4] if len(note) > 4 else False
            if not (0 <= int(pitch) < 128):
                raise RuntimeError("Invalid pitch %r" % (pitch,))
            if float(duration) <= 0.0:
                raise RuntimeError("Invalid duration %r" % (duration,))
            self._notes.append(MidiNote(pitch, start, duration, velocity, mute))
        self._renumber()

    def _renumber(self):
        """Live's note ids run from 1 within the clip, in time order."""
        self._notes.sort(key=lambda n: (n.start_time, n.pitch))
        for i, note in enumerate(self._notes):
            note.note_id = i + 1

    def add_new_notes(self, specs):
        self._midi_only("add_new_notes")
        for s in specs:
            self._notes.append(MidiNote(s["pitch"], s["start_time"], s["duration"],
                                        s.get("velocity", 100), s.get("mute", False)))
        self._renumber()

    def remove_notes(self, from_time, from_pitch, time_span, pitch_span):
        self._midi_only("remove_notes")
        gone = set(id(n) for n in self._select(from_time, from_pitch, time_span, pitch_span))
        self._notes = [n for n in self._notes if id(n) not in gone]
        self._renumber()

    def remove_notes_extended(self, from_pitch, pitch_span, from_time, time_span):
        self.remove_notes(from_time, from_pitch, time_span, pitch_span)

    def select_all_notes(self):
        self._midi_only("select_all_notes")

    def deselect_all_notes(self):
        self._midi_only("deselect_all_notes")

    def replace_selected_notes(self, notes):
        self._midi_only("replace_selected_notes")
        self._notes = []
        self.set_notes(notes)

    # Automation
    def automation_envelope(self, parameter):
        return self._envelopes.get(parameter._id)

    def create_automation_envelope(self, parameter):
        env = AutomationEnvelope(parameter)
        self._envelopes[parameter._id] = env
        return env

    def clear_envelope(self, parameter):
        self._envelopes.pop(parameter._id, None)

    def clear_all_envelopes(self):
        self._envelopes = {}

    # Transport
    def fire(self):
        if self._in_arrangement:
            raise RuntimeError("Arrangement clips cannot be fired")
        self._track._fire_clip(self)

    def stop(self):
        self._track._stop_playing()

    def duplicate_loop(self):
        span = self._loop_end - self._loop_start
        self._loop_end += span
        self._end_marker = max(self._end_marker, self._loop_end)
        if self._is_midi_clip:
            copies = [MidiNote(n.pitch, n.start_time + span, n.duration, n.velocity, n.mute)
                      for n in self._notes if self._loop_start <= n.start_time < self._loop_start + span]
            self._notes.extend(copies)
            self._notes.sort(key=lambda n: (n.start_time, n.pitch))

    def _copy_into(self, other):
        other._name = self._name
        other._notes = [MidiNote(n.pitch, n.start_time, n.duration, n.velocity, n.mute)
                        for n in self._notes]
        other._renumber()
        other._loop_start, other._loop_end = self._loop_start, self._loop_end
        other._start_marker, other._end_marker = self._start_marker, self._end_marker
        other._looping = self._looping
        other._color_index = self._color_index
        other._file_path = self._file_path
        other._groove = self._groove
        return other


# ── ClipSlot ────────────────────────────────────────────────────────────────

class ClipSlot(LiveObject):
    """Live.ClipSlot.ClipSlot: one cell of the Session grid."""

    def __init__(self, song, track, scene):
        LiveObject.__init__(self)
        self._song = song
        self._track = track
        self._scene = scene
        self._record_length = None
        self._launch_quantization = None
        self._clip = None
        self._has_stop_button = True
        self._is_triggered = False

    clip = live_prop("clip", readonly=True)
    has_stop_button = live_prop("has_stop_button", cast=bool)
    is_triggered = live_prop("is_triggered", readonly=True)
    canonical_parent = live_prop("track", readonly=True)

    @property
    def has_clip(self):
        return self._clip is not None

    @property
    def is_playing(self):
        return self._clip is not None and self._clip._is_playing

    @property
    def is_recording(self):
        return self._clip is not None and self._clip._is_recording

    @property
    def is_group_slot(self):
        return self._track._is_foldable

    @property
    def controls_other_clips(self):
        return self._track._is_foldable

    @property
    def playing_status(self):
        if self.is_playing:
            return 1
        return 2 if self.is_recording else 0

    def create_clip(self, length):
        self._song._charge("ClipSlot.create_clip")
        if self._clip is not None:
            raise RuntimeError("Clip slot already has a clip")
        if not self._track._has_midi_input:
            raise RuntimeError("Cannot create a MIDI clip on an audio track")
        if float(length) <= 0.0:
            raise RuntimeError("Invalid clip length")
        self._clip = Clip(self._song, self._track, float(length), midi=True)
        self._song._touch()

    def create_audio_clip(self, path):
        self._song._charge("ClipSlot.create_audio_clip")
        if self._clip is not None:
            raise RuntimeError("Clip slot already has a clip")
        if self._track._has_midi_input:
            raise RuntimeError("Cannot create an audio clip on a MIDI track")
        self._clip = self._song._audio_clip(self._track, path)
        self._song._touch()

    def delete_clip(self):
        if self._clip is None:
            raise RuntimeError("Clip slot has no clip")
        self._clip._is_playing = False
        self._clip = None
        self._song._touch()

    def fire(self, record_length=None, launch_quantization=None, force_legato=False):
        """Live 11+: `record_length` fires the slot as a fixed-length
        recording, `launch_quantization` overrides the global grid for this
        launch. The script passes both (`_record_clip`, `_start_capture`,
        `_start_live_capture`), and a model that did not take them made
        every fixed-length recording read as "needs Live 11 or newer"."""
        if record_length is not None:
            self._record_length = float(record_length)
            self._launch_quantization = launch_quantization
            if self._clip is None:
                # Live records what the track carries: MIDI on a MIDI track,
                # audio on an audio one. The Capture track is audio, which is
                # the whole point of `start_capture`.
                if self._track._has_midi_input:
                    self.create_clip(float(record_length))
                else:
                    self._clip = Clip(self._song, self._track, float(record_length),
                                      midi=False, name="", file_path=None)
                    self._song._touch()
            self._clip._is_recording = True
            self._track._arm = True
            self._track._fire_clip(self._clip)
            return
        if self._clip is None:
            # Firing an empty slot stops the track's clip, as in Live.
            self._track._stop_playing()
            return
        self._track._fire_clip(self._clip)

    def stop(self):
        self._track._stop_playing()

    def set_fire_button_state(self, state):
        if state:
            self.fire()


# ── Track ───────────────────────────────────────────────────────────────────

class RoutingType(LiveObject):
    def __init__(self, display_name, category=0):
        LiveObject.__init__(self)
        self._display_name = display_name
        self._category = category

    display_name = live_prop("display_name", readonly=True)
    category = live_prop("category", readonly=True)


_COLOR_TABLE = [
    0xFF94A6, 0xFFA529, 0xCC9927, 0xF7F47C, 0xBFFB00, 0x1AFF2F, 0x25FFA8, 0x5CFFE8,
    0x8BC5FF, 0x5480E4, 0x92A7FF, 0xD86CE4, 0xE553A0, 0xFFFFFF, 0xFF3636, 0xF66C03,
    0x99724B, 0xFFF034, 0x87FF67, 0x3DC300, 0x00BFAF, 0x19E9FF, 0x10A4EE, 0x007DC0,
    0x886CE4, 0xB677C6, 0xFF39D4, 0xD0D0D0, 0xE2675A, 0xFFA374, 0xD3AD71, 0xEDFFAE,
]



def _nearest_color_index(value):
    """Live snaps a written colour to the nearest swatch in its palette."""
    rgb = int(value)
    r, g, b = (rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255
    best, best_d = 0, None
    for i, swatch in enumerate(_COLOR_TABLE):
        sr, sg, sb = (swatch >> 16) & 255, (swatch >> 8) & 255, swatch & 255
        d = (sr - r) ** 2 + (sg - g) ** 2 + (sb - b) ** 2
        if best_d is None or d < best_d:
            best, best_d = i, d
    return best


def _not_on_master_mixer(prop):
    """`crossfade_assign` is on a track's mixer, not the master's."""
    def getter(self):
        if getattr(self, "_is_master", False):
            raise AttributeError("the master mixer has no crossfade_assign")
        return prop.fget(self)

    def setter(self, value):
        if getattr(self, "_is_master", False):
            raise AttributeError("the master mixer has no crossfade_assign")
        prop.fset(self, value)

    return property(getter, setter)


def _not_on_master(prop):
    """A member every track has except the master: `mute` and `solo`. A
    return has both; the master has neither. Verified 2026-09-20."""
    def getter(self):
        if self._kind == "master":
            raise AttributeError("the master track has no such member")
        return prop.fget(self)

    def setter(self, value):
        if self._kind == "master":
            raise AttributeError("the master track has no such member")
        prop.fset(self, value)

    return property(getter, setter)


def _playable_only(prop):
    """A member Live puts on a MIDI or audio track and nowhere else.

    The master and the returns are Tracks without the channel strip: no
    `arm`, `arrangement_clips`, `current_monitoring_state`,
    `playing_slot_index`, `fired_slot_index` or `implicit_arm`. `describe`
    on 12.4.6 lists none of them for either, and `describe` skips what
    raises. Verified 2026-09-20.
    """
    def getter(self):
        if self._kind not in ("midi", "audio"):
            raise AttributeError("'%s' is only on a MIDI or audio track" % self._kind)
        return prop.fget(self)

    if prop.fset is None:
        return property(getter)

    def setter(self, value):
        if self._kind not in ("midi", "audio"):
            raise AttributeError("'%s' is only on a MIDI or audio track" % self._kind)
        prop.fset(self, value)

    return property(getter, setter)


class Track(LiveObject):
    """Live.Track.Track. `kind` is "midi", "audio", "return" or "master";
    the two last have no clip slots."""

            
    def __init__(self, song, name, kind="midi", color_index=0):
        LiveObject.__init__(self)
        self._song = song
        self._name = name
        # A track Live has not been told the name of shows its position:
        # `name` is derived, not stored. `default_set()` and the tests that
        # want a named track set it, which pins it.
        self._named = bool(name) and kind in ("return", "master")
        self._kind = kind
        self._has_midi_input = kind == "midi"
        # Live's master and return tracks take audio in; only a MIDI track
        # does not. Verified 2026-09-20 against 12.4.6.
        self._has_audio_input = kind in ("audio", "return", "master")
        self._has_audio_output = True
        self._has_midi_output = False
        self._can_be_armed = kind in ("midi", "audio")
        self._is_foldable = False
        self._is_grouped = False
        self._is_visible = True
        self._is_part_of_selection = False
        self._clip_slots = []
        self._arrangement_clips = []
        self._devices = []
        self._mixer_device = MixerDevice(
            song, sends=len(song._return_tracks) if kind in ("midi", "audio") else 0,
            is_master=(kind == "master"))
        self._mute = False
        self._solo = False
        self._arm = False
        self._color_index = color_index
        self._current_monitoring_state = MONITOR_AUTO
        self._available_input_routing_types = (
            [RoutingType("All Ins"), RoutingType("Computer Keyboard")]
            if kind == "midi" else
            [RoutingType("Ext. In"), RoutingType("Resampling"), RoutingType("No Input")]
        )
        self._input_routing_type = self._available_input_routing_types[0] if self._available_input_routing_types else None
        self._available_output_routing_types = [RoutingType("Master")]
        self._output_routing_type = self._available_output_routing_types[0]
        self._playing_slot_index = -1
        self._fired_slot_index = -1
        self._output_meter_level = 0.0
        self._input_meter_level = 0.0
        self._implicit_arm = False

    @property
    def name(self):
        """Live derives an unnamed track's name from where it sits.

        Delete the first of four and the rest become 1-, 2-, 3-; an
        explicitly named track keeps its name wherever it moves. Verified
        against Live 12.4.6 on 2026-09-20: deleting track 0 of
        `1-MIDI 2-MIDI 3-Audio 4-Audio 5-MIDI` gave
        `1-MIDI 2-Audio 3-Audio 4-MIDI`."""
        if self._named:
            return self._name
        return "%d-%s" % (self._position(), "MIDI" if self._kind == "midi" else "Audio")

    @name.setter
    def name(self, value):
        text = "%s" % value
        # Clearing the name gives the track Live's default back.
        self._named = bool(text)
        self._name = text

    def _position(self):
        for i, t in enumerate(self._song._tracks):
            if t is self:
                return i + 1
        return len(self._song._tracks) + 1

    has_midi_input = live_prop("has_midi_input", readonly=True)
    has_audio_input = live_prop("has_audio_input", readonly=True)
    has_audio_output = live_prop("has_audio_output", readonly=True)
    has_midi_output = live_prop("has_midi_output", readonly=True)
    can_be_armed = live_prop("can_be_armed", readonly=True)
    is_foldable = live_prop("is_foldable", readonly=True)
    is_grouped = live_prop("is_grouped", readonly=True)
    is_visible = live_prop("is_visible", readonly=True)
    is_part_of_selection = live_prop("is_part_of_selection", readonly=True)
    clip_slots = seq_prop("clip_slots")
    devices = seq_prop("devices")
    mixer_device = live_prop("mixer_device", readonly=True)
    mute = live_prop("mute", cast=bool)
    solo = live_prop("solo", cast=bool)
    color_index = live_prop("color_index", cast=int)

    current_monitoring_state = live_prop("current_monitoring_state", cast=int)
    available_input_routing_types = _routing_seq("available_input_routing_types")
    available_output_routing_types = _routing_seq("available_output_routing_types")
    output_routing_type = live_prop("output_routing_type")
    playing_slot_index = live_prop("playing_slot_index", readonly=True)
    fired_slot_index = live_prop("fired_slot_index", readonly=True)

    def _meter(attr):
        storage = "_" + attr

        def getter(self):
            self._song._charge("Track.meter_read")
            return getattr(self, storage)
        return property(getter)

    output_meter_left = _meter("output_meter_left")
    output_meter_right = _meter("output_meter_right")
    output_meter_level = _meter("output_meter_level")
    input_meter_level = _meter("input_meter_level")
    del _meter
    implicit_arm = live_prop("implicit_arm", cast=bool)

    @property
    def arm(self):
        return self._arm

    @arm.setter
    def arm(self, value):
        if not self._can_be_armed:
            raise RuntimeError("Track '%s' cannot be armed" % self.name)
        self._arm = bool(value)
        # Live's exclusive_arm is on in a new set: arming one track disarms
        # the rest, which is why only the last track an instrument was
        # loaded onto stays armed after a build.
        if self._arm and self._song._exclusive_arm:
            for other in self._song._tracks:
                if other is not self:
                    other._arm = False

    @property
    def input_routing_type(self):
        return self._input_routing_type

    @input_routing_type.setter
    def input_routing_type(self, value):
        if value not in self._available_input_routing_types:
            raise RuntimeError("Routing type is not available on this track")
        self._input_routing_type = value

    @property
    def color(self):
        return _COLOR_TABLE[self._color_index % len(_COLOR_TABLE)]

    @color.setter
    def color(self, value):
        """Live takes an RGB integer and snaps it to the nearest swatch."""
        self._color_index = _nearest_color_index(value)

    @property
    def arrangement_clips(self):
        return Vector(sorted(self._arrangement_clips, key=lambda c: c._start_time))

    @property
    def canonical_parent(self):
        return self._song

    # ── What Live's Track can do ──
    def duplicate_clip_to_arrangement(self, clip, destination_time):
        self._song._charge("Track.duplicate_clip_to_arrangement")
        if float(destination_time) < 0.0:
            raise RuntimeError("Invalid destination time")
        copy = Clip(self._song, self, clip.length, midi=clip._is_midi_clip,
                    start_time=float(destination_time), in_arrangement=True)
        clip._copy_into(copy)
        # Live trims whatever the copy overlaps on the same track.
        self._trim_around(copy)
        self._arrangement_clips.append(copy)
        self._song._touch()
        return copy

    def _trim_around(self, new):
        kept = []
        for c in self._arrangement_clips:
            if c.end_time <= new.start_time or c.start_time >= new.end_time:
                kept.append(c)
                continue
            # Overlap: Live keeps the part of the old clip outside the new one.
            if c.start_time < new.start_time:
                c._loop_end = c._loop_start + (new.start_time - c.start_time)
                c._end_marker = c._loop_end
                kept.append(c)
            elif c.end_time > new.end_time:
                cut = new.end_time - c.start_time
                c._start_time = new.end_time
                c._loop_start += cut
                c._start_marker = c._loop_start
                kept.append(c)
        self._arrangement_clips = kept

    def create_audio_clip(self, path, position):
        if self._has_midi_input:
            raise RuntimeError("Cannot create an audio clip on a MIDI track")
        clip = self._song._audio_clip(self, path)
        clip._in_arrangement = True
        clip._start_time = float(position)
        self._trim_around(clip)
        self._arrangement_clips.append(clip)
        self._song._touch()
        return clip

    def delete_clip(self, clip):
        for i, c in enumerate(self._arrangement_clips):
            if c == clip:
                del self._arrangement_clips[i]
                self._song._touch()
                return
        for slot in self._clip_slots:
            if slot._clip == clip:
                slot.delete_clip()
                return
        raise RuntimeError("Clip is not on this track")

    def delete_device(self, index):
        self._song._charge("Track.delete_device")
        if index < 0 or index >= len(self._devices):
            raise IndexError("Device index out of range")
        del self._devices[index]
        self._song._touch()

    def stop_all_clips(self, quantized=True):
        self._stop_playing()

    # ── internal: what Live does when a clip fires ──
    def _fire_clip(self, clip):
        for slot in self._clip_slots:
            if slot._clip is not None:
                slot._clip._is_playing = False
        clip._is_playing = True
        for i, slot in enumerate(self._clip_slots):
            if slot._clip == clip:
                self._playing_slot_index = i
                self._fired_slot_index = -1
        if not self._song._is_playing:
            self._song.start_playing()

    def _stop_playing(self):
        for slot in self._clip_slots:
            if slot._clip is not None:
                slot._clip._is_playing = False
        self._playing_slot_index = -1

    def _add_slot(self, scene):
        self._clip_slots.append(ClipSlot(self._song, self, scene))

    def _remove_slot(self, index):
        del self._clip_slots[index]


# ── Scene and CuePoint ──────────────────────────────────────────────────────

class Scene(LiveObject):
    def __init__(self, song, name=""):
        LiveObject.__init__(self)
        self._song = song
        self._name = name
        self._tempo = -1.0
        self._tempo_enabled = False
        self._color_index = 0
        self._is_triggered = False

    name = live_prop("name", cast=str)
    tempo_enabled = live_prop("tempo_enabled", cast=bool)
    color_index = live_prop("color_index", cast=int)
    is_triggered = live_prop("is_triggered", readonly=True)

    @property
    def tempo(self):
        return self._tempo

    @tempo.setter
    def tempo(self, value):
        self._tempo = float(value)
        self._tempo_enabled = float(value) > 0

    @property
    def color(self):
        return _COLOR_TABLE[self._color_index % len(_COLOR_TABLE)]

    @color.setter
    def color(self, value):
        self._color_index = _nearest_color_index(value)

    @property
    def is_empty(self):
        i = self._song._scene_index(self)
        return not any(t._clip_slots[i].has_clip for t in self._song._tracks)

    @property
    def clip_slots(self):
        i = self._song._scene_index(self)
        return Vector(t._clip_slots[i] for t in self._song._tracks)

    def fire(self, force_legato=False, can_select_scene_on_launch=True):
        i = self._song._scene_index(self)
        if self._tempo_enabled and self._tempo > 0:
            self._song.tempo = self._tempo
        for t in self._song._tracks:
            slot = t._clip_slots[i]
            slot.fire()
        if can_select_scene_on_launch:
            self._song._view._selected_scene = self
        if not self._song._is_playing:
            self._song.start_playing()

    def set_fire_button_state(self, state):
        if state:
            self.fire()


class CuePoint(LiveObject):
    def __init__(self, song, name, time):
        LiveObject.__init__(self)
        self._song = song
        self._name = name
        self._time = float(time)

    name = live_prop("name", cast=str)
    time = live_prop("time", readonly=True)

    def jump(self):
        self._song.current_song_time = self._time


class Groove(LiveObject):
    def __init__(self, name, base=4):
        LiveObject.__init__(self)
        self._name = name
        self._base = base
        self._timing_amount = 1.0
        self._quantization_amount = 0.0
        self._random_amount = 0.0
        self._velocity_amount = 0.0

    name = live_prop("name", readonly=True)
    base = live_prop("base", cast=int)
    timing_amount = live_prop("timing_amount", cast=float)
    quantization_amount = live_prop("quantization_amount", cast=float)
    random_amount = live_prop("random_amount", cast=float)
    velocity_amount = live_prop("velocity_amount", cast=float)


class GroovePool(LiveObject):
    def __init__(self):
        LiveObject.__init__(self)
        self._grooves = []

    grooves = seq_prop("grooves")


# ── Views ───────────────────────────────────────────────────────────────────

class SongView(LiveObject):
    """Live.Song.Song.View: what is selected."""

    def __init__(self, song):
        LiveObject.__init__(self)
        self._song = song
        self._selected_track = None
        self._selected_scene = None
        self._highlighted_clip_slot = None
        self._selected_parameter = None
        self._detail_clip = None
        self._draw_mode = False
        self._follow_song = False

    highlighted_clip_slot = live_prop("highlighted_clip_slot")
    selected_parameter = live_prop("selected_parameter", readonly=True)
    detail_clip = live_prop("detail_clip")
    draw_mode = live_prop("draw_mode", cast=bool)
    follow_song = live_prop("follow_song", cast=bool)

    @property
    def selected_track(self):
        return self._selected_track

    @selected_track.setter
    def selected_track(self, track):
        if track not in self._song._all_tracks():
            raise RuntimeError("Track is not in this set")
        self._selected_track = track

    @property
    def selected_scene(self):
        return self._selected_scene

    @selected_scene.setter
    def selected_scene(self, scene):
        if scene not in self._song._scenes:
            raise RuntimeError("Scene is not in this set")
        self._selected_scene = scene

    def select_device(self, device):
        self._selected_device = device
        for t in self._song._all_tracks():
            if device in t._devices:
                self._selected_track = t


class ApplicationView(LiveObject):
    """Live.Application.Application.View: Session or Arranger on screen."""

    _VIEWS = ("Session", "Arranger", "Browser", "Detail", "Detail/Clip", "Detail/DeviceChain")

    def __init__(self):
        LiveObject.__init__(self)
        self._visible = set(["Session", "Browser"])
        self._focused_document_view = "Session"

    focused_document_view = live_prop("focused_document_view", readonly=True)

    def show_view(self, name):
        if name not in self._VIEWS:
            raise RuntimeError("Unknown view '%s'" % name)
        if name in ("Session", "Arranger"):
            self._visible.discard("Session")
            self._visible.discard("Arranger")
            self._focused_document_view = name
        self._visible.add(name)

    def hide_view(self, name):
        self._visible.discard(name)

    def is_view_visible(self, name):
        return name in self._visible

    def focus_view(self, name):
        self.show_view(name)


# ── Browser ─────────────────────────────────────────────────────────────────

class BrowserItem(LiveObject):
    """Live.Browser.BrowserItem: a folder or a loadable thing."""

    def __init__(self, name, uri, children=None, is_device=False, is_loadable=None,
                 loads=None, source="core"):
        LiveObject.__init__(self)
        self._name = name
        self._uri = uri
        self._children = list(children or [])
        self._is_device = is_device
        self._is_folder = is_loadable is False or (is_loadable is None and bool(children))
        self._is_loadable = (not self._is_folder) if is_loadable is None else bool(is_loadable)
        self._is_selected = False
        self._source = source
        # What loading it puts on a track: a Device factory, or a sample path.
        self._loads = loads

    name = live_prop("name", readonly=True)
    uri = live_prop("uri", readonly=True)
    is_device = live_prop("is_device", readonly=True)
    is_folder = live_prop("is_folder", readonly=True)
    is_loadable = live_prop("is_loadable", readonly=True)
    is_selected = live_prop("is_selected", readonly=True)
    source = live_prop("source", readonly=True)
    children = _vector_seq("children", BrowserItemVector)

    def __str__(self):
        """Live prints a browser item as `<TPythonBrowserItem Drums>`."""
        return "<TPythonBrowserItem %s>" % self._name

    def _find(self, uri):
        if self._uri == uri:
            return self
        for c in self._children:
            hit = c._find(uri)
            if hit is not None:
                return hit
        return None


class Browser(LiveObject):
    """Live.Browser.Browser: the roots, and load_item onto the selected track."""

    def __init__(self, song, roots):
        LiveObject.__init__(self)
        self._song = song
        self._roots = roots
        self._hotswap_target = None

    def _root(name):
        def getter(self):
            value = self._roots[name]
            # Live hands back a BrowserItemVector where the root is a list
            # of places rather than one item (user_folders).
            return BrowserItemVector(value) if isinstance(value, list) else value
        return property(getter)

    instruments = _root("instruments")
    drums = _root("drums")
    sounds = _root("sounds")
    audio_effects = _root("audio_effects")
    midi_effects = _root("midi_effects")
    max_for_live = _root("max_for_live")
    plugins = _root("plugins")
    clips = _root("clips")
    samples = _root("samples")
    packs = _root("packs")
    user_library = _root("user_library")
    user_folders = _root("user_folders")
    current_project = _root("current_project")
    hotswap_target = live_prop("hotswap_target")

    del _root

    def _all_roots(self):
        for name in ("instruments", "drums", "sounds", "audio_effects", "midi_effects",
                     "max_for_live", "plugins", "clips", "samples", "packs", "user_library",
                     "current_project"):
            yield self._roots[name]
        for f in self._roots["user_folders"]:
            yield f

    def _find(self, uri):
        for root in self._all_roots():
            hit = root._find(uri)
            if hit is not None:
                return hit
        return None

    def load_item(self, item):
        """Live loads onto the selected track (a highlighted slot for a
        sample), replacing the instrument when the item is one."""
        if not item.is_loadable:
            raise RuntimeError("'%s' is not loadable" % item.name)
        self._song._charge("Browser.load_item")
        track = self._song._view._selected_track
        if track is None:
            raise RuntimeError("No track selected")
        loads = item._loads
        if loads is None:
            raise RuntimeError("'%s' has nothing to load" % item.name)
        if isinstance(loads, str):
            # A sample: onto the highlighted slot of an audio track, or into
            # a Simpler on a MIDI track.
            slot = self._song._view._highlighted_clip_slot
            if track._has_midi_input:
                self._song._replace_instrument(track, Device(item.name, "OriginalSimpler",
                    parameters=[DeviceParameter("Volume", 0.85, 0.0, 1.0, display_db)]))
            elif slot is not None and slot._track == track and not slot.has_clip:
                slot.create_audio_clip(loads)
            else:
                raise RuntimeError("Nowhere to load the sample")
            return
        device = loads(self._song)
        if device._type == TYPE_INSTRUMENT:
            if not track._has_midi_input:
                raise RuntimeError("Cannot load an instrument on an audio track")
            self._song._replace_instrument(track, device)
            # Live arms the track an instrument lands on, so it can be
            # played straight away. Verified on 12.4.6, 2026-09-20:
            # load_browser_item onto an unarmed track left arm True, and
            # left every other track alone.
            if track._can_be_armed:
                track.arm = True
        else:
            track._devices.append(device)
        self._song._view.select_device(device)
        self._song._touch()

    def relation_to_hotswap_target(self, item):
        return 0

    def stop_preview(self):
        pass


class Application(LiveObject):
    """Live.Application.Application."""

    def __init__(self, song, browser, version=(12, 4, 6)):
        LiveObject.__init__(self)
        self._song = song
        self._browser = browser
        self._view = ApplicationView()
        self._version = version
        self._control_surfaces = ()
        self._open_dialog_count = 0
        self._current_dialog_message = ""

    browser = live_prop("browser", readonly=True)
    view = live_prop("view", readonly=True)
    control_surfaces = live_prop("control_surfaces", readonly=True)
    open_dialog_count = live_prop("open_dialog_count", readonly=True)
    current_dialog_message = live_prop("current_dialog_message", readonly=True)

    def get_major_version(self):
        return self._version[0]

    def get_minor_version(self):
        return self._version[1]

    def get_bugfix_version(self):
        return self._version[2]

    def get_document(self):
        return self._song

    def press_current_dialog_button(self, index):
        pass


# ── Song ────────────────────────────────────────────────────────────────────

class Song(LiveObject):
    """Live.Song.Song: the set. Built empty, or from `default_set()`."""

                                                        
    def __init__(self, scenes=8, version=(12, 4, 6)):
        LiveObject.__init__(self)
        self._tracks = []
        self._return_tracks = []
        self._scenes = [Scene(self) for _ in range(scenes)]
        self._cue_points = []
        self._master_track = Track(self, "Master", kind="master")
        self._view = SongView(self)
        self._groove_pool = GroovePool()
        self._tempo = 120.0
        self._signature_numerator = 4
        self._signature_denominator = 4
        self._is_playing = False
        self._current_song_time = 0.0
        self._loop = False
        self._loop_start = 0.0
        self._loop_length = 16.0
        self._back_to_arranger = False
        self._record_mode = False
        self._arrangement_overdub = False
        self._session_record = False
        self._session_automation_record = False
        self._metronome = False
        self._clip_trigger_quantization = Quantization(Q_BAR)
        self._midi_recording_quantization = RecordingQuantization(0)
        self._root_note = 0
        self._scale_name = "Major"
        # Live 12 ships with the global scale switched on (C Major):
        # verified 2026-09-20 against a fresh 12.4.6 set.
        self._scale_mode = True
        self._groove_amount = 0.0
        self._file_path = ""
        self._name = ""
        self._nudge_down = False
        self._nudge_up = False
        self._punch_in = False
        self._punch_out = False
        self._exclusive_arm = True
        self._exclusive_solo = True
        self._select_on_launch = True
        self._can_undo = False
        self._can_redo = False
        self._undo_depth = 0
        self._undo_steps = 0
        self._changes = 0
        self._version = version
        self._browser = Browser(self, default_browser(self))
        self._application = Application(self, self._browser, version)
        self._playing_since = None
        self._color_cursor = 0
        # Where the model reads "now" and what a Live call costs. A wall
        # clock and no charge by default: `scripts/fake-live.py` swaps in
        # the measured table, `harness.load()` a driven clock.
        self._clock = WallClock()
        self._latency = None

    # ── plain properties ──
    signature_numerator = live_prop("signature_numerator", cast=int)
    signature_denominator = live_prop("signature_denominator", cast=int)
    # Live lets the transport be started by writing this.
    is_playing = live_prop("is_playing", cast=bool)
    loop = live_prop("loop", cast=bool)
    loop_start = live_prop("loop_start", cast=float)
    loop_length = live_prop("loop_length", cast=float)
    back_to_arranger = live_prop("back_to_arranger", cast=bool)
    record_mode = live_prop("record_mode", cast=bool)
    arrangement_overdub = live_prop("arrangement_overdub", cast=bool)
    session_record = live_prop("session_record", cast=bool)
    session_automation_record = live_prop("session_automation_record", cast=bool)
    metronome = live_prop("metronome", cast=bool)
    clip_trigger_quantization = live_prop("clip_trigger_quantization", cast=Quantization)
    midi_recording_quantization = live_prop("midi_recording_quantization", cast=RecordingQuantization)
    root_note = live_prop("root_note", cast=int)
    scale_name = live_prop("scale_name", cast=str)
    scale_mode = live_prop("scale_mode", cast=bool)
    groove_amount = live_prop("groove_amount", cast=float)
    # Song.file_path is Live's path to the open Set, read-only, and empty
    # when the Set has never been saved (Cycling '74 LOM reference, Song >
    # file_path). It had the Clip helper `_audio_only` on it, whose getter
    # reads `self._is_midi_clip`, so every read raised
    # `AttributeError: 'Song' object has no attribute '_is_midi_clip'`
    # (measured against this model, 2026-09-20). `_file_path` is the
    # backing, so a test can be a saved Set or an unsaved one.
    file_path = live_prop("file_path", readonly=True)
    name = live_prop("name", readonly=True)
    nudge_down = live_prop("nudge_down", cast=bool)
    nudge_up = live_prop("nudge_up", cast=bool)
    punch_in = live_prop("punch_in", cast=bool)
    punch_out = live_prop("punch_out", cast=bool)
    exclusive_arm = live_prop("exclusive_arm", readonly=True)
    exclusive_solo = live_prop("exclusive_solo", readonly=True)
    select_on_launch = live_prop("select_on_launch", readonly=True)
    can_undo = live_prop("can_undo", readonly=True)
    can_redo = live_prop("can_redo", readonly=True)
    master_track = live_prop("master_track", readonly=True)
    view = live_prop("view", readonly=True)
    groove_pool = live_prop("groove_pool", readonly=True)
    tracks = seq_prop("tracks")
    return_tracks = seq_prop("return_tracks")
    scenes = seq_prop("scenes")
    visible_tracks = seq_prop("tracks")

    @property
    def cue_points(self):
        return Vector(sorted(self._cue_points, key=lambda c: c._time))

    @property
    def tempo(self):
        return self._tempo

    @tempo.setter
    def tempo(self, value):
        value = float(value)
        if not (20.0 <= value <= 999.0):
            raise RuntimeError("Tempo %r is out of Live's range (20..999)" % value)
        self._tempo = value

    @property
    def current_song_time(self):
        # Live's play position moves with the transport, so a reader that
        # keeps asking sees it move. Without this the cue tick and the clock
        # channel read a frozen 0.0 while the transport runs.
        self._advance()
        return self._current_song_time

    @current_song_time.setter
    def current_song_time(self, value):
        value = float(value)
        if value < 0.0:
            raise RuntimeError("Invalid song time")
        self._current_song_time = value
        # Live relocates: play continues from here, not from wherever the
        # transport had got to. Without this the next `_advance` would add
        # everything elapsed since `start_playing`.
        if self._is_playing:
            self._playing_since = self._now()

    @property
    def song_length(self):
        """Live: the end of the last Arrangement clip, at least one bar."""
        end = 0.0
        for t in self._tracks:
            for c in t._arrangement_clips:
                end = max(end, c.end_time)
        return max(end, float(self._signature_numerator))

    @property
    def last_event_time(self):
        return self.song_length

    # ── transport ──
    def start_playing(self):
        self._is_playing = True
        self._playing_since = self._now()

    def stop_playing(self):
        self._advance()
        self._is_playing = False
        for t in self._tracks:
            t._stop_playing()

    def continue_playing(self):
        self._is_playing = True
        self._playing_since = self._now()

    def stop_all_clips(self, quantized=True):
        for t in self._tracks:
            t._stop_playing()

    def jump_by(self, beats):
        self.current_song_time = max(0.0, self._current_song_time + float(beats))

    def jump_to_next_cue(self):
        for c in self.cue_points:
            if c._time > self._current_song_time + 1e-6:
                self.current_song_time = c._time
                return True
        return False

    def jump_to_prev_cue(self):
        prev = None
        for c in self.cue_points:
            if c._time < self._current_song_time - 1e-6:
                prev = c
        if prev is None:
            return False
        self.current_song_time = prev._time
        return True

    def get_current_beats_song_time(self):
        self._advance()
        bpb = self._signature_numerator
        t = self._current_song_time
        return types.SimpleNamespace(
            bars=int(t // bpb) + 1,
            beats=int(t % bpb) + 1,
            sub_division=int((t % 1.0) * 4) + 1,
            ticks=int((t % 0.25) * 240))

    def get_current_smpte_song_time(self, fmt):
        seconds = self._current_song_time * 60.0 / self._tempo
        return types.SimpleNamespace(hours=int(seconds // 3600), minutes=int(seconds // 60) % 60,
                                     seconds=int(seconds) % 60, frames=0)

    def _now(self):
        return self._clock.now()

    def _charge(self, call):
        """What this Live call costs, off the measured table. No table
        means a free call, which is what the script's own suite runs on."""
        if self._latency is not None:
            return self._latency.charge(call)
        return 0.0

    def _advance(self):
        """Play position moves while playing: the beats since we started."""
        if self._is_playing and self._playing_since is not None:
            elapsed = self._now() - self._playing_since
            self._playing_since = self._now()
            beats = elapsed * self._tempo / 60.0
            t = self._current_song_time + beats
            if self._loop and self._loop_length > 0 and t >= self._loop_start + self._loop_length:
                span = self._loop_length
                t = self._loop_start + (t - self._loop_start) % span
            self._current_song_time = t

    # ── building the set ──
    def _touch(self):
        self._changes += 1
        self._can_undo = True

    def _next_color(self):
        c = self._color_cursor
        self._color_cursor = (self._color_cursor + 1) % len(_COLOR_TABLE)
        return c

    def _all_tracks(self):
        return list(self._tracks) + list(self._return_tracks) + [self._master_track]

    def _scene_index(self, scene):
        for i, s in enumerate(self._scenes):
            if s == scene:
                return i
        raise RuntimeError("Scene is not in this set")

    def _insert_track(self, track, index):
        if index < -1 or index > len(self._tracks):
            raise IndexError("Track index out of range")
        for s in self._scenes:
            track._add_slot(s)
        if index == -1:
            self._tracks.append(track)
        else:
            self._tracks.insert(index, track)
        self._view._selected_track = track
        self._touch()

    def create_midi_track(self, index=-1):
        self._charge("Song.create_midi_track")
        # No name: Live shows the position until someone sets one.
        self._insert_track(Track(self, "", "midi", self._next_color()), index)

    def create_audio_track(self, index=-1):
        self._charge("Song.create_audio_track")
        self._insert_track(Track(self, "", "audio", self._next_color()), index)

    def create_return_track(self):
        self._charge("Song.create_return_track")
        if len(self._return_tracks) >= 12:
            raise RuntimeError("Live allows at most 12 return tracks")
        letter = chr(ord("A") + len(self._return_tracks))
        track = Track(self, "%s-Return" % letter, "return", self._next_color())
        self._return_tracks.append(track)
        # Every track gains a send.
        for t in self._tracks:
            t._mixer_device._sends.append(MixerDevice._new_send(len(self._return_tracks) - 1, track))
        self._touch()

    def delete_track(self, index):
        self._charge("Song.delete_track")
        if index < 0 or index >= len(self._tracks):
            raise IndexError("Track index out of range")
        gone = self._tracks.pop(index)
        if self._view._selected_track == gone:
            self._view._selected_track = self._tracks[min(index, len(self._tracks) - 1)] if self._tracks else self._master_track
        self._touch()

    def delete_return_track(self, index):
        if index < 0 or index >= len(self._return_tracks):
            raise IndexError("Return track index out of range")
        del self._return_tracks[index]
        for t in self._tracks:
            del t._mixer_device._sends[index]
        self._touch()

    def duplicate_track(self, index):
        src = self._tracks[index]
        copy = Track(self, src._name, src._kind, src._color_index)
        self._insert_track(copy, index + 1)
        for i, slot in enumerate(src._clip_slots):
            if slot._clip is not None:
                new = Clip(self, copy, slot._clip.length, midi=slot._clip._is_midi_clip)
                slot._clip._copy_into(new)
                copy._clip_slots[i]._clip = new

    def create_scene(self, index=-1):
        self._charge("Song.create_scene")
        if index < -1 or index > len(self._scenes):
            raise IndexError("Scene index out of range")
        scene = Scene(self)
        if index == -1:
            self._scenes.append(scene)
            for t in self._tracks:
                t._add_slot(scene)
        else:
            self._scenes.insert(index, scene)
            for t in self._tracks:
                t._clip_slots.insert(index, ClipSlot(self, t, scene))
        self._view._selected_scene = scene
        self._touch()

    def delete_scene(self, index):
        if index < 0 or index >= len(self._scenes):
            raise IndexError("Scene index out of range")
        if len(self._scenes) == 1:
            raise RuntimeError("Cannot delete the last scene")
        del self._scenes[index]
        for t in self._tracks:
            t._remove_slot(index)
        self._touch()

    def duplicate_scene(self, index):
        self._charge("Song.duplicate_scene")
        if index < 0 or index >= len(self._scenes):
            raise IndexError("Scene index out of range")
        src = self._scenes[index]
        self.create_scene(index + 1)
        new = self._scenes[index + 1]
        new._name = src._name
        new._tempo, new._tempo_enabled = src._tempo, src._tempo_enabled
        for t in self._tracks:
            old = t._clip_slots[index]._clip
            if old is not None:
                c = Clip(self, t, old.length, midi=old._is_midi_clip)
                old._copy_into(c)
                t._clip_slots[index + 1]._clip = c

    def capture_and_insert_scene(self):
        """Live: a new scene after the selected one, holding what plays."""
        after = self._scene_index(self._view._selected_scene) if self._view._selected_scene else len(self._scenes) - 1
        self.create_scene(after + 1)
        for t in self._tracks:
            i = t._playing_slot_index
            if i >= 0 and t._clip_slots[i]._clip is not None:
                old = t._clip_slots[i]._clip
                c = Clip(self, t, old.length, midi=old._is_midi_clip)
                old._copy_into(c)
                t._clip_slots[after + 1]._clip = c

    def set_or_delete_cue(self):
        """Toggle a cue point at the play position, as Live's button does."""
        t = self._current_song_time
        for c in self._cue_points:
            if abs(c._time - t) < 1e-6:
                self._cue_points.remove(c)
                self._touch()
                return
        self._cue_points.append(CuePoint(self, "%d" % (int(t // self._signature_numerator) + 1), t))
        self._touch()

    def move_device(self, device, target, index):
        self._charge("Song.move_device")
        """Live: a device from wherever it is to `index` on `target`, which
        is a track or a chain."""
        owner = None
        for t in self._all_tracks():
            if device in t._devices:
                owner = t
        if owner is None:
            raise RuntimeError("Device is not in this set")
        owner._devices.remove(device)
        devices = target._devices
        index = max(0, min(int(index), len(devices)))
        devices.insert(index, device)
        self._touch()

    def _replace_instrument(self, track, device):
        """Loading an instrument replaces the one there, as Live does."""
        track._devices = [d for d in track._devices if d._type != TYPE_INSTRUMENT]
        at = 0
        for i, d in enumerate(track._devices):
            if d._type == TYPE_MIDI_EFFECT:
                at = i + 1
        track._devices.insert(at, device)
        device._canonical_parent = track

    def _audio_clip(self, track, path):
        name = path.replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0]
        return Clip(self, track, 4.0, midi=False, name=name, file_path=path)

    # ── undo ──
    def begin_undo_step(self):
        self._undo_depth += 1

    def end_undo_step(self):
        if self._undo_depth == 0:
            raise RuntimeError("end_undo_step without begin_undo_step")
        self._undo_depth -= 1
        if self._undo_depth == 0:
            self._undo_steps += 1

    def undo(self):
        self._can_redo = True

    def redo(self):
        pass

    def tap_tempo(self):
        pass

    def re_enable_automation(self):
        for t in self._all_tracks():
            for d in t._devices:
                for p in d._parameters:
                    p.re_enable_automation()

    def is_cue_point_selected(self):
        return any(abs(c._time - self._current_song_time) < 1e-6 for c in self._cue_points)


# ── A set to start from ─────────────────────────────────────────────────────

def _instrument(name, class_name, params=(), device_type=TYPE_INSTRUMENT):
    def make(song):
        return Device(name, class_name, [DeviceParameter(*p) for p in params], device_type)
    return make


def _rack(name, drum=False):
    def make(song):
        r = RackDevice(name, song, "DrumGroupDevice" if drum else "InstrumentGroupDevice", drum=drum)
        if drum:
            for i, pad in enumerate(("Kick", "Snare", "Hat Closed", "Hat Open")):
                r._fill_pad(36 + i, pad)
        return r
    return make


_EQ_PARAMS = (
    ("1 Frequency A", 0.5, 0.0, 1.0, display_hz(10.0, 22000.0)),
    ("1 Gain A", 0.5, 0.0, 1.0, lambda v: "%.1f dB" % ((float(v) - 0.5) * 30.0)),
    ("1 Filter Type A", 3.0, 0.0, 7.0, None, ("Low Cut 48", "Low Cut 12", "Low Shelf", "Bell",
                                              "Notch", "High Shelf", "High Cut 12", "High Cut 48")),
)
_REVERB_PARAMS = (
    ("Dry/Wet", 1.0, 0.0, 1.0, display_percent),
    ("Decay Time", 0.3, 0.0, 1.0, lambda v: "%.0f ms" % (200 + float(v) * 60000)),
    ("PreDelay", 0.1, 0.0, 1.0, lambda v: "%.1f ms" % (float(v) * 250)),
)
_DELAY_PARAMS = (
    ("Dry/Wet", 0.3, 0.0, 1.0, display_percent),
    ("Feedback", 0.4, 0.0, 1.0, display_percent),
)
_SYNTH_PARAMS = (
    ("Filter Freq", 0.6, 0.0, 1.0, display_hz(20.0, 20000.0)),
    ("Filter Res", 0.2, 0.0, 1.0, display_percent),
    ("Volume", 0.85, 0.0, 1.0, display_db),
)


def default_browser(song):
    """Live's browser roots with a small, realistic tree: a handful of
    instruments, drum kits, effects and a user folder with samples."""
    def dev(name, class_name, params=(), dtype=TYPE_INSTRUMENT, uri=None):
        uri = uri or "query:Synths#%s" % name.replace(" ", "%20")
        return BrowserItem(name, uri, is_device=True, loads=_instrument(name, class_name, params, dtype))

    instruments = BrowserItem("Instruments", "query:Synths", [
        dev("Analog", "UltraAnalog", _SYNTH_PARAMS),
        dev("Operator", "Operator", _SYNTH_PARAMS),
        dev("Wavetable", "InstrumentVector", _SYNTH_PARAMS),
        dev("Simpler", "OriginalSimpler", _SYNTH_PARAMS),
        BrowserItem("Instrument Rack", "query:Synths#Instrument%20Rack", is_device=True, loads=_rack("Instrument Rack")),
        BrowserItem("Drum Rack", "query:Synths#Drum%20Rack", is_device=True, loads=_rack("Drum Rack", drum=True)),
        BrowserItem("Bass", "query:Synths:Bass", [
            BrowserItem("Basic Sub.adg", "query:Synths:Bass#Basic%20Sub.adg", loads=_rack("Basic Sub")),
            BrowserItem("Growl Bass.adg", "query:Synths:Bass#Growl%20Bass.adg", loads=_rack("Growl Bass")),
        ]),
    ])
    drums = BrowserItem("Drums", "query:Drums", [
        BrowserItem("Drum Rack", "query:Drums#Drum%20Rack", is_device=True, loads=_rack("Drum Rack", drum=True)),
        BrowserItem("Kits", "query:Drums:Kits", [
            BrowserItem("Rollin Breaks Kit.adg", "query:Drums:Kits#Rollin%20Breaks%20Kit.adg", loads=_rack("Rollin Breaks Kit", drum=True)),
            BrowserItem("808 Core Kit.adg", "query:Drums:Kits#808%20Core%20Kit.adg", loads=_rack("808 Core Kit", drum=True)),
        ]),
    ])
    sounds = BrowserItem("Sounds", "query:Sounds", [
        BrowserItem("Pad", "query:Sounds:Pad", [
            BrowserItem("Warm Pad.adg", "query:Sounds:Pad#Warm%20Pad.adg", loads=_rack("Warm Pad")),
        ]),
        BrowserItem("Keys", "query:Sounds:Keys", [
            BrowserItem("Grand Piano.adg", "query:Sounds:Keys#Grand%20Piano.adg", loads=_rack("Grand Piano")),
        ]),
    ])
    audio_effects = BrowserItem("Audio Effects", "query:AudioFx", [
        dev("EQ Eight", "Eq8", _EQ_PARAMS, TYPE_AUDIO_EFFECT, "query:AudioFx#EQ%20Eight"),
        dev("Reverb", "Reverb", _REVERB_PARAMS, TYPE_AUDIO_EFFECT, "query:AudioFx#Reverb"),
        dev("Delay", "Delay", _DELAY_PARAMS, TYPE_AUDIO_EFFECT, "query:AudioFx#Delay"),
        dev("Compressor", "Compressor2", (("Threshold", 0.8, 0.0, 1.0, display_db), ("Ratio", 0.3, 0.0, 1.0, lambda v: "%.1f : 1" % (1 + float(v) * 9))), TYPE_AUDIO_EFFECT, "query:AudioFx#Compressor"),
        dev("Auto Filter", "AutoFilter", (("Frequency", 0.7, 0.0, 1.0, display_hz(20.0, 20000.0)),), TYPE_AUDIO_EFFECT, "query:AudioFx#Auto%20Filter"),
        dev("Utility", "StereoGain", (("Gain", 0.5, 0.0, 1.0, lambda v: "%.1f dB" % ((float(v) - 0.5) * 70)),), TYPE_AUDIO_EFFECT, "query:AudioFx#Utility"),
    ])
    midi_effects = BrowserItem("MIDI Effects", "query:MidiFx", [
        dev("Arpeggiator", "MidiArpeggiator", (("Rate", 0.5, 0.0, 1.0, display_plain),), TYPE_MIDI_EFFECT, "query:MidiFx#Arpeggiator"),
        dev("Chord", "MidiChord", (), TYPE_MIDI_EFFECT, "query:MidiFx#Chord"),
        dev("Scale", "MidiScale", (), TYPE_MIDI_EFFECT, "query:MidiFx#Scale"),
    ])
    samples = BrowserItem("Samples", "query:Samples", [
        BrowserItem("kick.wav", "userfolder:/Samples#kick.wav", loads="/Samples/kick.wav"),
        BrowserItem("snare.wav", "userfolder:/Samples#snare.wav", loads="/Samples/snare.wav"),
    ])
    user_library = BrowserItem("User Library", "query:UserLibrary", [
        BrowserItem("Defaults", "query:UserLibrary#Defaults", []),
        samples,
    ])
    user_folders = [
        BrowserItem("My Samples", "userfolder:/Users/me/Samples", [
            BrowserItem("loop.wav", "userfolder:/Users/me/Samples#loop.wav", loads="/Users/me/Samples/loop.wav"),
        ]),
    ]
    return {
        "instruments": instruments, "drums": drums, "sounds": sounds,
        "audio_effects": audio_effects, "midi_effects": midi_effects,
        "max_for_live": BrowserItem("Max for Live", "query:MaxForLive", []),
        "plugins": BrowserItem("Plug-ins", "query:Plugins", []),
        "clips": BrowserItem("Clips", "query:Clips", []),
        "samples": samples,
        "packs": BrowserItem("Packs", "query:Packs", [
            BrowserItem("Core Library", "query:Packs#Core%20Library", [drums, sounds]),
        ]),
        "user_library": user_library,
        "user_folders": user_folders,
        "current_project": BrowserItem("Current Project", "query:CurrentProject", []),
    }


def default_set(version=(12, 4, 6)):
    """What a new Live set is: two MIDI tracks, two audio tracks, eight
    scenes, two returns (A-Reverb, B-Delay), a Master at 0 dB, 120 BPM."""
    song = Song(scenes=8, version=version)
    song.create_return_track()
    song._return_tracks[0]._name = "A-Reverb"
    song._return_tracks[0]._devices.append(_instrument("Reverb", "Reverb", _REVERB_PARAMS, TYPE_AUDIO_EFFECT)(song))
    song.create_return_track()
    song._return_tracks[1]._name = "B-Delay"
    song._return_tracks[1]._devices.append(_instrument("Delay", "Delay", _DELAY_PARAMS, TYPE_AUDIO_EFFECT)(song))
    song.create_midi_track()
    song.create_midi_track()
    song.create_audio_track()
    song.create_audio_track()
    song._view._selected_track = song._tracks[0]
    song._view._selected_scene = song._scenes[0]
    song._changes = 0
    song._can_undo = False
    return song


def live_module(song):
    """A stand-in for the `Live` module: the constants the script imports."""
    live = types.ModuleType("Live")
    live.Song = types.SimpleNamespace(
        Song=Song,
        Quantization=types.SimpleNamespace(
            q_no_q=Q_NONE, q_8_bars=Q_8_BARS, q_4_bars=Q_4_BARS,
            q_2_bars=Q_2_BARS, q_bar=Q_BAR, q_half=Q_HALF,
            q_half_triplet=Q_HALF_TRIPLET, q_quarter=Q_QUARTER,
            q_quarter_triplet=Q_QUARTER_TRIPLET, q_eight=Q_EIGHTH,
            q_eight_triplet=Q_EIGHTH_TRIPLET, q_sixtenth=Q_SIXTEENTH,
            q_sixtenth_triplet=Q_SIXTEENTH_TRIPLET, q_thirtytwoth=Q_THIRTYSECOND),
        RecordingQuantization=types.SimpleNamespace(rec_q_no_q=0, rec_q_quarter=1, rec_q_eight=2,
                                                   rec_q_eight_triplet=3, rec_q_eight_eight_triplet=4,
                                                   rec_q_sixtenth=5, rec_q_sixtenth_triplet=6,
                                                   rec_q_sixtenth_sixtenth_triplet=7, rec_q_thirtysecond=8))
    live.Clip = types.SimpleNamespace(Clip=Clip, MidiNote=MidiNote, AutomationEnvelope=AutomationEnvelope,
                                      LaunchMode=types.SimpleNamespace(trigger=0, gate=1, toggle=2, repeat=3))
    live.Track = types.SimpleNamespace(Track=Track, RoutingType=RoutingType)
    live.ClipSlot = types.SimpleNamespace(ClipSlot=ClipSlot)
    live.Device = types.SimpleNamespace(Device=Device, DeviceType=types.SimpleNamespace(
        audio_effect=TYPE_AUDIO_EFFECT, instrument=TYPE_INSTRUMENT, midi_effect=TYPE_MIDI_EFFECT))
    live.DeviceParameter = types.SimpleNamespace(DeviceParameter=DeviceParameter,
                                                 AutomationState=types.SimpleNamespace(none=0, playing=1, overridden=2))
    live.MixerDevice = types.SimpleNamespace(MixerDevice=MixerDevice)
    live.Scene = types.SimpleNamespace(Scene=Scene)
    live.Browser = types.SimpleNamespace(Browser=Browser, BrowserItem=BrowserItem)
    live.Application = types.SimpleNamespace(Application=Application, get_application=lambda: song._application)
    live.RackDevice = types.SimpleNamespace(RackDevice=RackDevice)
    live.DrumPad = types.SimpleNamespace(DrumPad=DrumPad)
    live.Chain = types.SimpleNamespace(Chain=Chain)
    return live


# Live's listener boilerplate, on every object that has it. Last, so every
# property defined above is covered.
for _cls in (Song, Track, ClipSlot, Clip, Device, RackDevice, Chain, DrumPad,
             DeviceParameter, MixerDevice, Scene, CuePoint, Groove, GroovePool,
             SongView, ApplicationView, Browser, BrowserItem, Application,
             RoutingType, AutomationEnvelope):
    with_listeners(_cls)
del _cls


# Live calls both of them `View`; the model needs two classes but only one
# name, and `describe` reports `type(obj).__name__`.
SongView.__name__ = SongView.__qualname__ = "View"
ApplicationView.__name__ = ApplicationView.__qualname__ = "View"


# The channel strip Live gives a MIDI or audio track and withholds from
# the master and the returns. `mute` and `solo` stay: a return has both.
for _name in ("arm", "arrangement_clips", "current_monitoring_state",
              "playing_slot_index", "fired_slot_index", "implicit_arm"):
    setattr(Track, _name, _playable_only(getattr(Track, _name)))
for _name in ("mute", "solo"):
    setattr(Track, _name, _not_on_master(getattr(Track, _name)))
del _name
MixerDevice.crossfade_assign = _not_on_master_mixer(MixerDevice.crossfade_assign)
