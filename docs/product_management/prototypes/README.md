# Prototypes

Throwaway artifacts reviewed *before* implementation. The prototype never becomes the
implementation — it is the specification for one.

**For a tool**, the interface is a conversation, so the prototype is a **transcript**:
`<story-slug>.md` containing the prompt a producer would actually type, every tool call
Claude would make with the real tool name and parameter names, the text each call returns
including the error cases, and Claude's reply. If the transcript needs a paragraph of
explanation, the tool will too — fix the tool.

**For a screen** (the Mac app), the prototype is a **clickable HTML mock-up**:
`<story-slug>.html`, self-contained, real copy and fake data, every state reachable by
clicking, with the design notes and open questions on the page so the review happens there.
Publish it as an artifact for review and link the artifact from the story's Prototype section.

Delete the prototype when its story is pruned.

| Prototype | Story | Review link |
|---|---|---|
| [mac-app-installs-runs-and-watches-the-server.html](mac-app-installs-runs-and-watches-the-server.html) | [mac-app-installs-runs-and-watches-the-server](../stories/mac-app-installs-runs-and-watches-the-server.md) | https://claude.ai/artifact/D2fY7k4dPf8DqEHtYc6QGh |
| [song-writing-feel-notes-groove-quantize-record-undo.md](song-writing-feel-notes-groove-quantize-record-undo.md) | [song-writing-feel-notes-groove-quantize-record-undo](../stories/song-writing-feel-notes-groove-quantize-record-undo.md) | transcript, in the repo — not yet reviewed |
| [app-taps-live-output-spectrum-and-meters.html](app-taps-live-output-spectrum-and-meters.html) | [app-taps-live-output-spectrum-and-meters](../stories/app-taps-live-output-spectrum-and-meters.md) | https://claude.ai/artifact/L5mpwmGRgwoFE5PzK9XHUS — not yet reviewed |
| [notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md](notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md) | [notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix](../stories/notes-by-bar-and-key-sections-that-add-replies-carry-cost-and-fix.md) | transcript, in the repo — not yet reviewed |
