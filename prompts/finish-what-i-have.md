# Finish what I have already got

For the set that has been open for three weeks.

---

You are working in my open Ableton Live set through the AbletonMusicMaker tools. There is already material here. I want it finished, not restarted.

First, look, and be honest:
1. get_context: every track and its devices, the clips in every slot, the sections and the setlist, the key and the tempo.
2. Tell me in a short list what is here and what state it is in — which parts are finished, which are sketches, what is empty, and what clashes (two basses, a track with no clip in half the sections).
3. Then ask me what "finished" means for this one: an arrangement I can bounce, a loop to keep, a demo to send, or something to perform. And ask what is untouchable.

Then, in this order, checking with me between stages:
- Fix the foundation: set_key if the set is still sitting in Live's default C Major, set_tempo if the material disagrees with it.
- Fill the holes. A section that needs a part gets one — create_clip and add_notes_to_clip, or make_section from what is playing. Reach for feel (swing, humanize, groove) before you add more notes: a stiff part usually needs feel, not more parts.
- Arrange it: arrange places the Session clips along the timeline in bars, and create_locator marks each part so I can find it in Live.
- Then mix with measurements, not adjectives: capture_mix over 8 bars of the loudest section, read peak, RMS per bar, crest and the octave bands back to me, fix what it shows with set_track_mixer in dB, set_send and shape_sound, and capture again to prove the fix. clear_captures when we are done.
- Finally, tell me what you did not touch, and why.

How I want you to work:
- Do not rewrite my parts. If a part is wrong, say so and ask before you replace it.
- Keep every change to one undo step where you can (feel takes undo: true), so I can put anything back.
- Two or three lines per stage. Let me hear it and say keep or change.
- Live's API cannot save my set: when it is where I want it, tell me once to press Cmd+S. export_set only if I ask for a copy of the document.

Start with get_context and tell me what you found.
