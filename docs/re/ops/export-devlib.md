# Op: USB export — `exportLibrary.db` (OneLibrary / Device Library Plus) statement sequence

Status: **verified** for a first export of one 5-track playlist to an empty FAT32 stick (368 statements on `/Volumes/RBTEST/PIONEER/rekordbox/exportLibrary.db`, L30147–30625, 22:20:55–22:22:22). The final file was compared with `fixtures/oracle-a/rbtest/PIONEER/rekordbox/exportLibrary.db`, which also contains a later, **untraced** export of a 161-track playlist made with the normal app. The final state of both exports is reproduced table for table by the `mb-devlib` writer (§ Replay). Table semantics: `../devlib.md`; file copy, ANLZ and artwork rules: `../export.md`.

## Global facts
- **No transactions.** Not one `begin`/`commit` was sent to exportLibrary.db; every statement autocommits (WAL, `synchronous = NORMAL`).
- The key is set with `sqlite3_key` (not visible). Keying and the passphrase: `../devlib.md` § Key.
- Two connections were used: the UI thread created the file (phase 1), the export worker reopened it (phase 2 onwards).
- String literals are double-quoted; menu names contain U+FFFA … U+FFFB around the key word (`"￺GENRE￻"`).
- New IDs are always `select max(<id>) as maxId from <table>` + 1 (NULL → 1). Name lookups are exact `name = "<name>"`.
- master.db sees **no writes** during the export (`export-usb.md`).

## Phase 1 — create the DB (UI thread, at export start; the stick had no `PIONEER/rekordbox`)
```
PRAGMA synchronous = NORMAL
PRAGMA journal_mode = WAL
for each table T in creation order (content, genre, artist, album, label, key, color, playlist, playlist_content,
    hotCueBankList, hotCueBankList_cue, history, history_content, image, cue, menuItem, category, sort,
    property, recommendedLike, myTag, myTag_content):
  select name from sqlite_master where type = 'table' and name = '<T>'
  create table <T>(<columns exactly as in ../devlib/schema.sql>)
for each index I (index_playlist_content_playlist_id, index_myTag_content_myTag_id,
    index_myTag_content_content_id, index_hotCueBankList_cue_hotCueBankList_id):
  select name from sqlite_master where type = 'index' and name = '<I>'
  create index <I> on <table>(<col>)
insert into property(deviceName, dbVersion, numberOfContents, createdDate, backGroundColorType, myTagMasterDBID) values ("", "10000", 0, "<DATE>", 0, 0)
[master] select * from djmdColor where rb_local_deleted = 0 order by cast(ID as integer)
insert into color(color_id, name) values (1, "Pink"), (2, "Red"), (3, "Orange"), (4, "Yellow"), (5, "Green"), (6, "Aqua"), (7, "Blue"), (8, "Purple")
[master] select * from djmdMenuItems where rb_local_deleted = 0
insert into menuItem(menuItem_id, kind, name) values (1, 128, "￺GENRE￻"), …, (27, 170, "￺MATCHING￻")      -- one multi-row insert, master row order
[master] select * from djmdCategory where rb_local_deleted = 0 order by Seq
insert into category(category_id, menuItem_id, sequenceNo, isVisible) values (…)                     -- master Seq order
[master] select * from djmdSort where rb_local_deleted = 0 order by Seq
insert into sort(sort_id, menuItem_id, sequenceNo, isVisible, isSelectedAsSubColumn) values (…)        -- master Seq order
select * from menuItem
select count(history_id) as numHistory from history
```
Mappings (verified against master): `color` ← `djmdColor` (`ID`, `Commnt`); `menuItem` ← `djmdMenuItems` (`ID`, `Class`, `Name`); `category` ← `djmdCategory` (`ID`, `MenuItemID`, `Seq`, `isVisible = !(Disable & 1)`); `sort` ← `djmdSort` (`ID`, `MenuItemID`, `Seq`, `!(Disable & 1)`, `(Disable >> 1) & 1`).

**Index handling:** on this open the indexes were only created when missing; no `drop index` ran. **Resolved 2026-10-12 (binary):** `createIndexes(bool)` @0x1006F6BA8 drops only when called with `true`, and `startupDeviceDB` passes `false`. So `../export.md` §6.3 step 5 ("drops and recreates on every open") is wrong, and the 20:00 rewrite of the fixture stick needs another explanation.

## Phase 2 — rebuild (export worker, 18 s later)
```
[master] begin exclusive; commit                              -- empty
select * from content order by content_id
select distinct playlist_id from playlist_content
select * from key
select distinct cue_id from hotCueBankList_cue
[master] begin exclusive; commit                              -- empty
delete from history_content
select * from image where path is not null and path != ""
select path from content
select analysisDataFilePath from content
-- new connection
PRAGMA synchronous = NORMAL
PRAGMA journal_mode = WAL
select name from sqlite_master where type = 'table' and name = '<T>'      (22×, all present)
select name from sqlite_master where type = 'index' and name = '<I>'      (4×, all present)
select count(history_id) as numHistory from history
for each table T in creation order:
  drop table if exists <T>
  select name from sqlite_master where type = 'table' and name = '<T>'
  create table <T>(…)
for each index I:
  drop index if exists <I>
  select name from sqlite_master where type = 'index' and name = '<I>'
  create index <I> on <table>(<col>)
insert into color(color_id, name) values (1, "Pink"), …, (8, "Purple")
insert into menuItem(menuItem_id, kind, name) values (…27 rows…)
insert into category(category_id, menuItem_id, sequenceNo, isVisible) values (…)                     -- now in category_id order
insert into sort(sort_id, menuItem_id, sequenceNo, isVisible, isSelectedAsSubColumn) values (…)        -- now in sort_id order (0…17)
insert into property(deviceName, dbVersion, numberOfContents, createdDate, backGroundColorType, myTagMasterDBID) values ("", "1000", 0, "<DATE>", 0, 0)
select * from image where path is not null and path != ""
select * from content order by content_id
select distinct playlist_id from playlist_content
select * from key
select distinct cue_id from hotCueBankList_cue
[master] begin exclusive; commit                              -- empty
select * from content order by content_id
```
- The rebuild drops and recreates every table, then re-seeds color/menuItem/category/sort **in ID order** and the property row **last**, with **`dbVersion = "1000"`** and `deviceName = ""`.
- **Confirmed in the binary (2026-10-12):** this is the Device Library Plus conversion from the (equally new) classic `export.pdb` (`../export.md` §6.4).
  - `clearAllData` @0x1007284B0 is exactly `createTables(true)` + `createIndexes(true)`: drop + create for every table, then every index, as traced.
  - `insertAllDevProperty` @0x100737658 copies the classic `djdbProperty` row (`DevName`, `DBVersion`, `SongNumber`, `DateCreated`, `ColorID`) and `djdbexDBPropertyOption.MyTagMasterDBID`.
- So a fresh stick ends with `dbVersion "1000"`, not the `"10000"` of `initPropertyTable`. The final stick still says `"1000"` and `""` after the later export.
- Replaying phase 1 + phase 2 also reproduces rekordbox's page layout: the index root pages 26–29 come out rotated against a direct create (`DevLibWriter::create` with `replay_conversion`).

## Phase 3 — create the device playlist (UI thread)
```
select * from playlist where playlist_id_parent = 0 or playlist_id_parent is null order by sequenceNo
select * from playlist where name = "<name>" and (playlist_id_parent = 0 or playlist_id_parent is null)   -- match by name (none)
select count(playlist_id) as numRows from playlist                                                       -- ≤ 1000 check
select count(playlist_id) as numList from playlist where playlist_id_parent = 0 or playlist_id_parent is null
select max(playlist_id) as maxId from playlist
insert into playlist(playlist_id, sequenceNo, name, attribute, playlist_id_parent) values (<max+1>, 0, "<name>", 0, 0)
select sequenceNo from playlist where playlist_id = <PL> and playlist_id_parent = 0
select * from playlist where playlist_id = <PL>
select * from image where image_id = 0
```
- `image_id` is not written (NULL). `attribute` 0 = playlist.
- **Sequence rule (resolved 2026-10-12, binary):** `rekordboxDBController::createNewList` @0x1017E6808 runs four steps.
  1. It counts the siblings (`numList`). The new list's `sequenceNo` is 0 when there are none, otherwise `max(sequenceNo)+1` (`select max(sequenceNo) as maxSeq from playlist where playlist_id_parent = 0 or playlist_id_parent is null`).
  2. It inserts the list there.
  3. If the caller passed a position (not −1) that is not past the end, it calls `movePlaylist(id, parent, position)`.
  4. `exportPlaylist` passes position 0. The traced `select sequenceNo from playlist where playlist_id = 1 and playlist_id_parent = 0` is `move_sequence` reading the current position; 0 = 0, so nothing moved.
- For the untraced second export, the same code gives these statements (reconstructed, not traced):
  1. `insert … values (2, 1, "oracle-all", 0, 0)`;
  2. then, in one transaction:
     - `update playlist set sequenceNo = sequenceNo + 1 where sequenceNo >= 0 and sequenceNo < 1 and playlist_id_parent = 0`;
     - `update playlist set sequenceNo = 0 where playlist_id = 2 and playlist_id_parent = 0`.
- That matches the final stick (`oracle-all` 0, `oracle-small` 1).

## Phase 4 — My Tag tree (UI thread)
```
select myTagMasterDBID from property
[master] SELECT * FROM djmdCloudProperty WHERE Reserved1 = 'SyncService'      (3×, with My Tag tree reads in between)
select * from property
update property set myTagMasterDBID = <MT>
delete from myTag
insert into myTag(myTag_id, sequenceNo, name, attribute, myTag_id_parent) values (…whole tree…)
delete from myTag_content
select content_id, masterContentId from content where masterDbId = <DBID>
```
- `myTag` rows: `myTag_id` = `djmdMyTag.ID`, `sequenceNo` = `Seq − 1`, `name`, `attribute` = `Attribute` (1 group, 0 tag), `myTag_id_parent` = `ParentID` with `'root'` → 0. Order: each top-level group followed by its tags, groups by `Seq`. 28 rows here.
- **`<MT>` was the `ID` of the `djmdCloudProperty` row whose `Reserved1 = 'SyncService'`**, read just before the update. **Resolved 2026-10-12 (binary):** `syncMyTagLocal` @0x100C52988 uses `DBID + 1`, unless `AppSyncDBController::getCloudDBID` returns non-zero, which happens when cloud sync is set up (this library syncs with Dropbox). Then it uses that cloud DBID. Both sticks fit.

## Phase 5 — per track (export worker), in playlist order
```
[master] SELECT * FROM djmdRecommendLike WHERE (ContentID1 = '<CID>' OR ContentID2 = '<CID>') AND (DataCreatedH <= 0 OR DataCreatedL <= 0) AND rb_local_deleted = 0   (2×)
[master] select * from djmdRecommendLike where rb_local_deleted = 0 and (ContentID1 = <CID> or ContentID2 = <CID>)
select content_id from content where masterDbId = <DBID> and masterContentId = <CID>          -- already on the stick? (no)
        (audio copy to /Contents/…, seconds; not traced)
select content_id from content where masterDbId = <DBID> and masterContentId = <CID>
[master] select CueUpdated, AnalysisUpdated, TrackInfoUpdated from djmdContent where rb_local_deleted = 0 and ID = <CID>
[master] select … from djmdCue where rb_local_deleted = 0 and ContentID = <CID>
select content_id from content where masterDbId = <DBID> and masterContentId = <CID>
[master] select CueUpdated, AnalysisUpdated, TrackInfoUpdated from djmdContent where …
[artist] select artist_id from artist where name = "<Artist>"   ↳ none: select max(artist_id) as maxId from artist; insert into artist(artist_id, name) values (<max+1>, "<Artist>")
[genre]  select genre_id from genre where name = "<Genre>"      ↳ none: select max(genre_id) …;  insert into genre(genre_id, name) values (…)
[album]  select album_id from album where name = "<Album>"      ↳ none: select max(album_id) …;  insert into album(album_id, name, isComplation) values (<max+1>, "<Album>", 0)
[label]  select label_id from label where name = "<Label>"      ↳ none: select max(label_id) …;  insert into label(label_id, name) values (…)
[key]    select key_id from key where name = "<ScaleName>"      ↳ none: select max(key_id) …;    insert into key(key_id, name) values (…)
select max(content_id) as maxId from content
insert into content(<columns, see below>) values (…)
update property set numberOfContents = (select count(*) from content)
select name from color where color_id = <color_id>
[artwork] select max(image_id) as maxId from image                       (2×)
          insert into image(image_id, path) values (<IMG>, "/PIONEER/Artwork/<%05d of IMG/20+1>/b<IMG>.jpg")
          update content set image_id = <IMG> where content_id = <DC>
select * from myTag where myTag_id_parent = 0 or myTag_id_parent is null order by sequenceNo
select * from myTag where myTag_id_parent = <g> order by sequenceNo     (for each group)
select myTag_id, content_id from myTag_content where content_id in (<DC>)
[master] select … from djmdCue where rb_local_deleted = 0 and ContentID = <CID>
select * from playlist where playlist_id = <PL>
select * from image where image_id = 0
select content_id from playlist_content where playlist_id = <PL> order by sequenceNo
select count(content_id) as numSongs from playlist_content where playlist_id = <PL>
insert into playlist_content(playlist_id, content_id, sequenceNo) values (<PL>, <DC>, <numSongs + 1>)
```
- Lookups run only for non-empty names; `artist` is inserted with `(artist_id, name)` only.
- The device `album` is matched by name. `getAndAddAlbumID` @0x100704388 first looks up or adds the **album artist**. When there is one, it adds ` and artist_id = <id>` to the select and `artist_id` to the insert. A set compilation flag adds ` and isComplation = 1`. None of the 5 traced tracks had an album artist; 6 of the 97 albums on the final stick do.
- The full lookup order of `addTrack` is artist, genre, album (album artist inside), remixer, label, original artist, key, composer, then the lyricist inside `insertTrack`.
- No `cue`, `myTag_content`, `recommendedLike` or `history` rows were written (no cues, tags, likes or histories in the source).
- ANLZ files, artwork files (`b<id>.jpg`, `b<id>_m.jpg`) and the audio copy are file writes (`../export.md` §1.3, §1.6, §3). `export.pdb` / `exportExt.pdb` go through DeviceSQL and are not visible.

### `content` insert
Column order (bracketed columns only when the value exists):
```
content_id, path, fileName, title, [artist_id_artist, album_id, genre_id,] bpmx100, length, trackNo, bitrate, bitDepth,
djComment, fileType, rating, releaseYear, [label_id,] key_id, dateCreated, color_id, djPlayCount, masterDbId,
masterContentId, analysisDataFilePath, fileSize, discNo, subtitle, samplingRate, analysedBits, releaseDate, dateAdded,
contentLink, hasModified, isHotCueAutoLoadOn, isKuvoDeliverStatusOn, kuvoDeliveryComment, artist_id_lyricist, isrc
```
| Device column | Value (verified on 5 rows) |
|---|---|
| `content_id` | max + 1 |
| `path` | `/Contents/<artist name or UnknownArtist>/<album name or UnknownAlbum>/<FileNameL>` (sanitising rules: `../export.md` §1.3) |
| `fileName` | `FileNameL` |
| `title` | `Title` |
| `artist_id_artist`, `album_id`, `genre_id`, `label_id` | device IDs from the lookups; omitted (NULL) when the master has none |
| `key_id` | device key ID (always written) |
| `bpmx100`, `length`, `trackNo`, `bitrate`, `bitDepth`, `djComment`, `fileType`, `rating`, `releaseYear`, `djPlayCount`, `fileSize`, `discNo`, `subtitle`, `samplingRate`, `releaseDate`, `isrc` | copies of `BPM`, `Length`, `TrackNo`, `BitRate`, `BitDepth`, `Commnt`, `FileType`, `Rating`, `ReleaseYear`, `DJPlayCount`, `FileSize`, `DiscNo`, `Subtitle`, `SampleRate`, `ReleaseDate`, `ISRC` |
| `dateCreated` | `DateCreated` |
| `dateAdded` | `StockDate` |
| `color_id` | `ColorID` as integer (0) |
| `masterDbId` | `<DBID>` |
| `masterContentId` | master `ID` |
| `analysisDataFilePath` | `/PIONEER/USBANLZ/P%03X/%08X/ANLZ0000.DAT` (`../export.md` §1.6) |
| `analysedBits` | **`Analysed & 0x3F`** (105 → 41) |
| `contentLink` | **`(ContentLink & 0xC1600) \| 0x100`** (1902094 → 788224 = 0xC0700) |
| `hasModified` | 0 |
| `isHotCueAutoLoadOn` | 1 (`HotCueAutoLoad = "on"`) |
| `isKuvoDeliverStatusOn` | 1 (`DeliveryControl = "on"`) |
| `kuvoDeliveryComment` | `DeliveryComment` (`""`) |
| `artist_id_lyricist` | 0 (no lyricist) |
| not written | `titleForSearch`, `artist_id_remixer`/`originalArtist`/`composer` (no data here), `image_id` (set by the update after the image insert), `cueUpdateCount`, `analysisDataUpdateCount`, `informationUpdateCount` (NULL) |

**Optional columns** in the insert, in their positions: `[artist_id_artist, album_id, genre_id]` after `title`; `[artist_id_remixer, label_id, artist_id_originalArtist]` between `releaseYear` and `key_id`; `[image_id]` after `djPlayCount`; `[artist_id_composer]` after `discNo` (`../devlib.md` § content).

**State after the untraced re-export** (normal app, `oracle-all`, which contains the 5 tracks again): the 5 rows now have `analysedBits = 105`, `contentLink = 1902336` (0x1D0700), `cueUpdateCount = ''` (empty text), `analysisDataUpdateCount = '6'`, `informationUpdateCount = '4'`; the 152 rows inserted by that export have 41 / 788224 / NULL counters like the traced inserts. So the update-on-re-export path (`../export.md` §4.2) writes the full `Analysed` value, a different `contentLink` merge and the three counters, while the insert path writes none of the counters.
- **Explained by the code (2026-10-12):** `overwrite_export` @0x1009B8748 runs these writes:
  1. `setTrackUpdated` → `update content set informationUpdateCount = "4" where content_id = N`;
  2. `setAnalysisUpdated` → `analysedBits = 105` (the full `Analysed` byte), `cueUpdateCount = ""`, `analysisDataUpdateCount = "6"`;
  3. `modifyContentLink` with device | 0x100 | (source & 0x1D0600) = 0x1D0700.
- The `mb-devlib` replay issues exactly these and reproduces all 5 rows.

## Phase 6 — quit
```
select count(playlist_id) as numPlaylist from playlist where attribute in (0,4,-128)
```
Then the connection is closed (WAL checkpoint, not visible). The stick files' mtimes moved to 22:35, the end of the later normal-app session.

## Replay with `mb-devlib` (2026-10-12)
`cargo run -p mb-devlib --example devlib_rebuild -- fixtures/oracle-a fixtures/oracle-a/rbtest <out.db>` replays phases 1–5 for both exports from master.db through `DevLibWriter`.
- **What it takes from the stick:** the device audio and ANLZ paths (export-layer data), the playlist export order and `createdDate`.
- **The format gate refuses 4 tracks:** WAV at 22.05 kHz, as in `export-log.txt`.
- **Result:** every table identical row for row and column for column with the stick, `sqlite_master` (root pages included), page size, page count and the cipher settings. Details: `../devlib.md` § Writer.

## What a writer must reproduce (first export to an empty stick)
1. Create all 22 tables and 4 indexes with the exact DDL.
2. Seed `color` 1–8, `menuItem`, `category`, `sort` (ID order) and one `property` row (`deviceName ""`, `dbVersion "1000"`, `numberOfContents 0`, `createdDate <DATE>`, `backGroundColorType 0`, `myTagMasterDBID 0`).
3. Per playlist: `playlist` row (max+1, `sequenceNo` = end of the sibling list, `attribute` 0, parent 0), then moved to position 0.
4. My Tags: set `myTagMasterDBID`, rewrite the whole `myTag` tree, empty `myTag_content`.
5. Per track, in playlist order: artist/genre/album/label/key rows on demand (max+1), the `content` row, `numberOfContents` refresh, the `image` row and `content.image_id`, then the `playlist_content` row (1-based).
No transactions are needed to match rekordbox, but wrapping the whole export in one transaction gives the same final content.
