//! rekordbox 7.2.19's track analysis, ported. So far: the waveform tags (PWAV, PWV2, PWV3, PWV4,
//! PWV5, PWV6, PWV7, PWVC; `docs/re/analysis-waveforms.md`), the auto-gain loudness
//! (`docs/re/analysis-loudness.md`), the Normal-mode beat grid with its BPM
//! (`docs/re/analysis-beats.md`), the phrase pass's downbeat shift (`docs/re/analysis-phrase.md`)
//! and the key (`docs/re/analysis-key.md`).
//!
//! The analysis reads the decoded track (`mb-audio`, rekordbox's timeline) at positions
//! `0 .. L − 1`, quantised to 16 bit (`mb_audio::analysis_i16`), in blocks of two seconds, and
//! feeds the same samples to BALib (PWAV/PWV2/PWV3, beats), WaveCreator (PWV4–7, PWVC) and the
//! loudness meter.

pub mod balib2;
pub mod beats;
pub mod embed;
pub mod hpbeat;
pub mod key;
pub mod loud;
pub mod loudness;
pub mod phrase;
pub mod vocal;
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
    /// Beat grid and BPM (Normal mode).
    pub beats: bool,
    /// Phrase analysis; with `beats` it moves the grid onto the first downbeat
    /// (`analysis-phrase.md`).
    pub phrase: bool,
    /// Key (`enableKeyAnalysis`); needs `beats` (`analysis-key.md`).
    pub key: bool,
    /// High-precision beat analysis (`HighPrecisionBeatAnalyze`, mode from
    /// `trackHiPrecisionBeatAnalysisMode`); needs `beats` (`analysis-beats-hp.md`).
    pub high_precision: Option<hpbeat::HpMode>,
    /// Vocal detection (`trackAnalysisSettingsVocalDetect`, default off), with rekordbox's own
    /// network and statistics from the installed app (`analysis-vocal.md`).
    pub vocal: bool,
    /// Normal-mode Dynamic (`trackAnalysisMode` 1, analysis mode 1); ignored when
    /// `high_precision` is set (`analysis-beats.md` § 9).
    pub dynamic: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            waveforms: true,
            loudness: true,
            beats: true,
            phrase: true,
            key: true,
            high_precision: None,
            vocal: false,
            dynamic: false,
        }
    }
}

/// The results of one analysis pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub waveforms: Option<Waveforms>,
    pub loudness: Option<Loudness>,
    pub beats: Option<beats::BeatGrid>,
    /// Key code: 0–11 major, 12–23 minor, root `code % 12` with 0 = A ([`key::scale_name`]).
    pub key: Option<u8>,
    /// PQT2 `analysis_type`: 2 for a high-precision Dynamic grid, else 1 (`notifyBeat`).
    pub grid_type: u8,
    /// Why the high-precision grid was not made, when it was asked for (the Normal-mode grid
    /// is kept then).
    pub high_precision_error: Option<String>,
    /// The phrase analysis result (PSSI), when it ran and rekordbox would store it.
    pub phrase: Option<phrase::PhraseResult>,
    /// PVDI: one vocal level (0–4) per 1024 samples at 22.05 kHz, when vocal detection ran and
    /// gave a result.
    pub vocal: Option<Vec<u8>>,
    /// Why vocal detection, when asked for, gave nothing (rekordbox's install or model missing).
    pub vocal_error: Option<String>,
}

/// Analyses the waveforms of the audio file at `path`.
pub fn analyze_waveforms(path: impl AsRef<Path>) -> Result<Waveforms> {
    let a = analyze(
        path,
        Options {
            waveforms: true,
            loudness: false,
            beats: false,
            phrase: false,
            key: false,
            high_precision: None,
            vocal: false,
            ..Default::default()
        },
    )?;
    Ok(a.waveforms.unwrap_or_default())
}

/// Analyses the audio file at `path` in one decode pass.
pub fn analyze(path: impl AsRef<Path>, opts: Options) -> Result<Analysis> {
    analyze_with(path, opts, None)
}

/// [`analyze`] with the track's stored grid (its PQTZ beats and BPM × 100). When the
/// high-precision pass finds no beats, rekordbox runs key and phrase analysis on the stored grid
/// instead (`getBeatListFromDatabase`, `CBeatAnalyzeThread::analyze` @0x1001528c4,
/// `analysis-phrase.md` § 1.1); `Analysis::beats` stays `None` then.
pub fn analyze_with(
    path: impl AsRef<Path>,
    opts: Options,
    stored: Option<&beats::BeatGrid>,
) -> Result<Analysis> {
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
    // Normal-mode Dynamic takes BALib v1's varied analyser; high precision wins over it.
    let dynamic = opts.dynamic && opts.high_precision.is_none();
    let mut beat = opts.beats.then(|| {
        if dynamic {
            beats::BeatAnalyzer::new_dynamic(rate, total)
        } else {
            beats::BeatAnalyzer::new(rate, total)
        }
    });
    let mut phrase = (opts.beats && opts.phrase).then(|| phrase::PhraseAnalyzer::new(rate, total));
    let mut keys =
        (opts.beats && opts.key).then(|| key::KeyCollector::new(rate, total, info.channels));
    // madmom's input: the `(L + R)/2` float samples
    let mut mono = (opts.beats && opts.high_precision.is_some()).then(|| Vec::with_capacity(total as usize));
    let mut vocal_error = None;
    let mut voc = match opts.vocal.then(vocal::Model::get) {
        Some(Ok(m)) => Some((m, vocal::VocalDetector::new(rate, m.tables()), Vec::with_capacity(block_len(rate)))),
        Some(Err(e)) => {
            vocal_error = Some(e);
            None
        }
        None => None,
    };
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
        if let Some(m) = &mut mono {
            m.extend(frames[..got].iter().map(|f| (f[1] + f[0]) * 0.5));
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
        if let Some(b) = &mut beat {
            b.put(pcm);
        }
        if let Some(p) = &mut phrase {
            p.put(pcm);
        }
        if let Some(k) = &mut keys {
            k.put(pcm);
        }
        if let Some((_, det, buf)) = &mut voc {
            // the thread's mono mix of the 16-bit samples (`CBeatAnalyzeThread::analyze`)
            buf.clear();
            if info.channels == 2 {
                buf.extend(pcm.iter().map(|s| (f32::from(s[1]) + f32::from(s[0])) * 0.5));
            } else {
                buf.extend(pcm.iter().map(|s| f32::from(s[0])));
            }
            det.put(buf);
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
    let mut grid = beat.and_then(beats::BeatAnalyzer::finish);
    // PQT2 type: 2 for analysis modes 1 and 4 (`notifyBeat`).
    let (mut grid_type, mut high_precision_error) = (if dynamic { 2 } else { 1 }, None);
    if let (Some(mode), Some(m), Some(g)) = (opts.high_precision, &mono, &grid) {
        match hpbeat::grid(mode, m, rate, total, g) {
            Ok(hp) => (grid, grid_type) = (hp, 2),
            Err(e) => high_precision_error = Some(e),
        }
    }
    drop(mono);
    // A Dynamic mode found no beats: key and phrase take the stored grid.
    let mut stored = (grid_type == 2 && grid.is_none()).then(|| stored.cloned()).flatten();
    let mut phrases = None;
    if let (Some(g), Some(p)) = (grid.as_mut().or(stored.as_mut()), &phrase) {
        // The Dynamic high-precision grid is not shifted (`phrase::run`'s `urahaku`).
        phrases = phrase::run(p, g, rate, total, Some(path.as_ref()), grid_type != 2);
    }
    // The key job runs on the finished grid; `getKey` takes BPM × 100 · 0.01. Without a grid
    // only the whole-track estimate counts (no segments).
    let key = keys.map(|k| {
        let (beats, bpm) = grid.as_ref().or(stored.as_ref()).map_or((Vec::new(), 0.0), |g| {
            let beats = g.beats.iter().map(|b| (b.beat_in_bar, b.time_ms)).collect();
            (beats, g.bpm as f32 * 0.01)
        });
        key::detect(k.buffer(), rate, total, bpm, &beats) as u8
    });
    Ok(Analysis {
        waveforms,
        loudness: meter.map(loudness::LoudnessAnalyzer::finish),
        beats: grid,
        key,
        grid_type,
        high_precision_error,
        phrase: phrases,
        vocal: match voc.map(|(m, det, _)| m.detect(det)) {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                vocal_error = Some(e);
                None
            }
            None => None,
        },
        vocal_error,
    })
}
