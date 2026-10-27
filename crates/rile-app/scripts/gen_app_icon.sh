#!/bin/sh
# Regenerate the checked-in icons from the approved accent artwork.
set -eu
cd "$(dirname "$0")/.."
command -v rsvg-convert >/dev/null || { echo "Install librsvg (brew install librsvg)" >&2; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir "$tmp/Rile.iconset"
for accent in blue purple pink red orange green teal graphite; do
    for size in 16 32 128 256 512; do
        rsvg-convert -w "$size" -h "$size" "assets/app-icons/$accent.svg" -o "$tmp/Rile.iconset/icon_${size}x${size}.png"
        double=$((size * 2))
        rsvg-convert -w "$double" -h "$double" "assets/app-icons/$accent.svg" -o "$tmp/Rile.iconset/icon_${size}x${size}@2x.png"
    done
    iconutil -c icns "$tmp/Rile.iconset" -o "assets/app-icons/$accent.icns"
done
cp assets/app-icons/blue.icns assets/Rile.icns
echo "Generated the eight Pathline accent icons and default Finder icon"
