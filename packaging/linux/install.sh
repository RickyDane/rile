#!/bin/sh
# Installs (or updates) Rile Server: shares folders of this Linux machine with
# Rile apps on the local network ("Nearby").
#
#   curl -fsSL https://raw.githubusercontent.com/RickyDane/rile/master/packaging/linux/install.sh | sudo sh
#
# Options (environment variables):
#   RILE_VERSION=v0.9.0       install this release instead of the newest one
#   RILE_TARBALL=/path.tar.gz install from a local archive (no download)
#   RILE_PREFIX=/usr/local    where bin/rile-server and bin/rilectl go
#   RILE_NO_SETUP=1           don't start the interactive setup
set -eu

REPO="RickyDane/rile"
PREFIX="${RILE_PREFIX:-/usr/local}"
BIN="$PREFIX/bin"
CONFIG="/etc/rile-server/config.toml"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || die "Rile Server runs on Linux (this is $(uname -s))."
[ "$(id -u)" -eq 0 ] || die "run this as root, e.g.: curl -fsSL … | sudo sh"

case "$(uname -m)" in
  x86_64 | amd64) TRIPLE="x86_64-unknown-linux-musl" ;;
  aarch64 | arm64) TRIPLE="aarch64-unknown-linux-musl" ;;
  *) die "unsupported CPU $(uname -m) (x86_64 and aarch64 are available)" ;;
esac
ASSET="rile-server-$TRIPLE.tar.gz"

fetch() { # url dest
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    die "curl or wget is needed"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  else die "sha256sum is needed to verify the download"
  fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

if [ -n "${RILE_TARBALL:-}" ]; then
  say "Installing from $RILE_TARBALL"
  cp "$RILE_TARBALL" "$TMP/$ASSET"
else
  if [ -n "${RILE_VERSION:-}" ]; then
    API="https://api.github.com/repos/$REPO/releases/tags/$RILE_VERSION"
  else
    # Newest first, prereleases included (Rile ships betas).
    API="https://api.github.com/repos/$REPO/releases?per_page=20"
  fi
  fetch "$API" "$TMP/releases.json" || die "can't reach GitHub ($API)"
  URL="$(grep -o "\"browser_download_url\": *\"[^\"]*/$ASSET\"" "$TMP/releases.json" | head -n 1 | sed 's/.*"\(https[^"]*\)"$/\1/')"
  [ -n "$URL" ] || die "no release with $ASSET found${RILE_VERSION:+ for $RILE_VERSION}"
  BASE="${URL%/*}"
  say "Downloading ${BASE##*/download/} for $TRIPLE…"
  fetch "$URL" "$TMP/$ASSET"
  fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS"
  WANT="$(grep " \*\{0,1\}$ASSET\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)"
  [ -n "$WANT" ] || die "SHA256SUMS has no entry for $ASSET"
  [ "$(sha256 "$TMP/$ASSET")" = "$WANT" ] || die "checksum mismatch for $ASSET — download corrupted?"
  say "✓ Checksum verified"
fi

tar -xzf "$TMP/$ASSET" -C "$TMP"
for b in rile-server rilectl; do
  [ -f "$TMP/$b" ] || die "$ASSET doesn't contain $b"
done
mkdir -p "$BIN"
# Replace atomically, so a running service keeps its old binary until restarted.
for b in rile-server rilectl; do
  install -m 0755 "$TMP/$b" "$BIN/.$b.new"
  mv -f "$BIN/.$b.new" "$BIN/$b"
done
say "✓ Installed $("$BIN/rilectl" --version) to $BIN"

if [ -f "$CONFIG" ]; then
  if [ -d /run/systemd/system ] && systemctl is-enabled rile-server >/dev/null 2>&1; then
    systemctl restart rile-server
    say "✓ Restarted the rile-server service"
  fi
  say ""
  say "Rile Server is updated. Change what it shares with: sudo rilectl share --help"
  exit 0
fi

if [ -n "${RILE_NO_SETUP:-}" ]; then
  say "Run the setup with: sudo rilectl setup"
elif [ -r /dev/tty ] && (exec </dev/tty) 2>/dev/null; then
  # stdin is this script when piped from curl: talk to the terminal instead.
  "$BIN/rilectl" setup </dev/tty
else
  say "No terminal for the interactive setup. Run it with: sudo rilectl setup"
fi
