//! A model of the DeviceSQL storage layer that writes export.pdb / exportExt.pdb the way
//! rekordbox 7.2.19 does. Spec: `docs/re/pdb.md` § Writer, § Storage engine.
//!
//! The model follows the runtime functions one to one, so a sequence of operations produces the
//! same pages, slots, counters and transaction ids as rekordbox running the same sequence:
//! - disk page manager: `TE_DPgm_Open` @0x102f55c40 (open = transaction id + 1),
//!   `TE_DPgm_Commit` @0x102f554d4 (+1 when the transaction wrote), `TE_DPgm_Close` @0x102f56568
//!   (+1, state 5), `TE_DPgm_PageGetNew` @0x102f54f88 (page numbers and empty candidates),
//!   `TE_DPgm_PageMarkDirty` @0x102f552e8 (page transaction id);
//! - storage manager: `TE_psmgr_request` @0x102f66f24 (insert), `TE_PSmgrGetReusedSpace`
//!   @0x102f68e58, `TE_PSmgrGetSpaceAtEnd` @0x102f696ac, `TE_PSmgrGetSpaceMidPage` @0x102f69930,
//!   `TE_psmgr_area_free` @0x102f673c4 (delete), `TE_PSmgrClearDirtyBits` @0x102f694d4,
//!   `TE_PSmgrAddDirtyArea` @0x102f69d28, `TE_PSmgrAddToReuseCache` @0x102f6a50c,
//!   `TE_PSmgrHandleDirtyHeader` @0x102f69890, `TE_PSmgrClearHeaderDirtyBits` @0x102f683b0,
//!   `TE_PSmgrInitHeaderPage` @0x102f68b84, `TE_PSmgrPageInit` @0x102f68d3c;
//! - rows: `TE_RowFormSmgr` @0x102f7572c ([`crate::encode`]); an UPDATE that changes a string
//!   column is an insert of the new row followed by a delete of the old one
//!   (`TE_RunUpdateScanSmgr` @0x102f6e9e0).
//!
//! Every page has an in-memory image and a disk image. Changes reach the disk image only where the
//! runtime writes the page: when it is created, when it is written immediately, and at commit for
//! pages marked dirty. This matters for `TE_PSmgrClearDirtyBits` on a full insert page, which
//! changes the buffer without marking it dirty.

use std::collections::{BTreeMap, BTreeSet};

use crate::encode::{EncodedRow, encode_row};
use crate::page::{
    FileHeader, HEADER_LIST_EMPTY, HEADER_LIST_START, HEAP_START, NO_PAGE, NONE_13, Pdb,
    ROW_GROUP_BYTES, ROWS_PER_GROUP, TableDirEntry,
};
use crate::row::Value;
use crate::schema::{DbKind, RowLayout};
use crate::{Error, Result};

/// Page size rekordbox uses for both files.
pub const PAGE_SIZE: usize = 4096;

/// A row's place: page number and slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowRef {
    pub page: u32,
    pub slot: u16,
}

/// Session state of one table's storage manager (`TE_psmgr_open` @0x102f66b30 resets it).
#[derive(Debug, Clone, Copy)]
struct TableSession {
    /// psmgr+0x20: page that takes inserts at its end; none after open.
    cur_page: u32,
    /// psmgr+0x24: rotating index into the reuse list.
    cursor: u32,
    /// psmgr+0x28: the header page was handled in this transaction; reset at commit.
    header_dirty: bool,
}

impl TableSession {
    fn new() -> Self {
        Self {
            cur_page: NO_PAGE,
            cursor: 0,
            header_dirty: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Page {
    mem: Vec<u8>,
    disk: Option<Vec<u8>>,
}

/// Counters of what the model did, for tests and diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub inserts_at_end: usize,
    pub inserts_reused_end: usize,
    pub inserts_mid_page: usize,
    pub inserts_new_page: usize,
    pub deletes: usize,
    pub commits: usize,
}

/// One DeviceSQL database file being written.
#[derive(Debug, Clone)]
pub struct Db {
    kind: DbKind,
    header: FileHeader,
    pages: BTreeMap<u32, Page>,
    sess: Vec<TableSession>,
    layouts: Vec<RowLayout>,
    /// Pages marked dirty in the current transaction (`TE_DPgm_PageIsDirty`).
    txn_dirty: BTreeSet<u32>,
    /// Something was written in the current transaction (pgm+0x1c).
    txn_changed: bool,
    in_txn: bool,
    is_open: bool,
    pub stats: Stats,
}

fn rd16(p: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([p[o], p[o + 1]])
}
fn wr16(p: &mut [u8], o: usize, v: u16) {
    p[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn rd32(p: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]])
}
fn wr32(p: &mut [u8], o: usize, v: u32) {
    p[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// Page header fields (`docs/re/pdb.md` § Pages).
const P_NEXT: usize = 0x0c;
const P_XID: usize = 0x10;
const P_PACKED: usize = 0x18;
const P_FREE: usize = 0x1c;
const P_USED: usize = 0x1e;
const P_DIRTY_N: usize = 0x20;
const P_DIRTY_FIRST: usize = 0x22;
const P_AUX0: usize = 0x24;
const P_AUX1: usize = 0x26;
// Table header page body.
const H_SELF: usize = 0x28;
const H_FIRST_DATA: usize = 0x2c;
const H_FREE_HEAD: usize = 0x30;
const H_REUSE_COUNT: usize = 0x38;
const H_FIRST_HOLE: usize = 0x3a;

const FLAG_REUSE: u32 = 0x1000_0000;

fn slot_count(p: &[u8]) -> u32 {
    rd32(p, P_PACKED) & 0x1fff
}
fn live_count(p: &[u8]) -> u32 {
    (rd32(p, P_PACKED) >> 13) & 0x1fff
}
fn set_counts(p: &mut [u8], slots: u32, live: u32) {
    let packed = rd32(p, P_PACKED);
    wr32(
        p,
        P_PACKED,
        (packed & 0xfc00_0000) | (slots & 0x1fff) | ((live & 0x1fff) << 13),
    );
}
fn dirty_n(p: &[u8]) -> u16 {
    rd16(p, P_DIRTY_N) & 0x3fff
}
fn dirty_first(p: &[u8]) -> u16 {
    rd16(p, P_DIRTY_FIRST) & 0x3fff
}
fn set_dirty_n(p: &mut [u8], v: u16) {
    let old = rd16(p, P_DIRTY_N);
    wr16(p, P_DIRTY_N, (old & 0xc000) | (v & 0x3fff));
}
fn set_dirty_first(p: &mut [u8], v: u16) {
    let old = rd16(p, P_DIRTY_FIRST);
    wr16(p, P_DIRTY_FIRST, (old & 0xc000) | (v & 0x3fff));
}

/// `TE_PSmgrAddDirtyArea`: count one more dirty slot (or list entry) and keep the minimum.
fn add_dirty_area(p: &mut [u8], idx: u32) {
    if dirty_n(p) == NONE_13 {
        set_dirty_n(p, 1);
        set_dirty_first(p, idx as u16);
    } else {
        let v = dirty_n(p) + 1;
        set_dirty_n(p, v);
        if idx < u32::from(dirty_first(p)) {
            set_dirty_first(p, idx as u16);
        }
    }
}

/// `TE_PSmgrCalcAddedOffset`: footer bytes a new slot costs.
fn added_offset(slot: u32) -> u32 {
    if slot.is_multiple_of(ROWS_PER_GROUP as u32) {
        6
    } else {
        2
    }
}

/// Footer positions of slot `s` in a page of `ps` bytes: (present mask, dirty mask, offset).
fn footer(ps: usize, s: u32) -> (usize, usize, usize) {
    let end = ps - ROW_GROUP_BYTES * (s as usize / ROWS_PER_GROUP);
    let k = s as usize % ROWS_PER_GROUP;
    (end - 4, end - 2, end - 6 - 2 * k)
}

impl Db {
    /// A new, empty file opened for writing (`TE_DPgm_Open` on a missing file, then the open
    /// transaction-id increment). Tables are added with [`Db::create_table`] inside a transaction.
    pub fn create(kind: DbKind) -> Self {
        let mut db = Self {
            kind,
            header: FileHeader {
                reserved0: 0,
                page_size: PAGE_SIZE as u32,
                table_count: 0,
                next_unused_page: 1,
                state: 0,
                xid_lo: 0,
                xid_hi: 0,
                tables: Vec::new(),
            },
            pages: BTreeMap::new(),
            sess: Vec::new(),
            layouts: Vec::new(),
            txn_dirty: BTreeSet::new(),
            txn_changed: false,
            in_txn: false,
            is_open: false,
            stats: Stats::default(),
        };
        db.open();
        db
    }

    /// Loads an existing file and opens it for writing (incremental mode).
    pub fn load(kind: DbKind, bytes: &[u8]) -> Result<Self> {
        let header = FileHeader::parse(bytes)?;
        if header.page_size as usize != PAGE_SIZE {
            return Err(Error::BadPageSize(header.page_size));
        }
        let mut pages = BTreeMap::new();
        for (i, chunk) in bytes.chunks(PAGE_SIZE).enumerate().skip(1) {
            if chunk.iter().any(|&b| b != 0) {
                let mut mem = chunk.to_vec();
                mem.resize(PAGE_SIZE, 0);
                pages.insert(
                    i as u32,
                    Page {
                        disk: Some(mem.clone()),
                        mem,
                    },
                );
            }
        }
        let tables = kind.tables();
        let mut db = Self {
            kind,
            sess: vec![TableSession::new(); header.tables.len()],
            layouts: tables.iter().map(|t| t.layout()).collect(),
            header,
            pages,
            txn_dirty: BTreeSet::new(),
            txn_changed: false,
            in_txn: false,
            is_open: false,
            stats: Stats::default(),
        };
        db.layouts.truncate(db.header.tables.len());
        db.open();
        Ok(db)
    }

    pub fn kind(&self) -> DbKind {
        self.kind
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Current transaction id (the id the pages of the running transaction get).
    pub fn xid(&self) -> u32 {
        self.header.xid_lo
    }

    /// `TE_DPgm_Open`: a writable open bumps the transaction id and resets every table's
    /// session state (`TE_psmgr_open`).
    pub fn open(&mut self) {
        assert!(!self.is_open, "database already open");
        self.header.xid_lo += 1;
        self.header.state = 4;
        for s in &mut self.sess {
            *s = TableSession::new();
        }
        self.is_open = true;
    }

    /// `TE_DPgm_Close`: state 5 and one more transaction id.
    pub fn close(&mut self) {
        assert!(self.is_open && !self.in_txn, "close outside a transaction");
        self.header.state = 5;
        self.header.xid_lo += 1;
        self.is_open = false;
    }

    /// Close and reopen without writing anything (rekordbox does this when it reopens a device).
    pub fn reopen(&mut self) {
        self.close();
        self.open();
    }

    pub fn begin(&mut self) {
        assert!(self.is_open && !self.in_txn, "begin inside a transaction");
        self.in_txn = true;
        self.txn_dirty.clear();
        self.txn_changed = false;
    }

    /// `TE_DPgm_Commit`: dirty pages are written with the current id, which then advances.
    pub fn commit(&mut self) {
        assert!(self.in_txn, "commit outside a transaction");
        if self.txn_changed {
            for &n in &self.txn_dirty {
                let p = self.pages.get_mut(&n).expect("dirty page exists");
                p.disk = Some(p.mem.clone());
            }
            self.header.xid_lo += 1;
            self.header.state = 1;
            self.stats.commits += 1;
        }
        self.txn_dirty.clear();
        self.txn_changed = false;
        for s in &mut self.sess {
            s.header_dirty = false;
        }
        self.in_txn = false;
    }

    /// Runs `f` in one transaction.
    pub fn txn<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.begin();
        let r = f(self);
        self.commit();
        r
    }

    /// The file as rekordbox leaves it on disk: page 0 plus every page written so far, up to the
    /// highest one (reserved empty candidates beyond it are not part of the file).
    pub fn to_bytes(&self) -> Vec<u8> {
        let last = self
            .pages
            .iter()
            .filter(|(_, p)| p.disk.is_some())
            .map(|(&n, _)| n)
            .max()
            .unwrap_or(0);
        let mut out = vec![0u8; PAGE_SIZE * (last as usize + 1)];
        out[..PAGE_SIZE].copy_from_slice(&self.header.to_bytes());
        for (&n, p) in &self.pages {
            if let Some(d) = &p.disk {
                let o = n as usize * PAGE_SIZE;
                out[o..o + PAGE_SIZE].copy_from_slice(d);
            }
        }
        out
    }

    /// Reader over the current disk state.
    pub fn to_pdb(&self) -> Result<Pdb> {
        Pdb::parse(self.to_bytes())
    }

    fn mem(&self, n: u32) -> &[u8] {
        &self.pages.get(&n).expect("page exists").mem
    }

    fn mem_mut(&mut self, n: u32) -> &mut [u8] {
        &mut self.pages.get_mut(&n).expect("page exists").mem
    }

    /// `TE_DPgm_PageMarkDirty`: the page joins the transaction and takes its id.
    fn mark_dirty(&mut self, n: u32) {
        if self.txn_dirty.insert(n) {
            let xid = self.header.xid_lo;
            let hi = self.header.xid_hi;
            let p = self.mem_mut(n);
            wr32(p, P_XID, xid);
            wr32(p, P_XID + 4, hi);
        }
        self.txn_changed = true;
    }

    /// `TE_DPgm_PageWriteImmediate`: the page goes to disk now, without a new transaction id.
    fn write_immediate(&mut self, n: u32) {
        self.txn_dirty.remove(&n);
        let p = self.pages.get_mut(&n).expect("page exists");
        p.disk = Some(p.mem.clone());
    }

    /// `TE_DPgm_PageGetNew`: hands out the table's next page. The first page of a table is
    /// `next_unused_page` with the empty candidate right after it; later pages take the current
    /// candidate and reserve `next_unused_page` as the new one. The page is written at once.
    fn dpgm_page_get_new(&mut self, t: usize) -> u32 {
        let e = &mut self.header.tables[t];
        if e.last_page == NO_PAGE {
            let n = self.header.next_unused_page;
            e.first_page = n;
            e.last_page = n;
            e.empty_candidate = n + 1;
            self.header.next_unused_page += 2;
        } else {
            e.last_page = e.empty_candidate;
            e.empty_candidate = self.header.next_unused_page;
            self.header.next_unused_page += 1;
        }
        let (n, ty, next) = (e.last_page, e.table_type, e.empty_candidate);
        let mut mem = vec![0u8; PAGE_SIZE];
        wr32(&mut mem, 4, n);
        wr32(&mut mem, 8, ty);
        wr32(&mut mem, P_NEXT, next);
        self.pages.insert(
            n,
            Page {
                disk: Some(mem.clone()),
                mem,
            },
        );
        self.txn_changed = true;
        n
    }

    /// `TE_PSmgrPageInit`: kind 1 (data) or 3 (table header).
    fn page_init(p: &mut [u8], kind: u32) {
        let packed = rd32(p, P_PACKED);
        let packed = (packed & 0x1fff_ffff & !0x0400_0000) | 0x0400_0000 | (kind << 29);
        wr32(p, P_PACKED, packed);
        let free = if kind == 1 {
            (PAGE_SIZE - HEAP_START) as u16
        } else {
            0
        };
        wr16(p, P_FREE, free);
        wr16(p, P_USED, 0);
        set_dirty_n(p, NONE_13);
        set_dirty_first(p, NONE_13);
    }

    /// Registers the next table of the schema and writes its header page
    /// (`TE_DPgm_Register` + `TE_PSmgrInitHeaderPage`). Must run inside a transaction; rekordbox
    /// creates all tables of a file in its first transaction.
    pub fn create_table(&mut self) -> Result<u32> {
        assert!(self.in_txn);
        let t = self.header.tables.len();
        let schema = self
            .kind
            .tables()
            .get(t)
            .ok_or(Error::UnknownTable(t as u32))?;
        self.header.tables.push(TableDirEntry {
            table_type: schema.type_id,
            empty_candidate: NO_PAGE,
            first_page: NO_PAGE,
            last_page: NO_PAGE,
        });
        self.header.table_count = self.header.tables.len() as u32;
        self.sess.push(TableSession::new());
        self.layouts.push(schema.layout());
        let n = self.dpgm_page_get_new(t);
        let cap = ((PAGE_SIZE - HEAP_START - 0x28) / 4) as u16;
        let p = self.mem_mut(n);
        Self::page_init(p, 3);
        wr32(p, H_SELF, n);
        wr32(p, H_FIRST_DATA, NO_PAGE);
        wr32(p, H_FREE_HEAD, NO_PAGE);
        wr16(p, H_REUSE_COUNT, 0);
        wr16(p, H_FIRST_HOLE, NONE_13);
        wr16(p, P_AUX0, cap);
        wr16(p, P_AUX1, 0);
        for i in 0..cap as usize {
            wr32(p, HEADER_LIST_START + 4 * i, HEADER_LIST_EMPTY);
        }
        // Inside the creating transaction the immediate write is deferred to the commit
        // (the header pages of a new file carry the first transaction id).
        self.mark_dirty(n);
        Ok(n)
    }

    fn header_page(&self, t: usize) -> u32 {
        self.header.tables[t].first_page
    }

    /// `TE_PSmgrClearDirtyBits`: forget the previous transaction's dirty marks of a data page.
    fn clear_dirty_bits(&mut self, n: u32) {
        let p = self.mem_mut(n);
        if dirty_n(p) == NONE_13 {
            return;
        }
        let mut g = dirty_first(p) as usize / ROWS_PER_GROUP;
        while dirty_n(p) != 0 {
            let at = PAGE_SIZE - ROW_GROUP_BYTES * g - 2;
            let bits = rd16(p, at).count_ones() as u16;
            let v = dirty_n(p) - bits;
            set_dirty_n(p, v);
            wr16(p, at, 0);
            g += 1;
        }
        set_dirty_n(p, NONE_13);
        set_dirty_first(p, NONE_13);
        let packed = rd32(p, P_PACKED);
        if packed & FLAG_REUSE != 0 && slot_count(p) == live_count(p) {
            wr32(p, P_PACKED, packed & !FLAG_REUSE);
        }
    }

    /// Page lookup by the storage manager: first touch in a transaction clears old dirty bits.
    fn touch(&mut self, n: u32) {
        if !self.txn_dirty.contains(&n) {
            self.clear_dirty_bits(n);
        }
    }

    /// `TE_PSmgrClearHeaderDirtyBits(…, 0)` + sync: commit the previous transaction's reuse-list
    /// changes (drop entries marked for removal, clear the flag bits).
    fn handle_dirty_header(&mut self, t: usize) {
        if self.sess[t].header_dirty {
            return;
        }
        let h = self.header_page(t);
        let p = self.mem_mut(h);
        if dirty_n(p) == NONE_13 {
            return;
        }
        let mut idx = dirty_first(p) as usize;
        let mut first_hole = NONE_13;
        while dirty_n(p) != 0 {
            let at = HEADER_LIST_START + 4 * idx;
            let mut e = rd32(p, at);
            if e & 1 == 1 {
                if e & 4 != 0 {
                    e &= !4;
                    e = (e & 7) | HEADER_LIST_EMPTY;
                    if first_hole == NONE_13 {
                        first_hole = idx as u16;
                    }
                    let v = rd16(p, H_REUSE_COUNT) - 1;
                    wr16(p, H_REUSE_COUNT, v);
                }
                e &= !3;
                wr32(p, at, e);
                let v = dirty_n(p) - 1;
                set_dirty_n(p, v);
            }
            idx += 1;
        }
        set_dirty_n(p, NONE_13);
        set_dirty_first(p, NONE_13);
        if first_hole != NONE_13 && first_hole < rd16(p, H_FIRST_HOLE) {
            wr16(p, H_FIRST_HOLE, first_hole);
        }
        self.mark_dirty(h);
    }

    /// `TE_PSmgrAddToReuseCache`: list a page that got a deleted row. Returns whether it was
    /// added (false when the list is full).
    fn add_to_reuse_cache(&mut self, t: usize, page: u32) -> bool {
        let h = self.header_page(t);
        let p = self.mem_mut(h);
        let cap = rd16(p, P_AUX0);
        let count = rd16(p, H_REUSE_COUNT);
        if cap == count {
            return false;
        }
        if dirty_n(p) == NONE_13 {
            set_dirty_n(p, 1);
        } else {
            let v = dirty_n(p) + 1;
            set_dirty_n(p, v);
        }
        let hw = rd16(p, P_AUX1);
        if count == hw {
            let at = HEADER_LIST_START + 4 * count as usize;
            wr32(p, at, (page << 3) | 3);
            if dirty_first(p) == NONE_13 {
                set_dirty_first(p, count);
            }
            wr16(p, H_REUSE_COUNT, count + 1);
            wr16(p, P_AUX1, hw + 1);
        } else {
            wr16(p, H_REUSE_COUNT, count + 1);
            let hole = rd16(p, H_FIRST_HOLE);
            let at = HEADER_LIST_START + 4 * hole as usize;
            wr32(p, at, (page << 3) | 3);
            if dirty_first(p) == NONE_13 || hole < dirty_first(p) {
                set_dirty_first(p, hole);
            }
            if count + 1 == hw {
                wr16(p, H_FIRST_HOLE, NONE_13);
            } else {
                let mut i = hole as usize + 1;
                while i < hw as usize {
                    if rd32(p, HEADER_LIST_START + 4 * i) >> 3 == NO_PAGE {
                        wr16(p, H_FIRST_HOLE, i as u16);
                        break;
                    }
                    i += 1;
                }
            }
        }
        self.mark_dirty(h);
        true
    }

    /// `TE_PSmgrPageGetNew` (the free list is always empty in rekordbox files, so
    /// `TE_PSmgrGetEmptyPage` always ends here): a new data page becomes the insert page.
    fn get_empty_page(&mut self, t: usize) -> u32 {
        let n = self.dpgm_page_get_new(t);
        Self::page_init(self.mem_mut(n), 1);
        let h = self.header_page(t);
        if rd32(self.mem(h), H_FIRST_DATA) == NO_PAGE {
            wr32(self.mem_mut(h), H_FIRST_DATA, n);
            self.write_immediate(h);
        }
        self.sess[t].cur_page = n;
        n
    }

    /// `TE_PSmgrGetSpaceAtEnd`: append a slot. Returns (slot, heap offset).
    fn space_at_end(&mut self, n: u32, size: u32, need: u32) -> (u16, u16) {
        let p = self.mem_mut(n);
        let slot = slot_count(p);
        let v = live_count(p) + 1;
        set_counts(p, slot + 1, v);
        add_dirty_area(p, slot);
        let v = rd16(p, P_FREE) - need as u16;
        wr16(p, P_FREE, v);
        let used = rd16(p, P_USED);
        let (pm, dm, om) = footer(PAGE_SIZE, slot);
        let bit = 1u16 << (slot % 16);
        let v = rd16(p, dm) | bit;
        wr16(p, dm, v);
        let v = rd16(p, pm) | bit;
        wr16(p, pm, v);
        wr16(p, om, used);
        wr16(p, P_USED, used + size as u16);
        (slot as u16, used)
    }

    /// `TE_PSmgrGetSpaceMidPage`: first deleted slot (not deleted in this transaction) whose
    /// space up to the next slot holds `size` bytes.
    fn space_mid_page(&mut self, n: u32, size: u32) -> Option<(u16, u16)> {
        let p = self.mem_mut(n);
        let slots = slot_count(p);
        let mut base = 0u32;
        let mut g = 0usize;
        loop {
            let end = PAGE_SIZE - ROW_GROUP_BYTES * g;
            let present = rd16(p, end - 4);
            let dirty = rd16(p, end - 2);
            let mut last = false;
            if present | dirty != 0xffff {
                let n_in_group = if base + 16 < slots {
                    16
                } else {
                    let r = slots - base;
                    if r == 0 {
                        return None;
                    }
                    last = true;
                    r
                };
                for k in 0..n_in_group {
                    if (present | dirty) & (1 << k) != 0 {
                        continue;
                    }
                    let s = base + k;
                    let off = rd16(p, footer(PAGE_SIZE, s).2);
                    let next = if s == slots - 1 {
                        rd16(p, P_USED)
                    } else {
                        rd16(p, footer(PAGE_SIZE, s + 1).2)
                    };
                    let space = u32::from(next.wrapping_sub(off));
                    if size <= space {
                        wr16(p, end - 4, present | (1 << k));
                        wr16(p, end - 2, dirty | (1 << k));
                        let v = live_count(p) + 1;
                        set_counts(p, slots, v);
                        add_dirty_area(p, s);
                        return Some((s as u16, off));
                    }
                }
                if last {
                    return None;
                }
            } else if base + 16 >= slots {
                return None;
            }
            base += 16;
            g += 1;
        }
    }

    /// `TE_PSmgrGetReusedSpace`: try the pages of the reuse list (at most two).
    fn reused_space(&mut self, t: usize, size: u32) -> Option<(u32, u16, u16)> {
        self.handle_dirty_header(t);
        let h = self.header_page(t);
        let (count, hdirty) = {
            let p = self.mem(h);
            (rd16(p, H_REUSE_COUNT), dirty_n(p))
        };
        if count == 0 || count == hdirty {
            return None;
        }
        let mut tries = 0;
        loop {
            let hw = u32::from(rd16(self.mem(h), P_AUX1));
            let entry = loop {
                let cursor = self.sess[t].cursor;
                let e = rd32(self.mem(h), HEADER_LIST_START + 4 * cursor as usize);
                if e & 1 != 1 && e >> 3 != NO_PAGE {
                    break e;
                }
                self.sess[t].cursor = (cursor + 1) % hw;
            };
            let n = entry >> 3;
            self.touch(n);
            self.mark_dirty(n);
            let (slots, free) = {
                let p = self.mem(n);
                (slot_count(p), u32::from(rd16(p, P_FREE)))
            };
            let need = size + added_offset(slots);
            if need <= free && slots < 0x800 {
                let (slot, off) = self.space_at_end(n, size, need);
                self.stats.inserts_reused_end += 1;
                return Some((n, slot, off));
            }
            if let Some((slot, off)) = self.space_mid_page(n, size) {
                let p = self.mem(n);
                if live_count(p) == slot_count(p) {
                    let cursor = self.sess[t].cursor;
                    let hp = self.mem_mut(h);
                    add_dirty_area(hp, cursor);
                    let at = HEADER_LIST_START + 4 * cursor as usize;
                    wr32(hp, at, rd32(hp, at) | 1 | 4);
                    let dp = self.mem_mut(n);
                    let packed = rd32(dp, P_PACKED);
                    wr32(dp, P_PACKED, packed & !FLAG_REUSE);
                    self.sess[t].cursor = (cursor + 1) % hw;
                }
                self.sess[t].header_dirty = true;
                self.mark_dirty(h);
                self.stats.inserts_mid_page += 1;
                return Some((n, slot, off));
            }
            if tries == 1 {
                return None;
            }
            tries += 1;
            self.sess[t].cursor = (self.sess[t].cursor + 1) % hw;
        }
    }

    /// `TE_psmgr_request`: find room for a row of `size` bytes (unaligned) in table `t`.
    fn request(&mut self, t: usize, size: usize) -> Result<(u32, u16, u16)> {
        let size = size.next_multiple_of(4) as u32;
        if let Some(r) = self.reused_space(t, size) {
            return Ok(r);
        }
        let (n, fresh) = match self.sess[t].cur_page {
            NO_PAGE => (self.get_empty_page(t), true),
            n => (n, false),
        };
        self.touch(n);
        let (slots, free) = {
            let p = self.mem(n);
            (slot_count(p), u32::from(rd16(p, P_FREE)))
        };
        let need = size + added_offset(slots);
        if free < need || slots > 0x7ff {
            if size + added_offset(0) > (PAGE_SIZE - HEAP_START) as u32 {
                return Err(Error::BadPage {
                    page: n,
                    reason: format!("row of {size} bytes does not fit in a page"),
                });
            }
            let n = self.get_empty_page(t);
            self.mark_dirty(n);
            let (slot, off) = self.space_at_end(n, size, size + added_offset(0));
            self.stats.inserts_new_page += 1;
            return Ok((n, slot, off));
        }
        let (slot, off) = self.space_at_end(n, size, need);
        self.mark_dirty(n);
        if fresh {
            self.stats.inserts_new_page += 1;
        } else {
            self.stats.inserts_at_end += 1;
        }
        Ok((n, slot, off))
    }

    /// Encodes a row for table `t` (values in schema column order).
    pub fn encode(&self, t: u32, values: &[Value]) -> Result<EncodedRow> {
        let schema = self.kind.table(t).ok_or(Error::UnknownTable(t))?;
        encode_row(self.kind, schema, &self.layouts[t as usize], values)
    }

    /// INSERT of one row (`TE_TableInsert` → `TE_RowFormSmgr` → `TE_psmgr_request`).
    pub fn insert(&mut self, t: u32, values: &[Value]) -> Result<RowRef> {
        let row = self.encode(t, values)?;
        self.insert_encoded(t, row)
    }

    /// INSERT of an already encoded row.
    pub fn insert_encoded(&mut self, t: u32, mut row: EncodedRow) -> Result<RowRef> {
        assert!(self.in_txn, "insert outside a transaction");
        let ti = t as usize;
        if ti >= self.header.tables.len() {
            return Err(Error::UnknownTable(t));
        }
        let (page, slot, off) = self.request(ti, row.size)?;
        row.set_slot(slot);
        let start = HEAP_START + off as usize;
        let p = self.mem_mut(page);
        row.write_into(&mut p[start..]);
        Ok(RowRef { page, slot })
    }

    /// DELETE of one row (`TE_psmgr_area_free`). The row bytes stay; the slot loses its present
    /// bit and the page joins the table's reuse list.
    pub fn delete(&mut self, t: u32, r: RowRef) -> Result<()> {
        assert!(self.in_txn, "delete outside a transaction");
        let ti = t as usize;
        let n = r.page;
        if !self.pages.contains_key(&n) {
            return Err(Error::BadPage {
                page: n,
                reason: "delete on a missing page".into(),
            });
        }
        self.touch(n);
        self.mark_dirty(n);
        if rd32(self.mem(n), P_PACKED) & FLAG_REUSE == 0 {
            self.handle_dirty_header(ti);
            self.sess[ti].header_dirty = true;
            if self.add_to_reuse_cache(ti, n) {
                let p = self.mem_mut(n);
                let packed = rd32(p, P_PACKED);
                wr32(p, P_PACKED, packed | FLAG_REUSE);
            }
        }
        let p = self.mem_mut(n);
        let live = live_count(p);
        if live == 0 {
            return Err(Error::BadRow {
                page: n,
                slot: r.slot,
                reason: "delete on a page without live rows".into(),
            });
        }
        let v = live - 1;
        set_counts(p, slot_count(p), v);
        if rd32(p, P_PACKED) & FLAG_REUSE == 0 && live - 1 == 0 {
            // `TE_PSmgrAddToFreeList`: only reached when the reuse list is full.
            return Err(Error::BadPage {
                page: n,
                reason: "free-page list (reuse list full) is not modelled".into(),
            });
        }
        let (pm, dm, _) = footer(PAGE_SIZE, u32::from(r.slot));
        let bit = 1u16 << (r.slot % 16);
        if rd16(p, pm) & bit == 0 {
            return Err(Error::BadRow {
                page: n,
                slot: r.slot,
                reason: "delete of a slot that is not live".into(),
            });
        }
        if rd16(p, dm) & bit == 0 {
            let v = rd16(p, dm) | bit;
            wr16(p, dm, v);
            let v = rd16(p, pm) & !bit;
            wr16(p, pm, v);
            add_dirty_area(p, u32::from(r.slot));
        } else {
            let v = rd16(p, dm) & !bit;
            wr16(p, dm, v);
            let v = dirty_n(p) - 1;
            set_dirty_n(p, v);
            let v = rd16(p, pm) & !bit;
            wr16(p, pm, v);
        }
        self.stats.deletes += 1;
        Ok(())
    }

    /// UPDATE that changes a string column (or that rekordbox runs through the copying path):
    /// insert the new row, then delete the old one (`TE_RunUpdateScanSmgr`).
    pub fn update(&mut self, t: u32, old: RowRef, values: &[Value]) -> Result<RowRef> {
        let new = self.insert(t, values)?;
        self.delete(t, old)?;
        Ok(new)
    }

    /// UPDATE of fixed-size columns only, in place (`TE_RunInPlaceUpdateScanSmgr` @0x102f6f5cc):
    /// the row keeps its slot; the page is written with the new transaction id.
    pub fn update_in_place(&mut self, t: u32, r: RowRef, values: &[Value]) -> Result<()> {
        let mut row = self.encode(t, values)?;
        row.set_slot(r.slot);
        let n = r.page;
        self.touch(n);
        self.mark_dirty(n);
        let (_, _, om) = footer(PAGE_SIZE, u32::from(r.slot));
        let off = rd16(self.mem(n), om) as usize;
        let layout = &self.layouts[t as usize];
        let fixed: Vec<std::ops::Range<usize>> = row
            .written
            .iter()
            .filter(|w| w.end <= layout.fixed_end)
            .cloned()
            .collect();
        let p = self.mem_mut(n);
        for w in fixed {
            p[HEAP_START + off + w.start..HEAP_START + off + w.end]
                .copy_from_slice(&row.bytes[w.clone()]);
        }
        Ok(())
    }
}
