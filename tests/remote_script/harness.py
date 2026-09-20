"""Load the Remote Script under a plain interpreter, with Live stubbed.

Live's own Python is the only place the script normally runs. Here
`_Framework.ControlSurface` and `Live` are stand-ins, and `schedule_message`
puts callbacks on a queue the test drains by hand with `tick()`, so the
executor's slices, the clock tick and the socket drain run deterministically.
The script is exec'd from source, which also proves it still parses outside
Live. Python 3 runs the tests; the script itself stays 2.7-compatible.
"""
import io
import os
import sys
import types
import collections

SCRIPT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "AbletonMusicMaker_Remote_Script", "__init__.py")


class FakeSurface(object):
    """What the script asks of Live's ControlSurface, recorded and driven."""

    def __init__(self, c_instance=None):
        self.logged = []
        self.shown = []
        self.scheduled = collections.deque()   # (due_tick, callback)
        self.now_tick = 0
        self._song_obj = FakeSong()
        self._app_obj = FakeApplication()

    def log_message(self, message):
        self.logged.append("%s" % message)

    def show_message(self, message):
        self.shown.append("%s" % message)

    def schedule_message(self, ticks, callback):
        self.scheduled.append((self.now_tick + max(0, int(ticks)), callback))

    def song(self):
        return self._song_obj

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
            due = [c for (t, c) in self.scheduled if t <= self.now_tick]
            self.scheduled = collections.deque((t, c) for (t, c) in self.scheduled if t > self.now_tick)
            for cb in due:
                cb()
                ran += 1
        return ran


class FakeSong(object):
    def __init__(self):
        self.is_playing = False
        self.tempo = 120.0
        self.current_song_time = 0.0
        self.signature_numerator = 4
        self.signature_denominator = 4
        self.tracks = []
        self.scenes = []
        self.return_tracks = []
        self.master_track = types.SimpleNamespace(name="Master")
        self.clip_trigger_quantization = 4

    def begin_undo_step(self):
        pass

    def end_undo_step(self):
        pass

    def get_current_beats_song_time(self):
        bpb = self.signature_numerator
        return types.SimpleNamespace(bars=int(self.current_song_time // bpb) + 1,
                                     beats=int(self.current_song_time % bpb) + 1)


class FakeApplication(object):
    def get_major_version(self):
        return 12

    def get_minor_version(self):
        return 4

    def get_bugfix_version(self):
        return 6


def load(bind=False):
    """The script's namespace with Live's modules stubbed. `bind=False` keeps
    the constructor from opening a real socket: the class is returned and the
    test builds an instance with `instance(ns)`."""
    framework = types.ModuleType("_Framework")
    surface = types.ModuleType("_Framework.ControlSurface")
    surface.ControlSurface = FakeSurface
    framework.ControlSurface = surface
    sys.modules["_Framework"] = framework
    sys.modules["_Framework.ControlSurface"] = surface
    live = types.ModuleType("Live")
    live.Song = types.SimpleNamespace(
        Quantization=types.SimpleNamespace(q_bar=1, q_quarter=2, q_no_q=0))
    sys.modules["Live"] = live
    ns = {"__name__": "ableton_remote_script_under_test", "__file__": SCRIPT}
    with io.open(SCRIPT, encoding="utf-8") as handle:
        source = handle.read()
    exec(compile(source, SCRIPT, "exec"), ns)
    return ns


def instance(ns, open_socket=False):
    """An AbletonMCP with the socket server replaced unless asked for."""
    cls = ns["AbletonMCP"]
    if not open_socket:
        cls.start_server = lambda self: setattr(self, "running", True)
    obj = cls(None)
    return obj
