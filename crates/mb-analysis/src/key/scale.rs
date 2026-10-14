//! The decision half of `BeatAnalyzer_1_0::KeyCodeScale` @0x101158438 (`analysis-key.md` § 6):
//! from the constant-Q matrix to the two key codes `(key1, key2)`.
//!
//! Key codes: 0–11 major, 12–23 minor, root pitch class `code % 12` with 0 = A. Pitch classes in
//! the ranking are 0-based; the degree table holds them 1-based, as the binary does.

use super::cqt::{self, Bank};

/// The harmonic index of a key (`harmonic_mixing_table` @0x1037c7ad8): its place on the circle
/// of fifths, minor keys 0–11 from A♭ minor, major keys 12–23 from B major, so a key and its
/// relative share `index % 12`.
fn harmonic(key: usize) -> i32 {
    let k = key as i32;
    if key < 12 {
        12 + (7 * (k + 10)) % 12
    } else {
        (7 * (k - 11)) % 12
    }
}

/// The candidate scores for mode 1 (`param_7 = 1`), `[long track, short track]` halves.
/// Rank of the root among the profile's five strongest classes → base score; then, for each of
/// the seven strongest classes, a bonus when it is the candidate's third, fourth or fifth.
struct Tables {
    first_maj: [f32; 10],
    first_min: [f32; 10],
    third: [f32; 14],
    fourth: [f32; 14],
    fifth: [f32; 14],
}

/// `table_1st_maj` / `table_1st_maj2` (equal).
const FIRST_MAJ: [f32; 10] = [3.2, 2.7, 2.1, 1.6, 1.1, 3.2, 2.7, 2.1, 1.6, 1.1];
/// `table_1st_min` / `table_1st_min2` (equal).
const FIRST_MIN: [f32; 10] = [3.781, 2.7, 2.1, 1.6, 1.1, 3.67, 2.7, 2.1, 1.6, 1.1];
/// `table_4th2` (the whole-track pass uses it too).
const FOURTH2: [f32; 14] = [
    0.2, 0.16, 0.128, 0.102, 0.072, 0.043, 0.0, 0.2, 0.17, 0.13, 0.1, 0.07, 0.03, 0.0,
];
/// `table_3rd2`.
const THIRD2: [f32; 14] = [
    2.4, 1.92, 1.536, 1.229, 0.8, 0.4, 0.0, 2.4, 2.0, 1.6, 1.2, 0.8, 0.4, 0.0,
];
/// `table_5th` (equal to `table_5th2`).
const FIFTH: [f32; 14] = [
    2.4, 1.92, 1.536, 1.229, 0.8, 0.48, 0.0, 2.4, 2.0, 1.6, 1.2, 0.8, 0.4, 0.0,
];

/// Whole track: `table_1st_maj`, `table_1st_min`, `table_3rd`, `table_4th2`, `table_5th`.
const WHOLE: Tables = Tables {
    first_maj: FIRST_MAJ,
    first_min: FIRST_MIN,
    third: [
        2.4, 1.92, 1.42, 1.229, 0.84, 0.4, 0.0, 2.4, 2.0, 1.6, 1.2, 0.8, 0.4, 0.0,
    ],
    fourth: FOURTH2,
    fifth: FIFTH,
};

/// Windows: `table_1st_maj2`, `table_1st_min2`, `table_3rd2`, `table_4th2`, `table_5th2`.
const WINDOW: Tables = Tables {
    first_maj: FIRST_MAJ,
    first_min: FIRST_MIN,
    third: THIRD2,
    fourth: FOURTH2,
    fifth: FIFTH,
};

/// The weight of each degree (root, 2nd … 7th) in the 24-key score; a degree whose class ranks
/// at place `p` (1–12) adds `weight · p`, rounded to f32.
const DEGREE_WEIGHT: [f32; 7] = [4.0, 0.9, 3.0, 0.9, 3.5, 0.6, 2.0];

/// Whole-track bonus for the five best of the 24-key ranking.
const BONUS: [f32; 5] = [9.0, 6.0, 5.0, 4.0, 1.0];

/// `QSort3` @0x10113fb48: `QSort` (descending) and then every run of equal values re-sorted by
/// ascending index (`QSort2_Ascend`). Equivalent to this stable insertion sort.
pub fn qsort3(v: &mut [f32], idx: &mut [i32]) {
    for i in 1..v.len() {
        let (x, ix) = (v[i], idx[i]);
        let mut j = i;
        while j > 0 && (v[j - 1] < x || (v[j - 1] == x && idx[j - 1] > ix)) {
            v[j] = v[j - 1];
            idx[j] = idx[j - 1];
            j -= 1;
        }
        v[j] = x;
        idx[j] = ix;
    }
}

/// The degree table `[key][d]`: the 1-based pitch class of degree `d` of `key`.
fn deg(key: usize, d: usize) -> i32 {
    const MAJOR: [usize; 7] = [0, 2, 4, 5, 7, 9, 11];
    const MINOR: [usize; 7] = [0, 2, 3, 5, 7, 8, 10];
    let (root, steps) = if key < 12 {
        (key, MAJOR)
    } else {
        (key - 12, MINOR)
    };
    ((root + steps[d]) % 12 + 1) as i32
}

/// The 0-based place of 1-based pitch class `v` in the ranking (0 when absent).
fn pos(r: &[i32; 12], v: i32) -> i32 {
    r.iter().position(|&p| p + 1 == v).unwrap_or(0) as i32
}

/// Ranks the 12-class profile.
fn rank(profile: &[f32; 12]) -> [i32; 12] {
    let mut v = *profile;
    let mut idx: [i32; 12] = std::array::from_fn(|i| i as i32);
    qsort3(&mut v, &mut idx);
    idx
}

/// `(f0, f1)` (1-based pitch classes) → whether their harmonic indices are neighbours.
fn neighbour(key: usize, hj: i32, hk: i32) -> bool {
    (key != 16 && hj - 1 == hk)
        || (key != 2 && hj + 1 == hk)
        || (hj + 11 == hk && (hk == 11 || hk == 23))
        || (hj - 11 == hk && (hk == 0 || hk == 12))
}

/// The ten candidates (the five strongest classes as major and minor roots), scored and ranked:
/// the best `B` and the second `S`.
fn candidates(r: &[i32; 12], short: bool, t: &Tables) -> (usize, usize) {
    let s = short as usize;
    let pick = |key: usize, v: i32| -> Option<&[f32; 14]> {
        if deg(key, 2) == v {
            Some(&t.third)
        } else if deg(key, 3) == v {
            Some(&t.fourth)
        } else if deg(key, 4) == v {
            Some(&t.fifth)
        } else {
            None
        }
    };
    let mut sc = [0f32; 10];
    for c in 0..5 {
        let pc = r[c] as usize;
        let mut maj = t.first_maj[s * 5 + c];
        let mut min = t.first_min[s * 5 + c];
        for (j, &rj) in r.iter().enumerate().take(7) {
            if let Some(tab) = pick(pc, rj + 1) {
                maj += tab[s * 7 + j];
            }
            if let Some(tab) = pick(pc + 12, rj + 1) {
                min += tab[s * 7 + j];
            }
        }
        sc[2 * c] = maj;
        sc[2 * c + 1] = min;
    }
    let mut idx: [i32; 10] = std::array::from_fn(|i| i as i32);
    qsort3(&mut sc, &mut idx);
    let key = |c: i32| r[(c / 2) as usize] as usize + if c & 1 == 1 { 12 } else { 0 };
    (key(idx[0]), key(idx[1]))
}

/// Degree places of `key` in the ranking.
fn places(r: &[i32; 12], key: usize) -> [i32; 7] {
    std::array::from_fn(|d| pos(r, deg(key, d)))
}

/// The test shared by both passes when `S` is minor and its root ranks second or third
/// (`analysis-key.md` § 6.3): `Err(())` swaps outright (every comparison favours `S`),
/// `Ok(h)` is one more reason to swap.
fn root_second(pb: &[i32; 7], ps: &[i32; 7]) -> Result<bool, ()> {
    let a = ps.iter().sum::<i32>() - pb.iter().sum::<i32>();
    let b = (ps[0] - pb[0]) + (ps[2] - pb[2]) + (ps[4] - pb[4]);
    let c = (ps[5] - pb[5]) + (ps[6] - pb[6]);
    if a < 0 && b < 0 && c < 0 {
        return Err(());
    }
    if a > 0 && b > 0 && c > 0 {
        return Ok(false);
    }
    let d = (ps[0] - pb[0]) + (ps[1] - pb[1]) + (ps[2] - pb[2]) + (ps[4] - pb[4]);
    Ok(if a < 5 && c < 5 {
        b + a + c < 4 && b < 1 && (b < -4 || d < -4)
    } else {
        (c < 1 || a < 1) && b < -5 && b + a + c < 4
    })
}

/// Swap `B` and `S`? Combines the three tests with [`root_second`].
fn swap(r: &[i32; 12], pb: &[i32; 7], ps: &[i32; 7], s: usize, b66: bool, u110: bool) -> bool {
    let second = s >= 12 && (r[1] == s as i32 - 12 || r[2] == s as i32 - 12);
    if !second {
        return b66 || u110;
    }
    match root_second(pb, ps) {
        Err(()) => true,
        Ok(h) => b66 || u110 || h,
    }
}

/// The whole-track choice between `B` and `S` (`analysis-key.md` § 6.2): `(key1, key2)`.
fn whole_rules(r: &[i32; 12], short: bool, b0: usize, s: usize) -> (usize, usize) {
    let mut b = b0;
    // Short track, parallel pair: the minor unless the major's third clearly ranks higher.
    if short && (s + 12 == b || b + 12 == s) {
        let (min, maj) = if b > 11 { (b, s) } else { (s, b) };
        let p_maj3 = pos(r, deg(maj, 2));
        let p_min3 = pos(r, deg(min, 2));
        b = min;
        if p_min3 >= 3 && p_maj3 + 2 < p_min3 {
            let lhs = pos(r, deg(min, 5)) + p_min3 + pos(r, deg(min, 6));
            let rhs = p_maj3 + 2 + pos(r, deg(maj, 5)) + pos(r, deg(maj, 6));
            b = if lhs <= rhs { min } else { maj };
        }
    }
    let pb = places(r, b);
    let ps = places(r, s);
    // S is B's parallel minor and its 3rd/6th/7th rank higher.
    let b66 = b + 12 == s && {
        let (b3, s3, b6, s6, b7, s7) = (pb[2], ps[2], pb[5], ps[5], pb[6], ps[6]);
        if b3 <= s3 && b6 <= s6 && b7 <= s7 {
            false
        } else {
            let n = (s3 < b3) as i32 + (s6 < b6) as i32 + (s7 < b7) as i32;
            n > 1 || s6 + s3 + s7 < b6 + b3 + b7
        }
    };
    let harm = harmonic_whole(b, s);
    // S is minor with its root ranked first.
    let u110 = s >= 12 && r[0] == s as i32 - 12 && {
        let lim = if b > 11 { 8 } else { 6 };
        let c1 = (ps[0] - pb[0]) + (ps[1] - pb[1]) + (ps[2] - pb[2]) + (ps[4] - pb[4]) < lim;
        let u = pb[2] + pb[0] + pb[4];
        let c = (ps[2] + ps[0] - u) + ps[4];
        if !(c1 && (u > 7 || c < 5)) {
            false
        } else {
            let t = if s - 12 == b || s + 12 == b {
                0
            } else if pb[0] < 2 {
                2
            } else if pb[0] == 2 {
                3
            } else {
                4
            };
            let a = ps.iter().sum::<i32>() - pb.iter().sum::<i32>();
            let e = (ps[5] - pb[5]) + (ps[6] - pb[6]);
            if harm {
                a <= t && c <= t && e <= t
            } else {
                !(t < a && t < c) || e <= t
            }
        }
    };
    let swap = swap(r, &pb, &ps, s, b66, u110);
    if swap { (s, b) } else { (b, s) }
}

/// The whole-track harmonic relation of `B` and `S`.
fn harmonic_whole(b: usize, s: usize) -> bool {
    let hb = harmonic(b);
    let hs = harmonic(s);
    if !((b == 16 || hs - 1 != hb) && (b == 2 || hs + 1 != hb)) {
        return true;
    }
    if hs + 11 == hb && (hb == 11 || hb == 23) {
        return true;
    }
    if hs - 11 == hb && (hb == 0 || hb == 12) {
        return true;
    }
    if (12..24).contains(&b) && s < 12 {
        if !((hs - 13 != hb || b == 16) && (b == 2 || hs - 11 != hb)) {
            return true;
        }
        if hs - 1 == hb && (hb == 11 || hb == 23) {
            return true;
        }
        return hs - 23 == hb && (b == 23 || b == 2);
    }
    if b < 12 && (12..24).contains(&s) {
        let h12 = hb - 12;
        if (h12 != 11 && hs - 1 == h12) || (h12 != 12 && hs == hb - 13) {
            return true;
        }
        if hs == hb - 23 && (hb == 23 || hb == 35) {
            return true;
        }
        return hs == hb - 1 && (hb == 12 || h12 == 12);
    }
    false
}

/// The window harmonic relation (a reduced form of [`harmonic_whole`]).
fn harmonic_window(b: usize, s: usize) -> bool {
    let hb = harmonic(b);
    let hs = harmonic(s);
    if !((b == 16 || hs - 1 != hb) && (b == 2 || hs + 1 != hb)) {
        return true;
    }
    if hs + 11 == hb && (hb == 11 || hb == 23) {
        return true;
    }
    hs - 11 == hb && (b == 23 || b == 2)
}

/// The per-window choice between `B` and `S` (`analysis-key.md` § 6.5).
fn window_rules(r: &[i32; 12], b: usize, s: usize) -> usize {
    let pb = places(r, b);
    let ps = places(r, s);
    let b66 = b + 12 == s && (ps[2] < pb[2] || ps[5] < pb[5] || ps[6] < pb[6]);
    let harm = harmonic_window(b, s);
    let u110 = s >= 12 && r[0] == s as i32 - 12 && {
        let lim = if b < 12 { 6 } else { 8 };
        let c1 = (ps[0] - pb[0]) + (ps[1] - pb[1]) + (ps[2] - pb[2]) + (ps[4] - pb[4]) < lim;
        let u = pb[2] + pb[0] + pb[4];
        let c = (ps[2] + ps[0] - u) + ps[4];
        if !(c1 && (u > 7 || c < 6)) {
            false
        } else {
            let t = if pb[0] < 3 {
                2
            } else if pb[0] == 3 {
                3
            } else {
                4
            };
            let a = ps.iter().sum::<i32>() - pb.iter().sum::<i32>();
            let e = (ps[5] - pb[5]) + (ps[6] - pb[6]);
            let n = (a <= t) as i32 + (c <= t) as i32 + (e <= t) as i32;
            if harm { n >= 2 } else { n >= 1 }
        }
    };
    let swap = swap(r, &pb, &ps, s, b66, u110);
    if swap { s } else { b }
}

/// The 24-key ranking: every key scored by where its degrees rank (`DEGREE_WEIGHT`), ascending
/// (best first). Also returns the plain place sums over degrees {0, 2, 4} and over all degrees.
fn rank24(r: &[i32; 12]) -> ([i32; 24], [i32; 24], [i32; 24]) {
    let mut w = [0f32; 24];
    let mut tri = [0i32; 24];
    let mut all = [0i32; 24];
    for k in 0..24 {
        for d in 0..7 {
            let p = pos(r, deg(k, d)) as usize;
            w[k] += DEGREE_WEIGHT[d] * (p + 1) as f32;
            all[k] += p as i32 + 1;
            if d == 0 || d == 2 || d == 4 {
                tri[k] += p as i32 + 1;
            }
        }
    }
    let mut idx: [i32; 24] = std::array::from_fn(|i| i as i32);
    qsort3(&mut w, &mut idx);
    // Reversed to ascending, then one bubble pass over equal neighbours.
    w.reverse();
    idx.reverse();
    for i in 0..23 {
        if w[i] == w[i + 1] && idx[i] > idx[i + 1] {
            idx.swap(i, i + 1);
            w.swap(i, i + 1);
        }
    }
    (idx, tri, all)
}

/// The harmonic score of `key` against the keys `others` (`analysis-key.md` § 6.4).
fn harmonic_score(key: usize, others: &[i32]) -> f32 {
    let hk = harmonic(key);
    let mut sc = 0f32;
    let mut first = true;
    for &o in others {
        let o = o as usize;
        let hj = harmonic(o);
        if neighbour(key, hj, hk) {
            // Minor keys: 1.6 for the first neighbour, 1.0 for the rest.
            sc += if key >= 12 && first { 1.6 } else { 1.0 };
            first = false;
        } else if hj + 12 == hk || hj - 12 == hk {
            sc += 1.5;
        } else if o as i32 - 12 == key as i32 || o + 12 == key {
            sc += 3.0;
        }
    }
    sc
}

/// The five best of a 24-key ranking re-scored by harmony with `pool` plus `bonus[i]`; returns
/// the indices (into `order`) of the best two.
fn top5(order: &[i32; 24], pool: usize, bonus: impl Fn(usize) -> f32) -> (usize, usize) {
    let mut sc = [0f32; 5];
    for (i, s) in sc.iter_mut().enumerate() {
        *s = harmonic_score(order[i] as usize, &order[..pool]) + bonus(i);
    }
    let mut idx: [i32; 5] = std::array::from_fn(|i| i as i32);
    qsort3(&mut sc, &mut idx);
    (idx[0] as usize, idx[1] as usize)
}

/// The 12-class profile of frames `start .. start + win` of the raw matrix.
fn window_profile(m: &[f32], frames: usize, start: usize, win: usize) -> [f32; 12] {
    let weights = [0.65f32, 0.5, 1.0, 1.0, 1.0, 1.0];
    std::array::from_fn(|pc| {
        let o: [f32; 6] = std::array::from_fn(|oct| {
            let row = (pc + 12 * oct) * frames + start;
            cqt::row_sum(&m[row..row + win])
        });
        let mut s = o[0] * weights[0];
        for oct in 1..6 {
            s = o[oct].mul_add(weights[oct], s);
        }
        s * s
    })
}

/// Intermediate results, for comparing with the binary.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    /// Whole-track profile.
    pub profile: [f32; 12],
    /// Per-window rule result.
    pub window_keys: Vec<i32>,
    /// Accepted window votes.
    pub votes: Vec<i32>,
    /// Per window: ranking, `B`, `S`.
    pub windows: Vec<([i32; 12], usize, usize)>,
}

/// `KeyCodeScale(…)`: `x[..n]` at `rate` (the decimated rate), reference `pitch`; `segment` is
/// `param_10`, `seg_index` rebuilds the bank when 0. Returns `(key1, key2)`.
pub fn key_code_scale(
    x: &[f32],
    rate: f32,
    pitch: f32,
    segment: bool,
    seg_index: usize,
    bank: &mut Bank,
    trace: Option<&mut Trace>,
) -> (i32, i32) {
    let n = x.len();
    if seg_index == 0 {
        cqt::build_bank(bank, n, rate, pitch, segment);
    }
    let (m, frames, max) = cqt::cqt(x, n, bank);
    let mut norm = m.clone();
    let prof = cqt::profile(&mut norm, frames, max, rate, bank.hop);
    drop(norm);
    let short = (n as i32 as f32) < rate * 30.0;
    decide(&prof, &m, frames, rate, bank.hop, short, trace)
}

/// Everything after the whole-track profile.
pub fn decide(
    prof: &[f32; 12],
    m: &[f32],
    frames: usize,
    rate: f32,
    hop: i32,
    short: bool,
    trace: Option<&mut Trace>,
) -> (i32, i32) {
    // Whole track.
    let r = rank(prof);
    let (b, s) = candidates(&r, short, &WHOLE);
    let (key1, key2) = whole_rules(&r, short, b, s);
    let (order, _, _) = rank24(&r);
    let (t1, t2) = top5(&order, 12, |i| BONUS[i]);
    let (top1, top2) = (order[t1] as usize, order[t2] as usize);

    // 30-second windows every 10 seconds.
    let win = (rate * 30.0 / hop as f32) as u32 as usize;
    let step = ((rate * 10.0) / hop as f32) as i32;
    let count = ((frames as i32 - win as i32) as f32 / step as f32) as u32 as i32;
    let mut window_keys = Vec::new();
    let mut votes = Vec::new();
    let mut windows = Vec::new();
    if count > 0 {
        for w in 0..count {
            let start = step * (w + 1);
            if start + win as i32 >= frames as i32 {
                break;
            }
            let p = window_profile(m, frames, start as usize, win);
            let rw = rank(&p);
            let (bw, sw) = candidates(&rw, short, &WINDOW);
            let pv = window_rules(&rw, bw, sw);
            window_keys.push(pv as i32);
            windows.push((rw, bw, sw));
            let (o, tri, all) = rank24(&rw);
            let cand = window_choice(&o, &tri, &all, pv);
            if pv == cand {
                let root = if cand > 11 { cand - 12 } else { cand } as i32;
                if root == rw[0] || root == rw[1] {
                    votes.push(cand as i32);
                }
            }
        }
    }
    let vote = {
        let mut h = [0i32; 24];
        for &v in &votes {
            h[v as usize] += 1;
        }
        let mut best = 24usize;
        let mut max = 0;
        for (k, &c) in h.iter().enumerate() {
            if c > max {
                max = c;
                best = k;
            }
        }
        best
    };
    if let Some(t) = trace {
        t.profile = *prof;
        t.window_keys = window_keys;
        t.votes = votes;
        t.windows = windows;
    }
    let k1 = finish(&r, key1, key2, top1, top2, vote);
    (k1 as i32, key2 as i32)
}

/// A window's 24-key choice (`analysis-key.md` § 6.6).
fn window_choice(o: &[i32; 24], tri: &[i32; 24], all: &[i32; 24], pv: usize) -> usize {
    let o0 = o[0] as usize;
    let min_tri = o[..5].iter().map(|&k| tri[k as usize]).min().unwrap();
    if o0 >= 12 {
        let root = o0 as i32 - 12;
        if o[1..5].contains(&root) {
            let min_all = o[..5].iter().map(|&k| all[k as usize]).min().unwrap();
            if min_all == all[o0] && min_tri == tri[o0] && o0 == pv {
                return pv;
            }
        }
    }
    if o[0] == o[1] - 12 && min_tri == tri[o[1] as usize] {
        return o[1] as usize;
    }
    let (t1, _) = top5(o, 5, |i| (5 - i) as f32);
    o[t1] as usize
}

/// The final choice of `key1` (`analysis-key.md` § 6.7).
fn finish(r: &[i32; 12], key1: usize, key2: usize, top1: usize, top2: usize, vote: usize) -> usize {
    let sum = |k: usize| places(r, k).iter().sum::<i32>();
    let r0 = r[0] as usize;
    let (s1, s2, s3) = (sum(key1), sum(r0), sum(r0 + 12));
    let mut best = if s1 <= s2 { key1 } else { r0 };
    if s1.min(s2) > s3 {
        best = r0 + 12;
    }
    let vroot = if (12..24).contains(&vote) {
        vote - 12
    } else {
        vote
    };
    let set = [vroot, best % 12, top2 % 12, top1 % 12, key2 % 12, key1 % 12];
    let list: Vec<usize> = (0..24)
        .filter(|&k| {
            [0, 2, 4]
                .iter()
                .all(|&d| set.contains(&((deg(k, d) - 1) as usize)))
        })
        .collect();
    let pos0 = |v: usize| r.iter().position(|&p| p as usize == v).unwrap_or(0);
    let p_key1 = pos0(key1 % 12);
    let p_vote = pos0(vroot);
    let check = |res: usize| -> usize {
        if res != top1 && res != top2 && res != best {
            if key2 != best || key2 != top1 {
                key1
            } else {
                key2
            }
        } else {
            res
        }
    };
    let from_vote = |res: usize| -> usize {
        // LAB_1011659e0
        if p_vote > 3 && vote != key2 {
            vote
        } else {
            check(res)
        }
    };
    let mut u88 = key1;
    if key1 != vote && !((key1 == top1 || key1 == top2) && p_key1 < 2) {
        if list.is_empty() {
            return if vote < 24 { from_vote(vote) } else { key1 };
        }
        if p_key1 >= 3 || !list.contains(&key1) {
            u88 = 24;
        }
    }
    if list.contains(&vote) {
        u88 = vote;
    }
    if u88 < 24 {
        return u88;
    }
    if vote >= 24 {
        return key1;
    }
    if key1 != vote {
        from_vote(vote)
    } else {
        check(key1)
    }
}
