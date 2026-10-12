# Intelligent playlists (smart lists)

Status: **binary-derived, not traced.** The format, parser, evaluator, serialiser and write path come from rekordbox 7.2.19's arm64 slice (Ghidra, plus the property, operator and unit tables read from `__DATA`). No fixture library contains a smart list, so nothing here has been compared with a list rekordbox wrote. Code: `crates/mb-core/src/smartlist.rs` (format and evaluation), `crates/mb-library/src/playlist.rs` (`create_smart`, `set_smart_list`), `apps/musicbox/src/smartlist_ui.rs` (editor).

## Storage
- A smart list is a `djmdPlaylist` row with `Attribute` 4. Its rules are XML text in the row's `SmartList` column.
- The tracks are **not stored**. There are no `djmdSongPlaylist` rows, and the list is evaluated again on every read (§ Evaluation).
- `masterPlaylists6.xml` lists the node with `Attribute="4"` like any other playlist.
- `rekordbox.xml` exports a smart list as a plain playlist (`Type="1"`) of its current tracks. `DjPlaylistsWriter::addPlaylistNode` @0x1010566cc calls `getTrackList` with the list's attribute.

## XML format
```
<?xml version="1.0" encoding="UTF-8"?>

<NODE Id="<playlist ID as signed i32>" LogicalOperator="1|2" AutomaticUpdate="0">
  <CONDITION PropertyName="<name>" Operator="<code>" ValueUnit="<unit>" ValueLeft="<v>"
             ValueRight="<v>"/>
  …
</NODE>
```
- **One `NODE` with flat `CONDITION` children.** There is no nesting.
  - Writer: `SmartPlaylist::Node::createNode(Id, LogicalOperator, AutomaticUpdate)` @0x102166784 and `SmartPlaylist::Condition::createCondition` @0x102164dd8.
  - Attribute order: `Id LogicalOperator AutomaticUpdate` and `PropertyName Operator ValueUnit ValueLeft ValueRight`.
- **`Id` is the playlist's ID**, written through `XmlElement::setAttribute(int)`, so IDs at or above 2³¹ come out negative.
  - When an existing list is edited, the dialog passes its own list ID (`SmartPlaylistComp+400`).
  - The evaluation never reads `Id`.
- **`LogicalOperator`** is the ID of the "Match … following conditions" combo box: 1 = "all of the", 2 = "any of the".
- **`AutomaticUpdate`:** `SmartPlaylistComp::buttonClicked` @0x102165ff0 always calls `createNode(…, false)`, so the stored value is `0`. This holds even though the dialog has an "Update automatically" toggle. The evaluation never reads it either.

### Serialisation (verified in the binary)
`buttonClicked` stores `XmlElement::toString(TextFormat)` with `dtd` set to "" and `customEncoding` set to `UTF-8`. The `TextFormat` defaults (`TextFormat::TextFormat` @0x1024acb40) are header on, `lineWrapLength` 60 and `newLineChars` CRLF. That gives:
- **Header:** `<?xml version="1.0" encoding="UTF-8"?>`, then two CRLFs, the element at indent 0, and a final CRLF.
- **Wrapping and indentation** (`writeElementAsText` @0x1024f2938):
  - Before each attribute, if the bytes written for attributes on the current line exceed 60, write CRLF and then indent + tag length + 1 spaces.
  - Children go on their own lines, two spaces deeper.
  - An element without children ends with `/>`.
- **Escaping in attributes** (`XmlOutputFunctions::escapeIllegalXmlChars` @0x1024f3014):
  - ASCII characters in the legal table @0x103882c84 are written as they are. That table is space, letters, digits and ``!#$%'()*+,-./:;=?@[\]_{|}~``.
  - `"` `&` `<` `>` become `&quot; &amp; &lt; &gt;`.
  - Everything else becomes `&#<decimal code point>;`. That covers CR, LF, the backtick, control characters and every non-ASCII character (`é` → `&#233;`).

## Properties
Order and labels as in the dialog. `PropertyName` values come from `xml_property_name1..5` @0x1052e9f90, labels from `property_name1..5` @0x1053afe20. The field is the `RowDataTrack` offset that `db::operate` @0x1020b5010 reads; columns were cross-checked against `DjPlaylistsWriter::addTrackNode`.

| Label | `PropertyName` | Kind | Track field |
|---|---|---|---|
| Track Title | `name` | text | +0x20 Title |
| Artist | `artist` | text | +0xd0 artist name |
| Album | `album` | text | +0x108 album name |
| Genre | `genre` | text | +0x180 genre name |
| Album artist | `albumArtist` | text | +0x148 album artist name |
| Comments | `comments` | text | +0x360 Commnt |
| Composer | `producer` | text | +0x338 composer name |
| Mix name | `mixName` | text | +0x390 Subtitle |
| Remixer | `remixedBy` | text | +0x300 remixer name |
| Label | `label` | text | +0x1b8 label name |
| Key | `key` | text | +0x210 key name (ScaleName) |
| Original artist | `originalArtist` | text | +0x290 original artist name |
| File name | `fileName` | text | +0xa8 FileNameL |
| BPM | `bpm` | number | +0x3a8 BPM×100 (int) |
| Time | `duration` | number | +0x3d8 Length in s (u16) |
| Year | `year` | number | +0x37a ReleaseYear (u16) |
| DJ play count | `counter` | number | +0x3ac DJPlayCount (u16) |
| Rating | `rating` | number | +0x378 Rating 0..5 (char) |
| Release Date | `dateReleased` | date | +0x380 ReleaseDate |
| Date Added | `stockDate` | date | +0x3c8 StockDate |
| Date Created | `dateCreated` | date | +0x3c0 DateCreated |
| My Tag | `myTag` | My Tag | +0x440 the track's My Tag IDs |
| Color | `grouping` | colour | +0x240 ColorID (int) |

Unknown names are dropped by the parser. Names are compared case-insensitively.

## Operators and values
Operator codes are indexes into `xml_operator` @0x1052ea088 (`==`, `!=`, `>`, `<`, "is in the range", "is in the last", "is not in the last", "contains", "does not contain", "starts with", "ends with"). The dialog offers these per kind (`ConditionComp::updateComboBoxCondition` @0x10216a0e4):

| Kind | Operators |
|---|---|
| text | 1 is, 2 is not, 8 contains, 9 does not contain, 10 starts with, 11 ends with |
| number | 1, 2, 3 greater, 4 less, 5 in the range |
| date | 1, 2, 3, 4, 6 in the last, 7 not in the last, 5 |
| My Tag | 8, 9 |
| colour | 1, 2 |

Stored values (the dialog's code):
- **Text:** `ValueLeft` is the text.
- **BPM:** decimals are stored ×100 (`createCondition`: `"128"` → 12800, `"128.5"` → 12850). An empty `ValueUnit` or `ValueRight` becomes `0`.
- **Time:** minutes × 60 + seconds (`textEditorTextChanged` @0x10216e708).
- **Rating:** the star count 0..5 (`sliderValueChanged` @0x10216e5dc). When the second field is hidden, `ValueRight` is `"0"`. On reading, values above 50 are divided by 51, so old 0..255 lists still work.
- **Dates:** `YYYY-MM-DD` (`DateComboBox::getTimeAsString` @0x1011042f4).
  - For "in the last" / "not in the last", `ValueLeft` is a count and `ValueUnit` is `day` or `month`.
  - Keys come from `xml_range_unit` @0x1052ea070. The labels are "day(s)" and "month(s)".
  - The parser tests `ValueUnit` case-insensitively against `month`. Anything else counts as days.
- **My Tag:** `ValueLeft` is the `djmdMyTag` ID.
- **Colour:** `ValueLeft` is the colour ID 1..8 (Pink, Red, Orange, Yellow, Green, Aqua, Blue, Purple).

**Which conditions are kept** (`getSmartlistCondition` @0x1020b57fc, final check @0x1020b6438):
- The property must be known.
- The operator must be in 1..7, or in 8..11 with a non-empty value string.
- Only text and My Tag conditions fill that string, so operators 8..11 are dropped for numbers, dates and colours.
- `getSmartlistNode` @0x1020b4c88 sets `LogicalOperator` to 1 when there are conditions and the stored value is neither 1 nor 2. A node with no kept condition yields an **empty** list.

## Evaluation (`db::getSmartlistContentData` @0x1020b4684)
- **Which tracks and in what order:**
  - Every collection track in `getAllCollection` order, which is `djmdContent` rowid order (`xml.md`).
  - Spotify tracks are skipped.
- **Matching:** all conditions with `LogicalOperator` 1, any of them otherwise.
- **Display order:** the result has no order of its own beyond this. Each track's track number is set to its position.
- **Text** (`operateString` @0x1020b550c):
  - If the track field is empty: "is" matches when the value is also empty, and "is not" matches when the value is non-empty. Every other operator fails.
  - If the value is empty: only "is not" matches.
  - Otherwise `db::CollationRule` @0x1020b38cc…@0x1020b4274 compares using an ICU `StringSearch` (US locale) whose collator is set to **primary strength** (`CollationRule::CollationRule` @0x1020b4e44). Case, accents and width are ignored.
  - `equals`, `contains`, `startsWith` and `endsWith` match pattern characters against the text one collation element at a time.
- **Numbers:**
  - A negative BPM or colour value never matches.
  - 1 is `==`, 2 is `!=`, 3 is track > value, 4 is track < value, 5 is left ≤ track ≤ right. Every other operator fails.
  - Verified in the disassembly: operator 1 for BPM jumps to the equality test @0x1020b5270 (the decompiler showed a false fall-through).
- **Dates:**
  - `dateToDay` @0x1020b568c needs exactly 10 characters.
  - It reads the digits by position and does not check the separators. It runs them through `mktime` at local midnight, adds the local offset and divides by 86400.
  - The result is 0 (no match) for anything else, and for dates at or before 1970-01-01.
  - Operators: 1 `==`, 2 `!=`, 3 `>`, 4 and 7 `<`, 6 `≥`, 5 range.
- **"In the last N days"** (`pastDayToDay` @0x1020b6600) is today's local day number − (N − 1). So "last 1 day" means today only.
- **"In the last N months"** (`pastMonthToDay` @0x1020b6524) is quirky:
  - It takes the current local date and time, goes back N − 1 months, and sets the day of the month to 0. That is the **last day of the month before**.
  - The result goes through `mktime` and is divided by 86400 without the offset.
  - So "in the last 1 month" in October covers 30 September onwards.
- **My Tag:** operator 8 matches when the track has the tag; 9 matches when it does not.

## Writing (master.db)
- **Creating:** `rekordboxDBController::addPlaylist(parent, attribute, name)` @0x1017e614c calls `AppSyncDBController::insertPlaylist` @0x100a7cf0c with attribute 4. The insert is the traced playlist create (`ops/playlist-create.md`) and does not set `SmartList`.
- **Rules:** `DatabaseMediator::modifySmartListCriteria` @0x100c4c4c8 calls `AppSyncDBController::modifySmartPlaylistCriteria` @0x100a7ee90. It updates `SmartList` through the same one-property playlist update as `modifyPlaylistName` @0x100a7e700 (the traced rename): one builder update, one USN.
- **Edit dialog:** `browse::TreeViewer::treeMessageEditSmartList` @0x10156ba98 opens the dialog for an existing list. On OK it writes the criteria, then renames through `PlaylistSyncManager::DbProxy::modifyListName` when the name changed.
- *Hypothesis:* the creation flow is insert first and criteria after the dialog. The stored rows do not depend on that order.
- *Hypothesis:* the criteria update sets the list's `masterPlaylists6.xml` timestamp to now, like a rename.

## USB export
- Sync Manager's `playlists3*.sync` files list smart lists (`Attribute="4"`) with a `Dev_ID`. On the reference stick they exist on the device as ordinary playlists of their tracks.
- `mb-usb` still refuses smart lists (`export.md` § 8). Exporting the evaluated tracks as a plain list is the follow-up.

## musicbox
| Area | Behaviour |
|---|---|
| `mb_core::smartlist` | Parse, serialise (byte-compatible with the JUCE writer above) and evaluate |
| Text matching | Approximation of ICU primary strength: NFKD, combining marks removed, lower case, `ß` → `ss` |
| Time zone | The app passes the local UTC offset (`localtime_r`); `mb-xml` uses UTC |
| Snapshot | `LibrarySnapshot::smart_lists` holds each smart list's XML; `Track` gained `album_artist`, `mix_name`, `file_name` and `date_created` for the evaluator |
| `mb-library` | `Command::CreateSmartList` runs the create statements, then the criteria update. `Command::SetSmartList` runs one builder update, with undo |
| Allowed on smart lists | Rename, move and delete (with restore) |
| Refused on smart lists | Entry ops |

## Open questions
- [ ] Trace rekordbox creating and editing a smart list. Confirm the statement order, the `Id` value of a new list, and the exact `SmartList` bytes.
- [ ] What "Update automatically" does, given that `createNode` always gets `false`.
- [ ] `DatabaseMediator::fixMyTagIdsInSmartlistCriteria` @0x101942a5c (My Tag ID remapping; probably cloud or import).
- [ ] How close the NFKD approximation is to ICU primary strength for non-Latin scripts (kana, ligatures).
- [ ] How rekordbox exports a smart list to a device: as a snapshot of its tracks at export time, presumably via `getTrackList`.
