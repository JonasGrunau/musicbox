# exportLibrary.db (Device Library Plus / OneLibrary)

Status: key **verified**; read **verified** (`mb-devlib`, fixture tests); schema **verified**; column meanings **verified** from the writer code unless marked *hypothesis*; write rules **verified** for everything a USB export writes (the `mb-devlib` writer rebuilds rekordbox's oracle-a export row-for-row, see § Writer), **hypothesis** for cue, hot cue bank, history and relation rows (from the code only, no fixture rows). Nothing has been tested on a player yet.

All addresses are for the rekordbox 7.2.19 arm64 slice (`~/re-work/bin/rekordbox-arm64`). `__TEXT` maps 1:1, so VA = 0x100000000 + file offset. The evidence comes from llvm-objdump disassembly and string/xref scans, because the REA decompiler timed out on this binary.

## Role
- **Where:** a USB stick at `PIONEER/rekordbox/exportLibrary.db`. `-wal`/`-shm` only exist while the file is open. A stick that rekordbox closed cleanly has neither.
- **Readers:** OneLibrary players (CDJ-3000X, XDJ-AZ, OPUS-QUAD, …). Older players read `export.pdb`. rekordbox 7.2.19 writes both.
- **Writer:** class `db::DevSqliteDBController` (source file `rekordboxDevSqliteDB.cpp`), which derives from `db::rekordboxDBController`. Its connection is `DevSqliteDBController::DevSQLiteDB`, a subclass of the generic `SQLiteDB` wrapper, stored at `this+0x250`.

## Key

### Constants (all in `__TEXT,__cstring`)
| VA | Content | Use |
|---|---|---|
| 0x104F8E7C7 | `exportLibrary.db` | → static `DevSqliteDBController::_dbFileName` (VA 0x1056C7500) |
| 0x104F8E7D8 | `64.FMiVKYF6lAn2Y0OtK8F4SGJGclnMl.T4RW3pw80t7Y+euW+AIy8K1.8aJhpZMo+J+nun.OyDvTsCD6C6mTenVC` | → static `DevSqliteDBController::_dbPwd` (VA 0x1056C7508) |
| 0x104F8E94E | `alphatheta corporation` | Blowfish key for `_dbPwd` |

The static initialiser `__GLOBAL__sub_I_rekordboxDevSqliteDB.cpp` (0x10073E0DC) builds both juce::Strings at +0xD3C…+0xD6C. The only code reference to `_dbPwd` is `startupDeviceDB`.

### Call path
1. `DevSqliteDBController::startupDeviceDB` @0x1006F4A88:
   - +0xB0: constructs `CryptoHelpers::BlowFishHelper` (ctor @0x10056FA84) from `juce::String("alphatheta corporation")`.
   - +0xDC: calls `BlowFishHelper::decryptedString(_dbPwd)` @0x10056F9AC.
   - +0x10C: creates the connection with `SQLiteDB::SQLiteDB(file, passphrase, true)` @0x1010FC5C4.
   - +0x134: calls `DevSQLiteDB::openDB(file, passphrase, …)` @0x1007399D4.
2. `SQLiteDB::open` @0x1010FC674 calls `sqlite3_open_v2(path, SQLITE_OPEN_READWRITE|CREATE|FULLMUTEX)` (flags 0x10006). If the passphrase is not empty, it then calls `sqlite3_key(db, utf8, byteLength)`.
   - No `cipher_*` pragmas are set, so SQLCipher 4 defaults apply.
   - rekordbox bundles SQLCipher 4.6.0 (see masterdb.md). Our bundled 4.14.0 opens the file with defaults.

### Decode algorithm (`BlowFishHelper`, used for several secrets)
- **Helper state:** the ctor stores the integer 8 (the Blowfish block size) in field 0. It then creates `juce::BlowFish(keyUtf8, keyByteLength)`.
- **juce::BlowFish is standard Blowfish:**
  - ctor @0x102520AEC: the initial P/S tables at VA 0x103883E38 / 0x103883E80 are the usual hex digits of pi (P[0] = 0x243F6A88, S0[0] = 0xD1310BA6).
  - The key bytes are cycled big-endian into the 18 P words. The tables are then refreshed by encrypting a zero block.
  - `decrypt(uint32& l, uint32& r)` @0x1025211AC runs the 16 rounds in reverse, from P[17] down to P[0].
- **`decryptedString(s)`** @0x10056F9AC runs three steps.
  1. `conv_string_to_array(s, base64=true, pad=false)` @0x10057003C:
     - Calls `juce::MemoryBlock::fromBase64Encoding` @0x1024AA2EC. This is JUCE's own base64 flavour, not RFC 4648:
       - The text before the first `.` is a decimal byte count N.
       - A zeroed N-byte buffer is allocated.
       - Each following character carries 6 bits. They are written least-significant bit first at an increasing bit position, and bits beyond N bytes are dropped.
       - The decode table at VA 0x103881EFF (80 entries, indexed by `char - '+'`) gives the alphabet `.ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+` for values 0…63. Other characters between `+` and `z` decode as 0.
       - So the `.` characters inside the payload are the digit 0, not separators.
     - The buffer is then read as ceil(N/4) **little-endian** 32-bit words (`MemoryBlock::getBitRange(32·i, 32)`).
  2. `decrypt_array(words, swap=false)` @0x10056FE18:
     - Walks the words in pairs (L = w[2k], R = w[2k+1]; a missing R is 0).
     - Calls `BlowFish::decrypt(L, R)` on each pair.
     - Stores the results unswapped.
  3. `conv_array_to_string(words, base64=false)` @0x10056FB68:
     - Writes the words back little-endian into a 4·count-byte block.
     - Reads it as a UTF-8 string, which ends at the first NUL.
     - Pad strip: if the code of the **last character** is below the block size (8), it removes that many characters from the end.
- **Equivalent with off-the-shelf parts:**
  1. JUCE-base64 decode.
  2. Blowfish-ECB-decrypt the bytes with *little-endian* word order (`blowfish::BlowfishLE`) and the 22-byte key `alphatheta corporation`.
  3. Cut at the first NUL, then apply the pad strip.
- **Result:** a 64-character ASCII alphanumeric passphrase. It is handed to `sqlite3_key` as text, so SQLCipher runs PBKDF2 on it.
- **Implementation:** `mb_devlib::derive_passphrase(ENCODED_PASSPHRASE)` / `mb_devlib::passphrase()` (`crates/mb-devlib/src/key.rs`).

### The same helper decodes the master.db passphrase
- `rb::middleware::db::rekordbox::MasterDbKey::MasterDbKey()` @0x100832D00 and `rb::middleware::db::product::ProductDB::getDbPassword()` @0x10056F8C4 run the same `BlowFishHelper` with:
  - the key `pioneerdj corporation` (VA 0x104F86BB6)
  - the encoded string `64.7BFX0BOY6F5j1j4DoaX5FQQJhu1eCqEUBFLaGD9wkuH2bjysMVy.o9Wn6ryqj.i0K0eVTU+RDh07CUTqZAYIqC` (VA 0x104F86BCC)
- **Cross-check, 2026-10-11:** `mb_devlib::decode_with_key` on that pair gives exactly the passphrase that `mb_masterdb::derive_passphrase` gets from `options.json`'s `dp` in the `sandbox-initial` fixture.
- This second, independent target confirms the decode algorithm, and the master.db passphrase does not actually need `options.json`.
- Other callers of `decryptedString` include `CloudAgentAPI::openDB`, `CloudAgentAPI::Agent::start`/`getAuthorizationToken`, `AppSyncDBController::backupDBFile` and `CloudManager::decryptedString`.
- Side note for masterdb.md: the Blowfish key `ZOwUlUZYqe9Rdm6j` is found at file offset **0x4FAE8DD**, not 0x4FAE9B5.

### Verification (2026-10-11)
- The fixture `fixtures/stick/PIONEER/rekordbox/exportLibrary.db` was copied to a temp dir and opened with `PRAGMA key = passphrase()`.
- `PRAGMA quick_check` and `integrity_check` both return `ok`.
- 117 pages, `freelist_count` 0.
- `cipher_page_size` 4096, `kdf_iter` 256000, `cipher_hmac_algorithm` HMAC_SHA512, `cipher_kdf_algorithm` PBKDF2_HMAC_SHA512 (all SQLCipher 4 defaults).
- A wrong passphrase fails on first page access.

## File-level settings
| Setting | Value | Source |
|---|---|---|
| Cipher | SQLCipher 4 defaults: 4096-byte pages, 80-byte reserve, PBKDF2-HMAC-SHA512 ×256000, HMAC-SHA512 | `SQLiteDB::open` sets no cipher pragmas; fixture |
| `journal_mode` | `WAL` | `DevSQLiteDB::openDB` @0x1007399D4 executes `PRAGMA synchronous = NORMAL` (cstring 0x104F8F9B9), then `PRAGMA journal_mode = WAL` (0x104F8F9D5) |
| `synchronous` | NORMAL (per connection, not stored) | same |
| `page_size` | 4096 | fixture |
| `user_version`, `application_id` | 0, 0 (never set) | fixture; no such pragma strings are referenced |
| `auto_vacuum` | 0 (none) | fixture |
| `encoding` | UTF-8 | fixture |
| Open flags | `READWRITE \| CREATE \| FULLMUTEX` | `SQLiteDB::open` |

## Startup sequence (`startupDeviceDB` @0x1006F4A88)
1. If the DB file does not exist, it creates the parent directory and sets a "new DB" flag (`this+0x7C8`).
2. It decodes the passphrase, then opens the DB: open, key, `synchronous=NORMAL`, `journal_mode=WAL`.
3. `createTables(false)` @0x1006F4DF4:
   - For each of the 22 table descriptors (`DevDBTables`, built by `DevDBTables::DevDBTables` @0x100739E30), it calls `SQLiteDB::createTable` @0x1010FD180.
   - `createTable` emits `create table <name>(<col> <type>[ primary key], …)` only when `tableExists` is false.
   - With `true`, it drops and recreates every table.
   - The creation order below is also the `sqlite_master` order.
4. `createIndexes(false)` @0x1006F6BA8 creates the 4 indexes `index_playlist_content_playlist_id`, `index_myTag_content_myTag_id`, `index_myTag_content_content_id` and `index_hotCueBankList_cue_hotCueBankList_id` when missing (`SQLiteDB::createIndex` checks `sqlite_master` first). It drops them only when called with `true`. **Resolved 2026-10-12** (binary + SQL trace): an earlier reading had the drop on every open. Only `clearAllData` @0x1007284B0 = `createTables(true)` + `createIndexes(true)` drops and recreates everything; the Device Library Plus conversion runs it (ops/export-devlib.md phase 2).
5. When the file did not exist before, it calls vtable slot +0x5B0, which is `initPropertyTable` @0x1006F26A8 (resolved from `vtable for db::DevSqliteDBController` @0x1051ADAC0). Otherwise it calls `rekordboxDBController::readMenuItemTable`. On failure it calls slot +0x5C8 (`closeDB`).

## Schema
- **Source:** full DDL dumped by our own tool, in `devlib/schema.sql`.
- **Tables:** 22, created in this order: content, genre, artist, album, label, key, color, playlist, playlist_content, hotCueBankList, hotCueBankList_cue, history, history_content, image, cue, menuItem, category, sort, property, recommendedLike, myTag, myTag_content.
- **Column types:** only `integer` and `varchar`.
- **Keys:** a single `<table>_id integer primary key` (a rowid alias) on every table except the link tables, `property` and `recommendedLike`. There are no foreign keys, NOT NULL constraints or defaults.
- **DDL text:** `sqlite_master` holds `CREATE TABLE content(content_id integer primary key, title varchar, …)` (no spaces after the table name) and `CREATE INDEX index_x on t(col)`. A writer that wants byte-identical DDL should emit exactly that text.
- **IDs:**
  - Most new IDs are `max(<id>)+1`, 1 for an empty table (`get_next_id` @0x100703FF4; `insertTrack` and `setHotCue` do the same inline).
  - **Playlists, folders, hot cue bank lists** (`rekordboxDBController::createNewList` @0x1017E6808): `sequenceNo` = 0 if the parent has no lists yet, otherwise `max(sequenceNo) + 1` among the siblings (`getListCountInFolder`, `getPlaylistFolderSeqMax` @0x100720A80). The caller then passes a target position; when it is not −1 and not past the end, the new list is moved there (`movePlaylist` @0x100721AB0 → `move_sequence` @0x100710230). The USB export passes 0 (`exportPlaylist` reads it from a per-list position array, default 0), so **every newly exported list ends up on top** and the existing siblings shift down by one. **Resolved 2026-10-12** (binary + fixture): this is what put `oracle-all` at 0 and `oracle-small` at 1.
  - `move_sequence` runs, in one transaction (`transactionStart(2)`): when moving up, `update T set sequenceNo = sequenceNo + 1 where sequenceNo >= <new> and sequenceNo < <old> and <parent> = P`; when moving down, `… - 1 where sequenceNo > <old> and sequenceNo <= <new> …`; then `update T set sequenceNo = <new> where <id> = X and <parent> = P`.
  - **Histories** use `get_next_seq` @0x10070E954 instead: `max(sequenceNo) + 1` among siblings, where an empty parent gives NULL → 0 → **1**. So the first history gets `sequenceNo` 1, unlike playlists (0).
  - Link-table entries are 1-based: `playlist_content` gets `count(entries) + 1` (export), `history_content` continues after the highest `sequenceNo` of the history (`insertTrackToHistory` @0x100713578).
  - IDs are small integers, unlike master.db's random u32 strings. The fixture's playlists 4–7 and 16385+ (0x4001…) suggest a second ID range, perhaps for lists created on a player ("TAG LIST 00x"), with later exports continuing from max+1 (*hypothesis*).
- **Rows:** none are ever soft-deleted; there are no `rb_*` sync columns.

## Tables
The column → member mapping comes from the `DevDBTables::<Table>` constructors (0x10073A67C …). Each table object holds juce::String column names at fixed offsets from the controller (`content` @+0x258, `cue` @+0x5B8, `property` @+0x6E8, …). Every SQL builder references those members, so every insert and update below is identified by column.

### content (one row per exported track)
The row is written by `insertTrack(RowDataTrack*)` @0x10070562C as a single INSERT. It then runs `update_property_num_content`.

- **Lookups before the insert** (`rekordboxDBController::addTrack` @0x1017E1EE4), each only for a non-empty name: artist (+0xD0), genre (+0x180), album (+0x108, with the compilation flag +0x120 and album-artist name +0x148), remixer (+0x300), label (+0x1B8), original artist (+0x290), key (+0x210), composer (+0x338). The lyricist (+0x458) is looked up inside `insertTrack`, after `select max(content_id)`. (`export.md` §1.5 lists remixer, original artist and composer before label and key; the code order is the one here.)
- **Column order of the INSERT** (the NamedValueSet insertion order, confirmed by the SQL trace): `content_id, path, fileName, title, [artist_id_artist, album_id, genre_id,] bpmx100, length, trackNo, bitrate, bitDepth, djComment, fileType, rating, releaseYear, [artist_id_remixer, label_id, artist_id_originalArtist,] key_id, dateCreated, color_id, djPlayCount, [image_id,] masterDbId, masterContentId, analysisDataFilePath, fileSize, discNo, [artist_id_composer,] subtitle, samplingRate, analysedBits, releaseDate, dateAdded, contentLink, hasModified, isHotCueAutoLoadOn, isKuvoDeliverStatusOn, kuvoDeliveryComment, artist_id_lyricist, isrc`. Bracketed columns are only present when non-zero, so they are NULL otherwise.
- **Value widths** (from the `juce::var` constructors): `length`, `bitDepth`, `releaseYear`, `djPlayCount` and `discNo` are signed 16-bit; `fileType`, `rating`, `color_id` and `analysedBits` are signed 8-bit; `fileSize`, `samplingRate`, `masterDbId`, `masterContentId`, `contentLink` and the lookup IDs are unsigned 32-bit; `bpmx100`, `trackNo` and `bitrate` are signed 32-bit. A value outside its width wraps. For example, a track longer than 32767 s gets a negative `length`.
- **Strings** are never NULL: an empty source field is written as `''` (`subtitle`, `djComment`, `releaseDate`, `isrc`, `kuvoDeliveryComment`). `SQLiteDB::toSqlString` @0x1010FD708 double-quotes strings with `""` escaping (`addEscapeChars` @0x1010FD648), so stored strings equal the source. A *void* `juce::var` would be written as `""`, not NULL.
- **Name lookups inline the name as a double-quoted literal** (`where name = "<name>"`). SQLite resolves a double-quoted token as an identifier first, so a name equal to a column name of the lookup table (`name`, `genre_id`, …) matches every row. This is a rekordbox quirk; the `mb-devlib` writer binds the value instead.

| Column | Meaning |
|---|---|
| `content_id` | `max+1`. Referenced by every other table. |
| `title`, `subtitle` | Strings. `subtitle` is `''` when empty. |
| `titleForSearch` | Not written by 7.2.19 (NULL). Older exports wrote `''`. Players presumably fall back to `title`. |
| `bpmx100` | BPM × 100 (int). |
| `length` | Seconds (int16 in `RowDataTrack`). |
| `trackNo`, `discNo`, `releaseYear` | Ints, 0 = unknown. |
| `artist_id_artist`, `artist_id_remixer`, `artist_id_originalArtist`, `artist_id_composer` | → `artist`. Written **only when ≠ 0**, so NULL means none. Older exports wrote 0. |
| `artist_id_lyricist` | Always written. With a lyricist name, the value comes from vtable slot +0x858 = `getAndAddArtistID`, which adds the name to `artist` if needed; without one it is 0. |
| `album_id`, `genre_id`, `label_id`, `image_id` | → album / genre / label / image. Same NULL-when-none rule. |
| `key_id` | → `key`, always written (0 = none). |
| `color_id` | Colour slot 1…8 (→ `color`), 0 = none. |
| `djComment` | The track comment. |
| `rating` | 0…5 (*hypothesis*, copied from master `Rating`; the fixture is all 0). |
| `releaseDate` | `YYYY-MM-DD` or `''`. |
| `dateCreated` | `YYYY-MM-DD` (master `DateCreated`). |
| `dateAdded` | `YYYY-MM-DD` (master `StockDate`). Today's date (`DBToolBox::getDateString`) if empty. |
| `path` | Device path of the audio file, starting with `/` and relative to the stick root (`/Contents/<artist>/<album>/<file>`). |
| `fileName` | The source's file name, or the part of `path` after the last `/`. |
| `fileSize` | Bytes. |
| `fileType` | Same codes as master.db (fixture: 12 = AIFF, 5 = FLAC). |
| `bitrate` | kbit/s (1411 for 16-bit/44.1 kHz PCM, 0 for FLAC). |
| `bitDepth`, `samplingRate` | Ints. |
| `isrc` | String. |
| `djPlayCount` | Int. |
| `isHotCueAutoLoadOn` | 0/1, from master `HotCueAutoLoad`. Changed by `setAllHotCueAutoLoad` / `modifyHotCueAutoLoad`. |
| `isKuvoDeliverStatusOn`, `kuvoDeliveryComment` | From master `DeliveryControl` / `DeliveryComment`. |
| `masterDbId` | `djmdProperty.DBID` of the source master.db (fixture 2328226057). |
| `masterContentId` | `djmdContent.ID` in that library. Used to match re-exports (`getContentIDByMasterIDs`). |
| `analysisDataFilePath` | Device path of `ANLZ0000.DAT` (`/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.DAT`). `.EXT` and `.2EX` sit next to it. |
| `analysedBits` | Copy of master `Analysed`. Updated by `setAnalyzed` / `setAnalysisUpdated`. Fixture: 41 and 105. |
| `contentLink` | Flag word copied from master `ContentLink`. **Bit 0 = beat grid modified** (`selectModifiedBeatGridTracks` @0x10070C30C selects rows with bit 0 set). insertTrack has a dead branch that would OR in 0x80. Fixture: 0xC0700 for every row. |
| `hasModified` | 0/1 (meaning *open*; fixture all 0). |
| `cueUpdateCount`, `analysisDataUpdateCount`, `informationUpdateCount` | Strings copied from master `CueUpdated` / `AnalysisUpdated` / `TrackInfoUpdated`. They are compared with the source on re-export to skip unchanged tracks (`compareTrackUpdated` @0x1006F1AF4 & co. compare `var::toString` of the column, so NULL reads as ""). INSERT does not write them; `setTrackUpdated` @0x1006F1FEC (`informationUpdateCount`) and `setAnalysisUpdated` @0x1006F21EC (`analysedBits` = the full source `Analysed` byte, `cueUpdateCount`, `analysisDataUpdateCount`) do. Because they are bound as strings, an empty marker is stored as **text `''`** while numbers get integer affinity. |

**Re-export of a track that is already on the stick** (`DatabaseMediator::overwrite_export` @0x1009B8748). The writes, in order:
1. If the info marker differs (`compareTrackUpdated`), or one other source field differs: `checkAndSetChanges` + `modifyTrack` for changed fields, then `setTrackUpdated(info)`.
2. If the analysis or cue marker differs, or extended cues are now needed: `setAnalysisUpdated(id, Analysed, analysis, cue)` (full `Analysed`, not `& 0x3F`).
3. Then the `contentLink` merge: the device value gains 0x100, plus the source's 0x200, 0x400, 0x40000 and 0x80000 bits. 0x10000 and 0x100000 come from the source only when the source row's kind (RowDataTrack+0x50) is 1 or 6, which is the collection (*hypothesis* for the meaning of +0x50).
4. `modifyContentLink` @0x10071EAAC runs only when the merge changed something.
5. The device ANLZ set is deleted and rewritten (`export.md` §4.2).

Verified on oracle-a: rows 1–5 (inserted by the first export, then re-exported) end with `analysedBits` 105, `contentLink` 1902336, `cueUpdateCount` `''`, `analysisDataUpdateCount` 6 and `informationUpdateCount` 4.

The fixture holds two generations of rows:
- 1–620 come from an older rekordbox. Unknown IDs are 0, `titleForSearch` is `''` and `isKuvoDeliverStatusOn` is 0.
- 621–704 come from the 7.x insert path. Unknown IDs are NULL, `titleForSearch` is NULL and `isKuvoDeliverStatusOn` is 1.

### artist / album / genre / label / key / color
- `artist(artist_id, name, nameForSearch)`; `genre(genre_id, name)`; `label(label_id, name)`.
  - Added on demand by `getAndAdd{Artist,Genre,Label,Key}ID`. Verified for genre (@0x100703C3C): it selects by exact `name = '<name>'`, and only when that finds nothing does it insert `get_next_id` (max+1).
  - `nameForSearch` is `''` or NULL.
- `album(album_id, name, artist_id, image_id, isComplation, nameForSearch)`: written by `getAndAddAlbumID(name, compilation, albumArtist, artistId)` @0x100704388. The spelling `isComplation` is rekordbox's. The function runs these steps:
  1. It looks up or adds the **album artist** name through `getAndAddArtistID`. An empty name gives 0 and no SQL. The 4th argument, the track artist ID, is unused.
  2. It selects by `name = "<name>"`, adds ` and artist_id = <id>` when there is an album artist, and adds ` and isComplation = <flag>` when the flag is non-zero. So an album without an album artist matches any same-named album.
  3. If nothing matches, it inserts `(album_id = max+1, name, [artist_id,] isComplation)`. `image_id` and `nameForSearch` stay NULL. Verified on oracle-a: 6 of 97 albums have an album artist.
- `key(key_id, name)`: free-text key names copied from the collection. Fixture examples: `Gmin`, `Dm`, `9A`, `Unknown`, `Ebmin`.
- `color(color_id, name)`: copied from master `djmdColor` (ID → `color_id`, `Commnt` → `name`) when the DB is created (`initPropertyTable`). Fixture: 1 Pink, 2 Red, 3 Orange, 4 Yellow, 5 Green, 6 Aqua, 7 Blue, 8 Purple.
- **Changing a lookup column later** (`modifyGenre` @0x10071FB08 and the other `modify<Lookup>` functions) differs from the insert:
  - It looks up or adds the name, then writes the ID with `update content set <col> = <id>`, so an empty name writes **0**, not NULL.
  - It then runs `delete_unused_record` @0x100719890, which deletes lookup rows no longer referenced by a value > 0. For genre, label and key that is the matching `content` column. For artists it is all five `content` artist columns and `album.artist_id`.

### image
- `image(image_id, path)`: added by `addImage` → `calc_next_device_image_path` @0x10070A018.
- **Path:** `/PIONEER/Artwork/%05d/b<id>.jpg` with folder number `id / 20 + 1`, so ids 1–19 go to `00001` and 20–39 to `00002`.
  - The root is `/.PIONEER` instead of `/PIONEER` when a flag argument is set (*open*: which devices use it).
- **Files on the stick:** `b<id>.jpg` is 80×80 and `b<id>_m.jpg` is 240×240. The fixture has both for all 702 rows.
- **The `a<id>` files** in the same folders belong to `export.pdb`.
- `content.image_id` and `album.image_id` point here.

### playlist / playlist_content
- **`playlist(playlist_id, sequenceNo, name, image_id, attribute, playlist_id_parent)`**, written by `insertPlaylist(seq, name, listType, parent)` @0x100720DB4:
  - `attribute` maps rekordbox's internal list type as 8 (playlist) → **0**, 9 (folder) → **1**, and passes any other type through.
  - `sequenceNo` is the 0-based position among siblings.
  - Parent 0 = root.
  - `image_id` is NULL on new rows (older rows: 0).
- **`playlist_content(playlist_id, content_id, sequenceNo)`:** `sequenceNo` is **1-based** and contiguous per playlist (fixture: 1…n for every list). It is renumbered by `renumberPlaylistTrack` / `fixPlaylistTracksOrder`.
- **Index:** `index_playlist_content_playlist_id`.

### cue (memory cues, loops, hot cues, hot-cue-bank cues)
- **Writers:**
  - `setHotCue(contentId, inMs, in150, inMpegFrame, inMpegAbs, outMs, out150, outMpegFrame, kind, outMpegAbs)` @0x10071D6C4 inserts `cue_id` (max+1), `content_id`, `kind`, `inUsec` and `outUsec`, the two `*150FramePerSec` columns, the two `*MpegFrameNumber` columns and the two `*MpegAbs` columns.
    - `inUsec` = `inMs × 1000` as unsigned 32-bit, so it wraps after about 71.6 min.
    - `outUsec` is −1 when `outMs` is −1.
  - `setMemoryCue` @0x10071D68C is a tail call through vtable slot +0x1008, which is `setHotCue`.
  - `addCueOption2(cueId, analyzer::Cue)` @0x100716058 then updates every column except `isActiveLoop`: kind, colorTableIndex, cueComment, the µs positions with the sub-ms part, the 150 fps and MPEG columns, beatLoopNumerator/Denominator, and the block seek columns. `isActiveLoop` stays NULL unless `enableActiveLoop` writes it.
  - `setHotCueBanklistCueInfoWithID` @0x1006F8120 writes all columns in one go.
- **Reader:** `set_cueInfo` @0x1006F8C38.
- **The fixture has no cue rows**, so everything here comes from code.

| Column | Meaning (`analyzer::Cue` field) |
|---|---|
| `cue_id` | `max+1`. |
| `content_id` | → `content`. |
| `kind` | `0` = memory cue, read back as a **loop** when `outUsec ≠ -1`. `1…17` = hot cue slot (1 = A); `101` is also kept. Everything else is dropped on read (`db::dsql_conv_cue_kind` @0x1019AA930 maps it to -2). |
| `colorTableIndex` | Byte at Cue+0x4C. `addCueOption2` first sets it from Cue+0x31, then overwrites it with Cue+0x4C. |
| `cueComment` | String (Cue+0x38). |
| `isActiveLoop` | 0/1 (`enableActiveLoop`, `modifyCueOptionActiveLoop`). |
| `beatLoopNumerator`, `beatLoopDenominator` | u16 pair (Cue+0x40/+0x42): the beat-loop size as a fraction. |
| `inUsec` / `outUsec` | **Microseconds** = `ms × 1000 + subMs`, where `ms` is Cue+0x10/+0x18 and `subMs` (0…999) is Cue+0x44/+0x46. **`outUsec = -1` means no out point.** On read, `ms = usec / 1000` and `subMs = usec % 1000` (u32 arithmetic). |
| `in150FramePerSec` / `out150FramePerSec` | Position in 1/150 s "frames": the CDJ cue unit and master `InFrame`/`OutFrame` (Cue+0x14/+0x1C). |
| `inMpegFrameNumber` / `outMpegFrameNumber` | MPEG frame index (master `InMpegFrame`; Cue+0x20/+0x28). |
| `inMpegAbs` / `outMpegAbs` | Absolute MPEG position (master `InMpegAbs`; Cue+0x24/+0x2C). |
| `inDecodingStartFramePosition` / `outDecodingStartFramePosition` | int64 (Cue+0x50/+0x58). |
| `inFileOffsetInBlock` / `OutFileOffsetInBlock` | int64 (Cue+0x60/+0x68). Note the capital `O` in `OutFileOffsetInBlock`. |
| `inNumberOfSampleInBlock` / `outNumberOfSampleInBlock` | u32 (Cue+0x70/+0x74). |

The last three pairs are block-seek information for compressed formats, so a player can start decoding at a cue without scanning. They are probably what master.db keeps in `InPointSeekInfo` / `OutPointSeekInfo` (*hypothesis*). For PCM files (WAV/AIFF), 0 is the natural value (*hypothesis*).

### hotCueBankList / hotCueBankList_cue
- **`hotCueBankList`** has the same shape as `playlist`. It is written by `addHotCueBanklist(name, parent, attribute, sequenceNo)` @0x100721F08. Note the argument order differs from `insertPlaylist(seq, name, type, parent)`.
  - It inserts `(hotCueBankList_id = max+1, sequenceNo, name, attribute, hotCueBankList_id_parent)` with `image_id` NULL.
  - `attribute` is the raw byte (no 8→0 / 9→1 mapping).
  - `createNewList` gives it the same sequence and move rules as playlists (list type 4).
  - The `DevDBTables::HotCueBankList` descriptor holds the id column name twice: a computed `hotCueBankList_id` and a literal one. That is why its member offsets are shifted by 8 against `playlist`.
- **`hotCueBankList_cue(hotCueBankList_id, cue_id, sequenceNo)`:** `setHotCueBankCue` @0x10071DC08 inserts the cue row through `setHotCue` and then the link row, inside one transaction (`transactionStart(2)`).
- **Index:** `index_hotCueBankList_cue_hotCueBankList_id`.

### history / history_content
- **`history(history_id, sequenceNo, name, attribute, history_id_parent)`** and **`history_content(history_id, content_id, sequenceNo)`** hold play histories (presumably recorded by players and imported by rekordbox; *hypothesis*).
- rekordbox writes them in `addNewHistory(name, attribute, parent, time)` @0x10071313C and `insertTrackToHistory` @0x100713578.
  - `addNewHistory`: `history_id = max+1`; `sequenceNo` is from `get_next_seq` (siblings' max + 1, so 1 for the first); then `(history_id, sequenceNo, name, attribute, history_id_parent)`.
  - `insertTrackToHistory(id, contentIds)`: reads the highest `sequenceNo` (`order by sequenceNo desc`), then inserts one `(history_id, content_id, sequenceNo)` row per track, numbered from that value + 1, or from 1 for an empty history.
- The fixture has no history rows.

### myTag / myTag_content
- **`myTag(myTag_id, sequenceNo, name, attribute, myTag_id_parent)`:** the whole tree is rewritten by `writeMyTag(masterDbId, list)` @0x1007121E4.
  - It sets `property.myTagMasterDBID`. If there is no property row, it inserts `("", "10000", 0, today, 0, id)` instead.
  - It deletes every `myTag` row, then bulk-inserts.
  - Its caller `syncMyTagList` @0x1017F273C passes `sequenceNo = Seq − 1`. On a full rewrite it then clears `myTag_content` (`clearMyTagTrack`).
  - Track links are added per track by `insertMyTagOnTrack` @0x10070EC20, a multi-row `(myTag_id, content_id)` insert.
  - `attribute` 1 = column/folder (fixture: the 4 top-level groups "Genre", "Components", "Situation", "Untitled Column", ids 1–4, parent 0).
  - `attribute` 0 = tag. Tag ids are the master.db My Tag IDs (random u32).
- **`myTag_content(myTag_id, content_id)`** has no sequence column (the fixture has 0 rows). Indexes: `index_myTag_content_myTag_id`, `index_myTag_content_content_id`.

### menuItem / category / sort (player browse menus)
- **Source:** all three are copied from master.db when the DB is created (`initPropertyTable` @0x1006F26A8). It reads `djmdMenuItems`, `djmdCategory` and `djmdSort` with `rb_local_deleted = 0`.
- **Fallback defaults:** if master.db has no rows, the defaults come from `initMenuItemsTable` @0x10071B504, `initCategoryTable` @0x10071BDD0 and `initSortTable` @0x10071D090.
- **`menuItem(menuItem_id, kind, name)`:**
  - ID ← `ID`, kind ← `Class`, name ← `Name`.
  - Names are `U+FFFA` + KEY + `U+FFFB`. The defaults wrap them with wide-string prefix/suffix literals (`initMenuItemsTable` lambda @0x10071BC18). They look like translation keys the player resolves.
  - Fixture/defaults:
    - 1 GENRE 128, 2 ARTIST 129, 3 ALBUM 130, 4 TRACK 131, 5 BPM 133, 6 RATING 134, 7 YEAR 135
    - 8 REMIXER 136, 9 LABEL 137, 10 ORIGINAL ARTIST 138, 11 KEY 139, 12 CUE 141, 13 COLOR 142, 14 TIME 146
    - 15 BITRATE 147, 16 FILE NAME 148, 17 PLAYLIST 132, 18 HOT CUE BANK 152, 19 HISTORY 149, 20 SEARCH 145
    - 21 COMMENTS 150, 22 DATE ADDED 140, 23 DJ PLAY COUNT 151, 24 FOLDER 144, 25 DEFAULT 161, 26 ALPHABET 162, 27 MATCHING 170
- **`category(category_id, menuItem_id, sequenceNo, isVisible)`:**
  - The top-level browse menu.
  - `isVisible = !(Disable & 1)`.
  - Visible entries are numbered 1…n in `sequenceNo` and hidden ones are 0.
- **`sort(sort_id, menuItem_id, sequenceNo, isVisible, isSelectedAsSubColumn)`:**
  - `isVisible = !(Disable & 1)` and `isSelectedAsSubColumn = (Disable >> 1) & 1`.
  - `sort_id` starts at **0**.
  - Fixture: COMMENTS is the sub column.

### property (exactly one row, no key)
| Column | Meaning |
|---|---|
| `deviceName` | Volume label (`updateDeviceName`). Fixture: `REKORDBOX`. |
| `dbVersion` | String. `initPropertyTable` and the insert branch of `writeMyTag` write `"10000"` (cstring 0x104F8E832). **A fresh 7.2.19 stick still ends with `"1000"` and `deviceName ""`** (resolved 2026-10-12, binary + trace). The Device Library Plus conversion first runs `clearAllData`, then `insertAllDevProperty` @0x100737658, which copies the row from export.pdb (`djdbProperty`: `DevName`, `DBVersion`, `SongNumber`, `DateCreated`, `ColorID`; `djdbexDBPropertyOption.MyTagMasterDBID`). The classic DB's version is "1000". The old fixture stick says "1000" for the same reason. |
| `numberOfContents` | `count(content)`, refreshed after every insert or remove (`update_property_num_content` @0x10070669C, using an `update … set … = (select count(*) …)` template). |
| `createdDate` | `YYYY-MM-DD`, set at creation. |
| `backGroundColorType` | Device colour chosen in rekordbox (`selectDeviceColor` @0x1007189D8). Initially 0; the fixture has 2. |
| `myTagMasterDBID` | The ID passed to `writeMyTag`. Initially 0. `DatabaseMediator::syncMyTagLocal` @0x100C52988 passes **master DBID + 1**. When the library has cloud sync set up, it passes the cloud library's DBID instead (`AppSyncDBController::getCloudDBID` ≠ 0, the ID of the `djmdCloudProperty` row `Reserved1 = 'SyncService'`). **Resolved 2026-10-12:** the old stick has DBID + 1 (2328226058); RBTEST has the SyncService ID 2140940981 (its library syncs with Dropbox). |

### recommendedLike
- `recommendedLike(content_id_1, content_id_2, rating, createdDate)` holds "like" relations between two tracks.
- Written by `addTracksRelation` @0x10070C808 / @0x10070D17C.
- `createdDate` is `juce::Time::currentTimeMillis()` (ms since the Unix epoch). `rating` was not seen being written.
- The fixture has 0 rows.

## Writer (`mb-devlib`, verified 2026-10-12)
`mb_devlib::DevLibWriter` (`crates/mb-devlib/src/write.rs`) reproduces `DevSqliteDBController` call by call. Every method names its rekordbox function. Values are bound as parameters instead of inlined, which stores the same values (see § content, *Strings*). The DDL text and the seed defaults are in `crates/mb-devlib/src/schema.rs`.

| Rule | Writer API | Status |
|---|---|---|
| Open/key/`synchronous=NORMAL`/`journal_mode=WAL`, SQLCipher 4 defaults | `create`, `open` | verified (oracle-a) |
| 22 tables + 4 indexes, byte-identical `sqlite_master.sql` | `create` (`schema::TABLES`, `INDEXES`) | verified |
| Seed: `initPropertyTable` (`"10000"`), then conversion `clearAllData` + re-seed (color, menuItem, category, sort, property `"1000"`, `deviceName ""`) | `create` with `CreateOptions::replay_conversion` (default) | verified, including page layout |
| Menu rows from master (`Class & 0xFF`, U+FFFA/U+FFFB names, `Disable` bits) or the built-in defaults | `MenuSeed::from_master`, `MenuSeed::defaults` | verified (from master); defaults from code |
| `get_next_id` max+1; name lookups; album with album artist and compilation flag | `get_or_add`, `get_or_add_album` | verified |
| `addTrack` lookup order, `insertTrack` column set, NULL/0/'' and widths | `add_track(&NewTrack)` | verified (157/157 rows) |
| `numberOfContents` refresh | `add_track`, `update_number_of_contents` | verified |
| Image `max+1`, path `%05d` folder `id/20+1`, owner `image_id` update | `add_image` | verified (95/95) |
| Playlist create at end + move to position, folders attribute 1 | `create_playlist`, `move_playlist`, `find_playlist` | verified (2/2, incl. the move) |
| `playlist_content` 1-based by count | `append_playlist_entry`, `clear_playlist` | verified (162/162) |
| My Tag tree rewrite, `myTagMasterDBID` | `write_my_tags`, `clear_my_tag_content`, `add_my_tags_to_track` | verified (28/28 tree rows; links from code) |
| Re-export: duplicate check, markers, full `Analysed`, `contentLink` merge | `find_exported_track`, `track_markers`, `set_track_updated`, `set_analysis_updated`, `reexport_content_link`, `set_content_link` | verified (5/5 rows) |
| `modify*` for plain and lookup columns (0 for none, `delete_unused_record`) | `modify_column`, `modify_lookup`, `delete_unused` | hypothesis (code only) |
| Histories (`get_next_seq` from 1), history entries | `add_history`, `add_tracks_to_history` | hypothesis (code only) |
| Cues (`setHotCue` + `addCueOption2`), hot cue bank lists and cues | `add_cue`, `create_hot_cue_bank_list`, `add_hot_cue_bank_cue` | hypothesis (code only) |
| `recommendedLike` | `add_recommended_like` | hypothesis (code only) |
| Device Library Plus conversion of an existing export.pdb: `initPropertyTable` seed, `clearAllData`, then one multi-row insert per table (`insertAllDev*`) | `create_for_conversion`, `insert_all` (rows mapped by `mb_usb::convert`, `export.md` § 6.4) | verified (reference stick, synthetic round trip) |
| Clean close: checkpoint, no `-wal`/`-shm` | `close` (errors if a side file remains) | verified |

The writer does not implement:
- track removal (`removeTrack` @0x100718AEC);
- list deletion with renumbering (`deleteList`, `delete_and_renumber`);
- `checkAndSetChanges` (which fields `modifyTrack` rewrites on a re-export);
- the file side of an export: audio, ANLZ and artwork JPEGs.

**Verification: rebuild of oracle-a.** `cargo run -p mb-devlib --example devlib_rebuild -- fixtures/oracle-a fixtures/oracle-a/rbtest <out.db>`, also run as the test `tests/writer.rs::oracle_a_rebuild_matches_rekordbox`.
- **Replay.** The example replays both exports from `master.db`: `oracle-small` (5 inserts), then `oracle-all` (152 inserts, 5 updated in place, 4 refused by the format gate: WAV at 22.05 kHz).
- **What it takes from the stick.** Only the export-layer data: the device audio and ANLZ paths per `masterContentId`, the playlist export order, and `createdDate`.
- **Result: all 22 tables identical row by row and column by column, storage classes included:**
  - content 157, genre 14, artist 93, album 97, label 45, key 22, color 8, playlist 2, playlist_content 162, image 95, menuItem 27, category 22, sort 17, property 1, myTag 28;
  - the other 7 tables are empty on both sides.
- **Schema and settings.** The 26 `sqlite_master` entries are identical, root pages included. `page_size` 4096, `journal_mode` wal, `user_version`/`application_id`/`auto_vacuum` 0, UTF-8, the cipher settings, `page_count` 41 and `freelist_count` 0 are all equal.
- **Raw pages, decrypted (scratch tool):** 39 of 41 are byte-identical.
  - Page 1 differs only in the header's file change counter, version-valid-for and SQLite version number (3051003 here, 3045003 in rekordbox's SQLCipher 4.6.0).
  - Page 30, the first `content` leaf, holds the same 15 cells at other offsets: rows 1–5 were rewritten in place by rekordbox's second, untraced export, whose exact UPDATE statements are unknown.

## What a writer must reproduce
1. **Encryption:** SQLCipher 4 defaults with `PRAGMA key = passphrase()`. There is no plaintext header and no custom KDF settings.
2. **Pragmas:** `synchronous=NORMAL` and `journal_mode=WAL` (the header stays in WAL mode). Leave `page_size` 4096, `user_version` 0 and `auto_vacuum` none. Close cleanly so the checkpoint runs and `-wal`/`-shm` disappear before the stick is unplugged.
3. **DDL:** create all 22 tables in rekordbox's order with its exact DDL text (`devlib/schema.sql`), plus the 4 indexes. rekordbox always creates every table, even when it stays empty; whether players need that is untested.
4. **Initial rows:**
   - one `property` row (`dbVersion` "1000" and `deviceName` "" on a fresh stick, see § property; `createdDate` today; `numberOfContents` kept equal to `count(content)`)
   - `color` 1…8
   - `menuItem` / `category` / `sort` (copy them from master.db like rekordbox, or use the defaults above)
5. **IDs:** integer IDs allocated as max+1. playlist_content and history_content `sequenceNo` are 1-based. Playlist and hot cue bank list siblings are 0-based (a new list goes to the end, then moves to the requested position: 0 for an export). History siblings start at 1.
6. **Paths:** device paths start with `/` and are relative to the stick root. Artwork goes in `/PIONEER/Artwork/%05d/b<id>.jpg` (folder `id/20+1`), together with an 80×80 `b<id>.jpg` and a 240×240 `b<id>_m.jpg`. `analysisDataFilePath` must point at the copied ANLZ files.
7. **Unknown lookups:** match 7.2.19 by writing NULL (not 0) for the optional artist/album/genre/label/image columns, and leave `titleForSearch` NULL.
8. **Cues:**
   - `inUsec`/`outUsec` are in µs, with -1 for no out point.
   - Fill the 1/150 s frame columns and the MPEG frame/offset columns as rekordbox does for master `djmdCue`. The seek info for non-PCM formats needs more RE.
   - The `kind` codes are listed in the cue section.
9. **Change markers:** keep `masterDbId`/`masterContentId` and the three `*UpdateCount` markers consistent with the source library, so that rekordbox's own re-export can recognise the tracks.

## Reader notes (`mb-devlib`)
- **Storage is loosely typed,** so `mb-devlib` coerces each value it reads:
  - NULL vs 0 for "none"
  - `''` text in integer columns
  - integers stored where text is expected
- **Opening:** `DevLibDb::open_read_only` uses a normal read-only connection, which still creates `-shm`/`-wal` for a WAL database. `DevLibDb::open_immutable` uses `immutable=1` and touches nothing, so use it on sticks.
- **Snapshot:** `DevLibDb::load_snapshot(stick_root)` maps onto `mb_core::LibrarySnapshot`. Device paths are joined onto the mount point. The device-only data (cues, My Tags, histories, hot cue banks, menus, change markers) is in `mb_devlib::DeviceLibrary` (`load_library`).

## Open questions
- [ ] `cue.kind` 17 and 101: which slots or features they are (hot cue bank? a 17th hot cue?), and the A…P slot mapping on 16-hot-cue players.
- [ ] `colorTableIndex` vs the Cue+0x31 "colour" byte, and whether players read `colorTableIndex` against the hot-cue palette.
- [ ] Semantics of `contentLink` bits other than bit 0, and of `hasModified`.
- [x] `myTagMasterDBID`: master DBID + 1, or the cloud DBID when cloud sync is set up (`syncMyTagLocal`, resolved 2026-10-12).
- [ ] `backGroundColorType` value range and its palette.
- [ ] `history.attribute` values, and how players write history (no samples yet).
- [ ] When `/.PIONEER` is used instead of `/PIONEER` for artwork.
- [ ] Whether players care about `dbVersion`. A 7.2.19 stick ends with "1000" through the conversion; "10000" only survives where no conversion runs.
- [x] Full insert order for an export, and which statements run inside transactions: ops/export-devlib.md (autocommit everywhere except `move_sequence` and `setHotCueBankCue`).
- [ ] What `checkAndSetChanges` / `modifyTrack` rewrites on a re-export. Untraced, and the only residual raw-page difference in the oracle-a rebuild.
- [ ] RowDataTrack+0x50, the row kind that gates the 0x10000/0x100000 `contentLink` merge: 1 and 6 are taken to be collection rows (*hypothesis*).
- [ ] master `djmdContent.Lyricist`: taken to be a plain name (it is a string in RowDataTrack+0x458 and in `DevAddContent_`). The corpus has no lyricists.
- [ ] Whether `smart` playlists are exported as plain lists (`attribute` 0): the fixture has none.
