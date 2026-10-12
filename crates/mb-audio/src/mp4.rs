//! Minimal MP4 reader for the sample tables, following rekordbox's own parsers
//! (`rbxfrm::FileReadMp4::getFrameMap` @0x10281cca8 for AAC, `Mp4AtomParser::getEntireMp4FrameMap`
//! @0x10284292c for ALAC). Both walk the atoms in file order, take the first `stts`, `stsc`,
//! `stco`/`co64` and `stsz`, and place packet *i* at the sum of the `stts` deltas before it. Edit
//! lists (`elst`), `iTunSMPB` and the packet-table priming are ignored, so the timeline starts at
//! the first decoded sample of the first packet. Spec: `docs/re/decode.md` § MP4.

use crate::reader::FrameInfo;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Aac,
    Alac,
    Other([u8; 4]),
}

#[derive(Debug, Clone)]
pub struct Mp4Track {
    pub codec: Codec,
    /// Sample entry channel count and sample size.
    pub channels: u16,
    pub sample_size: u16,
    /// Rate from the sample entry (16.16, integer part).
    pub entry_rate: u32,
    /// AAC: rate and channel configuration from the AudioSpecificConfig; SBR signalled.
    pub asc_rate: Option<u32>,
    pub sbr: bool,
    /// ALAC magic cookie fields.
    pub alac_bits: Option<u8>,
    pub alac_rate: Option<u32>,
    pub frames: Vec<FrameInfo>,
    /// Sum of all packet durations (rekordbox's track length).
    pub total: u64,
}

struct Atom {
    kind: [u8; 4],
    /// Offset of the payload and its length.
    body: usize,
    len: usize,
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}
fn be32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
fn be64(d: &[u8], at: usize) -> Option<u64> {
    d.get(at..at + 8).map(|b| u64::from_be_bytes(b.try_into().unwrap()))
}

/// Flattened atom list in file order, descending into the containers that lead to the sample
/// tables and into sample entries.
fn atoms(d: &[u8], start: usize, end: usize, out: &mut Vec<Atom>, depth: u32) {
    let mut at = start;
    while at + 8 <= end && depth < 12 {
        let Some(size32) = be32(d, at) else { break };
        let kind: [u8; 4] = d[at + 4..at + 8].try_into().unwrap();
        let (hdr, size) = match size32 {
            0 => (8, end - at),
            1 => match be64(d, at + 8) {
                Some(s) => (16, s as usize),
                None => break,
            },
            s => (8, s as usize),
        };
        if size < hdr || at + size > end {
            break;
        }
        let body = at + hdr;
        let len = size - hdr;
        out.push(Atom { kind, body, len });
        match &kind {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf" | b"udta" => {
                atoms(d, body, body + len, out, depth + 1)
            }
            // stsd: version/flags + entry count, then sample entries.
            b"stsd" => atoms(d, body + 8, body + len, out, depth + 1),
            // Audio sample entries: 28 bytes of fields (44 / 64 for QuickTime sound description
            // versions 1 / 2) before their child atoms.
            b"mp4a" | b"alac" if len > 28 => {
                let skip = match be16(d, body + 8) {
                    Some(1) => 44,
                    Some(2) => 64,
                    _ => 28,
                };
                atoms(d, body + skip, body + len, out, depth + 1)
            }
            // esds is a full box: skip version/flags.
            _ => {}
        }
        at += size;
    }
}

/// Reads an MPEG-4 descriptor length (up to four 7-bit groups).
fn desc_len(d: &[u8], at: &mut usize) -> Option<usize> {
    let mut n = 0usize;
    for _ in 0..4 {
        let b = *d.get(*at)?;
        *at += 1;
        n = (n << 7) | usize::from(b & 0x7f);
        if b & 0x80 == 0 {
            break;
        }
    }
    Some(n)
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}
impl Bits<'_> {
    fn get(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            let byte = *self.d.get(self.pos / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Some(v)
    }
}

/// AudioSpecificConfig → (rate, sbr). HE-AAC (object type 5/29 or the 0x2b7 extension) reports
/// the SBR output rate, as rekordbox's `readAudioSpecificConfig` does before `decoderOpen`
/// halves it again when the file format reports the core rate.
fn parse_asc(asc: &[u8]) -> Option<(u32, bool)> {
    let mut b = Bits { d: asc, pos: 0 };
    let read_rate = |b: &mut Bits| -> Option<u32> {
        let i = b.get(4)?;
        if i == 15 { b.get(24) } else { AAC_RATES.get(i as usize).copied() }
    };
    let mut aot = b.get(5)?;
    if aot == 31 {
        aot = 32 + b.get(6)?;
    }
    let rate = read_rate(&mut b)?;
    let _channels = b.get(4)?;
    if aot == 5 || aot == 29 {
        let ext = read_rate(&mut b)?;
        return Some((ext, true));
    }
    // Backward-compatible explicit SBR signalling after the GASpecificConfig (AAC-LC: 3 bits).
    if aot == 2 && asc.len() * 8 > b.pos + 3 + 11 + 5 {
        b.get(3)?;
        if b.get(11)? == 0x2b7 && b.get(5)? == 5 && b.get(1)? == 1 {
            return Some((read_rate(&mut b)?, true));
        }
    }
    Some((rate, false))
}

pub fn parse(d: &[u8]) -> Result<Mp4Track> {
    let mut list = Vec::new();
    atoms(d, 0, d.len(), &mut list, 0);
    let find = |k: &[u8; 4]| list.iter().find(|a| &a.kind == k);
    let bad = |m: &str| Error::Format(format!("mp4: {m}"));
    find(b"mdat").ok_or_else(|| bad("no mdat"))?;
    let entry = list
        .iter()
        .find(|a| matches!(&a.kind, b"mp4a" | b"alac") && a.len >= 28)
        .ok_or_else(|| bad("no audio sample entry"))?;
    let codec = match (&entry.kind, find(b"esds").is_some()) {
        (b"mp4a", true) => Codec::Aac,
        (b"alac", _) => Codec::Alac,
        (k, _) => Codec::Other(*k),
    };
    let e = entry.body;
    let channels = be16(d, e + 16).unwrap_or(0);
    let sample_size = be16(d, e + 18).unwrap_or(0);
    let entry_rate = be32(d, e + 24).unwrap_or(0) >> 16;
    let mut track = Mp4Track {
        codec,
        channels,
        sample_size,
        entry_rate,
        asc_rate: None,
        sbr: false,
        alac_bits: None,
        alac_rate: None,
        frames: Vec::new(),
        total: 0,
    };
    if let Some(esds) = find(b"esds") {
        // ES_Descriptor (0x03) → DecoderConfigDescriptor (0x04) → DecoderSpecificInfo (0x05).
        let end = esds.body + esds.len;
        let mut at = esds.body + 4;
        while at < end {
            let tag = d[at];
            at += 1;
            let Some(n) = desc_len(d, &mut at) else { break };
            match tag {
                0x03 => {
                    let flags = d.get(at + 2).copied().unwrap_or(0);
                    at += 3;
                    if flags & 0x80 != 0 {
                        at += 2;
                    }
                    if flags & 0x40 != 0 {
                        at += 1 + usize::from(d.get(at).copied().unwrap_or(0));
                    }
                    if flags & 0x20 != 0 {
                        at += 2;
                    }
                }
                0x04 => at += 13,
                0x05 => {
                    if let Some((rate, sbr)) = d.get(at..at + n).and_then(parse_asc) {
                        track.asc_rate = Some(rate);
                        track.sbr = sbr;
                    }
                    break;
                }
                _ => at += n,
            }
        }
    }
    if let Some(cookie) = list.iter().find(|a| &a.kind == b"alac" && a.len >= 28 && a.body != entry.body) {
        // ALACSpecificConfig after version/flags: frameLength u32, compatibleVersion u8, bitDepth u8, …
        track.alac_bits = d.get(cookie.body + 9).copied();
        track.alac_rate = be32(d, cookie.body + 4 + 20);
    }
    // Sample tables.
    let stts = find(b"stts").ok_or_else(|| bad("no stts"))?;
    let stsz = find(b"stsz").ok_or_else(|| bad("no stsz"))?;
    let stsc = find(b"stsc").ok_or_else(|| bad("no stsc"))?;
    let (chunk_offsets, wide) = match (find(b"stco"), find(b"co64")) {
        (Some(a), _) => (a, false),
        (None, Some(a)) => (a, true),
        _ => return Err(bad("no stco")),
    };
    let mut deltas: Vec<(u32, u32)> = Vec::new();
    let n = be32(d, stts.body + 4).unwrap_or(0) as usize;
    for i in 0..n {
        let at = stts.body + 8 + 8 * i;
        if at + 8 > stts.body + stts.len {
            break;
        }
        deltas.push((be32(d, at).unwrap(), be32(d, at + 4).unwrap()));
    }
    let uniform = be32(d, stsz.body + 4).unwrap_or(0);
    let count = be32(d, stsz.body + 8).unwrap_or(0) as usize;
    let size_of = |i: usize| if uniform != 0 { Some(uniform) } else { be32(d, stsz.body + 12 + 4 * i) };
    let mut chunks: Vec<u64> = Vec::new();
    let nc = be32(d, chunk_offsets.body + 4).unwrap_or(0) as usize;
    for i in 0..nc {
        let v = if wide { be64(d, chunk_offsets.body + 8 + 8 * i) } else { be32(d, chunk_offsets.body + 8 + 4 * i).map(u64::from) };
        match v {
            Some(v) => chunks.push(v),
            None => break,
        }
    }
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let ns = be32(d, stsc.body + 4).unwrap_or(0) as usize;
    for i in 0..ns {
        let at = stsc.body + 8 + 12 * i;
        match (be32(d, at), be32(d, at + 4)) {
            (Some(first), Some(per)) => runs.push((first, per)),
            _ => break,
        }
    }
    // Walk samples.
    let mut delta_iter = deltas.iter().flat_map(|&(c, dlt)| std::iter::repeat_n(dlt, c as usize));
    let mut pos: u64 = 0;
    let (mut run, mut chunk, mut in_chunk) = (0usize, 0usize, 0u32);
    let mut offset = chunks.first().copied().unwrap_or(0);
    for i in 0..count {
        let dur = delta_iter.next().ok_or_else(|| bad("stts shorter than stsz"))?;
        let size = size_of(i).ok_or_else(|| bad("short stsz"))?;
        track.frames.push(FrameInfo { offset: offset as u32, pos: pos as i32, samples: dur, size });
        pos += u64::from(dur);
        offset += u64::from(size);
        in_chunk += 1;
        let per = runs.get(run).map_or(u32::MAX, |r| r.1);
        if in_chunk >= per {
            in_chunk = 0;
            chunk += 1;
            if runs.get(run + 1).is_some_and(|next| chunk as u32 + 1 >= next.0) {
                run += 1;
            }
            offset = chunks.get(chunk).copied().unwrap_or(offset);
        }
    }
    track.total = pos;
    if track.frames.is_empty() {
        return Err(bad("no packets"));
    }
    Ok(track)
}
