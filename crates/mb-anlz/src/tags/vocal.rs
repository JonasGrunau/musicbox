//! PVDI: vocal detection (.2EX). Writer `write_pmai_ex2_vocal_detect` @0x100786f44, defaults from
//! `init_pmai_ex2_vocal_detect` @0x100785e88. On USB the body is XOR-scrambled and bit 15 of the
//! version is set (`ExportAnalysisExtend2` @0x1007b9a04, `ConvUSB_ScrambleVocalDetect` @0x1007b3b3c).
//! No fixture has PVDI yet: this whole view is a **hypothesis**. Spec: `docs/re/anlz.md` § PVDI.

use super::{Put, Rd, TagResult, check_tag};
use crate::usb::scramble_vocal_detect;
use crate::{Section, Tag};

/// Version bit that marks a scrambled body.
pub const PVDI_SCRAMBLED: u16 = 0x8000;

/// PVDI section, data always held unscrambled.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `version` (bit 15 = body scrambled; `get/set_pmai_ex2_vocal_detect_version`) |
/// | 0x0e | u16 | `unk_0e` (default 1024; hypothesis: hop size in samples) |
/// | 0x10 | u16 | `unk_10` (default 22050; hypothesis: sample rate) |
/// | 0x12 | u16 | entry size (default 1) |
/// | 0x14 | u32 | entry count |
/// | 0x18 | bytes | entries (count × entry size) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocalDetect {
    pub version: u16,
    pub unk_0e: u16,
    pub unk_10: u16,
    pub entry_size: u16,
    /// Plain (unscrambled) entry bytes.
    pub data: Vec<u8>,
}

impl VocalDetect {
    pub fn is_scrambled(&self) -> bool {
        self.version & PVDI_SCRAMBLED != 0
    }

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PVDI])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (version, unk_0e, unk_10, entry_size) = (h.u16()?, h.u16()?, h.u16()?, h.u16()?);
        let count = h.u32()? as usize;
        h.finish()?;
        if entry_size == 0 || count.checked_mul(entry_size.into()) != Some(s.body.len()) {
            return h.err("entry count does not match body");
        }
        let mut data = s.body.clone();
        if version & PVDI_SCRAMBLED != 0 {
            // The export scrambles `point_num` bytes; with entry size 1 that is the whole body.
            scramble_vocal_detect(&mut data[..count]);
        }
        Ok(Self {
            version,
            unk_0e,
            unk_10,
            entry_size,
            data,
        })
    }

    pub fn encode(&self) -> Section {
        let count = self.data.len() / usize::from(self.entry_size.max(1));
        let mut header = Vec::with_capacity(12);
        header.put_u16(self.version);
        header.put_u16(self.unk_0e);
        header.put_u16(self.unk_10);
        header.put_u16(self.entry_size);
        header.put_u32(count as u32);
        let mut body = self.data.clone();
        if self.is_scrambled() {
            scramble_vocal_detect(&mut body[..count]);
        }
        Section {
            tag: Tag::PVDI,
            header,
            body,
        }
    }
}
