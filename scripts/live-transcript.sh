#!/usr/bin/env bash
# The transcript differential's other half: a fixed script of commands run
# against a REAL Ableton Live, with every reply kept.
#
#   scripts/live-transcript.sh                 # -> tests/fixtures/live-transcript-<version>.json
#   scripts/live-transcript.sh --out /tmp/x.json
#
# A test replays the same script against the fake Live and diffs the replies
# field by field. Refresh this whenever SCRIPT_VERSION changes, and say in
# the PR that you did — or that you did not, and why.
#
# It BUILDS: tracks, clips, notes, placements, a scene. Point it at a
# scratch set, not at work you care about. Nothing is cleaned up, so the
# transcript is also a record of what the set looked like afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$#" -eq 0 ]; then
  version="$(python3 - <<'PY'
import json, os, socket
host = os.environ.get("ABLETON_HOST", "127.0.0.1")
port = int(os.environ.get("ABLETON_PORT", "9877"))
s = socket.create_connection((host, port), timeout=10)
s.sendall(b'{"id":1,"type":"get_script_info","params":{}}\n')
buf = b""
while b"\n" not in buf:
    buf += s.recv(65536)
print((json.loads(buf.split(b"\n")[0])["result"].get("live") or {}).get("version", "unknown"))
PY
)"
  mkdir -p tests/fixtures
  set -- --out "tests/fixtures/live-transcript-${version}.json"
fi

exec python3 scripts/live-transcript.py "$@"
