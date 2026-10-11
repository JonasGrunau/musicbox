# master.db

Status: **key verified**, **read verified**, write rules **verified** for the traced operations (2026-10-11 SQL trace; per-operation specs in `ops/INDEX.md`). Untraced writers (rekordboxAgent) and unexercised operations remain open.

## Location
rekordboxAgent's `~/Library/Application Support/Pioneer/rekordboxAgent/storage/options.json` has the form `{"options": [[key, value], …], "defaults": {…}}`. The relevant keys:

| Key | Meaning |
|---|---|
| `db-path` | absolute path to master.db |
| `analysis-data-root-path` | root of the local ANLZ tree (`<root>/PIONEER/USBANLZ/…`) |
| `settings-root-path` | `~/Library/Application Support/Pioneer/rekordbox6` |
| `dp` | encrypted SQLCipher passphrase (see Key) |

The library can be moved with "Move Master Database" (binary strings "Move Master Database" and `PIONEER/Master/`), so these paths must always be read from options.json and never hard-coded.

## Key
- master.db is SQLCipher 4 with default parameters:
  - 4096-byte pages
  - 80-byte reserve (16-byte IV + 64-byte HMAC-SHA512)
  - KDF: PBKDF2-HMAC-SHA512 with 256,000 iterations over the *passphrase* string
- To get the passphrase:
  1. Base64-decode `dp`. This gives 64 bytes.
  2. Blowfish-decrypt it in ECB mode, big-endian word order, with the key `ZOwUlUZYqe9Rdm6j`.
  3. Trim trailing NUL and whitespace. The plaintext uses no padding scheme.
- **Where the key lives:** the 16-byte Blowfish key is a cstring in the rekordbox 7.2.19 arm64 slice at file offset 0x4FAE8DD (VA ≈ 0x104FAE8DD), between `rekordbox_client` and `https://cloud.kuvo.com`. It does not appear in rekordboxAgent's `app.asar`.
  - TODO (Ghidra): find the xrefs to it and record the encrypt/decrypt call sites.
- **The passphrase is also stored in the binary itself:** `MasterDbKey::MasterDbKey()` @0x100832D00 decodes `64.7BFX0BOY…` (VA 0x104F86BCC) with the same `BlowFishHelper` and key `pioneerdj corporation` (VA 0x104F86BB6). This gives exactly the same passphrase as decoding `dp`; see devlib.md § Key for the algorithm.
- **The passphrase is identical across installs:**
  - The empty template `Contents/Resources/master.db` decrypts with it.
  - New installs copy that template, so the first 16 bytes (the salt) start out identical too.
- **Verified 2026-10-11:** `mb-masterdb` (`examples/probe.rs`) opens `fixtures/sandbox-initial/rekordbox/master.db` and reads all 48 tables.

## Journal and writers
- **Journal mode:** the DB is in WAL mode (header bytes 18/19 = 2). rekordbox runs `PRAGMA journal_mode = WAL` and `PRAGMA synchronous = NORMAL`. When rekordbox exits it checkpoints and deletes `-wal`/`-shm`.
- **Two concurrent writers while rekordbox runs:**
  - the main app: `SQLiteDB::open` @0x1010fc674 with bundled `libsqlcipher.0.dylib` (SQLCipher 4.6.0 / SQLite 3.45.3)
  - rekordboxAgent: Sequelize 4 + @journeyapps/sqlcipher
- **Backups:** rekordbox makes them with `sqlcipher_export` (`SQLiteDB::backup` @0x1010fc94c) into `master.backup.YYYYMMDD.db`, which gets a new salt. musicbox must never reuse that name.

## Schema
- Full DDL, dumped by our own tool: `masterdb/schema.sql`. It has 48 tables and 288 explicit indexes.
- The schema is Sequelize-generated: backtick-quoted identifiers, and index names in the form `snake_table__col_parts`.
- Every synced table has these common columns: `UUID, rb_data_status, rb_local_data_status, rb_local_deleted, rb_local_synced, usn, rb_local_usn, created_at, updated_at`.

## Write rules (verified 2026-10-11 by SQL trace unless marked)
Exact statement sequences, placeholders and value derivations per operation are in `ops/` (start with `ops/common.md` and `ops/INDEX.md`). Evidence: `fixtures/oracle-a/traces/sqltrace-37549.log` (main rekordbox process) plus before/after DBs. rekordboxAgent's writes are not traced; they are inferred from USN gaps and the after-DB.

**Change counter** (verified)
- `agentRegistry` row `registry_id='localUpdateCount'`, column `int_1`, is the global change counter. Both the main app and rekordboxAgent use it.
- Before each write statement the counter is read and set to `int_1 + 1`; the statement stores that value as `rb_local_usn`. **One increment per statement, not per row**: a multi-row statement and a whole add-to-playlist batch share one value. An increment is spent even when the statement changes no row.
- `agentRegistry.updated_at` never changes (the ORM path writes back the value it read).
- The read and the write are separate statements, and during analysis they run in autocommit mode while the agent increments the same counter, so **USNs are not guaranteed unique** (the after-DB has one duplicate). A writer should bump and write in one transaction.
- `max(rb_local_usn)` equalled the counter in both fixtures, but that is not guaranteed (spent-but-unused values, rewritten rows).
- `usn` is never written while cloud sync is off (all NULL).
- rekordbox spends 5 USNs on every launch rewriting the fixed `djmdSampler` rows (`ops/startup.md`).

**Row formats** (verified)
- `created_at`/`updated_at`: `YYYY-MM-DD HH:MM:SS.mmm +00:00`, UTC, a fresh timestamp per statement; equal on insert.
- Inserts set `rb_data_status = 0, rb_local_deleted = 0, rb_local_synced = 0`; `rb_local_data_status` and `usn` take their defaults (0, NULL).
- Updates set `rb_data_status = case when usn != 0 then 257 else 0 end` (0 for never-synced rows) together with the new `rb_local_usn` and `updated_at`. The builder path updates one column per statement.
- IDs: `djmdContent.ID` is random in [1, 2^28−1] (`AppSyncDBController::insertTrack` @0x100a56e3c); other numeric IDs are random u32; all stored as decimal text. `djmdMixerParam.ID` and `djmdSongPlaylist.ID` are UUID strings. `UUID` columns are v4.
- **Deletes are hard for never-synced rows** (`rb_data_status = 0 AND rb_local_synced = 0`) and soft only for rows with sync history (`rb_local_deleted = 1`, `rb_data_status` 2, or 258 if it was non-zero). Orphaned artist/album/genre/label/key rows are removed the same way (`ops/remove-tracks.md`).
- Name lookups (artist, genre, album, label, key) are exact, case-sensitive and ignore deleted rows.

**`djmdContent`** (verified)
- `MasterDBID` = `djmdProperty.DBID`; `MasterSongID` = `ID`; `DeviceID` = `djmdProperty.DeviceID` = `djmdDevice.ID`.
- `AnalysisDataPath` = `/PIONEER/USBANLZ/<UUID[0:3]>/<UUID[3:]>/ANLZ0000.DAT`, relative to `analysis-data-root-path`; `ImagePath` = `/PIONEER/Artwork/<UUID[0:3]>/<UUID[3:]>/artwork.jpg`. Both are written by analysis, not import.
- `BPM` = bpm × 100.
- `FileType`: 1 MP3, 4 M4A/AAC, 5 FLAC, 6 M4A/ALAC, 11 WAV, 12 AIFF.
- `rb_file_id` = the audio file's inode number (`juce::File::getFileIdentifier`), as text.
- `FileNameS` = `""`; `SearchStr` is never written (NULL); `FolderPath` is the full path including the file name.
- Import writes `ContentLink = 14`, `Analysed = 0`, `KeyID = "0"` when there is no key tag, and leaves BPM/SampleRate/BitRate/BitDepth at 0 (`ops/import-track.md`).
- Analysis ends with `Analysed = 105`, `ContentLink = 1902094`, `AnalysisUpdated` +3 and `TrackInfoUpdated` +2 per pass (counters stored as text) for Normal mode with all options (`ops/analyse-track.md`).

**`djmdMixerParam`** (verified) — one row per analysed track, created by the first analysis: `GainHigh/GainLow` and `PeakHigh/PeakLow` are the high and low 16-bit words of an IEEE-754 float32 (auto-gain factor, peak level).

**`contentFile`** (state verified on 602/602 rows; written by rekordboxAgent, not the main app)
- One row per ANLZ file (`.DAT`, `.EXT`, `.2EX`; no `.3EX` with these settings) and one for `artwork.jpg`.
- `ID` = `<content UUID>_<url-encoded Path>`, `Hash` = MD5 of the file, `Size` = byte size, `rb_local_path` = absolute path, `rb_priority` = 50, other `rb_*` flags 0/NULL. Details in `ops/common.md`.

**Playlists** (verified) — new playlists go to `Seq` 1 and push siblings down; each also gets a `djmdCloudFilterPlaylist` row; add-to-playlist appends `TrackNo` n+1… with one USN per batch (`ops/playlist-create.md`, `ops/playlist-add-tracks.md`).

**Side files**
- `masterPlaylists6.xml` (verified): one `NODE` per playlist in `Seq` order with hex `Id`, `ParentId`, `Attribute`, a millisecond `Timestamp` (last change of the list or of a member track; a pure `Seq` shift does not change it), `Lib_Type` and `CheckType`; CRLF line endings; flushed lazily. `mb_masterdb::playlists_xml` round-trips the three fixture files byte for byte; nesting for folders is a *hypothesis* (`ops/playlist-edit.md`).
- `datafile.edb` and `ExtData.edb` (verified): **not rewritten** by playlist creation, adds or analysis (byte-identical before and after).
- Backup at quit (verified): `ATTACH '<dir>/tmp.db' … KEY <passphrase>; sqlcipher_export`; then renamed to `master.backup.%Y%m%d.db` (one per day, last quit wins, `journal_mode = delete`) and old backups pruned (`ops/shutdown.md`).
- No master.db writes for settings changes, rekordbox.xml export or USB export.

## Open questions
- [x] Exact statement sequence per operation: done for startup, import, remove, playlist create/rename/add, analysis, settings, XML export, USB export, shutdown (`ops/`).
- [x] Does rekordbox 7 update `*.edb` when playlists change? No (byte-identical after playlist creation and adds). My Tag edits not tested.
- [x] `rb_file_id` (inode), `SearchStr` (never written), `FileNameS` (`""`).
- [ ] `ContentLink` bit meanings (observed transitions in `ops/analyse-track.md`).
- [ ] Meaning of `Analysed` bits 0x08, 0x20 and 0x40 (0x01 = analysed and 0x02 = queued are verified; see `ops/analyse-track.md`).
- [ ] rekordboxAgent's exact statements (`contentFile`, `agentNotification*`): needs a trace of the agent process.
- [ ] Ops not yet exercised: playlist folders/smart lists/delete/reorder, track edits, cues, My Tags, history, duplicate import. musicbox's current patterns for folders, delete, move and entry edits (`ops/playlist-edit.md`) and for track edits (`ops/edit-track.md`) are hypotheses built from the traced blocks and rekordbox's SQL templates in the binary.
