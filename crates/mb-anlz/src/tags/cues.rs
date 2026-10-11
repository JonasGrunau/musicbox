//! Cue lists: PCOB with PCPT entries (.DAT and .EXT) and PCO2 with PCP2 entries (.EXT).
//! Writers `write_pmai_hot_cue` @0x100f5b3a8, `write_pmai_memory_cue` @0x100f5b558,
//! `write_pmai_cue_data` @0x100f5b7fc, `write_pmai_ext_cue` @0x1013d06c4,
//! `write_pmai_ext_cue_ext` @0x1013d087c, `write_pmai_ext_cue_ext_data` @0x1013d1340.
//! Spec: `docs/re/anlz.md` § PCOB, § PCO2.

use super::{Put, Rd, TagResult, check_tag, decode_utf16z, encode_utf16z};
use crate::{Section, Tag};

/// Which list a PCOB/PCO2 holds (u16 at 0x0e of the list header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CueKind {
    Memory,
    Hot,
    Other(u16),
}

impl CueKind {
    pub fn from_raw(v: u16) -> Self {
        match v {
            0 => Self::Memory,
            1 => Self::Hot,
            v => Self::Other(v),
        }
    }

    pub fn raw(self) -> u16 {
        match self {
            Self::Memory => 0,
            Self::Hot => 1,
            Self::Other(v) => v,
        }
    }
}

/// Cue type byte (0x1c in PCPT, 0x10 in PCP2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CueType {
    Point,
    Loop,
    Other(u8),
}

impl CueType {
    pub fn from_raw(v: u8) -> Self {
        match v {
            1 => Self::Point,
            2 => Self::Loop,
            v => Self::Other(v),
        }
    }

    pub fn raw(self) -> u8 {
        match self {
            Self::Point => 1,
            Self::Loop => 2,
            Self::Other(v) => v,
        }
    }
}

/// "No loop end" / "no link" marker.
pub const NONE_U32: u32 = 0xffff_ffff;
pub const NONE_U16: u16 = 0xffff;

/// Converts a cue position to milliseconds the way `convertToMsec` @0x100f5bb1c does (with its
/// rounding flag off): unit 1000 means the value already is in ms, unit 0 yields 0, any other unit
/// is positions per second, rounded up.
pub fn position_to_ms(value: u32, time_unit: u16) -> u32 {
    match time_unit {
        0 => 0,
        1000 => value,
        unit => (f64::from(value) * 1000.0 / f64::from(unit)).ceil() as u32,
    }
}

/// PCPT entry (56 bytes; sub-atom header 0x1c).
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | `hot_cue`: 0 = memory cue, 1.. = hot cue A.. (the .DAT list holds A–C only) |
/// | 0x10 | u32 | `unk_10` (0 in fixtures) |
/// | 0x14 | u16 | `status` (1 = in use when written by 7.x; 0 in older files) |
/// | 0x16 | u16 | `unk_16` (0) |
/// | 0x18 | u16 | `prev` index in the memory-cue list (0xffff = none) |
/// | 0x1a | u16 | `next` index in the memory-cue list (0xffff = none) |
/// | 0x1c | u8 | cue type (1 point, 2 loop) |
/// | 0x1d | u8 | `unk_1d` (writer stores 0) |
/// | 0x1e | u16 | `time_unit` (1000 = positions in ms) |
/// | 0x20 | u32 | `time` (cue position) |
/// | 0x24 | u32 | `loop_end` (0xffffffff = none) |
/// | 0x28 | u32 × 4 | `unk_28` (0 when `status` = 1, else 0xffffffff) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueEntry {
    pub unk_0c: u16,
    pub hot_cue: u16,
    pub unk_10: u32,
    pub status: u16,
    pub unk_16: u16,
    pub prev: u16,
    pub next: u16,
    pub cue_type: CueType,
    pub unk_1d: u8,
    pub time_unit: u16,
    pub time: u32,
    pub loop_end: u32,
    pub unk_28: [u32; 4],
}

impl CueEntry {
    const HEADER_LEN: u32 = 0x1c;
    const LEN: u32 = 0x38;

    pub fn time_ms(&self) -> u32 {
        position_to_ms(self.time, self.time_unit)
    }

    pub fn loop_end_ms(&self) -> Option<u32> {
        (self.loop_end != NONE_U32).then(|| position_to_ms(self.loop_end, self.time_unit))
    }

    fn read(r: &mut Rd) -> TagResult<Self> {
        if r.array::<4>()? != Tag::PCPT.0 {
            return r.err("PCOB entry is not PCPT");
        }
        if r.u32()? != Self::HEADER_LEN || r.u32()? != Self::LEN {
            return r.err("unsupported PCPT length");
        }
        Ok(Self {
            unk_0c: r.u16()?,
            hot_cue: r.u16()?,
            unk_10: r.u32()?,
            status: r.u16()?,
            unk_16: r.u16()?,
            prev: r.u16()?,
            next: r.u16()?,
            cue_type: CueType::from_raw(r.u8()?),
            unk_1d: r.u8()?,
            time_unit: r.u16()?,
            time: r.u32()?,
            loop_end: r.u32()?,
            unk_28: [r.u32()?, r.u32()?, r.u32()?, r.u32()?],
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&Tag::PCPT.0);
        out.put_u32(Self::HEADER_LEN);
        out.put_u32(Self::LEN);
        out.put_u16(self.unk_0c);
        out.put_u16(self.hot_cue);
        out.put_u32(self.unk_10);
        out.put_u16(self.status);
        out.put_u16(self.unk_16);
        out.put_u16(self.prev);
        out.put_u16(self.next);
        out.put_u8(self.cue_type.raw());
        out.put_u8(self.unk_1d);
        out.put_u16(self.time_unit);
        out.put_u32(self.time);
        out.put_u32(self.loop_end);
        for v in self.unk_28 {
            out.put_u32(v);
        }
    }
}

/// PCOB section.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | list kind (1 hot, 0 memory) |
/// | 0x10 | u16 | `unk_10` (0) |
/// | 0x12 | u16 | entry count |
/// | 0x14 | u16 | `unk_14`; hypothesis: first index of the memory-cue list (0xffff = empty) |
/// | 0x16 | u16 | `unk_16`; hypothesis: last index of the memory-cue list (0xffff = empty) |
/// | 0x18 | [`CueEntry`] × count | entries |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueList {
    pub unk_0c: u16,
    pub kind: CueKind,
    pub unk_10: u16,
    pub unk_14: u16,
    pub unk_16: u16,
    pub entries: Vec<CueEntry>,
}

impl CueList {
    pub fn empty(kind: CueKind) -> Self {
        Self {
            unk_0c: 0,
            kind,
            unk_10: 0,
            unk_14: NONE_U16,
            unk_16: NONE_U16,
            entries: Vec::new(),
        }
    }

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PCOB])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (unk_0c, kind, unk_10, count) =
            (h.u16()?, CueKind::from_raw(h.u16()?), h.u16()?, h.u16()?);
        let (unk_14, unk_16) = (h.u16()?, h.u16()?);
        h.finish()?;
        let mut b = Rd::new(s.tag, &s.body);
        let entries = (0..count)
            .map(|_| CueEntry::read(&mut b))
            .collect::<TagResult<_>>()?;
        b.finish()?;
        Ok(Self {
            unk_0c,
            kind,
            unk_10,
            unk_14,
            unk_16,
            entries,
        })
    }

    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(12);
        header.put_u16(self.unk_0c);
        header.put_u16(self.kind.raw());
        header.put_u16(self.unk_10);
        header.put_u16(self.entries.len() as u16);
        header.put_u16(self.unk_14);
        header.put_u16(self.unk_16);
        let mut body = Vec::with_capacity(self.entries.len() * CueEntry::LEN as usize);
        for e in &self.entries {
            e.write(&mut body);
        }
        Section {
            tag: Tag::PCOB,
            header,
            body,
        }
    }
}

/// PCP2 entry (sub-atom header 0x10, variable length).
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | `hot_cue`: 0 = memory cue, 1.. = hot cue A.. (up to H in fixtures) |
/// | 0x10 | u8 | cue type (1 point, 2 loop) |
/// | 0x11 | u8 | `unk_11` (0) |
/// | 0x12 | u16 | `time_unit` (1000 = ms) |
/// | 0x14 | u32 | `time` |
/// | 0x18 | u32 | `loop_end` (0xffffffff = none) |
/// | 0x1c | u8 | `color_index`: memory-cue colour 1..=8, 0 = none (setter stores index + 1) |
/// | 0x1d | u8 | `unk_1d` (1 when the 44-byte extension is present in fixtures) |
/// | 0x1e | u16 | `in_us`; hypothesis: sub-ms part of `time` in µs (0..=999) |
/// | 0x20 | u16 | `out_us`; hypothesis: sub-ms part of `loop_end` in µs |
/// | 0x22 | u16 | `unk_22` (0) |
/// | 0x24 | u16 | `loop_numerator` (beat-loop size; 0 = none) |
/// | 0x26 | u16 | `loop_denominator` |
/// | 0x28 | u16 | `unk_28` (0) |
/// | 0x2a | u16 | comment length in bytes, including the NUL (0 = no comment) |
/// | 0x2c | UTF-16BE | comment, NUL-terminated |
/// | … | bytes | `ext`: 0, 4 or 44 bytes; first 4 = hot-cue colour code, R, G, B |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueEntryExt {
    pub unk_0c: u16,
    pub hot_cue: u16,
    pub cue_type: CueType,
    pub unk_11: u8,
    pub time_unit: u16,
    pub time: u32,
    pub loop_end: u32,
    pub color_index: u8,
    pub unk_1d: u8,
    pub in_us: u16,
    pub out_us: u16,
    pub unk_22: u16,
    pub loop_numerator: u16,
    pub loop_denominator: u16,
    pub unk_28: u16,
    pub comment: Option<String>,
    /// Everything after the comment, kept raw. rekordbox writes 4 bytes (colour code, R, G, B) or
    /// 44 bytes (those 4, then four u64 and two u32 that are zero in all fixtures).
    pub ext: Vec<u8>,
}

impl CueEntryExt {
    const HEADER_LEN: u32 = 0x10;
    const FIXED_LEN: usize = 0x2c;

    pub fn time_ms(&self) -> u32 {
        position_to_ms(self.time, self.time_unit)
    }

    pub fn loop_end_ms(&self) -> Option<u32> {
        (self.loop_end != NONE_U32).then(|| position_to_ms(self.loop_end, self.time_unit))
    }

    /// Hot-cue colour: (palette code, [R, G, B]) when the extension carries one. Code 0 appears in
    /// older files that still carry an RGB value.
    pub fn color(&self) -> Option<(u8, [u8; 3])> {
        (self.ext.len() >= 4).then(|| (self.ext[0], [self.ext[1], self.ext[2], self.ext[3]]))
    }

    /// Hot-cue letter (`A` for 1), or `None` for memory cues.
    pub fn hot_cue_letter(&self) -> Option<char> {
        (1..=26)
            .contains(&self.hot_cue)
            .then(|| char::from(b'A' + (self.hot_cue - 1) as u8))
    }

    fn read(r: &mut Rd) -> TagResult<Self> {
        if r.array::<4>()? != Tag::PCP2.0 {
            return r.err("PCO2 entry is not PCP2");
        }
        if r.u32()? != Self::HEADER_LEN {
            return r.err("unsupported PCP2 header length");
        }
        let total = r.u32()? as usize;
        if total < Self::FIXED_LEN {
            return r.err("PCP2 entry too short");
        }
        let mut e = Rd::new(Tag::PCP2, r.bytes(total - 12)?);
        let mut v = Self {
            unk_0c: e.u16()?,
            hot_cue: e.u16()?,
            cue_type: CueType::from_raw(e.u8()?),
            unk_11: e.u8()?,
            time_unit: e.u16()?,
            time: e.u32()?,
            loop_end: e.u32()?,
            color_index: e.u8()?,
            unk_1d: e.u8()?,
            in_us: e.u16()?,
            out_us: e.u16()?,
            unk_22: e.u16()?,
            loop_numerator: e.u16()?,
            loop_denominator: e.u16()?,
            unk_28: e.u16()?,
            comment: None,
            ext: Vec::new(),
        };
        let comment_len = e.u16()? as usize;
        if comment_len > 0 {
            v.comment = Some(decode_utf16z(Tag::PCP2, e.bytes(comment_len)?)?);
        }
        v.ext = e.rest().to_vec();
        Ok(v)
    }

    fn write(&self, out: &mut Vec<u8>) {
        let comment = self
            .comment
            .as_deref()
            .map(encode_utf16z)
            .unwrap_or_default();
        out.extend_from_slice(&Tag::PCP2.0);
        out.put_u32(Self::HEADER_LEN);
        out.put_u32((Self::FIXED_LEN + comment.len() + self.ext.len()) as u32);
        out.put_u16(self.unk_0c);
        out.put_u16(self.hot_cue);
        out.put_u8(self.cue_type.raw());
        out.put_u8(self.unk_11);
        out.put_u16(self.time_unit);
        out.put_u32(self.time);
        out.put_u32(self.loop_end);
        out.put_u8(self.color_index);
        out.put_u8(self.unk_1d);
        out.put_u16(self.in_us);
        out.put_u16(self.out_us);
        out.put_u16(self.unk_22);
        out.put_u16(self.loop_numerator);
        out.put_u16(self.loop_denominator);
        out.put_u16(self.unk_28);
        out.put_u16(comment.len() as u16);
        out.extend_from_slice(&comment);
        out.extend_from_slice(&self.ext);
    }
}

/// PCO2 section.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | list kind (1 hot, 0 memory) |
/// | 0x10 | u16 | entry count |
/// | 0x12 | u16 | `unk_12` (0) |
/// | 0x14 | [`CueEntryExt`] × count | entries |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueListExt {
    pub unk_0c: u16,
    pub kind: CueKind,
    pub unk_12: u16,
    pub entries: Vec<CueEntryExt>,
}

impl CueListExt {
    pub fn empty(kind: CueKind) -> Self {
        Self {
            unk_0c: 0,
            kind,
            unk_12: 0,
            entries: Vec::new(),
        }
    }

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PCO2])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (unk_0c, kind, count, unk_12) =
            (h.u16()?, CueKind::from_raw(h.u16()?), h.u16()?, h.u16()?);
        h.finish()?;
        let mut b = Rd::new(s.tag, &s.body);
        let entries = (0..count)
            .map(|_| CueEntryExt::read(&mut b))
            .collect::<TagResult<_>>()?;
        b.finish()?;
        Ok(Self {
            unk_0c,
            kind,
            unk_12,
            entries,
        })
    }

    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(8);
        header.put_u16(self.unk_0c);
        header.put_u16(self.kind.raw());
        header.put_u16(self.entries.len() as u16);
        header.put_u16(self.unk_12);
        let mut body = Vec::new();
        for e in &self.entries {
            e.write(&mut body);
        }
        Section {
            tag: Tag::PCO2,
            header,
            body,
        }
    }
}
