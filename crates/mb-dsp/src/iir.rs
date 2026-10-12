//! BALib's band filters (`BeatAnalyzer_2_0::IirPassFilter`, designs @0x1011433c8–0x101143a2c,
//! run by `IirPassFilter::operate` @0x101143a2c over `balib::IirFilter::operate` @0x101143184).
//! Spec: `analysis-waveforms.md` § 3.2.
//!
//! Each filter is a cascade of identical-design biquads in double precision; `operate` widens the
//! float input to double, runs the sections one after another and narrows the result to float.

use crate::Biquad;

/// `2π` as BALib stores it (0x401921fb54442d11, 6.28318530717958).
const TWO_PI: f64 = f64::from_bits(0x4019_21fb_5444_2d11);
/// `ln 2 / 2` (0x3fd62e42fefa39ef), the bandwidth scale of `createBandPathFilterW`.
const HALF_LN2: f64 = f64::from_bits(0x3fd6_2e42_fefa_39ef);

#[derive(Debug, Clone)]
pub struct IirPassFilter {
    sections: Vec<Biquad>,
    a: Vec<f64>,
    b: Vec<f64>,
}

/// `(sin w, cos w, w)` for `w = f · 2π / fs` with float parameters widened to double.
fn angle(f: f32, fs: f32) -> (f64, f64, f64) {
    let w = (f64::from(f) * TWO_PI) / f64::from(fs);
    let (s, c) = crate::libm::sincos(w);
    (s, c, w)
}

/// Low-pass section: `α = sin/(2Q)`, `b0 = (0.5 − cos·0.5)·inv`, `b1 = inv − cos·inv` (fused).
fn lowpass(f: f32, fs: f32, q: f32) -> Biquad {
    let (s, c, _) = angle(f, fs);
    let alpha = s / (f64::from(q) + f64::from(q));
    let inv = 1.0 / (alpha + 1.0);
    let b0 = (-c).mul_add(0.5, 0.5) * inv;
    let b1 = (-c).mul_add(inv, inv);
    Biquad::new(b0, b1, b0, (c * -2.0) * inv, (-alpha).mul_add(inv, inv))
}

/// High-pass section: `b0 = (cos·0.5 + 0.5)·inv` (fused), `b1 = −(cos + 1)·inv`.
fn highpass(f: f32, fs: f32, q: f32) -> Biquad {
    let (s, c, _) = angle(f, fs);
    let alpha = s / (f64::from(q) + f64::from(q));
    let inv = 1.0 / (alpha + 1.0);
    let b0 = c.mul_add(0.5, 0.5) * inv;
    let b1 = -(c + 1.0) * inv;
    Biquad::new(b0, b1, b0, (c * -2.0) * inv, (-alpha).mul_add(inv, inv))
}

impl IirPassFilter {
    fn with(sections: Vec<Biquad>) -> Self {
        Self {
            sections,
            a: Vec::new(),
            b: Vec::new(),
        }
    }

    /// `createLowPathFilter(f, fs, q)`: two low-pass sections.
    pub fn low_path(f: f32, fs: f32, q: f32) -> Self {
        let s = lowpass(f, fs, q);
        Self::with(vec![s, s])
    }

    /// `createHiPathFilter(f, fs, q)`: two high-pass sections.
    pub fn hi_path(f: f32, fs: f32, q: f32) -> Self {
        let s = highpass(f, fs, q);
        Self::with(vec![s, s])
    }

    /// `createMidPathFilterB(lo, hi, fs, q)`: high-pass at `lo`, low-pass at `hi`, twice.
    pub fn mid_path_b(lo: f32, hi: f32, fs: f32, q: f32) -> Self {
        let h = highpass(lo, fs, q);
        let l = lowpass(hi, fs, q);
        Self::with(vec![h, l, h, l])
    }

    /// `createBandPathFilterQ(f, fs, q)`: two band-pass sections, `b0 = α·inv`, `b2 = −α·inv`.
    pub fn band_path_q(f: f32, fs: f32, q: f32) -> Self {
        let (s, c, _) = angle(f, fs);
        let alpha = s / (f64::from(q) + f64::from(q));
        let inv = 1.0 / (alpha + 1.0);
        let sec = Biquad::new(
            alpha * inv,
            0.0,
            (-alpha) * inv,
            (c * -2.0) * inv,
            (-alpha).mul_add(inv, inv),
        );
        Self::with(vec![sec, sec])
    }

    /// `createBandPathFilterW(f, fs, octaves)`: `α = sinh(ln2/2 · bw · w / sin w) · sin w`.
    pub fn band_path_w(f: f32, fs: f32, bw: f32) -> Self {
        let (s, c, w) = angle(f, fs);
        let sh = (((f64::from(bw) * HALF_LN2) * w) / s).sinh();
        let alpha = sh * s;
        let inv = 1.0 / (alpha + 1.0);
        let sec = Biquad::new(
            alpha * inv,
            0.0,
            (-(sh * s)) * inv,
            (c * -2.0) * inv,
            (-alpha).mul_add(inv, inv),
        );
        Self::with(vec![sec, sec])
    }

    /// `createThroughFilter`: two unity sections.
    pub fn through() -> Self {
        let s = Biquad::new(1.0, 0.0, 0.0, 0.0, 0.0);
        Self::with(vec![s, s])
    }

    /// `operate`: filters `input` into `out` (same length), keeping state across calls.
    pub fn operate(&mut self, input: &[f32], out: &mut [f32]) {
        let n = input.len().min(out.len());
        self.a.clear();
        self.a.extend(input[..n].iter().map(|&x| f64::from(x)));
        self.b.resize(n, 0.0);
        for s in &mut self.sections {
            s.process(&self.a, &mut self.b);
            std::mem::swap(&mut self.a, &mut self.b);
        }
        for (o, &x) in out.iter_mut().zip(&self.a) {
            *o = x as f32;
        }
    }
}
