# Op: edit track metadata (title, artist, album, genre, label, key, rating, colour, comment, …)

Status: **hypothesis**. No track field was edited in the 2026-10-11 trace (`INDEX.md` § Coverage gaps). The pattern is assembled from verified building blocks; it is what `mb-library` writes (`crates/mb-library/src/edit.rs`).

## musicbox pattern
One field per op, in one transaction:
```
[name fields only] lookup-or-create the new name                     -- import-track.md; +1 if a row is created
UPDATE-COL <Col> = <value>                                            -- common.md; +1
[name fields only, if the old ID differs] orphan step of the old row's table   -- +1 if it is orphaned
COUNTER TrackInfoUpdated += 1                                          -- common.md; +1
```
| Field | Column | Value |
|---|---|---|
| title, comment, lyricist | `Title`, `Commnt`, `Lyricist` | text |
| rating | `Rating` | integer 0…5 (`../xml.md`: XML = stars × 51) |
| colour | `ColorID` | text `"0"`…`"8"` (VARCHAR; `"0"` = none, as at import) |
| track/disc number, year | `TrackNo`, `DiscNo`, `ReleaseYear` | integer |
| artist, remixer, composer, original artist | `ArtistID`, `RemixerID`, `ComposerID`, `OrgArtistID` | `djmdArtist.ID` as text; NULL when cleared |
| genre, label | `GenreID`, `LabelID` | ID as text; NULL when cleared |
| key | `KeyID` | `djmdKey.ID` (ScaleName stored verbatim); `"0"` when cleared, as at import |
| album | `AlbumID` | album looked up as (Name, AlbumArtistID of the track's current album, Compilation 0); NULL when cleared |

Why these pieces:
- **UPDATE-COL**: every traced single-field change of a content row (analysis, rename) is one builder update of one column with one bump.
- **Lookup-or-create**: the import's exact-name lookups (`import-track.md`); analysis writes `KeyID` after its own lookup the same way (`analyse-track.md`).
- **Orphan step**: analysis runs the `djmdKey` orphan query after every `KeyID` write (`analyse-track.md` L63–64). musicbox runs the step of the replaced row's table (album, then artist, for an album change, because an orphaned album can orphan its album artist).
- **`TrackInfoUpdated`**: analysis bumps it after changing track info; the USB export reads `CueUpdated, AnalysisUpdated, TrackInfoUpdated` per track (`export-usb.md`), so a counter change is how a later export notices the edit.
- **masterPlaylists6.xml**: the playlists containing the track get a new `Timestamp`, as after analysis completion (`playlist-create.md` §5).
- An edit to the current value writes nothing.

## Undo (musicbox)
The inverse is the same field with the old value (by name for lookup fields). It runs as a new edit with new USNs; `TrackInfoUpdated` keeps counting up. Lookup rows created by the undone edit become orphans and are removed by its orphan step.

## Open questions
- [ ] Trace a title, artist, album, genre, label, key, rating, colour, comment and My Tag edit in rekordbox: statement order, whether the counter is bumped (and which one) and whether rating/colour count as track info.
- [ ] Does rekordbox run the orphan step after an edit, and for which tables?
- [ ] Album edits: does the new album keep the album artist? Album-artist edits (`djmdAlbum.AlbumArtistID`) are not implemented.
- [ ] BPM edits belong to beat-grid editing (ANLZ + `BPM`), not to this op.
- [ ] Multi-track edits: one statement per track, or `ID in (…)`?
