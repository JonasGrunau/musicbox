//! PWV3, the monochrome scrolling waveform: BALib's `ZoomViewMaker` (`BeatAnalyzer_2_0`,
//! `create` @0x1011568c4, `putSample` @0x101156a80, `analyzeZV` @0x101156e14), packed by
//! `CMusicAnalyzeFile::set_pmai_ext_zoom_wave` @0x100f262b4. Spec: `analysis-waveforms.md` § 2.

/// `150π` as rekordbox stores it: the 150 Hz corner of the whiteness low-pass.
const PI_150: f64 = 471.23889803846896;
/// Height squash: `h' = trunc(h² · K)`, so 32767 maps to 31.
const SQUASH: f64 = 2.9327451233027466e-08;

pub struct ZoomViewMaker {
    /// Samples per column (rate / 150).
    block: usize,
    /// Total samples the analysis will feed (`L − 1`).
    total: u64,
    seen: u64,
    /// Interleaved samples of the current column.
    buf: Vec<[i16; 2]>,
    /// Mono history `x[-2], x[-1]` then the current column's mono samples.
    mono: Vec<i16>,
    /// Filter output history `y[-2], y[-1]` then the current column's outputs.
    out: Vec<f64>,
    /// Peak |mono| over the whole track (for the final normalisation).
    global_peak: i16,
    /// Per column: (height, whiteness).
    columns: Vec<(i16, i16)>,
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

/// `(int)(((double)r + (double)l) * 0.5)`, its absolute value and the clamp to 0x7fff.
#[inline]
fn mono_abs(l: i16, r: i16) -> (i32, u32) {
    let m = ((f64::from(r) + f64::from(l)) * 0.5) as i32;
    let a = (m.unsigned_abs()) & 0xffff;
    (m, a.min(0x7fff))
}

impl ZoomViewMaker {
    /// `create(rate, total, 150, normalise = true)`.
    pub fn new(rate: u32, total: u64) -> Self {
        let block = (rate / 150) as usize;
        // tan(150π / rate), then a Butterworth low-pass without fused operations.
        let k = mb_dsp::libm::tan(PI_150 / f64::from(rate));
        let t = k * std::f64::consts::SQRT_2;
        let k2 = k * k;
        let inv = 1.0 / ((t + 1.0) + k2);
        let twice = k2 + k2;
        Self {
            block,
            total,
            seen: 0,
            buf: Vec::with_capacity(block),
            mono: vec![0; 2],
            out: vec![0.0; 2],
            global_peak: 0,
            columns: Vec::with_capacity((total / block.max(1) as u64 + 2) as usize),
            b0: k2 * inv,
            b1: twice * inv,
            b2: k2 * inv,
            a1: (twice + -2.0) * inv,
            a2: ((1.0 - t) + k2) * inv,
        }
    }

    /// Feeds interleaved samples; stops at `total`.
    pub fn put(&mut self, samples: &[[i16; 2]]) {
        for &[l, r] in samples {
            if self.seen >= self.total {
                return;
            }
            self.buf.push([l, r]);
            let (_, a) = mono_abs(l, r);
            if i32::from(self.global_peak) < a as i32 {
                self.global_peak = a as i16;
            }
            self.seen += 1;
            if self.buf.len() >= self.block || self.seen >= self.total {
                self.analyze_column();
            }
        }
    }

    /// `analyzeZV`: one column from the buffered samples.
    fn analyze_column(&mut self) {
        let n = self.buf.len();
        self.mono.truncate(2);
        self.out.truncate(2);
        let mut peak: u32 = 0;
        for &[l, r] in &self.buf {
            let (m, a) = mono_abs(l, r);
            self.mono.push(m as i16);
            if (peak & 0xffff) <= a {
                peak = a;
            }
        }
        for i in 0..n {
            let x0 = f64::from(self.mono[i + 2]);
            let x1 = f64::from(self.mono[i + 1]);
            let x2 = f64::from(self.mono[i]);
            let y1 = self.out[i + 1];
            let y2 = self.out[i];
            let mut y = self.b0 * x0;
            y = self.b1.mul_add(x1, y);
            y = self.b2.mul_add(x2, y);
            y = (-y1).mul_add(self.a1, y);
            y = (-y2).mul_add(self.a2, y);
            self.out.push(y);
        }
        // Keep the last two inputs/outputs as the next column's history.
        let (m0, m1) = (self.mono[n], self.mono[n + 1]);
        let (o0, o1) = (self.out[n], self.out[n + 1]);
        self.mono[0] = m0;
        self.mono[1] = m1;
        self.out[0] = o0;
        self.out[1] = o1;
        let whiteness = if n == 0 {
            if peak & 0xffff != 0 {
                Self::level(0.0)
            } else {
                Some(7)
            }
        } else {
            let mut fpeak: u32 = 0;
            for &y in &self.out[2..2 + n] {
                let v = (y as i32).unsigned_abs().min(0x7fff);
                if v > (fpeak & 0xffff) {
                    fpeak = v;
                }
            }
            if peak & 0xffff == 0 {
                Some(7)
            } else {
                Self::level(f64::from(fpeak as u16 as i16) / f64::from(peak as u16 as i16))
            }
        };
        // A negative ratio cannot happen (both peaks are ≥ 0); rekordbox then leaves the slot as
        // allocated (zero).
        self.columns.push((peak as i16, whiteness.unwrap_or(0)));
        self.buf.clear();
    }

    /// Low-pass peak / full peak → 0 (all bass) … 7.
    fn level(ratio: f64) -> Option<i16> {
        Some(if ratio >= 0.875 {
            0
        } else if ratio >= 0.75 {
            1
        } else if ratio >= 0.625 {
            2
        } else if ratio >= 0.5 {
            3
        } else if ratio >= 0.375 {
            4
        } else if ratio >= 0.25 {
            5
        } else if ratio >= 0.125 {
            6
        } else if ratio < 0.0 {
            return None;
        } else {
            7
        })
    }

    /// Normalises the heights to the track's peak and packs the PWV3 bytes
    /// (`height & 0x1f | whiteness << 5`).
    pub fn finish(self) -> Vec<u8> {
        let n = self.columns.len();
        let peak = self.global_peak;
        let mut heights: Vec<i16> = self.columns.iter().map(|c| c.0).collect();
        if peak != 0 {
            let p = f64::from(peak);
            // putSample's normalisation: a 2-wide vector loop over the first n − r columns, a scalar
            // loop over the rest (r = n & 3, or 4 when n is a multiple of 4; all scalar when n ≤ 4).
            // The two loops round differently.
            let vec_end = if n > 4 {
                n - if n & 3 == 0 { 4 } else { n & 3 }
            } else {
                0
            };
            let scale = 32767.0 / p;
            for h in &mut heights[..vec_end] {
                let v = (scale * f64::from(*h)) as i64 as i32;
                let v = f64::from(v);
                *h = ((v * v) * SQUASH) as i64 as i32 as i16;
            }
            let inv = 1.0 / p;
            for h in &mut heights[vec_end..] {
                let v = ((f64::from(*h) * 32767.0) * inv).trunc();
                *h = ((v * v) * SQUASH) as i32 as i16;
            }
        } else {
            heights.iter_mut().for_each(|h| *h = 0);
        }
        heights
            .iter()
            .zip(&self.columns)
            .map(|(&h, &(_, w))| (h as u8 & 0x1f) | ((w as u8) << 5))
            .collect()
    }
}
