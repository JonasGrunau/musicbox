# RE tooling

## Ghidra
- **Project:** `~/re-work/ghidra/rekordbox.gpr`, holding the program `rekordbox-arm64`, the arm64 slice made with `lipo -thin arm64`.
- **Java:** Ghidra 12.1.4 needs JDK 21 on `PATH`. The asdf shim fails outside folders that have a `.tool-versions` file:
  ```
  export JAVA_HOME=~/.asdf/installs/java/temurin-21.0.7+6.0.LTS PATH="$JAVA_HOME/bin:$PATH"
  ```
- **Initial import:** 2026-10-11, full auto-analysis in 1,524 s with 24 GB heap; project 1.6 GB. Log: `~/re-work/ghidra/import.log`.
- **Decompile helper** (read-only, about 4 s per call, serialised with a lock dir because the project can only be opened once):
  ```
  re/ghidra/decompile.sh dump <query>...    # → ~/re-work/decomp/<addr>_<name>.c
  re/ghidra/decompile.sh list <substring>... # addr, size, qualified name
  ```
  A query is `0xADDRESS`, an exact qualified name (`analyzer::CMusicAnalyzeFile::write_pmai`), or `~substring`. Decompiler output stays in `~/re-work`; never copy it into the repo.
- REA's decompiler timed out on this 135 MB binary; use the helper or `llvm-objdump` instead.
- GhidraMCP (interactive): open `~/re-work/ghidra/rekordbox.gpr` in the Ghidra GUI with the GhidraMCP plugin enabled, then restart Claude Code so the `ghidra` MCP server connects. The headless helper can't run while the GUI holds the project.

## SQL trace of rekordbox (`re/hooks/`)
`re/hooks/build.sh` builds `~/re-work/rekordbox-instr.app`: an APFS clone of the installed app that uses almost no extra disk. The installed app is only read.
- **How it works:**
  1. In the clone, the real `Contents/MacOS/libsqlcipher.0.dylib` is renamed to `libsqlcipher_real.0.dylib`.
  2. A shim built from `sqltrace.c` takes its place. The shim re-exports the real library and overrides `sqlite3_open` and `sqlite3_open_v2` to attach `sqlite3_trace_v2` (STMT) to every connection.
  3. rekordbox's entitlement `com.apple.security.cs.disable-library-validation` lets the ad-hoc-signed shim load.
- **Launch:** `~/re-work/rekordbox-instr.app/Contents/MacOS/rekordbox`
- **Output:** `~/re-work/traces/sqltrace-<pid>.log`, one line per statement:
  ```
  <unix ms>\t<thread>\t<db file>\t<expanded sql>
  ```
- **What the trace covers:** rekordbox imports these from libsqlcipher: `open`, `open_v2`, `key`, `prepare_v2`, `step`, `bind_*`, `exec`, `column_*`, `busy_timeout`, `changes`, `finalize`, `reset` and `close`. So the trace covers master.db and probably exportLibrary.db.
- **What it does NOT cover:**
  - The rekordboxAgent (Electron) process, which opens master.db through its own node-sqlcipher. It also serves a REST API on 127.0.0.1:30001 that the main app calls (`db::CloudAgentAPI`). Writes that go through the agent need a separate capture.
  - The statically linked SQLite 3.41.1 and the DeviceSQL runtime (export.pdb).
- **Risk:** the self-integrity check (`SecStaticCodeCheckValidity` import) or the license runtime may reject the clone. If that happens, use static RE and before/after diffs.
