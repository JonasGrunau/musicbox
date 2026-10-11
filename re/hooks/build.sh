#!/bin/sh
# Builds an instrumented APFS clone of rekordbox at ~/re-work/rekordbox-instr.app.
# The installed app is only read. Re-run after a rekordbox update.
set -eu
SRC="/Applications/rekordbox 7/rekordbox.app"
DST="$HOME/re-work/rekordbox-instr.app"
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/re-work/traces"
rm -rf "$DST"
cp -cR "$SRC" "$DST"   # clonefile: shares blocks with the original, ~0 extra disk
LIB="$DST/Contents/MacOS"
mv "$LIB/libsqlcipher.0.dylib" "$LIB/libsqlcipher_real.0.dylib"
install_name_tool -id @executable_path/libsqlcipher_real.0.dylib "$LIB/libsqlcipher_real.0.dylib" 2>/dev/null
codesign --force -s - "$LIB/libsqlcipher_real.0.dylib"
clang -dynamiclib -arch arm64 -O2 -o "$LIB/libsqlcipher.0.dylib" "$HERE/sqltrace.c" \
  -install_name @executable_path/libsqlcipher.0.dylib -compatibility_version 9.0.0 -current_version 9.6.0 \
  -Wl,-reexport_library,"$LIB/libsqlcipher_real.0.dylib"
codesign --force -s - "$LIB/libsqlcipher.0.dylib"
echo "built $DST — launch with: \"$LIB/rekordbox\"   traces: ~/re-work/traces/"
