# musicbox: a Rust drop-in for rekordbox 7's library and USB management

## Context
musicbox is a Rust/egui desktop app that replaces rekordbox 7.2.19 for library work:
- collection, playlists, intelligent playlists, history, My Tag
- cue, hot cue, loop and grid editing, with a preview player
- track analysis, rekordbox.xml exchange, and USB export/import for **all** Pioneer/AlphaTheta players

It reads and writes the **same on-disk library** as rekordbox, so the two apps can be swapped freely. It has no sign-in, cloud, telemetry, streaming, licensing or DJ-performance features, and no rekordbox branding or assets. The repo `/Users/jonasgrunau/Projects/musicbox` holds only an initial commit.

### User decisions (2026-10-11)
| Topic | Decision |
|---|---|
| Players | All. Write export.pdb + exportExt.pdb + ANLZ DAT/EXT/2EX + exportLibrary.db (OneLibrary) + settings files. |
| UI | egui/eframe; looks like rekordbox, with our own icons and fonts. |
| v1 extras | Preview player + waveform, cue/grid editing, rekordbox.xml, intelligent playlists + History. |
| Source of truth | **Our own RE.** All rekordbox-specific logic is our own code. Community projects (rekordcrate, pyrekordbox, rbox, rbxport, baken, crate-digger) are reading material only, never dependencies, and no code is copied from them. Generic crates are fine. |
| Local rekordbox library | Disposable sandbox until musicbox is ready (it was created today and holds only demo tracks). |
| **USB stick `/Volumes/REKORDBOX`** | **Must stay exactly as it is.** Read-only reference forever (see Safety rule 1). |
| Neural parts | Load rekordbox's own TensorFlow 2.16.1 and ONNX Runtime 1.18 libraries and models from the user's install via dlopen at runtime. Never bundled. |
| Instrumentation | Allowed on a **copy** of rekordbox in `~/re-work` (an SQL-logging shim). The original app is never touched. |
| Delivery order | Library + USB export first, using ANLZ files rekordbox produced. musicbox's own analysis lands afterwards. |

## Established facts (surveys 2026-10-11; re-verify during RE)
**Binary and runtime**
- `/Applications/rekordbox 7/rekordbox.app/Contents/MacOS/rekordbox` is universal; the arm64 slice is about 135 MB.
- It is **not stripped**: about 98k demangled C++ functions and about 403 source-path strings. UI is JUCE 8.
- Statically linked: TagLib, libFLAC, vorbis, libebur128, NumCpp, the Encirq DeviceSQL runtime (`edb_*`, `__epl_*`), and an in-house DSP library (`rbxfrm::dsp::Fft`, `HanningWindow`, `SrcCore`, `SampleRateConverter`).
- Hardened runtime with `disable-library-validation`, no `get-task-allow`, SIP on. It imports `SecStaticCodeCheckValidity`, so it may check its own integrity.

**Library location and settings**
- Paths come from `~/Library/Application Support/Pioneer/rekordboxAgent/storage/options.json` (`db-path`, `analysis-data-root-path`, `dp`). The library can be moved ("Move Master Database", `PIONEER/Master/`), so never hard-code it.
- Preferences live in `~/Library/Application Support/Pioneer/rekordbox6/rekordbox3.settings` (XML `VALUE` entries). Current values:
  - `HighPrecisionBeatAnalyze=0`
  - `trackAnalysisSettingsSongStruct=0`, `trackAnalysisSettingsVocalDetect=0`
  - `trackAnalysisSettingsNetworkAnalysis=1` (cloud download of analysis results)
  - `AutoGain=1`, `create320x320Artwork=1`

**master.db**
- SQLCipher 4 with default parameters. The passphrase is the Blowfish-ECB decryption of `dp`.
- WAL mode; must be read through the WAL.
- 48 tables (DDL captured → `docs/re/masterdb/schema.sql`).
- Two writers: rekordbox (C++ `SQLiteDB`) and rekordboxAgent (Sequelize).

**Writing to master.db**
- **Counters:** `agentRegistry('localUpdateCount').int_1` increments per change, and every changed row gets `rb_local_usn` set.
- **Row formats:** timestamps `YYYY-MM-DD HH:MM:SS.mmm +00:00`; IDs are random u32 values stored as decimal strings; UUIDs are v4; deletes are soft (`rb_local_deleted=1`).
- **Analysis files:**
  - `AnalysisDataPath` = `/PIONEER/USBANLZ/<uuid[0:3]>/<uuid[3:]>/ANLZ0000.DAT`.
  - `contentFile` holds the MD5 and size of each ANLZ file.
- **Companion files:**
  - `masterPlaylists6.xml` is kept in sync.
  - `datafile.edb` and `ExtData.edb` (legacy DeviceSQL mirrors) travel with master.db.

**ANLZ files**
- **Formats:** local `.DAT/.EXT/.2EX` are PMAI-tagged. `.3EX` is MessagePack holding ML embeddings. Local PPTH is `?/<file>`; on USB it is `/Contents/...`.
- **Tag set:** PPTH, PVBR, PQTZ, PQT2, PWAV, PWV2–7, PWVC, PCOB/PCO2, PSSI, PVB2, and **PVDI** (vocal, new).
- **Writer and validator:**
  - Writer: `analyzer::CMusicAnalyzeFile` (`write_pmai*`); per-tag header-length tables @0x10378f9c0.
  - Validator: `PCF2::checkAtom*`.
- **USB conversion:**
  - Entry point: `ExportAnalysisData` @0x1007b7c0c.
  - PSSI and PVDI are scrambled with `ConvUSB_Scramble*`.
  - Folder hash: `CreateAnlzFileFolderPath`.

**Analysis (driver `analyzer::CBeatAnalyzeThread::analyze` @0x1001528c4)**
- **Decode.** All formats are decoded to stereo float; 44.1/48 kHz are analysed natively, other rates are resampled (multiples of 8k → 48k, otherwise → 44.1k).

  | Format | Decoder |
  |---|---|
  | MP3 | libmpg123 feed API + `MpegFrameOffset` |
  | AAC/M4A | AudioToolbox |
  | ALAC | ALACDecoder |
  | FLAC | libFLAC |
  | WAV/AIFF | own readers |

- **BPM and grid: BALib** (`BeatAnalyzer_2_0::BA_*`, classic DSP).
  - Normal mode uses Const; Dynamic uses Varied + Hendou.
  - BPM-range presets run from "70-180" to "128-255".
  - **High-precision mode** adds a madmom-style network (`models/detect_beat/model7|8`, a TF SavedModel: STFT front end inside the graph, three bidirectional peephole-LSTM layers with 25 units, dense 50→3) plus a C++ downbeat HMM (`madmom::downbeat_tracking`).
- **Key:** BALib `BA_GetKey` → `KeyJudge2.cpp`. Stored in the DB only.
- **Waveforms:**
  - PWAV/PWV2/PWV3 come from BALib (Loud/Dots/Zoom wave).
  - PWV4/PWV5/PWV6/PWV7/PWVC come from `WaveCreator`.
- **Phrase (PSSI):** rule-based `SongStructureAnalyze`.
- **Loudness:** libebur128 integrated LUFS; gain dB = −10.0 − LUFS.
- **Vocal:** librosa-style features → TF BiLSTM (`vd_param_raw.pb`) → PVDI.
- **Optional ML (ONNX):** auto-cue (SampleCNN + XGBoost; reads `cue_personal_trend.json`) and embeddings (`.3EX`).
- **`Analysed` bits:** 105 = 0x69, 121 = 0x79. Bits 0x01|0x20 mean current version (`RowDataTrack::isAnalyzedOldVer`). Bits 0x08/0x10/0x40 are still open.

**USB stick (the reference)**
- Contents:
  - `PIONEER/rekordbox/{export.pdb, exportExt.pdb, exportLibrary.db, playlists3.sync, playlists3Plus.sync, RBFLTR.DAT}`
  - `PIONEER/*.DAT` settings, `PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ000N.*` (703 sets, one `ANLZ0001` collision)
  - Artwork `a|b N[_m].jpg` (80/240 px)
- It belongs to a different library (sync DBID −1966741239). Its ANLZ files are mostly from 2020–21, so they are **not** 7.2.19 references for analysis.
- `exportLibrary.db` is SQLCipher 4 with a second key, which we re-derive and document. Its schema is captured.
- **rekordbox itself wrote to the stick today:** `DeviceLibBackup/…json` at 19:42 and `export.pdb`, `exportExt.pdb` and `exportLibrary.db` at 20:00. Nothing in this session wrote to it.

**Not reimplemented:** Bugsnag, analytics, Gigya/KUVO, licensing (`rbinit`, Sentinel), rekordboxAgent/cloud, streaming, Dropbox/Google Drive, `rb_http_server`, the updater, `djprofile.nxs`, LINK export (Pro DJ Link), `networkAnalyze6.db`.

**Machine:** M4 Pro, 48 GB RAM, **40 GB free disk**. Ghidra 12.1.4 at `~/Applications/ghidra_12.1.4_PUBLIC` (Temurin 21 set in `launch.properties`). Homebrew ffmpeg is installed.

## Safety rules (every phase, every agent session)
1. **The stick `/Volumes/REKORDBOX` is never written to.**
   - Remount it read-only before any work: `diskutil unmount /Volumes/REKORDBOX && diskutil mount readOnly /dev/disk6s1`.
   - Never run rekordbox while it is plugged in (rekordbox rewrites every stick it sees).
   - All export testing uses FAT32 disk images (`hdiutil create -type SPARSE -fs "MS-DOS FAT32"`) or a separate spare stick.
2. **No writes to any real library during development.** Work goes into the sandbox library (disposable) or copies under `fixtures/`. In the shipped app, writing the live library is opt-in after M2 is verified.
3. **Refuse writes while `rekordbox` or `rekordboxAgent` is running.** On launch detection (polling or NSWorkspace), finish the current transaction and switch to read-only.
4. **Back up before the first write of each session** into `~/Library/Application Support/musicbox/backups/<ts>/`: master.db via `sqlcipher_export`, plus `.edb` files, `masterPlaylists6.xml`, and copy-on-write copies of every ANLZ file touched. Never use rekordbox's `master.backup.*` names.
5. **Clean-room hygiene:**
   - Decompiler output and Ghidra projects stay in `~/re-work/` (outside the repo).
   - `docs/re/` is written in our own words: addresses, behaviour, constants.
   - No community code is copied.
   - Model files and rekordbox dylibs are never copied into the repo or the app bundle.

## Architecture: Cargo workspace
```
crates/
  mb-core       pure types (Track, Playlist, Cue, BeatGrid, Key, Color…), ID/UUID/time formatting. No I/O.
  mb-masterdb   master.db storage: options.json paths + dp→key, rusqlite(bundled-sqlcipher), typed rows (48 tables),
                private WriteTxn = the ONLY place USN/ID/timestamp/counter rules live; masterPlaylists6.xml; .edb handling
  mb-library    command layer: one module per rekordbox operation (import, playlist ops, edit, relocate, delete,
                cue/grid, device import); undo as inverse commands (new USNs); change events; job manager
  mb-anlz       PMAI + all tags incl. PVDI, .3EX msgpack, USB conversion (PPTH, folder hash, scrambling); byte-exact
  mb-pdb        DeviceSQL export.pdb/exportExt.pdb reader + writer + lint
  mb-devlib     exportLibrary.db reader/writer (shares row definitions with mb-masterdb where the schemas mirror each other)
  mb-usb        device discovery (DiskArbitration/volume watch), ownership (DBID), planner (pure: lib+device+selection→ExportPlan)
                + executor (copy, artwork, ANLZ, PDB-last, F_FULLFSYNC, eject); formats: *SETTING.DAT, RBFLTR.DAT, *.sync, DeviceLibBackup
  mb-audio      decode spine: mpg123 (dlopen bundled lib) + frame map, AudioToolbox FFI, FLAC/WAV/AIFF/ALAC; rekordbox's SRC port; tags (lofty); artwork
  mb-dsp        ports of rekordbox primitives: Fft, HanningWindow, SrcCore, IIR filters (bit-matching, not rustfft)
  mb-analysis   BALib (beat/BPM/key/mono waves), WaveCreator (colour/3-band/PWVC), loudness, phrase, vocal features, job pipeline
  mb-nn         all unsafe FFI: dlopen TF 2.16.1 C API + ORT 1.18 from rekordbox install; version + model-MD5 checks, refuse on mismatch
  mb-xml        rekordbox.xml import/export
  mb-player     preview playback (cpal) on the same decode spine
apps/musicbox   eframe UI
tools/rbdiff    lib + CLI: db (canonicalised by natural keys), sqltrace, anlz (per-tag tolerances), pdb/devlib (rows + lint), analysis scorecards
tools/probegen  synthetic, copyright-free probe audio (sines, clicks at exact BPMs, tempo changes, every format/rate/bit depth)
xtask           re dump-fn <symbol> (headless Ghidra → ~/re-work), fixtures snapshot, corpus scoring
re/hooks        SQL-logging shim dylib for the instrumented rekordbox copy
docs/re/        INDEX.md + one spec per subsystem (status: verified / hypothesis / open; evidence; next actions)
docs/re/ops/    one spec per library operation (exact SQL from the trace)
docs/scorecards/ committed JSON metrics only
docs/progress.md handoff log between sessions;  CLAUDE.md: safety rules, commands, clean-room rule
fixtures/       gitignored real data + committed manifest.toml (sha256, source, rekordbox ver, macOS ver, analysis settings)
tests/golden/   committed synthetic probes + rekordbox's analysis of them (copyright-free)
```
**Generic dependencies:**
- UI and audio I/O: eframe/egui, egui_extras, cpal
- Database: rusqlite (`bundled-sqlcipher`)
- Crypto: blowfish, md-5
- Audio metadata: lofty, `libloading` (for the bundled mpg123/TF/ORT)
- Images: image (artwork resize)
- File formats: quick-xml, rmp-serde
- System: notify, sysinfo, uuid, crc

**UI ↔ DB contract:**
- One writer actor owns the write connection; each command is one `BEGIN IMMEDIATE` transaction.
- Short-lived read connections only, never long read transactions, because those block rekordbox's WAL checkpoints.
- The UI renders an `Arc` snapshot updated from change events. Sort and filter run off the UI thread (50k+ rows).
- External changes are detected via `PRAGMA data_version`.
- CJK fonts are loaded from the system (PingFang/Hiragino).

## RE method (same loop per subsystem)
1. **Locate:** symbol or string in Ghidra (headless project in `~/re-work`; GUI with GhidraMCP for interactive sessions) or REA.
2. **Spec:** `docs/re/<subsystem>.md` with addresses, behaviour, constants and open questions.
3. **Observe:**
   - The SQL trace from the instrumented copy, for DB operations.
   - Before/after `rbdiff db` snapshots.
   - rekordbox's output on probe audio.
4. **Implement**, with comments that reference the spec section.
5. **Score** against fixtures and the acceptance thresholds below. Record the scorecard and update `docs/progress.md`.

## Milestones (delivery order agreed: library + export first)

### M0: Setup, oracles, spikes (3–5 days; Ghidra auto-analysis runs 4–8 h unattended)
1. **Protect the stick.** Remount it read-only, then snapshot `PIONEER/` (read-only copy, about 1 GB) and a sha256 list of `Contents/` into `fixtures/stick/`.
2. **Repo hygiene.**
   - Move the stray `anlz.ksy`, `pdb.ksy`, `readme.adoc`, `db.py` and `dl/` to `~/re-work/reference/`.
   - Add `.gitignore` entries (`fixtures/`, `.omc/`, `target/`), the workspace skeleton with an empty eframe window, `CLAUDE.md`, `docs/re/INDEX.md` and `docs/progress.md`.
3. **Ghidra.** `lipo -thin arm64` into `~/re-work`, start the headless import and analysis in the background, and add `xtask re dump-fn`.
4. **Oracle corpus.**
   - `tools/probegen` synthetic set (committed).
   - About 150 real tracks **copied (read-only) from the stick**, at most 10 GB, transcoded with ffmpeg into every format (MP3 CBR/VBR ± LAME header, AAC, ALAC, FLAC, WAV, AIFF; 44.1/48/96 kHz).
5. **Oracle generation in the sandbox library** (user session, about 60 min, written checklist):
   - Turn network analysis OFF. Turn phrase, vocal and key ON. Leave high-precision OFF.
   - Import the corpus, analyse it, export `rekordbox.xml`, then do a fresh USB export of the corpus playlists to an **empty FAT32 disk image**.
   - Repeat the analysis with high-precision ON and with Dynamic mode, into separate snapshots.
   - Snapshot everything into `fixtures/oracle/`.
6. **Spikes (half a day each):**
   1. Does rekordbox accept a mounted disk image as a USB device? If not, we need a spare stick.
   2. Instrumented copy: a shim `libsqlcipher.0.dylib` in `~/re-work/rekordbox-instr.app`, logging SQL to a file. If the integrity check blocks it, fall back to static RE + diffs.
   3. dlopen TF from the rekordbox install, run `detect_beat/model7` on the demo MP3, compare the beats to rekordbox's PQTZ.
   4. dlopen the bundled libmpg123 and decode the demo MP3 → compare sample counts and offset with PVBR.

### M1: Read-only browser (1.5–2.5 weeks) → "musicbox shows my collection, playlists, and stick like rekordbox"
1. `mb-masterdb` reader: read paths from options.json, derive the key, open through the WAL, typed rows.
2. UI shell with rekordbox's layout:
   - Tree: Collection, Playlists, Intelligent, History, My Tag, Hot Cue Bank, Devices.
   - Virtualized, sortable track table with configurable columns.
   - Artwork, search/filter bar, and the dark palette (`#282828`, `#1E1E1E`, `#25292B`, accent `#1473EB`).
3. `mb-anlz` full read/write, plus waveform rendering (PWV5 detail, PWV4 overview, PWV6/7 3-band).
4. `mb-pdb` and `mb-devlib` readers, and `mb-usb` discovery: browse any stick read-only, filtering out `._*` files.

**Verify:**
- The library matches the `rekordbox.xml` oracle: tracks, cues, TEMPO, playlists.
- The stick shows 704 tracks.
- ANLZ read→write is byte-identical on all fixture sets.

### M2: Editing + import from device (2–4 weeks) → "edits in musicbox show up correctly in rekordbox"
1. Capture SQL traces of rekordbox doing each operation, then write `docs/re/ops/*.md`. Operations:
   - import file
   - playlist and folder create/rename/move/delete/reorder
   - add/remove tracks
   - metadata edit, rating, colour, comment, My Tag
   - hot cue bank lists
   - delete and relocate missing files
2. Implement them in `mb-library` through `WriteTxn`, with undo.
3. Fill every field rekordbox sets when importing:
   - `ContentLink`, `rb_file_id`, `MasterDBID`/`MasterSongID`, `DeviceID`
   - `SearchStr`, `FileNameL/S`, `OrgFolderPath`
   - `Analysed`, `*Updated`
   - artwork in local `_org/_s/_m` variants
   - NFC-normalised paths
   - duplicate detection
4. **Import from device:** stick → collection, copying rekordbox-made ANLZ files with PPTH rewritten. History import is deduplicated via `ImportedHistory.xml`.
5. Add drag and drop, multi-select and context menus.

**Verify:**
- `rbdiff sqltrace` and `rbdiff db` show our operations are equivalent to rekordbox's.
- User check (about 5 minutes): rekordbox opens the edited sandbox with no repair prompt and shows every change.

### M3: USB export walking skeleton (1–2 weeks) → "5 tracks + 1 playlist play on a CDJ"
1. RE the `DatabaseMediator::execExport` flow and the DeviceSQL writer, then write `docs/re/export.md`.
2. Write the minimum fresh export:
   - export.pdb + exportExt.pdb + exportLibrary.db + settings DATs
   - ANLZ files (PPTH, folder hash, scrambling)
   - artwork (a/b × plain/_m; 320 px when that setting is on)
   - PDB written last, then `F_FULLFSYNC`

**Verify:**
- `rbdiff pdb/devlib` rows equal rekordbox's fresh export of the same playlist in the oracle disk image.
- `pdb-lint` is clean.
- User check (about 15 minutes): a **spare** stick plays on CDJ-3000, NXS2 and a OneLibrary player.

### M4: Full export / sync (3–6 weeks) → "my sticks are managed by musicbox"
1. Incremental sync: adds, updates, deletes, orphan cleanup, stable IDs, `playlists3*.sync` semantics, and DeviceLibBackup bookkeeping.
2. Player data must survive: history tables, `ImportedHistory.xml`, MySettings.
3. Handle device ownership (DBID), multiple sticks with different libraries, SD cards, exFAT/HFS+, `/.PIONEER`.
4. FAT32 rules: illegal characters, 4 GB limit, case collisions, NFD/NFC, no xattrs. Add a MySetting editor.

**Verify:**
- Repeated syncs on the disk image stay lint-clean.
- rekordbox opens a **copy** of the image without "library corrupted".
- User CDJ check.

### M5: Decode spine + player + cue/grid editing (2–3 weeks) → "set hot cues and fix grids in musicbox"
1. `mb-audio` decode spine shared by preview and analysis: mpg123 + `MpegFrameOffset`, AudioToolbox, the rest. Result: PVBR/PVB2 exact and timelines exact.
2. `mb-player` preview: play, scrub, playhead on the waveform.
3. Cue editing (memory, hot cues A–H, loops, colours, comments) → `djmdCue` + PCOB/PCO2.
4. Grid editing (shift, downbeat, BPM, ×2/÷2), mirroring `djplay::BeatGridAdjustment` → PQTZ/PQT2.

**Verify:** the result equals rekordbox doing the same edit (`rbdiff anlz` + `db`).

### M6: Analysis parity (3–5 months total, one scorecard per step)
musicbox never overwrites analysis made by rekordbox until that subsystem's scorecard is green. A musicbox sidecar DB records where each piece of analysis came from.

| Order | Subsystem | Estimate |
|---|---|---|
| 1 | Waveforms: `mb-dsp` ports, BALib Loud/Dots/Zoom → PWAV/PWV2/PWV3, WaveCreator → PWV4–7 + PWVC | 1–2 weeks |
| 2 | Beat/BPM via BALib, Normal + Dynamic, BPM range | 2–4 weeks |
| 3 | High-precision beat: TF via `mb-nn` + port of the downbeat HMM | 1–2 weeks |
| 4 | Loudness/auto-gain: libebur128 port | under 1 week |
| 5 | Key: `KeyJudge2` | 2–6 weeks |
| 6 | Vocal: features + TF graph → PVDI | 1–2 weeks |
| 7 | Phrase: `SongStructureAnalyze` | 3–6 weeks |
| 8 | Optional: auto-cue + `.3EX` embeddings via ORT | 1–2 weeks |

Also in this milestone: a collection-wide analysis queue with progress, cancel and priority, which respects analysis lock.

### M7: Extras (2–4 weeks)
- rekordbox.xml import/export.
- Intelligent playlists: RE the `SmartList` XML, then an evaluator.
- History: browse it, plus import from the device.
- Preferences: musicbox reads `rekordbox3.settings` for shared values (analysis mode, BPM range, key notation, export options).
- App bundle via `cargo-bundle`, ad-hoc signed.

**Total:** about 3–4 months until musicbox replaces rekordbox for library + export (with analysis still copied from rekordbox's output), and about 7–11 months until full analysis parity.

## Acceptance thresholds
| Subsystem | Target |
|---|---|
| ANLZ round-trip | 100% byte-identical |
| DB operations | Rows + USN order exact; SQL trace equivalent |
| Fresh PDB export | Rows exact (byte-exact pages are a stretch goal); lint clean |
| Incremental sync | Lint clean; player data preserved |
| PVBR/PVB2 | Exact |
| Waveforms | Byte-exact on ≥99% of tracks; ±1 elsewhere, with root cause noted |
| Beat | BPM×100 exact on ≥99%; ≥99.5% of beats within 1 ms; downbeat phase ≥99% |
| Key | Exact on ≥99% |
| Phrase | Segmentation and kinds exact on ≥95% to start |
| Gain | Within 0.01 dB |
| Artwork | Same variants and sizes; PSNR > 40 dB |

## Verification commands
- `cargo test --workspace`: unit tests plus golden probes, runs in under 1 minute. Fixture tests are skipped automatically when `fixtures/` is absent.
- `cargo run -p rbdiff -- <db|sqltrace|anlz|pdb|devlib|analysis> …`: the scorecard goes to `docs/scorecards/`.
- `cargo run -p musicbox`: manual UI check against rekordbox screenshots.
- User checks are limited to: the oracle session (M0), rekordbox opening edited sandbox copies (M2, M5), and spare-stick CDJ tests (M3, M4).

## Risks
- **The instrumented copy may fail rekordbox's self-integrity check.** Fallback: static RE + before/after diffs. Last resort: a SIP-off macOS VM on an external SSD.
- **AudioToolbox output can change between macOS versions.** The macOS version goes into every fixture manifest.
- **Disk space (40 GB free).** Budget: Ghidra project in `~/re-work` 5–10 GB, corpus ≤10 GB, sparse FAT32 images 1–2 GB. A VM needs an external SSD.
- **Phrase analysis has zero ground truth** until the M0 oracle session runs with phrase analysis ON.
- **Real-player testing** depends on the user and their hardware.
