# AbletonMCP/init.py
from __future__ import absolute_import, print_function, unicode_literals

from _Framework.ControlSurface import ControlSurface
import os
import errno
import math
import re
import socket
import sys
import json
import threading
import time
import traceback
import types

# Live's own module: quantization constants for fixed-length recording.
try:
    import Live
except ImportError:  # outside Live (tests, tooling) the module does not exist
    Live = None

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

# Bumped whenever the TCP command surface changes; the MCP server compares
# this to EXPECTED_REMOTE_SCRIPT_VERSION.
SCRIPT_VERSION = "1.33.1"
PROTOCOL_VERSION = 2
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

# A socket round trip costs about 200 ms on Live 12.4.6 whatever the command
# does (measured: an unknown command, a tiny read and get_context all take
# the same 200 ms, with or without this log line, with TCP_NODELAY on both
# ends). That is Live scheduling the script's socket thread, not this code,
# which is why the server prefers one-round-trip commands such as
# get_context. Set this to False to keep routine commands out of Log.txt.
LOG_EVERY_COMMAND = False

# Which commands this script serves. Nothing here is hand-maintained: the
# names are read back from this file's own dispatch, so a handler that exists
# is declared and a name with no handler cannot be. The server's
# ALL_REMOTE_COMMANDS (src/tools.rs) is the source of truth for what may be
# asked for; a Rust test compares it with the dispatch of the script the
# binary embeds, so the two drifting apart fails the build rather than a
# producer's session. If this file cannot be read back, the list is empty and
# the server falls back to its own — which it may do, because it shipped this
# script and checks its version.
_COMMAND_EQ = re.compile(r'command_type\s*==\s*u?[\'"]([a-z_]+)[\'"]')
_COMMAND_IN = re.compile(r'command_type\s+in\s+\(([^)]*)\)')
_NAME_IN_GROUP = re.compile(r'[\'"]([a-z_]+)[\'"]')


def _served_commands():
    """The commands the dispatch answers, read from this file."""
    try:
        path = os.path.abspath(__file__)
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


SCRIPT_CAPABILITIES = _served_commands()

def create_instance(c_instance):
    """Create and return the AbletonMCP script instance"""
    return AbletonMCP(c_instance)

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


# ── The generic surface: paths into Live's object model ────────────────────
#
# `describe` and `run` reach Live's own objects and nothing else. The socket
# has no authentication and loopback is the boundary (decision 0003), so this
# must be exactly as dangerous as the fixed command list and not one step
# more: three roots, no underscore attributes (which covers every dunder, so
# no __class__, __globals__ or __builtins__ to climb out through), no import,
# no eval, no file. Every refusal below is a test.

PATH_ROOTS = ("song", "application", "browser")
_PATH_MAX_STEPS = 24


def _parse_path(path):
    """[('attr', name) | ('index', n), ...] with the root first. Raises
    ValueError with the reason a path is refused."""
    text = ("%s" % (path or "")).strip()
    if not text:
        raise ValueError("a path is required")
    steps = []
    i = 0
    n = len(text)
    name = ""
    while i < n:
        ch = text[i]
        if ch == ".":
            if not name:
                raise ValueError("empty name in %r" % text)
            steps.append(("attr", name))
            name = ""
            i += 1
        elif ch == "[":
            if name:
                steps.append(("attr", name))
                name = ""
            close = text.find("]", i)
            if close < 0:
                raise ValueError("unclosed [ in %r" % text)
            digits = text[i + 1:close].strip()
            try:
                steps.append(("index", int(digits)))
            except ValueError:
                raise ValueError("index must be a whole number, got %r" % digits)
            i = close + 1
            if i < n and text[i] == ".":
                i += 1
        else:
            name += ch
            i += 1
    if name:
        steps.append(("attr", name))
    if not steps or steps[0][0] != "attr":
        raise ValueError("a path starts with a name, not an index")
    if steps[0][1] not in PATH_ROOTS:
        raise ValueError("path must start with %s" % ", ".join(PATH_ROOTS))
    if len(steps) > _PATH_MAX_STEPS:
        raise ValueError("a path is at most %d steps" % _PATH_MAX_STEPS)
    for kind, value in steps:
        if kind == "attr" and value.startswith("_"):
            raise ValueError("%r is not reachable: names starting with _ are refused" % value)
    return steps


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


class AbletonMCP(ControlSurface):
    """AbletonMCP Remote Script for Ableton Live"""
    
    def __init__(self, c_instance):
        """Initialize the control surface"""
        ControlSurface.__init__(self, c_instance)
        self.log_message(
            "AbletonMCP Remote Script initializing... (script v%s)"
            % SCRIPT_VERSION
        )
        
        # Socket server for communication
        self.server = None
        self.client_threads = []
        # Clients read on the tick (SOCKET_READER == "main_thread_tick").
        # Appended by the accept thread, drained by the main thread.
        self._clients = []
        self._clients_lock = threading.Lock()
        self.server_thread = None
        self.running = False
        
        # The Song handle is not cached: it goes stale when a set is reloaded
        # (see the _song property below).

        # Passive human-UI event queue (drained by MCP → Supabase)
        self._passive_events = []
        self._passive_lock = threading.Lock()
        self._passive_max = 500
        self._passive_track_count = None
        self._passive_track_bindings = []  # (track, [(add_name, callback), ...]) for cleanup
        self._song_passive_callbacks = []

        # The performance clock: pending cues, their events, one pending
        # Session recording, and whether a tick is armed.
        self._cues = []
        self._cue_next_id = 1
        self._cue_lock = threading.Lock()
        self._perf_events = []
        self._perf_tick_armed = False
        # The clock tick: armed for the life of the script, it only samples
        # the tick period and caches what Live says its version is, so the
        # handshake can report both without touching Live from its thread.
        self._tick_sampler = _TickSampler(600)
        self._clock_tick_armed = False
        self._live_version = None
        self._started_at = time.time()
        # The clock channel runs off a schedule, not off the last send: see
        # _clock_event_tick. 0.0 means "not scheduled yet".
        self._next_clock_event = 0.0
        self._clock_event_every = None
        self._clock_event_sent_stopped = False
        self._levels_event_bar = None
        self._watch_prev = None
        self._changes_ticks = 0
        self._pending_record = None
        # Performance mode: while on, every response carries a clock and the
        # tick watches which scene row plays (phrases count from there).
        self._performance_mode = False
        # True while this script armed Live's Arrangement Record for a take.
        self._arr_recording = False
        self._scene_phrase = {}     # scene index -> bars per phrase
        self._scene_started = {}    # scene index -> bar it last started on
        self._current_scene = None
        # Levels: the meter peaks of the current bar, the last completed bar,
        # and the loudest the master got while each scene row played.
        self._bar_peaks = None
        self._last_bar_peaks = None
        self._section_peaks = {}
        self._mix_snapshots = {}
        self._snapshot_next_id = 1
        self._library_key_cache = None
        # The browser walk is a generator shared by every client: two clients
        # paging at once ("generator already executing") must take turns.
        self._browser_lock = threading.Lock()
        
        # Start the socket server
        self.start_server()
        self._arm_clock_tick()

        # Passive LOM listeners (a dataset-tier leftover) are no longer
        # registered: attaching listeners to a clip while it records, from
        # inside a Live notification, deadlocked Live during the first capture.
        
        self.log_message("AbletonMCP initialized")
        
        # Show a message in Ableton
        self.show_message("AbletonMCP: Listening for commands on port " + str(DEFAULT_PORT))
    
    def disconnect(self):
        """Called when Ableton closes or the control surface is removed"""
        self.log_message("AbletonMCP disconnecting...")
        self.running = False

        try:
            self._teardown_passive_listeners()
        except Exception as e:
            self.log_message("Passive listener teardown error: " + str(e))
        
        # Stop the server
        if self.server:
            try:
                self.server.close()
            except:
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
    
    def _handle_client(self, client):
        """Handle communication with a connected client"""
        self.log_message("Client handler started")
        client.settimeout(None)  # No timeout for client socket
        buffer = ''  # Changed from b'' to '' for Python 2
        
        try:
            while self.running:
                try:
                    # Receive data
                    data = client.recv(8192)
                    
                    if not data:
                        # Client disconnected
                        self.log_message("Client disconnected")
                        break
                    
                    # Accumulate data in buffer with explicit encoding/decoding
                    try:
                        # Python 3: data is bytes, decode to string
                        buffer += data.decode('utf-8')
                    except AttributeError:
                        # Python 2: data is already string
                        buffer += data
                    
                    try:
                        # Try to parse command from buffer
                        command = json.loads(buffer)  # Removed decode('utf-8')
                        buffer = ''  # Clear buffer after successful parse
                        
                        if LOG_EVERY_COMMAND:
                            self.log_message("Received command: " + str(command.get("type", "unknown")))
                        
                        # Process the command and get response
                        response = self._process_command(command)
                        
                        # Send the response with explicit encoding
                        try:
                            # Python 3: encode string to bytes
                            client.sendall(json.dumps(response).encode('utf-8'))
                        except AttributeError:
                            # Python 2: string is already bytes
                            client.sendall(json.dumps(response))
                    except ValueError:
                        # Incomplete data, wait for more
                        continue
                        
                except Exception as e:
                    self.log_message("Error handling client data: " + str(e))
                    self.log_message(traceback.format_exc())
                    
                    # Send error response if possible
                    error_response = {
                        "status": "error",
                        "message": str(e)
                    }
                    try:
                        # Python 3: encode string to bytes
                        client.sendall(json.dumps(error_response).encode('utf-8'))
                    except AttributeError:
                        # Python 2: string is already bytes
                        client.sendall(json.dumps(error_response))
                    except:
                        # If we can't send the error, the connection is probably dead
                        break
                    
                    # For serious errors, break the loop
                    if not isinstance(e, ValueError):
                        break
        except Exception as e:
            self.log_message("Error in client handler: " + str(e))
        finally:
            try:
                client.close()
            except:
                pass
            self.log_message("Client handler stopped")
    
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
    COMMAND_TIMEOUTS = {"create_audio_clip": 60.0, "create_tracks": 180.0, "write_clips": 60.0,
                        "search_browser": 20.0, "get_browser_index": 20.0, "get_library_status": 20.0,
                        "place_sample": 60.0}
    MUTATING_COMMANDS = frozenset([
        "create_midi_track", "create_audio_track", "set_track_name", "create_clip",
        "create_audio_clip", "add_notes_to_clip", "set_clip_name", "set_arrangement_clip_name",
        "delete_clip", "clear_notes_from_clip", "set_tempo", "fire_clip", "stop_clip",
        "start_playback", "stop_playback", "load_browser_item", "load_instrument_or_effect",
        "switch_to_arrangement_view", "set_current_song_time", "duplicate_session_clip_to_arrangement",
        "create_locator", "set_track_mixer", "set_send", "set_track_color",
        "set_clip_color", "delete_arrangement_clip", "delete_locator", "set_clip_loop",
        "set_clip_launch", "set_clip_automation", "ensure_capture_track", "start_capture",
        "stop_capture", "play_from", "delete_track", "back_to_arrangement", "set_arrangement_loop",
        "set_launch_quantization", "create_scene", "fire_scene", "stop_all_clips", "set_crossfader",
        "record_clip", "set_scale", "set_slot_stop_buttons", "set_scene", "start_live_capture",
        "restore_mix", "capture_scene", "duplicate_scene", "set_clip_groove", "set_device_parameter",
        "set_device_parameters", "place_clips", "delete_arrangement_clips",
        "duplicate_arrangement_clip", "create_return_track", "create_tracks", "write_clips",
        "start_arrangement_record", "stop_arrangement_record", "place_sample",
        "delete_device", "move_device",
    ])

    def _slice_budget(self):
        return self.SLICE_MS_PLAYING if self._safe_song_property("is_playing", bool, False) else self.SLICE_MS_STOPPED

    def _answer(self, response_queue, payload):
        """A handler that answers from a later tick reports through here, on
        the main thread, so the clock stamp is read where Live is."""
        if self._performance_mode and "clock" not in payload:
            try:
                payload["clock"] = self._clock()
            except Exception as e:
                self.log_message("clock error: " + str(e))
        response_queue.put(payload)

    def _process_command(self, command):
        """Process a command from the client and return a response"""
        command_type = command.get("type", "")
        params = command.get("params", {}) or {}
        if command_type == "get_script_info":
            # The handshake touches nothing in Live and answers even while
            # the main thread is busy.
            return {"status": "success", "result": self._get_script_info()}
        return self._run_on_main(command_type, params)

    def _run_on_main(self, command_type, params, sink=None):
        """Run a command on Live's main thread. Without `sink` (the
        background-thread reader) this blocks the calling thread until the
        answer arrives. With one (the tick reader, already on the main
        thread) the first slice runs now, later slices on later ticks, and
        the answer is written to the sink; nothing waits."""
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
                        item = next(acc["gen"])
                    except StopIteration:
                        acc["ms"] += (time.time() - t0) * 1000.0
                        finish({"status": "success", "result": {}})
                        return
                    if isinstance(item, Done):
                        acc["ms"] += (time.time() - t0) * 1000.0
                        finish({"status": "success", "result": item.result})
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

    def _dispatch(self, command_type, params, response_queue):
        """Every command, on the main thread: the result, a generator the
        executor slices, or DEFERRED when the handler answers later."""
        result = None
        if command_type == "get_session_info":
            return self._get_session_info()
        elif command_type == "get_track_info":
            return self._get_track_info(
                params.get("track_index", 0), params.get("kind", "track"))
        elif command_type == "create_midi_track":
            index = params.get("index", -1)
            result = self._create_midi_track(index)
        elif command_type == "create_audio_track":
            index = params.get("index", -1)
            result = self._create_audio_track(index)
        elif command_type == "set_track_name":
            track_index = params.get("track_index", 0)
            name = params.get("name", "")
            result = self._set_track_name(track_index, name, params.get("kind", "track"))
        elif command_type == "create_clip":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            length = params.get("length", 4.0)
            result = self._create_clip(track_index, clip_index, length)
        elif command_type == "place_sample":
            return self._place_sample(params)
        elif command_type == "list_sample_folders":
            return self._list_sample_folders(params.get("roots"))
        elif command_type == "create_audio_clip":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            path = params.get("path", "")
            result = self._create_audio_clip(track_index, clip_index, path)
        elif command_type == "add_notes_to_clip":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            notes = params.get("notes", [])
            result = self._add_notes_to_clip(track_index, clip_index, notes)
        elif command_type == "clear_notes_from_clip":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            result = self._clear_notes_from_clip(track_index, clip_index)
        elif command_type == "set_clip_name":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            name = params.get("name", "")
            result = self._set_clip_name(track_index, clip_index, name)
        elif command_type == "set_arrangement_clip_name":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            name = params.get("name", "")
            result = self._set_arrangement_clip_name(track_index, clip_index, name)
        elif command_type == "set_tempo":
            tempo = params.get("tempo", 120.0)
            result = self._set_tempo(tempo)
        elif command_type == "fire_clip":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            result = self._fire_clip(track_index, clip_index)
        elif command_type == "stop_clip":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            result = self._stop_clip(track_index, clip_index)
        elif command_type == "delete_clip":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            result = self._delete_clip(track_index, clip_index)
        elif command_type == "start_playback":
            result = self._start_playback()
        elif command_type == "stop_playback":
            result = self._stop_playback()
        elif command_type == "load_browser_item":
            track_index = params.get("track_index", 0)
            item_uri = params.get("item_uri", "")
            result = self._load_browser_item(track_index, item_uri, params.get("kind", "track"))
        # ── Arrangement view commands ──────────────────────────────
        elif command_type == "switch_to_arrangement_view":
            result = self._switch_to_arrangement_view()
        elif command_type == "set_current_song_time":
            time_val = params.get("time", 0.0)
            result = self._set_current_song_time(time_val)
        elif command_type == "duplicate_session_clip_to_arrangement":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            destination_time = params.get("destination_time", 0.0)
            result = self._duplicate_session_clip_to_arrangement(
                track_index, clip_index, destination_time)
        elif command_type == "create_locator":
            name = params.get("name", "")
            time_val = params.get("time", 0.0)
            result = self._create_locator(name, time_val, response_queue)
        elif command_type == "set_track_mixer":
            result = self._set_track_mixer(
                params.get("track_index", 0), params.get("kind", "track"),
                params.get("volume"), params.get("pan"),
                params.get("mute"), params.get("solo"), params.get("arm"),
                params.get("volume_db"))
        elif command_type == "set_send":
            result = self._set_send(
                params.get("track_index", 0), params.get("kind", "track"),
                params.get("send_index"), params.get("send_name"),
                params.get("value", 0.0))
        elif command_type == "set_track_color":
            result = self._set_track_color(
                params.get("track_index", 0), params.get("kind", "track"),
                params.get("color_index", 0))
        elif command_type == "set_clip_color":
            result = self._set_clip_color(
                params.get("track_index", 0), params.get("clip_index", 0),
                params.get("arrangement", False), params.get("color_index", 0))
        elif command_type == "delete_arrangement_clip":
            result = self._delete_arrangement_clip(
                params.get("track_index", 0), params.get("clip_index", 0))
        elif command_type == "delete_locator":
            result = self._delete_locator(
                params.get("name"), params.get("time"), response_queue)
        elif command_type == "set_clip_loop":
            result = self._set_clip_loop(
                params.get("track_index", 0), params.get("clip_index", 0),
                params.get("arrangement", False), params)
        elif command_type == "set_clip_launch":
            result = self._set_clip_launch(
                params.get("track_index", 0), params.get("clip_index", 0), params)
        elif command_type == "play_from":
            result = self._play_from(params.get("time", 0.0), response_queue)
        elif command_type == "delete_track":
            result = self._delete_track(params.get("track_index", -1))
        elif command_type == "back_to_arrangement":
            result = self._back_to_arrangement()
        elif command_type == "set_arrangement_loop":
            result = self._set_arrangement_loop(
                params.get("start"), params.get("length"), params.get("enabled"))
        elif command_type == "set_launch_quantization":
            result = self._set_launch_quantization(params.get("name", "1_bar"))
        elif command_type == "create_scene":
            result = self._create_scene(
                params.get("index", -1), params.get("name"), params.get("tempo"),
                params.get("phrase_bars"))
        elif command_type == "set_scene":
            result = self._set_scene(
                params.get("index", 0), params.get("name"), params.get("tempo"),
                params.get("phrase_bars"))
        elif command_type == "set_performance_mode":
            result = self._set_performance_mode(params.get("on", True))
        elif command_type == "capture_scene":
            result = self._capture_scene(
                params.get("name"), params.get("phrase_bars"), params.get("after"))
        elif command_type == "duplicate_scene":
            result = self._duplicate_scene(
                params.get("index", 0), params.get("name"), params.get("phrase_bars"))
        elif command_type == "place_clips":
            result = self._place_clips(
                params.get("track_index", 0), params.get("clip_index", 0),
                params.get("times", []))
        elif command_type == "delete_arrangement_clips":
            result = self._delete_arrangement_clips(
                params.get("track_index", 0), params.get("indices"),
                params.get("all", False), params.get("from_beat"), params.get("to_beat"))
        elif command_type == "arrangement_summary":
            result = self._arrangement_summary()
        elif command_type == "start_arrangement_record":
            result = self._start_arrangement_record(
                params.get("from_beat", 0.0), params.get("replace", False))
        elif command_type == "stop_arrangement_record":
            result = self._stop_arrangement_record(
                params.get("from_beat"), params.get("back_to_arrangement", True))
        elif command_type == "duplicate_arrangement_clip":
            result = self._duplicate_arrangement_clip(
                params.get("track_index", 0), params.get("clip_index", 0),
                params.get("times", []))
        elif command_type == "create_return_track":
            result = self._create_return_track(params.get("name"))
        elif command_type == "create_tracks":
            result = self._create_tracks(
                params.get("tracks", []), params.get("on_existing", "add"))
        elif command_type == "write_clips":
            result = self._write_clips(params.get("clips", []))
        elif command_type == "set_device_parameters":
            result = self._set_device_parameters(
                params.get("track_index", 0), params.get("device_index", 0),
                params.get("values", []), params.get("kind", "track"))
        elif command_type == "delete_device":
            result = self._delete_device(
                params.get("track_index", 0), params.get("device_index", 0),
                params.get("kind", "track"))
        elif command_type == "move_device":
            result = self._move_device(
                params.get("track_index", 0), params.get("device_index", 0),
                params.get("to_index", 0), params.get("kind", "track"))
        elif command_type == "set_clip_groove":
            result = self._set_clip_groove(
                params.get("track_index"), params.get("clip_index"),
                params.get("groove_index"), params.get("timing"),
                params.get("random"), params.get("velocity"),
                params.get("global_amount"))
        elif command_type == "start_live_capture":
            result = self._start_live_capture(params.get("bars", 1), params.get("name"))
        elif command_type == "snapshot_mix":
            result = self._snapshot_mix()
        elif command_type == "restore_mix":
            result = self._restore_mix(params.get("id"))
        elif command_type == "fire_scene":
            result = self._fire_scene(params.get("scene_index", 0))
        elif command_type == "stop_all_clips":
            result = self._stop_all_clips()
        elif command_type == "set_crossfader":
            result = self._set_crossfader(params.get("value"), params.get("assign"))
        elif command_type == "record_clip":
            result = self._record_clip(
                params.get("track_index", 0), params.get("bars", 4), params.get("name"))
        elif command_type == "schedule_cue":
            result = self._schedule_cue(params.get("cue", params))
        elif command_type == "cancel_cue":
            result = self._cancel_cue(params.get("id"), params.get("reason", "cancelled"))
        elif command_type == "set_scale":
            result = self._set_scale(params.get("root_note"), params.get("scale_name"))
        elif command_type == "set_slot_stop_buttons":
            result = self._set_slot_stop_buttons(
                params.get("track_index", 0), params.get("has_stop_button", True),
                params.get("slots"))
        elif command_type == "capture_status":
            result = self._capture_status(params.get("slot", 0))
        elif command_type == "get_track_meters":
            result = self._get_track_meters()
        elif command_type == "list_captures":
            result = self._list_captures()
        elif command_type == "ensure_capture_track":
            result = self._ensure_capture_track()
        elif command_type == "start_capture":
            result = self._start_capture(
                params.get("start", 0.0), params.get("bars", 8),
                params.get("name", "capture"), response_queue)
        elif command_type == "stop_capture":
            result = self._stop_capture(params.get("slot"))
        elif command_type == "set_clip_automation":
            result = self._set_clip_automation(
                params.get("track_index", 0), params.get("clip_index", 0),
                params.get("arrangement", False), params.get("target", {}),
                params.get("points", []), params.get("mode", "linear"),
                params.get("resolution", 0.25), params.get("clear", True))
        # Add the new browser commands
        elif command_type == "get_browser_tree":
            category_type = params.get("category_type", "all")
            return self.get_browser_tree(category_type)
        elif command_type == "get_browser_items_at_path":
            path = params.get("path", "")
            return self.get_browser_items_at_path(path)
        # Read-only arrangement command – no main-thread scheduling required
        elif command_type == "get_arrangement_clips":
            track_index = params.get("track_index", 0)
            return self._get_arrangement_clips(track_index)
        elif command_type == "get_returns":
            return self._get_returns()
        elif command_type == "search_browser":
            return self._search_browser(
                params.get("query", ""), params.get("category", "all"),
                params.get("limit", 30))
        elif command_type == "get_clip_info":
            return self._get_clip_info(
                params.get("track_index", 0), params.get("clip_index", 0),
                params.get("arrangement", False))
        elif command_type == "get_library_status":
            return self._get_library_status()
        elif command_type == "get_clip_automation":
            return self._get_clip_automation(
                params.get("track_index", 0), params.get("clip_index", 0),
                params.get("arrangement", False), params.get("target", {}),
                params.get("resolution", 1.0))
        elif command_type == "get_drum_rack_pads":
            return self._get_drum_rack_pads(
                params.get("track_index", 0), params.get("device_index", -1),
                params.get("kind", "track"))
        # Dataset / state-snapshot reads
        elif command_type == "get_clip_notes":
            track_index = params.get("track_index", 0)
            clip_index = params.get("clip_index", 0)
            return self._get_clip_notes(track_index, clip_index)
        elif command_type == "get_device_parameters":
            return self._get_device_parameters(
                params.get("track_index", 0), params.get("device_index", 0),
                params.get("kind", "track"))
        elif command_type == "describe":
            return self._describe(params.get("path"))
        elif command_type == "run":
            return self._run_ops(params)
        elif command_type == "get_meter_scale":
            return self._get_meter_scale()
        elif command_type == "get_session_snapshot":
            include_notes = params.get("include_notes", True)
            include_params = params.get("include_params", True)
            return self._get_session_snapshot(
                include_notes=include_notes,
                include_params=include_params,
            )
        elif command_type == "drain_passive_events":
            return self._drain_passive_events()
        elif command_type == "get_performance_state":
            return self._get_performance_state()
        elif command_type == "get_grooves":
            return self._get_grooves()
        elif command_type == "get_context":
            return self._get_context(bool(params.get("include_library", False)))
        elif command_type == "get_browser_index":
            return self._get_browser_index(
                params.get("category", "all"), params.get("offset", 0),
                params.get("limit", 500), params.get("budget_s", 1.0))
        elif command_type == "set_device_parameter":
            return self._set_device_parameter(
                params.get("track_index", 0),
                params.get("device_index", 0),
                params.get("parameter_index", 0),
                params.get("value"),
                params.get("kind", "track"),
                params.get("value_display"),
            )
        else:
            raise ValueError("Unknown command: " + command_type)
        return result

    
    # Command implementations

    def _get_script_info(self):
        """Handshake payload for MCP server version / capability checks."""
        return {
            "name": "AbletonMCP",
            "script_version": SCRIPT_VERSION,
            "protocol_version": PROTOCOL_VERSION,
            "port": DEFAULT_PORT,
            "bind_host": HOST,
            "bind_is_loopback": HOST in ("127.0.0.1", "localhost", "::1"),
            "capabilities": list(SCRIPT_CAPABILITIES),
            "snapshot_schema": "ableton_mcp_snapshot_v2",
            "passive_listeners": True,
            "performance_mode": bool(getattr(self, "_performance_mode", False)),
            "socket_reader": SOCKET_READER,
            "tick": self._tick_stats(),
            "live": {"version": self._live_version, "python": sys.version.split()[0]},
        }

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

    def _get_session_info(self):
        """Get information about the current session"""
        try:
            result = {
                "tempo": self._song.tempo,
                "signature_numerator": self._song.signature_numerator,
                "signature_denominator": self._song.signature_denominator,
                "track_count": len(self._song.tracks),
                "return_track_count": len(self._song.return_tracks),
                "master_track": {
                    "name": "Master",
                    "volume": self._song.master_track.mixer_device.volume.value,
                    "panning": self._song.master_track.mixer_device.panning.value
                },
                # Read via _safe_song_property so an attribute missing on a
                # given Live version falls back to its default.
                "is_playing":        self._safe_song_property("is_playing",        bool,  False),
                "current_song_time": self._safe_song_property("current_song_time", float, 0.0),
                "song_length":       self._safe_song_property("song_length",       float, 0.0),
                "loop":              self._safe_song_property("loop",              bool,  False),
                "loop_start":        self._safe_song_property("loop_start",        float, 0.0),
                "loop_length":       self._safe_song_property("loop_length",       float, 0.0),
                # True when the tracks are following Session clips, so the
                # Arrangement is not what you would hear.
                "back_to_arranger":  self._safe_song_property("back_to_arranger",  bool,  False),
            }
            return result
        except Exception as e:
            self.log_message("Error getting session info: " + str(e))
            raise
    
    def _get_track_info(self, track_index, kind="track"):
        """Get information about a track, a return track or the master"""
        try:
            track = self._resolve_track(track_index, kind)
            kind = str(kind or "track").lower()
            if kind not in ("track", "return", "master"):
                kind = "track"

            # Get clip slots (the returns and the master have none)
            clip_slots = []
            for slot_index, slot in enumerate(getattr(track, "clip_slots", [])):
                clip_info = None
                if slot.has_clip:
                    clip = slot.clip
                    clip_info = {
                        "name": clip.name,
                        "length": clip.length,
                        "is_playing": clip.is_playing,
                        "is_recording": clip.is_recording
                    }
                
                clip_slots.append({
                    "index": slot_index,
                    "has_clip": slot.has_clip,
                    "clip": clip_info
                })
            
            # Get devices
            devices = []
            for device_index, device in enumerate(track.devices):
                devices.append({
                    "index": device_index,
                    "name": device.name,
                    "class_name": device.class_name,
                    "type": self._get_device_type(device)
                })
            
            result = {
                "index": track_index if kind != "master" else 0,
                "kind": kind,
                "name": track.name,
                "is_audio_track": self._safe_attr(track, "has_audio_input", bool, False),
                "is_midi_track": self._safe_attr(track, "has_midi_input", bool, False),
                "mute": self._safe_attr(track, "mute", bool, False),
                "solo": self._safe_attr(track, "solo", bool, False),
                "arm": self._safe_arm(track),
                "volume": float(track.mixer_device.volume.value),
                "volume_db": self._volume_db(track.mixer_device.volume),
                "panning": float(track.mixer_device.panning.value),
                "clip_slots": clip_slots,
                "devices": devices
            }
            return result
        except Exception as e:
            self.log_message("Error getting track info: " + str(e))
            raise
    
    def _safe_arm(self, track):
        """Read track.arm, returning False for tracks that have no arm state.

        Live raises RuntimeError("Master and Return Tracks have no 'Arm'
        state!") for group tracks as well as return and main tracks. A
        `getattr(track, "arm", False)` does not guard this: the attribute
        exists, so getattr's default never applies -- reading it is what
        throws, and the error is a RuntimeError rather than an AttributeError.

        Check can_be_armed first so the common path does not rely on raising,
        and keep a narrow catch for Live versions that do not expose that
        property on every track type.
        """
        try:
            if not getattr(track, "can_be_armed", False):
                return False
            return bool(track.arm)
        except (AttributeError, RuntimeError):
            return False

    def _create_midi_track(self, index):
        """Create a new MIDI track at the specified index"""
        try:
            # Create the track
            self._song.create_midi_track(index)
            
            # Get the new track
            new_track_index = len(self._song.tracks) - 1 if index == -1 else index
            new_track = self._song.tracks[new_track_index]
            
            result = {
                "index": new_track_index,
                "name": new_track.name
            }
            return result
        except Exception as e:
            self.log_message("Error creating MIDI track: " + str(e))
            raise

    def _create_audio_track(self, index):
        """Create a new audio track at the specified index"""
        try:
            # Create the track
            self._song.create_audio_track(index)

            # Get the new track
            new_track_index = len(self._song.tracks) - 1 if index == -1 else index
            new_track = self._song.tracks[new_track_index]

            result = {
                "index": new_track_index,
                "name": new_track.name
            }
            return result
        except Exception as e:
            self.log_message("Error creating audio track: " + str(e))
            raise


    def _set_track_name(self, track_index, name, kind="track"):
        """Set the name of a track (or a return track with kind 'return')"""
        try:
            track = self._resolve_track(track_index, kind)
            track.name = name
            
            result = {
                "name": track.name
            }
            return result
        except Exception as e:
            self.log_message("Error setting track name: " + str(e))
            raise
    
    def _create_clip(self, track_index, clip_index, length):
        """Create a new MIDI clip in the specified track and clip slot"""
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")
            
            track = self._song.tracks[track_index]
            
            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")
            
            clip_slot = track.clip_slots[clip_index]
            
            # Check if the clip slot already has a clip
            if clip_slot.has_clip:
                raise Exception("Clip slot already has a clip")
            
            # Create the clip
            clip_slot.create_clip(length)
            
            result = {
                "name": clip_slot.clip.name,
                "length": clip_slot.clip.length
            }
            return result
        except Exception as e:
            self.log_message("Error creating clip: " + str(e))
            raise

    def _create_audio_clip(self, track_index, clip_index, path):
        """Create an audio clip in the specified audio track clip slot by importing a file.

        Requires Ableton Live 12.0.5 or newer (the underlying
        ClipSlot.create_audio_clip Live API was introduced in 12.0.5 — it is
        not available in earlier 12.0.x releases).
        """
        try:
            if not path:
                raise ValueError("Audio file path is required")

            if not os.path.isabs(path):
                raise ValueError("Audio file path must be absolute (got: %s)" % path)

            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")

            track = self._song.tracks[track_index]

            if getattr(track, "has_midi_input", False) or not getattr(track, "has_audio_input", True):
                raise ValueError("Track %d is not an audio track" % track_index)

            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")

            clip_slot = track.clip_slots[clip_index]

            if clip_slot.has_clip:
                raise Exception("Clip slot already has a clip")

            if not hasattr(clip_slot, "create_audio_clip"):
                raise Exception(
                    "ClipSlot.create_audio_clip is unavailable in this Ableton Live "
                    "version. Requires Live 12.0.5 or newer."
                )

            clip_slot.create_audio_clip(path)

            result = {
                "name": clip_slot.clip.name,
                "length": clip_slot.clip.length,
                "is_audio_clip": clip_slot.clip.is_audio_clip
            }
            return result
        except Exception as e:
            self.log_message("Error creating audio clip: " + str(e))
            raise

    # ── Samples ───────────────────────────────────────────────────────────────
    #
    # Finding: _list_sample_folders names the folders that hold samples -- the
    # Core Library inside the application, the Packs and the User Library
    # around this script, the open set's own folder, and Live's Places. Only
    # Live knows where those are; the server walks them itself, because file
    # I/O in Live's embedded interpreter runs about two files a second
    # (measured 2026-09-19), which no sample library survives.
    # Placing: _place_sample creates the clip and fits it inside one task, so
    # Live wraps the whole thing in a single undo step.

    # A sample this much of a bar or longer is treated as a loop; shorter is a
    # one-shot and is left exactly as Live loaded it.
    LOOP_BARS_FLOOR = 0.75
    # How far Live's own warped length may sit from a whole number of bars and
    # still be snapped to it.
    BAR_SNAP_TOLERANCE = 0.10

    def _user_library_dir(self):
        """Live's User Library: this script sits in
        <User Library>/Remote Scripts/<this folder>/__init__.py."""
        here = os.path.dirname(os.path.abspath(__file__))
        return os.path.dirname(os.path.dirname(here))

    def _core_library_samples_dir(self):
        """The Core Library ships inside the application, in a different place
        per platform; the first candidate that exists wins, and none is fine."""
        try:
            exe = os.path.abspath(sys.executable or "")
        except Exception:
            exe = ""
        if not exe:
            return None
        here = os.path.dirname(exe)
        contents = os.path.dirname(here)
        for path in (os.path.join(contents, "App-Resources", "Core Library", "Samples"),
                     os.path.join(here, "Resources", "Core Library", "Samples"),
                     os.path.join(contents, "Resources", "Core Library", "Samples")):
            try:
                if os.path.isdir(path):
                    return path
            except Exception:
                pass
        return None

    def _project_dir(self):
        """The folder of the open set, once it has been saved."""
        path = self._safe_song_property("file_path", str, "")
        if not path:
            return None
        try:
            folder = os.path.dirname(os.path.abspath(path))
            return folder if os.path.isdir(folder) else None
        except Exception:
            return None

    def _user_folder_path(self, item):
        """Where a Live browser Place is on disk, when Live says. Live does not
        document a path on a browser item, so every likely spelling is tried and
        only an existing directory is believed."""
        for attr in ("path", "file_path", "absolute_path", "folder", "directory"):
            value = self._safe_attr(item, attr, None, None)
            try:
                if value and os.path.isdir("%s" % value):
                    return "%s" % value
            except Exception:
                pass
        uri = self._safe_attr(item, "uri", None, None)
        if not uri:
            return None
        text = "%s" % uri
        if text.startswith("file://"):
            text = text[7:]
        else:
            cut = text.find(":/")
            text = text[cut + 1:] if cut >= 0 else text
        try:
            return text if text and os.path.isdir(text) else None
        except Exception:
            return None

    def _sample_folders(self, roots=None):
        """Every folder to look in: Live's own (the Core Library, the Packs
        beside the User Library, the User Library's Samples, this project) plus
        the ones the server passes. Deduplicated, only what exists."""
        out, seen = [], set()

        def add(name, path, source):
            if not path:
                return
            try:
                full = os.path.abspath("%s" % path)
                if not os.path.isdir(full):
                    return
                key = os.path.normcase(full)
            except Exception:
                return
            if key in seen:
                return
            seen.add(key)
            out.append({"name": name, "path": full, "source": source})

        unresolved = []
        core = self._core_library_samples_dir()
        add("Core Library", core, "core_library")
        try:
            library = self._user_library_dir()
        except Exception:
            library = None
        if library:
            add("Factory Packs", os.path.join(os.path.dirname(library), "Factory Packs"), "packs")
            add("User Library", os.path.join(library, "Samples"), "user_library")
        add("This project", self._project_dir(), "project")
        try:
            places = getattr(self.application().browser, "user_folders", None) or []
        except Exception:
            places = []
        for item in list(places):
            name = "%s" % (self._safe_attr(item, "name", None, "") or "")
            path = self._user_folder_path(item)
            if path:
                add(name or "Place", path, "places")
            elif name:
                unresolved.append(name)
        for path in list(roots or []):
            add(os.path.basename(("%s" % path).rstrip(os.sep)) or "%s" % path, path, "added")
        return out, unresolved

    def _list_sample_folders(self, roots=None):
        """The folders a scan would walk, and the Places Live would not locate."""
        folders, unresolved = self._sample_folders(roots)
        return {"folders": folders, "places_without_path": unresolved}

    def _arrangement_clip_at(self, track, at):
        """The Arrangement clip that starts at this beat, if Live put one there."""
        best, distance = None, None
        for clip in list(track.arrangement_clips):
            start = self._safe_attr(clip, "start_time", float, None)
            if start is None:
                continue
            gap = abs(start - at)
            if distance is None or gap < distance:
                best, distance = clip, gap
        if best is None or distance > 0.01:
            return None
        return best

    def _fit_sample_clip(self, clip, bars, fit, where):
        """Warp and loop an audio clip to whole bars, then report what Live
        ended up with. Live's own Auto-Warp does the tempo detection; this only
        decides whether the clip is a loop or a one-shot, sets the markers, and
        reads them back. It never rewrites warp markers, so it never stretches
        the material -- 'fitted' says which of those happened.

        Where it went decides whether it loops: a Session row plays through its
        section, so a loop belongs there; a point in the Arrangement is an event
        at that bar, so it is left as one hit unless `bars` asked otherwise."""
        beats_per_bar = float(self._safe_song_property("signature_numerator", int, 4) or 4)
        tempo = float(self._safe_song_property("tempo", float, 120.0) or 120.0)
        seconds_per_bar = beats_per_bar * 60.0 / max(1.0, tempo)
        frames = self._safe_attr(clip, "sample_length", float, None)
        rate = self._safe_attr(clip, "sample_rate", float, None)
        duration_s = (frames / rate) if (frames and rate) else None
        warping = bool(self._safe_attr(clip, "warping", bool, False))
        if warping:
            beats = self._safe_attr(clip, "length", float, None)
            heard = (beats / beats_per_bar) if beats else None
        else:
            heard = (duration_s / seconds_per_bar) if duration_s else None
        out = {"beats_per_bar": beats_per_bar, "duration_s": duration_s,
               "heard_bars": round(heard, 3) if heard is not None else None}
        target = None
        if not fit:
            out["fitted"] = "raw"
        elif bars is not None:
            target = float(bars)
            if target <= 0.0:
                raise ValueError("bars must be more than 0")
            out["fitted"] = "asked"
        elif where == "arrangement":
            # One hit at one bar: Live's own warp stands, nothing is looped.
            out["fitted"] = "at_bar"
        elif heard is None:
            out["fitted"] = "unmeasured"
        elif heard < self.LOOP_BARS_FLOOR:
            out["fitted"] = "one_shot"
        else:
            nearest = float(int(math.floor(heard + 0.5)))
            if nearest >= 1.0 and abs(heard - nearest) <= self.BAR_SNAP_TOLERANCE * nearest:
                target, out["fitted"] = nearest, "snapped"
            else:
                out["fitted"] = "off_grid"
        if target is not None:
            out["bars"] = target
            end = target * beats_per_bar
            # Order matters: unwarped audio cannot loop, and the markers are in
            # beats only once the clip is warped. Each step is best effort --
            # what Live accepted is read back below.
            for attr, value in (("warping", True), ("looping", True),
                                ("loop_start", 0.0), ("loop_end", end), ("end_marker", end)):
                try:
                    setattr(clip, attr, value)
                except Exception as e:
                    out.setdefault("refused", []).append("%s: %s" % (attr, str(e)))
        out["warping"] = bool(self._safe_attr(clip, "warping", bool, False))
        out["looping"] = bool(self._safe_attr(clip, "looping", bool, False))
        out["loop_end"] = self._safe_attr(clip, "loop_end", float, None)
        out["length"] = self._safe_attr(clip, "length", float, None)
        return out

    def _place_sample(self, spec):
        """A sample into the song: an audio clip in a Session slot or in the
        Arrangement at a beat position, warped and looped to whole bars,
        transposed and named -- all in one task, so Live undoes it in one step.

        The file is referenced where it lies, exactly as dragging it into Live
        would do; nothing is copied. Needs the Live version that has the
        create_audio_clip function for the target (Live 12).
        """
        spec = spec or {}
        track = self._resolve_track(spec.get("track_index", -1))
        if getattr(track, "has_midi_input", False) or not getattr(track, "has_audio_input", True):
            raise ValueError("'%s' is a MIDI track; a sample needs an audio track" % track.name)
        path = spec.get("path") or None
        item_uri = spec.get("item_uri") or None
        slot = spec.get("slot")
        position = spec.get("position")
        if not path and not item_uri:
            raise ValueError("give the sample's path, or a browser item_uri")
        if path and not os.path.isabs("%s" % path):
            raise ValueError("Audio file path must be absolute (got: %s)" % path)
        if (slot is None) == (position is None):
            raise ValueError("give a slot (a Session row) or a position in beats (the Arrangement), not both")
        clip, where = None, None
        if position is not None:
            at = float(position)
            if at < 0.0:
                raise ValueError("an Arrangement position cannot be before bar 1")
            if not path:
                raise ValueError(
                    "Live tells a client the file of a browser item only once it is a clip, "
                    "and the Arrangement needs the file: put the sample in a section first")
            if not hasattr(track, "create_audio_clip"):
                raise Exception(
                    "Track.create_audio_clip is unavailable in this Ableton Live version; "
                    "placing a sample in the Arrangement needs Live 12")
            track.create_audio_clip("%s" % path, at)
            clip, where = self._arrangement_clip_at(track, at), "arrangement"
        else:
            index = int(slot)
            slots = list(track.clip_slots)
            if index < 0 or index >= len(slots):
                raise IndexError("slot %d is outside '%s' (the set has %d rows)" % (
                    index, track.name, len(slots)))
            clip_slot = slots[index]
            if clip_slot.has_clip:
                raise ValueError("slot %d on '%s' already holds '%s'" % (
                    index, track.name, clip_slot.clip.name))
            if path:
                if not hasattr(clip_slot, "create_audio_clip"):
                    raise Exception(
                        "ClipSlot.create_audio_clip is unavailable in this Ableton Live "
                        "version. Requires Live 12.0.5 or newer.")
                clip_slot.create_audio_clip("%s" % path)
            else:
                browser = self.application().browser
                item = self._find_browser_item_by_uri(browser, "%s" % item_uri)
                if item is None:
                    raise ValueError("Browser item with URI '%s' not found" % item_uri)
                # Live loads a browser item into what is highlighted, so the
                # target slot is highlighted first.
                self._song.view.highlighted_clip_slot = clip_slot
                browser.load_item(item)
            if not clip_slot.has_clip:
                raise Exception("Live put no clip in slot %d of '%s'" % (index, track.name))
            clip, where = clip_slot.clip, "session"
        result = {"track": "%s" % track.name, "track_index": int(spec.get("track_index", -1)),
                  "where": where}
        if slot is not None:
            result["slot"] = int(slot)
        if clip is None:
            # Live made the clip (nothing raised) but did not hand it back at
            # the position asked for: say so rather than guess.
            result["name"] = os.path.splitext(os.path.basename("%s" % path))[0]
            result["read_back"] = False
            return result
        bars = spec.get("bars")
        result.update(self._fit_sample_clip(clip, None if bars is None else float(bars),
                                            spec.get("fit", True) is not False, where))
        transpose = spec.get("transpose")
        if transpose is not None:
            semitones = int(transpose)
            if semitones < -48 or semitones > 48:
                raise ValueError("transpose is in semitones, -48 to 48 (got %d)" % semitones)
            clip.pitch_coarse = semitones
            result["transpose"] = semitones
        name = spec.get("name")
        if name:
            clip.name = "%s" % name
        result["name"] = "%s" % clip.name
        result["file_path"] = self._safe_attr(clip, "file_path", str, None)
        result["read_back"] = True
        if where == "arrangement":
            result["start_time"] = self._safe_attr(clip, "start_time", float, None)
            result["end_time"] = self._safe_attr(clip, "end_time", float, None)
        return result

    def _add_notes_to_clip(self, track_index, clip_index, notes):
        """Add MIDI notes to a clip"""
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")
            
            track = self._song.tracks[track_index]
            
            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")
            
            clip_slot = track.clip_slots[clip_index]
            
            if not clip_slot.has_clip:
                raise Exception("No clip in slot")
            
            clip = clip_slot.clip
            
            # Convert note data to Live's format
            live_notes = []
            for note in notes:
                pitch = note.get("pitch", 60)
                start_time = note.get("start_time", 0.0)
                duration = note.get("duration", 0.25)
                velocity = note.get("velocity", 100)
                mute = note.get("mute", False)
                
                live_notes.append((pitch, start_time, duration, velocity, mute))
            
            # Add the notes
            clip.set_notes(tuple(live_notes))
            
            result = {
                "note_count": len(notes)
            }
            return result
        except Exception as e:
            self.log_message("Error adding notes to clip: " + str(e))
            raise
    
    def _set_clip_name(self, track_index, clip_index, name):
        """Set the name of a clip"""
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")
            
            track = self._song.tracks[track_index]
            
            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")
            
            clip_slot = track.clip_slots[clip_index]
            
            if not clip_slot.has_clip:
                raise Exception("No clip in slot")
            
            clip = clip_slot.clip
            clip.name = name
            
            result = {
                "name": clip.name
            }
            return result
        except Exception as e:
            self.log_message("Error setting clip name: " + str(e))
            raise

    def _set_arrangement_clip_name(self, track_index, clip_index, name):
        """Set the name of a clip placed in the Arrangement timeline.

        clip_index indexes into track.arrangement_clips, in the same order
        as returned by _get_arrangement_clips (i.e. ordered by start_time).
        """
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")

            track = self._song.tracks[track_index]
            arrangement_clips = list(track.arrangement_clips)

            if clip_index < 0 or clip_index >= len(arrangement_clips):
                raise IndexError("Clip index out of range")

            clip = arrangement_clips[clip_index]
            clip.name = name

            result = {
                "name": clip.name
            }
            return result
        except Exception as e:
            self.log_message("Error setting arrangement clip name: " + str(e))
            raise

    def _set_tempo(self, tempo):
        """Set the tempo of the session"""
        try:
            self._song.tempo = tempo
            
            result = {
                "tempo": self._song.tempo
            }
            return result
        except Exception as e:
            self.log_message("Error setting tempo: " + str(e))
            raise
    
    def _fire_clip(self, track_index, clip_index):
        """Fire a clip"""
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")
            
            track = self._song.tracks[track_index]
            
            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")
            
            clip_slot = track.clip_slots[clip_index]
            
            if not clip_slot.has_clip:
                raise Exception("No clip in slot")
            
            clip_slot.fire()

            result = {"fired": True}
            result.update(self._landing())
            return result
        except Exception as e:
            self.log_message("Error firing clip: " + str(e))
            raise
    
    def _stop_clip(self, track_index, clip_index):
        """Stop a clip"""
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")
            
            track = self._song.tracks[track_index]
            
            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")
            
            clip_slot = track.clip_slots[clip_index]
            
            clip_slot.stop()
            
            result = {
                "stopped": True
            }
            return result
        except Exception as e:
            self.log_message("Error stopping clip: " + str(e))
            raise

    def _delete_clip(self, track_index, clip_index):
        """Delete the clip in the given clip slot, freeing the slot for reuse."""
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")

            track = self._song.tracks[track_index]

            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")

            clip_slot = track.clip_slots[clip_index]

            if not clip_slot.has_clip:
                return {"deleted": False, "reason": "Clip slot was already empty"}

            clip_slot.delete_clip()

            return {"deleted": True}
        except Exception as e:
            self.log_message("Error deleting clip: " + str(e))
            raise


    def _start_playback(self):
        """Start playing the session"""
        try:
            self._song.start_playing()
            
            result = {
                "playing": self._song.is_playing
            }
            return result
        except Exception as e:
            self.log_message("Error starting playback: " + str(e))
            raise
    
    def _stop_playback(self):
        """Stop playing the session"""
        try:
            self._song.stop_playing()
            
            result = {
                "playing": self._song.is_playing
            }
            return result
        except Exception as e:
            self.log_message("Error stopping playback: " + str(e))
            raise
    
    # ── Arrangement view implementations ──────────────────────────────────────

    def _switch_to_arrangement_view(self):
        """Switch Ableton's main window to the Arrangement view"""
        try:
            self.application().view.show_view("Arranger")
            return {"view": "Arranger"}
        except Exception as e:
            self.log_message("Error switching to arrangement view: " + str(e))
            raise

    def _set_current_song_time(self, time_val):
        """Move the arrangement playhead to a position in beats"""
        try:
            # Live applies the move on its own tick, so reading the position
            # straight back returns where the playhead *was*. Report what was
            # requested as the new position and the old one beside it.
            previous = float(self._song.current_song_time)
            self._song.current_song_time = float(time_val)
            return {"current_song_time": float(time_val),
                    "previous_song_time": previous}
        except Exception as e:
            self.log_message("Error setting current song time: " + str(e))
            raise

    def _get_arrangement_clips(self, track_index):
        """Return all clips placed in the Arrangement timeline for a track.

        Each clip dict contains:
          name, start_time, end_time, length, color,
          is_midi_clip, is_audio_clip, is_playing
        """
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")

            track = self._song.tracks[track_index]
            clips = []

            # track.arrangement_clips is available in Live 11 / 12
            for clip in track.arrangement_clips:
                clips.append({
                    "name": clip.name,
                    "start_time": clip.start_time,
                    "end_time": clip.end_time,
                    "length": clip.length,
                    "color": clip.color,
                    "is_midi_clip": clip.is_midi_clip,
                    "is_audio_clip": clip.is_audio_clip,
                    "is_playing": clip.is_playing
                })

            return {
                "track_index": track_index,
                "track_name": track.name,
                "clip_count": len(clips),
                "clips": clips
            }
        except Exception as e:
            self.log_message("Error getting arrangement clips: " + str(e))
            raise

    def _clear_notes_from_clip(self, track_index, clip_index):
        """Remove all MIDI notes from a Session clip.

        Pairs with _add_notes_to_clip to make a real replace (clear, then add),
        which the write-only API otherwise can't do. Counts notes first so the
        result can report how many were removed.
        """
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")

            track = self._song.tracks[track_index]

            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")

            clip_slot = track.clip_slots[clip_index]

            if not clip_slot.has_clip:
                raise Exception("No clip in slot")

            clip = clip_slot.clip

            if not clip.is_midi_clip:
                raise Exception("Clip is not a MIDI clip; no notes to clear")

            length = clip.length

            # Count existing notes for the report (best-effort; never fatal).
            cleared = 0
            try:
                getter = getattr(clip, "get_notes_extended", None)
                if getter is not None:
                    cleared = len(list(getter(0, 128, 0.0, length)))
                else:
                    cleared = len(list(clip.get_notes(0.0, 0, length, 128)))
            except Exception:
                cleared = 0

            # Remove every note across the full pitch/time range. Prefer the
            # modern API (Live 11+); fall back to the legacy signature. Argument
            # order mirrors the get/remove _extended family:
            #   remove_notes_extended(from_pitch, pitch_span, from_time, time_span)
            # vs the legacy remove_notes(from_time, from_pitch, time_span, pitch_span).
            remover = getattr(clip, "remove_notes_extended", None)
            if remover is not None:
                remover(0, 128, 0.0, length)
            else:
                clip.remove_notes(0.0, 0, length, 128)

            return {
                "track_index": track_index,
                "clip_index": clip_index,
                "clip_name": clip.name,
                "cleared_count": cleared,
            }
        except Exception as e:
            self.log_message("Error clearing notes from clip: " + str(e))
            raise

    def _duplicate_session_clip_to_arrangement(self, track_index, clip_index, destination_time):
        """Copy a Session-view clip into the Arrangement timeline.

        Uses the real Live API:
          track.duplicate_clip_to_arrangement(clip, destination_time)

        Available in Live 11 / 12.  destination_time is in beats from the
        start of the arrangement.
        """
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")

            track = self._song.tracks[track_index]

            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip slot index out of range")

            clip_slot = track.clip_slots[clip_index]

            if not clip_slot.has_clip:
                raise Exception(
                    "No clip in slot " + str(clip_index) +
                    " on track " + str(track_index)
                )

            clip = clip_slot.clip

            # Duplicate to arrangement at the requested beat position
            track.duplicate_clip_to_arrangement(clip, float(destination_time))

            return {
                "success": True,
                "track_index": track_index,
                "track_name": track.name,
                "clip_name": clip.name,
                "destination_time": destination_time
            }
        except Exception as e:
            self.log_message("Error duplicating clip to arrangement: " + str(e))
            raise

    def _create_locator(self, name, time_val, response_queue=None):
        """Create (or rename) a named locator at the given beat position.

        Live's Song.set_or_delete_cue() toggles a cue at the current song
        time, and a playhead move is applied on Live's own tick, not when the
        property is assigned. So the toggle runs two ticks later, and the
        socket is answered from there. A cue already at that time is renamed
        instead of toggled (toggling would delete it).
        """
        try:
            song = self._song
            target_time = float(time_val)
            tolerance = 1e-3

            def cue_at(t):
                for cue in song.cue_points:
                    if abs(cue.time - t) < tolerance:
                        return cue
                return None

            def describe(cue):
                if name:
                    try:
                        cue.name = str(name)
                    except Exception as e:
                        self.log_message("Could not rename locator: " + str(e))
                return {"success": True, "time": float(cue.time), "name": str(cue.name)}

            existing = cue_at(target_time)
            if existing is not None:
                return describe(existing)

            original_time = float(song.current_song_time)
            song.current_song_time = target_time

            def finish():
                try:
                    if abs(float(song.current_song_time) - target_time) > tolerance:
                        # Still not there: give it one more tick.
                        song.current_song_time = target_time
                        self.schedule_message(2, finish_or_fail)
                        return
                    song.set_or_delete_cue()
                    cue = cue_at(target_time)
                    if cue is None:
                        raise Exception("Live did not create a cue at beat %s (playhead at %s)" % (
                            target_time, song.current_song_time))
                    result = describe(cue)
                    try:
                        song.current_song_time = original_time
                    except Exception:
                        pass
                    if response_queue is not None:
                        self._answer(response_queue, {"status": "success", "result": result})
                except Exception as e:
                    self.log_message("Error creating locator: " + str(e))
                    if response_queue is not None:
                        self._answer(response_queue, {"status": "error", "message": str(e)})

            def finish_or_fail():
                if abs(float(song.current_song_time) - target_time) > tolerance:
                    msg = "Could not move the playhead to beat %s to place the locator (it is at %s)" % (
                        target_time, song.current_song_time)
                    self.log_message(msg)
                    if response_queue is not None:
                        self._answer(response_queue, {"status": "error", "message": msg})
                    return
                finish()

            if response_queue is None:
                # No socket to answer (direct call): do it now and hope the
                # move has landed, as the old code did.
                finish()
                return {"success": True, "time": target_time, "name": name}
            self.schedule_message(2, finish)
            return DEFERRED
        except Exception as e:
            self.log_message("Error creating locator: " + str(e))
            raise

    # ── Mixer, colours, drum pads, arrangement deletion ──────────────────────

    NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

    def _live_note_name(self, pitch):
        """Live's own naming: C3 is MIDI 60, so the kick pad C1 is 36."""
        pitch = int(pitch)
        return "%s%d" % (self.NOTE_NAMES[pitch % 12], pitch // 12 - 2)

    def _resolve_track(self, track_index, kind="track"):
        """A track by index among the song's tracks, its return tracks, or the master."""
        kind = str(kind or "track").lower()
        if kind == "master":
            return self._song.master_track
        if kind == "return":
            tracks = list(self._song.return_tracks)
        else:
            kind = "track"
            tracks = list(self._song.tracks)
        track_index = int(track_index)
        if track_index < 0 or track_index >= len(tracks):
            raise IndexError("%s index %d out of range (0-%d)" % (kind, track_index, len(tracks) - 1))
        return tracks[track_index]

    def _volume_db(self, param):
        """Live's own reading of a volume fader ("-6.0 dB") as a number; -inf as -80."""
        try:
            text = "%s" % param.value_string
        except Exception:
            try:
                text = "%s" % param.str_for_value(param.value)
            except Exception:
                return None
        text = text.replace("dB", "").strip()
        if "inf" in text:
            return -80.0
        try:
            return float(text)
        except ValueError:
            return None

    def _value_for_db(self, param, db):
        """The fader value whose display reads `db`, by bisection over Live's own curve."""
        target = float(db)
        if target <= -70.0:
            return float(param.min)
        lo, hi = float(param.min), float(param.max)
        for _ in range(40):
            mid = (lo + hi) / 2.0
            try:
                text = ("%s" % param.str_for_value(mid)).replace("dB", "").strip()
                shown = -80.0 if "inf" in text else float(text)
            except Exception:
                return max(lo, min(hi, 0.85 + target / 60.0))
            if shown < target:
                lo = mid
            else:
                hi = mid
            if hi - lo < 1e-6:
                break
        return (lo + hi) / 2.0

    def _mixer_state(self, track):
        state = {
            "name": track.name,
            "volume": float(track.mixer_device.volume.value),
            "volume_db": self._volume_db(track.mixer_device.volume),
            "panning": float(track.mixer_device.panning.value),
            "sends": self._serialize_sends(track),
        }
        try:
            state["mute"] = bool(track.mute)
            state["solo"] = bool(track.solo)
        except Exception:
            pass
        state["arm"] = self._safe_arm(track)
        try:
            state["color_index"] = int(track.color_index)
        except Exception:
            pass
        return state

    def _set_track_mixer(self, track_index, kind, volume, pan, mute, solo, arm, volume_db=None):
        """Volume, pan, mute, solo and arm in one call; unspecified values stay."""
        try:
            track = self._resolve_track(track_index, kind)
            is_master = str(kind or "track").lower() == "master"
            if volume_db is not None:
                db = float(volume_db)
                if db > 6.0:
                    raise ValueError("a fader goes up to +6 dB")
                track.mixer_device.volume.value = self._value_for_db(track.mixer_device.volume, db)
            if volume is not None:
                v = float(volume)
                if v < 0.0 or v > 1.0:
                    raise ValueError("volume must be between 0.0 and 1.0 (Live's mixer parameter; 0.85 is 0 dB)")
                track.mixer_device.volume.value = v
            if pan is not None:
                p = float(pan)
                if p < -1.0 or p > 1.0:
                    raise ValueError("pan must be between -1.0 (left) and 1.0 (right)")
                track.mixer_device.panning.value = p
            if mute is not None:
                if is_master:
                    raise ValueError("the master track has no mute")
                track.mute = bool(mute)
            if solo is not None:
                if is_master:
                    raise ValueError("the master track has no solo")
                track.solo = bool(solo)
            if arm is not None:
                if not getattr(track, "can_be_armed", False):
                    raise ValueError("this track cannot be armed (group, return or master)")
                track.arm = bool(arm)
            return self._mixer_state(track)
        except Exception as e:
            self.log_message("Error setting track mixer: " + str(e))
            raise

    def _get_returns(self):
        """The return tracks, so sends can be addressed by name."""
        try:
            letters = "ABCDEFGHIJKL"
            returns = []
            for i, track in enumerate(self._song.return_tracks):
                returns.append({
                    "index": i,
                    "letter": letters[i] if i < len(letters) else str(i),
                    "name": track.name,
                    "volume": float(track.mixer_device.volume.value),
                    "volume_db": self._volume_db(track.mixer_device.volume),
                    "mute": bool(track.mute),
                    "devices": [d.name for d in track.devices],
                })
            return {"count": len(returns), "returns": returns}
        except Exception as e:
            self.log_message("Error listing returns: " + str(e))
            raise

    def _set_send(self, track_index, kind, send_index, send_name, value):
        """Set one send of a track, by return-track name, letter or index."""
        try:
            track = self._resolve_track(track_index, kind)
            sends = list(track.mixer_device.sends)
            returns = list(self._song.return_tracks)
            idx = None
            if send_name:
                wanted = str(send_name).strip().lower()
                for i, r in enumerate(returns):
                    if r.name.lower() == wanted:
                        idx = i
                        break
                if idx is None:
                    letters = "abcdefghijkl"
                    if len(wanted) == 1 and wanted in letters and letters.index(wanted) < len(sends):
                        idx = letters.index(wanted)
                if idx is None:
                    raise ValueError("no return track named '%s'; returns are: %s" % (
                        send_name, ", ".join("%s (%s)" % (r.name, "ABCDEFGHIJKL"[i]) for i, r in enumerate(returns))))
            elif send_index is not None:
                idx = int(send_index)
            else:
                raise ValueError("give send_name or send_index")
            if idx < 0 or idx >= len(sends):
                raise IndexError("send index %d out of range; this track has %d sends" % (idx, len(sends)))
            v = float(value)
            if v < 0.0 or v > 1.0:
                raise ValueError("send value must be between 0.0 and 1.0")
            sends[idx].value = v
            return {
                "track": track.name,
                "send_index": idx,
                "return_name": returns[idx].name if idx < len(returns) else None,
                "value": float(sends[idx].value),
            }
        except Exception as e:
            self.log_message("Error setting send: " + str(e))
            raise

    def _set_track_color(self, track_index, kind, color_index):
        try:
            track = self._resolve_track(track_index, kind)
            track.color_index = int(color_index)
            return {"name": track.name, "color_index": int(track.color_index), "color": int(track.color)}
        except Exception as e:
            self.log_message("Error setting track color: " + str(e))
            raise

    def _set_clip_color(self, track_index, clip_index, arrangement, color_index):
        try:
            track = self._resolve_track(track_index)
            if arrangement:
                clips = list(track.arrangement_clips)
                if clip_index < 0 or clip_index >= len(clips):
                    raise IndexError("Arrangement clip index out of range")
                clip = clips[clip_index]
            else:
                if clip_index < 0 or clip_index >= len(track.clip_slots):
                    raise IndexError("Clip index out of range")
                slot = track.clip_slots[clip_index]
                if not slot.has_clip:
                    raise ValueError("No clip in slot %d" % clip_index)
                clip = slot.clip
            clip.color_index = int(color_index)
            return {"name": clip.name, "color_index": int(clip.color_index), "color": int(clip.color)}
        except Exception as e:
            self.log_message("Error setting clip color: " + str(e))
            raise

    def _find_drum_rack_in(self, device, depth):
        """The first Drum Rack inside a rack's chains (up to four levels)."""
        if depth > 4 or not getattr(device, "can_have_chains", False):
            return None
        try:
            chains = list(device.chains)
        except Exception:
            return None
        for chain in chains:
            try:
                for d in chain.devices:
                    if getattr(d, "can_have_drum_pads", False):
                        return d
                    inner = self._find_drum_rack_in(d, depth + 1)
                    if inner is not None:
                        return inner
            except Exception:
                continue
        return None

    def _get_drum_rack_pads(self, track_index, device_index=-1, kind="track"):
        """Which sample sits on which pad of a track's Drum Rack."""
        try:
            track = self._resolve_track(track_index, kind)
            devices = list(track.devices)
            device_index = int(device_index if device_index is not None else -1)
            if device_index >= 0:
                if device_index >= len(devices):
                    raise IndexError("Device index out of range")
                device = devices[device_index]
                if not getattr(device, "can_have_drum_pads", False):
                    raise ValueError("device %d (%s) is not a Drum Rack" % (device_index, device.name))
                chosen = (device_index, device)
            else:
                chosen = None
                for i, d in enumerate(devices):
                    if getattr(d, "can_have_drum_pads", False):
                        chosen = (i, d)
                        break
                if chosen is None:
                    # A kit is often an Instrument Rack wrapping the Drum Rack:
                    # walk into rack chains, depth first.
                    for i, d in enumerate(devices):
                        inner = self._find_drum_rack_in(d, 0)
                        if inner is not None:
                            chosen = (i, inner)
                            break
                if chosen is None:
                    raise ValueError("no Drum Rack on track %d (%s), not even inside a rack; devices: %s" % (
                        track_index, track.name, ", ".join(d.name for d in devices) or "none"))
            pads = []
            for pad in chosen[1].drum_pads:
                chains = list(getattr(pad, "chains", []))
                if not chains:
                    continue
                pads.append({
                    "pitch": int(pad.note),
                    "note_name": self._live_note_name(pad.note),
                    "pad_name": str(pad.name),
                    "chains": [str(c.name) for c in chains],
                    "mute": bool(pad.mute),
                    "solo": bool(pad.solo),
                })
            return {
                "track_index": int(track_index),
                "track_name": track.name,
                "device_index": chosen[0],
                "device_name": chosen[1].name,
                "pad_count": len(pads),
                "pads": pads,
            }
        except Exception as e:
            self.log_message("Error reading drum rack pads: " + str(e))
            raise

    def _place_clips(self, track_index, clip_index, times):
        """A Session clip placed at several Arrangement times in one round trip."""
        try:
            track = self._resolve_track(track_index)
            slots = list(track.clip_slots)
            ci = int(clip_index)
            if ci < 0 or ci >= len(slots) or not slots[ci].has_clip:
                raise ValueError("slot %d on '%s' holds no clip" % (ci, track.name))
            clip = slots[ci].clip
            placed, failed = [], []
            for t in list(times or []):
                yield None
                try:
                    track.duplicate_clip_to_arrangement(clip, float(t))
                    placed.append(float(t))
                except Exception as e:
                    failed.append({"time": float(t), "error": str(e)})
            yield Done({"track": "%s" % track.name, "clip": "%s" % clip.name, "length": float(clip.length),
                        "placed": placed, "failed": failed,
                        "arrangement_clips": len(list(track.arrangement_clips))})
        except Exception as e:
            self.log_message("Error placing clips: " + str(e))
            raise

    def _delete_arrangement_clips(self, track_index, indices=None, all_clips=False, from_beat=None, to_beat=None):
        """Several Arrangement clips of one track in one round trip: by index,
        every one, or every one that starts inside [from_beat, to_beat)."""
        try:
            track = self._resolve_track(track_index)
            clips = list(track.arrangement_clips)
            chosen = []
            if all_clips:
                chosen = list(range(len(clips)))
            elif indices is not None:
                chosen = [int(i) for i in indices]
            else:
                lo = float(from_beat) if from_beat is not None else float("-inf")
                hi = float(to_beat) if to_beat is not None else float("inf")
                chosen = [i for i, c in enumerate(clips) if lo <= float(c.start_time) < hi]
            for i in chosen:
                if i < 0 or i >= len(clips):
                    raise IndexError("Arrangement clip index %d out of range; track has %d" % (i, len(clips)))
            removed = []
            for i in sorted(set(chosen), reverse=True):
                yield None
                c = clips[i]
                removed.append({"index": i, "name": "%s" % c.name, "start_time": float(c.start_time),
                                "end_time": float(c.end_time)})
                track.delete_clip(c)
            removed.reverse()
            yield Done({"track": "%s" % track.name, "removed": removed,
                        "remaining": len(list(track.arrangement_clips))})
        except Exception as e:
            self.log_message("Error deleting arrangement clips: " + str(e))
            raise

    def _duplicate_arrangement_clip(self, track_index, clip_index, times):
        """Copies of an Arrangement clip at other Arrangement times (Live 11+)."""
        try:
            track = self._resolve_track(track_index)
            clips = list(track.arrangement_clips)
            ci = int(clip_index)
            if ci < 0 or ci >= len(clips):
                raise IndexError("Arrangement clip index %d out of range; track has %d" % (ci, len(clips)))
            clip = clips[ci]
            placed, failed = [], []
            for t in list(times or []):
                yield None
                try:
                    track.duplicate_clip_to_arrangement(clip, float(t))
                    placed.append(float(t))
                except Exception as e:
                    failed.append({"time": float(t), "error": str(e)})
            yield Done({"track": "%s" % track.name, "clip": "%s" % clip.name,
                        "length": float(clip.end_time) - float(clip.start_time),
                        "placed": placed, "failed": failed,
                        "arrangement_clips": len(list(track.arrangement_clips))})
        except Exception as e:
            self.log_message("Error duplicating arrangement clip: " + str(e))
            raise

    def _arrangement_summary(self):
        """What the Arrangement holds, in one pass: the last beat any clip ends
        on, how many clips there are and how many tracks carry one. Needs
        track.arrangement_clips (Live 11+); on an older Live it reports
        supported False and the server records nothing rather than recording
        over work it cannot see."""
        try:
            end = 0.0
            clips = 0
            tracks = 0
            supported = True
            for track in self._song.tracks:
                yield None
                try:
                    arranged = list(track.arrangement_clips)
                except Exception:
                    supported = False
                    break
                if not arranged:
                    continue
                tracks += 1
                clips += len(arranged)
                for clip in arranged:
                    ends = float(clip.end_time)
                    if ends > end:
                        end = ends
            bpb = self._beats_per_bar()
            bars = int(math.ceil(end / float(bpb))) if end > 0 else 0
            yield Done({"supported": supported, "end_beat": end, "bars": bars,
                        "clips": clips, "tracks": tracks, "beats_per_bar": bpb})
        except Exception as e:
            self.log_message("Error reading the arrangement summary: " + str(e))
            raise

    def _start_arrangement_record(self, from_beat=0.0, replace=False):
        """Arm Live's Arrangement Record so the performance is written down.

        With replace, every Arrangement clip on every track is deleted first —
        the producer asked for that by name, having been told how many bars it
        is. The playhead is set before record_mode, so the first bar played is
        the first bar of the take. Starting the transport is left to the
        caller's fire."""
        try:
            removed_clips = 0
            removed_tracks = 0
            if replace:
                for track in self._song.tracks:
                    yield None
                    try:
                        arranged = list(track.arrangement_clips)
                    except Exception:
                        continue
                    if not arranged:
                        continue
                    removed_tracks += 1
                    for clip in arranged:
                        track.delete_clip(clip)
                        removed_clips += 1
            start = float(from_beat)
            if start < 0.0:
                start = 0.0
            song = self._song
            song.current_song_time = start
            song.record_mode = True
            self._arr_recording = True
            self._arm_perf_tick()
            bpb = self._beats_per_bar()
            yield Done({"from_beat": start, "from_bar": int(start // bpb) + 1,
                        "replaced_clips": removed_clips,
                        "replaced_tracks": removed_tracks,
                        "record_mode": bool(song.record_mode)})
        except Exception as e:
            self.log_message("Error starting the arrangement record: " + str(e))
            raise

    def _stop_arrangement_record(self, from_beat=None, back_to_arrangement=True):
        """Disarm Arrangement Record and report the take: the clips that end
        after the take's first beat, and the tracks carrying them. Also puts
        the tracks back on the timeline, so the set is not left overridden by
        the Session clips the performance fired."""
        try:
            song = self._song
            self._clear_record_mode()
            start = None if from_beat is None else float(from_beat)
            clips = 0
            tracks = 0
            end = 0.0
            for track in self._song.tracks:
                yield None
                try:
                    arranged = list(track.arrangement_clips)
                except Exception:
                    continue
                hit = 0
                for clip in arranged:
                    ends = float(clip.end_time)
                    if start is None or ends > start + 1e-6:
                        hit += 1
                        if ends > end:
                            end = ends
                if hit:
                    tracks += 1
                    clips += hit
            if back_to_arrangement:
                try:
                    song.back_to_arranger = False
                except Exception as e:
                    self.log_message("could not return to the arrangement: " + str(e))
            bpb = self._beats_per_bar()
            yield Done({"clips": clips, "tracks": tracks, "end_beat": end,
                        "end_bar": (int(math.ceil(end / float(bpb))) if end > 0 else 0),
                        "from_beat": start,
                        "from_bar": (None if start is None else int(start // bpb) + 1),
                        "record_mode": bool(self._safe_song_property("record_mode", bool, False))})
        except Exception as e:
            self.log_message("Error stopping the arrangement record: " + str(e))
            raise

    def _clear_record_mode(self):
        """Take Live out of Arrangement Record, whoever asked. Used by the stop
        command and by the performance tick when the transport stops, so a
        half-finished take never leaves the set armed."""
        self._arr_recording = False
        try:
            self._song.record_mode = False
        except Exception as e:
            self.log_message("could not clear record_mode: " + str(e))

    def _live_notes(self, notes):
        """Note objects as the tuples clip.set_notes takes."""
        out = []
        for note in notes or []:
            out.append((int(note.get("pitch", 60)), float(note.get("start_time", 0.0)),
                        float(note.get("duration", 0.25)), int(round(float(note.get("velocity", 100)))),
                        bool(note.get("mute", False))))
        return tuple(out)

    def _create_tracks(self, tracks, on_existing="add"):
        """Several tracks in one round trip: create, name, load the instrument
        or effect by URI, set the fader (dB or raw), pan, colour and sends.
        Stops at the first failure and reports how far it got.

        `on_existing` says what to do about a name the set already has:
        "add" (the old behaviour) makes another, "converge" reuses the track
        that is there, so re-running a document after a dropped connection
        finishes it instead of building a second copy, and "fail" refuses."""
        created = []
        on_existing = str(on_existing or "add").lower()
        try:
            for spec in list(tracks or []):
                yield None  # one track (with its device load) per slice
                wanted = spec.get("name")
                if wanted and on_existing != "add":
                    there = None
                    for i, t in enumerate(self._song.tracks):
                        if ("%s" % t.name).strip().lower() == ("%s" % wanted).strip().lower():
                            there = i
                            break
                    if there is not None:
                        if on_existing == "fail":
                            raise ValueError("the set already has a track named '%s'" % wanted)
                        created.append({"index": there,
                                        "name": "%s" % self._song.tracks[there].name,
                                        "reused": True})
                        continue
                kind = str(spec.get("kind") or "midi").lower()
                if kind == "audio":
                    self._create_audio_track(-1)
                else:
                    self._create_midi_track(-1)
                index = len(self._song.tracks) - 1
                track = self._song.tracks[index]
                entry = {"index": index, "name": "%s" % track.name}
                # Recorded before the device load, so a failure half-way names
                # the tracks that are really in the set.
                created.append(entry)
                name = spec.get("name")
                if name:
                    track.name = "%s" % name
                    entry["name"] = "%s" % track.name
                uri = spec.get("instrument_uri")
                if uri:
                    loaded = self._load_browser_item(index, uri, "track")
                    dev = loaded.get("loaded_device") or {}
                    entry["device"] = dev.get("name")
                if spec.get("volume_db") is not None or spec.get("volume") is not None or spec.get("pan") is not None:
                    self._set_track_mixer(index, "track", spec.get("volume"), spec.get("pan"),
                                          None, None, None, spec.get("volume_db"))
                for send in list(spec.get("sends") or []):
                    self._set_send(index, "track", send.get("index"), send.get("name"), send.get("value", 0.0))
                if spec.get("color_index") is not None:
                    try:
                        track.color_index = int(spec.get("color_index"))
                    except Exception as e:
                        entry["color_error"] = str(e)
            yield Done({"created": created})
        except Exception as e:
            self.log_message("Error creating tracks: " + str(e))
            if not created:
                # Nothing was made: the reason is the whole story.
                raise
            raise RuntimeError("after %d of %d tracks (%s): %s" % (
                len(created), len(list(tracks or [])), ", ".join(c["name"] for c in created), str(e)))

    def _write_clips(self, clips):
        """Several Session clips in one round trip: each entry creates the
        clip at {track_index, clip_index} with `length`, names it and writes
        its `notes`; an entry with `copy_of` copies the notes of that slot on
        the same track inside Live instead (the notes travel once). Every
        slot is checked before the first clip is made."""
        try:
            song = self._song
            specs = list(clips or [])
            for i, spec in enumerate(specs):
                ti, ci = int(spec.get("track_index", -1)), int(spec.get("clip_index", -1))
                if ti < 0 or ti >= len(song.tracks):
                    raise IndexError("clip %d: track index %d out of range" % (i + 1, ti))
                slots = list(song.tracks[ti].clip_slots)
                if ci < 0 or ci >= len(slots):
                    raise IndexError("clip %d: slot %d out of range on '%s' (create_scene adds rows)" % (i + 1, ci, song.tracks[ti].name))
                if slots[ci].has_clip:
                    raise ValueError("clip %d: slot %d on '%s' already holds a clip" % (i + 1, ci, song.tracks[ti].name))
            written = []
            try:
                for spec in specs:
                    yield None  # one clip per slice while the music plays
                    ti, ci = int(spec.get("track_index")), int(spec.get("clip_index"))
                    track = song.tracks[ti]
                    slot = track.clip_slots[ci]
                    source = spec.get("copy_of")
                    notes = spec.get("notes")
                    length = spec.get("length")
                    if source is not None:
                        src_slot = track.clip_slots[int(source)]
                        if not src_slot.has_clip:
                            raise ValueError("copy_of: slot %d on '%s' holds no clip" % (int(source), track.name))
                        src = src_slot.clip
                        if length is None:
                            length = float(src.length)
                        if notes is None:
                            notes = self._notes_from_clip(src)
                        if not spec.get("name"):
                            spec = dict(spec, name="%s" % src.name)
                    slot.create_clip(float(length or 4.0))
                    clip = slot.clip
                    if spec.get("name"):
                        clip.name = "%s" % spec.get("name")
                    live_notes = self._live_notes(notes)
                    if live_notes:
                        clip.set_notes(live_notes)
                    written.append({"track_index": ti, "clip_index": ci, "name": "%s" % clip.name,
                                    "length": float(clip.length), "notes": len(live_notes),
                                    "copied": source is not None})
            except Exception as e:
                raise RuntimeError("after %d of %d clips: %s" % (len(written), len(specs), str(e)))
            yield Done({"written": written})
        except Exception as e:
            self.log_message("Error writing clips: " + str(e))
            raise

    def _create_return_track(self, name=None):
        """A new return track at the end (Song.create_return_track), optionally named."""
        try:
            song = self._song
            before = len(song.return_tracks)
            song.create_return_track()
            returns = list(song.return_tracks)
            if len(returns) <= before:
                raise RuntimeError("Live did not add a return track (the limit is 12)")
            track = returns[-1]
            if name:
                track.name = "%s" % name
            letters = "ABCDEFGHIJKL"
            i = len(returns) - 1
            return {"index": i, "letter": letters[i] if i < len(letters) else str(i),
                    "name": "%s" % track.name, "return_count": len(returns)}
        except Exception as e:
            self.log_message("Error creating return track: " + str(e))
            raise

    def _delete_arrangement_clip(self, track_index, clip_index):
        """Remove one clip from the Arrangement; clip_index as in get_arrangement_clips."""
        try:
            track = self._resolve_track(track_index)
            clips = list(track.arrangement_clips)
            if clip_index < 0 or clip_index >= len(clips):
                raise IndexError("Arrangement clip index %d out of range; track has %d" % (clip_index, len(clips)))
            clip = clips[clip_index]
            info = {
                "name": clip.name,
                "start_time": float(clip.start_time),
                "end_time": float(clip.end_time),
            }
            track.delete_clip(clip)
            info["remaining"] = len(list(track.arrangement_clips))
            return info
        except Exception as e:
            self.log_message("Error deleting arrangement clip: " + str(e))
            raise

    def _delete_locator(self, name=None, time_val=None, response_queue=None):
        """Remove a locator by name or by beat position.

        Same two-tick dance as _create_locator: move the playhead, let Live
        apply it, then toggle the cue away and answer the socket.
        """
        try:
            song = self._song
            target = None
            for cue in song.cue_points:
                if name and str(cue.name) == str(name):
                    target = cue
                    break
                if time_val is not None and abs(float(cue.time) - float(time_val)) < 1e-3:
                    target = cue
                    break
            if target is None:
                existing = ", ".join("'%s' @ %s" % (c.name, c.time) for c in song.cue_points) or "none"
                raise ValueError("no locator matches (name=%s, time=%s); locators: %s" % (name, time_val, existing))
            info = {"deleted": str(target.name), "time": float(target.time)}
            cue_time = float(target.time)
            original_time = float(song.current_song_time)
            song.current_song_time = cue_time

            def finish():
                try:
                    if abs(float(song.current_song_time) - cue_time) > 1e-3:
                        msg = "Could not move the playhead to beat %s to remove the locator" % cue_time
                        if response_queue is not None:
                            self._answer(response_queue, {"status": "error", "message": msg})
                        return
                    song.set_or_delete_cue()
                    still_there = any(abs(float(c.time) - cue_time) < 1e-3 for c in song.cue_points)
                    if still_there:
                        raise Exception("Live did not remove the cue at beat %s" % cue_time)
                    try:
                        song.current_song_time = original_time
                    except Exception:
                        pass
                    info["remaining"] = len(list(song.cue_points))
                    if response_queue is not None:
                        self._answer(response_queue, {"status": "success", "result": info})
                except Exception as e:
                    self.log_message("Error deleting locator: " + str(e))
                    if response_queue is not None:
                        self._answer(response_queue, {"status": "error", "message": str(e)})

            if response_queue is None:
                finish()
                return info
            self.schedule_message(2, finish)
            return DEFERRED
        except Exception as e:
            self.log_message("Error deleting locator: " + str(e))
            raise

    # ── Search, clip settings, meters, automation ────────────────────────────

    BROWSER_CATEGORIES = ["instruments", "sounds", "drums", "audio_effects", "midi_effects",
                          "samples", "packs", "user_library"]

    def _browser_indexer(self, category_roots):
        """A generator that walks the browser once, yielding loadable items.
        Kept on self so successive searches continue where the last stopped."""
        max_depth = 7
        for cat, root in category_roots:
            stack = [(root, [], 0)]
            while stack:
                item, path, depth = stack.pop()
                name = item.name if hasattr(item, "name") else ""
                here = path + [name] if name else path
                if depth > 0 and bool(getattr(item, "is_loadable", False)):
                    yield {"name": name, "uri": getattr(item, "uri", None),
                           "path": "/".join(here), "category": cat,
                           "is_device": bool(getattr(item, "is_device", False))}
                if depth >= max_depth:
                    continue
                try:
                    kids = list(item.children) if hasattr(item, "children") else []
                except Exception:
                    kids = []
                for kid in reversed(kids):
                    stack.append((kid, here, depth + 1))

    def _browser_index(self, wanted, budget_s):
        """Grow the per-category index for up to budget_s seconds of wall
        time, yielding every few items so the executor can slice the walk
        across ticks; the last value is Done((items so far, complete?)).
        While the music plays the wall budget is capped, so a search never
        holds the show: the index grows a little on every call and the
        server keeps what it has seen."""
        if self._safe_song_property("is_playing", bool, False):
            budget_s = min(float(budget_s), 2.0)
        for step in self._browser_index_steps(wanted, budget_s):
            yield step

    def _browser_index_steps(self, wanted, budget_s):
        if not hasattr(self, "_index"):
            self._index = {}
        state = self._index.get(wanted)
        if state is None:
            app = self.application()
            roots = []
            for name in self.BROWSER_CATEGORIES:
                if (wanted == "all" and name in ("instruments", "sounds", "drums", "audio_effects", "midi_effects")) or wanted == name:
                    root = getattr(app.browser, name, None)
                    if root is not None:
                        roots.append((name, root))
            if not roots:
                raise ValueError("unknown category '%s'; use one of: all, %s" % (wanted, ", ".join(self.BROWSER_CATEGORIES)))
            state = {"items": [], "gen": self._browser_indexer(roots), "done": False}
            self._index[wanted] = state
        started = time.time()
        while not state["done"] and time.time() - started < budget_s:
            try:
                for _ in range(10):
                    state["items"].append(next(state["gen"]))
            except StopIteration:
                state["done"] = True
            yield None
        yield Done((state["items"], state["done"]))

    def _search_browser(self, query, category="all", limit=30):
        """Find loadable browser items whose name or path contains every word of
        the query. The browser is indexed once per category and kept for the
        session, so the first search pays the walk and the rest are instant."""
        try:
            words = [w for w in str(query or "").lower().split() if w]
            if not words:
                raise ValueError("give a query, e.g. 'analog bass' or 'techno kit'")
            wanted = str(category or "all").lower()
            limit = max(1, min(int(limit or 30), 200))
            started = time.time()
            items, complete = None, False
            for step in self._browser_index(wanted, 8.0):
                if isinstance(step, Done):
                    items, complete = step.result
                    break
                yield None
            hits = []
            total = 0
            for it in items:
                hay = (it["path"] or "").lower()
                if all(w in hay for w in words):
                    total += 1
                    if len(hits) < limit:
                        hits.append(it)
            yield Done({
                "query": query,
                "category": wanted,
                "total_matches": total,
                "returned": len(hits),
                "indexed_items": len(items),
                "index_complete": complete,
                "truncated_walk": not complete,
                "seconds": round(time.time() - started, 2),
                "items": hits,
            })
        except Exception as e:
            self.log_message("Error searching browser: " + str(e))
            raise

    def _search_browser_legacy(self, query, category="all", limit=30):
        try:
            app = self.application()
            browser = app.browser
            words = [w for w in str(query or "").lower().split() if w]
            if not words:
                raise ValueError("give a query, e.g. 'analog bass' or 'techno kit'")
            categories = []
            wanted = str(category or "all").lower()
            for name in self.BROWSER_CATEGORIES:
                if (wanted == "all" and name in ("instruments", "sounds", "drums", "audio_effects", "midi_effects")) or wanted == name:
                    root = getattr(browser, name, None)
                    if root is not None:
                        categories.append((name, root))
            if not categories:
                raise ValueError("unknown category '%s'; use one of: all, %s" % (category, ", ".join(self.BROWSER_CATEGORIES)))
            limit = max(1, min(int(limit or 30), 200))
            hits = []
            # Every `.children` read crosses into Live and costs real time, and
            # the browser is tens of thousands of items. So: a time budget
            # (the server waits 25 s for this command), folders whose own name
            # matches a query word are walked first, and depth is capped.
            budget_s = 12.0
            started = time.time()
            state = {"visited": 0, "total": 0, "out_of_time": False}

            def matches_any(name):
                low = name.lower()
                return any(w in low for w in words)

            def walk(item, path, cat, depth):
                if state["out_of_time"] or depth > 6:
                    return
                if time.time() - started > budget_s:
                    state["out_of_time"] = True
                    return
                state["visited"] += 1
                name = item.name if hasattr(item, "name") else ""
                here = path + [name] if name else path
                loadable = bool(getattr(item, "is_loadable", False))
                if loadable and depth > 0:
                    hay = " ".join(here).lower()
                    if all(w in hay for w in words):
                        state["total"] += 1
                        if len(hits) < limit:
                            hits.append({
                                "name": name,
                                "uri": getattr(item, "uri", None),
                                "path": "/".join(here),
                                "category": cat,
                                "is_device": bool(getattr(item, "is_device", False)),
                            })
                try:
                    kids = list(item.children) if hasattr(item, "children") else []
                except Exception:
                    kids = []
                if not kids:
                    return
                first = [k for k in kids if matches_any(getattr(k, "name", "") or "")]
                rest = [k for k in kids if k not in first]
                for kid in first + rest:
                    if len(hits) >= limit and state["total"] >= limit * 4:
                        return
                    walk(kid, here, cat, depth + 1)

            for cat, root in categories:
                walk(root, [], cat, 0)
            return {
                "query": query,
                "category": wanted,
                "total_matches": state["total"],
                "returned": len(hits),
                "truncated_walk": state["out_of_time"],
                "seconds": round(time.time() - started, 2),
                "visited": state["visited"],
                "items": hits,
            }
        except Exception as e:
            self.log_message("Error searching browser: " + str(e))
            raise

    def _get_library_status(self):
        """What this Live has: version, top-level browser contents, installed packs."""
        try:
            app = self.application()
            try:
                version = "%d.%d.%d" % (app.get_major_version(), app.get_minor_version(), app.get_bugfix_version())
            except Exception:
                version = "unknown"
            browser = app.browser

            def top_level(root_name):
                root = getattr(browser, root_name, None)
                out = []
                if root is None:
                    return out
                try:
                    kids = list(root.children)
                except Exception:
                    return out
                for k in kids[:300]:
                    entry = {"name": getattr(k, "name", "?")}
                    try:
                        entry["is_folder"] = bool(list(k.children))
                    except Exception:
                        entry["is_folder"] = False
                    entry["is_loadable"] = bool(getattr(k, "is_loadable", False))
                    out.append(entry)
                return out

            packs = top_level("packs")
            # One level into each pack, so a search can go straight to a folder.
            try:
                root = getattr(browser, "packs", None)
                kids = list(root.children) if root is not None else []
                for entry, pack in zip(packs, kids):
                    try:
                        entry["folders"] = [str(getattr(k, "name", "?")) for k in list(pack.children)[:40]]
                    except Exception:
                        entry["folders"] = []
            except Exception:
                pass
            result = {
                "live_version": version,
                "instruments": top_level("instruments"),
                "audio_effects": top_level("audio_effects"),
                "midi_effects": top_level("midi_effects"),
                "packs": packs,
                "drums": top_level("drums"),
                "sounds": top_level("sounds"),
                "has_user_library": getattr(browser, "user_library", None) is not None,
            }
            names = [i["name"] for i in result["instruments"]]
            if "Wavetable" in names and "Operator" in names:
                result["edition_hint"] = "Suite-level instrument set"
            elif "Wavetable" not in names and "Operator" not in names:
                result["edition_hint"] = "Standard or Intro-level instrument set"
            return result
        except Exception as e:
            self.log_message("Error reading library status: " + str(e))
            raise

    # ── Transport and structure helpers ──────────────────────────────────────

    def _play_from(self, time_val, response_queue=None):
        """Move the playhead and play from there (continue_playing), on the
        tick after the move has landed."""
        try:
            song = self._song
            target = float(time_val)
            if song.is_playing:
                song.stop_playing()
            song.current_song_time = target

            def finish():
                try:
                    song.continue_playing()

                    def settle():
                        # A seek made while the transport was stopped does not
                        # always survive continue_playing: Live resumes from
                        # where it last stopped. A seek while it is rolling
                        # lands, so confirm and repeat it if it did not.
                        try:
                            now = float(song.current_song_time)
                            if abs(now - target) > 0.5:
                                song.current_song_time = target
                                now = float(song.current_song_time)
                            result = {"playing_from": target, "at": now,
                                      "is_playing": bool(song.is_playing)}
                            if response_queue is not None:
                                self._answer(response_queue, {"status": "success", "result": result})
                        except Exception as e:
                            if response_queue is not None:
                                self._answer(response_queue, {"status": "error", "message": str(e)})

                    self.schedule_message(1, settle)
                except Exception as e:
                    if response_queue is not None:
                        self._answer(response_queue, {"status": "error", "message": str(e)})

            if response_queue is None:
                finish()
                return {"playing_from": target}
            self.schedule_message(2, finish)
            return DEFERRED
        except Exception as e:
            self.log_message("Error playing from position: " + str(e))
            raise

    def _delete_track(self, track_index):
        try:
            tracks = list(self._song.tracks)
            track_index = int(track_index)
            if track_index < 0 or track_index >= len(tracks):
                raise IndexError("track index %d out of range (0-%d)" % (track_index, len(tracks) - 1))
            if len(tracks) == 1:
                raise ValueError(
                    "Live keeps at least one track in a set, so the last track cannot be deleted. "
                    "Create the track that replaces it first, then delete this one.")
            if self._safe_attr(tracks[track_index], "is_frozen", bool, False):
                raise ValueError(
                    "'%s' is frozen; unfreeze it in Live (right-click the track, Unfreeze Track) and delete it then."
                    % tracks[track_index].name)
            name = tracks[track_index].name
            self._song.delete_track(track_index)
            return {"deleted": name, "index": track_index, "track_count": len(self._song.tracks)}
        except Exception as e:
            self.log_message("Error deleting track: " + str(e))
            raise

    def _back_to_arrangement(self):
        try:
            self._song.back_to_arranger = False
            return {"back_to_arrangement": True}
        except Exception as e:
            self.log_message("Error returning to arrangement: " + str(e))
            raise

    def _set_arrangement_loop(self, start=None, length=None, enabled=None):
        try:
            song = self._song
            if start is not None:
                song.loop_start = float(start)
            if length is not None:
                song.loop_length = float(length)
            if enabled is not None:
                song.loop = bool(enabled)
            return {"loop_start": float(song.loop_start), "loop_length": float(song.loop_length),
                    "loop": bool(song.loop)}
        except Exception as e:
            self.log_message("Error setting arrangement loop: " + str(e))
            raise

    # ── Capture: record the master through a Resampling track ────────────────

    CAPTURE_TRACK_NAME = "Capture"

    @property
    def _song(self):
        """Always the current set: a cached handle went stale on set reload."""
        return self.song()

    def _find_capture_track(self):
        for i, t in enumerate(self._song.tracks):
            try:
                if t.name == self.CAPTURE_TRACK_NAME and t.has_audio_input:
                    return i, t
            except Exception:
                continue
        return None, None

    def _ensure_capture_track(self):
        """The Capture track: audio, input Resampling, monitoring off, muted, armed."""
        try:
            index, track = self._find_capture_track()
            created = False
            if track is None:
                self._song.create_audio_track(-1)
                index = len(self._song.tracks) - 1
                track = self._song.tracks[index]
                track.name = self.CAPTURE_TRACK_NAME
                created = True
            chosen = None
            names = []
            for rt in track.available_input_routing_types:
                names.append(str(rt.display_name))
                if str(rt.display_name).lower() == "resampling":
                    chosen = rt
            if chosen is None:
                raise ValueError("no Resampling input on this track; inputs are: %s" % ", ".join(names))
            if str(getattr(track.input_routing_type, "display_name", "")).lower() != "resampling":
                track.input_routing_type = chosen
            try:
                track.current_monitoring_state = 2  # Off
            except Exception as e:
                self.log_message("monitoring state not set: " + str(e))
            track.mute = True
            if getattr(track, "can_be_armed", False):
                track.arm = True
            return {
                "index": index,
                "created": created,
                "input": str(getattr(track.input_routing_type, "display_name", "?")),
                "slots": len(track.clip_slots),
            }
        except Exception as e:
            self.log_message("Error ensuring capture track: " + str(e))
            raise

    def _recording_capture(self, track):
        for i, slot in enumerate(track.clip_slots):
            try:
                if slot.has_clip and slot.clip.is_recording:
                    return i, slot
            except Exception:
                continue
        return None, None

    def _start_capture(self, start, bars, name, response_queue=None):
        """Play from just before `start` and record `bars` bars of the master
        into the first free slot of the Capture track. The fire happens on a
        later tick (the playhead move is asynchronous) with beat quantization,
        so the recording begins exactly at `start`."""
        try:
            song = self._song
            index, track = self._find_capture_track()
            if track is None:
                raise ValueError("no Capture track; call ensure_capture_track first")
            rec_index, _ = self._recording_capture(track)
            if rec_index is not None:
                raise ValueError("a capture is already recording in slot %d ('%s')" % (
                    rec_index, track.clip_slots[rec_index].clip.name))
            slot_index = None
            for i, slot in enumerate(track.clip_slots):
                if not slot.has_clip:
                    slot_index = i
                    break
            if slot_index is None:
                raise ValueError("every slot of the Capture track holds a capture; delete one (delete_clip on track %d) first" % index)
            slot = track.clip_slots[slot_index]
            bars = int(bars)
            if bars < 1 or bars > 64:
                raise ValueError("bars must be between 1 and 64")
            beats_per_bar = int(song.signature_numerator)
            record_length = float(bars * beats_per_bar)
            start = float(start)
            if Live is None:
                raise RuntimeError("captures need Live's own Python (Live 11 or newer)")
            # A whole bar of preroll, so the fire is armed while the playhead is
            # still inside the bar before `start`: the seek and the transport are
            # asynchronous, and firing blind is what put digital silence at the
            # head of a capture. The quantized launch then lands on `start`.
            bar_beats = float(beats_per_bar)
            preroll = bar_beats if start >= bar_beats else start
            begin = max(0.0, start - preroll)
            on_bar = abs((start / bar_beats) - round(start / bar_beats)) < 1e-6
            on_beat = abs(start - round(start)) < 1e-6

            def quantization(*names):
                for attr in names:
                    value = getattr(Live.Song.Quantization, attr, None)
                    if value is not None:
                        return attr, value
                return None, None

            if preroll <= 0.0:
                quant_name, quant = quantization("q_no_q")
            elif on_bar:
                quant_name, quant = quantization("q_bar", "q_quarter", "q_no_q")
            elif on_beat:
                quant_name, quant = quantization("q_quarter", "q_no_q")
            else:
                quant_name, quant = quantization("q_no_q")
            if quant is None:
                raise RuntimeError("Live.Song.Quantization has no usable member")
            self._pending_capture = {"slot": slot_index, "name": str(name or "capture"),
                                     "start": start, "bars": bars}
            # Fixed-length Session recording needs Live 11+; probe the signature.
            try:
                slot.fire.__call__  # noqa
            except Exception:
                raise RuntimeError("captures need Live 11 or newer")
            if song.is_playing:
                song.stop_playing()
            song.current_song_time = begin
            state = {"ticks": 0, "seeks": 0}

            def fail(message):
                self._pending_capture = None
                self.log_message("Error starting capture: " + message)
                if response_queue is not None:
                    self._answer(response_queue, {"status": "error", "message": message})

            def fire_now(confirmed_at):
                slot.fire(record_length=record_length, launch_quantization=quant)
                result = {"slot": slot_index, "record_length": record_length,
                          "preroll_beats": preroll, "beats_per_bar": beats_per_bar,
                          "tempo": float(song.tempo), "started_at": start,
                          "begin_beat": begin, "confirmed_at": confirmed_at,
                          "launch_quantization": quant_name}
                if response_queue is not None:
                    self._answer(response_queue, {"status": "success", "result": result})
                return result

            def arm():
                """Fire only once Live says the playhead is inside the preroll
                and still before `start`."""
                try:
                    if preroll <= 0.0:
                        # Nothing to wait for: the take starts where the
                        # transport does.
                        fire_now(float(song.current_song_time))
                        if not song.is_playing:
                            song.continue_playing()
                        return
                    if not song.is_playing:
                        # continue_playing plays from current_song_time;
                        # start_playing would jump back to the start marker.
                        song.continue_playing()
                        self.schedule_message(1, arm)
                        return
                    state["ticks"] += 1
                    now = float(song.current_song_time)
                    # The window is the preroll: after `begin`, before `start`.
                    # A seek made while stopped does not always survive
                    # continue_playing, so seek again now that it is rolling
                    # and look next tick. Firing outside the window is what
                    # recorded the wrong bars.
                    if now < begin - 0.05 or now >= start - 0.02:
                        if state["ticks"] > 60:
                            fail("the playhead would not settle before beat %.2f (it is at %.2f); nothing was recorded"
                                 % (start, now))
                            return
                        state["seeks"] += 1
                        song.current_song_time = begin
                        self.schedule_message(1, arm)
                        return
                    fire_now(now)
                except TypeError as e:
                    fail("captures need Live 11 or newer (fixed-length recording): " + str(e))
                except Exception as e:
                    fail(str(e))

            if response_queue is None:
                song.continue_playing()
                return fire_now(float(song.current_song_time))
            self.schedule_message(1, arm)
            return DEFERRED
        except Exception as e:
            self.log_message("Error starting capture: " + str(e))
            raise

    def _capture_status(self, slot_index):
        try:
            index, track = self._find_capture_track()
            if track is None:
                raise ValueError("no Capture track")
            slot_index = int(slot_index)
            if slot_index < 0 or slot_index >= len(track.clip_slots):
                raise IndexError("slot out of range")
            slot = track.clip_slots[slot_index]
            info = {"slot": slot_index, "track_index": index, "has_clip": bool(slot.has_clip),
                    "is_recording": False, "is_playing": bool(self._song.is_playing)}
            if slot.has_clip:
                clip = slot.clip
                info["is_recording"] = bool(getattr(clip, "is_recording", False))
                pending = getattr(self, "_pending_capture", None)
                if pending and pending.get("slot") == slot_index:
                    wanted = "%s @ %s" % (pending["name"], self._fmt_beat(pending["start"]))
                    try:
                        if clip.name != wanted:
                            clip.name = wanted
                    except Exception:
                        pass
                info["name"] = clip.name
                info["length"] = float(clip.length)
                if not info["is_recording"]:
                    info["file_path"] = self._safe_attr(clip, "file_path", str, None)
                    if getattr(self, "_pending_capture", None) and self._pending_capture.get("slot") == slot_index:
                        self._pending_capture = None
            return info
        except Exception as e:
            self.log_message("Error reading capture status: " + str(e))
            raise

    def _fmt_beat(self, v):
        v = float(v)
        return str(int(v)) if v == int(v) else str(v)

    def _stop_capture(self, slot_index=None):
        """Stop the transport, and the slot if it is still recording."""
        try:
            song = self._song
            index, track = self._find_capture_track()
            stopped_slot = None
            if track is not None:
                slots = [int(slot_index)] if slot_index is not None else range(len(track.clip_slots))
                for i in slots:
                    if i < 0 or i >= len(track.clip_slots):
                        continue
                    slot = track.clip_slots[i]
                    try:
                        if slot.has_clip and slot.clip.is_recording:
                            slot.stop()
                            stopped_slot = i
                    except Exception:
                        pass
            if song.is_playing:
                song.stop_playing()
            self._pending_capture = None
            return {"stopped_slot": stopped_slot, "is_playing": bool(song.is_playing)}
        except Exception as e:
            self.log_message("Error stopping capture: " + str(e))
            raise

    def _list_captures(self):
        try:
            index, track = self._find_capture_track()
            if track is None:
                yield Done({"track_index": None, "captures": []})
                return
            out = []
            for i, slot in enumerate(track.clip_slots):
                if not slot.has_clip:
                    continue
                yield None
                clip = slot.clip
                out.append({
                    "slot": i,
                    "name": clip.name,
                    "length": float(clip.length),
                    "is_recording": bool(getattr(clip, "is_recording", False)),
                    "file_path": self._safe_attr(clip, "file_path", str, None),
                })
            yield Done({"track_index": index, "captures": out})
        except Exception as e:
            self.log_message("Error listing captures: " + str(e))
            raise

    def _clip_at(self, track_index, clip_index, arrangement=False):
        track = self._resolve_track(track_index)
        if arrangement:
            clips = list(track.arrangement_clips)
            if clip_index < 0 or clip_index >= len(clips):
                raise IndexError("Arrangement clip index %d out of range; track has %d" % (clip_index, len(clips)))
            return track, clips[clip_index]
        if clip_index < 0 or clip_index >= len(track.clip_slots):
            raise IndexError("Clip index out of range")
        slot = track.clip_slots[clip_index]
        if not slot.has_clip:
            raise ValueError("No clip in slot %d of track %d" % (clip_index, track_index))
        return track, slot.clip

    LAUNCH_MODES = {"trigger": 0, "gate": 1, "toggle": 2, "repeat": 3}
    LAUNCH_QUANTIZATIONS = {
        "global": 0, "none": 1, "8_bars": 2, "4_bars": 3, "2_bars": 4, "1_bar": 5, "bar": 5,
        "1/2": 6, "1/2t": 7, "1/4": 8, "1/4t": 9, "1/8": 10, "1/8t": 11, "1/16": 12, "1/16t": 13, "1/32": 14,
    }

    def _clip_info(self, track, clip, track_index, clip_index, arrangement):
        info = {
            "track_index": int(track_index),
            "clip_index": int(clip_index),
            "arrangement": bool(arrangement),
            "name": clip.name,
            "length": float(clip.length),
            "is_midi_clip": bool(getattr(clip, "is_midi_clip", False)),
            "start_marker": self._safe_attr(clip, "start_marker", float, None),
            "end_marker": self._safe_attr(clip, "end_marker", float, None),
            "launch_quantization": self._safe_attr(clip, "launch_quantization", int, None),
            "legato": self._safe_attr(clip, "legato", bool, None),
            "velocity_amount": self._safe_attr(clip, "velocity_amount", float, None),
        }
        if arrangement:
            info["start_time"] = float(clip.start_time)
            info["end_time"] = float(clip.end_time)
        info.update(self._serialize_clip_common(clip))
        modes = dict((v, k) for k, v in self.LAUNCH_MODES.items())
        if info.get("launch_mode") is not None:
            info["launch_mode_name"] = modes.get(info["launch_mode"])
        quants = dict((v, k) for k, v in self.LAUNCH_QUANTIZATIONS.items() if k != "bar")
        if info.get("launch_quantization") is not None:
            info["launch_quantization_name"] = quants.get(info["launch_quantization"])
        return dict((k, v) for k, v in info.items() if v is not None)

    def _get_clip_info(self, track_index, clip_index, arrangement=False):
        try:
            track, clip = self._clip_at(track_index, clip_index, arrangement)
            return self._clip_info(track, clip, track_index, clip_index, arrangement)
        except Exception as e:
            self.log_message("Error reading clip info: " + str(e))
            raise

    def _set_clip_loop(self, track_index, clip_index, arrangement, params):
        """Loop points and markers, in beats; unspecified values stay."""
        try:
            track, clip = self._clip_at(track_index, clip_index, arrangement)
            looping = params.get("looping")
            if looping is not None:
                clip.looping = bool(looping)
            # Order matters: Live rejects a loop_start beyond the current loop_end.
            ls, le = params.get("loop_start"), params.get("loop_end")
            if ls is not None and le is not None and float(ls) >= float(le):
                raise ValueError("loop_start must be before loop_end")
            if le is not None and (ls is None or float(le) > float(clip.loop_start)):
                clip.loop_end = float(le)
            if ls is not None:
                clip.loop_start = float(ls)
            if le is not None:
                clip.loop_end = float(le)
            sm, em = params.get("start_marker"), params.get("end_marker")
            if em is not None:
                clip.end_marker = float(em)
            if sm is not None:
                clip.start_marker = float(sm)
            return self._clip_info(track, clip, track_index, clip_index, arrangement)
        except Exception as e:
            self.log_message("Error setting clip loop: " + str(e))
            raise

    def _set_clip_launch(self, track_index, clip_index, params):
        """Launch mode, quantization and legato of a Session clip."""
        try:
            track, clip = self._clip_at(track_index, clip_index, False)
            mode = params.get("launch_mode")
            if mode is not None:
                if isinstance(mode, str):
                    key = mode.strip().lower()
                    if key not in self.LAUNCH_MODES:
                        raise ValueError("launch_mode must be one of %s" % ", ".join(sorted(self.LAUNCH_MODES)))
                    mode = self.LAUNCH_MODES[key]
                clip.launch_mode = int(mode)
            quant = params.get("launch_quantization")
            if quant is not None:
                if isinstance(quant, str):
                    key = quant.strip().lower()
                    if key not in self.LAUNCH_QUANTIZATIONS:
                        raise ValueError("launch_quantization must be one of %s" % ", ".join(sorted(self.LAUNCH_QUANTIZATIONS)))
                    quant = self.LAUNCH_QUANTIZATIONS[key]
                clip.launch_quantization = int(quant)
            legato = params.get("legato")
            if legato is not None:
                clip.legato = bool(legato)
            velocity_amount = params.get("velocity_amount")
            if velocity_amount is not None:
                clip.velocity_amount = float(velocity_amount)
            return self._clip_info(track, clip, track_index, clip_index, False)
        except Exception as e:
            self.log_message("Error setting clip launch: " + str(e))
            raise

    def _get_track_meters(self):
        """Output levels right now (0.0-1.0, Live's meter scale), for every track."""
        try:
            def meters(track):
                out = {"name": track.name}
                for attr in ("output_meter_left", "output_meter_right", "output_meter_level"):
                    v = self._safe_attr(track, attr, float, None)
                    if v is not None:
                        out[attr.replace("output_meter_", "")] = v
                try:
                    out["mute"] = bool(track.mute)
                except Exception:
                    pass
                return out
            tracks = []
            for i, t in enumerate(self._song.tracks):
                m = meters(t)
                m["index"] = i
                tracks.append(m)
            returns = []
            for i, t in enumerate(self._song.return_tracks):
                m = meters(t)
                m["index"] = i
                returns.append(m)
            return {
                "is_playing": bool(self._song.is_playing),
                "song_time": float(self._song.current_song_time),
                "tracks": tracks,
                "returns": returns,
                "master": meters(self._song.master_track),
            }
        except Exception as e:
            self.log_message("Error reading meters: " + str(e))
            raise

    def _automation_parameter(self, track, target):
        """Resolve {device_index, parameter_index} or {mixer: volume|panning|send, send_index}."""
        target = target or {}
        mixer = target.get("mixer")
        if mixer:
            mixer = str(mixer).lower()
            md = track.mixer_device
            if mixer == "volume":
                return md.volume, "volume"
            if mixer in ("pan", "panning"):
                return md.panning, "pan"
            if mixer == "send":
                sends = list(md.sends)
                i = int(target.get("send_index", 0))
                if i < 0 or i >= len(sends):
                    raise IndexError("send_index %d out of range" % i)
                return sends[i], "send %d" % i
            raise ValueError("mixer target must be volume, pan or send")
        devices = list(track.devices)
        di = int(target.get("device_index", -1))
        pi = int(target.get("parameter_index", -1))
        if di < 0 or di >= len(devices):
            raise IndexError("device_index %d out of range; track has %d devices" % (di, len(devices)))
        params = list(devices[di].parameters)
        if pi < 0 or pi >= len(params):
            raise IndexError("parameter_index %d out of range; %s has %d parameters" % (pi, devices[di].name, len(params)))
        return params[pi], "%s > %s" % (devices[di].name, params[pi].name)

    def _set_clip_automation(self, track_index, clip_index, arrangement, target, points, mode, resolution, clear):
        """Write an automation envelope into a clip as steps.

        Live's API writes envelopes with insert_step(time, length, value); a
        ramp is approximated by steps of `resolution` beats between points.
        """
        try:
            track, clip = self._clip_at(track_index, clip_index, arrangement)
            parameter, label = self._automation_parameter(track, target)
            pts = []
            for p in points or []:
                pts.append((float(p.get("time", 0.0)), float(p.get("value", 0.0))))
            if not pts:
                raise ValueError("give at least one point {time, value}")
            pts.sort(key=lambda tv: tv[0])
            lo, hi = float(parameter.min), float(parameter.max)
            for _, v in pts:
                if v < lo or v > hi:
                    raise ValueError("value %s is outside %s's range %s-%s" % (v, label, lo, hi))
            resolution = max(0.03125, float(resolution or 0.25))
            if clear:
                try:
                    clip.clear_envelope(parameter)
                except Exception:
                    pass
            envelope = clip.automation_envelope(parameter)
            if envelope is None:
                envelope = clip.create_automation_envelope(parameter)
            if envelope is None:
                raise RuntimeError("Live did not give an envelope for %s (is the parameter automatable?)" % label)
            length = float(clip.length)
            steps = 0
            mode = str(mode or "linear").lower()
            for i, (t, v) in enumerate(pts):
                if i + 1 < len(pts):
                    t_next, v_next = pts[i + 1]
                else:
                    t_next, v_next = max(length, t + resolution), v
                span = max(t_next - t, resolution)
                if mode == "step" or v_next == v:
                    envelope.insert_step(t, span, v)
                    steps += 1
                else:
                    n = max(1, int(round(span / resolution)))
                    for k in range(n):
                        frac = float(k) / n
                        envelope.insert_step(t + span * frac, span / n, v + (v_next - v) * frac)
                        steps += 1
            return {
                "clip": clip.name,
                "target": label,
                "points": len(pts),
                "steps_written": steps,
                "mode": mode,
                "range": [lo, hi],
            }
        except Exception as e:
            self.log_message("Error setting clip automation: " + str(e))
            raise

    def _get_clip_automation(self, track_index, clip_index, arrangement, target, resolution=1.0):
        """Sample an envelope every `resolution` beats (the API has no point list)."""
        try:
            track, clip = self._clip_at(track_index, clip_index, arrangement)
            parameter, label = self._automation_parameter(track, target)
            envelope = clip.automation_envelope(parameter)
            if envelope is None:
                return {"clip": clip.name, "target": label, "has_envelope": False, "samples": []}
            resolution = max(0.0625, float(resolution or 1.0))
            samples = []
            t = 0.0
            length = float(clip.length)
            while t <= length and len(samples) < 512:
                samples.append({"time": t, "value": float(envelope.value_at_time(t))})
                t += resolution
            return {"clip": clip.name, "target": label, "has_envelope": True,
                    "range": [float(parameter.min), float(parameter.max)], "samples": samples}
        except Exception as e:
            self.log_message("Error reading clip automation: " + str(e))
            raise

    # ── Browser implementations ───────────────────────────────────────────────

    def _load_browser_item(self, track_index, item_uri, kind="track"):
        """Load a browser item onto a track (or a return, or the master) by its URI"""
        try:
            track = self._resolve_track(track_index, kind)
            
            # Access the application's browser instance instead of creating a new one
            app = self.application()
            
            # Find the browser item by URI
            item = self._find_browser_item_by_uri(app.browser, item_uri)
            
            if not item:
                raise ValueError("Browser item with URI '{0}' not found".format(item_uri))
            
            # Select the track, and its last device, so an effect is inserted
            # at the end of the chain. An instrument replaces the track's
            # instrument regardless (Live's own rule).
            self._song.view.selected_track = track
            try:
                devices = list(track.devices)
                if devices:
                    self._song.view.select_device(devices[-1])
            except Exception as e:
                self.log_message("could not select the last device: " + str(e))

            before = [d.name for d in track.devices]

            # Load the item
            app.browser.load_item(item)

            after = [d.name for d in track.devices]
            new_devices = []
            for i, name in enumerate(after):
                if i >= len(before) or before[i] != name:
                    new_devices.append({"index": i, "name": name})
            result = {
                "loaded": True,
                "item_name": item.name,
                "track_name": track.name,
                "uri": item_uri,
                "devices_after": after,
                "new_devices": new_devices,
                "loaded_device": new_devices[-1] if new_devices else None,
            }
            return result
        except Exception as e:
            self.log_message("Error loading browser item: {0}".format(str(e)))
            self.log_message(traceback.format_exc())
            raise
    
    # Substring markers that point a URI at a likely root. Unmatched URIs fall
    # back to the default search order.
    _URI_ROOT_HINTS = (
        ('plugins',       ('vst:', 'vst3:', 'au:', 'query:plugins', 'plugin#')),
        ('max_for_live',  ('max for live', 'maxforlive', 'm4l', 'query:max')),
        ('user_library',  ('user library', 'userlibrary', 'query:user library', 'query:user-library')),
        ('packs',         ('query:packs', '/packs/')),
        ('samples',       ('query:samples', 'sample:', '/samples/')),
        ('drums',         ('query:drums', '/drums/')),
        ('instruments',   ('query:instruments', '/instruments/')),
        ('sounds',        ('query:sounds', '/sounds/')),
        ('audio_effects', ('query:audio effects', 'audioeffects', '/audio_effects/')),
        ('midi_effects',  ('query:midi effects', 'midieffects', '/midi_effects/')),
    )

    def _order_roots_by_uri(self, roots, uri):
        """Reorder ``roots`` so the URI's likely root is walked first."""
        if not isinstance(uri, (bytes, str)) or not uri:
            return roots
        lowered = uri.lower()
        for attr, markers in self._URI_ROOT_HINTS:
            if any(m in lowered for m in markers):
                head = [(a, r) for (a, r) in roots if a == attr]
                tail = [(a, r) for (a, r) in roots if a != attr]
                return head + tail
        return roots

    def _find_browser_item_by_uri(self, browser_or_item, uri, max_depth=10, current_depth=0):
        """Find a browser item by its URI.

        Top-level lookups are memoised on ``self._uri_cache`` so repeated
        loads of the same URI don't re-walk the entire browser tree.
        """
        if current_depth == 0:
            cache = getattr(self, '_uri_cache', None)
            if cache is None:
                self._uri_cache = cache = {}
            if uri in cache:
                return cache[uri]
            result = self._walk_browser_for_uri(browser_or_item, uri, max_depth, 0)
            if result is not None:
                cache[uri] = result
            return result
        return self._walk_browser_for_uri(browser_or_item, uri, max_depth, current_depth)

    def _walk_browser_for_uri(self, browser_or_item, uri, max_depth, current_depth):
        """Recursive walk used by :py:meth:`_find_browser_item_by_uri`."""
        try:
            # Check if this is the item we're looking for
            if hasattr(browser_or_item, 'uri') and browser_or_item.uri == uri:
                return browser_or_item

            # Stop recursion if we've reached max depth
            if current_depth >= max_depth:
                return None

            # Check if this is a browser with root categories
            if hasattr(browser_or_item, 'instruments'):
                roots = [
                    ('instruments', browser_or_item.instruments),
                    ('sounds', browser_or_item.sounds),
                    ('drums', browser_or_item.drums),
                    ('audio_effects', browser_or_item.audio_effects),
                    ('midi_effects', browser_or_item.midi_effects),
                ]
                for extra_attr in ('plugins', 'max_for_live', 'user_library', 'packs', 'samples'):
                    if hasattr(browser_or_item, extra_attr):
                        try:
                            roots.append((extra_attr, getattr(browser_or_item, extra_attr)))
                        except (AttributeError, RuntimeError) as e:
                            self.log_message("Could not access browser.{0}: {1}".format(extra_attr, str(e)))

                for _attr, category in self._order_roots_by_uri(roots, uri):
                    item = self._find_browser_item_by_uri(category, uri, max_depth, current_depth + 1)
                    if item:
                        return item

                return None

            # Check if this item has children
            if hasattr(browser_or_item, 'children') and browser_or_item.children:
                for child in browser_or_item.children:
                    item = self._find_browser_item_by_uri(child, uri, max_depth, current_depth + 1)
                    if item:
                        return item

            return None
        except Exception as e:
            self.log_message("Error finding browser item by URI: {0}".format(str(e)))
            return None
    
    # Helper methods

    def _find_blend_parameter(self, device):
        """Find Dry/Wet, Mix, or Amount on a device for Magnitude mapping."""
        preferred = ("Dry/Wet", "Dry Wet", "Mix", "Amount")
        by_name = {}
        for param in device.parameters:
            try:
                by_name[param.name] = param
            except Exception:
                continue
        for name in preferred:
            if name in by_name:
                return by_name[name], name
        # Case-insensitive fallback
        lowered = dict((k.lower(), (v, k)) for k, v in by_name.items())
        for name in preferred:
            hit = lowered.get(name.lower())
            if hit:
                return hit[0], hit[1]
        return None, None

    def _get_device_type(self, device):
        """Get the type of a device"""
        try:
            # Simple heuristic - in a real implementation you'd look at the device class
            if device.can_have_drum_pads:
                return "drum_machine"
            elif device.can_have_chains:
                return "rack"
            elif "instrument" in device.class_display_name.lower():
                return "instrument"
            elif "audio_effect" in device.class_name.lower():
                return "audio_effect"
            elif "midi_effect" in device.class_name.lower():
                return "midi_effect"
            else:
                return "unknown"
        except:
            return "unknown"

    # ── Performance: scenes, quantization, crossfader, recording, cues ──────
    #
    # The performance clock. Claude plans, Live executes: a cue is a list of
    # steps at absolute beats; the script walks them on its own tick
    # (schedule_message, re-armed while work remains), so a cue runs even if
    # the server is slow or gone. Launches are issued inside the bar before
    # their target and Live's quantization lands them on the bar; everything
    # else lands within one tick of its beat.

    # Song.clip_trigger_quantization: 0 None, 1 8 Bars, 2 4 Bars, 3 2 Bars,
    # 4 1 Bar, 5 1/2 ... 13 1/32. Not the per-clip table (LAUNCH_QUANTIZATIONS
    # starts at 0 Global, 1 None).
    GLOBAL_QUANTIZATIONS = {
        "none": 0, "8_bars": 1, "4_bars": 2, "2_bars": 3, "1_bar": 4, "bar": 4,
        "1/2": 5, "1/2t": 6, "1/4": 7, "1/4t": 8, "1/8": 9, "1/8t": 10,
        "1/16": 11, "1/16t": 12, "1/32": 13,
    }
    CROSSFADE_SIDES = {"a": 0, "none": 1, "b": 2}
    PITCH_CLASSES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
    CUE_ACTIONS = ("fire_scene", "fire_clip", "stop_clip", "stop_all_clips",
                   "set", "ramp", "stop_playback", "restore_mix", "ops")
    CUE_TARGETS = ("tempo", "crossfader", "volume", "mute", "send", "device")
    # A launch is issued this many beats before the end of the bar preceding
    # its target, so a 1-bar quantization lands it exactly on the target bar.
    LAUNCH_LEAD_MARGIN = 0.5
    PERF_EVENTS_MAX = 200

    def _beats_per_bar(self):
        try:
            return max(1, int(self._song.signature_numerator))
        except Exception:
            return 4

    def _bar_position(self):
        """(bar, beat_in_bar, beat): Live's own 1-based bar numbers."""
        song = self._song
        beat = float(song.current_song_time)
        bpb = self._beats_per_bar()
        try:
            bt = song.get_current_beats_song_time()
            return int(bt.bars), int(bt.beats), beat
        except Exception:
            return int(beat // bpb) + 1, int(beat % bpb) + 1, beat

    def _global_quantization_name(self, value):
        for name, v in self.GLOBAL_QUANTIZATIONS.items():
            if v == value and name != "bar":
                return name
        return str(value)

    def _clip_quantization_name(self, value):
        for name, v in self.LAUNCH_QUANTIZATIONS.items():
            if v == value and name != "bar":
                return name
        return str(value)

    def _perf_event(self, kind, detail=None):
        evt = {"type": kind, "ts": time.time(),
               "beat": self._safe_song_property("current_song_time", float, 0.0)}
        if detail:
            evt.update(detail)
        with self._cue_lock:
            self._perf_events.append(evt)
            if len(self._perf_events) > self.PERF_EVENTS_MAX:
                self._perf_events = self._perf_events[-self.PERF_EVENTS_MAX:]

    def _get_performance_state(self):
        """Where the set is, what plays, what is queued, the pending cues, and
        everything the clock did since the last call."""
        song = self._song
        self._sync_scene_phrases()
        bar, beat_in_bar, beat = self._bar_position()
        tracks = []
        for i, t in enumerate(song.tracks):
            playing = self._safe_attr(t, "playing_slot_index", int, -1)
            fired = self._safe_attr(t, "fired_slot_index", int, -1)
            entry = {"index": i, "name": str(t.name), "playing_slot_index": playing,
                     "fired_slot_index": fired, "arm": self._safe_arm(t),
                     "is_recording": False, "playing_clip_name": None,
                     "fired_clip_name": None, "slots_with_clips": [], "no_stop_slots": []}
            try:
                for si, s in enumerate(t.clip_slots):
                    if not s.has_clip:
                        if not bool(self._safe_attr(s, "has_stop_button", bool, True)):
                            entry["no_stop_slots"].append(si)
                        continue
                    entry["slots_with_clips"].append(si)
                    clip = s.clip
                    if si == playing:
                        entry["playing_clip_name"] = str(clip.name)
                    if si == fired:
                        entry["fired_clip_name"] = str(clip.name)
                    if bool(getattr(clip, "is_recording", False)):
                        entry["is_recording"] = True
            except Exception:
                pass
            tracks.append(entry)
        scenes = []
        for i, sc in enumerate(song.scenes):
            name = "%s" % sc.name
            section, _ = self._parse_section_name(name)
            scenes.append({
                "index": i, "name": name, "section": section,
                "tempo": self._safe_attr(sc, "tempo", float, None),
                "is_triggered": bool(self._safe_attr(sc, "is_triggered", bool, False)),
                "is_playing": any(t["playing_slot_index"] == i for t in tracks),
                "clip_tracks": [t["index"] for t in tracks if i in t["slots_with_clips"]],
                "phrase_bars": self._scene_phrase.get(i, 16),
                "phrase_default": i not in self._scene_phrase,
                "started_bar": self._scene_started.get(i),
            })
        quant = self._safe_song_property("clip_trigger_quantization", int, 4)
        root = self._safe_attr(song, "root_note", int, None)
        with self._cue_lock:
            cues = [self._public_cue(c) for c in self._cues]
            events = list(self._perf_events)
            self._perf_events = []
        pending = None
        if self._pending_record is not None:
            pending = dict((k, v) for k, v in self._pending_record.items() if k != "started")
        return {
            "is_playing": bool(song.is_playing),
            "tempo": float(song.tempo),
            "signature_numerator": int(song.signature_numerator),
            "signature_denominator": int(song.signature_denominator),
            "beat": beat, "bar": bar, "beat_in_bar": beat_in_bar,
            "clip_trigger_quantization": quant,
            "clip_trigger_quantization_name": self._global_quantization_name(quant),
            "scale_mode": self._safe_attr(song, "scale_mode", bool, None),
            "scale_name": self._safe_attr(song, "scale_name", str, None),
            "root_note": root,
            "root_note_name": self.PITCH_CLASSES[root % 12] if root is not None else None,
            "tracks": tracks,
            "scenes": scenes,
            "cues": cues,
            "pending_record": pending,
            "events": events,
            "performance_mode": bool(self._performance_mode),
            "phrase": self._phrase_info(bar),
            "current_scene": self._current_scene,
            "levels": self._levels(),
        }

    # ── Performance mode, the clock, phrases ────────────────────────────────

    def _set_performance_mode(self, on=True):
        self._performance_mode = bool(on)
        if self._performance_mode:
            self._arm_perf_tick()
        return {"performance_mode": self._performance_mode}

    def _landing(self):
        """The bar a launch issued now lands on, read from the transport after
        the fire: the next point of the global quantization grid."""
        song = self._song
        beat = float(song.current_song_time)
        bpb = self._beats_per_bar()
        q = self._safe_song_property("clip_trigger_quantization", int, 4)
        bar_now = int(beat // bpb) + 1
        if q == 0 or q >= 5 or not bool(song.is_playing):
            lands = bar_now
        else:
            grid = {1: 8, 2: 4, 3: 2, 4: 1}.get(q, 1)
            lands = ((bar_now - 1) // grid + 1) * grid + 1
        return {"lands_on_bar": lands, "issued_at_beat": beat, "issued_at_bar": bar_now,
                "issued_at_beat_in_bar": int(beat % bpb) + 1}

    def _note_scene_fired(self, index, lands_on_bar):
        self._current_scene = int(index)
        self._scene_started[int(index)] = int(lands_on_bar)

    def _watch_scene_rows(self):
        """A scene the producer fired from Live's own UI: the row most tracks
        play becomes the current scene, counted from the bar it was seen."""
        rows = {}
        try:
            for t in self._song.tracks:
                i = self._safe_attr(t, "playing_slot_index", int, -1)
                if i >= 0:
                    rows[i] = rows.get(i, 0) + 1
        except Exception:
            return
        if not rows:
            return
        row = max(rows.items(), key=lambda kv: kv[1])[0]
        if row != self._current_scene:
            bar, _, _ = self._bar_position()
            self._current_scene = row
            started = self._scene_started.get(row)
            if started is None or started > bar or started < bar - 1:
                self._scene_started[row] = bar

    def _phrase_info(self, bar=None):
        idx = self._current_scene
        if idx is None or idx not in self._scene_started:
            return None
        bars = int(self._scene_phrase.get(idx, 16))
        started = int(self._scene_started[idx])
        if bar is None:
            bar, _, _ = self._bar_position()
        if started > bar:
            # Fired while the transport was stopped: Live restarted from the
            # start marker, so count phrases from the bar it actually began.
            started = 1 + ((bar - 1) // bars) * bars
            self._scene_started[idx] = started
        k = max(1, (bar - started) // bars + 1)
        return {"scene_index": idx, "started_bar": started, "bars": bars,
                "ends_bar": started + k * bars, "default": idx not in self._scene_phrase}

    def _next_cue_step(self):
        best = None
        with self._cue_lock:
            for c in self._cues:
                for st in c["steps"]:
                    if st["done"]:
                        continue
                    if best is None or st["beat"] < best[1]["beat"]:
                        best = (c, st)
        if best is None:
            return None
        c, st = best
        return {"cue_id": c["id"], "bar": st.get("bar"), "beat": st["beat"],
                "label": st.get("label") or st["action"]}

    def _clock(self):
        song = self._song
        self._sync_scene_phrases()
        bar, bib, beat = self._bar_position()
        bpb = self._beats_per_bar()
        tempo = float(song.tempo)
        return {"beat": beat, "bar": bar, "beat_in_bar": bib, "tempo": tempo,
                "beats_per_bar": bpb, "is_playing": bool(song.is_playing),
                "seconds_to_next_bar": max(0.0, (bar * bpb - beat) * 60.0 / max(1.0, tempo)),
                "phrase": self._phrase_info(bar), "next_cue": self._next_cue_step(),
                "pending_cues": len(self._cues), "levels": self._levels()}

    # ── Sections: the scene-name convention ─────────────────────────────────
    #
    # A section is a scene row named "<name> · <bars>"; the suffix is its
    # phrase length. The name is the memory: Live's own Save keeps it, and
    # every state read seeds the phrase table from the names, so nothing is
    # lost when Live restarts. The "Setlist:" scene holds the song and is
    # never parsed for a phrase.

    SECTION_SEP = " \u00b7 "
    SETLIST_PREFIX = "Setlist:"

    def _parse_section_name(self, name):
        """'Groove · 8' -> ('Groove', 8); 'Groove' -> ('Groove', None)."""
        text = ("%s" % (name if name is not None else "")).strip()
        if text.startswith(self.SETLIST_PREFIX):
            return text, None
        sep = self.SECTION_SEP
        if sep in text:
            head, tail = text.rsplit(sep, 1)
            tail = tail.strip()
            if tail.isdigit() and head.strip():
                return head.strip(), int(tail)
        return text, None

    def _section_name(self, base, bars):
        base = ("%s" % base).strip()
        if bars is None:
            return base
        return base + self.SECTION_SEP + ("%d" % int(bars))

    def _clamp_phrase(self, bars):
        pb = int(bars)
        if pb < 1 or pb > 128:
            raise ValueError("phrase_bars must be between 1 and 128")
        return pb

    def _sync_scene_phrases(self):
        """Rebuild the phrase table from the scene names (the names win)."""
        try:
            table = {}
            for i, sc in enumerate(self._song.scenes):
                _, bars = self._parse_section_name(sc.name)
                if bars is not None:
                    table[i] = max(1, min(128, bars))
            self._scene_phrase = table
        except Exception:
            pass

    def _shift_scene_tables(self, new_index):
        """Rows at or after new_index moved down by one."""
        shift = lambda d: dict((k + 1 if k >= new_index else k, v) for k, v in d.items())
        self._scene_phrase = shift(self._scene_phrase)
        self._scene_started = shift(self._scene_started)
        self._section_peaks = shift(self._section_peaks)
        if self._current_scene is not None and self._current_scene >= new_index:
            self._current_scene += 1

    def _scene_result(self, i, scene, extra=None):
        base, bars = self._parse_section_name(scene.name)
        out = {"index": i, "name": "%s" % scene.name, "section": base,
               "tempo": self._safe_attr(scene, "tempo", float, None),
               "phrase_bars": self._scene_phrase.get(i, 16),
               "phrase_default": i not in self._scene_phrase,
               "scene_count": len(self._song.scenes)}
        if extra:
            out.update(extra)
        return out

    def _set_scene(self, index, name=None, tempo=None, phrase_bars=None):
        try:
            i, scene = self._resolve_scene(index)
            if name is not None or phrase_bars is not None:
                current_base, current_bars = self._parse_section_name(scene.name)
                if name is not None and ("%s" % name).strip().startswith(self.SETLIST_PREFIX):
                    scene.name = ("%s" % name).strip()
                else:
                    if name is not None:
                        base, named_bars = self._parse_section_name(name)
                    else:
                        base, named_bars = current_base, None
                    if phrase_bars is not None:
                        bars = self._clamp_phrase(phrase_bars)
                    elif named_bars is not None:
                        bars = named_bars
                    else:
                        bars = current_bars  # a plain rename keeps the phrase
                    scene.name = self._section_name(base, bars)
            if tempo is not None:
                try:
                    scene.tempo = float(tempo)
                except Exception as e:
                    raise ValueError("a scene tempo needs Live 11 or newer: " + str(e))
            self._sync_scene_phrases()
            return self._scene_result(i, scene)
        except Exception as e:
            self.log_message("Error setting scene: " + str(e))
            raise

    def _capture_scene(self, name=None, phrase_bars=None, after=None):
        """Live's own Capture and Insert Scene: the playing clips are copied
        into a new row below `after` (default: the current row) and Live
        launches that row with no audible interruption. The phrase count and
        the section peak carry over to the copy."""
        try:
            song = self._song
            scenes = list(song.scenes)
            if not scenes:
                raise ValueError("the set has no scenes")
            if after is None:
                after = self._current_scene if self._current_scene is not None else len(scenes) - 1
            after = int(after)
            if after < 0 or after >= len(scenes):
                raise IndexError("scene index %d out of range (0-%d)" % (after, len(scenes) - 1))
            song.view.selected_scene = scenes[after]
            song.capture_and_insert_scene()
            new_index = after + 1
            scenes = list(song.scenes)
            if len(scenes) <= new_index:
                raise RuntimeError("Live did not insert a scene")
            scene = scenes[new_index]
            was_current = self._current_scene == after
            self._shift_scene_tables(new_index)
            if was_current:
                self._current_scene = new_index
                if after in self._scene_started:
                    self._scene_started[new_index] = self._scene_started[after]
                if after in self._section_peaks:
                    self._section_peaks[new_index] = self._section_peaks[after]
            if name:
                base, bars = self._parse_section_name(name)
            else:
                base, bars = self._parse_section_name(scene.name)
            if phrase_bars is not None:
                bars = self._clamp_phrase(phrase_bars)
            elif bars is None:
                bars = self._scene_phrase.get(after)
            scene.name = self._section_name(base, bars)
            self._sync_scene_phrases()
            clips, _, _ = self._scene_row(new_index)
            return self._scene_result(new_index, scene, {"clips": clips, "launched": True,
                                                          "captured_from": after})
        except Exception as e:
            self.log_message("Error capturing scene: " + str(e))
            raise

    def _duplicate_scene(self, index, name=None, phrase_bars=None):
        """A copy of a row below the original, named as a section."""
        try:
            song = self._song
            i, source = self._resolve_scene(index)
            song.duplicate_scene(i)
            new_index = i + 1
            scenes = list(song.scenes)
            if len(scenes) <= new_index:
                raise RuntimeError("Live did not duplicate the scene")
            scene = scenes[new_index]
            self._shift_scene_tables(new_index)
            source_base, source_bars = self._parse_section_name(source.name)
            if name:
                base, bars = self._parse_section_name(name)
            else:
                base, bars = source_base + " copy", None
            if phrase_bars is not None:
                bars = self._clamp_phrase(phrase_bars)
            elif bars is None:
                bars = source_bars
            scene.name = self._section_name(base, bars)
            self._sync_scene_phrases()
            clips, _, _ = self._scene_row(new_index)
            return self._scene_result(new_index, scene, {"clips": clips, "source_index": i})
        except Exception as e:
            self.log_message("Error duplicating scene: " + str(e))
            raise

    # ── The Groove Pool (Live 11+): assign a pool groove to a clip ──────────

    GROOVE_AMOUNTS = ("timing_amount", "random_amount", "velocity_amount", "quantization_amount")

    def _groove_pool(self):
        pool = getattr(self._song, "groove_pool", None)
        if pool is None:
            raise ValueError("this Live has no Groove Pool in its API (Live 11 or newer)")
        return pool

    def _groove_entry(self, i, g):
        entry = {"index": i, "name": "%s" % getattr(g, "name", "groove %d" % i)}
        base = self._safe_attr(g, "base", int, None)
        if base is not None:
            entry["base"] = base
        for attr in self.GROOVE_AMOUNTS:
            v = self._safe_attr(g, attr, float, None)
            if v is not None:
                entry[attr] = v
        return entry

    def _get_grooves(self):
        """The set's Groove Pool: every groove with its amounts, the global
        amount, and whether the API offers any way to add one (it does not
        on Live 12.4; the reply says what to drag)."""
        try:
            song = self._song
            out = {"groove_amount": self._safe_attr(song, "groove_amount", float, None),
                   "grooves": [], "can_add": False, "pool_functions": []}
            try:
                pool = self._groove_pool()
            except ValueError as e:
                out["error"] = str(e)
                return out
            grooves = list(getattr(pool, "grooves", []) or [])
            out["grooves"] = [self._groove_entry(i, g) for i, g in enumerate(grooves)]
            try:
                names = [n for n in dir(pool) if not n.startswith("_")]
                out["pool_functions"] = [n for n in names if callable(getattr(pool, n, None))]
                # Live 12.4.6 offers only listeners here (add_grooves_listener):
                # there is no way to add a groove to the pool through the API.
                out["can_add"] = any(("add" in n.lower() or "create" in n.lower()) and "listener" not in n.lower()
                                     for n in out["pool_functions"])
            except Exception:
                pass
            return out
        except Exception as e:
            self.log_message("Error reading the groove pool: " + str(e))
            raise

    def _set_clip_groove(self, track_index=None, clip_index=None, groove_index=None,
                         timing=None, random=None, velocity=None, global_amount=None):
        """Assign a pool groove to a Session clip (groove_index -1 or None
        with a clip given removes it), set that groove's amounts, and/or the
        set's global groove amount."""
        try:
            song = self._song
            out = {}
            if global_amount is not None:
                v = max(0.0, min(1.0, float(global_amount)))
                song.groove_amount = v
                out["groove_amount"] = float(song.groove_amount)
            if track_index is not None and clip_index is not None:
                track = self._resolve_track(int(track_index))
                slots = list(track.clip_slots)
                ci = int(clip_index)
                if ci < 0 or ci >= len(slots) or not slots[ci].has_clip:
                    raise ValueError("slot %d on '%s' holds no clip" % (ci, track.name))
                clip = slots[ci].clip
                if not hasattr(clip, "groove"):
                    raise ValueError("this Live cannot assign a groove to a clip through the API (Live 11 or newer)")
                out["track"] = "%s" % track.name
                out["clip"] = "%s" % clip.name
                if groove_index is None or int(groove_index) < 0:
                    clip.groove = None
                    out["groove"] = None
                else:
                    grooves = list(getattr(self._groove_pool(), "grooves", []) or [])
                    gi = int(groove_index)
                    if gi >= len(grooves):
                        raise IndexError("groove index %d out of range (the pool has %d)" % (gi, len(grooves)))
                    g = grooves[gi]
                    clip.groove = g
                    for attr, value in (("timing_amount", timing), ("random_amount", random),
                                        ("velocity_amount", velocity)):
                        if value is not None:
                            try:
                                setattr(g, attr, max(0.0, min(1.0, float(value))))
                            except Exception as e:
                                out.setdefault("not_set", []).append("%s: %s" % (attr, str(e)))
                    out["groove"] = self._groove_entry(gi, g)
            if not out:
                raise ValueError("give a clip and a groove, or global_amount")
            return out
        except Exception as e:
            self.log_message("Error setting the clip groove: " + str(e))
            raise

    # ── Levels: meter peaks per bar and per section, kept by the tick ───────

    def _db_for(self, param, value):
        """Live's own dB reading of a fader-scale value; -inf becomes -80."""
        text = self._param_display(param, value)
        if text is None:
            return None
        text = text.replace("dB", "").replace("db", "").strip()
        if "inf" in text:
            return -80.0
        try:
            return float(text)
        except ValueError:
            return None

    # Live's output meters are linear in dB, not in amplitude and not on the
    # fader taper: 0.0 reads -70 dB and 1.0 reads +6 dB. Measured on Live
    # 12.4.6 against a file of known level, seven fader positions over 42 dB
    # (-6 to -48 dBFS): a straight-line fit gave dB = 76 * value - 70 with
    # zero residuals. 20*log10 of a meter value is not dB, which is why a
    # 12 dB fader move used to read as about 2 dB.
    METER_FLOOR_DB = -70.0
    METER_TOP_DB = 6.0

    def _meter_points(self):
        """The curve as two points; it is a straight line in dB."""
        return [[0.0, self.METER_FLOOR_DB], [1.0, self.METER_TOP_DB]]

    def _get_meter_scale(self):
        """The curve the server converts meter readings with."""
        span = self.METER_TOP_DB - self.METER_FLOOR_DB
        return {"points": self._meter_points(),
                "source": "measured on Live 12.4.6: dB = %.0f * value %+.0f" % (
                    span, self.METER_FLOOR_DB),
                "reference": "post_fader",
                "floor_db": self.METER_FLOOR_DB, "top_db": self.METER_TOP_DB,
                "zero_dbfs": round(-self.METER_FLOOR_DB / span, 4)}

    def _db(self, v):
        """A meter reading (Live's 0-1 meter units) as dB. Silence is -80."""
        v = float(v)
        if v <= 0.0:
            return -80.0
        span = self.METER_TOP_DB - self.METER_FLOOR_DB
        return round(max(-80.0, min(self.METER_TOP_DB, span * v + self.METER_FLOOR_DB)), 1)

    def _meter_of(self, track):
        best = 0.0
        for attr in ("output_meter_left", "output_meter_right", "output_meter_level"):
            v = self._safe_attr(track, attr, float, None)
            if v is not None and v > best:
                best = v
        return best

    def _level_tick(self, bar):
        try:
            song = self._song
            master = self._meter_of(song.master_track)
            tracks = [self._meter_of(t) for t in song.tracks]
            cur = self._bar_peaks
            if cur is None or cur["bar"] != bar:
                if cur is not None:
                    self._last_bar_peaks = cur
                cur = {"bar": bar, "master": 0.0, "tracks": [0.0] * len(tracks)}
                self._bar_peaks = cur
            if len(cur["tracks"]) != len(tracks):
                # tracks were added or removed: the old peaks name the wrong rows
                cur["tracks"] = [0.0] * len(tracks)
                self._last_bar_peaks = None
            cur["master"] = max(cur["master"], master)
            cur["tracks"] = [max(a, b) for a, b in zip(cur["tracks"], tracks)]
            idx = self._current_scene
            if idx is not None:
                self._section_peaks[idx] = max(self._section_peaks.get(idx, 0.0), master)
        except Exception as e:
            self.log_message("level tick error: " + str(e))

    def _levels(self):
        """The loudest the master and each track got over this bar and the
        last one, in dB from Live's 0-1 meters, plus the peak of every
        section row seen so far."""
        cur = self._bar_peaks
        if cur is None:
            return None
        last = self._last_bar_peaks or {"master": 0.0, "tracks": []}
        master = max(cur["master"], last.get("master", 0.0))
        tracks = []
        try:
            names = ["%s" % t.name for t in self._song.tracks]
        except Exception:
            names = []
        for i, v in enumerate(cur["tracks"]):
            prev = last["tracks"][i] if i < len(last.get("tracks", [])) else 0.0
            tracks.append({"index": i, "name": names[i] if i < len(names) else str(i),
                           "peak_db": self._db(max(v, prev))})
        return {"bar": cur["bar"], "master_peak_db": self._db(master), "tracks": tracks,
                "section_peaks": dict(("%d" % k, self._db(v)) for k, v in self._section_peaks.items())}

    # ── Listening without stopping: a fixed-length capture on the bar ───────

    def _start_live_capture(self, bars, name=None):
        """Record `bars` bars of the master into the Capture track's next free
        slot, fired on the global quantization while the transport keeps
        running. The transport is never stopped or moved."""
        try:
            index, track = self._find_capture_track()
            if track is None:
                self._ensure_capture_track()
                index, track = self._find_capture_track()
            rec_index, _ = self._recording_capture(track)
            if rec_index is not None:
                raise ValueError("a capture is already recording in slot %d" % rec_index)
            bars = int(bars)
            if bars < 1 or bars > 16:
                raise ValueError("bars must be between 1 and 16 for a live capture")
            slot_index = None
            for i, slot in enumerate(track.clip_slots):
                if not slot.has_clip:
                    slot_index = i
                    break
            if slot_index is None:
                raise ValueError("every slot of the Capture track holds a capture; delete one first")
            if Live is None:
                raise RuntimeError("captures need Live's own Python (Live 11 or newer)")
            bpb = self._beats_per_bar()
            record_length = float(bars * bpb)
            slot = track.clip_slots[slot_index]
            try:
                slot.fire(record_length=record_length)
            except TypeError as e:
                raise RuntimeError("fixed-length recording needs Live 11 or newer: " + str(e))
            landing = self._landing()
            start_beat = float((landing["lands_on_bar"] - 1) * bpb)
            self._pending_capture = {"slot": slot_index, "name": str(name or "listen"),
                                     "start": start_beat, "bars": bars}
            out = {"slot": slot_index, "track_index": index, "record_length": record_length,
                   "bars": bars, "tempo": float(self._song.tempo), "started_at": start_beat}
            out.update(landing)
            return out
        except Exception as e:
            self.log_message("Error starting live capture: " + str(e))
            raise

    # ── Mix snapshots ───────────────────────────────────────────────────────

    def _mix_state_of(self, track, has_sends=True, has_mute=True):
        st = {"volume": float(track.mixer_device.volume.value),
              "panning": float(track.mixer_device.panning.value)}
        if has_sends:
            try:
                st["sends"] = [float(x.value) for x in track.mixer_device.sends]
            except Exception:
                st["sends"] = []
        if has_mute:
            try:
                st["mute"] = bool(track.mute)
            except Exception:
                pass
        return st

    def _snapshot_mix(self):
        try:
            song = self._song
            snap = {"beat": float(song.current_song_time),
                    "tracks": [self._mix_state_of(t) for t in song.tracks],
                    "returns": [self._mix_state_of(t) for t in song.return_tracks],
                    "master": self._mix_state_of(song.master_track, False, False)}
            sid = self._snapshot_next_id
            self._snapshot_next_id += 1
            self._mix_snapshots[sid] = snap
            if len(self._mix_snapshots) > 16:
                oldest = min(self._mix_snapshots.keys())
                del self._mix_snapshots[oldest]
            return {"id": sid, "tracks": len(snap["tracks"]), "returns": len(snap["returns"]),
                    "beat": snap["beat"]}
        except Exception as e:
            self.log_message("Error taking mix snapshot: " + str(e))
            raise

    def _apply_mix_state(self, track, st):
        track.mixer_device.volume.value = float(st["volume"])
        track.mixer_device.panning.value = float(st["panning"])
        for i, v in enumerate(st.get("sends", [])):
            try:
                track.mixer_device.sends[i].value = float(v)
            except Exception:
                pass
        if "mute" in st:
            try:
                track.mute = bool(st["mute"])
            except Exception:
                pass

    def _restore_mix(self, snapshot_id):
        try:
            snap = self._mix_snapshots.get(int(snapshot_id if snapshot_id is not None else -1))
            if snap is None:
                raise ValueError("no mix snapshot %s (snapshot_mix first)" % snapshot_id)
            song = self._song
            n = 0
            for t, st in zip(list(song.tracks), snap["tracks"]):
                self._apply_mix_state(t, st)
                n += 1
            for t, st in zip(list(song.return_tracks), snap["returns"]):
                self._apply_mix_state(t, st)
                n += 1
            self._apply_mix_state(song.master_track, snap["master"])
            return {"id": int(snapshot_id), "restored": n + 1}
        except Exception as e:
            self.log_message("Error restoring mix snapshot: " + str(e))
            raise

    # ── The browser index, paged for the server's own copy ──────────────────

    def _library_key(self):
        if self._library_key_cache is not None:
            return self._library_key_cache
        try:
            app = self.application()
            version = "%d.%d.%d" % (app.get_major_version(), app.get_minor_version(), app.get_bugfix_version())
        except Exception:
            version = "unknown"
        packs = []
        try:
            root = getattr(self.application().browser, "packs", None)
            if root is not None:
                packs = sorted(str(getattr(c, "name", "")) for c in root.children)
        except Exception:
            pass
        digest = 0
        for name in packs:
            for ch in name:
                digest = (digest * 31 + ord(ch)) % 4294967291
        self._library_key_cache = "live-%s-packs-%d-%08x" % (version, len(packs), digest)
        return self._library_key_cache

    def _get_browser_index(self, category="all", offset=0, limit=500, budget_s=1.0):
        """A page of the walked browser index; the walk resumes across calls
        and never holds the socket longer than budget_s."""
        try:
            wanted = str(category or "all").lower()
            budget = max(0.1, min(float(budget_s or 1.0), 8.0))
            items, complete = None, False
            for step in self._browser_index(wanted, budget):
                if isinstance(step, Done):
                    items, complete = step.result
                    break
                yield None
            offset = max(0, int(offset or 0))
            limit = max(1, min(int(limit or 500), 2000))
            page = items[offset:offset + limit]
            yield Done({"category": wanted, "offset": offset, "returned": len(page),
                        "total_walked": len(items), "index_complete": bool(complete),
                        "library_key": self._library_key(), "items": page})
        except Exception as e:
            self.log_message("Error paging the browser index: " + str(e))
            raise

    def _public_step(self, s):
        out = {"index": s["index"], "action": s["action"], "beat": s["beat"],
               "bar": s.get("bar"), "label": s.get("label"), "done": bool(s["done"])}
        if s["action"] == "ramp":
            out["end_beat"] = s.get("end_beat")
            out["to"] = s.get("to")
            if s.get("to_db") is not None:
                out["to_db"] = s.get("to_db")
        return out

    def _public_cue(self, c):
        return {"id": c["id"], "name": c["name"],
                "steps": [self._public_step(s) for s in c["steps"]],
                "pending": len([s for s in c["steps"] if not s["done"]])}

    def _set_launch_quantization(self, name):
        key = str(name or "").lower().strip()
        if key not in self.GLOBAL_QUANTIZATIONS:
            raise ValueError("launch quantization must be one of %s" % ", ".join(
                sorted(k for k in self.GLOBAL_QUANTIZATIONS if k != "bar")))
        self._song.clip_trigger_quantization = self.GLOBAL_QUANTIZATIONS[key]
        value = int(self._song.clip_trigger_quantization)
        return {"clip_trigger_quantization": value,
                "name": self._global_quantization_name(value)}

    def _resolve_scene(self, scene_index):
        scenes = list(self._song.scenes)
        i = int(scene_index)
        if i < 0 or i >= len(scenes):
            raise IndexError("scene index %d out of range (0-%d)" % (i, len(scenes) - 1))
        return i, scenes[i]

    def _create_scene(self, index=-1, name=None, tempo=None, phrase_bars=None):
        try:
            song = self._song
            count = len(song.scenes)
            index = int(index if index is not None else -1)
            if index < -1 or index > count:
                raise IndexError("scene index %d out of range (-1 for the end, else 0-%d)" % (index, count))
            song.create_scene(index)
            new_index = count if index == -1 else index
            scene = song.scenes[new_index]
            if index != -1:
                self._shift_scene_tables(new_index)
            if name or phrase_bars is not None:
                if name and ("%s" % name).strip().startswith(self.SETLIST_PREFIX):
                    scene.name = ("%s" % name).strip()
                else:
                    base, bars = self._parse_section_name(name) if name else ("", None)
                    if phrase_bars is not None:
                        bars = self._clamp_phrase(phrase_bars)
                    scene.name = self._section_name(base, bars)
            if tempo is not None:
                try:
                    scene.tempo = float(tempo)
                except Exception as e:
                    raise ValueError("a scene tempo needs Live 11 or newer: " + str(e))
            self._sync_scene_phrases()
            return self._scene_result(new_index, scene)
        except Exception as e:
            self.log_message("Error creating scene: " + str(e))
            raise

    def _scene_row(self, scene_index):
        """What firing a scene does: its clips, the clips whose own launch
        quantization is finer than a bar, and the armed tracks whose slot is
        empty (Live records into those when Start Recording on Scene Launch
        is on)."""
        clips, off_grid, would_record = [], [], []
        for ti, t in enumerate(self._song.tracks):
            try:
                slots = list(t.clip_slots)
                if scene_index >= len(slots):
                    continue
                s = slots[scene_index]
                if s.has_clip:
                    c = s.clip
                    clips.append({"track_index": ti, "track": str(t.name), "name": str(c.name)})
                    q = self._safe_attr(c, "launch_quantization", int, 0)
                    if q == 1 or q >= 6:
                        off_grid.append({"track": str(t.name), "name": str(c.name),
                                         "launch_quantization": self._clip_quantization_name(q)})
                elif self._safe_arm(t):
                    would_record.append(str(t.name))
            except Exception:
                continue
        return clips, off_grid, would_record

    def _fire_scene(self, scene_index):
        try:
            i, scene = self._resolve_scene(scene_index)
            clips, off_grid, would_record = self._scene_row(i)
            scene.fire()
            landing = self._landing()
            self._note_scene_fired(i, landing["lands_on_bar"])
            quant = self._safe_song_property("clip_trigger_quantization", int, 4)
            out = {"fired": True, "scene_index": i, "name": str(scene.name),
                   "clips": clips, "off_grid_clips": off_grid,
                   "would_record": would_record,
                   "clip_trigger_quantization": quant,
                   "clip_trigger_quantization_name": self._global_quantization_name(quant)}
            out.update(landing)
            return out
        except Exception as e:
            self.log_message("Error firing scene: " + str(e))
            raise

    def _stop_all_clips(self):
        self._song.stop_all_clips()
        return {"stopped": True}

    def _set_crossfader(self, value=None, assign=None):
        try:
            song = self._song
            out = {}
            if value is not None:
                v = float(value)
                if v < 0.0 or v > 1.0:
                    raise ValueError("crossfader must be between 0.0 (A) and 1.0 (B)")
                p = song.master_track.mixer_device.crossfader
                p.value = float(p.min) + (float(p.max) - float(p.min)) * v
                out["crossfader"] = v
            assigned = []
            for a in (assign or []):
                t = self._resolve_track(a.get("track_index", 0))
                side = str(a.get("side", "none")).lower()
                if side not in self.CROSSFADE_SIDES:
                    raise ValueError("side must be A, B or none")
                t.mixer_device.crossfade_assign = self.CROSSFADE_SIDES[side]
                assigned.append({"track_index": int(a.get("track_index", 0)), "track": str(t.name),
                                 "side": side.upper() if side != "none" else "none"})
            if assigned:
                out["assigned"] = assigned
            if not out:
                raise ValueError("give value and/or assign")
            return out
        except Exception as e:
            self.log_message("Error setting crossfader: " + str(e))
            raise

    def _record_clip(self, track_index, bars, name=None):
        """Arm the track and fire its first empty slot as a fixed-length
        recording on the global quantization; the tick names the clip and
        disarms the track when the recording ends."""
        try:
            song = self._song
            track = self._resolve_track(track_index)
            if not getattr(track, "can_be_armed", False):
                raise ValueError("track '%s' cannot be armed (group, return or master)" % track.name)
            bars = int(bars)
            if bars < 1 or bars > 64:
                raise ValueError("bars must be between 1 and 64")
            for s in track.clip_slots:
                if s.has_clip and bool(getattr(s.clip, "is_recording", False)):
                    raise ValueError("track '%s' is already recording" % track.name)
            pr = self._pending_record
            if pr is not None and pr.get("track_index") == int(track_index):
                raise ValueError("a recording is already pending on track '%s'" % track.name)
            slot_index = None
            for i, s in enumerate(track.clip_slots):
                if not s.has_clip:
                    slot_index = i
                    break
            if slot_index is None:
                raise ValueError("every slot on '%s' holds a clip; create_scene adds a row" % track.name)
            if Live is None:
                raise RuntimeError("recording a clip needs Live's own Python (Live 11 or newer)")
            record_length = float(bars * self._beats_per_bar())
            track.arm = True
            slot = track.clip_slots[slot_index]
            try:
                slot.fire(record_length=record_length)
            except TypeError as e:
                track.arm = False
                raise RuntimeError("fixed-length recording needs Live 11 or newer: " + str(e))
            landing = self._landing()
            self._pending_record = {"track_index": int(track_index), "slot": slot_index,
                                    "name": str(name or "take"), "record_length": record_length,
                                    "seen_recording": False, "started": time.time()}
            self._arm_perf_tick()
            quant = self._safe_song_property("clip_trigger_quantization", int, 4)
            out = {"track_index": int(track_index), "track": str(track.name), "slot": slot_index,
                   "record_length": record_length, "bars": bars,
                   "clip_trigger_quantization": quant,
                   "clip_trigger_quantization_name": self._global_quantization_name(quant)}
            out.update(landing)
            return out
        except Exception as e:
            self.log_message("Error recording clip: " + str(e))
            raise

    def _set_scale(self, root_note=None, scale_name=None):
        """Live 12's scale settings (root_note 0-11, scale_name e.g. 'Minor');
        turns scale mode on. Errors on older Lives, which have no scale."""
        try:
            song = self._song
            if not hasattr(song, "scale_name"):
                raise ValueError("this Live has no scale setting (Live 12 or newer)")
            out = {}
            if root_note is not None:
                song.root_note = int(root_note) % 12
                out["root_note"] = int(song.root_note)
                out["root_note_name"] = self.PITCH_CLASSES[int(song.root_note) % 12]
            if scale_name is not None:
                try:
                    song.scale_name = str(scale_name)
                except Exception as e:
                    raise ValueError("Live does not know the scale '%s': %s" % (scale_name, str(e)))
                out["scale_name"] = str(song.scale_name)
            try:
                song.scale_mode = True
                out["scale_mode"] = True
            except Exception:
                pass
            return out
        except Exception as e:
            self.log_message("Error setting scale: " + str(e))
            raise

    def _set_slot_stop_buttons(self, track_index, has_stop_button=True, slots=None):
        """Stop buttons on a track's empty slots. Without one, a scene launch
        leaves the track's playing clip running, so a layer added mid-set
        survives every section change without copies."""
        try:
            track = self._resolve_track(track_index)
            changed = []
            for i, slot in enumerate(track.clip_slots):
                if slots is not None and i not in [int(x) for x in slots]:
                    continue
                if slot.has_clip:
                    continue
                try:
                    slot.has_stop_button = bool(has_stop_button)
                    changed.append(i)
                except Exception:
                    continue
            return {"track_index": int(track_index), "track": str(track.name),
                    "has_stop_button": bool(has_stop_button), "slots": changed}
        except Exception as e:
            self.log_message("Error setting slot stop buttons: " + str(e))
            raise

    def _get_context(self, include_library=False):
        """Everything an agent needs to orient, in one round trip: the set,
        every track with its devices, clips by slot, mixer and play state,
        the returns, the scenes, the performance clock, and (on request) the
        library summary. Drains the performance events like
        get_performance_state does."""
        try:
            song = self._song
            perf = self._get_performance_state()
            perf_tracks = dict((t["index"], t) for t in perf.get("tracks", []))
            tracks = []
            for i, t in enumerate(song.tracks):
                ps = perf_tracks.get(i, {})
                devices = []
                try:
                    for d in t.devices:
                        devices.append({"name": str(d.name), "type": self._get_device_type(d)})
                except Exception:
                    pass
                clips = []
                try:
                    for si, s in enumerate(t.clip_slots):
                        if s.has_clip:
                            c = s.clip
                            clips.append({
                                "slot": si, "name": str(c.name), "length": float(c.length),
                                "is_midi": bool(getattr(c, "is_midi_clip", False)),
                                "is_playing": bool(c.is_playing),
                            })
                except Exception:
                    pass
                entry = {
                    "index": i, "name": str(t.name),
                    "kind": "audio" if bool(getattr(t, "has_audio_input", False)) else "midi",
                    "is_group": bool(getattr(t, "is_foldable", False)),
                    "arm": self._safe_arm(t),
                    "mute": bool(self._safe_attr(t, "mute", bool, False)),
                    "solo": bool(self._safe_attr(t, "solo", bool, False)),
                    "volume": float(t.mixer_device.volume.value),
                    "volume_db": self._volume_db(t.mixer_device.volume),
                    "panning": float(t.mixer_device.panning.value),
                    "color_index": self._safe_attr(t, "color_index", int, None),
                    "devices": devices,
                    "clips": clips,
                    "arrangement_clips": len(list(getattr(t, "arrangement_clips", []) or [])),
                    "playing_slot_index": ps.get("playing_slot_index", -1),
                    "fired_slot_index": ps.get("fired_slot_index", -1),
                    "no_stop_slots": ps.get("no_stop_slots", []),
                    "is_recording": bool(ps.get("is_recording", False)),
                }
                tracks.append(entry)
            letters = "ABCDEFGHIJKL"
            returns = []
            for i, r in enumerate(song.return_tracks):
                returns.append({
                    "index": i, "letter": letters[i] if i < len(letters) else str(i),
                    "name": str(r.name),
                    "devices": [str(d.name) for d in r.devices],
                    "volume": float(r.mixer_device.volume.value),
                    "volume_db": self._volume_db(r.mixer_device.volume),
                })
            scenes = []
            for sc in perf.get("scenes", []):
                sc = dict(sc)
                sc["clip_count"] = len(sc.get("clip_tracks", []))
                scenes.append(sc)
            try:
                app = self.application()
                live_version = "%d.%d.%d" % (app.get_major_version(), app.get_minor_version(), app.get_bugfix_version())
            except Exception:
                live_version = "unknown"
            out = {
                "live_version": live_version,
                "script_version": SCRIPT_VERSION,
                "session": {
                    "tempo": float(song.tempo),
                    "signature_numerator": int(song.signature_numerator),
                    "signature_denominator": int(song.signature_denominator),
                    "is_playing": bool(song.is_playing),
                    "bar": perf.get("bar"), "beat_in_bar": perf.get("beat_in_bar"), "beat": perf.get("beat"),
                    "clip_trigger_quantization": perf.get("clip_trigger_quantization"),
                    "clip_trigger_quantization_name": perf.get("clip_trigger_quantization_name"),
                    "scale_mode": perf.get("scale_mode"), "scale_name": perf.get("scale_name"),
                    "root_note_name": perf.get("root_note_name"),
                    "loop": self._safe_song_property("loop", bool, False),
                    "loop_start": self._safe_song_property("loop_start", float, 0.0),
                    "loop_length": self._safe_song_property("loop_length", float, 0.0),
                    "song_length": self._safe_song_property("song_length", float, 0.0),
                    "master_volume": float(song.master_track.mixer_device.volume.value),
                    "master_volume_db": self._volume_db(song.master_track.mixer_device.volume),
                },
                "tracks": tracks,
                "returns": returns,
                "scenes": scenes,
                "cues": perf.get("cues", []),
                "pending_record": perf.get("pending_record"),
                "events": perf.get("events", []),
            }
            if include_library:
                try:
                    out["library"] = self._get_library_status()
                except Exception as e:
                    out["library_error"] = str(e)
            return out
        except Exception as e:
            self.log_message("Error building context: " + str(e))
            raise

    # ── the cue store ────────────────────────────────────────────────────────

    def _validate_cue_step(self, step):
        action = step["action"]
        if action == "fire_scene":
            self._resolve_scene(step.get("scene_index", -1))
        elif action in ("fire_clip", "stop_clip"):
            track = self._resolve_track(step.get("track_index", 0))
            ci = int(step.get("clip_index", 0))
            if ci < 0 or ci >= len(track.clip_slots):
                raise IndexError("clip index %d out of range on '%s'" % (ci, track.name))
            if action == "fire_clip" and not track.clip_slots[ci].has_clip:
                raise ValueError("slot %d on '%s' holds no clip" % (ci, track.name))
        elif action == "restore_mix":
            if int(step.get("snapshot_id", -1)) not in self._mix_snapshots:
                raise ValueError("no mix snapshot %s" % step.get("snapshot_id"))
        elif action in ("set", "ramp"):
            if str(step.get("target") or "") not in self.CUE_TARGETS:
                raise ValueError("target must be one of %s" % ", ".join(self.CUE_TARGETS))
            self._perf_param(step)  # raises if the track, device or parameter is missing
            key = "value" if action == "set" else "to"
            if step.get(key) is None and step.get(key + "_db") is None:
                raise ValueError("a %s step needs '%s' (or '%s_db' for a fader)" % (action, key, key))
            if step.get(key + "_db") is not None and step.get("target") != "volume":
                raise ValueError("'%s_db' is for volume steps" % key)
            if step.get("target") == "tempo":
                t = float(step.get(key))
                if t < 20.0 or t > 999.0:
                    raise ValueError("tempo must be between 20 and 999 BPM")

    def _schedule_cue(self, cue):
        try:
            song = self._song
            if not song.is_playing:
                raise ValueError("cues need the transport running; start the performance first")
            steps_in = list((cue or {}).get("steps") or [])
            if not steps_in:
                raise ValueError("a cue needs at least one step")
            if len(steps_in) > 64:
                raise ValueError("at most 64 steps per cue")
            now = float(song.current_song_time)
            bpb = self._beats_per_bar()
            steps = []
            for i, st in enumerate(steps_in):
                action = str(st.get("action") or "")
                if st.get("ops") is not None and not action:
                    # A step may be a batch of generic ops instead of a named
                    # action: Rust plans, the tick still fires it.
                    action = "ops"
                if action not in self.CUE_ACTIONS:
                    raise ValueError("step %d: unknown action '%s'" % (i + 1, action))
                if st.get("beat") is None:
                    raise ValueError("step %d has no beat" % (i + 1))
                beat = float(st.get("beat"))
                step = dict(st)
                step["beat"] = beat
                step["action"] = action
                step["index"] = i
                step["done"] = False
                step["fired_at"] = None
                if action == "ramp":
                    end_beat = float(st.get("end_beat", beat))
                    if end_beat <= beat:
                        raise ValueError("step %d: a ramp needs end_beat after beat" % (i + 1))
                    if end_beat - beat > 64 * bpb:
                        raise ValueError("step %d: ramps are at most 64 bars" % (i + 1))
                    step["end_beat"] = end_beat
                    step["from_value"] = None
                elif beat <= now:
                    raise ValueError("step %d is at beat %s but it is beat %.2f" % (
                        i + 1, self._fmt_beat(beat), now))
                self._validate_cue_step(step)
                steps.append(step)
            replaced = []
            if cue.get("replaces") is not None:
                # One round trip for cancel-and-re-plan: the old plan goes
                # only once the new one has been validated.
                try:
                    replaced = self._cancel_cue(int(cue.get("replaces")), "replaced")["cancelled"]
                except ValueError:
                    replaced = []
            with self._cue_lock:
                cue_id = self._cue_next_id
                self._cue_next_id += 1
                entry = {"id": cue_id, "name": str(cue.get("name") or ("cue %d" % cue_id)),
                         "steps": steps, "beats_per_bar": bpb, "created_beat": now,
                         "quantization": self._safe_song_property("clip_trigger_quantization", int, 4)}
                self._cues.append(entry)
            self._arm_perf_tick()
            return {"id": cue_id, "name": entry["name"], "beat_now": now,
                    "steps": [self._public_step(s) for s in steps], "replaced": replaced}
        except Exception as e:
            self.log_message("Error scheduling cue: " + str(e))
            raise

    def _cancel_cue(self, cue_id=None, reason="cancelled"):
        with self._cue_lock:
            if cue_id is None:
                gone = list(self._cues)
                self._cues = []
            else:
                cue_id = int(cue_id)
                gone = [c for c in self._cues if c["id"] == cue_id]
                self._cues = [c for c in self._cues if c["id"] != cue_id]
        if cue_id is not None and not gone:
            raise ValueError("no pending cue with id %d" % cue_id)
        for c in gone:
            left = [s for s in c["steps"] if not s["done"]]
            self._perf_event("cue_cancelled", {"cue_id": c["id"], "name": c["name"],
                                               "steps_left": len(left), "reason": reason})
        return {"cancelled": [c["id"] for c in gone], "reason": reason}

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
        here once, on the main thread, for the handshake to report."""
        self._clock_tick_armed = False
        try:
            self._tick_sampler.note(time.time())
            if self._live_version is None:
                self._live_version = self._read_live_version()
            if SOCKET_READER == "main_thread_tick":
                self._drain_clients()
            self._event_tick()
            # The channels write after the drain flushed, so flush again:
            # an event must not wait a tick behind the reply that triggered it.
            if SOCKET_READER == "main_thread_tick":
                self._flush_clients()
        except Exception as e:
            self.log_message("clock tick error: " + str(e))
        if self.running:
            self._arm_clock_tick()

    EVENT_CHANNELS = ("clock", "levels", "changes", "cue")
    CLOCK_EVERY_MS_DEFAULT = 100.0
    # What Live's tick is worth in seconds, for the clock channel's slack.
    CLOCK_TICK_S = 0.1
    CHANGES_EVERY_TICKS = 3

    def _event_tick(self):
        """The three channels, on the tick, only while someone is listening.
        Counted inside the tick's own budget: each is a handful of reads."""
        try:
            if self._any_subscriber("clock"):
                self._clock_event_tick(time.time())
            if self._any_subscriber("levels"):
                bar = self._bar_position()[0]
                self._level_tick(bar)
                self._levels_event_tick(bar)
            if self._any_subscriber("changes"):
                self._changes_ticks += 1
                if self._changes_ticks >= self.CHANGES_EVERY_TICKS:
                    self._changes_ticks = 0
                    self._changes_tick()
        except Exception as e:
            self.log_message("event tick error: " + str(e))

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

    # ── The generic surface ────────────────────────────────────────────────

    def _path_root(self, name):
        if name == "song":
            return self._song
        if name == "application":
            return self.application()
        if name == "browser":
            return self.application().browser
        raise ValueError("path must start with %s" % ", ".join(PATH_ROOTS))

    def _walk(self, steps):
        """Follow every step but the last; returns (owner, last_step)."""
        obj = self._path_root(steps[0][1])
        for i in range(1, len(steps) - 1):
            obj = self._step(obj, steps[i], steps, i)
        return obj, (steps[-1] if len(steps) > 1 else None)

    def _step(self, obj, step, steps, i):
        kind, value = step
        where = self._path_text(steps[:i + 1])
        if kind == "index":
            try:
                seq = list(obj)
            except Exception:
                raise ValueError("%s is not a list" % self._path_text(steps[:i]))
            if value < 0 or value >= len(seq):
                raise ValueError("%s: index %d is out of range (%d)" % (
                    self._path_text(steps[:i]), value, len(seq)))
            return seq[value]
        if not hasattr(obj, value):
            raise ValueError("%s does not exist on Live %s" % (where, self._live_version or "?"))
        return getattr(obj, value)

    def _path_text(self, steps):
        out = ""
        for kind, value in steps:
            if kind == "attr":
                out = value if not out else out + "." + value
            else:
                out += "[%d]" % value
        return out

    def _resolve(self, path):
        """The object a path names."""
        steps = _parse_path(path)
        obj = self._path_root(steps[0][1])
        for i in range(1, len(steps)):
            obj = self._step(obj, steps[i], steps, i)
        return obj

    _DESCRIBE_TYPES = {bool: "bool", int: "int", float: "float", str: "str"}

    def _type_name(self, value):
        name = self._DESCRIBE_TYPES.get(type(value))
        if name:
            return name
        if isinstance(value, (list, tuple)):
            inner = self._type_name(value[0]) if len(value) else "?"
            return "list[%s]" % inner
        if value is None:
            return "none"
        try:
            return type(value).__name__
        except Exception:
            return "object"

    def _describe(self, path):
        obj = self._resolve(path)
        attrs = {}
        methods = []
        for name in sorted(dir(obj)):
            if name.startswith("_"):
                continue
            try:
                value = getattr(obj, name)
            except Exception:
                continue
            if callable(value):
                methods.append(name)
                continue
            readonly = True
            try:
                prop = getattr(type(obj), name, None)
                readonly = not (isinstance(prop, property) and prop.fset is not None)
            except Exception:
                pass
            attrs[name] = {"type": self._type_name(value), "readonly": readonly}
        try:
            class_name = type(obj).__name__
        except Exception:
            class_name = "object"
        name = None
        try:
            if hasattr(obj, "name"):
                name = "%s" % obj.name
        except Exception:
            pass
        return {"path": path, "class": class_name, "name": name, "attrs": attrs,
                "methods": methods, "live_version": self._live_version}

    RUN_OPS = ("get", "set", "call", "wait_tick")
    RUN_MAX_OPS = 512

    def _run_ops(self, params):
        """An ordered batch on the main thread, sliced by the executor: one
        round trip however many ops. The first failure stops the batch and
        the error names the op, so the server can say what did run."""
        ops = list((params or {}).get("ops") or [])
        if not ops:
            raise ValueError("run needs at least one op")
        if len(ops) > self.RUN_MAX_OPS:
            raise ValueError("at most %d ops per run" % self.RUN_MAX_OPS)
        out = {}
        done = 0
        for i, op in enumerate(ops):
            if not isinstance(op, dict):
                raise ValueError("run: op %d is not an object" % i)
            kind = "%s" % (op.get("op") or "")
            if kind not in self.RUN_OPS:
                raise ValueError("run: op %d: unknown op %r; known: %s" % (
                    i, kind, ", ".join(self.RUN_OPS)))
            try:
                if kind == "wait_tick":
                    ticks = max(1, min(32, int(op.get("ticks", 1))))
                    for _ in range(ticks):
                        yield None
                    done += 1
                    continue
                steps = _parse_path(op.get("path"))
                if kind == "get":
                    value = self._resolve(op.get("path"))
                    if op.get("as"):
                        out["%s" % op["as"]] = self._jsonable(value)
                elif kind == "set":
                    owner, last = self._walk(steps)
                    if last is None or last[0] != "attr":
                        raise ValueError("set needs a name to set, not an index")
                    name = last[1]
                    if not hasattr(owner, name):
                        raise ValueError("%s does not exist on Live %s" % (
                            op.get("path"), self._live_version or "?"))
                    try:
                        setattr(owner, name, op.get("value"))
                    except AttributeError:
                        raise ValueError("%s is read-only on Live %s" % (
                            op.get("path"), self._live_version or "?"))
                    if op.get("as"):
                        out["%s" % op["as"]] = self._jsonable(getattr(owner, name, None))
                else:
                    target = self._resolve(op.get("path"))
                    if not callable(target):
                        raise ValueError("%s is not a method" % op.get("path"))
                    args = list(op.get("args") or [])
                    value = target(*args)
                    if op.get("as"):
                        out["%s" % op["as"]] = self._jsonable(value)
            except ValueError as e:
                raise ValueError("run: op %d: %s" % (i, str(e)))
            done += 1
            yield None
        out["ops"] = done
        yield Done(out)

    def _jsonable(self, value):
        if isinstance(value, bool) or value is None:
            return value
        if isinstance(value, (int, float)):
            return value
        if isinstance(value, (list, tuple)):
            return [self._jsonable(v) for v in list(value)[:256]]
        try:
            return "%s" % value
        except Exception:
            return None

    # ── Events ──────────────────────────────────────────────────────────────

    def _publish(self, channel, payload):
        """One event to every client subscribed to `channel`, and to nobody
        else. Called on the tick, so it never blocks."""
        body = None
        with self._clients_lock:
            clients = list(self._clients)
        for c in clients:
            if c.closed or channel not in c.subscriptions:
                continue
            if body is None:
                body = dict(payload)
                body["event"] = channel
                body["t"] = round(time.time() - self._started_at, 4)
            self._client_write(c, None, body)

    def _any_subscriber(self, channel):
        with self._clients_lock:
            for c in self._clients:
                if not c.closed and channel in c.subscriptions:
                    return True
        return False

    def _subscribe(self, c, params):
        channels = params.get("channels") or []
        if not isinstance(channels, list):
            raise ValueError("channels must be a list")
        unknown = [x for x in channels if x not in self.EVENT_CHANNELS]
        if unknown:
            raise ValueError("unknown channel(s): %s; known: %s" % (
                ", ".join("%s" % u for u in unknown), ", ".join(self.EVENT_CHANNELS)))
        every = params.get("clock_every_ms")
        for name in channels:
            opts = {}
            if name == "clock" and every is not None:
                opts["clock_every_ms"] = max(0.0, float(every))
            c.subscriptions[name] = opts
        if "changes" in c.subscriptions and self._watch_prev is None:
            self._watch_prev = self._watch_snapshot()
        self._arm_clock_tick()
        return {"subscribed": sorted(c.subscriptions.keys()),
                "clock_every_ms": c.subscriptions.get("clock", {}).get(
                    "clock_every_ms", self.CLOCK_EVERY_MS_DEFAULT)}

    def _unsubscribe(self, c, params):
        channels = params.get("channels")
        if channels is None:
            c.subscriptions = {}
        else:
            for name in channels:
                c.subscriptions.pop(name, None)
        return {"subscribed": sorted(c.subscriptions.keys())}

    def _watch_snapshot(self):
        """The small set of things a `changes` subscriber is told about, as
        path -> value. Read on the main thread; no listener is registered on
        anything, which is what the first capture's deadlock taught us."""
        out = {}
        try:
            song = self._song
            out["song.is_playing"] = bool(song.is_playing)
            out["song.tempo"] = round(float(song.tempo), 4)
            out["song.signature_numerator"] = int(song.signature_numerator)
            tracks = list(song.tracks)
            out["song.tracks.count"] = len(tracks)
            for i, t in enumerate(tracks):
                out["song.tracks[%d].name" % i] = "%s" % t.name
                out["song.tracks[%d].mute" % i] = bool(self._safe_attr(t, "mute", bool, False))
                out["song.tracks[%d].solo" % i] = bool(self._safe_attr(t, "solo", bool, False))
                out["song.tracks[%d].arm" % i] = bool(self._safe_arm(t))
                out["song.tracks[%d].playing_slot_index" % i] = int(
                    self._safe_attr(t, "playing_slot_index", int, -1))
            scenes = list(song.scenes)
            out["song.scenes.count"] = len(scenes)
            for i, sc in enumerate(scenes):
                out["song.scenes[%d].name" % i] = "%s" % sc.name
                out["song.scenes[%d].is_triggered" % i] = bool(
                    self._safe_attr(sc, "is_triggered", bool, False))
        except Exception as e:
            self.log_message("watch snapshot error: " + str(e))
        return out

    def _changes_tick(self):
        now = self._watch_snapshot()
        prev = self._watch_prev or {}
        changed = []
        for key in sorted(set(list(prev.keys()) + list(now.keys()))):
            before = prev.get(key)
            after = now.get(key)
            if before != after:
                changed.append({"path": key, "from": before, "to": after})
        self._watch_prev = now
        if changed:
            self._publish("changes", {"changed": changed[:64]})

    def _clock_event_tick(self, now_s):
        every = self.CLOCK_EVERY_MS_DEFAULT
        with self._clients_lock:
            for c in self._clients:
                opts = c.subscriptions.get("clock")
                # 0 is a real answer -- every tick -- so ask whether the key
                # is there, never whether the number is truthy.
                if opts is not None and opts.get("clock_every_ms") is not None:
                    every = min(every, float(opts["clock_every_ms"]))
        every_s = max(0.0, every / 1000.0)
        if self._clock_event_every != every_s:
            # A subscriber changed the rate: begin the schedule again.
            self._clock_event_every = every_s
            self._next_clock_event = 0.0
        # A tick that lands fractionally early still counts as this
        # interval's tick. Live's tick is ~100 ms with about 10 ms of
        # jitter, so a hard floor at the interval fails on roughly half the
        # ticks when the interval is at or just above the tick period -- and
        # every skip costs a whole tick, so 100 ms asked for arrives at 200.
        slack = min(self.CLOCK_TICK_S / 2.0, every_s / 2.0)
        due = self._next_clock_event
        if due and now_s + slack < due:
            return
        song = self._song
        playing = bool(song.is_playing)
        if not playing and self._clock_event_sent_stopped:
            return
        # Advance the schedule rather than restart it from now, so an early
        # tick does not push the next one out and early and late average to
        # the rate asked for. More than one interval behind -- the transport
        # was stopped, or Live stalled -- and the schedule is clamped
        # forward instead of firing a burst to catch up.
        if not due or now_s - due > every_s:
            self._next_clock_event = now_s + every_s
        else:
            self._next_clock_event = due + every_s
        self._clock_event_sent_stopped = not playing
        bar, bib, beat = self._bar_position()
        bpb = self._beats_per_bar()
        tempo = float(song.tempo)
        self._publish("clock", {
            "bar": bar, "beat_in_bar": bib, "beat": beat, "tempo": tempo,
            "beats_per_bar": bpb, "playing": playing,
            "scene": self._current_scene,
            "phrase_bar": (self._phrase_info(bar) or {}).get("bar_in_phrase"),
            "next_bar_in_s": max(0.0, (bar * bpb - beat) * 60.0 / max(1.0, tempo)),
        })

    def _levels_event_tick(self, bar):
        if self._levels_event_bar == bar:
            return
        self._levels_event_bar = bar
        cur = self._bar_peaks
        if cur is None:
            return
        self._publish("levels", {"bar": cur["bar"], "master": round(cur["master"], 5),
                                 "tracks": [round(v, 5) for v in cur["tracks"]]})

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

    def _arm_perf_tick(self):
        if self._perf_tick_armed:
            return
        self._perf_tick_armed = True
        try:
            self.schedule_message(1, self._perf_tick)
        except Exception as e:
            self._perf_tick_armed = False
            self.log_message("could not arm the performance clock: " + str(e))

    def _perf_tick(self):
        self._perf_tick_armed = False
        more = True
        try:
            more = self._perf_step()
        except Exception as e:
            self.log_message("performance tick error: " + str(e))
            self.log_message(traceback.format_exc())
        if more:
            self._arm_perf_tick()

    def _launch_lead(self, quantization, bpb):
        """How many beats before its target a launch step is issued."""
        if quantization == 0:
            return 0.0
        if quantization >= 5:
            return self.LAUNCH_LEAD_MARGIN
        return max(0.0, bpb - self.LAUNCH_LEAD_MARGIN)

    def _perf_step(self):
        """One tick of the performance clock; True while there is work."""
        song = self._song
        playing = bool(song.is_playing)
        now = float(song.current_song_time)
        bpb = self._beats_per_bar()
        self._perf_record_step(playing)
        if self._arr_recording and not playing:
            self._clear_record_mode()
        if self._performance_mode and playing:
            self._watch_scene_rows()
            self._level_tick(self._bar_position()[0])
        with self._cue_lock:
            cues = list(self._cues)
        if cues and not playing:
            self._cancel_cue(None, "transport stopped at beat %s" % self._fmt_beat(now))
            return self._pending_record is not None or self._performance_mode
        quant = self._safe_song_property("clip_trigger_quantization", int, 4)
        for cue in cues:
            if cue["beats_per_bar"] != bpb:
                self._cancel_cue(cue["id"], "time signature changed")
                continue
            for step in cue["steps"]:
                if step["done"]:
                    continue
                action = step["action"]
                if action in ("fire_scene", "fire_clip", "stop_clip", "stop_all_clips"):
                    if now >= step["beat"] - self._launch_lead(quant, bpb):
                        self._run_cue_action(cue, step, now)
                elif action == "ramp":
                    if now >= step["beat"]:
                        self._run_cue_ramp(cue, step, now)
                elif now >= step["beat"]:
                    self._run_cue_action(cue, step, now)
            if all(s["done"] for s in cue["steps"]):
                with self._cue_lock:
                    self._cues = [c for c in self._cues if c["id"] != cue["id"]]
                self._perf_event("cue_done", {"cue_id": cue["id"], "name": cue["name"]})
        with self._cue_lock:
            pending = len(self._cues) > 0
        return pending or self._pending_record is not None or self._performance_mode

    def _run_cue_ops(self, cue, step, now):
        """A step that is a batch of ops: run it to completion on this tick.
        The batch is the producer's plan, already validated by the server."""
        gen = self._run_ops({"ops": step.get("ops") or []})
        while True:
            item = next(gen)
            if isinstance(item, Done):
                return item.result

    def _run_cue_action(self, cue, step, now):
        action = step["action"]
        detail = {"cue_id": cue["id"], "cue": cue["name"], "step": step["index"],
                  "action": action, "label": step.get("label"), "target_beat": step["beat"],
                  "target_bar": step.get("bar"), "issued_at": now,
                  "late": bool(now > step["beat"] + 0.25)}
        try:
            if action == "fire_scene":
                i, scene = self._resolve_scene(step.get("scene_index", -1))
                scene.fire()
                self._note_scene_fired(i, self._landing()["lands_on_bar"])
            elif action == "fire_clip":
                self._fire_clip(int(step.get("track_index", 0)), int(step.get("clip_index", 0)))
            elif action == "stop_clip":
                self._stop_clip(int(step.get("track_index", 0)), int(step.get("clip_index", 0)))
            elif action == "stop_all_clips":
                self._song.stop_all_clips()
            elif action == "stop_playback":
                self._song.stop_playing()
            elif action == "set":
                if step.get("value_db") is not None:
                    self._perf_write(step, self._value_for_db(self._perf_param(step)[1], float(step.get("value_db"))))
                else:
                    self._perf_write(step, float(step.get("value")))
            elif action == "restore_mix":
                self._restore_mix(step.get("snapshot_id"))
            elif action == "ops":
                detail["ops"] = self._run_cue_ops(cue, step, now)
            # How far past its beat the step actually fired. A launch is
            # issued before its beat on purpose (Live's own quantization
            # places it), so this is negative for those and near zero for
            # the rest; it is the number the story is judged by.
            detail["fired_at_beat"] = now
            detail["late_beats"] = round(now - step["beat"], 4)
            try:
                detail["late_ms"] = round(
                    (now - step["beat"]) * 60000.0 / max(1.0, float(self._song.tempo)), 2)
            except Exception:
                detail["late_ms"] = None
            self._perf_event("cue_step_fired", detail)
            self._publish("cue", detail)
        except Exception as e:
            detail["error"] = str(e)
            self._perf_event("cue_step_failed", detail)
        step["done"] = True
        step["fired_at"] = now

    def _run_cue_ramp(self, cue, step, now):
        detail = {"cue_id": cue["id"], "cue": cue["name"], "step": step["index"],
                  "label": step.get("label"), "target": step.get("target"), "to": step.get("to")}
        try:
            if step.get("from_value") is None:
                if step.get("to_db") is not None:
                    # A fader ramp given in dB: the target on Live's own curve.
                    step["to"] = self._value_for_db(self._perf_param(step)[1], float(step.get("to_db")))
                step["from_value"] = self._perf_read(step)
                detail["from"] = step["from_value"]
                self._perf_event("ramp_started", dict(detail, action="ramp", issued_at=now))
            span = float(step["end_beat"]) - float(step["beat"])
            frac = (now - float(step["beat"])) / span if span > 0 else 1.0
            if frac >= 1.0:
                self._perf_write(step, float(step["to"]))
                step["done"] = True
                step["fired_at"] = now
                self._perf_event("ramp_done", dict(detail, action="ramp", issued_at=now))
            else:
                frac = max(0.0, frac)
                v = float(step["from_value"]) + (float(step["to"]) - float(step["from_value"])) * frac
                self._perf_write(step, v)
        except Exception as e:
            detail["error"] = str(e)
            self._perf_event("cue_step_failed", dict(detail, action="ramp"))
            step["done"] = True
            step["fired_at"] = now

    def _perf_param(self, step):
        """The Live object a set or ramp step writes, as (kind, object)."""
        target = str(step.get("target") or "")
        song = self._song
        if target == "tempo":
            return "tempo", song
        if target == "crossfader":
            return "unit", song.master_track.mixer_device.crossfader
        kind = str(step.get("kind") or "track")
        if target == "volume":
            return "param", self._resolve_track(step.get("track_index", 0), kind).mixer_device.volume
        if target == "mute":
            return "mute", self._resolve_track(step.get("track_index", 0), kind)
        if target == "send":
            track = self._resolve_track(step.get("track_index", 0), kind)
            sends = list(track.mixer_device.sends)
            si = int(step.get("send_index", 0))
            if si < 0 or si >= len(sends):
                raise IndexError("send index %d out of range" % si)
            return "param", sends[si]
        if target == "device":
            track = self._resolve_track(step.get("track_index", 0), kind)
            devices = list(track.devices)
            di = int(step.get("device_index", 0))
            if di < 0 or di >= len(devices):
                raise IndexError("device index %d out of range on '%s'" % (di, track.name))
            params = list(devices[di].parameters)
            pi = int(step.get("parameter_index", 0))
            if pi < 0 or pi >= len(params):
                raise IndexError("parameter index %d out of range on '%s'" % (pi, devices[di].name))
            return "param", params[pi]
        raise ValueError("unknown target '%s'" % target)

    def _perf_read(self, step):
        kind, obj = self._perf_param(step)
        if kind == "tempo":
            return float(obj.tempo)
        if kind == "unit":
            span = float(obj.max) - float(obj.min)
            return (float(obj.value) - float(obj.min)) / span if span else 0.0
        if kind == "mute":
            return 1.0 if obj.mute else 0.0
        return float(obj.value)

    def _perf_write(self, step, value):
        kind, obj = self._perf_param(step)
        value = float(value)
        if kind == "tempo":
            obj.tempo = max(20.0, min(999.0, value))
        elif kind == "unit":
            v = max(0.0, min(1.0, value))
            obj.value = float(obj.min) + (float(obj.max) - float(obj.min)) * v
        elif kind == "mute":
            obj.mute = bool(value >= 0.5)
        else:
            obj.value = max(float(obj.min), min(float(obj.max), value))

    def _perf_record_step(self, playing):
        pr = self._pending_record
        if pr is None:
            return
        try:
            track = list(self._song.tracks)[pr["track_index"]]
            slot = track.clip_slots[pr["slot"]]
            if slot.has_clip:
                clip = slot.clip
                if str(clip.name) != pr["name"]:
                    try:
                        clip.name = pr["name"]
                    except Exception:
                        pass
                if bool(getattr(clip, "is_recording", False)):
                    pr["seen_recording"] = True
                    return
                if pr["seen_recording"]:
                    track.arm = False
                    self._perf_event("recording_done", {
                        "track_index": pr["track_index"], "track": str(track.name),
                        "slot": pr["slot"], "name": pr["name"],
                        "length": float(clip.length)})
                    self._pending_record = None
                    return
            if (pr["seen_recording"] and not playing) or time.time() - pr["started"] > 180:
                track.arm = False
                self._perf_event("recording_abandoned", {
                    "track_index": pr["track_index"], "track": str(track.name),
                    "slot": pr["slot"], "name": pr["name"],
                    "reason": "transport stopped" if not playing else "nothing recorded"})
                self._pending_record = None
        except Exception as e:
            self.log_message("recording watch error: " + str(e))
            self._pending_record = None

    # ── Passive human-UI listeners ──────────────────────────────────────────────

    def _enqueue_passive(self, event_type, detail=None, track_index=None, clip_index=None):
        """Append a coarse human-UI event (capped FIFO)."""
        evt = {
            "type": event_type,
            "ts": time.time(),
            "track_index": track_index,
            "clip_index": clip_index,
            "detail": detail if detail is not None else {},
        }
        with self._passive_lock:
            self._passive_events.append(evt)
            if len(self._passive_events) > self._passive_max:
                self._passive_events = self._passive_events[-self._passive_max:]

    def _drain_passive_events(self):
        """Return and clear the passive event queue (called by MCP poller)."""
        with self._passive_lock:
            events = list(self._passive_events)
            self._passive_events = []
        return {"events": events, "count": len(events)}

    def _safe_add_listener(self, obj, add_name, callback):
        try:
            if obj is not None and hasattr(obj, add_name):
                getattr(obj, add_name)(callback)
                return True
        except Exception as e:
            self.log_message("add listener %s failed: %s" % (add_name, str(e)))
        return False

    def _safe_remove_listener(self, obj, remove_name, callback):
        try:
            if obj is not None and hasattr(obj, remove_name):
                getattr(obj, remove_name)(callback)
        except Exception:
            pass

    def _setup_passive_listeners(self):
        """Register high-signal LOM listeners for Mode C / assisted human edits."""
        song = self._song

        def on_tempo():
            try:
                self._enqueue_passive("tempo_changed", {"tempo": float(song.tempo)})
            except Exception:
                self._enqueue_passive("tempo_changed")

        def on_sig_num():
            try:
                self._enqueue_passive(
                    "time_signature_changed",
                    {
                        "signature_numerator": int(song.signature_numerator),
                        "signature_denominator": int(song.signature_denominator),
                    },
                )
            except Exception:
                self._enqueue_passive("time_signature_changed")

        def on_sig_den():
            on_sig_num()

        def on_is_playing():
            try:
                self._enqueue_passive(
                    "playback_changed",
                    {"is_playing": bool(song.is_playing)},
                )
            except Exception:
                self._enqueue_passive("playback_changed")

        def on_tracks():
            count = len(song.tracks)
            previous = getattr(self, "_passive_track_count", None)
            self._passive_track_count = count
            detail = {"track_count": count}
            if previous is not None:
                detail["previous_count"] = previous
                detail["removed"] = count < previous
                detail["added"] = count > previous
            self._enqueue_passive("tracks_changed", detail)
            try:
                self._rebind_track_listeners()
            except Exception as e:
                self.log_message("rebind track listeners failed: " + str(e))

        self._safe_add_listener(song, "add_tempo_listener", on_tempo)
        self._safe_add_listener(song, "add_signature_numerator_listener", on_sig_num)
        self._safe_add_listener(song, "add_signature_denominator_listener", on_sig_den)
        self._safe_add_listener(song, "add_is_playing_listener", on_is_playing)
        self._safe_add_listener(song, "add_tracks_listener", on_tracks)

        self._song_passive_callbacks = [
            ("tempo_listener", on_tempo),
            ("signature_numerator_listener", on_sig_num),
            ("signature_denominator_listener", on_sig_den),
            ("is_playing_listener", on_is_playing),
            ("tracks_listener", on_tracks),
        ]

        self._rebind_track_listeners()
        self.log_message("Passive LOM listeners registered")

    def _teardown_passive_listeners(self):
        song = getattr(self, "_song", None)
        for suffix, cb in getattr(self, "_song_passive_callbacks", []):
            self._safe_remove_listener(song, "remove_" + suffix, cb)
        self._clear_track_listeners()

    def _clear_track_listeners(self):
        for track, bindings in getattr(self, "_passive_track_bindings", []):
            for add_name, callback in bindings:
                remove_name = "remove_" + add_name[len("add_"):]
                target = getattr(callback, "_passive_target", None)
                if target is not None:
                    self._safe_remove_listener(target, remove_name, callback)
                else:
                    self._safe_remove_listener(track, remove_name, callback)
        self._passive_track_bindings = []

    def _rebind_track_listeners(self):
        self._clear_track_listeners()
        song = self._song
        for track_index, track in enumerate(song.tracks):
            bindings = []

            def make_track_cb(kind, t_index):
                def _cb():
                    detail = {}
                    try:
                        t = song.tracks[t_index]
                        if kind == "name_changed":
                            detail["name"] = t.name
                        elif kind == "mute_changed":
                            detail["mute"] = bool(t.mute)
                        elif kind == "solo_changed":
                            detail["solo"] = bool(t.solo)
                        elif kind == "arm_changed":
                            detail["arm"] = self._safe_arm(t)
                        elif kind == "devices_changed":
                            detail["device_count"] = len(t.devices)
                        elif kind == "volume_changed":
                            detail["volume"] = float(t.mixer_device.volume.value)
                        elif kind == "panning_changed":
                            detail["panning"] = float(t.mixer_device.panning.value)
                    except Exception:
                        pass
                    self._enqueue_passive(kind, detail, track_index=t_index)
                return _cb

            pairs = [
                ("add_name_listener", "name_changed"),
                ("add_mute_listener", "mute_changed"),
                ("add_solo_listener", "solo_changed"),
                ("add_arm_listener", "arm_changed"),
                ("add_devices_listener", "devices_changed"),
            ]
            for add_name, kind in pairs:
                cb = make_track_cb(kind, track_index)
                if self._safe_add_listener(track, add_name, cb):
                    bindings.append((add_name, cb))

            try:
                mixer = track.mixer_device
                vol_cb = make_track_cb("volume_changed", track_index)
                vol_cb._passive_target = mixer.volume
                if self._safe_add_listener(mixer.volume, "add_value_listener", vol_cb):
                    bindings.append(("add_value_listener", vol_cb))
                pan_cb = make_track_cb("panning_changed", track_index)
                pan_cb._passive_target = mixer.panning
                if self._safe_add_listener(mixer.panning, "add_value_listener", pan_cb):
                    bindings.append(("add_value_listener", pan_cb))
            except Exception as e:
                self.log_message("mixer listeners failed on track %d: %s" % (track_index, str(e)))

            try:
                for clip_index, slot in enumerate(track.clip_slots):
                    def make_slot_cb(t_index, c_index):
                        def _cb():
                            has = False
                            try:
                                has = bool(song.tracks[t_index].clip_slots[c_index].has_clip)
                            except Exception:
                                pass
                            self._enqueue_passive(
                                "clip_slot_changed",
                                {"has_clip": has},
                                track_index=t_index,
                                clip_index=c_index,
                            )
                            try:
                                self._bind_clip_listeners(t_index, c_index)
                            except Exception:
                                pass
                        return _cb

                    slot_cb = make_slot_cb(track_index, clip_index)
                    slot_cb._passive_target = slot
                    if self._safe_add_listener(slot, "add_has_clip_listener", slot_cb):
                        bindings.append(("add_has_clip_listener", slot_cb))
                    if slot.has_clip:
                        self._bind_clip_listeners(track_index, clip_index, bindings)
            except Exception as e:
                self.log_message("clip slot listeners failed on track %d: %s" % (track_index, str(e)))

            self._passive_track_bindings.append((track, bindings))

    def _bind_clip_listeners(self, track_index, clip_index, bindings=None):
        try:
            track = self._song.tracks[track_index]
            slot = track.clip_slots[clip_index]
            if not slot.has_clip:
                return
            clip = slot.clip

            def on_name():
                name = ""
                try:
                    name = clip.name
                except Exception:
                    pass
                self._enqueue_passive(
                    "clip_name_changed",
                    {"name": name},
                    track_index=track_index,
                    clip_index=clip_index,
                )

            def on_notes():
                self._enqueue_passive(
                    "clip_notes_changed",
                    {},
                    track_index=track_index,
                    clip_index=clip_index,
                )

            def on_playing():
                playing = False
                try:
                    playing = bool(clip.is_playing)
                except Exception:
                    pass
                self._enqueue_passive(
                    "clip_playing_changed",
                    {"is_playing": playing},
                    track_index=track_index,
                    clip_index=clip_index,
                )

            for add_name, cb in [
                ("add_name_listener", on_name),
                ("add_notes_listener", on_notes),
                ("add_playing_status_listener", on_playing),
            ]:
                cb._passive_target = clip
                if self._safe_add_listener(clip, add_name, cb):
                    if bindings is not None:
                        bindings.append((add_name, cb))
        except Exception as e:
            self.log_message(
                "bind clip listeners %d/%d failed: %s"
                % (track_index, clip_index, str(e))
            )

    # ── Dataset / state snapshot helpers ──────────────────────────────────────

    def _safe_attr(self, obj, attr, cast=None, default=None):
        try:
            val = getattr(obj, attr)
            if callable(val):
                return default
            if cast is not None:
                return cast(val)
            return val
        except Exception:
            return default

    def _notes_from_clip(self, clip):
        """Extract MIDI notes from a clip (incl. MPE/expression when available)."""
        notes = []
        if not clip or not getattr(clip, "is_midi_clip", False):
            return notes

        if hasattr(clip, "get_notes_extended"):
            try:
                raw = clip.get_notes_extended(0, 128, 0.0, float(clip.length) + 1.0)
                for n in raw:
                    entry = {
                        "pitch": int(getattr(n, "pitch", 0)),
                        "start_time": float(getattr(n, "start_time", 0.0)),
                        "duration": float(getattr(n, "duration", 0.0)),
                        "velocity": float(getattr(n, "velocity", 0)),
                        "mute": bool(getattr(n, "mute", False)),
                    }
                    for opt, caster in [
                        ("probability", float),
                        ("velocity_deviation", float),
                        ("release_velocity", float),
                        ("note_id", int),
                    ]:
                        if hasattr(n, opt):
                            try:
                                entry[opt] = caster(getattr(n, opt))
                            except Exception:
                                pass
                    for opt in ("pitch_bend_range", "pressure", "timbre", "slide"):
                        if hasattr(n, opt):
                            try:
                                entry[opt] = float(getattr(n, opt))
                            except Exception:
                                pass
                    notes.append(entry)
                return notes
            except Exception as e:
                self.log_message("get_notes_extended failed, falling back: " + str(e))

        if hasattr(clip, "get_notes"):
            try:
                raw = clip.get_notes(0.0, 0, float(clip.length) + 1.0, 128)
                for n in raw:
                    notes.append({
                        "pitch": int(n[0]),
                        "start_time": float(n[1]),
                        "duration": float(n[2]),
                        "velocity": float(n[3]),
                        "mute": bool(n[4]) if len(n) > 4 else False,
                    })
            except Exception as e:
                self.log_message("get_notes failed: " + str(e))
        return notes

    def _warp_markers_from_clip(self, clip):
        markers = []
        try:
            raw = getattr(clip, "warp_markers", None)
            if not raw:
                return markers
            for m in raw:
                markers.append({
                    "beat_time": float(getattr(m, "beat_time", getattr(m, "time", 0.0))),
                    "sample_time": float(
                        getattr(m, "sample_time", getattr(m, "time", 0.0))
                    ),
                })
        except Exception as e:
            self.log_message("warp_markers read failed: " + str(e))
        return markers

    def _automated_params_for_device(self, device):
        automated = []
        try:
            for param in device.parameters:
                is_auto = False
                try:
                    if hasattr(param, "automation_state"):
                        is_auto = int(param.automation_state) != 0
                    elif hasattr(param, "is_automated"):
                        is_auto = bool(param.is_automated)
                except Exception:
                    continue
                if is_auto:
                    automated.append(param.name)
        except Exception:
            pass
        return automated

    # Racks nest, and a pathological project could nest deeply. Cap the walk so
    # a snapshot can never blow the stack or the payload size.
    _MAX_CHAIN_DEPTH = 4

    # ── Device parameters: what Live shows, not only the float ──────────────

    _DISPLAY_UNITS = {"khz": ("hz", 1000.0), "hz": ("hz", 1.0), "ms": ("s", 0.001),
                      "s": ("s", 1.0), "db": ("db", 1.0), "%": ("%", 1.0),
                      "st": ("st", 1.0), "k": ("", 1000.0)}

    def _param_items(self, param):
        """The labels a quantized parameter accepts, as Live spells them."""
        try:
            if not bool(getattr(param, "is_quantized", False)):
                return []
            return ["%s" % item for item in list(getattr(param, "value_items", []) or [])]
        except Exception:
            return []

    def _param_display(self, param, value=None):
        """Live's own display string for a value: "200 Hz", "Low Cut 48 dB".
        The Live Object Model gives str_for_value; value_string is not in it,
        so it is only a fallback for the current value."""
        try:
            if value is None:
                return "%s" % param.str_for_value(param.value)
            return "%s" % param.str_for_value(float(value))
        except Exception:
            pass
        if value is None:
            try:
                return "%s" % param.value_string
            except Exception:
                pass
        return None

    def _display_parts(self, text):
        """(number, unit) from one of Live's display strings, normalized:
        "1.20 kHz" -> (1200.0, "hz"), "12.0 ms" -> (0.012, "s")."""
        if text is None:
            return (None, "")
        text = ("%s" % text).strip().lower().replace(",", ".")
        if "inf" in text:
            return (-80.0 if text.startswith("-") else None, "db")
        digits = ""
        i = 0
        while i < len(text) and (text[i].isdigit() or text[i] in "+-."):
            digits += text[i]
            i += 1
        try:
            value = float(digits)
        except ValueError:
            return (None, text)
        unit = text[i:].strip()
        known = self._DISPLAY_UNITS.get(unit)
        if known is not None:
            return (value * known[1], known[0])
        return (value, unit)

    def _value_from_display(self, param, text):
        """The value whose display reads `text`: an exact label for a
        quantized parameter, a bisection over Live's own str_for_value for a
        continuous one. Raises ValueError naming what the parameter takes."""
        want = ("%s" % text).strip()
        if want == "":
            raise ValueError("give a value")
        low = want.lower()
        items = self._param_items(param)
        if items:
            lo_i, hi_i = int(round(float(param.min))), int(round(float(param.max)))
            if hi_i - lo_i <= 512:
                for step in range(lo_i, hi_i + 1):
                    label = self._param_display(param, step)
                    if label is not None and label.strip().lower() == low:
                        return float(step)
            for index, label in enumerate(items):
                if label.strip().lower() == low:
                    return float(param.min) + float(index)
            raise ValueError("'%s' takes one of: %s" % (param.name, ", ".join(items)))
        target, unit = self._display_parts(want)
        if target is None:
            raise ValueError("'%s' is not a value '%s' can show" % (want, param.name))
        lo, hi = float(param.min), float(param.max)
        show_lo, show_hi = self._param_display(param, lo), self._param_display(param, hi)
        n_lo = self._display_parts(show_lo)[0]
        n_hi = self._display_parts(show_hi)[0]
        if n_lo is None or n_hi is None:
            raise ValueError("Live does not show '%s' as a number; give the raw value between %s and %s" % (
                param.name, lo, hi))
        if target < min(n_lo, n_hi) - 1e-6 or target > max(n_lo, n_hi) + 1e-6:
            raise ValueError("'%s' runs %s to %s; '%s' is outside that" % (
                param.name, show_lo, show_hi, want))
        ascending = n_hi >= n_lo
        for _ in range(48):
            mid = (lo + hi) / 2.0
            n_mid = self._display_parts(self._param_display(param, mid))[0]
            if n_mid is None:
                break
            if (n_mid < target) == ascending:
                lo = mid
            else:
                hi = mid
            if abs(hi - lo) < 1e-7:
                break
        # Live's display is rounded, so the bisection lands at the edge of a
        # bucket: take whichever end actually reads closest to what was asked.
        best, best_gap = (lo + hi) / 2.0, None
        for candidate in (lo, hi, (lo + hi) / 2.0):
            shown = self._display_parts(self._param_display(param, candidate))[0]
            if shown is None:
                continue
            gap = abs(shown - target)
            if best_gap is None or gap < best_gap:
                best, best_gap = candidate, gap
        return best

    def _serialize_parameter(self, param, index, displays=True):
        """One parameter as the server renders it: the float Live automates,
        and the string Live shows a person. `displays` is off for the snapshot
        and the rack-chain walk, where the strings are never read and Live
        would pay three str_for_value calls per parameter."""
        entry = {
            "index": index,
            "name": "%s" % param.name,
            "value": float(param.value),
            "min": float(param.min),
            "max": float(param.max),
            "is_enabled": bool(getattr(param, "is_enabled", True)),
            "is_quantized": bool(getattr(param, "is_quantized", False)),
        }
        if displays:
            display = self._param_display(param)
            if display is not None:
                entry["display"] = display
                # kept under the old key too, so an older server still reads it
                entry["value_string"] = display
            items = self._param_items(param)
            if items:
                entry["items"] = items
            else:
                show_lo = self._param_display(param, entry["min"])
                show_hi = self._param_display(param, entry["max"])
                if show_lo is not None:
                    entry["display_min"] = show_lo
                if show_hi is not None:
                    entry["display_max"] = show_hi
        try:
            entry["automation_state"] = int(param.automation_state)
        except Exception:
            pass
        return entry

    def _serialize_device(self, device, device_index, include_params=True, depth=0):
        info = {
            "index": device_index,
            "name": device.name,
            "class_name": device.class_name,
            "type": self._get_device_type(device),
        }
        automated = self._automated_params_for_device(device)
        if automated:
            info["automated_parameters"] = automated
            info["automation_enabled"] = True
        else:
            info["automation_enabled"] = False

        if include_params:
            params = []
            try:
                for p_index, param in enumerate(device.parameters):
                    try:
                        params.append(self._serialize_parameter(param, p_index, displays=False))
                    except Exception:
                        continue
            except Exception as e:
                self.log_message("Error reading device parameters: " + str(e))
            info["parameters"] = params

        # Devices inside a rack carry the actual sound design — a drum rack's
        # nested Operator, an instrument rack's filter. Without this walk a rack
        # contributes only its 8 macros and the timbral state is invisible.
        if getattr(device, "can_have_chains", False):
            if depth >= self._MAX_CHAIN_DEPTH:
                info["chains_truncated"] = True
            else:
                info["chains"] = self._serialize_chains(
                    device, include_params=include_params, depth=depth
                )
        return info

    def _serialize_chains(self, rack, include_params=True, depth=0):
        chains = []
        try:
            chain_lists = [("chains", getattr(rack, "chains", []))]
            returns = getattr(rack, "return_chains", None)
            if returns:
                chain_lists.append(("return_chains", returns))

            for kind, chain_list in chain_lists:
                for chain_index, chain in enumerate(chain_list):
                    entry = {
                        "index": chain_index,
                        "kind": kind,
                        "chain_name": self._safe_attr(chain, "name", str, ""),
                        "mute": bool(self._safe_attr(chain, "mute", bool, False)),
                        "solo": bool(self._safe_attr(chain, "solo", bool, False)),
                    }
                    try:
                        mixer = chain.mixer_device
                        entry["volume"] = float(mixer.volume.value)
                        entry["panning"] = float(mixer.panning.value)
                    except Exception:
                        pass

                    # Drum racks expose the pad's note, which is what ties a
                    # nested device back to the kick/snare/hat it voices.
                    note = self._safe_attr(chain, "out_note", int, None)
                    if note is not None:
                        entry["out_note"] = note

                    nested = []
                    try:
                        for d_i, dev in enumerate(chain.devices):
                            nested.append(
                                self._serialize_device(
                                    dev,
                                    d_i,
                                    include_params=include_params,
                                    depth=depth + 1,
                                )
                            )
                    except Exception as e:
                        self.log_message("Error reading chain devices: " + str(e))
                    entry["devices"] = nested
                    chains.append(entry)
        except Exception as e:
            self.log_message("Error serializing rack chains: " + str(e))
        return chains

    def _serialize_clip_common(self, clip):
        info = {
            "looping": bool(self._safe_attr(clip, "looping", bool, False)),
            "loop_start": self._safe_attr(clip, "loop_start", float, None),
            "loop_end": self._safe_attr(clip, "loop_end", float, None),
            "warping": bool(self._safe_attr(clip, "warping", bool, False)),
            "warp_mode": self._safe_attr(clip, "warp_mode", int, None),
            "gain": self._safe_attr(clip, "gain", float, None),
            "pitch_coarse": self._safe_attr(clip, "pitch_coarse", int, None),
            "pitch_fine": self._safe_attr(clip, "pitch_fine", int, None),
            "launch_mode": self._safe_attr(clip, "launch_mode", int, None),
        }
        for attr in ("file_path", "file_path_relative"):
            path = self._safe_attr(clip, attr, str, None)
            if path:
                info["file_path"] = path
                break
        markers = self._warp_markers_from_clip(clip)
        if markers:
            info["warp_markers"] = markers
            info["warp_marker_count"] = len(markers)
        return dict((k, v) for k, v in info.items() if v is not None)

    def _serialize_session_clip(self, clip, include_notes=True):
        info = {
            "name": clip.name,
            "length": float(clip.length),
            "is_playing": bool(clip.is_playing),
            "is_recording": bool(getattr(clip, "is_recording", False)),
            "is_midi_clip": bool(getattr(clip, "is_midi_clip", False)),
            "is_audio_clip": bool(getattr(clip, "is_audio_clip", False)),
            "color": int(getattr(clip, "color", 0)),
        }
        info.update(self._serialize_clip_common(clip))
        if include_notes and info["is_midi_clip"]:
            info["notes"] = self._notes_from_clip(clip)
            info["note_count"] = len(info["notes"])
        return info

    def _serialize_arrangement_clip(self, clip, include_notes=True):
        info = {
            "name": clip.name,
            "start_time": float(clip.start_time),
            "end_time": float(clip.end_time),
            "length": float(clip.length),
            "color": int(getattr(clip, "color", 0)),
            "is_midi_clip": bool(getattr(clip, "is_midi_clip", False)),
            "is_audio_clip": bool(getattr(clip, "is_audio_clip", False)),
            "is_playing": bool(getattr(clip, "is_playing", False)),
        }
        info.update(self._serialize_clip_common(clip))
        if include_notes and info["is_midi_clip"]:
            info["notes"] = self._notes_from_clip(clip)
            info["note_count"] = len(info["notes"])
        return info

    def _serialize_sends(self, track):
        sends = []
        try:
            for i, send in enumerate(track.mixer_device.sends):
                sends.append({
                    "index": i,
                    "value": float(send.value),
                    "name": str(getattr(send, "name", "Send %d" % i)),
                })
        except Exception:
            pass
        return sends

    def _serialize_scenes(self):
        scenes = []
        try:
            for i, scene in enumerate(self._song.scenes):
                scenes.append({
                    "index": i,
                    "name": str(scene.name),
                    "tempo": self._safe_attr(scene, "tempo", float, None),
                    "is_triggered": bool(self._safe_attr(scene, "is_triggered", bool, False)),
                })
        except Exception as e:
            self.log_message("scenes serialize failed: " + str(e))
        return scenes

    def _serialize_cue_points(self):
        cues = []
        try:
            for cue in self._song.cue_points:
                cues.append({
                    "name": str(getattr(cue, "name", "")),
                    "time": float(getattr(cue, "time", 0.0)),
                })
        except Exception as e:
            self.log_message("cue_points serialize failed: " + str(e))
        return cues

    def _serialize_return_tracks(self, include_params=True):
        returns = []
        try:
            for i, track in enumerate(self._song.return_tracks):
                devices = []
                for d_i, device in enumerate(track.devices):
                    devices.append(
                        self._serialize_device(device, d_i, include_params=include_params)
                    )
                returns.append({
                    "index": i,
                    "name": track.name,
                    "mute": bool(track.mute),
                    "solo": bool(track.solo),
                    "volume": float(track.mixer_device.volume.value),
                    "panning": float(track.mixer_device.panning.value),
                    "devices": devices,
                })
        except Exception as e:
            self.log_message("return_tracks serialize failed: " + str(e))
        return returns

    def _serialize_master_track(self, include_params=True):
        """Master chain — the bus compressor/limiter that shapes the final sound."""
        try:
            track = self._song.master_track
            devices = []
            for d_i, device in enumerate(track.devices):
                devices.append(
                    self._serialize_device(device, d_i, include_params=include_params)
                )
            return {
                "volume": float(track.mixer_device.volume.value),
                "panning": float(track.mixer_device.panning.value),
                "devices": devices,
            }
        except Exception as e:
            self.log_message("master_track serialize failed: " + str(e))
            return None

    def _get_clip_notes(self, track_index, clip_index):
        try:
            if track_index < 0 or track_index >= len(self._song.tracks):
                raise IndexError("Track index out of range")
            track = self._song.tracks[track_index]
            if clip_index < 0 or clip_index >= len(track.clip_slots):
                raise IndexError("Clip index out of range")
            slot = track.clip_slots[clip_index]
            if not slot.has_clip:
                raise Exception("No clip in slot")
            clip = slot.clip
            if not getattr(clip, "is_midi_clip", False):
                raise Exception("Clip is not a MIDI clip")
            notes = self._notes_from_clip(clip)
            return {
                "track_index": track_index,
                "clip_index": clip_index,
                "clip_name": clip.name,
                "length": float(clip.length),
                "note_count": len(notes),
                "notes": notes,
            }
        except Exception as e:
            self.log_message("Error getting clip notes: " + str(e))
            raise

    def _get_device_parameters(self, track_index, device_index, kind="track"):
        """Generator: one device's parameters with Live's own display strings,
        on a track, a return or the master. It yields between parameters so a
        fifty-parameter device never holds the main thread for a whole slice."""
        track = self._resolve_track(track_index, kind)
        kind = str(kind or "track").lower()
        devices = list(track.devices)
        device_index = int(device_index)
        if device_index < 0 or device_index >= len(devices):
            raise IndexError("device index %d out of range ('%s' has %d device(s): %s)" % (
                device_index, track.name, len(devices),
                ", ".join("%s" % d.name for d in devices) or "none"))
        device = devices[device_index]
        info = {
            "index": device_index,
            "name": "%s" % device.name,
            "class_name": "%s" % device.class_name,
            "type": self._get_device_type(device),
        }
        automated = self._automated_params_for_device(device)
        if automated:
            info["automated_parameters"] = automated
        info["automation_enabled"] = bool(automated)
        params = []
        for p_index, param in enumerate(device.parameters):
            try:
                params.append(self._serialize_parameter(param, p_index))
            except Exception as e:
                self.log_message("parameter %d unreadable: %s" % (p_index, str(e)))
            yield None
        info["parameters"] = params
        if getattr(device, "can_have_chains", False):
            yield None
            info["chains"] = self._serialize_chains(device, include_params=True, depth=0)
        yield Done({
            "track_index": track_index if kind != "master" else 0,
            "kind": kind,
            "track_name": "%s" % track.name,
            "devices": ["%s" % d.name for d in devices],
            "device": info,
        })

    def _get_session_snapshot(self, include_notes=True, include_params=True):
        """Full v2 project state dump for trajectory dataset recording."""
        try:
            session = self._get_session_info()
            tracks = []
            for track_index, track in enumerate(self._song.tracks):
                clip_slots = []
                for slot_index, slot in enumerate(track.clip_slots):
                    clip_info = None
                    if slot.has_clip:
                        clip_info = self._serialize_session_clip(
                            slot.clip, include_notes=include_notes
                        )
                    clip_slots.append({
                        "index": slot_index,
                        "has_clip": bool(slot.has_clip),
                        "clip": clip_info,
                    })

                devices = []
                for device_index, device in enumerate(track.devices):
                    devices.append(
                        self._serialize_device(
                            device, device_index, include_params=include_params
                        )
                    )

                arrangement_clips = []
                try:
                    for clip in track.arrangement_clips:
                        arrangement_clips.append(
                            self._serialize_arrangement_clip(
                                clip, include_notes=include_notes
                            )
                        )
                except Exception as e:
                    self.log_message(
                        "arrangement_clips unavailable on track %d: %s"
                        % (track_index, str(e))
                    )

                tracks.append({
                    "index": track_index,
                    "name": track.name,
                    "is_audio_track": bool(track.has_audio_input),
                    "is_midi_track": bool(track.has_midi_input),
                    "mute": bool(track.mute),
                    "solo": bool(track.solo),
                    "arm": self._safe_arm(track),
                    "volume": float(track.mixer_device.volume.value),
                    "panning": float(track.mixer_device.panning.value),
                    "sends": self._serialize_sends(track),
                    "clip_slots": clip_slots,
                    "devices": devices,
                    "arrangement_clips": arrangement_clips,
                })

            return {
                "schema": "ableton_mcp_snapshot_v2",
                "session": session,
                "tracks": tracks,
                "scenes": self._serialize_scenes(),
                "return_tracks": self._serialize_return_tracks(
                    include_params=include_params
                ),
                "master_track": self._serialize_master_track(
                    include_params=include_params
                ),
                "cue_points": self._serialize_cue_points(),
                "include_notes": bool(include_notes),
                "include_params": bool(include_params),
            }
        except Exception as e:
            self.log_message("Error getting session snapshot: " + str(e))
            raise

    def _write_landing(self, param, asked, clamped):
        """What Live did with a write, read back off the parameter itself.

        Named apart from `_landing` (the bar a launch lands on) on purpose:
        the two shared a name until 1.33.1, and the later definition won, so
        `fire_clip`, `fire_scene`, `record_clip` and `start_live_capture`
        all raised TypeError inside Live.

        Live has three reasons to ignore or move a write, and a caller that
        is never told which one counts a step that did not happen as a step
        that did: the parameter is switched off right now (a rack macro owns
        it, or its device disabled it), an envelope automates it and will
        overwrite a manual write on the next playback tick, or it is
        quantized and snaps to its nearest step.
        """
        lo, hi = float(param.min), float(param.max)
        tol = max(1e-6, abs(hi - lo) * 1e-6)
        entry = {
            "asked": float(asked),
            "landed": bool(abs(float(param.value) - float(clamped)) <= tol),
            "is_enabled": bool(getattr(param, "is_enabled", True)),
            "is_quantized": bool(getattr(param, "is_quantized", False)),
        }
        try:
            entry["automation_state"] = int(param.automation_state)
        except Exception:
            pass
        return entry

    def _set_device_parameter(self, track_index, device_index, parameter_index,
                              value=None, kind="track", value_display=None):
        """One parameter, on a track, a return or the master. `value_display`
        is what Live shows ("200 Hz", "Low Cut 48 dB") and is resolved through
        Live's own strings; `value` is the raw number."""
        try:
            track = self._resolve_track(track_index, kind)
            kind = str(kind or "track").lower()
            devices = list(track.devices)
            device_index = int(device_index)
            if device_index < 0 or device_index >= len(devices):
                raise IndexError("device index %d out of range ('%s' has %d device(s))" % (
                    device_index, track.name, len(devices)))
            device = devices[device_index]
            params = list(device.parameters)
            parameter_index = int(parameter_index)
            if parameter_index < 0 or parameter_index >= len(params):
                raise IndexError("parameter index %d out of range ('%s' has %d)" % (
                    parameter_index, device.name, len(params)))
            param = params[parameter_index]
            old = float(param.value)
            old_display = self._param_display(param)
            if value_display is not None and ("%s" % value_display).strip() != "":
                target = self._value_from_display(param, value_display)
            elif value is None:
                raise ValueError("give value (the raw number) or value_display (what Live shows)")
            else:
                target = float(value)
            lo, hi = float(param.min), float(param.max)
            if target < lo - 1e-9 or target > hi + 1e-9:
                raise ValueError("'%s' takes %s to %s (%s to %s); %s is outside that" % (
                    param.name, lo, hi, self._param_display(param, lo),
                    self._param_display(param, hi), target))
            clamped = max(lo, min(hi, target))
            param.value = clamped
            out = {
                "track_index": track_index if kind != "master" else 0,
                "kind": kind,
                "track_name": "%s" % track.name,
                "device_index": device_index,
                "device": "%s" % device.name,
                "parameter_index": parameter_index,
                "name": "%s" % param.name,
                "old_value": old,
                "value": float(param.value),
                "min": lo,
                "max": hi,
            }
            if old_display is not None:
                out["old_display"] = old_display
            display = self._param_display(param)
            if display is not None:
                out["display"] = display
                out["value_string"] = display
            items = self._param_items(param)
            if items:
                out["items"] = items
            out.update(self._write_landing(param, target, clamped))
            return out
        except Exception as e:
            self.log_message("Error setting device parameter: " + str(e))
            raise

    def _set_device_parameters(self, track_index, device_index, values, kind="track"):
        """Several parameters of one device in one round trip: values is a
        list of {index, value}; each reply entry carries the name, the old
        and new value, the range and Live's display string."""
        try:
            track = self._resolve_track(track_index, kind)
            kind = str(kind or "track").lower()
            devices = list(track.devices)
            device_index = int(device_index)
            if device_index < 0 or device_index >= len(devices):
                raise IndexError("device index %d out of range ('%s' has %d device(s))" % (
                    device_index, track.name, len(devices)))
            device = devices[device_index]
            params = list(device.parameters)
            out = []
            for item in list(values or []):
                pi = int(item.get("index", -1))
                if pi < 0 or pi >= len(params):
                    raise IndexError("Parameter index %d out of range on '%s'" % (pi, device.name))
                param = params[pi]
                old = float(param.value)
                old_display = self._param_display(param)
                display_wanted = item.get("value_display")
                if display_wanted is not None and ("%s" % display_wanted).strip() != "":
                    new = self._value_from_display(param, display_wanted)
                else:
                    new = float(item.get("value", old))
                asked = new
                new = max(float(param.min), min(float(param.max), new))
                param.value = new
                entry = {"index": pi, "name": "%s" % param.name, "old_value": old,
                         "value": float(param.value), "min": float(param.min), "max": float(param.max)}
                if old_display is not None:
                    entry["old_display"] = old_display
                display = self._param_display(param)
                if display is not None:
                    entry["display"] = display
                    entry["value_string"] = display
                items = self._param_items(param)
                if items:
                    entry["items"] = items
                entry.update(self._write_landing(param, asked, new))
                out.append(entry)
            return {"track_index": track_index if kind != "master" else 0, "kind": kind,
                    "track_name": "%s" % track.name,
                    "device_index": device_index,
                    "device": "%s" % device.name, "class_name": "%s" % device.class_name,
                    "parameters": out}
        except Exception as e:
            self.log_message("Error setting device parameters: " + str(e))
            raise

    def _delete_device(self, track_index, device_index, kind="track"):
        """Take a device out of a chain. Live keeps it in its own undo history."""
        try:
            track = self._resolve_track(track_index, kind)
            kind = str(kind or "track").lower()
            devices = list(track.devices)
            device_index = int(device_index)
            if device_index < 0 or device_index >= len(devices):
                raise IndexError("device index %d out of range ('%s' has %d device(s): %s)" % (
                    device_index, track.name, len(devices),
                    ", ".join("%s" % d.name for d in devices) or "none"))
            if self._safe_attr(track, "is_frozen", bool, False):
                raise ValueError("'%s' is frozen; unfreeze it in Live before changing its devices" % track.name)
            name = "%s" % devices[device_index].name
            track.delete_device(device_index)
            return {"deleted": name, "index": device_index, "kind": kind,
                    "track_index": track_index if kind != "master" else 0,
                    "track_name": "%s" % track.name,
                    "devices": ["%s" % d.name for d in track.devices]}
        except Exception as e:
            self.log_message("Error deleting device: " + str(e))
            raise

    def _move_device(self, track_index, device_index, to_index, kind="track"):
        """Move a device inside its own chain, by index."""
        try:
            track = self._resolve_track(track_index, kind)
            kind = str(kind or "track").lower()
            devices = list(track.devices)
            device_index = int(device_index)
            if device_index < 0 or device_index >= len(devices):
                raise IndexError("device index %d out of range ('%s' has %d device(s))" % (
                    device_index, track.name, len(devices)))
            if self._safe_attr(track, "is_frozen", bool, False):
                raise ValueError("'%s' is frozen; unfreeze it in Live before changing its devices" % track.name)
            to_index = max(0, min(int(to_index), len(devices) - 1))
            device = devices[device_index]
            name = "%s" % device.name
            # Live inserts *before* the position it is given, counting the
            # device that is still in the chain. Moving later therefore needs
            # one more, so `to_index` means the index it ends up at.
            insert_at = to_index + 1 if to_index > device_index else to_index
            self._song.move_device(device, track, min(insert_at, len(devices)))
            after = ["%s" % d.name for d in track.devices]
            landed = to_index
            for i, n in enumerate(after):
                if n == name:
                    landed = i
                    break
            return {"moved": name, "from_index": device_index, "to_index": landed,
                    "kind": kind, "track_index": track_index if kind != "master" else 0,
                    "track_name": "%s" % track.name, "devices": after}
        except Exception as e:
            self.log_message("Error moving device: " + str(e))
            raise

    def get_browser_tree(self, category_type="all"):
        """
        Get a simplified tree of browser categories.
        
        Args:
            category_type: Type of categories to get ('all', 'instruments', 'sounds', etc.)
            
        Returns:
            Dictionary with the browser tree structure
        """
        try:
            # Access the application's browser instance instead of creating a new one
            app = self.application()
            if not app:
                raise RuntimeError("Could not access Live application")
                
            # Check if browser is available
            if not hasattr(app, 'browser') or app.browser is None:
                raise RuntimeError("Browser is not available in the Live application")
            
            # Log available browser attributes to help diagnose issues
            browser_attrs = [attr for attr in dir(app.browser) if not attr.startswith('_')]
            self.log_message("Available browser attributes: {0}".format(browser_attrs))
            
            result = {
                "type": category_type,
                "categories": [],
                "available_categories": browser_attrs
            }
            
            # Helper function to process a browser item and its children.
            # Folders are followed two levels down; deeper levels are counted
            # so the tree stays small while saying where the depth is.
            max_depth = 2

            def process_item(item, depth=0):
                if not item:
                    return None
                children = []
                try:
                    kids = list(item.children) if hasattr(item, 'children') else []
                except Exception:
                    kids = []
                result = {
                    "name": item.name if hasattr(item, 'name') else "Unknown",
                    "is_folder": bool(kids),
                    "is_device": hasattr(item, 'is_device') and item.is_device,
                    "is_loadable": hasattr(item, 'is_loadable') and item.is_loadable,
                    "uri": item.uri if hasattr(item, 'uri') else None,
                    "child_count": len(kids),
                    "children": children,
                }
                if depth < max_depth:
                    for kid in kids[:200]:
                        processed = process_item(kid, depth + 1)
                        if processed:
                            children.append(processed)
                return result
            
            # Process based on category type and available attributes
            if (category_type == "all" or category_type == "instruments") and hasattr(app.browser, 'instruments'):
                try:
                    instruments = process_item(app.browser.instruments)
                    if instruments:
                        instruments["name"] = "Instruments"  # Ensure consistent naming
                        result["categories"].append(instruments)
                except Exception as e:
                    self.log_message("Error processing instruments: {0}".format(str(e)))
            
            if (category_type == "all" or category_type == "sounds") and hasattr(app.browser, 'sounds'):
                try:
                    sounds = process_item(app.browser.sounds)
                    if sounds:
                        sounds["name"] = "Sounds"  # Ensure consistent naming
                        result["categories"].append(sounds)
                except Exception as e:
                    self.log_message("Error processing sounds: {0}".format(str(e)))
            
            if (category_type == "all" or category_type == "drums") and hasattr(app.browser, 'drums'):
                try:
                    drums = process_item(app.browser.drums)
                    if drums:
                        drums["name"] = "Drums"  # Ensure consistent naming
                        result["categories"].append(drums)
                except Exception as e:
                    self.log_message("Error processing drums: {0}".format(str(e)))
            
            if (category_type == "all" or category_type == "audio_effects") and hasattr(app.browser, 'audio_effects'):
                try:
                    audio_effects = process_item(app.browser.audio_effects)
                    if audio_effects:
                        audio_effects["name"] = "Audio Effects"  # Ensure consistent naming
                        result["categories"].append(audio_effects)
                except Exception as e:
                    self.log_message("Error processing audio_effects: {0}".format(str(e)))
            
            if (category_type == "all" or category_type == "midi_effects") and hasattr(app.browser, 'midi_effects'):
                try:
                    midi_effects = process_item(app.browser.midi_effects)
                    if midi_effects:
                        midi_effects["name"] = "MIDI Effects"
                        result["categories"].append(midi_effects)
                except Exception as e:
                    self.log_message("Error processing midi_effects: {0}".format(str(e)))
            
            # Try to process other potentially available categories
            for attr in browser_attrs:
                if attr not in ['instruments', 'sounds', 'drums', 'audio_effects', 'midi_effects'] and \
                   (category_type == "all" or category_type == attr):
                    try:
                        item = getattr(app.browser, attr)
                        if hasattr(item, 'children') or hasattr(item, 'name'):
                            category = process_item(item)
                            if category:
                                category["name"] = attr.capitalize()
                                result["categories"].append(category)
                    except Exception as e:
                        self.log_message("Error processing {0}: {1}".format(attr, str(e)))
            
            self.log_message("Browser tree generated for {0} with {1} root categories".format(
                category_type, len(result['categories'])))
            return result
            
        except Exception as e:
            self.log_message("Error getting browser tree: {0}".format(str(e)))
            self.log_message(traceback.format_exc())
            raise
    
    def get_browser_items_at_path(self, path):
        """
        Get browser items at a specific path.
        
        Args:
            path: Path in the format "category/folder/subfolder"
                 where category is one of: instruments, sounds, drums, audio_effects, midi_effects
                 or any other available browser category
                 
        Returns:
            Dictionary with items at the specified path
        """
        try:
            # Access the application's browser instance instead of creating a new one
            app = self.application()
            if not app:
                raise RuntimeError("Could not access Live application")
                
            # Check if browser is available
            if not hasattr(app, 'browser') or app.browser is None:
                raise RuntimeError("Browser is not available in the Live application")
            
            # Log available browser attributes to help diagnose issues
            browser_attrs = [attr for attr in dir(app.browser) if not attr.startswith('_')]
            self.log_message("Available browser attributes: {0}".format(browser_attrs))
                
            # Parse the path
            path_parts = path.split("/")
            if not path_parts:
                raise ValueError("Invalid path")
            
            # Determine the root category
            root_category = path_parts[0].lower()
            current_item = None
            
            # Check standard categories first
            if root_category == "instruments" and hasattr(app.browser, 'instruments'):
                current_item = app.browser.instruments
            elif root_category == "sounds" and hasattr(app.browser, 'sounds'):
                current_item = app.browser.sounds
            elif root_category == "drums" and hasattr(app.browser, 'drums'):
                current_item = app.browser.drums
            elif root_category == "audio_effects" and hasattr(app.browser, 'audio_effects'):
                current_item = app.browser.audio_effects
            elif root_category == "midi_effects" and hasattr(app.browser, 'midi_effects'):
                current_item = app.browser.midi_effects
            else:
                # Try to find the category in other browser attributes
                found = False
                for attr in browser_attrs:
                    if attr.lower() == root_category:
                        try:
                            current_item = getattr(app.browser, attr)
                            found = True
                            break
                        except Exception as e:
                            self.log_message("Error accessing browser attribute {0}: {1}".format(attr, str(e)))
                
                if not found:
                    # If we still haven't found the category, return available categories
                    return {
                        "path": path,
                        "error": "Unknown or unavailable category: {0}".format(root_category),
                        "available_categories": browser_attrs,
                        "items": []
                    }
            
            # Navigate through the path
            for i in range(1, len(path_parts)):
                part = path_parts[i]
                if not part:  # Skip empty parts
                    continue
                
                if not hasattr(current_item, 'children'):
                    return {
                        "path": path,
                        "error": "Item at '{0}' has no children".format('/'.join(path_parts[:i])),
                        "items": []
                    }
                
                found = False
                for child in current_item.children:
                    if hasattr(child, 'name') and child.name.lower() == part.lower():
                        current_item = child
                        found = True
                        break
                
                if not found:
                    return {
                        "path": path,
                        "error": "Path part '{0}' not found".format(part),
                        "items": []
                    }
            
            # Get items at the current path
            items = []
            if hasattr(current_item, 'children'):
                for child in current_item.children:
                    item_info = {
                        "name": child.name if hasattr(child, 'name') else "Unknown",
                        "is_folder": hasattr(child, 'children') and bool(child.children),
                        "is_device": hasattr(child, 'is_device') and child.is_device,
                        "is_loadable": hasattr(child, 'is_loadable') and child.is_loadable,
                        "uri": child.uri if hasattr(child, 'uri') else None
                    }
                    items.append(item_info)
            
            result = {
                "path": path,
                "name": current_item.name if hasattr(current_item, 'name') else "Unknown",
                "uri": current_item.uri if hasattr(current_item, 'uri') else None,
                "is_folder": hasattr(current_item, 'children') and bool(current_item.children),
                "is_device": hasattr(current_item, 'is_device') and current_item.is_device,
                "is_loadable": hasattr(current_item, 'is_loadable') and current_item.is_loadable,
                "items": items
            }
            
            self.log_message("Retrieved {0} items at path: {1}".format(len(items), path))
            return result
            
        except Exception as e:
            self.log_message("Error getting browser items at path: {0}".format(str(e)))
            self.log_message(traceback.format_exc())
            raise
