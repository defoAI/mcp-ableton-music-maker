#!/usr/bin/env bash
# The Remote Script's own tests: the file loaded under a plain interpreter
# with Live's modules stubbed and schedule_message driven by hand, so the
# protocol, the tick sampler and (later) the whitelist run in CI rather than
# in someone's Live session. Python 3; the script itself stays 2.7-compatible.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 -m unittest discover -s tests/remote_script -p 'test_*.py' -v
python3 scripts/check-script-helpers.py
