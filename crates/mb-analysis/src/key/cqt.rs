//! The front of `BeatAnalyzer_1_0::KeyCodeScale` @0x101158438 (`analysis-key.md` § 5.1–5.5): a
//! 72-tone constant-Q transform (six octaves from `pitch/16`), normalised, smoothed over time,
//! folded into a 12-pitch-class profile.

use super::fft::hanning;
use super::libm::sincosf;
use super::pitch::neon_dot;

/// The kernel tables `KeyCodeScale` builds on its first call of a job (`seg_index == 0`) and
/// reuses afterwards (rekordbox keeps them in the job's four 6 MB buffers).
#[derive(Debug, Clone)]
pub struct Bank {
    /// Tap counts as rekordbox stores them (floats).
    pub len: [f32; 72],
    pub w: [f32; 72],
    /// Hann-windowed cosine / sine taps; tone `k` at `(int)(len[0]·k)`.
    pub cos: Vec<f32>,
    pub sin: Vec<f32>,
    /// Frame hop in samples (`*param_14`).
    pub hop: i32,
}

impl Default for Bank {
    fn default() -> Self {
        Self {
            len: [0.0; 72],
            w: [0.0; 72],
            cos: Vec::new(),
            sin: Vec::new(),
            hop: 0x800,
        }
    }
}

/// The 73 tone frequencies `f_i = exp2f(i/12)·(pitch/16)`, `f_72 = 4·pitch`.
fn freqs(pitch: f32) -> [f32; 73] {
    let base = pitch * 0.0625;
    let mut f = [0f32; 73];
    for (i, v) in f.iter_mut().enumerate().take(72) {
        *v = (i as f32 * f32::from_bits(0x3daaaaab)).exp2() * base;
    }
    f[72] = pitch * 4.0;
    f
}

/// Builds the bank (`seg_index == 0`); `segment` is `param_10`.
pub fn build_bank(bank: &mut Bank, n: usize, rate: f32, pitch: f32, segment: bool) {
    let short = (n as f32) < rate * 30.0;
    bank.hop = if short && !segment {
        0x800 / 8
    } else if (rate as i32) == 4000 {
        if short { 0x200 } else { 0x400 }
    } else {
        0x800
    };
    let f = freqs(pitch);
    let two_rate = rate + rate;
    let n0f = two_rate / (f[1] - f[0]);
    let n0 = n0f as u32 as usize;
    bank.len[0] = n0f as i32 as f32;
    let inv = 1.0 / rate;
    let t: Vec<f32> = (0..n0).map(|i| (i + 1) as f32 * inv).collect();
    let mut hann = vec![0f32; n0];
    hanning(n0, &mut hann);
    let n0s = bank.len[0] as u32 as usize;
    bank.cos = vec![0f32; n0s * 72];
    bank.sin = vec![0f32; n0s * 72];
    let omega = f[0] * f32::from_bits(0x40c90fdb);
    for i in 0..n0s {
        let (s, c) = sincosf(t[i] * omega);
        bank.cos[i] = hann[i] * c;
        bank.sin[i] = hann[i] * s;
    }
    bank.w[0] = 1.0;
    for k in 1..72 {
        let q = two_rate / (f[k + 1] - f[k]);
        bank.len[k] = q as i32 as f32;
        let nk_h = (q as i32).max(0) as usize;
        if hann.len() < nk_h {
            hann.resize(nk_h, 0.0);
        }
        hanning(nk_h, &mut hann);
        let nk = bank.len[k] as u32 as usize;
        let off = (bank.len[0] * k as f32) as i32 as usize;
        let omega = f[k] * f32::from_bits(0x40c90fdb);
        for i in 0..nk {
            let (s, c) = sincosf(t[i] * omega);
            bank.cos[off + i] = hann[i] * c;
            bank.sin[off + i] = hann[i] * s;
        }
        bank.w[k] = bank.len[0] / bank.len[k];
    }
}

/// The constant-Q magnitudes `C[k][f]` (tone-major) plus their maximum.
pub fn cqt(x: &[f32], n: usize, bank: &Bank) -> (Vec<f32>, usize, f32) {
    let n0f = bank.len[0];
    let frames = ((n as f32 - n0f) / bank.hop as f32) as i32;
    let frames = frames.max(0) as usize;
    let mut m = vec![0f32; frames * 72];
    let mut max = 0f32;
    let half0 = (n0f * 0.5) as i32;
    for f in 0..frames {
        let mut off: i32 = 0;
        for k in 0..72 {
            let nk_f = bank.len[k];
            let half = (nk_f * 0.5) as i32;
            let start = f as i32 * bank.hop + half0 - half;
            let (re, im) = if start < 0 {
                (0.0, 0.0)
            } else {
                let nk = nk_f as i32;
                if nk < 1 {
                    (0.0, 0.0)
                } else {
                    let nk = nk as usize;
                    let s = start as usize;
                    let o = off as usize;
                    let xs = &x[s..s + nk];
                    (
                        neon_dot(&bank.cos[o..o + nk], xs),
                        neon_dot(&bank.sin[o..o + nk], xs),
                    )
                }
            };
            let mag = re.mul_add(re, im * im).sqrt() * bank.w[k];
            m[f + k * frames] = mag;
            max = max.max(mag);
            off = (n0f + off as f32) as i32;
        }
    }
    (m, frames, max)
}

/// `make_lpf1(1, fs)` @0x10113f898: `(A1, B)`.
fn make_lpf1(fs: f32) -> (f32, f32) {
    let t = ((std::f64::consts::PI / f64::from(fs)) as f32).tan();
    let b = t / (t + 1.0);
    let a1 = (t + -1.0) / (t + 1.0);
    (a1, b)
}

/// `filter1_hat_mono` @0x10113f808 (direct form II, first order).
fn filter1_hat_mono(x: &[f32], y: &mut [f32], a1: f32, b: f32) {
    let mut w = 0f32;
    for (o, &v) in y.iter_mut().zip(x) {
        let prev = b * w;
        w = (-a1).mul_add(w, v);
        *o = w.mul_add(b, prev);
    }
}

/// Normalises by the maximum, smooths each tone over time and returns the 12-pitch-class
/// profile (mode 1 octave weights 0.65, 0.5, 1, 1, 1, 1, then squared).
pub fn profile(m: &mut [f32], frames: usize, max: f32, rate: f32, hop: i32) -> [f32; 12] {
    if max.abs() >= f32::from_bits(0x3400_0000) && frames >= 1 {
        let inv = 1.0 / max;
        for v in m.iter_mut() {
            *v *= inv;
        }
    }
    let (a1, b) = make_lpf1(rate / hop as f32);
    let mut tmp = vec![0f32; frames];
    for k in 0..72 {
        let row = &mut m[k * frames..(k + 1) * frames];
        tmp.copy_from_slice(row);
        filter1_hat_mono(&tmp, row, a1, b);
    }
    let weights = [0.65f32, 0.5, 1.0, 1.0, 1.0, 1.0];
    let mut p = [0f32; 12];
    for (pc, out) in p.iter_mut().enumerate() {
        let mut o = [0f32; 6];
        for (oct, sum) in o.iter_mut().enumerate() {
            *sum = row_sum(&m[(pc + 12 * oct) * frames..(pc + 12 * oct + 1) * frames]);
        }
        let mut s = o[0] * weights[0];
        for oct in 1..6 {
            s = o[oct].mul_add(weights[oct], s);
        }
        *out = s * s;
    }
    p
}

/// Sum over frames: 4-lane chunks reduced `(l0 + l1) + (l2 + l3)`, then the rest one by one.
pub(crate) fn row_sum(v: &[f32]) -> f32 {
    let chunks = v.len() / 4;
    let mut s = 0f32;
    if chunks > 0 {
        let mut l = [0f32; 4];
        for c in v[..chunks * 4].chunks_exact(4) {
            for i in 0..4 {
                l[i] += c[i];
            }
        }
        s = (l[0] + l[1]) + (l[2] + l[3]);
    }
    for &x in &v[chunks * 4..] {
        s += x;
    }
    s
}
