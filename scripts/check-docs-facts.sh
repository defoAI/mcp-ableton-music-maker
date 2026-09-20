#!/usr/bin/env bash
# Fails when the verified snapshot in docs/facts/source-of-truth.md disagrees with the code.
#
#   scripts/check-docs-facts.sh
#
# Each check reads the real source (the code, the Remote Script, Cargo.toml, the verify
# script) and then requires the snapshot table row to carry that exact value in bold, plus
# the same figure wherever the README and CLAUDE.md repeat it. Update the snapshot and
# re-date it when a check fails; never edit the check to match stale prose.
set -euo pipefail
cd "$(dirname "$0")/.."

SNAPSHOT=docs/facts/source-of-truth.md
SCRIPT=AbletonMusicMaker_Remote_Script/__init__.py
fail=0
pass() { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1"; fail=1; }

# Real values, read from where they live.
tools="$(grep -c '#\[tool(name = ' src/tools.rs)"
# The command list lives in src/tools.rs; the script derives what it serves
# from its own dispatch, so count the one place it is written down.
commands="$(sed -n '/^pub const ALL_REMOTE_COMMANDS/,/^\];/p' src/tools.rs | grep -c '^    "')"
script_version="$(sed -nE 's/^SCRIPT_VERSION *= *"([^"]+)".*/\1/p' "$SCRIPT")"
crate_version="$(sed -nE 's/^version *= *"([^"]+)".*/\1/p' Cargo.toml | head -1)"
size_limit="$(sed -nE 's/^MAX_SIZE_MB="\$\{MAX_SIZE_MB:-([0-9]+)\}"/\1/p' docker/verify-image.sh)"
port="$(sed -nE 's/^DEFAULT_PORT *= *([0-9]+).*/\1/p' "$SCRIPT")"

row() {  # row <label> <value>: the snapshot row "| <label> | **<value>** ..."
  if grep -qE "^\| $1 \| \*\*$2\*\*" "$SNAPSHOT"; then pass "$1 = $2"; else bad "$1: snapshot does not say $2"; fi
}
row "MCP tools" "$tools"
row "Remote Script commands" "$commands"
row "Remote Script version" "$script_version"
row "Server version" "$crate_version"
row "Image size limit" "$size_limit"
row "Port" "$port"

# The same figures where prose repeats them.
grep -qE "\b$tools tools\b" README.md && pass "README says $tools tools" || bad "README does not say '$tools tools'"
grep -qE "\b$tools tool\b" CLAUDE.md && pass "CLAUDE.md says $tools tool bodies" || bad "CLAUDE.md does not say '$tools tool'"
grep -qE "\b$size_limit MB\b" README.md && pass "README says $size_limit MB" || bad "README does not say '$size_limit MB'"

# Every command the server may send has a handler in the script it embeds.
# The unit test does this in both directions; this runs without cargo.
missing="$(sed -n '/^pub const ALL_REMOTE_COMMANDS/,/^\];/p' src/tools.rs | grep -oE '"[a-z_]+"' | tr -d '"' \
  | while read -r c; do grep -q "command_type == \"$c\"" "$SCRIPT" || grep -q "command_type in (.*\"$c\"" "$SCRIPT" || echo "$c"; done)"
[ -z "$missing" ] && pass "every ALL_REMOTE_COMMANDS entry has a handler in the script" \
  || bad "no handler in the Remote Script: $(echo "$missing" | tr '\n' ' ')"

# And the script declares nothing by hand: the list has one home.
grep -q "SCRIPT_CAPABILITIES = \[" "$SCRIPT" \
  && bad "the Remote Script has a hand-typed capability list again" \
  || pass "the Remote Script derives its capability list"

if [ "$fail" -ne 0 ]; then echo "docs fact check FAILED"; exit 1; fi
echo "docs fact check passed"
