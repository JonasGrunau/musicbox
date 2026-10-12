//! FLAC. rekordbox decodes with its statically linked libFLAC (`rbxfrm::FileReadFlac` @0x1028144b8):
//! length = STREAMINFO total samples, value = `(float)sample · 1/2^(bits−1)`, mono duplicated.
//! FLAC is lossless, so any conforming decoder gives the same integers; symphonia's decoder
//! left-aligns them to 32 bits, and `(float)(s << (32−bits)) · 2^-31` is the same float.
//! Spec: `docs/re/decode.md` § FLAC.

use crate::reader::{Float2, Reader, ReaderInfo};
use crate::{Error, Result};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub struct FlacReader {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    info: ReaderInfo,
    /// Decoded samples of the current packet and the position of the first one.
    block: Vec<Float2>,
    block_start: i64,
}

fn err(e: symphonia::core::errors::Error) -> Error {
    Error::Format(format!("flac: {e}"))
}

impl FlacReader {
    pub fn open(path: &Path) -> Result<Self> {
        let mss = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(err)?;
        let format = probed.format;
        let track = format.default_track().ok_or_else(|| Error::Format("flac: no track".into()))?;
        let p = &track.codec_params;
        let channels = p.channels.map(|c| c.count() as u16).unwrap_or(0);
        let total = p.n_frames.unwrap_or(0);
        // FileReadFlac::analyze rejects 0 or >2 channels and an unknown or empty length.
        if !(1..=2).contains(&channels) || total == 0 || (total as i32) <= 0 {
            return Err(Error::Format("flac: unsupported stream".into()));
        }
        let info = ReaderInfo {
            sample_rate: p.sample_rate.unwrap_or(0),
            channels,
            bits: p.bits_per_sample.unwrap_or(16) as u16,
            length: total as u32,
        };
        let decoder = symphonia::default::get_codecs().make(p, &DecoderOptions::default()).map_err(err)?;
        let track = track.id;
        Ok(Self { format, decoder, track, info, block: Vec::new(), block_start: 0 })
    }

    /// Decodes the next packet into `block`; false at the end of the stream.
    fn next_block(&mut self) -> bool {
        loop {
            let Ok(packet) = self.format.next_packet() else { return false };
            if packet.track_id() != self.track {
                continue;
            }
            let ts = packet.ts() as i64;
            let Ok(decoded) = self.decoder.decode(&packet) else { continue };
            self.block.clear();
            match decoded {
                AudioBufferRef::S32(buf) => {
                    let l = buf.chan(0);
                    let r = buf.chan(buf.spec().channels.count() - 1);
                    const SCALE: f32 = 1.0 / 2_147_483_648.0;
                    self.block.extend(l.iter().zip(r).map(|(&a, &b)| [a as f32 * SCALE, b as f32 * SCALE]));
                }
                _ => return false,
            }
            self.block_start = ts;
            return true;
        }
    }

    fn seek(&mut self, pos: i64) {
        let to = SeekTo::TimeStamp { ts: pos.max(0) as u64, track_id: self.track };
        if self.format.seek(SeekMode::Accurate, to).is_ok() {
            self.decoder.reset();
        }
        self.block.clear();
        self.block_start = 0;
    }
}

impl Reader for FlacReader {
    fn info(&self) -> ReaderInfo {
        self.info
    }

    fn read(&mut self, pos: i64, out: &mut [Float2]) {
        out.fill([0.0; 2]);
        let total = i64::from(self.info.length);
        let mut p = pos.max(0);
        let end = (pos + out.len() as i64).min(total);
        while p < end {
            let block_end = self.block_start + self.block.len() as i64;
            if p < self.block_start || p >= block_end {
                // Sequential reads continue with the next packet; anything else seeks.
                let sequential = !self.block.is_empty() && p >= block_end && p - block_end < 65536;
                if !sequential {
                    self.seek(p);
                }
                let mut found = false;
                while self.next_block() {
                    if p < self.block_start + self.block.len() as i64 {
                        found = p >= self.block_start;
                        break;
                    }
                }
                if !found {
                    return;
                }
                continue;
            }
            let n = (block_end.min(end) - p) as usize;
            let src = (p - self.block_start) as usize;
            let dst = (p - pos) as usize;
            out[dst..dst + n].copy_from_slice(&self.block[src..src + n]);
            p += n as i64;
        }
    }
}
