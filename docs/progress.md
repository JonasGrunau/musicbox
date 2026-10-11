# Progress log

Newest entry first. Each entry records what changed, what is verified, and the next action.

## 2026-10-12 — M2 accepted by rekordbox
- **M2 user check passed ("all ok").** rekordbox 7.2.19 opened the sandbox library after musicbox edits (`mb-library` example `m2_check`) with no warning or repair dialog. All of these were shown correctly:
  - new folder + playlist with 5 tracks in order
  - playlist rename
  - rating 3 + colour Green on 5 tracks
  - title edit
  - file import
- Pre-check snapshot: `fixtures/sandbox-pre-m2`. musicbox backup: `~/Library/Application Support/musicbox/backups/20261012-010939.360`.
- **Writers:** export.pdb/exportExt.pdb (78/78 + 18/18 pages) and exportLibrary.db (all tables) reproduce rekordbox's RBTEST export byte-exact or row-exact.
- **UI:** the editing UI is done and tested headlessly with egui_kittest. The user asked for no OS-level input automation, so UI tests never touch the real mouse or keyboard.
- **Running:** `mb-usb` export engine (planner + executor), verified against RBTEST.
- **Next:**
  - Export in the UI. Writes go only to unprotected sticks: the reference stick's volume UUID 2335E32F-… is hard-blocked.
  - Spare-stick CDJ test (M3 user check).
  - Then M4 sync, and M5 decode/player/cue editing.

## 2026-10-11 (night) — M1 done, M2/M3 in progress
**Milestone status**
- **M0** done, except the analysis spikes (TF/mpg123 dlopen), which move to M6.
- **M1** done. The read-only browser covers local, export.pdb and exportLibrary.db libraries, with playlists, histories, My Tag, waveforms and artwork.
  - Verified against rekordbox.xml: 161/161 tracks, 0 unexplained mismatches.
  - The XML writer is byte-exact.
- **M2** in progress:
  - Done: process guard, session backup, `WriteTxn` (common.md rules, tests on fixture copies).
  - Running: `mb-library` ops (import, remove, playlists, metadata edits, masterPlaylists6.xml, undo).
- **M3** in progress:
  - Done: the ANLZ USB conversion is byte-exact.
  - Running: export.pdb/exportExt.pdb writer, exportLibrary.db writer.

**Repo**
- Private GitHub repo `JonasGrunau/musicbox`.
- For this session, commit and push straight to `main` (user's instruction).

**Next**
- Merge the three agents' results. Then add UI write actions behind a "write enabled" switch that only works on a library copy until the user check at the end of M2.
- Then the mb-usb planner/executor, to run a real export to the spare stick for the CDJ test.

## 2026-10-11 (later) — M0 corpus ready, M1 browser started
**Done**
- **Corpus**
  - `tools/probegen` generates 17 synthetic probes with known ground truth (`~/re-work/corpus/probes`, 466 MB). 4 of them are transcoded to 11 formats.
  - `tools/corpus/select.py` copied 100 real tracks read-only from the stick (`~/re-work/corpus/real`, 4.1 GB). They span 14 genres, and each was converted to one of AIFF, FLAC, MP3 CBR/VBR, AAC, ALAC or WAV.
  - Oracle export target: empty FAT32 MBR image at `~/re-work/images/rbtest-oracle.sparseimage`.
- **SQL trace clone:** `re/hooks/` builds `~/re-work/rekordbox-instr.app` (see `docs/re/tooling.md`). Not yet launched.
- **ANLZ (mb-anlz):** typed views for every tag. All 2,154 fixture files round-trip byte-exact through the typed views. The USB folder hash is verified on 704/704 stick sets. PSSI/PVDI scrambling is implemented but unverified (no fixtures yet).
- **musicbox app (M1 browser, read-only):**
  - Track table: virtualized, searchable and sortable, with Camelot/classic key display.
  - Playlist tree.
  - Deck strip with an RGB (PWV4) or 3-band (PWV6) overview and cue markers, drawn from rekordbox's own analysis files.
- **Fixtures run:** `cargo run -p musicbox -- --options fixtures/sandbox-initial/options.json --db fixtures/sandbox-initial/rekordbox/master.db --analysis-root fixtures/sandbox-initial/rekordbox/share`
- **UI testing:** egui only reacts to real mouse events (CGEvent), not AppleScript AX clicks. Cell labels must be `.selectable(false)` or they swallow row clicks.

- **mb-devlib:** the exportLibrary.db passphrase is decoded from the binary (`BlowFishHelper`, key "alphatheta corporation"). The same helper also decodes the master.db passphrase. The reader is verified on the fixture (704 tracks, 12 playlists, 981 entries).
- **Devices in the app:** `/Volumes` is polled every 2 s, plus `--device <root>`. Sticks are opened with `immutable=1`, so no side files are created. The real stick (mounted read-only) was browsed, and its export files are still byte-identical to the fixture afterwards.
- **Keys:** device key names like `Gmin`/`D#maj` are now mapped for alphanumeric display.

- **mb-pdb:** the export.pdb/exportExt.pdb reader is verified on the fixture.
  - Real counts are 1,355 playlist entries and 160 history entries. Community readers report 848 and 159 because they misread page offset 0x18.
  - The app now browses devices through export.pdb, as rekordbox's device browser does, with exportLibrary.db as the fallback.
- **`tools/rbdiff device <root>`** compares export.pdb against exportLibrary.db on one device. On the reference stick it finds 22 real differences:
  - Player-created playlists "TAG LIST 006–024" exist only in export.pdb. CDJs write tag lists to export.pdb only.
  - "Trial playlist - Cloud Library Sync" exists only in exportLibrary.db.
  - Track 692 has album and genre in export.pdb but not in exportLibrary.db.
  - **M4 sync must preserve player-written export.pdb data and reconcile the two DBs.**

- **Export flow spec** written (`docs/re/export.md`).
- **Spike 1 answered: disk images cannot be export targets.** rekordbox's `RemovableDeviceDetector` only accepts DiskArbitration protocol `USB` or `Secure Digital`. Oracle exports therefore need a spare physical stick or SD card; musicbox's own export tests can still use images.
- **Spike 2 answered: the instrumented clone runs.** macOS asked once for permission. The SQL trace works: the first session logged 30k statements to `~/re-work/traces/`.
- Oracle pass A settings: BPM range 70–180. There was no auto-gain option in the analysis settings, and nothing was greyed out.

- **Oracle observations (2026-10-11, 22:2x):**
  - **Export filter observed:** exporting `oracle-all` showed rekordbox's "Exportieren nicht möglich" dialog. It refuses FLAC/ALAC/WAV/AIFF outside 44.1/48/96 kHz, streaming tracks, AVI/MPG/M4V/MOV, MP4 with video, and DRUM CAPTURE files. OK skips them and exports the rest; our 22.05 kHz probes are skipped. This matches `check_support_export` (export.md §6).
  - **Instrumented clone relaunch problem:** after a quit, relaunching the clone made rekordbox close itself about 7 s after start. It calls `POST /api/v1/agent/quit` right after `showNotification`, with no crash. The first launch ran fine for 30 min. The cause is open; it could be a license or integrity check on relaunch. The normal app works. The trace from the first session is kept (`~/re-work/traces/sqltrace-37549.log`; 30k statements, includes the `oracle-small` export).
  - `ERROR Mismatch Category Setting djmdCategory` on stderr comes from `db::AppSyncDBController::getCategorySetting` @0x100a4d700 and is non-fatal.
  - Spare stick: SanDisk 3.2Gen1 123 GB, erased as FAT32/MBR, named `RBTEST`, volume UUID ADF6A64C-EF6E-34EB-B42B-4BC44934392E. Reference stick volume UUID: 2335E32F-F224-3E76-8EE8-42C79B582A15.

- **Oracle pass A captured** (`fixtures/oracle-a`, 112 MB):
  - The sandbox library: 169 tracks with phrase, vocal and key analysis.
  - `rekordbox.xml`.
  - RBTEST `PIONEER/`: 157 exported tracks and 2 playlists. Its export.pdb and exportLibrary.db agree with 0 mismatches.
  - The export log: the 4 probes at 22.05 kHz were skipped with reason [2]. The log is UTF-16LE with CRLF line endings, saved to `~/Documents/rekordbox/`.
  - The SQL traces.
- **`rbdiff anlz-export`:** for all 157 exported tracks, every USB ANLZ tag equals the local one after PSSI/PVDI unscrambling, except PPTH. `mb_anlz::usb::to_usb` reproduces all 471 USB files byte-exact. **The ANLZ part of the export writer is verified.**

- **Oracle pass B captured** (`fixtures/oracle-b`):
  - High-precision beat analysis is on. `trackAnalysisMode` still reads 0, so whether Dynamic was stored is open.
  - Compared with A: PQTZ differs on 154/169 files, PQT2 on 154 and PSSI on 142. PWAV differs on 4 and PWV2 on 1. Colour, 3-band and scroll waveforms, cues and PVDI are unchanged.
  - **A is the BALib reference, B the high-precision (madmom) reference.**
- **M0 is done except spikes 3 and 4** (TF dlopen, mpg123 dlopen). Those move to the analysis milestone, per the agreed order.

**Next (M1 finish → M2)**
- M1: verify the browser data against `fixtures/oracle-a/rekordbox.xml` (tracks, TEMPO, POSITION_MARK, playlists).
- M2: turn `fixtures/oracle-a/traces/sqltrace-37549.log` into `docs/re/ops/*.md`, covering import, playlist create/add, analysis registration and export bookkeeping. Then build `WriteTxn`.
- Ghidra headless analysis.
- User oracle session pass A.

**Next**
- Merge mb-pdb/mb-devlib, then add a Devices section to the tree.
- Compare the PWV4/PWV6 colour hypothesis against a rekordbox screenshot of the same track.

## 2026-10-11 — M0 started
**Done**
- Plan approved; a copy is in `docs/plan.md`.
- The stick `/Volumes/REKORDBOX` is remounted read-only (`diskutil mount readOnly /dev/disk6s1`).
  - `PIONEER/` is copied to `fixtures/stick/` (4,932 files; sha256 matches the stick, `fixtures/stick/PIONEER.sha256`). `PIONEER/extracted/` is excluded because it holds a credential.
  - Audio hashes go to `fixtures/stick/Contents.sha256` (written by a background job).
- Sandbox library snapshot is in `fixtures/sandbox-initial/` (master.db without WAL, plus share/, .edb, .xml and options.json).
- Stray reference files were moved out of the repo to `~/re-work/reference/`.
- The Cargo workspace exists: `mb-core`, `mb-masterdb`, `apps/musicbox` (empty eframe 0.36 window).
- **mb-masterdb opens master.db with our own key derivation.**
  - The Blowfish key is a cstring in the binary, at VA ≈0x104FAE8DD.
  - The schema dump is in `docs/re/masterdb/schema.sql`.
- Ghidra headless import and analysis of `~/re-work/bin/rekordbox-arm64` is running detached. Log: `~/re-work/ghidra/import.log`.

**Notes**
- rekordbox itself wrote to the stick on 2026-10-11 (19:42 DeviceLibBackup json; 20:00 export.pdb, exportExt.pdb, exportLibrary.db) before the stick was protected.
- If the stick is unplugged and replugged, macOS mounts it read-write again. Remount it read-only before doing anything else.

**Next**
- M0.4: `tools/probegen` and the corpus selection (about 150 tracks copied read-only from the stick, transcoded with ffmpeg).
- M0.5: the user's oracle-generation session in rekordbox (needs a checklist and an empty FAT32 image).
- M0.6 spikes: disk image as a USB device; instrumented copy (SQL shim); dlopen of TF/model7; dlopen of mpg123.
