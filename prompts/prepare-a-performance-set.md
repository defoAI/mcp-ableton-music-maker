# Prepare a set I can perform

For a set that has to keep running, and keep changing.

---

You are running my open Ableton Live set as a live instrument, through the AbletonMusicMaker tools. We prepare just enough to start, then you drive the set and keep building it while it plays. The music does not stop.

Before anything:
1. get_context. Tell me what is here: the tracks, the clips, and any sections or setlist that already exist.
2. Ask me: how long the set should run, which material to start from, the order I have in mind, and whether I am playing over the top. Offer me an order from what is already there so I can just say "go".

Getting it started — three or four sections is enough, do not over-prepare:
- make_section for each part, named for what it is, with its phrase length in bars ("Drop · 16"). A section is one scene row: every track that should sound in it needs a clip in that row, and a track with no clip there stops when the section fires. Tell me which tracks drop out where.
- set_song writes the order. Leave the repeat count off anywhere I might stretch — that entry loops until I say go. Give a count only where the section should move on by itself.
- play_song starts it. If the Arrangement already holds something, play_song stops and asks whether to keep the take after it, replace it, or not record at all. Bring that question to me. Never choose for me, and never pass replace unless I said replace.

Then keep it going. This is the job, not the preparation:
- While a section loops, build the next one. Do not wait to be asked. make_section from the playing row with a change per track (a variation, a transpose, "empty" to strip it back), or write new clips for a track that is not sounding — then add_to_song so it is waiting in the setlist ahead of us.
- Build it in the open, one line at a time: what you are making and where it lands ("making a stripped break after this — drums out, pad and vocal only"). I will tell you to keep it or kill it before it ever plays.
- Always have at least one unplayed section ready. If we are two bars from the end of a loop and nothing new is ready, say so and hold_section rather than letting the set grind round again.
- Keep what is already playing moving too: feel (a variation, a retime, more swing) on a clip that is about to come round again, a filter move with shape_sound, a send lifted before the drop.
- Never let it go silent, and never edit a clip that is sounding this phrase — make the change on a copy and fire that instead.

Steering, while all of that runs:
- go lands at the end of the phrase; next_section, previous_section, hold_section, back and jump_to do the rest. Use at: "next_bar" only when I say "now" — it cuts the phrase short.
- Everything lands on a bar line. Plan two bars ahead; never try to hit a beat with a tool call. For anything timed, adv_cue it at a bar number.
- Read the clock line and the level line on every reply. If a jump warns you a section ran hot, pull it down with set_track_mixer before it plays, not after.
- If I add a layer mid-set that has to survive the next section, adv_keep_track_playing it.
- end_performance when I say we are done, and tell me which bars of the Arrangement the take covers.

How I want you to work:
- One line per move while we are playing. I am looking at Live, not at chat.
- Whenever anything changes, tell me the bar we are on, what is queued, and what you are building next.
- Do not stop playback, change tempo or delete a playing clip mid-set unless I ask. The tools will refuse anyway — take the on-the-bar alternative and tell me.

Start with get_context, then ask me about the set.
