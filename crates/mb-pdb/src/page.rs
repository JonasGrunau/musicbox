//! Page level: file header, table directory, page chains, data pages and table header pages.
//! Spec: `docs/re/pdb.md` § File header, § Pages.
//!
//! All integers are little-endian. Page 0 is the file header; every other page starts with the
//! 24-byte page-manager header ([`PageHeader`] up to `xid_hi`), followed by the 16-byte storage
//! header (the rest of [`PageHeader`]) and the page body at offset 0x28.

use crate::{Error, Result, le_u16, le_u32};

/// "No page" sentinel used in page links (`TE_Dpgm_CheckPageHeader` @0x102f5916c accepts it as a
/// `next_page` value) and in table header pages.
pub const NO_PAGE: u32 = 0x03ff_ffff;
/// Sentinel for the 13/14-bit counters in a page header ("none").
pub const NONE_13: u16 = 0x1fff;
/// Start of the row heap (and of the header-page body) inside every page.
pub const HEAP_START: usize = 0x28;
/// Size of one row group footer: 16 slot offsets, the present mask and the dirty mask.
pub const ROW_GROUP_BYTES: usize = 0x24;
/// Slots per row group.
pub const ROWS_PER_GROUP: usize = 16;
/// Byte offset of the first entry of the reuse list in a table header page.
pub const HEADER_LIST_START: usize = 0x3c;
/// Empty reuse-list entry (`NO_PAGE << 3`).
pub const HEADER_LIST_EMPTY: u32 = 0x1fff_fff8;

/// Page 0. `TE_DPgm_InitialSetup` @0x102f588b8, `TE_DPgm_Close` @0x102f56568.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHeader {
    /// 0x00, always 0 in rekordbox files.
    pub reserved0: u32,
    /// 0x04, 4096 in every file seen.
    pub page_size: u32,
    /// 0x08, number of directory entries (20 in export.pdb, 9 in exportExt.pdb).
    pub table_count: u32,
    /// 0x0c, first page number never handed out. Every page number in the file is below it.
    pub next_unused_page: u32,
    /// 0x10, page-manager state: 5 = closed cleanly, 6 = closed with a flag set, 7 = written while
    /// open (set by `TE_DPgm_WritePage` before the first page write).
    pub state: u32,
    /// 0x14, low word of the 64-bit transaction id ("sequence"); incremented on close.
    pub xid_lo: u32,
    /// 0x18, high word of the transaction id.
    pub xid_hi: u32,
    /// 0x1c, one 16-byte entry per table.
    pub tables: Vec<TableDirEntry>,
}

/// One table directory entry in page 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableDirEntry {
    /// Table type = creation order in the DeviceSQL schema (`docs/re/pdb.md` § Tables).
    pub table_type: u32,
    /// A reserved, zero-filled page the table takes when it needs a new data page. The last
    /// page of the chain already links to it. It may lie beyond the end of the file.
    pub empty_candidate: u32,
    /// The table's header page (flags 0x64).
    pub first_page: u32,
    /// Last page of the chain; readers stop here.
    pub last_page: u32,
}

impl FileHeader {
    pub const SIZE_WITHOUT_TABLES: usize = 0x1c;

    pub fn parse(data: &[u8]) -> Result<Self> {
        let page_size = le_u32(data, 4)?;
        if !(0x200..=0x10000).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(Error::BadPageSize(page_size));
        }
        let table_count = le_u32(data, 8)?;
        let max_tables = (page_size as usize - Self::SIZE_WITHOUT_TABLES) / 16;
        if table_count as usize > max_tables {
            return Err(Error::BadPage {
                page: 0,
                reason: format!("{table_count} tables do not fit"),
            });
        }
        let mut tables = Vec::with_capacity(table_count as usize);
        for i in 0..table_count as usize {
            let o = Self::SIZE_WITHOUT_TABLES + 16 * i;
            tables.push(TableDirEntry {
                table_type: le_u32(data, o)?,
                empty_candidate: le_u32(data, o + 4)?,
                first_page: le_u32(data, o + 8)?,
                last_page: le_u32(data, o + 12)?,
            });
        }
        Ok(Self {
            reserved0: le_u32(data, 0)?,
            page_size,
            table_count,
            next_unused_page: le_u32(data, 0xc)?,
            state: le_u32(data, 0x10)?,
            xid_lo: le_u32(data, 0x14)?,
            xid_hi: le_u32(data, 0x18)?,
            tables,
        })
    }

    /// Encodes page 0 (zero-padded to the page size). Inverse of [`FileHeader::parse`].
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.page_size as usize];
        for (o, v) in [
            (0, self.reserved0),
            (4, self.page_size),
            (8, self.table_count),
            (0xc, self.next_unused_page),
            (0x10, self.state),
            (0x14, self.xid_lo),
            (0x18, self.xid_hi),
        ] {
            out[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, t) in self.tables.iter().enumerate() {
            let o = Self::SIZE_WITHOUT_TABLES + 16 * i;
            for (k, v) in [t.table_type, t.empty_candidate, t.first_page, t.last_page]
                .into_iter()
                .enumerate()
            {
                out[o + 4 * k..o + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    pub fn table(&self, table_type: u32) -> Option<&TableDirEntry> {
        self.tables.iter().find(|t| t.table_type == table_type)
    }
}

/// What a page holds, from the top three bits of the flags byte (`TE_PSmgrPageInit` @0x102f68d3c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    /// Never written (all zero): reserved empty candidates and unused pages.
    Unused,
    /// Kind 1: rows (flags 0x24, or 0x34 when the page sits in the table's reuse list).
    Data,
    /// Kind 2: free-page list page (`TE_PSmgrAddToFreeList` @0x102f6a9b8); not seen in
    /// rekordbox files.
    Kind2,
    /// Kind 3: the table header page that starts every chain (flags 0x64).
    TableHeader,
    Other(u8),
}

/// Header of every page except page 0 (bytes 0x00..0x28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageHeader {
    /// 0x00, always 0.
    pub reserved0: u32,
    /// 0x04, this page's own number.
    pub page_index: u32,
    /// 0x08, owning table type.
    pub table_type: u32,
    /// 0x0c, next page of the chain (or the table's empty candidate after the last page).
    pub next_page: u32,
    /// 0x10, transaction id (low word) of the last write; never above the file header's.
    pub xid_lo: u32,
    /// 0x14, transaction id (high word).
    pub xid_hi: u32,
    /// 0x18 bits 0..12: number of row slots ever allocated in the page (live and deleted).
    pub slot_count: u16,
    /// 0x18 bits 13..25: number of live (present) rows.
    pub live_count: u16,
    /// 0x18 bits 26..31 as the raw top byte (0x1b): `kind << 5 | in_reuse_list << 4 | 0x04`.
    pub flags: u8,
    /// 0x1c: free heap bytes (`heap size - used - footer`).
    pub free_size: u16,
    /// 0x1e: bytes of the heap allocated to rows, from 0x28 upwards (deleted rows included).
    pub used_size: u16,
    /// 0x20 (14 bits): number of dirty slots (data page) or dirty reuse-list entries (header
    /// page), or 0x1fff for none (`TE_PSmgrAddDirtyArea` @0x102f69d28).
    pub dirty_count: u16,
    /// 0x22 (14 bits): lowest dirty slot (data page) or list entry (header page), or 0x1fff.
    pub first_dirty_slot: u16,
    /// 0x24: 0 on data pages; capacity of the reuse list on table header pages (1004 for 4 KiB).
    pub aux0: u16,
    /// 0x26: 0 on data pages; slots of the reuse list in use (high-water mark) on header pages.
    pub aux1: u16,
}

impl PageHeader {
    pub const SIZE: usize = HEAP_START;

    pub fn parse(page: &[u8]) -> Result<Self> {
        let packed = le_u32(page, 0x18)?;
        Ok(Self {
            reserved0: le_u32(page, 0)?,
            page_index: le_u32(page, 4)?,
            table_type: le_u32(page, 8)?,
            next_page: le_u32(page, 0xc)?,
            xid_lo: le_u32(page, 0x10)?,
            xid_hi: le_u32(page, 0x14)?,
            slot_count: (packed & 0x1fff) as u16,
            live_count: ((packed >> 13) & 0x1fff) as u16,
            flags: (packed >> 24) as u8 & !0x03,
            free_size: le_u16(page, 0x1c)?,
            used_size: le_u16(page, 0x1e)?,
            dirty_count: le_u16(page, 0x20)?,
            first_dirty_slot: le_u16(page, 0x22)?,
            aux0: le_u16(page, 0x24)?,
            aux1: le_u16(page, 0x26)?,
        })
    }

    /// Encodes the 40 header bytes. Inverse of [`PageHeader::parse`].
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let packed = u32::from(self.slot_count & 0x1fff)
            | (u32::from(self.live_count & 0x1fff) << 13)
            | (u32::from(self.flags & !0x03) << 24);
        for (o, v) in [
            (0, self.reserved0),
            (4, self.page_index),
            (8, self.table_type),
            (0xc, self.next_page),
            (0x10, self.xid_lo),
            (0x14, self.xid_hi),
            (0x18, packed),
        ] {
            out[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (o, v) in [
            (0x1c, self.free_size),
            (0x1e, self.used_size),
            (0x20, self.dirty_count),
            (0x22, self.first_dirty_slot),
            (0x24, self.aux0),
            (0x26, self.aux1),
        ] {
            out[o..o + 2].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    pub fn kind(&self) -> PageKind {
        if self.flags == 0 && self.page_index == 0 && self.next_page == 0 {
            return PageKind::Unused;
        }
        match self.flags >> 5 {
            1 => PageKind::Data,
            2 => PageKind::Kind2,
            3 => PageKind::TableHeader,
            k => PageKind::Other(k),
        }
    }

    /// Flag 0x10: the page has reclaimable space from deleted rows and is listed in the table
    /// header page's reuse list (`TE_psmgr_area_free` @0x102f673c4, `TE_PSmgrAddToReuseCache`).
    pub fn in_reuse_list(&self) -> bool {
        self.flags & 0x10 != 0
    }
}

/// One row slot of a data page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// Slot number (0-based). Header rows store it in bits 21..31 of their row header.
    pub index: u16,
    /// Row offset relative to the heap start (page offset 0x28).
    pub offset: u16,
    /// Present bit: the row is live. Deleted rows keep their bytes and offset.
    pub present: bool,
    /// Dirty bit: the slot changed (insert or delete) in the last write transaction.
    pub dirty: bool,
}

/// A page holding rows (kind 1).
#[derive(Debug, Clone, Copy)]
pub struct DataPage<'a> {
    pub number: u32,
    pub header: PageHeader,
    pub bytes: &'a [u8],
}

impl<'a> DataPage<'a> {
    /// Number of row groups in the footer.
    pub fn group_count(&self) -> usize {
        (self.header.slot_count as usize).div_ceil(ROWS_PER_GROUP)
    }

    /// Bytes taken by the slot footer at the end of the page.
    pub fn footer_len(&self) -> usize {
        2 * self.header.slot_count as usize + 4 * self.group_count()
    }

    /// (present mask, dirty mask) of row group `g`.
    pub fn group_masks(&self, g: usize) -> Result<(u16, u16)> {
        let base = self.bytes.len() - ROW_GROUP_BYTES * g;
        Ok((le_u16(self.bytes, base - 4)?, le_u16(self.bytes, base - 2)?))
    }

    /// Every allocated slot in slot order, live and deleted (`TE_psmgr_getnext` @0x102f67818).
    pub fn slots(&self) -> Result<Vec<Slot>> {
        let n = self.header.slot_count as usize;
        let mut out = Vec::with_capacity(n);
        for g in 0..self.group_count() {
            let base = self.bytes.len() - ROW_GROUP_BYTES * g;
            let (present, dirty) = self.group_masks(g)?;
            for k in 0..ROWS_PER_GROUP.min(n - ROWS_PER_GROUP * g) {
                let offset = le_u16(self.bytes, base - 6 - 2 * k)?;
                out.push(Slot {
                    index: (ROWS_PER_GROUP * g + k) as u16,
                    offset,
                    present: present >> k & 1 != 0,
                    dirty: dirty >> k & 1 != 0,
                });
            }
        }
        Ok(out)
    }

    /// Bytes from the row start to the end of the heap area (the footer is excluded).
    pub fn row_bytes(&self, slot: &Slot) -> Result<&'a [u8]> {
        let start = HEAP_START + slot.offset as usize;
        let end = self.bytes.len() - self.footer_len();
        if start >= end {
            return Err(Error::BadRow {
                page: self.number,
                slot: slot.index,
                reason: format!("offset {:#x} outside the heap", slot.offset),
            });
        }
        Ok(&self.bytes[start..end])
    }
}

/// The table header page that starts each chain (kind 3, flags 0x64).
/// `TE_PSmgrInitHeaderPage` @0x102f68b84.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderPage {
    pub number: u32,
    pub header: PageHeader,
    /// 0x28: this page's own number.
    pub self_page: u32,
    /// 0x2c: first data page of the table, or [`NO_PAGE`] when the table has none.
    pub first_data_page: u32,
    /// 0x30: head of the free-page list (kind-2 pages), [`NO_PAGE`] when empty.
    pub free_list_head: u32,
    /// 0x34: tail of the free-page list; 0 until the first free-list page exists.
    pub free_list_tail: u32,
    /// 0x38: number of valid entries in the reuse list.
    pub reuse_count: u16,
    /// 0x3a: index of the first empty slot inside the used part of the list, or 0x1fff.
    pub first_free_entry: u16,
    /// Raw list entries, `aux0` of them: `page << 3 | flags` with flag bit 0 = dirty, bit 1 =
    /// added in the current transaction, bit 2 = removal pending
    /// (`TE_PSmgrAddToReuseCache` @0x102f6a50c); [`HEADER_LIST_EMPTY`] for none.
    pub entries: Vec<u32>,
}

impl HeaderPage {
    pub fn parse(number: u32, page: &[u8]) -> Result<Self> {
        let header = PageHeader::parse(page)?;
        let cap = header.aux0 as usize;
        let mut entries = Vec::with_capacity(cap);
        for i in 0..cap {
            entries.push(le_u32(page, HEADER_LIST_START + 4 * i)?);
        }
        Ok(Self {
            number,
            header,
            self_page: le_u32(page, 0x28)?,
            first_data_page: le_u32(page, 0x2c)?,
            free_list_head: le_u32(page, 0x30)?,
            free_list_tail: le_u32(page, 0x34)?,
            reuse_count: le_u16(page, 0x38)?,
            first_free_entry: le_u16(page, 0x3a)?,
            entries,
        })
    }

    /// Pages currently in the reuse list (pages with space from deleted rows).
    pub fn reuse_pages(&self) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|&&e| e != HEADER_LIST_EMPTY)
            .map(|e| e >> 3)
            .collect()
    }
}

/// A whole PDB file, kept byte for byte.
#[derive(Debug, Clone)]
pub struct Pdb {
    data: Vec<u8>,
    pub header: FileHeader,
}

impl Pdb {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let header = FileHeader::parse(&data)?;
        if !data.len().is_multiple_of(header.page_size as usize) {
            return Err(Error::BadPage {
                page: 0,
                reason: format!(
                    "file length {} is not a multiple of the page size",
                    data.len()
                ),
            });
        }
        Ok(Self { data, header })
    }

    pub fn open(path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(data)
    }

    /// The original file bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn page_size(&self) -> usize {
        self.header.page_size as usize
    }

    /// Pages physically present in the file (page numbers may reach beyond, see
    /// [`TableDirEntry::empty_candidate`]).
    pub fn page_count(&self) -> u32 {
        (self.data.len() / self.page_size()) as u32
    }

    pub fn page(&self, number: u32) -> Result<&[u8]> {
        let ps = self.page_size();
        let start = number as usize * ps;
        self.data.get(start..start + ps).ok_or(Error::Truncated {
            offset: start,
            need: ps,
            len: self.data.len(),
        })
    }

    pub fn page_header(&self, number: u32) -> Result<PageHeader> {
        PageHeader::parse(self.page(number)?)
    }

    /// Page numbers of a table's chain from its header page to `last_page`, following
    /// `next_page`. Errors on loops, out-of-file links and pages owned by another table.
    pub fn chain(&self, entry: &TableDirEntry) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut page = entry.first_page;
        loop {
            if page >= self.page_count() {
                return Err(Error::BadChain {
                    table: entry.table_type,
                    page,
                    reason: "link beyond end of file",
                });
            }
            if !seen.insert(page) {
                return Err(Error::BadChain {
                    table: entry.table_type,
                    page,
                    reason: "loop",
                });
            }
            let h = self.page_header(page)?;
            if h.page_index != page || h.table_type != entry.table_type {
                return Err(Error::BadChain {
                    table: entry.table_type,
                    page,
                    reason: "page header does not match",
                });
            }
            out.push(page);
            if page == entry.last_page {
                return Ok(out);
            }
            page = h.next_page;
        }
    }

    /// The header page of a table.
    pub fn header_page(&self, entry: &TableDirEntry) -> Result<HeaderPage> {
        HeaderPage::parse(entry.first_page, self.page(entry.first_page)?)
    }

    /// Data pages of a table in chain order.
    pub fn data_pages(&self, entry: &TableDirEntry) -> Result<Vec<DataPage<'_>>> {
        let mut out = Vec::new();
        for number in self.chain(entry)? {
            let bytes = self.page(number)?;
            let header = PageHeader::parse(bytes)?;
            match header.kind() {
                PageKind::Data => out.push(DataPage {
                    number,
                    header,
                    bytes,
                }),
                PageKind::TableHeader => {}
                other => {
                    return Err(Error::BadPage {
                        page: number,
                        reason: format!("unexpected page kind {other:?} in chain"),
                    });
                }
            }
        }
        Ok(out)
    }

    /// Live rows of a table: (page, slot, row bytes) in chain and slot order.
    pub fn live_rows(&self, table_type: u32) -> Result<Vec<(u32, Slot, &[u8])>> {
        let entry = self
            .header
            .table(table_type)
            .ok_or(Error::UnknownTable(table_type))?;
        let mut out = Vec::new();
        for page in self.data_pages(entry)? {
            for slot in page.slots()? {
                if slot.present {
                    out.push((page.number, slot, page.row_bytes(&slot)?));
                }
            }
        }
        Ok(out)
    }
}
