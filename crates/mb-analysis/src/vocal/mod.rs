//! Vocal detection (PVDI): rekordbox's `analyzer::VocalDetector` (ctor @0x1005acb08, `setData`
//! @0x1005acda8, `applyEndProcess` @0x1005ad82c). The mono mix is low-passed and resampled to
//! 22.05 kHz, cut into 2048-sample frames (hop 1024, Hann), and turned into 29 features per
//! frame (18 MFCC deltas and an 11-band fluctogram). rekordbox's network (`vd_param.pb`, through
//! [`mb_nn`]) rates each frame; the PVDI byte is that probability in steps of 0.2.
//! Spec: `docs/re/analysis-vocal.md`.

pub mod dsp;
pub mod features;
pub mod infer;

use crate::key::fft;
use features::{Fluctogram, Tables};

const N_FFT: usize = 2048;
const HOP: usize = 1024;

/// Intermediate results for the development comparisons.
#[derive(Debug, Default, Clone)]
pub struct Debug {
    pub power: Vec<f64>,
    pub mel: Vec<f64>,
    pub fluct: Vec<f64>,
}

/// One track's vocal detector, fed with the analysis loop's blocks.
pub struct VocalDetector {
    rate: u32,
    lp: dsp::LowPass,
    lag: dsp::Lagrange,
    /// The frame buffer (+0x0) and the samples it holds (+0x2000).
    buf: Box<[f32; N_FFT]>,
    count: i32,
    sintbl: Vec<f32>,
    bitrev: Vec<usize>,
    hann: Vec<f32>,
    tables: Tables,
    fluct_state: Fluctogram,
    /// Mel frames (128 per frame) and fluctogram rows (11 per frame pair) so far.
    mel: Vec<f64>,
    fluct: Vec<f64>,
    pub debug: Option<Debug>,
}

impl VocalDetector {
    /// `rate` = the analysis rate; `tables` = the shared filter tables ([`Tables::new`]).
    pub fn new(rate: u32, tables: Tables) -> Self {
        let mut hann = vec![0f32; N_FFT];
        fft::hanning(N_FFT, &mut hann);
        Self {
            rate,
            lp: dsp::LowPass::new(f64::from(rate), 11025.0),
            lag: dsp::Lagrange::default(),
            buf: Box::new([0.0; N_FFT]),
            count: 0,
            sintbl: fft::make_sintbl(N_FFT),
            bitrev: fft::make_bitrev(N_FFT),
            hann,
            tables,
            fluct_state: Fluctogram::default(),
            mel: Vec::new(),
            fluct: Vec::new(),
            debug: None,
        }
    }

    pub fn hann(&self) -> &[f32] {
        &self.hann
    }

    pub fn lowpass(&self) -> &dsp::LowPass {
        &self.lp
    }

    /// The power spectrum (1025 bins) of one windowed frame, `x(i)` giving sample `i`.
    fn frame(&self, x: impl Fn(usize) -> f32, out: &mut Vec<f64>) {
        let mut re = vec![0f32; N_FFT];
        let mut im = vec![0f32; N_FFT];
        for (i, r) in re.iter_mut().enumerate() {
            *r = self.hann[i] * x(i);
        }
        fft::fft(&mut re, &mut im, false, &self.sintbl, &self.bitrev);
        for k in 0..=N_FFT / 2 {
            out.push(f64::from(im[k].mul_add(im[k], re[k] * re[k])));
        }
    }

    fn features_of(&mut self, power: &[f64]) {
        let mel = self.tables.melspec(power);
        let fl = self.fluct_state.update(&self.tables, power);
        if let Some(d) = &mut self.debug {
            d.power.extend_from_slice(power);
            d.mel.extend_from_slice(&mel);
            d.fluct.extend_from_slice(&fl);
        }
        self.mel.extend(mel);
        self.fluct.extend(fl);
    }

    /// `setData`: one block of mono samples (`(L + R)/2` of the 16-bit values, as floats).
    pub fn put(&mut self, mono: &[f32]) {
        let n = mono.len();
        let m = ((n as f64 * 22050.0) / f64::from(self.rate)) as i32;
        let m_us = m.max(0) as usize;
        let scale = f32::from_bits(0x3800_0000);
        let mut x: Vec<f32> = mono.iter().map(|v| v * scale).collect();
        self.lp.process(&mut x);
        let mut out = vec![0f32; m_us];
        self.lag.process(&x, &mut out, self.rate);
        // the first block of at least 1024 samples is preceded by its first 1024, mirrored
        if self.count < 1 && m > 0x3ff {
            self.count = 1024;
            for i in 0..1024 {
                self.buf[i] = out[1023 - i];
            }
        }
        let count = self.count;
        let total = count + m;
        let mut power = Vec::new();
        let mut start = 0i32;
        while start <= total - N_FFT as i32 {
            let (buf, out) = (&self.buf, &out);
            self.frame(
                |i| {
                    let v = start as usize + i;
                    if (v as i32) < count { buf[v] } else { out[v - count as usize] }
                },
                &mut power,
            );
            start += HOP as i32;
        }
        // keep the samples the next frame needs (quirk kept: when the block is shorter than
        // that, the old part is taken from `buf[m..]`, not from its end)
        let keep = (total & 0x3ff) + 1024;
        self.count = keep;
        let k = keep as usize;
        if keep > m {
            let old = k - m_us;
            self.buf.copy_within(m_us..m_us + old, 0);
            self.buf[old..k].copy_from_slice(&out);
            self.buf[k..].fill(0.0);
        } else {
            self.buf[k..].fill(0.0);
            self.buf[..k].copy_from_slice(&out[m_us - k..]);
        }
        if !power.is_empty() {
            self.features_of(&power);
        }
    }

    /// `applyEndProcess` up to the features: the last frame (mirrored past the end), then the
    /// feature matrix (frames × 29, normalised by the caller). `None` when there are too few
    /// frames for the deltas.
    pub fn features(mut self) -> Option<(Vec<f64>, Option<Debug>)> {
        let count = self.count;
        let mut power = Vec::new();
        let mut start = 0i32;
        while start < count - HOP as i32 {
            let buf = &self.buf;
            self.frame(
                |i| {
                    let v = start + i as i32;
                    buf[if v < count { v } else { 2 * count - 1 - v } as usize]
                },
                &mut power,
            );
            start += HOP as i32;
        }
        if !power.is_empty() {
            self.features_of(&power);
        }
        let frames = self.mel.len() / 128;
        features::power_to_db(&mut self.mel);
        let mfcc = self.tables.mfcc(&self.mel)?;
        let delta = features::mfcc_delta(&mfcc, frames)?;
        let mut f = vec![0f64; frames * 29];
        for j in 0..frames {
            f[29 * j..29 * j + 18].copy_from_slice(&delta[18 * j..18 * j + 18]);
        }
        for j in 0..frames.saturating_sub(1) {
            f[29 * j + 18..29 * j + 29].copy_from_slice(&self.fluct[11 * j..11 * j + 11]);
        }
        Some((f, self.debug))
    }
}

/// What the vocal pass takes from the installed rekordbox: its network and, from its executable,
/// the learned feature statistics and the Chebyshev window table. None of it is in musicbox.
pub struct Model {
    session: mb_nn::Session,
    norm: [(f64, f64); 29],
    cheb: Vec<f64>,
}

/// `normalize_feature`'s table: scales and offsets of features 0–27, in pairs
/// (`scale[2m], scale[2m+1], offset[2m], offset[2m+1]`).
const NORM_TABLE: mb_nn::ExeRegion = mb_nn::ExeRegion { vmaddr: 0x1_0378_f6f0, len: 448, md5: "89bc4e6967c1ddd0cfe059636108509c" };
/// Feature 28's scale and offset, encoded in `normalize_feature`'s code as `movz/movk` into x10
/// (scale, the first four instructions) and x11 (offset, the last four).
const NORM_LAST: mb_nn::ExeRegion = mb_nn::ExeRegion { vmaddr: 0x1_0077_c5f0, len: 40, md5: "9b0a4ca8931d565c6c7a6fa54fbfeff1" };
/// The 240-point window `chebwin` copies.
const CHEBWIN: mb_nn::ExeRegion = mb_nn::ExeRegion { vmaddr: 0x1_0378_ef70, len: 1920, md5: "8f667e8b7ba347db966dd043c3b57eba" };

fn f64s(b: &[u8]) -> Vec<f64> {
    b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect()
}

/// A 64-bit immediate built by `movz` + three `movk` into register `rd`.
fn mov_imm(code: &[u8], rd: u32) -> Option<u64> {
    let mut v = 0u64;
    for (k, w) in code.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).enumerate() {
        let opc = w >> 23;
        let want = if k == 0 { 0x1a5 } else { 0x1e5 };
        if opc != want || w & 31 != rd {
            return None;
        }
        let shift = ((w >> 21) & 3) * 16;
        v |= u64::from((w >> 5) & 0xffff) << shift;
    }
    Some(v)
}

impl Model {
    fn load() -> Result<Self, String> {
        let table = f64s(&mb_nn::executable_bytes(NORM_TABLE).map_err(|e| e.to_string())?);
        let code = mb_nn::executable_bytes(NORM_LAST).map_err(|e| e.to_string())?;
        let scale = mov_imm(&code[0..16], 10).ok_or("feature 28 scale not found")?;
        let offset = mov_imm(&code[24..40], 11).ok_or("feature 28 offset not found")?;
        let mut norm = [(0.0, 0.0); 29];
        for m in 0..14 {
            norm[2 * m] = (table[4 * m], table[4 * m + 2]);
            norm[2 * m + 1] = (table[4 * m + 1], table[4 * m + 3]);
        }
        norm[28] = (f64::from_bits(scale), f64::from_bits(offset));
        let cheb = f64s(&mb_nn::executable_bytes(CHEBWIN).map_err(|e| e.to_string())?);
        let session = mb_nn::Session::open(mb_nn::Model::Vocal, 1).map_err(|e| e.to_string())?;
        Ok(Self { session, norm, cheb })
    }

    /// Loaded once per process.
    pub fn get() -> Result<&'static Model, String> {
        static MODEL: std::sync::OnceLock<Result<Model, String>> = std::sync::OnceLock::new();
        MODEL.get_or_init(Self::load).as_ref().map_err(Clone::clone)
    }

    /// The filter tables, built once.
    pub fn tables(&self) -> Tables {
        Tables::new(self.cheb.clone())
    }

    /// The PVDI bytes from a detector's features (`None` below 9 frames).
    pub fn detect(&self, det: VocalDetector) -> Result<Option<Vec<u8>>, String> {
        let Some((mut f, _)) = det.features() else { return Ok(None) };
        features::normalize(&mut f, &self.norm);
        let p = infer::probabilities(&self.session, &f).map_err(|e| e.to_string())?;
        Ok(Some(p.into_iter().map(infer::level).collect()))
    }

    pub fn norm(&self) -> &[(f64, f64); 29] {
        &self.norm
    }

    pub fn session(&self) -> &mb_nn::Session {
        &self.session
    }
}
