# Op: import from a device (tracks, playlists, histories)

Status: **binary-derived** (rekordbox 7.2.19 arm64, Ghidra; no SQL trace and no fixture with a device import). The analysis-file conversion is **verified** against fixture pairs (below). Building blocks: `common.md`, `import-track.md`, `analyse-track.md`, `edit-cues.md`.

musicbox: `mb_library::device_import` (tracks, playlists), `mb_library::device_history` (histories, `ImportedHistory.xml`), `mb_anlz::local::to_local` (analysis files), `mb_usb::stamp` (device rows).

## Entry points
| What | Function |
|---|---|
| Import tracks (optionally into a playlist) | `DatabaseMediator::importTrackFromDevice` @0x101a189f8 → `importFromDeviceManager::start` @0x1001dc470 → worker `importFromDeviceThread::run` @0x1001d9618 |
| Import a device playlist | `DatabaseMediator::importDevicePlaylist` @0x101a10670 (same manager), then `addTrackToList` (vtable +0x350) |
| Import histories | `DatabaseMediator::importHistoryFromDevice` @0x100c5345c → `importHistoryFromDeviceLocal` @0x100c535c4 |

## Tracks
### 1. Which tracks are imported (`importFromDeviceManager::start`)
For each selected device track:
1. `rekordboxDBController::isCollectionSong(path)` @0x1017e0fcc: a live collection row has exactly the device file's path as `FolderPath` (looked up by path hash) → the track is **already present** (status 3).
2. Otherwise `getMainTrackByDeviceTrack` @0x10052d18c: when the device row's `masterDbId` and `masterContentId` are both non-zero and `masterDbId` equals this library's DBID, the collection row with `ID = masterContentId` (vtable +0x48) → **already present**.
3. Otherwise status 1: to be imported.

Already-present tracks are not touched; their collection IDs are returned (and used for a playlist import).

### 2. Copying the audio (`import_from_d_ask_file_copy` @0x1001dc94c, `import_from_d_file_copy` @0x1001daa68)
- When status-1 tracks exist, a dialog ("The playlist you are trying to import contains music files that are not present in rekordbox on your computer. These music files will be copied to this computer.") shows their count and total size and the destination folder. Cancel aborts the import; OK marks every status-1 track 4 ("copy").
- **Destination folder:** the user's Music folder (`File::getSpecialLocation(3)`) + `PioneerDJ/Imported from Device` (`importFromDeviceCopySettingWindow` @0x1001d6af0); "Change" picks another one for the session.
- **File name:** `calc_device_file_path(row, folder, …)`, the function the USB export uses for `/Contents/…` (`../export.md` § 1.3–1.4): `<folder>/<artist>/<album>/<file>` with the same character, length and clash (`-1`, `-2`, …) rules. The copy is a plain `File::copyFileTo`.
- *Hypothesis:* the clash test is the local file system's existence check, as on the stick.

### 3. The collection row (`addTrackFromDevice` @0x101a16f34 → `rekordboxDBController::addTrack` @0x1017e1ee4 → `AppSyncDBController::insertTrack` @0x100a56e3c)
The worker copies the device `RowDataTrack` and, before adding it, empties its analysis path (+0x3f0) and artwork path (+0x3e8) and zeroes the 8 bytes at +0x408 (`masterDbId`, `masterContentId`). It sets the path to the copied file. `addTrack` then:
- looks up or creates the name rows (`getAndAddArtistID`, `…GenreID`, `…AlbumID` with album artist and compilation, `…LabelID`, `…KeyID`; vtable +0x850…+0x870) for artist, genre, album, remixer, original artist, composer, label and key;
- sets `ContentLink = (device ContentLink & 0x26000) | 0xE` (= 14 for every RBTEST row);
- inserts with the builder column list of `import-track.md` (`insertTrack`): `FolderPath`, `FileNameL`, `Title`, the lookup IDs, `BPM`, `Length`, `TrackNo`, `BitRate`, `BitDepth`, `Commnt`, `FileType`, `Rating`, `ReleaseYear`, `RemixerID`, `LabelID`, `OrgArtistID`, `KeyID`, `StockDate` (today when empty), `ColorID`, `DJPlayCount`, `ImagePath` "", `MasterDBID` = **this DBID** (`insertTrack` overwrites +0x408 with the controller's DBID), `MasterSongID` = **the new ID** (because +0x40c was zeroed), `AnalysisDataPath` "", `FileSize`, `DiscNo`, `ComposerID`, `Subtitle`, `SampleRate`, `Analysed`, `ReleaseDate`, `DateCreated`, `ContentLink`, `HotCueAutoLoad` "on", `DeliveryControl` "on", `DeliveryComment`, `Lyricist`, `ISRC`, sampler columns, `rb_file_id`, `OrgFolderPath`, `ExtInfo`, `DeviceID`, `Reserved1` — every value taken from the device row.
- Afterwards `setBPMColorIDKuvoComment` (vtable +0x4c8 @0x1017ecd20) writes BPM, colour and KUVO comment again. *Not replayed by musicbox* (same values as the insert).
- *Hypothesis:* the lookup order (musicbox: the file-import order, then original artist and composer), `DateCreated` (device value, today when empty) and the `Analysed` value of the insert (0; it is set in step 4).

### 4. Analysis files (`import_device_analysis` @0x1001db460, `CAnalyzerIF::initPathAndDeviceCue` @0x10162ee04)
- The device `.DAT`, and `.EXT` / `.2EX` when the analysis status says they exist, are copied byte for byte to the new track's local analysis path `<analysis root>/PIONEER/USBANLZ/<uuid[0:3]>/<uuid[3:]>/ANLZ0000.*` (the content UUID's folder, `CreateAnlzFileFolderPath`'s local branch, `../anlz.md`).
- `initPathAndDeviceCue` then opens the `.DAT` and `.EXT`, sets their PPTH to the new audio path in the local form (`?/<file name>`, `get_new_format_ppth`), clears every cue list (`clear_pmai_all_cue_data`, `clear_pmai_ext_all_cue_data`) and writes both files again (`write_pmai`, `write_pmai_ext`). It sets the `.2EX` path in memory but **never writes the `.2EX`**: that file keeps the device PPTH and its scrambled PVDI.
- Neither the EXT reader (`read_pmai_ext_song_struct`) nor the writer calls a scramble function, so PSSI stays in its scrambled (device) form in the local `.EXT`. Both forms are self-describing (`../anlz.md` § PSSI/PVDI scramble), so rekordbox reads either.
- If the rewrite fails, the copied files are deleted.
- Then `modTrackAnalysisPath` (vtable +0x468) stores `AnalysisDataPath` = `/PIONEER/USBANLZ/<uuid[0:3]>/<uuid[3:]>/ANLZ0000.DAT` and `modAnalyzeStatus` (vtable +0x490) runs with `(device status & 0xC060020) | 0x49`, where the device status is `getAnalyzeStatus` = `(Analysed | ContentLink << 8) & 0x3C86707B`. As for an analysis (`analyse-track.md` § Status bits) it writes **both** columns: `Analysed` = status & 0x7B (device 41 → 105) and `ContentLink` = (inserted value & ~0x8670) | 0xE | ((status >> 8) & 0x3C8670) (RBTEST rows: 14 → 0xC060E = 787982). Phrase and vocal bits never come back: the device `ContentLink` (`(ContentLink & 0xC1600) | 0x100`) does not carry them.
- **Verified 2026-10-13 against rekordbox:** a musicbox import that left `ContentLink` at 14 made rekordbox open its "analysed with an old version" alert (`MissingWaveDataAlertWindow`, setting key `askMissingWaveDataMaster`) on the next start. `RowDataTrack::isAnalyzedOldVer` @0x10008a5b0 counts an analysed track (`Analysed` bit 0x01) as old when `ContentLink & 0x80000` is 0 (plus masked-bit checks against constants @0x10374e670/678). `mb_library::device_import::imported_status` writes the value above.

**Verified** (`crates/mb-library/tests/device_import.rs`, `to_local_reverses_rekordboxs_usb_files`): for the 133 oracle-a tracks whose device file name equals the original name, `to_local` of rekordbox's RBTEST `.DAT` is **byte-identical** to oracle-a's local `.DAT` (133/133), and the `.EXT` equals the local one once PSSI is unscrambled (133/133; byte-wise it differs only in the scrambled PSSI, as described above).

### 5. Cues (`getDeviceContentCueInfo` @0x1017ddf60 → `updateCue`, vtable +0x4f0 @0x1017ed46c)
The device track's cues (read from its analysis files) go into `djmdCue`; cues of kind 101 (hot cue bank) are skipped. musicbox reads the `.EXT` PCO2 lists (all hot cues, colours, comments), else the `.DAT` PCOB lists, and adds each cue with the `edit-cues.md` add-cue op. *Hypothesis:* rekordbox writes them as one `updateCue` batch; the resulting rows are the same, but the counters (`CueUpdated`) and USN counts differ.

### 6. Artwork (`add_track_artwork` @0x1001dbf44)
`addArtwork` (vtable +0x2c8) assigns the local artwork path; the device image is then written as the local `artwork.jpg` / `_m` / `_s` set (`CAnalyzerIF::writeFileImage`), or, without a device image, the image embedded in the audio file (`writeTagImage`). `ImagePath` = `/PIONEER/Artwork/<uuid[0:3]>/<uuid[3:]>/artwork.jpg`. *Hypothesis for musicbox:* the files are byte copies of the device's 240 px (`artwork.jpg`, `_m`) and 80 px (`_s`) images instead of re-encoded ones; the embedded-image fallback is not implemented.

### 7. My Tags and like relations (`import_device_ex_track_data` @0x1001db064)
My Tags matched by the local tree (`getAllMyTagList`) and "like" relations (`addTrackRelation`). *Not implemented by musicbox.*

### 8. Device rows (`updateMasterDBInfo`, device controller vtable +0x428 @0x1017e1d24)
`getMasterIDs` @0x101941fb4 gives `(this DBID, new content ID)` without cloud sync (cloud: the row's own master IDs). When they differ from the device row's `masterDbId` / `masterContentId`, rekordbox writes them onto **the stick's row** of the browsed database, so a later export recognises the track (`../export.md` § 4.1). musicbox writes both databases (export.pdb by ID with a copying update, exportLibrary.db by path) in `mb_usb::stamp::stamp_master_ids`, and only on devices it may write to.

### 9. Playlist
`importDevicePlaylist` imports the list's tracks as above and adds every one of them (imported or already present) to a new local playlist with the device list's name (`trimPlaylistName`). *Hypothesis:* the new list goes on top of the root, like a playlist create.

## Histories (`importHistoryFromDeviceLocal` @0x100c535c4)
1. For each device history (`getHistoryItem` / `getHistoryTrack`, device vtable +0xe0 / +0xf8), every track is mapped with `getMainTrackByDeviceTrack` (above). **Unmapped tracks are dropped; histories never import tracks.** A history with no mapped track is skipped.
2. **Dedup:** `<PIONEER dir>/rekordbox/ImportedHistory.xml` on the stick (`DeviceHistory::ImportedHistoryManager`):
   ```
   <?xml version="1.0" encoding="UTF-8"?>

   <IMPORTED-HISTORY>
     <DEVICE-LIBRARY>          (export.pdb histories)
       <HISTORY id="<device history ID>">
         <CONTENTS><hash></CONTENTS>
       </HISTORY>
     </DEVICE-LIBRARY>
     <DEVICE-LIBRARY-PLUS>…</DEVICE-LIBRARY-PLUS>   (exportLibrary.db histories)
   </IMPORTED-HISTORY>
   ```
   Written with `juce::XmlElement::writeTo` and the default format (CRLF, empty line after the declaration, two-space indentation; *hypothesis*, no fixture has the file). `<hash>` = lower-case hex SHA-256 (`getHash` @0x101a8cd08) of the concatenated `%08x` pairs (master DB ID, content ID) of the mapped tracks in play order. A history whose (database, ID, hash) is listed is skipped (`ImportedHistory::exist` @0x101a8c094). *Hypothesis:* which content ID goes into the hash; for mapped tracks the master content ID and the collection ID are the same.
3. **Folders** (`getHistoryFolder` @0x1017f385c): today's year folder and month folder, created when missing. Existing folders are found by the year (month) of their `DateCreated`.
4. **History rows** (`createNewHistory` → `CloudAgentAPI::HistoryIF::addNewHistory` @0x100a6388c, ORM `HistoryRepository::SaveNewHistory`):

   | Row | `ID` | `UUID` | `Name` | `Attribute` | `ParentID` |
   |---|---|---|---|---|---|
   | year folder | the year (`2026`) | the ID as text | `"2026"` | 1 | `root` |
   | month folder | year × 100 + month (`202610`) | the ID as text | `"10"` | 1 | the year folder |
   | history | random u32 ≥ 3000 | v4 UUID | `HISTORY YYYY-MM-DD`, then ` (n)` | 0 | the month folder |

   `Seq` = max sibling `Seq` + 1; `DateCreated` = local `YYYY-MM-DD HH:MM:SS`. ` (n)` = one more than the highest suffix among the month folder's histories that start with the same name (1 when only the plain name exists).
5. **Entries** (`insertTrackToHistory` @0x100a63fc8): one `djmdSongHistory` row per played track (`TrackNo` = max + 1, `ID` and `UUID` two v4 UUIDs like `djmdSongPlaylist`), one ORM save each.
6. **Play counts** (`updateDjPlayCount` @0x1017f3b94): each track's `DJPlayCount` grows by its number of plays in the imported histories.
7. The imported histories are added to `ImportedHistory.xml`; with the preference `DeleteHistoryWhenImported` ("Delete from the device after importing the play history") they are removed from the device (`removeImportedHistory` @0x100c4b9c8). The automatic import runs when the preference `importHistory` ("Import the play history automatically") is on and the device is mounted (`../export.md` § 6.3). *musicbox:* manual import only; never deletes device histories.

**Verified by musicbox's tests only** (no rekordbox oracle): two synthetic histories built from RBTEST rows imported into an oracle-a copy (folders, names with ` (1)`, entries, play counts, dedup via the XML, undo), and the reference stick's 21 histories imported into oracle-a: all skipped, nothing written (its tracks belong to another library).

## Open
- Statement order, USN count and dialect of the whole import (no trace).
- What `importDevicePlaylist` does with folders, and where the new list goes.
- The Plus database path (`DEVICE-LIBRARY-PLUS`) and exportLibrary.db histories: musicbox reads export.pdb only.
- My Tag and "like" import; artwork re-encoding; the embedded-image fallback.
- Which DB the automatic history import uses when both exist, and how `DeleteHistoryWhenImported` deletes (rows and renumbering).
