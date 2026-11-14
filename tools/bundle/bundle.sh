#!/bin/sh
# Builds target/bundle/musicbox.app (release) and signs it ad hoc.
#   tools/bundle/bundle.sh            -> target/bundle/musicbox.app
#   tools/bundle/bundle.sh --install  -> also copies it to ~/Applications
#   tools/bundle/bundle.sh --pkg      -> also builds target/bundle/musicbox.pkg; install it with
#                                        sudo installer -pkg target/bundle/musicbox.pkg -target /
# Installed by the system installer, /Applications/musicbox.app carries no com.apple.provenance
# attribute, so the files it writes to a FAT stick get no `._` AppleDouble companions. A copy
# made by a process started from a terminal (`--install`) is tagged, and every file and folder
# it creates on the stick is then written twice and its companion deleted again; an export runs
# 3.7x faster without (docs/re/export.md § 8).
# Environment (all optional; the release workflow sets them, .github/workflows/release.yml):
#   MB_VERSION             version for Info.plist and the package (default: Cargo.toml's);
#                          a suffix (1.2.0-beta.1) is dropped there, which takes MAJOR.MINOR.PATCH
#   MB_SIGN_IDENTITY       codesign identity, e.g. "Developer ID Application: …" (default: ad hoc);
#                          a real identity signs with the hardened runtime and
#                          tools/bundle/musicbox.entitlements
#   MB_INSTALLER_IDENTITY  "Developer ID Installer: …" to sign the package (default: unsigned)
# Nothing from rekordbox is bundled: libmpg123 and AudioToolbox are loaded at run time
# (docs/re/decode.md), and the app reads the user's own library and settings.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
INSTALL=0
PKG=0
for arg in "$@"; do
  case "$arg" in
    --install) INSTALL=1 ;;
    --pkg) PKG=1 ;;
    *) echo "bundle.sh: unknown option $arg" >&2; exit 2 ;;
  esac
done
VERSION="${MB_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
VERSION="${VERSION%%-*}"
SIGN_IDENTITY="${MB_SIGN_IDENTITY:--}"
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
if [ "$SIGN_IDENTITY" = - ]; then
  codesign --force --sign - --timestamp=none "$APP"
else
  # Notarization needs the hardened runtime; the entitlements let it load rekordbox's libmpg123,
  # which another team signed.
  codesign --force --timestamp --options runtime \
    --entitlements "$ROOT/tools/bundle/musicbox.entitlements" --sign "$SIGN_IDENTITY" "$APP"
fi
codesign --verify --strict --verbose=1 "$APP"
echo "built $APP"
if [ "$INSTALL" = 1 ]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/musicbox.app"
  cp -R "$APP" "$HOME/Applications/musicbox.app"
  echo "installed $HOME/Applications/musicbox.app"
fi
if [ "$PKG" = 1 ]; then
  # pkgbuild stores extended attributes in its payload (as `._` entries; neither
  # COPYFILE_DISABLE nor --filter keeps them out), and the installer would put the tag back.
  # So the payload and its bill of materials are made again without them.
  WORK="$ROOT/target/bundle/pkg-work"
  PKGFILE="$ROOT/target/bundle/musicbox.pkg"
  rm -rf "$WORK" "$PKGFILE"
  mkdir -p "$WORK/root"
  ditto --norsrc --noextattr --noacl --noqtn "$APP" "$WORK/root/musicbox.app"
  pkgbuild --analyze --root "$WORK/root" "$WORK/components.plist" >/dev/null
  # Installed where the package says, not over another copy with the same bundle ID.
  plutil -replace 0.BundleIsRelocatable -bool false "$WORK/components.plist"
  pkgbuild --root "$WORK/root" --component-plist "$WORK/components.plist" --ownership preserve \
    --identifier local.musicbox.app --version "$VERSION" --install-location /Applications \
    "$WORK/component.pkg" >/dev/null
  pkgutil --expand "$WORK/component.pkg" "$WORK/expanded"
  rm -f "$WORK/expanded/Payload" "$WORK/expanded/Bom"
  ditto -c -z --norsrc --noextattr --noacl --noqtn "$WORK/root" "$WORK/expanded/Payload"
  mkbom "$WORK/root" "$WORK/expanded/Bom"
  if [ -n "${MB_INSTALLER_IDENTITY:-}" ]; then
    pkgutil --flatten "$WORK/expanded" "$WORK/unsigned.pkg"
    productsign --timestamp --sign "$MB_INSTALLER_IDENTITY" "$WORK/unsigned.pkg" "$PKGFILE"
    pkgutil --check-signature "$PKGFILE"
  else
    pkgutil --flatten "$WORK/expanded" "$PKGFILE"
  fi
  if pkgutil --payload-files "$PKGFILE" | grep -q '/\._'; then
    echo "bundle.sh: the package payload still holds extended attributes" >&2
    exit 1
  fi
  rm -rf "$WORK"
  echo "built $PKGFILE"
  echo "install: sudo installer -pkg $PKGFILE -target /   (remove ~/Applications/musicbox.app first if present)"
fi
