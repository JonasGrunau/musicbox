//! PPTH: the track path the file belongs to (all three file kinds).
//! Writer `write_pmai_path` @0x100f5ae2c. Spec: `docs/re/anlz.md` § PPTH.

use super::{Put, Rd, TagResult, check_tag, decode_utf16z, encode_utf16z};
use crate::{Section, Tag};

/// PPTH section.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (always 0) |
/// | 0x0e | u16 | path length in bytes, including the NUL terminator |
/// | 0x10 | UTF-16BE | path, NUL-terminated |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub unk_0c: u16,
    /// Local files: `?/<file name>`. USB files: the track's path on the device, e.g.
    /// `/Contents/Artist/Album/track.mp3`.
    pub path: String,
}

impl Path {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            unk_0c: 0,
            path: path.into(),
        }
    }

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PPTH])?;
        let mut h = Rd::new(s.tag, &s.header);
        let unk_0c = h.u16()?;
        let len = h.u16()? as usize;
        h.finish()?;
        if len != s.body.len() {
            return h.err("path length field does not match body");
        }
        Ok(Self {
            unk_0c,
            path: decode_utf16z(s.tag, &s.body)?,
        })
    }

    pub fn encode(&self) -> Section {
        let body = encode_utf16z(&self.path);
        let mut header = Vec::with_capacity(4);
        header.put_u16(self.unk_0c);
        header.put_u16(body.len() as u16);
        Section {
            tag: Tag::PPTH,
            header,
            body,
        }
    }

    /// True for rekordbox 7's local form `?/<file name>` (`is_new_format_ppth` @0x100656e80).
    pub fn is_local_form(&self) -> bool {
        self.path.starts_with("?/")
    }
}
