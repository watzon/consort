#!/usr/bin/env bash
# Builds Consort.app for macOS, and optionally a zip and a dmg.
#
#   scripts/bundle-macos.sh                      build for this Mac
#   scripts/bundle-macos.sh --target x86_64-apple-darwin
#   scripts/bundle-macos.sh --binary target/debug/consort-desktop --no-archive
#   scripts/bundle-macos.sh --dmg                also make a dmg
#   scripts/bundle-macos.sh --bundle-id dev.consort.desktop.dev
#
# Output goes to dist/ (change it with --out):
#   Consort.app
#   Consort-macos-<arm64|x86_64>.zip    for app updates and downloads
#   Consort-macos-<arm64|x86_64>.dmg    only with --dmg
#
# Signing and notarization run only when these variables are set. Without
# them the app gets an ad-hoc signature, which is enough to run it on the Mac
# that built it, and a downloaded copy needs a right-click and Open.
#
#   APPLE_SIGNING_IDENTITY   "Developer ID Application: Name (TEAMID)"
#   APPLE_ID                 the Apple ID for notarization
#   APPLE_TEAM_ID            the team ID
#   APPLE_APP_PASSWORD       an app-specific password for that Apple ID
#
# The bundle ID is the constant below, unless --bundle-id changes it. The
# version comes from [workspace.package] in Cargo.toml.
set -euo pipefail
cd "$(dirname "$0")/.."

BUNDLE_ID="dev.consort.desktop"
APP_NAME="Consort"
MIN_MACOS="12.0"

target=""
binary=""
out="dist"
archive=1
dmg=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --target) target="$2"; shift 2 ;;
    --binary) binary="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --bundle-id) BUNDLE_ID="$2"; shift 2 ;;
    --no-archive) archive=0; shift ;;
    --dmg) dmg=1; shift ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "bundle-macos: unknown option $1" >&2; exit 2 ;;
  esac
done

[ "$(uname -s)" = "Darwin" ] || { echo "bundle-macos: run this script on macOS" >&2; exit 1; }

version="$(awk '/^\[workspace.package\]/ { in_pkg = 1; next } /^\[/ { in_pkg = 0 } in_pkg && /^version/ { gsub(/"/, "", $3); print $3; exit }' Cargo.toml)"
[ -n "$version" ] || { echo "bundle-macos: no version in Cargo.toml" >&2; exit 1; }

if [ -z "$target" ]; then
  case "$(uname -m)" in
    arm64) target="aarch64-apple-darwin" ;;
    *) target="x86_64-apple-darwin" ;;
  esac
fi
case "$target" in
  aarch64-apple-darwin) key="macos-arm64" ;;
  x86_64-apple-darwin) key="macos-x86_64" ;;
  *) echo "bundle-macos: unsupported target $target" >&2; exit 2 ;;
esac

if [ -z "$binary" ]; then
  # The deployment target makes the binary agree with LSMinimumSystemVersion.
  MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS" cargo build --release --locked -p consort-desktop --target "$target"
  binary="target/${target}/release/consort-desktop"
fi
[ -f "$binary" ] || { echo "bundle-macos: no program at $binary" >&2; exit 1; }

icon="design/logo/icons/Consort.icns"
[ -f "$icon" ] || scripts/make-icons.sh

app="${out}/${APP_NAME}.app"
rm -rf "$app"
mkdir -p "${app}/Contents/MacOS" "${app}/Contents/Resources"
cp "$binary" "${app}/Contents/MacOS/${APP_NAME}"
chmod 755 "${app}/Contents/MacOS/${APP_NAME}"
cp "$icon" "${app}/Contents/Resources/${APP_NAME}.icns"

cat >"${app}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>${APP_NAME}</string>
  <key>CFBundleIconFile</key>
  <string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>CFBundleVersion</key>
  <string>${version}</string>
  <key>LSMinimumSystemVersion</key>
  <string>${MIN_MACOS}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>MIT License</string>
</dict>
</plist>
PLIST
plutil -lint "${app}/Contents/Info.plist" >/dev/null

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "Signing with ${APPLE_SIGNING_IDENTITY}"
  codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$app"
else
  echo "No APPLE_SIGNING_IDENTITY. Signing ad hoc."
  codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"

notarize=0
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
  notarize=1
fi

if [ "$archive" -eq 1 ]; then
  zip="${out}/${APP_NAME}-${key}.zip"
  rm -f "$zip"
  ditto -c -k --keepParent "$app" "$zip"
  if [ "$notarize" -eq 1 ]; then
    echo "Sending the app to Apple for notarization. This takes a few minutes."
    xcrun notarytool submit "$zip" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$app"
    # The stapled app goes into the zip, so the ticket travels with it.
    rm -f "$zip"
    ditto -c -k --keepParent "$app" "$zip"
  fi
  echo "Wrote $zip"
fi

if [ "$dmg" -eq 1 ]; then
  image="${out}/${APP_NAME}-${key}.dmg"
  rm -f "$image"
  staging="$(mktemp -d)"
  trap 'rm -rf "$staging"' EXIT
  cp -R "$app" "$staging/"
  ln -s /Applications "$staging/Applications"
  hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$staging" -format UDZO "$image"
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$image"
  fi
  if [ "$notarize" -eq 1 ]; then
    xcrun notarytool submit "$image" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$image"
  fi
  echo "Wrote $image"
fi

echo "Wrote $app (version ${version}, bundle ID ${BUNDLE_ID})"
