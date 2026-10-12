//! Typed views over PMAI sections. Spec: `docs/re/anlz.md` ("Tags").
//!
//! Every view decodes a [`Section`] into a struct and encodes it back. The rule is that a typed
//! round trip is byte-identical: fields whose meaning is unknown are kept under offset-based names,
//! and anything after the known layout is kept as raw bytes. Length fields that rekordbox derives
//! (entry counts, byte lengths) are recomputed on encode; `decode` rejects input where they do not
//! match the data, so such input stays an [`TypedSection::Unknown`] raw section instead.
//!
//! Offsets in field docs are relative to the start of the section (the tag), as in the spec.

mod beatgrid;
mod cues;
mod path;
mod phrase;
mod seek;
mod vbr;
mod vocal;
mod wave;

pub use beatgrid::{Beat, BeatGrid, ExtBeatGrid};
pub use cues::{CueEntry, CueEntryExt, CueKind, CueList, CueListExt, CueType, NONE_U16, NONE_U32};
pub use path::Path;
pub use phrase::{Phrase, SongStructure};
pub use seek::{SeekPoint, SeekTable};
pub use vbr::Vbr;
pub use vocal::VocalDetect;
pub use wave::{
    ColorColumn, PreviewColumn, Wave3Band, WaveColorPreview, WaveColorScroll, WaveNormalize,
    WavePreview, WaveScroll,
};

use crate::{Section, Tag};

/// Why a section could not be decoded into its typed view.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{tag}: {msg}")]
pub struct TagError {
    pub tag: Tag,
    pub msg: &'static str,
}

pub type TagResult<T> = std::result::Result<T, TagError>;

impl Tag {
    pub const PPTH: Tag = Tag(*b"PPTH");
    pub const PVBR: Tag = Tag(*b"PVBR");
    pub const PQTZ: Tag = Tag(*b"PQTZ");
    pub const PWAV: Tag = Tag(*b"PWAV");
    pub const PWV2: Tag = Tag(*b"PWV2");
    pub const PCOB: Tag = Tag(*b"PCOB");
    pub const PCPT: Tag = Tag(*b"PCPT");
    pub const PWV3: Tag = Tag(*b"PWV3");
    pub const PCO2: Tag = Tag(*b"PCO2");
    pub const PCP2: Tag = Tag(*b"PCP2");
    pub const PQT2: Tag = Tag(*b"PQT2");
    pub const PWV5: Tag = Tag(*b"PWV5");
    pub const PWV4: Tag = Tag(*b"PWV4");
    pub const PVB2: Tag = Tag(*b"PVB2");
    pub const PSSI: Tag = Tag(*b"PSSI");
    pub const PWV7: Tag = Tag(*b"PWV7");
    pub const PWV6: Tag = Tag(*b"PWV6");
    pub const PWVC: Tag = Tag(*b"PWVC");
    pub const PVDI: Tag = Tag(*b"PVDI");
}

/// A section decoded into its typed view, or kept raw when the tag is unknown or the content does
/// not fit the documented layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedSection {
    Path(Path),
    Vbr(Vbr),
    BeatGrid(BeatGrid),
    /// PWAV (400 columns) or PWV2 (100 columns).
    WavePreview(WavePreview),
    /// PCOB (in .DAT and .EXT).
    CueList(CueList),
    /// PWV3.
    WaveScroll(WaveScroll),
    /// PCO2.
    CueListExt(CueListExt),
    ExtBeatGrid(ExtBeatGrid),
    /// PWV5.
    WaveColorScroll(WaveColorScroll),
    /// PWV4.
    WaveColorPreview(WaveColorPreview),
    /// PVB2.
    SeekTable(SeekTable),
    /// PSSI.
    SongStructure(SongStructure),
    /// PWV6 (preview, 1200 columns) or PWV7 (scroll, 150 columns per second).
    Wave3Band(Wave3Band),
    /// PWVC.
    WaveNormalize(WaveNormalize),
    /// PVDI.
    VocalDetect(VocalDetect),
    Unknown(Section),
}

impl TypedSection {
    /// Decodes a section. Falls back to [`TypedSection::Unknown`] if the tag is unknown, the typed
    /// decoder rejects it, or (as a safety net) the typed view would not re-encode to the same bytes.
    pub fn decode(section: &Section) -> Self {
        match Self::try_decode(section) {
            Ok(typed) if &typed.encode() == section => typed,
            _ => TypedSection::Unknown(section.clone()),
        }
    }

    /// Decodes a section with a known tag, reporting why it does not fit the typed layout.
    /// Unknown tags decode to `Ok(Unknown)`.
    pub fn try_decode(s: &Section) -> TagResult<Self> {
        Ok(match &s.tag.0 {
            b"PPTH" => Self::Path(Path::decode(s)?),
            b"PVBR" => Self::Vbr(Vbr::decode(s)?),
            b"PQTZ" => Self::BeatGrid(BeatGrid::decode(s)?),
            b"PWAV" | b"PWV2" => Self::WavePreview(WavePreview::decode(s)?),
            b"PCOB" => Self::CueList(CueList::decode(s)?),
            b"PWV3" => Self::WaveScroll(WaveScroll::decode(s)?),
            b"PCO2" => Self::CueListExt(CueListExt::decode(s)?),
            b"PQT2" => Self::ExtBeatGrid(ExtBeatGrid::decode(s)?),
            b"PWV5" => Self::WaveColorScroll(WaveColorScroll::decode(s)?),
            b"PWV4" => Self::WaveColorPreview(WaveColorPreview::decode(s)?),
            b"PVB2" => Self::SeekTable(SeekTable::decode(s)?),
            b"PSSI" => Self::SongStructure(SongStructure::decode(s)?),
            b"PWV6" | b"PWV7" => Self::Wave3Band(Wave3Band::decode(s)?),
            b"PWVC" => Self::WaveNormalize(WaveNormalize::decode(s)?),
            b"PVDI" => Self::VocalDetect(VocalDetect::decode(s)?),
            _ => Self::Unknown(s.clone()),
        })
    }

    pub fn encode(&self) -> Section {
        match self {
            Self::Path(v) => v.encode(),
            Self::Vbr(v) => v.encode(),
            Self::BeatGrid(v) => v.encode(),
            Self::WavePreview(v) => v.encode(),
            Self::CueList(v) => v.encode(),
            Self::WaveScroll(v) => v.encode(),
            Self::CueListExt(v) => v.encode(),
            Self::ExtBeatGrid(v) => v.encode(),
            Self::WaveColorScroll(v) => v.encode(),
            Self::WaveColorPreview(v) => v.encode(),
            Self::SeekTable(v) => v.encode(),
            Self::SongStructure(v) => v.encode(),
            Self::Wave3Band(v) => v.encode(),
            Self::WaveNormalize(v) => v.encode(),
            Self::VocalDetect(v) => v.encode(),
            Self::Unknown(s) => s.clone(),
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

/// Big-endian cursor over a byte slice. Offsets passed to errors are only for messages.
pub(crate) struct Rd<'a> {
    tag: Tag,
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Rd<'a> {
    pub(crate) fn new(tag: Tag, buf: &'a [u8]) -> Self {
        Self { tag, buf, pos: 0 }
    }

    pub(crate) fn err<T>(&self, msg: &'static str) -> TagResult<T> {
        Err(TagError { tag: self.tag, msg })
    }

    pub(crate) fn bytes(&mut self, n: usize) -> TagResult<&'a [u8]> {
        match self.buf.get(self.pos..self.pos + n) {
            Some(b) => {
                self.pos += n;
                Ok(b)
            }
            None => self.err("truncated"),
        }
    }

    pub(crate) fn array<const N: usize>(&mut self) -> TagResult<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    pub(crate) fn u8(&mut self) -> TagResult<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> TagResult<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> TagResult<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub(crate) fn u64(&mut self) -> TagResult<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    pub(crate) fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let r = &self.buf[self.pos..];
        self.pos = self.buf.len();
        r
    }

    /// Fails unless the whole buffer was consumed.
    pub(crate) fn finish(&self) -> TagResult<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            self.err("unexpected trailing bytes")
        }
    }
}

/// Big-endian appenders.
pub(crate) trait Put {
    fn put_u8(&mut self, v: u8);
    fn put_u16(&mut self, v: u16);
    fn put_u32(&mut self, v: u32);
    fn put_u64(&mut self, v: u64);
}

impl Put for Vec<u8> {
    fn put_u8(&mut self, v: u8) {
        self.push(v);
    }
    fn put_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_be_bytes());
    }
    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_be_bytes());
    }
    fn put_u64(&mut self, v: u64) {
        self.extend_from_slice(&v.to_be_bytes());
    }
}

pub(crate) fn check_tag(s: &Section, allowed: &[Tag]) -> TagResult<()> {
    if allowed.contains(&s.tag) {
        Ok(())
    } else {
        Err(TagError {
            tag: s.tag,
            msg: "wrong tag for this view",
        })
    }
}

/// Decodes UTF-16BE text that must end in exactly one NUL unit (the PPTH / PCP2 comment form).
pub(crate) fn decode_utf16z(tag: Tag, bytes: &[u8]) -> TagResult<String> {
    let err = |msg| Err(TagError { tag, msg });
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return err("UTF-16 text has odd or zero length");
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    let (last, text) = units.split_last().unwrap();
    if *last != 0 || text.contains(&0) {
        return err("UTF-16 text is not NUL-terminated exactly once");
    }
    String::from_utf16(text).or(err("invalid UTF-16"))
}

pub(crate) fn encode_utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_be_bytes)
        .collect()
}
