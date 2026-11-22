#!/usr/bin/env bash
# Build, sign and notarize the macOS DMG and the zip of the command-line tool.
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build].
# Signing uses MACOS_SIGN_IDENTITY and MACOS_KEYCHAIN. Notarization also uses the APPLE_* values.
# Without MACOS_SIGN_IDENTITY, the script signs ad-hoc for local tests.
# The script builds LAME and puts it in CraftRadio.app/Contents/Frameworks (docs/mp3-release.md).
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

ARCH=universal
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,6p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

# Keep this value the same as LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
SHORT_VERSION="${VERSION%%-*}"
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/CraftRadio.app"
DMG="$DIST/craftradio-$VERSION-macos-$ARCH.dmg"
CLI_ZIP="$DIST/craftradio-cli-$VERSION-macos-$ARCH.zip"
LAME_DYLIB=libmp3lame.0.dylib

NOTARIZE=0
if [[ "$IDENTITY" = "-" ]]; then
  warn "macOS: ad-hoc signing (no MACOS_SIGN_IDENTITY); artifacts are not notarized"
elif [[ -n "${APPLE_ID:-}" ]] && [[ -n "${APPLE_PASSWORD:-}" ]] && [[ -n "${APPLE_TEAM_ID:-}" ]]; then
  NOTARIZE=1
else
  warn "macOS: APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID incomplete; signed but not notarized"
fi

echo "==> CraftRadio $VERSION for macOS ($ARCH), identity: $IDENTITY, notarize: $NOTARIZE"

# ---- LAME --------------------------------------------------------------------------------------
# Build one shared library per architecture, then join them with lipo.
# The install name is @rpath, so the app finds the copy in its own Frameworks folder.
LAME_OUT="$WORK/lame"
build_lame() {
  local src lipo_inputs=() t arch host prefix
  src="$(fetch_lame "$CARGO_TARGET_DIR/lame-src")"
  for t in "${TARGETS[@]}"; do
    case "$t" in
      aarch64-*) arch=arm64; host=aarch64-apple-darwin ;;
      x86_64-*) arch=x86_64; host=x86_64-apple-darwin ;;
    esac
    prefix="$CARGO_TARGET_DIR/lame-$arch"
    rm -rf "$prefix" "$src-build-$arch"
    cp -R "$src" "$src-build-$arch"
    (
      cd "$src-build-$arch"
      CFLAGS="-arch $arch -O2 -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET" \
        LDFLAGS="-arch $arch -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET" \
        ./configure --host="$host" --prefix="$prefix" --enable-shared --disable-static \
        --disable-frontend --disable-dependency-tracking >/dev/null
      make -j"$(sysctl -n hw.ncpu)" >/dev/null
      make install >/dev/null
    )
    lipo_inputs+=("$prefix/lib/$LAME_DYLIB")
  done
  rm -rf "$LAME_OUT"
  mkdir -p "$LAME_OUT"
  lipo -create -output "$LAME_OUT/$LAME_DYLIB" "${lipo_inputs[@]}"
  install_name_tool -id "@rpath/$LAME_DYLIB" "$LAME_OUT/$LAME_DYLIB"
  # The linker looks for libmp3lame.dylib. The fat file serves both architectures.
  ln -sf "$LAME_DYLIB" "$LAME_OUT/libmp3lame.dylib"
  lipo -info "$LAME_OUT/$LAME_DYLIB"
}

# ---- build -------------------------------------------------------------------------------------
# --skip-build keeps an earlier LAME build when there is one.
if [[ "$SKIP_BUILD" = 0 || ! -f "$LAME_OUT/$LAME_DYLIB" ]]; then
  build_lame
fi
if [[ "$SKIP_BUILD" = 0 ]]; then
  args=()
  for t in "${TARGETS[@]}"; do args+=(--target "$t"); done
  # LAME_LIB_DIR tells vendor/mp3lame-sys to link this LAME build.
  # The rpath lets the app find the library in Contents/Frameworks.
  (cd "$ROOT" && LAME_LIB_DIR="$LAME_OUT" \
    RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,@executable_path/../Frameworks" \
    cargo build --release --locked "${CARGO_PKGS[@]}" "${args[@]}")
fi

rm -rf "${WORK:?}/bin"
mkdir -p "$WORK/bin"
for bin in "${BINS[@]}"; do
  inputs=()
  for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$bin"); done
  lipo -create -output "$WORK/bin/$bin" "${inputs[@]}"
  lipo -info "$WORK/bin/$bin"
done
# The app must load LAME through @rpath, not from a path on the build machine.
if ! otool -L "$WORK/bin/craftradio" | grep -q "@rpath/$LAME_DYLIB"; then
  otool -L "$WORK/bin/craftradio" >&2
  echo "error: craftradio does not load @rpath/$LAME_DYLIB" >&2
  exit 1
fi

# ---- signing helpers ---------------------------------------------------------------------------
sign() {
  # A real identity gets a secure timestamp. Ad-hoc signatures cannot have one.
  # A real identity also gets the hardened runtime, which notarization needs.
  # Ad-hoc code has no Team ID, so the hardened runtime would refuse to load the bundled LAME.
  local ts=(--timestamp --options runtime)
  [[ "$IDENTITY" = "-" ]] && ts=(--timestamp=none)
  local kc=()
  [[ -n "${MACOS_KEYCHAIN:-}" ]] && kc=(--keychain "$MACOS_KEYCHAIN")
  codesign --force --sign "$IDENTITY" ${kc[@]+"${kc[@]}"} "${ts[@]}" "$@"
}

notarize() {
  local file="$1" out id status
  echo "==> notarizing $(basename "$file") (this can take a few minutes)"
  out="$(xcrun notarytool submit "$file" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" --wait --timeout 1h --output-format json)" || true
  echo "$out"
  id="$(printf '%s' "$out" | plutil -extract id raw -o - - 2>/dev/null || true)"
  status="$(printf '%s' "$out" | plutil -extract status raw -o - - 2>/dev/null || true)"
  if [[ "$status" != "Accepted" ]]; then
    if [[ -n "$id" ]]; then
      xcrun notarytool log "$id" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" || true
    fi
    echo "error: notarization of $(basename "$file") failed (status: ${status:-unknown})" >&2
    exit 1
  fi
}

# ---- CraftRadio.app ----------------------------------------------------------------------------
echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$WORK/bin/craftradio" "$APP/Contents/MacOS/craftradio"
cp "$WORK/bin/craftradio-cli" "$APP/Contents/MacOS/craftradio-cli"
cp "$LAME_OUT/$LAME_DYLIB" "$APP/Contents/Frameworks/$LAME_DYLIB"
cp "$ROOT/assets/app-icon/craftradio.icns" "$APP/Contents/Resources/craftradio.icns"
mkdir -p "$APP/Contents/Resources/Licenses"
copy_docs "$APP/Contents/Resources/Licenses"
copy_lame_notice "$APP/Contents/Resources/Licenses" "CraftRadio.app/Contents/Frameworks/$LAME_DYLIB"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${CRAFTRADIO_BUILD_SHA:-unknown}/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"

# Sign the nested code first, then the bundle. The bundle signature does not use --deep.
sign --identifier "$APP_ID.lame" "$APP/Contents/Frameworks/$LAME_DYLIB"
sign --identifier "$APP_ID-cli" "$APP/Contents/MacOS/craftradio-cli"
sign --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/craftradio"
sign --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"
# The app must start with the bundled LAME. It answers --version before it opens a window.
"$APP/Contents/MacOS/craftradio" --version

if [[ "$NOTARIZE" = 1 ]]; then
  ditto -c -k --keepParent "$APP" "$WORK/craftradio-notarize.zip"
  notarize "$WORK/craftradio-notarize.zip"
  retry xcrun stapler staple "$APP"
  retry xcrun stapler validate "$APP"
  spctl --assess --type execute -vvv "$APP"
fi

# ---- DMG ---------------------------------------------------------------------------------------
echo "==> building $DMG"
STAGE="$WORK/dmg"
rm -rf "$STAGE"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/CraftRadio.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# `hdiutil create` sometimes fails on CI runners with "Resource busy", so it gets five tries.
# Usage: hdiutil_retry <what> <hdiutil args…>
hdiutil_retry() {
  local what="$1" attempt
  shift
  for attempt in 1 2 3 4 5; do
    if hdiutil "$@"; then
      return 0
    fi
    [[ "$attempt" = 5 ]] && { echo "hdiutil $what failed 5 times" >&2; return 1; }
    echo "hdiutil $what failed (attempt $attempt of 5); retrying in $((attempt * 10))s" >&2
    sleep $((attempt * 10))
  done
}
hdiutil_retry create create -srcfolder "$STAGE" -volname "CraftRadio" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"
sign "$DMG"
codesign --verify --strict --verbose=2 "$DMG"
# The app in the image must also pass the strict check.
MOUNT="$WORK/dmg-check"
rm -rf "$MOUNT"
mkdir -p "$MOUNT"
hdiutil_retry attach attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG" >/dev/null
if ! codesign --verify --strict --deep --verbose=2 "$MOUNT/CraftRadio.app"; then
  hdiutil_retry detach detach "$MOUNT" >/dev/null || true
  exit 1
fi
hdiutil_retry detach detach "$MOUNT" >/dev/null
if [[ "$NOTARIZE" = 1 ]]; then
  notarize "$DMG"
  retry xcrun stapler staple "$DMG"
  retry xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature -vvv "$DMG"
fi

# ---- CLI ---------------------------------------------------------------------------------------
echo "==> building $CLI_ZIP"
CLI_DIR="$WORK/craftradio-cli-$VERSION-macos-$ARCH"
rm -rf "$CLI_DIR"
mkdir -p "$CLI_DIR"
cp "$WORK/bin/craftradio-cli" "$CLI_DIR/"
copy_docs "$CLI_DIR"
sign --identifier "$APP_ID-cli" "$CLI_DIR/craftradio-cli"
codesign --verify --strict --verbose=2 "$CLI_DIR/craftradio-cli"
rm -f "$CLI_ZIP"
ditto -c -k --keepParent "$CLI_DIR" "$CLI_ZIP"
# A bare Mach-O cannot hold a stapled ticket. Gatekeeper finds the ticket online on first run.
if [[ "$NOTARIZE" = 1 ]]; then notarize "$CLI_ZIP"; fi
"$CLI_DIR/craftradio-cli" --version

echo "==> done (check the files as shipped with packaging/macos/verify.sh --arch $ARCH)"
ls -lh "$DIST"/craftradio*-"$VERSION"-macos-"$ARCH".*
