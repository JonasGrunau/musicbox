# Progress log

Newest entry first. Each entry records what changed, what is verified, and the next action.

## 2026-10-14 (later) — M6: embeddings done; reference data for analysis modes 1–3; CUE analysis started
| Part | State | Commits |
|---|---|---|
| ONNX Runtime | rekordbox's own `libonnxruntime.1.18.0` via dlopen, version and model MD5s checked, sessions set up as rekordbox's CPU runner (`mb_nn::ort`) | d15e6dd |
| `.3EX` embedding | 16 kHz audio CNN, UMAP, energy, genre, vocal share, MessagePack as rekordbox writes it; 14/14 sandbox-initial files byte-identical; `ApplyAnalysis` writes it (`contentFile` row first, `ContentLink` 0x200000, undo); `TrackAnalysisSettingsEmbedding` (default on; off in the user's preferences). Spec `analysis-embedding.md`, scorecard `embedding.json` | aec9ad4, 4b9eeb9 |
| Reference data, analysis modes 1–3 | made by driving rekordbox on oracle-a: `oracle-c` (mode 3, high precision Normal), `oracle-d` (mode 2, high precision Auto, rekordbox's default high-precision mode), `oracle-e` (mode 1, Normal-mode Dynamic). The ports are still open (`analysis-beats.md` § 9, `analysis-beats-hp.md` § 8) | 4b9eeb9 (manifest) |
| CUE analysis | spec draft `analysis-cue.md`: Manual is rule-based on the beat grid, Auto uses samplecnn + cue-prediction ONNX models and the personal trend. Reference runs (fixtures `oracle-cue/*`) in progress; Manual implementation next | 4b9eeb9 |

Findings from the rekordbox runs:
- **rekordbox crashes in mode 3.** High-precision Normal mode quits rekordbox (no crash report, no log line) on the sampler loops House3/House4/Breaks2/Breaks3, twice in a row. Their DAT is rewritten, their EXT/2EX are not. Mode 2 and mode 1 finish all 169 tracks.
- **Debug CSVs.** In modes 2 and 3, `BeatAnalyzer_2_0::Analyzer::analyzeFocus` appends `interval.csv` and `interval2–5.csv` to `common::PropertyPath()/csv` (`~/Library/Application Support/Pioneer/rekordbox6/csv`). macOS flagged rekordbox for 2 GB of file writes in 4 minutes. Copies are kept in `~/re-work/oracle-modes/`, outside the repo; they hold the Focus intermediate values per track.
- **Right-click while selected.** A synthetic right-click that carries a modifier flag toggles one row out of a select-all. The input helper now clears the flags of mouse events.
- **Screen lock.** UI driving stops while the Mac is locked; runs resume after unlock.

Open (user): the CDJ-3000 test with RBTEST, the My Settings check.

## 2026-10-14 — M6: high precision, phrase and vocal done
| Part | State | Commits |
|---|---|---|
| High-precision beats, Dynamic (mode 4) | rekordbox's own TensorFlow 2.16.1 + models loaded at run time (`mb-nn`, nothing bundled); madmom + bar HMM + BALib v2 Focus; oracle-b PQTZ/PQT2/BPM 154/154; 12 sweeps without beats keep the old grid with BPM 0, as rekordbox. Normal/Auto modes (2/3) not ported. Spec `analysis-beats-hp.md`, scorecard `beats-hp.json` | 998d497, fcdd72b |
| Phrase (PSSI) | stages A–D ported, stage D equal to rekordbox's own code on 161/161; PSSI written by `analyse_track` (ContentLink 0x10000) when the phrase preference is on; oracle-a 159/161, oracle-b (HP) 159/160; misses = 96 kHz probes (decoded-input side) | 998d497, a6ad190 |
| Stored-grid fallback | when HP finds no beats, key and phrase run on the stored grid, as rekordbox does (oracle-b sweeps: key and PSSI right) | a6ad190 |
| Vocal (PVDI) | rekordbox's own vocal network (the plain `vd_param_raw.pb` it ships; nothing decrypted) and the learned feature statistics read MD5-checked from the installed executable at run time; PVDI identical 169/169 on oracle-a and oracle-b, every stage bit-identical to rekordbox's VocalDetector; `trackAnalysisSettingsVocalDetect`, ContentLink 0x100000; the app notes when the pass is skipped | e17624a |
| ONNX: `.3EX` embeddings, auto-cue | see the next entry | – |

Workers' doc/scorecard files are written by the coordinator: a hook stops subagents from writing report files, and it is not bypassed.

## 2026-10-13 — rekordbox acceptance runs, automated (Mac checks 3–6 done)
**Method.** musicbox's own commands (`cargo run -p mb-library --example accept_check -- <options.json> <step>`, the same `Command`s the UI sends) on the sandbox library, then rekordbox started in the background with RBTEST mounted (`open -g`, quit by Apple Event), with before/after snapshots: every master.db table as sorted TSV (`cargo run -p mb-masterdb --example dump`), the ANLZ tree's MD5s, the stick's files, and the stick's export.pdb/exportExt.pdb/exportLibrary.db rows (`pdb_dump`, `mb-devlib --example query`). Reference stick not mounted at any time.

**What rekordbox does on its own** (baseline run, nothing changed): rewrites the three `djmdSampler` rows (new USNs), writes `master.backup.<date>.db`, `automixPlaylist6.xml` and a few settings files; on the stick it rewrites export.pdb/exportExt.pdb with the same rows (header `xid` +3, `state` 5 → 4) and, on quit, refreshes `DeviceLibBackup/rbDevLibBaInfo_<DBID>.json` (its PC-side backup). With `AutoAnalysis` = 1 it opens "Analysis Setting / Die Auto-Analyse startet" at startup while any track has no analysis.

**Results**
| Check | Result |
|---|---|
| 3. `m7_check` smart lists (36 / 30 / 170 tracks) | rows untouched by rekordbox; rekordbox's own evaluation (its XML export, run 7) equals musicbox's track for track. (Window capture failed earlier only because the screen was locked.) |
| 4. Device import (RBTEST `musicbox check`, one track removed first so it is copied anew) | **bug found and fixed (1b594a4):** the imported row kept `ContentLink` 14, and rekordbox opened its "analysed with an old version" alert (`MissingWaveDataAlertWindow`). `modAnalyzeStatus` also writes `ContentLink`; now 0xC060E. After the fix: no alert, rows untouched. The device row stamp survives rekordbox's rewrite. |
| 5. XML import (oracle-a `oracle-small`, tracks already in the collection) | rows untouched. |
| 6. Analysis of the unanalysed `m2-import-test.wav` | no auto-analysis prompt any more; rows and ANLZ untouched. |
| musicbox export of the three smart lists to RBTEST (9 tracks copied, 4 unsupported probes skipped like rekordbox) | `rbdiff device` 0 mismatches, lint clean; after rekordbox mounted it: export.pdb, exportExt.pdb and exportLibrary.db rows identical (1137 PDB rows, 1080 devlib rows). |

**Smart lists and XML against rekordbox's own output (run 7):** rekordbox's menu "Export Collection in xml format" (driven via the menu bar, file saved from its panel) lists the three `mb …` intelligent playlists with its own evaluation: identical to musicbox's, track for track (171, 36, 31). musicbox's XML export of the same sandbox equals rekordbox's byte for byte apart from `PRODUCT`, after one fix: rekordbox.xml hot-cue RGB comes from the UI palette (`djplay::getPadColor`), not the device one (`docs/re/xml.md`); the app's cue colours now use it too.

**First-analysis artwork (7d8724d):** musicbox's analysis now extracts the embedded picture like rekordbox (`ImagePath`, `artwork.jpg`/`_s`/`_m`, the `contentFile` row; `analyse-track.md` § Artwork). Checked in rekordbox: an MP3 imported and analysed by musicbox (`accept_check import` + `analyse`) starts without any prompt, and rekordbox leaves its row, ANLZ and artwork files untouched.

**Also fixed (1b594a4):** 1107 AppleDouble `._` files on RBTEST. macOS tags files a provenance-tracked process creates with `com.apple.provenance` (not removable), and FAT stores it in `._<name>`. Every device write now deletes the companions of what it wrote (`mb_usb::appledouble`); RBTEST was cleaned (339 old ones under `Contents/`).

**rekordbox quirk:** an Apple-Event quit during startup ran `backupDeviceLibrary` and deadlocked in JUCE's URL session (`SharedSession::removeTask`); rekordbox had to be killed. Quit only after the main window is up.

**M6 in progress (workers):** phrase stage D (stages A–C now match rekordbox bit for bit on 161/161; IntroEndJudge ported, 161/161); TF: `mb-nn` loads rekordbox's TF 2.16.1 (activations bit-identical), madmom beat lists 166/166 on oracle-b, which was made in mode 4 (high precision + `trackHiPrecisionBeatAnalysisMode` 1 = Dynamic).

**User checks left:** studio CDJ test of RBTEST (now 166 tracks, lists `musicbox check`, `oracle-all`, three `mb …` smart lists; no `._` files) and the My Settings check.

## 2026-10-12 (later) — status board: M4 done, M5/M7 built, M6 waveforms + loudness
**Milestones**
| | State | Commits |
|---|---|---|
| M3 | export done; **CDJ check pending** (studio) | 1faf385 |
| M4 | done: deletes (both DBs + files + sync files), old-stick conversion, `playlists3*.sync`, foreign My Tags kept, row paths stay taken, ≥ 4 GiB on FAT32 planned as failed copy; export onto a reference-stick clone keeps every row and file ; My Settings editor (466caa5) | 5abe8e9, 22d7451, d846f4a, eb85e3d, 1729250, 466caa5 |
| M5 | built from the binary (no trace has a cue write): cue/grid ops with undo, deck UI; device cues built from `djmdCue` (410/410 reference-stick cue sets byte-exact). rekordbox check passed; grid edit byte-exact vs rekordbox | 0d0d509, b3e4840 |
| M6 | waveforms 8/8 tags byte-exact on 169/169 (oracle-b); auto-gain bit-exact 169/169 (AAC fixed: the decode depended on the stack alignment at `AudioConverterFillComplexBuffer`, `decode.md` § 6); Normal-mode beat grid: BPM 161/161, grids identical 160/161 with the phrase pass's downbeat shift; phrase analysis **paused after stage C / partial D** (no PSSI yet); key 169/169 (oracle-a, commit b9d0a53; was uncommitted) | 617fc5f, 8def9c4, 9e29e7d |
| M7 | smart lists (rules, editor, XML, USB export); import from device (tracks, playlists, histories); app bundle (`tools/bundle/bundle.sh`); key display follows `ShowOriginalKey`/`KeyStringSetting`. rekordbox.xml export (byte-exact vs oracle-a) and import. **rekordbox checks pending** (`m7_check`, device import, XML) | 0d0d509, 88593cc, d0babf3, e93d3a4, 3997b37, 53af01b, 35c036c |

**M6 key (done; awaiting commit):**
- **Result:** the key rekordbox stored, from both rekordbox's grid and our own pass:
  - oracle-a: 169/169 tracks.
  - oracle-b: 169/169 from its own grid, 160/169 from ours. The 9 misses come only from oracle-b's high-precision grids; for all 9, our Normal-mode key equals oracle-a's.
- **Scope:** the key buffer collected during the decode pass, `CutBeats`, `PitchDetect`, `KeyCodeScale` (constant-Q profile, candidate rules, 30 s window votes) and `Key_judge_with_keys` over the beat segments.
- **Where:**
  - Code: `crates/mb-analysis/src/key/`; `Options.key` (on by default) and `Analysis.key` (0–11 major, 12–23 minor; `key::scale_name`).
  - Spec: `docs/re/analysis-key.md`. Scorecard: `docs/scorecards/key.json`.
- **Validation:** every stage was also compared with rekordbox's own routines, run natively in a harness outside the repo. Segment pairs 6339/6339, window choices 1018/1018, judgement 400k/400k random inputs.
- **Commands:**
  - `cargo run --release -p mb-analysis --example key_score` (`MB_ORACLE=oracle-b` for the second fixture)
  - `cargo test --release -p mb-analysis --test key` (`MB_ORACLE_FULL=1` for all tracks)
- **Library and app:**
  - `analyse_track` returns the key's `ScaleName`; `ApplyAnalysis` replays the traced key block: the `djmdKey` row by name (inserted when missing), `KeyID`, orphaned key rows removed, `TrackInfoUpdated`. Undo restores the old key (re-creating a removed tag key by name).
  - `AnalysisSettings.key` comes from the `enableKeyAnalysis` preference (on when missing, *hypothesis*).
  - `tests/analyse_ops.rs` now checks the key on every oracle track (161/161 with `MB_ORACLE_FULL=1`, phrase on as rekordbox had it) and the key block's rows, counters and undo.
- **Open questions:** spec § 10.

**M6 phrase (paused after stage C / partial D; awaiting commit):**
- **Spec:** `docs/re/analysis-phrase.md`: stages A–D, the grid-shift hook, a function map with port status (145 functions), the remaining list, and handoff notes (§ 9).
- **Stages A/B and the grid shift are done and on by default** (`Options.phrase`). Against oracle-a: grids identical 160/161 (was 140), bars 161/161, beats within 1 ms 99.87 %. Scorecard: `docs/scorecards/beats.json`.
  - The one miss is `wav_96k32f/click_128_offset1234`: we move it, rekordbox did not (open question, `analysis-phrase.md` § 6).
- **Stage C** (per-beat features in ~35 s blocks) is ported but unverified. It runs only through `phrase::stage_c` (example `phrase_debug`); `analyze` does not call it.
- **Stage D** is about 30 % ported (feature post-processing and the genre decision), but not wired in.
  - Remaining: 42 functions, about 101 KB of code. Largest: `IntroEndJudge` (25.8 KB), `Sub_Main_D` (8.7 KB), `SongStructureJudge` (5.4 KB).
  - PSSI is not produced; `analyse_track` keeps an existing PSSI as before.
- **Open:** `analyse_track` now always shifts the grid. rekordbox does this only with phrase analysis on (thread flag +0x3908, setter not found). Check whether a preference should gate it.
- **Tools** for continuing are in `~/re-work/phrase/` (outside the repo).

**M7 rekordbox.xml (done and committed in 35c036c; user check pending):**
- **Export** (`mb_library::xml::export_document`, app **File → Export Collection in xml format…**): rebuilds rekordbox's own oracle-a export byte for byte (161 tracks). POSITION_MARKs come from `djmdCue` via the USB export's `checked_cues` with the pad-colour tables. Preferences used: `language`, `HotCueColorType`, `bridgeExportBeatGrid`. musicbox names itself in `PRODUCT`; rekordbox's import ignores that node.
- **Import** (`Command::ImportXml`, spec `docs/re/ops/import-xml.md`, binary-derived):
  - "rekordbox xml" in the tree: the bridge file when `showRbXml` = 1, or **File → Open rekordbox xml…**. "Import to Collection" on lists, folders and rows.
  - New tracks are inserted in place with the XML's values; album artist, original artist, lyricist, ISRC and release date come from the file's tags.
  - TEMPO → a local `.DAT` grid and `Analysed` 0x10; POSITION_MARK → cues with the nearest pad colour.
  - Lists are created in XML order. rekordbox's two questions (update tracks already in the collection; replace lists with the same name) are a dialog.
  - Undo is one batch.
  - Rows for the same files equal rekordbox's own rows. Grids are within 2 ms: the TEMPO format's own loss, which rekordbox's round trip has too.
- **Tests:**
  - `cargo test -p mb-xml`
  - `cargo test -p mb-library --test xml_ops`
  - `cargo test -p musicbox --test ui_xml`

**M6 analysis job + queue (done, not committed by the worker; user check pending):**
- **Spec:** `docs/re/ops/analyse-track.md` § Files, § Status bits, § musicbox.
  - Tag order per file from `write_pmai*`; EXT/2EX sections only when their data exists.
  - Analysis lock = `Analysed` 0x80.
  - `ContentLink` 0x10000 phrase, 0x100000 vocal, 0x200000 embeddings, 0xC0600 on completion.
- **`mb_library::analyse_track` (pure).** Builds `.DAT`/`.EXT`/`.2EX` plus the columns and the gain row.
  - On all 161 rekordbox-analysed oracle-a tracks it equals rekordbox's files section by section, except PSSI/PVDI (not ported yet) and the 21 grids the phrase pass moves.
  - Columns and mixer words are equal on 161/161.
  - New: `mb_audio::flac_seek` (PVB2, 23/23 byte-exact).
- **`Command::ApplyAnalysis` / `RestoreAnalysis`.** The traced statement sequence plus `contentFile` rows, with undo. Locked tracks are refused; PSSI/PVDI of an earlier rekordbox analysis are kept.
- **App:**
  - "Analyze Track" in the table's context menu;
  - a worker-thread queue (`analysis_jobs.rs`) where the selected track goes first;
  - a status item with Cancel;
  - with the `AutoAnalysis` preference on, tracks without any analysis are queued once (e.g. after an import).
- **Tests:**
  - `cargo test -p mb-library --test analyse_ops` (`MB_ORACLE_FULL=1 cargo test --release …` for all 161);
  - `cargo test -p mb-audio --test oracle pvb2`;
  - `cargo test -p musicbox --test ui_analysis`.
- **Not done:** `.3EX` (artwork extraction, the key block and the lock toggle are done since).

**User checks waiting** (all need rekordbox quit first; sandbox library only):
1. Studio: RBTEST on the CDJ-3000 (§ M3 entry below), then the My Settings check (musicbox "My Settings…" on RBTEST: LANGUAGE German, JOG DISPLAY MODE ARTWORK, TEMPO RANGE WIDE, Waveform color 3Band; the CDJ shows them after loading the stick's My Settings).
2. ~~Mac: `m5_check`~~ **passed 2026-10-12** (cues, hot cues, loop, comment, grid shift, ×2 all shown; the doubled BPM is 2 × the first beat's tempo, which rekordbox's own grid edit also does). A rekordbox grid edit was captured as `fixtures/oracle-grid-edit`; musicbox reproduces it byte for byte.
3–6: **done by automated runs on 2026-10-13** (entry above).
3. Mac: same with `--example m7_check`: three "mb …" intelligent playlists with the printed counts and rules.
4. Mac: device import from RBTEST (musicbox "Import to Collection" on `oracle-small`, "Import Histories" after the CDJ wrote histories), then rekordbox shows the tracks with waveforms/cues and the histories; a rekordbox export of `oracle-small` to RBTEST copies nothing again.
5. Mac: XML. musicbox "Export Collection in xml format…" → rekordbox imports a playlist from it (metadata, grid, cues). Then musicbox "Open rekordbox xml…" on a rekordbox-written XML → "Import to Collection" → rekordbox shows the list, grids at once, cue colours right.
6. Mac: analysis. In musicbox (editing on) import an unanalysed audio file, or right-click a track → "Analyze Track"; wait for "Analysed 1 track"; quit musicbox, open rekordbox: the track shows waveform, beat grid and BPM straight away without re-analysing, and no repair prompt.

**M4 My Settings editor (done and committed in 466caa5; CDJ check pending):** `docs/re/mysetting.md` maps every MYSETTING/MYSETTING2/DJMMYSETTING/DEVSETTING setting (label, byte, values, menu order, unset default; DJM paired settings 3/13, 7/15, 16/17) from the binary; consistent with the reference stick (CDJ-3000-written MYSETTING2/DEVSETTING), the oracle stick and the local files. `mb_usb::mysettings` reads/edits/writes them (only the edited bytes change, player headers and unnamed bytes kept, CRC recomputed, unreadable files left alone, reference stick and running rekordbox refused). App: device header context menu **My Settings…** (PLAYER / MIXER / DJ SYSTEM combos; protected devices read-only). Tests: `cargo test -p mb-usb --lib mysettings`, `cargo test -p mb-usb --test mysettings`, `cargo test -p musicbox --test ui_mysettings`.
- CDJ check (studio, with RBTEST): in musicbox right-click RBTEST → My Settings… → set LANGUAGE German, JOG DISPLAY MODE ARTWORK, TEMPO RANGE WIDE, DJ SYSTEM Waveform color 3Band → OK; plug RBTEST into the CDJ-3000 and load the stick's My Settings (rekordbox-style "My Settings" load on the player): the menus show German, the jog display shows artwork, tempo range WIDE, waveforms in 3Band.

**Open, not assigned**
- ~~Export log language~~ done: the texts go through the user's rekordbox language file (options.json `lang-path`), as `juce::translate` does; the German file gives exactly the oracle log's texts.
- ~~AAC decode parity~~ done (7ec372e): Apple's AAC decoder gives ±1 in a few samples when `AudioConverterFillComplexBuffer` is entered at SP ≡ 48 (mod 64). `mb-audio` now enters it through a trampoline at a fixed alignment matching rekordbox. Auto-gain 169/169 and all waveform tags 19/19 AAC, both oracles, stable under concurrent runs. Test: `cargo test -p mb-audio --test aac_stack`.
- M6 left: phrase stages C/D and PSSI (paused, `analysis-phrase.md` § 9), vocal/PVDI (TF), Dynamic and high-precision beats (TF + downbeat HMM; the user's rekordbox has high precision on, so musicbox's own grids differ from what that rekordbox would write), auto-cue/embeddings. Key is done (b9d0a53) (ORT); an analysis queue in the app.

**Housekeeping:** the disk is tight (≈ 20 GB free after deleting `target/debug`). Workers share one target dir; testing a staged snapshot in a scratch copy doubles the build artifacts, so prefer testing in the repo when the tree is coherent.

## 2026-10-12 — M3 export on the spare stick; CDJ check pending
- **Spare stick RBTEST** (volume UUID 0BAB78DC-CFAA-390B-9A88-30FC468A7B99 after the re-erase; it may show up as disk6, so always check the UUID) was wiped and filled by musicbox alone: `cargo run --release -p mb-usb --example export -- live /Volumes/RBTEST "musicbox check" "oracle-all"`.
  - 162 of 166 steps exported (157 unique tracks). The 4 skipped tracks are the 22.05 kHz probes, which the format gate refuses, as rekordbox does.
  - `rbdiff device`: 157 tracks, 2 playlists, 0 mismatches. pdb-lint: 54 pages, 0 errors.
  - macOS added 1448 AppleDouble `._` files (rekordbox's own exports have them too).
  - Export log: `~/Library/Application Support/musicbox/export-logs/Export Log rekordbox 2026-10-12.txt`.
- **UI:** export ("Export to ▸" with progress and Cancel) and preview playback are wired and tested headlessly.
- **Pending user check (CDJ-3000 + DJM-900NXS2, in the studio):**
  1. both playlists browse with the right counts;
  2. tracks load with title, artist, BPM, key and artwork;
  3. waveforms, beat grid and cues show;
  4. `click_128_offset1234` shows 128.00 BPM.
- **M4 deletes (done and committed):** "Remove from device…" for playlists, folders (tree) and tracks (table), with "Also delete tracks no other playlist uses". `mb-usb::remove` removes from export.pdb, exportExt.pdb and exportLibrary.db in lock-step using rekordbox's per-DB row rules (`docs/re/export.md` § 4.3, from `removeTrack` / `deleteList` and the DeviceSQL procedures), clears the lists' `Dev_ID`s in `playlists3*.sync`, then deletes the files no remaining row uses. Tested on a synthetic stick (`crates/mb-usb/tests/remove.rs`, `apps/musicbox/tests/ui_device_remove.rs`) and on an APFS clone of the reference stick's `PIONEER` folder (6 lists + 608 orphan tracks removed; tag lists, histories, My Tags and every other row intact; lint clean).
- **M4 conversion (done and committed):** a stick with export.pdb but no exportLibrary.db is now converted to Device Library Plus before an export, instead of being refused (`crates/mb-usb/src/convert.rs`; `DevLibWriter::create_for_conversion` / `insert_all`). The 22-table mapping of `execDbConversion` and the `a` → `b` artwork copy come from the binary (`docs/re/export.md` § 6.4). On a copy of the reference stick's classic DBs, 14 tables come out identical, every converted content/artist/album/playlist row matches except fields that later exports rewrote, and all 1404 `b` files are byte-identical (`crates/mb-usb/tests/convert.rs`). Converting over an existing exportLibrary.db is not implemented.
- **M5 cue/grid editing (done and committed; user check pending):**
  - Specs from the binary: `docs/re/ops/edit-cues.md`, `docs/re/ops/edit-grid.md`. Collection cues live only in `djmdCue` (no USN) plus the `contentCue` JSON mirror (one USN per write) and `CueUpdated`. rekordbox never writes them into the local ANLZ files; its USB export builds PCOB/PCO2 from the database rows, and `mb-usb` now does the same (next bullet).
  - **Cue export fixed (`mb-usb`, committed):** the device PCOB/PCO2 lists are built from `djmdCue` (`crates/mb-usb/src/cues.rs`, `docs/re/export.md` § 1.6 "Cues"), for new tracks and re-exports, and replace the lists carried over from the local files. Lists are in reverse row order: A–C in the `.DAT`, D onwards in the `.EXT` PCOB, all hot cues in PCO2. Hot-cue colours come from the 64-entry pad palette, read from the binary; cues without a colour code get the `HotCueColorType` default.
    - `tests/oracle.rs` is still byte-exact.
    - 410/410 reference-stick cue sets written by 7.x are rebuilt byte for byte.
    - Cues added through `mb-library` reach an exported stick, and a re-exported track gets its new cue.
    - An unchanged playlist is skipped on re-export, as in rekordbox, so cue edits reach a stick only when the track itself is exported again.
  - Grid saves rewrite the local `.DAT` PQTZ, empty the `.EXT` PQT2, and update `BPM` (when the first beat's tempo changes) and `AnalysisUpdated`.
  - `mb-library`: `Command::AddCue` / `DeleteCue` / `EditCue` / `SetBeatGrid`, with undo, plus `grid::{shift, set_downbeat, set_tempo, double, halve}`. Analysis files are backed up into the session backup before their first change. Tests: `crates/mb-library/tests/cue_grid_ops.rs` (oracle-a copies; undo restores the files byte for byte).
  - UI: a detail strip with the beat grid, memory/hot-cue (A–H) controls, colour, comment, delete, and grid buttons on the deck; the deck keeps its track across reloads. Test: `apps/musicbox/tests/ui_cues.rs`.
  - User check: `cargo run -p mb-library --example m5_check -- <options.json>` on the sandbox, then open rekordbox; the example prints what to look for.
- **Import from device (done and committed; user check pending):** "Import to Collection" on device playlists (tree) and device tracks (table), "Import Histories" on a device's Histories header (`docs/re/ops/import-device.md`, from the binary). New tracks are copied to `~/Music/PioneerDJ/Imported from Device/<artist>/<album>/<file>` (export naming), inserted from the device row (`MasterDBID`/`MasterSongID` = this library), their device analysis becomes the local set (`mb_anlz::local::to_local`: PPTH + cues cleared; byte-exact vs oracle-a on 133/133 `.DAT`, `.EXT` equal up to the scrambled PSSI rekordbox keeps), cues go to `djmdCue`, artwork to the local set; tracks already in the collection (by path or by master IDs) are reused; on writable sticks the device rows get the new master IDs (`mb_usb::stamp`). Histories map device tracks to collection tracks only, create `YYYY`/`M` folders and `HISTORY YYYY-MM-DD[ (n)]` lists, raise `DJPlayCount`, and dedup through the stick's `ImportedHistory.xml`. Undo for both. Tests: `crates/mb-library/tests/device_import.rs`, `apps/musicbox/tests/ui_device_import.rs`. Not done: My Tags/likes, exportLibrary.db histories, deleting histories after import.
- **M7 smart lists (done and committed; user check pending):** intelligent playlists from the binary (`docs/re/smartlist.md`; no fixture has one). `mb_core::smartlist` parses and writes the `SmartList` XML byte-compatibly with JUCE's writer, and evaluates the 23 properties and 11 operators like `db::operate`: collection order, primary-strength text matching (approximated), the "in the last N days/months" quirks. Smart lists are evaluated in the browser and in the rekordbox.xml export. `mb-library`: `Command::CreateSmartList` / `SetSmartList` with undo; smart lists can now be renamed, moved and deleted. UI: "New Intelligent Playlist" and "Edit Intelligent Playlist…" with a rule editor. Tests: `mb-core` unit tests (every property and operator), `crates/mb-library/tests/smartlist_ops.rs`, `apps/musicbox/tests/ui_smartlist.rs`. User check: `cargo run -p mb-library --example m7_check -- <options.json>` on the sandbox, then rekordbox (the example prints what to look for). Since then, smart lists also export to USB/SD. The tracks are evaluated when the export starts and written as a plain device playlist; an unchanged result is skipped and a changed one refilled (`docs/re/export.md` § 1.7). "Export to ▸" works on smart lists. Tests: `crates/mb-usb/tests/smartlist.rs`, `apps/musicbox/tests/ui_smartlist.rs` (`export_smart_list_to_device`).
- **M6 waveforms (done and committed):** new crates `mb-dsp` (rekordbox's biquads and BALib filter designs, fused operations, libm `__sincos_stret`) and `mb-analysis` (`analyze_waveforms(path)` → PWAV, PWV2, PWV3, PWV4, PWV5, PWV6, PWV7, PWVC). Spec: `docs/re/analysis-waveforms.md`.
  - Against oracle-b, all eight tags are byte-exact on 169/169 tracks. Against oracle-a, 4 sampler loops differ in PWAV colour; they hold an older analysis.
  - In some earlier runs, 4–10 AAC tracks were ±1 off because AudioToolbox returned a different decode (§ 6 there).
  - Scorecard: `docs/scorecards/waveforms.json`. Tests: `cargo test -p mb-analysis`.
  - mb-audio fix: ALAC packets larger than their PCM ("escape" packets) failed to read, and the rest of the track decoded as silence (`decode.md` § 6).
- **M6 loudness / auto-gain (done and committed):** `mb_analysis::analyze(path, Options { loudness: true, .. })` → `Loudness { lufs, gain_db, gain, peak }`. These are the `djmdMixerParam` floats. Spec: `docs/re/analysis-loudness.md`; scorecard: `docs/scorecards/loudness.json`.
  - How it is computed: rekordbox's statically linked libebur128, ported operation by operation. The gain is `(float)exp10((−10 − LUFS)/20)`. Only every `min(seconds/30 + 1, 16)`-th 2-second block is measured.
  - The gain float is bit-exact on all 150 non-AAC tracks and on 12 of 19 AAC tracks; the other 7 AAC tracks are one ulp off. The sample peak is bit-exact on 169/169. Results are identical against oracle-a and oracle-b.
  - rekordbox's own AAC values are identical in both sessions, so our AudioToolbox decode differs from its decode slightly. Next for `mb-audio`.
- **M6 beat grid, Normal mode (done; awaiting commit):** `mb_analysis::analyze(path, Options { beats: true, .. })` → `BeatGrid { bpm, beats }` (`djmdContent.BPM` and the PQTZ/PQT2 beats with µs remainders). It is a port of BALib v1's constant-tempo Analyzer (`BeatAnalyzer_1_0`) plus `legacyBeatAnalyze`/`convertToMicrosec`. Spec: `docs/re/analysis-beats.md`; scorecard: `docs/scorecards/beats.json`; code: `crates/mb-analysis/src/beats/`.
  - **Against oracle-a** (161 tracks rekordbox analysed): BPM × 100 exact 161/161 and beat count 161/161. PQTZ and PQT2 are identical on 140/161, every beat to the ms; 92502/94897 beats match exactly and none is off by 1 ms.
  - **The other 21** (`click_128_offset1234` ×7, `sweep_20_20k` ×9, three more probes, two real tracks) have the same tempo, spacing, count and PQT2 µs values. After BALib, rekordbox's **phrase pass** (`SongStructureAnalyze` in `CBeatAnalyzeThread::analyze`) moved the whole grid to its own first downbeat, or renumbered the bars. Hook point and data: spec § 8. That pass needs the phrase/PSSI analyser.
  - PQT2's per-beat u16 is now verified as the µs remainder (`anlz.md`).
  - **Tests:**
    - `cargo test --release -p mb-analysis --test oracle beats_match_rekordbox`: probes; `MB_ORACLE_FULL=1` for all 161; phrase-moved tracks must be uniform shifts.
    - `--test synthetic click_track_grid`: synthetic 128 BPM click after 1.234 s.
    - Scorer: `cargo run --release -p mb-analysis --example beat_score -- [--verbose] [filter]`.
  - **Not covered:** Dynamic mode (varied analyzer) and high precision (BALib v2 + madmom TF + downbeat HMM); spec § 9.

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
- **Protected volumes:** `apps/musicbox/src/protect.rs` + `~/Library/Application Support/musicbox/protected-volumes.txt`. The reference stick UUID is always blocked. Every device write path must call `protect::write_refusal(root)` first.
- **Running background jobs (2026-10-12 ~01:15), results land as notifications:**
  1. `crates/mb-usb`: export planner + executor; target is reproducing `fixtures/oracle-a/rbtest` (paths, audio, ANLZ, artwork, pdb, devlib, settings).
  2. `crates/mb-audio` + `crates/mb-player` + `docs/re/decode.md`: decode spine (dlopen rekordbox's libmpg123, AudioToolbox for AAC) with a per-format offset table, plus cpal preview player.
- **On their completion:** review, run tests, commit and push to `main`; update INDEX/progress.
- **Then wire into the UI:**
  - Export: drag a playlist onto a device, or a context menu. Check `protect::write_refusal` first.
  - Preview: play/scrub in the deck.
  - Cover all of it with egui_kittest tests (no OS input).
- **Then:** spare-stick (RBTEST) CDJ test with the user (M3 check); M4 sync; M5 cue/grid editing; M6 analysis.
- **Session rules:**
  - Commit and push straight to `main`, no branches.
  - Never synthetic OS input or region screenshots.
  - Reference stick stays unplugged or read-only.
  - Never run rekordbox while musicbox tests the sandbox.
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
  - Spare stick: SanDisk 3.2Gen1 123 GB, FAT32/MBR, named `RBTEST`. Volume UUID was ADF6A64C-EF6E-34EB-B42B-4BC44934392E; after the 2026-10-12 re-erase for the M3 CDJ test it is 0BAB78DC-CFAA-390B-9A88-30FC468A7B99. Reference stick volume UUID: 2335E32F-F224-3E76-8EE8-42C79B582A15.

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
