# Implementation Plan: {Story Title}

**Story:** `{story slug}`
**Developer:** {Name}
**Started:** YYYY-MM-DD

## Approach
{Brief description of how this will be implemented}

## Tasks
- [ ] Remote Script handler (if any) — then `SCRIPT_CAPABILITIES`, `SCRIPT_VERSION`, `ALL_REMOTE_COMMANDS`
- [ ] Tool body as a plain function, bound with `#[tool]`, run through `Server::run`
- [ ] Tests
- [ ] `docs/architecture/overview.md` and `docs/technical/feature-matrix.md` updated
- [ ] `docs/facts/source-of-truth.md` snapshot re-verified if a count or version moved
- [ ] Review

## Files Changed
- `src/tools.rs`
- `AbletonMusicMaker_Remote_Script/__init__.py`

## Testing

<!-- The story's Test Coverage section is the checklist. Work it, don't restate it. -->

- [ ] `cargo test` green (all nine suites)
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --check` clean
- [ ] `scripts/check-docs-facts.sh` clean
- [ ] `docker build --target test .` and `docker/verify-image.sh` if the Dockerfile changed
- [ ] Manual run against Live: script reinstalled, Live restarted, `get_remote_script_info`
      reports the new version, the story's verification steps pass
- [ ] No test skipped, disabled or deleted to go green

## Rollout
- [ ] PR into `main`
- [ ] CI green (the Rust gate and the Mac app)
- [ ] Release notes name the story by slug; `SCRIPT_VERSION` bump called out so users reinstall

---

## Changelog
| Date | Change |
|------|--------|
| YYYY-MM-DD | Plan created |
