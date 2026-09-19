#!/usr/bin/env bash
# Assertions about the built runtime image. Run after `docker build`:
#
#   docker/verify-image.sh mcp-ableton-music-maker:local
#
# Each check is a property the Dockerfile is supposed to guarantee. If one
# fails, the image should not be used or pushed.
set -euo pipefail

IMAGE="${1:-mcp-ableton-music-maker:local}"
MAX_SIZE_MB="${MAX_SIZE_MB:-50}"
fail=0

pass() { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1"; fail=1; }

hardened=(--network none --read-only --cap-drop ALL --security-opt no-new-privileges:true)

echo "verifying $IMAGE"

# 1. Distroless: no shell, so no way to run anything but the binaries.
if docker run --rm "${hardened[@]}" --entrypoint /bin/sh "$IMAGE" -c true >/dev/null 2>&1; then
  bad "image contains a shell"
else
  pass "no shell in the runtime image"
fi

# 2. Runs as a non-root user.
user="$(docker image inspect "$IMAGE" --format '{{.Config.User}}')"
if [[ "$user" == "nonroot" || "$user" == "65532" || "$user" == "65532:65532" ]]; then
  pass "runs as $user"
else
  bad "runs as '${user:-root}'"
fi

# 3. Telemetry and dataset switches are hard-off in the image environment.
env_list="$(docker image inspect "$IMAGE" --format '{{join .Config.Env "\n"}}')"
for var in ABLETON_MCP_DISABLE_TELEMETRY=true ABLETON_MCP_DISABLE_DATASET=true; do
  grep -qx "$var" <<<"$env_list" && pass "$var" || bad "$var missing from image env"
done

# 4. The binary itself reports every gate off and no credentials baked in.
status="$(docker run --rm "${hardened[@]}" "$IMAGE" --privacy-status 2>/dev/null || true)"
check_gate() {
  if python3 -c 'import json,sys; s=json.loads(sys.argv[1]); sys.exit(0 if s[sys.argv[2]] == json.loads(sys.argv[3]) else 1)' "$status" "$1" "$2" 2>/dev/null; then
    pass "privacy status: $1 = $2"
  else
    bad "privacy status: $1 != $2 (got: ${status:0:300})"
  fi
}
check_gate telemetry_enabled false
check_gate dataset_enabled false
check_gate has_supabase_credentials false
check_gate would_prompt_for_consent false

# 5. The MCP handshake works over stdio without Live present (the server
#    logs a warning and continues), and stdout carries only JSON-RPC.
resp="$(printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"verify","version":"0"}}}' \
  | timeout 60 docker run --rm -i "${hardened[@]}" -e ABLETON_HOST=127.0.0.1 -e ABLETON_PORT=1 \
      "$IMAGE" 2>/dev/null || true)"
if grep -q '"serverInfo"' <<<"$resp" && grep -q '"AbletonMusicMaker"' <<<"$resp"; then
  pass "MCP initialize handshake over stdio returns serverInfo"
else
  bad "no MCP initialize response on stdout: ${resp:0:300}"
fi
while IFS= read -r line; do
  [ -z "$line" ] && continue
  python3 -c 'import json,sys; json.loads(sys.argv[1])' "$line" 2>/dev/null \
    || { bad "non-JSON line on stdout: ${line:0:120}"; break; }
done <<<"$resp"

# 6. Size budget.
bytes="$(docker image inspect "$IMAGE" --format '{{.Size}}')"
mb=$(( bytes / 1000000 ))
[ "$mb" -le "$MAX_SIZE_MB" ] && pass "image size ${mb} MB (limit ${MAX_SIZE_MB} MB)" \
  || bad "image size ${mb} MB exceeds ${MAX_SIZE_MB} MB"

# 7. The installer binary is present and embeds the Remote Script.
if docker run --rm "${hardened[@]}" --entrypoint /app/ableton-music-maker-install-script "$IMAGE" --help >/dev/null 2>&1; then
  pass "installer binary runs"
else
  bad "installer binary missing or broken"
fi

if [ "$fail" -ne 0 ]; then
  echo "verification FAILED"; exit 1
fi
echo "verification passed"
