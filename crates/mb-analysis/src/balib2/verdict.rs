//! The verdict between BALib v2's two beat lists (`BeatInfoAnalyzer::analyze` @0x101198e4c,
//! 0x10119fe0c–0x1011a53c0, `analysis-beats-hp.md` § 8). The constant analyser runs the beat
//! information analysis twice, constant (v1's constant path) and varied (v1's Dynamic path); four
//! tests on the varied list then decide whether the tempo varies ("hendou"):
//!
//! - A: `judgeHendouDynamicPeak` @0x1011a6714 on `UnitBeatAdjust`'s low band (an input here);
//! - D: two long interval segments with different whole-number tempos;
//! - B: a wide spread of the 16-beat mean interval around the reference interval;
//! - C, only when A, B and D are clear: the beat positions wander against a uniform grid.
//!
//! Any of them keeps the varied list and its BPM, otherwise the constant list and its BPM. The C
//! test also refines both BPMs (0x1011a2d20–0x1011a2f04), whatever its outcome.
//!
//! Every rounding, fused multiply-add and single-precision constant below is as in the binary;
//! the eight-lane sums use `neon_sum8`'s order.

use crate::beats::BeatInfo;
use crate::hpbeat::focus::{neon_sum8, neon_sumsq8};

const EPS: f64 = f64::EPSILON;
const EPS_F: f32 = f32::EPSILON;

/// The verdict's outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    pub a: bool,
    pub b: bool,
    pub c: bool,
    pub d: bool,
    /// The constant pass's BPM after the C test's refinement.
    pub const_bpm: f32,
    /// The varied pass's BPM after the C test's refinement.
    pub varied_bpm: f32,
    /// `analyze`'s last out flag: the tempo check halved a reference tempo above 180.
    pub halved: bool,
}

impl Verdict {
    pub fn hendou(&self) -> bool {
        self.a || self.b || self.c || self.d
    }

    /// The BPM `analyze` returns with the chosen list.
    pub fn bpm(&self) -> f32 {
        if self.hendou() {
            self.varied_bpm
        } else {
            self.const_bpm
        }
    }
}

/// The verdict on the varied list `list`, given both passes' BPMs (the constant pass's
/// 60000 / first beat length, the varied pass's longest segment tempo) and test A. `None` when
/// it is skipped: a zero BPM, an empty list or a zero rate.
pub fn run(
    rate: i32,
    const_bpm: f32,
    varied_bpm: f32,
    list: &[BeatInfo],
    a: bool,
) -> Option<Verdict> {
    if const_bpm.abs() <= EPS_F || varied_bpm.abs() <= EPS_F || list.is_empty() || rate == 0 {
        return None;
    }
    let n = list.len() as i32;
    let t = times(rate, list);
    // Intervals in ms: `iv` is normalised by the segments below, `raw` stays.
    let mut iv = vec![0.0; list.len()];
    for i in 0..list.len() - 1 {
        iv[i] = (t[i + 1] - t[i]).abs().mul_add(1000.0, 0.5);
    }
    let raw = iv.clone();
    let skip = if n < 22 {
        if n - 1 == 0 || n < 1 { 0 } else { n - 2 }
    } else {
        20
    };
    let tail = if n < 53 { n.max(16) - 17 } else { 36 };
    let m = n - skip;
    let nd = m - 1;
    let mut dd = vec![0.0; list.len()];
    let mut d = false;
    if m >= 2 {
        for i in 0..nd as usize {
            dd[i] = (iv[i + 1] - iv[i]).abs();
        }
        let runs = Runs::find(&dd[..nd as usize]);
        if runs.k >= 1 {
            d = segments(&t, &mut iv, &runs, nd);
        }
    }
    let r = reference(&iv, &dd, n, m, nd, varied_bpm);
    let b = spread(&iv, &dd, n, m, nd, tail, r);
    let (tempo, halved) = check_tempo(varied_bpm, r);
    let mut v = Verdict {
        a,
        b,
        c: false,
        d,
        const_bpm,
        varied_bpm,
        halved,
    };
    if !a && !b && !d {
        let beat = (f64::from(rate) * 60.0) / f64::from(tempo);
        v.c = Drift {
            rate,
            list,
            raw: &raw,
            r,
            beat,
            varied0: varied_bpm,
        }
        .test(&mut v.const_bpm, &mut v.varied_bpm);
    }
    Some(v)
}

/// Beat times in s, rounded to the ms: pairs scaled by `1000 / rate`, the last one or two beats
/// by `1000` then `1 / rate` (0x1011a003c, 0x1011a009c).
fn times(rate: i32, list: &[BeatInfo]) -> Vec<f64> {
    let n = list.len();
    let rate = f64::from(rate);
    let vec_n = if n < 3 {
        0
    } else {
        n - if n & 1 != 0 { 1 } else { 2 }
    };
    let (scale, inv) = (1000.0 / rate, 1.0 / rate);
    list.iter()
        .enumerate()
        .map(|(i, b)| {
            let p = f64::from(b.pos);
            if i < vec_n {
                (p * scale).round() * 0.001
            } else {
                (p * 1000.0 * inv).round() * 0.001
            }
        })
        .collect()
}

/// Stretches where neighbouring intervals differ by more than 11 ms: first and last index of
/// each, counted when the stretch closes; the scan stops after the 32nd.
struct Runs {
    k: usize,
    start: [i32; 32],
    end: [i32; 32],
}

impl Runs {
    fn find(dd: &[f64]) -> Runs {
        let mut r = Runs {
            k: 0,
            start: [0; 32],
            end: [0; 32],
        };
        let mut open = false;
        for (i, &x) in dd.iter().enumerate() {
            if x <= 11.0 {
                if open {
                    r.k += 1;
                    if r.k - 1 > 30 {
                        break;
                    }
                }
            } else {
                if !open {
                    r.start[r.k] = i as i32;
                }
                r.end[r.k] = i as i32;
            }
            open = x > 11.0;
        }
        r
    }
}

/// The segments between the stretches: normalise each by its mean, rescale by the longest
/// segment's mean where a segment sits at a harmonic of it (else by its own), and run test D.
fn segments(t: &[f64], iv: &mut [f64], runs: &Runs, nd: i32) -> bool {
    let (k, s, e) = (runs.k, &runs.start, &runs.end);
    let mut mean = [0.0f64; 33];
    // Segment j spans [lo, hi): [0, s0), (e[j−1], s[j]), then (e[k−1], nd).
    let norm = |iv: &mut [f64], lo: i32, hi: i32| -> f64 {
        if lo < hi {
            let seg = &mut iv[lo as usize..hi as usize];
            let mj = neon_sum8(seg) / f64::from(hi - lo);
            if mj.abs() > EPS {
                let r = 1.0 / mj;
                seg.iter_mut().for_each(|x| *x *= r);
            }
            mj
        } else {
            iv[lo.max(0) as usize]
        }
    };
    if s[0] < 1 {
        mean[0] = iv[0];
    } else {
        mean[0] = norm(iv, 0, s[0]);
    }
    let fill = |iv: &mut [f64], a: i32, b: i32| {
        if a <= b {
            iv[a as usize..=b as usize].fill(1.0);
        }
    };
    fill(iv, s[0], e[0]);
    for j in 1..k {
        mean[j] = norm(iv, e[j - 1] + 1, s[j]);
        fill(iv, s[j], e[j]);
    }
    let last_lo = e[k - 1] + 1;
    mean[k] = norm(iv, last_lo, nd);

    // Lengths (beats) and durations (s) of the segments; the first is measured from beat 1.
    let mut cnt = vec![0i32; k + 1];
    let mut dur = vec![0f32; k + 1];
    cnt[0] = s[0] - 1;
    dur[0] = (t[s[0] as usize] - t[1]) as f32;
    let (mut most, mut best) = (cnt[0], 0usize);
    for j in 1..k {
        cnt[j] = s[j] - e[j - 1] - 1;
        dur[j] = (t[s[j] as usize] - t[(e[j - 1] + 1) as usize]) as f32;
        if most < cnt[j] {
            best = j;
        }
        most = most.max(cnt[j]);
    }
    cnt[k] = nd - e[k - 1] - 1;
    dur[k] = (t[nd as usize] - t[last_lo as usize]) as f32;
    if most < cnt[k] {
        best = k;
    }

    let r = mean[best];
    let harmonic = |x: f64| {
        (r * 0.485 < x && x < r * 0.515)
            || (r * 0.646_666_666_666_666_6 < x && x < r * 0.686_666_666_666_666_6)
            || (r * 1.455 < x && x < r * 1.545)
            || (r * 1.94 < x && x < r * 2.06)
    };
    let scale = |iv: &mut [f64], lo: i32, hi: i32, x: f64| {
        let f = if harmonic(x) { r } else { x };
        if lo < hi {
            iv[lo as usize..hi as usize]
                .iter_mut()
                .for_each(|v| *v *= f);
        }
    };
    scale(iv, 0, e[0] + 1, mean[0]);
    for j in 1..k {
        scale(iv, e[j - 1] + 1, e[j] + 1, mean[j]);
    }
    scale(iv, last_lo, nd, mean[k]);

    // Test D (0x1011a10a8–0x1011a1478, single precision): every segment's tempo close to a whole
    // BPM, no segment over 65 % of the track, the long ones over 7.5 % each and within 55 % of
    // each other, and two long ones with different whole BPMs.
    let tn = t[nd as usize];
    let mut whole = vec![0i32; k + 1];
    let (mut max_frac, mut max_share, mut min_share) = (0f32, 0f32, 1f32);
    let mut share = vec![0f32; k + 1];
    for j in 0..=k {
        let mf = mean[j] as f32;
        let (bpm, w) = if mf.abs() <= EPS_F {
            (120.0f32, 120)
        } else {
            let b = 60000.0f32 / mf;
            (b, b.round() as i32)
        };
        whole[j] = w;
        let frac = (bpm - w as f32).abs();
        share[j] = if tn.abs() > EPS {
            (f64::from(dur[j]) * (1.0 / tn)) as f32
        } else {
            0.5
        };
        if j == 0 || max_frac <= frac {
            max_frac = frac;
        }
        if j == 0 || max_share <= share[j] {
            max_share = share[j];
        }
        if cnt[j] > 16 && share[j] < min_share {
            min_share = share[j];
        }
    }
    let long = |j: usize| cnt[j] > 16 && share[j] > 0.075f32;
    let two = (0..k).any(|i| long(i) && (i..=k).any(|j| long(j) && whole[i] != whole[j]));
    max_frac < 0.2f32
        && max_share < 0.65f32
        && min_share > 0.075f32
        && max_share - min_share < 0.55f32
        && two
}

/// The reference interval (ms): the mean of the longest stretch of at least min(n, 37) intervals
/// whose neighbours differ by at most 11, 20 or 30 ms, else the median interval.
fn reference(iv: &[f64], dd: &[f64], n: i32, m: i32, nd: i32, varied_bpm: f32) -> f64 {
    if m < 2 {
        return (60.0 / f64::from(varied_bpm)) * 1000.0;
    }
    let min_run = n.min(37);
    let cnt = nd.max(1) as usize;
    for thr in [11.0, 20.0, 30.0] {
        let (c, s) = longest(iv, dd, cnt, thr, min_run);
        if c != 0 {
            return s / f64::from(c);
        }
    }
    let mut c = iv[..cnt].to_vec();
    if nd > 1 {
        for i in 0..(nd - 1) as usize {
            for j in i + 1..nd as usize {
                if c[i] < c[j] {
                    c.swap(i, j);
                }
            }
        }
    }
    if nd & (0x8000_0001u32 as i32) == 1 {
        c[(nd >> 1) as usize]
    } else {
        let h = (nd >> 1) as usize;
        (c[h] + c[h - 1]) * 0.5
    }
}

/// The longest stretch (count, sum) of at least `min_run` intervals with neighbour differences
/// up to `thr`; the first of equal length wins.
fn longest(iv: &[f64], dd: &[f64], cnt: usize, thr: f64, min_run: i32) -> (i32, f64) {
    let (mut best_c, mut best_s, mut cur_c, mut cur_s) = (0, 0.0, 0, 0.0);
    for k in 0..cnt {
        if thr < dd[k] {
            if !(cur_c < min_run || cur_c <= best_c) {
                best_c = cur_c;
                best_s = cur_s;
            }
            cur_c = 0;
            cur_s = 0.0;
        } else {
            cur_s += iv[k];
            cur_c += 1;
        }
    }
    if !(cur_c < min_run || cur_c <= best_c) {
        (cur_c, cur_s)
    } else {
        (best_c, best_s)
    }
}

/// Test B: the 16-beat mean intervals' deviation from the reference, against the share of
/// stable stretches longer than 23 intervals.
fn spread(iv: &[f64], dd: &[f64], n: i32, m: i32, nd: i32, tail: i32, r: f64) -> bool {
    let w = n - tail - 1;
    let mut avg = vec![0.0; n as usize];
    for i in 0..w.max(0) {
        let c = if n <= i + 16 { n - i - 1 } else { 16 };
        if c > 0 {
            let mut s = avg[i as usize];
            for j in 0..c {
                s += iv[(i + j) as usize];
            }
            avg[i as usize] = s / f64::from(c);
        }
    }
    let dev = if w < 1 {
        0.0
    } else {
        let w = w as usize;
        let ad: Vec<f64> = avg[..w].iter().map(|&x| (x - r).abs()).collect();
        let total = neon_sum8(&ad);
        // the mean deviation of the windows within 5 % of the reference
        let (mut near_s, mut near_c) = (0.0, 0);
        let inv = 1.0 / r;
        for (i, &x) in avg[..w].iter().enumerate() {
            let near = x.abs() <= EPS || {
                let q = r / x;
                (if q < 1.0 { x * inv } else { q }) < 1.05
            };
            if near {
                near_s += ad[i];
                near_c += 1;
            }
        }
        let den = (f64::from(n) - f64::from(tail)) + -1.0;
        let pct = (f64::from(near_c) * 100.0) / den;
        if near_c != 0 && pct > 70.0 {
            near_s / f64::from(near_c)
        } else {
            total / den
        }
    };
    let mut stable = 0;
    if m >= 2 {
        let mut run = 0;
        for &c in &dd[..nd.max(1) as usize] {
            let next = if c <= 11.0 { run + 1 } else { 0 };
            if !(run > 23 && c > 11.0) {
                run = 0;
            }
            stable += run;
            run = next;
        }
    }
    let share = f64::from(stable) / f64::from(n);
    (2.25 <= dev && share <= 0.19) || (1.56 < dev && share < 0.057)
}

/// The varied BPM checked against the reference interval (0x1011a190c–0x1011a1a40, 0x1011a28b4):
/// snapped to a whole BPM within 0.05 %, doubled or halved at an octave, or replaced by the
/// reference tempo when neither is within 0.8–1.25×. Feeds only the C test's beat length.
fn check_tempo(v: f32, r: f64) -> (f32, bool) {
    let w = v.round();
    let (vd, wd) = (f64::from(v), f64::from(w));
    let mut s = if wd * 0.9995 <= vd && vd <= wd * 1.0005 {
        w
    } else {
        v
    };
    let rt = 60000.0 / r;
    let sd = f64::from(s);
    let mut halved = false;
    if sd * 1.98 <= rt && rt <= sd * 2.02 {
        if s <= 90.0 {
            s = s + s;
        }
    } else if sd * 0.495 <= rt && rt <= sd * 0.505 {
        if 140.0 <= s {
            s *= 0.5;
        }
    } else if !(sd * 0.8 <= rt && rt <= sd * 1.25) {
        if 70.0 <= rt {
            s = rt as f32;
            if rt > 180.0 {
                s *= 0.5;
                halved = true;
            }
        } else {
            s = rt as f32 + rt as f32;
        }
    }
    (s, halved)
}

/// Test C's inputs.
struct Drift<'a> {
    rate: i32,
    list: &'a [BeatInfo],
    /// Raw intervals (ms).
    raw: &'a [f64],
    /// Reference interval (ms).
    r: f64,
    /// Beat length in samples from the checked tempo.
    beat: f64,
    /// The varied BPM as passed in.
    varied0: f32,
}

impl Drift<'_> {
    /// Test C (0x1011a1c00–0x1011a52ac). Positions between 5 % and 80 % of the track, outside
    /// irregular stretches, are compared with a uniform grid; after removing the linear trend
    /// and short excursions, the residuals' 32-beat means must wander widely for C to hold.
    /// Fewer than 65 usable beats count as C.
    fn test(&self, cb: &mut f32, vb: &mut f32) -> bool {
        let n = self.list.len() as i32;
        let (st, en) = self.irregular();
        let lo5 = (f64::from(n) * 0.05 + 0.5) as i32;
        let hi80 = (f64::from(n) * 0.8 + 0.5) as i32;
        if hi80 - lo5 < 65 {
            return true;
        }
        // Segments between the stretches that start by the 80 % mark (none if all 128 do).
        let kk = (0..128).find(|&s| st[s] == 0 || hi80 < st[s]).unwrap_or(0);
        let mut a = vec![0i32; kk + 1];
        let mut b = vec![0i32; kk + 1];
        a[0] = lo5;
        for s in 0..kk {
            b[s] = st[s] - 2;
            a[s + 1] = en[s] + 2;
        }
        b[kk] = hi80;
        let mut last = kk as i32;
        let mut s = 0i32;
        'drop: loop {
            while a[s as usize] > b[s as usize] {
                for j in s as usize..last as usize {
                    a[j] = a[j + 1];
                    b[j] = b[j + 1];
                }
                a[last as usize] = 0;
                b[last as usize] = 0;
                last -= 1;
                if last < s {
                    break 'drop;
                }
            }
            s += 1;
            if s > last {
                break;
            }
        }
        if last < 0 {
            return true;
        }
        let segs = (last + 1) as usize;
        let lens: Vec<i32> = (0..segs).map(|s| b[s] - a[s] + 1).collect();
        let total: i32 = lens.iter().sum();
        if total < 1 {
            return true;
        }
        let mut res = self.residuals(&a, &lens, total);
        let mut beat = self.beat;
        self.detrend(&mut res, &mut beat, cb, vb);
        if total < 2 {
            return true;
        }
        let (l, cd) = excursions(&mut res, beat);
        let cnt = cd.len() as i32;
        if l - cnt < 2 {
            return true;
        }
        let (cf, dv) = phases(&res, l, &cd, beat);
        measures(&cf, &dv, n, beat)
    }

    /// Stretches of two or more intervals more than 10 ms off the reference: first beat and one
    /// past the last, up to 128.
    fn irregular(&self) -> ([i32; 128], [i32; 128]) {
        let n = self.list.len() as i32;
        let (mut st, mut en) = ([0i32; 128], [0i32; 128]);
        if n < 3 {
            return (st, en);
        }
        let e: Vec<f64> = self.raw[..(n - 1) as usize]
            .iter()
            .map(|&x| x - self.r)
            .collect();
        let (mut k, mut i) = (0usize, 0i32);
        loop {
            let j = i;
            i = j + 1;
            if e[j as usize].abs() > 10.0 && e[(j + 1) as usize].abs() > 10.0 {
                st[k] = j + 2;
                let mut p = j + 1;
                if p < n - 2 {
                    loop {
                        p += 1;
                        if !(e[p as usize].abs() > 10.0 && p < n - 2) {
                            break;
                        }
                    }
                }
                i = p + 1;
                en[k] = p + 1;
                k += 1;
            }
            if !(i < n - 2 && k < 128) {
                break;
            }
        }
        (st, en)
    }

    /// Positions minus the uniform grid, chained across segments (each segment starts at the
    /// previous residual). Grid positions are `trunc(beat·k + 0.5)` with a fused multiply-add.
    fn residuals(&self, a: &[i32], lens: &[i32], total: i32) -> Vec<i32> {
        let pos = |i: i32| self.list[i as usize].pos;
        let grid = |k: i32| self.beat.mul_add(f64::from(k), 0.5) as i32;
        let mut res = vec![0i32; total as usize];
        if lens[0] <= 0 {
            return res;
        }
        let (a0, p0) = (a[0], pos(a[0]));
        let mut idx = 1usize;
        for k in 1..lens[0] {
            res[idx] = pos(a0 + k)
                .wrapping_add(grid(a0))
                .wrapping_sub(p0.wrapping_add(grid(k + a0)));
            idx += 1;
        }
        for s in 1..lens.len() {
            if lens[s] <= 0 {
                continue;
            }
            let (as_, ps) = (a[s], pos(a[s]));
            let base = res[idx - 1];
            res[idx] = base;
            let off = base.wrapping_sub(ps).wrapping_add(grid(as_));
            for k in 1..lens[s] {
                res[idx + k as usize] = off.wrapping_add(pos(as_ + k)).wrapping_sub(grid(k + as_));
            }
            idx += lens[s].max(1) as usize;
        }
        res
    }

    /// Removes the residuals' least-squares line; when its slope is not the beat length itself,
    /// refines the beat length and both BPMs from it (0x1011a2bc0–0x1011a2f04).
    fn detrend(&self, res: &mut [i32], beat: &mut f64, cb: &mut f32, vb: &mut f32) {
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for (k, &y) in res.iter().enumerate() {
            let (x, y) = (k as f64, f64::from(y));
            sx += x;
            sy += y;
            sxx = x.mul_add(x, sxx);
            sxy = y.mul_add(x, sxy);
        }
        let lf = res.len() as f64;
        let mx = sx / lf;
        let mxl = mx * lf;
        let q = mxl * mx;
        if (sxx - q).abs() <= EPS {
            return;
        }
        let my = sy / lf;
        let slope = (-mxl).mul_add(my, sxy) / (sxx - q);
        for (k, y) in res.iter_mut().enumerate() {
            *y = y.wrapping_sub((k as f64 - mx).mul_add(slope, my).round() as i32);
        }
        if (*beat - slope).abs() <= EPS {
            return;
        }
        let rf = f64::from(self.rate);
        let shorter = *beat - slope;
        let per_min = rf * 60.0;
        let drift = slope / rf;
        // Both BPMs with the drift taken out of their beat lengths (in s).
        let v_len = 60.0 / f64::from(self.varied0);
        let v_new = if (v_len - drift).abs() > EPS {
            (60.0 / (v_len - drift)) as f32
        } else {
            0.0
        };
        let c_len = 60.0 / f64::from(*cb);
        let c_new = if (c_len - drift).abs() > EPS {
            (60.0 / (c_len - drift)) as f32
        } else {
            0.0
        };
        let (x0, x1) = (per_min / *beat, per_min / shorter);
        let (r0, r1) = (x0.round(), x1.round());
        if r0 == r1 && (x0 - r0).abs() > (x1 - r1).abs() {
            *beat = shorter;
        }
        // A corrected BPM replaces the stored one when it rounds to the same whole BPM and is
        // closer to it.
        let w = v_new.round();
        if self.varied0.round() == w && (*vb - w).abs() > (v_new - w).abs() {
            *vb = v_new;
        }
        let (wc, wn) = (cb.round(), c_new.round());
        if wc == wn && (*cb - wc).abs() > (c_new - wn).abs() {
            *cb = c_new;
        }
        // Beat length snapped to a tenth of a BPM within 0.1 %.
        let x = (rf * 600.0) / *beat;
        let rx = x.round();
        if !(x > rx * 1.001 || x < rx * 0.999) {
            *beat = (rf * 600.0) / rx;
        }
        *vb = snap_tenth(*vb);
        *cb = snap_tenth(*cb);
    }
}

/// A BPM within 0.1 % of a tenth of a BPM becomes that tenth (single precision).
fn snap_tenth(v: f32) -> f32 {
    let r = (v * 10.0).round();
    let (lo, hi) = (
        r * f32::from_bits(0x3dcc_985f),
        r * f32::from_bits(0x3dcd_013b),
    );
    if v <= hi && v >= lo { r * 0.1 } else { v }
}

/// Jumps between neighbouring residuals above 1/16 beat (after removing the smallest jump);
/// two jumps less than 20 beats apart cut the residuals between them out. Then the residuals are
/// lowered by their minimum around the jumps. Returns the residual count and the jumps.
fn excursions(res: &mut [i32], beat: f64) -> (i32, Vec<i32>) {
    let len = res.len();
    let mut dif: Vec<i32> = (0..len - 1)
        .map(|k| res[k + 1].wrapping_sub(res[k]).wrapping_abs())
        .collect();
    let low = dif.iter().copied().min().unwrap_or(0);
    dif.iter_mut().for_each(|x| *x = x.wrapping_sub(low));
    let thr = beat * 0.0625;
    let jumps: Vec<i32> = (0..dif.len())
        .filter(|&k| thr < f64::from(dif[k]))
        .map(|k| k as i32)
        .collect();
    let mut cd = vec![0i32; len];
    cd[..jumps.len()].copy_from_slice(&jumps);
    let mut cnt = jumps.len() as i32;
    let mut l = len as i32;
    if cnt - 1 != 0 && cnt > 0 {
        let mut i = 0i32;
        'scan: loop {
            loop {
                let (ja, jb) = (cd[i as usize], cd[(i + 1) as usize]);
                let gap = jb - ja;
                if gap > 19 {
                    break;
                }
                let nl = l - gap;
                if ja + 1 < nl {
                    for j in ja + 1..nl {
                        res[j as usize] = res[(j + gap) as usize];
                    }
                }
                if gap > 0 {
                    res[nl as usize..l as usize].fill(0);
                }
                for j in i..cnt - 1 {
                    cd[j as usize] = cd[(j + 1) as usize] - gap;
                }
                cd[(cnt - 1) as usize] = 0;
                cnt -= 1;
                l = nl;
                if cnt - 1 <= i {
                    break 'scan;
                }
            }
            i += 1;
            if i >= cnt - 1 {
                break;
            }
        }
    }
    let cd = cd[..cnt.max(0) as usize].to_vec();
    // The minimum: before the first jump, between jumps (where the binary compares the j-th
    // residual, not the range's) and after the last; the last residual is left out.
    let mut low = res[0];
    if cd.is_empty() {
        if l > 2 {
            low = res[1..(l - 1) as usize].iter().fold(low, |m, &x| m.min(x));
        }
    } else {
        if cd[0] > 6 {
            low = res[1..(cd[0] - 5) as usize]
                .iter()
                .fold(low, |m, &x| m.min(x));
        }
        for j in 0..cd.len() - 1 {
            if cd[j] + 5 < cd[j + 1] - 5 {
                low = low.min(res[j]);
            }
        }
        let lo = cd[cd.len() - 1] + 5;
        if lo < l - 1 {
            low = res[lo as usize..(l - 1) as usize]
                .iter()
                .fold(low, |m, &x| m.min(x));
        }
    }
    if l > 0 {
        res[..l as usize]
            .iter_mut()
            .for_each(|x| *x = x.wrapping_sub(low));
    }
    (l, cd)
}

/// Per range between jumps (5 beats clear of each): the mean residual in beats picks a phase
/// target (a whole, half or quarter beat off); each residual becomes its offset from the target
/// (`cf`) and its distance to it (`dv`). Ranges with no clear phase keep the residual in both.
fn phases(res: &[i32], l: i32, cd: &[i32], beat: f64) -> (Vec<i32>, Vec<i32>) {
    let cnt = cd.len() as i32;
    let size = (l - 1 - cnt).max(0) as usize;
    let (mut cf, mut dv) = (vec![0i32; size], vec![0i32; size]);
    let mut at = 0usize;
    let sum = |lo: i32, hi: i32| {
        res[lo as usize..hi as usize]
            .iter()
            .fold(0.0, |s, &x| s + f64::from(x))
    };
    let mut put = |lo: i32, hi: i32, q: f64| {
        let fl = q.trunc();
        let frac = q - fl;
        let target = if frac < 0.1 && -0.1 < frac {
            Some(beat * fl)
        } else if 0.9 < frac {
            Some(fl.mul_add(beat, beat))
        } else if frac < -0.9 {
            Some(fl.mul_add(beat, -beat))
        } else if 0.4 < frac && frac < 0.6 {
            Some((fl + 0.5) * beat)
        } else if -0.6 < frac && frac < -0.4 {
            Some((fl + -0.5) * beat)
        } else if 0.15 < frac && frac < 0.35 {
            Some((fl + 0.25) * beat)
        } else if -0.35 < frac && frac < -0.15 {
            Some((fl + -0.25) * beat)
        } else if 0.65 < frac && frac < 0.85 {
            Some((fl + 0.75) * beat)
        } else if -0.85 < frac && frac < -0.65 {
            Some((fl + -0.75) * beat)
        } else {
            None
        };
        for &v in &res[lo as usize..hi as usize] {
            let x = f64::from(v);
            match target {
                Some(tg) => {
                    cf[at] = ((0.5 - tg) + x) as i32;
                    dv[at] = ((x - tg).abs() + 0.5) as i32;
                }
                None => {
                    cf[at] = v;
                    dv[at] = v;
                }
            }
            at += 1;
        }
    };
    if cd.is_empty() {
        if l > 1 {
            let c = l - 1;
            put(0, c, sum(0, c) / (beat * f64::from(c)));
        }
    } else {
        if cd[0] >= 6 {
            let c = cd[0] - 5;
            put(0, c, sum(0, c) / (beat * f64::from(c)));
        }
        let inv = 1.0 / beat;
        for j in 1..cd.len() {
            let (lo, hi) = (cd[j - 1] + 5, cd[j] - 5);
            if lo < hi {
                let mean = sum(lo, hi) / f64::from(hi - lo);
                put(lo, hi, mean * inv);
            }
        }
        let lo = cd[cd.len() - 1] + 5;
        if lo < l - 1 {
            let c = l - 1 - lo;
            put(lo, l - 1, sum(lo, l - 1) / (beat * f64::from(c)));
        }
    }
    cf.truncate(at);
    dv.truncate(at);
    (cf, dv)
}

/// The final C measures on 32-beat means of the phase offsets and distances: the distances'
/// range, the 16-step changes of both and their second differences, all against the beat
/// length or fixed bounds, scaled by the beat count `n`.
fn measures(cf: &[i32], dv: &[i32], n: i32, beat: f64) -> bool {
    let nout = cf.len();
    if nout < 64 {
        return true;
    }
    let m = nout - 31;
    let mut off = vec![0.0; m];
    let mut dist = vec![0.0; m];
    for i in 0..m {
        let (mut s1, mut s2) = (0.0, 0.0);
        for k in 0..32 {
            s1 += f64::from(cf[i + k]);
            s2 += f64::from(dv[i + k]);
        }
        off[i] = s1 * 0.03125;
        dist[i] = s2 * 0.03125;
    }
    let low = dist.iter().copied().fold(dist[0], f64::min);
    dist.iter_mut().for_each(|x| *x -= low);
    let high = dist.iter().copied().fold(dist[0], f64::max);
    let k2 = nout - 47;
    let d_off: Vec<f64> = (0..k2).map(|i| off[i + 16] - off[i]).collect();
    let d_dist: Vec<f64> = (0..k2).map(|i| dist[i + 16] - dist[i]).collect();
    let s_off = neon_sum8(&d_off);
    let s_dist = neon_sum8(&d_dist.iter().map(|x| x.abs()).collect::<Vec<_>>());
    let var = neon_sumsq8(&d_off, s_off / k2 as f64);
    let k3 = nout - 63;
    let dd_dist: Vec<f64> = (0..k3)
        .map(|i| (d_dist[i + 16] - d_dist[i]).abs())
        .collect();
    let s_dd = neon_sum8(&dd_dist);
    let nf = f64::from(n);
    let (mf, k2f, k3f) = (m as f64, k2 as f64, k3 as f64);
    high * nf > beat * 0.13 * mf
        && (s_dist * nf) / k2f > k2f * 380.0
        && (s_dd * nf) / k3f > k3f * 130.0
        && (var * nf) / k2f > beat * 6.4 * k2f
}
