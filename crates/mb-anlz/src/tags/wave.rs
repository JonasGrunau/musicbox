//! Waveforms: PWAV/PWV2 (.DAT previews), PWV3/PWV5/PWV4 (.EXT), PWV6/PWV7/PWVC (.2EX).
//! Writers `write_pmai_wave400pt` @0x100f5b1c8, `write_pmai_wave100pt` @0x100f5b2b8,
//! `write_pmai_ext_zoom_wave` @0x1013d0554, `write_pmai_ext_new_zoom_wave` @0x1013d0c40,
//! `write_pmai_ext_new_whole_wave` @0x1013d0dcc, `write_pmai_ex2_3bands_zoom_wave` @0x100786bb4,
//! `write_pmai_ex2_3bands_whole_wave` @0x100786d28, `write_pmai_ex2_wave_normalize_coef` @0x100786e6c.
//! Spec: `docs/re/anlz.md` § Waveforms.

use super::{Put, Rd, TagResult, check_tag};
use crate::{Section, Tag};

/// One column of a monochrome waveform byte (PWAV, PWV2, PWV3): low 5 bits height, high 3 bits
/// intensity ("whiteness"). Layout verified from the PWV3 packing in `ExportAnalysisExtendWithCue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreviewColumn {
    /// 0..=31 (PWV2 only uses 0..=15 in fixtures).
    pub height: u8,
    /// 0..=7; higher is whiter/brighter.
    pub intensity: u8,
}

impl PreviewColumn {
    pub fn from_byte(b: u8) -> Self {
        Self {
            height: b & 0x1f,
            intensity: b >> 5,
        }
    }

    pub fn to_byte(self) -> u8 {
        (self.height & 0x1f) | (self.intensity << 5)
    }
}

/// PWAV (400 columns) and PWV2 (100 columns): fixed-width monochrome previews in the .DAT.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | column count (400 / 100) |
/// | 0x10 | u16 | `unk_10` (1, set by `initialize_pmai`) |
/// | 0x12 | u16 | `unk_12` (writer stores 0) |
/// | 0x14 | u8 × count | columns ([`PreviewColumn`]) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavePreview {
    /// `PWAV` or `PWV2`.
    pub tag: Tag,
    pub unk_0c: u16,
    pub unk_10: u16,
    pub unk_12: u16,
    pub data: Vec<u8>,
}

impl WavePreview {
    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PWAV, Tag::PWV2])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (unk_0c, count, unk_10, unk_12) = (h.u16()?, h.u16()?, h.u16()?, h.u16()?);
        h.finish()?;
        if count as usize != s.body.len() {
            return h.err("column count does not match body");
        }
        Ok(Self {
            tag: s.tag,
            unk_0c,
            unk_10,
            unk_12,
            data: s.body.clone(),
        })
    }

    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(8);
        header.put_u16(self.unk_0c);
        header.put_u16(self.data.len() as u16);
        header.put_u16(self.unk_10);
        header.put_u16(self.unk_12);
        Section {
            tag: self.tag,
            header,
            body: self.data.clone(),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn column(&self, i: usize) -> Option<PreviewColumn> {
        self.data.get(i).copied().map(PreviewColumn::from_byte)
    }

    pub fn columns(&self) -> impl Iterator<Item = PreviewColumn> + '_ {
        self.data.iter().copied().map(PreviewColumn::from_byte)
    }
}

/// Common header of the variable-length waveforms:
/// `u16 unk_0c | u16 entry_size | u32 entry_count | [4 tag-specific bytes]`.
fn read_grid_header(s: &Section, entry_size: u16, tail_len: usize) -> TagResult<(u16, Vec<u8>)> {
    let mut h = Rd::new(s.tag, &s.header);
    let unk_0c = h.u16()?;
    if h.u16()? != entry_size {
        return h.err("unexpected entry size");
    }
    let count = h.u32()? as usize;
    let tail = h.bytes(tail_len)?.to_vec();
    h.finish()?;
    if count.checked_mul(entry_size.into()) != Some(s.body.len()) {
        return h.err("entry count does not match body");
    }
    Ok((unk_0c, tail))
}

fn write_grid_header(unk_0c: u16, entry_size: u16, body_len: usize, tail: &[u8]) -> Vec<u8> {
    let mut header = Vec::with_capacity(8 + tail.len());
    header.put_u16(unk_0c);
    header.put_u16(entry_size);
    header.put_u32((body_len / usize::from(entry_size)) as u32);
    header.extend_from_slice(tail);
    header
}

/// PWV3: monochrome scrolling ("zoom") waveform, one byte per column.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | entry size (1) |
/// | 0x10 | u32 | column count |
/// | 0x14 | u16 | `columns_per_second` (150, i.e. one column per 1/150 s) |
/// | 0x16 | u16 | `unk_16` (0) |
/// | 0x18 | u8 × count | columns ([`PreviewColumn`]) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveScroll {
    pub unk_0c: u16,
    pub columns_per_second: u16,
    pub unk_16: u16,
    pub data: Vec<u8>,
}

impl WaveScroll {
    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PWV3])?;
        let (unk_0c, tail) = read_grid_header(s, 1, 4)?;
        Ok(Self {
            unk_0c,
            columns_per_second: u16::from_be_bytes([tail[0], tail[1]]),
            unk_16: u16::from_be_bytes([tail[2], tail[3]]),
            data: s.body.clone(),
        })
    }

    pub fn encode(&self) -> Section {
        let mut tail = Vec::with_capacity(4);
        tail.put_u16(self.columns_per_second);
        tail.put_u16(self.unk_16);
        Section {
            tag: Tag::PWV3,
            header: write_grid_header(self.unk_0c, 1, self.data.len(), &tail),
            body: self.data.clone(),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn column(&self, i: usize) -> Option<PreviewColumn> {
        self.data.get(i).copied().map(PreviewColumn::from_byte)
    }

    pub fn columns(&self) -> impl Iterator<Item = PreviewColumn> + '_ {
        self.data.iter().copied().map(PreviewColumn::from_byte)
    }
}

/// One PWV5 column: a big-endian u16 packed as `rrr ggg bbb hhhhh 00`
/// (packing seen in `ExportAnalysisExtendWithCue` @0x1007b83fc; the 3/5 bit widths are also stored in
/// the PWV5 header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColorColumn {
    /// 0..=7
    pub red: u8,
    /// 0..=7
    pub green: u8,
    /// 0..=7
    pub blue: u8,
    /// 0..=31
    pub height: u8,
}

impl ColorColumn {
    pub fn from_raw(v: u16) -> Self {
        Self {
            red: (v >> 13 & 7) as u8,
            green: (v >> 10 & 7) as u8,
            blue: (v >> 7 & 7) as u8,
            height: (v >> 2 & 0x1f) as u8,
        }
    }

    pub fn raw(self) -> u16 {
        u16::from(self.red & 7) << 13
            | u16::from(self.green & 7) << 10
            | u16::from(self.blue & 7) << 7
            | u16::from(self.height & 0x1f) << 2
    }

    /// The colour scaled to 8 bits per channel.
    pub fn rgb8(self) -> [u8; 3] {
        let s = |c: u8| (u16::from(c) * 255 / 7) as u8;
        [s(self.red), s(self.green), s(self.blue)]
    }
}

/// PWV5: colour scrolling waveform, one u16 per column.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | entry size (2) |
/// | 0x10 | u32 | column count |
/// | 0x14 | u16 | `columns_per_second` (150) |
/// | 0x16 | u8 | `color_bits` (3) |
/// | 0x17 | u8 | `height_bits` (5) |
/// | 0x18 | u16 × count | columns ([`ColorColumn`]) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveColorScroll {
    pub unk_0c: u16,
    pub columns_per_second: u16,
    pub color_bits: u8,
    pub height_bits: u8,
    pub data: Vec<u16>,
}

impl WaveColorScroll {
    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PWV5])?;
        let (unk_0c, tail) = read_grid_header(s, 2, 4)?;
        Ok(Self {
            unk_0c,
            columns_per_second: u16::from_be_bytes([tail[0], tail[1]]),
            color_bits: tail[2],
            height_bits: tail[3],
            data: s
                .body
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect(),
        })
    }

    pub fn encode(&self) -> Section {
        let mut tail = Vec::with_capacity(4);
        tail.put_u16(self.columns_per_second);
        tail.put_u8(self.color_bits);
        tail.put_u8(self.height_bits);
        let body: Vec<u8> = self.data.iter().flat_map(|v| v.to_be_bytes()).collect();
        Section {
            tag: Tag::PWV5,
            header: write_grid_header(self.unk_0c, 2, body.len(), &tail),
            body,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn column(&self, i: usize) -> Option<ColorColumn> {
        self.data.get(i).copied().map(ColorColumn::from_raw)
    }

    pub fn columns(&self) -> impl Iterator<Item = ColorColumn> + '_ {
        self.data.iter().copied().map(ColorColumn::from_raw)
    }
}

/// PWV4: colour preview, 1200 columns of 6 bytes.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | entry size (6) |
/// | 0x10 | u32 | column count (1200) |
/// | 0x14 | u32 | `unk_14` (0) |
/// | 0x18 | [u8; 6] × count | columns |
///
/// Column bytes (hypothesis, from the 6-byte point renderer `UiPlayer::deviceUpdateWholeWaveform`
/// @0x10202d088): bytes 3, 4, 5 are three band amplitudes (0..=127) that give both the RGB colour
/// and the height; bytes 0..=2 (0..=255) feed the single-colour ("blue") style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveColorPreview {
    pub unk_0c: u16,
    pub unk_14: u32,
    pub data: Vec<[u8; 6]>,
}

impl WaveColorPreview {
    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PWV4])?;
        let (unk_0c, tail) = read_grid_header(s, 6, 4)?;
        Ok(Self {
            unk_0c,
            unk_14: u32::from_be_bytes(tail[..4].try_into().unwrap()),
            data: s
                .body
                .chunks_exact(6)
                .map(|c| c.try_into().unwrap())
                .collect(),
        })
    }

    pub fn encode(&self) -> Section {
        let body: Vec<u8> = self.data.concat();
        Section {
            tag: Tag::PWV4,
            header: write_grid_header(self.unk_0c, 6, body.len(), &self.unk_14.to_be_bytes()),
            body,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Height 0..=127 and 8-bit colour of column `i`, following the RGB style of the renderer:
    /// height = max(b3, b4, b5); colour = (b3, b4, b5) scaled so the largest band is 255
    /// (white when all are 0). Hypothesis — see the struct docs.
    pub fn rgb_column(&self, i: usize) -> Option<([u8; 3], u8)> {
        let c = self.data.get(i)?;
        let bands = [c[3].min(127), c[4].min(127), c[5].min(127)];
        let max = *bands.iter().max().unwrap();
        if max == 0 {
            return Some(([255, 255, 255], 0));
        }
        let scale = |v: u8| (u16::from(v) * 255 / u16::from(max)) as u8;
        Some(([scale(bands[0]), scale(bands[1]), scale(bands[2])], max))
    }

    /// The largest band value over all columns; the renderer scales heights by 127 / this.
    pub fn max_height(&self) -> u8 {
        self.data
            .iter()
            .map(|c| c[3].max(c[4]).max(c[5]))
            .max()
            .unwrap_or(0)
    }
}

/// PWV6 (3-band preview, 1200 columns) and PWV7 (3-band scrolling waveform, 150 columns/s):
/// 3 bytes per column.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | entry size (3) |
/// | 0x10 | u32 | column count |
/// | 0x14 | u16 | PWV7 only: `columns_per_second` (150) |
/// | 0x16 | u16 | PWV7 only: `unk_16` (0) |
/// | … | [u8; 3] × count | columns; hypothesis: low, mid, high band amplitude |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wave3Band {
    /// `PWV6` or `PWV7`.
    pub tag: Tag,
    pub unk_0c: u16,
    /// PWV7 only: (`columns_per_second`, `unk_16`).
    pub scroll: Option<(u16, u16)>,
    pub data: Vec<[u8; 3]>,
}

impl Wave3Band {
    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PWV6, Tag::PWV7])?;
        let is_scroll = s.tag == Tag::PWV7;
        let (unk_0c, tail) = read_grid_header(s, 3, if is_scroll { 4 } else { 0 })?;
        let scroll = is_scroll.then(|| {
            (
                u16::from_be_bytes([tail[0], tail[1]]),
                u16::from_be_bytes([tail[2], tail[3]]),
            )
        });
        Ok(Self {
            tag: s.tag,
            unk_0c,
            scroll,
            data: s
                .body
                .chunks_exact(3)
                .map(|c| c.try_into().unwrap())
                .collect(),
        })
    }

    pub fn encode(&self) -> Section {
        let body: Vec<u8> = self.data.concat();
        let mut tail = Vec::new();
        if let Some((rate, unk)) = self.scroll {
            tail.put_u16(rate);
            tail.put_u16(unk);
        }
        Section {
            tag: self.tag,
            header: write_grid_header(self.unk_0c, 3, body.len(), &tail),
            body,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Band amplitudes of column `i` (hypothesis: low, mid, high).
    pub fn column(&self, i: usize) -> Option<[u8; 3]> {
        self.data.get(i).copied()
    }
}

/// PWVC: waveform normalisation coefficients (header 0x0e, so the body starts at 0x0e).
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 × 3 | coefficients; default 100/100/100 (table @0x10378f9c8), fixtures mostly 80/100/100 |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveNormalize {
    pub unk_0c: u16,
    pub coefficients: Vec<u16>,
}

impl WaveNormalize {
    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PWVC])?;
        let mut h = Rd::new(s.tag, &s.header);
        let unk_0c = h.u16()?;
        h.finish()?;
        if !s.body.len().is_multiple_of(2) {
            return h.err("odd PWVC body");
        }
        Ok(Self {
            unk_0c,
            coefficients: s
                .body
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect(),
        })
    }

    pub fn encode(&self) -> Section {
        Section {
            tag: Tag::PWVC,
            header: self.unk_0c.to_be_bytes().to_vec(),
            body: self
                .coefficients
                .iter()
                .flat_map(|v| v.to_be_bytes())
                .collect(),
        }
    }
}
