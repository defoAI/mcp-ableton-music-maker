# Story naming and product-management guide

Canonical guide for how features of mcp-ableton-music-maker are named, designed and
maintained. **A story is identified by its name (slug) — there are no sequential numeric
IDs.** Read it before creating or renaming a story.

---

## 1. File naming — the slug *is* the identifier

**Every story file is named by a descriptive kebab-case slug. The slug is the story's identity
— there is no number anywhere (not in the filename, the H1, or a Details row).**

| Kind | Filename | Example |
|------|----------|---------|
| User story | `<slug>.md` | `arrangement-automation-envelopes-readable-and-writable.md` |
| Implementation plan | `<slug>-impl.md` | `arrangement-automation-envelopes-readable-and-writable-impl.md` |
| Multi-part impl | `<slug>-impl-part1.md` | |

### Why names, not numbers
- Sequential numbers collide when two people allocate "the next one" in parallel. A slug
  derived from the title never collides.
- A number tells you nothing about what the story *is*. Slugs cluster by topic
  (`arrangement-…`, `device-…`, `browser-…`) in a plain `ls`.

### Code and tests do **not** reference stories
Source code and tests describe the system **as it is** — they never point back at a story. No
`// story: …` comments, no `#[test] fn story_xyz…` names. A story is a *design* artifact; once
it ships, the code stands on its own. If a comment needs to explain *why*, explain the reason.

### Deriving a slug
1. Take the story title (`Arrangement Automation Envelopes Readable and Writable`).
2. Lowercase; strip punctuation, emoji, quotes.
3. Drop leading stop-words (`the`, `a`, `for`, `to`, `of`, `and`, `with`, `via`, …).
4. Keep the first **~8 meaningful words**; join with hyphens.
5. → `arrangement-automation-envelopes-readable-and-writable.md`.

If two stories would collide, append a distinguishing word. The full title is always
preserved in the H1 inside the file.

### Cross-linking between stories
Link by **slug filename**, using the slug (or a short human phrase) as the link text. In PRs,
commits and release notes, refer to a story by its **slug**, never a number.

---

## 2. Status, priority, size

```yaml
Status:   Draft | Ready | In Progress | Done
Priority: P0 (now) | P1 (this week) | P2 (this month) | P3 (backlog)
Size:     S (<1 day) | M (1-3 days) | L (3+ days, consider splitting)
```

Annotate Priority with a one-line reason, not just the level.

---

## 3. Story shape

Stories are structured design specs. Required sections: **Story · Details · Context**
(Problem / Current State / Root Cause) **· Open Questions · Prototype · Acceptance
Criteria** (grouped + "No Regressions") **· Affected Files · Remote Script compatibility ·
Privacy · Test Coverage · Implementation Notes · Verification · Out of Scope · Dependencies ·
Related Stories · Changelog.** See the comment block at the top of
[templates/user_story.md](templates/user_story.md).

Conventions specific to this product:

- **Start with questions, not instructions.** A story begins with the producer's problem —
  what they are trying to do in Live and where it stalls — never with a tool signature.
  Produce every clarifying question you have, and get answers, before writing acceptance
  criteria. "None — the problem is fully specified" is a valid answer, stated explicitly.
- **The prototype is a transcript.** This product has no screens; its interface is the
  conversation. So the prototype for a tool is a throwaway transcript in
  [`prototypes/`](prototypes/): the real prompt a producer would type, the tool calls Claude
  would make with their real parameter names, and the text each returns. Review happens
  there. If the transcript needs explaining, so will the tool.
- **A tool's description is its documentation.** The `#[tool]` doc comment is what the model
  and the user see. The story writes it in full, as the model will read it, including what
  the tool refuses and what error it returns.
- **Every Remote Script change is a versioned change.** A story that adds or changes a
  command must say so in its **Remote Script compatibility** section and walk the checklist:
  handler in the script, name in `SCRIPT_CAPABILITIES`, bump `SCRIPT_VERSION`, entry in
  `tools::ALL_REMOTE_COMMANDS`, then the tool body. The script stays compatible with Live's
  bundled Python: no f-strings, no type hints, no third-party imports.
- **Any new data capture is a privacy change.** If a story records, uploads or stores
  anything new, it names the change to `TERMS.md` and the test in `tests/activity.rs` or
  `tests/local_only.rs` that pins it. There is no story that quietly widens what is stored,
  and nothing ever uploads.
- **Tool bodies are plain functions.** `fn(&LiveState, &Params) -> Result<String, String>`,
  bound by `#[tool]` and run through `Server::run`. Failures are `CallToolResult::error`,
  never JSON-RPC errors. A story does not propose an exception.
- **Every claim about current behaviour is anchored to a `file:line` link.**

---

## 4. Lifecycle

1. **Create** — derive a slug from the title; copy `templates/user_story.md` →
   `stories/<slug>.md`; fill every section to spec density; Status `Draft`.
2. **Refine** — `Ready` once the open questions are answered, the prototype transcript is
   reviewed, and Solution + ACs are agreed.
3. **Plan** — copy `templates/implementation_plan.md` → `stories/<slug>-impl.md`; Status
   `In Progress`.
4. **Build & verify** — implement against the ACs; keep the nine `cargo test` suites and
   `docker/verify-image.sh` honest; update [architecture/overview](../architecture/overview.md)
   and the [feature-matrix](../technical/feature-matrix.md) in the same PR.
5. **Close** — Status `Done`; add a Changelog line. **Then prune**: once the code has stood on
   its own for a release, delete the story and its impl plan. Git holds them.

Issues (bugs, tasks, verifications) are tracked in GitHub
[`defoAI/mcp-ableton-music-maker`](https://github.com/defoAI/mcp-ableton-music-maker/issues) —
stories are the *design* layer, issues the *work-tracking* layer.

---

## 5. Quick reference

```bash
ls docs/product_management/stories/ | grep arrangement     # slugs cluster by topic
grep -rl 'load_browser_item' docs/product_management/stories/
grep -c '#\[tool(name = ' src/tools.rs                       # the real tool count
```
