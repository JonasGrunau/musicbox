//! Stage C's spectra: `c_fft` (@0x100b875a8 …), `FFT_calc_1` @0x101cab8b4 (2048 points at
//! each beat), `FFT_calc_2` @0x1000ee7a8 (4096 points at beats 1 and 3 of each bar) and the
//! constant-Q pitch frames of `IntroEndFind_a(_init)` @0x100dafb90 / @0x100daf408
//! (`analysis-phrase.md` § 6.4).

/// `c_fft`: radix-2 decimation-in-time with a `sin` table of `N + N/4` entries and a Hann
/// window.
#[derive(Debug, Clone)]
pub struct Fft {
    n: usize,
    table: Vec<f64>,
    swaps: Vec<(usize, usize)>,
    window: Vec<f64>,
}

impl Fft {
    /// `c_fft::init(n)` + `make_hanning_window`.
    pub fn new(n: usize) -> Self {
        let step = std::f64::consts::TAU / n as f64;
        let table = (0..n + n / 4).map(|k| (k as f64 * step).sin()).collect();
        let mut swaps = Vec::new();
        let half = n / 2;
        let mut j = 0usize;
        for i in 0..n {
            if i < j {
                swaps.push((i, j));
            }
            if i + 1 >= n {
                break;
            }
            let mut m = half;
            while m <= j && m > 0 {
                j -= m;
                m >>= 1;
            }
            j += m;
        }
        let mid = (n as i32 - 1) as f64 * 0.5;
        let c = std::f64::consts::PI / (mid + 1.0);
        let even = n & !1;
        let mut window: Vec<f64> = (0..even)
            .map(|k| ((k as f64 - mid) * c).cos().mul_add(0.5, 0.5))
            .collect();
        for k in even..n {
            window.push(
                (((k as f64 - mid) * std::f64::consts::PI) * (1.0 / (mid + 1.0)))
                    .cos()
                    .mul_add(0.5, 0.5),
            );
        }
        Self {
            n,
            table,
            swaps,
            window,
        }
    }

    /// `c_fft::window` then `c_fft::fft` on interleaved complex data (`2n` values).
    pub fn transform(&self, d: &mut [f64]) {
        for (k, w) in self.window.iter().enumerate() {
            d[2 * k] *= w;
            d[2 * k + 1] *= w;
        }
        for &(i, j) in &self.swaps {
            d.swap(2 * i, 2 * j);
            d.swap(2 * i + 1, 2 * j + 1);
        }
        let n = self.n;
        let mut half = 1;
        let mut step = n;
        while half < n {
            step /= 2;
            let mut l = 0;
            for jj in 0..half {
                let s = self.table[l];
                let c = self.table[l + n / 4];
                let mut k = jj;
                while k < n {
                    let (a, b) = (2 * k, 2 * (k + half));
                    let (re, im) = (d[b], d[b + 1]);
                    let tr = re.mul_add(c, im * s);
                    let ti = im.mul_add(c, -(re * s));
                    let (r0, i0) = (d[a], d[a + 1]);
                    d[b] = r0 - tr;
                    d[b + 1] = i0 - ti;
                    d[a] = tr + r0;
                    d[a + 1] = i0 + ti;
                    k += 2 * half;
                }
                l += step;
            }
            half *= 2;
        }
    }
}

/// `20·log10|X|` of the first `bins` bins, −200 below 1e-10 (`FFT_calc_1/2`).
fn log_magnitude(d: &[f64], bins: usize) -> Vec<f64> {
    (0..bins)
        .map(|k| {
            let (re, im) = (d[2 * k], d[2 * k + 1]);
            let m = im.mul_add(im, re * re).sqrt();
            if m < 1e-10 { -200.0 } else { m.log10() * 20.0 }
        })
        .collect()
}

/// `FFT_calc_1` (`n = 2048`, beats 1–4) and `FFT_calc_2` (`n = 4096`, beats 1 and 3): per bar
/// of the block, the bar's samples (zero-padded) and an `n`-point spectrum starting at each
/// chosen beat. Returns the lines in order.
#[allow(clippy::too_many_arguments)]
pub fn bar_spectra(
    fft: &Fft,
    x: &[f64],
    bpm: f64,
    fs: f64,
    bar: i32,
    beats: &[i32],
    n_beats: i32,
    which: &[i32],
) -> Option<Vec<Vec<f64>>> {
    let n = fft.n;
    let nbars = (n_beats - bar + 1) / 4;
    if n_beats - bar < 3 {
        return Some(Vec::new());
    }
    let p = (fs * 60.0) / bpm;
    let mut pc = p as i32;
    if 0.0 < p - (p as i64) as f64 {
        pc += 1;
    }
    let size = (pc * 4 + n as i32 + 1).max(1) as usize;
    let len = x.len() as i32;
    let mut out = Vec::new();
    for b in 1..=nbars {
        let first = (bar + (b - 1) * 4) as usize;
        let start = beats[first - 1];
        let end = if b < nbars {
            beats[(bar - 1 + b * 4) as usize] - 1
        } else {
            size as i32 + start - 1
        };
        let stop = end.min(len);
        let mut buf = vec![0.0f64; size.max((stop - start + 1).max(0) as usize)];
        if stop - start >= 0 {
            let s = (start - 1) as usize;
            let m = (stop - start + 1) as usize;
            buf[..m].copy_from_slice(&x[s..s + m]);
        }
        for &j in which {
            let o = (beats[first - 1 + j as usize] - start) as usize;
            let mut d = vec![0.0f64; 2 * n];
            for k in 0..n {
                d[2 * k] = buf.get(o + k).copied().unwrap_or(0.0);
            }
            fft.transform(&mut d);
            out.push(log_magnitude(&d, n / 2));
        }
    }
    Some(out)
}

/// `hanning_ex(n)`: a symmetric Hann window without its zero end points.
fn hanning_ex(n: i32) -> Vec<f64> {
    let mut w = vec![0.0f64; n.max(0) as usize];
    let tau = std::f64::consts::TAU;
    if n & 1 == 0 {
        let inv = 1.0 / f64::from(n + 1);
        for k in 0..(n as usize / 2) {
            let c = ((k as i32 + 1) as f64 * tau * inv).cos();
            let v = c.mul_add(-0.5, 0.5);
            w[k] = v;
            w[n as usize - 1 - k] = v;
        }
    } else {
        let half = (n + 1) / 2;
        let d = f64::from(n + 1);
        if n > 2 {
            let lim = half.max(2) - 1;
            for k in 0..lim as usize {
                let c = ((k as i32 + 1) as f64 * tau * (1.0 / d)).cos();
                let v = c.mul_add(-0.5, 0.5);
                w[k] = v;
                w[n as usize - 1 - k] = v;
            }
        }
        let c = ((half as f64 * tau) / d).cos();
        w[(half - 1) as usize] = c.mul_add(-0.5, 0.5);
    }
    w
}

/// The constant-Q kernel of `IntroEndFind_a_init(fs)`: 72 semitone bins from 27.5 Hz, bin
/// `k` a windowed complex exponential of `N_k = ⌊2fs / (f_{k+1} − f_k)⌋` samples.
#[derive(Debug, Clone)]
pub struct Cqt {
    /// +0x19b8: window lengths.
    len: Vec<i32>,
    /// +0x19d8: `N_0 / N_k`.
    weight: Vec<f64>,
    /// +0x19f8: 72 lines of `N_0` complex taps (re, im).
    kernel: Vec<Vec<(f64, f64)>>,
    /// +0x1990 / +0x19b0: samples carried to the next block.
    carry: Vec<f64>,
}

impl Cqt {
    pub fn new(fs: f64) -> Self {
        let mut freq = vec![27.5f64; 73];
        for (k, f) in freq.iter_mut().enumerate().skip(1) {
            *f = (k as f64 * 0.08333333333333333).exp2() * 27.5;
        }
        let n0 = ((fs + fs) / (freq[1] - freq[0])) as i32;
        let cols = n0.max(0) as usize;
        let inv = 1.0 / fs;
        let t: Vec<f64> = (0..cols).map(|j| (j as i32 + 1) as f64 * inv).collect();
        let pairs = if cols > 1 { cols & !1 } else { 0 };
        let mut kernel: Vec<Vec<(f64, f64)>> = Vec::with_capacity(72);
        for &f in freq.iter().take(12) {
            let w = f * std::f64::consts::TAU;
            kernel.push(
                (0..cols)
                    .map(|j| {
                        let a = w * t[j];
                        if j < pairs {
                            mb_dsp::libm::sincos(a).swap_pair()
                        } else {
                            (a.cos(), a.sin())
                        }
                    })
                    .collect(),
            );
        }
        for k in 12..72 {
            let prev = &kernel[k - 12];
            let line = prev
                .iter()
                .enumerate()
                .map(|(j, &(a, b))| {
                    let re = if j < pairs {
                        (-b).mul_add(b, a * a)
                    } else {
                        a.mul_add(a, -(b * b))
                    };
                    (re, a * (b + b))
                })
                .collect();
            kernel.push(line);
        }
        let mut len = vec![0i32; 72];
        let mut weight = vec![0.0f64; 72];
        len[0] = n0;
        weight[0] = 1.0;
        for k in 0..72 {
            if k > 0 {
                len[k] = ((fs + fs) / (freq[k + 1] - freq[k])) as i32;
                weight[k] = f64::from(len[0]) / f64::from(len[k]);
            }
            let w = hanning_ex(len[k]);
            for (j, tap) in kernel[k].iter_mut().enumerate() {
                let wj = w.get(j).copied().unwrap_or(0.0);
                tap.0 *= wj;
                tap.1 *= wj;
            }
        }
        Self {
            len,
            weight,
            kernel,
            carry: Vec::new(),
        }
    }

    /// `IntroEndFind_a` on one block: magnitude frames (hop 256) of bins 23–71; carries the
    /// unused tail to the next block unless `last`. Returns 72 lines × frames (bins below 23 are
    /// 0) and the block's frame count.
    pub fn frames(&mut self, x: &[f64], last: bool) -> (Vec<Vec<f64>>, usize) {
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(x);
        let n0 = self.len[0];
        let count = (((data.len() as i32 - n0) as f64) * 0.00390625) as i32;
        let count = count.max(0) as usize;
        let mut lines = vec![vec![0.0f64; count]; 72];
        let c0 = (n0 as i64 as f64 * 0.5) as i32;
        for f in 0..count {
            for k in 23..72 {
                let nk = self.len[k];
                let v = if nk < 1 {
                    0.0
                } else {
                    let o = (f as i32) * 256 + c0 - (nk as i64 as f64 * 0.5) as i32;
                    let (re, im) = dot(
                        &data[o as usize..(o + nk) as usize],
                        &self.kernel[k][..nk as usize],
                    );
                    self.weight[k] * im.mul_add(im, re * re).sqrt()
                };
                lines[k][f] = v;
            }
        }
        if !last {
            let used = count * 256;
            self.carry = data[used.min(data.len())..].to_vec();
        }
        (lines, count)
    }
}

/// Complex dot product `Σ x·k` as compiled: up to 7 terms in order; else 8 fused lanes,
/// reduced like `MatArrayFloat::sum`, then the tail.
fn dot(x: &[f64], k: &[(f64, f64)]) -> (f64, f64) {
    let n = x.len();
    let (mut re, mut im) = (0.0f64, 0.0f64);
    let mut i = 0;
    if n >= 8 {
        let mut a = [0.0f64; 8];
        let mut b = [0.0f64; 8];
        let full = n & !7;
        while i < full {
            for l in 0..8 {
                a[l] = k[i + l].0.mul_add(x[i + l], a[l]);
                b[l] = k[i + l].1.mul_add(x[i + l], b[l]);
            }
            i += 8;
        }
        re = (a[6] + (a[4] + (a[2] + a[0]))) + (a[7] + (a[5] + (a[3] + a[1])));
        im = (b[6] + (b[4] + (b[2] + b[0]))) + (b[7] + (b[5] + (b[3] + b[1])));
    }
    while i < n {
        re = k[i].0.mul_add(x[i], re);
        im = k[i].1.mul_add(x[i], im);
        i += 1;
    }
    (re, im)
}

trait SwapPair {
    fn swap_pair(self) -> (f64, f64);
}

impl SwapPair for (f64, f64) {
    /// `sincos` returns `(sin, cos)`; the kernel stores `(cos, sin)`.
    fn swap_pair(self) -> (f64, f64) {
        (self.1, self.0)
    }
}
