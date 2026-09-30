# MCP Ableton Music Maker — your data

_Last updated: 21 September 2026_

The software is licensed under the [GNU LGPL v3.0 or later](LICENSE). This page is about data, and it is short
because there is almost none.

## Nothing is uploaded

The server (`ableton-music-maker`) opens exactly one kind of network connection: the one to
Ableton Live on your own machine (port 9877). It has no code that can send anything
anywhere else — no telemetry, no analytics, no crash reports, no dataset. This is checked
on every build: the continuous-integration run fails if an HTTP client library enters the
dependency tree, and the Docker image is verified to carry no trace of one.

The Mac app is the same: it reads files the server writes on your Mac and never connects
to anything but Live. When you ask it to, it also listens to Live's audio, in memory (see
**Listening** below).

The Remote Script inside Live accepts connections from this machine only, unless you put
another address in a `bind_host.txt` file beside it.

## What is stored on your machine

| What | Where | Default | Off switch |
|---|---|---|---|
| **Activity log** — one line per tool call: the tool's name, which Live commands it sent, how long it took and how long those commands held Live's main thread, whether it succeeded, the error text if not, and the *size* of what went in and out | `~/.ableton-music-maker/activity/<session>.jsonl` | On | `ABLETON_MCP_ACTIVITY=false`, or the switch in the Mac app |
| **Payloads** — the parameters and results themselves, which contain your MIDI notes and the names you give tracks and clips | same files | **Off** | `ABLETON_MCP_ACTIVITY_PAYLOADS=true` turns it on; leave it unset to keep it off |
| **Heartbeat** — that a server is running, which client started it, and the versions involved | `~/.ableton-music-maker/sessions/<pid>.json` | On | Removed automatically when the server exits |
| **Sample folders** — the paths you told Claude to look in for samples (not Live's own folders, which need no telling) | `~/.ableton-music-maker/sample_folders.json` | **Only when you ask** (`adv_sample_folders add`) | Delete the file, or "Delete all local data" |
| **Set exports** — a whole set as a document: your track and clip names, every MIDI note, mixer values, sections and setlist | `~/.ableton-music-maker/sets/<name>.json` | **Only when you ask** (`export_set`, or `build_song` with `snapshot: true`). The same document can also be *returned to you in a reply* (`get_context {"as": "document"}`), which writes nothing at all | Delete the file or the folder, or "Delete all local data" |

Error text can contain a name you typed (a track called "Nick's bass", say). That is why the
log is yours to delete.

In the Docker image every path above sits under `/state`, the only writable volume.

## The one file written outside that folder

There is exactly one: **the Remote Script's handlers, inside Live's own User Library**
(`…/Ableton/User Library/Remote Scripts/AbletonMusicMaker/body.py`).

The Remote Script is two files. One of them, `__init__.py`, Live reads only when it starts.
The other, `body.py`, holds the handlers, and Live can be told to re-read it while it runs.
So when the server finds Live running an older copy of the handlers than the one it ships
with, it writes its own copy over that file and tells Live to re-read it — which is why a
new version of this software takes effect without you being asked to quit Ableton and lose
an unsaved set.

It writes that file and no other. It never writes `__init__.py` behind a running Live, it
writes only its own bundled copy — never anything from the network, which it has no way to
reach — and it does so only when the rest of the script already matches. Nothing about your
music is involved: the file is program code, the same for everyone, and it is the same file
`ableton-music-maker-install-script` puts there.

`ABLETON_MCP_AUTO_UPDATE_SCRIPT=false` turns this off. Then a version mismatch tells you to
run the installer yourself, as it did before.

## Captures

When Claude is asked to capture a section, the server records it **inside Live**: a Capture
track whose input is Resampling records a clip of the master, exactly as if you had pressed
record. The audio lands where Live puts its recordings, in your project's `Samples/Recorded`
folder. The server reads that file once to measure it (peak, RMS per bar, silence, clipping,
stereo correlation), writes nothing, and the Mac app plays it in place. Captures are your
project's files: nothing here copies, uploads or deletes them, and "Delete all local data"
leaves them alone.

## Listening

The Mac app's Listen screen hears what Live is sending to your speakers through a macOS
process tap of Live alone: not the microphone, not other apps, not the system. macOS asks
you once whether the app may record audio from other apps, and you can take that back any
time under System Settings › Privacy & Security › Screen & System Audio Recording. The
audio is analysed in memory about thirty times a second — a spectrum, the master level, six
range levels and a short waveform — and thrown away. It is never written to disk, never
sent anywhere, and it stops the moment no window is showing it. The only thing stored is
where you left the small float window.

## What the server hears from Live

The server and the Remote Script share one connection, and Live can speak on it without
being asked. The server has to ask first: it subscribes to named channels, and the script
sends nothing on a channel nobody subscribed to. There are four, and this is all of what
they carry:

| Channel | What arrives | How often |
|---|---|---|
| `clock` | the bar, the beat in the bar, the tempo, whether the transport is playing | while playing, at the rate the server asked for |
| `levels` | Live's own output meter for each track and the master, on Live's 0-to-1 meter scale | once a bar |
| `changes` | a track or clip name, a mute, a solo or a count that differs from the last look, with the value before and after | only when something differs |
| `cue` | that a scheduled step fired, and how late it was | when a step fires |

Two things are worth saying plainly. The script computes `changes` by **reading** the set on
Live's own tick and comparing it with the previous read — it registers no listener on any
Live object, which is what makes it safe to run while you work. And none of this is written
down: events live in memory in the running server, are handed to whatever asked for them,
and are gone. Nothing in this section reaches the activity log, and nothing leaves your
machine — see **Nothing is uploaded** above.

## Library index

To answer "find me an analog bass" without a round trip to Live, the server keeps its own
copy of Live's browser: the names, folder paths and URIs of the loadable items Live showed
the Remote Script — Ableton's own devices and packs, and the names of the third-party
plug-ins and Max for Live devices Live's browser lists. No audio, no notes, no project
content. It lives in memory and, by
default, as one JSON file per Live library under `~/.ableton-music-maker/library/`, so a
new server process is ready at once. `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory
only. "Delete all local data" removes the folder; it is rebuilt in the background when the
server next talks to Live.

Beside it, under `~/.ableton-music-maker/devices/`, is what the **devices themselves
answered to**: each device's parameter names as Live reports them, and the values that
were written with the string Live displayed for each, stamped with the Live version and
the day they were measured. It is one file per Live version. This is how Claude stops
paying twice for the same failed call — that a particular rack has no "cutoff", or that a
macro runs the other way round, is a fact about Ableton's own content, true in every set
anyone builds, so it is kept against the device and never against your song. It holds no
note, no audio, no path, and nothing about any piece of music you are making. The same
`ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory only; `adv_device_vocabulary` shows
you all of it in one call and `adv_device_vocabulary(action: "forget")` deletes it.

The same folder holds the **sample index**, built the first time Claude is asked for a
sample and never before: the name, folder, path, type and length of every audio file in the
folders Live names — the Core Library, your Packs, your User Library, the open set's own
folder — plus any you added yourself. Live says where those folders are; the server reads
them, and for a WAV or AIFF it reads the first few bytes of each file to learn how long it
is. That is the header, never the audio: nothing decodes, plays, copies or moves a sample,
and a sample Claude puts in your set is referenced where it lies, exactly as if you had
dragged it in. The same `ABLETON_MCP_LIBRARY_INDEX=false` keeps this index in memory only.

## Song memory

This is the most content-bearing thing the server writes, and it is on by default, so read this
one twice.

While Claude works on a song it keeps a written model of it: **what the track is trying to be,
the plan section by section, what each track is for, what you decided and why, and what the next
move is** — plus any note you tell it to keep ("the Smoke Bass is mine, never regenerate it") and
a one-line digest per session (the date, which tracks and sections were worked on, and the names
of the tools that ran). That is prose about your unreleased music, written by Claude rather than
typed by you. It is capped at 8 KB per song and lives in one JSON file per song under
`~/.ableton-music-maker/songs/`, identified by the path of the Live Set it belongs to — which is
the only path the file ever holds.

What is **not** in it: no MIDI note, no audio, no path outside your set's own. A track's **role**
(`Sitar [lead]`) is in the track's name and a parked idea is a clip in a `Stash:` scene row — both
are in your own Live set, kept by your own Cmd+S, and neither is copied here. Delete this file and
they are still there.

On by default is defensible only because it is local, capped, inspectable in one call and
deletable in one call. `ABLETON_MCP_SONG_MEMORY=false` stops every write to the folder;
`adv_song_memory` shows you the whole file and what is not in it, and
`adv_song_memory(action: "forget")` deletes it. So does "Delete all local data" in the Mac app, or
deleting the folder. Nothing uploads — see **Nothing is uploaded** above.

## Set exports

When Claude is asked to export a set, the server writes one JSON file under
`~/.ableton-music-maker/sets/`: the tracks with their device names and instrument URIs,
every Session clip with its notes, the mixer and sends, the sections and the setlist, tempo,
signature and key. That is your music, not metadata, which is why it is written only on an
explicit `export_set` call, or a `build_song` you gave `snapshot: true`, and never on its own;
nothing else the server does creates the folder. `import_set` reads it back into an empty set.

The same document can be handed back to you *in a reply* instead — `get_context {"as":
"document"}`, which `update_song` takes back — and that writes nothing at all: it goes to the
app you are already talking to, like every other answer. To answer twice without reading your
whole set twice, the server keeps the notes of your Session clips **in its own memory** while
it runs, checks them against Live before it uses them, and forgets them when it stops. They
are never written to disk.

The Live set itself is the memory
(sections and the setlist are scene names, kept by Live's own Save); an export is a backup
you asked for. Delete the file, the folder, or use "Delete all local data" in the Mac app,
which removes the exports along with the library index.

## Deleting it

Delete the `~/.ableton-music-maker` folder, or use **Delete all local data** in the Mac
app. The app also discards activity files older than seven days by default. Nothing needs
to be requested from anyone, because nobody else has a copy.

## Changes

Material changes get a new date above and a note in the release notes. The terms in effect
for you are the ones shipped with the version you run.
