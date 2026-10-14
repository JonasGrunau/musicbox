//! The vocal detector's features: rekordbox's own small librosa port (`analyzer::Librosa`,
//! ctor @0x100779958 with n_fft 2048 at 22050 Hz). Mel spectrogram (`make_mel_filter`
//! @0x100778a18, `get_melspec` @0x100779a28), `power_to_db` @0x100779c30, MFCC
//! (`make_mfcc_cos_table` @0x100779120, `get_mfcc` @0x10077a014), deltas (`get_mfcc_delta`
//! @0x10077a308) and the fluctogram (`get_fluctogram` @0x10077a9cc, `stft_interp` @0x10077b3a0,
//! `calc_fluctogram` @0x10077baa0). Summation orders follow the binary's vector code.
//! Spec: `analysis-vocal.md` § 3–4.

const N_BINS: usize = 1025;
const N_MELS: usize = 128;
const N_MFCC: usize = 18;
/// Log-frequency bins of the fluctogram: 120 per octave from 233.08 Hz.
const N_LOG: usize = 540;
/// Band width and hop on the log axis, and the number of bands.
const BAND: usize = 240;
const BAND_HOP: usize = 30;
const N_BANDS: usize = 11;
/// `2⁻⁵²`, the epsilon of the flatness tests.
const EPS: f64 = f64::EPSILON;

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

/// Dot product as the binary's 4 × 2-lane loops do it: fused accumulators over blocks of 8,
/// `((acc1 + acc0) + acc2) + acc3`, the lanes added, then the rest fused one by one.
fn dot8(w: &[f64], x: &[f64]) -> f64 {
    let n = w.len().min(x.len());
    let mut acc = [[0f64; 2]; 4];
    let blocks = n / 8;
    for b in 0..blocks {
        for (r, a) in acc.iter_mut().enumerate() {
            for (l, al) in a.iter_mut().enumerate() {
                let i = 8 * b + 2 * r + l;
                *al = w[i].mul_add(x[i], *al);
            }
        }
    }
    let v = add(add(add(acc[1], acc[0]), acc[2]), acc[3]);
    let mut s = v[0] + v[1];
    for i in 8 * blocks..n {
        s = w[i].mul_add(x[i], s);
    }
    s
}

/// The tables `Librosa` builds once, plus the two rekordbox ships precomputed (taken from the
/// user's installed rekordbox at run time).
pub struct Tables {
    /// 128 × 1025 mel weights (Slaney scale and normalisation).
    pub mel: Vec<f64>,
    /// 18 × 128 DCT-II rows (row 0 all 1).
    pub dct: Vec<f64>,
    /// Linear frequency of each FFT bin.
    freqs: Vec<f64>,
    /// The fluctogram's log-frequency axis.
    logf: Vec<f64>,
    /// Triangular band window (240).
    tri: Vec<f64>,
    /// Chebyshev band window (240), rekordbox's table (`chebwin` @0x10077b558).
    cheb: Vec<f64>,
}

impl Tables {
    pub fn new(cheb: Vec<f64>) -> Self {
        let sr = 22050.0;
        Self { mel: mel_filter(sr), dct: dct_table(), freqs: bin_freqs(sr), logf: log_axis(), tri: tri_window(), cheb }
    }

    /// `get_melspec`: 128 mel values per 1025-bin power frame.
    pub fn melspec(&self, power: &[f64]) -> Vec<f64> {
        let frames = power.len() / N_BINS;
        let mut out = Vec::with_capacity(frames * N_MELS);
        for f in power.chunks_exact(N_BINS) {
            for row in self.mel.chunks_exact(N_BINS) {
                out.push(dot8(row, f));
            }
        }
        out
    }

    /// `get_mfcc`: 18 coefficients per frame of the dB mel spectrogram (orthonormal DCT-II).
    pub fn mfcc(&self, mel_db: &[f64]) -> Option<Vec<f64>> {
        if mel_db.is_empty() {
            return None;
        }
        let frames = mel_db.len() / N_MELS;
        let mut out = vec![0f64; frames * N_MFCC];
        for (f, o) in mel_db.chunks_exact(N_MELS).zip(out.chunks_exact_mut(N_MFCC)) {
            o[0] = c0_sum(f) * f64::from_bits(0x3fb6_a09e_667f_3bcd);
            for (k, c) in o.iter_mut().enumerate().skip(1) {
                *c = dot8(&self.dct[k * N_MELS..(k + 1) * N_MELS], f) * 0.125;
            }
        }
        Some(out)
    }
}

/// The sum of a frame's 128 dB values in the order of `get_mfcc`'s unrolled code (pairs of
/// lanes, `q(n)` = values `2n, 2n + 1`).
fn c0_sum(x: &[f64]) -> f64 {
    let q = |n: usize| [x[2 * n], x[2 * n + 1]];
    let s4 = |a: usize, b: usize, c: usize, d: usize| add(add(q(a), q(b)), add(q(c), q(d)));
    let tail5 = |r: usize| add(add(add(add(q(r), q(r + 4)), q(r + 8)), q(r + 12)), q(r + 16));
    let lane = |r: usize| {
        add(
            add(
                add(s4(r, r + 4, r + 8, r + 12), add(add(q(r + 16), q(r + 20)), q(r + 24))),
                add(add(add(q(r + 28), q(r + 32)), q(r + 36)), q(r + 40)),
            ),
            tail5(r + 44),
        )
    };
    let mut a = q(0);
    for n in [4, 8, 16, 24, 32, 40, 48, 56] {
        a = add(a, q(n));
    }
    let mut b = [0f64; 2];
    for n in [12, 20, 28, 36, 44, 52, 60] {
        b = add(b, q(n));
    }
    let v = add(lane(3), add(lane(2), add(lane(1), add(a, b))));
    v[0] + v[1]
}

fn mel_filter(sr: f64) -> Vec<f64> {
    let fmax = sr * 0.5;
    let min_log_mel = f64::from_bits(0x402d_ffff_ffff_ffff);
    let mel_max = if 1000.0 <= fmax {
        (sr * 0.0005).ln().mul_add(f64::from_bits(0x402d_1714_87a8_33fb), min_log_mel)
    } else {
        sr * 0.0075
    };
    let mel_f: Vec<f64> = (0..N_MELS + 2)
        .map(|i| {
            let m = mel_max * i as f64;
            if m * f64::from_bits(0x3f7f_c07f_01fc_07f0) < min_log_mel {
                m * f64::from_bits(0x3fe0_8997_7bb3_4423)
            } else {
                m.mul_add(f64::from_bits(0x3f41_76c9_d7e0_d82c), f64::from_bits(0xbff0_801b_f4d7_903e)).exp() * 1000.0
            }
        })
        .collect();
    let fdiff: Vec<f64> = (0..N_MELS + 1).map(|i| mel_f[i + 1] - mel_f[i]).collect();
    let freqs = bin_freqs(sr);
    let mut w = vec![0f64; N_MELS * N_BINS];
    for i in 0..N_MELS {
        let (r_lo, r_hi) = (1.0 / fdiff[i], 1.0 / fdiff[i + 1]);
        let e = 1.0 / (mel_f[i + 2] - mel_f[i]);
        for (k, &f) in freqs.iter().enumerate() {
            let lower = -(mel_f[i] - f) * r_lo;
            let upper = (mel_f[i + 2] - f) * r_hi;
            w[i * N_BINS + k] = lower.min(upper).max(0.0) * (e + e);
        }
    }
    w
}

fn bin_freqs(sr: f64) -> Vec<f64> {
    let fmax = sr * 0.5;
    let inv = 1.0 / (N_BINS - 1) as f64;
    (0..N_BINS).map(|k| k as f64 * fmax * inv).collect()
}

fn dct_table() -> Vec<f64> {
    let mut t = vec![1f64; N_MFCC * N_MELS];
    for k in 1..N_MFCC {
        let c = k as f64 * std::f64::consts::PI / 256.0;
        for j in 0..N_MELS {
            t[k * N_MELS + j] = ((2 * j + 1) as f64 * c).cos();
        }
    }
    t
}

fn log_axis() -> Vec<f64> {
    let step = f64::from_bits(0x3f81_1111_1111_1111);
    let base = f64::from_bits(0x406d_229e_c465_c8a2);
    (0..N_LOG).map(|i| (i as f64 * step).exp2() * base).collect()
}

fn tri_window() -> Vec<f64> {
    let c = f64::from_bits(0x3f81_2358_e75d_3033);
    let one = f64::from_bits(0x3fef_ffff_ffff_ffff);
    (0..BAND).map(|i| (-(i as f64 + -119.5).abs()).mul_add(c, one)).collect()
}

/// `power_to_db` over the whole mel spectrogram: `10·(log10(max(S, 1e-10)) − log10(max S))`,
/// floored 80 dB below the maximum.
pub fn power_to_db(s: &mut [f64]) {
    if s.is_empty() {
        return;
    }
    let amin = 1e-10;
    let mx = s.iter().copied().fold(f64::NEG_INFINITY, |a, b| if a < b { b } else { a });
    let r = mx.max(amin).log10();
    let mut top = -f64::MAX;
    for v in s.iter_mut() {
        *v = (v.max(amin).log10() - r) * 10.0;
        top = top.max(*v);
    }
    let floor = top + -80.0;
    for v in s.iter_mut() {
        if *v < floor {
            *v = floor;
        }
    }
}

/// `get_mfcc_delta`: width-9 deltas, `(Σ n·(c[t+n] − c[t−n])) / 60`, the first and last four
/// frames copied from their neighbours. `None` below 9 frames.
pub fn mfcc_delta(c: &[f64], frames: usize) -> Option<Vec<f64>> {
    if frames * N_MFCC <= 0xa1 {
        return None;
    }
    let mut d = vec![0f64; frames * N_MFCC];
    let at = |t: usize, j: usize| c[t * N_MFCC + j];
    for t in 4..frames - 4 {
        for j in 0..N_MFCC {
            let d1 = at(t + 1, j) - at(t - 1, j);
            let d2 = at(t + 2, j) - at(t - 2, j);
            let d3 = at(t + 3, j) - at(t - 3, j);
            let d4 = at(t + 4, j) - at(t - 4, j);
            let v = d4.mul_add(4.0, d3.mul_add(3.0, d1 + (d2 + d2)));
            d[t * N_MFCC + j] = v * f64::from_bits(0x3f91_1111_1111_1111);
        }
    }
    let (first, last) = (4 * N_MFCC, (frames - 5) * N_MFCC);
    let row: Vec<f64> = d[first..first + N_MFCC].to_vec();
    let row_end: Vec<f64> = d[last..last + N_MFCC].to_vec();
    for t in 0..4 {
        d[t * N_MFCC..(t + 1) * N_MFCC].copy_from_slice(&row);
        d[(frames - 4 + t) * N_MFCC..(frames - 3 + t) * N_MFCC].copy_from_slice(&row_end);
    }
    Some(d)
}

/// The fluctogram's state across blocks: each band's last windowed spectrum and its flag.
#[derive(Default)]
pub struct Fluctogram {
    prev: Option<Vec<[f64; BAND]>>,
    flag: [bool; N_BANDS],
}

impl Fluctogram {
    /// `get_fluctogram` on one block of power frames: per band and frame pair, the shift (in
    /// 10-cent bins, −5…5) that best aligns the two spectra, where the earlier frame is tonal.
    /// Returns 11 values per frame pair (the first pair links to the previous block).
    pub fn update(&mut self, t: &Tables, power: &[f64]) -> Vec<f64> {
        let frames = power.len() / N_BINS;
        if frames == 0 {
            return Vec::new();
        }
        let mag: Vec<f64> = power.iter().map(|p| p.sqrt()).collect();
        let interp = stft_interp(&mag, &t.freqs, &t.logf, frames);
        let has_prev = self.prev.is_some();
        let rows = if has_prev { frames } else { frames - 1 };
        let mut out = vec![0f64; rows * N_BANDS];
        let prev = self.prev.get_or_insert_with(|| vec![[0f64; BAND]; N_BANDS]);
        for b in 0..N_BANDS {
            let mut seg = vec![[0f64; BAND]; frames];
            let mut flag = vec![false; frames];
            for j in 0..frames {
                let s = &mut seg[j];
                s.copy_from_slice(&interp[j * N_LOG + BAND_HOP * b..j * N_LOG + BAND_HOP * b + BAND]);
                flag[j] = tonal(s, &t.cheb);
                for (v, w) in s.iter_mut().zip(&t.tri) {
                    *v *= w;
                }
            }
            if has_prev && self.flag[b] {
                out[b] = calc_fluctogram(&prev[b], &seg[0]);
            }
            let off = usize::from(has_prev);
            for j in 0..frames - 1 {
                if flag[j] {
                    out[(j + off) * N_BANDS + b] = calc_fluctogram(&seg[j], &seg[j + 1]);
                }
            }
            self.flag[b] = flag[frames - 1];
            prev[b] = seg[frames - 1];
        }
        out
    }
}

/// A band counts as tonal when the Chebyshev-weighted share of its energy exceeds 0.1 and its
/// spectral flatness stays under 0.6.
fn tonal(s: &[f64; BAND], cheb: &[f64]) -> bool {
    let mut num = [[0f64; 2]; 4];
    let mut den = [[EPS, 0.0], [0.0; 2], [0.0; 2], [0.0; 2]];
    for b in 0..BAND / 8 {
        for r in 0..4 {
            for l in 0..2 {
                let i = 8 * b + 2 * r + l;
                num[r][l] = cheb[i].mul_add(s[i], num[r][l]);
                den[r][l] += s[i];
            }
        }
    }
    let n = add(add(add(num[1], num[0]), num[2]), num[3]);
    let d = add(add(add(den[1], den[0]), den[2]), den[3]);
    if (n[0] + n[1]) / (d[0] + d[1]) <= 0.1 {
        return false;
    }
    let mut lg = [[0f64; 2]; 4];
    let mut sm = [[0f64; 2]; 4];
    for b in 0..BAND / 8 {
        for r in 0..4 {
            for l in 0..2 {
                let i = 8 * b + 2 * r + l;
                lg[r][l] += (s[i] + EPS).ln();
                sm[r][l] += s[i];
            }
        }
    }
    let l = add(add(add(lg[1], lg[0]), lg[2]), lg[3]);
    let m = add(add(add(sm[1], sm[0]), sm[2]), sm[3]);
    let inv = f64::from_bits(0x3f71_1111_1111_1111);
    let gm = ((l[0] + l[1]) * inv).exp();
    gm / (m[0] + m[1]).mul_add(inv, EPS) < f64::from_bits(0x3fe3_3333_3333_3333)
}

/// `stft_interp`: the magnitude frames linearly interpolated onto the log-frequency axis.
fn stft_interp(mag: &[f64], freqs: &[f64], logf: &[f64], frames: usize) -> Vec<f64> {
    let n = freqs.len();
    let mut idx = vec![0usize; logf.len()];
    let mut cur = 0usize;
    for (i, &f) in logf.iter().enumerate() {
        while cur < n - 1 && f > freqs[cur + 1] {
            cur += 1;
        }
        idx[i] = cur;
    }
    let mut out = Vec::with_capacity(frames * logf.len());
    for j in 0..frames {
        let row = &mag[j * n..(j + 1) * n];
        for (i, &f) in logf.iter().enumerate() {
            let k = idx[i];
            let (f0, f1) = (freqs[k], freqs[k + 1]);
            let (m0, m1) = (row[k], row[k + 1]);
            out.push(((f - f0) * (m1 - m0)) / (f1 - f0) + m0);
        }
    }
    out
}

/// `calc_fluctogram`: cross-correlations of the two spectra at shifts −5…5 (each normalised by
/// the unshifted one, which itself counts as 1) and the shift of the first maximum; shift `r`
/// pairs `a[i]` with `b[i − r]`.
#[allow(clippy::neg_cmp_op_on_partial_ord, reason = "the binary's NaN behaviour (silent bands)")]
fn calc_fluctogram(a: &[f64; BAND], b: &[f64; BAND]) -> f64 {
    let corr = |r: i32| -> f64 {
        let (x, y): (&[f64], &[f64]) = if r <= 0 { (&a[..], &b[(-r) as usize..]) } else { (&b[..], &a[r as usize..]) };
        dot8(x, y)
    };
    let inv = 1.0 / corr(0);
    // the binary's running maximum: a later value wins unless it is `<=` the maximum so far,
    // so ties keep the first and NaNs (silent bands) move on to the last
    let mut best = -5i32;
    let mut m = corr(-5) * inv;
    for r in -4i32..=5 {
        let v = if r == 0 { 1.0 } else { corr(r) * inv };
        if !(v <= m) {
            best = r;
        }
        if m <= v {
            m = v;
        }
    }
    f64::from(best)
}

/// The 29 per-feature scale/offset pairs (`normalize_feature` @0x10077c4ec): `x·a + b`, fused.
pub fn normalize(f: &mut [f64], params: &[(f64, f64); 29]) {
    for row in f.chunks_exact_mut(29) {
        for (x, &(a, b)) in row.iter_mut().zip(params) {
            *x = x.mul_add(a, b);
        }
    }
}
