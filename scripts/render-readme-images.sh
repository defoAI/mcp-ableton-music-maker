#!/usr/bin/env bash
# Renders the README's images from their HTML sources.
#
#   scripts/render-readme-images.sh
#
# .github/readme/src/<name>.html → .github/readme/<name>.png, at twice the size
# for sharp screens. Needs a Chromium: $CHROME, else chromium / google-chrome on the PATH,
# else Playwright's headless shell. Fonts come from Google Fonts, so it needs a network;
# the images are illustrations of the product, drawn, not captured from Live.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC=.github/readme/src
OUT=.github/readme

chrome="${CHROME:-}"
if [ -z "$chrome" ]; then
  for c in chromium chromium-browser google-chrome; do
    command -v "$c" >/dev/null && { chrome="$(command -v "$c")"; break; }
  done
fi
if [ -z "$chrome" ]; then
  chrome="$(ls -d "$HOME"/.cache/ms-playwright/chromium_headless_shell-*/chrome-*/chrome-headless-shell 2>/dev/null | sort | tail -1 || true)"
fi
[ -n "$chrome" ] || { echo "no Chromium found; set CHROME" >&2; exit 1; }

render() {  # render <name> <width> <height>
  "$chrome" --headless --disable-gpu --hide-scrollbars --no-sandbox \
    --force-device-scale-factor=2 --window-size="$2,$3" --virtual-time-budget=8000 \
    --screenshot="$PWD/$OUT/$1.png" "file://$PWD/$SRC/$1.html" 2>/dev/null
  echo "  $OUT/$1.png"
}

render banner  1280 640
render session 1280 800
render perform 1280 560
render capture 1280 560
render listen  1280 800
