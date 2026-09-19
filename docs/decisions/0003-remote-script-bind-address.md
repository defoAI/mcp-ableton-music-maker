# 0003 — Which address does the Remote Script bind?

| | |
|---|---|
| **Status** | **Open** |
| **Raised** | 2026-09-19 |
| **Decided** | — |
| **Owner** | Nick |
| **Needed by** | Before the README can claim the script listens only to the local machine |
| **Blocks** | The security posture of the Live side; the "nothing else can reach Live" sentence |

## Context

The Remote Script opens a TCP server on `HOST = "0.0.0.0"`, port 9877
(`AbletonMusicMaker_Remote_Script/__init__.py`, the constants at the top). Any host that can
reach the machine on that port can drive Live: create tracks, load devices, fire clips. There
is no authentication on the socket.

The server in Docker reaches Live through `host.docker.internal`. On Docker Desktop for Mac
that proxy connects from the host's own loopback, so binding `127.0.0.1` *should* still work
from a container — but it has not been verified on a Mac with Live and Docker Desktop
running, and a wrong guess breaks the only path into Live. The Python-era plan left it at
`0.0.0.0` for that reason and marked the test "pending on Mac".

## Options

- **A. Keep `0.0.0.0`, rely on the macOS application firewall** — works everywhere,
  including a remote Linux Docker engine. Against: the product's whole privacy story is
  "nothing leaves your machine", and the Live side is open to the LAN by default.
- **B. Bind `127.0.0.1`** — closes the LAN. Against: unverified from Docker Desktop; breaks
  the remote-engine case outright.
- **C. Make it configurable from the script's folder** — a small file next to `__init__.py`
  read at load time, default `127.0.0.1`. Against: one more thing to install and explain;
  Live must be restarted to change it.

## Recommendation

**B**, if the one-line test below passes on a Mac. The remote-engine case is not a supported
setup (Live does not run on Linux), so it should not decide the default for everyone else.

## Cheapest available evidence

With Live open and the script loaded, change `HOST` to `127.0.0.1`, restart Live, and run:

```bash
docker run --rm mcp-ableton-music-maker:local --privacy-status >/dev/null  # image sanity
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  | docker run --rm -i mcp-ableton-music-maker:local 2>&1 | grep -i 'could not reach' && echo UNREACHABLE || echo REACHED
```

## Decision

*Not yet made.*

## Consequences

Under **B**: one constant changes, `SCRIPT_VERSION` bumps, the README's troubleshooting table
gains a row for people who deliberately run the server on another machine, and the
"listens only on localhost" claim becomes approved in
[brand-and-claims](../marketing/brand-and-claims.md). Under **A**: the README must say the
port is reachable from the network and point at the firewall.
