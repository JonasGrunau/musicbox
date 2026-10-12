//! `BeatAnalyzer_1_0::PitchDetect` @0x101169898 (`analysis-key.md` § 4): the tuning reference
//! (434…446 Hz) whose semitone filter bank gives the steadiest strongest tone. Mode 1 only.

use super::fft::hanning;
use super::libm::sincosf;

/// `2^(k/12)` for `k = 1..24` (f32 table @0x1037c7680).
const RATIO_BITS: [u32; 24] = [
    0x3f879c7d, 0x3f8facd6, 0x3f9837f0, 0x3fa14518, 0x3faadc08, 0x3fb504f3, 0x3fbfc887, 0x3fcb2ff5,
    0x3fd744fd, 0x3fe411f1, 0x3ff1a1bf, 0x40000000, 0x40079c7d, 0x400facd7, 0x401837f0, 0x40214518,
    0x402adc09, 0x403504f3, 0x403fc887, 0x404b2ff6, 0x405744fd, 0x406411f1, 0x4071a1c0, 0x40800000,
];

pub(super) fn semitone_ratio(k: usize) -> f32 {
    f32::from_bits(RATIO_BITS[k - 1])
}

/// `Σ kernel·x` in the binary's order: 16-element chunks into four 4-lane accumulators with
/// fused multiply-adds, the reduction `a3 + (a2 + (a1 + a0))` and `(l0 + l1) + (l2 + l3)`, then
/// the remaining products fused one by one.
pub(super) fn neon_dot(kernel: &[f32], x: &[f32]) -> f32 {
    let n = kernel.len();
    let chunks = n / 16;
    let mut s = 0f32;
    if chunks > 0 {
        let mut acc = [[0f32; 4]; 4];
        for c in 0..chunks {
            for q in 0..4 {
                for l in 0..4 {
                    let i = c * 16 + q * 4 + l;
                    acc[q][l] = kernel[i].mul_add(x[i], acc[q][l]);
                }
            }
        }
        let mut r = [0f32; 4];
        for l in 0..4 {
            r[l] = acc[3][l] + (acc[2][l] + (acc[1][l] + acc[0][l]));
        }
        s = (r[0] + r[1]) + (r[2] + r[3]);
    }
    for i in chunks * 16..n {
        s = kernel[i].mul_add(x[i], s);
    }
    s
}

struct Bank {
    n0: usize,
    lens: [usize; 24],
    w: [f32; 24],
    cos: Vec<f32>,
    sin: Vec<f32>,
}

/// The 24 semitone kernels for reference `f0`.
fn bank(f0: f32, two_rate: f32, inv_rate: f32) -> Bank {
    let mut freqs = [0f32; 25];
    freqs[0] = f0;
    for k in 1..25 {
        freqs[k] = semitone_ratio(k) * f0;
    }
    let n0 = (two_rate / (f0 * f32::from_bits(0x3d738fa0))) as i32;
    let n0 = n0.max(0) as usize;
    let t: Vec<f32> = (0..n0).map(|i| (i + 1) as f32 * inv_rate).collect();
    let mut cos = vec![0f32; n0 * 24];
    let mut sin = vec![0f32; n0 * 24];
    let mut lens = [0usize; 24];
    let mut w = [0f32; 24];
    let mut hann = vec![0f32; n0];
    for k in 0..24 {
        let (nk, f) = if k == 0 {
            (n0 as i32, f0)
        } else {
            ((two_rate / (freqs[k + 1] - freqs[k])) as i32, freqs[k])
        };
        let nk = nk.max(0) as usize;
        lens[k] = nk;
        w[k] = if k == 0 { 1.0 } else { n0 as f32 / nk as f32 };
        if hann.len() < nk {
            hann.resize(nk, 0.0);
        }
        hanning(nk, &mut hann);
        let omega = f * f32::from_bits(0x40c90fdb);
        for i in 0..nk {
            let (s, c) = sincosf(t[i] * omega);
            cos[k * n0 + i] = hann[i] * c;
            sin[k * n0 + i] = hann[i] * s;
        }
    }
    Bank {
        n0,
        lens,
        w,
        cos,
        sin,
    }
}

/// `PitchDetect(x, n, rate, decim, 1)`: the reference frequency in Hz.
pub fn pitch_detect(x: &[f32], n: usize, rate: f32, decim: i32) -> i32 {
    let short = rate * 30.0 > n as f32;
    let (start, step, end, hop_num) = if short {
        (2, 2, 11, 0x800)
    } else {
        (0, 1, 13, 0xc00)
    };
    let hop = (hop_num / decim) as usize;
    let two_rate = rate + rate;
    let inv_rate = 1.0 / rate;
    let mut scores = [0f32; 13];
    let mut prev_arg = 0usize;
    let pass = |u: usize, scores: &mut [f32; 13], prev_arg: &mut usize| {
        let f0 = u as f32 + 434.0;
        let b = bank(f0, two_rate, inv_rate);
        let span = n as i64 - b.n0 as i64;
        let mut p = 0usize;
        while (p as i64) < span {
            let mut max = f32::from_bits(0x0080_0000);
            let mut arg = 0usize;
            let mut prev = *prev_arg;
            for k in 0..24 {
                let nk = b.lens[k];
                let (re, im) = if nk < 1 {
                    (0.0, 0.0)
                } else {
                    let xs = &x[p..p + nk];
                    (
                        neon_dot(&b.cos[k * b.n0..k * b.n0 + nk], xs),
                        neon_dot(&b.sin[k * b.n0..k * b.n0 + nk], xs),
                    )
                };
                let mag = b.w[k] * im.mul_add(im, re * re).sqrt();
                if max < mag {
                    arg = k;
                    if p == 0 {
                        prev = k;
                    }
                }
                max = max.max(mag);
            }
            if arg == prev {
                scores[u] += max;
            }
            *prev_arg = arg;
            p += hop;
        }
    };
    let mut u = start;
    while u < end {
        pass(u, &mut scores, &mut prev_arg);
        u += step;
    }
    if short {
        let mut idx: Vec<usize> = (0..13).collect();
        let sorted = scores;
        idx.sort_by(|&a, &b| {
            sorted[b]
                .partial_cmp(&sorted[a])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        let (best, second) = (idx[0], idx[1]);
        let (lo, step, hi) = if best == 2 {
            (0, 1, 2)
        } else if best == 10 {
            (11, 1, 13)
        } else {
            (best.min(second) + 1, 2, best.max(second))
        };
        if hi > lo || best == 2 || best == 10 {
            let mut u = lo;
            loop {
                pass(u, &mut scores, &mut prev_arg);
                u += step;
                if u >= hi {
                    break;
                }
            }
        }
    }
    // max_v: the first maximum.
    let mut arg = 0;
    let mut m = scores[0];
    for (i, &s) in scores.iter().enumerate().skip(1) {
        let next = if m <= s { s } else { m };
        if !(s <= m) {
            arg = i;
        }
        m = next;
    }
    434 + arg as i32
}
