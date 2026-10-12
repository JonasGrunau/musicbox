//! Quarter-beat low-band energies: `LowPowerAnalyze16` @0x1010c8db0,
//! `LowPowerAnalyze16_b` @0x101bbce64 and `LowPowerAnalyze16forBarCorr` @0x101365734
//! (`analysis-phrase.md` § 4.3).

use super::filter::{prewarp, CFilter, SQRT2, TWO_PI};
use super::mat;

fn lowpass(x: &[f64], w: f64, fs: f64) -> Vec<f64> {
    let mut y = x.to_vec();
    CFilter::low2(prewarp(w, fs), SQRT2, fs).filter2(&mut y);
    y
}

/// `square_sum` of `y[lo .. lo + n]`; `None` where `link_from` fails (`n < 0`).
fn energy(y: &[f64], lo: i64, n: i32) -> Option<f64> {
    if n < 0 || lo < 0 {
        return None;
    }
    let lo = lo as usize;
    let hi = (lo + n as usize).min(y.len());
    Some(mat::square_sum(&y[lo.min(hi)..hi]))
}

/// The four quarter energies of each beat, from 1-based beat starts (`LowPowerAnalyze16*`).
fn per_beat(y: &[f64], beats: &[i32], q: i32, last: impl Fn(usize) -> i32) -> Option<Vec<f64>> {
    let mut out = Vec::with_capacity(beats.len() * 4);
    for (i, &b) in beats.iter().enumerate() {
        let next = last(i);
        let b = b as i64;
        let q64 = q as i64;
        out.push(energy(y, b - 1, q)?);
        out.push(energy(y, b + q64 - 1, q)?);
        out.push(energy(y, b + 2 * q64 - 1, q)?);
        let s = b + 3 * q64;
        out.push(energy(y, s - 1, (next as i64 - s) as i32)?);
    }
    Some(out)
}

/// `LowPowerAnalyze16(x, bpm, fb, fs, beats)`: 100 Hz low-pass quarter energies (×4 unless
/// `fs ≈ 5512.5`) and their per-bar sums (+0x3f0).
pub fn low_power16(x: &[f64], bpm: f64, fs: f64, beats: &[i32]) -> Option<Vec<f64>> {
    let y = lowpass(x, 628.3185307179587, fs);
    let p = (fs * 60.0) / bpm;
    let q = p.mul_add(0.25, 0.5) as i32;
    let n = beats.len();
    let mut e = per_beat(&y, beats, q, |i| {
        if i + 2 < n { beats[i + 1] } else { (p + 0.5) as i32 - 1 + beats[i] }
    })?;
    if fs <= 5512.45 || 5512.55 <= fs {
        for v in &mut e {
            *v *= 4.0;
        }
    }
    let bars = (n as i32 / 4) as usize;
    Some(
        (0..bars)
            .map(|j| e[j * 16..j * 16 + 16].iter().fold(0.0, |acc, &v| acc + v))
            .collect(),
    )
}

/// `LowPowerAnalyze16_b(x, bpm, ms, fs)` after `…_b_init(fs, fc)`: quarter energies on the
/// beat grid starting at `ms`, ×4.
pub fn low_power16_b(x: &[f64], fc: f64, bpm: f64, ms: i32, fs: f64) -> Option<Vec<f64>> {
    let y = lowpass(x, fc * TWO_PI, fs);
    let off = (ms as f64 * 0.001).mul_add(fs, 0.5) as i32;
    let p = (fs * 60.0) / bpm;
    let pi = (p + 0.5) as i32;
    let n = mat::cdiv(y.len() as i32 - off, pi);
    let q = p.mul_add(0.25, 0.5) as i32;
    let mut out = Vec::with_capacity(n.max(0) as usize * 4);
    for i in 0..n.max(0) {
        let base = off as i64 + i as i64 * pi as i64;
        let q64 = q as i64;
        out.push(energy(&y, base, q)?);
        out.push(energy(&y, base + q64, q)?);
        out.push(energy(&y, base + 2 * q64, q)?);
        out.push(energy(&y, base + 3 * q64, pi - 3 * q)?);
    }
    for v in &mut out {
        *v *= 4.0;
    }
    Some(out)
}

/// `LowPowerAnalyze16forBarCorr(x, bpm, fb, fs, beats)`: 100 Hz quarter energies per beat, ×4.
pub fn low_power16_bar(x: &[f64], bpm: f64, fs: f64, beats: &[i32]) -> Option<Vec<f64>> {
    let y = lowpass(x, 628.3185307179587, fs);
    let p = (fs * 60.0) / bpm;
    let q = p.mul_add(0.25, 0.5) as i32;
    let n = beats.len();
    let mut e = per_beat(&y, beats, q, |i| {
        if i + 1 < n { beats[i + 1] } else { (p + 0.5) as i32 - 1 + beats[i] }
    })?;
    for v in &mut e {
        *v *= 4.0;
    }
    Some(e)
}
