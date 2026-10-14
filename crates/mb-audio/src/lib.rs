//! Decode spine: PCM exactly as rekordbox 7.2.19 sees it, for preview playback and (M6) analysis.
//! Spec: `docs/re/decode.md`.
//!
//! [`open`] picks the reader rekordbox would pick for the file (by extension, then by whether its
//! own parser accepts the layout), and wraps it in rekordbox's sample-rate converter when the file
//! is neither 44.1 nor 48 kHz ([`analysis_rate`]). The decoder yields interleaved stereo `f32`
//! (mono is duplicated) on rekordbox's timeline: nothing is trimmed (MP3 encoder delay, AAC
//! priming) and a leading MP3 Xing/Info frame plays as 1152 samples of silence.
//!
//! | Format | rekordbox reader | here |
//! |---|---|---|
//! | MP3 | `FileReadMp3` + bundled libmpg123 | same scan + the same libmpg123 via `dlopen` |
//! | AAC (.m4a/.mp4) | `FileReadMp4` + AudioToolbox | same AudioToolbox calls |
//! | ALAC (.m4a) | `FileReadAlac` + bundled ALACDecoder | AudioToolbox ALAC (lossless) |
//! | FLAC | `FileReadFlac` + libFLAC | symphonia (lossless) |
//! | WAV/AIFF 16/24-bit | `FileReadWav` / `FileReadAiff` | port |
//! | other WAV/AIFF | JUCE readers | port of their scaling |

pub mod coreaudio;
pub mod flac;
pub mod flac_seek;
pub mod mp3;
pub mod mp4;
pub mod mpg123;
pub mod pcm;
mod reader;
pub mod resample;
pub mod timeline;

pub use reader::{Float2, FrameInfo, Reader, ReaderInfo};

use std::path::Path;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Format(String),
    #[error("libmpg123: {0}")]
    Mpg123(String),
    #[error("unsupported file: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp3,
    Aac,
    Alac,
    Flac,
    Wav,
    Aiff,
}

/// The rekordbox reader class that handles the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    FileReadMp3,
    FileReadMp4,
    FileReadAlac,
    FileReadFlac,
    FileReadWav,
    FileReadAiff,
    /// `JuceAudioReaderWrapper` (WAV/AIFF layouts rekordbox's own readers reject).
    Juce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub format: Format,
    pub backend: Backend,
    /// Sample rate of the file.
    pub file_rate: u32,
    /// Rate of the decoded stream: the analysis rate (44100 or 48000).
    pub sample_rate: u32,
    /// Channels in the file (output is always stereo).
    pub channels: u16,
    pub bits: u16,
    /// Length at the file rate (rekordbox `FileReader::getLength`).
    pub file_samples: u64,
    /// Length of the decoded stream at `sample_rate`.
    pub total_samples: u64,
}

impl StreamInfo {
    pub fn resampled(&self) -> bool {
        self.file_rate != self.sample_rate
    }

    pub fn duration_secs(&self) -> f64 {
        self.total_samples as f64 / f64::from(self.sample_rate)
    }
}

/// The rate rekordbox analyses (and here decodes) at: 44.1 and 48 kHz natively, other multiples of
/// 8 kHz at 48 kHz, everything else at 44.1 kHz (`CBeatAnalyzeThread::analyze` @0x1001528c4 tests
/// divisibility by 8000 with a multiply-rotate).
pub fn analysis_rate(file_rate: u32) -> u32 {
    match file_rate {
        44100 | 48000 => file_rate,
        r if r % 8000 == 0 => 48000,
        _ => 44100,
    }
}

/// The analysis input quantisation (`PcmReaderTemplate<Float2>::getStreamAt(int, short*, short*,
/// int)` @0x102853e3c): ×32768, clamp to [−32768, 32767] (NaN → −32768), truncate toward zero.
/// The analysis then works on `(float)` of these integers.
pub fn analysis_i16(x: f32) -> i16 {
    let v = x * 32768.0;
    let v = if v > 32767.0 {
        32767.0
    } else if v >= -32768.0 {
        v
    } else {
        -32768.0
    };
    v as i16
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// A decoder on rekordbox's timeline.
pub struct Decoder {
    reader: Box<dyn Reader>,
    info: StreamInfo,
    pos: u64,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").field("info", &self.info).field("pos", &self.pos).finish()
    }
}

/// Opens `path` with the reader rekordbox would use.
pub fn open(path: impl AsRef<Path>) -> Result<Decoder> {
    Decoder::open(path.as_ref())
}

fn base_reader(path: &Path) -> Result<(Box<dyn Reader>, Format, Backend)> {
    let ext = extension(path);
    match ext.as_str() {
        "mp3" => {
            let data = Arc::new(std::fs::read(path)?);
            Ok((Box::new(mp3::open(data)?), Format::Mp3, Backend::FileReadMp3))
        }
        "m4a" | "mp4" => {
            let data = std::fs::read(path)?;
            let track = mp4::parse(&data)?;
            match track.codec {
                mp4::Codec::Aac => {
                    let dec = coreaudio::AacDecoder::open(path, &track)?;
                    Ok((Box::new(reader::DecReader::new(dec)), Format::Aac, Backend::FileReadMp4))
                }
                mp4::Codec::Alac => Ok((Box::new(coreaudio::AlacReader::open(path, &track)?), Format::Alac, Backend::FileReadAlac)),
                mp4::Codec::Other(k) => Err(Error::Unsupported(format!("MP4 codec {}", String::from_utf8_lossy(&k)))),
            }
        }
        "flac" | "fla" => Ok((Box::new(flac::FlacReader::open(path)?), Format::Flac, Backend::FileReadFlac)),
        "wav" | "aif" | "aiff" => {
            let r = pcm::PcmReader::open(path, if ext == "wav" { "wav" } else { "aiff" })?;
            let (format, own) = if ext == "wav" { (Format::Wav, Backend::FileReadWav) } else { (Format::Aiff, Backend::FileReadAiff) };
            let backend = if r.layout().path == pcm::PcmPath::Rekordbox { own } else { Backend::Juce };
            Ok((Box::new(r), format, backend))
        }
        other => Err(Error::Unsupported(format!(".{other}"))),
    }
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, None)
    }

    /// Opens `path` decoded at `rate` (through rekordbox's sample-rate converter when the file
    /// differs), as `PcmReaderTemplate<Float2>` does when it is built with a target rate (the
    /// embedding analysis reads at 16 kHz).
    pub fn open_at(path: &Path, rate: u32) -> Result<Self> {
        Self::open_with(path, Some(rate))
    }

    fn open_with(path: &Path, target: Option<u32>) -> Result<Self> {
        let (reader, format, backend) = base_reader(path)?;
        let ri = reader.info();
        if ri.length == 0 || ri.sample_rate == 0 {
            return Err(Error::Format("empty stream".into()));
        }
        let rate = target.unwrap_or_else(|| analysis_rate(ri.sample_rate));
        let reader: Box<dyn Reader> = if rate == ri.sample_rate {
            reader
        } else {
            Box::new(resample::SampleRateConverter::new(reader, rate)?)
        };
        let out = reader.info();
        let info = StreamInfo {
            format,
            backend,
            file_rate: ri.sample_rate,
            sample_rate: out.sample_rate,
            channels: ri.channels,
            bits: ri.bits,
            file_samples: u64::from(ri.length),
            total_samples: u64::from(out.length),
        };
        Ok(Self { reader, info, pos: 0 })
    }

    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// Next sample position `read` will return.
    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn seek(&mut self, sample: u64) {
        self.pos = sample.min(self.info.total_samples);
    }

    /// Back to the start as rekordbox's analysis thread re-reads a track for its phrase passes
    /// (`analysis-phrase.md` § 1.4): a random-access read at 0. Through the sample-rate converter
    /// it re-primes the filter (`prepareSRCProcessing`, `decode.md` § 8), and for 96 → 48 kHz that
    /// stream differs from a fresh decode's. Without a converter it is the same stream.
    pub fn rewind(&mut self) {
        self.pos = 0;
        self.reader.reposition();
    }

    /// Random access: fills `out` from sample `pos` (silence outside the track).
    pub fn read_at(&mut self, pos: i64, out: &mut [Float2]) {
        self.reader.read(pos, out);
    }

    /// Reads up to `out.len()` stereo samples from the current position; returns how many were
    /// read (0 at the end of the track).
    pub fn read_frames(&mut self, out: &mut [Float2]) -> usize {
        let n = (self.info.total_samples.saturating_sub(self.pos) as usize).min(out.len());
        if n > 0 {
            self.reader.read(self.pos as i64, &mut out[..n]);
            self.pos += n as u64;
        }
        n
    }

    /// Like [`Decoder::read_frames`] into an interleaved `[L, R, L, R, …]` buffer; returns frames.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let mut tmp = vec![[0.0f32; 2]; out.len() / 2];
        let n = self.read_frames(&mut tmp);
        for (dst, src) in out.chunks_exact_mut(2).zip(&tmp[..n]) {
            dst.copy_from_slice(src);
        }
        n
    }

    /// Decodes the whole track.
    pub fn decode_all(&mut self) -> Vec<Float2> {
        self.seek(0);
        let mut out = vec![[0.0f32; 2]; self.info.total_samples as usize];
        let mut at = 0;
        while at < out.len() {
            let end = (at + 65536).min(out.len());
            let n = self.read_frames(&mut out[at..end]);
            if n == 0 {
                break;
            }
            at += n;
        }
        out
    }
}
