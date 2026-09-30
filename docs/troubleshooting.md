# When it goes wrong

| | |
|---|---|
| **"the Remote Script cannot run this command" — and the message offers `--reload`** | Only the script's handlers are behind. The server usually fixes this itself at startup, in place, with nothing restarted; if it did not, run `ableton-music-maker-install-script --reload` and keep playing. `ABLETON_MCP_AUTO_UPDATE_SCRIPT=false` is what turns the automatic half off. |
| **"the Remote Script cannot run this command" — and the message says restart** | The script's *loader* is behind, and Live only reads that file when it starts. Install again (the app's Setup offers **Update Remote Script**), restart Live, re-select the control surface. `ableton-music-maker --check` prints both versions and which half is behind. |
| **"could not connect to Ableton"** | Live is not running, or AbletonMusicMaker is not selected as a control surface. |
| **The server is on another machine, or a container cannot reach the host** | The script binds `127.0.0.1` by default. Put the address to bind on the first line of `bind_host.txt` beside the script's `__init__.py` and restart Live. `--check` prints what it bound. |
| **Two servers** | Only one should talk to Live. Remove the extra client entry; if it is the original AbletonMCP, the app offers to. |
| **A timeout** | Ask for less at once. Making many tracks is allowed 190 s, importing audio 65 s, a browser search 25 s, other changes 15 s, reads 10 s. |
| **Listen shows nothing while Live plays** | macOS hands over silence when the audio-capture permission is off. Allow the app under **System Settings → Privacy & Security → Screen & System Audio Recording**. |

Diagnostics go to stderr; `RUST_LOG=debug` traces every command. `ableton-music-maker --status` prints versions and paths.

---

[← README](../README.md)
