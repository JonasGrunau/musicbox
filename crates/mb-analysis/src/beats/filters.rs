//! BALib v1's double-precision envelope filters: `LowPathFilter2D` (create @0x10118c0a8,
//! operate @0x10118c13c), `HighPathFilter2D` (@0x10118c2cc, @0x10118c368) and
//! `HighPathFilter1D` (@0x10118c4f4, @0x10118c540). Spec: `analysis-beats.md` § 4.

/// 2π as BALib stores it (0x401921fb54442d11).
const TWO_PI: f64 = f64::from_bits(0x4019_21fb_5444_2d11);
/// 1/√2 as these filters store it (0x3fe6a09e667f3bcd).
const Q: f64 = f64::from_bits(0x3fe6_a09e_667f_3bcd);

/// Direct-form-I second-order section evaluated as BALib does:
/// `y = fma(x2, b2, fma(x1, b1, x·b0)) − fma(y2, a2, y1·a1)`.
#[derive(Debug, Clone)]
pub struct Filter2D {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Filter2D {
    fn design(f: f64, fs: f64) -> (f64, f64, f64, f64) {
        let (s, c) = mb_dsp::libm::sincos((f * TWO_PI) / fs);
        let alpha = s * Q;
        let inv = 1.0 / (alpha + 1.0);
        (c, alpha, inv, (c * -2.0) * inv)
    }

    fn with(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
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

    /// `LowPathFilter2D::create(f, fs)`.
    pub fn low(f: f64, fs: f64) -> Self {
        let (c, alpha, inv, a1) = Self::design(f, fs);
        let b0 = (-c).mul_add(0.5, 0.5) * inv;
        let b1 = (-c).mul_add(inv, inv);
        Self::with(b0, b1, b0, a1, (-alpha).mul_add(inv, inv))
    }

    /// `HighPathFilter2D::create(f, fs)`.
    pub fn high(f: f64, fs: f64) -> Self {
        let (c, alpha, inv, a1) = Self::design(f, fs);
        let b0 = c.mul_add(0.5, 0.5) * inv;
        let b1 = -(c + 1.0) * inv;
        Self::with(b0, b1, b0, a1, (-alpha).mul_add(inv, inv))
    }

    pub fn step(&mut self, x: f64) -> f64 {
        let ff = self
            .x2
            .mul_add(self.b2, self.x1.mul_add(self.b1, x * self.b0));
        let fb = self.y2.mul_add(self.a2, self.y1 * self.a1);
        let y = ff - fb;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    pub fn run(&mut self, buf: &mut [f64]) {
        for v in buf {
            *v = self.step(*v);
        }
    }
}

/// `HighPathFilter1D`: `y = fma(−y1, a1, fma(x1, b1, x·b0))`.
#[derive(Debug, Clone)]
pub struct Filter1D {
    b0: f64,
    b1: f64,
    a1: f64,
    x1: f64,
    y1: f64,
}

impl Filter1D {
    pub fn high(f: f64, fs: f64) -> Self {
        let w = (f * TWO_PI) / fs;
        let d = w + 2.0;
        let b0 = 2.0 / d;
        Self {
            b0,
            b1: -b0,
            a1: (w + -2.0) / d,
            x1: 0.0,
            y1: 0.0,
        }
    }

    pub fn step(&mut self, x: f64) -> f64 {
        let y = (-self.y1).mul_add(self.a1, self.x1.mul_add(self.b1, x * self.b0));
        self.x1 = x;
        self.y1 = y;
        y
    }
}
