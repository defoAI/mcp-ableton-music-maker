# 0003 — Which address does the Remote Script bind?

| | |
|---|---|
| **Status** | **Decided — loopback by default, a file beside the script overrides it** |
| **Raised** | 2026-09-19 |
| **Decided** | 2026-09-19 |
| **Owner** | Nick |
| **Unblocked** | The claim "listens only on this machine by default" in [brand-and-claims](../marketing/brand-and-claims.md) · the README's troubleshooting row for a server on another machine |

## Context

The Remote Script opens a TCP server on port 9877 with no authentication. Bound to
`0.0.0.0`, any host that can reach the machine on that port can drive Live: create tracks,
load devices, fire clips. The product's whole privacy story is "nothing leaves your machine",
and the Live side was open to the LAN by default.

The server in Docker reaches Live through `host.docker.internal`. On Docker Desktop for Mac
that proxy connects from the host's own loopback, so a loopback bind should still work from a
container; a remote Docker engine on another machine never can.

## Options

- **A. Keep `0.0.0.0`, rely on the macOS application firewall** — works everywhere,
  including a remote Linux Docker engine. Against: the Live side is open to the LAN by
  default, and the firewall is off on most Macs.
- **B. Bind `127.0.0.1`** — closes the LAN. Against: breaks the remote-engine case outright,
  and the Docker Desktop path was unverified.
- **C. Make it configurable from the script's folder** — a small file next to `__init__.py`
  read at load time, default `127.0.0.1`. Against: one more thing to explain; Live must be
  restarted to change it.

## Decision

**B with C as the escape hatch.** `DEFAULT_HOST` in the Remote Script is `127.0.0.1`. A file
named `bind_host.txt` beside `__init__.py`, whose first non-comment line is an address, binds
that address instead; anything missing, unreadable or empty falls back to loopback, so a typo
can never open the port. The script reports `bind_host` and `bind_is_loopback` in
`get_script_info`, and the server's `--check` and the Mac app show them. The remote-engine
case is not a supported setup (Live does not run on Linux), so it does not decide the default
for everyone else; it has the file.

Evidence: `tests/local_only.rs` pins the default and that no line of the script binds
`0.0.0.0`; the resolver was exercised against a missing file, `0.0.0.0`, a commented and
blank-padded address, an empty file, a comment-only file and an unreadable file, and answered
loopback in every case that should. The Docker Desktop for Mac path is still unverified on a
Mac with Docker installed; the troubleshooting row says what to do if it fails.

## Consequences

- `SCRIPT_VERSION` 1.24.0. Existing installs keep binding `0.0.0.0` until the script is
  reinstalled and Live restarted; the app's Setup step offers the update.
- The README's troubleshooting table gains the row for people who deliberately run the server
  on another machine, and "listens only on this machine by default" becomes an approved claim.
- `docs/facts/source-of-truth.md` names `DEFAULT_HOST` and the override file as the source.
