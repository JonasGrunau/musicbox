//! The second-order section WaveCreator uses (`analyzer::BandPassFilter`; design inline in
//! `WaveCreator::WaveCreator` @0x100d53c70, run by `BandPassFilter::Operate` @0x100d57280).
//! Spec: `analysis-waveforms.md` § 4.1.

/// Direct form I biquad with rekordbox's coefficient design (Q = 1/√2) and evaluation order.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

/// 1/√2 as rekordbox stores it (0x3fe6a09e667f3bcc).
const HALF_SQRT2: f64 = 0.7071067811865475;

/// `sin`, `cos`, `1/(1+α)` and the shared denominator coefficients of one corner frequency.
struct Design {
    cos: f64,
    alpha: f64,
    inv: f64,
}

impl Design {
    /// `w = (1/fs) · 2πf`; `α = sin w · 1/√2`; `inv = 1/(α + 1)`.
    fn new(inv_rate: f64, two_pi_f: f64) -> Self {
        let (sin, cos) = crate::libm::sincos(inv_rate * two_pi_f);
        let alpha = sin * HALF_SQRT2;
        Self {
            cos,
            alpha,
            inv: 1.0 / (alpha + 1.0),
        }
    }

    /// `a1 = (cos · −2) · inv`, `a2 = inv − α·inv` (fused).
    fn den(&self) -> (f64, f64) {
        (
            (self.cos * -2.0) * self.inv,
            (-self.alpha).mul_add(self.inv, self.inv),
        )
    }
}

impl Biquad {
    /// A section with the given coefficients (`a0` = 1) and zero state.
    pub fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn with(b0: f64, b1: f64, a: (f64, f64)) -> Self {
        Self::new(b0, b1, b0, a.0, a.1)
    }

    /// Low-pass. `two_pi_f` is the constant rekordbox stores (2π·f, rounded to f64);
    /// `inv_rate` = `1.0 / rate`. `b0 = (0.5 − cos·0.5)·inv` and `b1 = inv − cos·inv` (both fused).
    pub fn lowpass(inv_rate: f64, two_pi_f: f64) -> Self {
        let d = Design::new(inv_rate, two_pi_f);
        let b0 = (-d.cos).mul_add(0.5, 0.5) * d.inv;
        let b1 = (-d.cos).mul_add(d.inv, d.inv);
        Self::with(b0, b1, d.den())
    }

    /// High-pass. `b0 = (cos·0.5 + 0.5)·inv` (fused), `b1 = −(cos + 1)·inv`.
    pub fn highpass(inv_rate: f64, two_pi_f: f64) -> Self {
        let d = Design::new(inv_rate, two_pi_f);
        let b0 = d.cos.mul_add(0.5, 0.5) * d.inv;
        let b1 = -(d.cos + 1.0) * d.inv;
        Self::with(b0, b1, d.den())
    }

    /// One output: `ff = fma(x2, b2, fma(x1, b1, x·b0))`, `fb = fma(y2, a2, y1·a1)`, `ff − fb`.
    #[inline(always)]
    fn step(&self, x: f64, x1: f64, x2: f64, y1: f64, y2: f64) -> f64 {
        let ff = x2.mul_add(self.b2, x1.mul_add(self.b1, x * self.b0));
        let fb = y2.mul_add(self.a2, y1 * self.a1);
        ff - fb
    }

    /// Filters `x` into `y` (same length), keeping the state across calls.
    pub fn process(&mut self, x: &[f64], y: &mut [f64]) {
        let n = x.len().min(y.len());
        let (mut x1, mut x2, mut y1, mut y2) = (self.x1, self.x2, self.y1, self.y2);
        for i in 0..n {
            let out = self.step(x[i], x1, x2, y1, y2);
            y[i] = out;
            x2 = x1;
            x1 = x[i];
            y2 = y1;
            y1 = out;
        }
        (self.x1, self.x2, self.y1, self.y2) = (x1, x2, y1, y2);
    }
}
