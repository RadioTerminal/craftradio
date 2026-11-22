#!/usr/bin/env bash
# Build and package CraftRadio for Linux on x86_64 or aarch64.
# The output is an AppImage with its zsync file, a deb, an rpm and a tar.gz, in $DIST.
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar"].
# It needs cargo, nfpm, the LAME development files and the ALSA development files.
# The script downloads a pinned appimagetool when it is missing.
# Build on an old distribution (CI uses Ubuntu 22.04) so that the binaries run on newer ones.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"

SKIP_BUILD=0
FORMATS="appimage deb rpm tar"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,7p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
export CRAFTRADIO_MAINTAINER="${CRAFTRADIO_MAINTAINER:-CraftRadio contributors <https://github.com/RadioTerminal/craftradio/issues>}"
BASENAME="craftradio-$VERSION-linux-$ARCH"

echo "==> CraftRadio $VERSION for Linux $ARCH ($FORMATS)"

# The shared CI action installs ALSA but not LAME. Install LAME when it is missing.
if [[ -n "${GITHUB_ACTIONS:-}" ]] && ! dpkg -s libmp3lame-dev >/dev/null 2>&1; then
  sudo apt-get install -y --no-install-recommends libmp3lame-dev
fi

if [[ "$SKIP_BUILD" = 0 ]]; then
  # The RUNPATH lets the tarball, AppImage and Flatpak find their own copy of LAME.
  # The deb and rpm do not have that folder. Thus the app uses the LAME of the system.
  # The shell must not expand $ORIGIN. The dynamic linker reads it.
  (cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,\$ORIGIN/../lib/craftradio" \
    cargo build --release --locked "${CARGO_PKGS[@]}")
fi
BIN="$CARGO_TARGET_DIR/release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree for every format --------------------------------------------------------
for bin in "${BINS[@]}"; do
  install -Dm755 "$BIN/$bin" "$STAGE/usr/bin/$bin"
  strip "$STAGE/usr/bin/$bin" 2>/dev/null || true
done
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$CRAFTRADIO_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/craftradio"
copy_docs "$STAGE/usr/share/doc/craftradio"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

# The tarball and the AppImage bring LAME. Copy the library that the build linked.
LAME_SO="$(ldd "$STAGE/usr/bin/craftradio" | awk '/libmp3lame\.so/ { print $3; exit }')"
if [[ -z "$LAME_SO" || ! -f "$LAME_SO" ]]; then
  ldd "$STAGE/usr/bin/craftradio" >&2
  echo "error: craftradio does not link libmp3lame.so" >&2
  exit 1
fi
LAME_SONAME="$(basename "$LAME_SO")"
# Usage: add_lame <usr folder>. It adds lib/craftradio with LAME and its notice.
add_lame() {
  install -Dm644 "$(readlink -f "$LAME_SO")" "$1/lib/craftradio/$LAME_SONAME"
  mkdir -p "$1/share/doc/craftradio"
  copy_lame_notice "$1/share/doc/craftradio" "lib/craftradio/$LAME_SONAME"
}

has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  add_lame "$WORK/tar/$BASENAME"
  # The app must start with the LAME in the tarball.
  env LD_LIBRARY_PATH= "$WORK/tar/$BASENAME/bin/craftradio" --version
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
  export VERSION
  export NFPM_ARCH="$DEB_ARCH"
  # nfpm does not expand variables in `src`, so put the stage path in a copy of the file.
  sed -e "s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPDIR="$WORK/CraftRadio.AppDir"
  cp -R "$STAGE" "$APPDIR"
  add_lame "$APPDIR/usr"
  # AppRun installs the desktop entry and icons, so Wayland docks show the right icon.
  install -Dm755 "$HERE/AppRun" "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$ROOT/assets/app-icon/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  # This script uses pinned releases of appimagetool and its runtime, with known SHA-256 values.
  APPIMAGETOOL_VERSION=1.9.1
  APPIMAGE_RUNTIME_VERSION=20251108
  case "$ARCH" in
    x86_64)
      APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
      APPIMAGE_RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64)
      APPIMAGETOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
      APPIMAGE_RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
  esac
  # Usage: fetch_pinned <url> <sha256> <file>. It downloads the file when it is missing.
  # It removes a file that does not match the checksum.
  fetch_pinned() {
    if [[ ! -f "$3" ]]; then
      curl --proto '=https' -fsSL -o "$3.download" "$1"
      mv "$3.download" "$3"
    fi
    if [[ "$(sha256 "$3")" != "$2" ]]; then
      rm -f "$3"
      echo "$1: SHA-256 does not match the pinned release" >&2
      exit 2
    fi
  }
  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [[ -z "$TOOL" ]]; then
    TOOL="$CARGO_TARGET_DIR/appimagetool-$APPIMAGETOOL_VERSION-$ARCH.AppImage"
    fetch_pinned "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$ARCH.AppImage" "$APPIMAGETOOL_SHA256" "$TOOL"
    chmod 755 "$TOOL"
  fi
  RUNTIME="$CARGO_TARGET_DIR/appimage-runtime-$APPIMAGE_RUNTIME_VERSION-$ARCH"
  fetch_pinned "https://github.com/AppImage/type2-runtime/releases/download/$APPIMAGE_RUNTIME_VERSION/runtime-$ARCH" "$APPIMAGE_RUNTIME_SHA256" "$RUNTIME"
  # appimagetool runs in $DIST, so every path must be absolute.
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  RUNTIME="$(cd "$(dirname "$RUNTIME")" && pwd)/$(basename "$RUNTIME")"
  rm -f "$OUT.zsync"
  # AppImageUpdate reads this update information and gets the .zsync of the latest release.
  REPO="${GITHUB_REPOSITORY:-RadioTerminal/craftradio}"
  UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|craftradio-*-linux-$ARCH.AppImage.zsync"
  # Extract-and-run works without FUSE. With zsyncmake, appimagetool also writes the .zsync file.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream --runtime-file "$RUNTIME" -u "$UPDATE_INFO" "$APPDIR" "$OUT")
  echo "wrote $OUT"
  if [[ -s "$OUT.zsync" ]]; then
    echo "wrote $OUT.zsync"
  else
    warn "zsyncmake not found, so $OUT.zsync was not written; AppImage delta updates need it"
  fi
fi

"$STAGE/usr/bin/craftradio-cli" --version
echo "==> done"
ls -lh "$DIST"
