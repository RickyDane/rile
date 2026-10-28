#!/bin/sh
# Regenerate the checked-in app icons: every style × accent as a 512px PNG for
# the runtime Dock icon, plus the default Finder icon (Folio Bolt, blue).
set -eu
cd "$(dirname "$0")/.."
command -v rsvg-convert >/dev/null || { echo "Install librsvg (brew install librsvg)" >&2; exit 1; }
node scripts/gen_app_icons.mjs
for svg in assets/app-icons/*/*.svg; do
    rsvg-convert -w 512 -h 512 "$svg" -o "${svg%.svg}.png"
done
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir "$tmp/Rile.iconset"
for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" assets/app-icons/folio-bolt/blue.svg -o "$tmp/Rile.iconset/icon_${size}x${size}.png"
    double=$((size * 2))
    rsvg-convert -w "$double" -h "$double" assets/app-icons/folio-bolt/blue.svg -o "$tmp/Rile.iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$tmp/Rile.iconset" -o assets/Rile.icns
echo "Generated the app icon styles and default Finder icon"
