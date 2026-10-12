//! Stage C's per-beat level features (`analysis-phrase.md` § 6.2). Each runs once per
//! `Sub_Main_C` block on that block's samples; the filter states run on across blocks.

use super::filter::{CFilter, SQRT2, prewarp};
use super::mat;
use super::power;

const K1: f64 = 1.8477590650225735;
const K2: f64 = 0.7653668647301797;

/// Low-pass at `w` (rad/s), Butterworth Q.
fn low(w: f64, fs: f64) -> CFilter {
    CFilter::low2(prewarp(w, fs), SQRT2, fs)
}

/// High-pass `make2(1, 0, 0, 1, w·k, w²)`.
fn high(w: f64, k: f64, fs: f64) -> CFilter {
    let w = prewarp(w, fs);
    CFilter::make2(1.0, 0.0, 0.0, 1.0, w * k, w * w, fs)
}

/// The first-order smoother `make1(0, w, 1, w)`.
fn smooth(w: f64, fs: f64) -> CFilter {
    let w = prewarp(w, fs);
    CFilter::make1(0.0, w, 1.0, w, fs)
}

/// `max_val` of `y[lo .. lo + n]`, 0 for an empty window.
fn peak(y: &[f64], lo: i64, n: i64) -> f64 {
    if lo < 0 || n <= 0 {
        return 0.0;
    }
    let lo = lo as usize;
    let hi = (lo + n as usize).min(y.len());
    if lo >= hi {
        0.0
    } else {
        mat::max_val(&y[lo..hi])
    }
}

/// The rectified, smoothed envelope the level features share: `filters → |·| → smoother`.
fn envelope(x: &[f64], filters: &mut [CFilter], sm: &mut CFilter, first_order: bool) -> Vec<f64> {
    let mut y = x.to_vec();
    for f in filters {
        f.filter2(&mut y);
    }
    for v in &mut y {
        *v = v.abs();
    }
    if first_order {
        sm.filter1(&mut y)
    } else {
        sm.filter2(&mut y)
    }
    y
}

/// Per beat `i < n`: `f(y[b[i]−1 .. b[i+1]−1])` when `0 < b[i] ≤ len` and `b[i+1] − 1 ≤ len`,
/// else 0 (the guarded loops of `LowPeakAnalyze`, `LowLevelAnalyze_a`, …).
fn per_beat(y: &[f64], b: &[i32], n: usize, f: impl Fn(&[f64]) -> f64) -> Vec<f64> {
    let len = y.len() as i32;
    (0..n)
        .map(|i| {
            let (s, e) = (b[i], b[i + 1]);
            if s <= 0 || s > len || e - 1 > len || e < s {
                return 0.0;
            }
            f(&y[(s - 1) as usize..(e - 1) as usize])
        })
        .collect()
}

/// The beat starts of a block plus the end of its last beat: `beats[n − 1] + ⌊P⌋`, kept only
/// when `− 1` of it is inside the block (`LowLevelAnalyze_a`).
fn with_end(y_len: usize, beats: &[i32], n: usize, p: f64) -> Vec<i32> {
    let end = beats[n - 1] + p as i32;
    let end = if end - 1 <= y_len as i32 {
        end
    } else {
        y_len as i32
    };
    let mut b = beats[..n].to_vec();
    b.push(end);
    b
}

/// `LowPeakAnalyze` (+0x1180/+0x11b8): 100 Hz low-pass envelope peaks per beat (+0x11f0) and
/// per quarter (+0x1210), and the jump flags (+0x1230) against the previous beat.
#[derive(Debug, Clone)]
pub struct LowPeak {
    lp: CFilter,
    sm: CFilter,
}

pub struct LowPeakOut {
    pub peaks: Vec<f64>,
    pub quarters: Vec<f64>,
    pub flags: Vec<f64>,
}

impl LowPeak {
    pub fn new(fs: f64) -> Self {
        Self {
            lp: low(628.3185307179587, fs),
            sm: smooth(31.41592653589793, fs),
        }
    }

    pub fn analyze(
        &mut self,
        x: &[f64],
        bpm: f64,
        fs: f64,
        beats: &[i32],
        n: usize,
        prev: f64,
    ) -> LowPeakOut {
        let y = envelope(x, std::slice::from_mut(&mut self.lp), &mut self.sm, true);
        let len = y.len() as i32;
        let mut b = beats[..n].to_vec();
        b.push(beats[n - 1] + ((fs * 60.0) / bpm) as i32);
        let peaks = per_beat(&y, &b, n, mat::max_val);
        let mut quarters = Vec::with_capacity(4 * n);
        for i in 0..n {
            let (s, e) = (b[i], b[i + 1]);
            let q = (f64::from(e - s)).mul_add(0.25, 0.5) as i32;
            let edges = [s, s + q, s + 2 * q, s + 3 * q, e];
            for k in 0..4 {
                let (lo, hi) = (edges[k], edges[k + 1]);
                let ok = 0 < lo && lo <= len && hi - 1 <= len;
                quarters.push(if ok {
                    peak(&y, i64::from(lo) - 1, i64::from(hi - lo))
                } else {
                    0.0
                });
            }
        }
        let mut flags = vec![0.0; n];
        if prev != 0.0 && n > 0 {
            let v = peaks[0];
            if v <= prev {
                if v < prev && v + v < prev {
                    flags[0] = 0.5;
                }
            } else if prev * 1.77 < v {
                flags[0] = 1.0;
            }
            if v <= 0.09 && prev <= 0.09 {
                flags[0] *= 0.8;
            }
        }
        for i in 1..n {
            let (a, v) = (peaks[i - 1], peaks[i]);
            if v <= a {
                if v < a && a > v + v {
                    flags[i] = 0.5;
                }
            } else if a * 1.77 < v {
                flags[i] = 1.0;
            }
            if v <= 0.09 && a <= 0.09 {
                flags[i] *= 0.8;
            }
        }
        LowPeakOut {
            peaks,
            quarters,
            flags,
        }
    }
}

/// `LowLevelAnalyze_a` (+0x468/+0x4a0): 150 Hz low-pass envelope sum per beat, ×4 (+0xc78).
#[derive(Debug, Clone)]
pub struct LowLevel {
    lp: CFilter,
    sm: CFilter,
}

impl LowLevel {
    pub fn new(fs: f64) -> Self {
        Self {
            lp: low(942.4777960769379, fs),
            sm: smooth(31.41592653589793, fs),
        }
    }

    pub fn analyze(&mut self, x: &[f64], bpm: f64, fs: f64, beats: &[i32], n: usize) -> Vec<f64> {
        let y = envelope(x, std::slice::from_mut(&mut self.lp), &mut self.sm, true);
        let b = with_end(y.len(), beats, n, (fs * 60.0) / bpm);
        let mut v = per_beat(&y, &b, n, mat::sum);
        for e in &mut v {
            *e *= 4.0;
        }
        v
    }
}

/// `LowPowerAnalyze16_40Hz` (+0x1250 … +0x12f8): 40 Hz, four sections, quarter energies
/// (+0x1330, ×4 unless `fs ≈ 5512.5`) and bar sums (+0x1350).
#[derive(Debug, Clone)]
pub struct LowPower40 {
    lp: [CFilter; 4],
}

impl LowPower40 {
    pub fn new(fs: f64) -> Self {
        let f = low(251.32741228718345, fs);
        Self { lp: [f; 4] }
    }

    pub fn analyze(
        &mut self,
        x: &[f64],
        bpm: f64,
        fs: f64,
        beats: &[i32],
        n: usize,
    ) -> Option<(Vec<f64>, Vec<f64>)> {
        let mut y = x.to_vec();
        for f in &mut self.lp {
            f.filter2(&mut y);
        }
        let p = (fs * 60.0) / bpm;
        let q = p.mul_add(0.25, 0.5) as i32;
        let mut e = power::per_beat(&y, &beats[..n], q, |i| {
            if i + 1 < n {
                beats[i + 1]
            } else {
                (p + 0.5) as i32 - 1 + beats[i]
            }
        })?;
        if fs <= 5512.45 || 5512.55 <= fs {
            for v in &mut e {
                *v *= 4.0;
            }
        }
        let bars = (n as i32 / 4) as usize;
        let sums = (0..bars)
            .map(|j| e[j * 16..j * 16 + 16].iter().fold(0.0, |acc, &v| acc + v))
            .collect();
        Some((e, sums))
    }
}

/// The end entry `MiddleLevelAnalyze_a`, `MiddlePeakAnalyze` and `AllLevelAnalyze_a` append is
/// never reached: they keep the whole beat array, so beat `i` ends at `beats[i + 1]`.
fn whole(beats: &[i32]) -> &[i32] {
    beats
}

/// `MiddleLevelAnalyze_a` (+0x1370/+0x13a8): 500 Hz high-pass envelope sum per beat (+0x13e0).
#[derive(Debug, Clone)]
pub struct MiddleLevel {
    hp: CFilter,
    sm: CFilter,
}

impl MiddleLevel {
    pub fn new(fs: f64) -> Self {
        Self {
            hp: high(3141.592653589793, SQRT2, fs),
            sm: smooth(31.41592653589793, fs),
        }
    }

    pub fn analyze(&mut self, x: &[f64], beats: &[i32], n: usize) -> Vec<f64> {
        let y = envelope(x, std::slice::from_mut(&mut self.hp), &mut self.sm, true);
        per_beat(&y, whole(beats), n, mat::sum)
    }
}

/// `MiddlePeakAnalyze` (+0x1400 … +0x14e0): 400 Hz–1 kHz band (two 4th-order sections),
/// 10 Hz smoothed envelope, peak per beat (+0x1518).
#[derive(Debug, Clone)]
pub struct MiddlePeak {
    bp: [CFilter; 4],
    sm: CFilter,
}

impl MiddlePeak {
    pub fn new(fs: f64) -> Self {
        let lo = prewarp(6283.185307179586, fs);
        let lo2 = lo * lo;
        Self {
            bp: [
                high(2513.2741228718346, K1, fs),
                high(2513.2741228718346, K2, fs),
                CFilter::make2(0.0, 0.0, lo2, 1.0, lo * K1, lo2, fs),
                CFilter::make2(0.0, 0.0, lo2, 1.0, lo * K2, lo2, fs),
            ],
            sm: smooth(62.83185307179586, fs),
        }
    }

    pub fn analyze(&mut self, x: &[f64], beats: &[i32], n: usize) -> Vec<f64> {
        let y = envelope(x, &mut self.bp, &mut self.sm, true);
        per_beat(&y, whole(beats), n, mat::max_val)
    }
}

/// `AllLevelAnalyze_a`: peak of the rectified signal per beat (+0x1538).
pub fn all_level(x: &[f64], beats: &[i32], n: usize) -> Vec<f64> {
    let y: Vec<f64> = x.iter().map(|v| v.abs()).collect();
    per_beat(&y, whole(beats), n, mat::max_val)
}

/// `AllPowerAnalyze16`: quarter energies (+0x1558) and rectified quarter peaks (+0x1578) of
/// the unfiltered signal. `None` where a window is negative (`copy_from` fails).
pub fn all_power(
    x: &[f64],
    bpm: f64,
    fs: f64,
    beats: &[i32],
    n: usize,
) -> Option<(Vec<f64>, Vec<f64>)> {
    let p = (fs * 60.0) / bpm;
    let q = p.mul_add(0.25, 0.5) as i32;
    let mut energy = Vec::with_capacity(4 * n);
    let mut peaks = Vec::with_capacity(4 * n);
    for i in 0..n {
        let b = beats[i];
        let next = if i + 1 < n {
            beats[i + 1]
        } else {
            (p + 0.5) as i32 - 1 + b
        };
        let edges = [b, b + q, b + 2 * q, b + 3 * q, next];
        for k in 0..4 {
            let (lo, len) = (edges[k] as i64 - 1, edges[k + 1] - edges[k]);
            if len < 0 || lo < 0 {
                return None;
            }
            let lo = lo as usize;
            let hi = (lo + len as usize).min(x.len());
            let w = &x[lo.min(hi)..hi];
            energy.push(mat::square_sum(w));
            let a: Vec<f64> = w.iter().map(|v| v.abs()).collect();
            peaks.push(if a.is_empty() { 0.0 } else { mat::max_val(&a) });
        }
    }
    Some((energy, peaks))
}

/// `HiHatAnalyze_a` (fs/2 high band): a fast (attack 2000/s, release 20/s) and a slow (1000/s,
/// 13.3/s) follower from 0 each block; an onset is where the fast one is above the slow one
/// and above 0.06, at least `fs·13.33/bpm` after the previous. Counts onsets per beat (+0x1970).
pub fn hihat(x: &[f64], bpm: f64, fs: f64, beats: &[i32], n: usize) -> Vec<u8> {
    let inv = 1.0 / fs;
    let (mut fast, mut slow) = (0.0f64, 0.0f64);
    let mut hits = Vec::new();
    for (i, &v) in x.iter().enumerate() {
        let a = v.abs();
        let cf = if fast > a { 20.0 } else { 2000.0 };
        let cs = if slow > a { 13.333333333333334 } else { 1000.0 };
        fast = (cf * (a - fast)).mul_add(inv, fast);
        slow = (cs * (a - slow)).mul_add(inv, slow);
        if fast > slow && fast > 0.06 {
            hits.push(i as i32 + 1);
        }
    }
    let gap = (fs * 13.333333333333332) / bpm;
    let mut onsets = Vec::new();
    let mut last = 0;
    for &h in &hits {
        if !(gap >= f64::from(h - last)) {
            onsets.push(h);
            last = h;
        }
    }
    let mut count = vec![0u8; n];
    if n == 0 {
        return count;
    }
    for &o in &onsets {
        for i in 0..n - 1 {
            if beats[i] <= o && o < beats[i + 1] {
                count[i] = count[i].wrapping_add(1);
            }
        }
        if beats[n - 1] <= o {
            count[n - 1] = count[n - 1].wrapping_add(1);
        }
    }
    count
}

/// `MatArrayFloat::sum(i, j)`: elements `i ..= j`, 0 outside the array.
fn sum_range(y: &[f64], i: i32, j: i32) -> f64 {
    let n = y.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0.0;
    }
    mat::sum(&y[i as usize..=j as usize])
}

/// `DividedBandAnalyze`: per-beat envelope sums of the fs/32 signal below 500 Hz (+0x1c00)
/// and of the fs/8 signal above 500 Hz (+0x1c20).
#[derive(Debug, Clone)]
pub struct DividedBand {
    lo: CFilter,
    hi: CFilter,
    lo_sm: CFilter,
    hi_sm: CFilter,
}

impl DividedBand {
    pub fn new(fs32: f64, fs8: f64) -> Self {
        Self {
            lo: low(3141.592653589793, fs32),
            hi: high(3141.592653589793, SQRT2, fs8),
            lo_sm: smooth(31.41592653589793, fs32),
            hi_sm: smooth(31.41592653589793, fs8),
        }
    }

    pub fn analyze(
        &mut self,
        x32: &[f64],
        x8: &[f64],
        beats32: &[i32],
        beats8: &[i32],
        n: usize,
    ) -> (Vec<f64>, Vec<f64>) {
        let mut a = x32.to_vec();
        self.lo.filter2(&mut a);
        for v in &mut a {
            *v = v.abs();
        }
        let mut b = x8.to_vec();
        self.hi.filter2(&mut b);
        for v in &mut b {
            *v = v.abs();
        }
        self.lo_sm.filter1(&mut a);
        self.hi_sm.filter1(&mut b);
        let mut sa = Vec::with_capacity(n);
        let mut sb = Vec::with_capacity(n);
        for i in 0..n.saturating_sub(1) {
            sa.push(sum_range(&a, beats32[i] - 1, beats32[i + 1] - 2));
            sb.push(sum_range(&b, beats8[i] - 1, beats8[i + 1] - 2));
        }
        if n > 0 {
            sa.push(sum_range(&a, beats32[n - 1] - 1, a.len() as i32 - 1));
            sb.push(sum_range(&b, beats8[n - 1] - 1, b.len() as i32 - 1));
        }
        (sa, sb)
    }
}

/// `HighPeakAnalyze32` (fs/2 high band): 5–7 kHz band (two 4th-order sections), 10 Hz
/// envelope, peak in each eighth of a beat (+0x1d58, 8 per beat).
#[derive(Debug, Clone)]
pub struct HighPeak {
    bp: [CFilter; 4],
    sm: CFilter,
}

impl HighPeak {
    pub fn new(fs: f64) -> Self {
        let lo = prewarp(43982.2971502571, fs);
        let lo2 = lo * lo;
        Self {
            bp: [
                high(31415.926535897932, K1, fs),
                high(31415.926535897932, K2, fs),
                CFilter::make2(0.0, 0.0, lo2, 1.0, lo * K1, lo2, fs),
                CFilter::make2(0.0, 0.0, lo2, 1.0, lo * K2, lo2, fs),
            ],
            sm: smooth(62.83185307179586, fs),
        }
    }

    pub fn analyze(
        &mut self,
        x: &[f64],
        bpm: f64,
        fs: f64,
        beats: &[i32],
        n: usize,
    ) -> Option<Vec<f64>> {
        let y = envelope(x, &mut self.bp, &mut self.sm, true);
        let p = (fs * 60.0) / bpm;
        let end = beats[n - 1] + p as i32;
        let end = if end - 1 <= y.len() as i32 {
            end
        } else {
            y.len() as i32
        };
        let mut b = beats[..n].to_vec();
        b.push(end);
        let e = ((p + 0.5) as i64 as f64).mul_add(0.125, 0.5) as i32;
        let mut out = Vec::with_capacity(8 * n);
        for i in 0..n {
            let s = i64::from(b[i]);
            let e64 = i64::from(e);
            // Eighth 0 spans e + 1 samples, 1–6 span e, 7 runs to the next beat.
            let mut lo = s - 1;
            for k in 0..8i64 {
                let n_k = if k == 0 {
                    e64 + 1
                } else if k < 7 {
                    e64
                } else {
                    i64::from(b[i + 1]) - (s + 7 * e64 + 1)
                };
                if n_k < 0 {
                    return None;
                }
                out.push(peak(&y, lo, n_k));
                lo += n_k;
            }
        }
        Some(out)
    }
}
