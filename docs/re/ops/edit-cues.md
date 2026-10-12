# Op: add, delete and edit cues (memory cues, hot cues, loops) of a collection track

Status: **binary-derived** (rekordbox 7.2.19 arm64, Ghidra). No cue was written in the 2026-10-11 trace (it only reads `djmdCue`), and no fixture library has cue rows, so nothing here is checked against rekordbox output yet. The M5 user check (`mb-library` example `m5_check`) is the first test. Implementation: `crates/mb-library/src/cues.rs`.

## Where collection cues live
- **Only in master.db.** `rekordboxDBController::setCue` @0x1017deef4 calls `CAnalyzerIF::setDeviceCue` only for device databases (types 1 and 2); the master controller (`AppSyncDBController`, type 3, set in its constructor @0x100a3acf8) writes the database rows and nothing else. The local `ANLZ0000.DAT/.EXT` keep their (empty) PCOB/PCO2 lists.
- **The USB export builds the device PCOB/PCO2 from these rows** (`getContentCueInfo`, vtable +0x438, then `getExportCue` @0x1009bc884; `../export.md` § 1.6). An exporter that copies the local cue tags instead loses every cue set in rekordbox.
- `djmdCue` is written through the ORM layer **without the library-sync columns** (`DaoBase<Cue, NoLibrarySyncSql>`): no USN bump, and `rb_data_status`, `rb_local_deleted`, `rb_local_synced`, `usn`, `rb_local_usn` keep their defaults (0/0/0/NULL/NULL).
- The cloud-sync mirror is `contentCue`: one row per track with the JSON of all its cue rows. It is written through `DaoBase<ContentCue, LibrarySyncSql>` with one USN bump per write.

## Columns of a new cue (`CloudAgentAPI::CueIF::setCue` @0x100a79374)
| Column | Value |
|---|---|
| `ID` | random u32 as decimal text, redrawn while 0 or 0xFFFFFFFF (`juce::Random::nextInt`; no collision check) |
| `ContentID` | the track's `djmdContent.ID` |
| `InMsec` | start in ms |
| `InFrame` | `(int)(InMsec × 0.15)`: 1/150 s, truncated (`djplay::DjplayDB::setHotCueData` @0x1001ff7f8) |
| `InMpegFrame`, `InMpegAbs` | 0, except for MP3s with a VBR seek table: `InFrame >> 1` and the byte offset from `MpegFrameOffset::GetMpegFrameOffset` @0x101633708 (open, see below) |
| `OutMsec`, `OutFrame` | loop end and its frame; a cue point has `OutMsec` −1, `OutFrame` 0 |
| `OutMpegFrame`, `OutMpegAbs` | as the In columns for loops; 0 for points |
| `Kind` | 0 memory cue or memory loop; hot cues A–C = 1–3, D onwards = 5–17 (pad n: n+1 for n < 3, else n+2; 4 is skipped) |
| `Color` | −1 |
| `ColorTableIndex`, `ActiveLoop`, `Comment`, `BeatLoopSize`, `CueMicrosec` | NULL (unless `addCueOption` follows) |
| `InPointSeekInfo`, `OutPointSeekInfo` | NULL; FLAC tracks get `"a,b,c"` seek points through `addCueOption2` (open) |
| `UUID` | new v4 |
| `ContentUUID` | NULL at insert, then the track's `djmdContent.UUID` (library sync) |
| `created_at`, `updated_at` | now, `YYYY-MM-DD HH:MM:SS.mmm +00:00` |

`rekordboxDBController::setCue` maps the player-side kinds: −1 and 4 (memory loop) are stored as `Kind` 0 with the out point; 0 is stored with `OutMsec` −1; hot cues keep their kind. Reading back (`AppSyncDBController::set_cueInfo` @0x100a4c304), `Kind` 0 with an out point becomes −1, and an `ActiveLoop` memory loop becomes 4.

## Sequence of one add (`DatabaseMediator::setCue` @0x101e529bc, browse type 1)
1. INSERT `djmdCue` (columns above). No USN.
2. **Library sync** (`CueRepository::librarySyncByContentId` @0x1005c9650):
   1. For every cue row of the track whose `ContentUUID` differs from the track's UUID: `UPDATE djmdCue SET ContentUUID = <content UUID>, updated_at = <TS> WHERE ID = <cue>`.
   2. `SELECT * FROM djmdCue WHERE ContentID = <id>` (no ORDER BY, so row order) → JSON array.
   3. USN bump, then the `contentCue` row with `ContentID = <id>`:
      - none: INSERT `ID` = content UUID, `ContentID`, `Cues` = JSON, `rb_cue_count` = number of rows, `UUID` = new v4, `rb_data_status` 0, `rb_local_deleted` 0, `rb_local_synced` 0, `rb_local_usn` = the USN, `created_at` = `updated_at` = now (`ContentCue::NewData` + `LibrarySync::NewStatus` @0x101a734bc; `rb_local_data_status` and `usn` are not set);
      - present: UPDATE `Cues`, `rb_cue_count`, `rb_data_status` = 257 if `usn` is set else 0, `rb_local_deleted` 0, `rb_local_usn`, `updated_at` (`LibrarySync::ToUpdateStatus` @0x101a73874).
3. **`addCueOption`** (vtable +0xb78 → @0x100a65658 → @0x100a6573c), only when the cue has a colour (`Color` byte < 0x80), a comment, a colour code, a beat-loop size or a sub-ms part. It updates all six option columns in one ORM update: `Color` (unsigned byte, so "no colour" is 255), `ColorTableIndex`, `ActiveLoop`, `Comment`, `BeatLoopSize` (two u16 halves), `CueMicrosec` (in/out sub-ms µs, zeroed unless one is 1–999), `updated_at`; then library sync again (+1 USN). An active-loop flag alone does not trigger it.
4. `deleteCueOption2` (vtable +0xbf8): only a select unless the cue is in a hot-cue bank list.
5. `addCueOption2` (vtable +0xbe8) for FLAC seek points (not reimplemented).
6. **`updateCueUpdated`** (vtable +0x608 @0x1017e5fe0 → `AppSyncDBController::modifyCueUpdated` @0x100a7caa0): `djmdContent.CueUpdated` = old + 1 as text through `CloudAgentAPI::modifyTable`, i.e. the COUNTER/UPDATE-COL pattern of `common.md` (+1 USN).
7. `update_playlist_timestamp([track])`: the playlists holding the track get a new `masterPlaylists6.xml` `Timestamp`.

USN cost: 2 for a cue without options, 3 with options.

## Delete (`rekordboxDBController::deleteCue` @0x1017de7ac, type 3)
`AppSyncDBController::deleteCue` @0x100a791b8 → `CueRepository::DeleteById` @0x1005c8778: `DELETE FROM djmdCue WHERE ID = <id>` (no USN, no soft branch), library sync (+1), then `updateCueUpdated` (+1). `hasCue` (vtable +0x9f0) is checked afterwards for the cached "has cues" flag (memory only).

## Edits (type 3: `rekordboxDBController::modifyCueColor` @0x1017ee19c and siblings)
The controller looks the cue up in the track's cue list, then calls one of:
| Edit | AppSync function | Column |
|---|---|---|
| memory-cue colour | `modifyCueColor` @0x100a65b04 (vtable +0xb90) | `Color` (unsigned byte) |
| comment | `modifyCueComment` @0x100a65c0c (+0xb98) | `Comment` |
| hot-cue colour code | `modifyCueColorTblIdx` @0x100a65d30 (+0xba0) | `ColorTableIndex` |
| active loop | `modifyCueOptionActiveLoop` @0x100a65e38 (+0xba8) | `ActiveLoop` |

Each is one ORM update of that column plus `updated_at` (`CueRepository::UpdateCue` @0x1005c77dc), library sync (+1), then `updateCueUpdated` (+1).

## `contentCue.Cues` JSON (`Cue::ToJson` @0x100c2b5a0, `EntityCommon::ToJson` @0x1000c25d0)
- An array `[` … `]` of objects joined by `,`, no spaces; each object `{` … `}` with `"Field":value` elements joined by `,`.
- Field order is the entity order: `ID, ContentID, ContentUUID, InMsec, InFrame, InMpegFrame, InMpegAbs, InPointSeekInfo, OutMsec, OutFrame, OutMpegFrame, OutMpegAbs, OutPointSeekInfo, Kind, Color, ColorTableIndex, ActiveLoop, Comment, BeatLoopSize, CueMicrosec, UUID, created_at, updated_at`. NULL columns are left out.
- `ID` and `ContentID` are numbers in quotes (`Field<Id>::ToJsonElement` @0x1000c2e74); integer columns are bare numbers; text, UUID and date columns are `juce::String::quoted` (a `"` is added at either end unless one is already there; **nothing is escaped**).

## musicbox
- `Command::AddCue`, `DeleteCue`, `EditCue` replay the sequences above inside one transaction per op (rekordbox runs them in autocommit; the stored result is the same).
- Undo of a delete re-inserts the identical row (`Command::RestoreCue`), then library sync and `CueUpdated` as for any cue change. Undo of an add deletes the cue; undo of an edit writes the old value.
- The MPEG and FLAC seek columns are left at 0/NULL.

## Open questions
- [ ] Verify everything above against rekordbox: trace a cue add, colour change, comment, delete in the instrumented copy (the clone quits after about 7 s; see `../tooling.md`), or diff a library before/after the user adds cues.
- [ ] `InMpegAbs`/`OutMpegAbs` for VBR MP3s (`GetMpegFrmOfst` @0x101800a60, which reads the file) and the FLAC seek-point strings (`FlacAnalyzeManager::getSeekPointAtTime`).
- [ ] The 64-entry hot-cue colour palette behind `ColorTableIndex` (`CAnalyzerIF::getHotCueDefaultColor` @0x10162e784 asks a colour object through vtable +0x170/+0x180). Eight codes are known from the stick's PCO2 tags (`../anlz.md` § PCO2).
- [ ] Whether the UI's hot-cue creation passes a default colour code (the "hot cue colour type" setting, `hcTypeColorFull`/`hcTypeCold1`/`hcTypeCold2`) so that `addCueOption` always runs for hot cues.
- [ ] Moving a cue (`DatabaseMediator::MoveCues` @0x101495740, `AppSyncDBController::modifyCues` @0x100a65f40) is not reimplemented.
- [ ] Hot cue bank lists (`djmdSongHotCueBanklist`, `hotCueBanklistCue`) are not covered.
