# MCP Ableton Music Maker — your data

_Last updated: 19 September 2026_

The software is licensed under [MIT](LICENSE). This page is about data, and it is short
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
| **Set exports** — a whole set as a document: your track and clip names, every MIDI note, mixer values, sections and setlist | `~/.ableton-music-maker/sets/<name>.json` | **Only when you ask** (`export_set`) | Delete the file or the folder, or "Delete all local data" |

Error text can contain a name you typed (a track called "Nick's bass", say). That is why the
log is yours to delete.

In the Docker image every path above sits under `/state`, the only writable volume.

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

## Library index

To answer "find me an analog bass" without a round trip to Live, the server keeps its own
copy of Live's browser: the names, folder paths and URIs of the loadable items Live showed
the Remote Script. No audio, no notes, no project content. It lives in memory and, by
default, as one JSON file per Live library under `~/.ableton-music-maker/library/`, so a
new server process is ready at once. `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory
only. "Delete all local data" removes the folder; it is rebuilt in the background when the
server next talks to Live.

## Set exports

When Claude is asked to export a set, the server writes one JSON file under
`~/.ableton-music-maker/sets/`: the tracks with their device names and instrument URIs,
every Session clip with its notes, the mixer and sends, the sections and the setlist, tempo,
signature and key. That is your music, not metadata, which is why it is written only on an
explicit `export_set` call and never on its own; nothing else the server does creates the
folder. `import_set` reads it back into an empty set. The Live set itself is the memory
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
