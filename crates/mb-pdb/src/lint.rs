//! `pdb-lint`: structural checks of an export.pdb / exportExt.pdb file. Spec: `docs/re/pdb.md`
//! § Writer requirements, § Verification.
//!
//! Errors are violations of rules the runtime itself checks (`TE_Dpgm_CheckFirstHeader`,
//! `TE_Dpgm_CheckPageHeader`, `TE_Dpgm_CheckPsmgrHeader`, `TE_Pgm_CheckFlatRow`) or invariants
//! that hold on every page rekordbox has written. Warnings are things rekordbox does not produce
//! but a reader would survive (non-canonical string forms, dangling references, gaps in
//! playlist positions).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::page::{
    FileHeader, HEADER_LIST_EMPTY, HEAP_START, NO_PAGE, NONE_13, PageHeader, PageKind, Pdb,
};
use crate::row::Row;
use crate::schema::DbKind;
use crate::string::DeviceString;
use crate::tables::read_rows;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// Page the finding is about (0 = file header), if any.
    pub page: Option<u32>,
    pub message: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match self.page {
            Some(p) => write!(f, "{sev}: page {p:#x}: {}", self.message),
            None => write!(f, "{sev}: {}", self.message),
        }
    }
}

/// Summary of a lint run.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub pages_checked: usize,
    pub rows_checked: usize,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings.len() - self.errors()
    }

    pub fn is_clean(&self) -> bool {
        self.errors() == 0
    }

    fn err(&mut self, page: impl Into<Option<u32>>, msg: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Error,
            page: page.into(),
            message: msg.into(),
        });
    }

    fn warn(&mut self, page: impl Into<Option<u32>>, msg: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Warning,
            page: page.into(),
            message: msg.into(),
        });
    }
}

/// The dirty counters (0x20/0x22) against the dirty bits. The count is exact; the first-dirty
/// field is a lower bound: `TE_psmgr_area_free` @0x102f673c4 deleting a slot inserted in the same
/// transaction decrements the count without moving the hint (the count may even reach 0), and
/// `TE_PSmgrClearDirtyBits` scans upwards from the hint.
fn dirty_counters_ok(count: u16, first: u16, dirty: &[u16]) -> bool {
    match dirty.first() {
        None => (count == NONE_13 && first == NONE_13) || (count == 0 && first != NONE_13),
        Some(&lo) => count as usize == dirty.len() && first <= lo,
    }
}

/// Guesses the file kind from the table count (20 = export.pdb, 9 = exportExt.pdb).
pub fn kind_of(bytes: &[u8]) -> Option<DbKind> {
    match FileHeader::parse(bytes).ok()?.table_count {
        20 => Some(DbKind::Export),
        9 => Some(DbKind::ExportExt),
        _ => None,
    }
}

/// Runs every check.
pub fn lint(bytes: &[u8], kind: DbKind) -> Report {
    let mut rep = Report::default();
    let pdb = match Pdb::parse(bytes.to_vec()) {
        Ok(p) => p,
        Err(e) => {
            rep.err(0, format!("file does not parse: {e}"));
            return rep;
        }
    };
    let h = pdb.header.clone();
    let ps = pdb.page_size();
    check_file_header(&pdb, kind, &mut rep);
    let file_xid = (h.xid_hi, h.xid_lo);

    // Chains.
    let mut owner: HashMap<u32, u32> = HashMap::new();
    let mut chains: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut max_page_xid = (0u32, 0u32);
    for e in &h.tables {
        let mut chain = Vec::new();
        let mut page = e.first_page;
        let mut ok = true;
        loop {
            if page >= pdb.page_count() {
                rep.err(
                    page,
                    format!("table {}: chain leaves the file", e.table_type),
                );
                ok = false;
                break;
            }
            if let Some(o) = owner.insert(page, e.table_type) {
                rep.err(
                    page,
                    format!("table {}: page already used by table {o}", e.table_type),
                );
                ok = false;
                break;
            }
            let ph = match pdb.page_header(page) {
                Ok(x) => x,
                Err(err) => {
                    rep.err(page, err.to_string());
                    ok = false;
                    break;
                }
            };
            if ph.page_index != page {
                rep.err(page, format!("page number field is {:#x}", ph.page_index));
            }
            if ph.table_type != e.table_type {
                rep.err(
                    page,
                    format!(
                        "table type {} in a chain of table {}",
                        ph.table_type, e.table_type
                    ),
                );
            }
            if ph.reserved0 != 0 {
                rep.err(page, "page header byte 0x00 is not 0");
            }
            if !(ph.next_page < h.next_unused_page || ph.next_page == NO_PAGE) {
                rep.err(
                    page,
                    format!("next page {:#x} ≥ next_unused_page", ph.next_page),
                );
            }
            if (ph.xid_hi, ph.xid_lo) > file_xid {
                rep.err(
                    page,
                    format!(
                        "transaction id {}:{} above the file's {}:{}",
                        ph.xid_hi, ph.xid_lo, file_xid.0, file_xid.1
                    ),
                );
            }
            max_page_xid = max_page_xid.max((ph.xid_hi, ph.xid_lo));
            chain.push(page);
            if page == e.last_page {
                if ph.next_page != e.empty_candidate {
                    rep.err(
                        page,
                        format!(
                            "table {}: last page links to {:#x}, not to the empty candidate {:#x}",
                            e.table_type, ph.next_page, e.empty_candidate
                        ),
                    );
                }
                break;
            }
            page = ph.next_page;
        }
        if ok {
            chains.insert(e.table_type, chain);
        }
    }
    if h.state == 5 && max_page_xid >= file_xid {
        rep.err(
            0,
            "file transaction id is not above every page's (a clean close increments it)",
        );
    }

    // Empty candidates and orphan pages.
    let mut candidates = BTreeSet::new();
    for e in &h.tables {
        if !candidates.insert(e.empty_candidate) {
            rep.err(
                e.empty_candidate,
                format!(
                    "table {}: empty candidate shared with another table",
                    e.table_type
                ),
            );
        }
        if owner.contains_key(&e.empty_candidate) {
            rep.err(
                e.empty_candidate,
                format!("table {}: empty candidate is part of a chain", e.table_type),
            );
        }
        if e.empty_candidate >= h.next_unused_page {
            rep.err(
                e.empty_candidate,
                format!("table {}: empty candidate ≥ next_unused_page", e.table_type),
            );
        }
        if e.empty_candidate < pdb.page_count()
            && pdb
                .page(e.empty_candidate)
                .map(|p| p.iter().any(|&b| b != 0))
                .unwrap_or(false)
        {
            rep.err(
                e.empty_candidate,
                format!(
                    "table {}: empty candidate inside the file is not zero",
                    e.table_type
                ),
            );
        }
    }
    for n in 1..pdb.page_count() {
        if owner.contains_key(&n) {
            continue;
        }
        if pdb
            .page(n)
            .map(|p| p.iter().any(|&b| b != 0))
            .unwrap_or(false)
        {
            rep.err(n, "page is in no chain but not zero");
        }
    }
    if let Some(&last) = owner.keys().max()
        && last + 1 != pdb.page_count()
    {
        rep.warn(
            None,
            format!(
                "file has {} pages; the highest chained page is {last:#x}",
                pdb.page_count()
            ),
        );
    }
    let max_used = owner
        .keys()
        .chain(candidates.iter())
        .copied()
        .max()
        .unwrap_or(0);
    if max_used >= h.next_unused_page {
        rep.err(0, format!("page {max_used:#x} ≥ next_unused_page"));
    }

    // Pages of each table.
    for e in &h.tables {
        let Some(chain) = chains.get(&e.table_type) else {
            continue;
        };
        let Some(schema) = kind.table(e.table_type) else {
            rep.err(
                None,
                format!("table type {} is not in the schema", e.table_type),
            );
            continue;
        };
        let layout = schema.layout();
        let mut flagged = BTreeSet::new();
        let mut data_pages = Vec::new();
        for (i, &n) in chain.iter().enumerate() {
            rep.pages_checked += 1;
            let page = pdb.page(n).expect("chained page in file");
            let ph = PageHeader::parse(page).expect("parsed above");
            if i == 0 {
                if ph.kind() != PageKind::TableHeader || ph.flags != 0x64 {
                    rep.err(
                        n,
                        format!("first page of a chain has flags {:#x}, not 0x64", ph.flags),
                    );
                }
                continue;
            }
            if ph.kind() != PageKind::Data || (ph.flags != 0x24 && ph.flags != 0x34) {
                rep.err(n, format!("data page with flags {:#x}", ph.flags));
                continue;
            }
            if ph.in_reuse_list() {
                flagged.insert(n);
            }
            data_pages.push(n);
            check_data_page(&pdb, n, kind, e.table_type, schema, &layout, &mut rep);
        }
        check_header_page(
            &pdb,
            e.first_page,
            chain,
            &data_pages,
            &flagged,
            e.empty_candidate,
            ps,
            &mut rep,
        );
    }

    check_contents(&pdb, kind, &mut rep);
    rep
}

fn check_file_header(pdb: &Pdb, kind: DbKind, rep: &mut Report) {
    let h = &pdb.header;
    if h.page_size != 4096 {
        rep.warn(
            0,
            format!("page size {} (rekordbox writes 4096)", h.page_size),
        );
    }
    if h.reserved0 != 0 {
        rep.err(0, "file header byte 0x00 is not 0");
    }
    let want = kind.tables().len() as u32;
    if h.table_count != want {
        rep.err(
            0,
            format!("{} tables, the schema has {want}", h.table_count),
        );
    }
    for (i, t) in h.tables.iter().enumerate() {
        if t.table_type != i as u32 {
            rep.err(
                0,
                format!("directory entry {i} has table type {}", t.table_type),
            );
        }
    }
    if !matches!(h.state, 0 | 1 | 5) {
        rep.err(
            0,
            format!(
                "page-manager state {} (open files are refused unless 0, 1 or 5)",
                h.state
            ),
        );
    } else if h.state != 5 {
        rep.warn(
            0,
            format!("page-manager state {} (rekordbox closes with 5)", h.state),
        );
    }
    if h.xid_hi != 0 {
        rep.warn(0, "transaction id high word is not 0");
    }
    if let Ok(p0) = pdb.page(0) {
        let end = FileHeader::SIZE_WITHOUT_TABLES + 16 * h.tables.len();
        if p0[end..].iter().any(|&b| b != 0) {
            rep.err(0, "bytes after the table directory are not zero");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_header_page(
    pdb: &Pdb,
    n: u32,
    chain: &[u32],
    data_pages: &[u32],
    flagged: &BTreeSet<u32>,
    candidate: u32,
    ps: usize,
    rep: &mut Report,
) {
    let Ok(hp) = pdb.header_page(&crate::page::TableDirEntry {
        table_type: 0,
        empty_candidate: candidate,
        first_page: n,
        last_page: n,
    }) else {
        rep.err(n, "header page does not parse");
        return;
    };
    let ph = hp.header;
    let cap = ((ps - HEAP_START - 0x28) / 4) as u16;
    if ph.slot_count != 0 || ph.live_count != 0 || ph.free_size != 0 || ph.used_size != 0 {
        rep.err(n, "header page has row counters");
    }
    if ph.aux0 != cap {
        rep.err(
            n,
            format!("reuse list capacity {} (expected {cap})", ph.aux0),
        );
    }
    if hp.self_page != n {
        rep.err(n, format!("header page names itself {:#x}", hp.self_page));
    }
    let first_data = data_pages.first().copied().unwrap_or(NO_PAGE);
    if hp.first_data_page != first_data {
        rep.err(
            n,
            format!(
                "first data page field {:#x}, chain has {first_data:#x}",
                hp.first_data_page
            ),
        );
    }
    let next_expected = chain.get(1).copied().unwrap_or(candidate);
    if ph.next_page != next_expected {
        rep.err(
            n,
            format!(
                "header page links to {:#x}, expected {next_expected:#x}",
                ph.next_page
            ),
        );
    }
    if hp.free_list_head != NO_PAGE || hp.free_list_tail != 0 {
        rep.warn(
            n,
            "free-page list is not empty (rekordbox never leaves one)",
        );
    }
    let hw = ph.aux1 as usize;
    if hw > hp.entries.len() {
        rep.err(
            n,
            format!("reuse list high-water mark {hw} above its capacity"),
        );
        return;
    }
    if hp.entries[hw..].iter().any(|&e| e != HEADER_LIST_EMPTY) {
        rep.err(n, "reuse list entries beyond the high-water mark");
    }
    let used: Vec<(usize, u32)> = hp.entries[..hw]
        .iter()
        .enumerate()
        .filter(|(_, e)| **e >> 3 != NO_PAGE)
        .map(|(i, e)| (i, *e))
        .collect();
    if hp.reuse_count as usize != used.len() {
        rep.err(
            n,
            format!("reuse count {} but {} entries", hp.reuse_count, used.len()),
        );
    }
    let listed: BTreeSet<u32> = used.iter().map(|(_, e)| e >> 3).collect();
    let pending_removal: BTreeSet<u32> = used
        .iter()
        .filter(|(_, e)| e & 4 != 0)
        .map(|(_, e)| e >> 3)
        .collect();
    for p in &listed {
        if !chain.contains(p) {
            rep.err(n, format!("reuse list names page {p:#x} outside the chain"));
        }
    }
    // A page leaves the list lazily: its entry is marked for removal (bit 2) when the page fills up.
    let live_listed: BTreeSet<u32> = listed
        .iter()
        .filter(|p| !pending_removal.contains(p) || flagged.contains(p))
        .copied()
        .collect();
    if &live_listed != flagged {
        rep.err(
            n,
            format!("reuse list {live_listed:x?} differs from the pages flagged 0x10 {flagged:x?}"),
        );
    }
    let dirty: Vec<u16> = used
        .iter()
        .filter(|(_, e)| e & 1 == 1)
        .map(|(i, _)| *i as u16)
        .collect();
    if !dirty_counters_ok(ph.dirty_count, ph.first_dirty_slot, &dirty) {
        rep.err(
            n,
            format!(
                "header dirty counters {:#x}/{:#x} do not fit the entries ({} dirty, first {:?})",
                ph.dirty_count,
                ph.first_dirty_slot,
                dirty.len(),
                dirty.first()
            ),
        );
    }
    let first_hole = hp.entries[..hw]
        .iter()
        .position(|&e| e >> 3 == NO_PAGE)
        .map_or(NONE_13, |i| i as u16);
    if hp.first_free_entry != first_hole {
        rep.warn(
            n,
            format!(
                "first-hole field {:#x}, first empty entry {first_hole:#x}",
                hp.first_free_entry
            ),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn check_data_page(
    pdb: &Pdb,
    n: u32,
    kind: DbKind,
    table_type: u32,
    schema: &'static crate::schema::TableSchema,
    layout: &crate::schema::RowLayout,
    rep: &mut Report,
) {
    let bytes = pdb.page(n).expect("page");
    let ps = bytes.len();
    let header = PageHeader::parse(bytes).expect("header");
    let page = crate::page::DataPage {
        number: n,
        header,
        bytes,
    };
    let limit = match ps {
        4096 => 510,
        8192 => 1022,
        16384 => 2046,
        _ => 2047,
    };
    if header.slot_count > limit || header.live_count > limit {
        rep.err(n, format!("more than {limit} slots"));
        return;
    }
    if header.aux0 != 0 || header.aux1 != 0 {
        rep.err(n, "data page has bytes 0x24..0x28 set");
    }
    if header.free_size as usize + header.used_size as usize + page.footer_len() != ps - HEAP_START
    {
        rep.err(
            n,
            format!(
                "free {} + used {} + footer {} ≠ {}",
                header.free_size,
                header.used_size,
                page.footer_len(),
                ps - HEAP_START
            ),
        );
    }
    let Ok(slots) = page.slots() else {
        rep.err(n, "slot footer does not parse");
        return;
    };
    let live = slots.iter().filter(|s| s.present).count();
    if live != header.live_count as usize {
        rep.err(
            n,
            format!("live count {} but {live} present bits", header.live_count),
        );
    }
    let dirty: Vec<u16> = slots.iter().filter(|s| s.dirty).map(|s| s.index).collect();
    if !dirty_counters_ok(header.dirty_count, header.first_dirty_slot, &dirty) {
        rep.err(
            n,
            format!(
                "dirty counters {:#x}/{:#x} do not fit the dirty masks ({} bits, first {:?})",
                header.dirty_count,
                header.first_dirty_slot,
                dirty.len(),
                dirty.first()
            ),
        );
    }
    let groups = page.group_count();
    if groups > 0 {
        let rem = header.slot_count as usize - 16 * (groups - 1);
        if rem < 16 {
            let (p, d) = page.group_masks(groups - 1).expect("masks");
            if (p | d) >> rem != 0 {
                rep.err(n, "mask bits set beyond the slot count");
            }
        }
    }
    let deleted = slots.iter().any(|s| !s.present);
    if deleted && !header.in_reuse_list() {
        rep.warn(
            n,
            "page has deleted slots but is not in the reuse list (flag 0x10)",
        );
    }
    if header.in_reuse_list() && !deleted {
        rep.err(n, "page flagged 0x10 without deleted slots");
    }
    let word_base = match kind {
        DbKind::Export => 1u16,
        DbKind::ExportExt => 49,
    };
    for (i, slot) in slots.iter().enumerate() {
        rep.rows_checked += 1;
        if slot.offset % 4 != 0 {
            rep.err(
                n,
                format!(
                    "slot {}: offset {:#x} not 4-aligned",
                    slot.index, slot.offset
                ),
            );
        }
        let next = slots.get(i + 1).map_or(header.used_size, |s| s.offset);
        if next < slot.offset {
            rep.err(n, format!("slot {}: offsets not increasing", slot.index));
            continue;
        }
        if slot.offset >= header.used_size {
            rep.err(
                n,
                format!("slot {}: offset beyond the used heap", slot.index),
            );
            continue;
        }
        let row_bytes = match page.row_bytes(slot) {
            Ok(b) => b,
            Err(e) => {
                rep.err(n, e.to_string());
                continue;
            }
        };
        let row = match Row::decode(schema, layout, n, *slot, row_bytes) {
            Ok(r) => r,
            Err(e) => {
                rep.err(n, format!("slot {}: {e}", slot.index));
                continue;
            }
        };
        if row.end > (next - slot.offset) as usize {
            rep.err(
                n,
                format!(
                    "slot {}: row ends at {} past its allocation of {}",
                    slot.index,
                    row.end,
                    next - slot.offset
                ),
            );
        }
        if let Some(rh) = row.header {
            if rh.slot() != slot.index {
                rep.err(
                    n,
                    format!("slot {}: row header names slot {}", slot.index, rh.slot()),
                );
            }
            if rh.table_word() != word_base + table_type as u16 {
                rep.err(
                    n,
                    format!(
                        "slot {}: row header table word {}",
                        slot.index,
                        rh.table_word()
                    ),
                );
            }
            if rh.0 & 3 != 0 || rh.deleted() {
                rep.err(
                    n,
                    format!(
                        "slot {}: row header flag bits {:#x}",
                        slot.index,
                        rh.0 & 0x1f
                    ),
                );
            }
        }
        for (ci, off) in row.string_offsets.iter().enumerate() {
            let (Some(off), Some(s)) = (off, row.values[ci].as_str()) else {
                continue;
            };
            if !s.is_short() && off % 4 != 0 {
                rep.err(
                    n,
                    format!(
                        "slot {}: long string {} not 4-aligned",
                        slot.index, schema.columns[ci].name
                    ),
                );
            }
            if slot.present && s.isrc().is_none() && DeviceString::from_text(&s.text()) != *s {
                rep.warn(
                    n,
                    format!(
                        "slot {}: {} is not in rekordbox's string form",
                        slot.index, schema.columns[ci].name
                    ),
                );
            }
        }
    }
}

/// Content checks across tables (warnings).
fn check_contents(pdb: &Pdb, kind: DbKind, rep: &mut Report) {
    let mut ids: HashMap<u32, BTreeSet<u32>> = HashMap::new();
    for t in kind.tables() {
        let Ok(rows) = read_rows(pdb, kind, t.type_id) else {
            continue;
        };
        if t.column_index("ID").is_some() {
            let mut set = BTreeSet::new();
            for r in &rows {
                if !set.insert(r.uint("ID")) {
                    rep.warn(
                        None,
                        format!("{}: ID {} appears twice", t.name, r.uint("ID")),
                    );
                }
            }
            ids.insert(t.type_id, set);
        }
    }
    if kind != DbKind::Export {
        return;
    }
    let has = |t: u32, id: u32| id == 0 || ids.get(&t).is_some_and(|s| s.contains(&id));
    if let Ok(tracks) = read_rows(pdb, kind, 0) {
        for r in &tracks {
            for (col, t) in [
                ("ARTISTID", 2),
                ("REMIXERID", 2),
                ("ORGARTISTID", 2),
                ("COMPOSERID", 2),
                ("ALBUMID", 3),
                ("GENREID", 1),
                ("LABELID", 4),
                ("KEYID", 5),
                ("IMAGEID", 13),
            ] {
                if !has(t, r.uint(col)) {
                    rep.warn(
                        None,
                        format!(
                            "track {}: {col} {} does not exist",
                            r.uint("ID"),
                            r.uint(col)
                        ),
                    );
                }
            }
        }
    }
    let mut lists: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if let Ok(entries) = read_rows(pdb, kind, 8) {
        for r in &entries {
            if !has(0, r.uint("CONTENTID")) || !has(7, r.uint("PLAYLISTID")) {
                rep.warn(
                    None,
                    format!(
                        "playlist entry {}/{} is dangling",
                        r.uint("PLAYLISTID"),
                        r.uint("CONTENTID")
                    ),
                );
            }
            lists
                .entry(r.uint("PLAYLISTID"))
                .or_default()
                .push(r.uint("TRACKNO"));
        }
    }
    for (pl, mut pos) in lists {
        pos.sort_unstable();
        if pos.iter().enumerate().any(|(i, &p)| p != i as u32 + 1) {
            rep.warn(None, format!("playlist {pl}: TRACKNO values are not 1..n"));
        }
    }
    if let Ok(nodes) = read_rows(pdb, kind, 7) {
        let mut seqs: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for r in &nodes {
            if !has(7, r.uint("PARENTID")) {
                rep.warn(
                    None,
                    format!(
                        "playlist {}: parent {} does not exist",
                        r.uint("ID"),
                        r.uint("PARENTID")
                    ),
                );
            }
            seqs.entry(r.uint("PARENTID"))
                .or_default()
                .push(r.uint("SEQ"));
        }
        for (parent, mut s) in seqs {
            s.sort_unstable();
            let ok = s.iter().enumerate().all(|(i, &v)| v == i as u32)
                || s.iter().enumerate().all(|(i, &v)| v == i as u32 + 1);
            if !ok {
                rep.warn(
                    None,
                    format!("playlists under {parent}: SEQ values are not consecutive"),
                );
            }
        }
    }
    let mut hist: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if let Ok(entries) = read_rows(pdb, kind, 12) {
        for r in &entries {
            hist.entry(r.uint("HISTORYID"))
                .or_default()
                .push(r.uint("SEQ"));
        }
    }
    for (h, mut pos) in hist {
        pos.sort_unstable();
        if pos.iter().enumerate().any(|(i, &p)| p != i as u32 + 1) {
            rep.warn(None, format!("history {h}: SEQ values are not 1..n"));
        }
    }
}
