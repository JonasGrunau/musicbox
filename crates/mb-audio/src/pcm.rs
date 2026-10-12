//! WAV and AIFF. rekordbox reads 16/24-bit integer PCM with its own parsers
//! (`rbxfrm::FileReadWav::analyze` @0x102838f6c, `FileReadAiff::analyze` @0x102810ad0); every other
//! layout (WAVE_FORMAT_EXTENSIBLE, float, 8/32-bit, unusual AIFC) falls through the factory list to
//! JUCE's readers (`JuceAudioReaderWrapper::fileRead` @0x10283e8ec). Spec: `docs/re/decode.md` § PCM.
//!
//! Both paths produce the same values for integer samples: the sample is placed in the top bits
//! of an `i32`, converted to `f32` and multiplied by 2^-31 (rekordbox stores 0x30000000 = 2^-31 as
//! its scale; JUCE multiplies by `1.0f / 0x7fffffff`, which rounds to the same float). Float WAV is
//! passed through unchanged.

use crate::reader::{Float2, Reader, ReaderInfo};
use crate::{Error, Result};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Signed integer, `bytes` per sample, little or big endian.
    Int { bytes: u8, big_endian: bool },
    /// 8-bit unsigned (WAV only, JUCE path).
    U8,
    Float32 { big_endian: bool },
    Float64 { big_endian: bool },
}

/// Which code in rekordbox reads the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmPath {
    /// `FileReadWav` / `FileReadAiff`.
    Rekordbox,
    /// JUCE's `WavAudioFormat` / `AiffAudioFormat` behind `JuceAudioReaderWrapper`.
    Juce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmLayout {
    pub path: PcmPath,
    pub encoding: Encoding,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits: u16,
    pub data_offset: u64,
    pub frames: u32,
}

impl PcmLayout {
    fn bytes_per_sample(&self) -> usize {
        match self.encoding {
            Encoding::Int { bytes, .. } => usize::from(bytes),
            Encoding::U8 => 1,
            Encoding::Float32 { .. } => 4,
            Encoding::Float64 { .. } => 8,
        }
    }

    fn block_align(&self) -> usize {
        self.bytes_per_sample() * usize::from(self.channels)
    }
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

struct Src<'a> {
    file: &'a File,
    len: u64,
}

impl Src<'_> {
    fn read(&self, pos: u64, n: usize) -> Option<Vec<u8>> {
        if pos.checked_add(n as u64)? > self.len {
            return None;
        }
        let mut buf = vec![0; n];
        self.file.read_exact_at(&mut buf, pos).ok()?;
        Some(buf)
    }
}

/// `FileReadWav::analyze`: RIFF/WAVE, the first `fmt ` chunk must be PCM (tag 1) with 16 or 24 bits
/// and at most two channels, then the first `data` chunk after it. Chunks are walked with
/// `size + 8` steps (no RIFF pad byte), exactly like the binary.
fn wav_rekordbox(src: &Src) -> Option<PcmLayout> {
    let h = src.read(0, 12)?;
    if &h[..4] != b"RIFF" || &h[8..12] != b"WAVE" {
        return None;
    }
    let mut at: u64 = 12;
    let (channels, rate, bits) = loop {
        let c = src.read(at, 8)?;
        let size = u64::from(le32(&c[4..]));
        if &c[..4] == b"fmt " {
            let f = src.read(at + 8, 18)?;
            let (tag, channels, rate, bits) = (le16(&f[0..]), le16(&f[2..]), le32(&f[4..]), le16(&f[14..]));
            if channels > 2 || tag != 1 || (bits | 8) != 24 {
                return None;
            }
            at += size + 8;
            break (channels, rate, bits);
        }
        at += size + 8;
    };
    loop {
        let c = src.read(at, 8)?;
        let size = le32(&c[4..]);
        if &c[..4] == b"data" {
            let bytes = u32::from(bits >> 3);
            if channels == 0 || bytes == 0 {
                return None;
            }
            return Some(PcmLayout {
                path: PcmPath::Rekordbox,
                encoding: Encoding::Int { bytes: bytes as u8, big_endian: false },
                channels,
                sample_rate: rate,
                bits,
                data_offset: at + 8,
                frames: size / (bytes * u32::from(channels)),
            });
        }
        at += u64::from(size) + 8;
    }
}

/// JUCE `WavAudioFormatReader`: PCM 8/16/24/32, IEEE float 32/64 and WAVE_FORMAT_EXTENSIBLE with
/// either sub-format; chunks are word aligned.
fn wav_juce(src: &Src) -> Option<PcmLayout> {
    let h = src.read(0, 12)?;
    if &h[..4] != b"RIFF" || &h[8..12] != b"WAVE" {
        return None;
    }
    let mut at: u64 = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    while let Some(c) = src.read(at, 8) {
        let size = u64::from(le32(&c[4..]));
        if &c[..4] == b"fmt " {
            let f = src.read(at + 8, size.clamp(16, 40) as usize)?;
            let mut tag = le16(&f[0..]);
            if tag == 0xfffe && f.len() >= 40 {
                tag = le16(&f[24..]);
            }
            fmt = Some((tag, le16(&f[2..]), le32(&f[4..]), le16(&f[14..])));
        } else if &c[..4] == b"data" {
            let (tag, channels, rate, bits) = fmt?;
            let encoding = match (tag, bits) {
                (1, 8) => Encoding::U8,
                (1, 16 | 24 | 32) => Encoding::Int { bytes: (bits / 8) as u8, big_endian: false },
                (3, 32) => Encoding::Float32 { big_endian: false },
                (3, 64) => Encoding::Float64 { big_endian: false },
                _ => return None,
            };
            if channels == 0 {
                return None;
            }
            let mut layout = PcmLayout {
                path: PcmPath::Juce,
                encoding,
                channels,
                sample_rate: rate,
                bits,
                data_offset: at + 8,
                frames: 0,
            };
            layout.frames = (size / layout.block_align() as u64) as u32;
            return Some(layout);
        }
        at += (size + 1) & !1;
        at += 8;
    }
    None
}

/// IEEE 754 80-bit extended → f64, as `FileReadAiff::getLongDoubleBe` @0x1028115f4.
fn extended_be(b: &[u8]) -> f64 {
    let exp = (u32::from(b[0] & 0x7f) << 8) | u32::from(b[1]);
    let hi = be32(&b[2..]);
    let lo = be32(&b[6..]);
    let sign = b[0] & 0x80 != 0;
    let v = if exp == 0 && hi == 0 && lo == 0 {
        0.0
    } else if exp == 0x7fff {
        f64::INFINITY
    } else {
        let hi = f64::from(hi);
        let lo = f64::from(lo);
        let mut v = if exp < 0x401e { hi / f64::from(1u32 << ((0x401e - exp) & 31)) } else { hi * f64::from(1u32 << ((exp - 0x401e) & 31)) };
        if exp > 0x403d {
            v += lo * f64::from(1u32 << ((exp - 0x403e) & 31));
        } else {
            v += lo / f64::from(1u32 << ((0x403e - exp) & 31));
        }
        v
    };
    if sign { -v } else { v }
}

/// `FileReadAiff::analyze`: FORM AIFF/AIFC, COMM with 1–2 channels and 16/24 bits (AIFC: NONE,
/// twos, in24 big endian; sowt little endian), then SSND. Sample data starts 16 bytes into SSND
/// (the offset field is ignored); a zero byte where a chunk id should start is skipped.
fn aiff_rekordbox(src: &Src) -> Option<PcmLayout> {
    let h = src.read(0, 12)?;
    let aifc = match &h[8..12] {
        b"AIFF" => false,
        b"AIFC" => true,
        _ => return None,
    };
    if &h[..4] != b"FORM" {
        return None;
    }
    let mut at: u64 = 12;
    let (channels, rate, bits, big_endian) = loop {
        let c = src.read(at, 8)?;
        if c[0] == 0 {
            at += 1;
            continue;
        }
        let size = u64::from(be32(&c[4..]));
        if &c[..4] == b"COMM" {
            let f = src.read(at + 8, 18)?;
            let channels = be16(&f[0..]);
            let bits = be16(&f[6..]);
            let rate = extended_be(&f[8..18]) as i64 as u32;
            if !(1..=2).contains(&channels) || (bits | 8) != 24 {
                return None;
            }
            let mut big_endian = true;
            if aifc && size > 0x15 {
                let id = src.read(at + 8 + 18, 4)?;
                match &id[..] {
                    b"NONE" | b"twos" | b"in24" => {}
                    b"sowt" => big_endian = false,
                    _ => return None,
                }
            }
            at += size + 8;
            break (channels, rate, bits, big_endian);
        }
        at += size + 8;
    };
    loop {
        let c = src.read(at, 8)?;
        if c[0] == 0 {
            at += 1;
            continue;
        }
        let size = u64::from(be32(&c[4..]));
        if &c[..4] == b"SSND" {
            if size == 0 {
                return None;
            }
            let bytes = u64::from(bits >> 3);
            let payload = if src.len < size + at + 8 { src.len.saturating_sub(at + 16) } else { size - 8 };
            return Some(PcmLayout {
                path: PcmPath::Rekordbox,
                encoding: Encoding::Int { bytes: bytes as u8, big_endian },
                channels,
                sample_rate: rate,
                bits,
                data_offset: at + 16,
                frames: (payload / (bytes * u64::from(channels))) as u32,
            });
        }
        at += size + 8;
    }
}

/// JUCE `AiffAudioFormatReader` for what rekordbox's own reader rejects (8/32-bit, float AIFC).
fn aiff_juce(src: &Src) -> Option<PcmLayout> {
    let h = src.read(0, 12)?;
    let aifc = &h[8..12] == b"AIFC";
    if &h[..4] != b"FORM" || !(aifc || &h[8..12] == b"AIFF") {
        return None;
    }
    let mut at: u64 = 12;
    let mut comm: Option<(u16, u16, u32, Encoding)> = None;
    while let Some(c) = src.read(at, 8) {
        let size = u64::from(be32(&c[4..]));
        if &c[..4] == b"COMM" {
            let f = src.read(at + 8, 18)?;
            let (channels, bits) = (be16(&f[0..]), be16(&f[6..]));
            let rate = extended_be(&f[8..18]) as u32;
            let mut enc = Encoding::Int { bytes: bits.div_ceil(8) as u8, big_endian: true };
            if aifc && size >= 22 {
                let id = src.read(at + 26, 4)?;
                enc = match &id[..] {
                    b"sowt" => Encoding::Int { bytes: bits.div_ceil(8) as u8, big_endian: false },
                    b"fl32" | b"FL32" => Encoding::Float32 { big_endian: true },
                    b"fl64" | b"FL64" => Encoding::Float64 { big_endian: true },
                    _ => enc,
                };
            }
            comm = Some((channels, bits, rate, enc));
        } else if &c[..4] == b"SSND" {
            let (channels, bits, rate, encoding) = comm?;
            let off = src.read(at + 8, 4).map(|o| u64::from(be32(&o))).unwrap_or(0);
            let mut layout = PcmLayout {
                path: PcmPath::Juce,
                encoding,
                channels,
                sample_rate: rate,
                bits,
                data_offset: at + 16 + off,
                frames: 0,
            };
            if channels == 0 || layout.bytes_per_sample() == 0 {
                return None;
            }
            layout.frames = (size.saturating_sub(8 + off) / layout.block_align() as u64) as u32;
            return Some(layout);
        }
        at += ((size + 1) & !1) + 8;
    }
    None
}

/// Decides which reader rekordbox would use for this file.
pub fn probe(path: &Path, ext: &str) -> Result<PcmLayout> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let src = Src { file: &file, len };
    let layout = match ext {
        "wav" => wav_rekordbox(&src).or_else(|| wav_juce(&src)),
        _ => aiff_rekordbox(&src).or_else(|| aiff_juce(&src)),
    };
    layout.ok_or_else(|| Error::Format(format!("unsupported {ext} layout")))
}

/// Converts one sample the way rekordbox / JUCE do (see the module docs).
#[inline]
fn sample(enc: Encoding, b: &[u8]) -> f32 {
    const SCALE: f32 = 1.0 / 2_147_483_648.0;
    match enc {
        Encoding::Int { bytes, big_endian } => {
            let n = usize::from(bytes).min(4);
            let skip = usize::from(bytes) - n;
            let mut v: u32 = 0;
            for k in 0..n {
                let byte = if big_endian { b[k] } else { b[skip + n - 1 - k] };
                v = (v << 8) | u32::from(byte);
            }
            ((v << (32 - 8 * n)) as i32) as f32 * SCALE
        }
        Encoding::U8 => (((i32::from(b[0]) - 128) << 24) as f32) * SCALE,
        Encoding::Float32 { big_endian } => {
            let a = [b[0], b[1], b[2], b[3]];
            if big_endian { f32::from_be_bytes(a) } else { f32::from_le_bytes(a) }
        }
        Encoding::Float64 { big_endian } => {
            let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
            (if big_endian { f64::from_be_bytes(a) } else { f64::from_le_bytes(a) }) as f32
        }
    }
}

pub struct PcmReader {
    file: File,
    layout: PcmLayout,
    buf: Vec<u8>,
}

impl PcmReader {
    pub fn open(path: &Path, ext: &str) -> Result<Self> {
        let layout = probe(path, ext)?;
        Ok(Self { file: File::open(path)?, layout, buf: Vec::new() })
    }

    pub fn layout(&self) -> &PcmLayout {
        &self.layout
    }
}

impl Reader for PcmReader {
    fn info(&self) -> ReaderInfo {
        ReaderInfo {
            sample_rate: self.layout.sample_rate,
            channels: self.layout.channels,
            bits: self.layout.bits,
            length: self.layout.frames,
        }
    }

    fn read(&mut self, pos: i64, out: &mut [Float2]) {
        out.fill([0.0; 2]);
        let total = i64::from(self.layout.frames);
        let start = pos.max(0);
        let end = (pos + out.len() as i64).min(total);
        if start >= end {
            return;
        }
        let ba = self.layout.block_align();
        let bps = self.layout.bytes_per_sample();
        let last = (usize::from(self.layout.channels) - 1) * bps;
        self.buf.resize((end - start) as usize * ba, 0);
        let off = self.layout.data_offset + start as u64 * ba as u64;
        // A truncated data chunk reads as silence past the end of the file.
        let got = read_full(&self.file, &mut self.buf, off);
        let frames = got / ba;
        let enc = self.layout.encoding;
        for (i, f) in self.buf[..frames * ba].chunks_exact(ba).enumerate() {
            out[(start - pos) as usize + i] = [sample(enc, &f[..bps]), sample(enc, &f[last..last + bps])];
        }
    }
}

fn read_full(file: &File, buf: &mut [u8], mut off: u64) -> usize {
    let mut done = 0;
    while done < buf.len() {
        match file.read_at(&mut buf[done..], off) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                done += n;
                off += n as u64;
            }
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_scaling_is_exact_power_of_two() {
        let enc16 = Encoding::Int { bytes: 2, big_endian: false };
        assert_eq!(sample(enc16, &(-32768i16).to_le_bytes()), -1.0);
        assert_eq!(sample(enc16, &1i16.to_le_bytes()), 1.0 / 32768.0);
        let enc24 = Encoding::Int { bytes: 3, big_endian: true };
        assert_eq!(sample(enc24, &[0x7f, 0xff, 0xff]), 8_388_607.0 / 8_388_608.0);
        assert_eq!(sample(enc24, &[0x80, 0x00, 0x00]), -1.0);
        assert_eq!(sample(Encoding::U8, &[0]), -1.0);
    }

    #[test]
    fn extended_rates() {
        let r = |b: [u8; 10]| extended_be(&b) as i64;
        assert_eq!(r([0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]), 44100);
        assert_eq!(r([0x40, 0x0e, 0xbb, 0x80, 0, 0, 0, 0, 0, 0]), 48000);
        assert_eq!(r([0x40, 0x0f, 0xbb, 0x80, 0, 0, 0, 0, 0, 0]), 96000);
    }
}
