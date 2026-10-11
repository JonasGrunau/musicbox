# Op: startup

Status: **verified** (4 traces: 37549, 27865, 29204, 44426; identical write pattern). Building blocks: `common.md`.

## Trigger
rekordbox launch, before the UI is usable.

## Statement sequence (master.db)
1. Connection setup (L1–2). The key is set with `sqlite3_key` (not visible).
   ```
   PRAGMA synchronous = NORMAL
   PRAGMA journal_mode = WAL
   ```
2. Schema/initialisation checks, reads only in this library (L3–16):
   ```
   select count(*) as numIndex from sqlite_master where type = 'index' and name = 'djmd_cue__content_i_d_rb_local_deleted';
   select name from sqlite_master where type = 'table' and name = 'djmdProperty'
   select * from djmdProperty                                   (twice)
   select count(ID) as numItems from djmdColor     where rb_local_deleted = 0
   select count(ID) as numItems from djmdMenuItems where rb_local_deleted = 0
   select count(ID) as numItems from djmdCategory  where rb_local_deleted = 0
   select count(ID) as numItems from djmdSort      where rb_local_deleted = 0
   select * from djmdSort where rb_local_deleted = 0 and MenuItemID = 12 order by Seq
   select count(ID) as numItems from djmdMyTag     where rb_local_deleted = 0
   ```
   The branches that create the index or seed empty tables were not taken (all present). *Open:* their statements.
3. **Sampler fix-up: 5 ORM writes on every launch** (L13–41). Autocommit, no transaction.
   ```
   select ID from djmdSampler where rb_local_deleted = 0 and ParentID = 'root'
   select count(ID) as numList from djmdSampler where rb_local_deleted = 0 and ParentID = 'root'
   SELECT * FROM djmdSampler WHERE ID = '1' AND rb_local_deleted = 0                (twice)
   [USN bump O]
   UPDATE djmdSampler SET Attribute = 3,Name = 'Sampler Root',ParentID = '4294967295',Seq = 1,rb_data_status = 0,rb_local_data_status = 0,rb_local_deleted = 0,rb_local_synced = 0,rb_local_usn = <U1>,updated_at = '<TS>' WHERE ID = '1'
   select count(ID) as numList from djmdSampler where rb_local_deleted = 0 and ParentID = 1
   SELECT * FROM djmdSampler WHERE ID = '2' AND rb_local_deleted = 0                (twice)
   [USN bump O]
   UPDATE djmdSampler SET Attribute = 5,Name = 'All Samples',ParentID = '1',Seq = 4,… rb_local_usn = <U2> … WHERE ID = '2'
   SELECT * FROM djmdSampler WHERE ID = '2' AND rb_local_deleted = 0
   SELECT * FROM djmdSampler WHERE ParentID = '1' AND ID != '2' AND rb_local_deleted = 0 ORDER BY Seq ASC
   [USN bump O]
   UPDATE djmdSampler SET Attribute = 5,Name = 'All Samples',ParentID = '1',Seq = 1,… rb_local_usn = <U3> … WHERE ID = '2'
   (same three steps for ID '3' 'Capture' Attribute 6: Seq = 4 with <U4>, then Seq = 2 with <U5>)
   ```
   - The pattern is "re-create the fixed sampler nodes": write the root, then move "All Samples" and "Capture" to the end of their parent (`Seq` = sibling count + 1 = 4) and back to their fixed position (1, 2). Siblings already in place are not rewritten.
   - Net effect: values unchanged, **`localUpdateCount` += 5**, and `rb_local_usn` / `updated_at` of sampler rows 1, 2, 3 move to the new USNs (`rb_local_usn` 2 and 3 end at `<U3>`, `<U5>`).
   - Seen in all four traces: USNs 506–510, 7743–7747, 7748–7752, 7753–7757; and in the untraced last session (sampler rows at 7758/7760/7762 in the after-DB).
4. More reads (L42–45): `djmdProperty` (`BaseDBDrive, CurrentDBDrive`), `djmdMenuItems`, `count(ID) … from djmdHistory`.
5. Track cache load inside an empty-write transaction (L81–123):
   ```
   select ID, FolderPath, FileNameL, Title, ArtistID, … from djmdContent …   (full collection read)
   select * from djmdKey where rb_local_deleted = 0
   select distinct ContentID from djmdCue where rb_local_deleted = 0 and Kind <= 17
   select * from djmdActiveCensor where rb_local_deleted = 0
   select ID, Name from djmdMyTag where rb_local_deleted = 0 order by Seq
   select MyTagID, ContentID from djmdSongMyTag where rb_local_deleted = 0
   begin exclusive
     per track: select * from djmdSongTagList / djmdSongRequestList where rb_local_deleted = 0 and ContentID = <CID>; artist/album/genre/label lookups by ID
   commit
   ```
6. UI reads continue (playlists, My Tag tree, `djmdSharedPlaylist`, `djmdCloudProperty WHERE Reserved1 = 'SyncService'`, histogram query over `djmdSongHistory`).

## Other databases opened at startup (all read-only in this session)
| DB | Statements |
|---|---|
| `<settings-root>/LightingDB/user.db3` | `PRAGMA journal_mode = MEMORY`, then `SELECT` on `lighting_property`, `fixture`, `direct_control` |
| `<rekordbox dir>/product.db` | `SELECT * FROM product_base_model WHERE vid = … AND pid = …` (USB device identification; repeats when a device is plugged in) |
| `networkAnalyze6.db`, `networkRecommend.db` | `select name from sqlite_master …`, `PRAGMA table_info(manage_tbl)`, `select * from manage_tbl` |

## Counters
- `localUpdateCount`: **+5** per launch (sampler fix-up). Nothing else is written at startup.
- No backup at startup; the backup runs at quit (`shutdown.md`).

## Notes for musicbox
- musicbox does not need to replay the sampler fix-up. It must tolerate rekordbox spending 5 USNs and rewriting sampler rows on every rekordbox launch.
- A writer opening master.db should use the same pragmas (`synchronous = NORMAL`, `journal_mode = WAL`).
