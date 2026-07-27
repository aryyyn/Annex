#!/usr/bin/env bash
#
# Builds Annex.app.
#
# Why a bundle is not optional
# ----------------------------
# macOS attaches privacy permissions to an *application*, and a bare executable
# run from a shell is not one. Without this, the Screen Recording grant lands on
# your terminal rather than on Annex, which means the permission follows the
# wrong thing, cannot be revoked independently, and vanishes if you launch from
# somewhere else. A bundle also gets you Spotlight, Login Items, and a Finder
# icon, none of which a loose binary can have.
#
# Signing
# -------
# Signs ad-hoc if no Developer ID is present, which is enough to run locally.
# Read the note printed at the end before relying on that: an ad-hoc signature
# has real consequences for how long a permission grant survives.

set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Annex"
BUNDLE_ID="app.annex.host"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
OUT="${1:-target/bundle}"
APP="$OUT/$APP_NAME.app"

echo "==> building release binary"
cargo build --release -p annex-host

echo "==> laying out $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/annex "$APP/Contents/MacOS/annex"

echo "==> generating the icon"
ICONSET="$OUT/$APP_NAME.iconset"
rm -rf "$ICONSET"
# Drawn by the same code that documents it, so the repository carries no
# binary asset and the icon cannot drift from its source.
./target/release/annex --emit-iconset "$ICONSET" >/dev/null
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

echo "==> writing Info.plist"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>                 <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>          <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>           <string>$BUNDLE_ID</string>
  <key>CFBundleVersion</key>              <string>$VERSION</string>
  <key>CFBundleShortVersionString</key>   <string>$VERSION</string>
  <key>CFBundleExecutable</key>           <string>annex</string>
  <key>CFBundlePackageType</key>          <string>APPL</string>
  <key>CFBundleIconFile</key>             <string>AppIcon</string>
  <key>LSMinimumSystemVersion</key>       <string>12.3</string>

  <!-- Menu bar only: no Dock icon, no application menu. Annex has no windows
       of its own, so a Dock tile would be an empty promise. -->
  <key>LSUIElement</key>                  <true/>

  <key>NSHumanReadableCopyright</key>     <string>MIT licensed</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

echo "==> signing"
IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
  | grep -m1 'Developer ID Application' | sed 's/.*"\(.*\)"/\1/' || true)"

if [ -n "$IDENTITY" ]; then
  echo "    using $IDENTITY"
  # Hardened runtime is required for notarisation. It does not conflict with
  # the private CGVirtualDisplay API: notarisation is an automated malware
  # scan and does not inspect for private-API use. App Review does, and that
  # applies only to the Mac App Store, which is out of scope by design.
  codesign --force --options runtime --timestamp \
           --sign "$IDENTITY" "$APP"
  SIGNED_WITH="Developer ID"
else
  codesign --force --sign - "$APP"
  SIGNED_WITH="ad-hoc"
fi

codesign --verify --deep --strict "$APP" && echo "    signature verifies ($SIGNED_WITH)"

echo
echo "built $APP"
echo
if [ "$SIGNED_WITH" = "ad-hoc" ]; then
cat <<'NOTE'
  Signed ad-hoc, because no Developer ID certificate is installed.

  This runs fine locally, with one real consequence: macOS identifies an
  ad-hoc signed app by the hash of its code, and that hash changes on every
  rebuild. The Screen Recording permission is tied to that identity, so you
  will have to grant it again after each rebuild, and stale entries pile up
  in System Settings.

  A Developer ID certificate gives the app a stable identity, so the grant
  survives rebuilds, and it is also what lets you notarise and hand the app
  to anyone else.
NOTE
fi
