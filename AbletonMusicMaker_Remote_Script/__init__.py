# AbletonMusicMaker/__init__.py -- the loader: the half of the script Live holds.
"""Live imports this file once, when the control surface is selected, and calls
`create_instance` once. That module object, that function and the instance it
returns live as long as Live does, and nothing here can change without Live
being restarted. So this file is kept to the parts that genuinely cannot move
-- the socket, the accept thread, the client list, the framing, the tick, the
executor that puts work on Live's main thread, and the one command the loader
answers itself -- and every handler lives in `body.py` beside it.

`body.py` defines `Body(Loader)` and carries `SCRIPT_VERSION`. The loader
imports it by path at startup and the instance *is* a `Body`, so method
resolution costs exactly what it did when this was one file: one class, one
lookup, nothing forwarded. `reload_body` re-reads that file and reassigns
`instance.__class__`. The instance keeps its identity, so the listening
socket, the accepted clients, their subscriptions, the pending cues, the
performance mode, the scene tables and the meter peaks are the same objects
a microsecond after the swap as before it. A fix reaches Live in the time it
takes to rebuild, with the transport running and the set unsaved.

What the loader cannot do is reload itself: Live holds it. When *this* file
changes, Live has to be restarted, and the server says so in those words
rather than the other ones. `LOADER_VERSION` is what it compares to tell the
two cases apart, and the line count of this file is a ratchet in
`tests/local_only.rs` -- every statement here is one that needs a restart.

No path and no source ever crosses the socket. `reload_body` takes no
arguments at all; it re-reads one fixed filename beside this one, which only
someone with local write access could have put there (decisions 0010 and
0011). It is refused from anything but loopback.

Live 10 runs Python 2.7 and Live 11 and 12 run Python 3: no f-strings, no
type hints, no third-party imports, in this file and in the body.
"""
from __future__ import absolute_import, print_function, unicode_literals

from _Framework.ControlSurface import ControlSurface
import os
import errno
import re
import socket
import sys
import json
import threading
import time
import traceback
import types

# Importing one file by its path: importlib on Live 11 and 12, `imp` on Live
# 10's Python 2.7. This is the only import by path in either file, and the
# path is one this file computes -- never one that arrived over the socket.
try:
    import importlib.util as _importlib_util
except ImportError:  # Live 10's Python 2.7
    _importlib_util = None
    import imp as _imp
else:
    _imp = None

# Change queue import for Python 2
try:
    import Queue as queue  # Python 2
except ImportError:
    import queue  # Python 3


# Constants for socket communication
DEFAULT_PORT = 9877
# Loopback: only programs on this Mac can drive Live through this socket, and
# the socket has no authentication. A file named bind_host.txt beside this
# script overrides it with its first non-comment line -- needed only when the
# MCP server runs on another machine, or in a container that cannot reach the
# host's loopback. Live must be restarted after changing it.
DEFAULT_HOST = "127.0.0.1"
BIND_FILE_NAME = "bind_host.txt"


def _configured_host():
    """The address to bind. Anything missing, unreadable or empty falls back
    to loopback: a typo must never open the port to the network."""
    try:
        here = os.path.dirname(os.path.abspath(__file__))
        path = os.path.join(here, BIND_FILE_NAME)
        if not os.path.isfile(path):
            return DEFAULT_HOST
        handle = open(path, "r")
        try:
            lines = handle.read().splitlines()
        finally:
            handle.close()
        for line in lines:
            value = line.strip()
            if value and not value.startswith("#"):
                return value
    except Exception:
        pass
    return DEFAULT_HOST


HOST = _configured_host()

# Bumped whenever this file changes -- which is the one change that still
# needs Live restarted. `body.py` carries SCRIPT_VERSION and declares the
# oldest loader it will run on in NEEDS_LOADER; the server compares both.
LOADER_VERSION = "1.1.0"
PROTOCOL_VERSION = 2
# The body, beside this file. Never a path from a request: `reload_body`
# carries no arguments, and this name is the only file it can ever read.
BODY_FILE_NAME = "body.py"
# Where the reload may be asked from. Widening the bind with bind_host.txt
# (decision 0003) must not hand a remote caller the reload.
LOOPBACK_HOSTS = ("127.0.0.1", "localhost", "::1")
# Where client sockets are read. "main_thread_tick": sockets are non-blocking
# and drained from the same tick the clock runs on, so a message waits one
# tick, not one turn of Live's thread scheduler. "background_thread": a
# Python thread per client, scheduled by Live on its own terms (measured
# 2026-09-20: about 200 ms per message whatever the command does). A file
# named socket_reader.txt beside this script holding the word "thread"
# selects the old reader, so the two can be compared on the same Live.
READER_FILE_NAME = "socket_reader.txt"


def _configured_reader():
    try:
        here = os.path.dirname(os.path.abspath(__file__))
        path = os.path.join(here, READER_FILE_NAME)
        if os.path.isfile(path):
            handle = open(path, "r")
            try:
                text = handle.read()
            finally:
                handle.close()
            for line in text.splitlines():
                value = line.strip().lower()
                if value and not value.startswith("#"):
                    return "background_thread" if value == "thread" else "main_thread_tick"
    except Exception:
        pass
    return "main_thread_tick"


SOCKET_READER = _configured_reader()



def _split_documents(buf):
    """Every complete JSON document at the front of `buf` (bytes), and what is
    left. Documents may be newline-delimited or simply concatenated; a
    partial one stays in the remainder until the rest arrives."""
    try:
        text = buf.decode("utf-8")
    except AttributeError:
        text = buf
    except UnicodeDecodeError:
        # A multi-byte character split across reads: wait for the rest.
        return [], buf
    decoder = json.JSONDecoder()
    docs = []
    pos = 0
    n = len(text)
    while True:
        while pos < n and text[pos] in " \t\r\n":
            pos += 1
        if pos >= n:
            return docs, b""
        try:
            doc, end = decoder.raw_decode(text, pos)
        except ValueError:
            rest = text[pos:]
            try:
                return docs, rest.encode("utf-8")
            except AttributeError:
                return docs, rest
        docs.append(doc)
        pos = end

# A handler returns this when it will answer the socket itself, from a later
# tick. Needed wherever Live applies the first step asynchronously (moving
# the playhead) and the second step must see the result.
DEFERRED = object()


class Done(object):
    """The last value a sliced handler yields: its result."""
    __slots__ = ("result",)

    def __init__(self, result):
        self.result = result


class Tick(object):
    """Yield this to give Live the frame back for real.

    A bare `yield None` is a *permission* to slice, and the executor takes it
    only when the slice budget is spent -- a yield that costs nothing is
    resumed on the very next line, inside the same tick. That is right for the
    budget and wrong for a handler that has to wait: Live applies some writes
    on its own tick rather than when the property is assigned (moving the
    playhead is the one the script depends on), so a read or a call that must
    see the result has to be on a later tick, whatever the budget says.

    `yield Tick(n)` re-schedules the task n ticks later, always.
    """
    __slots__ = ("ticks",)
    MAX = 32

    def __init__(self, ticks=1):
        self.ticks = max(1, min(Tick.MAX, int(ticks)))

# A socket round trip costs about 200 ms on Live 12.4.6 whatever the command
# does (measured: an unknown command, a tiny read and get_context all take
# the same 200 ms, with or without this log line, with TCP_NODELAY on both
# ends). That is Live scheduling the script's socket thread, not this code,
# which is why the server prefers one-round-trip commands such as
# get_context. Set this to False to keep routine commands out of Log.txt.
LOG_EVERY_COMMAND = False

# Which commands are served, read back from a dispatch's own source. Nothing
# is hand-maintained on this side: the names come out of the file that
# answers them, so a handler that exists is declared and a name with no
# handler cannot be. The loader answers four commands before the body is
# consulted (the handshake, the two subscription verbs and the reload); the
# body's own `SCRIPT_CAPABILITIES` is read the same way, out of `body.py`,
# by this same function. The server's ALL_REMOTE_COMMANDS (src/tools.rs) is
# the source of truth for what may be asked for, and a Rust test compares it
# with both files' dispatches, so the two drifting apart fails the build
# rather than a producer's session.
# What a body says it needs, matched in its source *before* it is executed:
# a body written for a newer loader is missing whatever that loader hands
# over, so executing it first fails at its own import check ("Tick is
# missing") instead of saying the one thing the producer can act on.
_NEEDS_LOADER_RE = re.compile(r'^NEEDS_LOADER\s*=\s*u?[\'"]([0-9.]+)[\'"]', re.M)
_COMMAND_EQ = re.compile(r'command_type\s*==\s*u?[\'"]([a-z_]+)[\'"]')
_COMMAND_IN = re.compile(r'command_type\s+in\s+\(([^)]*)\)')
_NAME_IN_GROUP = re.compile(r'[\'"]([a-z_]+)[\'"]')


def _served_commands(path):
    """The commands the dispatch in `path` answers, read from that file.
    Called at import time, by this file for itself and by the body for
    itself. An unreadable file gives an empty list rather than an exception:
    the script must still load inside Live if a path moved."""
    try:
        path = os.path.abspath(path)
        if path[-4:-1] == ".py" and path[-1:] in ("c", "o"):
            path = path[:-1]
        handle = open(path, "r")
        try:
            source = handle.read()
        finally:
            handle.close()
    except Exception:
        return []
    found = set(_COMMAND_EQ.findall(source))
    for group in _COMMAND_IN.findall(source):
        found.update(_NAME_IN_GROUP.findall(group))
    return sorted(found)


LOADER_CAPABILITIES = _served_commands(__file__)

# Everything `body.py` may reach for out of this file, and nothing else: the
# class it derives from, the two sentinels a sliced handler answers with, the
# logging switch, and the derivation above, which the body runs on its own
# source. `tests/remote_script/test_reload.py` pins the list.
BODY_EXPORTS = ("Loader", "Done", "Tick", "DEFERRED", "LOG_EVERY_COMMAND", "_served_commands")


def _body_path():
    """`body.py` beside this file, and nowhere else."""
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), BODY_FILE_NAME)


def _version_tuple(text):
    parts = []
    for piece in ("%s" % (text or "0")).split("."):
        digits = ""
        for ch in piece:
            if ch.isdigit():
                digits += ch
            else:
                break
        parts.append(int(digits or 0))
    while len(parts) < 3:
        parts.append(0)
    return tuple(parts[:3])


# One generation per import, so a body is never loaded under a name Python
# already has a module for -- and so a rewrite inside one filesystem
# timestamp second is still picked up.
_BODY_GENERATION = [0]


def _load_body_module():
    """Read and compile `body.py` into a fresh module, and return it.

    Off Live's main thread: reading and compiling a file touches nothing in
    Live, so it has no business on the thread that must answer within 8 ms
    while music plays. The caller does the one assignment that has to be on
    that thread.

    This is the only function in either file that opens a file after import
    time and the only one that imports by path; `tests/local_only.rs` and
    `tests/remote_script/test_generic.py` allow it here by name and nowhere
    else. Bytecode writing is off for the duration, so a body is never read
    back from a `.pyc` whose mtime granularity hid a fast rewrite.
    """
    path = _body_path()
    if not os.path.isfile(path):
        raise RuntimeError("%s is not beside the loader (looked in %s)" % (
            BODY_FILE_NAME, os.path.dirname(path)))
    handle = open(path, "r")
    try:
        needs = _NEEDS_LOADER_RE.search(handle.read())
    finally:
        handle.close()
    if needs and _version_tuple(needs.group(1)) > _version_tuple(LOADER_VERSION):
        raise RuntimeError(
            "that body needs loader v%s and this loader is v%s: restart Live, "
            "or re-select AbletonMusicMaker under Settings -> Link, Tempo & MIDI" % (
                needs.group(1), LOADER_VERSION))
    _BODY_GENERATION[0] += 1
    name = "AbletonMusicMaker_body_%d" % _BODY_GENERATION[0]
    saved = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        module = types.ModuleType(str(name))
        # What the body is handed. It imports nothing from the loader by
        # name -- these are in its namespace before its first line runs,
        # which is how `class Body(Loader)` can be its first statement and
        # how a body loaded any other way fails at import rather than at the
        # first request. The list is short on purpose: a body that needs
        # something else has to say so here, in the file that costs a
        # restart.
        for key in BODY_EXPORTS:
            module.__dict__[key] = globals()[key]
        module.__dict__["__file__"] = path
        module.__dict__["__name__"] = str(name)
        sys.modules[str(name)] = module
        try:
            if _importlib_util is not None:
                spec = _importlib_util.spec_from_file_location(str(name), path)
                spec.loader.exec_module(module)
            else:  # Live 10's Python 2.7
                _imp.load_source(str(name), path)
                module = sys.modules[str(name)]
        finally:
            # Kept out of sys.modules so an old generation is collected once
            # nothing references it; what keeps a body alive is the class the
            # instance wears and the generators still running on the old one.
            sys.modules.pop(str(name), None)
    finally:
        sys.dont_write_bytecode = saved
    return module


def _body_class(module):
    """The `Body` this module defines, checked before anything is swapped."""
    cls = getattr(module, "Body", None)
    if cls is None:
        raise RuntimeError("%s defines no Body class" % BODY_FILE_NAME)
    if not (isinstance(cls, type) and issubclass(cls, Loader)):
        raise RuntimeError("%s defines a Body that does not derive from the loader" % BODY_FILE_NAME)
    needs = getattr(module, "NEEDS_LOADER", "0.0.0")
    if _version_tuple(needs) > _version_tuple(LOADER_VERSION):
        raise RuntimeError(
            "that body needs loader v%s and this loader is v%s: run "
            "ableton-music-maker-install-script, then restart Live" % (needs, LOADER_VERSION))
    return cls


# The module the instance being built is about to wear. The constructor opens
# the socket, so the body has to be attached *before* `__init__` runs rather
# than after it returns: a client that connects in between would otherwise be
# told there is no body.
_PENDING_BODY = []


def create_instance(c_instance):
    """Live calls this once. It builds the one instance, wearing the body.

    A body that will not load is not fatal: the loader alone still listens,
    still answers `get_script_info` -- which is how the server finds out --
    and still serves `reload_body`, so a broken body can be replaced without
    Live being restarted. That is the whole point of the split.
    """
    module = None
    cls = Loader
    failure = None
    try:
        module = _load_body_module()
        cls = _body_class(module)
    except Exception:
        module = None
        failure = traceback.format_exc()
    _PENDING_BODY[:] = [module]
    try:
        instance = cls(c_instance)
    finally:
        del _PENDING_BODY[:]
    instance._body_module = module
    if failure is not None:
        instance.log_message("AbletonMCP: the body did not load, serving the loader alone")
        instance.log_message(failure)
        instance.show_message("AbletonMCP: body.py did not load - reinstall the Remote Script")
    else:
        instance.log_message("AbletonMCP: body v%s on loader v%s" % (
            instance._body_version(), LOADER_VERSION))
    return instance


class _TickSampler(object):
    """How often Live calls a schedule_message(1, ...) callback: the period
    of the main-thread tick, which nothing else in this product measures.

    Every callback notes the time; the intervals go into a ring. stats() is
    the median period, the jitter (p95 minus p50) and the worst interval
    over the ring, in milliseconds. Read from any thread; written from the
    main thread only."""

    def __init__(self, capacity=600):
        self._capacity = max(2, int(capacity))
        self._intervals = []
        self._last = None
        self._lock = threading.Lock()

    def note(self, now):
        """Called from the tick. `now` is time.time()."""
        with self._lock:
            if self._last is not None:
                self._intervals.append((now - self._last) * 1000.0)
                if len(self._intervals) > self._capacity:
                    del self._intervals[0:len(self._intervals) - self._capacity]
            self._last = now

    def stats(self):
        with self._lock:
            xs = sorted(self._intervals)
        n = len(xs)
        if n == 0:
            return {"period_ms": None, "jitter_ms": None, "max_ms": None, "samples": 0}
        p50 = xs[int(0.5 * (n - 1))]
        p95 = xs[int(0.95 * (n - 1))]
        return {"period_ms": round(p50, 2), "jitter_ms": round(p95 - p50, 2),
                "max_ms": round(xs[-1], 2), "samples": n}



class _Client(object):
    """One socket, read and written on the main-thread tick.

    Requests carrying an `id` may overlap and are answered whenever their
    handler finishes; requests without one are answered in order, one at a
    time, which is how a server from before ids sees exactly the behaviour
    it always had. Writes are buffered and bounded: a client that stops
    reading is dropped, never allowed to stall the tick."""

    MAX_OUTBOUND = 4 * 1024 * 1024
    MAX_STARTS_PER_TICK = 8

    def __init__(self, sock, address):
        self.sock = sock
        self.address = address
        self.inbuf = b""
        self.outbuf = b""
        self.closed = False
        self.queue = []          # parsed requests not started yet
        self.busy = False        # an id-less request is in flight
        self.inflight = {}       # token -> (deadline, sink, command_type)
        self.subscriptions = {}  # channel -> options (phase 2)
        self.next_token = 1


class _ClientSink(object):
    """Where a handler's answer goes when the request came in on the tick:
    the client's outbound buffer, with the request's id attached. Stands in
    for the queue the background-thread reader waits on."""

    def __init__(self, script, client, request_id, token):
        self.script = script
        self.client = client
        self.request_id = request_id
        self.token = token
        self.done = False

    def put(self, payload):
        if self.done:
            return  # answered already (a timeout got there first)
        self.done = True
        self.client.inflight.pop(self.token, None)
        if self.request_id is None:
            self.client.busy = False
        self.script._client_write(self.client, self.request_id, payload)

class Loader(ControlSurface):
    """The frozen half: the socket, the accept thread, the framing, the tick,
    the executor, and the one command answered before the body is consulted.

    The body wears this class as its base, so `self` is one object with one
    class and nothing is forwarded. A bare Loader -- what Live gets when
    `body.py` will not load -- still listens, still answers the handshake and
    still serves `reload_body`; everything else is refused with the reason.
    """

    # The body's, overridden there. Named here so a bare loader has them.
    COMMAND_TIMEOUTS = {}
    MUTATING_COMMANDS = frozenset()
    # The two methods a request passes through before the body sees it. A
    # body that defined either could make itself unreplaceable, so a test
    # fails the build when one appears in `Body.__dict__`.
    RESERVED_METHODS = ("_client_start", "_process_command")
    # The module `body.py` was loaded into; None when only the loader runs.
    _body_module = None

    def __init__(self, c_instance):
        """Initialize the control surface"""
        # Before anything else, and before the socket exists: which body this
        # instance is wearing. `create_instance` put it there.
        self._body_module = _PENDING_BODY[0] if _PENDING_BODY else None
        ControlSurface.__init__(self, c_instance)
        self.log_message(
            "AbletonMCP Remote Script initializing... (loader v%s)" % LOADER_VERSION)

        # Socket server for communication
        self.server = None
        self.client_threads = []
        # Clients read on the tick (SOCKET_READER == "main_thread_tick").
        # Appended by the accept thread, drained by the main thread.
        self._clients = []
        self._clients_lock = threading.Lock()
        self.server_thread = None
        self.running = False

        # The clock tick: armed for the life of the script, it samples the
        # tick period, caches what Live says its version is, drains the
        # client sockets, runs the body's channels and applies a reload whose
        # import has finished.
        self._tick_sampler = _TickSampler(600)
        self._clock_tick_armed = False
        self._live_version = None
        self._started_at = time.time()
        # The reload in flight: one at a time, imported on a worker thread
        # and applied on Live's thread from the next tick.
        self._reload = None
        self._reload_count = 0
        self._reload_last_ms = None

        # Everything the body keeps. On the first load every one of them is
        # missing and all are set; on a reload only what this body added is.
        self._apply_body_defaults()

        # Start the socket server
        self.start_server()
        self._arm_clock_tick()

        self.log_message("AbletonMCP initialized")

        # Show a message in Ableton
        self.show_message("AbletonMCP: Listening for commands on port " + str(DEFAULT_PORT))

    def disconnect(self):
        """Called when Ableton closes or the control surface is removed"""
        self.log_message("AbletonMCP disconnecting...")
        self.running = False

        teardown = getattr(self, "_body_teardown", None)
        if teardown is not None:
            try:
                teardown()
            except Exception as e:
                self.log_message("Body teardown error: " + str(e))

        # Stop the server
        if self.server:
            try:
                self.server.close()
            except Exception:
                pass

        with self._clients_lock:
            for c in self._clients:
                try:
                    c.sock.close()
                except Exception:
                    pass
            self._clients = []

        # Wait for the server thread to exit
        if self.server_thread and self.server_thread.is_alive():
            self.server_thread.join(1.0)

        # Clean up any client threads
        for client_thread in self.client_threads[:]:
            if client_thread.is_alive():
                # We don't join them as they might be stuck
                self.log_message("Client thread still alive during disconnect")

        ControlSurface.disconnect(self)
        self.log_message("AbletonMCP disconnected")

    def start_server(self):
        """Start the socket server in a separate thread"""
        try:
            self.server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self.server.bind((HOST, DEFAULT_PORT))
            self.server.listen(5)  # Allow up to 5 pending connections
            
            self.running = True
            self.server_thread = threading.Thread(target=self._server_thread)
            self.server_thread.daemon = True
            self.server_thread.start()
            
            self.log_message("Server started on " + str(HOST) + ":" + str(DEFAULT_PORT))
        except Exception as e:
            self.log_message("Error starting server: " + str(e))
            self.show_message("AbletonMCP: Error starting server - " + str(e))
    
    def _server_thread(self):
        """Server thread implementation - handles client connections"""
        try:
            self.log_message("Server thread started")
            # Set a timeout to allow regular checking of running flag
            self.server.settimeout(1.0)
            
            while self.running:
                try:
                    # Accept connections with timeout
                    client, address = self.server.accept()
                    self.log_message("Connection accepted from " + str(address))
                    self.show_message("AbletonMCP: Client connected")

                    if SOCKET_READER == "main_thread_tick":
                        try:
                            client.setblocking(False)
                        except Exception:
                            pass
                        with self._clients_lock:
                            self._clients.append(_Client(client, address))
                        continue

                    # Handle client in a separate thread
                    client_thread = threading.Thread(
                        target=self._handle_client,
                        args=(client,)
                    )
                    client_thread.daemon = True
                    client_thread.start()
                    
                    # Keep track of client threads
                    self.client_threads.append(client_thread)
                    
                    # Clean up finished client threads
                    self.client_threads = [t for t in self.client_threads if t.is_alive()]
                    
                except socket.timeout:
                    # No connection yet, just continue
                    continue
                except Exception as e:
                    if self.running:  # Only log if still running
                        self.log_message("Server accept error: " + str(e))
                    time.sleep(0.5)
            
            self.log_message("Server thread stopped")
        except Exception as e:
            self.log_message("Server thread error: " + str(e))
    
    # ── The executor: Live's main thread, in slices ──────────────────────
    #
    # Live's Python API is not thread-safe. The socket thread only parses
    # JSON and waits; everything that touches Live runs inside one task on
    # Live's main thread (schedule_message). A handler with a lot to do is a
    # generator: it yields between units of work and the executor resumes it
    # on the next tick once the slice budget is spent, so no task holds the
    # main thread longer than SLICE_MS_PLAYING while music plays. A mutating
    # command is one undo step per slice. Every reply carries main_ms (time
    # inside the task, all slices) and slices, so the server's activity log
    # shows what each command cost Live.

    SLICE_MS_PLAYING = 8.0
    SLICE_MS_STOPPED = 40.0
    SLOW_SLICE_MS = 25.0

    def _slice_budget(self):
        return self.SLICE_MS_PLAYING if self._safe_song_property("is_playing", bool, False) else self.SLICE_MS_STOPPED

    def _answer(self, response_queue, payload):
        """A handler that answers from a later tick reports through here, on
        the main thread, so the clock stamp is read where Live is."""
        if getattr(self, "_performance_mode", False) and "clock" not in payload:
            try:
                payload["clock"] = self._clock()
            except Exception as e:
                self.log_message("clock error: " + str(e))
        response_queue.put(payload)

    def _process_command(self, command, peer=None):
        """Process a command from the client and return a response.

        Reserved: a body never sees this method, and a body that defined one
        would be refused by the build (RESERVED_METHODS). This is the
        background-thread reader's way in; the tick reader's is
        `_client_start`, and both classify a request the same way.
        """
        command_type = command.get("type", "")
        params = command.get("params", {}) or {}
        if command_type == "get_script_info":
            # The handshake touches nothing in Live and answers even while
            # the main thread is busy.
            return {"status": "success", "result": self._get_script_info()}
        if command_type == "reload_body":
            answer = queue.Queue()
            self._begin_reload(answer, peer, params)
            try:
                return answer.get(timeout=14.0)
            except queue.Empty:
                return {"status": "error", "message": self._still_running("the reload did not finish")}
        return self._run_on_main(command_type, params)

    def _run_on_main(self, command_type, params, sink=None):
        """Run a command on Live's main thread. Without `sink` (the
        background-thread reader) this blocks the calling thread until the
        answer arrives. With one (the tick reader, already on the main
        thread) the first slice runs now, later slices on later ticks, and
        the answer is written to the sink; nothing waits."""
        if self._body_module is None:
            payload = {"status": "error", "main_ms": 0.0, "slices": 0,
                       "message": "the Remote Script's body did not load, so `%s` cannot run: "
                                  "run ableton-music-maker-install-script, then reload" % command_type}
            if sink is not None:
                sink.put(payload)
                return None
            return payload
        response_queue = sink if sink is not None else queue.Queue()
        acc = {"gen": None, "ms": 0.0, "slices": 0}
        mutates = command_type in self.MUTATING_COMMANDS

        def finish(payload):
            payload["main_ms"] = round(acc["ms"], 2)
            payload["slices"] = acc["slices"]
            self._answer(response_queue, payload)

        def task():
            t0 = time.time()
            undo = False
            acc["slices"] += 1
            try:
                if mutates:
                    try:
                        self._song.begin_undo_step()
                        undo = True
                    except Exception:
                        undo = False
                if acc["gen"] is None:
                    result = self._dispatch(command_type, params, response_queue)
                    if isinstance(result, types.GeneratorType):
                        acc["gen"] = result
                    elif result is DEFERRED:
                        return  # the handler answers from a later tick itself
                    else:
                        acc["ms"] += (time.time() - t0) * 1000.0
                        finish({"status": "success", "result": result})
                        return
                budget = self._slice_budget()
                while True:
                    try:
                        # `yielded`, not `item`: a name the Live API surface
                        # scanner reads as a browser item (live-api-surface.py).
                        yielded = next(acc["gen"])
                    except StopIteration:
                        acc["ms"] += (time.time() - t0) * 1000.0
                        finish({"status": "success", "result": {}})
                        return
                    if isinstance(yielded, Done):
                        acc["ms"] += (time.time() - t0) * 1000.0
                        finish({"status": "success", "result": yielded.result})
                        return
                    if isinstance(yielded, Tick):
                        # Not a slice for the budget's sake: a wait. The task
                        # resumes on a later tick however little it has spent,
                        # which is the only way a handler can see a write Live
                        # applies on its own frame.
                        acc["ms"] += (time.time() - t0) * 1000.0
                        self.schedule_message(yielded.ticks, task)
                        return
                    if (time.time() - t0) * 1000.0 >= budget:
                        acc["ms"] += (time.time() - t0) * 1000.0
                        self.schedule_message(1, task)
                        return
            except Exception as e:
                acc["ms"] += (time.time() - t0) * 1000.0
                self.log_message("Error in %s: %s" % (command_type, str(e)))
                self.log_message(traceback.format_exc())
                finish({"status": "error", "message": str(e)})
            finally:
                if undo:
                    try:
                        self._song.end_undo_step()
                    except Exception:
                        pass
                held = (time.time() - t0) * 1000.0
                if held > self.SLOW_SLICE_MS:
                    self.log_message("slow slice: %s held Live's main thread %.0f ms (slice %d)" % (
                        command_type, held, acc["slices"]))

        if sink is not None:
            task()
            return None
        try:
            self.schedule_message(0, task)
        except AssertionError:
            task()
        timeout = self.COMMAND_TIMEOUTS.get(command_type, 10.0)
        try:
            payload = response_queue.get(timeout=timeout)
        except queue.Empty:
            return {"status": "error", "message": "Timeout waiting for %s to complete" % command_type}
        if "main_ms" not in payload:
            payload["main_ms"] = round(acc["ms"], 2)
            payload["slices"] = acc["slices"]
        return payload

    def _get_script_info(self):
        """The handshake, answered by the loader.

        It reports both halves -- `script_version` from the body,
        `loader_version` from this file -- because they are updated in
        different ways: a body behind is a reload, a loader behind is the one
        remaining restart. It answers even when the body did not load at all,
        which is how the server finds out that it should write one.
        """
        body = self._body_module
        return {
            "name": "AbletonMCP",
            "script_version": self._body_version(),
            "loader_version": LOADER_VERSION,
            "protocol_version": PROTOCOL_VERSION,
            "port": DEFAULT_PORT,
            "bind_host": HOST,
            "bind_is_loopback": HOST in LOOPBACK_HOSTS,
            "capabilities": self._capabilities(),
            "snapshot_schema": "ableton_mcp_snapshot_v2",
            "passive_listeners": True,
            "performance_mode": bool(getattr(self, "_performance_mode", False)),
            "socket_reader": SOCKET_READER,
            "tick": self._tick_stats(),
            "live": {"version": self._live_version, "python": sys.version.split()[0]},
            "reload": {
                "supported": True,
                "body_loaded": body is not None,
                "body_file": _body_path(),
                "count": self._reload_count,
                "last_ms": self._reload_last_ms,
            },
        }

    def _body_version(self):
        """What the loaded body calls itself, or None when none loaded."""
        return getattr(self._body_module, "SCRIPT_VERSION", None)

    def _capabilities(self):
        """What this script serves: the loader's four commands and the
        body's, each derived from the file that answers them."""
        body = list(getattr(self._body_module, "SCRIPT_CAPABILITIES", []) or [])
        return sorted(set(body) | set(LOADER_CAPABILITIES))

    def _apply_body_defaults(self, defaults=None):
        """Give the body the state it keeps, without touching what is already
        there. On the first load every attribute is missing and all of them
        are set; on a reload only the ones this body added are, which is how
        a body that needs a new attribute gets one without losing the old
        ones. Returns the names it had to add, which the reload reports."""
        if defaults is None:
            factory = getattr(self, "_body_defaults", None)
            if factory is None:
                return []
            defaults = factory()
        added = []
        for name, value in defaults.items():
            if not hasattr(self, name):
                setattr(self, name, value)
                added.append(name)
        return sorted(added)

    def _tick_stats(self):
        st = self._tick_sampler.stats()
        st["playing"] = self._safe_song_property("is_playing", bool, None)
        return st
    
    def _safe_song_property(self, attr, cast, default):
        """Read self._song.<attr> with cast, returning default on common failures.
        Catches only narrow exceptions so genuine bugs still surface."""
        try:
            return cast(getattr(self._song, attr))
        except (AttributeError, TypeError, ValueError):
            return default


    @property
    def _song(self):
        """Always the current set: a cached handle went stale on set reload."""
        return self.song()

    # ── the clock ────────────────────────────────────────────────────────────

    def _arm_clock_tick(self):
        if self._clock_tick_armed or not self.running:
            return
        self._clock_tick_armed = True
        try:
            self.schedule_message(1, self._clock_tick)
        except Exception as e:
            self._clock_tick_armed = False
            self.log_message("could not arm the clock tick: " + str(e))


    def _clock_tick(self):
        """One callback per tick, for the life of the script. Records the
        interval and re-arms; the cost is a timestamp. Live's version is read
        here once, on the main thread, for the handshake to report, and a
        reload whose import has finished is applied here -- the one place the
        swap can happen, because it is the one place that is Live's thread and
        is not inside a handler."""
        self._clock_tick_armed = False
        try:
            self._tick_sampler.note(time.time())
            if self._live_version is None:
                self._live_version = self._read_live_version()
            if SOCKET_READER == "main_thread_tick":
                self._drain_clients()
            if self._reload is not None:
                self._reload_finish()
            events = getattr(self, "_event_tick", None)
            if events is not None:
                events()
            # The channels write after the drain flushed, so flush again:
            # an event must not wait a tick behind the reply that triggered it.
            if SOCKET_READER == "main_thread_tick":
                self._flush_clients()
        except Exception as e:
            self.log_message("clock tick error: " + str(e))
        if self.running:
            self._arm_clock_tick()


    # ── Clients on the tick ─────────────────────────────────────────────────

    def _drain_clients(self):
        with self._clients_lock:
            clients = list(self._clients)
        now = time.time()
        for c in clients:
            if c.closed:
                continue
            self._client_read(c)
            self._client_start(c, now)
            self._client_expire(c, now)
            self._client_flush(c)
        with self._clients_lock:
            self._clients = [c for c in self._clients if not c.closed]

    def _flush_clients(self):
        with self._clients_lock:
            clients = list(self._clients)
        for c in clients:
            if not c.closed:
                self._client_flush(c)
        with self._clients_lock:
            self._clients = [c for c in self._clients if not c.closed]

    def _client_close(self, c, why):
        if c.closed:
            return
        c.closed = True
        try:
            c.sock.close()
        except Exception:
            pass
        self.log_message("Client %s closed: %s" % (str(c.address), why))

    def _client_read(self, c):
        while True:
            try:
                data = c.sock.recv(65536)
            except socket.error as e:
                code = e.args[0] if e.args else None
                if code in (errno.EAGAIN, errno.EWOULDBLOCK):
                    break
                self._client_close(c, "read error: " + str(e))
                return
            except Exception as e:
                self._client_close(c, "read error: " + str(e))
                return
            if not data:
                self._client_close(c, "disconnected")
                return
            c.inbuf += data
        if c.inbuf:
            docs, rest = _split_documents(c.inbuf)
            c.inbuf = rest
            for d in docs:
                if isinstance(d, dict):
                    c.queue.append(d)
                else:
                    self._client_write(c, None, {"status": "error", "message": "a request must be a JSON object"})

    def _client_start(self, c, now):
        started = 0
        while c.queue and started < c.MAX_STARTS_PER_TICK:
            if c.busy:
                break
            req = c.queue.pop(0)
            rid = req.get("id")
            command_type = req.get("type", "")
            params = req.get("params", {}) or {}
            if LOG_EVERY_COMMAND:
                self.log_message("Received command: " + str(command_type))
            if command_type == "get_script_info":
                self._client_write(c, rid, {"status": "success", "result": self._get_script_info()})
                continue
            if command_type == "reload_body":
                # Answered by the loader, before the body is consulted: a
                # body whose dispatch raises on every request can still be
                # replaced. The answer comes from a later tick.
                self._begin_reload(_ClientSink(self, c, rid, c.next_token),
                                   c.address[0] if c.address else None, params)
                c.next_token += 1
                continue
            if command_type in ("subscribe", "unsubscribe"):
                # Per-socket bookkeeping: it touches nothing in Live, so it
                # never goes near the executor.
                try:
                    handler = self._subscribe if command_type == "subscribe" else self._unsubscribe
                    self._client_write(c, rid, {"status": "success", "result": handler(c, params)})
                except Exception as e:
                    self._client_write(c, rid, {"status": "error", "message": str(e)})
                continue
            token = c.next_token
            c.next_token += 1
            sink = _ClientSink(self, c, rid, token)
            if rid is None:
                c.busy = True
            deadline = now + self.COMMAND_TIMEOUTS.get(command_type, 10.0)
            c.inflight[token] = (deadline, sink, command_type)
            started += 1
            try:
                self._run_on_main(command_type, params, sink=sink)
            except Exception as e:
                sink.put({"status": "error", "message": str(e)})

    def _client_expire(self, c, now):
        for token, (deadline, sink, command_type) in list(c.inflight.items()):
            if now > deadline and not sink.done:
                sink.put({"status": "error",
                          "message": "Timeout waiting for %s to complete" % command_type})


    def _client_write(self, c, request_id, payload):
        if c.closed:
            return
        if request_id is not None:
            payload = dict(payload)
            payload["id"] = request_id
        line = json.dumps(payload) + "\n"
        try:
            line = line.encode("utf-8")
        except AttributeError:
            pass
        c.outbuf += line
        if len(c.outbuf) > c.MAX_OUTBOUND:
            self._client_close(c, "not reading: %d bytes waiting" % len(c.outbuf))

    def _client_flush(self, c):
        while c.outbuf and not c.closed:
            try:
                sent = c.sock.send(c.outbuf)
            except socket.error as e:
                code = e.args[0] if e.args else None
                if code in (errno.EAGAIN, errno.EWOULDBLOCK):
                    return
                self._client_close(c, "write error: " + str(e))
                return
            except Exception as e:
                self._client_close(c, "write error: " + str(e))
                return
            if sent <= 0:
                return
            c.outbuf = c.outbuf[sent:]

    def _read_live_version(self):
        try:
            app = self.application()
            parts = [app.get_major_version(), app.get_minor_version(), app.get_bugfix_version()]
            return ".".join(str(int(x)) for x in parts)
        except Exception:
            return "unknown"


    # ── the reload ──────────────────────────────────────────────────────────
    #
    # `reload_body` is the one command the loader answers itself, before the
    # body is consulted, so a body whose dispatch raises on every request can
    # still be replaced. It carries no arguments at all: no path and no source
    # crosses the socket, before or after this story (decisions 0010, 0011).
    # The file read and the compile happen on a worker thread; Live's own
    # thread does one assignment and nothing else.

    def _still_running(self, reason):
        """A refusal names what went wrong *and* what is still serving, which
        is the half a producer actually needs."""
        version = self._body_version()
        running = ("body v%s" % version) if version else "no body"
        return "%s. Still running %s on loader v%s." % (reason, running, LOADER_VERSION)

    def _refuse_reload(self, sink, reason):
        sink.put({"status": "error", "main_ms": 0.0, "slices": 0,
                  "message": self._still_running(reason)})

    def _begin_reload(self, sink, peer, params):
        """Start a reload. `sink` is anything with `put(payload)`: the tick
        reader's client sink, or a queue for the background-thread reader."""
        if params:
            self._refuse_reload(sink, "reload_body takes no parameters: it re-reads %s "
                                      "beside the loader and nothing else" % BODY_FILE_NAME)
            return
        if peer is not None and peer not in LOOPBACK_HOSTS:
            self._refuse_reload(sink, "reload_body is refused from %s: the reload is "
                                      "loopback only, whatever bind_host.txt says" % peer)
            return
        if self._reload is not None:
            self._refuse_reload(sink, "a reload is already running")
            return
        state = {"sink": sink, "started": time.time(), "module": None, "cls": None,
                 "defaults": None, "error": None, "traceback": None,
                 "done": False, "import_ms": None}
        self._reload = state
        worker = threading.Thread(target=self._reload_worker, args=(state,))
        worker.daemon = True
        worker.start()

    def _reload_worker(self, state):
        """Off Live's main thread: the file, the compile, and every check that
        can be made before anything changes. Nothing here touches Live and
        nothing here changes the running script, so a failure is a message."""
        try:
            module = _load_body_module()
            cls = _body_class(module)
            state["defaults"] = cls._body_defaults()
            state["cls"] = cls
            state["module"] = module
        except Exception as e:
            state["error"] = "%s: %s" % (type(e).__name__, e)
            state["traceback"] = traceback.format_exc()
        state["import_ms"] = round((time.time() - state["started"]) * 1000.0, 2)
        state["done"] = True

    def _reload_state(self):
        """What a reload must not lose, counted. Read before the swap and
        again after it, so the reply reports what is still there rather than
        only that nothing raised."""
        with self._clients_lock:
            live = [c for c in self._clients if not c.closed]
            clients = len(live)
            subscriptions = sum(len(c.subscriptions) for c in live)
        return {
            "clients": clients,
            "subscriptions": subscriptions,
            "cues": len(getattr(self, "_cues", []) or []),
            "performance_mode": bool(getattr(self, "_performance_mode", False)),
            "scene_phrases": len(getattr(self, "_scene_phrase", {}) or {}),
            "bar_peaks": getattr(self, "_bar_peaks", None) is not None,
            "mix_snapshots": len(getattr(self, "_mix_snapshots", {}) or {}),
            "attributes": len(self.__dict__),
        }

    def _reload_finish(self):
        """On Live's main thread, from the tick: the swap, and nothing else.

        Every way this can fail has already happened on the worker with the
        running body untouched, so there is no state in which the script is
        half replaced. Work already in flight -- a sliced generator, an armed
        schedule_message closure -- finishes against the old body, which
        Python keeps alive while it is referenced; both read and write the
        same attributes on this same object, so there is no second copy of
        the state to diverge.
        """
        state = self._reload
        if state is None or not state["done"]:
            return
        self._reload = None
        sink = state["sink"]
        if state["error"] is not None:
            self.log_message("reload refused: " + state["error"])
            self.log_message(state["traceback"] or "")
            sink.put({"status": "error", "main_ms": 0.0, "slices": 0,
                      "message": self._still_running(state["error"]),
                      "traceback": state["traceback"]})
            return
        before = self._reload_state()
        was = self._body_version()
        t0 = time.time()
        try:
            self.__class__ = state["cls"]
            self._body_module = state["module"]
            added = self._apply_body_defaults(state["defaults"])
        except Exception as e:
            main_ms = round((time.time() - t0) * 1000.0, 2)
            self.log_message("reload failed during the swap: " + str(e))
            sink.put({"status": "error", "main_ms": main_ms, "slices": 1,
                      "message": self._still_running("%s: %s" % (type(e).__name__, e)),
                      "traceback": traceback.format_exc()})
            return
        main_ms = round((time.time() - t0) * 1000.0, 2)
        kept = self._reload_state()
        kept["unchanged"] = all(kept[k] == before[k] for k in before if k != "attributes")
        self._reload_count += 1
        self._reload_last_ms = main_ms
        self.log_message("reload: body v%s to v%s in %s ms on the main thread" % (
            was, self._body_version(), main_ms))
        sink.put({"status": "success", "main_ms": main_ms, "slices": 1, "result": {
            "was": was,
            "now": self._body_version(),
            "loader_version": LOADER_VERSION,
            "import_ms": state["import_ms"],
            "main_ms": main_ms,
            "count": self._reload_count,
            "generation": _BODY_GENERATION[0],
            "attributes_added": added,
            "kept": kept,
        }})
