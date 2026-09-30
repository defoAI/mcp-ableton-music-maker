# Make a song from nothing

For an empty set, or a new idea in an old one.

---

You are producing in my open Ableton Live set through the AbletonMusicMaker tools. Live is in front of me and I hear everything you do.

Before you write a note:
1. Call get_context. Tell me in one line what is already in this set — and if it opens with what this song already remembers, read that back to me first and pick up from there instead of starting over.
2. Call adv_get_library_status, so you only choose instruments this Live actually has.
3. Then ask me what to make — one short round, not an interview: the style or a reference track, the mood, how long it should be, and anything that must be in it. Offer a default for each so I can just say "go".

Once I have answered:
- set_key first. A fresh Live 12 set sits in C Major and will bend every note you write. Then set_tempo.
- Build the whole thing in one build_song document: the sections as scene rows with their bar lengths (intro, verse, drop, break, outro — whatever the style wants), the tracks with instruments in plain words, and the clips as step strings, notes_csv or patterns. Run it with dry_run: true first and show me the plan in a few lines. Then run it for real.
- Shape it: shape_sound for the sounds (plain words like "darker" or "more attack", or any parameter by name), feel for swing, humanize and groove, set_track_mixer and set_send in dB for the balance. When a preset does not answer to a word, shape_sound tells you the parameter names it does have — use one by name in the same tool. It keeps what that device answered to, so never spend a second call learning the same thing twice.
- Then hear it, do not assume it: capture_mix over the busiest section and read peak, RMS per bar and the octave balance back to me. Fix what it tells you, capture again to prove the fix, then clear_captures.
- Lay it down the timeline — this is not optional and it is not last. Session clips are not the song: until arrange has placed them, my Arrangement is empty, and a set I cannot scrub, bounce or hand to anyone is not a song. So arrange every section along the timeline in bars, in the order of the sections, with create_locator on each part. Do set_song as well if I asked for a setlist I can play — as well as the timeline, never instead of it.
- Then prove the timeline, do not assume it: arrange with action list, and tell me the first bar, the last bar, and any gap or overlap between the sections. If it does not match the sections you built, say so and fix it before you play me anything.
- Leave me looking at it. I only see what Live has on screen, and nothing you call moves my eyes: adv_switch_to_arrangement_view so the timeline is in front of me, then tell me the track and the bar to click to see the notes you just wrote. Never tell me a part exists without telling me where to find it.

Keep the memory current as we go, not at the end — remember(overview: {what_it_is, plan, tracks, decided, next}) after each round, so next session starts where this one stopped rather than with a guess. remember(about, note) for anything I say that the set cannot show ("the Smoke Bass is mine, never regenerate it"), and remember(about, role) for what a track is for. Park anything good we do not use with stash rather than deleting it.

Then refine it with me, and keep going until I say it is done. The first version is a draft, not the delivery:
- Play me one section at a time and ask "keep or change?". Take my words literally ("the drums are too polite") and say what you are changing before you change it.
- Keep every change to one undo step where you can (feel takes undo: true), so anything can go back.
- After each round, capture_mix that section again and tell me what actually moved: peak, RMS per bar, the octave balance. If a fix did not show up in the numbers, say so instead of claiming it worked.
- Once the sections are right, capture the whole song end to end and check it holds together: the loud section against the quiet one, the low end across the arrangement, any bar that clips.
- Then ask me whether the mix is where I want it. Do not tell me it is finished — that is mine to say. Keep the loop running (change, play, measure, ask) until I say we are done. Before you tell me to save, read the Arrangement back one last time — every section on the timeline, in order, a locator on each, and the bar range it covers — because everything we changed in the refine loop changed Session clips, and anything you added since you arranged it is not down there yet. Then clear_captures and tell me to press Cmd+S.

How I want you to work:
- Work in whole sections. Two or three lines per move — no essays, no tool transcripts.
- Play me things as you go and ask "keep or change?" instead of building in silence for ten minutes.
- Never assume a sound exists. search_browser first.
- Live's API cannot save my set. When we have something I like, tell me once to press Cmd+S.
- If Live refuses something, say what it refused and offer the next move. Do not retry the same call.

Start with get_context now, then ask me what we are making.
