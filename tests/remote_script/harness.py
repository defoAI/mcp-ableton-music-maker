"""Load the Remote Script under a plain interpreter, against the fake Live.

Live's own Python is the only place the script normally runs. Here
`_Framework.ControlSurface` is a stand-in and `Live` is
`fake_live.live_module()`, but `song()` and `application()` return the real
model from `fake_live` — a set that a track created is really in, a note
added is really read back from. `schedule_message` puts callbacks on a queue
the test drains by hand with `tick()`, so the executor's slices, the clock
tick and the socket drain run deterministically. The script is exec'd from
source, which also proves it still parses outside Live. Python 3 runs the
tests; the script itself stays 2.7-compatible.

The script is two files. `load()` exec's the loader and then loads `body.py`
through the loader's own `_load_body_module`, exactly as it happens inside
Live, so `ns["Body"]` is the class the instance wears and `ns["Loader"]` is
the frozen half. `instance(ns)` builds a `Body`; `instance(ns,
body=False)` builds a bare `Loader`, which is what Live gets when the body
will not load.

    ns = harness.load()              # a fresh default_set() behind it
    script = harness.instance(ns)
    song = script.song()             # fake_live.Song
    script._dispatch("create_midi_track", {}, None)
    song.tracks[-1].name             # really there

`load(song=...)` puts a set of the test's own making behind the script;
`load(latency=...)` gives the model Live's per-call cost (see
`fake_live.LatencyTable`), which is off by default so the script tests
stay fast.
"""
import io
import os
import sys
import types
import collections

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import fake_live  # noqa: E402

SCRIPT_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "AbletonMusicMaker_Remote_Script")
SCRIPT = os.path.join(SCRIPT_DIR, "__init__.py")   # the loader
BODY = os.path.join(SCRIPT_DIR, "body.py")         # every handler

# The set the next FakeSurface picks up. `load()` sets it; the surface is
# built later, by the script's own constructor, and has no other way to be
# told which set it is looking at.
_NEXT_SONG = []


class FakeSurface(object):
    """What the script asks of Live's ControlSurface, recorded and driven.

    `song()` and `application()` are the fake Live; everything else is the
    part of ControlSurface the script uses, with `schedule_message` turned
    into a queue the test drains.
    """

    def __init__(self, c_instance=None):
        self.logged = []
        self.shown = []
        self.scheduled = collections.deque()   # (due_tick, callback)
        self.now_tick = 0
        self._song_obj = _NEXT_SONG.pop() if _NEXT_SONG else fake_live.default_set()
        self._app_obj = self._song_obj._application

    def log_message(self, message):
        self.logged.append("%s" % message)

    def show_message(self, message):
        self.shown.append("%s" % message)

    def schedule_message(self, ticks, callback):
        self.scheduled.append((self.now_tick + max(0, int(ticks)), callback))

    def song(self):
        return self._song_obj

    def _all_songs(self):
        """Every set this surface drives. One, unless a subclass serves a set
        per connection — each still gets Live's frame."""
        return (self._song_obj,)

    def application(self):
        return self._app_obj

    def disconnect(self):
        pass

    # ── driving ───────────────────────────────────────────────────────────
    def tick(self, n=1):
        """Advance n ticks, running every callback that is due, in order."""
        ran = 0
        for _ in range(n):
            self.now_tick += 1
            # Live grants a playhead move on its own frame, before the
            # callbacks this tick is for run — which is why a handler that
            # moves the playhead and waits a tick can read it back, and one
            # that does not, cannot.
            for song in self._all_songs():
                song._apply_pending()
            due = [c for (t, c) in self.scheduled if t <= self.now_tick]
            self.scheduled = collections.deque((t, c) for (t, c) in self.scheduled if t > self.now_tick)
            for cb in due:
                cb()
                ran += 1
            # Live's tick is 100 ms. The callbacks run at the instant the
            # tick is for, and time moves on afterwards — so a test that
            # puts the playhead somewhere and ticks once sees the handler
            # read the position it set, not that position plus a tick.
            clock = self._song_obj._clock
            if clock.driven:
                clock.advance(fake_live.TICK_S)
        return ran

    def run_until(self, done, limit=400):
        """Tick until `done()` is true, or fail. For generators the executor
        spreads over ticks."""
        for _ in range(limit):
            if done():
                return True
            self.tick()
        return done()


def two_track_set(clips=False):
    """Kick and Bass over two named scenes — a real set, not a stub.
    `clips=True` puts a four-bar clip in every slot, so firing a scene is
    something the set can be asked about afterwards."""
    song = fake_live.Song(scenes=2)
    for name in ("Kick", "Bass"):
        song.create_midi_track()
        song.tracks[-1].name = name
    song.scenes[0].name = "Intro · 8"
    song.scenes[1].name = "Drop · 8"
    if clips:
        for t in song.tracks:
            for i, slot in enumerate(t.clip_slots):
                slot.create_clip(16.0)
                slot.clip.name = "%s %d" % (t.name, i + 1)
    return song


def load(bind=False, song=None, version=(12, 4, 6), latency=None, clock=None,
         surface_class=None):
    """The script's namespace with Live stubbed by the model. `bind=False`
    keeps the constructor from opening a real socket: the class is returned
    and the test builds an instance with `instance(ns)`.

    `song` puts a set of the caller's making behind the script (default: a
    fresh `default_set()`); `latency` a `fake_live.LatencyTable` so calls
    cost what they cost in Live (default: free); `clock` a wall clock for a
    test that runs on real time (a real socket, a real ticker thread) rather
    than the driven one the rest use."""
    song = song if song is not None else fake_live.default_set(version)
    song._clock = clock if clock is not None else fake_live.DrivenClock()
    if latency is not None:
        latency.clock = song._clock
    song._latency = latency
    del _NEXT_SONG[:]
    _NEXT_SONG.append(song)

    framework = types.ModuleType("_Framework")
    surface = types.ModuleType("_Framework.ControlSurface")
    surface.ControlSurface = surface_class or FakeSurface
    framework.ControlSurface = surface
    sys.modules["_Framework"] = framework
    sys.modules["_Framework.ControlSurface"] = surface
    sys.modules["Live"] = fake_live.live_module(song)
    ns = {"__name__": "ableton_remote_script_under_test", "__file__": SCRIPT}
    with io.open(SCRIPT, encoding="utf-8") as handle:
        source = handle.read()
    exec(compile(source, SCRIPT, "exec"), ns)
    # The body, loaded the way the loader loads it inside Live: by path, into
    # a fresh module, with the loader's namespace already in it.
    module = ns["_load_body_module"]()
    ns["_body_module"] = module
    ns["Body"] = ns["_body_class"](module)
    ns["SCRIPT_VERSION"] = module.SCRIPT_VERSION
    ns["SCRIPT_CAPABILITIES"] = module.SCRIPT_CAPABILITIES
    # The body's own module-level names (`_parse_path`, `PATH_ROOTS`, ...)
    # reachable the way they were when this was one file. The loader's win:
    # what the body was handed is the loader's object, not a copy.
    for key, value in vars(module).items():
        if not key.startswith("__"):
            ns.setdefault(key, value)
    ns["_fake_song"] = song
    return ns


def instance(ns, open_socket=False, body=True):
    """The script's one instance, with the socket server replaced unless
    asked for. `body=False` gives the bare loader — what Live is left with
    when `body.py` will not load, which still listens and still reloads."""
    cls = ns["Body"] if body else ns["Loader"]
    if not open_socket:
        ns["Loader"].start_server = lambda self: setattr(self, "running", True)
    obj = cls(None)
    obj._body_module = ns["_body_module"] if body else None
    return obj
