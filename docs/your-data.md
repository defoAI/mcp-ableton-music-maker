# Your data

Nothing is uploaded, by the server or the app; there is no code that could. What is kept, all under `~/.ableton-music-maker/`:

| | Default | Off switch |
|---|---|---|
| Activity log: tool names, the Live commands sent, timings, sizes, results | on | `ABLETON_MCP_ACTIVITY=false`, or the app |
| Parameters and results, which contain your MIDI and names | **off** | `ABLETON_MCP_ACTIVITY_PAYLOADS=true` turns it on |
| The server's copy of Live's browser: names, paths, URIs | on | `ABLETON_MCP_LIBRARY_INDEX=false` keeps it in memory |
| A set exported as a document | **only on `export_set`** | delete the file |
| Listening in the app | off until you start it | stops the moment no window shows it; never written |
| **Remember what Live plays**: measurements of what Live played, per bar, per 16th and per frame, stamped with bar, section and revision — **measurements are written, audio is not** (about 3 KB a second) | **off** until you turn it on | the switch in the app; kept 30 days (a setting) |
| Comments, picks and replies in the app's Feedback screen, and each set's change log | only when used | **Delete all local data** |
| Claude playing the set to measure it | **only while "Claude may play the set" is on** | turns off when you press play, or after 20 minutes |

Captures and takes are your project's own recordings and are never copied. **Delete all local data** in the app removes everything above. The whole story, in plain words: [TERMS.md](../TERMS.md).

---

[← README](../README.md)
