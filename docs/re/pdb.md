# export.pdb / exportExt.pdb (Encirq DeviceSQL)

Status: **reader verified**. Every page and row of both fixture files decodes, and every header field below is either checked against the fixture on every page (`crates/mb-pdb/tests/fixture.rs`) or traced to the binary. The writer rules at the end are partly **hypothesis**; each one is marked.

Status words used below:
- **verified (fixture)**: holds on every page/row of `fixtures/stick/PIONEER/rekordbox/export.pdb` and `exportExt.pdb`.
- **verified (binary)**: read from the rekordbox 7.2.19 arm64 code at the address given.
- **hypothesis**: fits the evidence but is not proven.
- **open**: unknown.

Implementation: `crates/mb-pdb`. The fixture is a 2026-10-11 rekordbox 7.2.19 rewrite of an older stick (see `fixtures/manifest.toml`), so it carries deleted rows and update history. That makes it a good reader test and a poor template for byte-exact writer output.

## Files
| File | Content | Tables |
|---|---|---|
| `PIONEER/rekordbox/export.pdb` | Main device library: tracks, lookups, playlists, history, browse menus | 20 (types 0..19) |
| `PIONEER/rekordbox/exportExt.pdb` | Extension: My Tag tree, tag membership, extra options | 9 (types 0..8) |

`exportLibrary.db` (OneLibrary, SQLCipher) is a separate format, see `devlib.md`.

## Where the format lives in the binary
DeviceSQL is statically linked. Generated "EPL" code (`__epl_*`, `_edb_*_MODULEn_*`, `DEV_MODULEn_*` forms) sits on top of the `TE_*` runtime.

| What | Address |
|---|---|
| Schema of export.pdb: `_edb_DEV_MODULE1_table_inits` (calls `TETableCreate` for DJDBCONTENT/GENRE/ARTIST/ALBUM, then `__epl_DJDB*_create`) | 0x101456e2c |
| Schema of exportExt.pdb: `__epl_DJDBEX*_create`, called from `_edb_MAS_MODULE1_table_inits` | 0x100a19e70 |
| `TETableCreate(db, name, storage, 1, 1, ncols, colnames[], coltypes[], …)` | 0x102f49484 |
| `TE_TableCreate` (table number = DB-wide creation counter, `db+0x78`) | 0x102f84f88 |
| Column positions: `TE_ComputeRealPositions` | 0x102f747b0 |
| Header vs. no-header rows: `TE_DetermineFlatRowType`, `TE_ReComputeOffsets` | 0x102f74d98, 0x102f74c3c |
| Row encoder: `TE_RowFormSmgr` | 0x102f7572c |
| Row field access: `TE_GetAttrFR`, `TE_GetAttrFRNoHdr` | 0x102f74ed0, 0x102f7518c |
| Strings: `TEStringForm`, `TEStringFromUnicode`, `TE_GetUnicodeStr` | 0x102f7b2a4, 0x102f7b8d4, 0x102f7c514 |
| Page storage manager ("psmgr"): `TE_psmgr_getnext` (row scan), `TE_psmgr_area_free` (delete), `TE_PSmgrGetSpaceAtEnd` (insert), `TE_PSmgrPageInit`, `TE_PSmgrInitHeaderPage` | 0x102f67818, 0x102f673c4, 0x102f696ac, 0x102f68d3c, 0x102f68b84 |
| Disk page manager: `TE_DPgm_InitialSetup`, `TE_DPgm_WritePage`, `TE_DPgm_WriteHeader`, `TE_DPgm_Close` | 0x102f588b8, 0x102f5847c, 0x102f58570, 0x102f56568 |
| Validators: `TE_Dpgm_CheckFirstHeader`, `TE_Dpgm_CheckPageHeader`, `TE_Dpgm_CheckPsmgrHeader`, `TE_Pgm_CheckFlatRow` | 0x102f587e0, 0x102f5916c, 0x102f592e8, 0x102f59574 |
| Transaction id: `TE_XidAdd` / `TE_XidIncr` (64-bit, two u32 words) | 0x102f8a320, 0x102f8a38c |
| Track insert: `db::dsqlAddTrack(RbDBIndex, RowDataTrack*)` → `DevAddContent_` (47 parameters, one per column except ID, TAG and RESERVED10) | 0x1019a78d0, 0x10124c338 |
| master.db query that fills `RowDataTrack` (same column order as DJDBCONTENT) | string @0x104f9db00 |
| Column updates: `DEV_MODULE6_DEVMODIFY*` forms (record layout gives the RESERVEDn meanings) | 0x1006b…–0x1006cc… |

Tooling note: the REA/Ghidra MCP provider timed out on every request during this work. Most of the analysis used `nm`, `objdump` and a small instruction emulator (kept in the session scratchpad, not in the repo). The emulator replays the `TETableCreate` call sites and decodes the chained-fixup pointers to the column-name and column-type string arrays. The late checks used the headless decompiler (`re/ghidra/decompile.sh`): reuse-list flag bits, header-page free list, dirty-bit clearing, and category enable/disable.

## File header (page 0)
| Off | Size | Field | Value in fixture | Status |
|---|---|---|---|---|
| 0x00 | 4 | reserved | 0 | verified (fixture) |
| 0x04 | 4 | page size | 4096 | verified; `CheckFirstHeader` compares it to the page manager's page size |
| 0x08 | 4 | table count | 20 / 9 | verified |
| 0x0c | 4 | next unused page: first page number never handed out | 0x13d / 0x15 | verified: above every chained page and every empty candidate; `CheckPageHeader` requires `page.index < it` and `page.next < it` (or `next == 0x03ffffff`) |
| 0x10 | 4 | page-manager state | 5 | verified (binary): `TE_DPgm_Close` writes 5 (6 if a close flag is set); `TE_DPgm_WritePage` writes 7 into the in-memory header before the first page write of a session. `InitialSetup` accepts 0, 1, 5 |
| 0x14 | 4 | transaction id, low word ("sequence") | 0x4f8f / 0x23c | verified: ≥ every page's id; `TE_DPgm_Close` increments it with `TE_XidIncr` |
| 0x18 | 4 | transaction id, high word | 0 | verified (binary): carry of `TE_XidAdd`; `CheckPageHeader` compares it to page 0x14 |
| 0x1c | 16×n | table directory | | see below |
| … | | zero | | verified (fixture) |

The directory can hold `(page_size − 0x1c) / 16` entries (`InitialSetup`, stored at `pgm+0x34`).

### Table directory entry (16 bytes)
| Off | Field | Status |
|---|---|---|
| 0 | table type (0..n−1, in directory order) | verified |
| 4 | empty candidate: a reserved page the table will take as its next data page | verified: never in a chain; all zero when inside the file; beyond EOF for 1 export table (0x13c) and 3 ext tables (0x12..0x14); every table's last page links to it |
| 8 | first page = the table's header page (flags 0x64) | verified |
| 12 | last page of the chain | verified: readers stop here, not at a sentinel |

Every in-file page is either in exactly one chain or is an all-zero empty candidate (verified for both files: 19 + 6 candidate pages). The file ends after the highest chained page; candidates past that are not written. In the initial layout table *t* got header page 2t+1 and candidate 2t+2 (verified pattern in both files; pages added later are allocated from `next_unused_page` and interleave across tables).

## Pages
Every page except page 0 has a 40-byte header. Bytes 0x00..0x17 are the disk page manager's; 0x18..0x27 are the storage manager's ("psmgr" header, `psh` below = page+0x18).

| Off | Size | Field | Status |
|---|---|---|---|
| 0x00 | 4 | reserved, 0 | verified (fixture) |
| 0x04 | 4 | own page number | verified (every chained page) |
| 0x08 | 4 | table type | verified (every chained page = directory type); `CheckPageHeader` requires `≤ table count` |
| 0x0c | 4 | next page in the chain; on the last page the table's empty candidate | verified |
| 0x10 | 4 | transaction id (low) of the last write of this page | verified ≤ header id; `CheckPageHeader`: if `header.hi == 0 || header.hi == page.hi` then `header.lo ≥ page.lo` |
| 0x14 | 4 | transaction id (high) | verified 0; `CheckPageHeader`: `header.hi ≥ page.hi` |
| 0x18 | 4 | bits 0..12 slot count; bits 13..25 live-row count; bits 26..31 flags | verified (binary: `CheckPsmgrHeader`, `psmgr_getnext`, `area_free`, `GetSpaceAtEnd`; fixture: live count = number of present bits on all 278 data pages) |
| 0x1c | 2 | free bytes | verified: `free + used + footer = page_size − 0x28` on every data page |
| 0x1e | 2 | used bytes (heap allocated to rows, from 0x28 up, deleted rows included) | verified |
| 0x20 | 2 (14 bits) | number of dirty slots (data page) or dirty reuse-list entries (header page); 0x1fff = none | verified (fixture, every page; binary `TE_PSmgrAddDirtyArea`, `area_free`) |
| 0x22 | 2 (14 bits) | lowest dirty slot (data page) or list entry (header page); 0x1fff = none | verified (fixture, every page) |
| 0x24 | 2 | data page: 0. Header page: capacity of the reuse list (1004 = (4096−40)/4 − 10) | verified |
| 0x26 | 2 | data page: 0. Header page: slots of the reuse list in use (high-water mark) | verified |

The flags byte (offset 0x1b) is `kind << 5 | reuse << 4 | 0x04`. `TE_PSmgrPageInit` always sets bit 2 (0x04) and writes the kind into bits 5..7: 1 = data, 2 = unused by rekordbox, 3 = table header page. Bit 4 (0x10) is set when the page joins the table's reuse list.
- **0x24**: data page, no reuse.
- **0x34**: data page in the reuse list; verified, it has at least one deleted slot.
- **0x64**: table header page.

`CheckPsmgrHeader` limits (verified, binary):
- **Slot and live counts:** at most 510 for 4 KiB pages (1022 for 8 KiB, 2046 for 16 KiB, 2047 up to 64 KiB).
- **u16 at 0x20 and 0x22:** each below 0x2000.
- **Kind:** at most 3.

Fields that older descriptions call "num_rows_small/num_rows_large" are this packed u32 and the first-dirty-slot hint. The live-row count is the 13-bit field, not a byte. See [Verification](#verification) for the miscounts the old reading causes.

### Data pages (kind 1)
- The row heap runs from 0x28 upwards. Each row is allocated `align4(size)` bytes at offset `used` (`TE_PSmgrGetSpaceAtEnd`), so every row offset is a multiple of 4 (verified).
- The slot footer grows down from the end of the page in **row groups** of 16 slots, 0x24 bytes each. Group *g* (0-based) ends at `page_size − 0x24·g`. Slot *k* of a group stores its u16 heap offset at `end − 6 − 2k`, so the footer is only as long as the slots in use: `2·slots + 4·groups` bytes. The two u16 masks sit at the very end:

  | At (relative to group end) | Field |
  |---|---|
  | −2 | dirty mask (bit k = slot 16g+k changed in the last write transaction) |
  | −4 | present mask (bit k = slot 16g+k is live) |
  | −6 − 2k | heap offset of slot 16g+k |

- Read slots in order 0..slot_count−1 and keep those whose present bit is set (`TE_psmgr_getnext` @0x102f67818, verified).
- **Insert** (`GetSpaceAtEnd`): `slot = slot_count++`, `live++`, set present and dirty bits, write the offset, `used += align4(size)`, `free -= align4(size) + (slot % 16 == 0 ? 6 : 2)` (`TE_PSmgrCalcAddedOffset` @0x102f6965c).
- **Delete** (`TE_psmgr_area_free`): `live--`, clear the present bit. If the slot was inserted in the same transaction, clear its dirty bit and decrement 0x20; otherwise set its dirty bit. The row bytes and the slot stay. On the first deletion the page joins the header page's reuse list and gets flag 0x10. A page whose live count drops to 0 (outside the reuse list) goes to a free list. Not seen in the fixture: emptied pages there are still chained, e.g. 0x92 in history entries.
- Dirty bits are cleared lazily the next time a transaction touches the page (`TE_PSmgrClearDirtyBits` @0x102f694d4, verified). It zeroes the dirty masks group by group from the first dirty slot and sets 0x20 and 0x22 back to 0x1fff. It also clears flag 0x10 when the page has no deleted slots left (slot count = live count). In a file the dirty bits only mark slots changed in the page's last write. Readers ignore them.
- `TE_PSmgrAddDirtyArea` @0x102f69d28 (verified) keeps the counters current: 0x20 goes 0x1fff → 1 → +1, and 0x22 = the minimum dirty slot.
- Rows can later be rewritten in place with fewer bytes, so the gaps between consecutive row offsets may exceed the row's encoded size. In the fixture this happens on 142 of 146 track pages and one playlist page. Readers must take each slot's offset as authoritative and never derive row starts from sizes.

### Table header pages (kind 3, flags 0x64)
The first page of every chain. Written by `TE_PSmgrInitHeaderPage` @0x102f68b84.

| Off | Field | Status |
|---|---|---|
| 0x18 | slot/live counts 0, flags 0x64 | verified |
| 0x1c / 0x1e | free = 0, used = 0 | verified |
| 0x20 | number of list entries with the dirty bit (bit 0) set; 0x1fff = none | verified (binary `TE_PSmgrAddToReuseCache` @0x102f6a50c; fixture: 0x1fff everywhere except `DJDBHOTCUEBANKLIST`, which has 1) |
| 0x22 | index of the first dirty list entry; 0x1fff = none | verified (same; `DJDBHOTCUEBANKLIST`: 0) |
| 0x24 | list capacity (1004) | verified |
| 0x26 | list slots in use (high-water mark, may include holes) | verified |
| 0x28 | own page number | verified |
| 0x2c | first data page of the table, 0x03ffffff if none | verified |
| 0x30 | head of the free-page list (pages of kind 2), 0x03ffffff = none | verified (binary `TE_PSmgrAddToFreeList` @0x102f6a9b8); 0x03ffffff in every fixture table |
| 0x34 | tail of the free-page list (set together with 0x30 when the first free-list page is created); 0 until then | verified (binary); 0 in the fixture |
| 0x38 | u16 number of valid list entries | verified |
| 0x3a | u16 index of the first hole in the used part of the list, 0x1fff = none | verified (tracks: 96; playlist entries: 1) |
| 0x3c | 1004 × u32 list entries: `page << 3 \| bits`, empty = 0x1ffffff8 | verified: the set of listed pages equals the set of pages with flag 0x10 for every table of both files |
| 0xfec | 20 zero bytes | verified |

- The list is the **reuse cache**: pages with space from deleted rows.
- The low 3 bits of an entry are transaction bookkeeping (verified, binary):
  - bit 0 = dirty;
  - bit 1 = added in the current transaction;
  - bit 2 = removal pending.

  `TE_PSmgrAddToReuseCache` sets bits 0 and 1. `TE_PSmgrFlipDirtyFreePageBits` @0x102f687c8 clears them on the next transaction that touches the page and resets 0x20/0x22 to 0x1fff (it uses the same scheme for free-list pages). In the fixture the only non-zero value is 3, on the `DJDBHOTCUEBANKLIST` entry added in the last transaction.
- The header page's `next_page` is the first data page, or the empty candidate for an empty table (verified).
- No other index structure is stored. The runtime creates its MPHash/MPAVL indexes (`IDXCONTENT`, `IDXCONTENTTITLE`, … in `DEV_MODULE1_table_inits`) in memory; they never reach the file (verified: no page in either file is unaccounted for).

### Deleted rows
- A deleted row only has its present bit cleared. Its bytes, slot and offset stay, and the page keeps flag 0x10 and its reuse-list entry.
- In header rows, bit 4 of the row header is a separate storage-layer "deleted" flag, checked by `TE_Pgm_CheckFlatRow`. It is 0 on every fixture row, deleted ones included.
- The fixture has 455 deleted track slots: 1159 slots, 704 live.

## Rows
### Physical layout (verified, binary + fixture)
`TE_ComputeRealPositions` places the columns of the `TETableCreate` list:
1. Group the columns by size class in this order: 8-byte, 4-byte (`INT32`/`BIT32`), 2-byte (`INT16`/`BIT16`), 1-byte (`INT8`/`BIT8`), then variable (`NVARCHAR`/`VARCHAR`).
2. **Inside a class the declaration order is reversed**: the last declared column of the class comes first.
3. Fixed columns are packed back to back with no padding. They start at offset 4 in header rows and at 0 otherwise.
4. The first variable column (in physical order) starts at the next 4-aligned offset and is stored **inline**.

`TE_DetermineFlatRowType` decides the row format:
- **No-header rows:** tables with at most one variable column. They hold the fixed columns and one trailing inline string. Every table except tracks, artists, albums, property and the ext tables with ≥2 strings.
- **Header rows:** tables with two or more variable columns. A 4-byte row header comes first. Right after the inline string comes an array of `ncols − nfixed` offsets, one per remaining variable column, in physical order. The array is aligned to its entry width. Each entry is the column's string offset from the row start (`TE_GetAttrFR` @0x102f74ed0).

This one rule reproduces the "subtype/index_shift/unknown" fields of older descriptions:
- The track "u16 = 3" before the 21 offsets is RESERVED10: an inline empty string (byte 0x03) plus one alignment byte.
- The "u8 3" in artist and album rows is the inline empty SEARCHSTR.
- The track "bitmask" u32 at 0x04 is CONTENTLINK.

### Row header (header rows only, verified)
`TE_RowFormSmgr` @0x102f7572c and `TE_SmgrRowSetOffset` @0x102f76cd0.

| Bits | Field |
|---|---|
| 0..1 | 0 (`CheckFlatRow` rejects anything else) |
| 2..3 | offset-array entry width: 0 = u8, 1 = u16, 2 = u32 |
| 4 | storage "deleted" flag (always 0 in rekordbox files) |
| 5..20 | table number in its DeviceSQL database: type + 1 in export.pdb (verified on 4 tables), type + 49 in exportExt.pdb (verified on types 3 and 7). `CheckFlatRow` requires `≤` the DB's table count |
| 21..31 | slot index of the row in its page (verified on every header row); ≤ 510 for 4 KiB pages |

Examples of the low u16 (the old "subtype"):
- artists/albums with u8 offsets: 0x0060 / 0x0080;
- with u16 offsets ("far"): 0x0064 / 0x0084;
- tracks: 0x0024;
- property: 0x0280.

The high u16 (the old "index_shift") is `slot << 5`.

### Offset width and allocation size (verified, binary; allocation verified on all contiguous pages)
`TE_RowFormSmgr` computes, per row:
```
size = fixed_end                      // offset of the variable area for header rows
for each variable column in physical order (inline one first):
    size = align4(size) + encoded_len(string)
nvar = number of offset-array entries
width = 1 if size + align4(nvar)   + 4 < 0x100
        2 if size + align4(2·nvar) + 4 < 0x10000
        4 otherwise
size += align4(width · nvar)
```
`size` is the allocation request; the page then reserves `align4(size)`. The real encoding is packed tighter:
- the inline string sits at the 4-aligned variable start;
- the offset array follows it, aligned to `width`;
- the other strings follow in physical order, back to back;
- **long-form strings (0x40/0x90) are moved to a 4-aligned offset** (verified on all 719 long strings);
- short strings get no alignment.

`fixed_end` here is the 4-aligned start of the variable area (equivalent for every rekordbox table). The fixture matches this allocation on every page where rows were not rewritten in place.

## Strings (verified, binary + fixture)
| First byte | Form | Layout |
|---|---|---|
| odd | short ASCII | byte = `(total_len << 1) \| 1`, `total_len` counts the header byte; `total_len − 1` ASCII bytes follow. Empty string = `0x03` |
| 0x40 | long ASCII | u32 header `(total_len << 8) \| 0x40`, `total_len` includes the 4 header bytes; 1 byte per char |
| 0x90 | UTF-16LE | u32 header `(total_len << 8) \| 0x90`; 2 bytes per char |

Header bits of the long form (`TE_GetUnicodeStr`):
- bits 6..7: bytes per character;
- bits 4..5: encoding (1 = UTF-16);
- bits 8..31: total length.

No terminator in any form.

**Encoder choice** (`TEStringFromUnicode` → `TEStringForm`):
- all UTF-16 code units ≤ 0x7f → ASCII: short when ≤ 126 characters, else 0x40;
- otherwise 0x90.

The fixture's longest short string is 125 characters; its 0x40 strings are 131–143 bytes.

**NULL strings** become the empty short string `0x03`. `DevAddContent_` passes NULL for FILENAMES, SEARCHSTR, TAG and RESERVED1 (and nothing for RESERVED10); all are `03` in the fixture.

**ISRC special case** (`db::dsqlAddTrack` @0x1019a7a14):
- When the ISRC (`RowDataTrack+0x460`) is exactly 12 UTF-8 bytes, rekordbox builds a 16-byte zeroed buffer `{0x03, isrc[0..12]}` and passes it as if it were a UTF-16 string.
- The first "code unit" 0x5103 is above 0x7f, so the runtime stores it as **0x90** with the 14 raw bytes `03 'Q' 'Z' … '4' 00` (total length 0x12).
- Decode: kind 0x90 and payload[0] == 0x03 → ASCII up to the first NUL.
- Other ISRC lengths become the empty string.
- Fixture: 671 rows have the ISRC form, 33 are `03`.

**Menu-item names** are UTF-16 wrapped in U+FFFA … U+FFFB (`\u{fffa}GENRE\u{fffb}`).

## Tables
Column names and types come from the `TETableCreate` calls. The table type is the creation order among the persistent tables: storage name `EXTERNAL` for export.pdb, `EXTERNALEX` for exportExt.pdb. `DJDBTRACKCUE` (storage `DEFAULT`) is created after DJDBPROPERTY but is not persisted, so export.pdb has 20 tables, not 21.

Offsets below are physical (row-relative). "→" marks string columns reached through the offset array, listed in array order.

### export.pdb
| Type | Table | Rows (fixture) | Layout |
|---|---|---|---|
| 0 | `DJDBCONTENT` tracks | 704 | header row, u16 offsets in every fixture row; see below |
| 1 | `DJDBGENRE` | 14 | 0x00 ID u32, 0x04 NAME |
| 2 | `DJDBARTIST` | 572 | 0x00 header, 0x04 ID u32, 0x08 SEARCHSTR (inline), → NAME |
| 3 | `DJDBALBUM` | 605 | 0x00 header, 0x04 IMAGEID u32, 0x08 ALBUMARTISTID u32, 0x0c ID u32, 0x10 COMPILATION u8, 0x14 SEARCHSTR (inline), → NAME |
| 4 | `DJDBLABEL` | 209 | 0x00 ID u32, 0x04 NAME |
| 5 | `DJDBKEY` | 58 | 0x00 ID u32, 0x04 SEQ u8, 0x08 SCALENAME |
| 6 | `DJDBCOLOR` | 8 | 0x00 COLORCODE u32, 0x04 SORTKEY u8, 0x05 ID u8, 0x08 COMMNT (colour name) |
| 7 | `DJDBPLAYLIST` playlist tree | 30 | 0x00 PARENTID, 0x04 IMAGEID, 0x08 SEQ, 0x0c ID (u32 each), 0x10 ATTRIBUTE u8, 0x14 NAME |
| 8 | `DJDBSONGPLAYLIST` playlist entries | 1355 | 0x00 TRACKNO, 0x04 CONTENTID, 0x08 PLAYLISTID (u32 each) |
| 9 | `DJDBHOTCUEBANKLIST` | 0 | as type 7 |
| 10 | `DJDBSONGHOTCUEBANKLIST` | 0 | 0x00 CUEID, 0x04 TRACKNO, 0x08 CONTENTID, 0x0c HOTCUEBANKLISTID (u32) |
| 11 | `DJDBHISTORY` history playlists | 21 | 0x00 ID u32, 0x04 NAME |
| 12 | `DJDBSONGHISTORY` history entries | 160 | 0x00 CONTENTID u32, 0x04 HISTORYID u32, 0x08 SEQ u16, 0x0a ONAIR u8 |
| 13 | `DJDBIMAGE` artwork | 702 | 0x00 ID u32, 0x04 PATH |
| 14 | `DJDBMIXERPARAM` | 0 | 0x00 CONTENTID u32, 0x04 LOW, 0x06 MID, 0x08 HIGH, 0x0a GAIN (u16) |
| 15 | `DJDBHOTCUEBANKPOINT` | 0 | 0x00 OUTMPEGABS, 0x04 OUTMPEGFRAME, 0x08 OUTFRAME, 0x0c OUTMSEC, 0x10 INMPEGABS, 0x14 INMPEGFRAME, 0x18 INFRAME, 0x1c INMSEC, 0x20 CONTENTID, 0x24 ID (u32), 0x28 KIND u8 |
| 16 | `DJDBMENUITEMS` browse menu items ("columns") | 27 | 0x00 ID u16, 0x02 CLASS u8, 0x04 NAME (UTF-16 with FFFA/FFFB) |
| 17 | `DJDBCATEGORY` | 22 | 0x00 MENUITEMID u16, 0x02 ID u16, 0x04 INFOORDER u8, 0x05 DISABLE u8, 0x06 SEQ u8 |
| 18 | `DJDBSORT` | 17 | 0x00 MENUITEMID u16, 0x02 ID u16, 0x04 DISABLE u8, 0x05 SEQ u8 |
| 19 | `DJDBPROPERTY` | 1 | 0x00 header, 0x04 SONGNUMBER u32, 0x08 PROTECTED u8, 0x09 COLORID u8, 0x0c DATECREATED (inline), → DBVERSION, → DEVNAME |

Field meanings and status:
- **Genre, label, history, artwork:** ID + name/path. Verified.
- **Artist:** used for artist, remixer, original artist and composer (track ID columns point into it). Verified by name resolution. Album rows' ALBUMARTISTID is 0 in the fixture.
- **Key:**
  - SCALENAME is master.db `djmdKey.ScaleName` as stored: mixed styles in the fixture (`Gmin`, `Am`, `9A`, `Unknown`).
  - SEQ is a single byte. It equals ID for IDs ≤ 54; the higher IDs reuse 55..59.
- **Colour:** IDs 1..8 Pink, Red, Orange, Yellow, Green, Aqua, Blue, Purple; SORTKEY = ID. COLORCODE is 0 in the fixture (meaning open).
- **Playlist tree:**
  - SEQ is the position among siblings.
  - PARENTID 0 = top level.
  - ATTRIBUTE: 0 = playlist, 1 = folder (hypothesis; the fixture has no folders). It comes from `rekordboxDBController::insertPlaylist(…, char attribute, …)` @0x1014233fc.
  - IMAGEID is 0 in the fixture.
  - IDs 16385+ (0x4001..) are the "TAG LIST nnn" lists. They are ordinary rows here.
- **Playlist entries:** TRACKNO is the 1-based position. Verified: contiguous 1..n in all 30 playlists, every CONTENTID exists, and no row is orphaned.
- **History entries:** SEQ is the 1-based play order (verified contiguous). ONAIR is 0 in the fixture.
- **Menu items:** CLASS 0x80..0xaa (meaning per value open). Full list in the fixture, ID → name:
  - 1 GENRE, 2 ARTIST, 3 ALBUM, 4 TRACK, 5 BPM, 6 RATING, 7 YEAR, 8 REMIXER, 9 LABEL;
  - 10 ORIGINAL ARTIST, 11 KEY, 12 CUE, 13 COLOR, 14 TIME, 15 BITRATE, 16 FILE NAME, 17 PLAYLIST, 18 HOT CUE BANK;
  - 19 HISTORY, 20 SEARCH, 21 COMMENTS, 22 DATE ADDED, 23 DJ PLAY COUNT, 24 FOLDER, 25 DEFAULT, 26 ALPHABET, 27 MATCHING.
- **Category:** the top-level browse menu.
  - SEQ = display position of enabled entries, 0 for disabled ones. Verified (binary): `DEVDISABLECATEGORY` @0x1006bffa0 sets SEQ = 0 and decrements the SEQ of the entries after it; `DEVENABLECATEGORY` @0x1006c0a70 sets SEQ = max + 1. The fixture has 1..10.
  - DISABLE: both forms copy a caller-supplied byte into it, so it is a value, not a boolean (verified, binary). In the fixture it is 1 on hidden entries and 0 on shown ones, except DATE ADDED (5, shown, SEQ 6) and MATCHING (3, hidden). Per-value meaning is open.
  - INFOORDER = position in the track-info sub-columns (TRACK 1, ARTIST 2, ALBUM 3, TIME 4, BPM 5, BITRATE 6), 99 = none (hypothesis). Written by `DevInitCategoryTable*`, `DevSetSubColumn`.
- **Sort:** DISABLE 0 = offered, 1 = not; SEQ = order. Same enable/disable forms as categories. One row has DISABLE 2, SEQ 1 (meaning open).
- **Property:**
  - DEVNAME "REKORDBOX", DBVERSION "1000", DATECREATED "2020-10-28" (creation date of the stick library).
  - SONGNUMBER = track count (704, verified).
  - COLORID 8 (meaning open, probably the device colour), PROTECTED 0.

#### Tracks (`DJDBCONTENT`, type 0)
| Off | Column | Meaning | Status |
|---|---|---|---|
| 0x00 | row header | 0x24 \| slot<<21 (u16 offsets) | verified |
| 0x04 | CONTENTLINK | `djmdContent.ContentLink`; 788224 (0x000c0700) on every fixture row | source verified (binary), meaning open |
| 0x08 | SAMPLERATE | Hz | verified (44100) |
| 0x0c | COMPOSERID | artist ID | verified (binary), 0 in fixture |
| 0x10 | FILESIZE | bytes | verified (plausible sizes; FLAC 34 MB) |
| 0x14 | MASTERCONTENTID | `djmdContent.MasterSongID` of the source library | verified (binary: getter `RowDataTrack::getMasterContentID` = +0x40c) |
| 0x18 | MASTERDBID | `djmdContent.MasterDBID` (2328226057 on all rows) | verified (binary) |
| 0x1c | IMAGEID | artwork ID (type 13), 0 = none | verified (resolves to `/PIONEER/Artwork/...`) |
| 0x20 | KEYID | key ID | verified (resolves) |
| 0x24 | ORGARTISTID | original artist ID | verified (binary) |
| 0x28 | LABELID | label ID | verified (resolves) |
| 0x2c | REMIXERID | artist ID | verified (binary) |
| 0x30 | BITRATE | kbit/s (1411 for 16-bit AIFF, 2822 for 32-bit, 0 for the FLAC) | verified |
| 0x34 | TRACKNO | track number | verified |
| 0x38 | BPM | tempo × 100 | verified |
| 0x3c | GENREID | genre ID | verified (resolves) |
| 0x40 | ALBUMID | album ID | verified (resolves) |
| 0x44 | ARTISTID | artist ID | verified (resolves, spot-checked) |
| 0x48 | ID | track ID (1..704 in the fixture) | verified |
| 0x4c | DISCNO | disc number | verified (binary) |
| 0x4e | DJPLAYCOUNT | play count | verified (binary) |
| 0x50 | RELEASEYEAR | year | verified (2025 for a 2025 release) |
| 0x52 | BITDEPTH | bits | verified (16/24/32) |
| 0x54 | LENGTH | seconds | verified |
| 0x56 | ANALYSED | `djmdContent.Analysed & 0x3f` (41 on every row) | source verified (binary); bit meanings open (see masterdb.md) |
| 0x57 | DISABLEQUANTIZE | rekordbox passes 0 | verified (binary) |
| 0x58 | COLORID | colour 1..8, 0 = none | verified (binary) |
| 0x59 | RATING | 0..5 | verified (binary) |
| 0x5a | FILETYPE | `djmdContent.FileType` (5 FLAC, 12 AIFF in fixture) | verified |
| 0x5c | RESERVED10 (inline) | unused, `03` | verified |
| 0x5e | 21 × u16 offsets | in the order below | verified |

String columns in offset-array order, with their master.db source (the `RowDataTrack` fill query and the `DevAddContent_` argument order):

| # | Column | Source / meaning | Fixture |
|---|---|---|---|
| 0 | RESERVED9 | `ISRC` (special 0x90 form) | 671 set |
| 1 | RESERVED8 | `Lyricist` | "" |
| 2 | RESERVED7 | `TrackInfoUpdated` (counter as text) | "1".."n" or "" |
| 3 | RESERVED6 | `AnalysisUpdated` (counter as text) | "1"/"2"/"3" |
| 4 | RESERVED5 | `CueUpdated` (counter as text) | "1", "19" or "" |
| 5 | RESERVED4 | `DeliveryComment` | "" |
| 6 | RESERVED3 | `DeliveryControl` ("ON" or NULL) | 84 × "ON" |
| 7 | RESERVED2 | `HotCueAutoLoad` ("ON" or NULL) | all "ON" |
| 8 | RESERVED1 | `ModifiedByRBM`; NULL on export | "" |
| 9 | TAG | `Tag`; not passed (NULL) | "" |
| 10 | DATECREATED | `DateCreated` (YYYY-MM-DD) | differs from STOCKDATE on 579 rows |
| 11 | RELEASEDATE | `ReleaseDate` (YYYY-MM-DD or YYYY) | |
| 12 | SUBTITLE | `Subtitle` (mix name) | "" |
| 13 | SEARCHSTR | NULL | "" |
| 14 | ANALYSISDATAPATH | device path of ANLZ0000.DAT | `/PIONEER/USBANLZ/P00D/00001CE1/ANLZ0000.DAT` |
| 15 | STOCKDATE | `StockDate` = date added | |
| 16 | COMMNT | comment | |
| 17 | TITLE | title | |
| 18 | FILENAMES | NULL | "" |
| 19 | FILENAMEL | file name | |
| 20 | FOLDERPATH | full device path of the audio file | `/Contents/…` |

How RESERVED1..9 were tied to their sources:
- **`DevAddContent_` arguments:** they are, in order, ModifiedByRBM (NULL), HotCueAutoLoad, DeliveryControl, DeliveryComment, CueUpdated, AnalysisUpdated, TrackInfoUpdated, Lyricist and ISRC.
- **DevModify record offsets:** the `DEVMODIFY*` forms write their value into a record with 8 bytes per pointer/INT32 column. TITLE lands at +0x28 and COMMNT at +0x70, which calibrates it. MODIFIEDBYRBM … LYRICIST then land on the slots of RESERVED1 … RESERVED8.
- **Fixture:** the values agree.

DATECREATED vs STOCKDATE is verified from the binary (the fill query lists `StockDate` at the STOCKDATE position and `DateCreated` at DATECREATED). Older descriptions call string 10 "date added" and string 15 "analyze date"; by the binary, string 15 (STOCKDATE) is the date added.

### exportExt.pdb
| Type | Table | Rows | Layout |
|---|---|---|---|
| 0 | `DJDBEXRECOMMENDLIKE` | 0 | no header: 0x00 DATACREATEDL, 0x04 DATACREATEDH, 0x08 CONTENTID2, 0x0c CONTENTID1 (u32), 0x10 RESERVED2 u16, 0x12 RESERVED1 u8, 0x13 LIKERATE u8, 0x14 RESERVED3 |
| 1 | `DJDBEXHISTORY` | 0 | header: 0x04 RESERVED3, 0x08 PARENTID, 0x0c SEQ, 0x10 ID (u32), 0x14 RESERVED2 u16, 0x16 RESERVED1 u8, 0x17 ATTRIBUTE u8, 0x18 RESERVED4 (inline), → DATECREATED (VARCHAR), → NAME |
| 2 | `DJDBEXSONGHISTORY` | 0 | no header: 0x00 TRACKNO, 0x04 CONTENTID, 0x08 HISTORYID (u32), 0x0c RESERVED1 |
| 3 | `DJDBEXMYTAG` My Tag tree | 28 | header: 0x04 RESERVED4, 0x08 RESERVED3, 0x0c PARENTID, 0x10 SEQ, 0x14 ID (u32), 0x18 RESERVED2 u16, 0x1a RESERVED1 u8, 0x1b ATTRIBUTE u8, 0x1c RESERVED6 (inline), → NAME, → RESERVED5 |
| 4 | `DJDBEXSONGMYTAG` tag membership | 0 | no header: 0x00 RESERVED1, 0x04 CONTENTID, 0x08 MYTAGID (u32), 0x0c RESERVED2 |
| 5 | `DJDBEXCUEOPTION` | 0 | header: 0x04 RESERVED8, 0x08 RESERVED7, 0x0c RESERVED6, 0x10 RESERVED5, 0x14 CUEMICRO, 0x18 BEATSIZE, 0x1c CUEID (u32), 0x20 RESERVED2 u16, 0x22 RESERVED1 u8, 0x23 COLOR u8, 0x24 OUTSEEKINFO (inline VARCHAR), → INSEEKINFO, → COMMNT |
| 6 | `DJDBEXCONTENTOPTION` | 0 | header: 0x04 RESERVED5..0x14 RESERVED1, 0x18 CONTENTID (u32), 0x1c RESERVEDSTRING5 (inline), → RESERVEDSTRING4..1 |
| 7 | `DJDBEXDBPROPERTYOPTION` | 1 | header: 0x04 RESERVED5..0x14 RESERVED1, 0x18 MYTAGMASTERDBID (u32), 0x1c RESERVEDSTRING5 (inline), → RESERVEDSTRING4..1, → DBVERSION |
| 8 | `DJDBEXGENERALOPTION` | 0 | header: 0x04 RESERVED4..0x10 RESERVED1, 0x14 TABLEID, 0x18 TABLEKIND (u32), 0x1c RESERVEDSTRING3 (inline), → RESERVEDSTRING2, → RESERVEDSTRING1 |

- **My Tag** (verified):
  - ATTRIBUTE 1 marks the four columns (IDs 1..4: "Genre", "Components", "Situation", "Untitled Column"; their SEQ 0..3 is the column order).
  - ATTRIBUTE 0 marks tags. Their PARENTID is the column ID, their SEQ the 0-based order inside the column, and their ID a random master.db u32.
- **DBPROPERTYOPTION:** MYTAGMASTERDBID 2328226058, the tracks' MASTERDBID + 1 (meaning **hypothesis**). DBVERSION and the reserved columns are empty.
- Tables 0, 1, 2, 4, 5, 6 and 8 are empty in the fixture, so their layouts are **verified (binary)** only.

## Verification
`cargo test -p mb-pdb` (fixtures present). Live row counts:

| export.pdb | rows | export.pdb | rows |
|---|---|---|---|
| 0 tracks | 704 | 10 song hot cue bank list | 0 |
| 1 genres | 14 | 11 history playlists | 21 |
| 2 artists | 572 | 12 history entries | **160** |
| 3 albums | 605 | 13 artwork | 702 |
| 4 labels | 209 | 14 mixer params | 0 |
| 5 keys | 58 | 15 hot cue bank points | 0 |
| 6 colours | 8 | 16 menu items | 27 |
| 7 playlist tree | 30 | 17 categories | 22 |
| 8 playlist entries | **1355** | 18 sorts | 17 |
| 9 hot cue bank lists | 0 | 19 property | 1 |

exportExt.pdb: type 3 My Tag 28, type 7 property option 1, all others 0.

**The expected 848 playlist entries and 159 history entries are an artefact of the old reading.** That reading takes the low byte of 0x18 as the row count and prefers the u16 at 0x22 when it is larger. On pages with 284 slots it then sees 28 or the first dirty slot (11, 191, 283, 143, 202, …) and scans only that many slots.

The test `legacy_counts_explain_848_and_159` reproduces exactly 848 and 159 with the old reading. Two facts confirm 1355 and 160:
- the 13-bit live counts in the page headers add up to 1355 and 160;
- the entries form contiguous positions 1..n in every playlist and history list, with no orphans.

Invariants checked on every page of both files (`headers_round_trip_and_invariants_hold`):
- **Re-encoding:** the file header and all 40-byte page headers re-encode to identical bytes.
- **Chains:** every chain is loop-free and type-consistent. The last page links to the empty candidate, and in-file candidates are zero.
- **Page counters:**
  - live count = number of present bits;
  - dirty count and first dirty slot match the dirty masks;
  - `free + used + footer = page_size − 0x28`.
- **Reuse list:** the header page's reuse list equals the set of pages flagged 0x10, and its count field matches. Header-page 0x20/0x22 match the entries' dirty bits, and the free list is empty (0x30 = 0x03ffffff, 0x34 = 0).
- **Rows:**
  - every slot (live and deleted) decodes;
  - rows end within `used`;
  - offsets are 4-aligned;
  - the header-row slot field equals the slot;
  - the table word is type+1 (export) or type+49 (ext);
  - long strings are 4-aligned.

## Writer requirements
What a writer has to produce for players and rekordbox to accept the file. Each rule is marked **[F]** (verified in the fixture), **[B]** (read from the binary) or **[H]** (hypothesis).

**File and pages**
1. **[F][B] Page 0.**
   - Page size 4096.
   - The table count is 20 (export) or 9 (ext), with directory entries in type order.
   - `state = 5`.
   - `xid_hi = 0`; `xid_lo` ≥ every page's id.
   - Everything after the directory is zero.
2. **[F] One chain per table.** It starts with a table header page (flags 0x64) even when the table is empty. An empty table is just the header page: `first_page = last_page = header page`, `next_page` = empty candidate, and 0x2c = 0x03ffffff.
3. **[F][B] Empty candidates.** Every table owns one distinct reserved page.
   - The last chain page's `next_page` points to it.
   - Candidates inside the file are all zero.
   - Candidates may lie past EOF. The file ends after the last written page.
   - `next_unused_page` = highest page number used anywhere (chains and candidates) + 1.
4. **[F] Page header bytes 0x00 and 0x14 are 0.** 0x04 = own page number; 0x08 = table type; 0x10 = a transaction id ≤ the header's.
5. **[B] Page numbers and links.**
   - `page < next_unused_page`.
   - `next_page < next_unused_page` or 0x03ffffff.
   - Page type ≤ table count.
6. **[F][H] Page allocation order.** rekordbox's fresh-database order is header 2t+1, candidate 2t+2, then sequential growth from `next_unused_page`. Players follow links, so any order that satisfies rules 2–5 should work **[H]**.

**Data pages**

7. **[F][B] Header fields.**
   - Flags 0x24, no deletions, so no 0x34 and an empty reuse list.
   - 0x18 packs `slot_count | live_count << 13 | 0x24 << 24`.
   - `used` = sum of `align4(alloc)`.
   - `free = 4056 − used − (2·slots + 4·groups)`.
   - 0x24 = 0x26 = 0.
8. **[F][B] Dirty state.** Either all dirty masks are 0 and 0x20 = 0x22 = 0x1fff (clean, as after `TE_PSmgrClearDirtyBits`), or mirror the insert path:
   - dirty bit set for every inserted slot;
   - 0x20 = count;
   - 0x22 = lowest slot.

   The fixture has both kinds of page. Clean is the safer choice **[H]**.
9. **[F][B] Rows.**
   - Rows are placed in slot order from heap offset 0, each at a 4-aligned offset.
   - Footer groups follow the layout above. Unused bits of the last group's masks are 0.
   - The page is full when `align4(alloc) + (slot % 16 == 0 ? 6 : 2) > free`.
   - At most 510 slots per 4 KiB page **[B]**.

**Header pages**

10. **[F][B]**
    - Flags 0x64, counters 0.
    - 0x20 = 0x22 = 0x1fff, 0x24 = 1004, 0x26 = 0.
    - 0x28 = own page, 0x2c = first data page or 0x03ffffff, 0x30 = 0x03ffffff, 0x34 = 0.
    - 0x38 = 0, 0x3a = 0x1fff.
    - 1004 entries of 0x1ffffff8, then 20 zero bytes.
    - With deletions in the written file: list each such page as `page << 3` (flag bits 0), set 0x38 to the entry count and 0x26 to the high-water mark, and give the page flag 0x34 **[F][B]**.

**Rows**

11. **[B][F] Layout.** Use the column lists in `crates/mb-pdb/src/schema.rs` (extracted from the binary) and the layout rule above. Fixed columns are packed with no padding; no-header rows end with their single string at the 4-aligned offset after the fixed part.
12. **[B][F] Header rows.**
    - Row header = `slot << 21 | table_word << 5 | width_code << 2`, with table_word = type+1 (export) or type+49 (ext).
    - Choose the width with the allocation formula. Use u16 for every track row (u8 is impossible there, because the fixed part alone is 0x5c).
    - The inline string goes at the 4-aligned variable start. The offset array follows, aligned to the width.
    - Strings follow in physical order; long strings are 4-aligned.
13. **[B] Allocation.** Allocate `align4(alloc)` with the formula in § Offset width. Slack between the encoded end and the next row is allowed; it appears in the fixture.

**Strings and values**

14. **[B][F] Strings.**
    - ASCII ≤ 126 chars → short form; longer ASCII → 0x40; anything else → 0x90 UTF-16LE.
    - NULL → `03`.
    - ISRC → the special form above.
    - Menu-item names in UTF-16 with U+FFFA/U+FFFB.
15. **[F] Fixed values rekordbox writes.**
    - **Tracks:** FILENAMES, SEARCHSTR, TAG, RESERVED1 and RESERVED10 = `03`.
    - **Tracks:** DISABLEQUANTIZE = 0; ANALYSED = `Analysed & 0x3f`.
    - **Tracks:** RESERVED2/3 = "ON" or NULL from boolean flags (`RowDataTrack+0x3ae/+0x3af`).
    - **Property row:** DEVNAME "REKORDBOX", DBVERSION "1000", SONGNUMBER = track count.
    - **Menu items:** the 27 rows listed above.
    - **Category and sort:** reproduce rekordbox's tables (`DevInitMenuItemsTable`, `DevInitCategoryTable`, `DevInitSortTable` @0x100b61140…0x100b60ed0). Copying the fixture's rows is the pragmatic choice **[H]**.
16. **[H] Player-irrelevant detail.** IDs only need to be consistent across tables. rekordbox numbers tracks, artists and the other lookups 1..n in insertion order, and the fixture's track IDs are 1..704. Keep IDs ≤ 0x7fffffff where possible; master.db random IDs above that already appear as My Tag IDs.
17. **[H] exportExt.pdb.** Must exist alongside export.pdb with all 9 tables. rekordbox writes a DBPROPERTYOPTION row even when nothing else is present.

## Open questions
- [ ] CONTENTLINK value 0x000c0700 on every track: what it encodes.
- [ ] ANALYSED bits (shared with master.db `Analysed`).
- [ ] Category DISABLE values 3 and 5; sort DISABLE 2; menu-item CLASS codes; COLORCODE; property COLORID.
- [ ] Layout of free-list pages (kind 2; entries use the same 3 flag bits as the reuse list). Not present in rekordbox files seen so far.
- [ ] Playlist ATTRIBUTE values besides 0 (folder = 1 is a hypothesis); how smart playlists are exported.
- [ ] Whether players care about dirty masks, the reuse list or `xid` (byte-exact writer output needs an oracle export of a small library).
- [ ] Tables 9, 10, 14, 15 and the empty ext tables: no fixture rows, layouts from the binary only.
- [ ] Does rekordbox ever emit u32 offset arrays or 0x40/0x90 strings in other tables? Only seen in tracks (0x40 paths, 0x90 ISRC) and menu items (0x90).
