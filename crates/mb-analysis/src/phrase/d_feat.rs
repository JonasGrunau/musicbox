//! Stage D's per-bar reductions of the stage C features (`…Analyze_b`, `analysis-phrase.md`
//! § 7.1). Bars start at the first downbeat (`bar`, 1-based beat index).

use super::mat;

/// C's `x / 4` for possibly negative `x`.
fn div4(x: i32) -> i32 {
    x / 4
}

/// `MatArrayFloat::sum(i, j)`.
fn sum_ij(y: &[f64], i: i32, j: i32) -> f64 {
    let n = y.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0.0;
    }
    mat::sum(&y[i as usize..=j as usize])
}

/// `LowLevelAnalyze_b(level, bar)`: bar sums (the last two bars stay 0) and bar-to-bar jump
/// flags (more than 20 % either way, cleared when both bars are quiet and the beat level around
/// the bar line is below 0.09).
pub fn low_level_b(level: &[f64], bar: i32) -> (Vec<f64>, Vec<u8>) {
    let d = level.len() as i32 - bar + 4;
    let nbars = div4(d).max(0) as usize;
    let mut sums = vec![0.0; nbars];
    let mut flags = vec![0u8; nbars];
    if d > 11 {
        let m = nbars.max(3);
        for (k, s) in sums.iter_mut().enumerate().take(m - 2) {
            let i = bar - 1 + 4 * k as i32;
            *s = sum_ij(level, i, i + 3);
        }
        for j in 0..m - 2 {
            let (a, b) = (sums[j], sums[j + 1]);
            let mut f = if b <= a {
                b < a && a > b * 1.2
            } else {
                a * 1.2 < b
            };
            if b <= 750.0 && a <= 750.0 {
                let x = level.get(4 * j + 3).copied().unwrap_or(0.0);
                let y = level.get(4 * j + 4).copied().unwrap_or(0.0);
                if y <= 0.09 && x <= 0.09 {
                    f = false;
                }
            }
            if f {
                flags[j + 1] = 1;
            }
        }
    }
    (sums, flags)
}

/// `MiddleLevelAnalyze_b(level, bar)`: bar sums (all but the last bar) and jump flags (×1.2 up,
/// ×1.21 down; zero sums count as 0.003).
pub fn middle_level_b(level: &[f64], bar: i32) -> (Vec<u8>, Vec<f64>) {
    let d = level.len() as i32 - bar + 4;
    let nbars = div4(d).max(0) as usize;
    let mut sums = vec![0.0; nbars];
    let mut flags = vec![0u8; nbars];
    let last = nbars as i32 - 1;
    if d > 7 {
        for k in 0..last.max(1) as usize {
            let i = bar - 1 + 4 * k as i32;
            if let Some(s) = sums.get_mut(k) {
                *s = sum_ij(level, i, i + 3);
            }
        }
    }
    if d > 11 {
        let m = last.max(2) as usize;
        for k in 1..m {
            let b = if sums[k] != 0.0 { sums[k] } else { 0.003 };
            let a = if sums[k - 1] != 0.0 {
                sums[k - 1]
            } else {
                0.003
            };
            let f = if a < b {
                1.2 < b / a
            } else {
                b < a && 1.21 < a / b
            };
            if f {
                flags[k] = 1;
            }
        }
    }
    (flags, sums)
}

/// `MiddlePeakAnalyze_b(peak, n, bar)`: scales `peak[..n − 1]` by `1/max` in place (stage C's
/// array) and returns bar means (all but the last bar).
pub fn middle_peak_b(peak: &mut [f64], n: i32, bar: i32) -> Vec<f64> {
    let m = mat::max_val(peak);
    let s = if m != 0.0 { 1.0 / m } else { 333.3333333333333 };
    if n > 1 {
        for v in peak.iter_mut().take((n - 1) as usize) {
            *v *= s;
        }
    }
    let d = peak.len() as i32 - bar;
    let nbars = div4(d + 1).max(0) as usize;
    let mut out = vec![0.0; nbars];
    if d + 1 >= 8 {
        let cnt = (nbars.max(2) - 1).min(nbars);
        for (k, o) in out.iter_mut().enumerate().take(cnt) {
            let i = bar - 1 + 4 * k as i32;
            *o = sum_ij(peak, i, i + 3) * 0.25;
        }
    }
    out
}

/// `AllLevelAnalyze_b(level, bar)`: the level scaled by `1/max` and its bar means.
pub fn all_level_b(level: &[f64], bar: i32) -> (Vec<f64>, Vec<f64>) {
    let m = mat::max_val(level);
    let inv = 1.0 / m;
    let s = if inv != 0.0 { inv } else { 0.003 };
    let norm: Vec<f64> = level.iter().map(|v| v * s).collect();
    let d = level.len() as i32 - bar + 4;
    let nbars = div4(d).max(0) as usize;
    let mut out = vec![0.0; nbars];
    if d > 7 {
        let cnt = (nbars.max(2) - 1).min(nbars);
        for (k, o) in out.iter_mut().enumerate().take(cnt) {
            let i = bar - 1 + 4 * k as i32;
            *o = sum_ij(&norm, i, i + 3) * 0.25;
        }
    }
    (out, norm)
}

/// `HighPeakAnalyze32_b(peaks, bar)`: the eighth-beat peaks from the first downbeat, scaled by
/// `1/max` (all but the last 24 values move; the rest stay as they were), and the bar count.
pub fn high_peak_b(peaks: &[f64], bar: i32) -> (Vec<f64>, i32) {
    let mut out = peaks.to_vec();
    let off = 8 * bar - 8;
    let len = peaks.len() as i32 - off;
    if off < 0 || len < 0 {
        return (out, 0);
    }
    let w = &peaks[off as usize..];
    let m = if w.is_empty() { 0.0 } else { mat::max_val(w) };
    let s = if m != 0.0 { 1.0 / m } else { 333.3333333333333 };
    if len > 24 {
        for i in 0..(len - 24) as usize {
            out[i] = peaks[i + off as usize] * s;
        }
    }
    (out, len / 8)
}

/// `HiHatAnalyze_b(counts, bar)`: hi-hat onsets per bar quantised to 0/4/8/16, short islands
/// (1, 2 and 3 bars) smoothed away, then 1 where the level changes from the previous bar.
pub fn hihat_b(counts: &[u8], bar: i32) -> Vec<u8> {
    let d = counts.len() as i32 - bar;
    let nbars = div4(d + 4).max(0) as usize;
    let mut out = vec![0u8; nbars];
    if d < 0 {
        return out;
    }
    let mut sums = vec![0u8; nbars];
    for i in 0..=d {
        let k = (i as usize) >> 2;
        sums[k] = sums[k].wrapping_add(counts[(bar - 1 + i) as usize]);
    }
    let mut lvl: Vec<u8> = vec![0; nbars.max(1)];
    for k in 0..nbars.max(1) {
        let s = sums.get(k).copied().unwrap_or(0);
        lvl[k] = if s < 3 {
            0
        } else if s < 7 {
            4
        } else if s < 12 {
            8
        } else {
            16
        };
    }
    let get = |l: &Vec<u8>, i: usize| l.get(i).copied().unwrap_or(0);
    if d > 7 {
        smooth(&mut lvl, nbars.max(3), 1, get);
        if d > 11 {
            smooth(&mut lvl, nbars.max(4), 2, get);
            if d > 15 {
                smooth(&mut lvl, nbars.max(5), 3, get);
            }
        }
    }
    if d > 3 {
        for k in 1..nbars.max(2) {
            if get(&lvl, k - 1) != get(&lvl, k)
                && let Some(o) = out.get_mut(k)
            {
                *o = 1;
            }
        }
    }
    out
}

/// One island pass of `HiHatAnalyze_b`: where `lvl[k−1] == lvl[k+w]` and the `w` bars between
/// differ from it, they take its value; repeated until nothing changes.
fn smooth(lvl: &mut Vec<u8>, m: usize, w: usize, get: impl Fn(&Vec<u8>, usize) -> u8) {
    if lvl.len() < m {
        lvl.resize(m, 0);
    }
    loop {
        let mut changed = false;
        let mut k = 1;
        while k + w < m {
            let c = get(lvl, k - 1);
            if c == get(lvl, k + w) && (0..w).all(|j| get(lvl, k + j) != c) {
                for j in 0..w {
                    lvl[k + j] = c;
                }
                changed = true;
            }
            k += 1;
        }
        if !changed {
            break;
        }
    }
}

/// `IntroEndFind_b(bpm, fs, cqt, frames, first, n)`: normalises the constant-Q frames in place,
/// counts strong spectral peaks per frame, sums them per bar and marks bars with many peaks
/// (`> 0.36` of the maximum) and jumps. Returns (strong, jump, per-beat level).
pub fn intro_end_find_b(
    cqt: &mut [Vec<f64>],
    frames: i32,
    first: i32,
    n: i32,
) -> (Vec<u8>, Vec<u8>, Vec<f64>) {
    let mut m = 0.0f64;
    let mut seen = false;
    for line in cqt.iter() {
        for &v in line {
            if !seen {
                m = v;
                seen = true;
            } else if !(v <= m) {
                m = v;
            }
        }
    }
    if m <= 0.0 {
        return (vec![0], vec![0], vec![0.0]);
    }
    let inv = 1.0 / m;
    for line in cqt.iter_mut() {
        for v in line.iter_mut() {
            *v *= inv;
        }
    }
    let nf = frames.max(0) as usize;
    let at = |k: usize, f: usize| cqt[k].get(f).copied().unwrap_or(0.0);
    let mut peaks: Vec<Vec<f64>> = vec![vec![0.0; nf]; 0x48];
    let mut count = vec![0i32; nf];
    for f in 0..nf {
        let mut peak = at(0, f);
        let mut held = false;
        let mut rising = false;
        for k in 1..0x48 {
            let cur = at(k, f);
            let prev = at(k - 1, f);
            if cur <= prev {
                if rising {
                    if peak < prev {
                        peak = prev;
                        held = true;
                    }
                } else if held {
                    if peak * 0.9 <= cur {
                        held = true;
                    } else {
                        if 0.001 < cur {
                            peaks[count[f] as usize][f] = peak;
                            count[f] += 1;
                        }
                        held = false;
                        peak = 0.0;
                    }
                } else {
                    held = false;
                }
            }
            rising = prev < cur;
        }
    }
    let rows = count.iter().copied().max().unwrap_or(0).max(1) as usize;
    let strong: Vec<i32> = (0..nf)
        .map(|f| (0..rows).filter(|&r| 0.1 < peaks[r][f]).count() as i32)
        .collect();
    let nbars = (n / 4).max(0) as usize;
    let mut e = vec![0.0f64; nbars];
    if n > 3 {
        let (mut prev, mut idx) = (0i32, first);
        let scale = f64::from(frames) * 4.0;
        let inv_n = 1.0 / f64::from(n);
        for (b, eb) in e.iter_mut().enumerate() {
            let end = (scale * (b as i32 + 1) as f64).mul_add(inv_n, 0.5) as i32;
            if prev < end {
                let cnt = end - prev;
                let vals: Vec<f64> = (0..cnt)
                    .map(|j| f64::from(strong.get((idx + j) as usize).copied().unwrap_or(0)))
                    .collect();
                *eb = int_sum(&vals);
                idx += cnt;
            }
            prev = idx;
        }
    }
    let mx = mat::max_val(&e) as i32;
    let mx = if mx == 0 { 1 } else { mx };
    let inv = 1.0 / f64::from(mx);
    let en: Vec<f64> = e.iter().map(|v| v * inv).collect();
    let mut o1 = vec![0u8; 4 * nbars];
    let mut o2 = vec![0u8; 4 * nbars];
    if n > 3 && nbars > 0 {
        if 0.36 < en[0] {
            o1[0] = 1;
        }
        if n & !3 != 4 {
            for j in 1..nbars {
                if 0.36 < en[j] {
                    o1[j] = 1;
                }
                if o1[j - 1] < o1[j] {
                    if en[j - 1] * 1.2 < en[j] {
                        o2[j] = 1;
                    }
                } else if 0.2 < en[j] && 1.8 < en[j] / en[j - 1] {
                    o2[j] = 1;
                }
            }
        }
    }
    let mut o3 = vec![0.0f64; 4 * nbars];
    if n > 3 {
        for (b, &v) in en.iter().enumerate() {
            o3[4 * b..4 * b + 4].fill(v);
        }
    }
    (o1, o2, o3)
}

/// A sum of small integers held as doubles (exact in any order).
fn int_sum(v: &[f64]) -> f64 {
    v.iter().sum()
}

/// `findpeaks_ex(x)`: MATLAB-style local maxima (a rise followed by a fall or flat), with
/// 1-based locations, the prominence against the higher of the two bases (the minimum on each
/// side up to the next higher value), and the width at half prominence with linear
/// interpolation.
pub struct Peaks {
    pub vals: Vec<f64>,
    pub locs: Vec<i32>,
    pub widths: Vec<f64>,
    pub proms: Vec<f64>,
}

pub fn findpeaks_ex(x: &[f64]) -> Option<Peaks> {
    let len = x.len() as i32;
    let mut vals = Vec::new();
    let mut locs = Vec::new();
    if len > 1 {
        let mut state = 0;
        for i in 0..(len - 1) as usize {
            let d = x[i] - x[i + 1];
            let s = if d < 0.0 {
                1
            } else {
                if state == 1 {
                    locs.push(i as i32 + 1);
                    vals.push(x[i]);
                }
                if d != 0.0 { 2 } else { 0 }
            };
            state = s;
        }
    }
    if vals.is_empty() {
        return Some(Peaks {
            vals: vec![0.0],
            locs: vec![0],
            widths: vec![0.0],
            proms: vec![0.0],
        });
    }
    let at = |p: i32| x.get((p - 1) as usize).copied().unwrap_or(0.0);
    let n = vals.len();
    let mut proms = vec![0.0; n];
    let mut left = 0i32;
    for u in 0..n {
        let (v, loc) = (vals[u], locs[u]);
        let mut j = loc;
        while j > 0 {
            let k = j - 1;
            if v < x[k as usize] {
                left = k + 1;
                break;
            }
            if k == 0 {
                left = 1;
            }
            j -= 1;
        }
        let cnt = loc - left + 1;
        if cnt < 0 {
            return None;
        }
        let lo = (left - 1).max(0) as usize;
        let left_min = mat::min_val(&x[lo..(lo + cnt as usize).min(x.len())]);
        // The right base: up to the first higher value (or the end).
        let mut right = len;
        for p in loc..=len {
            if v < x[(p - 1) as usize] {
                right = p;
                break;
            }
        }
        let rcnt = right - loc + 1;
        if rcnt < 0 {
            return None;
        }
        let ro = (loc - 1).max(0) as usize;
        let right_min = mat::min_val(&x[ro..(ro + rcnt as usize).min(x.len())]);
        // `fmaxnm`: the higher of the two bases.
        proms[u] = v - left_min.max(right_min);
    }
    let mut widths = vec![0.0; n];
    let mut right = 0i32;
    for u in 0..n {
        let h = proms[u].mul_add(-0.5, vals[u]);
        let loc = locs[u];
        let mut l = left;
        let mut found = false;
        let mut j = loc;
        while j > 0 {
            let k = j - 1;
            if x[k as usize] < h {
                l = k + 1;
                found = true;
                break;
            }
            if k == 0 {
                l = 1;
            }
            j -= 1;
        }
        left = l;
        if !(found && len < loc) && loc <= len {
            let mut p = loc;
            right = loop {
                if at(p) < h {
                    break p;
                }
                if p == len {
                    break len;
                }
                p += 1;
            };
        }
        let (il, ir) = (left, right);
        let a = (at(il) - h) / (at(il + 1) - at(il));
        let b = (h - at(ir - 1)) / (at(ir) - at(ir - 1));
        widths[u] = ((a - f64::from(il)) + f64::from(ir - 1)) + b;
    }
    Some(Peaks {
        vals,
        locs,
        widths,
        proms,
    })
}

/// `UtaUmuAnalyze_a(fft1)`: per spectrum line, the largest spectral peak above bin 112 that is
/// strong (38.8 × the track's mean level), wide relative to its position (> 0.11) and
/// prominent (> 10 dB); 0 when none.
pub fn uta_umu(lines: &[Vec<f64>]) -> Option<Vec<f64>> {
    let nl = lines.len();
    let w = super::filter::prewarp(471.23889803846896, 5512.5);
    let proto = super::filter::CFilter::low2(w, std::f64::consts::SQRT_2, 5512.5);
    let mut smoothed = Vec::with_capacity(nl);
    let mut cands: Vec<Vec<(i32, f64, f64, f64)>> = Vec::with_capacity(nl);
    for line in lines {
        let mut y = line.clone();
        let mut f = proto;
        f.clear();
        f.filter2(&mut y);
        let pk = findpeaks_ex(&y)?;
        let mut c = Vec::with_capacity(pk.vals.len());
        for u in 0..pk.vals.len() {
            let loc = pk.locs[u];
            let ratio = if loc != 0 {
                pk.widths[u] / f64::from(loc)
            } else {
                0.0
            };
            if loc < 0x70 {
                c.push((0, 0.0, 0.0, 0.0));
            } else {
                c.push((loc, pk.vals[u], ratio, pk.proms[u]));
            }
        }
        cands.push(c);
        smoothed.push(y);
    }
    let mut level = vec![0.0f64; nl];
    for (i, y) in smoothed.iter().enumerate() {
        let mut s = 0.0f64;
        for k in 0x6f..0x2e7 {
            s += mb_dsp::libm::exp10(y.get(k).copied().unwrap_or(0.0) * 0.05);
        }
        level[i] = (s * 0.0015822784810126582).log10() * 20.0;
    }
    let mut s = 0.0f64;
    for &l in &level {
        s += mb_dsp::libm::exp10(l * 0.05);
    }
    let g = (s / nl as f64).log10();
    let out = cands
        .iter()
        .map(|c| {
            let mut best = 0.0f64;
            let mut first = true;
            for k in 0..1024usize {
                let (_, a, r, p) = c.get(k).copied().unwrap_or((0, 0.0, 0.0, 0.0));
                let v = if a <= g * 38.8 || r <= 0.11 || p <= 10.0 {
                    0.0
                } else {
                    a
                };
                if first {
                    best = v;
                    first = false;
                } else if !(v <= best) {
                    best = v;
                }
            }
            best
        })
        .collect();
    Some(out)
}
