//! Stage D's intro detection: `IntroEndJudge_main` @0x100e5536c and `IntroEndJudge`
//! @0x10234508c (`analysis-phrase.md` § 7). The judge looks at the first 25–28 bars of four
//! curves (beat peaks, low-band bar sums, mid-band bar peaks, and the per-line vocal peaks of
//! `UtaUmuAnalyze_a`) and proposes the bar where the intro ends.

use super::mat::{self, cdiv};

/// `0.003`, the binary's stand-in for a zero divisor.
const EPS: f64 = 0.003;

fn nz(v: f64) -> f64 {
    if v == 0.0 { EPS } else { v }
}

/// `MatArrayFloat::sum(i, j)` @0x100dfad6c: 0 for an invalid range.
fn sum_ij(y: &[f64], i: i32, j: i32) -> f64 {
    let n = y.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0.0;
    }
    mat::sum(&y[i as usize..=j as usize])
}

/// Rounds half away from zero, as `x + (x < 0 ? −0.5 : 0.5)` then truncation.
fn round_half(v: f64) -> i32 {
    (if v < 0.0 { -0.5 } else { 0.5 } + v) as i32
}

/// The bar (1-based, rounded) of a 1-based position counted in quarter bars:
/// `fma(pos − 1, 0.25, 1)`.
fn quarter_bar(pos: i32) -> i32 {
    round_half(f64::from(pos - 1).mul_add(0.25, 1.0))
}

/// Reads `y[i]`, 0 past the end (the binary reads past short arrays; the callers pad the
/// inputs so this does not happen for real tracks).
fn at(y: &[f64], i: i32) -> f64 {
    if i < 0 { 0.0 } else { y.get(i as usize).copied().unwrap_or(0.0) }
}

/// `r ≥ 1 ? r : 1/r` for the ratio of consecutive values, each zero replaced by 0.003.
fn spread(r: f64) -> f64 {
    if 1.0 <= r { r } else { 1.0 / r }
}

/// Part 1 of `IntroEndJudge`: the bar where the beat-peak level jumps (bar means of the
/// normalised beat peaks, a jump of 2× that then holds). Returns 0 when none.
fn level_jump_bar(peaks: &[f64], bar: i32) -> Option<i32> {
    let s = bar - 1;
    let m = peaks.len() as i32 - s;
    let nb = cdiv(m, 4);
    if nb < 0 {
        return None;
    }
    let mut bm = vec![0.0f64; nb as usize];
    let last = nb - 1;
    let count = if last < 2 { 1 } else { last } as usize;
    if m > 7 {
        for k in 0..count {
            let i = s + 4 * k as i32;
            bm[k] = sum_ij(peaks, i, i + 3) * 0.25;
        }
    }
    let inv = 1.0 / mat::max_val(&bm);
    let scale = if inv != 0.0 { inv } else { EPS };
    if m > 7 {
        for v in &mut bm[..count] {
            *v *= scale;
        }
    }
    // The ratio of each bar to the one before (zeros replaced in place) for the first 25 bars.
    if bm.len() < 25 {
        bm.resize(25, 0.0);
    }
    let mut ratio = [0.0f64; 25];
    for k in 1..25 {
        if bm[k - 1] == 0.0 {
            bm[k - 1] = EPS;
        }
        ratio[k] = bm[k] / bm[k - 1];
    }
    let cands: Vec<i32> = (0..25)
        .filter(|&k| ratio[k] >= 2.0 && bm[k] > 0.095)
        .map(|k| k as i32 + 1)
        .collect();
    if cands.is_empty() {
        return Some(0);
    }
    let mut acc: Vec<i32> = Vec::new();
    // Pass 1: the level holds over the next two and three bars (within 1.2× and 1.3×).
    for &c in &cands {
        let base = nz(at(&bm, c - 1));
        let s3 = nz(sum_ij(&bm, c, c + 2) * (1.0 / 3.0));
        let s2 = nz(sum_ij(&bm, c, c + 1) * 0.5);
        let r3 = s3 / base;
        let r2 = s2 / base;
        let q3 = if r3 < 1.0 { base / s3 } else { r3 };
        let q2 = if r2 < 1.0 { base / s2 } else { r2 };
        if 1.3 < q3 || 1.2 < q2 {
            continue;
        }
        acc.push(c);
    }
    let n = cands.len();
    let mut dev3 = vec![0.0f64; n];
    let mut dev7 = vec![0.0f64; n];
    let mut dev_before = vec![0.0f64; n];
    let mut rise = vec![0.0f64; n];
    let len = bm.len().min(nb.max(0) as usize) as i32;
    if (acc.len()) < n {
        // Pass 2: small deviation from the mean of the next 3 and 7 bars.
        let mut b3 = [0.0f64; 3];
        let mut b7 = [0.0f64; 7];
        for (k, &c) in cands.iter().enumerate() {
            let m3 = sum_ij(&bm, c, c + 2) * (1.0 / 3.0);
            let m7 = if nb < c + 7 {
                if last - c < 0 { 0.0 } else { sum_ij(&bm, c, last) / f64::from(last - c + 1) }
            } else {
                sum_ij(&bm, c, c + 6) * 0.14285714285714285
            };
            for j in 0..7 {
                if c + j + 1 < len {
                    let v = at(&bm, c + j);
                    b7[j as usize] = (v - m7).abs();
                    if j < 3 {
                        b3[j as usize] = (v - m3).abs();
                    }
                }
            }
            dev3[k] = mat::sum(&b3) / 3.0;
            dev7[k] = mat::sum(&b7) / 7.0;
            if dev3[k] <= 0.1 && dev7[k] <= 0.13 && acc.last() != Some(&c) {
                acc.push(c);
            }
        }
    }
    if acc.len() < n {
        // Pass 3: a steady stretch before the jump and a 3× rise.
        let mut w = 0i32;
        for (k, &c) in cands.iter().enumerate() {
            let mut w1 = c - 1;
            if (c - 2) as u32 > 2 {
                w1 = w;
            }
            w = if c < 5 { w1 } else { 4 };
            let lo = c - 1 - w;
            let mut before: Vec<f64> = (lo..c - 1).map(|i| at(&bm, i)).collect();
            let mean = if w < 1 { 0.0 } else { mat::sum(&before) / f64::from(w) };
            let m = nz(mean);
            for v in &mut before {
                *v = (*v - m).abs();
            }
            dev_before[k] = if w < 1 { 0.0 } else { mat::sum(&before) / f64::from(w) };
            rise[k] = at(&bm, c - 1) / m;
            if dev3[k] <= 0.14
                && dev7[k] <= 0.16
                && dev_before[k] <= 0.04
                && 3.0 <= rise[k]
                && acc.last() != Some(&c)
            {
                acc.push(c);
            }
        }
    }
    if acc.len() < n {
        // Pass 4: looser deviation limits with a very flat stretch before.
        for (k, &c) in cands.iter().enumerate() {
            if dev3[k] <= 0.18
                && dev7[k] <= 0.3
                && dev_before[k] <= 0.01
                && 3.0 <= rise[k]
                && acc.last() != Some(&c)
            {
                acc.push(c);
            }
        }
    }
    if acc.is_empty() {
        return Some(0);
    }
    acc.sort();
    let first = acc[0];
    // `MatArrayInt::min_idx(.., 1)`: the 1-based index of the minimum, i.e. the next entry.
    if acc.len() > 1 && acc[1] - first == 1 {
        return Some(acc[1]);
    }
    Some(first)
}

/// Part 2 of `IntroEndJudge`: the vocal lines (`UtaUmuAnalyze_a`, 4 per bar, the first 112
/// scaled by `1/max`). Returns (the bar estimate, its line position, the line hits of the
/// first scan).
fn vocal_start(uta: &[f64]) -> (i32, i32, Vec<i32>) {
    let mx = mat::max_val(uta);
    let scale = if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 };
    let a: Vec<f64> = (0..112).map(|i| at(uta, i) * scale).collect();
    let pos = |i: i32| at(&a, i) > 0.0;
    // Scan A: lines ≥ 0.5 followed by enough voiced lines.
    let mut hits: Vec<i32> = Vec::new();
    for u in 0..96i32 {
        let d = if u & 3 != 0 { at(&a, u + 2) + at(&a, u + 1) + at(&a, u + 3) } else { 1.0 };
        if !(0.5 <= at(&a, u)) || d == 0.0 {
            continue;
        }
        let cnt = 1 + (1..=7).filter(|&k| pos(u + k)).count() as i32;
        let mean = sum_ij(&a, u, u + 7) / f64::from(cnt);
        let ok = if cnt < 4 {
            false
        } else if cnt == 4 {
            !(mean < 0.5)
        } else {
            !(mean < 0.4) || !(mean < 0.5)
        };
        if ok {
            hits.push(u + 1);
        }
    }
    // Scan B: a voiced stretch of 16 lines after near-silence.
    let mut start = 0i32;
    let mut found = false;
    for l in 0..92i32 {
        let s0 = sum_ij(&a, 0, l + 4);
        if l + 5 > 34 && s0 != 0.0 {
            break;
        }
        if !pos(l + 5) {
            continue;
        }
        let cnt = 1 + (6..=20).filter(|&k| pos(l + k)).count() as i32;
        let m = sum_ij(&a, l + 5, l + 20) * 0.0625;
        if !((cnt > 7 && m > 0.1) || (cnt > 5 && m > 0.2)) {
            continue;
        }
        let (p0, p1, p2, p3) = (at(&a, l + 5), at(&a, l + 6), at(&a, l + 7), at(&a, l + 8));
        let mean = sum_ij(&a, 4, l + 4) / f64::from(l + 1);
        if (!(p2 * p0 * p3 <= 0.0) || p1 * p0 > 0.0) && !(mean >= 0.05) {
            start = l + 6;
            found = true;
            break;
        }
    }
    if !found {
        // Scan D: from line 8, two consecutive bars with 3+ voiced lines end the search;
        // otherwise a dense 12-line stretch louder than everything before it.
        let mut hit = None;
        'd: for i in 8..34i32 {
            let s = sum_ij(&a, 0, i - 1);
            let mut on = [0.0f64; 112];
            for j in 0..i as usize {
                if a[j] > 0.0 {
                    on[j] = 1.0;
                }
            }
            let q = (i >> 2) & 0x3fff_ffff;
            let mut bars = [0.0f64; 8];
            bars[0] = sum_ij(&on, 0, 3);
            for k in 1..q.min(8) {
                bars[k as usize] = sum_ij(&on, 4 * k, 4 * k + 3);
            }
            for k in 1..q.min(8) as usize {
                if 3.0 <= bars[k] && 3.0 <= bars[k - 1] {
                    break 'd;
                }
            }
            if !pos(i) {
                continue;
            }
            let s12 = sum_ij(&a, i, i + 11);
            let cnt3 = 1 + (1..=3).filter(|&k| pos(i + k)).count();
            if cnt3 <= 2 {
                continue;
            }
            let c1 = (8..=11).filter(|&k| pos(i + k)).count();
            let c2 = (4..=7).filter(|&k| pos(i + k)).count();
            if c2 > 2 && c1 > 2 && s12 * 0.08333333333333333 > 0.4 && s / f64::from(i) < s12 * 0.041666666666666664 {
                hit = Some(i + 1);
                break;
            }
        }
        if let Some(h) = hit {
            start = h;
        } else {
            // Scan C: a voiced line after near-silence with a 24-line mean ≥ 0.1.
            for l in 14..73i32 {
                let s = sum_ij(&a, 0, l - 8);
                if !pos(l - 7) {
                    continue;
                }
                let cnt = 1 + (l - 6..=l).filter(|&k| pos(k)).count() as i32;
                let d16 = sum_ij(&a, l - 7, l + 16);
                let d0 = sum_ij(&a, l - 7, l);
                if cnt > 2
                    && !(d16 * 0.041666666666666664 < 0.1)
                    && !(s / f64::from(l - 7) >= 0.04)
                    && (0.65 <= d0 / f64::from(cnt)
                        || (0.7 < at(&a, l - 7) && 0.3 < at(&a, l - 6) && 0.3 < at(&a, l - 5)))
                {
                    start = l - 6;
                    break;
                }
            }
        }
    }
    let first = hits.first().copied().unwrap_or(0);
    let a_bar = quarter_bar(start);
    let b_bar = quarter_bar(first);
    // Which estimate wins (asm @0x102346e38).
    let (mut bar, mut at_pos);
    let early = b_bar == 1 || (a_bar, b_bar) == (5, 4) || (a_bar, b_bar) == (9, 8);
    if !early && (a_bar, b_bar) == (8, 9) {
        bar = 9;
        at_pos = first;
    } else {
        let keep = if early { true } else { b_bar > a_bar && a_bar != 1 };
        if (a_bar, b_bar) != (4, 5) && keep {
            bar = a_bar;
            at_pos = start;
        } else {
            bar = b_bar;
            at_pos = first;
        }
        if bar < 2 {
            return (bar, at_pos, hits);
        }
    }
    if ((at_pos - 14) as u32) < 6 {
        bar = 5;
    }
    if ((at_pos - 30) as u32) < 6 {
        bar = 9;
    }
    let near = (0..=35).contains(&at_pos) && ((14..=19).contains(&at_pos) || (30..=35).contains(&at_pos));
    if !near {
        let second = hits.get(1).copied().unwrap_or(0);
        if bar == 8 {
            if quarter_bar(second) == 9 {
                bar = 9;
            }
        } else if bar == 4 && quarter_bar(second) == 5 {
            bar = 5;
        }
    }
    let _ = &mut at_pos;
    (bar, at_pos, hits)
}

/// One band of part 3 (`IntroEndJudge` inlines it twice: the low-band bar sums, then the
/// mid-band bar peaks): the bars after the biggest bar-to-bar changes are tested as intro
/// ends. `aa` holds the first 27 normalised bar values, `l` the change of each bar against the
/// one before as a factor ≥ 1, `r` the raw ratio. Returns the accepted bar (1-based) or 0.
fn band_end(aa: &mut [f64; 27], l: &mut [f64; 28], r: &[f64; 28], mid: bool) -> i32 {
    if !mid {
        // Quiet stretches (two bars under 0.02 with a small change) count as no change.
        for k in 1..=16 {
            if aa[k - 1] < 0.02 && aa[k] < 0.02 && l[k] < 3.0 {
                l[k] = 1.0;
            }
        }
    }
    let mut a00: Vec<f64> = l[..17].to_vec();
    let idx = mat::sort_descend_idx(&mut a00);
    let kept: Vec<i32> = idx
        .iter()
        .copied()
        .filter(|&i| 3 < i || 1.35 <= l[(i - 1) as usize])
        .collect();
    // The kept list is copied over the sorted index list's buffer, and the first five
    // entries are read from it (stale indices fill a short list).
    let mut buf = idx.clone();
    buf[..kept.len()].copy_from_slice(&kept);
    let mut c: Vec<i32> = buf[..5].to_vec();
    if c[1] - c[0] == 1 {
        c.remove(1);
    } else if c[0] - c[1] == 1 {
        c.remove(0);
    }
    if c[2] - c[1] == 1 {
        c.remove(2);
    } else if c[1] - c[0] == 1 {
        c.remove(1);
    }
    c.truncate(3);
    c.sort();
    if mid {
        reorder(&mut c, l);
    }
    let r1 = r[1];
    let at = |a: &[f64; 27], i: i32| a[i as usize];
    let mut accepted = None;
    for &cand in &c {
        let u = cand - 1;
        if cand == 2 {
            if r1 > 1.0 && (l[2] + l[3]) * 0.5 < 1.4 && l[1] > 1.5 {
                accepted = Some(cand);
                break;
            }
            continue;
        }
        if cand < 3 {
            continue;
        }
        let (mean, lo) = if 0.85 <= r1 {
            (sum_ij(aa, 0, cand - 2) / f64::from(u), 1)
        } else {
            (sum_ij(aa, 1, cand - 2) / f64::from(cand - 2), 2)
        };
        let hi = lo.max(u);
        let dev = if !mid || (lo <= cand && cand - lo != 0) {
            mat::abs_dev_sum(&aa[(lo - 1) as usize..hi as usize], mean)
        } else {
            0.0
        };
        let dev = dev / f64::from(cand - lo);
        let dv = if mid { dev } else { nz(dev) };
        let x = at(aa, u);
        // Where the bar breaks away from the mean before it.
        let breaks = if 0.005 <= dv {
            let q1 = x / (mean - dv);
            !(x / (dv + mean) <= 1.15) || q1.is_nan() || q1 < 0.8695652173913044
        } else {
            !((mean - x).abs() / dv <= 15.0)
        };
        let mut flag;
        let mut to_level = false;
        if !breaks {
            if cand <= 3 {
                continue;
            }
            flag = false;
            if 1.3 < l[u as usize] {
                let w = if (cand as u32) < 10 { 2 } else { 4 };
                let st = cand - w - 1;
                let mut m2 = 0.0;
                let mut d2 = 0.0;
                if cand - 2 - st >= 0 {
                    m2 = sum_ij(aa, st, cand - 2) / f64::from(cand - 2 - st + 1);
                }
                if st < u {
                    for k in st..st + w {
                        d2 += (m2 - at(aa, k)).abs();
                    }
                }
                let wd = d2 / f64::from(w as u32);
                if 1.2 < x / (wd + m2) || x / (m2 - wd) < 0.8333333333333334 {
                    flag = true;
                } else {
                    to_level = true;
                }
            } else {
                to_level = true;
            }
        } else {
            flag = cand > 3;
        }
        if !to_level {
            if (aa.len() as i32) < cand + 3 {
                if !flag {
                    continue;
                }
            } else {
                if aa[(cand + 2) as usize] == 0.0 {
                    aa[(cand + 2) as usize] = EPS;
                }
                let m1 = (l[(cand + 1) as usize] + l[cand as usize]) * 0.5;
                let step = spread(at(aa, cand) / aa[(cand + 2) as usize]);
                let m2 = (l[(cand + 2) as usize] + l[(cand + 1) as usize]) * 0.5;
                let prev = l[u as usize] * 0.6;
                if m1 < 1.4 || m2 < 1.4 || m1 < prev || m2 < prev || step < 1.1 {
                    accepted = Some(cand);
                    break;
                }
                continue;
            }
        }
        // The bar against the mean of the two (mid) or three (low) bars before it.
        let before = if mid {
            sum_ij(aa, cand - 3, cand - 2) * 0.5
        } else {
            sum_ij(aa, cand - 4, cand - 2) * 0.3333333333333333
        };
        let q = x / nz(before);
        if !(0.7..=1.4).contains(&q) {
            accepted = Some(cand);
            break;
        }
    }
    let Some(mut cand) = accepted else {
        return 0;
    };
    // The accepted bar may move one on when the change continues into the next bar.
    if aa[(cand + 1) as usize] == 0.0 {
        aa[(cand + 1) as usize] = EPS;
    }
    let d = aa[(cand + 1) as usize];
    let u = (cand - 1) as usize;
    if cand < 3 && !(1.6 < r[u]) {
        return cand;
    }
    let v = r[cand as usize];
    if r[u] <= 1.0 {
        if 0.83 < v * r[u] {
            cand += 1;
        }
    } else if !(v <= 1.15) || v < 0.6 {
        let q = aa[u] / d;
        if !(0.95..=1.05).contains(&q) {
            cand += 1;
        }
    }
    cand
}

/// The mid pass reorders its three candidates when two are two bars apart (asm @0x102349918).
fn reorder(c: &mut [i32], l: &mut [f64; 28]) {
    let (c0, c1, c2) = (c[0], c[1], c[2]);
    let i = |k: i32| k as usize;
    if l[i(c0 - 1)] == 0.0 {
        l[i(c0 - 1)] = EPS;
    }
    if l[i(c1 - 1)] == 0.0 {
        l[i(c1 - 1)] = EPS;
    }
    let d1 = l[i(c1 - 1)];
    let tri = |l: &[f64; 28], k: i32| (l[i(k)] + l[i(k - 2)]) + l[i(k + 1)];
    let mut a = true;
    if c0 >= 3 && c1 - c0 == 2 {
        a = !(d1 / l[i(c0 - 1)] > 2.0) && tri(l, c1) >= tri(l, c0);
    }
    let orig = [c0, c1, c2];
    let swap01 = [orig[1], orig[0], orig[2]];
    let swap12 = [orig[0], orig[2], orig[1]];
    let rev = [orig[2], orig[1], orig[0]];
    let new = if c2 - c1 != 2 {
        if a { orig } else { swap01 }
    } else if l[i(c2 - 1)] / d1 <= 2.0 {
        let b = tri(l, c2) >= tri(l, c1);
        if a || b {
            if !a { swap01 } else if b { orig } else { swap12 }
        } else {
            rev
        }
    } else if a {
        swap12
    } else {
        rev
    };
    c.copy_from_slice(&new);
}

/// Normalises `y[..n − 1]` in place by `scale` (the binaries leave the last value) and
/// returns the first 28 bar-to-bar ratios: (raw, as a factor ≥ 1). Index 0 stays 0.
fn ratios(y: &mut [f64], scale: f64) -> ([f64; 28], [f64; 28]) {
    let n = y.len();
    if n > 1 {
        for v in &mut y[..n - 1] {
            *v *= scale;
        }
    }
    let mut raw = [0.0f64; 28];
    let mut f = [0.0f64; 28];
    for k in 1..28 {
        let r = nz(at(y, k as i32)) / nz(at(y, k as i32 - 1));
        raw[k] = r;
        f[k] = spread(r);
    }
    (raw, f)
}

/// The value each estimate adds to its weight (`IntroEndJudge`'s switch on the bar).
fn bar_weight(w: &mut f64, v: i32) {
    if v == 0 {
        *w = 0.0;
        return;
    }
    let add = match v {
        2 => 1.2,
        3 => 1.3,
        5 | 17 => 1.8,
        9 => 1.85,
        v if v > 17 => 0.44999999999999996,
        _ => 1.0,
    };
    *w += add;
}

/// `IntroEndJudge`: four estimates of the intro's end bar (level jump, vocal start, low-band
/// change, mid-band change) and a vote among them. `low` and `mid` are normalised in place.
/// Returns (the four estimates as beat numbers `bar − 4 + 4·e`, 0 for none; the chosen intro
/// end beat; 1).
pub fn intro_end_judge(
    peaks: &[f64],
    low: &mut [f64],
    mid: &mut [f64],
    bar: i32,
    uta: &[f64],
) -> Option<(Vec<i32>, i32, i32)> {
    let jump = level_jump_bar(peaks, bar)?;
    let (vocal, _, _) = vocal_start(uta);
    let inv = 1.0 / mat::max_val(low);
    let (low_raw, low_f) = ratios(low, if inv != 0.0 { inv } else { EPS });
    let mx = mat::max_val(mid);
    let (mid_raw, mid_f) = ratios(mid, if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 });
    let mut aa = [0.0f64; 27];
    for (k, v) in aa.iter_mut().enumerate() {
        *v = at(low, k as i32);
    }
    let mut l = low_f;
    let low_end = band_end(&mut aa, &mut l, &low_raw, false);
    for (k, v) in aa.iter_mut().enumerate() {
        *v = at(mid, k as i32);
    }
    let mut l = mid_f;
    let mid_end = band_end(&mut aa, &mut l, &mid_raw, true);
    let est = [jump, vocal, low_end, mid_end];
    // The vote ignores estimates at bar 1.
    let mut e = est;
    for v in &mut e {
        if *v == 1 {
            *v = 0;
        }
    }
    let eq = |j: usize| -> [i32; 4] { std::array::from_fn(|k| i32::from(e[j] == e[k])) };
    let (eq0, eq1, eq2) = (eq(0), eq(1), eq(2));
    let s = |a: &[i32; 4]| a.iter().sum::<i32>();
    let mut w = [0.0f64; 4];
    let majority = if s(&eq0) >= 3 && e[0] != 0 {
        Some(e[0])
    } else if s(&eq1) >= 3 && e[1] != 0 {
        Some(e[1])
    } else {
        None
    };
    if majority.is_none() {
        if s(&eq0) == 2 && e[0] != 0 {
            w = eq0.map(f64::from);
        }
        if s(&eq1) == 2 && e[1] != 0 && e[0] != e[1] {
            for k in 0..4 {
                w[k] += f64::from(eq1[k]);
            }
        }
        if s(&eq2) == 2 && e[2] != 0 && e[0] != e[2] && e[1] != e[2] {
            for k in 0..4 {
                w[k] += f64::from(eq2[k]);
            }
        }
        let mut u: Vec<i32> = e.to_vec();
        u.sort();
        u.dedup();
        if u[0] == 0 {
            u.remove(0);
        }
        if u.len() == 1 {
            for k in 0..4 {
                if e[k] == u[0] && u[0] != 0 {
                    w[k] += 0.4;
                }
            }
        } else if !u.is_empty() {
            let gap = u[1] - u[0];
            for k in 0..4 {
                for (j, &uj) in u.iter().enumerate() {
                    if e[k] != uj || uj == 0 {
                        continue;
                    }
                    let old = w[k];
                    w[k] = match j {
                        0 => old + if 3 < gap { 0.7 } else { 0.4 },
                        1 => if gap < 4 { old + 0.2 } else { old },
                        _ => old + -0.35,
                    };
                }
            }
        }
        // Estimates one bar apart support each other.
        let mut sorted: Vec<f64> = e.iter().map(|&v| f64::from(v)).collect();
        let idx = sort_descend_idx_int(&e, &mut sorted);
        let sv: Vec<i32> = sorted.iter().map(|&v| v as i32).collect();
        for t in 0..3 {
            if sv[t] - sv[t + 1] == 1 {
                w[(idx[t] - 1) as usize] += 0.75;
                if idx[t + 1] == 1 {
                    w[0] += 0.86;
                } else if idx[t] == 3 && idx[t + 1] == 4 {
                    w[2] += 0.3;
                }
            }
        }
        for k in 0..4 {
            bar_weight(&mut w[k], e[k]);
        }
    }
    // A low-band estimate at a weak change in a flat stretch does not count.
    let mut est = est;
    if e[2] > 0 {
        let k = (e[2] - 1) as usize;
        let step = if k == 0 { 0.0 } else { (at(low, k as i32) - at(low, k as i32 - 1)).abs() };
        if low_f[k] < 1.3 && step < 0.09 {
            w[2] = 0.0;
            e[2] = 0;
            est[2] = 0;
        }
    }
    let chosen = match majority {
        Some(v) => v,
        None => {
            let mut uw: Vec<f64> = w.to_vec();
            uw.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            uw.dedup();
            uw.reverse();
            let mut sw = w.to_vec();
            let order = mat::sort_descend_idx(&mut sw);
            let best = (order[0] - 1) as usize;
            let top = e[best];
            let same = w.iter().zip(&sw).filter(|(a, b)| a == b).count();
            let mut pick = top;
            if uw.len() > 1 && same == 1 && uw[1] == w[2] {
                let d = (top - e[2]).abs();
                if d < 5 && w[best] - w[2] <= 1.1 && order[0] != 1 {
                    let x = low_raw[(low_end - 1).max(0) as usize];
                    pick = if (0.32..=1.5).contains(&x) { top } else { e[2] };
                }
            }
            pick
        }
    };
    let base = bar - 4;
    let list: Vec<i32> = e.iter().map(|&v| if v != 0 { base + v * 4 } else { 0 }).collect();
    let end = if chosen != 0 { base + chosen * 4 } else { bar };
    let _ = est;
    Some((list, end, 1))
}

/// `MatArrayInt::sort_descend_idx(1)` on a copy of `e`: `sorted` gets the values in
/// descending order (stable), the result the 1-based original positions.
fn sort_descend_idx_int(e: &[i32; 4], sorted: &mut [f64]) -> Vec<i32> {
    let mut v: Vec<(i32, i32)> = e.iter().enumerate().map(|(i, &x)| (x, i as i32 + 1)).collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (d, s) in sorted.iter_mut().zip(&v) {
        *d = f64::from(s.0);
    }
    v.into_iter().map(|s| s.1).collect()
}

/// `IntroEndJudge_main(peaks, low_bars, mid_bars, bar, uta, secure)`: runs [`intro_end_judge`]
/// on the first bars (tracks under 28 bars are repeated to fill them) and settles the intro
/// end against the secure points. Returns (the estimates sorted descending, the intro end
/// beat, 1 when an intro was found).
pub fn intro_end_judge_main(
    peaks: &[f64],
    low: &[f64],
    mid: &[f64],
    bar: i32,
    uta: &[f64],
    secure: &[i32],
) -> Option<(Vec<i32>, i32, i32)> {
    let mut a0 = peaks.to_vec();
    let mut c0 = low.to_vec();
    let mut e0 = mid.to_vec();
    let mut u0 = uta.to_vec();
    let nbar = (a0.len() as i32 / 4)
        .min(c0.len() as i32)
        .min(e0.len() as i32)
        .min(u0.len() as i32 / 4);
    let mut list: Vec<i32>;
    let mut end = 0i32;
    let mut found;
    let none;
    if nbar < 2 {
        found = 0;
        list = vec![0];
        none = true;
    } else {
        if nbar < 28 {
            let nb = nbar as usize;
            let k = ((nbar + 27) as u32 & 0xff) / (nbar as u32 & 0xff);
            let base = (a0[..4 * nb].to_vec(), c0[..nb].to_vec(), e0[..nb].to_vec(), u0[..4 * nb].to_vec());
            let (mut a, mut c, mut e, mut u) = base.clone();
            for _ in 0..=k {
                a.extend_from_slice(&base.0);
                c.extend_from_slice(&base.1);
                e.extend_from_slice(&base.2);
                u.extend_from_slice(&base.3);
            }
            (a0, c0, e0, u0) = (a, c, e, u);
        }
        let (l, en, f) = intro_end_judge(&a0, &mut c0, &mut e0, bar, &u0)?;
        list = l;
        end = en;
        found = f;
        if nbar < 8 {
            found = 0;
            list = vec![0];
            end = 0;
            none = true;
        } else {
            none = false;
        }
    }
    let mut copy = list.clone();
    let total = |v: &[i32]| v.iter().fold(0i32, |a, &b| a.wrapping_add(b));
    if !none && total(&copy) == 0 {
        if nbar < 28 {
            found = 0;
        } else {
            found = 1;
            list = vec![bar];
            end = bar;
        }
    }
    list.sort_by(|a, b| b.cmp(a));
    if !none && total(&copy) != 0 && nbar < 28 {
        found = 1;
        for v in &mut copy {
            if *v <= nbar {
                end = *v;
            } else {
                *v = 0;
            }
        }
        if total(&copy) == 0 {
            list = vec![bar];
            end = bar;
        }
    }
    let mut out_end = end;
    // Bars from the downbeat of each estimate, and of each secure point.
    let bars: Vec<i32> = list
        .iter()
        .map(|&v| {
            let d = v - bar;
            cdiv(d, 4) + i32::from(d > 3)
        })
        .collect();
    if secure.first().is_some_and(|&s| s != 0) {
        let sbars: Vec<i32> = secure.iter().map(|&v| cdiv(v - bar, 4) + 1).collect();
        let mut hit = false;
        for &s in secure.iter().rev() {
            for &v in &list {
                if s == v {
                    out_end = s;
                    hit = true;
                    found = 1;
                }
            }
        }
        if !hit {
            let m = mat::crem(sbars[0], 4);
            if list.len() > 1 {
                let mut best = 0;
                for j in 0..list.len() - 1 {
                    if mat::crem(bars[j], 4) == m {
                        let v = list[j];
                        best = if best != 0 { best.min(v) } else { v };
                    }
                }
                if best != 0 {
                    out_end = best;
                    found = 1;
                }
            }
        }
    }
    Some((list, out_end, found))
}
