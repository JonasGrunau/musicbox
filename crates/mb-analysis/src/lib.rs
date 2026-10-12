//! rekordbox 7.2.19's track analysis, ported. So far: the waveform tags (PWAV, PWV2, PWV3, PWV4,
//! PWV5, PWV6, PWV7, PWVC; `docs/re/analysis-waveforms.md`) and the auto-gain loudness
//! (`docs/re/analysis-loudness.md`).
//!
//! The analysis reads the decoded track (`mb-audio`, rekordbox's timeline) at positions
//! `0 .. L − 1`, quantised to 16 bit (`mb_audio::analysis_i16`), in blocks of two seconds, and
//! feeds the same samples to BALib (PWAV/PWV2/PWV3), WaveCreator (PWV4–7, PWVC) and the
//! loudness meter.

pub mod loud;
pub mod loudness;
pub mod wavecreator;
pub mod zoom;

pub use loudness::Loudness;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Audio(#[from] mb_audio::Error),
    #[error("track has no samples")]
    Empty,
}

pub type Result<T> = std::result::Result<T, Error>;

/// The waveform tag bodies, ready for `mb_anlz::tags`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Waveforms {
    /// PWAV: 400 column bytes (`height | colour << 5`).
    pub pwav: Vec<u8>,
    /// PWV2: 100 dot heights.
    pub pwv2: Vec<u8>,
    /// PWV3 column bytes (150 per second).
    pub pwv3: Vec<u8>,
    /// PWV4: 1200 × 6 bytes.
    pub pwv4: Vec<[u8; 6]>,
    /// PWV5 columns (150 per second): `r << 13 | g << 10 | b << 7 | height << 2`.
    pub pwv5: Vec<u16>,
    /// PWV6: 1200 × (low, mid, high).
    pub pwv6: Vec<[u8; 3]>,
    /// PWV7 columns (150 per second): (low, mid, high).
    pub pwv7: Vec<[u8; 3]>,
    /// PWVC coefficients.
    pub pwvc: [u16; 3],
}

/// Samples per analysis block: `CBeatAnalyzeThread::analyze` reads `2 × rate` samples per step
/// (`analysis-waveforms.md` § 1).
fn block_len(rate: u32) -> usize {
    2 * rate as usize
}

/// What to analyse (rekordbox's per-job flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub waveforms: bool,
    /// Auto-gain (`AutoGain` preference).
    pub loudness: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            waveforms: true,
            loudness: true,
        }
    }
}

/// The results of one analysis pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub waveforms: Option<Waveforms>,
    pub loudness: Option<Loudness>,
}

/// Analyses the waveforms of the audio file at `path`.
pub fn analyze_waveforms(path: impl AsRef<Path>) -> Result<Waveforms> {
    let a = analyze(
        path,
        Options {
            waveforms: true,
            loudness: false,
        },
    )?;
    Ok(a.waveforms.unwrap_or_default())
}

/// Analyses the audio file at `path` in one decode pass.
pub fn analyze(path: impl AsRef<Path>, opts: Options) -> Result<Analysis> {
    let mut dec = mb_audio::open(path.as_ref())?;
    let info = *dec.info();
    let len = info.total_samples;
    if len < 2 {
        return Err(Error::Empty);
    }
    let rate = info.sample_rate;
    // The analysis loop reads positions 0 .. L − 2 (decode.md § 2).
    let total = len - 1;
    let mut waves = opts.waveforms.then(|| {
        (
            zoom::ZoomViewMaker::new(rate, total),
            wavecreator::WaveCreator::new(rate, len),
            loud::LoudAnalyzer::new(rate, total),
        )
    });
    let mut meter = opts
        .loudness
        .then(|| loudness::LoudnessAnalyzer::new(rate, total));
    let block = block_len(rate);
    let mut frames = vec![[0.0f32; 2]; block];
    let mut pcm = vec![[0i16; 2]; block];
    let mut pos = 0u64;
    while pos < total {
        let n = ((total - pos) as usize).min(block);
        let got = dec.read_frames(&mut frames[..n]);
        if got == 0 {
            break;
        }
        for (dst, src) in pcm[..got].iter_mut().zip(&frames[..got]) {
            *dst = [
                mb_audio::analysis_i16(src[0]),
                mb_audio::analysis_i16(src[1]),
            ];
        }
        let pcm = &pcm[..got];
        if let Some((zoom, wave, loud)) = &mut waves {
            zoom.put(pcm);
            wave.put(pcm);
            loud.put(pcm);
        }
        if let Some(m) = &mut meter {
            m.put(pcm);
        }
        pos += got as u64;
    }
    let waveforms = waves.map(|(zoom, wave, loud)| {
        let w = wave.finish();
        let (pwav, pwv2) = loud.finish();
        Waveforms {
            pwav,
            pwv2,
            pwv3: zoom.finish(),
            pwv4: w.pwv4,
            pwv5: w.pwv5,
            pwv6: w.pwv6,
            pwv7: w.pwv7,
            pwvc: w.pwvc,
        }
    });
    Ok(Analysis {
        waveforms,
        loudness: meter.map(loudness::LoudnessAnalyzer::finish),
    })
}
