# export.pdb / exportExt.pdb (Encirq DeviceSQL)

Status: **reader verified, writer verified**. Every page and row of both fixture files decodes, and every header field below is either checked against the fixture on every page (`crates/mb-pdb/tests/fixture.rs`) or traced to the binary. The writer (`crates/mb-pdb/src/engine.rs`, `writer.rs`) models the DeviceSQL storage layer function by function; replaying the oracle export history through it reproduces rekordbox's `export.pdb` and `exportExt.pdb` **byte for byte** (§ Writer verification). The few writer details that are still **hypothesis** are marked.

Status words used below:
- **verified (fixture)**: holds on every page/row of `fixtures/stick/PIONEER/rekordbox/export.pdb` and `exportExt.pdb`.
- **verified (binary)**: read from the rekordbox 7.2.19 arm64 code at the address given.
- **hypothesis**: fits the evidence but is not proven.
- **open**: unknown.

Implementation: `crates/mb-pdb`. Two fixtures:
- `fixtures/stick/PIONEER/rekordbox` ("the stick"): a 2026-10-11 rekordbox 7.2.19 rewrite of an older stick (see `fixtures/manifest.toml`), so it carries deleted rows and years of update history. A good reader test.
- `fixtures/oracle-a/rbtest/PIONEER/rekordbox` ("the oracle stick"): a fresh export by rekordbox 7.2.19 to an empty FAT32 stick: playlist `oracle-small` (5 tracks), then, in a later session, `oracle-all` (157 tracks). Its source library is `fixtures/oracle-a`. The byte-exact writer target.

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

Tooling note: the REA/Ghidra MCP provider timed out on every request during this work. Most of the analysis used `nm`, `objdump` and a small instruction emulator (kept in the session scratchpad, not in the repo). The emulator replays the `TETableCreate` call sites and decodes the chained-fixup pointers to the column-name and column-type string arrays. The late checks used the headless decompiler (`re/ghidra/decompile.sh`): reuse-list flag bits, header-page free list, dirty-bit clearing, and category enable/disable. The writer work (§ Storage engine) decompiled the `TE_DPgm_*`, `TE_psmgr_*`/`TE_PSmgr*`, `TE_RowFormSmgr` and update-scan functions the same way.

## File header (page 0)
| Off | Size | Field | Value in fixture | Status |
|---|---|---|---|---|
| 0x00 | 4 | reserved | 0 | verified (fixture) |
| 0x04 | 4 | page size | 4096 | verified; `CheckFirstHeader` compares it to the page manager's page size |
| 0x08 | 4 | table count | 20 / 9 | verified |
| 0x0c | 4 | next unused page: first page number never handed out | 0x13d / 0x15 | verified: above every chained page and every empty candidate; `CheckPageHeader` requires `page.index < it` and `page.next < it` (or `next == 0x03ffffff`). Advanced by `TE_DPgm_PageGetNew` (§ Storage engine) |
| 0x10 | 4 | page-manager state | 5 | verified (binary): `TE_DPgm_Close` writes 5 (6 if a close flag is set); `TE_DPgm_Open` sets 4 and `TE_DPgm_Commit` 1 in memory; `TE_DPgm_WritePage` writes 7 before the first page write of a session when `COMMITOPT=1`. `InitialSetup` accepts 0, 1, 5 |
| 0x14 | 4 | transaction id, low word ("sequence") | 0x4f8f / 0x23c | verified: ≥ every page's id. +1 at every writable open (`TE_DPgm_Open`), +1 at every commit that wrote a page (`TE_DPgm_Commit`), +1 at close (`TE_DPgm_Close`), all through `TE_XidIncr` (§ Storage engine) |
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

Every in-file page is either in exactly one chain or is an all-zero empty candidate (verified for both files: 19 + 6 candidate pages). The file ends after the highest chained page; candidates past that are not written (a candidate is only a reserved number until the table takes it). Table *t* of a new file gets header page 2t+1 and candidate 2t+2; every later page of any table is the table's current candidate, and `next_unused_page` becomes the new candidate (verified, binary: `TE_DPgm_PageGetNew` @0x102f54f88; and by the byte-exact replay).

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
| 0x20 | 2 (14 bits) | number of dirty slots (data page) or dirty reuse-list entries (header page); 0x1fff = none. It can also be 0 with no dirty bit (a slot inserted and deleted in one transaction) | verified (fixture, every page; binary `TE_PSmgrAddDirtyArea`, `area_free`) |
| 0x22 | 2 (14 bits) | lowest dirty slot (data page) or list entry (header page); 0x1fff = none. A lower bound: deleting a slot inserted in the same transaction lowers 0x20 but leaves 0x22 | verified (binary `TE_psmgr_area_free`; equality holds on every fixture page) |
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
- **Insert at the end** (`GetSpaceAtEnd`): `slot = slot_count++`, `live++`, set present and dirty bits, write the offset, `used += align4(size)`, `free -= align4(size) + (slot % 16 == 0 ? 6 : 2)` (`TE_PSmgrCalcAddedOffset` @0x102f6965c).
- **Insert into a deleted slot** (`GetSpaceMidPage`): the first slot that is neither present nor dirty and whose space (offset of the next slot, or `used` for the last one, minus its own offset) holds `align4(size)` is taken again: same slot number, same offset, present and dirty bits set, `live++`; `used` and `free` do not change. Which insert path is used is described in § Storage engine.
- **Delete** (`TE_psmgr_area_free`): `live--`, clear the present bit. If the slot was inserted in the same transaction, clear its dirty bit and decrement 0x20 (0x22 stays); otherwise set its dirty bit. The row bytes and the slot stay. On the first deletion the page joins the header page's reuse list and gets flag 0x10. A page whose live count drops to 0 outside the reuse list goes to a free list; that only happens when the reuse list is full (1004 pages), so emptied pages normally stay chained (e.g. 0x92 in the stick's history entries).
- Dirty bits are cleared lazily the next time a transaction touches the page (`TE_PSmgrClearDirtyBits` @0x102f694d4, verified). It zeroes the dirty masks group by group from the first dirty slot and sets 0x20 and 0x22 back to 0x1fff. It also clears flag 0x10 when the page has no deleted slots left (slot count = live count). In a file the dirty bits only mark slots changed in the page's last write. Readers ignore them.
- `TE_PSmgrAddDirtyArea` @0x102f69d28 (verified) keeps the counters current: 0x20 goes 0x1fff → 1 → +1, and 0x22 = the minimum dirty slot.
- A deleted slot taken again by a shorter row keeps its old allocation, and the bytes of the earlier row stay behind the new one (the row formatter writes only the new row's fields, see § Offset width). So the gap between consecutive row offsets may exceed the row's encoded size; on the stick this happens on 142 of 146 track pages and one playlist page, on the oracle stick for 39 of 875 rows. Readers must take each slot's offset as authoritative and never derive row starts from sizes.

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
- A deleted row only has its present bit cleared. Its bytes, slot and offset stay, and the page keeps flag 0x10 and its reuse-list entry. A later insert can take the slot again (§ Data pages); when no deleted slot is left the page leaves the reuse list.
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

`fixed_end` here is the 4-aligned start of the variable area (equivalent for every rekordbox table). Verified: every slot of all four fixture files (7826 rows, deleted ones included) re-encodes to the stored bytes, the allocation never exceeds the slot's space, and it equals the space for every slot that was not taken over from a deleted row (`tests/writer.rs` `encoder_reproduces_every_stored_row`).

**What the formatter writes** (verified, binary + oracle replay): `TE_RowFormSmgr` writes straight into the page area returned by the storage manager: the row header (all 32 bits; the slot is set right after by `TE_SmgrRowSetOffset`), each fixed column, the offset-array entries and the string bytes. Alignment gaps are skipped, not zeroed. On a fresh page they are zero; in a reused slot they and everything after the new row keep the old row's bytes.

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

## Storage engine (how rekordbox writes the files)
Status: **verified** (binary, function by function; replaying the oracle export through the model reproduces both oracle files byte for byte). Model: `crates/mb-pdb/src/engine.rs`. Every rule of § Writer follows from this section.

### Sessions and transaction ids
- **Open** (`TE_DPgm_Open` @0x102f55c40): a new file starts as an all-zero header with `next_unused_page` = 1 and id 0. Every writable open adds 1 to the id; the running transaction uses that value ("current id").
- **Table sessions** (`TE_psmgr_open` @0x102f66b30): each table starts every session with no insert page (psmgr+0x20 = 0x03ffffff) and its reuse cursor at 0. So the first insert of a session never continues the page the previous session filled: it uses the reuse list or a new page.
- **Page id** (`TE_DPgm_PageMarkDirty` @0x102f552e8): the first change of a page in a transaction stores the current id at page 0x10.
- **Commit** (`TE_DPgm_Commit` @0x102f554d4): if the transaction changed anything, the dirty pages are written and the id advances by 1. A transaction that changed nothing (a lookup that found its row) does not advance it.
- **Close** (`TE_DPgm_Close` @0x102f56568): state 5, id + 1, header written.
- Consequence: a session costs 1 (open) + its writing transactions + 1 (close). A closed file's id is the last page id + 2 when the last session wrote; every later open/close without writes adds 2.
- **New file:** until `TE_DPgm_PageManagerActivate` @0x102f5708c (pgm+0x21 set), immediate page writes are deferred into the first transaction, so a new file's header pages carry id 1 like the rows of that transaction.

### Pages
- **Numbers** (`TE_DPgm_PageGetNew` @0x102f54f88): a table's first page is `next_unused_page`, its candidate the page after it, and `next_unused_page` += 2. Every later page of the table is its candidate; `next_unused_page` becomes the new candidate and += 1. The new page is zeroed, gets its number, type and `next` (the new candidate) and is written at once. The previous last page already links to it.
- **Init:** `TE_PSmgrPageInit` @0x102f68d3c and `TE_PSmgrInitHeaderPage` @0x102f68b84, as in § Pages.
- **First data page:** when a table gets its first data page, header-page field 0x2c is set and the header page is written at once *without* a new id (`TE_PSmgrSyncHeaderPage(…, 1)` → `TE_DPgm_PageWriteImmediate`). That is why the header pages of tables filled later still show id 1.
- **Buffer vs disk:** a change reaches the file only when its page is created, written immediately, or committed dirty. `TE_PSmgrClearDirtyBits` on a full insert page changes only the buffer (the page is released without being marked dirty), so the file keeps that page's last committed dirty bits.

### Insert (`TE_psmgr_request` @0x102f66f24)
`size` = `align4` of the row's allocation request (§ Offset width).
1. **Reuse list first** (`TE_PSmgrGetReusedSpace` @0x102f68e58):
   - `TE_PSmgrHandleDirtyHeader`: once per transaction and table, if the header page has dirty entries, `TE_PSmgrClearHeaderDirtyBits(…, 0)` drops the entries marked for removal (bit 2), clears bits 0/1 of the others, resets 0x20/0x22, updates the first-hole field and marks the header page dirty.
   - The list is skipped when it is empty or every entry is dirty (added in this transaction).
   - From the session's cursor, dirty and empty entries are skipped. The page found has its old dirty bits cleared (first touch in the transaction) and is **marked dirty even if nothing ends up there**. Then:
     - room at its end (`free ≥ size + 6` for a slot that starts a row group, `+ 2` otherwise): append there. Done.
     - otherwise a deleted slot (`TE_PSmgrGetSpaceMidPage` @0x102f69930, § Data pages). If the page then has no deleted slot, its entry gets bits 0 and 2 (removal pending), the header's dirty counters count it, the page loses flag 0x10 and the cursor moves on. The header page is marked dirty. Done.
     - otherwise the cursor moves to the next entry and one more page is tried; after two failures the insert falls through.
2. **Insert page:** the session's insert page, or a new page (`TE_PSmgrGetEmptyPage` → `TE_PSmgrPageGetNew`; the free list is always empty) when there is none. Its old dirty bits are cleared on first touch. Append if `free ≥ size + 6|2` and it has fewer than 2048 slots; otherwise a new page becomes the insert page.

So rows fill pages in insertion order, pages with deleted rows are filled first, and every session starts a new page per table unless the reuse list has room.

### Delete (`TE_psmgr_area_free` @0x102f673c4)
1. Clear the page's old dirty bits on first touch; mark it dirty.
2. If the page is not in the reuse list: `HandleDirtyHeader`, then `TE_PSmgrAddToReuseCache` @0x102f6a50c. The entry `page << 3 | 3` goes to the high-water mark (0x26 += 1) or into the first hole; 0x38 += 1; header dirty counters updated; header page marked dirty. The page gets flag 0x10.
3. `live -= 1`, present bit cleared, dirty bit as in § Data pages.

### Update
- **Copying UPDATE** (`TE_RunUpdateScanSmgr` @0x102f6e9e0): matching rows are collected, then each one (in reverse scan order) is inserted anew through the normal insert path and the old row is deleted, in one transaction. Verified on the oracle stick: every track IMAGEID update, every property update and the playlist reorder.
- **In-place UPDATE** (`TE_RunInPlaceUpdateScanSmgr` @0x102f6f5cc): copies new values of fixed-size columns into the row. Which statements use it, and how the page gets marked dirty (`TE_SmgrAreaMarkDirty` dispatches to storage-manager callback slot 14), is **open**. The oracle stick shows no in-place update that survived later writes; the model marks the page dirty.

## Export operation sequence (rekordbox 7.2.19)
Status: **verified** by the byte-exact replay of the oracle stick (`crates/mb-pdb/src/replay.rs`), except where marked. Every bullet is one transaction.

**Creating the stick** (`createDeviceDB` → `startupDeviceDB` → `initPropertyTable`):
1. All tables (header pages 1, 3, …) and the property row: DEVNAME "", DBVERSION "1000", SONGNUMBER 0, today's date, COLORID 0, PROTECTED 0. exportExt.pdb: all tables and the DBPROPERTYOPTION row.
2. Colours (8 rows), then menu items (27), categories (master `Seq` order) and sorts (master `Seq` order), one transaction each.

**Playlists** (UI thread, when the jobs are built):
- The first list on an empty stick: one transaction inserts the row with SEQ 0 and rewrites it with the same values (copying update). A deleted copy stays in slot 0.
- A further top-level list: inserted with SEQ = number of siblings. Later in the session one transaction moves it to position 0: every sibling is rewritten with SEQ + 1, then the new list with SEQ 0. On the oracle stick this ran after the jobs queued before it (timing **hypothesis**).
- An existing list whose tracks differ: all its entries are deleted in one transaction; its job re-adds them.

**Per new track** (`execExport` → `export_track_data_to_db` → `addTrack`, then `export_artwork`, `add_track_to_device_playlist`):
1. Each missing lookup row, in the order artist, genre, album, remixer, original artist, composer, label, key. A name that exists costs nothing.
2. The track row with IMAGEID 0, together with the property row rewritten with SONGNUMBER = track count (copying update): one `DevAddContent_` transaction.
3. If the track has artwork: the image row, then the track row rewritten with its IMAGEID (copying update).
4. The playlist entry.

A track that is already on the stick and unchanged costs no transaction; only its entry is added.

**My Tags** (exportExt.pdb, `syncMyTag`): one transaction deletes all My Tag rows (none on a new stick, so no id), then one transaction inserts the whole tree: each column by SEQ, followed by its tags by SEQ.

### The oracle stick's history (fitted ids)
| Ids | Session | Operations |
|---|---|---|
| 1–5 | 1 | create (+ property), colours, menu items, categories, sorts |
| 6 | 1 | `oracle-small` (insert + rewrite) |
| 7–42 | 1 | its 5 tracks: lookups, track + property, image, track update, entry |
| 43–45 | – | commit → 43, close → 44, open → 45 |
| 46–65 | – | ten close/open cycles without writes; the last open makes 65 current (**fitted**; the same ten cycles fit exportExt.pdb) |
| 65 | 2 | `oracle-small`'s 5 entries deleted |
| 66 | 2 | a write whose page is rewritten later (**unknown**) |
| 67 | 2 | `oracle-all` inserted with SEQ 1 |
| 68–72 | 2 | `oracle-small`'s entries re-added (its tracks unchanged) |
| 73 | 2 | a write whose page is rewritten later (**unknown**) |
| 74 | 2 | reorder: `oracle-small` SEQ 1, then `oracle-all` SEQ 0 |
| 75–818 | 2 | `oracle-all` in list order: 152 new tracks, 5 already there (entry only) |
| 819–822 | – | commit → 819, close, one more open/close cycle |

exportExt.pdb: 1 create + DBPROPERTYOPTION, 2 My Tag tree; close/open (3–5) and the same ten cycles (→ 25); 25/26 and 27/28 two more My Tag syncs (delete, insert); close and one more cycle (→ 32). Only the cycle counts and the two unknown writes are fitted; every page, slot, offset, mask, counter and stale byte follows from the model.

## Writer
Code: `crates/mb-pdb/src/encode.rs` (row formatter), `engine.rs` (storage model), `writer.rs` (API), `defaults.rs` (rows of a new stick), `lint.rs` (pdb-lint), `compare.rs` (page diff), `replay.rs` (oracle replay).

API:
- **`writer::write_export(&ExportDb)` / `write_ext(&ExtDb)`** (compact layout): the creation sequence above, then each remaining table in one transaction. Dense pages, no deleted rows; each page's dirty bits mark the rows of its table's transaction, as on rekordbox's colour, menu, category and sort pages.
- **`writer::export_like_rekordbox(&ExportDb)` / `ext_like_rekordbox(&ExtDb)`**: one export session in rekordbox's operation sequence. The first top-level list is created as rekordbox does; further lists get their final SEQ (the later reorder is a **hypothesis** and is not replayed). Then each list's tracks in entry order, tracks in no list, and the remaining rows compactly.
- **`writer::Exporter`**: the operation sequence step by step (`create`, `create_first_playlist`, `insert_playlist`, `create_playlist_at_top`, `add_track`, `update_track`, `add_entry`, `clear_playlist`, `reopen`, `finish`).
- **Incremental mode:** `Exporter::load(bytes)` opens an existing export.pdb as a new session (id + 1, no insert pages), indexes its live rows and continues with the same operations. Tested on the stick: a new list at position 0, a new track with artist and artwork, and an edited track. Only the changed pages change.
- **`engine::Db`**: the storage model (`create`, `load`, `begin`/`commit`/`txn`, `create_table`, `insert`, `delete`, `update`, `update_in_place`, `reopen`, `close`, `to_bytes`).
- **`defaults`**: colours, menu items, categories and sorts of a fresh rekordbox 7.2.19 library (those of the oracle stick), and the property and DBPROPERTYOPTION rows of a new stick. A library with edited browse menus needs its own master.db rows.
- **IDs** are the caller's. rekordbox uses `max + 1` per table in insertion order, its own playlist numbering, and master.db IDs for My Tags.

### Writer rules
Status: **[V]** verified (binary + byte-exact replay), **[F]** holds in every fixture file, **[H]** hypothesis. `pdb-lint` checks the structural ones.

**File and pages**
1. **[V] Page 0.** Page size 4096; table count 20 / 9, directory in type order; state 5 after close; `xid_hi` 0; id above every page's; zero after the directory.
2. **[V] Chains.** One per table, starting at its header page (flags 0x64). An empty table is only its header page, with 0x2c = 0x03ffffff and `next` = its candidate.
3. **[V] Page numbers and candidates.** As `TE_DPgm_PageGetNew` hands them out (§ Storage engine). Each table owns one candidate, its last page links to it, and candidates are never written: inside the file they are zero, and the file ends at the highest chained page.
4. **[V] Page header.** 0x00 = 0, 0x04 own number, 0x08 table type, 0x0c next, 0x10 id of the last transaction that changed the page, 0x14 = 0.
5. **[V] Data page counters.**
   - 0x18 = `slots | live << 13 | flags << 24`.
   - `free + used + 2·slots + 4·groups = 4056`.
   - 0x20 = number of dirty bits (0x1fff for none; 0 after an insert and delete in one transaction); 0x22 ≤ the lowest dirty slot (0x1fff for none).
   - 0x24 = 0x26 = 0; flag 0x10 exactly on the pages in the reuse list.
6. **[V] Dirty bits** mark the slots changed by the page's last transaction (inserted, deleted, or deleted and taken again). A later transaction clears them when it first touches the page. **[H]** Players ignore them; all-clean pages (masks 0, counters 0x1fff) are valid too, since `TE_PSmgrClearDirtyBits` produces them.
7. **[V] Header page.** As § Table header pages. The reuse list holds every page with a deleted slot, plus entries marked for removal (bit 2) until the table's next writing transaction. Its dirty bits mark entries changed in that table's last writing transaction.
8. **[V] Placement.** Rows go where the insert path puts them, `align4(size)` each. At most 2048 slots by the insert check (slot index ≤ 0x7ff), 510 by `CheckPsmgrHeader` for 4 KiB pages; the footer limits real pages far below.

**Rows and values**

9. **[V] Layout.** Row layout and header word per § Rows; offset width and allocation per § Offset width; alignment gaps are not written.
10. **[V] Strings.** ASCII ≤ 126 chars → short; longer ASCII → 0x40; anything else → 0x90 UTF-16LE; NULL → `03`; ISRC special form; menu names in U+FFFA … U+FFFB.
11. **[V] Values rekordbox writes.**
    - **Tracks:** FILENAMES, SEARCHSTR, TAG, RESERVED1 and RESERVED10 = `03`; DISABLEQUANTIZE 0; ANALYSED = `Analysed & 0x3f`; CONTENTLINK = `(src & 0xC1600) | 0x100`; RESERVED2/3 "ON" or ""; RESERVED5–7 the update counters as text.
    - **Property:** DEVNAME "" and DBVERSION "1000" on a fresh stick ("REKORDBOX" on the old stick); SONGNUMBER = track count, rewritten after every track.
    - **Colours, menu items, categories, sorts:** copied from master.db.
12. **[V] exportExt.pdb** is created with all 9 tables and the DBPROPERTYOPTION row in its first transaction. MYTAGMASTERDBID is the value rekordbox has at creation (oracle: 2140940981; source: `export.md` § 2.5).
13. **[H] Players.** Only the structure matters to them (chains, slot footers, row layouts, strings), and IDs only need to be consistent. The compact layout meets every rule above.

### Writer verification
`cargo test -p mb-pdb` (`tests/writer.rs`); examples `pdb_write` (read + rewrite), `pdb_lint`, `pdb_replay`, `pdb_slots`.

| Step | Result |
|---|---|
| a. Round trip | read → typed rows → `write_export` / `write_ext` → read gives identical rows for both files of both fixtures. `export_like_rekordbox` gives the same rows in its own order. Every stored slot of the four files (7826 rows, deleted ones included) re-encodes to rekordbox's bytes. |
| b. pdb-lint | 0 errors and 0 warnings on all four rekordbox files and on every file the writer produces (compact, export sequence, incremental onto the stick, from scratch). Corrupted files are flagged. |
| c. export.pdb vs the oracle | `replay::oracle_export` = rekordbox's file, **78/78 pages identical**. The compact rewrite shares 33 of 78 pages: the header pages of tables 1–6 and 9–18, the colour, menu-item, category and sort pages, and the zero candidates. The rest differs by design: rows packed table by table, no deleted versions, other ids. |
| d. exportExt.pdb vs the oracle | `replay::oracle_ext` = rekordbox's file, **18/18 pages identical**. The compact rewrite differs in 3 pages: page 0 (id), and the My Tag header and data pages (one tree instead of three). |

## Open questions
- [ ] CONTENTLINK value 0x000c0700 on every track: what it encodes.
- [ ] ANALYSED bits (shared with master.db `Analysed`).
- [ ] Category DISABLE values 2, 3 and 5; sort DISABLE 2; menu-item CLASS codes; COLORCODE; property COLORID.
- [ ] Layout of free-list pages (kind 2; entries use the same 3 flag bits as the reuse list). Only reached when a reuse list is full; not present in rekordbox files seen so far, not modelled by the writer.
- [ ] Playlist ATTRIBUTE values besides 0 (folder = 1 is a hypothesis); how smart playlists are exported.
- [ ] Whether players care about dirty masks, the reuse list or the ids. The writer reproduces rekordbox's, so this only matters for simpler writers. Needs a player test.
- [ ] Tables 9, 10, 14, 15 and the empty ext tables: no fixture rows, layouts from the binary only.
- [ ] Does rekordbox ever emit u32 offset arrays or 0x40/0x90 strings in other tables? Only seen in tracks (0x40 paths, 0x90 ISRC) and menu items (0x90).
- [ ] In-place UPDATE (`TE_RunInPlaceUpdateScanSmgr`): which DevModify statements use it, and how it marks the page dirty (storage-manager callback slot 14).
- [ ] Oracle replay, fitted parts: the ten close/open cycles at the start of session 2, and the two writes at ids 66 and 73 whose pages were rewritten later. A trace of the classic DB (DeviceSQL calls) would settle them.
- [ ] When rekordbox moves a new top-level list to position 0 when several new lists are exported in one session (`export_like_rekordbox` keeps the final SEQ instead).
