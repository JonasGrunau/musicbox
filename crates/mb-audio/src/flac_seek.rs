//! The FLAC seek table rekordbox stores as PVB2 (`.EXT`). Spec: `docs/re/decode.md` § 7 PVB2.
//!
//! `CBeatAnalyzeThread::createSeekTable` @0x10015b454 → `FlacAnalyzeManager::createSeekTable(400)`
//! @0x101313e1c: target *i* is `i · ⌊total / 400⌋`; libFLAC skips frame by frame, and every target
//! inside a frame gets that frame's first sample, its byte offset from the first frame and its
//! block size. Repeated points (several targets in one frame) are dropped.
//!
//! The frames are found here by walking the frame headers (sync code, CRC-8, consecutive frame or
//! sample numbers); no audio is decoded.

use crate::{Error, Result};
use std::path::Path;

/// One seek point: first sample of a frame, its byte offset from the first frame, its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekPoint {
    pub sample: u64,
    pub byte_offset: u64,
    pub block_samples: u32,
}

/// What PVB2 holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeekTable {
    /// STREAMINFO total samples.
    pub total_samples: u64,
    pub points: Vec<SeekPoint>,
}

/// Number of targets rekordbox asks for.
pub const TARGETS: u64 = 400;

/// A frame: byte position in the file, first sample, block size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    pos: usize,
    first: u64,
    block: u32,
}

/// The seek table of the FLAC file at `path`.
pub fn seek_table(path: &Path) -> Result<SeekTable> {
    let data = std::fs::read(path)?;
    seek_table_from(&data).ok_or_else(|| Error::Unsupported(format!("{}: no FLAC frames", path.display())))
}

/// The seek table of a whole FLAC file in memory.
pub fn seek_table_from(data: &[u8]) -> Option<SeekTable> {
    let (first_frame, info) = metadata(data)?;
    let frames = frames(data, first_frame, &info)?;
    let total = info.total_samples;
    let step = total / TARGETS;
    let mut points: Vec<SeekPoint> = Vec::new();
    let mut f = 0usize;
    for i in 0..TARGETS {
        let target = i * step;
        while f < frames.len() && frames[f].first + u64::from(frames[f].block) <= target {
            f += 1;
        }
        let Some(frame) = frames.get(f) else { break };
        let point = SeekPoint {
            sample: frame.first,
            byte_offset: (frame.pos - first_frame) as u64,
            block_samples: frame.block,
        };
        if points.last() != Some(&point) {
            points.push(point);
        }
    }
    Some(SeekTable { total_samples: total, points })
}

/// The STREAMINFO values the frame walk needs.
struct StreamInfo {
    /// Fixed-block-size streams number frames; the first sample is `number × min_block`.
    min_block: u32,
    total_samples: u64,
}

/// Skips `fLaC` and the metadata blocks; returns the first frame's offset and STREAMINFO.
fn metadata(data: &[u8]) -> Option<(usize, StreamInfo)> {
    // An ID3v2 tag in front of the stream is skipped like libFLAC does.
    let mut pos = 0usize;
    if data.starts_with(b"ID3") && data.len() >= 10 {
        let size = data[6..10].iter().fold(0usize, |a, &b| (a << 7) | usize::from(b & 0x7f));
        pos = 10 + size;
    }
    if data.get(pos..pos + 4)? != b"fLaC" {
        return None;
    }
    pos += 4;
    let mut info = None;
    loop {
        let head = data.get(pos..pos + 4)?;
        let last = head[0] & 0x80 != 0;
        let kind = head[0] & 0x7f;
        let len = (usize::from(head[1]) << 16) | (usize::from(head[2]) << 8) | usize::from(head[3]);
        let body = data.get(pos + 4..pos + 4 + len)?;
        if kind == 0 && body.len() >= 18 {
            let min_block = u32::from(u16::from_be_bytes([body[0], body[1]]));
            let b = &body[10..18];
            let total_samples = ((u64::from(b[3]) & 0x0f) << 32)
                | (u64::from(b[4]) << 24)
                | (u64::from(b[5]) << 16)
                | (u64::from(b[6]) << 8)
                | u64::from(b[7]);
            info = Some(StreamInfo { min_block, total_samples });
        }
        pos += 4 + len;
        if last {
            break;
        }
    }
    Some((pos, info?))
}

/// CRC-8 with polynomial 0x07 (the frame header checksum).
fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

/// A parsed frame header: (variable block size, coded number, block size).
fn header(data: &[u8], pos: usize) -> Option<(bool, u64, u32)> {
    let h = data.get(pos..)?;
    if h.len() < 6 || h[0] != 0xff || h[1] & 0xfe != 0xf8 {
        return None;
    }
    let variable = h[1] & 1 != 0;
    let bs_code = h[2] >> 4;
    let sr_code = h[2] & 0x0f;
    let ch = h[3] >> 4;
    let size_code = (h[3] >> 1) & 7;
    if bs_code == 0 || sr_code == 15 || ch > 10 || size_code == 3 || size_code == 7 || h[3] & 1 != 0 {
        return None;
    }
    // The coded number (UTF-8-like, up to 7 bytes).
    let lead = h[4];
    let (extra, mut num) = match lead {
        0x00..=0x7f => (0, u64::from(lead)),
        0xc0..=0xdf => (1, u64::from(lead & 0x1f)),
        0xe0..=0xef => (2, u64::from(lead & 0x0f)),
        0xf0..=0xf7 => (3, u64::from(lead & 0x07)),
        0xf8..=0xfb => (4, u64::from(lead & 0x03)),
        0xfc..=0xfd => (5, u64::from(lead & 0x01)),
        0xfe => (6, 0),
        _ => return None,
    };
    let mut i = 5usize;
    for _ in 0..extra {
        let b = *h.get(i)?;
        if b & 0xc0 != 0x80 {
            return None;
        }
        num = (num << 6) | u64::from(b & 0x3f);
        i += 1;
    }
    let block = match bs_code {
        1 => 192,
        2..=5 => 576 << (bs_code - 2),
        6 => {
            i += 1;
            u32::from(*h.get(i - 1)?) + 1
        }
        7 => {
            i += 2;
            u32::from(u16::from_be_bytes([*h.get(i - 2)?, *h.get(i - 1)?])) + 1
        }
        _ => 256 << (bs_code - 8),
    };
    match sr_code {
        12 => i += 1,
        13 | 14 => i += 2,
        _ => {}
    }
    let crc = *h.get(i)?;
    if crc8(&h[..i]) != crc {
        return None;
    }
    Some((variable, num, block))
}

/// Every frame from `start` on.
fn frames(data: &[u8], start: usize, info: &StreamInfo) -> Option<Vec<Frame>> {
    let (variable, num, block) = header(data, start)?;
    let first_of = |variable: bool, num: u64| if variable { num } else { num * u64::from(info.min_block) };
    let mut out = vec![Frame { pos: start, first: first_of(variable, num), block }];
    let mut pos = start + 1;
    while pos + 1 < data.len() {
        // The next frame starts at the next sync code with a valid header that continues the
        // numbering.
        let Some(off) = data[pos..].windows(2).position(|w| w[0] == 0xff && w[1] & 0xfe == 0xf8) else {
            break;
        };
        let at = pos + off;
        let prev = *out.last()?;
        match header(data, at) {
            Some((v, n, b)) if v == variable && first_of(v, n) == prev.first + u64::from(prev.block) => {
                out.push(Frame { pos: at, first: first_of(v, n), block: b });
                pos = at + 1;
            }
            _ => pos = at + 1,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_check_value() {
        assert_eq!(crc8(b"123456789"), 0xf4);
    }
}
