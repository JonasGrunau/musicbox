//! PWV4, PWV5, PWV6, PWV7 and PWVC: `analyzer::WaveCreator` (constructor @0x100d53c70,
//! `readWaveData` @0x100d564c4, `createWholeWaveData` @0x100d549fc, `createColorWaveData`
//! @0x100d54c70, `create3BandsWholeWaveData` @0x100d55210, `create3BandsZoomWaveData`
//! @0x100d55fbc). Spec: `analysis-waveforms.md` § 4.
//!
//! WaveCreator filters a mono mix through 18 biquads and keeps three per-millisecond summaries
//! (records A, B, C); the waveforms are built from those summaries after the last sample.

use mb_dsp::Biquad;

/// `1/32767` as rekordbox stores it (0x3f00002000400080).
const INV_32767: f64 = f64::from_bits(0x3f00_0020_0040_0080);
/// 0.001, the "both channels equally loud" threshold of the mono mix.
const MONO_EPS: f64 = f64::from_bits(0x3f50_624d_d2f1_a9fc);
/// π / 32768.
const PI_32768: f64 = f64::from_bits(0x3f19_21fb_5444_2d18);
/// The squash of the colour waveform heights (same constant as PWV3).
const SQUASH: f64 = 2.9327451233027466e-08;

/// `2πf` constants as rekordbox stores them, by corner frequency.
mod w {
    pub const F100: f64 = f64::from_bits(0x4083_a28c_59d5_433b);
    pub const F200: f64 = f64::from_bits(0x4093_a28c_59d5_433b);
    pub const F250: f64 = f64::from_bits(0x4098_8b2f_704a_9409);
    pub const F300: f64 = f64::from_bits(0x409d_73d2_86bf_e4d8);
    pub const F400: f64 = f64::from_bits(0x40a3_a28c_59d5_433b);
    pub const F1200: f64 = f64::from_bits(0x40bd_73d2_86bf_e4d8);
    pub const F1500: f64 = f64::from_bits(0x40c2_6863_9437_ef07);
    pub const F2000: f64 = f64::from_bits(0x40c8_8b2f_704a_9409);
    pub const F3000: f64 = f64::from_bits(0x40d2_6863_9437_ef07);
    pub const F9000: f64 = f64::from_bits(0x40eb_9c95_5e53_e68b);
}

/// ARM `fmaxnm`: NaN loses, +0 beats −0.
#[inline]
fn fmaxnm(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a == b {
        if a.is_sign_negative() { b } else { a }
    } else if a > b {
        a
    } else {
        b
    }
}

/// ARM `fminnm`: NaN loses, −0 beats +0.
#[inline]
fn fminnm(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a == b {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

#[inline]
fn fmaxnm32(a: f32, b: f32) -> f32 {
    fmaxnm(f64::from(a), f64::from(b)) as f32
}

#[inline]
fn fminnm32(a: f32, b: f32) -> f32 {
    fminnm(f64::from(a), f64::from(b)) as f32
}

/// `fcvtzs w, d, #15`: trunc(d · 2¹⁵), saturating to i32; the low 16 bits are stored.
#[inline]
fn fix15(d: f64) -> i16 {
    (d * 32768.0) as i32 as i16
}

/// A signed peak in [−1, 1] as stored in records A and B.
fn signed_peak(d: f64) -> i16 {
    if (d > 0.0 && d < 1.0) || (-1.0..0.0).contains(&d) {
        fix15(d)
    } else if d >= 1.0 {
        0x7fff
    } else if d >= -1.0 {
        0
    } else {
        i16::MIN
    }
}

/// A magnitude below 1 as Q15, 0x7fff from 1 up (and for NaN).
fn magnitude(d: f64) -> i16 {
    if d < 1.0 { fix15(d) } else { 0x7fff }
}

/// Filters by their offset in the C++ object (for the spec cross-reference).
struct Filters {
    lp400: Biquad,   // +0x30
    lp200a: Biquad,  // +0x88
    lp200b: Biquad,  // +0xe0
    hp200a: Biquad,  // +0x138
    hp200b: Biquad,  // +0x190
    lp2000a: Biquad, // +0x1e8
    lp2000b: Biquad, // +0x240
    hp2000a: Biquad, // +0x298
    hp2000b: Biquad, // +0x2f0
    lp100: Biquad,   // +0x348
    hp300: Biquad,   // +0x3a0
    lp3000: Biquad,  // +0x3f8
    hp1500: Biquad,  // +0x450
    lp300: Biquad,   // +0x4a8
    hp250: Biquad,   // +0x500
    lp1200: Biquad,  // +0x558
    hp3000: Biquad,  // +0x5b0
    lp9000: Biquad,  // +0x608
}

impl Filters {
    fn new(rate: u32) -> Self {
        let inv = 1.0 / f64::from(rate);
        Self {
            lp400: Biquad::lowpass(inv, w::F400),
            lp200a: Biquad::lowpass(inv, w::F200),
            lp200b: Biquad::lowpass(inv, w::F200),
            hp200a: Biquad::highpass(inv, w::F200),
            hp200b: Biquad::highpass(inv, w::F200),
            lp2000a: Biquad::lowpass(inv, w::F2000),
            lp2000b: Biquad::lowpass(inv, w::F2000),
            hp2000a: Biquad::highpass(inv, w::F2000),
            hp2000b: Biquad::highpass(inv, w::F2000),
            lp100: Biquad::lowpass(inv, w::F100),
            hp300: Biquad::highpass(inv, w::F300),
            lp3000: Biquad::lowpass(inv, w::F3000),
            hp1500: Biquad::highpass(inv, w::F1500),
            lp300: Biquad::lowpass(inv, w::F300),
            hp250: Biquad::highpass(inv, w::F250),
            lp1200: Biquad::lowpass(inv, w::F1200),
            hp3000: Biquad::highpass(inv, w::F3000),
            lp9000: Biquad::lowpass(inv, w::F9000),
        }
    }
}

pub struct WaveCreator {
    /// 1000 / gcd(rate, 1000) and rate / gcd(rate, 1000): one millisecond is `r / d` samples.
    d: u64,
    r: u64,
    /// Milliseconds in the track: ⌈L · 1000 / rate⌉ (+0x688).
    n_ms: u64,
    /// Milliseconds summarised so far (+0x668).
    done_ms: u64,
    /// Columns of the colour and 3-band scrolling waveforms: ⌈L · 150 / rate⌉ (+0x670, +0x8550).
    columns: usize,
    filters: Filters,
    /// Record A (+0x6a0): max, min, |LP400|, low, mid, high (band peak × share of samples won).
    rec_a: Vec<[i16; 6]>,
    /// Record B (+0x6a8): max, min, |LP100|, |300–3000|, |HP1500|.
    rec_b: Vec<[i16; 5]>,
    /// Record C (+0x6b0): |LP300|, |250–1200|, |3000–9000|.
    rec_c: Vec<[i16; 3]>,
    mono: Vec<f64>,
    b10: Vec<f64>,
    b18: Vec<f64>,
    b20: Vec<f64>,
    b28: Vec<f64>,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl WaveCreator {
    /// `WaveCreator(reader, capacity)` for a track of `len` samples at `rate`.
    pub fn new(rate: u32, len: u64) -> Self {
        let rate64 = u64::from(rate);
        let g = gcd(rate64, 1000);
        let n_ms = (rate64 + len * 1000 - 1) / rate64;
        // `fcvtpu(len · 150.0 / rate)`: rounds up.
        let columns = ((len as i32 as f64 * 150.0) / f64::from(rate)).ceil() as u32 as usize;
        Self {
            d: 1000 / g,
            r: rate64 / g,
            n_ms,
            done_ms: 0,
            columns,
            filters: Filters::new(rate),
            rec_a: vec![[0; 6]; n_ms as usize],
            rec_b: vec![[0; 5]; n_ms as usize],
            rec_c: vec![[0; 3]; n_ms as usize],
            mono: Vec::new(),
            b10: Vec::new(),
            b18: Vec::new(),
            b20: Vec::new(),
            b28: Vec::new(),
        }
    }

    /// Ranges of the milliseconds summarised from a block of `n` samples: (start, count) per
    /// millisecond, relative to the block. Milliseconds restart at every block; a trailing partial
    /// millisecond is dropped.
    fn ms_ranges(&self, n: usize) -> Vec<(usize, usize, u64)> {
        let (d, r) = (self.d, self.r);
        let per_block = (d * n as u64) / r;
        let remaining = self.n_ms - self.done_ms;
        let mut out = Vec::new();
        if d * (n as u64) < r || remaining == 0 {
            return out;
        }
        let mut i = 0u64;
        while i < per_block && i < remaining {
            let x1 = d - 1 + i * r;
            let x2 = d + r - 1 + i * r;
            let start = x1 / d;
            let x3 = i * r + (d + r - 1) - start * d;
            // Samples in this millisecond (the divisor of the band shares, before the ≥ 1 clamp).
            let samples = x3 / d;
            let count = (x2 / d - x1 / d).max(1);
            out.push((start as usize, count as usize, samples));
            i += 1;
        }
        out
    }

    /// `readWaveData`: one analysis block of 16-bit stereo samples.
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        if self.n_ms <= self.done_ms || pcm.is_empty() {
            return;
        }
        let n = pcm.len();
        self.mono.clear();
        for &[l, r] in pcm {
            let l = f64::from(f32::from(l)) * INV_32767;
            let r = f64::from(f32::from(r)) * INV_32767;
            let (al, ar) = (l.abs(), r.abs());
            let louder = if al > ar { l } else { r };
            let diff = (al - ar).abs();
            self.mono.push(if diff < MONO_EPS {
                louder
            } else {
                (r + l) * 0.5
            });
        }
        for b in [&mut self.b10, &mut self.b18, &mut self.b20, &mut self.b28] {
            b.clear();
            b.resize(n, 0.0);
        }
        let ranges = self.ms_ranges(n);
        let base = self.done_ms as usize;
        let f = &mut self.filters;

        // Record A.
        f.lp200a.process(&self.mono, &mut self.b10);
        f.lp200b.process(&self.b10, &mut self.b18);
        f.hp200a.process(&self.mono, &mut self.b10);
        f.hp200b.process(&self.b10, &mut self.b20);
        f.lp2000a.process(&self.b20, &mut self.b10);
        f.lp2000b.process(&self.b10, &mut self.b20);
        f.hp2000a.process(&self.mono, &mut self.b10);
        f.hp2000b.process(&self.b10, &mut self.b28);
        f.lp400.process(&self.mono, &mut self.b10);
        for (i, &(start, count, samples)) in ranges.iter().enumerate() {
            let (mut max, mut min) = (-f64::MAX, f64::MAX);
            let (mut lp, mut low, mut mid, mut high) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            let (mut won_low, mut won_mid, mut won_high) = (0i32, 0i32, 0i32);
            for j in start..start + count {
                let m = self.mono[j];
                max = fmaxnm(max, m);
                min = fminnm(min, m);
                lp = fmaxnm(lp, self.b10[j].abs());
                let (l, md, h) = (self.b18[j], self.b20[j], self.b28[j]);
                low = fmaxnm(low, l.abs());
                mid = fmaxnm(mid, md.abs());
                high = fmaxnm(high, h.abs());
                let top = if l < md { fmaxnm(md, h) } else { fmaxnm(l, h) };
                if top == l {
                    won_low += 1;
                } else if top == h {
                    won_high += 1;
                } else {
                    won_mid += 1;
                }
            }
            let cnt = samples as f64;
            let share = |peak: f64, won: i32| -> i16 {
                if peak < 1.0 {
                    (((peak * 32768.0) * f64::from(won)) / cnt) as i32 as i16
                } else {
                    0x7fff
                }
            };
            self.rec_a[base + i] = [
                signed_peak(max),
                signed_peak(min),
                magnitude(lp),
                share(low, won_low),
                share(mid, won_mid),
                share(high, won_high),
            ];
        }

        // Record B.
        f.lp100.process(&self.mono, &mut self.b18);
        f.hp300.process(&self.mono, &mut self.b10);
        f.lp3000.process(&self.b10, &mut self.b20);
        f.hp1500.process(&self.mono, &mut self.b28);
        for (i, &(start, count, _)) in ranges.iter().enumerate() {
            let (mut max, mut min) = (-f64::MAX, f64::MAX);
            let (mut low, mut mid, mut high) = (0.0f64, 0.0f64, 0.0f64);
            for j in start..start + count {
                max = fmaxnm(max, self.mono[j]);
                min = fminnm(min, self.mono[j]);
                low = fmaxnm(low, self.b18[j].abs());
                mid = fmaxnm(mid, self.b20[j].abs());
                high = fmaxnm(high, self.b28[j].abs());
            }
            self.rec_b[base + i] = [
                signed_peak(max),
                signed_peak(min),
                magnitude(low),
                magnitude(mid),
                magnitude(high),
            ];
        }

        // Record C.
        f.lp300.process(&self.mono, &mut self.b18);
        f.hp250.process(&self.mono, &mut self.b10);
        f.lp1200.process(&self.b10, &mut self.b20);
        f.hp3000.process(&self.mono, &mut self.b10);
        f.lp9000.process(&self.b10, &mut self.b28);
        for (i, &(start, count, _)) in ranges.iter().enumerate() {
            let (mut low, mut mid, mut high) = (0.0f64, 0.0f64, 0.0f64);
            for j in start..start + count {
                low = fmaxnm(low, self.b18[j].abs());
                mid = fmaxnm(mid, self.b20[j].abs());
                high = fmaxnm(high, self.b28[j].abs());
            }
            self.rec_c[base + i] = [magnitude(low), magnitude(mid), magnitude(high)];
        }
        let per_block = (self.d * n as u64) / self.r;
        self.done_ms += per_block;
    }

    /// Builds all waveforms (`createWave(true, true, true, true)`).
    pub fn finish(self) -> Output {
        let pwv4 = self.whole();
        let pwv5 = self.color();
        let (pwv6, pwvc) = self.bands_whole();
        let pwv7 = self.bands_zoom(pwvc);
        Output {
            pwv4,
            pwv5,
            pwv6,
            pwvc,
            pwv7,
        }
    }

    /// `createWholeWaveData` + `notifyWhole`: 1200 points of record A.
    fn whole(&self) -> Vec<[u8; 6]> {
        let n = self.n_ms as i64;
        let rec = &self.rec_a;
        let v = |i: i64, f: usize| f32::from(rec[i as usize][f]);
        let mut pts = vec![[0f32; 6]; 1200];
        let mut k = 0usize;
        let mut acc: i64 = 0;
        let mut pos: i64 = -1;
        let (mut max, mut min, mut lp, mut low, mut mid, mut high) =
            (-f32::MAX, f32::MAX, 0f32, 0f32, 0f32, 0f32);
        let mut i: i64 = 0;
        while k < 1200 {
            // Low share: the largest value of the ±12 ms window, tracked by its position.
            if pos >= 0 && i <= pos + 12 && i + 12 < n {
                let x = v(i + 12, 3);
                if fmaxnm32(low, x) == x {
                    low = x;
                    pos = i + 12;
                }
            } else {
                let back = i.min(12);
                for j in -back..13 {
                    let at = i + j;
                    if at >= n {
                        break;
                    }
                    let x = v(at, 3);
                    if fmaxnm32(low, x) == x {
                        low = x;
                        pos = at;
                    }
                }
            }
            // Mid share: the largest value of the previous, this and the next millisecond.
            let from = (i - 1).max(0);
            for at in from..(i + 2) {
                if at >= n {
                    break;
                }
                mid = fmaxnm32(mid, v(at, 4));
            }
            max = fmaxnm32(max, v(i, 0));
            min = fminnm32(min, v(i, 1));
            lp = fmaxnm32(lp, v(i, 2));
            high = fmaxnm32(high, v(i, 5));
            pts[k] = [max, min, lp, low, mid, high];
            if acc + 1200 < n {
                acc += 1200;
            } else {
                if k + 1 > 1199 {
                    break;
                }
                acc = acc + 1200 - n;
                k += 1;
                let mut reset = false;
                while k < 1200 {
                    if acc - n < 0 {
                        reset = true;
                        break;
                    }
                    pts[k] = [max, min, lp, low, mid, high];
                    k += 1;
                    acc -= n;
                }
                if reset {
                    pos = -1;
                    (max, min, lp, low, mid, high) = (-f32::MAX, f32::MAX, 0.0, 0.0, 0.0, 0.0);
                }
            }
            i += 1;
        }
        let scale = 1.0f32 / 32768.0;
        pts.iter()
            .map(|p| p.map(|x| ((x * 128.0) * scale) as i32 as u8))
            .collect()
    }

    /// `createColorWaveData`: one colour column per 1/150 s from record B.
    fn color(&self) -> Vec<u16> {
        let c = self.columns;
        let n = self.n_ms as i64;
        let rec = &self.rec_b;
        let v = |i: i64, f: usize| f32::from(rec[i as usize][f]);
        let mut amax = vec![0f32; c];
        let mut amin = vec![0f32; c];
        let mut alow = vec![0f32; c];
        let mut amid = vec![0f32; c];
        let mut ahigh = vec![0f32; c];
        // The arrays come from an uninitialised allocation; the peak scan below reads a column
        // before its first write. Fresh pages are zero, which is what this models.
        let mut peak: i32 = 0;
        let mut k = 0usize;
        let mut acc: i64 = 0;
        let mut pos: i64 = -1;
        let (mut max, mut min, mut low, mut mid, mut high) =
            (-f32::MAX, f32::MAX, 0f32, 0f32, 0f32);
        let mut i: i64 = 0;
        if c > 0 {
            loop {
                let h = fmaxnm32(amax[k].abs(), amin[k].abs()) as i32;
                let cand = h.min(0x7fff).max(peak);
                if pos >= 0 && i <= pos + 12 && i + 12 < n {
                    let x = v(i + 12, 2);
                    if fmaxnm32(low, x) == x {
                        low = x;
                        pos = i + 12;
                    }
                } else {
                    let back = i.min(12);
                    for j in -back..13 {
                        let at = i + j;
                        if at >= n {
                            break;
                        }
                        let x = v(at, 2);
                        if fmaxnm32(low, x) == x {
                            low = x;
                            pos = at;
                        }
                    }
                }
                let from = (i - 1).max(0);
                for at in from..(i + 2) {
                    if at >= n {
                        break;
                    }
                    mid = fmaxnm32(mid, v(at, 3));
                }
                max = fmaxnm32(max, v(i, 0));
                min = fminnm32(min, v(i, 1));
                high = fmaxnm32(high, v(i, 4));
                amax[k] = max;
                amin[k] = min;
                alow[k] = low;
                amid[k] = mid;
                ahigh[k] = high;
                if acc + c as i64 - n < 0 {
                    acc += c as i64;
                } else {
                    acc = acc + c as i64 - n;
                    k += 1;
                    if k < c {
                        loop {
                            if acc - n < 0 {
                                pos = -1;
                                (max, min, low, mid, high) = (-f32::MAX, f32::MAX, 0.0, 0.0, 0.0);
                                break;
                            }
                            amax[k] = max;
                            amin[k] = min;
                            alow[k] = low;
                            amid[k] = mid;
                            ahigh[k] = high;
                            k += 1;
                            acc -= n;
                            if k >= c {
                                break;
                            }
                        }
                    }
                }
                peak = i32::from(cand as i16);
                i += 1;
                if k >= c {
                    break;
                }
            }
        }
        let mut packed = vec![0u16; c];
        let mut heights = vec![0i16; c];
        for k in 0..c {
            let (r, g, b) = zoom_colour(alow[k], amid[k], ahigh[k]);
            heights[k] = (fmaxnm32(amax[k].abs(), amin[k].abs()) as i32).min(0x7fff) as i16;
            let (r3, g3, b3) = (r >> 5, g >> 5, b >> 5);
            let (rf, gf, bf) = if r.max(g).max(b) > 31 {
                (r3, g3, b3)
            } else {
                let m = r.max(g).max(b);
                (
                    if m != r { r3 } else { 1 },
                    if m != g { g3 } else { 1 },
                    if r.max(g) > b { b3 } else { 1 },
                )
            };
            packed[k] = u16::from(rf) | u16::from(gf) << 3 | u16::from(bf) << 6;
        }
        let mut out = vec![0u16; c];
        let inv = if peak >= 1 {
            Some(1.0 / f64::from(peak))
        } else {
            None
        };
        for k in 0..c {
            let p = packed[k];
            let h = match inv {
                Some(inv) => {
                    let t = ((f64::from(heights[k]) * 32767.0) * inv).trunc();
                    (((t * t) * SQUASH) as i32 & 0x1f) as u16
                }
                None => 0,
            };
            let (r, g, b) = (p & 7, (p >> 3) & 7, (p >> 6) & 7);
            out[k] = r << 13 | g << 10 | b << 7 | h << 2;
        }
        out
    }

    /// `create3BandsWholeWaveData`: PWV6 (1200 × low/mid/high) and the PWVC coefficients.
    fn bands_whole(&self) -> (Vec<[u8; 3]>, [u16; 3]) {
        let n = self.n_ms as i64;
        let rec = &self.rec_c;
        let back = [n / 1000, (n / 1000 * 2) / 3, n / 3000];
        let mut avg = [vec![0f64; 1200], vec![0f64; 1200], vec![0f64; 1200]];
        let mut count = [0i64; 3];
        let mut sum = [0f64; 3];
        let mut top = [0f64; 3];
        let mut k: i64 = 0;
        let mut acc: i64 = 0;
        let mut i: i64 = 0;
        loop {
            for b in 0..3 {
                let s = i - back[b];
                let from = if count[b] == 0 {
                    count[b] = if s < 0 { -s } else { 0 };
                    s.max(0)
                } else {
                    i
                };
                if from <= i && from < n {
                    let mut j = from;
                    loop {
                        let x = f64::from(rec[j as usize][b]);
                        sum[b] += x;
                        if top[b] <= x {
                            top[b] = x;
                        }
                        count[b] += 1;
                        if j >= i {
                            break;
                        }
                        j += 1;
                        if j >= n {
                            break;
                        }
                    }
                }
            }
            let cur = [
                sum[0] / count[0] as f64,
                sum[1] / count[1] as f64,
                sum[2] / count[2] as f64,
            ];
            for b in 0..3 {
                avg[b][k as usize] = cur[b];
            }
            if acc + 1200 - n < 0 {
                acc += 1200;
            } else {
                acc = acc + 1200 - n;
                if k >= 1199 {
                    k += 1;
                } else {
                    loop {
                        if acc - n < 0 {
                            count = [0; 3];
                            sum = [0.0; 3];
                            k += 1;
                            break;
                        }
                        for b in 0..3 {
                            avg[b][(k + 1) as usize] = cur[b];
                        }
                        k += 1;
                        acc -= n;
                        if k == 1199 {
                            k += 1;
                            break;
                        }
                    }
                }
            }
            i += 1;
            if k > 1199 {
                break;
            }
        }
        // Totals with the vector loop's association: eight running sums, combined as
        // ((s0 + s2) + s4) + s6 and ((s1 + s3) + s5) + s7, then added.
        let total = |a: &[f64]| -> f64 {
            let mut s = [0f64; 8];
            for chunk in a.chunks_exact(8) {
                for (acc, &x) in s.iter_mut().zip(chunk) {
                    *acc += x;
                }
            }
            (((s[0] + s[2]) + s[4]) + s[6]) + (((s[1] + s[3]) + s[5]) + s[7])
        };
        let (s_low, s_mid, s_high) = (total(&avg[0]), total(&avg[1]), total(&avg[2]));
        let (c_low, c_mid, c_high);
        if s_low > 0.0 && s_mid > 0.0 && s_high > 0.0 {
            let p_hm = s_high * s_mid;
            let p_hl = s_high * s_low;
            let p_ml = s_mid * s_low;
            let m = if p_hl < p_hm {
                fminnm(p_ml, p_hl)
            } else {
                fminnm(p_ml, p_hm)
            };
            let inv = 1.0 / m;
            c_low = p_hm * inv;
            c_mid = p_hl * inv;
            c_high = p_ml * inv;
        } else if s_low > 0.0 && s_mid > 0.0 {
            let m = fminnm(s_low, s_mid);
            c_low = s_mid / m;
            c_high = 1.0;
            c_mid = s_low / m;
        } else if s_low > 0.0 && s_high > 0.0 {
            let m = fminnm(s_low, s_high);
            c_low = s_high / m;
            c_mid = 1.0;
            c_high = s_low / m;
        } else {
            c_low = 1.0;
            let m = fminnm(s_mid, s_high);
            let (a, b) = if s_high <= 0.0 {
                (1.0, 1.0)
            } else {
                (s_mid / m, s_high / m)
            };
            c_high = if s_mid <= 0.0 { 1.0 } else { a };
            c_mid = if s_mid <= 0.0 { 1.0 } else { b };
        }
        let mut peak = 0f64;
        for ((&l, &m), &h) in avg[0].iter().zip(&avg[1]).zip(&avg[2]) {
            let x = m.mul_add(c_mid, l * c_low);
            peak = fmaxnm(peak, h.mul_add(c_high, x));
        }
        let (g_low, g_mid, g_high) = if peak > 0.0 {
            let inv = 1.0 / peak;
            (
                (c_low * 32768.0) * inv,
                (c_mid * 32768.0) * inv,
                (c_high * 32768.0) * inv,
            )
        } else {
            (c_low, c_mid, c_high)
        };
        let hi_level = (-(PI_32768 * top[2]).cos()).mul_add(16384.0, 16384.0);
        let coef = |g: f64, level: f64, cap: f64| -> u16 {
            let g = if level > 0.0 {
                fminnm(g, 32768.0 / level)
            } else {
                g
            };
            let x = if g < 0.8 {
                80.0
            } else {
                fminnm(g, cap) * 100.0
            };
            x as i32 as u16
        };
        let pwvc = [
            coef(g_low, top[0], 3.0),
            coef(g_mid, top[1], 3.0),
            coef(g_high, hi_level, 5.0),
        ];
        let m_low = fminnm(fmaxnm(g_low, 0.8), 3.0) * 0.00390625;
        let m_mid = fminnm(fmaxnm(g_mid, 0.8), 3.0) * 0.00390625;
        let m_high = fminnm(fmaxnm(g_high, 0.8), 5.0) * 0.00390625;
        let pwv6 = (0..1200)
            .map(|k| {
                [
                    (avg[0][k] * m_low) as i64 as i32 as u8,
                    (avg[1][k] * m_mid) as i64 as i32 as u8,
                    (avg[2][k] * m_high) as i64 as i32 as u8,
                ]
            })
            .collect();
        (pwv6, pwvc)
    }

    /// `create3BandsZoomWaveData`: PWV7, decaying peak envelopes of record C, scaled by PWVC.
    fn bands_zoom(&self, coef: [u16; 3]) -> Vec<[u8; 3]> {
        let c = self.columns;
        let n = self.n_ms as i64;
        let rec = &self.rec_c;
        let mut out = vec![[0u8; 3]; c];
        if c == 0 {
            return out;
        }
        let g = |x: u16| f64::from(f32::from(x) * 0.01f32);
        let s_low = g(coef[0]) * 0.00390625;
        let s_mid = g(coef[1]) * 0.00390625;
        let s_high = g(coef[2]);
        // Per band: record field, warm-up length, decay.
        const BANDS: [(usize, i64, f64); 3] = [
            (0, 300, f64::from_bits(0x3fef_ae14_7ae1_47ae)),
            (1, 200, f64::from_bits(0x3fef_5c28_f5c2_8f5c)),
            (2, 100, f64::from_bits(0x3fef_0a3d_70a3_d70a)),
        ];
        let mut env = [0f64; 3];
        let mut run = [0i64; 3];
        let mut k = 0usize;
        let mut acc: i64 = 0;
        let mut i: i64 = 0;
        while k < c {
            for (b, &(field, warm, decay)) in BANDS.iter().enumerate() {
                let from = if run[b] == 0 {
                    let s = i - warm;
                    env[b] = if s < 0 {
                        0.0
                    } else {
                        f64::from(rec[s as usize][field])
                    };
                    s
                } else {
                    i
                };
                for j in from..=i {
                    let x = if j < 0 {
                        0.0
                    } else {
                        f64::from(rec[j as usize][field])
                    };
                    if env[b] < x {
                        env[b] = x;
                    } else if env[b] > x {
                        env[b] = (env[b] - x).mul_add(decay, x);
                    }
                }
                run[b] += i + 1 - from;
            }
            let lo = f64::from(env[0] as f32);
            let mi = f64::from(env[1] as f32);
            let hi = f64::from(env[2] as f32);
            let col = [
                (s_low * lo) as i32 as u8,
                (s_mid * mi) as i32 as u8,
                (((-(hi * PI_32768).cos()).mul_add(64.0, 64.0)) * s_high) as i32 as u8,
            ];
            out[k] = col;
            if acc + c as i64 - n < 0 {
                acc += c as i64;
            } else {
                acc = acc + c as i64 - n;
                k += 1;
                if k < c {
                    loop {
                        if acc - n < 0 {
                            run = [0; 3];
                            env = [0.0; 3];
                            break;
                        }
                        out[k] = col;
                        k += 1;
                        acc -= n;
                        if k >= c {
                            break;
                        }
                    }
                }
            }
            i += 1;
        }
        out
    }
}

/// `juce::Colour` from `WaveCreator::calcZoomWaveColour` @0x100d573d4 (low, mid, high peaks).
fn zoom_colour(a: f32, b: f32, c: f32) -> (u8, u8, u8) {
    let m = if a < b {
        fmaxnm32(b, c)
    } else {
        fmaxnm32(a, c)
    };
    if m == 0.0 {
        return (255, 255, 255);
    }
    let k = 1.0f32 / m;
    let mut cc = (c * 255.0) * k;
    cc = fmaxnm32(fminnm32(cc, 255.0), 0.0);
    let mut aa = (a * 255.0) * k;
    let mut bb = (b * 255.0) * k;
    if cc < 64.0 {
        let t = cc * f32::from_bits(0xbc80_0000);
        let s = (bb * f32::from_bits(0x3aab_5602)) * aa;
        let d = t.mul_add(s, s);
        aa -= d;
        bb -= d;
    }
    let lim = fminnm32(fmaxnm32(aa, cc), 128.0);
    let s = bb * f32::from_bits(0xbb19_999a);
    bb = s.mul_add(lim, bb);
    cc *= f32::from_bits(0x3fa6_6666);
    let clamp = |x: f32| fmaxnm32(fminnm32(x, 255.0), 0.0) as i32 as u8;
    (clamp(aa), clamp(bb), clamp(cc))
}

/// WaveCreator's waveforms.
pub struct Output {
    pub pwv4: Vec<[u8; 6]>,
    pub pwv5: Vec<u16>,
    pub pwv6: Vec<[u8; 3]>,
    pub pwvc: [u16; 3],
    pub pwv7: Vec<[u8; 3]>,
}
