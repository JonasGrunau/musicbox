//! Device cue tags built from master.db `djmdCue` rows, as rekordbox's export does
//! (`docs/re/export.md` § 1.6 "Cues"). rekordbox never writes collection cues into the local
//! ANLZ files; the device PCOB/PCO2 tags come from the database at export time:
//! `getContentCueInfo` → `checkCueInfos` @0x101e5610c → `getExportCue` @0x1009bc884 →
//! `convert_cue_to_extPointdata` @0x1009bd4b8 → `ExportAnalysisData` @0x1007b7c0c (.DAT) and
//! `ExportAnalysisExtendWithCue` @0x1007b83fc (.EXT).

use mb_anlz::tags::{CueEntry, CueEntryExt, CueKind, CueList, CueListExt, CueType, NONE_U16, NONE_U32};
use mb_anlz::{AnlzFile, Tag};

/// One `djmdCue` row as `AppSyncDBController::getContentCueInfo` reads it (NULL columns read as
/// 0 or "", as `juce::var` casts them).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceCue {
    pub kind: i64,
    pub in_msec: i64,
    pub in_mpeg_frame: i64,
    pub in_mpeg_abs: i64,
    /// −1 = no out point.
    pub out_msec: i64,
    pub out_mpeg_frame: i64,
    pub out_mpeg_abs: i64,
    pub color: i64,
    pub color_table_index: i64,
    pub active_loop: i64,
    pub comment: String,
    pub beat_loop_size: i64,
    pub cue_microsec: i64,
    pub in_point_seek_info: String,
    pub out_point_seek_info: String,
}

/// The player-side cue (`analyzer::Cue`) built by `AppSyncDBController::set_cueInfo` @0x100a4c304.
#[derive(Debug, Clone)]
struct PlayerCue {
    kind: i32,
    in_ms: u32,
    /// 0 = no out point.
    out_ms: u32,
    in_mpeg_frame: u32,
    in_mpeg_abs: u32,
    out_mpeg_frame: u32,
    out_mpeg_abs: u32,
    color: u8,
    color_table_index: u8,
    comment: String,
    /// (numerator, denominator); both 0 unless both are set.
    beat_loop: (u16, u16),
    /// Sub-ms µs of the in and out point.
    micro: (u16, u16),
    /// FLAC seek points (sample, offset, frame samples) of the in and out point.
    seek_in: Option<(u64, u64, u32)>,
    seek_out: Option<(u64, u64, u32)>,
}

fn seek_points(text: &str) -> Option<(u64, u64, u32)> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 3 {
        return None;
    }
    let n = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
    Some((n(parts[0]) as u64, n(parts[1]) as u64, n(parts[2]) as u32))
}

/// `db::dsql_conv_cue_kind(char, bool)` @0x1019aa930: 0 becomes −1 for a loop; 1–17 and 101
/// stay; anything else is −2 (dropped).
fn conv_kind(kind: i64, has_out: bool) -> i32 {
    match kind as u8 {
        0 => {
            if has_out {
                -1
            } else {
                0
            }
        }
        k @ 1..=17 => i32::from(k),
        101 => 101,
        _ => -2,
    }
}

fn player_cue(row: &SourceCue) -> PlayerCue {
    let out_raw = row.out_msec as u32;
    let mut kind = conv_kind(row.kind, out_raw != NONE_U32);
    let (out_ms, out_frame_fields) = if out_raw == NONE_U32 {
        (0, (0, 0))
    } else {
        (out_raw, (row.out_mpeg_frame as u32, row.out_mpeg_abs as u32))
    };
    let active = row.active_loop & 1 != 0 && out_ms != 0;
    if active && (kind == 4 || (kind - 1) as u32 > 16) {
        kind = 4;
    }
    let bl = row.beat_loop_size as u32;
    let beat_loop = if bl > 0xffff && bl & 0xffff != 0 { ((bl >> 16) as u16, bl as u16) } else { (0, 0) };
    let us = row.cue_microsec as u32;
    let in_ms = row.in_msec as u32;
    PlayerCue {
        kind,
        in_ms,
        out_ms,
        in_mpeg_frame: row.in_mpeg_frame as u32,
        in_mpeg_abs: row.in_mpeg_abs as u32,
        out_mpeg_frame: out_frame_fields.0,
        out_mpeg_abs: out_frame_fields.1,
        color: row.color as u8,
        color_table_index: row.color_table_index as u8,
        comment: row.comment.clone(),
        beat_loop,
        micro: (us as u16, (us >> 16) as u16),
        seek_in: seek_points(&row.in_point_seek_info),
        seek_out: if in_ms < out_raw { seek_points(&row.out_point_seek_info) } else { None },
    }
}

/// A collection cue as rekordbox's cue loader hands it to the browser and to the rekordbox.xml
/// writer (`AppSyncDBController::set_cueInfo` @0x100a4c304 per row, then `checkCueInfos`
/// @0x101e5610c), in `djmdCue` row order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCue {
    /// Player-side kind: −1 memory loop, 0 memory cue, 4 active memory loop, 1–3 / 5–17 hot cues.
    pub kind: i32,
    pub in_ms: u32,
    /// 0 = no out point (`OutMsec` −1).
    pub out_ms: u32,
    /// `ColorTableIndex` (hot-cue colour code 1–64; 0 = default).
    pub color_table_index: u8,
    pub comment: String,
}

/// The cues of one track after `set_cueInfo` and `checkCueInfos`.
pub fn checked_cues(rows: &[SourceCue]) -> Vec<CheckedCue> {
    check_cue_infos(rows.iter().map(player_cue).collect())
        .into_iter()
        .map(|c| CheckedCue {
            kind: c.kind,
            in_ms: c.in_ms,
            out_ms: c.out_ms,
            color_table_index: c.color_table_index,
            comment: c.comment,
        })
        .collect()
}

/// `CAnalyzerIF::getHotCueDefaultColor(number, is_loop, colour code, colour type, device)`
/// @0x10162e784 for devices: the code's device pad colour when it is 1–64, else the default
/// code's ([`default_hot_cue_code`]). The UI and rekordbox.xml use [`hot_cue_ui_rgb`].
pub fn hot_cue_rgb(number: u16, is_loop: bool, color_table_index: u8, color_type: u8) -> [u8; 3] {
    let code = if (1..=64).contains(&color_table_index) {
        color_table_index
    } else {
        default_hot_cue_code(number, is_loop, color_type)
    };
    PAD_COLORS[usize::from(code)]
}

fn is_hot(kind: i32) -> bool {
    (1..=3).contains(&kind) || (5..=17).contains(&kind)
}

fn is_memory(kind: i32) -> bool {
    matches!(kind, -1 | 0 | 4)
}

/// `checkCueInfos` @0x101e5610c: duplicate hot cues (the later row wins), duplicate memory cues
/// with the same in and out point (the earlier row wins), at most 10 memory cues (the first ten),
/// and rows of an unknown kind are dropped.
fn check_cue_infos(mut cues: Vec<PlayerCue>) -> Vec<PlayerCue> {
    // checkHotCueDuplicate @0x101e5658c: walks from the end, keeps the first kind it sees.
    let mut seen = Vec::new();
    let mut keep = vec![true; cues.len()];
    for i in (0..cues.len()).rev() {
        if is_hot(cues[i].kind) {
            if seen.contains(&cues[i].kind) {
                keep[i] = false;
            } else {
                seen.push(cues[i].kind);
            }
        }
    }
    // checkMemoryCueDuplicate @0x101e567b8.
    for i in 0..cues.len() {
        if !keep[i] || !is_memory(cues[i].kind) {
            continue;
        }
        for j in i + 1..cues.len() {
            if keep[j] && is_memory(cues[j].kind) && cues[j].in_ms == cues[i].in_ms && cues[j].out_ms == cues[i].out_ms {
                keep[j] = false;
            }
        }
    }
    let mut memory = 0;
    for i in 0..cues.len() {
        if keep[i] && is_memory(cues[i].kind) {
            memory += 1;
            if memory > 10 {
                keep[i] = false;
            }
        }
    }
    let mut keep = keep.into_iter();
    cues.retain(|c| keep.next().unwrap_or(false) && c.kind != -2);
    cues
}

/// `djplay::getPadColor` @0x10027e378: the RGB rekordbox shows for the 64 colour codes in its own
/// UI and writes into rekordbox.xml (`getHotCueDefaultColor` with the device flag off, as
/// `addPositionMarkNode` @0x1010574b0 calls it), filled by the same `PadColour.cpp` initialiser.
/// Verified on rekordbox's XML export: code 8 → 69,172,219; code 23 → 125,193,61.
pub const UI_PAD_COLORS: [[u8; 3]; 65] = [
    [0x00, 0x00, 0x00], [0x30, 0x5a, 0xff], [0x50, 0x73, 0xff], [0x50, 0x8c, 0xff],
    [0x50, 0xa0, 0xff], [0x50, 0xb4, 0xff], [0x50, 0xb0, 0xf2], [0x50, 0xae, 0xe8],
    [0x45, 0xac, 0xdb], [0x00, 0xe0, 0xff], [0x19, 0xda, 0xf0], [0x32, 0xd2, 0xe6],
    [0x21, 0xb4, 0xb9], [0x20, 0xaa, 0xa0], [0x1f, 0xa3, 0x92], [0x19, 0xa0, 0x8c],
    [0x14, 0xa5, 0x84], [0x14, 0xaa, 0x7d], [0x10, 0xb1, 0x76], [0x30, 0xd2, 0x6e],
    [0x37, 0xde, 0x5a], [0x3c, 0xeb, 0x50], [0x28, 0xe2, 0x14], [0x7d, 0xc1, 0x3d],
    [0x8c, 0xc8, 0x32], [0x9b, 0xd7, 0x23], [0xa5, 0xe1, 0x16], [0xa5, 0xdc, 0x0a],
    [0xaa, 0xd2, 0x08], [0xb4, 0xc8, 0x05], [0xb4, 0xbe, 0x04], [0xba, 0xb4, 0x04],
    [0xc3, 0xaf, 0x04], [0xe1, 0xaa, 0x00], [0xff, 0xa0, 0x00], [0xff, 0x96, 0x00],
    [0xff, 0x8c, 0x00], [0xff, 0x75, 0x00], [0xe0, 0x64, 0x1b], [0xe0, 0x46, 0x1e],
    [0xe0, 0x30, 0x1e], [0xe0, 0x28, 0x23], [0xe6, 0x28, 0x28], [0xff, 0x37, 0x6f],
    [0xff, 0x2d, 0x6f], [0xff, 0x12, 0x7b], [0xf5, 0x1e, 0x8c], [0xeb, 0x2d, 0xa0],
    [0xe6, 0x37, 0xb4], [0xde, 0x44, 0xcf], [0xde, 0x44, 0x8d], [0xe6, 0x30, 0xb4],
    [0xe6, 0x19, 0xdc], [0xe6, 0x00, 0xff], [0xdc, 0x00, 0xff], [0xcc, 0x00, 0xff],
    [0xb4, 0x32, 0xff], [0xb9, 0x3c, 0xff], [0xc5, 0x42, 0xff], [0xaa, 0x5a, 0xff],
    [0xaa, 0x72, 0xff], [0x82, 0x72, 0xff], [0x64, 0x73, 0xff], [0x00, 0x00, 0x00],
    [0xff, 0xff, 0xff],
];

/// [`hot_cue_rgb`] with [`UI_PAD_COLORS`]: the colour of a hot cue in rekordbox's UI and XML.
pub fn hot_cue_ui_rgb(number: u16, is_loop: bool, color_table_index: u8, color_type: u8) -> [u8; 3] {
    let code = if (1..=64).contains(&color_table_index) {
        color_table_index
    } else {
        default_hot_cue_code(number, is_loop, color_type)
    };
    UI_PAD_COLORS[usize::from(code)]
}

/// `djplay::getDevicePadColor` @0x10027e398: the 64 hot-cue colour codes (RGB), filled by the
/// `PadColour.cpp` initialiser @0x10027ecc8. Index 0 (and anything outside 1–64) is black.
pub const PAD_COLORS: [[u8; 3]; 65] = [
    [0x00, 0x00, 0x00],
    [0x00, 0x00, 0xff],
    [0x00, 0x1c, 0xff],
    [0x00, 0x38, 0xff],
    [0x00, 0x54, 0xff],
    [0x00, 0x70, 0xff],
    [0x00, 0x8c, 0xff],
    [0x00, 0xa8, 0xff],
    [0x00, 0xc4, 0xff],
    [0x00, 0xe0, 0xff],
    [0x00, 0xff, 0xff],
    [0x00, 0xff, 0xe8],
    [0x00, 0xff, 0xd1],
    [0x00, 0xff, 0xba],
    [0x00, 0xff, 0xa3],
    [0x00, 0xff, 0x8c],
    [0x00, 0xff, 0x75],
    [0x00, 0xff, 0x5e],
    [0x00, 0xff, 0x47],
    [0x00, 0xff, 0x30],
    [0x00, 0xff, 0x1a],
    [0x00, 0xff, 0x00],
    [0x1a, 0xff, 0x00],
    [0x33, 0xff, 0x00],
    [0x4d, 0xff, 0x00],
    [0x66, 0xff, 0x00],
    [0x80, 0xff, 0x00],
    [0x99, 0xff, 0x00],
    [0xb3, 0xff, 0x00],
    [0xcc, 0xff, 0x00],
    [0xe6, 0xff, 0x00],
    [0xff, 0xff, 0x00],
    [0xff, 0xe8, 0x00],
    [0xff, 0xd1, 0x00],
    [0xff, 0xba, 0x00],
    [0xff, 0xa3, 0x00],
    [0xff, 0x8c, 0x00],
    [0xff, 0x75, 0x00],
    [0xff, 0x5e, 0x00],
    [0xff, 0x47, 0x00],
    [0xff, 0x30, 0x00],
    [0xff, 0x1a, 0x00],
    [0xff, 0x00, 0x00],
    [0xff, 0x00, 0x17],
    [0xff, 0x00, 0x2e],
    [0xff, 0x00, 0x45],
    [0xff, 0x00, 0x5c],
    [0xff, 0x00, 0x73],
    [0xff, 0x00, 0x8a],
    [0xff, 0x00, 0xa1],
    [0xff, 0x00, 0xb8],
    [0xff, 0x00, 0xcf],
    [0xff, 0x00, 0xe6],
    [0xff, 0x00, 0xff],
    [0xe6, 0x00, 0xff],
    [0xcc, 0x00, 0xff],
    [0xb3, 0x00, 0xff],
    [0x99, 0x00, 0xff],
    [0x80, 0x00, 0xff],
    [0x66, 0x00, 0xff],
    [0x4d, 0x00, 0xff],
    [0x33, 0x00, 0xff],
    [0x1a, 0x00, 0xff],
    [0x00, 0x00, 0x00],
    [0xff, 0xff, 0xff],
];

/// Default colour code per hot-cue number (1-based; index 0 unused) for the preference
/// `HotCueColorType` 2, 3 and 4 (`analyzer::hcTypeCold1` @0x1037e58a4, `hcTypeCold2`
/// @0x1037e58b5, `hcTypeColorFull` @0x1037e58c6).
const HC_TYPE_COLD1: [u8; 17] = [0x24, 0x04, 0x12, 0x3a, 0x08, 0x0c, 0x3c, 0x0e, 0x01, 0x04, 0x12, 0x3a, 0x08, 0x0c, 0x3c, 0x0e, 0x01];
const HC_TYPE_COLD2: [u8; 17] = [0x24, 0x0e, 0x0b, 0x08, 0x05, 0x03, 0x01, 0x3d, 0x3b, 0x0e, 0x0b, 0x08, 0x05, 0x03, 0x01, 0x3d, 0x3b];
const HC_TYPE_COLOR_FULL: [u8; 17] = [0x24, 0x2b, 0x08, 0x17, 0x3c, 0x13, 0x26, 0x01, 0x20, 0x2b, 0x08, 0x17, 0x3c, 0x13, 0x26, 0x01, 0x20];

/// `CAnalyzerIF::getHotCueDefaultColor` @0x10162e784 for a cue without a colour code: 0x24 for
/// loops, else the preference's table, else 0x16 (0x1a above pad 8).
pub fn default_hot_cue_code(number: u16, is_loop: bool, color_type: u8) -> u8 {
    let n = usize::from(number).min(16);
    if is_loop {
        0x24
    } else {
        match color_type {
            4 => HC_TYPE_COLOR_FULL[n],
            3 => HC_TYPE_COLD2[n],
            2 => HC_TYPE_COLD1[n],
            _ if number > 8 => 0x1a,
            _ => 0x16,
        }
    }
}

/// The six device cue lists of one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCues {
    /// `.DAT` PCOB hot: hot cues A–C.
    pub dat_hot: CueList,
    /// `.DAT` PCOB memory: memory cues and loops, linked by `prev`/`next`.
    pub dat_memory: CueList,
    /// `.EXT` PCOB hot: hot cues D onwards.
    pub ext_hot: CueList,
    /// `.EXT` PCOB memory: always empty.
    pub ext_memory: CueList,
    /// `.EXT` PCO2 hot: every hot cue.
    pub ext2_hot: CueListExt,
    /// `.EXT` PCO2 memory.
    pub ext2_memory: CueListExt,
}

impl DeviceCues {
    pub fn is_empty(&self) -> bool {
        self.ext2_hot.entries.is_empty() && self.ext2_memory.entries.is_empty()
    }
}

/// The hot-cue number of the PCPT/PCP2 entry (`getExportCue`): A–C keep their kind, kind 5
/// (D) is 4, kinds 6–17 are kind − 1; memory cues 0.
fn hot_number(kind: i32) -> u16 {
    match kind {
        1..=3 => kind as u16,
        5 => 4,
        6..=17 => (kind - 1) as u16,
        _ => 0,
    }
}

fn pcpt(c: &PlayerCue) -> CueEntry {
    CueEntry {
        unk_0c: 0,
        hot_cue: hot_number(c.kind),
        unk_10: if c.kind == 4 { 4 } else { 0 },
        status: 1,
        unk_16: 0,
        prev: NONE_U16,
        next: NONE_U16,
        cue_type: if c.out_ms == 0 { CueType::Point } else { CueType::Loop },
        unk_1d: 0,
        time_unit: 1000,
        time: c.in_ms,
        loop_end: if c.out_ms == 0 { NONE_U32 } else { c.out_ms },
        unk_28: [c.in_mpeg_frame, c.out_mpeg_frame, c.in_mpeg_abs, c.out_mpeg_abs],
    }
}

/// `convert_cue_to_extPointdata` @0x1009bd4b8 + the writer's layout: 0x2c fixed bytes, the
/// comment, then 44 bytes (colour code, R, G, B, the FLAC seek points).
fn pcp2(c: &PlayerCue, entry: &CueEntry, color_type: u8) -> CueEntryExt {
    let mut ext = vec![0u8; 44];
    if is_hot(c.kind) {
        let (code, rgb) = if (1..=64).contains(&c.color_table_index) {
            (c.color_table_index, PAD_COLORS[usize::from(c.color_table_index)])
        } else {
            // hypothesis: the colour object behind `getHotCueDefaultColor` (vtable +0x170) is the
            // device pad palette; every default-coloured entry on the reference stick matches it.
            let default = default_hot_cue_code(entry.hot_cue, c.in_ms < c.out_ms, color_type);
            (0, PAD_COLORS[usize::from(default)])
        };
        ext[0] = code;
        ext[1..4].copy_from_slice(&rgb);
    }
    if let Some((a, b, n)) = c.seek_in {
        ext[4..12].copy_from_slice(&a.to_be_bytes());
        ext[20..28].copy_from_slice(&b.to_be_bytes());
        ext[36..40].copy_from_slice(&n.to_be_bytes());
        if let Some((a, b, n)) = c.seek_out {
            ext[12..20].copy_from_slice(&a.to_be_bytes());
            ext[28..36].copy_from_slice(&b.to_be_bytes());
            ext[40..44].copy_from_slice(&n.to_be_bytes());
        }
    }
    // The comment goes out when its UTF-16 size with the terminator is 3–0xffff bytes.
    let units = c.comment.encode_utf16().count();
    let comment = (!c.comment.is_empty() && (units + 1) * 2 <= 0xffff).then(|| c.comment.clone());
    CueEntryExt {
        unk_0c: 0,
        hot_cue: entry.hot_cue,
        cue_type: entry.cue_type,
        unk_11: 0,
        time_unit: 1000,
        time: entry.time,
        loop_end: entry.loop_end,
        color_index: if c.color < 8 { c.color + 1 } else { 0 },
        unk_1d: 1,
        in_us: if c.micro.0 < 1000 { c.micro.0 } else { 0 },
        out_us: if c.micro.1 < 1000 { c.micro.1 } else { 0 },
        unk_22: 0,
        loop_numerator: c.beat_loop.0,
        loop_denominator: c.beat_loop.1,
        unk_28: 0,
        comment,
        ext,
    }
}

/// The device cue lists for the cue rows of one track (row order = `djmdCue` row order).
/// `color_type` is rekordbox's preference `HotCueColorType` (default colours of hot cues
/// without a colour code).
pub fn device_cues(rows: &[SourceCue], color_type: u8) -> DeviceCues {
    let cues = check_cue_infos(rows.iter().map(player_cue).collect());
    let mut out = DeviceCues {
        dat_hot: CueList::empty(CueKind::Hot),
        dat_memory: CueList::empty(CueKind::Memory),
        ext_hot: CueList::empty(CueKind::Hot),
        ext_memory: CueList::empty(CueKind::Memory),
        ext2_hot: CueListExt::empty(CueKind::Hot),
        ext2_memory: CueListExt::empty(CueKind::Memory),
    };
    // `getExportCue` walks the list from the last cue to the first.
    for c in cues.iter().rev() {
        let entry = pcpt(c);
        let ext = pcp2(c, &entry, color_type);
        match c.kind {
            1..=3 => {
                out.dat_hot.entries.push(entry);
                out.ext2_hot.entries.push(ext);
            }
            5..=17 => {
                out.ext_hot.entries.push(entry);
                out.ext2_hot.entries.push(ext);
            }
            -1 | 0 | 4 => {
                out.dat_memory.entries.push(entry);
                out.ext2_memory.entries.push(ext);
            }
            _ => {}
        }
    }
    // The memory list is doubly linked in array order; the header holds first and last index.
    let n = out.dat_memory.entries.len();
    for (i, e) in out.dat_memory.entries.iter_mut().enumerate() {
        e.prev = if i == 0 { NONE_U16 } else { (i - 1) as u16 };
        e.next = if i + 1 == n { NONE_U16 } else { (i + 1) as u16 };
    }
    if n > 0 {
        out.dat_memory.unk_14 = 0;
        out.dat_memory.unk_16 = (n - 1) as u16;
    }
    out
}

/// `is_need_exporting_cue_ext` @0x1009b8b28: some cue (other than kind 101) has a colour, a
/// comment, a beat-loop size, a colour code or a sub-ms part.
pub fn has_extended_data(rows: &[SourceCue]) -> bool {
    rows.iter().map(player_cue).any(|c| {
        c.kind != 101
            && ((c.color as i8) >= 0
                || !c.comment.is_empty()
                || (c.beat_loop.0 != 0 && c.beat_loop.1 != 0)
                || c.color_table_index != 0
                || c.micro.0 != 0
                || c.micro.1 != 0)
    })
}

/// Replaces the first section with `tag` whose list kind matches, or inserts it after the last
/// section of `after` that exists.
fn put(file: &mut AnlzFile, tag: Tag, kind: CueKind, section: mb_anlz::Section, after: &[Tag]) {
    let kind_of = |s: &mb_anlz::Section| s.header.get(2..4).map(|b| u16::from_be_bytes([b[0], b[1]]));
    if let Some(i) = file.sections.iter().position(|s| s.tag == tag && kind_of(s) == Some(kind.raw())) {
        file.sections[i] = section;
        return;
    }
    let at = file.sections.iter().rposition(|s| after.contains(&s.tag)).map_or(file.sections.len(), |i| i + 1);
    file.sections.insert(at, section);
}

/// Puts the cue lists into a USB `.DAT` (hot, then memory, after PWV2).
pub fn apply_dat(file: &mut AnlzFile, cues: &DeviceCues) {
    put(file, Tag::PCOB, CueKind::Hot, cues.dat_hot.encode(), &[Tag::PPTH, Tag::PVBR, Tag::PQTZ, Tag::PWAV, Tag::PWV2]);
    put(file, Tag::PCOB, CueKind::Memory, cues.dat_memory.encode(), &[Tag::PCOB]);
}

/// Puts the cue lists into a USB `.EXT` (PCOB hot, PCOB memory, PCO2 hot, PCO2 memory, after
/// PWV3).
pub fn apply_ext(file: &mut AnlzFile, cues: &DeviceCues) {
    put(file, Tag::PCOB, CueKind::Hot, cues.ext_hot.encode(), &[Tag::PPTH, Tag::PWV3]);
    put(file, Tag::PCOB, CueKind::Memory, cues.ext_memory.encode(), &[Tag::PCOB]);
    put(file, Tag::PCO2, CueKind::Hot, cues.ext2_hot.encode(), &[Tag::PCOB]);
    put(file, Tag::PCO2, CueKind::Memory, cues.ext2_memory.encode(), &[Tag::PCO2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rekordbox's own XML export of the sandbox (2026-10-13): hot cue A with code 8, and hot
    /// cue C without a code under `HotCueColorType` 4.
    #[test]
    fn ui_palette_matches_rekordbox_xml() {
        assert_eq!(hot_cue_ui_rgb(1, false, 8, 4), [69, 172, 219]);
        assert_eq!(hot_cue_ui_rgb(3, false, 0, 4), [125, 193, 61]);
        assert_eq!(hot_cue_rgb(1, false, 8, 4), [0x00, 0xc4, 0xff], "devices keep their palette");
    }

    fn row(kind: i64, in_ms: i64, out: i64) -> SourceCue {
        SourceCue { kind, in_msec: in_ms, out_msec: out, color: -1, ..Default::default() }
    }

    #[test]
    fn no_cues_give_the_empty_lists() {
        let c = device_cues(&[], 4);
        assert_eq!(c.dat_hot, CueList::empty(CueKind::Hot));
        assert_eq!(c.dat_memory, CueList::empty(CueKind::Memory));
        assert_eq!(c.ext2_hot, CueListExt::empty(CueKind::Hot));
        assert!(c.is_empty());
    }

    #[test]
    fn lists_split_and_reverse_like_get_export_cue() {
        let rows = [row(0, 10_000, -1), row(1, 20_000, -1), row(5, 30_000, -1), row(0, 40_000, 44_000), row(3, 50_000, -1)];
        let c = device_cues(&rows, 4);
        assert_eq!(c.dat_hot.entries.iter().map(|e| (e.hot_cue, e.time)).collect::<Vec<_>>(), vec![(3, 50_000), (1, 20_000)]);
        assert_eq!(c.ext_hot.entries.iter().map(|e| e.hot_cue).collect::<Vec<_>>(), vec![4]);
        assert_eq!(c.ext2_hot.entries.iter().map(|e| e.hot_cue).collect::<Vec<_>>(), vec![3, 4, 1]);
        let m = &c.dat_memory;
        assert_eq!(m.entries.iter().map(|e| (e.time, e.loop_end, e.cue_type)).collect::<Vec<_>>(), vec![(40_000, 44_000, CueType::Loop), (10_000, NONE_U32, CueType::Point)]);
        assert_eq!((m.unk_14, m.unk_16, m.entries[0].prev, m.entries[0].next, m.entries[1].prev, m.entries[1].next), (0, 1, NONE_U16, 1, 0, NONE_U16));
        // Hot cue colours: no code → code 0 with the ColorFull default (C = 23, D = 60).
        assert_eq!(&c.ext2_hot.entries[0].ext[..4], &[0, 0x33, 0xff, 0x00]);
        assert_eq!(&c.ext2_hot.entries[1].ext[..4], &[0, 0x4d, 0x00, 0xff]);
        assert!(c.ext2_memory.entries.iter().all(|e| e.ext == vec![0; 44] && e.unk_1d == 1));
    }

    #[test]
    fn options_and_cleanup() {
        let mut hot = row(1, 1_000, -1);
        hot.color_table_index = 8;
        hot.comment = "drop".into();
        hot.color = 255;
        let mut mem = row(0, 2_000, -1);
        mem.color = 2;
        let dup_hot = row(1, 5_000, -1);
        let dup_mem = row(0, 2_000, -1);
        let c = device_cues(&[dup_hot, hot, mem, dup_mem, row(42, 7_000, -1)], 4);
        assert_eq!(c.dat_hot.entries.len(), 1, "the later hot A wins");
        assert_eq!(c.dat_hot.entries[0].time, 1_000);
        let e = &c.ext2_hot.entries[0];
        assert_eq!((&e.ext[..4], e.comment.as_deref(), e.color_index), (&[8, 0x00, 0xc4, 0xff][..], Some("drop"), 0));
        assert_eq!(c.ext2_memory.entries.len(), 1, "duplicate memory cue dropped");
        assert_eq!(c.ext2_memory.entries[0].color_index, 3);
        assert!(!has_extended_data(&[row(0, 1, -1)]));
        assert!(has_extended_data(&[SourceCue { color_table_index: 8, ..row(1, 1, -1) }]));
        let loops: Vec<SourceCue> = (0..12).map(|i| row(0, i * 1000, -1)).collect();
        assert_eq!(device_cues(&loops, 4).dat_memory.entries.len(), 10, "at most ten memory cues");
    }
}
