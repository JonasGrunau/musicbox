//! `BeatAnalyzer_1_0::resample2` @0x101146498: the key buffer's anti-alias filter and
//! decimation (`analysis-key.md` § 1). An 8th-order IIR low-pass whose state starts at zero on
//! every call (rekordbox calls it once per analysis block), then every `d`-th output from
//! index 7.

/// Feedback coefficients (f32 constants widened to f64), applied to `w[n-1] .. w[n-8]`.
const A: [f64; 8] = [
    f32_as_f64(0x40198fbbc0000000),
    f32_as_f64(0xc032001620000000),
    f32_as_f64(0x403d2bcd20000000),
    f32_as_f64(0xc03dbb3b60000000),
    f32_as_f64(0x4033817120000000),
    f32_as_f64(0xc02014fd60000000),
    f32_as_f64(0x3ffe756d80000000),
    f32_as_f64(0xbfc95b5760000000),
];
/// Feed-forward: `b0` is applied in f32, the rest in f64.
const B0: f32 = f32::from_bits(0x343d40b5);
const B1: f64 = f32_as_f64(0x3eb7a816a0000000);
const B2: f64 = f32_as_f64(0x3ed4b313e0000000);
const B3: f64 = f32_as_f64(0x3ee4b313e0000000);
const B4: f64 = f32_as_f64(0x3ee9dfd8c0000000);
const B8: f64 = f32_as_f64(0x3e87a816a0000000);

const fn f32_as_f64(bits: u64) -> f64 {
    f64::from_bits(bits)
}

/// Filters `src` into `scratch` and writes `dst[j] = scratch[7 + j·decim]`.
pub fn resample2(dst: &mut [f32], src: &[f32], decim: usize, scratch: &mut Vec<f32>) {
    scratch.clear();
    scratch.reserve(src.len());
    // w[0] = w[n-1] … w[7] = w[n-8]
    let mut w = [0f64; 8];
    for &x in src {
        let mut acc = w[0] * A[0];
        for k in 1..8 {
            acc = w[k].mul_add(A[k], acc);
        }
        let w0 = (acc + f64::from(x)) as f32;
        let p0 = f64::from(w0 * B0);
        let mut t = w[3] * B4;
        t = (w[4] + w[2]).mul_add(B3, t);
        t = (w[5] + w[1]).mul_add(B2, t);
        t = w[7].mul_add(B8, t);
        t = (w[6] + w[0]).mul_add(B1, t);
        scratch.push((t + p0) as f32);
        w.copy_within(0..7, 1);
        w[0] = f64::from(w0);
    }
    for (j, d) in dst.iter_mut().enumerate() {
        *d = scratch[7 + j * decim];
    }
}
