//! Auto CUE candidates (Intelligent Cue Creation, `docs/re/analysis-cue.md` § 4). rekordbox's
//! `analyzer::CuePointPredictor` scores every phrase start with two models from the install, a
//! waveform CNN (`samplecnn`) and a tree-ensemble regressor (`cue_prediction_model_no_abs_position`),
//! and `ScoreCalculator::getTotalScores` @0x1019190dc turns the scores into up to 16 candidate
//! beats, best first. The CueManager places the cues from them (`mb-library`'s `cue_analysis`).

use crate::beats::Beat;
use crate::phrase::PhraseResult;
use crate::vocal::dsp::Lagrange;
use mb_nn::ort::{OnnxModel, OrtSession, OrtTensor};

/// One CNN input row: 3¹⁰ samples at 22.05 kHz.
const CHUNK: usize = 59_049;
/// Zeros put before and after the collected audio (`getScores`).
const PAD: usize = 178_605;
/// Half window lengths tried in turn: 3, 2 and 1 chunks either side of the candidate.
const HALVES: [u64; 3] = [3 * CHUNK as u64, 2 * CHUNK as u64, CHUNK as u64];
/// Phrase kinds and moods with a one-hot code (the tables the predictor's constructor
/// @0x101b48d08 builds: kind k → 1 at k − 1 of 10, mood m → 1 at m − 1 of 3).
const KINDS: usize = 10;
const MOODS: usize = 3;
/// `getTotalScores` keeps this many candidates.
const MAX_CANDIDATES: usize = 16;

/// `CuePointPredictor::setData` @0x101b4b874: each analysis block's mono mix, scaled by 2⁻¹⁵
/// and resampled to 22.05 kHz with JUCE's Lagrange interpolator (no low-pass, unlike the vocal
/// detector), appended to one buffer.
pub struct Collector {
    rate: u32,
    lag: Lagrange,
    buf: Vec<f32>,
}

impl Collector {
    pub fn new(rate: u32) -> Self {
        Self { rate, lag: Lagrange::default(), buf: Vec::new() }
    }

    /// One block of mono samples (`(L + R)/2` of the 16-bit values, as floats).
    pub fn put(&mut self, mono: &[f32]) {
        if self.rate < 1 {
            return;
        }
        let m = ((mono.len() as f64 * 22050.0) / f64::from(self.rate)) as u32 as i32;
        if m < 1 {
            return;
        }
        let scale = f32::from_bits(0x3800_0000);
        let x: Vec<f32> = mono.iter().map(|v| v * scale).collect();
        let at = self.buf.len();
        self.buf.resize(at + m as usize, 0.0);
        self.lag.process(&x, &mut self.buf[at..], self.rate);
    }

    /// The collected 22.05 kHz audio.
    pub fn audio(&self) -> &[f32] {
        &self.buf
    }
}

/// The 22.05 kHz audio of the file at `path` as the analysis loop collects it, with the
/// analysis rate: for scoring phrase starts on a grid from elsewhere (e.g. one rekordbox stored).
pub fn collect(path: &std::path::Path) -> crate::Result<(u32, Vec<f32>)> {
    let mut dec = mb_audio::open(path)?;
    let info = *dec.info();
    if info.total_samples < 2 {
        return Err(crate::Error::Empty);
    }
    let (rate, total) = (info.sample_rate, info.total_samples - 1);
    let mut c = Collector::new(rate);
    let block = crate::block_len(rate);
    let mut frames = vec![[0.0f32; 2]; block];
    let mut mono = Vec::with_capacity(block);
    let mut pos = 0u64;
    while pos < total {
        let n = ((total - pos) as usize).min(block);
        let got = dec.read_frames(&mut frames[..n]);
        if got == 0 {
            break;
        }
        mono.clear();
        mono.extend(frames[..got].iter().map(|f| {
            let (l, r) = (mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1]));
            if info.channels == 2 { (f32::from(r) + f32::from(l)) * 0.5 } else { f32::from(l) }
        }));
        c.put(&mono);
        pos += got as u64;
    }
    Ok((rate, c.buf))
}

/// The two auto-cue models, opened once as `initilaize_session` @0x101b49614 opens them.
pub struct Models {
    cnn: OrtSession,
    regressor: OrtSession,
}

impl Models {
    fn load() -> Result<Self, String> {
        let open = |m| OrtSession::open_cue(m).map_err(|e| e.to_string());
        Ok(Self { cnn: open(OnnxModel::SampleCnn)?, regressor: open(OnnxModel::CuePrediction)? })
    }

    pub fn get() -> Result<&'static Models, String> {
        static M: std::sync::OnceLock<Result<Models, String>> = std::sync::OnceLock::new();
        M.get_or_init(Self::load).as_ref().map_err(Clone::clone)
    }

    /// `onnx_get_features` @0x101b4b454: the window as rows of [`CHUNK`] samples through the
    /// CNN, all 512 features of each row in order.
    fn features(&self, window: &[f32]) -> Result<Vec<f32>, String> {
        let rows = window.len() / CHUNK;
        if rows == 0 {
            return Ok(Vec::new());
        }
        let t = OrtTensor::F32 { dims: vec![rows as i64, CHUNK as i64], data: window[..rows * CHUNK].to_vec() };
        let out = self.cnn.run(&[("waveform", &t)], &["features"]).map_err(|e| e.to_string())?;
        match out.into_iter().next() {
            Some(OrtTensor::F32 { data, .. }) => Ok(data),
            _ => Err("samplecnn: f32 features expected".into()),
        }
    }

    /// `CuePointPredictor::getScores` @0x101b49f08: one score per distinct phrase-start
    /// position, in phrase order, less the smallest. Empty when a phrase start lies outside the
    /// grid or the mood is not 1–3. `beats` is the finished grid, `rate` the analysis rate.
    pub fn scores(&self, audio: &[f32], rate: u32, beats: &[Beat], phrases: &PhraseResult) -> Result<Vec<f32>, String> {
        if rate < 1 || beats.is_empty() || phrases.count < 1 || audio.is_empty() || !(1..=3).contains(&phrases.mood) {
            return Ok(Vec::new());
        }
        let positions: Vec<i64> = beats.iter().map(|b| sample_position(b.time_ms, rate)).collect();
        let to_22k = 1.0 / f64::from(rate);
        // an ordered map from the candidate's 22.05 kHz position to its phrase kind: a repeated
        // position keeps its place and takes the later kind
        let mut keys: Vec<(i64, i32)> = Vec::new();
        for k in 0..phrases.count as usize {
            let (Some(&pos), Some(&kind)) = (phrases.cols[0].get(k), phrases.cols[1].get(k)) else { return Ok(Vec::new()) };
            if pos < 1 || pos as usize > positions.len() {
                return Ok(Vec::new());
            }
            // `fcvtas`: to nearest, halves away from zero
            let key = (positions[pos as usize - 1] as f64 * 22050.0 * to_22k).round() as i64;
            match keys.iter_mut().find(|(p, _)| *p == key) {
                Some(e) => e.1 = kind,
                None => keys.push((key, kind)),
            }
        }
        let mut padded = vec![0f32; PAD + audio.len() + PAD];
        padded[PAD..PAD + audio.len()].copy_from_slice(audio);
        let mut rows: Vec<f64> = Vec::new();
        let mut width = 0;
        for &(p, kind) in &keys {
            let mut row = vec![0f64; KINDS + MOODS];
            if (1..=KINDS as i32).contains(&kind) {
                row[kind as usize - 1] = 1.0;
            }
            row[KINDS + phrases.mood as usize - 1] = 1.0;
            let c = ((p as f32) + f32::from_bits(0x482e_6b41)) as i64;
            let window = HALVES.iter().find_map(|&half| {
                let start = if c < half as i64 + 1 { 0 } else { c as u64 - half };
                let end = (c as u64).wrapping_add(half);
                (end.wrapping_sub(start) == 2 * half && end < padded.len() as u64).then(|| &padded[start as usize..end as usize])
            });
            if let Some(w) = window {
                row.extend(self.features(w)?.into_iter().map(f64::from));
            }
            width = row.len();
            rows.extend(row);
        }
        if keys.is_empty() || rows.len() != keys.len() * width {
            return Ok(Vec::new());
        }
        let x = OrtTensor::F64 { dims: vec![keys.len() as i64, width as i64], data: rows };
        let out = self.regressor.run(&[("input", &x)], &["variable"]).map_err(|e| e.to_string())?;
        let Some(OrtTensor::F32 { data: mut scores, .. }) = out.into_iter().next() else {
            return Err("cue regressor: f32 scores expected".into());
        };
        let min = scores.iter().copied().reduce(|a, b| if b < a { b } else { a }).unwrap_or(0.0);
        for s in &mut scores {
            *s -= min;
        }
        Ok(scores)
    }
}

/// A beat's position in samples as the thread computes it (0x100156024): `ms · (rate · 0.001)`
/// fused with the add of 1.5 · 2⁵² that rounds to an integer, the low 32 bits taken. The fused
/// product is exact, so a time on a half sample (44.1 kHz, ms ending in 5) rounds up rather than
/// to even; that moves the window by one 22.05 kHz sample and can change the score.
fn sample_position(ms: u32, rate: u32) -> i64 {
    let r = f64::from(rate) * 0.001;
    i64::from(f64::from(ms).mul_add(r, 6_755_399_441_055_744.0).to_bits() as u32 as i32)
}

/// `ScoreCalculator::minMaxNormalization` @0x101919d98: min and max taken with 0, then
/// `(v − min) · (1 / (max − min))` when that range is positive.
fn min_max(m: &mut [(i32, f32)]) {
    let (mut lo, mut hi) = (0f32, 0f32);
    for &(_, v) in m.iter() {
        lo = v.min(lo);
        hi = v.max(hi);
    }
    if hi - lo > 0.0 {
        let r = 1.0 / (hi - lo);
        for e in m.iter_mut() {
            e.1 = (e.1 - lo) * r;
        }
    }
}

/// The thread's step after `getScores` (`CBeatAnalyzeThread::analyze` @0x1001528c4) and
/// `getTotalScores`, without a personal CUE tendency: scores are paired with phrase starts from
/// the last phrase back, normalised, and the 16 best kept, best first, as 0-based beat indices.
///
/// The personal tendency (`getPersonalTrendScoreMap` @0x101919a68) only adds scores when
/// `cue_personal_trend.json` holds non-zero values, which musicbox does not model; its
/// near-beat damping (`adjustScoreNearBeat` @0x101919f00) applies to those scores only.
pub fn total_scores(phrases: &PhraseResult, scores: &[f32]) -> Vec<(i32, f32)> {
    let mut left = scores.to_vec();
    // a map by beat index: the first pair for an index stays
    let mut map: Vec<(i32, f32)> = Vec::new();
    for k in (0..phrases.count.max(0) as usize).rev() {
        let Some(v) = left.pop() else { break };
        let beat = phrases.cols[0].get(k).copied().unwrap_or(0).wrapping_sub(1);
        if !map.iter().any(|e| e.0 == beat) {
            map.push((beat, v));
        }
    }
    map.sort_by_key(|e| e.0);
    min_max(&mut map);
    // hypothesis: ties keep beat order (std::sort is not stable, ties are rare)
    map.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    map.truncate(MAX_CANDIDATES);
    map
}

/// The candidates as the thread stores them for the CueManager (`BeatAnalyzeInfo`+0x20): the
/// grid's beat for each index, a zero beat for an index past the grid.
pub fn candidates(beats: &[Beat], total: &[(i32, f32)]) -> Vec<Beat> {
    total
        .iter()
        .map(|&(i, _)| beats.get(i as u32 as usize).copied().unwrap_or(Beat { beat_in_bar: 0, tempo: 0, time_ms: 0, sub_ms: 0 }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_sample_positions_round_up() {
        // 5 ms · 44.1 = 220.5: rounding half to even would give 220
        assert_eq!(sample_position(5, 44100), 221);
        assert_eq!(sample_position(15, 44100), 662);
        assert_eq!(sample_position(10, 44100), 441);
        assert_eq!(sample_position(1000, 48000), 48000);
    }

    #[test]
    fn min_max_takes_zero_into_the_range() {
        let mut m = vec![(1, 2.0), (5, 4.0), (9, 0.0)];
        min_max(&mut m);
        assert_eq!(m, [(1, 0.5), (5, 1.0), (9, 0.0)]);
        let mut flat = vec![(1, 0.0), (2, 0.0)];
        min_max(&mut flat);
        assert_eq!(flat, [(1, 0.0), (2, 0.0)]);
    }

    #[test]
    fn scores_pair_with_phrases_from_the_end() {
        let mut p = PhraseResult { count: 3, mood: 1, ..PhraseResult::default() };
        p.cols[0] = vec![1, 33, 65];
        p.cols[1] = vec![1, 2, 5];
        let t = total_scores(&p, &[0.0, 3.0, 1.5]);
        assert_eq!(t, [(32, 1.0), (64, 0.5), (0, 0.0)]);
        // fewer scores than phrases: the first phrases go without
        let t = total_scores(&p, &[2.0, 4.0]);
        assert_eq!(t, [(64, 1.0), (32, 0.5)]);
    }
}
