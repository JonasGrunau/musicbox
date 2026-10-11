# master.db

Status: **key verified**, **read verified**, write rules **hypothesis** (still to be confirmed by M2 SQL traces).

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

## Write rules (hypothesis — confirm with SQL traces in M2)

**Change counter**
- `agentRegistry` has a row with `registry_id='localUpdateCount'`, and its `int_1` is the global change counter.
- Each change increments the counter and stores the new value in the row's `rb_local_usn`.
- On a fresh library the counter equals `max(rb_local_usn)`.
- `usn` stays NULL when cloud sync is off.

**Row formats**
- `created_at`/`updated_at` look like `YYYY-MM-DD HH:MM:SS.mmm +00:00` (UTC).
- IDs are random unsigned 32-bit numbers stored as decimal strings. UUIDs are v4.
- Deletes are soft: `rb_local_deleted=1`.

**`djmdContent`**
- `MasterDBID` = `djmdProperty.DBID`, and `DeviceID` = `djmdDevice.ID`.
- `AnalysisDataPath` = `/PIONEER/USBANLZ/<UUID[0:3]>/<UUID[3:]>/ANLZ0000.DAT`, relative to `analysis-data-root-path`.
- `BPM` is stored as bpm×100.
- `FileType` codes: 1 = mp3, 11 = wav, 12 = aiff (still to be completed).

**`contentFile`**
- One row per ANLZ file (DAT, EXT, 2EX, 3EX).
- `ID` = `<content UUID>_<url-encoded path>`.
- `Hash` = MD5 of the file, `Size` = its byte size.

**Side files**
- `masterPlaylists6.xml` lists playlist nodes with hex `Id`, `ParentId`, `Attribute`, a millisecond `Timestamp`, `Lib_Type` and `CheckType`. Line endings are CRLF.
- `datafile.edb` and `ExtData.edb` are legacy DeviceSQL mirrors (colours, My Tag tree, sampler folders). We don't know yet whether rekordbox 7 rewrites them on edits.

## Open questions
- [ ] Exact statement sequence per operation: M2 SQL trace.
- [ ] Does rekordbox 7 update `*.edb` when playlists or tags change?
- [ ] `ContentLink`, `rb_file_id`, `SearchStr`, `FileNameS` derivation.
- [ ] Meaning of `Analysed` bits 0x08, 0x10 and 0x40 (see `analysis.md`).
