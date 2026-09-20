#!/usr/bin/env bash
# The measurements the streams story is decided by, against a running Live:
#
#   scripts/live-latency.sh [samples]
#
# Prints the tick period the Remote Script reports, the round-trip cost with
# and without a touch of Live's API (p50 and p95), and — once the later phases
# exist — the event rate, cue lateness and the generic-batch comparison. Each
# line names the phase that owns it. Nothing in the set is changed.
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --quiet --example live_latency -- "${1:-40}"
