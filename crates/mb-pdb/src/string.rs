//! DeviceSQL strings. Spec: `docs/re/pdb.md` § Strings.
//!
//! The first byte decides the form (`TEStringForm` @0x102f7b2a4, `TE_GetUnicodeStr` @0x102f7c514):
//! - bit 0 set: short ASCII. The byte is `(total_len << 1) | 1`, `total_len` counting itself;
//!   `total_len - 1` ASCII bytes follow. rekordbox uses it for ASCII text of up to 126 characters.
//! - bit 0 clear: long form. A u32 header: bits 8..31 = total length including the 4 header bytes,
//!   bits 6..7 = bytes per character, bits 4..5 = encoding (1 = UTF-16). rekordbox writes `0x40`
//!   (ASCII, 1 byte per char) and `0x90` (UTF-16LE, 2 bytes per char).
//!
//! No form carries a terminator. ISRC values are a special case of `0x90`, see [`DeviceString::isrc`].

use crate::{Error, Result};

/// A decoded DeviceSQL string, keeping the raw header and payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceString {
    /// First header byte: odd for short ASCII, otherwise the long-form flags (0x40, 0x90, ...).
    pub kind: u8,
    /// Bytes after the header (1 header byte for short strings, 4 for long ones).
    pub payload: Vec<u8>,
}

impl DeviceString {
    pub const KIND_LONG_ASCII: u8 = 0x40;
    pub const KIND_UTF16: u8 = 0x90;

    /// Decodes the string starting at `data[offset]`.
    pub fn parse(data: &[u8], offset: usize) -> Result<Self> {
        let first = *data.get(offset).ok_or(Error::BadString {
            offset,
            reason: "offset beyond row",
        })?;
        if first & 1 == 1 {
            let total = (first >> 1) as usize;
            if total == 0 {
                return Err(Error::BadString {
                    offset,
                    reason: "short string with zero length",
                });
            }
            let payload = data
                .get(offset + 1..offset + total)
                .ok_or(Error::BadString {
                    offset,
                    reason: "short string runs past row",
                })?;
            return Ok(Self {
                kind: first,
                payload: payload.to_vec(),
            });
        }
        let header = data.get(offset..offset + 4).ok_or(Error::BadString {
            offset,
            reason: "long string header runs past row",
        })?;
        let total = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) >> 8;
        let total = total as usize;
        if total < 4 {
            return Err(Error::BadString {
                offset,
                reason: "long string shorter than its header",
            });
        }
        let payload = data
            .get(offset + 4..offset + total)
            .ok_or(Error::BadString {
                offset,
                reason: "long string runs past row",
            })?;
        Ok(Self {
            kind: first,
            payload: payload.to_vec(),
        })
    }

    /// Encodes the string the way rekordbox does (`TEStringFromUnicode` @0x102f7b8d4): all code
    /// units <= 0x7f → ASCII (short form up to 126 characters, else long 0x40); otherwise UTF-16LE
    /// (0x90).
    pub fn from_text(text: &str) -> Self {
        if text.is_ascii() {
            if text.len() < 0x7f {
                Self {
                    kind: (((text.len() + 1) << 1) | 1) as u8,
                    payload: text.as_bytes().to_vec(),
                }
            } else {
                Self {
                    kind: Self::KIND_LONG_ASCII,
                    payload: text.as_bytes().to_vec(),
                }
            }
        } else {
            let payload = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            Self {
                kind: Self::KIND_UTF16,
                payload,
            }
        }
    }

    /// The ISRC form rekordbox writes (`db::dsqlAddTrack` @0x1019a78d0): the bytes `0x03`,
    /// the 12 ASCII characters and `0x00`, stored as a 0x90 string.
    pub fn from_isrc(isrc: &str) -> Self {
        let mut payload = vec![0x03];
        payload.extend_from_slice(isrc.as_bytes());
        if payload.len() % 2 == 1 {
            payload.push(0);
        }
        Self {
            kind: Self::KIND_UTF16,
            payload,
        }
    }

    /// Encoded bytes (header + payload). Inverse of [`DeviceString::parse`].
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.encoded_len());
        if self.is_short() {
            out.push((((self.payload.len() + 1) << 1) | 1) as u8);
        } else {
            let total = (self.payload.len() + 4) as u32;
            out.extend_from_slice(&((total << 8) | u32::from(self.kind)).to_le_bytes());
        }
        out.extend_from_slice(&self.payload);
        out
    }

    /// Total encoded length including the header.
    pub fn encoded_len(&self) -> usize {
        self.payload.len() + if self.is_short() { 1 } else { 4 }
    }

    pub fn is_short(&self) -> bool {
        self.kind & 1 == 1
    }

    pub fn is_empty(&self) -> bool {
        self.payload.is_empty()
    }

    /// Characters are two bytes wide (bits 6..7 of the header = 2).
    pub fn is_utf16(&self) -> bool {
        !self.is_short() && (self.kind >> 6) == 2
    }

    /// The ISRC special case: a 0x90 string whose raw payload is `0x03`, ASCII, optional `0x00`.
    /// Returns the ASCII part.
    pub fn isrc(&self) -> Option<String> {
        if self.kind == Self::KIND_UTF16 && self.payload.first() == Some(&0x03) {
            let body = &self.payload[1..];
            let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
            let body = &body[..end];
            if body.iter().all(u8::is_ascii_graphic) {
                return Some(String::from_utf8_lossy(body).into_owned());
            }
        }
        None
    }

    /// Text value. ASCII forms are read as Latin-1 (lossless for any byte), UTF-16 as UTF-16LE with
    /// replacement of unpaired surrogates, and the ISRC form as its ASCII content.
    pub fn text(&self) -> String {
        if let Some(isrc) = self.isrc() {
            return isrc;
        }
        if self.is_utf16() {
            let units: Vec<u16> = self
                .payload
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            self.payload.iter().map(|&b| b as char).collect()
        }
    }
}

impl std::fmt::Display for DeviceString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_ascii_round_trip() {
        let s = DeviceString::from_text("Techno");
        assert_eq!(s.to_bytes(), b"\x0fTechno");
        assert_eq!(DeviceString::parse(&s.to_bytes(), 0).unwrap(), s);
        assert_eq!(DeviceString::from_text("").to_bytes(), [0x03]);
    }

    #[test]
    fn long_ascii_and_utf16() {
        let long = "x".repeat(127);
        let s = DeviceString::from_text(&long);
        assert_eq!(s.kind, 0x40);
        assert_eq!(&s.to_bytes()[..4], &[0x40, 131, 0, 0]);
        assert_eq!(DeviceString::parse(&s.to_bytes(), 0).unwrap().text(), long);
        let u = DeviceString::from_text("\u{fffa}GENRE\u{fffb}");
        assert_eq!(&u.to_bytes()[..4], &[0x90, 0x12, 0, 0]);
        assert_eq!(u.text(), "\u{fffa}GENRE\u{fffb}");
    }

    #[test]
    fn isrc_special_case() {
        let bytes = [
            0x90, 0x12, 0, 0, 0x03, b'Q', b'Z', b'S', b'6', b'6', b'2', b'1', b'6', b'6', b'2',
            b'2', b'4', 0,
        ];
        let s = DeviceString::parse(&bytes, 0).unwrap();
        assert_eq!(s.isrc().as_deref(), Some("QZS662166224"));
        assert_eq!(s.text(), "QZS662166224");
        assert_eq!(DeviceString::from_isrc("QZS662166224").to_bytes(), bytes);
    }
}
