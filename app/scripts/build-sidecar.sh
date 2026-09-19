#!/usr/bin/env bash
# Build the server crate and place the binary where Tauri expects a sidecar:
#   src-tauri/binaries/ableton-music-maker-<host triple>
# Tauri strips the triple when bundling, so the app carries
# Contents/MacOS/ableton-music-maker — the path the client configs point at.
set -euo pipefail
profile="${1:-debug}"
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$here/.." && pwd)"
triple="$(rustc -vV | sed -n 's/^host: //p')"
if [ "$profile" = "release" ]; then
  (cd "$repo" && cargo build --release --locked --bin ableton-music-maker)
  src="$repo/target/release/ableton-music-maker"
else
  (cd "$repo" && cargo build --bin ableton-music-maker)
  src="$repo/target/debug/ableton-music-maker"
fi
mkdir -p "$here/src-tauri/binaries"
cp "$src" "$here/src-tauri/binaries/ableton-music-maker-$triple"
echo "sidecar: $here/src-tauri/binaries/ableton-music-maker-$triple"
