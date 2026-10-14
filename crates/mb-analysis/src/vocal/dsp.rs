//! The vocal detector's input stage: JUCE's two-pole low-pass (`IIRCoefficients::makeLowPass`
//! @0x10243b8dc, `IIRFilterBase::processSamples` @0x10243b858) and the five-point Lagrange
//! resampler to 22.05 kHz (`LagrangeTraits::valueAtOffset` @0x10243b978, driven from
//! `VocalDetector::setData` @0x1005acda8). Spec: `analysis-vocal.md` § 2.

/// Low-pass at 11025 Hz, Q = 1/√2, coefficients in f32 as JUCE stores them.
#[derive(Debug, Clone)]
pub struct LowPass {
    c: [f32; 5],
    lv1: f32,
    lv2: f32,
}

impl LowPass {
    pub fn new(rate: f64, freq: f64) -> Self {
        let n = 1.0 / (freq * std::f64::consts::PI / rate).tan();
        let n2 = n * n;
        let a = n * std::f64::consts::SQRT_2;
        let c1 = 1.0 / ((a + 1.0) + n2);
        let c2 = c1 + c1;
        // fmsub: c2 − n²·c2
        let c3 = (-n2).mul_add(c2, c2);
        let c4 = c1 * ((1.0 - a) + n2);
        Self { c: [c1 as f32, c2 as f32, c1 as f32, c3 as f32, c4 as f32], lv1: 0.0, lv2: 0.0 }
    }

    pub fn coefficients(&self) -> [f32; 5] {
        self.c
    }

    /// Transposed direct form II, the multiply-adds fused as in the binary.
    pub fn process(&mut self, x: &mut [f32]) {
        let [b0, b1, b2, a1, a2] = self.c;
        let (mut lv1, mut lv2) = (self.lv1, self.lv2);
        for s in x {
            let i = *s;
            let out = i.mul_add(b0, lv1);
            *s = out;
            let t = i.mul_add(b1, lv2);
            lv1 = (-out).mul_add(a1, t);
            lv2 = i.mul_add(b2, -(out * a2));
        }
        (self.lv1, self.lv2) = (lv1, lv2);
    }
}

/// The interpolator state (`new 0x28`): five history samples, the sub-sample position (starts
/// at 1.0) and the ring index.
#[derive(Debug, Clone)]
pub struct Lagrange {
    hist: [f32; 5],
    pos: f64,
    idx: usize,
}

impl Default for Lagrange {
    fn default() -> Self {
        Self { hist: [0.0; 5], pos: 1.0, idx: 0 }
    }
}

fn wrap(i: usize) -> usize {
    if i + 1 == 5 { 0 } else { i + 1 }
}

impl Lagrange {
    /// `out.len()` samples from `input`, stepping `rate · (1/22050)` input samples per output.
    /// Like the binary, input is consumed as the position demands (past the end it reads 0).
    pub fn process(&mut self, input: &[f32], out: &mut [f32], rate: u32) {
        let step = f64::from(rate) * (1.0 / 22050.0);
        let mut k = 0usize;
        for o in out {
            let mut idx = self.idx;
            if 1.0 <= self.pos {
                loop {
                    self.hist[idx] = input.get(k).copied().unwrap_or(0.0);
                    idx = wrap(idx);
                    self.pos += -1.0;
                    k += 1;
                    if self.pos < 1.0 {
                        break;
                    }
                }
                self.idx = idx;
            }
            *o = value_at_offset(&self.hist, self.pos as f32, idx);
            self.pos += step;
        }
    }
}

/// `LagrangeTraits::valueAtOffset` as compiled: the five weights in the binary's order of
/// operations (paired lanes, fused where the binary fuses).
fn value_at_offset(h: &[f32; 5], o: f32, index: usize) -> f32 {
    let (i0, i1) = (index, wrap(index));
    let i2 = wrap(i1);
    let i3 = wrap(i2);
    let i4 = wrap(i3);
    let third = f32::from_bits(0x3eaa_aaab);
    let two_thirds = f32::from_bits(0x3f2a_aaab);
    // the centre sample's weight
    let mut c = 1.0 - o;
    c = o.mul_add(c, c);
    let half = o * 0.5;
    c = half.mul_add(c, c);
    c = (-half).mul_add(c, c);
    let centre = c * h[i2];
    let v4 = [o * third, o * 0.25];
    let v6 = [half, v4[0]];
    let v16 = [(v6[0] + -0.5) * (two_thirds - v4[0]), (v6[1] + -third) * (0.5 - v4[1])];
    let v16 = [v16[0] * h[i1], v16[1] * h[i0]];
    let v2 = [(o + 2.0) * v16[0], (-1.0 - o) * v16[1]];
    let v3 = [(v4[0] + two_thirds) * (v6[0] + 0.5), (v4[1] + 0.5) * (v6[1] + third)];
    let v1 = [(2.0 - o) * v3[0], (o + -1.0) * v3[1]];
    let v1 = [v1[0] * h[i3] + v2[0], v1[1] * h[i4] + v2[1]];
    let s = v1[0] + 0.5 * v1[1];
    s.mul_add(o, centre)
}
