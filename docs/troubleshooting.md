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

## Known issue: `capture_mix` records Session loops instead of the Arrangement (2026-10-12)

Seen on a 9-track set built by voidmusic (`Bb86_Polaroid`, Live 12.4.6, script 1.38.0): every
`capture_mix` take recorded the tracks' Session clips rather than the Arrangement. Bars 1–8 (no drums
or bass in the Arrangement) came back with the Break and Bass Session loops; soloed per-track takes
showed Break/Bass playing and Crackle (whose only clips are in the Arrangement) silent. The capture
reply itself says "The tracks are following Session clips". Calling `adv_back_to_arrangement`
immediately before each take did not change it; after a few takes every track came back silent.
`_start_capture` only fires the Capture track's slot (`slot.fire(record_length=…)`), so the
switch-over seems to come from firing a Session slot while tracks hold Session clips — worth testing
`song.back_to_arranger = False` after the fire, or recording into the Arrangement instead.
Workaround used: Live's File → Export Audio/Video of the Arrangement.

**Fixed in Remote Script 1.39.0 (2026-10-15).** Measured on Live 12.4.6: firing the Capture track's
slot takes that track off the Arrangement and lights Back to Arrangement for the whole set, and a
seek made while the transport is stopped is ignored by `continue_playing()`, so takes could start
from wherever Live last stopped. A capture now starts the transport exactly where it is asked, puts
any track left on Session clips back on the Arrangement before recording (and names it), refuses a
take recorded while a track was off the Arrangement or at the wrong bar, and puts the Capture track
back on the Arrangement afterwards. A server that embeds an older body writes it back over 1.39.0
when it starts, so update the server (and the Mac app, which bundles one) as well as the script.
