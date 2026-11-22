# shellcheck shell=bash
# Shared setup for the packaging scripts. Source it: `. "$(dirname "$0")/../env.sh"`.
#
# It exports ROOT, VERSION, DIST, CARGO_TARGET_DIR and the CRAFTRADIO_BUILD_* values.
# It also sets BINS, CARGO_PKGS, APP_ID and the LAME_* values. Comments below explain each.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT

# The root Cargo.toml has the only copy of the version, in [workspace.package].
# `cargo xtask version` prints the same value. Awk does not need a build of xtask.
workspace_version() {
  awk '
    /^\[/ { in_pkg = ($0 == "[workspace.package]") ; next }
    in_pkg && $1 == "version" { gsub(/[" ]/, "", $3); print $3; exit }
  ' "$ROOT/Cargo.toml"
}

# CRAFTRADIO_VERSION overrides the workspace version.
VERSION="${CRAFTRADIO_VERSION:-$(workspace_version)}"
if [ -z "$VERSION" ]; then
  echo "error: could not read [workspace.package] version from $ROOT/Cargo.toml" >&2
  exit 1
fi
export VERSION

# The output folder for release files.
DIST="${DIST:-$ROOT/dist/release}"
mkdir -p "$DIST"
export DIST

if [ -z "${CRAFTRADIO_BUILD_SHA:-}" ]; then
  CRAFTRADIO_BUILD_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)"
fi
export CRAFTRADIO_BUILD_SHA
export CRAFTRADIO_BUILD_DATE="${CRAFTRADIO_BUILD_DATE:-$(date -u +%Y-%m-%d)}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# The scripts that source this file use APP_ID, BINS and CARGO_PKGS.
# shellcheck disable=SC2034
{
# The reverse-DNS id for the bundle, desktop file, metainfo and Flatpak.
APP_ID=io.github.radioterminal.craftradio
# The programs to ship, and their cargo packages.
BINS=(craftradio craftradio-cli)
CARGO_PKGS=(-p radio -p radio-cli)
}

# LAME 3.100 is the last LAME release. Only the app links it. The CLI does not.
LAME_VERSION=3.100
LAME_URL="https://downloads.sourceforge.net/project/lame/lame/$LAME_VERSION/lame-$LAME_VERSION.tar.gz"
LAME_SHA256=ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e

# Run a command a maximum of five times, with 15 s between tries.
# Use it for remote services. The Apple notarization service sometimes drops connections.
retry() {
  local attempt
  for attempt in 1 2 3 4 5; do
    "$@" && return 0
    [ "$attempt" = 5 ] || { echo "retrying ($attempt/5): $*" >&2; sleep 15; }
  done
  return 1
}

# Write a GitHub Actions warning. Outside Actions, write to stderr.
warn() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::warning::$*"; else echo "warning: $*" >&2; fi
}

# Copy the licence and readme files into a package folder.
copy_docs() {
  local dest="$1" f
  for f in README.md LICENSE-MIT LICENSE-APACHE; do
    if [ -f "$ROOT/$f" ]; then cp "$ROOT/$f" "$dest/"; fi
  done
}

# Put the LAME licence and replacement steps next to a bundled libmp3lame.
# The LGPL relinking terms need them (docs/mp3-release.md).
# $1 is the destination folder. $2 tells a person where the library is.
copy_lame_notice() {
  local dest="$1" where="$2"
  cp "$ROOT/packaging/lame/COPYING" "$dest/LICENSE-LAME.txt"
  cat >"$dest/LAME.txt" <<NOTE
CraftRadio encodes MP3 with LAME $LAME_VERSION (https://lame.sourceforge.io), licensed under the
GNU LGPL (LICENSE-LAME.txt). LAME is not built into CraftRadio: it is the separate shared library
$where. To use your own build of LAME, build LAME $LAME_VERSION or a compatible
version as a shared library from its source ($LAME_URL) and replace that file.
NOTE
}

# Download the LAME source into folder $1, check it and unpack it. Print the unpacked folder.
fetch_lame() {
  local dir="$1" tgz="$1/lame-$LAME_VERSION.tar.gz"
  mkdir -p "$dir"
  [ -f "$tgz" ] || retry curl -fsSL -o "$tgz" "$LAME_URL" >&2
  if [ "$(sha256 "$tgz")" != "$LAME_SHA256" ]; then
    echo "error: $tgz does not match LAME_SHA256" >&2
    rm -f "$tgz"
    return 1
  fi
  rm -rf "$dir/lame-$LAME_VERSION"
  tar -xzf "$tgz" -C "$dir"
  # LAME 3.100 exports a symbol that it does not define. New linkers refuse this export list.
  sed -i.bak '/lame_init_old/d' "$dir/lame-$LAME_VERSION/include/libmp3lame.sym"
  echo "$dir/lame-$LAME_VERSION"
}

# Print the SHA-256 of a file, on Linux and macOS.
sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
