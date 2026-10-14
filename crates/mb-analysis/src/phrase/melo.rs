//! Stage D's mood-2/3 chain (`analysis-phrase.md` § 7): `ABmeloAnalyze`, `HooksoloAnalyze`,
//! `TenkaimeiAnalyze` and `YokuyoAnalyze`. Blocks are compared pairwise; similar blocks share
//! a section label (61, 62, …).

use super::mat;

/// `MatArrayFloat::sum(i, j)`: 0 for an invalid range.
fn sum_ij(y: &[f64], i: i32, j: i32) -> f64 {
    let n = y.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0.0;
    }
    mat::sum(&y[i as usize..=j as usize])
}

/// `TenkaimeiAnalyze(similar, labels)` @0x100540924: a hook (40) similar to a later down
/// (50) block, or the reverse, clears the down block; then similar unlabelled blocks share a
/// new label from 61 on, the last block becomes outro (60) unless it is a hook, and the
/// remaining unlabelled blocks get labels of their own. `similar[r][c]` (c > r) is 1 for
/// similar blocks. Returns false when a row is missing.
pub fn tenkaimei_analyze(similar: &[Vec<u8>], labels: &mut [i32]) -> bool {
    let l = labels.len();
    if l == 0 {
        return false;
    }
    let mut mark = vec![0u8; l];
    let mut next = 61;
    if l >= 2 {
        for r in 0..l - 1 {
            let Some(row) = similar.get(r) else { return false };
            for c in r + 1..l {
                if row.get(c) != Some(&1) {
                    continue;
                }
                if labels[r] == 50 {
                    if labels[c] == 40 {
                        labels[r] = 0;
                        mark[r] = 1;
                    }
                } else if labels[r] == 40 && labels[c] == 50 {
                    labels[c] = 0;
                    mark[c] = 1;
                }
            }
        }
        for r in 0..l - 1 {
            let Some(row) = similar.get(r) else { return false };
            let mut fresh = 0;
            for c in r + 1..l {
                if row.get(c) != Some(&1) {
                    continue;
                }
                let (a, b) = (labels[r], labels[c]);
                if a != 0 {
                    if b != 0 || a == 10 || a == 50 {
                        continue;
                    }
                    if a == 40 {
                        if mark[c] == 0 {
                            labels[c] = 40;
                        }
                    } else {
                        labels[c] = a;
                    }
                } else if b == 0 {
                    labels[r] = next;
                    labels[c] = next;
                    fresh = 1;
                } else if b == 10 || b == 50 {
                } else if b == 40 {
                    if mark[r] == 0 {
                        labels[r] = 40;
                    }
                } else {
                    labels[r] = b;
                }
            }
            next += fresh;
        }
    }
    if labels[l - 1] != 40 {
        labels[l - 1] = 60;
    }
    if l >= 3 {
        for v in labels[1..l - 1].iter_mut() {
            if *v == 0 {
                *v = next;
                next += 1;
            }
        }
    }
    true
}

/// `YokuyoAnalyze(level, blocks, _, out, spread)` @0x100f097fc: whether the block levels
/// vary (Japanese 抑揚, "intonation"): the mean absolute deviation of the block means from
/// their mean, over the long or loud blocks, reaches 0.09 (0.075 when block 1 is quiet and
/// left out). Returns (flag, deviation).
pub fn yokuyo_analyze(level: &[f64], blocks: &[i32]) -> (i32, f64) {
    let l = blocks.len();
    if l < 3 {
        return (0, 0.0);
    }
    let mut b = blocks.to_vec();
    if b[l - 1] > level.len() as i32 {
        b[l - 1] = level.len() as i32;
    }
    let mut avg = vec![0.0f64; l - 1];
    for t in 0..l - 1 {
        let (s, e) = (b[t] - 1, b[t + 1] - 1);
        if e - s >= 0 {
            avg[t] = sum_ij(level, s, e) / (e - s + 1) as f64;
        }
    }
    let na = (l - 1) as i32;
    let m = na - 2;
    let mut mean = 0.0;
    if m != 0 && na > 1 {
        mean = sum_ij(&avg, 1, m) / m as f64;
    }
    let a1 = avg[1];
    let half = mean * 0.5;
    let quiet = a1 < half;
    if quiet {
        mean = 0.0;
        if l >= 4 {
            mean = sum_ij(&avg, 2, l as i32 - 2) / (l as i32 - 3) as f64;
        }
    }
    let k = if quiet && l > 3 { 3 } else { 2 };
    let thr = if quiet && l > 3 { 0.075 } else { 0.09 };
    if k >= na {
        return (0, 0.0);
    }
    let hi = k.max(na - 1);
    let cnt = (hi - k + 1) as usize;
    let k0 = (k - 1) as usize;
    let mut sel = Vec::new();
    for j in 0..cnt {
        let idx = k0 + j;
        let dev = (mean - avg[idx]).abs();
        if b[idx + 1] - b[idx] > 16 || avg[idx] > mean * 0.7 {
            sel.push(dev);
        }
    }
    if sel.is_empty() {
        return (0, 0.0);
    }
    let d = mat::sum(&sel) / sel.len() as f64;
    ((thr <= d) as i32, d)
}

/// Scales `v[..n]` by `1/max(v)` (333.33… when the maximum is 0).
fn scale_to_max(v: &mut [f64], n: usize) {
    let m = mat::max_val(v);
    let s = if m != 0.0 { 1.0 / m } else { 333.3333333333333 };
    let k = n.min(v.len());
    for x in v[..k].iter_mut() {
        *x *= s;
    }
}

fn at(v: &[f64], i: i32) -> f64 {
    if i < 0 { 0.0 } else { v.get(i as usize).copied().unwrap_or(0.0) }
}

/// Block means of `v` over `[from(t), to(t)]`, or `v[single(t)]` for blocks too short.
fn block_means(
    v: &[f64],
    bars: &[i32],
    n: usize,
    len: usize,
    range: impl Fn(i32, i32) -> (i32, i32, bool, i32),
) -> Vec<f64> {
    let mut out = vec![0.0f64; len];
    for t in 0..n {
        let (s, e, short, one) = range(bars[t], bars[t + 1]);
        out[t] = if short {
            at(v, one)
        } else if e - s < 0 {
            0.0
        } else {
            sum_ij(v, s, e) / (e - s + 1) as f64
        };
    }
    out
}

/// `HooksoloAnalyze(bars, quarters, blocks, eighths, mood, labels)` @0x1019169cc: the first
/// labels of the mood-2/3 chain from block means (bar level, quarter level, eighth level, each
/// scaled to its maximum). Block 0 is intro (10); loud blocks — or blocks that jump up
/// against the previous one — are hooks (40); after that, quiet blocks following silence
/// stay intro and blocks clearly quieter than the previous one are downs (50).
pub fn hooksolo_analyze(bars_lv: &[f64], quarters: &[f64], blocks: &[i32], eighths: &[f64], mood: i32) -> Vec<i32> {
    let mut q = quarters.to_vec();
    let want = 4 * bars_lv.len();
    if want > q.len() {
        q.resize(want, 0.0);
    }
    let mut bl = bars_lv.to_vec();
    scale_to_max(&mut bl, bars_lv.len());
    let mut ei = eighths.to_vec();
    scale_to_max(&mut ei, eighths.len());
    let mut bars = blocks.to_vec();
    if let Some(&b0) = bars.first() {
        bars[0] = 1;
        for v in bars.iter_mut().skip(1) {
            *v = (*v - b0) / 4 + 1;
        }
    }
    let nb = blocks.len();
    if mat::sum(&bl) == 0.0 {
        return vec![0; nb];
    }
    let mut sorted = bars.clone();
    super::verse::sort_delete0(&mut sorted);
    let nz = super::verse::sort_delete0_buf(&bl).1 as i32;
    sorted.push(nz - 2);
    let bars = sorted;
    let n = bars.len() - 1;
    let quarter_range = |a: i32, b: i32| {
        let (s0, e) = (4 * a, 4 * b);
        (s0 - 4, e - 5, e + !s0 < 2, e - 5)
    };
    let mut x118 = block_means(&q, &bars, n, nb, quarter_range);
    scale_to_max(&mut x118, n);
    let mut x158 = block_means(&ei, &bars, n, nb, quarter_range);
    scale_to_max(&mut x158, n);
    let mut x138 = block_means(&bl, &bars, n, nb, |a, b| (a - 1, b - 2, b + !a < 2, b - 2));
    scale_to_max(&mut x138, n);
    let mut out = vec![0i32; n];
    if n == 0 {
        return out;
    }
    out[0] = 10;
    let (buf, cnt) = super::verse::sort_delete0_buf(&x158);
    let d9 = if cnt > 0 { mat::sum(&buf[..cnt]) / cnt as f64 } else { 0.0 };
    let d8 = if nb > 0 { mat::sum(&x118) / nb as f64 } else { 0.0 };
    let loud = |a: f64| (d8 > 0.8 && a > 0.92) || (a > 0.91 && d8 <= 0.8);
    let check = |x138: f64, x158: f64| x138 >= 0.4 && (d9 < 0.4 || x158 >= 0.4);
    if check(x138[0], x158[0]) && (loud(x118[0]) || (x118[0] > 0.8 && x158[0] > 0.88)) {
        out[0] = 40;
    }
    if n == 1 {
        return out;
    }
    for t in 1..n {
        if !check(x138[t], x158[t]) {
            continue;
        }
        let d1 = x118[t];
        if loud(d1) || (d1 > 0.8 && x158[t] > 0.88) {
            out[t] = 40;
            continue;
        }
        if x158[t] == 0.0 {
            x158[t] = 0.003;
        }
        let d2 = x158[t];
        if x158[t - 1] == 0.0 {
            x158[t - 1] = 0.003;
        }
        if d1 > 0.85 && d2 > 0.65 && d2 / x158[t - 1] > 1.3 {
            out[t] = 40;
            continue;
        }
        if x118[t] == 0.0 {
            x118[t] = 0.003;
        }
        let d1 = x118[t];
        if x118[t - 1] == 0.0 {
            x118[t - 1] = 0.003;
        }
        if (d1 > 0.85 && d1 / x118[t - 1] > 1.5) || (out[t - 1] == 40 && d1 / x118[t - 1] > 0.88) {
            out[t] = 40;
        }
    }
    if n < 2 {
        return out;
    }
    for t in 1..n {
        let p = out[t - 1];
        let thr = match (p == 40, mood == 2) {
            (true, true) => 0.6,
            (true, false) => 0.62,
            (false, true) => 0.5,
            (false, false) => 0.54,
        };
        if p == 10 && x158[t] == 0.0 {
            if out[t] != 40 {
                out[t] = 10;
            }
        } else if out[t] != 40 && x158[t] < x158[t - 1] * thr {
            out[t] = 50;
        }
    }
    out
}

/// Σ d(k)² for k < n as clang vectorised it: four 2-lane `fmla` accumulators over each
/// 8-element chunk, reduced as `((a1 + a0) + a2) + a3`, lane 0 + lane 1, then a scalar `fmadd`
/// tail.
fn ssq8(n: usize, d: impl Fn(usize) -> f64) -> f64 {
    let nv = if n >= 8 { n & !7 } else { 0 };
    let mut s = 0.0f64;
    if nv > 0 {
        let mut acc = [[0.0f64; 2]; 4];
        for k in (0..nv).step_by(8) {
            for (a, lanes) in acc.iter_mut().enumerate() {
                for (l, v) in lanes.iter_mut().enumerate() {
                    let x = d(k + 2 * a + l);
                    *v = x.mul_add(x, *v);
                }
            }
        }
        let lane = |l: usize| ((acc[1][l] + acc[0][l]) + acc[2][l]) + acc[3][l];
        s = lane(0) + lane(1);
    }
    for k in nv..n {
        let x = d(k);
        s = x.mul_add(x, s);
    }
    s
}

/// A running sum continued over one row as clang vectorised it: four 2-lane accumulators
/// (the running sum in lane 0 of the first), reduced as `((a1 + a0) + a2) + a3`, lane 0 +
/// lane 1, then a scalar tail. Returns the new sum and the count of `x > 0`.
fn row_sum8(row: &[f64], start: f64, f: impl Fn(f64) -> f64) -> (f64, i32) {
    let n = row.len();
    let nv = if n >= 8 { n & !7 } else { 0 };
    let mut s = start;
    let mut cnt = 0;
    if nv > 0 {
        let mut acc = [[0.0f64; 2]; 4];
        acc[0][0] = start;
        for k in (0..nv).step_by(8) {
            for (a, lanes) in acc.iter_mut().enumerate() {
                for (l, v) in lanes.iter_mut().enumerate() {
                    let x = row[k + 2 * a + l];
                    *v += f(x);
                    cnt += (x > 0.0) as i32;
                }
            }
        }
        let lane = |l: usize| ((acc[1][l] + acc[0][l]) + acc[2][l]) + acc[3][l];
        s = lane(0) + lane(1);
    }
    for &x in &row[nv..] {
        s += f(x);
        cnt += (x > 0.0) as i32;
    }
    (s, cnt)
}

/// `fmaxnm(x, -0.0)`: `x` for `x ≥ +0`, else −0 (NaN included).
fn fmaxnm_neg0(x: f64) -> f64 {
    if x > 0.0 || (x == 0.0 && x.is_sign_positive()) { x } else { -0.0 }
}

/// `ABmeloAnalyze(features, blocks, first, end, similar)` @0x100c019d8: which blocks sound
/// alike. Rows of `features` (one per two beats) are compared pairwise by the inverse squared
/// distance of their mean-removed values; a block pair's similarity is the mean along the
/// diagonal of that matrix over the shorter block; pairs at or above mean + standard deviation
/// (over the positive pairs) are similar. Returns `similar[r][c]` (c > r) or None on a failed
/// row lookup.
pub fn ab_melo_analyze(features: &[Vec<f64>], blocks: &[i32], first: i32, end: i32) -> Option<Vec<Vec<u8>>> {
    let mut b = blocks.to_vec();
    b.push(end);
    let r = features.len();
    let cols = features.first().map_or(0, |v| v.len());
    let rows_mean = |v: &[f64]| if cols >= 1 { mat::sum(v) / cols as f64 } else { 0.0 };
    let mut dist = vec![vec![0.0f64; r]; r.saturating_sub(1)];
    if r >= 2 {
        for i in 0..r - 1 {
            let ri = &features[i];
            let mi = rows_mean(ri);
            for j in i + 1..r {
                let rj = &features[j];
                let mj = rows_mean(rj);
                dist[i][j] = if cols < 1 {
                    65535.0
                } else {
                    let d = ssq8(cols, |k| (mj + ri[k]) - (mi + rj[k]));
                    if d == 0.0 { 65535.0 } else { 1.0 / d }
                };
            }
        }
    }
    let n = b.len();
    let m = n - 1;
    let mut sim = vec![vec![0.0f64; m]; m];
    let mut out = vec![vec![0u8; m]; m];
    let idx: Vec<i32> = b
        .iter()
        .map(|&v| {
            let x = (v - first) as f64;
            (x.mul_add(0.5, 1.0) as i32).max(1)
        })
        .collect();
    if n >= 3 {
        for rr in 0..n - 2 {
            for c in rr + 1..m {
                let w0 = (idx[rr + 1] - idx[rr]).min(idx[c + 1] - idx[c]);
                let w = if w0 + idx[c] > r as i32 { r as i32 - idx[c] } else { w0 };
                let mut s = 0.0f64;
                if w >= 1 {
                    for k in 0..w {
                        let row = dist.get((k + idx[rr]) as usize)?;
                        s += row.get((k + idx[c]) as usize).copied().unwrap_or(0.0);
                    }
                }
                if w != 0 {
                    sim[rr][c] = s / w as f64;
                }
            }
        }
    }
    let (mut mean, mut std) = (0.0, 0.0);
    if m >= 1 {
        let (mut sum, mut cnt) = (0.0f64, 0);
        for row in &sim {
            let (s, c) = row_sum8(row, sum, fmaxnm_neg0);
            sum = s;
            cnt += c;
        }
        mean = if cnt >= 1 { sum / cnt as f64 } else { sum };
        let (mut ss, mut cnt2) = (0.0f64, 0);
        for row in &sim {
            let (s, c) = row_sum8(row, ss, |x| if x > 0.0 { (x - mean) * (x - mean) } else { -0.0 });
            ss = s;
            cnt2 += c;
        }
        std = if cnt2 >= 2 { (ss / (cnt2 - 1) as f64).sqrt() } else { ss };
    }
    if n <= 2 {
        return Some(out);
    }
    let thr = std + mean;
    for rr in 0..n - 2 {
        for c in rr + 1..m {
            out[rr][c] = (sim[rr][c] >= thr) as u8;
        }
    }
    Some(out)
}
