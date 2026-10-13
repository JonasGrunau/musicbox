# rekordbox.xml (collection export)

Status: **writer verified byte-exact.** `mb-xml` parses the fixtures/oracle-a export and writes it back identically. `rbdiff xml` also rebuilds the whole file from master.db + ANLZ, and that rebuild matches byte for byte too. Every TRACK attribute, all 242 TEMPO entries and the playlist tree match the library (0 unexplained mismatches). **POSITION_MARK and Colour are from code only**: no fixture track has a cue or a colour.

Status per item: **V** = verified against the fixture, **C** = from the binary, no fixture data, **H** = hypothesis.

## Where it comes from
- **Menu:** File → Export Collection in xml format. `db::DjPlaylistsWriter::write(File)` @0x1009759dc builds the writer and runs it as a `ThreadWithProgressWindow`.
- **Constructor** @0x101054f0c:
  - collection = `DatabaseIF::getAllCollection(false)`
  - top-level playlist nodes = `getListItem(1, 0, 2, 0)`
  - hot-cue colour type = `SettingIF::getCurrentHotCueColorType()` (stored at +0x28c)
  - builds the root and PRODUCT: `getProductName()` and `getFileVersion(true)`
- **run** @0x10105794c: `addCollectionNode`, then `addPlaylistsNode`, then `write`.
- **write** @0x101055610: `DjPlaylistsElement::writeToFile(file, dtd "", "UTF-8", lineWrap 60)` through a `juce::TemporaryFile`.
- **Element class:** `db::DjPlaylistsElement` is rekordbox's own small element class:
  - child list +0x08, attribute list +0x10, tag name +0x18
  - writer `writeElementAsText` @0x10105451c, `writeToStream` @0x101054c08, `setAttribute` @0x101054e44
  - escaper `DjPlaylistsOutputFunctions::escapeIllegalXmlChars` @0x10105494c
- **Import:** `db::DjPlaylistsAnalyzer` (`analyzeTrack` @0x1010511f4, `getTrackBeatInfo` @0x101052944, `getTrackCueInfo` @0x101053228, `locationFromURI` @0x101053ed0) reads files with `juce::XmlElement`. What the import writes is in `ops/import-xml.md`.
- **Preferences** (`rekordbox3.settings`):
  - `bridgeExportBeatGrid` selects the TEMPO mode (§ TEMPO).
  - `HotCueColorType` sets the default hot-cue colours.
  - `language` sets the UI language of the `Kind` labels. 2 = German on the user's machine; options.json has `lang` = `de` and `lang-path` = `german.lang` at the same time. The value mapping beyond 2 is *open*.
  - `bridgeImportedLibraryFile` and `showRbXml` belong to the import side.
- **PRODUCT is not read back:** `analyzeDjPlaylists` @0x101050b14 only looks at COLLECTION and PLAYLISTS, so a writer may name itself there. musicbox's export writes `musicbox` (`XmlExportOptions::product`).

## Format (V unless marked)
```
<?xml version="1.0" encoding="UTF-8"?>

<DJ_PLAYLISTS Version="1.0.0">
  <PRODUCT Name="rekordbox" Version="7.2.19" Company="AlphaTheta"/>
  <COLLECTION Entries="161">
    <TRACK TrackID=… Name=… Artist=… Composer=… Album=… Grouping=… Genre=… Kind=… Size=…
           TotalTime=… DiscNumber=… TrackNumber=… Year=… AverageBpm=… DateAdded=… BitRate=…
           SampleRate=… Comments=… PlayCount=… Rating=… Location=… Remixer=… Tonality=…
           Label=… Mix=… [Colour=…]>
      <TEMPO Inizio=… Bpm=… Metro="4/4" Battito=…/>            (0..n)
      <POSITION_MARK Name=… Type=… Start=… [End=…] Num=… [Red=… Green=… Blue=…]/>   (0..n)
    </TRACK>
  </COLLECTION>
  <PLAYLISTS>
    <NODE Type="0" Name="ROOT" Count=…>
      <NODE Name=… Type="0" Count=…> … </NODE>                       folder
      <NODE Name=… Type="1" KeyType="0" Entries=…><TRACK Key=…/>…</NODE>   playlist
    </NODE>
  </PLAYLISTS>
</DJ_PLAYLISTS>
```
Attribute order is the order of the `setAttribute` calls. It is fixed, and the line wrapping depends on it.
- The writer always emits all 25 TRACK attributes. `Colour` is the 26th, written only when the track has a colour.
- Elements without children are self-closing. That includes a TRACK without TEMPO/POSITION_MARK and an empty playlist (`Entries="0"/>`, as for "CUE-Analyse Wiedergabeliste" in the fixture).
- The root folder's attributes come in a different order from deeper NODEs:
  - root (`addPlaylistsNode` @0x101056400): `Type="0" Name="ROOT" Count=n`
  - deeper nodes (`addPlaylistNode` @0x1010566cc): `Name, Type, Count`, or `Name, Type, KeyType, Entries`

### TRACK attributes (`addTrackNode` @0x101055910)
| Attribute | Source | Written as | Status |
|---|---|---|---|
| TrackID | `djmdContent.ID` | `juce::String(int)`: signed 32-bit, so IDs ≥ 2^31 come out negative | V (no fixture ID ≥ 2^31) |
| Name | Title | text | V |
| Artist / Composer / Remixer | djmdArtist.Name via ArtistID / ComposerID / RemixerID | text | V (Composer empty) |
| Album / Genre / Label | djmdAlbum / djmdGenre / djmdLabel | text | V |
| Grouping | RowDataTrack+0x258. The import writes the colour *name* there (Pink…Purple), so it is the colour label (`djmdColor.Commnt` of ColorID) | text | H (empty in fixture; djmdColor names Pink…Purple) |
| Kind | `FileType` → `juce::translate` of `MP3 File`(1) `AAC File`(2) `MP4 File`(3) `M4A File`(4) `FLAC File`(5) `ALAC File`(6) `WAV File`(11) `AIFF File`(12), else `Unknown Format` | UI language. German: `Mp3-Datei ` `Aac-Datei ` `Mp4-Datei ` `M4a-Datei ` `FLAC-Datei` `ALAC-Datei` `Wav-Datei ` `Aiff-Datei ` `Unbekanntes Format`. **Note the trailing spaces**, which come from `german.lang` | V for 1, 4, 5, 6, 11, 12 (German) |
| Size | FileSize | signed 32-bit | V |
| TotalTime | Length (s) | u16 | V |
| DiscNumber | DiscNo | u16, 0 when NULL | V (all 0) |
| TrackNumber | TrackNo | i32, 0 when NULL | V |
| Year | ReleaseYear | u16, 0 when NULL | V |
| AverageBpm | BPM (×100) | `%.02f` of `bpm × 0.01` (multiply, constant 0x3F847AE147AE147B) | V |
| DateAdded | StockDate | text `YYYY-MM-DD` | V |
| BitRate | BitRate | i32 (0 for ALAC) | V |
| SampleRate | SampleRate | i32 | V |
| Comments | Commnt | text | V |
| PlayCount | DJPlayCount | u16 | V (all 0) |
| Rating | Rating (0..5) | `(char)rating × 51` → 0/51/102/153/204/255 | V (all 0) / C |
| Location | FolderPath | see § Location | V |
| Tonality | djmdKey.ScaleName | text | V |
| Mix | RowDataTrack+0x390 | text. Hypothesis: `Subtitle` (both empty in fixture) | H |
| Colour | ColorID 1..8 | `0xFF007F` `0xFF0000` `0xFFA500` `0xFFFF00` `0x00FF00` `0x25FDE9` `0x0000FF` `0x660099` (Pink, Red, Orange, Yellow, Green, Aqua, Blue, Purple). Omitted for 0 | C |

- **Spotify:** tracks with `RowDataTrack::isSpotifyFile` are skipped. They are not counted in `Entries` and not listed in playlists.
- **Collection order:** `getAllCollection` order. This equals djmdContent table (rowid) order in the fixture, and also `created_at` order there. **V** (161/161).

### Location (`locationToURI` @0x101056ce0)
- Output: `file://localhost`, followed by the path's UTF-8 bytes.
- A byte is copied unchanged when `CharacterFunctions::isLetterOrDigit(char)` holds (ASCII letters and digits only) or when it is one of `!#$()*+,-./:;=?@_~`.
- Every other byte becomes `%` plus two **lowercase** hex digits. That covers space, `&`, `'`, `[`, `]`, `%` and every byte ≥ 0x80.
- **V** on 161/161, including `%20 %26 %27` and the non-ASCII bytes of a mojibake path (`%c3%83%c2%a5`).
- **Import** (`locationFromURI`):
  1. Decode every `%xx`, with hex digits in either case.
  2. Read the bytes as UTF-8.
  3. Keep what follows the first `file://localhost`, matched case-insensitively. Without the prefix the path is empty.

### TEMPO (`addTempoNode` @0x101057074)
- **Source:** `DatabaseIF::getTrackInfo(path)`. For a collection track this loads the beat grid from the track's ANLZ files through `CAnalyzerIF::load_info` (`DatabaseMediator::getTrackInfo` @0x101e55948).
- **Beat format:** 8-byte beats `u16 beat_in_bar, u16 tempo×100, u32 ms`, the PQTZ layout.
- **Setting `bridgeExportBeatGrid`** (`common::SettingFile`, default 1):
  - 0 or less: no TEMPO
  - 1: reduced list (below)
  - 2: one TEMPO per beat
  - any other value: none
- **Values:**
  - `Inizio` = `%.03f` of `ms × 0.001` (multiply by the constant 0x3F50624DD2F1A9FC)
  - `Bpm` = `%.02f` of `tempo × 0.01`
  - `Metro` = constant `4/4`
  - `Battito` = beat_in_bar

**Reduction rule** (mode 1). Two running predictions of the next beat time, in ms as doubles, both starting at 0:
- `grid` advances from the last anchor.
- `local` is the previous beat's own time plus one beat length.
- `last_tempo` starts at 0.

For each beat (time `t`, tempo `T`):
1. The beat is **written** if `T != last_tempo`, or if `t` equals none of `trunc(grid)`, `trunc(grid+0.5)`, `trunc(local)` and `trunc(local+0.5)`. Here `trunc` is a conversion to unsigned that truncates toward zero (`fcvtzu`).
2. If it is written: `last_tempo = T`. If `t != trunc(grid)`, then `grid = (t == trunc(grid+0.5)) ? grid : t`. In other words, the fractional prediction is kept while the beat lands on it, and otherwise the grid restarts at `t`.
3. In every case: `len = 6000000.0 / T`, `grid += len`, `local = t + len`.

**V**: `mb_xml::export::tempo_anchors` reproduces all 242 TEMPO entries of the 161 tracks exactly.
- **The fixture has no in-track tempo changes.** Analysis was in Normal (constant-tempo) mode, so even `tempo_120_to_126` has one PQTZ tempo.
- Each track gets one entry at its first beat (161 entries). The other 81 come from grid discontinuities: an integer-ms PQTZ beat 1 ms away from both rounded predictions.
- Example: track 105478531 at 120.00 BPM. Beat 82 is at 41020 ms, but the prediction is exactly 41019.000, so it gets a new TEMPO with `Battito` set to its own bar position.
- Distribution: 137 tracks have 1 entry, 16 have 2, 5 have 3, and one each has 6, 9 and 43.
- So TEMPO is "one entry per tempo change **or** grid jump of ≥ 1 ms". It is not a list of tempo changes alone.
- On import, `getTrackBeatInfo` reads `Inizio`/`Bpm`/`Battito`/`Metro` back.

### POSITION_MARK (`addPositionMarkNode` @0x1010574b0), C (no fixture cues)
- **Source:** `TrackInfo` +0x80, an `OwnedArray<analyzer::Cue>`. For collection tracks it is filled from the database cue rows by `getContentCueInfo` (main controller vtable +0x438, row order) and `AppSyncDBController::set_cueInfo` @0x100a4c304 per row, then `checkCueInfos` @0x101e5610c. These are the same steps the USB export uses (`export.md` § 1.6 "Cues"; `mb_usb::cues::checked_cues`). It does not come from the ANLZ files.
- **`OutMsec` −1 becomes end 0** in `set_cueInfo`, so a cue point is `Type` 0. A `Kind` 0 row with an out point is read back as −1 (memory loop), and an active memory loop as 4. All three are written as `Num` −1.
- **Cue fields used:** kind +0x0c, start ms +0x10, end ms +0x18, name +0x38, colour index byte +0x4c.

**Num** from the cue kind (the `djmdCue.Kind` value):

| Kind | Num | Meaning |
|---|---|---|
| 1..3 | 0..2 | hot cues A–C |
| 5..9 | 3..7 | hot cues D–H |
| -1, 0, 4 | -1 | memory cue |
| anything else | — | skipped |

**Attributes, in this order:**
1. `Name`: the cue's name/comment
2. `Type`: `4` if end ≠ 0, else `0`. The writer never emits 1, 2 or 3 (fade-in, fade-out, load)
3. `Start`: `%.03f` of `start × 0.001`
4. `End`: only for Type 4
5. `Num`
6. `Red`, `Green`, `Blue`: hot cues only (Num ≥ 0)

**Hot-cue colour** comes from `CAnalyzerIF::getHotCueDefaultColor(num+1, start < end, colourIndex, hotCueColorType, false)` @0x10162e784:
- A colour index of 1..64 is looked up in the palette.
- Otherwise the index is set to a default first:
  - 0x24 (36) for loops
  - else a value from the tables `hcTypeColorFull`/`hcTypeCold2`/`hcTypeCold1`, chosen by colour type 4/3/2
  - else 0x16 (22), or 0x1a (26) for hot cues above 8
- Memory cues get no RGB.
- **The palette is the UI one, not the device one** (corrected 2026-10-13): `addPositionMarkNode` calls `getHotCueDefaultColor` with the device flag off, which reads `djplay::getPadColor` @0x10027e378 (`mb_usb::cues::UI_PAD_COLORS`); the device PCO2 colours use `getDevicePadColor` (`PAD_COLORS`, 410/410 reference-stick cue sets, `export.md` § 1.6). Both 65-entry tables are filled by the `PadColour.cpp` initialiser @0x10027ecc8. The default codes are the three `hcType*` arrays (`default_hot_cue_code`). **Verified** on rekordbox's own export of the sandbox (hot cue A with code 8 → 69,172,219; hot cue C without a code, `HotCueColorType` 4 → code 23 → 125,193,61); oracle-a had no coloured hot cue, so its byte-exact match could not show this. musicbox's export of that sandbox (171 tracks, smart lists, imported and musicbox-analysed tracks) equals rekordbox's byte for byte apart from `PRODUCT`.
- The writer takes hot cues A–H only (`Num` 0–7). Pads above H (kinds 10–17) are not written.
- **musicbox** (`mb_library::xml::position_marks`) writes the marks in that order with those rules. `tests/xml_ops.rs` `cues_and_colours_in_the_export` covers a memory cue with a comment, a memory loop, a hot cue with a colour code, a default-coloured hot cue, a hot loop and a pad above H. This is checked against the rules above, not yet against a rekordbox-written XML with cues.

**Import** (`getTrackCueInfo`; full rules in `ops/import-xml.md`):
- **Skipped marks.** A mark is dropped unless:
  - `Type` is 0 or 4 (1..3 are dropped)
  - 0 ≤ `Start` ≤ TotalTime, and for loops `Start` ≤ `End` ≤ TotalTime
  - `Num` is in -1..7
- **Number parsing:** commas in `Start`/`End` are replaced before parsing.
- **Kind:** Num -1 with Type 4 gives kind -1; Num < 3 gives Num+1 (so memory = 0); otherwise Num+2.
- **Positions:** ms = `(int)(sec × 1000 + 0.5)`, and frames = ms × 150 / 1000.

### Playlists
- **Tree:** folder (`ListDataPlaylist`+0x94 == 1) → `Type="0"`, with `Count` = number of children, then the children in list order.
- **Playlists:** anything else, including smart lists, which `getTrackList` evaluates → `Type="1" KeyType="0" Entries=n`, then one `<TRACK Key="<ID as signed int>"/>` per non-Spotify track, in playlist order.
- **KeyType="1"** (Key = Location URI) is only accepted on import (`isEnableTrackID`).
- **V:**
  - The fixture's three top-level playlists match djmdPlaylist (ParentID `root`, ordered by `Seq`).
  - Their entries match djmdSongPlaylist ordered by `TrackNo`, for both oracle-small (5) and oracle-all (161).

## Layout rules for byte-exact output (V)
These come from `writeToStream`/`writeElementAsText` with indent 0 and lineWrap 60.

**Header and line ends**
1. `<?xml version="1.0" encoding="UTF-8"?>`, CRLF, CRLF (an empty line), then the root element, then a final CRLF.
2. Every newline is CRLF (`juce::newLine`). There is no BOM.

**Indentation and wrapping**
3. An element at indent *n* starts with *n* spaces and `<NAME`.
4. Each attribute is written as ` name="value"`: a leading space, raw name, `="`, the escaped value, `"`.
5. **Wrapping:** keep a counter of bytes written for attributes on the current line, starting at 0.
   - Before each attribute, if the counter is **> 60**, write CRLF plus `n + len(NAME) + 1` spaces and reset the counter to 0.
   - The counter includes the leading space and both quotes, and counts escaped bytes (UTF-8 bytes, not characters).
   - The fixture pins the threshold exactly: every observed wrap had a counter ≥ 61, and every non-wrap ≤ 60.
6. **Closing:** with no children, `/>`. Otherwise:
   - `>`
   - for each child element: CRLF, then the child at indent *n*+2
   - CRLF, *n* spaces and `</NAME>`
   - Text children are written inline and suppress the newline/indent around them (rekordbox never writes text).

**Escaping and numbers**
7. **Escaping:**
   - `&`→`&amp;`, `"`→`&quot;`, `'`→`&apos;`, `<`→`&lt;`, `>`→`&gt;`
   - Inside attribute values, CR, LF and every other character below U+0020 (including TAB) are **dropped**. In text, CR/LF are kept.
   - Everything else is written as raw UTF-8. There are no numeric character references, and non-ASCII is not escaped.
8. **Numbers:**
   - Integers are plain decimal.
   - BPM uses `%.02f` and seconds use `%.03f`, both through `vsnprintf`: round-half-even on the exact binary value. Rust's `{:.2}`/`{:.3}` give identical results.

## Implementation
- `crates/mb-library/src/xml.rs`:
  - `export_document`: the whole document from master.db, in `djmdContent` row order. It adds per track the PQTZ of the local `.DAT`, `FileType`, `Subtitle` (Mix), `ColorID` and its `djmdColor.Commnt` (Grouping), and the cue rows.
  - `write_document`: writes a temporary file and renames it into place.
  - `XmlExportOptions::from_settings`: the preferences above.
  - The app's **File → Export Collection in xml format…** runs it in the background.
- `crates/mb-xml/src/import.rs`: the import's value rules (`ops/import-xml.md`).
- `crates/mb-xml`:
  - `dom.rs`: generic element tree. quick-xml reader; writer implementing the layout rules above.
  - `model.rs`: typed `Document`/`Track`/`Tempo`/`PositionMark`/`PlaylistNode`. All TRACK attributes are `Option`, so absent ones stay absent. Unknown attributes and elements are kept in `Extra` and written after the known ones. `Count`/`Entries` are recomputed when writing.
  - `export.rs`: library → XML rules. These cover the ID sign, ×0.001/×0.01 conversions, rating, colours, Kind labels, `location_to_uri`/`location_from_uri`, `tempo_anchors`/`tempos_from_beat_grid`, `position_mark`, `track_from_core` and `playlists_from_snapshot`.
- **Tests** (`crates/mb-xml/tests/fixture.rs`):
  - fixture → tree → bytes identical
  - fixture → typed → bytes identical
  - typed → bytes → typed equal
  - unknown content survives

## Comparison: `rbdiff xml fixtures/oracle-a fixtures/oracle-a/rekordbox.xml`
- **Input:** XML with 161 tracks, written at 20:07:01 UTC; master.db with 169 tracks and 0 cues. The Kind language is detected as German.
- **Tracks:** every per-track field matches on 161 of 161 tracks, with no unknown attributes. The checked fields are:
  - TrackID, Name, Artist, Composer, Album, Grouping, Genre, Kind, Size, TotalTime, DiscNumber, TrackNumber, Year, AverageBpm, DateAdded, BitRate, SampleRate, Comments, PlayCount, Rating, Location (decoded and encoded), Remixer, Tonality, Label, Mix, Colour
  - TEMPO vs the PQTZ reduction
  - POSITION_MARK vs djmdCue
- **Collection order:** matches djmdContent rowid order.
- **Playlists:** 4 nodes; names, KeyType and entry lists all match.
- **Explained difference:** 8 tracks exist only in master.db. They are the GROOVE CIRCUIT sampler WAVs (House 1…, Breaks 3), created at 20:23:24 UTC, after the XML was written.
- **Rebuild:** a rebuild from master.db + ANLZ, restricted to the 161 exported tracks, is **byte-exact** (117,642 bytes).
- **Result:** 0 unexplained mismatches.

## Open questions
- [ ] **POSITION_MARK on real cues:** a rekordbox-written XML of tracks with memory cues, hot cues A–H, loops and cue colours, to confirm the order and the colours. The pieces themselves are now known from the binary (§ POSITION_MARK).
- [ ] `Grouping` = colour name and `Mix` = Subtitle. Both are empty in the fixture. musicbox writes `djmdColor.Commnt` (e.g. `Orange` for colour 3) and `Subtitle`; check against a rekordbox export of a coloured track with a subtitle.
- [ ] `Colour` for ColorID 1..8. Constants are from the binary; no fixture yet.
- [ ] TEMPO with real tempo changes (Dynamic analysis) and in mode 2. The rule is from code and only verified for constant-tempo grids.
- [ ] Rating ≠ 0, PlayCount ≠ 0, DiscNumber ≠ 0, IDs ≥ 2^31 (negative TrackID/Key), English UI `Kind` labels, Spotify tracks, smart playlists and folders. None of these appear in the fixture.
- [ ] Collection order: rowid vs `getAllCollection`'s own ordering after deletes and re-adds.
