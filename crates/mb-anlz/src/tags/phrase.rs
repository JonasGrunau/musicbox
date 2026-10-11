//! PSSI: song structure / phrases (.EXT). Writer `write_pmai_ext_song_struct` @0x1013d10b4.
//! On USB exports the section is XOR-scrambled (`ConvUSB_ScrambleSongStruct` @0x1007b3890, see
//! [`crate::usb::scramble_song_structure`]). No fixture has PSSI yet: layout from code only, so
//! this whole view is a **hypothesis**. Spec: `docs/re/anlz.md` § PSSI.

use super::{Put, Rd, TagResult, check_tag};
use crate::usb::scramble_song_structure;
use crate::{Section, Tag};

/// One 24-byte phrase entry, stored as 12 big-endian u16 words (the writer emits it word by word).
/// Field names for the words are hypotheses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Phrase {
    pub words: [u16; 12],
}

impl Phrase {
    /// Phrase number, starting at 1 (word 0).
    pub fn index(&self) -> u16 {
        self.words[0]
    }
    /// Beat number where the phrase starts (word 1).
    pub fn beat(&self) -> u16 {
        self.words[1]
    }
    /// Phrase kind; its meaning depends on the track mood (word 2).
    pub fn kind(&self) -> u16 {
        self.words[2]
    }
    /// Non-zero when the phrase ends with a fill-in (word 10).
    pub fn fill(&self) -> u16 {
        self.words[10]
    }
    /// Beat number where the fill-in starts (word 11).
    pub fn beat_fill(&self) -> u16 {
        self.words[11]
    }
}

/// PSSI section, always held unscrambled; `scrambled` records the stored form.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (writer stores 0; also the scramble start index) |
/// | 0x0e | u16 | entry size (24) |
/// | 0x10 | u16 | phrase count (its low byte keys the scramble) |
/// | 0x12 | u16 | `mood` (scrambled from here on) |
/// | 0x14 | u32 | `unk_14` (writer stores 0) |
/// | 0x18 | u16 | `unk_18` (writer stores 0; non-zero means the section is scrambled) |
/// | 0x1a | u16 | `end_beat` |
/// | 0x1c | u8 | `bank` |
/// | 0x1d | u8 | `unk_1d` |
/// | 0x1e | u8 | `unk_1e` |
/// | 0x1f | u8 | `unk_1f` (local writer stores 0, the USB export copies a value) |
/// | 0x20 | [`Phrase`] × count | phrases |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongStructure {
    pub scrambled: bool,
    pub unk_0c: u16,
    pub mood: u16,
    pub unk_14: u32,
    pub unk_18: u16,
    pub end_beat: u16,
    pub bank: u8,
    pub unk_1d: u8,
    pub unk_1e: u8,
    pub unk_1f: u8,
    pub phrases: Vec<Phrase>,
}

impl SongStructure {
    const ENTRY_SIZE: u16 = 24;

    /// True when a stored PSSI section is scrambled. The export scrambles only when bytes 0x18 and
    /// 0x19 are both zero (they always are in plain form), and the key never maps both to zero.
    pub fn is_scrambled(s: &Section) -> bool {
        s.header
            .get(0x18 - 12..0x1a - 12)
            .is_some_and(|b| b != [0, 0])
    }

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PSSI])?;
        let scrambled = Self::is_scrambled(s);
        let plain = if scrambled {
            let mut bytes = section_bytes(s);
            scramble_song_structure(&mut bytes);
            let header_len = 12 + s.header.len();
            Section {
                tag: s.tag,
                header: bytes[12..header_len].to_vec(),
                body: bytes[header_len..].to_vec(),
            }
        } else {
            s.clone()
        };
        let mut h = Rd::new(s.tag, &plain.header);
        let unk_0c = h.u16()?;
        if h.u16()? != Self::ENTRY_SIZE {
            return h.err("unsupported PSSI entry size");
        }
        let count = h.u16()? as usize;
        let v = Self {
            scrambled,
            unk_0c,
            mood: h.u16()?,
            unk_14: h.u32()?,
            unk_18: h.u16()?,
            end_beat: h.u16()?,
            bank: h.u8()?,
            unk_1d: h.u8()?,
            unk_1e: h.u8()?,
            unk_1f: h.u8()?,
            phrases: Vec::new(),
        };
        h.finish()?;
        if count * usize::from(Self::ENTRY_SIZE) != plain.body.len() {
            return h.err("phrase count does not match body");
        }
        let mut b = Rd::new(s.tag, &plain.body);
        let mut phrases = Vec::with_capacity(count);
        for _ in 0..count {
            let mut words = [0u16; 12];
            for w in &mut words {
                *w = b.u16()?;
            }
            phrases.push(Phrase { words });
        }
        Ok(Self { phrases, ..v })
    }

    /// Encodes in the stored form (scrambled when `self.scrambled`).
    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(20);
        header.put_u16(self.unk_0c);
        header.put_u16(Self::ENTRY_SIZE);
        header.put_u16(self.phrases.len() as u16);
        header.put_u16(self.mood);
        header.put_u32(self.unk_14);
        header.put_u16(self.unk_18);
        header.put_u16(self.end_beat);
        header.extend_from_slice(&[self.bank, self.unk_1d, self.unk_1e, self.unk_1f]);
        let body: Vec<u8> = self
            .phrases
            .iter()
            .flat_map(|p| p.words)
            .flat_map(u16::to_be_bytes)
            .collect();
        let plain = Section {
            tag: Tag::PSSI,
            header,
            body,
        };
        if !self.scrambled {
            return plain;
        }
        let mut bytes = section_bytes(&plain);
        scramble_song_structure(&mut bytes);
        let header_len = 12 + plain.header.len();
        Section {
            tag: Tag::PSSI,
            header: bytes[12..header_len].to_vec(),
            body: bytes[header_len..].to_vec(),
        }
    }
}

/// The full on-disk bytes of one section (prefix + header + body).
fn section_bytes(s: &Section) -> Vec<u8> {
    let header_len = 12 + s.header.len();
    let mut out = Vec::with_capacity(header_len + s.body.len());
    out.extend_from_slice(&s.tag.0);
    out.put_u32(header_len as u32);
    out.put_u32((header_len + s.body.len()) as u32);
    out.extend_from_slice(&s.header);
    out.extend_from_slice(&s.body);
    out
}
