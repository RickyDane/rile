#!/bin/sh
# Build a Finder-launchable app. Pass --debug for a development build.
set -eu
cd "$(dirname "$0")/../../.."
profile=release
case "${1:-}" in
    "") ;;
    --debug) profile=debug ;;
    *) echo "Usage: sh $0 [--debug]" >&2; exit 1 ;;
esac
if [ "$profile" = release ]; then
    cargo build -p rile-app --release
else
    cargo build -p rile-app
fi
target_dir=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
app="$target_dir/$profile/Rile.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target_dir/$profile/rile" "$app/Contents/MacOS/rile"
cp crates/rile-app/Info.plist "$app/Contents/Info.plist"
cp crates/rile-app/assets/Rile.icns "$app/Contents/Resources/Rile.icns"
printf 'APPL????' > "$app/Contents/PkgInfo"
codesign --force --deep --sign - "$app"
echo "Built $app"
