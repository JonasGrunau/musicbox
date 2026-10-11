//! PVB2: seek table (.EXT). Writer `write_pmai_ext_seek_table` @0x1013d0f40, setter
//! `set_pmai_ext_seek_table_data` @0x100f26928, `MstStoreSeekTable` @0x1007b761c.
//! Spec: `docs/re/anlz.md` § PVB2. Only one fixture has it, so the meaning is a hypothesis.

use super::{Put, Rd, TagResult, check_tag};
use crate::{Section, Tag};

/// One 20-byte seek point (hypothesis: sample position, byte offset in the audio file, and the
/// number of samples in the block; the fixture has 0x6c00 sample steps and 4608 per entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SeekPoint {
    pub sample: u64,
    pub byte_offset: u64,
    pub block_samples: u32,
}

/// PVB2 section.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | `unk_0e` (0) |
/// | 0x10 | u64 | `total_samples` (hypothesis) |
/// | 0x18 | u32 | entry count (400 in the fixture) |
/// | 0x1c | u32 | entry size (20, from `init_pmai_ext_seek_table`) |
/// | 0x20 | [`SeekPoint`] × count | entries |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeekTable {
    pub unk_0c: u16,
    pub unk_0e: u16,
    pub total_samples: u64,
    pub points: Vec<SeekPoint>,
}

impl SeekTable {
    const ENTRY_SIZE: u32 = 20;

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PVB2])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (unk_0c, unk_0e, total_samples) = (h.u16()?, h.u16()?, h.u64()?);
        let count = h.u32()? as usize;
        if h.u32()? != Self::ENTRY_SIZE {
            return h.err("unsupported PVB2 entry size");
        }
        h.finish()?;
        if count.checked_mul(Self::ENTRY_SIZE as usize) != Some(s.body.len()) {
            return h.err("entry count does not match body");
        }
        let mut b = Rd::new(s.tag, &s.body);
        let points = (0..count)
            .map(|_| {
                Ok(SeekPoint {
                    sample: b.u64()?,
                    byte_offset: b.u64()?,
                    block_samples: b.u32()?,
                })
            })
            .collect::<TagResult<_>>()?;
        Ok(Self {
            unk_0c,
            unk_0e,
            total_samples,
            points,
        })
    }

    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(20);
        header.put_u16(self.unk_0c);
        header.put_u16(self.unk_0e);
        header.put_u64(self.total_samples);
        header.put_u32(self.points.len() as u32);
        header.put_u32(Self::ENTRY_SIZE);
        let mut body = Vec::with_capacity(self.points.len() * Self::ENTRY_SIZE as usize);
        for p in &self.points {
            body.put_u64(p.sample);
            body.put_u64(p.byte_offset);
            body.put_u32(p.block_samples);
        }
        Section {
            tag: Tag::PVB2,
            header,
            body,
        }
    }
}
