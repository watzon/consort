#!/usr/bin/env bash
# Makes the app icon files from design/logo/app-icon-dark.svg, the Three
# voices mark on a rounded square in the dark canvas color.
#
#   scripts/make-icons.sh
#
# It needs macOS (sips and iconutil) and rsvg-convert (brew install librsvg).
# rsvg-convert draws the SVG, because it supports the edge filter and the
# screen blend that the mark uses. The output is in design/logo/icons/ and is
# committed, so a build does not need these tools:
#
#   app-icon-<size>.png   16, 32, 64, 128, 256, 512 and 1024 px, tile to the
#                         edge. For Linux packages and the Windows build.
#   Consort.icns          For the macOS app. The tile fills 80 percent of the
#                         canvas, as the macOS icon grid asks, so the Dock
#                         icon matches the icons of other apps.
set -euo pipefail
cd "$(dirname "$0")/.."

source_svg="design/logo/app-icon-dark.svg"
out="design/logo/icons"
sizes=(16 32 64 128 256 512 1024)

command -v rsvg-convert >/dev/null 2>&1 || { echo "make-icons: install rsvg-convert (brew install librsvg)" >&2; exit 1; }
command -v sips >/dev/null 2>&1 || { echo "make-icons: sips is missing. Run this script on macOS" >&2; exit 1; }
command -v iconutil >/dev/null 2>&1 || { echo "make-icons: iconutil is missing. Run this script on macOS" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# One 1024 px master for each look. sips makes every smaller size from it.
rsvg-convert -w 1024 -h 1024 "$source_svg" -o "$work/full.png"
# The macOS look: the tile is 824 px of 1024, centered, with a clear margin.
# The tile goes into the wrapper as a data URL, because rsvg-convert does not
# read files that an SVG names.
rsvg-convert -w 824 -h 824 "$source_svg" -o "$work/tile.png"
cat >"$work/padded.svg" <<SVG
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1024" height="1024" viewBox="0 0 1024 1024">
  <image x="100" y="100" width="824" height="824" xlink:href="data:image/png;base64,$(base64 <"$work/tile.png" | tr -d '\n')"/>
</svg>
SVG
rsvg-convert -w 1024 -h 1024 "$work/padded.svg" -o "$work/padded.png"

for size in "${sizes[@]}"; do
  sips -z "$size" "$size" "$work/full.png" --out "$out/app-icon-${size}.png" >/dev/null
done

iconset="$work/Consort.iconset"
mkdir -p "$iconset"
# The names are the ones iconutil expects: <size>x<size> and @2x.
for pair in "16 16x16" "32 16x16@2x" "32 32x32" "64 32x32@2x" "128 128x128" "256 128x128@2x" "256 256x256" "512 256x256@2x" "512 512x512" "1024 512x512@2x"; do
  size="${pair%% *}"
  name="${pair##* }"
  sips -z "$size" "$size" "$work/padded.png" --out "$iconset/icon_${name}.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$out/Consort.icns"
echo "Wrote $out/Consort.icns and $(printf '%s ' "${sizes[@]}")px PNG files"
