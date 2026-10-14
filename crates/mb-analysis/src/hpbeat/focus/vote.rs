//! The constant path's bar numbering after `UnitBeatAdjust` (0x10117db28–0x101180744).
//!
//! The adjusted grid keeps the bar numbers the anchor gave it; this pass checks them against the
//! audio. Per beat it takes the low band (one beat from the beat, `calcLowInteg` at half the
//! tempo) and the full band (`calcAllInteg`), each with a lead-in value for the stretch before
//! the first beat, and their rises from beat to beat scaled to the largest. Votes come from:
//! - the first strong low and full-band rise (head) and the last strong fall (tail);
//! - every rise well above the median rise, at least five beats apart;
//! - the bar numbers of the 16 loudest low-band beats.
//!
//! A table of cases over the tallies keeps the numbering or rotates every beat's bar number.

use super::Focus;
use crate::balib2::adjust::UnitBeatAdjust;
use crate::hpbeat::focus::{neon_sum8, neon_sumsq8};

/// One bar position back (1 → 4).
fn prev(s: i32) -> i32 {
    if s < 2 { s + 3 } else { s - 1 }
}

/// One bar position on (4 → 1).
fn next(s: i32) -> i32 {
    if s > 3 { s - 3 } else { s + 1 }
}

/// Half a bar on.
fn half(s: i32) -> i32 {
    if s <= 2 { s + 2 } else { s - 2 }
}

/// How the numbering changes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shift {
    Keep,
    Prev,
    Half,
    Next,
    /// `s + d`, plus 4 when that is below 1 (no wrap at the top).
    By(i32),
}

/// Rises `x[i] − x[i−1]` (`d[0] = x[0]`), with `d[0..=n]` scaled to their largest (taken with 0).
fn rises(x: &[f64], n: usize) -> Vec<f64> {
    let mut d = vec![0.0; x.len()];
    d[0] = x[0];
    for i in 1..x.len() {
        d[i] = x[i] - x[i - 1];
    }
    let top = d[1..=n].iter().fold(d[0].max(0.0), |a, &v| a.max(v));
    if top.abs() > f64::EPSILON {
        d[0] /= top;
        let inv = 1.0 / top;
        d[1..=n].iter_mut().for_each(|v| *v *= inv);
    }
    d
}

/// One past the last index whose magnitude is over 0.7 (0 if none).
fn last_loud(d: &[f64]) -> usize {
    (0..d.len())
        .rev()
        .find(|&k| d[k].abs() > 0.7)
        .map_or(0, |k| k + 1)
}

/// Whether the new maxima of `|d|`, scanning back from the end down to `from`, are irregular:
/// more than four of them over 0.005, or two of the first four not on neighbouring beats.
fn irregular(d: &[f64], from: usize, n: usize) -> bool {
    let m = d.len();
    let mut run = vec![0.0f64; m];
    run[m - 1] = d[m - 1].abs();
    for k in (0..m - 1).rev() {
        run[k] = run[k + 1].max(d[k].abs());
    }
    let (mut at, mut cnt) = ([0usize; 4], 0usize);
    for k in from..n {
        if run[k] - run[k + 1] > 0.005 {
            if cnt < 4 {
                at[cnt] = k;
            }
            cnt += 1;
        }
    }
    cnt > 4 || (cnt > 1 && at[..cnt].windows(2).any(|w| w[1] - w[0] != 1))
}

/// Mean and summed squared deviation of `v`'s magnitudes, 8-wide like the binary.
fn spread(v: &[f64], div: f64) -> (f64, f64) {
    let a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    let mean = neon_sum8(&a) / div;
    (mean, neon_sumsq8(&a, mean))
}

/// The bar number a rise at index `k` votes for: the beat it leads into, or past the end, the
/// one after the last beat.
fn stat_at(stat: &[i32], k: usize) -> i32 {
    let n = stat.len();
    if k > n {
        next(stat[k - 2])
    } else {
        stat[k - 1]
    }
}

/// The median of `|x|`.
fn median_abs(x: &[f64]) -> f64 {
    let mut s: Vec<f64> = x.iter().map(|v| v.abs()).collect();
    s.sort_by(|a, b| b.total_cmp(a));
    let m = s.len();
    if m % 2 == 1 {
        s[m / 2]
    } else {
        (s[m / 2] + s[m / 2 - 1]) * 0.5
    }
}

/// Indices `1..=n+1` where `x` is over the median plus 0.2 (or plus 0.1 when that gives fewer
/// than five), with their bar votes; then only those at least five beats after the previous four
/// candidates are kept. Returns the kept votes' tallies, or zeros when fewer than two are kept.
fn median_votes(x: &[f64], stat: &[i32]) -> [i32; 4] {
    let n = stat.len();
    let med = median_abs(x);
    let pick = |thr: f64| -> Vec<(usize, i32)> {
        (1..=n + 1)
            .filter(|&k| x[k] > thr)
            .map(|k| (k, stat_at(stat, k)))
            .collect()
    };
    let mut c = pick(med + 0.2);
    if c.len() < 5 {
        c = pick(med + 0.1);
    }
    let mut kept = Vec::new();
    for (k, &(i, s)) in c.iter().enumerate() {
        if c[k.saturating_sub(4)..k].iter().all(|&(j, _)| i - j > 4) {
            kept.push(s);
        }
    }
    let mut t = [0i32; 4];
    if kept.len() >= 2 {
        tally(&mut t, &kept);
    }
    t
}

fn tally(t: &mut [i32; 4], stats: &[i32]) {
    for &s in stats {
        if (1..=4).contains(&s) {
            t[(s - 1) as usize] += 1;
        }
    }
}

/// First index of the largest count (ties to the earlier one).
fn first_max(t: &[i32; 4]) -> usize {
    (1..4).fold(0, |a, i| if t[i] > t[a] { i } else { a })
}

/// Last index of the largest count.
fn last_max(t: &[i32; 4]) -> usize {
    (1..4).fold(0, |a, i| if t[i] >= t[a] { i } else { a })
}

impl Focus {
    /// Checks the adjusted grid's bar numbers and rotates them (0x10117db28–0x101180744).
    pub(super) fn vote_bars(&mut self, adjust: &UnitBeatAdjust) {
        let n = self.b.v.len();
        if n == 0 {
            return;
        }
        let pos: Vec<i32> = self.b.v.iter().map(|b| b.pos).collect();
        let stat: Vec<i32> = self.b.v.iter().map(|b| b.stat).collect();
        let bpm = self.bpm;
        let rate = f64::from(self.inp.rate);
        let m = n + 2;

        // Per-beat band sums, index k + 1 for beat k; index 0 is the stretch before the first
        // beat when that is at least half a beat long, scaled from its own tempo.
        let (mut l, mut a) = (vec![0.0f64; m], vec![0.0f64; m]);
        let q = (f64::from(bpm) * f64::from(pos[0])) / (rate * 60.0);
        if q >= 0.5 {
            let b = if pos[0] == 0 {
                120.0
            } else {
                (rate * 60.0) / f64::from(pos[0])
            };
            l[0] = (adjust.low_integ(b as f32 * 0.5, 0) * b) / f64::from(bpm);
            a[0] = (adjust.all_integ(b as f32, 0) * b) / f64::from(bpm);
        }
        for k in 0..n {
            let v = adjust.low_integ(bpm * 0.5, pos[k]);
            l[k + 1] = if v < 0.0 && k != 0 { 0.0 } else { v };
        }
        for k in 0..n {
            let v = adjust.all_integ(bpm, pos[k]);
            a[k + 1] = if v < 0.0 && k != 0 { 0.0 } else { v };
        }
        let dl = rises(&l, n);
        let da = rises(&a, n);

        // Whether the falls after the last big change are scattered (their tail votes are
        // dropped then).
        let l_odd = irregular(&dl, last_loud(&dl), n);
        let a_odd = irregular(&da, last_loud(&da), n);

        // Head votes: the first low rise over 0.35 and the first full-band rise over mean +
        // 1.5σ of the first 48 (from index 1 when there is no lead-in).
        let w = n.min(48);
        let (hl, ha) = if q < 0.5 {
            let hl = (1..=n).find(|&k| dl[k] > 0.35).map_or(0, |k| stat[k - 1]);
            let (mean, ss) = if w < 1 {
                (0.0, 0.0)
            } else {
                spread(&da[1..=w], w as f64)
            };
            let thr = (ss / w as f64).sqrt().mul_add(1.5, mean);
            let ha = (1..=n).find(|&k| da[k] > thr).map_or(0, |k| stat[k - 1]);
            (hl, ha)
        } else {
            let at = |k: usize| if k == 0 { prev(stat[0]) } else { stat[k - 1] };
            let hl = (0..=n).find(|&k| dl[k] > 0.35).map_or(0, at);
            let c = (w + 1) as f64;
            let (mean, ss) = spread(&da[..=w], c);
            let thr = (ss / c).sqrt().mul_add(1.5, mean);
            let ha = (0..=n).find(|&k| da[k] > thr).map_or(0, at);
            (hl, ha)
        };

        // Tail votes: the last fall below −(mean + σ/2) of the last 48, scanning back.
        let tail = |d: &[f64]| -> i32 {
            let start = n + 1 - w;
            let (mean, ss) = if start > n {
                (0.0, 0.0)
            } else {
                let r: Vec<f64> = d[start + 1..=n + 1].iter().rev().copied().collect();
                spread(&r, w as f64)
            };
            let thr = -0.5 * (ss / w as f64).sqrt() - mean;
            (2..=n + 1)
                .rev()
                .find(|&k| d[k] < thr)
                .map_or(0, |k| stat_at(&stat, k))
        };
        let (tl, ta) = (tail(&dl), tail(&da));

        // Rises well above the median low rise.
        let k = median_votes(&dl, &stat);

        // The bar numbers of the 16 loudest low-band beats (index n and n + 1 continue the bar).
        let mut st: Vec<i32> = stat.clone();
        st.push(next(stat[n - 1]));
        st.push(half(stat[n - 1]));
        let mut lc = l.clone();
        for i in 0..m - 1 {
            for j in i + 1..m {
                if lc[i] < lc[j] {
                    lc.swap(i, j);
                    st.swap(i, j);
                }
            }
        }
        let mut top = [0i32; 4];
        tally(&mut top, &st[..16.min(m)]);

        // Head and tail tallies.
        let mut v = [0i32; 4];
        tally(&mut v, &[hl, ha]);
        if !l_odd {
            tally(&mut v, &[tl]);
        }
        if !a_odd {
            tally(&mut v, &[ta]);
        }
        let (first, last) = (first_max(&v), last_max(&v));
        let m123 = v[0].max(v[1]).max(v[2]);

        // The winning bar: the median votes' clear leader, or else the same vote on the low
        // band itself (scaled to its largest).
        let lead = first_max(&k);
        let others = (0..4)
            .filter(|&i| i != lead)
            .map(|i| k[i])
            .fold(0, i32::max);
        let win = if (k[lead] - others).abs() <= 10 && others > 4 {
            let top = l[..=n].iter().fold(0.0f64, |a, &x| a.max(x));
            if top.abs() > f64::EPSILON {
                let inv = 1.0 / top;
                l[..=n].iter_mut().for_each(|x| *x *= inv);
            }
            let s = median_votes(&l, &stat);
            let mut best = (s[0].max(0), i32::from(s[0] > 0));
            for (i, &c) in s.iter().enumerate().skip(1) {
                if best.0 < c {
                    best = (c, i as i32 + 1);
                }
            }
            best.1
        } else {
            lead as i32 + 1
        };

        let shift = decide(&k, &v, first, last, m123, &top, win, [hl, ha, tl, ta]);
        if shift != Shift::Keep {
            for b in &mut self.b.v {
                b.stat = match shift {
                    Shift::Keep => b.stat,
                    Shift::Prev => prev(b.stat),
                    Shift::Half => half(b.stat),
                    Shift::Next => next(b.stat),
                    Shift::By(d) => {
                        let s = b.stat + d;
                        if s < 1 { s + 4 } else { s }
                    }
                };
            }
        }
    }
}

/// The case table (0x1011801c8–0x101181240). `k`: median-vote tallies; `v`: head/tail tallies
/// with their first and last leader and `m123` = the largest of `v[0..3]`; `top`: loudest-16
/// tallies; `win`: the winning bar; `votes`: head low, head full, tail low, tail full.
#[allow(clippy::too_many_arguments)]
fn decide(
    k: &[i32; 4],
    v: &[i32; 4],
    first: usize,
    last: usize,
    m123: i32,
    top: &[i32; 4],
    win: i32,
    votes: [i32; 4],
) -> Shift {
    let only = |t: &[i32; 4], i: usize| (0..4).all(|j| (t[j] != 0) == (j == i));
    // The median votes name a single bar.
    if only(k, 0) {
        return Shift::Keep;
    }
    if only(k, 1) {
        return Shift::Prev;
    }
    if only(k, 2) {
        return Shift::Half;
    }
    if only(k, 3) {
        return Shift::Next;
    }
    // One bar clearly ahead in both the median votes and the head/tail votes.
    let lead = |i: usize| first == i || last == i;
    let few = |i: usize| k[i] != 0 && (0..4).all(|j| j == i || k[j] <= 1);
    if lead(0) && few(0) {
        return Shift::Keep;
    }
    if lead(1) && few(1) {
        return Shift::Prev;
    }
    if lead(2) && few(2) {
        return Shift::Half;
    }
    if (last == 3 || v[3] > m123) && k[0] <= 1 && k[1] <= 1 && k[2] <= 10 && k[3] != 0 {
        return Shift::Next;
    }
    // The head and tail votes agree (a tail vote may be one beat late).
    let [hl, ha, tl, ta] = votes;
    if hl != 1 && hl == ha && (hl == tl || hl == prev(tl)) && (hl == ta || hl == prev(ta)) {
        return Shift::By(1 - hl);
    }
    if first == 0 && last == 0 && win == 1 {
        return Shift::Keep;
    }
    if (first != 0 && first as i32 + 1 == win) || (last >= 1 && last as i32 + 1 == win) {
        return Shift::By(1 - win);
    }
    // The loudest beats name a single bar the head/tail votes also gave.
    for i in 0..4 {
        if v[i] != 0 && only(top, i) {
            return [Shift::Keep, Shift::Prev, Shift::Half, Shift::Next][i];
        }
    }
    if first != 0 && first == last {
        return Shift::By(-(first as i32));
    }
    Shift::Keep
}
