# Op: USB export — `exportLibrary.db` (OneLibrary / Device Library Plus) statement sequence

Status: **verified** for a first export of one 5-track playlist to an empty FAT32 stick (368 statements on `/Volumes/RBTEST/PIONEER/rekordbox/exportLibrary.db`, L30147–30625, 22:20:55–22:22:22). The final file was compared with `fixtures/oracle-a/rbtest/PIONEER/rekordbox/exportLibrary.db`, which also contains a later, **untraced** export of a 161-track playlist made with the normal app. Table semantics: `../devlib.md`; file copy, ANLZ and artwork rules: `../export.md`.

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

**Index handling:** on this open the indexes were only created when missing; no `drop index` ran. This contradicts `../export.md` §6.3 step 5 and `../devlib.md` § Startup step 4 ("drops and recreates on every open"); the drop only appears in the phase 2 rebuild.

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
- The rebuild drops and recreates every table, then re-seeds color/menuItem/category/sort **in ID order** and the property row **last**, with **`dbVersion = "1000"`** and `deviceName = ""`. This matches the Device Library Plus conversion from the (equally new) classic `export.pdb` (`../export.md` §6.4), which copies the classic property row. The final stick still says `"1000"` and `""` after the later export.
- So a fresh stick ends with `dbVersion "1000"`, not the `"10000"` of `initPropertyTable` (`../devlib.md` § property is incomplete on this point).

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
- `sequenceNo` was 0 for the first list. After the untraced second export the stick has `oracle-all` at 0 and `oracle-small` at 1, so a new list goes to position 0 and existing siblings move down (*hypothesis*, untraced; `../devlib.md` says max+1).

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
- **`<MT>` was the `ID` of the `djmdCloudProperty` row whose `Reserved1 = 'SyncService'`**, read just before the update. It is **not** `DBID + 1` as `../export.md` §2.5 states for the reference stick. Which value applies when (sync service configured or not) is *open*.

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
- Lookups run only for non-empty names; the device `album` is matched by **name only** (no album artist) and inserted without `artist_id`/`image_id`/`nameForSearch`. `artist` is inserted with `(artist_id, name)` only.
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

**State after the untraced re-export** (normal app, `oracle-all`, which contains the 5 tracks again): the 5 rows now have `analysedBits = 105`, `contentLink = 1902336` (0x1D0700), `cueUpdateCount = ''` (empty text), `analysisDataUpdateCount = '6'`, `informationUpdateCount = '4'`; the 152 rows inserted by that export have 41 / 788224 / NULL counters like the traced inserts. So the update-on-re-export path (`../export.md` §4.2) writes the full `Analysed` value, a different `contentLink` merge and the three counters, while the insert path writes none of the counters.

## Phase 6 — quit
```
select count(playlist_id) as numPlaylist from playlist where attribute in (0,4,-128)
```
Then the connection is closed (WAL checkpoint, not visible). The stick files' mtimes moved to 22:35, the end of the later normal-app session.

## What a writer must reproduce (first export to an empty stick)
1. Create all 22 tables and 4 indexes with the exact DDL.
2. Seed `color` 1–8, `menuItem`, `category`, `sort` (ID order) and one `property` row (`deviceName ""`, `dbVersion "1000"`, `numberOfContents 0`, `createdDate <DATE>`, `backGroundColorType 0`, `myTagMasterDBID 0`).
3. Per playlist: `playlist` row (max+1, `sequenceNo` 0 for the first, `attribute` 0, parent 0).
4. My Tags: set `myTagMasterDBID`, rewrite the whole `myTag` tree, empty `myTag_content`.
5. Per track, in playlist order: artist/genre/album/label/key rows on demand (max+1), the `content` row, `numberOfContents` refresh, the `image` row and `content.image_id`, then the `playlist_content` row (1-based).
No transactions are needed to match rekordbox, but wrapping the whole export in one transaction gives the same final content.
