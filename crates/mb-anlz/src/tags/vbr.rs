//! PVBR: VBR seek index (.DAT). Writer `write_pmai_vbr` @0x100f5af18. Spec: `docs/re/anlz.md` § PVBR.

use super::{Put, Rd, TagResult, check_tag};
use crate::{Section, Tag};

/// Number of index entries in a full PVBR table.
pub const VBR_INDEX_LEN: usize = 400;

/// PVBR section.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0 in all fixtures) |
/// | 0x0e | u16 | `unk_0e` (the writer stores 0) |
/// | 0x10 | u32 × 400 | `index` (all zero for CBR files) |
/// | 0x650 | u32 | `sample_count` (optional) |
///
/// The writer accepts three body sizes: 1604 (table + sample count), 1600 (table only), 4 (count only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vbr {
    pub unk_0c: u16,
    pub unk_0e: u16,
    /// 400 entries or empty.
    pub index: Vec<u32>,
    /// Set by `set_pmai_number_of_sample` @0x101619de4. Hypothesis: the decoded sample count.
    pub sample_count: Option<u32>,
}

impl Vbr {
    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PVBR])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (unk_0c, unk_0e) = (h.u16()?, h.u16()?);
        h.finish()?;
        let mut b = Rd::new(s.tag, &s.body);
        let has_table = match s.body.len() {
            1604 | 1600 => true,
            4 => false,
            _ => return b.err("unsupported PVBR body size"),
        };
        let mut index = Vec::new();
        if has_table {
            for _ in 0..VBR_INDEX_LEN {
                index.push(b.u32()?);
            }
        }
        let sample_count = if b.remaining() == 4 {
            Some(b.u32()?)
        } else {
            None
        };
        b.finish()?;
        Ok(Self {
            unk_0c,
            unk_0e,
            index,
            sample_count,
        })
    }

    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(4);
        header.put_u16(self.unk_0c);
        header.put_u16(self.unk_0e);
        let mut body = Vec::with_capacity(4 * self.index.len() + 4);
        for v in &self.index {
            body.put_u32(*v);
        }
        if let Some(n) = self.sample_count {
            body.put_u32(n);
        }
        Section {
            tag: Tag::PVBR,
            header,
            body,
        }
    }
}
