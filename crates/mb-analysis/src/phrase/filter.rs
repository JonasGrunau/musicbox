//! The phrase analyser's IIR sections (`c_filter`, @0x100351a00–0x100351ba8) and its 8:1
//! decimator (`SongStructureAnalyze_resample2`, init @0x1002ca5b4, `filter_8th_ds::downsample`
//! @0x100ac3ea8). Spec: `analysis-phrase.md` § 3.

/// 2π as the designs store it (0x401921fb54442d18).
pub const TWO_PI: f64 = std::f64::consts::TAU;
/// √2 (0x3ff6a09e667f3bcd): the Q of the second-order Butterworth sections.
pub const SQRT2: f64 = std::f64::consts::SQRT_2;

/// `c_filter`: a bilinear-transform section with coefficients `b0, b1, b2, a1, a2` (+0..+0x20)
/// and the transposed-direct-form state `s1, s2` (+0x28, +0x30).
#[derive(Debug, Clone, Copy, Default)]
pub struct CFilter {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

/// `c_filter::prewarp(w, fs) = tan(w / 2fs) · 2fs` (@0x100351a3c).
pub fn prewarp(w: f64, fs: f64) -> f64 {
    let fs2 = fs + fs;
    mb_dsp::libm::tan(w / fs2) * fs2
}

impl CFilter {
    /// `c_filter::make2(p1 … p6, fs)` @0x100351aa8: `H(s) = (p1·s² + p2·s + p3) / (p4·s² + p5·s
    /// + p6)` through the bilinear transform, evaluated in the binary's order.
    #[allow(clippy::too_many_arguments)]
    pub fn make2(p1: f64, p2: f64, p3: f64, p4: f64, p5: f64, p6: f64, fs: f64) -> Self {
        let t = 1.0 / fs;
        let tt = t * t;
        let d5 = tt * p6;
        let t2 = t + t;
        let d4 = t2 * p5;
        let d3 = p4 * 4.0;
        let inv = 1.0 / ((d4 + d3) + d5);
        let d2 = tt * p3;
        let d1 = t2 * p2;
        let d0 = p1 * 4.0;
        let inv2 = inv + inv;
        Self {
            b0: inv * ((d1 + d0) + d2),
            b1: inv2 * (d2 - d0),
            b2: inv * ((d0 - d1) + d2),
            a1: inv2 * (d5 - d3),
            a2: inv * ((d3 - d4) + d5),
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// `c_filter::make1(p1 … p4, fs)` @0x100351a64: `(p1·s + p2) / (p3·s + p4)`.
    pub fn make1(p1: f64, p2: f64, p3: f64, p4: f64, fs: f64) -> Self {
        let t = 1.0 / fs;
        let d3 = t * p4;
        let d2 = p3 + p3;
        let inv = 1.0 / (d3 + d2);
        let d1 = t * p2;
        let d0 = p1 + p1;
        Self {
            b0: inv * (d1 + d0),
            b1: inv * (d1 - d0),
            a1: inv * (d3 - d2),
            ..Self::default()
        }
    }

    /// The second-order low-pass `make2(0, 0, w², 1, w·k, w², fs)` with `w = prewarp(f·2π)`.
    pub fn low2(w: f64, k: f64, fs: f64) -> Self {
        let w2 = w * w;
        Self::make2(0.0, 0.0, w2, 1.0, w * k, w2, fs)
    }

    /// `c_filter::clear` (stage D's per-line smoothing).
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    /// `c_filter::filter2` @0x100351b68, in place.
    pub fn filter2(&mut self, x: &mut [f64]) {
        let (mut s1, mut s2) = (self.s1, self.s2);
        for v in x {
            let xi = *v;
            let y = xi.mul_add(self.b0, s1);
            s1 = (-y).mul_add(self.a1, xi.mul_add(self.b1, s2));
            s2 = xi.mul_add(self.b2, -(y * self.a2));
            *v = y;
        }
        self.s1 = s1;
        self.s2 = s2;
    }

    /// `c_filter::filter1` @0x100351b28, in place.
    pub fn filter1(&mut self, x: &mut [f64]) {
        let mut s = self.s1;
        for v in x {
            let xi = *v;
            let y = xi.mul_add(self.b0, s);
            s = xi.mul_add(self.b1, -(y * self.a1));
            *v = y;
        }
        self.s1 = s;
    }
}

/// The four Butterworth `k` of an 8th-order design.
const K8: [f64; 4] = [
    1.9615705608064609,
    1.6629392246050905,
    1.1111404660392046,
    0.39018064403225666,
];

/// `SongStructureAnalyze_resample2` (four low-pass sections at `fc`) or `resample_hpf` (four
/// high-pass sections), then `filter_8th_ds::downsample` by `ratio`.
#[derive(Debug, Clone)]
pub struct Decimator {
    sections: [CFilter; 4],
    ratio: usize,
}

impl Decimator {
    /// `resample2::init(fc, rate, ratio)`.
    pub fn low(fc: f64, rate: f64, ratio: usize) -> Self {
        let w = prewarp(fc * TWO_PI, rate);
        Self {
            sections: K8.map(|k| CFilter::low2(w, k, rate)),
            ratio,
        }
    }

    /// `resample_hpf::init(fc, rate, ratio)`.
    pub fn high(fc: f64, rate: f64, ratio: usize) -> Self {
        let w = prewarp(fc * TWO_PI, rate);
        Self {
            sections: K8.map(|k| CFilter::make2(1.0, 0.0, 0.0, 1.0, w * k, w * w, rate)),
            ratio,
        }
    }

    /// `filter_8th_ds::downsample`: filters one chunk and keeps samples `ratio·(k + 1) − 1` for
    /// `k < n / ratio`, so each chunk restarts the decimation phase.
    pub fn downsample(&mut self, chunk: &[f64]) -> Vec<f64> {
        let mut y = chunk.to_vec();
        for s in &mut self.sections {
            s.filter2(&mut y);
        }
        let r = self.ratio;
        (0..y.len() / r).map(|k| y[(k + 1) * r - 1]).collect()
    }
}
