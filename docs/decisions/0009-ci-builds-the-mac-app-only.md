# 0009 — What does CI build?

| | |
|---|---|
| **Status** | **Decided — the Mac app and the Rust gate, both on macOS; the Docker image is built where it is used** |
| **Raised** | 2026-09-19 |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | A CI run that can go green · the claims about where the image lives |

## Context

CI ran four jobs: `rust` and two `image` jobs and a `manifest` job on Ubuntu runners, and
`mac-app` on macOS. The Ubuntu jobs built the Docker image for amd64 and arm64, verified it,
scanned it and pushed it to `ghcr.io/defoAI/mcp-ableton-music-maker` on `main`.

Two things made that the wrong shape. The product ships for one platform: the Mac app, and a
Remote Script that runs inside Live, which exists on macOS and Windows and not on Linux. And
the image jobs were the only thing failing — every run on 2026-09-19, on `main` and on every
branch, went red at "Verify runtime image" while `rust` and `mac-app` passed, so the signal
that the Mac build was healthy was buried under a permanent red cross.

## Options

- **A. Fix the image job and keep publishing** — keeps a published multi-architecture image.
  Against: it builds and publishes an artefact for a platform the product does not target,
  on every push, and it is the slowest part of the run by a wide margin.
- **B. Build only the Mac app; keep the image as a local path** — the decision.
- **C. Delete the Dockerfile as well** — refused. The image is a real way to run the server,
  the hardening contract in `docker/verify-image.sh` is worth keeping, and a container is
  still the sensible way to run it on a machine that is not the one Live is on.

## Decision

**B.** `ci.yml` has two jobs, `rust` and `mac-app`, both on `macos-latest`; nothing runs on
Ubuntu and nothing is pushed to a registry, so the workflow no longer needs `packages: write`.
The `rust` job is unchanged in what it checks — format, clippy, tests, the dependency gate
that fails on an HTTP client, and the documentation fact check — it simply runs where the
product runs.

The Dockerfile, `docker-compose.yml` and `docker/verify-image.sh` stay. The image contract is
unchanged; what changed is who checks it. Anyone changing the Dockerfile runs
`docker build --target test .` and `docker/verify-image.sh` themselves before pushing.

## Consequences

- No image is published. Anything that said the image lives at GHCR now says it is built
  where it is used: the README, the feature matrix, the fact snapshot, the claims list and
  the product plan.
- The hardening claims in [brand-and-claims](../marketing/brand-and-claims.md) are still
  true and still backed by `verify-image.sh`, but "verified in CI on every build" is not a
  claim we can make any more, and it has been removed.
- A red CI run now means the Rust gate or the Mac app is broken, which is the point.
- Reversing this is cheap while the Dockerfile is still here: the jobs were deleted, not
  the thing they built.
