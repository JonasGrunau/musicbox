# Op: import one audio file into the collection

Status: **verified** (161 imports, L602–3684, 21:59:26–27). Building blocks: `common.md`.

## Trigger
A folder (or files) dropped on the collection. rekordbox imports file by file on one worker thread. With auto-analysis on, analysis starts about 30 s later (`analyse-track.md`).

## What import writes and what it does not
- **Writes:** `djmdArtist`, `djmdGenre`, `djmdAlbum`, `djmdLabel`, `djmdKey` (only rows that do not exist yet), then one `djmdContent` row.
- **Does not write:** `contentFile`, `djmdMixerParam`, `djmdCue`, artwork (`ImagePath` stays `""`), `AnalysisDataPath` (`""`), BPM/SampleRate/BitRate/BitDepth (0), `SearchStr` (NULL), the `*Updated` counters (NULL). These come from analysis (`analyse-track.md`). No side file is written.

## Batch frame
```
begin exclusive
commit                                            -- empty, once before the batch (L602)
<per-file transaction> × N
commit                                            -- stray, no open transaction (L3682); ignore
begin exclusive
commit                                            -- empty, once after the batch (L3683)
```

## Per-file transaction
Each step in brackets runs only if the tag value is non-empty. Each lookup that finds nothing is followed by a USN bump (B form) and an insert. A found row is reused and costs no USN.
```
begin exclusive
[artist]        select ID from djmdArtist where rb_local_deleted = 0 and Name = "<Artist>"
                  ↳ none: [USN bump] insert into djmdArtist (ID, UUID, Name, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at)
                                     values ("<RID32>", "<UUIDv4>", "<Artist>", 0, 0, <U>, 0, "<TS>", "<TS>")
[genre]         select ID from djmdGenre where rb_local_deleted = 0 and Name = "<Genre>"
                  ↳ none: [USN bump] insert into djmdGenre (ID, UUID, Name, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at) values (…)
[album, with album artist]
                select ID from djmdArtist where rb_local_deleted = 0 and Name = "<AlbumArtist>"
                  ↳ none: [USN bump] insert into djmdArtist (…)
                select ID from djmdAlbum where rb_local_deleted = 0 and Name = "<Album>" and AlbumArtistID = <AAID> and Compilation = 0
                  ↳ none: [USN bump] insert into djmdAlbum (ID, UUID, Name, AlbumArtistID, Compilation, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at)
                                     values ("<RID32>", "<UUIDv4>", "<Album>", "<AAID>", 0, 0, 0, <U>, 0, "<TS>", "<TS>")
[album, no album artist]
                select ID from djmdAlbum where rb_local_deleted = 0 and Name = "<Album>" and Compilation = 0
                  ↳ none: [USN bump] insert into djmdAlbum (ID, UUID, Name, Compilation, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at)
                                     values ("<RID32>", "<UUIDv4>", "<Album>", 0, 0, 0, <U>, 0, "<TS>", "<TS>")
[remixer]       select ID from djmdArtist where rb_local_deleted = 0 and Name = "<Remixer>"      ↳ none: create as for artist
[label]         select ID from djmdLabel where rb_local_deleted = 0 and Name = "<Label>"          ↳ none: create (same columns as genre)
[key]           select ID from djmdKey where rb_local_deleted = 0 and ScaleName = "<KeyTag>"
                  ↳ none: [USN bump] insert into djmdKey (ID, UUID, ScaleName, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at)
                                     values ("<RID32>", "<UUIDv4>", "<KeyTag>", 0, 0, <U>, 0, "<TS>", "<TS>")
select BaseDBDrive, CurrentDBDrive from djmdProperty
[USN bump]
insert into djmdContent (<column list, see below>) values (…)
commit
```
Then, outside the transaction, cache refresh reads: `select * from djmdContent where rb_local_deleted = 0 and ID = <CID>`, `select BaseDBDrive, CurrentDBDrive from djmdProperty`, and one `select * … where ID = …` per referenced album/artist/genre/label/key.

Observed order is fixed: artist → genre → album artist → album → remixer → label → key → content (all 100 tagged files). Original artist, composer and lyricist did not occur in the corpus; their position is *open* (the USB export uses artist, genre, album, remixer, original artist, composer, label, key; `../export.md` §1.5).

Notes:
- The album key is `(Name, AlbumArtistID, Compilation)`. `Compilation` was always 0; the compilation branch is *open*.
- Artist names are stored as the raw tag string: `"Moby,Victor Ruiz"` is one artist, not two.
- The key tag is stored **verbatim** as `ScaleName` (`Fm`, `Dmin`, `G#maj`, `9A`-style values would be stored as-is). Analysis later replaces `KeyID` with its own key (`analyse-track.md`), which may orphan these rows.
- No collision check on any generated ID.

## `djmdContent` insert
Column list (B dialect, this order; bracketed columns appear only when the value exists):
```
ID, UUID, FolderPath, FileNameL, FileNameS, Title, [ArtistID, AlbumID, GenreID,] BPM, Length, TrackNo, BitRate, BitDepth,
Commnt, FileType, Rating, ReleaseYear, [RemixerID,] [LabelID,] KeyID, StockDate, ColorID, DJPlayCount, ImagePath,
MasterDBID, MasterSongID, AnalysisDataPath, FileSize, DiscNo, Subtitle, SampleRate, Analysed, ReleaseDate, DateCreated,
ContentLink, ModifiedByRBM, HotCueAutoLoad, DeliveryControl, DeliveryComment, Lyricist, ISRC, SamplerTrackInfo,
SamplerPlayOffset, SamplerGain, rb_file_id, VideoAssociate, LyricStatus, ServiceID, OrgFolderPath, ExtInfo, DeviceID,
Reserved1, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at
```
Columns never written by import (stay NULL): `OrgArtistID`, `ComposerID`, `SearchStr`, `DisableQuantize`, `Tag`, `CueUpdated`, `AnalysisUpdated`, `TrackInfoUpdated`, `Reserved2–4`, `rb_LocalFolderPath`, `SrcID`, `SrcTitle`, `SrcArtistName`, `SrcAlbumName`, `SrcLength`, `usn`; `rb_local_data_status` defaults to 0.

| Column | Value at import | Derivation / evidence |
|---|---|---|
| `ID` | `<CID>` | random in [1, 2^28−1], `common.md` § IDs |
| `UUID` | `<UUIDv4>` | |
| `FolderPath` | absolute path of the audio file **including the file name** | 161/161 end with `/FileNameL` |
| `FileNameL` | file name with extension | |
| `FileNameS` | `""` | always empty (161/161, unchanged afterwards) |
| `Title` | tag title; if there is no title tag, the file name without extension | 61 untagged probes |
| `ArtistID`, `AlbumID`, `GenreID`, `RemixerID`, `LabelID` | ID from the lookup/create above, as text; **column omitted (NULL) when there is no tag** | |
| `KeyID` | ID of the key row as text, or **`"0"`** when there is no key tag (never NULL) | 106/161 `"0"` |
| `BPM`, `BitRate`, `BitDepth`, `SampleRate`, `Analysed` | 0 | set by analysis |
| `Length` | duration in whole seconds from the tag/decoder probe | analysis rewrites it; 44/161 end 1 s lower |
| `TrackNo`, `DiscNo`, `ReleaseYear` | tag numbers, 0 when absent (DiscNo was 0 for all) | |
| `Commnt`, `ISRC`, `ReleaseDate` | tag strings verbatim, `""` when absent (`ReleaseDate` is the raw tag, e.g. `2017-01-30`) | |
| `FileType` | 1 MP3, 4 M4A/AAC, 5 FLAC, 6 M4A/ALAC, 11 WAV, 12 AIFF | 161/161 by extension + codec |
| `Rating`, `DJPlayCount` | 0 | |
| `ColorID` | `"0"` | |
| `StockDate`, `DateCreated` | `<DATE>` (import day) | |
| `ImagePath`, `AnalysisDataPath`, `Subtitle`, `ModifiedByRBM`, `DeliveryComment`, `Lyricist`, `OrgFolderPath`, `Reserved1` | `""` | |
| `MasterDBID` | `<DBID>` | `djmdProperty.DBID` |
| `MasterSongID` | same as `ID` | 161/161 |
| `FileSize` | file size in bytes | |
| `ContentLink` | **14** | 161/161; see `analyse-track.md` for its later values |
| `HotCueAutoLoad`, `DeliveryControl` | `"on"` | |
| `SamplerTrackInfo`, `SamplerPlayOffset` | 0 | |
| `SamplerGain` | 0 (stored as REAL 0.0, FLOAT column) | |
| `rb_file_id` | the file's **inode number** (`juce::File::getFileIdentifier`), stored as text | `stat -f %i` matches 3/3 checked files; `AppSyncDBController::insertTrack` @0x100a56e3c |
| `VideoAssociate` | 0 (stored as text `'0'`) | |
| `LyricStatus`, `ServiceID` | 0 | |
| `ExtInfo` | `"null"` (JSON text of an empty value) | |
| `DeviceID` | `<DeviceID>` | `djmdProperty.DeviceID` = `djmdDevice.ID` |
| `rb_*`, `created_at`, `updated_at` | insert defaults, `common.md` | |

The tag reader itself (which frames/atoms feed Title, Commnt, ReleaseYear, …) is a separate topic. Some values are odd (one WAV got `Title = "M"`, `ReleaseYear = 3`); musicbox should match rekordbox's reader where possible, but those are tag-parsing questions, not write rules.

## Counters
- `localUpdateCount`: +1 per created artist/genre/album/label/key row, +1 for the content row. Here: 161 content + 93 artist + 14 genre + 97 album + 45 label + 30 key = 440 bumps (USN 526–965).
- `rb_local_usn` of each new row = its own bump.

## Example (normalised from L1745–1771)
```
begin exclusive
select ID from djmdArtist where rb_local_deleted = 0 and Name = "<Artist>"                → none
select int_1 as rb_local_usn from agentRegistry where registry_id = 'localUpdateCount'     → 683
update agentRegistry set int_1 = 684 where registry_id = 'localUpdateCount'
insert into djmdArtist (…) values ("<A>", "<uuid>", "<Artist>", 0, 0, 684, 0, "<TS>", "<TS>")
select ID from djmdGenre where rb_local_deleted = 0 and Name = "<Genre>"                   → found <G>
select ID from djmdAlbum where rb_local_deleted = 0 and Name = "<Album>" and Compilation = 0 → none
… bump 685, insert djmdAlbum
select ID from djmdArtist where rb_local_deleted = 0 and Name = "<Remixer>"                → none
… bump 686, insert djmdArtist (remixer <R>)
select ID from djmdLabel where rb_local_deleted = 0 and Name = "<Label>"                   → none
… bump 687, insert djmdLabel <L>
select ID from djmdKey where rb_local_deleted = 0 and ScaleName = "Fm"                     → none
… bump 688, insert djmdKey <K>
select BaseDBDrive, CurrentDBDrive from djmdProperty
… bump 689
insert into djmdContent (ID, UUID, FolderPath, FileNameL, FileNameS, Title, ArtistID, AlbumID, GenreID, BPM, Length, TrackNo, BitRate, BitDepth, Commnt, FileType, Rating, ReleaseYear, RemixerID, LabelID, KeyID, StockDate, …)
  values ("<CID>", "<uuid>", "<abs path>", "<file>", "", "<title>", "<A>", "<Al>", "<G>", 0, 410, 3, 0, 0, "<comment>", 12, 0, 2017, "<R>", "<L>", "<K>", "<DATE>", "0", 0, "", "<DBID>", "<CID>", "", <size>, 0, "", 0, 0, "<release date>", "<DATE>", 14, "", "on", "on", "", "", "<isrc>", 0, 0, 0, <inode>, 0, 0, 0, "", "null", "<DeviceID>", "", 0, 0, 689, 0, "<TS>", "<TS>")
commit
```

## Open
- Duplicate import (file already in the collection), compilation albums, original artist/composer/lyricist tags: not exercised.
- Whether `StockDate`/`DateCreated` use the local or the UTC date.
