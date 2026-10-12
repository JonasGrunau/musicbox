# musicbox

A Rust/egui replacement for rekordbox 7's library management: the collection, playlists, USB export and track analysis. It reads and writes the same files as rekordbox 7.2.19, so either app can be used on the same library and the same sticks.

The full plan is in `docs/plan.md`; progress and handoffs are in `docs/progress.md`.

## Safety rules (never break these)
1. **The stick `/Volumes/REKORDBOX` is a read-only reference.**
   - Never write to it.
   - Before any device work, check `mount | grep REKORDBOX` and confirm it shows `read-only`. If it doesn't, stop and ask the user.
   - Never start rekordbox while the stick is plugged in.
2. **Never write to a real library during development.** Use `fixtures/` copies, the sandbox library, or FAT32 disk images (`hdiutil create -type SPARSE -fs "MS-DOS FAT32"`).
3. **Never write `master.db` while `rekordbox` or `rekordboxAgent` is running.**
4. **Clean room:**
   - All rekordbox-specific logic is our own work, verified against the binary and real files.
   - Community projects (rekordcrate, pyrekordbox, rbox, rbxport, baken, crate-digger) are reference only: never a dependency, and their code is never copied.
   - Generic crates are fine.
   - Decompiler output and Ghidra projects stay in `~/re-work/`, outside the repo.
   - `docs/re/` is written in our own words: addresses, behaviour and constants.
   - Never copy rekordbox models, dylibs or skin assets into the repo or the app.

## UI testing
- Drive the app only through the headless harness (`apps/musicbox/tests/ui_*.rs`, egui_kittest): click by label, type, drag/drop, render off-screen. `MB_UI_SHOTS=<dir> cargo test -p musicbox` saves the renders.
- **Never** post OS-level mouse/keyboard events or take screen-region screenshots; they steal the user's input and can hit other apps.

## Layout
- `crates/mb-*` — libraries; see the architecture section of the plan.
- `apps/musicbox` — the egui app.
- `docs/re/` — the reverse-engineering source of truth. `INDEX.md` lists every spec with its status.
- `fixtures/` — gitignored real data. Only `fixtures/manifest.toml` is committed.
- `~/re-work/` (outside the repo):
  - `bin/rekordbox-arm64` — the arm64 slice
  - `ghidra/` — the headless project `rekordbox`
  - `reference/` — community format notes, for reading only

## Commands
- `cargo build` / `cargo test --workspace`
- `cargo run -p musicbox` — the app
- `tools/bundle/bundle.sh [--install]` — release build as `target/bundle/musicbox.app`, signed ad hoc (`--install` copies it to `~/Applications`). Don't launch it during development sessions; it opens a window on the user's screen.
- `cargo run -p mb-masterdb --example probe -- <options.json> [master.db]` — table row counts
- `cargo run -p mb-masterdb --example schema -- <options.json> <master.db>` — schema dump
- `re/ghidra/decompile.sh dump <0xaddr|qualified::name|~substr>...` decompiles into `~/re-work/decomp/`; `re/ghidra/decompile.sh list <substr>` lists matching functions.
- Ghidra headless (raw): put JDK 21 on `PATH` first, because the asdf `java` shim has no version set outside a project:
  ```
  export JAVA_HOME=~/.asdf/installs/java/temurin-21.0.7+6.0.LTS PATH="$JAVA_HOME/bin:$PATH"
  ~/Applications/ghidra_12.1.4_PUBLIC/support/analyzeHeadless ~/re-work/ghidra rekordbox -process rekordbox-arm64 -noanalysis -postScript <script>
  ```

## RE workflow (per subsystem)
1. Locate the code by symbol or string, in Ghidra or REA.
2. Write the spec in `docs/re/<topic>.md`: status, evidence (addresses), behaviour, open questions.
3. Observe behaviour: SQL trace, before/after diffs, or rekordbox's output on probe audio.
4. Implement, with comments pointing at the spec.
5. Score against fixtures and record the result in `docs/scorecards/`.
6. Update `docs/progress.md`.
