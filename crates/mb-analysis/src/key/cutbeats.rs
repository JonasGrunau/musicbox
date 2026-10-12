//! `BeatAnalyzer_1_0::CutBeats` @0x101144d00 (`analysis-key.md` § 3): an STFT that drops the
//! bins below a fraction of their octave-wide neighbourhood, smooths the lowest 20 bins over
//! time, and resynthesises. Only the key path's mode 1 is ported.

use super::fft::{calc_mag, calc_phase_sincos, fft, hanning, make_bitrev, make_sintbl};

/// Sum of `v` in the binary's order: 16-element chunks into four 4-lane accumulators, the
/// lane-wise reduction `a3 + (a2 + (a1 + a0))`, `(l0 + l1) + (l2 + l3)`, then the rest one by
/// one.
pub(super) fn neon_sum(v: &[f32]) -> f32 {
    let chunks = v.len() / 16;
    let mut s = 0f32;
    if chunks > 0 {
        let mut acc = [[0f32; 4]; 4];
        for c in v[..chunks * 16].chunks_exact(16) {
            for q in 0..4 {
                for l in 0..4 {
                    acc[q][l] += c[q * 4 + l];
                }
            }
        }
        let mut r = [0f32; 4];
        for l in 0..4 {
            r[l] = acc[3][l] + (acc[2][l] + (acc[1][l] + acc[0][l]));
        }
        s = (r[0] + r[1]) + (r[2] + r[3]);
    }
    for &x in &v[chunks * 16..] {
        s += x;
    }
    s
}

/// One-pole low-pass `make_lpf0` @0x10113f844: `(gain, pole)`.
fn make_lpf0(fc: f32) -> (f32, f32) {
    let pole = (f64::from(fc) * -std::f64::consts::TAU).exp() as f32;
    (1.0 - pole, pole)
}

/// `filter1` @0x10113f7d8: `y = x·gain + s` (fused), `s = y·pole`.
fn filter1(x: &[f32], y: &mut [f32], gain: f32, pole: f32) {
    let mut s = 0f32;
    for (o, &v) in y.iter_mut().zip(x) {
        let t = v.mul_add(gain, s);
        s = t * pole;
        *o = t;
    }
}

/// `CutBeats(dst, src, n, rate, decim, 1)`: writes `dst[..n]`.
pub fn cut_beats(dst: &mut [f32], src: &[f32], n: usize, rate: f32, decim: i32) {
    let fft_n = (4096 / decim) as usize;
    let h = fft_n / 2;
    let base = if rate * 30.0 > n as f32 { 512 } else { 1024 };
    let hop = (base / decim) as usize;
    let frames = (n as i64 - fft_n as i64) / hop as i64;
    let frames = frames.max(0) as usize;

    let mut lo = vec![0usize; h];
    let mut hi = vec![0usize; h];
    let mut w = vec![0f32; h];
    for k in 0..h {
        let l = (k as f32 * f32::from_bits(0x3f350565)) as i32;
        let mut u = (k as f32 * f32::from_bits(0x3fb50481)) as i32;
        if (h as i32) <= u {
            u = h as i32 - 1;
        }
        lo[k] = l as usize;
        hi[k] = u as usize;
        let d = u - l;
        let num = (k as f32 + 1.0).log10();
        w[k] = if d != 0 { num / d as f32 } else { num };
    }
    let mut win = vec![0f32; fft_n];
    hanning(fft_n, &mut win);
    let tbl = make_sintbl(fft_n);
    let rev = make_bitrev(fft_n);

    let mut mag = vec![0f32; frames * h];
    let mut sin = vec![0f32; frames * h];
    let mut cos = vec![0f32; frames * h];
    let mut re = vec![0f32; fft_n + 1];
    let mut im = vec![0f32; fft_n + 1];
    let mut band = vec![0f32; h];
    let band_end = h.max(5);
    for f in 0..frames {
        let p = f * hop;
        for i in 0..fft_n {
            re[i] = win[i] * src[p + i];
            im[i] = 0.0;
        }
        fft(&mut re[..fft_n], &mut im[..fft_n], false, &tbl, &rev);
        let row = f * h..(f + 1) * h;
        calc_mag(&mut mag[row.clone()], &re[..h], &im[..h]);
        calc_phase_sincos(
            &mut sin[row.clone()],
            &mut cos[row.clone()],
            &re[..h],
            &im[..h],
        );
        if fft_n >= 10 {
            let m = &mut mag[row];
            for k in 4..band_end {
                band[k] = if hi[k] >= lo[k] {
                    neon_sum(&m[lo[k]..=hi[k]])
                } else {
                    0.0
                };
            }
            for k in 4..band_end {
                if m[k] < w[k] * band[k] {
                    m[k] = 0.0;
                }
            }
        }
    }

    // Lowest 20 bins: one-pole low-pass over time, keep the smaller value.
    let (gain, pole) = make_lpf0(((fft_n / hop) as f32 * 5.0) / rate);
    let mut x = vec![0f32; frames];
    let mut y = vec![0f32; frames];
    for b in 0..20.min(h) {
        for f in 0..frames {
            x[f] = mag[f * h + b];
        }
        filter1(&x, &mut y, gain, pole);
        for f in 0..frames {
            if y[f] < mag[f * h + b] {
                mag[f * h + b] = y[f];
            }
        }
    }

    // Resynthesis (overlap-add).
    dst[..n].fill(0.0);
    let scale = hop as f32 * f32::from_bits(0x402aaaab);
    let inv = 1.0 / (fft_n as f32 * fft_n as f32);
    for f in 0..frames {
        re.fill(0.0);
        im.fill(0.0);
        if fft_n > 1 {
            for k in 0..h {
                let m = mag[f * h + k];
                let r = cos[f * h + k] * m;
                let s = sin[f * h + k] * m;
                re[k] = r;
                im[k] = s;
                re[fft_n - k] = r;
                im[fft_n - k] = -s;
            }
        }
        fft(&mut re[..fft_n], &mut im[..fft_n], true, &tbl, &rev);
        let p = f * hop;
        for i in 0..fft_n {
            let t = (re[i] * scale) * win[i];
            dst[p + i] = t.mul_add(inv, dst[p + i]);
        }
    }
}
