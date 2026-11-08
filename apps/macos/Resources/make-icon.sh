#!/bin/zsh
# Renders AppIcon.svg into AppIcon.icns (needs Google Chrome for the SVG render; sips and iconutil ship with macOS).
set -euo pipefail
cd "$(dirname "$0")"
TMP=$(mktemp -d)
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu --hide-scrollbars \
  --default-background-color=00000000 --window-size=1024,1024 --user-data-dir="$TMP/chrome" \
  --screenshot="$TMP/icon_1024.png" "file://$PWD/AppIcon.svg" >/dev/null 2>&1
SET="$TMP/AppIcon.iconset"
mkdir "$SET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$TMP/icon_1024.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$TMP/icon_1024.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o AppIcon.icns
cp "$TMP/icon_1024.png" AppIcon-1024.png
rm -rf "$TMP"
echo "wrote AppIcon.icns"
