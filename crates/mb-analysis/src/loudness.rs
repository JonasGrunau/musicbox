//! Auto-gain: `analyzer::LoudnessAnalyzer` (`init` @0x1013566c4, `addBuffer` @0x101356720,
//! `calcLoudness` @0x10135691c, `calcSamplePeak` @0x101356974) over the statically linked
//! libebur128 (`ebur128_init` @0x101ec0204, `filter_float` @0x101ec0e90, `calc_gating_block`
//! @0x101ec087c, `gated_loudness` @0x101ec1268), and `Loudness2AutoGainConverter`
//! @0x101356a30. Spec: `docs/re/analysis-loudness.md`.
//!
//! The arithmetic follows rekordbox's build of libebur128 instruction by instruction (fused
//! multiply-adds where the compiler made them, the same summation order), so the integrated
//! loudness comes out bit-identical.

/// `mStandardLoudnessLevel` (@0x1053ab8e8): the target loudness, −10 LUFS.
pub const TARGET_LUFS: f64 = -10.0;
/// The absolute gate, 10^((−70 + 0.691)/10) as rekordbox stores it.
const ABS_GATE: f64 = f64::from_bits(0x3e7f_791e_c6e1_d5b7);
/// Relative gate factor, 10^(−10/10).
const REL_GATE: f64 = 0.1;

/// libebur128 state for stereo input: integrated loudness (`EBUR128_MODE_I`) and the sample
/// peak (`EBUR128_MODE_SAMPLE_PEAK`), which rekordbox keeps in two separate states; both see
/// the same frames, so one filter run serves both here.
pub struct Ebur128 {
    samples_100ms: usize,
    /// Ring buffer of K-weighted samples, interleaved, 400 ms long.
    audio: Vec<f64>,
    /// Write position in samples (frames × 2).
    index: usize,
    /// Frames still needed before the next gating block.
    needed: usize,
    b: [f64; 5],
    a: [f64; 5],
    /// Filter state per channel: v[0] … v[4].
    v: [[f64; 5]; 2],
    /// Gating block energies in arrival order (libebur128 keeps them newest first).
    blocks: Vec<f64>,
    peak: [f64; 2],
}

impl Ebur128 {
    /// `ebur128_init(2, rate, mode)`.
    pub fn new(rate: u32) -> Self {
        let samples_100ms = (rate as usize + 5) / 10;
        let fs = rate as f64;
        // High shelf of the K-weighting (constants as stored in the binary).
        let k = mb_dsp::libm::tan(f64::from_bits(0x40b4_a414_1dba_f1ba) / fs);
        let kq = k * f64::from_bits(0x3ff6_a00e_da24_c22c);
        let k2 = k * k;
        let inv = 1.0 / ((kq + 1.0) + k2);
        let vh = f64::from_bits(0x3ff9_5b9b_16bf_11c6);
        let vbk = k * f64::from_bits(0x3ffc_7a95_abe2_5caf);
        let pb0 = ((vbk + vh) + k2) * inv;
        let pb1 = ((k2 + k2) + f64::from_bits(0xc009_5b9b_16bf_11c6)) * inv;
        let pb2 = ((vh - vbk) + k2) * inv;
        let pa1 = ((k2 + k2) + -2.0) * inv;
        let pa2 = ((1.0 - kq) + k2) * inv;
        // High-pass (RLB) part.
        let k = mb_dsp::libm::tan(f64::from_bits(0x405d_f397_63fa_798b) / fs);
        let k2 = k * k;
        let kq = k * f64::from_bits(0x3fff_faa5_33e0_bf11);
        let den = (kq + 1.0) + k2;
        let ra1 = ((k2 + k2) + -2.0) / den;
        let ra2 = ((1.0 - kq) + k2) / den;
        let b = [
            pb0,
            pb1 - (pb0 + pb0),
            (pb2 + pb0) - (pb1 + pb1),
            pb1 - (pb2 + pb2),
            pb2,
        ];
        let a = [
            1.0,
            ra1 + pa1,
            ra1.mul_add(pa1, ra2 + pa2),
            ra2.mul_add(pa1, ra1 * pa2),
            ra2 * pa2,
        ];
        Self {
            samples_100ms,
            audio: vec![0.0; samples_100ms * 4 * 2],
            index: 0,
            needed: samples_100ms * 4,
            b,
            a,
            v: [[0.0; 5]; 2],
            blocks: Vec::new(),
            peak: [0.0; 2],
        }
    }

    /// `ebur128_filter_float`: sample peak, then the K-weighting into the ring buffer.
    fn filter(&mut self, src: &[f32], frames: usize) {
        for c in 0..2 {
            let mut m = 0.0f64;
            for i in 0..frames {
                let x = f64::from(src[i * 2 + c]);
                m = if x > m {
                    x
                } else if m < -x {
                    -x
                } else {
                    m
                };
            }
            if self.peak[c] < m {
                self.peak[c] = m;
            }
        }
        let (b, a) = (self.b, self.a);
        for c in 0..2 {
            let [_, mut v1, mut v2, mut v3, mut v4] = self.v[c];
            let mut v0 = self.v[c][0];
            for i in 0..frames {
                let x = f64::from(src[i * 2 + c]);
                let fb = v4.mul_add(a[4], v3.mul_add(a[3], v2.mul_add(a[2], v1 * a[1])));
                v0 = x - fb;
                let y = b[4].mul_add(
                    v4,
                    b[3].mul_add(v3, b[2].mul_add(v2, b[1].mul_add(v1, b[0] * v0))),
                );
                self.audio[self.index + i * 2 + c] = y;
                v4 = v3;
                v3 = v2;
                v2 = v1;
                v1 = v0;
            }
            // Denormal flush of the state (v0 is overwritten by the next sample anyway).
            let flush = |x: f64| if x.abs() < f64::MIN_POSITIVE { 0.0 } else { x };
            self.v[c] = [v0, flush(v1), flush(v2), flush(v3), flush(v4)];
        }
    }

    /// `ebur128_calc_gating_block` for the last 400 ms.
    fn gating_block(&mut self) {
        let fpb = self.samples_100ms * 4;
        let frames = self.audio.len() / 2;
        let at = self.index / 2;
        let mut total = 0.0f64;
        for c in 0..2 {
            let mut sum = 0.0f64;
            let sq = |sum: f64, j: usize| self.audio[j * 2 + c].mul_add(self.audio[j * 2 + c], sum);
            if self.index < 2 * fpb {
                if self.index >= 2 {
                    for j in 0..at.max(1) {
                        sum = sq(sum, j);
                    }
                }
                for j in (frames + at - fpb)..frames {
                    sum = sq(sum, j);
                }
            } else {
                for j in (at - fpb)..at {
                    sum = sq(sum, j);
                }
            }
            total += sum;
        }
        let energy = total / fpb as f64;
        if energy >= ABS_GATE {
            self.blocks.push(energy);
        }
    }

    /// `ebur128_add_frames_float` with interleaved stereo frames.
    pub fn add_frames(&mut self, src: &[f32]) {
        let mut src = src;
        let mut frames = src.len() / 2;
        while frames > 0 {
            if frames < self.needed {
                self.filter(src, frames);
                self.index += frames * 2;
                self.needed -= frames;
                return;
            }
            let n = self.needed;
            self.filter(src, n);
            self.index += n * 2;
            self.gating_block();
            frames -= n;
            self.needed = self.samples_100ms;
            if self.index == self.audio.len() {
                self.index = 0;
            }
            src = &src[n * 2..];
        }
    }

    /// `ebur128_loudness_global`: integrated loudness in LUFS (−∞ when every block is gated).
    pub fn loudness_global(&self) -> f64 {
        // The block list is walked newest first.
        let mut sum = 0.0f64;
        for &e in self.blocks.iter().rev() {
            sum += e;
        }
        let n = self.blocks.len();
        if n == 0 {
            return f64::NEG_INFINITY;
        }
        let threshold = (REL_GATE * sum) / n as f64;
        let mut sum2 = 0.0f64;
        let mut n2 = 0u64;
        for &e in self.blocks.iter().rev() {
            sum2 += if e < threshold { -0.0 } else { e };
            if threshold <= e {
                n2 += 1;
            }
        }
        if n2 == 0 {
            return f64::NEG_INFINITY;
        }
        (sum2 / n2 as f64).ln().mul_add(4.3429448190325175, -0.691)
    }

    /// The largest absolute sample of either channel (`calcSamplePeak`).
    pub fn sample_peak(&self) -> f64 {
        let mut p = 0.0f64;
        for &c in &self.peak {
            if p <= c {
                p = c;
            }
        }
        p
    }
}

/// The auto-gain values rekordbox stores in `djmdMixerParam`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, LUFS.
    pub lufs: f64,
    /// `−10 − LUFS`.
    pub gain_db: f64,
    /// `(float)10^(gain_db / 20)`: `GainHigh`/`GainLow` are its upper and lower 16 bits.
    pub gain: f32,
    /// Sample peak, linear: `PeakHigh`/`PeakLow`.
    pub peak: f32,
}

impl Loudness {
    /// The `djmdMixerParam` columns: (GainHigh, GainLow, PeakHigh, PeakLow).
    pub fn mixer_param(&self) -> (u16, u16, u16, u16) {
        let g = self.gain.to_bits();
        let p = self.peak.to_bits();
        ((g >> 16) as u16, g as u16, (p >> 16) as u16, p as u16)
    }
}

/// `CBeatAnalyzeThread::analyze`'s loudness feed: only every `stride`-th analysis block of
/// `2 × rate` samples goes in (`stride = min(seconds/30 + 1, 16)`), as one continuous stream.
pub struct LoudnessAnalyzer {
    meter: Ebur128,
    stride: u64,
    block: u64,
    buf: Vec<f32>,
}

impl LoudnessAnalyzer {
    /// `total` is the number of analysed samples (`L − 1`).
    pub fn new(rate: u32, total: u64) -> Self {
        let seconds = total / u64::from(rate);
        Self {
            meter: Ebur128::new(rate),
            stride: (seconds / 30 + 1).min(16),
            block: 0,
            buf: Vec::new(),
        }
    }

    /// One analysis block (`addBuffer(l, r, n, 0x8000)`: samples / 32768, interleaved).
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        let take = self.block.is_multiple_of(self.stride);
        self.block += 1;
        if !take {
            return;
        }
        self.buf.clear();
        let scale = 1.0f32 / 32768.0;
        for &[l, r] in pcm {
            self.buf.push(f32::from(l) * scale);
            self.buf.push(f32::from(r) * scale);
        }
        self.meter.add_frames(&self.buf);
    }

    pub fn finish(self) -> Loudness {
        let lufs = self.meter.loudness_global();
        let gain_db = TARGET_LUFS - lufs;
        let gain = mb_dsp::libm::exp10(gain_db * 0.05) as f32;
        Loudness {
            lufs,
            gain_db,
            gain,
            peak: self.meter.sample_peak() as f32,
        }
    }
}
