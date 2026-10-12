//! MP3: rekordbox's own frame scanner (`rbxfrm::FileReadMp3::FrameAnalyzer::analyze` @0x102819158)
//! and frame-by-frame decoding through the bundled libmpg123 (`FileReadMp3::decoderOpen`
//! @0x102819b14, `FileReadMp3::decode` @0x10281a528). Spec: `docs/re/decode.md` § MP3.
//!
//! The scanner decides the timeline: every accepted frame adds 1152 (MPEG-1) or 576 (MPEG-2)
//! samples, a leading Xing/Info/VBRI frame becomes 1152 samples of silence, and nothing is trimmed
//! for encoder delay or padding.

use crate::mpg123;
use crate::reader::{DecReader, DecodeState, FrameDecoder, FrameInfo, ReaderInfo};
use crate::{Error, Result};
use std::sync::Arc;

/// `FileReadMp3::M1_SRTbl` @0x1038c5940 / `M2_SRTbl` @0x1038c594c.
const M1_SR: [u32; 3] = [44100, 48000, 32000];
const M2_SR: [u32; 3] = [22050, 24000, 16000];
/// `FileReadMp3::M1_BRTbl` @0x1038c57c0 (rows: layer I, II, III), kbit/s.
const M1_BR: [[i32; 16]; 3] = [
    [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, -1],
    [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, -1],
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, -1],
];
/// `FileReadMp3::M2_BRTbl` @0x1038c5880: all three rows hold the MPEG-2 layer III table.
const M2_BR: [i32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, -1];

/// Result of the frame scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameMap {
    pub frames: Vec<FrameInfo>,
    pub sample_rate: u32,
    pub channels: u16,
    /// 1152 (MPEG-1) or 576 (MPEG-2).
    pub samples_per_frame: u32,
    /// Total samples: every frame in the map, including a leading Xing/Info frame.
    pub total_samples: u32,
    /// Samples before the first audio frame: the leading Xing/Info/VBRI frame, 0 if none.
    pub music_start: u32,
    /// The leading frame carried a Xing (with frame count) or VBRI header.
    pub vbr_header: bool,
    /// The leading frame carried a LAME/ffmpeg "Info" (CBR) header.
    pub info_header: bool,
    /// Bitrate of the first frame in bit/s.
    pub first_bitrate: u32,
}

/// Header fields compared between consecutive frames (byte 0, byte 1 without the protection bit,
/// the sample-rate bits of byte 2 and the channel mode bits of byte 3).
fn signature(b: &[u8]) -> [u8; 4] {
    [b[0], b[1] & 0xfe, b[2] & 0x0c, b[3] & 0xc0]
}

fn is_tag(b: &[u8]) -> bool {
    &b[..3] == b"TAG"
}

/// Frame length in bytes exactly as the binary computes it: `(long)((spf/8 · kbps·1000) / rate)`
/// in double precision, plus the padding bit.
fn frame_bytes(spf: u32, kbps: i32, rate: u32, padding: u8) -> i64 {
    let x = (f64::from(spf >> 3) * (i64::from(kbps) * 1000) as f64) / f64::from(rate);
    x as i64 + i64::from(padding)
}

fn bitrate(mpeg1: bool, layer_bits: u8, index: u8) -> i32 {
    if mpeg1 {
        M1_BR[usize::from(layer_bits ^ 3)][usize::from(index)]
    } else {
        M2_BR[usize::from(index)]
    }
}

fn read4(data: &[u8], pos: i64) -> Option<&[u8]> {
    let p = usize::try_from(pos).ok()?;
    data.get(p..p + 4)
}

/// `FileReadMp3::checkContinuousMpegSync` @0x10281ad64: the candidate first frame and the two
/// frames after it must have matching headers. 0 = ok, 1 = not a frame start, 2 = stop scanning.
fn check_continuous(data: &[u8], pos: i64, sig: [u8; 4], mpeg1: bool, layer_bits: u8, spf: u32, rate: u32) -> u8 {
    let Some(b) = read4(data, pos) else { return 2 };
    if is_tag(b) {
        return 2;
    }
    let plausible = |b: &[u8]| b[0] == 0xff && b[1] >= 0xf0 && (b[1] & 0x06) != 0;
    if !plausible(b) || (b[2] & 0x0c) == 0x0c || b[2] < 0x10 || b[2] >> 4 == 0xf || signature(b) != sig {
        return 1;
    }
    let next = pos + frame_bytes(spf, bitrate(mpeg1, layer_bits, b[2] >> 4), rate, (b[2] >> 1) & 1);
    let Some(b) = read4(data, next) else { return 2 };
    if is_tag(b) {
        return 2;
    }
    if !plausible(b) || (b[2] & 0x0c) == 0x0c || b[2] < 0x10 || b[2] >> 4 == 0xf || signature(b) != sig {
        return 1;
    }
    let next = next + frame_bytes(spf, bitrate(mpeg1, layer_bits, b[2] >> 4), rate, (b[2] >> 1) & 1);
    let Some(b) = read4(data, next) else { return 2 };
    if is_tag(b) {
        return 2;
    }
    if !plausible(b) || (b[2] & 0x0c) == 0x0c || !(0x10..0xf0).contains(&b[2]) || signature(b) != sig {
        return 1;
    }
    0
}

/// `FileReadMp3::checkXingVbri` @0x10281ab60. Returns (xing_or_vbri, info) or None on a short read.
fn check_xing_vbri(data: &[u8], pos: i64, side_info: i64) -> Option<(bool, bool)> {
    let at = usize::try_from(pos + side_info + 4).ok()?;
    let h = data.get(at..at + 12)?;
    let (mut xing, mut info) = (false, false);
    if &h[..4] == b"Xing" {
        if h[7] & 1 != 0 {
            return Some((true, false));
        }
    } else if &h[..4] == b"Info" {
        info = true;
    }
    data.get(at..at + 18)?;
    let vbri_at = if side_info != 32 { usize::try_from(pos + 0x24).ok()? } else { at };
    let v = data.get(vbri_at..vbri_at + 18)?;
    if &v[..4] == b"VBRI" {
        xing = true;
    }
    Some((xing, info))
}

/// Port of `FileReadMp3::FrameAnalyzer::analyze` @0x102819158 over the whole file.
pub fn scan(data: &[u8]) -> Option<FrameMap> {
    let mut frames: Vec<FrameInfo> = Vec::new();
    let mut pos: i64 = 0;
    let mut sample: i64 = 0;
    let mut first = true;
    let mut bad = 0;
    let (mut xing, mut info) = (false, false);
    let mut music_start = 0u32;
    let mut vbr_header = false;
    let mut first_bitrate = 0u32;
    // Fields of the last accepted frame (FrameAnalyzer locals c0/b8/b0/c8).
    let (mut prev_mpeg1, mut prev_layer, mut prev_sr, mut prev_mode) = (false, 0u8, 0u8, 0u8);
    // FrameAnalyzer +0xc/+0x10/+0x28: rate, channels, samples per frame of the last parsed header.
    let (mut rate, mut channels, mut spf) = (0u32, 0u16, 0u32);
    while let Some(b) = read4(data, pos) {
        if &b[..3] == b"ID3" {
            let at = pos as usize + 3;
            match data.get(at..at + 2) {
                Some(v) if {
                    let ver = u32::from(v[1]) + u32::from(v[0]) * 10;
                    (ver & 0x1ffe) == 0x1e || ver == 0x14 || ver == 0x28
                } =>
                {
                    let s = read4(data, pos + 6)?;
                    let size = (u32::from(s[0] & 0x7f) << 21)
                        | (u32::from(s[1] & 0x7f) << 14)
                        | (u32::from(s[2] & 0x7f) << 7)
                        | u32::from(s[3] & 0x7f);
                    pos += i64::from(size) + 10;
                }
                _ => pos += 3,
            }
            continue;
        }
        if !(b[0] == 0xff && b[1] >= 0xf0) {
            pos += 1;
            continue;
        }
        let layer_bits = (b[1] >> 1) & 3;
        let sr_idx = (b[2] >> 2) & 3;
        if layer_bits == 0 || b[2] < 0x10 || b[2] >> 4 == 0xf || sr_idx == 3 {
            pos += 1;
            continue;
        }
        let mode = b[3] >> 6;
        let ch: u16 = if mode == 3 { 1 } else { 2 };
        let mpeg1 = (b[1] >> 3) & 1 != 0;
        rate = if mpeg1 { M1_SR } else { M2_SR }[usize::from(sr_idx)];
        channels = ch;
        spf = if mpeg1 { 1152 } else { 576 };
        let sig = signature(b);
        if first {
            match check_continuous(data, pos, sig, mpeg1, layer_bits, spf, rate) {
                1 => {
                    pos += 1;
                    continue;
                }
                2 => break,
                _ => {}
            }
        }
        let side_info: i64 = match (mpeg1, mode == 3) {
            (true, true) => 17,
            (true, false) => 32,
            (false, true) => 9,
            (false, false) => 17,
        };
        let kbps = bitrate(mpeg1, layer_bits, b[2] >> 4);
        let size = frame_bytes(spf, kbps, rate, (b[2] >> 1) & 1) as u32;
        if first {
            let Some((x, i)) = check_xing_vbri(data, pos, side_info) else { break };
            xing = x;
            info = i;
            frames.push(FrameInfo { offset: pos as u32, pos: sample as i32, samples: 0, size });
            if xing || info {
                music_start = spf;
                vbr_header = xing;
            }
            first = false;
            pos += i64::from(size);
            sample += i64::from(spf);
            first_bitrate = (kbps * 1000) as u32;
            (prev_mpeg1, prev_layer, prev_sr, prev_mode) = (mpeg1, layer_bits, sr_idx, mode);
        } else if mpeg1 == prev_mpeg1 && layer_bits == prev_layer && sr_idx == prev_sr && mode == prev_mode {
            frames.push(FrameInfo { offset: pos as u32, pos: sample as i32, samples: 0, size });
            bad = 0;
            pos += i64::from(size);
            sample += i64::from(spf);
        } else if frames.len() < 4 {
            // Too few frames to trust: start over one byte after the last accepted frame.
            if let Some(last) = frames.last() {
                pos = i64::from(last.offset) + 1;
            }
            sample = 0;
            bad = 0;
            frames.clear();
            first = true;
            xing = false;
            info = false;
            music_start = 0;
            vbr_header = false;
        } else {
            rate = if prev_mpeg1 { M1_SR } else { M2_SR }[usize::from(prev_sr)];
            channels = if prev_mode == 3 { 1 } else { 2 };
            spf = if prev_mpeg1 { 1152 } else { 576 };
            if bad > 3 {
                break;
            }
            bad += 1;
            pos += 1;
        }
    }
    if channels == 0 || frames.is_empty() {
        return None;
    }
    let _ = (xing, spf);
    Some(FrameMap {
        sample_rate: rate,
        channels,
        samples_per_frame: if prev_mpeg1 { 1152 } else { 576 },
        total_samples: sample as u32,
        music_start,
        vbr_header,
        info_header: info,
        first_bitrate,
        frames,
    })
}

/// PVBR index (`vbr_tbl_create` @0x10222c440): entry i (1-based) is the byte offset, relative to
/// the first audio frame, of frame ⌊frames·i/400⌋ − 8, or 0 while frames·i < 3600. rekordbox fills
/// it only for files with a Xing header (VBR); CBR files keep 400 zeros.
pub fn pvbr_index(audio_frames: &[FrameInfo]) -> Vec<u32> {
    let n = audio_frames.len() as u32;
    let base = audio_frames.first().map_or(0, |f| f.offset);
    (1..=400u32)
        .map(|i| {
            let x = n.wrapping_mul(i);
            if x < 0xe10 {
                return 0;
            }
            let target = (x / 400 - 8) as usize;
            audio_frames.get(target).map_or(0, |f| f.offset - base)
        })
        .collect()
}

/// The MP3 frame decoder (`FileReadMp3::decode`).
pub(crate) struct Mp3Decoder {
    data: Arc<Vec<u8>>,
    map: FrameMap,
    mpg: mpg123::Decoder,
    /// `+0xa8`
    next_frame: i32,
    /// `+0xfc`: samples by which mpg123's output lags the frames fed (one frame after a
    /// NEED_MORE on the first feed).
    shift: i32,
    state: DecodeState,
}

impl Mp3Decoder {
    pub fn new(data: Arc<Vec<u8>>) -> Result<Self> {
        let map = scan(&data).ok_or_else(|| Error::Format("no MPEG audio frames".into()))?;
        let mpg = mpg123::Decoder::new(map.sample_rate as i32, map.channels)?;
        let mut d = Self { data, map, mpg, next_frame: -1, shift: 0, state: DecodeState::new() };
        d.prime()?;
        Ok(d)
    }

    fn frame_bytes(&self, f: &FrameInfo) -> &[u8] {
        let size = (f.size & 0x7fff_ffff).min(0x2000) as usize;
        let start = (f.offset as usize).min(self.data.len());
        &self.data[start..(start + size).min(self.data.len())]
    }

    /// The decode loop at the end of `decoderOpen`: feed audio frames until mpg123 reports
    /// anything but NEED_MORE. This consumes mpg123's one-time NEW_FORMAT notice, so later
    /// decodes after `open_feed` return audio straight away.
    fn prime(&mut self) -> Result<()> {
        let frames = self.map.frames.clone();
        for f in frames.iter().filter(|f| i64::from(f.pos) >= i64::from(self.map.music_start)) {
            let bytes = self.frame_bytes(f).to_vec();
            self.mpg.feed(&bytes);
            let ret = self.mpg.decode_frame().ret;
            if ret == mpg123::MPG123_NEED_MORE {
                continue;
            }
            if ret != mpg123::MPG123_OK && self.mpg.errcode() == mpg123::MPG123_BAD_OUTFORMAT {
                return Err(Error::Mpg123("bad output format".into()));
            }
            return Ok(());
        }
        Ok(())
    }
}

impl FrameDecoder for Mp3Decoder {
    fn frames(&self) -> &[FrameInfo] {
        &self.map.frames
    }

    fn info(&self) -> ReaderInfo {
        ReaderInfo {
            sample_rate: self.map.sample_rate,
            channels: self.map.channels,
            bits: 16,
            length: self.map.total_samples,
        }
    }

    fn scale(&self) -> f32 {
        1.0 / 32768.0
    }

    fn next_frame(&mut self) -> &mut i32 {
        &mut self.next_frame
    }

    fn state(&mut self) -> &mut DecodeState {
        &mut self.state
    }

    fn decode(&mut self, pos: i32, first: i32, reset: bool) -> bool {
        self.state.index = 0;
        self.state.len = 0;
        if reset {
            self.mpg.open_feed();
            self.shift = 0;
        }
        let ch = i32::from(self.map.channels);
        let start = self.map.music_start as i32;
        let min_bytes = (self.map.channels as usize) * (self.map.samples_per_frame as usize) * 2;
        let n = self.map.frames.len() as i32;
        let mut i = first.max(0);
        while i < n {
            let f = self.map.frames[i as usize];
            if start <= f.pos {
                let bytes = self.frame_bytes(&f).to_vec();
                self.mpg.feed(&bytes);
                let frame = self.mpg.decode_frame();
                let ret = frame.ret;
                if ret == mpg123::MPG123_NEED_MORE && i + 1 < n {
                    self.shift += self.map.frames[(i + 1) as usize].pos - f.pos;
                    self.next_frame += 1;
                    i += 1;
                    continue;
                }
                let pcm: Vec<i16> = if frame.bytes < min_bytes { Vec::new() } else { frame.pcm.to_vec() };
                if ![mpg123::MPG123_NEW_FORMAT, mpg123::MPG123_NEED_MORE, mpg123::MPG123_OK].contains(&ret)
                    && self.mpg.errcode() == mpg123::MPG123_BAD_OUTFORMAT
                {
                    return false;
                }
                if self.next_frame <= i {
                    self.next_frame += 1;
                    self.state.len = pcm.len() as i32;
                    self.state.index = (self.shift + pos - f.pos) * ch;
                    self.state.pcm = pcm;
                    return self.state.len != 0;
                }
                i += 1;
            } else {
                // A frame before the music start (the Xing/Info frame): silence of its length.
                let span = if i + 1 < n { self.map.frames[(i + 1) as usize].pos - f.pos } else { start - f.pos };
                if self.next_frame <= i {
                    self.next_frame += 1;
                    self.state.index = (pos - f.pos + self.shift) * ch;
                    self.state.len = span.max(0) * ch;
                    self.state.pcm = vec![0; self.state.len as usize];
                    return self.state.len != 0;
                }
                i += 1;
            }
        }
        self.state.len != 0
    }
}

pub(crate) fn open(data: Arc<Vec<u8>>) -> Result<DecReader<Mp3Decoder>> {
    Ok(DecReader::new(Mp3Decoder::new(data)?))
}
