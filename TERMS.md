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
to anything but Live.

## What is stored on your machine

| What | Where | Default | Off switch |
|---|---|---|---|
| **Activity log** — one line per tool call: the tool's name, which Live commands it sent, how long it took, whether it succeeded, the error text if not, and the *size* of what went in and out | `~/.ableton-music-maker/activity/<session>.jsonl` | On | `ABLETON_MCP_ACTIVITY=false`, or the switch in the Mac app |
| **Payloads** — the parameters and results themselves, which contain your MIDI notes and the names you give tracks and clips | same files | **Off** | `ABLETON_MCP_ACTIVITY_PAYLOADS=true` turns it on; leave it unset to keep it off |
| **Heartbeat** — that a server is running, which client started it, and the versions involved | `~/.ableton-music-maker/sessions/<pid>.json` | On | Removed automatically when the server exits |

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

## Deleting it

Delete the `~/.ableton-music-maker` folder, or use **Delete all local data** in the Mac
app. The app also discards activity files older than seven days by default. Nothing needs
to be requested from anyone, because nobody else has a copy.

## Changes

Material changes get a new date above and a note in the release notes. The terms in effect
for you are the ones shipped with the version you run.
