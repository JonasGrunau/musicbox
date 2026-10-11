//! rekordbox analysis files (`ANLZ0000.DAT` / `.EXT` / `.2EX`). Spec: `docs/re/anlz.md`.
//!
//! The container is lossless: every section keeps its raw tag-specific header and body bytes, so
//! `parse` → `to_bytes` reproduces the input exactly. Typed tag views ([`tags`]) are layered on top
//! and are lossless as well. [`usb`] has the USB folder hash and the PSSI/PVDI scrambles.

pub mod tags;
pub mod usb;

use std::fmt;
use tags::TypedSection;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a PMAI file")]
    BadMagic,
    #[error("truncated at offset {0}")]
    Truncated(usize),
    #[error("section at offset {offset}: header length {header_len} / total length {total_len} invalid")]
    BadSection { offset: usize, header_len: u32, total_len: u32 },
    #[error("file length field {field} does not match actual length {actual}")]
    LengthMismatch { field: u32, actual: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Four-character section tag, e.g. `PQTZ`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tag(pub [u8; 4]);

impl Tag {
    pub const PMAI: Tag = Tag(*b"PMAI");
}

impl fmt::Debug for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// One tagged section. Layout: tag (4) | header_len u32 BE | total_len u32 BE | header rest | body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub tag: Tag,
    /// Tag-specific header bytes following the common 12-byte prefix.
    pub header: Vec<u8>,
    pub body: Vec<u8>,
}

/// A parsed PMAI file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnlzFile {
    /// PMAI header bytes following the common 12-byte prefix (16 bytes in 7.2.19 files).
    pub header: Vec<u8>,
    pub sections: Vec<Section>,
}

fn be_u32(data: &[u8], offset: usize) -> Result<u32> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or(Error::Truncated(offset))
}

impl AnlzFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.get(..4) != Some(b"PMAI") {
            return Err(Error::BadMagic);
        }
        let header_len = be_u32(data, 4)? as usize;
        let file_len = be_u32(data, 8)?;
        if file_len as usize != data.len() {
            return Err(Error::LengthMismatch { field: file_len, actual: data.len() });
        }
        if header_len < 12 || header_len > data.len() {
            return Err(Error::BadSection { offset: 0, header_len: header_len as u32, total_len: file_len });
        }
        let mut sections = Vec::new();
        let mut offset = header_len;
        while offset < data.len() {
            let tag = Tag(data.get(offset..offset + 4).ok_or(Error::Truncated(offset))?.try_into().unwrap());
            let section_header_len = be_u32(data, offset + 4)?;
            let total_len = be_u32(data, offset + 8)?;
            let (h, t) = (section_header_len as usize, total_len as usize);
            if h < 12 || t < h || offset + t > data.len() {
                return Err(Error::BadSection { offset, header_len: section_header_len, total_len });
            }
            sections.push(Section {
                tag,
                header: data[offset + 12..offset + h].to_vec(),
                body: data[offset + h..offset + t].to_vec(),
            });
            offset += t;
        }
        Ok(Self { header: data[12..header_len].to_vec(), sections })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let header_len = 12 + self.header.len();
        let total: usize = header_len
            + self.sections.iter().map(|s| 12 + s.header.len() + s.body.len()).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"PMAI");
        out.extend_from_slice(&(header_len as u32).to_be_bytes());
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(&self.header);
        for s in &self.sections {
            let h = 12 + s.header.len();
            out.extend_from_slice(&s.tag.0);
            out.extend_from_slice(&(h as u32).to_be_bytes());
            out.extend_from_slice(&((h + s.body.len()) as u32).to_be_bytes());
            out.extend_from_slice(&s.header);
            out.extend_from_slice(&s.body);
        }
        out
    }

    pub fn sections_with(&self, tag: Tag) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(move |s| s.tag == tag)
    }

    /// The 16 PMAI header bytes as fields, if they have the 7.x length.
    pub fn file_header(&self) -> Option<FileHeader> {
        FileHeader::decode(&self.header)
    }

    /// Every section decoded into its typed view (raw for unknown tags).
    pub fn typed_sections(&self) -> Vec<TypedSection> {
        self.sections.iter().map(TypedSection::decode).collect()
    }

    /// Builds a file from typed sections.
    pub fn from_typed(header: FileHeader, sections: &[TypedSection]) -> Self {
        Self { header: header.encode(), sections: sections.iter().map(TypedSection::encode).collect() }
    }

    fn first_typed<T>(&self, tag: Tag, f: impl Fn(&Section) -> tags::TagResult<T>) -> Option<T> {
        self.sections_with(tag).next().and_then(|s| f(s).ok())
    }

    /// The PPTH path.
    pub fn path(&self) -> Option<String> {
        self.first_typed(Tag::PPTH, tags::Path::decode).map(|p| p.path)
    }

    /// The PQTZ beat grid (.DAT).
    pub fn beat_grid(&self) -> Option<tags::BeatGrid> {
        self.first_typed(Tag::PQTZ, tags::BeatGrid::decode)
    }

    /// The PQT2 extended beat grid (.EXT).
    pub fn ext_beat_grid(&self) -> Option<tags::ExtBeatGrid> {
        self.first_typed(Tag::PQT2, tags::ExtBeatGrid::decode)
    }

    /// All PCOB lists (hot and memory).
    pub fn cue_lists(&self) -> Vec<tags::CueList> {
        self.sections_with(Tag::PCOB).filter_map(|s| tags::CueList::decode(s).ok()).collect()
    }

    /// All PCO2 lists (hot and memory).
    pub fn cue_lists_ext(&self) -> Vec<tags::CueListExt> {
        self.sections_with(Tag::PCO2).filter_map(|s| tags::CueListExt::decode(s).ok()).collect()
    }
}

/// The PMAI header bytes after the common 12-byte prefix (file offsets 0x0c..0x1c).
/// Defaults come from `initialize_pmai_header` @0x100f589b4; written by `write_pmai_header` @0x100f5ad28.
///
/// | Offset | Type | 7.x value |
/// |---|---|---|
/// | 0x0c | u16 | 0 |
/// | 0x0e | u16 | 1 (0 in one older stick EXT) |
/// | 0x10 | u16 | 1 |
/// | 0x12 | u16 | 0 |
/// | 0x14 | u16 | 1 |
/// | 0x16 | u16 | 0 |
/// | 0x18 | u32 | 0 |
///
/// The meaning of the values is open (likely format version numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileHeader {
    pub unk_0c: u16,
    pub unk_0e: u16,
    pub unk_10: u16,
    pub unk_12: u16,
    pub unk_14: u16,
    pub unk_16: u16,
    pub unk_18: u32,
}

impl Default for FileHeader {
    /// The values rekordbox 7.2.19 writes.
    fn default() -> Self {
        Self { unk_0c: 0, unk_0e: 1, unk_10: 1, unk_12: 0, unk_14: 1, unk_16: 0, unk_18: 0 }
    }
}

impl FileHeader {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let b: &[u8; 16] = bytes.try_into().ok()?;
        let u16_at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
        Some(Self {
            unk_0c: u16_at(0),
            unk_0e: u16_at(2),
            unk_10: u16_at(4),
            unk_12: u16_at(6),
            unk_14: u16_at(8),
            unk_16: u16_at(10),
            unk_18: u32::from_be_bytes([b[12], b[13], b[14], b[15]]),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        [self.unk_0c, self.unk_0e, self.unk_10, self.unk_12, self.unk_14, self.unk_16]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .chain(self.unk_18.to_be_bytes())
            .collect()
    }
}
