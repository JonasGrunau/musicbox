# Op: create a playlist (and rename it)

Status: **verified** (two playlists created and renamed, L8917–8937, L9380–9382, L9996–10019, L10617; 22:00:29–48). Building blocks: `common.md`. All statements run in autocommit mode on the UI thread (no `begin`).

## Trigger
"Create new playlist" in the tree (the UI names it `Unbetitelte Liste`, German for "Untitled List"), then rename by typing a name.

## 1. Insert (ORM dialect)
`<P>` = new playlist ID (`<RID32>`), `<PU>` = its UUID, `<N>` = sibling count, `<MAX>` = max sibling `Seq`.
```
select count(ID) as numList from djmdPlaylist where rb_local_deleted = 0 and ParentID = 'root'
select max(Seq) as maxSeq from djmdPlaylist where rb_local_deleted = 0 and ParentID = 'root'
SELECT * FROM djmdPlaylist WHERE ID = '<P>' AND rb_local_deleted = 0                    -- collision check
[USN bump O]
INSERT INTO djmdPlaylist (Attribute,ID,Name,ParentID,Seq,UUID,created_at,rb_data_status,rb_local_deleted,rb_local_synced,rb_local_usn,updated_at)
  VALUES (0,'<P>','<default name>','root',<MAX+1>,'<PU>','<TS>',0,0,0,<U>,'<TS>')
```
- `Attribute` 0 = playlist (1 = folder, not exercised). `ParentID` = `'root'` for top level, otherwise the parent's ID (*open*: not exercised).
- `ImagePath` and `SmartList` are not written (NULL).

## 2. Move the new node to the top (ORM dialect)
```
SELECT * FROM djmdPlaylist WHERE ID = '<P>' AND rb_local_deleted = 0
SELECT * FROM djmdPlaylist WHERE ParentID = 'root' AND ID != '<P>' AND rb_local_deleted = 0 ORDER BY Seq ASC
for the new node and then each sibling whose Seq changes, in the new order:
  [USN bump O]
  UPDATE djmdPlaylist SET Attribute = <a>,Name = '<name>',ParentID = '<parent>',Seq = <new seq>,rb_data_status = 0,rb_local_data_status = 0,rb_local_deleted = 0,rb_local_synced = 0,rb_local_usn = <U>,updated_at = '<TS>' WHERE ID = '<id>'
```
- Result: the new playlist gets `Seq = 1` and every former sibling moves down by one. Both creations did this (first: new 2→1, existing 1→2; second: new 3→1, then 1→2, 2→3).
- `Seq` is 1-based. Siblings whose `Seq` does not change are not written (same rule as the sampler fix-up in `startup.md`).
- *Open:* whether "top" is fixed or depends on the current selection in the tree (nothing was selected here).

## 3. Cloud filter row (ORM dialect, about 80 ms later)
```
SELECT * FROM djmdPlaylist WHERE ID = '<P>' AND rb_local_deleted = 0
SELECT * FROM djmdCloudFilterPlaylist WHERE PlaylistUUID = '<PU>' AND rb_local_deleted = 0  -- none
SELECT * FROM djmdPlaylist WHERE ID = '<P>' AND rb_local_deleted = 0
SELECT * FROM djmdCloudFilterPlaylist WHERE ID = '<F>' AND rb_local_deleted = 0           -- collision check
[USN bump O]
INSERT INTO djmdCloudFilterPlaylist (ID,PlaylistUUID,UUID,created_at,rb_data_status,rb_local_deleted,rb_local_synced,rb_local_usn,updated_at)
  VALUES ('<F>','<PU>','<UUIDv4>','<TS>',0,0,0,<U>,'<TS>')
```
`<F>` = `<RID32>`. `Seq` takes its default 0, `ParentID` stays NULL. Every new playlist gets one such row.

## 4. Rename (builder dialect)
```
[USN bump B]
update djmdPlaylist set Name = "<name>", rb_data_status = case when usn != 0 then 257 else 0 end, rb_local_usn = <U>, updated_at = "<TS>" where rb_local_deleted = 0 and ID = <P>
select * from djmdPlaylist where rb_local_deleted = 0 and ID = <P>
```
The cloud filter row is not touched by a rename.

## 5. `masterPlaylists6.xml` (file write, not visible in the trace)
`<rekordbox dir>/masterPlaylists6.xml`, CRLF line endings, UTF-8. After the session it held one `NODE` per playlist, in `Seq` order:
```
<?xml version="1.0" encoding="UTF-8"?>
                                                    (empty line)
<MASTER_PLAYLIST Version="3.0.0" AutomaticSync="0">
  <PRODUCT Name="rekordbox" Version="7.2.19" Company="Pioneer DJ"/>
  <PLAYLISTS>
    <NODE Id="<hex ID, upper case, no padding>" ParentId="0" Attribute="0" Timestamp="<ms since epoch>" Lib_Type="0" CheckType="0"/>
    …
  </PLAYLISTS>
</MASTER_PLAYLIST>
```
- `Id` is the playlist ID in hex (`200000` → `30D40`). `ParentId` 0 = root.
- **`Timestamp` = the last time the playlist or one of its tracks changed, in ms.** For the 5-track list it equals the last add-to-playlist statement (22:05:59.904, the insert ran at .903). For the 161-track list it equals the moment the last member track finished analysis (22:04:38.209). Analysis completion runs `select distinct PlaylistID from djmdSongPlaylist where … ContentID in (<CID>)` and then reads each playlist, which is when the timestamp is bumped (*hypothesis* for the mechanism, verified for the values).
- The file mtime (22:07:39) is later than both timestamps, so the file is flushed lazily, not on every change.
- `datafile.edb` and `ExtData.edb` were **not** rewritten (byte-identical before and after).

- `30D40` kept `Timestamp="1791739907631"` through both creations although its `Seq` was rewritten twice (2476–2479, 2758–2762): **a `Seq` shift is not a change** for the timestamp (verified).
- `mb_masterdb::MasterPlaylists` parses and rewrites all three fixture files byte for byte (`crates/mb-masterdb/tests/playlists_xml.rs`). Folders, nesting and the empty tree are *hypotheses* (`playlist-edit.md`).

## Counters
Per creation: 1 (insert) + number of rewritten nodes in step 2 + 1 (cloud filter row). Rename: +1. Here 2476–2479, 2595, 2758–2762, 2924.

## Replay check
`crates/mb-library/tests/session_oracle.rs` creates and renames both lists on a copy of `fixtures/sandbox-initial` after the replayed removal and import: 4, 1, 5 and 1 bumps; the final `rb_local_usn` order equals rekordbox's (oracle-all and the CUE list last written by the second create's move, oracle-small by its rename, cloud filter rows by the last step of each create); `Seq`, `Name`, `Attribute`, `ParentID`, NULL `ImagePath`/`SmartList`, status columns and storage classes equal the oracle-a rows; and `masterPlaylists6.xml` equals oracle-a's file byte for byte once the two random IDs and the two new timestamps are substituted.
