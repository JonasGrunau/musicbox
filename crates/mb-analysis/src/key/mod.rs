//! Key detection: BALib's `KeyCodeScale` and `KeyJudge2`, driven like
//! `analyzer::AnalyzeJobKey::getKey` @0x101b0a1a8. Spec: `docs/re/analysis-key.md`.
//!
//! During the decode pass `KeyCollector` builds rekordbox's key buffer: the analysis blocks
//! mixed to (mostly) the left channel and decimated by 8 through `resample2`. After the beat
//! grid is known, [`detect`] runs the job: beat suppression (`CutBeats`), the reference pitch
//! (`PitchDetect`), a whole-track and per-segment key estimate (`KeyCodeScale`) and the final
//! judgement over the segments (`Key_judge_with_keys`).

// Float tests are written as the binary branches (`b.le`, `b.lt`, … are taken on NaN); index
// loops follow the binary's.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::nonminimal_bool)]
#![allow(clippy::needless_range_loop)]

pub mod cqt;
pub mod cutbeats;
pub mod fft;
pub mod judge;
mod libm;
pub mod pitch;
mod resample;
pub mod scale;

pub use resample::resample2;

/// The `djmdKey.ScaleName` rekordbox stores for key code `code` (0–11 major, 12–23 minor, root
/// `code % 12` from A).
pub fn scale_name(code: u8) -> &'static str {
    const NAMES: [&str; 24] = [
        "A", "Bb", "B", "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "Am", "Bbm", "Bm", "Cm",
        "Dbm", "Dm", "Ebm", "Em", "Fm", "F#m", "Gm", "Abm",
    ];
    NAMES[code as usize % 24]
}

/// `CBeatAnalyzeThread::analyze`'s key buffer (`analysis-key.md` § 1).
#[derive(Debug, Clone)]
pub struct KeyCollector {
    rate: u32,
    decim: usize,
    mix: Option<(f32, f32)>,
    buf: Vec<f32>,
    pos: usize,
    left: Vec<f32>,
    scratch: Vec<f32>,
}

impl KeyCollector {
    /// `total` = `L − 1`, the analysed length; `channels` of the source file.
    pub fn new(rate: u32, total: u64, channels: u16) -> Self {
        let decim = match rate {
            88_200 | 96_000 => 16,
            176_400 | 192_000 => 32,
            _ => 8,
        };
        let q = (total / decim as u64) as u32;
        let k = if q > 10_800_000 { 3_600_000 } else { q } as usize;
        let short = total < 30 * u64::from(rate);
        // (a, b): left·a + right·b, only for stereo files.
        let mix = (channels == 2).then(|| {
            if short {
                (f32::from_bits(0x3f7fbe77), f32::from_bits(0x3a831200))
            } else {
                (f32::from_bits(0x3f7d70a4), f32::from_bits(0x3c23d700))
            }
        });
        Self {
            rate,
            decim,
            mix,
            buf: vec![0.0; k],
            pos: 0,
            left: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// One analysis block of 16-bit samples.
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        let k = self.buf.len();
        if self.pos >= k {
            return;
        }
        self.left.clear();
        self.left.extend(pcm.iter().map(|s| match self.mix {
            Some((a, b)) => f32::from(s[1]).mul_add(b, f32::from(s[0]) * a),
            None => f32::from(s[0]),
        }));
        let n = pcm.len() / self.decim;
        let count = if self.pos + n < k { n } else { k - self.pos };
        let pos = self.pos;
        resample2(
            &mut self.buf[pos..pos + count],
            &self.left,
            self.decim,
            &mut self.scratch,
        );
        self.pos += n;
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The finished key buffer.
    pub fn buffer(&self) -> &[f32] {
        &self.buf
    }
}

/// Decodes `path` and builds only the key buffer (development aid; `analyze` does the same
/// inside its single decode pass).
pub fn collect(path: &std::path::Path) -> crate::Result<(KeyCollector, u64)> {
    let mut dec = mb_audio::open(path)?;
    let info = *dec.info();
    let len = info.total_samples;
    if len < 2 {
        return Err(crate::Error::Empty);
    }
    let rate = info.sample_rate;
    let total = len - 1;
    let mut col = KeyCollector::new(rate, total, info.channels);
    let block = 2 * rate as usize;
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
        col.put(&pcm[..got]);
        pos += got as u64;
    }
    Ok((col, total))
}

/// The key code (0–11 major, 12–23 minor, root `code % 12` with 0 = A) of a key buffer:
/// [`estimates`] followed by [`judge::judge`].
pub fn detect(buf: &[f32], rate: u32, len: u64, bpm: f32, beats: &[(u16, u32)]) -> i32 {
    let e = estimates(buf, rate, len, bpm, beats, !beats.is_empty());
    judge::judge(e.key1, e.key2, e.segments, &e.keys, bpm)
}

/// The inputs and outputs of the key judgement (`KeyJudge`): the whole-track pair and one pair
/// per segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Estimates {
    pub key1: i32,
    pub key2: i32,
    /// Segments were analysed (`param_6`; off for buffers shorter than `0.42875·rate`).
    pub segments: bool,
    /// One `(key1, key2)` per segment; pairs after an early stop stay `(0, 0)`.
    pub keys: Vec<(i32, i32)>,
}

/// `AnalyzeJobKey::getKey` @0x101b0a1a8 up to the judgement (`analysis-key.md` § 2).
/// `buf` is the key buffer, `rate` the analysis rate, `len` the analysed length `L − 1`,
/// `beats` the grid as `(beat_in_bar, time_ms)`.
pub fn estimates(
    buf: &[f32],
    rate: u32,
    len: u64,
    bpm: f32,
    beats: &[(u16, u32)],
    segments: bool,
) -> Estimates {
    estimates_traced(buf, rate, len, bpm, beats, segments, &mut |_, _| {})
}

/// [`estimates`], handing each `KeyCodeScale` call's trace to `trace` (`None` = whole track,
/// `Some(i)` = segment `i`).
pub fn estimates_traced(
    buf: &[f32],
    rate: u32,
    len: u64,
    bpm: f32,
    beats: &[(u16, u32)],
    segments: bool,
    trace: &mut dyn FnMut(Option<usize>, &scale::Trace),
) -> Estimates {
    let ratef = rate as f32;
    let dur = len as f32 / ratef;
    let within = |lo: f32| lo < dur && dur < 501.0;
    // Beats per segment.
    let step = if (75.0 < bpm && bpm < 90.0 && within(240.0))
        || ((90.0..95.0).contains(&bpm) && within(210.0))
        || ((119.0..=125.0).contains(&bpm) && within(360.0))
        || ((125.0..=127.0).contains(&bpm) && within(405.0))
        || (135.0 < bpm && bpm <= 140.0 && within(360.0))
    {
        4
    } else if (bpm <= 141.0 && dur < 501.0) || (151.0 <= bpm && dur < 180.0) {
        2
    } else if dur < 1200.0 {
        4
    } else {
        8
    };
    // Segment boundaries (key-buffer samples at the full rate, `ms · (rate / 1000)`).
    let first = match beats.first() {
        Some(&(b, _)) if (2..=4).contains(&b) => 5 - b as usize,
        _ => 0,
    };
    let mut pos: Vec<i32> = (first..beats.len())
        .step_by(step)
        .map(|i| (beats[i].1 as i32).wrapping_mul((rate / 1000) as i32))
        .collect();
    let count = pos.len();
    if let Some(p) = pos.first_mut() {
        *p = -1;
    }
    let nseg = count.max(2) - 1;
    // The whole-track buffer: short buffers are padded (and lose their segments).
    let k = buf.len();
    let mut segments = segments;
    let mut kk = k;
    if (k as u32 as f32) < ratef * 0.42875 {
        segments = false;
        let t = if ratef * 0.0046875 <= k as f32 {
            ratef * 0.42875
        } else {
            ratef * 0.1875
        };
        // `fcvtps`: rounded up.
        kk = ((t / k as f32).ceil() as i32 as u32).wrapping_mul(k as u32) as usize;
    }
    let mut y = vec![0f32; kk.max(k)];
    let rate8 = ratef * 0.125;
    cutbeats::cut_beats(&mut y, buf, k, rate8, 8);
    y.truncate(kk);
    let mut keys = vec![(0, 0); nseg];
    let mut bank = cqt::Bank::default();
    let (mut key1, mut key2, mut pitch) = (0, 0, 0.0);
    if kk != 0 {
        pitch = pitch::pitch_detect(&y, kk, rate8, 8) as f32;
        let mut t = scale::Trace::default();
        (key1, key2) = scale::key_code_scale(&y, rate8, pitch, false, 0, &mut bank, Some(&mut t));
        trace(None, &t);
        if !segments {
            keys[0] = (key1, key2);
            return Estimates {
                key1,
                key2,
                segments,
                keys,
            };
        }
    } else if !segments {
        return Estimates {
            key1,
            key2,
            segments,
            keys,
        };
    }
    let mut off = 0usize;
    let mut prev = 0i32;
    for (i, slot) in keys.iter_mut().enumerate() {
        // Past the last boundary rekordbox reads uninitialised memory; stop instead.
        let Some(&p) = pos.get(i + 1) else { break };
        let n = (p - prev) / 8;
        // rekordbox compares bytes (4·n) with samples here.
        if (kk as u32) < (n as u32).wrapping_mul(4).wrapping_add(off as u32) {
            break;
        }
        let n = n as usize;
        let seg = &y[off..off + n];
        let nf = n as u32 as f32;
        let mut t = scale::Trace::default();
        *slot = if ratef * 0.5375 <= nf {
            scale::key_code_scale(seg, rate8, pitch, true, i, &mut bank, Some(&mut t))
        } else {
            let target = if ratef * 0.0046875 <= nf {
                ratef * 0.5375
            } else {
                ratef * 0.1875
            };
            let reps = (target / nf).ceil() as i32;
            let reps = reps.max(0) as usize;
            let tiled = seg.repeat(reps);
            scale::key_code_scale(&tiled, rate8, pitch, true, i, &mut bank, Some(&mut t))
        };
        trace(Some(i), &t);
        off += n;
        prev = p;
    }
    Estimates {
        key1,
        key2,
        segments,
        keys,
    }
}
