# RE index

Status values: **verified** (checked against real files or a trace), **hypothesis** (supported by evidence but not yet proven), **open** (not investigated yet).

| Spec | Subsystem | Status |
|---|---|---|
| [masterdb.md](masterdb.md) | master.db location, key, schema, write rules | key + read verified; writes hypothesis |
| [ops/INDEX.md](ops/INDEX.md) | exact rekordbox statement sequences per library operation (from SQL trace) | verified for startup, import, remove, playlist create/add, analysis, exports, shutdown; agent-side writes reconstructed; cue and grid edits binary-derived (`ops/edit-cues.md`, `ops/edit-grid.md`), user check pending; import from a device binary-derived (`ops/import-device.md`; device→local ANLZ verified 133/133) |
| [masterdb/schema.sql](masterdb/schema.sql) | master.db DDL (48 tables) | verified |
| [anlz.md](anlz.md) | ANLZ PMAI container and tags | container + all typed tags verified (3,132 files incl. PSSI/PVDI); USB folder hash verified (704/704); local→USB conversion `to_usb` byte-exact on 471 oracle files |
| [pdb.md](pdb.md) | export.pdb / exportExt.pdb (DeviceSQL) | reader verified; writer (DeviceSQL storage model) reproduces RBTEST export.pdb 78/78 and exportExt.pdb 18/18 pages byte-exact via replay; pdb-lint clean |
| [devlib.md](devlib.md) | exportLibrary.db (OneLibrary) | key verified (BlowFishHelper decode, also yields master.db key); read + schema verified; write rules hypothesis |
| [export.md](export.md) | USB export flow, paths, IDs, artwork, sync, settings files, export log | verified; `mb-usb` reproduces the RBTEST stick file-for-file (pdb byte-exact, devlib row-exact, audio/ANLZ/artwork/settings/log byte-exact); deletes of lists and tracks implemented in both DBs (§ 4.3 from the binary, § 8; tested on a clone of the reference stick); Device Library Plus conversion of an export.pdb-only stick implemented (§ 6.4: all 22 tables from the binary; on the reference stick every converted row matches except fields later exports rewrote, all 1404 `b` artwork files byte-identical); device cue lists built from `djmdCue` (§ 1.6 "Cues": 410/410 7.x-written stick cue sets rebuilt byte for byte) |
| [mysetting.md](mysetting.md) | My Settings field map: MYSETTING, MYSETTING2, DJMMYSETTING, DEVSETTING (names, bytes, values, menus) | verified from the binary and consistent with all fixture and local files; player behaviour untested on hardware |
| [xml.md](xml.md) | rekordbox.xml import/export format, TEMPO/POSITION_MARK derivation | reader + byte-exact writer verified on oracle-a (161/161 tracks); `mb_library::xml::export_document` rebuilds rekordbox's file byte for byte; POSITION_MARK/colour rules from the binary (no rekordbox-written cue fixture); import in `ops/import-xml.md` |
| [smartlist.md](smartlist.md) | intelligent playlists: `SmartList` XML, properties/operators, evaluation, write path | binary-derived (format, evaluator, serialiser, writes); no trace or rekordbox-written sample yet |
| [analysis-waveforms.md](analysis-waveforms.md) | waveform tags PWAV, PWV2, PWV3 (BALib) and PWV4–PWV7, PWVC (WaveCreator) | verified: all 8 tags byte-exact on 169/169 tracks vs oracle-b, AAC included and stable (stack-alignment fix in `mb-audio`, `decode.md` § 6); oracle-a differs only on 4 sampler loops it analysed with an older rekordbox |
| [analysis-loudness.md](analysis-loudness.md) | auto-gain: libebur128 integrated loudness and sample peak → `djmdMixerParam` | verified: gain float and peak bit-exact on 169/169 tracks (both oracles) |
| [analysis-beats.md](analysis-beats.md) | beat grid + BPM, Normal mode: BALib v1 (BeatSensor, BeatInfoAnalyzer, UnitEnvelopeMaker, UnitBeatAdjust) → `djmdContent.BPM`, PQTZ, PQT2 | verified vs oracle-a: BPM × 100 161/161, beat count 161/161, PQTZ+PQT2 identical 160/161 with the phrase pass's downbeat shift (`analysis-phrase.md`); the one miss is the 96 kHz click probe, which we move and rekordbox does not |
| [analysis-phrase.md](analysis-phrase.md) | phrase analysis (`SongStructureAnalyze`): stages A/B (downbeat and bar → grid shift), C (per-beat features in ~35 s blocks), D (genre, mood, phrases → PSSI) | verified for the grid shift (160/161 grids identical vs oracle-a); stage C ported, unverified; stage D about 30 % ported, not wired; PSSI not produced. Paused after stage C / partial D; handoff in § 9 |
| [analysis-key.md](analysis-key.md) | key: BALib v1 key buffer (`resample2`), `CutBeats`, `PitchDetect`, `KeyCodeScale` (72-tone constant-Q profile, candidates, rules, 30 s window votes), `Key_judge_with_keys` over the segments → `djmdContent.KeyID` | verified: 169/169 vs oracle-a from rekordbox's grid and from our own pass; oracle-b 169/169 from its grid, 160/169 from ours (the 9 differ only by oracle-b's high-precision grids) |
| analysis.md | analysis pipeline beyond waveforms, loudness, Normal-mode beats, phrase and key: vocal, high-precision/Dynamic beats | open |
| [decode.md](decode.md) | per-format decoding, analysis rate, 16-bit analysis input, resampler, PVBR | verified: lengths 169/169, PVBR 37/37 byte-exact, PVB2 23/23, per-format onset columns match rekordbox; MP3 and SRC sample identity confirmed through waveform parity; AAC decode varies between runs (analysis-waveforms.md § 6) |

## Binary landmarks (rekordbox 7.2.19, arm64 slice)
These come from the 2026-10-11 survey; confirm each one in Ghidra before relying on it.

**Analysis**
- `analyzer::CBeatAnalyzeThread::analyze` @0x1001528c4: the analysis driver.
- `db::DatabaseMediator::execAnalyze` @0x1014900ec and `analyze_complete` @0x101494740.
- `analyzer::CMusicAnalyzeFile::write_pmai` @0x100f5ab40, `write_pmai_ext` @0x1013d0230, `write_pmai_ex2` @0x1007869f0.

**USB export**
- `db::DatabaseMediator::execExport` @0x1009b543c.
- `analyzer::ExportAnalysisData` @0x1007b7c0c and `CreateAnlzFileFolderPath` @0x1007b2e18.
- `db::DevSqliteDBController` (exportLibrary.db) ≈0x1006f735c.

**Database and audio**
- `SQLiteDB::open` @0x1010fc674 (master.db).
- `rbxfrm::AudioReaderFactory` @0x10284dc6c.
