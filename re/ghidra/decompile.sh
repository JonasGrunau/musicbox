#!/bin/sh
# Headless Ghidra helpers for the rekordbox project (read-only; output stays in ~/re-work).
#   re/ghidra/decompile.sh dump <query>...   -> ~/re-work/decomp/<addr>_<name>.c
#       query: 0xADDRESS | exact::qualified::name | ~substring
#   re/ghidra/decompile.sh list <substring>... -> prints matches (addr, size, name)
#   re/ghidra/decompile.sh callers <0xADDRESS>... -> prints every reference to each function
# Calls are serialised with a lock directory because the project cannot be opened twice.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
export JAVA_HOME="$HOME/.asdf/installs/java/temurin-21.0.7+6.0.LTS"
export PATH="$JAVA_HOME/bin:$PATH"
GHIDRA="$HOME/Applications/ghidra_12.1.4_PUBLIC/support/analyzeHeadless"
LOCK="$HOME/re-work/ghidra/.musicbox-lock"
until mkdir "$LOCK" 2>/dev/null; do sleep 2; done
trap 'rmdir "$LOCK"' EXIT INT TERM
mode="$1"; shift
run() {
  GHIDRA_HEADLESS_MAXMEM=16G "$GHIDRA" "$HOME/re-work/ghidra" rekordbox -process rekordbox-arm64 \
    -noanalysis -readOnly -scriptPath "$HERE" -postScript "$@" 2>&1 | grep -E "WROTE|NOT FOUND|ERROR.*(Script|Exception)" || true
}
case "$mode" in
  dump) run MbDumpFunctions.java "$HOME/re-work/decomp" "$@" ;;
  list)
    out="$(mktemp -t ghidra-list)"
    run MbListFunctions.java "$out" "$@" >/dev/null
    cat "$out"; rm -f "$out" ;;
  callers)
    out="$(mktemp -t ghidra-callers)"
    run MbCallers.java "$out" "$@" >/dev/null
    cat "$out"; rm -f "$out" ;;
  *) echo "usage: $0 dump|list|callers ..." >&2; exit 2 ;;
esac
