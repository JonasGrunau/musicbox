# Op: remove tracks from the collection

Status: **verified** for never-synced rows (14 tracks removed in one action, L496–590, 21:59:17). The soft-delete branch is verified only as SQL text (no row took it). Building blocks: `common.md` (DELETE-ROWS, ORPHAN-CLEANUP).

## Trigger
Select tracks in the collection → "Remove from collection". Here the user removed all 14 demo tracks of the fresh library before importing the corpus.

## Statement sequence
`<IDs>` = the content IDs, comma-separated, unquoted. `<UUIDs>` = their `djmdContent.UUID`s, double-quoted.

Pre-reads (UI thread, no transaction):
```
select distinct HistoryID from djmdSongHistory where rb_local_deleted = 0 and ContentID in (<IDs>)
select distinct CloudExportPlaylistID from djmdCloudExportSongPlaylist where rb_local_deleted = 0 and ContentID in (<IDs>)
select distinct PlaylistID from djmdSongPlaylist where rb_local_deleted = 0 and ContentID in (<IDs>)
```
Main transaction (worker thread):
```
begin exclusive
DELETE-ROWS(djmdContent,               ID,          <IDs>)      -- USN +1
DELETE-ROWS(contentFile,               ContentID,   <IDs>)      -- USN +1
DELETE FROM djmdCue WHERE ContentUUID IN(<UUIDs>);              -- plain hard delete, no USN, no soft branch
SELECT * FROM contentCue WHERE ID IN(<UUIDs>)                    -- read only
DELETE-ROWS(djmdRecommendLike,         ContentID1,  <IDs>)      -- USN +1
DELETE-ROWS(djmdRecommendLike,         ContentID2,  <IDs>)      -- USN +1
DELETE-ROWS(djmdSongPlaylist,          ContentID,   <IDs>)      -- USN +1
DELETE-ROWS(djmdSongHistory,           ContentID,   <IDs>)      -- USN +1
DELETE-ROWS(djmdSongMyTag,             ContentID,   <IDs>)      -- USN +1
DELETE-ROWS(djmdSongSampler,           ContentID,   <IDs>)      -- USN +1
DELETE-ROWS(djmdMixerParam,            ContentID,   <IDs>)      -- USN +1
DELETE-ROWS(djmdCloudExportSongPlaylist, ContentID, <IDs>)      -- USN +1
select ID from djmdContent where rb_local_deleted = 0 and VideoAssociate in (<IDs>)
[USN bump]                                                       -- spent even though nothing matched
update djmdContent set VideoAssociate = '', rb_data_status = case when usn != 0 then 257 else 0 end, rb_local_usn = <U>, updated_at = "<TS>" where rb_local_deleted = 0 and VideoAssociate in (<IDs>)
commit
```
After the commit, still on the worker thread and in autocommit mode:
```
select ID from djmdSongHotCueBanklist where rb_local_deleted = 0 and ContentID = <CID>     (once per removed track)
SELECT * FROM djmdSongTagList ORDER BY TrackNo ASC
SELECT * FROM djmdSongRequestList ORDER BY TrackNo ASC
SELECT * FROM djmdActiveCensor WHERE ContentID IN (<IDs>)
ORPHAN-CLEANUP (artist, album, genre, label, key)                -- USN +1 per table with orphans
commit                                                           -- stray, no open transaction
```
Here 4 orphan deletions ran (2 artists, 1 album, 1 genre, 1 label; no keys existed).

## Notes
- Each DELETE-ROWS hard-deletes the never-synced rows and soft-deletes the rest (`common.md`). The after-DB has no trace of the 14 tracks, their 52 `contentFile` rows or their 14 mixer rows.
- `djmdCue` rows are deleted by `ContentUUID`, without USN and without a soft-delete branch. `contentCue` is only read (it would matter for cloud sync, *open*).
- The ANLZ and artwork files under `share/PIONEER/` are not visible in the trace. Whether rekordbox deletes them is *open* (check `share/` of a later fixture).
- Playlists that contained the tracks get no row update here (none existed).

## Counters
`localUpdateCount` +11 inside the transaction (10 DELETE-ROWS + VideoAssociate) + 1 per orphan table: 511–525 here.

## Replay check
`crates/mb-library/tests/session_oracle.rs` runs `mb_library` on a copy of `fixtures/sandbox-initial` (the exact pre-trace library, counter 505) with the same 14 IDs: +15 bumps, all rows of the 10 tables gone (52 `contentFile`, 14 mixer, 8 `djmdSongSampler` rows), no soft-deleted rows, and the orphans deleted are exactly the trace's (`4207772168`, `415774920`, `267108510`, `3229839016`, `1103520049`).

## Tracks that sit in playlists (musicbox, *hypothesis*)
Not traced: the removed demo tracks were in no playlist. DELETE-ROWS removes the `djmdSongPlaylist` rows and leaves `TrackNo` gaps; the traced add-to-playlist then appends at `count + 1`, which could repeat a `TrackNo`. musicbox therefore closes each gap after the orphan cleanup, with the entry-removal shift of `playlist-edit.md` (one builder update and one bump per removed membership, highest `TrackNo` first), and gives the affected playlists a new `masterPlaylists6.xml` `Timestamp`. Cost: +11, +1 per orphan table, +1 per playlist membership. Open: what rekordbox does.
