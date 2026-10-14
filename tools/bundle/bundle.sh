#!/bin/sh
# Builds target/bundle/musicbox.app (release) and signs it ad hoc.
#   tools/bundle/bundle.sh            -> target/bundle/musicbox.app
#   tools/bundle/bundle.sh --install  -> also copies it to ~/Applications
# Nothing from rekordbox is bundled: libmpg123 and AudioToolbox are loaded at run time
# (docs/re/decode.md), and the app reads the user's own library and settings.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
cargo build --release -p musicbox
APP="$ROOT/target/bundle/musicbox.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/release/musicbox" "$APP/Contents/MacOS/musicbox"
# The app icon is drawn in code (apps/musicbox/src/app_icon.rs); iconutil packs it.
ICONSET="$ROOT/target/bundle/musicbox.iconset"
rm -rf "$ICONSET"
cargo run --release -q -p musicbox --example app_icon -- "$ICONSET"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/musicbox.icns"
rm -rf "$ICONSET"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>musicbox</string>
  <key>CFBundleDisplayName</key><string>musicbox</string>
  <key>CFBundleIdentifier</key><string>local.musicbox.app</string>
  <key>CFBundleExecutable</key><string>musicbox</string>
  <key>CFBundleIconFile</key><string>musicbox</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
</dict>
</plist>
PLIST
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --verbose=1 "$APP"
echo "built $APP"
if [ "${1:-}" = "--install" ]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/musicbox.app"
  cp -R "$APP" "$HOME/Applications/musicbox.app"
  echo "installed $HOME/Applications/musicbox.app"
fi
