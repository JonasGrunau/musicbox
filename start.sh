#!/bin/sh
# Builds (release: a debug build draws large lists 10-40x slower) and starts musicbox. Arguments are passed through to the app:
#   ./start.sh                         -> default library (~/Library/Pioneer/rekordbox)
#   ./start.sh --db <master.db> ...    -> see `./start.sh --help`
# Refuses to start while rekordbox is running or the reference stick is writable
# (CLAUDE.md safety rules 1 and 3).
set -eu
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

for p in rekordbox rekordboxAgent; do
  if pgrep -x "$p" >/dev/null; then
    echo "start.sh: $p is running; quit it before starting musicbox" >&2
    exit 1
  fi
done

STICK="$(mount | grep ' on /Volumes/REKORDBOX ' || true)"
if [ -n "$STICK" ] && ! echo "$STICK" | grep -q 'read-only'; then
  echo "start.sh: /Volumes/REKORDBOX is mounted writable; remount it read-only first" >&2
  exit 1
fi

exec cargo run --release -p musicbox -- "$@"
