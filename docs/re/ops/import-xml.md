# Op: import tracks and lists from a rekordbox.xml

Status: **binary-derived** (rekordbox 7.2.19 arm64, Ghidra). No trace or fixture contains a rekordbox.xml import, so the statement order and counts below are read from the code, not observed. The value rules (TEMPO → beat grid, POSITION_MARK → cues) are ported 1:1 and unit-tested; the rows musicbox writes for imported files equal rekordbox's own rows for the same files (see Verification). Implementation: `crates/mb-xml/src/import.rs` (value rules), `crates/mb-library/src/xml.rs` (`Command::ImportXml`). Format: `../xml.md`.

## Where the XML comes from
- **The "rekordbox xml" tree** is rekordbox's bridge (`db::DjPlaylistsBridge::initialize` @0x100fdbea4). It reads the file named by the preference `bridgeImportedLibraryFile` (`rekordbox3.settings`). A missing or unreadable file shows "The rekordbox xml file specified is unable to be read properly."
- The preference `showRbXml` (0 on the user's machine) presumably switches the tree on (*hypothesis* from the name). musicbox opens the bridge file at startup only when it is 1. **File → Open rekordbox xml…** opens any file.
- **Parsing:** `DjPlaylistsAnalyzer::analyzeDjPlaylists` @0x101050b14 reads only `COLLECTION` (its `TRACK`s) and `PLAYLISTS` (`NODE`s). `PRODUCT` and `DJ_PLAYLISTS Version` are not looked at. A writer can therefore name itself in `PRODUCT`; musicbox writes `musicbox` there.

## Entry points
| User action | Path |
|---|---|
| Drag/"Import to Collection" on tracks | `ListViewer::importTrackFromBridge` @0x1003e9460 / `importTrackBridge` @0x1003e1fd4 → `BrowseHelper::prepareForImportXmlTracks` @0x101fcbe68 → `DatabaseIF::importBridgeTrack` @0x1020d7700 → `DatabaseMediator::importTrack` @0x101a0d1c8 |
| Import into a playlist | `ListViewer::importTrackBridgeToList` @0x1003e3b54 (same path, with the target list) |
| "Import Playlist" on a list or folder | `TreeViewer::treeMessageImportPlaylistFromBridge` @0x101569698 → `DatabaseIF::importBridgePlaylist` @0x1020d7970 → `DatabaseMediator::importPlaylist` @0x101a17f38 (tracks of the list and its sub-lists, `getPlaylistTrackRecursively`) → `importTrack` |

`importTrack` hands everything to `executeImport` @0x101a0dba4, then `ImportManager::run` @0x1001dcda4, the same thread as a file import. That thread runs `addTrack` @0x101a1577c per track and `create_analysis_file_for_import`. It then creates the lists (`addPlaylistInternal`) and fills them (`addTrackToPlaylistInternal`).

## Questions rekordbox asks
1. **A track whose file is already in the collection** (`isCollectionSong`, same path): `ImportXmlTrackAlert` @0x101fccfd0.
   - It says: "This track already exists in the current collection. Do you want to load information in the tag of the library being imported?"
   - It warns: "Beat Grid and Key information will not be loaded if the track is set locked out against analyzing."
   - It offers "Don't ask me again".
   - musicbox shows one checkbox for all such tracks (`XmlImport::update_existing`). It defaults to off, which leaves the track unchanged; the track is only added to the lists.
2. **A list with the same name at the target**: "One or several lists with the same name already exist. Do you want to replace them with the one you're importing?"
   - Yes deletes the old lists first. A name equal to the Cloud trial playlist is refused.
   - In musicbox this is `XmlImport::replace_same_name`. Undo restores the deleted lists.

## A new track (`addTrack`, source type 4 = rekordbox xml)
1. **Checks** (`TrackAdditionError`). The path must be absolute (3), the file must exist (4), the path must be at most 255 characters (2), the file type must be known from the extension and the file must be under 2 GiB (5). A failing track is skipped; musicbox reports the reason.
2. **The file is not copied**: the row points at the XML's `Location` (decoded per `../xml.md` § Location).
3. **Tag analysis runs** (`analyze_tag`), but `convertTagData` is skipped for this source type. The XML values stay.
   - A missing `Name` becomes the file name without extension.
   - From the file's tags the row takes only what the XML has no attribute for: album artist, original artist, lyricist, ISRC and release date. (RowDataTrack +0x140/+0x288/+0x380/+0x458/+0x460, matched to `convertTagData`'s field order.)
4. **Values from the XML** (`DjPlaylistsAnalyzer::analyzeTrack` @0x1010511f4):

   | XML | Column | Rule |
   |---|---|---|
   | Name, Artist, Composer, Album, Genre, Remixer, Label, Comments, Mix | Title, ArtistID, ComposerID, AlbumID, GenreID, RemixerID, LabelID, Commnt, Subtitle | lookup rows found or created by name, as in a file import |
   | Tonality | KeyID | by `ScaleName` |
   | AverageBpm | BPM | `(int)(long)(bpm × 100)`: **truncated**, so `127.93` (= 12792.999… in binary) gives 12792 |
   | TotalTime, Size, BitRate, SampleRate, DiscNumber, TrackNumber, Year, PlayCount | Length, FileSize, BitRate, SampleRate, DiscNo, TrackNo, ReleaseYear, DJPlayCount | as given; `Size` and `TotalTime` parsed as doubles and truncated |
   | DateAdded | StockDate | text |
   | Rating | Rating | value / 51 |
   | Colour | ColorID | the text compared, ignoring case, with `0xFF007F` … `0x660099` → 1–8; anything else: none |
   | Kind | — | unused. The type comes from the path (`DBToolBox::getFileType`), and `Kind`'s substring test (`MP3`, `AAC`, …) is case-sensitive, so German labels never match anyway |
   | Grouping, TrackID | — | not read for the row; the new row gets a new random ID like any import (*hypothesis*: `insertTrack` draws the ID as for a device import) |

5. **The insert** is the import-track builder insert (`import-track.md`), filled from those values. `MasterDBID`/`MasterSongID` are this library's, `ContentLink` = 14 and `Analysed` = 0.
   - With a colour, `setBPMColorIDKuvoComment` (vtable +0x4c8) sets `ColorID` once more after the insert. musicbox puts the colour into the insert and skips that second statement (*hypothesis* that it only rewrites the same value).
6. **`importRBXmlData` @0x101a161f0** runs when the XML has TEMPO entries (RowDataTrack +0x3fc bit 0x10):
   1. `getTrackBeatInfo` @0x101052944 turns the TEMPO entries into a per-beat grid (`mb_xml::import::beats_from_tempos`).
      - Every entry is a beat at `Inizio`.
      - Between entries, and from the last entry to `TotalTime`, beats are filled in every `60/Bpm` s with the bar position advancing modulo the `Metro` numerator.
      - A filled beat less than half a beat before the next entry is left out.
      - Entries with a non-positive `Bpm`, `Battito` or `Metro`, or not after the previous beat, or past `TotalTime`, are not beats. Exception: a first entry at 0.
      - The attributes of a skipped entry are still taken over for the fill.
      - Times: `(long)(s × 1000 + 0.5)` ms; tempo: `(long)(bpm × 100 + 0.5)`.
   2. The track's local analysis set is created (`createMusicAnalyzeFileFromAudioPath`) under the content UUID's folder. `set_pmai_beat_position(beats, n, 0)` and `write_pmai` then write the `.DAT`; the `.EXT` is not written.
      - musicbox writes the tag set every local `.DAT` has: PPTH `?/<file>`, PVBR with a zero index, PQTZ, zero PWAV (400) and PWV2 (100), and empty PCOB hot and memory lists.
      - *Hypothesis:* these are `initialize_pmai`'s defaults.
   3. Four column updates, one statement each:
      - `modTrackAnalysisPath` (`AnalysisDataPath`);
      - `modTrackBpm` (`BPM`, the value from the XML);
      - `modAnalyzeStatus`, which writes `Analysed` with bit 0x10 ("beat grid loaded from the XML") set, the other bits kept;
      - then `ContentLink`, unchanged.
      - The status word is `(Analysed | ContentLink << 8) & 0x3C86707B` (`getAnalyzeStatus` @0x1017e3730; write side `modAnalyzeStatus` @0x1017e37fc).
      - A new track therefore ends with `Analysed` = 16. rekordbox's later analysis treats the grid as loaded.
7. **Cues:** `getTrackCueInfo` @0x101053228 builds them, then `rekordboxDBController::setCue` (vtable +0x448) stores each one. These are the edit-cues.md steps 1–6 (insert, library sync, options, `CueUpdated`), without the mediator's playlist timestamp.
   - **Marks that are skipped:**
     - `Type` other than 0 or 4 (fade-in, fade-out and load marks);
     - `Start` < 0 or past `TotalTime`;
     - a loop whose `End` is before `Start` or past `TotalTime`;
     - `Num` outside −1…7.
   - **Kind:**
     - `Num` −1 with `Type` 4 → −1 (memory loop);
     - `Num` < 3 → `Num` + 1, so a −1 point is a memory cue (0);
     - otherwise `Num` + 2.
   - **Positions:** `(int)(s × 1000 + 0.5)` ms; frames = ms × 150 / 1000.
   - **Colour:** a hot cue with all three of `Red`/`Green`/`Blue` gets the colour code (1–64) whose pad colour (`djplay::getPadColor`) is nearest by Euclidean RGB distance; the first of equally near codes wins.
   - **Duplicates:**
     - a hot cue replaces an earlier one on the same pad;
     - a memory cue with an earlier memory cue's in and out point is dropped (*hypothesis*: the second loop of `getTrackCueInfo` read as "skip").

## A track already in the collection, "load information" = yes (`prepareForImportRBXmlTrack` @0x101a16fc4)
1. **`updateCollectionFromXml`** @0x100c5a590 copies the XML values that are non-empty (numbers: non-zero) and differ into the row, then calls `modifyTrack(flags, row)`, one metadata edit.
   - The fields are: title, artist, composer, album, genre, disc and track number, year, key, play count, rating, remixer, BPM (not on locked tracks), comment, label, Mix and colour.
   - musicbox runs one track-edit op per changed field (`edit-track.md`; USN count *hypothesis*). Not yet carried over: Mix, play count and BPM (no track-edit op for them).
2. **Beat grid:** unless the track is locked against analysis (`Analysed & 0x80`), `importRBXmlData(row, true)` runs.
   - It writes the same PQTZ into the existing `.DAT`, or creates one, then the same four updates.
   - It then sets `ContentLink |= 0xE` when a bit is missing.
3. **Cues:** when the XML has POSITION_MARKs, the track's cues are replaced. The old rows go (`updateCue`, vtable +0x4f0) and the XML's are added.
   - musicbox deletes them one by one with the delete-cue op, then adds the new ones. Statement grouping is a *hypothesis*.

## Lists (`ImportManager::run`)
- The dragged list, or for a folder the whole sub-tree, is created at the drop target. The tracks are imported first; each list then gets its entries in XML order, including tracks that were already in the collection.
- **`KeyType`:** 0 means `Key` = `TrackID`, 1 means `Key` = `Location` (`isEnableTrackID` @0x101051114).
- musicbox creates each top-level list as the first child of the target and the children of a folder in XML order.

## Undo (musicbox)
`Command::ImportXml` returns one `Command::Batch` that undoes everything in reverse order:
- the created lists are deleted;
- the new rows are removed (`remove-tracks.md`) and their analysis folders deleted;
- replaced lists are restored;
- for updated tracks, the edits, cue changes and grid (`RestoreBeatGrid` with the `.EXT` PQT2 kept) are reverted.

## Verification (`cargo test -p mb-library --test xml_ops`, `-p mb-xml`, `-p musicbox --test ui_xml`)
- **Import into another library:** `oracle-small`, exported from oracle-a and imported into a copy of `sandbox-initial`.
  - The five rows equal rekordbox's own rows for the same files in 17 columns: Title, BPM, Length, BitRate, SampleRate, FileSize, ReleaseYear, TrackNo, Commnt, StockDate, Rating, ColorID, FileType, and the artist, album, genre and key names.
  - The rebuilt grids have the same beat count up to `TotalTime`, bar positions and tempos.
  - 2559 of 4117 beat times are exact and the rest are within 2 ms.
  - That is the format's own loss, not the port's: the TEMPO reduction keeps a beat that matches the previous beat's prediction without a new entry, and the import refills a regular grid. rekordbox's own round trip has the same loss.
- **Updating a track already in the collection:** the title and a hot cue A come from the XML, with the colour code nearest to its RGB. Undo restores both. Without `update_existing` nothing changes.
- **Value rules:** unit tests in `mb_xml::import` cover the fill, the half-beat cut, the skipped entries, mark validity, kinds, duplicates, colour text, BPM truncation and the nearest pad colour.

## Open
- [ ] A traced import, for the statement order, the USN counts, whether `insertTrack` reuses the XML `TrackID` and the second `ColorID` statement.
- [ ] `createMusicAnalyzeFileFromAudioPath`'s exact initial tags, and whether an `.EXT` is created.
- [ ] What `showRbXml` and `RekordboxXmlChecked` control.
- [ ] Mix, play count and BPM updates for existing tracks (needs a track-edit op for them).
