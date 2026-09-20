# Prompts

Four finished prompts a producer copies and pastes into Claude. Each one arrives knowing the
tools, the order they go in, the rules Live imposes, and that its first job is to ask the
producer what to make.

They add nothing to the server: no MCP tool, no Remote Script command. They are text that
uses the surface that already exists.

| File | For |
|---|---|
| [make-a-song.md](make-a-song.md) | An empty set, and a track at the end of it. It refines until you say the mix is done. |
| [prepare-a-performance-set.md](prepare-a-performance-set.md) | A set that has to keep running, and keep changing. It writes the next section while this one plays. |
| [finish-what-i-have.md](finish-what-i-have.md) | Half-finished material, honestly assessed and taken to done. |
| [build-around-my-idea.md](build-around-my-idea.md) | One loop, one recording, one idea — and a track that serves it. |

## The shape of a file

```
# <title>

<one line: when to use this>

---

<the prompt body — everything below the rule is what gets copied>
```

Everything above the `---` is for the reader and for the app's card rail. Everything below it
is the prompt: copy it whole.

## Using them

**In the Mac app.** The Prompts screen embeds these four files with `include_str!` and shows
them in full. Pick a card, press **Copy prompt**, paste it into Claude, and answer the
questions it asks. The app never sends anything to Claude — the client starts the server, and
copy is the only honest action on the screen.

**Without the Mac app.** Open the file, copy everything below the `---`, and paste it into
Claude Code, Claude Desktop or Cursor with this server configured. The prompts name no
client; the text is the same wherever it is pasted.

## Keeping them true

`tests/prompts.rs` reads every file in this folder and fails the build when a tool-shaped
name in the text is not served by the server **in the spelling used** — an artist tool
unprefixed (`capture_mix`), a raw tool as `adv_` (`adv_keep_track_playing`). It also pins the
things the review asked for: every prompt opens with `get_context`, `make-a-song.md` carries
the instruction never to declare the mix finished, and `prepare-a-performance-set.md` carries
the instruction never to let the set fall silent.

Wording drift is a review problem. A named tool that does not exist is a build problem.
