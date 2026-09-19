#!/usr/bin/env bash
# Assertions about the built disk image. Run after `tauri build`:
#
#   app/scripts/verify-dmg.sh path/to/Ableton\ Music\ Maker_0.1.0_aarch64.dmg
#
# Each check is a property a downloaded image is supposed to have: it mounts,
# it carries the app, the app carries the server the client configs point at,
# and that server runs. With EXPECT_SIGNED=1 it also requires a Developer ID
# signature and a stapled notarisation ticket — what a Mac needs to open the
# app without Gatekeeper refusing it. Without those the image is good for
# testing on the machine that built it, not for a download.
set -euo pipefail

DMG="${1:?usage: verify-dmg.sh <dmg>}"
EXPECT_SIGNED="${EXPECT_SIGNED:-0}"
APP_NAME="${APP_NAME:-Ableton Music Maker.app}"
fail=0

pass() { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1"; fail=1; }

echo "verifying $DMG"
[ -f "$DMG" ] || { echo "  FAIL  no such file"; exit 1; }

mnt="$(mktemp -d)"
tmp="$(mktemp -d)"
cleanup() {
  hdiutil detach "$mnt" -quiet >/dev/null 2>&1 || true
  rm -rf "$mnt" "$tmp"
}
trap cleanup EXIT

# 1. It is a disk image and it mounts. A truncated or half-written image
#    fails here, which is the whole point of doing it before publishing.
if hdiutil attach "$DMG" -nobrowse -readonly -mountpoint "$mnt" >/dev/null 2>&1; then
  pass "image mounts"
else
  bad "image does not mount"; exit 1
fi

# 2. The drag-to-install layout: the app, and the /Applications shortcut
#    beside it that the background art points at.
app="$mnt/$APP_NAME"
if [ -d "$app" ]; then pass "carries $APP_NAME"; else bad "no $APP_NAME in the image"; exit 1; fi
if [ -L "$mnt/Applications" ]; then pass "has the /Applications shortcut"; else bad "no /Applications shortcut to drag onto"; fi

plist="$app/Contents/Info.plist"
key() { /usr/libexec/PlistBuddy -c "Print :$1" "$plist" 2>/dev/null || true; }

# 3. The app's own binary — the one Info.plist names, which is what macOS
#    launches, not whatever the bundle happens to be called.
exe="$app/Contents/MacOS/$(key CFBundleExecutable)"
if [ -x "$exe" ]; then
  archs="$(lipo -archs "$exe" 2>/dev/null || echo unknown)"
  case " $archs " in
    *" arm64 "*) pass "app binary is $archs" ;;
    *) bad "app binary is '$archs', not arm64" ;;
  esac
else
  bad "no app binary at ${exe#"$app/"} — the one CFBundleExecutable names"
fi

# 4. The sidecar: the server the client configs are pointed at, by the exact
#    path app/src-tauri/src/lib.rs builds (next to the app binary).
sidecar="$app/Contents/MacOS/ableton-music-maker"
if [ -x "$sidecar" ]; then
  archs="$(lipo -archs "$sidecar" 2>/dev/null || echo unknown)"
  case " $archs " in
    *" arm64 "*) pass "sidecar server is $archs" ;;
    *) bad "sidecar server is '$archs', not arm64" ;;
  esac
else
  bad "no server at Contents/MacOS/ableton-music-maker — the client configs would point at nothing"
fi

# 5. The sidecar runs, out of the mounted image, and says where it writes.
#    This is the check that catches a bundle that only looks complete.
if [ -x "$sidecar" ]; then
  status="$(ABLETON_MCP_STATE_DIR="$tmp/state" "$sidecar" --status 2>/dev/null || true)"
  if python3 -c 'import json,sys; s=json.loads(sys.argv[1]); sys.exit(0 if s["uploads"]=="none" and s["version"] else 1)' "$status" 2>/dev/null; then
    pass "sidecar --status runs: version $(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["version"])' "$status"), uploads none"
  else
    bad "sidecar --status did not return usable JSON: ${status:0:200}"
  fi
fi

# 6. Info.plist: the identifier the app's settings and login items key off,
#    the audio-capture prompt the Listen screen needs, and the floor Tauri
#    was told to set.
for k in CFBundleIdentifier:com.defoai.ableton-music-maker LSMinimumSystemVersion:12.0; do
  got="$(key "${k%%:*}")"
  if [ "$got" = "${k#*:}" ]; then pass "Info.plist ${k%%:*} = $got"; else bad "Info.plist ${k%%:*} is '${got:-missing}', expected ${k#*:}"; fi
done
if [ -n "$(key NSAudioCaptureUsageDescription)" ]; then
  pass "Info.plist carries the audio-capture prompt"
else
  bad "Info.plist has no NSAudioCaptureUsageDescription — the Listen screen cannot ask"
fi
version="$(key CFBundleShortVersionString)"
if [ -n "$version" ]; then pass "Info.plist version $version"; else bad "Info.plist has no CFBundleShortVersionString"; fi

# 7. Signature. macOS refuses to run an arm64 binary with no signature at
#    all, so every Mach-O here must carry at least the linker's ad-hoc one.
for path in "$exe" "$sidecar"; do
  [ -x "$path" ] || continue
  if codesign --verify --strict "$path" >/dev/null 2>&1; then
    pass "signature valid: $(basename "$path")"
  else
    bad "no valid signature on $(basename "$path") — macOS would kill it on launch"
  fi
done

# Developer ID prints an Authority chain; an ad-hoc signature prints only
# "Signature=adhoc", so fall back to that rather than calling it nothing.
desc="$(codesign -dvv "$app" 2>&1)"
authority="$(sed -n 's/^Authority=//p' <<<"$desc" | head -1)"
: "${authority:=$(sed -n 's/^Signature=//p' <<<"$desc" | head -1)}"
if [ "$EXPECT_SIGNED" = "1" ]; then
  case "$authority" in
    "Developer ID Application:"*) pass "signed by $authority" ;;
    *) bad "not signed with a Developer ID (authority: ${authority:-ad-hoc or none})" ;;
  esac
  if xcrun stapler validate "$app" >/dev/null 2>&1; then
    pass "notarisation ticket stapled to the app"
  else
    bad "no stapled notarisation ticket — a downloaded copy would be refused"
  fi
  if spctl -a -vv -t exec "$app" >/dev/null 2>&1; then
    pass "Gatekeeper accepts the app"
  else
    bad "Gatekeeper rejects the app"
  fi
else
  echo "  note  not notarised (authority: ${authority:-none}) — it runs, but a Mac that"
  echo "        downloads it refuses it until com.apple.quarantine is removed. A v* tag"
  echo "        builds the signed, notarised image; EXPECT_SIGNED=1 checks that one."
fi

if [ "$fail" -ne 0 ]; then
  echo "verification FAILED"; exit 1
fi
echo "verification passed"
