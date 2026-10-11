# Op: add tracks to a playlist

Status: **verified** (one batch of 161 tracks into `oracle-all`, L19128–19297; five single adds into `oracle-small`, L23477 … L29660). Building blocks: `common.md`.

## Trigger
Drag one or more collection tracks onto a playlist. One drag = one batch; each of the 5 single drags was its own batch.

## Statement sequence (UI thread)
`<P>` = playlist ID, `<n0>` = current entry count.
```
select * from djmdPlaylist where rb_local_deleted = 0 and ID = <P>
select ContentID, rb_local_deleted from djmdSongPlaylist where PlaylistID = <P> order by TrackNo
select * from djmdPlaylist where rb_local_deleted = 0 and ID = <P>
select count(ID) as numSongs from djmdSongPlaylist where rb_local_deleted = 0 and PlaylistID = <P>     → <n0>
select int_1 as rb_local_usn from agentRegistry where registry_id = 'localUpdateCount'
update agentRegistry set int_1 = <U> where registry_id = 'localUpdateCount'                         -- ONE bump, before the transaction
begin exclusive
insert into djmdSongPlaylist (ID, PlaylistID, ContentID, TrackNo, UUID, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at)
  values ("<UUIDv4>", "<P>", "<CID_1>", <n0+1>, "<UUIDv4>", 0, 0, <U>, 0, "<TS>", "<TS>")
… one insert per track, TrackNo <n0+2> … <n0+k>, every row with the same <U>
commit
select * from djmdPlaylist where rb_local_deleted = 0 and ID = <P>                                  (×3)
select ContentID, rb_local_deleted from djmdSongPlaylist where PlaylistID = <P> order by TrackNo
```

| Value | Derivation |
|---|---|
| `ID` | `<UUIDv4>` string (not a number) |
| `UUID` | another `<UUIDv4>` |
| `PlaylistID`, `ContentID` | IDs as text |
| `TrackNo` | 1-based, appended after the current count: `<n0>` + position in the dragged selection. The 161-row batch has TrackNo 1…161 contiguous. |
| `rb_local_usn` | the single `<U>` of the batch |
| `created_at` = `updated_at` | fresh `<TS>` per row (the 161 rows span 3 ms) |

## Notes
- **One USN per batch**, taken **outside** `begin exclusive` (the only op that does this). 161 rows → `localUpdateCount` +1.
- The `djmdPlaylist` row is not updated (its `updated_at` and `rb_local_usn` stay at the create/rename values).
- `masterPlaylists6.xml`: the playlist's `Timestamp` becomes the time of the add (`playlist-create.md` §5). This is a file write, not visible in the trace.
- Duplicate handling (track already in the list) and inserting at a position other than the end were not exercised (*open*; a mid-list insert presumably renumbers `TrackNo`).
- `datafile.edb` / `ExtData.edb` were not rewritten.
