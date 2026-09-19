# 0006 — One artist-facing surface; the raw layer stays served, named `adv_`

| | |
|---|---|
| **Status** | **Decided — the artist's set is the surface; every other tool is served as `adv_<name>`; nothing is hidden** |
| **Raised** | 2026-09-19 (the artist's review of the 96-tool list after the sections-and-songs story) |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | Every story and issue that adds a tool: which layer it goes in, what units it speaks, and whether it merges into an existing artist tool |

## Context

After the performance and sections work the server exposed three overlapping vocabularies
for the same objects: the song layer (`make_section`, `set_song`, `play_song`, `go`,
`jump_to`), the performance layer (`start_performance`, `cue`, `fire_scene`) and the raw
layer (`fire_clip`, `create_scene`, `set_scene`). An artist reading the list could not tell
which world they were in. Units flipped between calls (faders as Live's 0–1 parameter,
capture levels in dBFS, arrangement positions in beats, cue times in bars, everything else
by slot index). Four note-rewrite tools shared one undo, and the descriptions carried
implementation reassurance ("one state read and one cue") rather than what the artist gets.

The review proposed about thirty tools in five groups — Look, Build, Shape, Arrange, Play —
with everything else folded in or moved behind an "advanced" flag.

## Options

- **A. Serve only the artist's set; drop or gate the raw layer.** Cleanest list. Against:
  the raw layer is what `batch`, `build_song` and the performance stories are built on, other
  clients' users rely on it, and the MCP clients cannot re-enable what a server does not
  serve. An environment switch (`ABLETON_MCP_ADVANCED_TOOLS`) was tried and rejected: the
  person at the client should decide, not the person who wrote the config.
- **B. Serve everything, name the layers apart.** The decision. The artist's set
  (`CORE_TOOLS` in `src/tools.rs`) keeps its names; every other tool is served as
  `adv_<name>`. A marker inside the description was tried first and was not enough: both
  spellings of a job still sat side by side under near-identical names, so a tool search for
  the artist's words returned the raw layer as readily as the artist's tool, and the model
  picked either. The prefix sorts the two apart in a client's list and puts the artist's
  name first in a search. Neither client offers a standard per-tool group, and the MCP
  protocol has no grouping field (checked 2026-09-19: tool annotations are `title`,
  `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`; the server sets
  them).
- **C. Two servers.** One binary, two `--surface` modes. Against: two configs to keep in
  step for every client, and the split still has to be decided per tool.

## Decision

The surface is the artist's set, and it speaks the artist's units:

- **Five groups.** Look: `get_context`. Build: `build_song`, `make_section`, `create_clip`,
  `add_notes_to_clip`, `load_instrument_or_effect`, `search_browser`, `set_key`, `set_tempo`.
  Shape: `shape_sound`, `feel`, `set_track_mixer`, `set_send`, `create_return`. Arrange:
  `set_song`, `add_to_song`, `remove_from_song`, `arrange`, `create_locator`. Play:
  `play_song`, `go`, `jump_to`, `back`, `hold_section`, `next_section`, `previous_section`,
  `record_clip`, `capture_mix`, `clear_captures`, `end_performance`. Housekeeping:
  `delete_track`, `delete_clip`, `export_set`, `import_set`, `batch`.
- **Units.** Faders and levels in dB (`volume` is dB, `volume_db` spells it out, the raw 0–1
  parameter is `fader`; meters as dB, 0 dB the top); positions in
  the Arrangement as Live's 1-based bars; note times inside a clip in beats. A tool that
  takes beats keeps them as a second, optional form.
- **One tool per intent.** Feel is one tool (`feel`: swing, humanize, groove, retime, a
  variation) with one undo; sound is one tool (`shape_sound`, words first, any parameter by
  name as the fallback); the Arrangement is one tool (`arrange`: place, repeat, move,
  delete, shorten, list). A new capability extends one of these before it becomes a tool.
- **Sections, not scenes.** The artist's vocabulary is section and song; scene, slot and
  cue are the raw layer's words and appear only in advanced tools.
- **Descriptions say what the artist gets**, not how the server does it. Implementation
  facts (round trips, the script's clock) live in the architecture note.
- **Everything else is served as `adv_<name>`** (`adv_fire_scene`, `adv_cue`), with the
  standard MCP annotations. It is neither hidden nor deprecated: `batch`, `build_song` and
  the performance layer compose it, a producer who wants a scene by index still has one, and
  `batch` accepts a step written either way.
- **What the API cannot do is said once.** Live's Save (Cmd+S) and Arrangement-clip
  automation are stated in the instructions and in the one tool concerned, not in every
  reply.

## Consequences

- Every open issue and story that proposes a tool is read against this: does it extend an
  artist tool, is it advanced, and what units does it speak. Issues #33–#36 (arrangement
  editing) fold into `arrange`; #30 (groove) is `feel`; the song-writing-feel story's
  `set_groove` and `quantize_clip` are `feel` modes.
- `CORE_TOOLS` is the list; a test pins that every core tool is served under its own name
  and that the raw layer is served only as `adv_<name>`. Adding a tool means deciding its
  layer in the same PR.
- The count of served tools stays large. That is acceptable: the names say which layer a
  tool is in, and the client's own tool settings are the user's.
- A caller that hard-codes a raw tool name has to add the prefix. Nothing in this repo does
  except tests and the instructions; `batch` steps keep working either way.
