# Op: USB export — master.db side

Status: **verified** for the traced export of `oracle-small` (5 tracks) to an empty stick (22:19–22:22). The device database sequence is in `export-devlib.md`.

## Trigger
Stick plugged in (22:19:14), then "Export" of a playlist to the device (22:20:54).

## master.db writes
**None.** Between the plug-in and quit the main app sent no INSERT/UPDATE/DELETE to master.db. `localUpdateCount` stayed at its pre-export value, and the later untraced export of `oracle-all` (normal app) did not consume a USN either (every USN up to the final 7762 is accounted for by other ops). There is no "exported to device" bookkeeping in master.db.

The only master.db statements that look like writes are empty transactions on the export worker thread:
```
begin exclusive
commit
```
(3× during the device-DB rebuild, L30216, L30222, L30346). A writer can skip them.

## master.db reads (for building the device rows)
| When | Statements |
|---|---|
| plug-in | `product.db`: `SELECT * FROM product_base_model WHERE vid = <vid> AND pid = <pid>` (device model lookup) |
| export start (classic DB init) | `select * from djmdColor … order by cast(ID as integer)`, `select * from djmdMenuItems …`, `select * from djmdCategory … order by Seq`, `select * from djmdSort … order by Seq` |
| device DB init | the same four reads again, interleaved with the exportLibrary.db seeding |
| job build | `select * from djmdPlaylist where … ID = <P>`, `select ContentID, rb_local_deleted from djmdSongPlaylist where PlaylistID = <P> order by TrackNo` |
| My Tag sync | `SELECT * FROM djmdCloudProperty WHERE Reserved1 = 'SyncService'`, the `djmdMyTag` tree (`select * from djmdMyTag where rb_local_deleted = 0 and ParentID = … order by Seq`) |
| per track | `djmdRecommendLike` (3 queries), `select CueUpdated, AnalysisUpdated, TrackInfoUpdated from djmdContent where … ID = <CID>` (2×), `djmdCue` for the track (2×) |

The content rows themselves are not re-read: the export uses rekordbox's in-memory track cache, which was filled at startup and kept current by the ops above.

## Side files outside master.db (not visible in the trace)
- On the stick: `PIONEER/rekordbox/export.pdb`, `exportExt.pdb`, `exportLibrary.db`, `PIONEER/USBANLZ/…`, `PIONEER/Artwork/…`, `Contents/…`, the settings files (`MYSETTING.DAT`, `MYSETTING2.DAT`, `DJMMYSETTING.DAT`, `DEVSETTING.DAT`, `djprofile.nxs`) and `PIONEER/DeviceLibBackup/` (`../export.md` §5, §6.3).
- Locally: the export log (`~/Documents/rekordbox/`, UTF-16LE, written for the untraced `oracle-all` export because tracks were skipped).

## Counters
`localUpdateCount` and all `rb_local_usn` values unchanged.
