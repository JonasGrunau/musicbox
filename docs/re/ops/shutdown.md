# Op: shutdown (quit rekordbox)

Status: **verified** (same sequence in all 4 traces) plus the backup file handling from the binary (`db::AppSyncDBController::backupDBFile` @0x100a43c78, `SQLiteDB::backup` @0x1010fc94c, `db::DBFileDef::masterDbBackupFormatFilename` @0x10168040c, `masterDbBackupIsPotentialTargetFile` @0x10167f984).

## Statement sequence (UI thread)
1. Final reads (statistics), no writes (L30624–30633):
   ```
   select count(ID) as numRows from djmdRecommendLike where rb_local_deleted = 0
   exportLibrary.db: select count(playlist_id) as numPlaylist from playlist where attribute in (0,4,-128)   (only if a device is mounted)
   select ID, Seq, Name, ParentID from djmdPlaylist where rb_local_deleted = 0 and Attribute & 0x80
   SELECT * FROM djmdCloudProperty WHERE Reserved1 = 'SyncService'
   select Reserved1, Reserved2 from djmdCloudProperty where rb_local_deleted = 0 and ID = <row id>
   select * from djmdSampler where rb_local_deleted = 0 and ParentID = 1 order by Seq
   SELECT COUNT(*) AS count FROM djmdSongRequestList c WHERE c.rb_local_deleted = 0
   select count(ID) as numPlaylist from djmdPlaylist where rb_local_deleted = 0 and Attribute in (0,4,-128)
   select count(ID) as numPlaylist from djmdPlaylist where rb_local_deleted = 0 and Attribute in (1)
   select ContentID, rb_local_deleted from djmdSongPlaylist where PlaylistID = 100000 order by TrackNo
   ```
2. **Backup** (L30634–30638), about 0.1 s for a 2.5 MB DB:
   ```
   ATTACH DATABASE '<dir of master.db>/tmp.db' AS backup KEY '<passphrase>'
    BEGIN EXCLUSIVE
    SELECT sqlcipher_export('backup')
    COMMIT
    DETACH DATABASE backup
   ```
   - The four statements after ATTACH start with a space: the binary holds them as one string `ATTACH …; BEGIN EXCLUSIVE; SELECT sqlcipher_export('backup'); COMMIT; DETACH DATABASE backup;`, fills in `__BACKUP_DB__` and `__KEY__`, splits on `;` and runs each piece.
   - `<passphrase>` is the master.db passphrase itself (the same string `derive_passphrase` produces; the backup opens with our tool). Never copy it anywhere.
   - `tmp.db` is `getNonexistentChildFile("tmp", ".db")` in the master.db directory, so a numbered variant is used if `tmp.db` already exists.
3. Connection close (not a statement). With WAL mode the last close checkpoints and removes `-wal`/`-shm`; this is SQLite behaviour and is not visible in the trace. No `PRAGMA wal_checkpoint` was issued.

## Backup file handling (binary, file operations)
1. If the export succeeded and `tmp.db` is non-empty, it is moved (replacing) to **`master.backup.%Y%m%d.db`** formatted from `tmp.db`'s modification time (`juce::Time::formatted`, local time). Otherwise `tmp.db` is deleted.
2. Then rekordbox lists the directory and prunes old backups. A file counts as a backup if it matches `master\.(backup(\d?)|backup\.(\d{4})(\d{2})(\d{2}))\.db` (so the legacy `master.backup.db`, `master.backup2.db`, `master.backup3.db` too). Backups are grouped per calendar day; the exact retention count is *open* (the pruning loop has a bound of 5).
- Result: one file per day, holding the state at the **last** quit of that day. Fixture: `master.backup.20261011.db` in `oracle-a` has `localUpdateCount = 7762`, the same as master.db after the final session.
- The backup is a full `sqlcipher_export`: new salt (different first 16 bytes), same passphrase, **`journal_mode = delete`** (not WAL), same schema and rows.

## Counters
No change. `localUpdateCount` is untouched by shutdown.

## Notes for musicbox
- musicbox must never write to `tmp.db` or `master.backup*.db` in the library folder (rekordbox owns those names, and its pruning would delete foreign files matching the pattern).
- If musicbox wants its own safety copy, use `sqlcipher_export` with the same passphrase into a differently named file.
