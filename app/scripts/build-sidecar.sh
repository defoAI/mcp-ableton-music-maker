#!/usr/bin/env bash
# Build the server crate and place the binary where Tauri expects a sidecar:
#   src-tauri/binaries/ableton-music-maker-<target triple>
# Tauri strips the triple when bundling, so the app carries
# Contents/MacOS/ableton-music-maker — the path the client configs point at.
#
# The triple is the second argument, or TAURI_ENV_TARGET_TRIPLE (Tauri sets it
# for beforeBuildCommand, so `tauri build --target …` reaches here), or this
# host. The bundler looks the sidecar up by that exact name and stops if it is
# missing, so a build on a host of another architecture cross-compiles instead.
set -euo pipefail
profile="${1:-debug}"
host="$(rustc -vV | sed -n 's/^host: //p')"
triple="${2:-${TAURI_ENV_TARGET_TRIPLE:-$host}}"
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$here/.." && pwd)"
out="$here/src-tauri/binaries"

# The host build asks for no --target, so it shares target/<profile>/ with
# `cargo build` at the repo root and nothing is compiled twice.
args=(build --locked --bin ableton-music-maker)
dir="$repo/target"
if [ "$triple" != "$host" ]; then
  args+=(--target "$triple")
  dir="$dir/$triple"
fi
if [ "$profile" = "release" ]; then
  args+=(--release)
fi

(cd "$repo" && cargo "${args[@]}")
mkdir -p "$out"
cp "$dir/$profile/ableton-music-maker" "$out/ableton-music-maker-$triple"
echo "sidecar: $out/ableton-music-maker-$triple"
