# USB export and sync (rekordbox 7.2.19)

Status: **per-track pipeline, destination paths, artwork and incremental-update rules verified** (binary, and on the fixture stick where the stick can show it: 704/704 paths, 1404/1404 artwork files, 704/704 ID matches). **Settings files verified** (layout and CRC on all four fixture files). **Device detection and DB creation verified** (binary); why the fixture stick changed without an export is a **hypothesis** (§ 6.3). Player acceptance has not been tested on hardware; every rule here describes rekordbox's behaviour.

Status words: **verified** (binary and fixture agree, or binary alone when the stick cannot show it, as marked), **hypothesis** (fits the evidence, not proven), **open** (not known).

All addresses are for the rekordbox 7.2.19 arm64 slice (`~/re-work/bin/rekordbox-arm64`). Related specs: `pdb.md` (export.pdb / exportExt.pdb format), `devlib.md` (exportLibrary.db), `anlz.md` (ANLZ files and the USB folder hash), `masterdb.md` (source library).

## Vocabulary
- **Device handle**: rekordbox keeps one `rekordboxDBController` per database on a device. A stick that has both databases gets **two handles**: one for `export.pdb` (classic, DeviceSQL) and one for `exportLibrary.db` (Device Library Plus, SQLCipher). `DatabaseMediator::getSameDeviceDBList(handle)` @0x1010debd0 returns the other handles on the same physical device (a map at mediator+0x690). Handle 1 is the mobile (rekordbox for iOS/Android) device; handle 0 is the master library.
- **Controller fields** used by the export (verified from their uses):

| Offset | Meaning |
|---|---|
| +0x40 | byte: My Tag export enabled for this device (set by `syncMyTag`, § 2.5) |
| +0x44 | int: DB kind. 1 and 2 are device DBs (DeviceSQL, or SQLite when +0x188 is set); 3 is the cloud-library DB, which goes through the agent (`CloudAgentAPI::Transaction`); 0 is the master |
| +0x58 | DB file path (passed to `dsqlOpenDeviceDB`) |
| +0x60 | device root (mount point) |
| +0x68 | the device's `PIONEER` folder (`<root>/PIONEER` or `<root>/.PIONEER`) |
| +0x70 | byte: use `/.PIONEER` instead of `/PIONEER` in device paths (artwork, ANLZ) |
| +0x188 | byte: this handle is Device Library Plus (`DatabaseMediator::isDeviceLibraryPlus` @0x1010deb40 returns it) |

- The classic DB is driven by the base `rekordboxDBController`. Its virtual `insertTrack` is `dsqlAddTrack` (@0x10141f4ac → `db::dsqlAddTrack` @0x1019a78d0). The Plus DB is `DevSqliteDBController`, which overrides the SQL methods (`devlib.md`).

## Export algorithm (summary)
Exporting N tracks and some playlists to a stick, as rekordbox does it:

1. **Open or create the device DBs** (`createDeviceDB(handle, true)` @0x100ccb5a4; § 6.3).
   1. Write-probe the stick.
   2. Create `PIONEER/rekordbox/export.pdb` + `exportExt.pdb` with the property, colour, menu, category and sort rows.
   3. Write `DeviceLibBackup/rbDevLibBaInfo_<DBID>.json`.
   4. Create `exportLibrary.db` (tables, indexes, initial rows).
   5. The browser then syncs the My Tag tree and, when the preference is on, the settings files.
   6. Check `isDBAccessible`.
2. **Warn about the file system** (`getDriveFileSystemType` + `showExportWarningIfNeed` @0x1009c07e0) and **filter the tracks** (`check_support_export` @0x1009bfce4); see § 6.
3. **Build jobs.** Every playlist (or loose track set) becomes one `ExportDescription`.
   - Playlists are matched on the device by name and parent (§ 1.7). An unchanged device list is skipped; a changed one is emptied and refilled.
   - Folders are created right away (`exportPlaylistFolder`).
   - Each device gets one background thread (`DbBackGroundProcess`, "Exporting"). Jobs for a device that already has a running thread are appended to it.
   - When a new thread is created, the local `djprofile.nxs` is copied to `<PIONEER dir>/djprofile.nxs` (§ 5).
4. **Mark playlist exports in progress.** `PIONEER/rekordbox/sync.temp` (classic) or `sync.devlibplus.temp` (Plus) is created at the start, and deleted when the last job has finished all its tracks (§ 4.5).
5. **For each track, in playlist order** (`DbBackGroundProcess::run` @0x1001d36fc → `execExport` @0x1009b543c):
   1. Load the source row. Resolve cloud/streaming sources; check that the source file exists and its format is allowed.
   2. If the track is not analysed, or its ANLZ file is missing or from an old analyser version: queue an analysis, wait 1 s and retry the same track. **The export never computes analysis itself**; it repacks the local ANLZ files.
   3. Look for the track on the device (§ 4.1). If it is there, update the row in place, and redo ANLZ and artwork only if they changed (§ 4.2).
   4. If it is new: check free space (≥ 1.5 × file size), build `/Contents/<artist>/<album>/<file>` (§ 1.3), create the folders, and copy the audio. The audio is not copied when another DB on the same stick already has this track; that DB's path is reused.
   5. Compute the ANLZ path (`/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZnnnn.DAT`, § 1.6). Insert the track row (`export_track_data_to_db` → `addTrack`): artist, album, genre, label and key IDs are looked up or added first.
   6. Copy the artwork: two JPEGs per track and one image row (§ 3).
   7. Sync "like" relations and My Tags for the track.
   8. Write ANLZ `.DAT`, `.EXT` and `.2EX` from the local analysis and the current cues/beat grid. Then give every cue a device cue ID in memory (no `cue` rows are written, § 1.5).
   9. Repeat steps 3–8 for each other DB on the same stick (DB rows, artwork rows and cue IDs only; no second audio copy and no second ANLZ write).
   10. Append the track to the target device playlist in every DB (`add_track_to_device_playlist`).
6. **After the last track** of a job, delete the job's `sync*.temp` marker. `onExportFinished` @0x1009c8508 then deletes the local `<PIONEER dir>/export/temporary/` folder (downloaded cloud files).
7. **Sync bookkeeping:** on a stick where Sync Manager is set up, the device playlist edits also update `playlists3.sync` / `playlists3Plus.sync` (§ 4.4).
8. The DBs stay open while the stick is mounted. They are closed on eject or quit (§ 6.3 step 8): for exportLibrary.db that is the WAL checkpoint; for export.pdb, `TE_DPgm_Close` writes the header (state 5, new transaction ID; `pdb.md`). **This close is the last write of the whole flow.**

## 1. Order of operations

### 1.1 Entry points and the job queue (verified)
- **UI entry points:** `DatabaseIF::exportTrack` → `DatabaseMediator::exportTrack` @0x1009c4d88; `exportPlaylist` @0x1009c5c74 (also used for iTunes lists, browse type 3); `exportPlaylistFolder` @0x1009c5864; `exportPlaylistRecursively` / `exportPlaylistTrackRecursively`; `exportCollectionTrack` / `exportCollectionList`; `exportFile`. All of them end in `export_track` @0x1009bea70 or `export_list` / `export_list_track` @0x1009bf1d4 / 0x1009c2b60. Those build jobs. The only writes they do themselves are playlist/folder rows and their artwork, playlist clearing (§ 1.7) and the marker files (§ 4.5); every track file and track row is written by the background thread.
- **`export_track`** (loose tracks) runs these steps:
  1. `createDeviceDB(handle, true)` and `isDBAccessible(2, handle)`.
  2. If the device is not a cloud DB: `getDriveFileSystemType(root)` → `showExportWarningIfNeed(fs)`.
  3. `check_support_export(trackIds, handle, isiOS, &flag)` drops tracks the device cannot take.
  4. Build an `ExportDescription(handle, trackIds, playlistId, name, …, true)` (@0x1001d5a64). For every other DB on the same device it looks up the same-named list and records it with `setSameDevDbDesc`.
  5. Append the job to the device's running `DbBackGroundProcess`, or start a new one. One thread runs per device handle and the thread list is at mediator+0x748. A running process accepts new jobs unless it is cancelled (+0x2A4) or finishing (+0x2A5).
- **`ExportDescription`** fields:

| Offset | Meaning |
|---|---|
| +0x08 / +0x1C | next track index / track count |
| +0x10 | track ID array |
| +0x50 | kind: 2 = tracks or playlist, 4 = hot cue bank list (`execExportHCBLCue` @0x1009bd6ec) |
| +0x54 | device playlist ID (0 = no playlist) |
| +0x58 | playlist name |
| +0x70 | marker file to delete when the job is done (§ 4.5) |
| +0x98 | `IsSameDevDbExport`, default **1**: also write to the other DBs on the same stick |
| +0x99 | `IsSyncManager`, default 0 |

- **`DbBackGroundProcess::run`** @0x1001d36fc:
  - For each job and each track it calls `execExport(trackId, srcHandle, dstHandle, result, sameDevDb = desc+0x98, syncMgr = desc+0x99, map, thread)`.
  - Per-track results:

| Code | Meaning | What run does |
|---|---|---|
| 0 | exported (new) | next track |
| 1 | already on the device and unchanged | next track |
| 10 | must be analysed first | retry the same track after 1 s |
| 7 | device DB gone | message 10 to the browser, stop |
| 11 (0xB) | not enough space | message 9 to the browser, stop |

    The other codes are errors recorded in `TrackAdditionResult`: 3/8 copy failed, 4 source problem, 5 unsupported format, 9 ANLZ write failed, 12 DB insert failed.
  - When code < 2 and the job has a playlist, the track is appended to that device playlist in this DB and in every same-device DB (`add_track_to_device_playlist` @0x100c4d8d4).
  - The browser is refreshed (`notifyTrackListModified`) at most every 5 s.
  - When the job's last track is done, its marker file (+0x70) is deleted and `onExportFinished` runs.

### 1.2 Per-track pipeline: `execExport` @0x1009b543c (verified)
Parameters: `(trackId, srcHandle, dstHandle, result&, sameDevDb, syncMgr, map<handle, devTrackId>&, thread)`.

1. **Load the source row** (`getRowDataTrack`, vtable +0x48) from the master, or from a source device.
2. **Cloud and streaming sources.**
   - Cloud-only tracks are downloaded first to `<PIONEER dir>/export/temporary/` (`exportTemporaryFilePath`, through `CloudManager`).
   - Streaming tracks (`StreamingManager::getStreamingType` ≠ 0) are refused with error 5 or 16.
3. **Missing source file** → error 4, and the row is marked missing (`changeFileExistence`).
4. **Format gate**, on master `FileType` (RowDataTrack+0x3B8):
   - For the mobile device: only some types, and only 44.1/48 kHz.
   - For a normal device: FLAC (5), ALAC (6), WAV (11) and AIFF (12) are rejected unless the sample rate is 44100, 48000, 88200 or 96000, but only when bit 1 of the row flags (+0x3F8) is set. Type 64 is always rejected.
   - An `.m4a` (type 4) is re-probed with TagLib to tell AAC from ALAC.
   - Flag bit 1 is the "analysed" bit: the analysis gate (step 5) treats a clear bit as "not analysed". So the sample-rate check only applies once the track has been analysed and its sample rate is known; an unanalysed track is analysed first and checked on the retry (*hypothesis* for the intent, verified for the bit use).
5. **Analysis gate** (the result is code 10, so the background thread retries the track):
   - Row flag bit 1 clear (not analysed): queue a `DBCommandAnalyze` with priority (the list at mediator+0x730), retry.
   - The local `ANLZ0000.DAT` is missing: `analyzeCollectionTrack`, retry.
   - The 3-band flags are set but the local `.2EX` is missing: clear the 3-band flag (`clear3BandAnalyzedWA`) and go on.
   - `isAnalyzedOldVer`: `requestMissingAnalysis` (vtable +0x4A8) plus an analysis, retry.
6. **`$_0` @0x1009b74f8 builds `ExportInfo`.** It holds:

| Offset | Content |
|---|---|
| +0x00 | device root |
| +0x08 | the hidden-folder flag (controller +0x70) |
| +0x10 | the device row |
| +0x4F0 | track already on the device |
| +0x4F1 | ANLZ must be rewritten |
| +0x4F4 | result |

   It finds the track on the device (§ 4.1). If found, it runs `overwrite_export` (§ 4.2) and `export_art_when_overwrite` (§ 3).
7. **New track:** `export_track_data` @0x1009b8ca4. When syncMgr is set, the call holds a static lock.
   1. **Space check:** `File::getBytesFreeOnVolume(root)` must be at least 1.5 × the source file size, otherwise error 11. A zero volume size gives error 7.
   2. **Path:** if another DB on the same stick already holds this track (`isCollectionSongForDuplicateSongId` there), its path is reused. Otherwise `calc_device_file_path` (§ 1.3, § 1.4), then `File::copyFileTo(src, root + path)`, falling back to `FileTools::copyMusicFile`. Failure gives error 8.
   3. **ANLZ path:** reused from the same-device DB if it has one, otherwise `calc_device_analysis_path` (§ 1.6).
   4. **`export_track_data_to_db` @0x1009b841c:** copy the source row into a device row and rewrite these fields:

| Field | New value |
|---|---|
| path (+0x98) | the device path |
| file name (+0xA8) | the part after the last `/` |
| analysis path (+0x3F0) | the device ANLZ path |
| artwork path (+0x3E8) | `""` (set later by `export_artwork`) |
| image ID (+0x404) | 0 |
| flags (+0x3F8) | `&= 0xB3` |
| +0x3FC | `&= 0x3F` |
| `contentLink` (+0x400) | `(src.contentLink & 0xC1600) \| 0x100` |
| `masterDbId` (+0x408) | `getMasterDBID(src)` |
| `masterContentId` (+0x40C) | `getMasterContentID(src)` |
| My Tag array (+0x440) | cleared |
| `EditInfo` property | cleared |
| update markers | the source's (`getUpdatedInfo`, vtable +0x3F0, into +0x428/+0x420/+0x418) |

      Then `addTrack` (vtable +0x430); failure gives error 12. Fixture check for `contentLink`: every row has 0xC0700 = (0xC0700 & 0xC1600) | 0x100.
   5. If the source has artwork: `export_artwork` (§ 3).
8. **Same-device, all handles:** `$_3` @0x1009b7bb4 syncs "like" relations (`removeTrackRelation` / `addTrackRelation`, written to exportExt.pdb `DJDBEXRECOMMENDLIKE` or Plus `recommendedLike`). `$_4` @0x1009b7fa4 runs `replaceMyTagOnTrack` (vtable +0x518), but only when controller+0x40 is set (§ 2.5).
9. **ANLZ:** `$_5` @0x1009b8248 → `export_analysis_data` (§ 1.6), unless syncMgr is set and the target is Plus. In that case `$_7` @0x1009b8054 only assigns device cue IDs in memory (`getNewDevCueId`, skipping kind 101) and sets the cached track status (0xC2/0xC3). The classic pass of the same sync has presumably already written the ANLZ files (*hypothesis* for the ordering of the two handles).
10. **Cloud-library DB only (kind 3):** `$_6` @0x1009b82b8 calls `setCue` for every cue.
11. **Same-device copy** (when sameDevDb is set): for every handle from `getEnableSameDeviceDBList(dst)`, run `$_0` again for that DB. If the track is new there, `export_track_data_to_db` runs with the primary path and ANLZ path (the root prefix is cut with `fromFirstOccurrenceOf`), then `export_artwork`, `$_3`, `$_4` and `$_7` (cue IDs and status only). The device track ID per handle goes into the caller's map.
12. `Analytics::Helper::countExportStorageType`, then `setTrackAdditionSuccess`.

### 1.3 Destination path under `/Contents/` (verified: code + 704/704 stick paths)
`get_device_file_path_candidate` @0x1009ba39c builds the directory and the file name. `calc_device_file_path` @0x1009b6e0c then handles name clashes.

0. **The source already lives on this device:** if its path starts with `<root>/` and `hasSpecialCharInFilePath` @0x1009baeb8 is false, the path is kept as it is and nothing is copied (function returns 0). That check walks every component below the root and fails on one that contains an inhibited character or ends with `.`.
1. **Artist directory**, from the track artist name (RowDataTrack+0xD0):
   1. If the name is empty, use `UnknownArtist`. Otherwise `trim()` it, then replace every **inhibited character** with `_`.
   2. If the result ends with `.`, replace that last `.` with `_` (fixture: `Bedran.` → `Bedran_`, `P.R.O.G.` → `P.R.O.G_`).
   3. Convert to NFC (`convertToPrecomposedUnicode`).
   4. Take at most **48 characters** (`String::append(s, 48)`), `trim()` again, then strip trailing `.` characters (`trimCharactersAtEnd(".")`).
   5. Create the directory. If that fails, use `UnknownArtist`.
2. **Album directory:** the same steps on the album name (+0x108), with `UnknownAlbum` as fallback.
3. If `<root>/Contents/<artist>/<album>/` cannot be created, the whole directory becomes `/Contents/UnknownArtist/UnknownAlbum/`.
4. **File name:** the source file name without extension, with inhibited characters replaced by `_`, plus the source extension (with its dot, original case), converted to NFC.
   - The leading/trailing `.` rule and the trim do **not** apply to file names (fixture: `Bedran. - Every Time (Original Mix).aiff`).
   - If the name is longer than 48 characters: `name = full.substring(0, 48 − len(ext)) + ext`, where `ext` = the text from the last `.`, dot included. For example `Ajja, Alpha Portal - The Other Side (Origin.aiff` is 43 + 5 = 48.
5. **Result:** `/Contents/<artist>/<album>/<file>`. The device path starts with `/` and is relative to the stick root. It is used verbatim in the DB `path`, the ANLZ PPTH, and the ANLZ folder hash.

- **Inhibited characters** (`db::inhibitedCharacters(false)` @0x1004dee8c): `/ \ : ? * " < > | ~`. The `true` variant adds `¥` (U+00A5) and is not used here. Each character is replaced 1:1 by `_` (`replaceInhibitedCharactersWithUnderscores` @0x1004ded7c). Fixture: `Who Knows?` → `Who Knows_`, `Alert / Fog` → `Alert _ Fog`, `Culture: Psy` → `Culture_ Psy`, `"der turnbeutel"` → `_der turnbeutel_`.
- **Lengths are counted in Unicode code points** (juce `String::length`), after NFC.
  - All 704 stick paths have NFC components of at most 48 code points.
  - Five names read back longer (49/50) only because macOS's FAT driver returns them in NFD.
- **Checked:** artist and album directory names were recomputed from the export.pdb artist/album rows for all 704 tracks. All 704 artist directories and 702 of 704 album directories match. Neither album miss is a rule failure: one track's album was renamed after the export (its directory is still `UnknownAlbum`), and the other comparison failed only on `\"` escaping in the dump output.
- **Characters that are kept:** apostrophes, `&`, `!`, `#`, `,` and non-ASCII letters. There is no FAT 8.3 or charset sanitising beyond the list above.

### 1.4 Duplicate file names (verified: code + fixture)
`calc_device_file_path` @0x1009b6e0c runs this only when `<root>/<dir>/<name>` already exists:

- `ext` = from the last `.` (dot included); `base` = before the last `.`. Try `i = 1, 2, …`, with `suffix = "-" + i`:
  - If `len(suffix) + len(name) ≤ 48`: the candidate is `base + suffix + ext`.
  - Otherwise: the candidate is `base.substring(0, 48 − len(suffix) − len(ext)) + suffix + ext`.
- The first candidate that does not exist wins.
- **Fixture:** the same audio, imported twice into the master (content IDs 82728718 and 135670776, identical SHA-256), became `…/Inside/Distinctside - Inside (Original Mix).aiff` (ID 654) and `…(Original Mix)-1.aiff` (ID 665).
- Each path then hashes to its own ANLZ folder (P065/00012879 and P03F/0000EFED).

### 1.5 Which databases are written, and when
- **Track rows** (verified) are written per track, inside the background thread:
  - in the primary DB first, then in every same-device DB.
  - export.pdb: `dsqlAddTrack` → `DevAddContent_`, a DeviceSQL stored procedure (`_DevAddContent` @0x1012ee110).
  - exportLibrary.db: `DevSqliteDBController::insertTrack` @0x10070562c, one INSERT per track.
- **Order inside the row write** (`addTrack` @0x1017e1ee4; verified):
  1. `getAndAddArtistID` for the artist (+0xD0).
  2. Genre (+0x180).
  3. Album (+0x108), with the compilation flag (+0x120), the album-artist name (+0x148) and the artist ID.
  4. Artist IDs for the remixer (+0x300), original artist (+0x290) and composer (+0x338).
  5. Label (+0x1B8) and key (+0x210).
  6. `insertTrack`.
  - The lookups never run for empty names, which is why the Plus rows have NULL for missing IDs.
- **Other tables:**
  - The image row is added in `export_artwork` (§ 3).
  - Playlist rows are created when the jobs are built (§ 1.7); playlist entries are added after each track.
  - My Tag links are per track (`replaceMyTagOnTrack`). The My Tag tree is written separately by `syncMyTag` (§ 2.5).
  - Relations go to exportExt.pdb / `recommendedLike`.
- **Device cue rows** (exportLibrary.db `cue`):
  - **The export writes no cue rows** (verified: code + fixture). Cues are only put into the controller's in-memory cue cache (`addCueInfoToCache` @0x1017f6274) with fresh device cue IDs.
  - The "track status" 0xC3/0xC2 is not stored either: `setTrackStatus` @0x1017f60dc only patches the flags word (+0x3F8) of the controller's cached row, for the browser.
  - Fixture: 73 of the 84 tracks inserted by the 7.x path (IDs 621–704) have 132 DAT and 248 EXT cue entries in their ANLZ files, yet `cue` has 0 rows. **On an exported stick the cues live only in the ANLZ files.**
  - The `cue` table is written by device-side cue edits (`setHotCue`, `addCueOption2`; `devlib.md`).
  - export.pdb has no cue table; its cues live in the ANLZ files.
- **Transactions and flushing:**
  - exportLibrary.db runs in WAL mode with `synchronous=NORMAL`. Each statement auto-commits unless a caller opens `transactionStart` (`devlib.md`).
  - export.pdb goes through the DeviceSQL page manager. The header transaction ID and state are written by `TE_DPgm_Close` (`pdb.md`).
  - The export path itself has no explicit "flush everything" step. `onExportFinished` only deletes the temporary folder.
  - The files are closed, and so finalised on disk, only at eject or quit (§ 6.3 step 8).

### 1.6 ANLZ files (verified; formats in `anlz.md`)
**Path** (`calc_device_analysis_path` @0x1009b9c5c):
1. `make_anlz_file_path` (`CreateAnlzFileFolderPath` hash of the device path) gives `/PIONEER/USBANLZ/P%03X/%08X` (or `/.PIONEER/...` when controller+0x70 is set).
2. The function lists that folder on the stick. The file number is **one more than the highest `ANLZxxxx` hex number found there**, or `0000` for an empty folder: `ANLZ%04X.DAT`.
3. Backslashes are normalised to `/`.
- Fixture: folder `P046/00014454` holds `ANLZ0000` and `ANLZ0001` for two tracks.

**Write** (`export_analysis_data` @0x1009bbf04), right after the row insert and artwork:
1. Read the source cues (`getContentCueInfo`, vtable +0x438) and validate them (`checkCueInfos`).
2. `getExportCue` @0x1009bc884 splits them into DAT hot/memory lists and EXT extended lists. `getExportBeatGrid` @0x1009bcf3c builds the DAT and EXT beat grids.
3. Create the folder. **For a new track,** first `CAnalyzerIF::removeOldAnalysisFiles(devicePath, folder)`: it opens every `*.DAT` in the folder and deletes the DAT/EXT/2EX set whose PPTH equals the new device path (case-insensitive). Those are stale files left by an earlier export of the same path.
4. `analyzer::ExportAnalysisData(local .DAT → device .DAT, cues, grid)` @0x1007b7c0c.
5. `ExportAnalysisExtendWithCue(local .EXT → device .EXT, ext cues, ext grid)` @0x1007b83fc.
6. `ExportAnalysisExtend2(local .2EX → device .2EX)` @0x1007b9a04.
7. Assign device cue IDs in the in-memory cue cache, and set the cached track status to 0xC3 (has cues) or 0xC2. Neither reaches a file (§ 1.5).
- Any failure gives result 7 (no device DB) or 9.
- **Local `.3EX` files are not exported.**
- **Mobile target:** each file is pushed with `requestRBMSendFile`.

### 1.7 Playlists (verified unless marked)
- **Matching, not ID reuse.** For each master list, `exportPlaylist` calls `existsListName(2, parentDevId, name, &isFolder, &devId)` (vtable +0x308) on the device DB.
  - **Device list with the same name under the same parent:**
    1. Its tracks are read (`getPlaylistTrack`) and mapped back to master content IDs (`getDevContentIDByMasterTrack`).
    2. If the sequence equals the master list's, the list is **skipped**.
    3. Otherwise it is emptied (`removeAllTracksFromPlaylist`, vtable +0x368) and refilled by the background job in master order.
  - **No such list:** a new one is created (`createNewList`, vtable +0x328).
- **Folders** (`exportPlaylistFolder` @0x1009c5864):
  - A folder with the same name and parent is reused.
  - A *playlist* with that name is deleted first, together with its artwork. Its tracks go too when the "delete tracks" preference is on (§ 4.3).
  - Then the folder is created with attribute 1.
- **Limits:**
  - Lists plus folders on a device: at most 1000. `getNumOfListAndFolder(2) > 999` gives error 4.
  - Tracks per playlist (`get_device_playlist_track_num_max` @0x1009c1734): **10000**, or **1000** for the mobile device and when the setting `dbConversionDevice` is 11 or 12.
  - When a list grows past 1000 entries on a 10000-limit device, `add_track_to_device_playlist` @0x100c4d8d4 shows "Over 1000 tracks cannot be displayed in one playlist depending on the equipment you use." So some players read only the first 1000 entries (*hypothesis*: from the message text only).
  - When a track is already in the target list, the user is asked to add or skip duplicates; the answer applies to the rest of the job.
- **Playlist artwork** (Plus `playlist.image_id`): `write_artwork_file(…, 4, …)` / `delete_artwork_file` for lists that have an image.
- **Entry order:** entries are appended one by one as each track finishes, so a list's order is the master order. Entry `sequenceNo` is 1-based (`devlib.md`, `pdb.md`).
- **iTunes lists:** browse type 3 → `exportiTunesPlaylist`, with marker file `sync.itunes.temp`.

## 2. IDs on the device

### 2.1 Track IDs (verified)
- **Both DBs allocate `max(id) + 1`.**
  - Plus: `insertTrack` runs `select max(content_id) as maxId` and uses maxId + 1.
  - Classic: the ID comes back from the `DevAddContent_` procedure.
- **Fixture:** export.pdb track, artist, album and image IDs are contiguous `1..N` with no gaps, even though the table has 1159 slots for 704 live rows (deleted row versions). For all 704 tracks the export.pdb `ID` equals the exportLibrary.db `content_id` and the `IMAGEID`s are equal too, because both DBs are always written in lock-step.
- **A writer should still keep them equal.** The Plus conversion copies the classic IDs (§ 6.4), and lock-step exports keep them equal. But rekordbox itself never relies on it: it maps between DBs by master IDs (§ 4.1), and the same-device map in `execExport` stores one device ID per handle. Playlist IDs already differ (§ 2.3).
- **New stick:** IDs start at 1. **Existing stick:** they continue at max + 1, and existing rows keep their IDs because re-exports update in place (§ 4.2). The fixture shows this: IDs 1–620 date from 2020–21, and 621–704 were appended in 2025–26.

### 2.2 Artist, album, genre, label, key and colour IDs (verified for the Plus DB, hypothesis for the classic DB)
- `getAndAdd{Artist,Genre,Album,Label,Key}ID` (vtable +0x858/+0x850/+0x860/+0x870/+0x868) select the row by exact name. Only when nothing matches do they insert `max + 1` (`devlib.md`).
- Album lookups also take the compilation flag, the album-artist name and the artist ID.
- Colour IDs 1..8 are copied from master `djmdColor` when the DB is created.
- The classic DB uses DeviceSQL procedures. Its IDs are contiguous in the fixture (artists 1..572, albums 1..605), which fits the same rule.

### 2.3 Playlist IDs (verified)
- Plus: `playlist_id` is max + 1. Fixture: 16391, 16392 and 16393 follow the player-created "TAG LIST" lists at 16385+.
- The classic DB has its own numbering (fixture: 1..7 plus 16385..16408).
- **The two DBs' playlist IDs differ** for the same list: "NEU" is 1 in export.pdb and 16392 in exportLibrary.db. The sync files keep one Dev_ID per DB (§ 4.4).

### 2.4 Artwork IDs (verified)
- Classic: `dsql_get_next_image_id` @0x1019a8a40 sorts the image table by ID descending and takes the first + 1, or 1 for an empty table.
- Plus: max + 1 (`devlib.md`).
- One image row per exported track; there is no de-duplication. Fixture: 702 images for 704 tracks, and the same album cover is stored up to 7 times.

### 2.5 Relation to master.db (verified)
- **`masterDbId`** = `RowDataTrack::getMasterDBID` @0x1004e0d2c: the source's `MasterDBID` column if non-zero, otherwise the source library's DBID (`djmdProperty.DBID`). It is written as an unsigned 32-bit value (fixture 2328226057). The sync files print the same value as signed i32 (−1966741239).
- **`masterContentId`** = `getMasterContentID` @0x1004e0d44: the source's `MasterSongID` if non-zero, otherwise the source `djmdContent.ID`.
- Tracks that came into the library from another one (for example imported from a device) therefore keep pointing at their original library.
- **`myTagMasterDBID`** (Plus `property.myTagMasterDBID`; classic exportExt.pdb `DJDBEXDBPROPERTYOPTION.MYTAGMASTERDBID`) = **master DBID + 1**. `DatabaseMediator::syncMyTagLocal` @0x100c52988 computes `*(mediator+0x528) + 1` and passes it to `syncMyTagList` (vtable +0x510 @0x1017f273c), which writes the tree. Fixture: 2328226058 = 2328226057 + 1.
  - `syncMyTag` @0x100c51eec compares the device value: 0 → write; DBID + 1 → in sync; and it enables My Tag export (controller+0x40).
  - If the device value is the plain DBID, it asks to "resync My Tag after starting rekordbox 6".
  - Any other value ("synced with another computer") prompts to overwrite.

## 3. Artwork (verified: code + 1404 fixture files)
**Local sources.** The library keeps per-track artwork in four sizes, made at import by `CAnalyzerIF::writeFileImage` @0x101623e30. Each `write_image` @0x1016235e4 is a JUCE `JPEGImageFormat` with default quality, which is 85 in JUCE, and the alpha channel dropped.
- `<art>.jpg`: at most 800 px, keeping the aspect ratio.
- `<art>_s.jpg`: 80×80.
- `<art>_m.jpg`: 240×240.
- `<art>_l.jpg`: 320×320.
- **Square variants:** a non-square source is first centred (aspect kept) on a square canvas of side max(w, h) filled with **opaque black** (`DAT_1055527d0` = `juce::Colours::black` 0xFF000000, set in `__GLOBAL__sub_I_analyzer_if.cpp` @0x101634684), then rescaled to the target size with high-quality resampling (`create_rescaled_image` @0x1016233a4).
- `create_small_arts_if_not_exists` @0x1009bb8a4 regenerates missing `_s`/`_m` files (and `_l` when needed) before an export.

**Device files** (`export_artwork` @0x1009bb078):
1. If the device row has no artwork yet, call `addArtwork` (vtable +0x2C8) on the device DB. It allocates the next image ID and returns the path:
   - export.pdb (`dsql_calc_next_device_image_info` @0x1019a8c38): `/PIONEER/Artwork/%05d/a<id>.jpg`
   - exportLibrary.db (`DevSqliteDBController::calc_next_device_image_path` @0x10070a018): `/PIONEER/Artwork/%05d/b<id>.jpg`
   - Folder number = `id / 20 + 1` (multiply by 0xCCCCCCCD), so 00001 holds IDs 1–19 and 00002 holds 20–39. The fixture's 36 folders end with 00036 holding 700–702.
   - `/.PIONEER` is used when the hidden flag is set.
2. Create the folder. Copy local `_s` → `<a|b><id>.jpg` and local `_m` → `<a|b><id>_m.jpg`. These are byte copies, not re-encodes.
3. **320×320:** only when `dbConversionDevice` = 11 **and** the setting `create320x320Artwork` is true, also copy `_l` → `<a|b><id>_l.jpg`. Which device 11 is, is **open**.

**Fixture facts:**
- All 702 `a<N>.jpg` and `b<N>.jpg` are 80×80; all `_m` files are 240×240.
- Every file is a baseline JFIF 1.1 with standard IJG tables at **quality 85** and 4:2:0 subsampling, with no EXIF or ICC.
- `a<N>` and `b<N>` (and the `_m` pairs) are **byte-identical** (1404/1404), because both are copies of the same local file.

**Re-export** (`export_art_when_overwrite` @0x1009bbcfc): when the track is already on the device, the artwork is exported again only if the source has artwork and the device row has none, or the device `_m` file is missing.

## 4. Incremental sync

### 4.1 Is the track already on the device? (verified)
`$_0` in `execExport` runs two tests:
1. **`isCollectionSongForDuplicateSongId`** @0x1017e1330:
   1. Candidates = `getContentIDBsByMasterIDs(masterDbId, masterContentId)` (vtable +0x6D8; Plus override @0x1006fa6d8).
   2. For each candidate, `RowDataTrack::isSameTrackForDuplicateId` @0x1004e0d5c must match **masterDbId, masterContentId, length (+0x3D8), bitrate (+0x3DC), bitDepth (+0x3E0), fileType (+0x3B8), fileSize (+0x3BC) and samplingRate (+0x3E4)**. The column mapping comes from `DevSqliteDBController::insertTrack`.
   - So a re-encoded or edited audio file (different size) counts as a **new** track. It is copied again and gets a `-1` name.
2. **Otherwise:** if the source path is inside this device's root and the device DB has that path (`isCollectionSong`), the device row is adopted. `updateMasterDBInfo` (vtable +0x428) then stamps the master IDs onto it.

### 4.2 Update vs re-copy (verified)
`overwrite_export` @0x1009b8748 compares the three change markers. `getUpdatedInfo` (vtable +0x3F0) returns the source's `TrackInfoUpdated`, `AnalysisUpdated` and `CueUpdated`; the device row holds `informationUpdateCount`, `analysisDataUpdateCount` and `cueUpdateCount`.

1. **Metadata.** If one source string field (+0x210) differs from the device row's +0x230, or `compareTrackUpdated` (vtable +0x3F8) says the info marker changed:
   1. `checkAndSetChanges` diffs the rows and `modifyTrack` (vtable +0x278) updates the device row **in place** (same ID, same file).
   2. `setTrackUpdated` (vtable +0x410) stores the new marker, and `clearModifiedByRbm` runs.
   - **The audio file is never copied again** for an existing track.
2. **Analysis.** If `compareAnalysisUpdated` or `compareCueUpdated` differ, or extended cue data is now needed (`is_need_exporting_cue_ext` @0x1009b8b28: some master cue has a colour, comment, beat loop, colour-table index or sub-ms part, and the device row lacks `contentLink` bit 0x100):
   1. `setAnalysisUpdated` (vtable +0x418) stores both markers.
   2. **Delete the device ANLZ set** (`delete_device_analysis_files`; kept if another same-device DB still references it) and drop the cue cache.
   3. Merge `contentLink` bits from the source: 0x100 always, then 0x200, 0x400, 0x40000, 0x80000, and 0x10000/0x100000 for some analysis types. Save with `modContentLink`.
   4. Return 0, so ANLZ is rewritten (§ 1.6).
3. **Otherwise** it returns 1: the track is skipped and counted as a success.

### 4.3 Deleting tracks and playlists from the device (verified)
- **Delete a device playlist** (`deleteList` / `delete_list` @0x100c4b3f4): removes the list rows and its artwork.
  - When the preference `DeletePlaylistTracks` is on (`SettingIF::getCurrentDeletePlaylistTracks`), `removePlaylistTrackIfNotUsed` @0x100c4bf68 collects the tracks that no playlist on this DB or any same-device DB still uses (`get_not_used_playlist_tracks[_same_device]`). It then asks "Tracks which no longer exist in playlists will be deleted." and calls `removeFromCollection(tracks, true)`.
  - With the preference off, orphan tracks stay on the stick.
- **Remove a track from the device collection** (`removeFromCollection` @0x100c40d98 → `modifyRemove` @0x100c41cd4, which deletes the row and the files):
  1. `delete_device_song_file` @0x100c44684 deletes the audio file **only if** the path is under `<root>/Contents`, no same-device DB still references the path, and the master library does not reference that on-stick file. The read-only bit is cleared first.
  2. `delete_device_analysis_files` @0x100c44a54 deletes the ANLZ set unless a same-device DB still references the analysis path.
  3. The artwork files are removed too (`delete_artwork_file` @0x100c43800).
- **No automatic orphan sweep was found.** Files under `/Contents` or `/PIONEER/USBANLZ` that no DB references are left alone. The only automatic cleanup is the same-path ANLZ purge in § 1.6.

### 4.4 Sync Manager: `playlists3.sync` and `playlists3Plus.sync` (verified: code + fixture)
**Files and what they map**
- `PIONEER/rekordbox/playlists3.sync` belongs to export.pdb; `playlists3Plus.sync` belongs to exportLibrary.db. `PlaylistSyncManager::getDeviceSettingPath` @0x100fa8194 picks the file by `isDeviceLibraryPlus` and looks in `PIONEER/rekordbox`, then `.PIONEER/rekordbox`.
- Each file maps master playlist IDs to that DB's device playlist IDs, for the lists that "Sync Manager" keeps in sync.
- **Optional:** with no file, auto-sync is off and nothing happens.

**Format** (`DeviceSyncSetting::write` @0x100f9dc08; checked on the fixture: 1116 bytes, 14 CRLF)
- No BOM. `<?xml version="1.0" encoding="UTF-8"?>`, CRLF, an empty CRLF line, 2-space indentation, CRLF endings including on the last line.
- **Root element:** `<Sync DBID=… AutomaticSync=0|1 AllPlaylists=0|1 IncludeCue="1" ForcedSync=0|1 Timestamp="0">`, then `<Playlists>`.
  - `DBID` = the shared DBID if non-zero, otherwise the master DBID, printed as signed i32.
  - `IncludeCue` is a constant 1 and is never read.
  - The root `Timestamp` is the literal 0.
- **One `<NODE Id ParentId Attribute Lib_Type Dev_ID Timestamp CheckType/>` per synced list,** in that attribute order:

| Attribute | Meaning |
|---|---|
| `Id`, `ParentId` | master playlist IDs as uppercase unpadded hex (root pseudo-node `Id="0" ParentId="0" Attribute="1"` = the whole Playlists tree) |
| `Attribute` | master list type as signed char: 0 playlist, 1 folder, 4 smart, −128 the cloud trial list (*hypothesis*) |
| `Lib_Type` | 0 rekordbox, 1 iTunes |
| `Dev_ID` | the playlist ID in **that file's own DB** |
| `Timestamp` | the master list's change time (ms), as of the last successful sync |
| `CheckType` | tree checkbox state: 1 = all children included |

- **Fixture:** classic `Dev_ID`s 1, 3, 4, 5, 6, 7 resolve to the same-named export.pdb lists. Plus 4, 5, 6, 7 and 16391 resolve in exportLibrary.db. Plus 1 and 16390 are stale (the lists now exist as 16392/16393).

**When sync runs and what it does**
- `syncDeviceIfNeeded` @0x100fac3b4 runs **only when the device is first selected or opened in the browser tree**, not on mount.
- It requires the file's `DBID` to equal the current library's and `AutomaticSync="1"`.
- `mergeDevLibClassicToDevLibPlus` @0x100fa4b3c first makes the Plus file mirror the classic selection. It always rewrites `playlists3Plus.sync`.
- **Phase 1:** deletes device lists (by stored `Dev_ID`) whose master list is gone or moved.
- **Phase 2:** re-exports (via `exportPlaylist` with `syncMgr = true`) every list whose stored `Timestamp` differs from the master's in `masterPlaylists6.xml`, or that is smart, iTunes, forced or previously interrupted (§ 4.5). It then stores the new `Dev_ID`s and timestamps.
- **Device edits:** exporting into or editing a device list updates its node's timestamp to "now" (`updateDeviceSyncSetting` @0x100fabd98).

**Consequence for a writer:** a stale `Dev_ID` or `Timestamp` makes rekordbox delete or re-export lists on the next auto-sync. Either keep the files consistent or delete both.

### 4.5 "Export in progress" markers (verified)
- **When they exist:** while `exportPlaylist` jobs run, `exportPlaylist` creates an empty file in `<PIONEER dir>/rekordbox/`. The file is stored in the last job (+0x70) and deleted by the background thread when that job's tracks are all done.

| Marker | Used for |
|---|---|
| `sync.temp` | export.pdb |
| `sync.devlibplus.temp` | exportLibrary.db |
| `sync.itunes.temp` | iTunes lists |

- **What they mean to rekordbox:** `hasExportPlaylistCanceled` @0x1009c55b0 treats an existing marker as "the last export was interrupted", which forces Sync Manager to re-export.
- **For a writer:** never leave one behind.

## 5. Settings files (verified: code + all four fixture files)
**No settings file is written by the export itself.** None of the DbmExport functions references `MySettingFile`, `DeviceSettingFile` or `DJMMySettingFile`.

**When they are written**
- **When the device DB is created or updated** (`createDeviceDB` posts browser message 5/4 → `dbMessageUpdateDevice` @0x10031a1bc):
  - `DatabaseIF::syncMyTag(handle)`.
  - Then, if the preference "apply My Settings to devices" (`SettingIF::getCurrentApplyMySettingOnDevices`) is on:
    - `MySettingFile::exportToDeviceFromLocalFile(root, true)` @0x10024f68c writes `MYSETTING.DAT` and `MYSETTING2.DAT`;
    - `exportDJMMySettingFileToDevice` @0x1023d68e0 writes `DJMMYSETTING.DAT`.
  - Both **skip the write when the device file is already equal**, so remounting does not touch the mtimes.
- **When the device is added to the tree** (`TreeViewer::addDeviceToDeviceRoot` @0x10155a920):
  - `updateDeviceDevSettingData` @0x1023ec084 writes `DEVSETTING.DAT`, but only when the device has no valid one. An existing device file is never overwritten here.
  - `updateDeviceMySettingData` @0x1023eb9e0 fills a missing MYSETTING/DJMMYSETTING and asks before replacing a player-written one.
- Settings editors and Pro DJ Link requests also write them; those are not part of export.
- **Local sources** are in `~/Library/Application Support/Pioneer/rekordbox6/` (`common::PropertyPath` @0x10006d1ec): MYSETTING.DAT, MYSETTING2.DAT, DEVSETTING.DAT, DJMMYSETTING.DAT and djprofile.nxs. Missing local files mean the defaults below.
- **Folder:** `<root>/PIONEER/`. `<root>/.PIONEER/` is used only when hidden folders are allowed **and** statfs reports an `HFS` file system (`pioneerMySetting::getDeviceDirectoryPath` @0x10024f3a0; identical logic in `DeviceSettingFile::getDeviceDirectoryPath` @0x100c06c48). APFS, msdos and exfat get `PIONEER`.

**Common layout** (little-endian)

| Offset | Size | Field |
|---|---|---|
| 0x00 | u32 | 0x60, the length of the string block |
| 0x04 | char[32] | brand, NUL-padded |
| 0x24 | char[32] | product |
| 0x44 | char[32] | version |
| 0x64 | u32 | data length N |
| 0x68 | N | data |
| 0x68+N | u16 | CRC |
| 0x6A+N | u16 | 0 |

| File | Size | N | Header written by rekordbox |
|---|---|---|---|
| `MYSETTING.DAT` | 148 | 0x28 | `PIONEER` / `rekordbox` / `0.001` |
| `MYSETTING2.DAT` | 148 | 0x28 | keeps whatever header it already has (fixture: `PIONEER DJ` / `CDJ-3000` / `0.001`) |
| `DEVSETTING.DAT` | 140 | 0x20 | `PIONEER DJ` / `rekordbox` / app version cut to 3 parts (`getFileVersion(true)`: "7.2.19", *hypothesis* for the exact string) |
| `DJMMYSETTING.DAT` | 160 | 0x34 | `PioneerDJ` / `rekordbox` / `1.000` |

**Checksum: CRC-16/XMODEM** (poly 0x1021, init 0, no reflection, xorout 0). Implemented bit-serially in `pioneerMySetting::calcCRC` @0x10024dd34, `cdjDeviceSetting::calcCRC` @0x100c05f40 and `DJMMSContent::calcCRC` @0x1007d6004.
- MYSETTING, MYSETTING2 and DEVSETTING: over the data block only (0x68 .. 0x68+N). Fixture values 0x56CA, 0xF22C and 0xFA6E all match.
- DJMMYSETTING: over the **whole file up to the CRC** (0x00 .. 0x9C). Fixture 0x144B matches; a data-only CRC would give 0xCB6C.

**Data blocks** (field names from the getters; the bytes not named here are open)
- **MYSETTING:**
  - Starts with u32 0x12345678 and u32 2, then 32 setting bytes, each `0x80 | value`.
  - Bit 7 means "field set", which matters when merging (*hypothesis* on the exact meaning).
  - Defaults (0x70..0x8F): `81 83 81 88 81 01 82 81 81 01 01 01 82 80 80 81 80 81 80 00 00 81 00 00 81 81 81 80 81 80 00 00`.
- **MYSETTING2:** has **no magic**; the settings start at 0x68. Defaults: `81 80 83 83 81 80 80 00 00 00 80 81 85`, then zeros.
- **DEVSETTING:** u32 0x12345678, u32 1, then plain values with range checks. Defaults: 0x70..0x78 = `01` × 9.
- **DJMMYSETTING:** u32 0x12345678, u32 1, u32 0x20, 32 setting bytes, u32 0, then an empty BFX block (8 bytes). The file is 0x480 bytes when the BFX block is present.
- **Merge rule** (`saveDeviceFile` @0x10024fec4): for each byte with bit 7 set in the source, the target becomes its own bit 7 | (source & 0x7F); invalid fields are repaired; then the CRC is recomputed.

**Other files**
- **`djprofile.nxs`** (KUVO DJ profile, 160 bytes, contains the user name; contents not analysed):
  - copied from the local folder to `<PIONEER dir>/djprofile.nxs` **when an export thread starts** (`DbBackGroundProcess` ctor @0x1001d30e4, for handles > 1, if the local file exists);
  - also copied on KUVO activation (`KuvoService::copyProfileToConnectedDevices` @0x100f914ec, `PIONEER` folders only).
- **`RBFLTR.DAT`:** rekordbox never writes it to a device.
  - The only references are `FilterSettingManager` / `PSvDBServer` for the local Pro DJ Link DB server.
  - The stick copy has an `FMAI` header naming `CDJ-3000` `3.19`, so a player wrote it (*hypothesis*).
  - A writer should keep it and never create it.
- **DeviceLibBackup JSON:** see § 6.

## 6. Device detection, eligibility and mount-time writes

### 6.1 Detection (verified)
- **DiskArbitration** (`RemovableDeviceDetector::Pimpl` ctor @0x1021fd428) registers callbacks for disk appeared, changed and disappeared.
- **`diskAppeared`** @0x1021fd508 accepts a disk only if all of these hold:
  - its device protocol is exactly `USB` or `Secure Digital`;
  - it is not an `EFI System Partition` and not an `Apple_partition_map` disk;
  - `removableVolumeName` @0x1021fd864 yields a volume path (the disk is ejectable or mountable).
- **What happens next:** accepted volumes go into a BSD-name → path map. `DatabaseMediator::changeListenerCallback` @0x100cc85d4 posts `DBCommandDeviceChange` → `execDeviceChange` @0x100cc9dc8.
- **A second path:** `getConnectedDrives` @0x1021fceb8 scans `/Volumes`.

### 6.2 Eligibility (verified)
**File system** (`getDriveInfo` @0x1021fc79c via statfs; `check_device_info` @0x100ccac8c)
- **Accepted:** FAT12, FAT16 and FAT32 (msdos subtypes 0/1/2), exFAT and HFS(+).
- **Rejected:** the system root (`MNT_ROOTFS`, error 1); anything unclassified (error 2); APFS and NTFS (`isNotSupportedFilesystem` @0x1021fcde4).
- **Folder name:** **HFS volumes use the hidden folder `/.PIONEER`**; every other file system uses `/PIONEER`. The same rule appears in the settings-file code (§ 5) and `IsHFS` @0x1008fcc34. This is the controller+0x70 flag.

**Other gates**
- **Device count:** a non-cloud device is refused (error 4) once 24 handles are registered. Each stick takes two handles (classic + Plus).
- **Writable:** `check_device_write_access` @0x100ccb110 requires the root to be writable.
  - If `export.pdb` is missing, it creates the rekordbox folder plus an empty `export.pdb` as a probe, then deletes the probe.
  - If it exists, it must be writable.
  - A failure (error 3) drops the device. **A read-only stick is ignored completely**, which is why the read-only fixture stick is safe.

**Export filter** (`check_support_export` @0x1009bfce4, USB branch)
- **Refused:** streaming tracks; file type 0x40 (DRUM CAPTURE); video (AVI/MPG/M4V/MOV, MP4 with video); FLAC/ALAC/WAV/AIFF at sample rates other than 44.1, 48, 88.2 or 96 kHz.
- **Allowed with a warning:** FLAC/ALAC, and WAV/AIFF at 88.2/96 kHz. The warning is "Files playable only on specific DJ equipment" (preference `askExportHiResoTrack`).
- **The mobile handle** is stricter: for example, no WAV/AIFF on iOS devices.

**Warnings shown before an export**
- `showExportWarningIfNeed` @0x1009c07e0 shows a one-time exFAT / Device Library Plus compatibility notice (`askExportDeviceTracks`). It lists CDJ-3000X, CDJ-1500X, XDJ-AZ, OPUS-QUAD, OMNIS-DUO and XDJ-AN as equipment that supports exFAT.
- **Free space** is checked per track (≥ 1.5 × file size, § 1.2).
- **No explicit FAT32 4 GiB file-size check was found** (open).

### 6.3 What happens when a stick is mounted (verified unless marked)
`createDeviceDB` @0x100ccb5a4 runs the same steps, with creation enabled, at the start of an export (§ 1.1).

1. **No DB yet:** `check_exists_pioneer_folder` @0x100ccafe8 tests for `<root>/PIONEER/rekordbox` (`.PIONEER` on HFS). If it is missing, the device is listed without a DB and **nothing is written** until an export calls `createDeviceDB`.
2. **Write probe** (§ 6.2).
3. **Classic DB** (`rekordboxDBController::startupDeviceDB` @0x1017cdc08):
   - **If `export.pdb` is missing:** create the folder, delete any stale `exportExt.pdb`, and create the DeviceSQL pair. `initPropertyTable` @0x1017ce008 → `DevInitPropertyTable_` writes the property row with today's date, then copies colours, menu items, categories and sort entries from master.db.
   - **Otherwise:**
     - a file named `brokendb` in the rekordbox folder marks the DB as corrupt (the UI offers to delete it);
     - otherwise both `.pdb` files are opened and `devTableCheck` @0x10199a394 reads all 20 tables.
4. **Device UUID / backup info** (`getDeviceUUID` @0x100ccb9c0 → `initBackupInfoFile` @0x100ccc484):
   - writes `<PIONEER dir>/DeviceLibBackup/rbDevLibBaInfo_<DBID>.json` when it is missing or has no `uuid`;
   - only when the licence allows the device-library-restore feature (`ActivationComponent::canUseDeviceLibraryRestoreFunction`);
   - `<DBID>` is the **local master.db's** `djmdProperty.DBID` as unsigned decimal, not the stick's. Fixture: 417703505 = the sandbox library;
   - content: `{"uuid": "<random 32-hex juce::Uuid>", "info": []}` with CRLF line endings;
   - `info` gains one object per backup run (`backupDeviceLibrary` @0x100cccba4) with these keys: `device_id`, `device_name`, `background_color`, `background_color_libplus`, `device_filesystem`, `backup_pc_id`, `backup_pc_name`, `backup_location`, `backup_generation`, `backup_date` (`%Y/%m/%d %H:%M:%S`) and `backup_file_name` = `rbDevLibBa_<DBID>_<uuid>.zip`;
   - backups run on quit, on eject or from the menu, and only when backups are enabled for that device.
5. **Plus DB** (`addDeviceLibraryPlus` @0x1010dde60 → `DevSqliteDBController::startupDeviceDB` @0x1006f4a88):
   - creates `exportLibrary.db` if missing;
   - opens it in WAL mode and runs `createTables(false)` and `createIndexes(false)`. **`createIndexes` drops and recreates 4 indexes on every open**, so every mount writes WAL frames, and the main file changes at the next checkpoint or close.
6. **Browser side** (`dbMessageAddDevice` @0x100319b08 / `dbMessageUpdateDevice` @0x10031a1bc):
   - **My Tag:** `syncMyTag` (§ 2.5).
   - **Plus DB new or empty:** the Device Library Plus conversion is offered (`needDbConversion` @0x100728748 → `deviceSqliteDbConversion` @0x1010def78, § 6.4).
   - **Plus DB already filled:** `syncDevSettingsForDevLibPlus` @0x1010ddef4 copies the device name, categories, sort entries and colour names from the classic DB to the Plus DB, writing only what differs.
   - **Settings files** per § 5.
   - **History import:** if "import history automatically" is on, player histories are imported and logged in `<root>/PIONEER/rekordbox/ImportedHistory.xml` on the stick. They are optionally deleted from the device afterwards.
7. **Player usage logs:** `DJDeviceUsageLogManager::run` @0x1014463b4 **moves** `<PIONEER dir>/log/*.json` and `*.bin` (written by players) into a local folder and deletes them from the stick. This explains the fixture's empty `PIONEER/log` with the 19:42 mtime.
8. **Closing:**
   - `prepareForRemoveDevice` @0x100cc8c80 and `closeDB` (classic @0x1017cf9c4, Plus @0x1006f6e08) close the DBs on eject or quit.
   - The Plus close checkpoints the WAL into `exportLibrary.db`.
   - The classic close runs `TE_DPgm_Close`, which writes the `.pdb` headers (state, transaction ID; `pdb.md`).
   - `prepareForRemoveDevice` polls the rekordbox folder's mtime, so it expects the close to write.

**Why the fixture stick changed without an export** (hypothesis, consistent with the code above)
- **19:42:** the DeviceLibBackup json was created (step 4, for the sandbox library) and the player logs were moved off the stick (step 7).
- **20:00:** all three DBs were stamped when rekordbox closed them (quit, or the unmount before the read-only remount). `exportLibrary.db` carries the index rebuild from step 5; the `.pdb` files get the close header update. No track content needs to change for this.
- **Ruled out:** Sync Manager (wrong DBID, and it runs only on tree selection; the `.sync` mtimes are unchanged), My Tag writes (foreign owner) and history deletion (all 21 histories are still present).

### 6.4 Device Library Plus conversion (verified)
`deviceSqliteDbConversion` @0x1010def78 → `execDbConversion` @0x1010e1a38 builds the Plus DB from the classic one:
1. Stage in `PIONEER/.temp_libplus_conv/`, then move into place.
2. Clear the Plus tables, then bulk-copy content, genre, artist, album, label, key, colour, playlist, playlist entries, hot cue bank lists and more, **keeping the classic IDs**.
3. Copy `a<id>.jpg` / `a<id>_m.jpg` to `b<id>.jpg` / `b<id>_m.jpg`.
4. Handle EXT/2EX per track.
- The rekordbox UI checks free space (Artwork folder size) and asks before overwriting an existing Plus DB.
- **Fixture:** identical content and image IDs and byte-identical `a`/`b` files, and the Plus `property` row (`createdDate` 2020-10-28, `dbVersion` "1000") equals the classic one. That points at this conversion (*hypothesis*), after which both DBs were extended in lock-step by exports.

### 6.5 Files on the stick that rekordbox does not create
`PIONEER/CDJ`, `PIONEER/MPJ`, `PIONEER/TrashBox`, `PIONEER/.CacheData/*` (fixture mtimes 1979/2014, no player clock), `PIONEER/RBFLTR.DAT`, the contents of `PIONEER/log/`, and `ESOWMPInfo.txt` at the root. None of these names occurs in the rekordbox binary (string scan), so players or other hosts write them (*hypothesis*). A writer must leave them alone.

### 6.6 `DBFileDef::getFile` @0x10167f290
This is a **local** file-name table (master dir: `datafile.edb`, `ExtData.edb`, `master.db`, …; cloud mirror dir laid out like a stick: `PIONEER/rekordbox/export.pdb`, `PIONEER/USBANLZ`, `PIONEER/Artwork`, `Contents`). It is not a device table.

## 7. What a writer must reproduce for players to accept the stick
**Must** (the reader in the player depends on it, or rekordbox itself would reject the stick):
1. **Databases.**
   - Valid `export.pdb` (+ `exportExt.pdb`) per `pdb.md`, and for OneLibrary players `exportLibrary.db` per `devlib.md`, with **consistent IDs** inside each DB.
   - Every track's `path` must exist under `/Contents/`. `analysisDataFilePath` / `ANALYSISDATAPATH` must point at its `ANLZnnnn.DAT`, with `.EXT`/`.2EX` next to it.
   - The PPTH tag in each ANLZ file must hold the **same device path** as the DB row. The `Pxxx/xxxxxxxx` folder must be the hash of that path (`anlz.md`; verified 704/704).
2. **Artwork.** Image rows must point at existing JPEG files: `a<id>.jpg` (classic) and `b<id>.jpg` (Plus), 80×80, each with a 240×240 `_m` companion.
   - Players probably derive the `_m` name from the row path (*hypothesis*: no row stores it).
3. **Playlists.** Entries 1-based and contiguous; parents exist.
4. **Close cleanly.** exportLibrary.db checkpointed and closed (no `-wal`/`-shm` left); export.pdb header state 5 (`pdb.md`).
5. **Names a FAT32 player can open.** No inhibited characters, NFC, components of at most 48 code points (§ 1.3). This is what rekordbox guarantees; the player-side limits themselves are **open**.

**Should** (keeps rekordbox and the stick in agreement):

6. Fill `masterDbId`/`masterContentId` and the three update markers from the source, so rekordbox's next export recognises the tracks instead of duplicating them (§ 4.1–4.2).
7. Keep `contentLink` = `(src & 0xC1600) | 0x100`.
8. Set `myTagMasterDBID` = DBID + 1 if you write My Tags.
9. Do not leave `sync*.temp` markers. Keep `playlists3*.sync` consistent or delete them.
10. Keep the content and image IDs equal in export.pdb and exportLibrary.db, and give both rows the same path, ANLZ set and artwork number (§ 2.1, § 6.4).
11. Keep `export.pdb` writable and never leave a `brokendb` file, or rekordbox drops or flags the stick (§ 6.2–6.3).

## Writer checklist
- [ ] Target `/PIONEER` on FAT/exFAT and `/.PIONEER` on HFS(+). Refuse APFS/NTFS and read-only volumes. Never leave a `brokendb` file.
- [ ] Refuse to start without free space ≥ 1.5 × each file being copied (rekordbox's per-track rule).
- [ ] Analyse first. Export only tracks that have local `ANLZ0000.DAT`/`.EXT`/`.2EX`, and build the USB ANLZ files from them (`anlz.md` § Local vs USB).
- [ ] Audio path: `/Contents/<artist≤48>/<album≤48>/<file≤48>`.
  - [ ] Inhibited `/\:?*"<>|~` → `_`; NFC.
  - [ ] Directory names: trailing `.` → `_` before the cut; after the cut, trim and strip trailing `.`.
  - [ ] `UnknownArtist` / `UnknownAlbum` for empty names.
  - [ ] The file name cut keeps the extension.
  - [ ] On a clash, `-1`, `-2`, … inside the 48-character limit.
- [ ] Copy each audio file once per stick, even when two DBs are written; both DB rows share the path, ANLZ set and image ID.
- [ ] ANLZ: `/PIONEER/USBANLZ/P%03X/%08X/ANLZ%04X.{DAT,EXT,2EX}`, number = max existing in the folder + 1. Before writing a new track, delete any set in that folder whose PPTH equals the new path.
- [ ] IDs: max + 1 per table. Look up artist/album/genre/label/key by exact name before adding.
- [ ] Artwork: one image row per track.
  - [ ] Files `/PIONEER/Artwork/%05d/{a|b}<id>.jpg` + `_m.jpg`, with folder = id/20 + 1.
  - [ ] 80×80 and 240×240 baseline JPEG q85, letterboxed on black. Use the same bytes for `a` and `b`.
- [ ] Track rows:
  - [ ] `masterDbId` = source MasterDBID or library DBID; `masterContentId` = MasterSongID or ID.
  - [ ] Update markers copied from the source.
  - [ ] `contentLink` per § 1.2.
- [ ] Playlists:
  - [ ] Match existing device lists by (parent, name); skip identical lists; otherwise clear and refill in master order.
  - [ ] Folders attribute 1; at most 1000 lists; at most 10000 tracks per list (1000 for the special devices).
- [ ] Re-export:
  - [ ] Identify tracks by master IDs + (length, bitrate, bitDepth, fileType, fileSize, sampleRate).
  - [ ] Update rows in place; never re-copy existing audio.
  - [ ] Rewrite ANLZ only when the analysis or cue marker changed.
- [ ] Deletes:
  - [ ] Remove the audio only if no other DB on the stick (and no library on this machine) uses the path.
  - [ ] Same rule for ANLZ and artwork.
- [ ] Settings files:
  - [ ] Never as part of export.
  - [ ] If written: exact header strings, CRC-16/XMODEM (data block; whole file for DJMMYSETTING), sizes 148/148/140/160.
  - [ ] Never overwrite a player-written file silently; never create `RBFLTR.DAT`.
- [ ] Close both DBs cleanly; no `sync*.temp` left; `playlists3*.sync` consistent or absent.
- [ ] Leave `DeviceLibBackup/`, `log/`, `ImportedHistory.xml`, `RBFLTR.DAT`, `CDJ/`, `MPJ/`, `TrashBox/`, `.CacheData/` and unknown root files untouched.

## Open questions
- [ ] Whether OneLibrary players need `cue` rows at all (rekordbox's export writes none; cues are in ANLZ only). Needs a player test.
- [ ] The full per-device format matrix (mobile, cloud and USB branches of the format gate, plus `check_support_export`).
- [ ] The device behind `dbConversionDevice` 11/12 (1000-track limit, 320×320 `_l` artwork).
- [ ] Whether players need `_m` files to exist, and whether they ever read `_l`.
- [ ] Player-side path limits: is 48 code points per component a player constraint or only rekordbox's choice?
- [ ] The source field compared at +0x210 vs device +0x230 in `overwrite_export` (key name?).
- [ ] Whether the classic DB really allocates max + 1 inside `DevAddContent_` (fixture-consistent, but the procedure body is DeviceSQL bytecode).
- [ ] DJMMYSETTING field map, unnamed MYSETTING/DEVSETTING bytes, and the exact DEVSETTING version string rekordbox writes.
- [ ] No FAT32 4 GiB file-size check was found in the export path. What happens with a larger file?
- [ ] Whether merely opening or closing the DeviceSQL page manager rewrites the `.pdb` headers when nothing changed. This decides whether § 6.3's explanation of the 20:00 rewrite holds. Test with a before/after diff on a FAT32 image.
- [ ] For a brand-new stick: does `createDeviceDB` → `needDbConversion` show the conversion dialog, or create the Plus DB silently?
- [ ] The full table list of `deviceSqliteDbConversion` (history, image, property, My Tag), and why the fixture's exportLibrary.db has 12 playlists against 30 live ones in export.pdb.
- [ ] Which control writes `djprofile.nxs` on mount (vtable slot 0x240, `exportKuvoProfile`), besides the export-thread copy.
- [ ] Verify the whole flow byte-for-byte against the oracle export (`docs/oracle-session.md`, pass A) once it exists.
