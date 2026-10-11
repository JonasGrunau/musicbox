# RE index

Status values: **verified** (checked against real files or a trace), **hypothesis** (supported by evidence but not yet proven), **open** (not investigated yet).

| Spec | Subsystem | Status |
|---|---|---|
| [masterdb.md](masterdb.md) | master.db location, key, schema, write rules | key + read verified; writes hypothesis |
| [ops/INDEX.md](ops/INDEX.md) | exact rekordbox statement sequences per library operation (from SQL trace) | verified for startup, import, remove, playlist create/add, analysis, exports, shutdown; agent-side writes reconstructed |
| [masterdb/schema.sql](masterdb/schema.sql) | master.db DDL (48 tables) | verified |
| [anlz.md](anlz.md) | ANLZ PMAI container and tags | container + all typed tags verified (3,132 files incl. PSSI/PVDI); USB folder hash verified (704/704); local→USB conversion `to_usb` byte-exact on 471 oracle files |
| [pdb.md](pdb.md) | export.pdb / exportExt.pdb (DeviceSQL) | reader verified; writer (DeviceSQL storage model) reproduces RBTEST export.pdb 78/78 and exportExt.pdb 18/18 pages byte-exact via replay; pdb-lint clean |
| [devlib.md](devlib.md) | exportLibrary.db (OneLibrary) | key verified (BlowFishHelper decode, also yields master.db key); read + schema verified; write rules hypothesis |
| [export.md](export.md) | USB export flow, paths, IDs, artwork, sync, settings files | verified from binary + stick (writer not yet built); open questions listed |
| [xml.md](xml.md) | rekordbox.xml import/export format, TEMPO/POSITION_MARK derivation | reader + byte-exact writer verified on oracle-a (161/161 tracks); cues/colours untested |
| analysis.md | analysis pipeline, BALib, WaveCreator, key, phrase, loudness, vocal | open |
| decode.md | per-format decoding and resampling | open |

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
