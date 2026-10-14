//! Stage D's verse/hook candidates: `VerseHookAnaData23` @0x100b86a38 and the `VerseHook*`
//! judges (`analysis-phrase.md` § 7). A candidate is a beat index where a phrase may start.

use super::mat;

/// `0.003`, the binary's stand-in for a zero divisor.
const EPS: f64 = 0.003;

/// `MatArrayFloat::sum(i, j)`: 0 for an invalid range.
fn sum_ij(y: &[f64], i: i32, j: i32) -> f64 {
    let n = y.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0.0;
    }
    mat::sum(&y[i as usize..=j as usize])
}

/// One curve of [`VerseData`]: the values scaled by `1/max`, and per bar the change against the
/// previous bar, raw and as a factor ≥ 1 (index 0 stays 0; zeros in `norm` become 0.003).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Band {
    pub norm: Vec<f64>,
    pub spread: Vec<f64>,
    pub raw: Vec<f64>,
}

/// `VerseHookAnaData23`'s twelve outputs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerseData {
    /// Bar means of the beat peaks from the first downbeat.
    pub peak: Band,
    /// `LowLevelAnalyze_b`'s bar sums.
    pub low: Band,
    /// `MiddlePeakAnalyze_b`'s bar means.
    pub mid_bar: Band,
    /// The scaled `MiddlePeakAnalyze` beat peaks (stage C).
    pub mid: Band,
}

/// The ratio pass shared by the four curves: `count` ratios from index 1.
fn ratios(v: &mut [f64], count: usize, raw: &mut [f64], spread: &mut [f64]) {
    for k in 1..=count {
        if k >= v.len() {
            break;
        }
        if v[k - 1] == 0.0 {
            v[k - 1] = EPS;
        }
        if v[k] == 0.0 {
            v[k] = EPS;
        }
        let r = v[k] / v[k - 1];
        raw[k] = r;
        spread[k] = if 1.0 <= r { r } else { 1.0 / r };
    }
}

fn scale_of(v: &[f64]) -> f64 {
    let mx = mat::max_val(v);
    if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 }
}

/// A curve given per bar: all but the last value scaled, then the ratios.
fn band(src: &[f64]) -> Band {
    let mut norm = src.to_vec();
    let n = norm.len();
    let mut raw = vec![0.0f64; n];
    let mut spread = vec![0.0f64; n];
    let s = scale_of(&norm);
    if n > 1 {
        for v in &mut norm[..n - 1] {
            *v *= s;
        }
        if n > 2 {
            ratios(&mut norm, n - 2, &mut raw, &mut spread);
        }
    }
    Band { norm, spread, raw }
}

/// `VerseHookAnaData23(peaks, low_bars, mid, mid_bars, bar)`.
pub fn verse_hook_ana_data23(
    peaks: &[f64],
    low: &[f64],
    mid: &[f64],
    mid_bars: &[f64],
    bar: i32,
) -> VerseData {
    let m = peaks.len() as i32 - bar;
    let nb = mat::cdiv(m + 1, 4).max(0) as usize;
    let mut p = vec![0.0f64; nb];
    let mut spread = vec![0.0f64; nb];
    let mut raw = vec![0.0f64; nb];
    if m + 1 > 7 {
        let count = (nb as i32 - 1).max(1) as usize;
        let mut i = bar - 1;
        for d in p.iter_mut().take(count) {
            *d = sum_ij(peaks, i, i + 3) * 0.25;
            i += 4;
        }
        let s = scale_of(&p);
        for v in p.iter_mut().take(count) {
            *v *= s;
        }
        if m + 1 > 11 {
            let count = nb.max(3) - 2;
            ratios(&mut p, count, &mut raw, &mut spread);
        }
    }
    VerseData {
        peak: Band { norm: p, spread, raw },
        low: band(low),
        mid_bar: band(mid_bars),
        mid: band(mid),
    }
}

/// Beat steps tried first by `VerseHookJudge_LowPeak` (0x1038292d8); later steps are 16 beats
/// apart.
const LOW_PEAK_STEPS: [i32; 6] = [32, 64, 16, 96, 48, 80];

/// `VerseHookJudge_LowPeak(start, beat_flags, bar, bar_flags)` @0x1020ded04: from `start`,
/// candidate phrase starts 16–96 beats on where the low band jumps (`LowPeakAnalyze`'s flag on
/// the beat or the one before, or `LowLevelAnalyze_b`'s bar flag); each hit restarts the
/// search. Returns (the hit count, the list starting with `start`, padded with zeros to its
/// allocated size).
pub fn verse_hook_low_peak(start: i32, flags: &[f64], bar: i32, bar_flags: &[u8]) -> Option<(i32, Vec<i32>)> {
    let per_beat: Vec<u8> = bar_flags.iter().flat_map(|&v| [v; 4]).collect();
    let from = (bar - 1).max(0) as usize;
    let mut f: Vec<f64> = flags.get(from..).unwrap_or(&[]).to_vec();
    f.resize(per_beat.len(), 0.0);
    let len = f.len() as i32;
    let cap = (f64::from(len).mul_add(0.0625, 0.5)) as i32;
    if cap < 0 {
        return None;
    }
    let mut list = vec![0i32; cap as usize];
    let mut written: Vec<i32> = Vec::new();
    let put = |list: &mut Vec<i32>, written: &mut Vec<i32>, i: usize, v: i32| {
        if i < list.len() {
            list[i] = v;
        }
        written.push(v);
    };
    put(&mut list, &mut written, 0, start);
    let mut start = start;
    let mut next = 2i32;
    let mut step = 1i32;
    let fv = |i: i32| if i >= 0 { f.get(i as usize).copied().unwrap_or(0.0) } else { 0.0 };
    loop {
        let cand = if ((step - 1) as u32) < 6 {
            LOW_PEAK_STEPS[(step - 1) as usize] + start
        } else {
            step * 16 + start
        };
        let mut hit = false;
        if cand < len {
            let i = cand - bar;
            hit = fv(i) == 1.0
                || fv(i - 1) == 1.0
                || (i >= 0 && per_beat.get(i as usize).copied() == Some(1));
        }
        if hit {
            put(&mut list, &mut written, (next - 1) as usize, cand);
            next += 1;
            start = cand;
            step = 1;
        } else {
            step += 1;
        }
        if !(cand < len - 4 || step < 7) {
            break;
        }
    }
    // Zero entries among the written ones are removed (none beyond the allocation).
    let mut count = next;
    let mut i = next - 2;
    while i >= 0 {
        let v = if (i as usize) < list.len() { list[i as usize] } else { written[i as usize] };
        if v == 0 {
            if (i as usize) >= list.len() {
                return None;
            }
            list.remove(i as usize);
            count -= 1;
        }
        i -= 1;
    }
    Some((count - 1, list))
}

/// Per-beat copies of bar values (each repeated four times).
fn per_beat(v: &[f64]) -> Vec<f64> {
    v.iter().flat_map(|&x| [x; 4]).collect()
}

/// Reads `v[i]`, replacing (and storing) a zero with 0.003; 0 out of range.
fn nz_at(v: &mut [f64], i: i32) -> f64 {
    match v.get_mut(i as usize) {
        Some(x) if i >= 0 => {
            if *x == 0.0 {
                *x = EPS;
            }
            *x
        }
        _ => 0.0,
    }
}

pub(super) fn get(v: &[f64], i: i32) -> f64 {
    if i < 0 { 0.0 } else { v.get(i as usize).copied().unwrap_or(0.0) }
}

/// `BreakFind(count, starts, bar, low_bars, mid_bars, all_bars)` @0x101391d04: the first
/// candidate (after the first) where all three bands drop (low under 0.35×, mid under 0.65×,
/// full band under 0.8×) and come back 4, 8 or 12 beats later (low over 2.7×, mid over 1.3×,
/// full over 1.2×). Returns (break start, break end, deep: the mid band fell under 0.3× and
/// came back by at least 2.5×).
pub fn break_find(count: i32, starts: &[i32], bar: i32, low: &[f64], mid: &[f64], all: &[f64]) -> (i32, i32, i32) {
    let m = low.len().min(mid.len()).min(all.len());
    let mut a = per_beat(&low[..m]);
    let mut b = per_beat(&mid[..m]);
    let mut c = per_beat(&all[..m]);
    let (mut start, mut end, mut deep) = (0, 0, 0);
    if count <= 2 {
        return (start, end, deep);
    }
    let mut was_deep = false;
    for t in 1..=(count - 2) as usize {
        let l = starts.get(t).copied().unwrap_or(0);
        let i = l - bar;
        let a0 = nz_at(&mut a, i - 1);
        let b0 = nz_at(&mut b, i - 1);
        let c0 = nz_at(&mut c, i - 1);
        let r_mid = get(&b, i) / b0;
        if !(get(&a, i) / a0 < 0.35) || r_mid >= 0.65 || get(&c, i) / c0 >= 0.8 {
            continue;
        }
        if r_mid < 0.3 {
            was_deep = true;
            deep = 1;
        }
        let mut found = None;
        for (k, len) in [(3, 4), (7, 8), (11, 12)] {
            let (a1, b1, c1) = (nz_at(&mut a, i + k), nz_at(&mut b, i + k), nz_at(&mut c, i + k));
            if get(&a, i + k + 1) / a1 <= 2.7 {
                continue;
            }
            let rb = get(&b, i + k + 1) / b1;
            if rb <= 1.3 || get(&c, i + k + 1) / c1 <= 1.2 {
                continue;
            }
            found = Some((len, rb));
            break;
        }
        if let Some((len, rb)) = found {
            if was_deep && rb < 2.5 {
                deep = 0;
            }
            start = l;
            end = l + len;
            break;
        }
    }
    (start, end, deep)
}

/// `BigChangeFind(count, starts, bar, low_bars, mid_bars, all_bars, level, peaks)`
/// @0x1015adac0: from the first candidate, every 4 beats, the first two places where the bands
/// change a lot at once (five patterns of low/mid/full/peak/level ratios), skipping those on
/// the 16-beat grid before the first and those that are candidates already. Returns (first
/// change's phrase start, first change, second change's phrase start, second change).
#[allow(clippy::too_many_arguments)]
pub fn big_change_find(
    count: i32,
    starts: &[i32],
    bar: i32,
    low: &[f64],
    mid: &[f64],
    all: &[f64],
    level: &[f64],
    peaks: &[f64],
) -> (i32, i32, i32, i32) {
    let mut lv = level.to_vec();
    let from = (bar - 1).max(0) as usize;
    let mut pk: Vec<f64> = peaks.get(from..).unwrap_or(&[]).to_vec();
    let q = |n: usize| (n as f64).mul_add(0.25, 0.5) as i32;
    let m = (low.len() as i32)
        .min(mid.len() as i32)
        .min(all.len() as i32)
        .min(q(pk.len()))
        .min(q(lv.len()))
        .max(0) as usize;
    let mut a = per_beat(&low[..m]);
    let mut b = per_beat(&mid[..m]);
    let mut c = per_beat(&all[..m]);
    let first_start = starts.first().copied().unwrap_or(0);
    let off = first_start - (bar - 1);
    let span = m as i32 - (f64::from(off).mul_add(0.25, 0.5)) as i32;
    let mut out = (0, 0, 0, 0);
    if span - 8 <= 0 {
        return out;
    }
    let mut first = 0i32;
    let mut second = 0i32;
    let mut have = false;
    let lv_nz = |v: &mut Vec<f64>, i: i32| -> f64 {
        match v.get_mut(i as usize) {
            Some(x) if i >= 0 => {
                if *x == 0.0 {
                    *x = 0.018;
                }
                *x
            }
            _ => 0.0,
        }
    };
    'scan: for u in 1..span - 7 {
        let k = 4 * u + off;
        let a0 = nz_at(&mut a, k - 2);
        let b0 = nz_at(&mut b, k - 2);
        let c0 = nz_at(&mut c, k - 2);
        let p0 = nz_at(&mut pk, k - 2);
        let rl = get(&a, k - 1) / a0;
        let rm = get(&b, k - 1) / b0;
        let ra = get(&c, k - 1) / c0;
        let v1 = lv_nz(&mut lv, k - 1);
        let v0 = lv_nz(&mut lv, k - 2);
        let big = if rl <= 20.0 || rm <= 1.4 || ra <= 1.3 {
            let rp = get(&pk, k - 1) / p0;
            let rv = v1 / v0;
            (1.2 < rp && 2.5 < rl && 2.5 < rm && 1.7 < ra && 3.0 < rv)
                || (6.5 < rp && 8.5 < rl && rm < 0.95 && 1.0 < rv)
                || (2.7 < rm && 2.5 < ra && 1.5 < rv)
                || (rp < 0.1 && rl < 0.1 && 1.0 < rm && ra < 0.8 && rv < 0.8)
                || (4.0 < rp && 2.5 < rl && 1.3 < rm && ra < 1.0 && rv < 1.0)
        } else {
            true
        };
        if !big {
            continue;
        }
        if count > 1 {
            let d = f64::from(4 * u) * 0.0625;
            if d == ((d + 0.5) as i64) as f64 && !have {
                continue;
            }
            if have {
                second = k;
                break 'scan;
            }
            if starts[1..count as usize].contains(&(4 * u + first_start)) {
                have = false;
                continue;
            }
        }
        if have {
            second = k;
            break 'scan;
        }
        have = true;
        first = k;
    }
    let grid = |x: i32| (if x < 0 { x + 15 } else { x } & !15);
    if first != 0 {
        out.0 = grid(first + bar - 1 - first_start) + first_start;
        out.1 = first + bar - 1;
    }
    if second != 0 {
        let y = second + bar - 1;
        out.2 = grid(y - first_start) + first_start;
        out.3 = y;
    }
    out
}

/// The arguments of [`irregular_judge`] after the candidate list.
#[derive(Debug, Clone, Copy)]
pub struct Irregular {
    /// The intro end beat.
    pub intro: i32,
    pub bar: i32,
    /// The last candidate gap start (Sub_Main_D's scan of the candidate list).
    pub gap: i32,
    /// `BreakFind`'s break (start, end) and deep flag.
    pub brk: (i32, i32),
    pub deep: i32,
    /// `BigChangeFind`'s (phrase start, change) pairs.
    pub change1: (i32, i32),
    pub change2: (i32, i32),
}

/// `IrregularJudge(intro, count, starts, beat_flags, bar, gap, break…, changes…, bar_flags,
/// deep)` @0x101e46988: decides which irregular place (a break, a big change, or the best of
/// the gap and the 4/8/12 beats after it) starts the regular phrase grid, by comparing how
/// many low-band hits `VerseHookJudge_LowPeak` finds from each. Returns (kind 0/1/2, the
/// starts: intro, then the chosen place and the second change; the last `LowPeak` list).
pub fn irregular_judge(
    a: Irregular,
    starts: &[i32],
    flags: &[f64],
    bar_flags: &[u8],
) -> Option<(i32, Vec<i32>, Vec<i32>)> {
    let per: Vec<u8> = bar_flags.iter().flat_map(|&v| [v; 4]).collect();
    let mut last = starts.to_vec();
    let low = |s: i32, last: &mut Vec<i32>| -> Option<i32> {
        let (n, l) = verse_hook_low_peak(s, flags, a.bar, bar_flags)?;
        *last = l;
        Some(n)
    };
    let fl = |i: i32| if i >= 0 { flags.get(i as usize).copied().unwrap_or(0.0) } else { 0.0 };
    let pb = |i: i32| if i >= 0 { per.get(i as usize).copied().unwrap_or(0) } else { 0 };
    let gap = a.gap;
    let mut pick = gap;
    if a.brk.0 != 0 {
        let n_a = low(a.brk.0, &mut last)?;
        let n_b = low(a.brk.1, &mut last)?;
        let lim = if a.deep == 1 { n_b + 2 } else { n_b };
        if n_a <= lim {
            pick = a.brk.1;
        }
    }
    let mut chosen = pick;
    let mut second = 0;
    if a.change1.0 != 0 && pick != a.change1.1 {
        let n_c = low(a.change1.0, &mut last)?;
        let n_d = low(a.change1.1, &mut last)?;
        if n_c <= n_d {
            chosen = a.change1.1;
            if pick != gap {
                second = a.change1.1;
                chosen = pick;
            }
        }
    }
    let mut extra = second;
    if chosen != a.change2.1 && a.change2.0 != 0 && second == 0 {
        let n_e = low(a.change2.0, &mut last)?;
        let n_f = low(a.change2.1, &mut last)?;
        if n_f < n_e {
            extra = 0;
        } else if chosen == gap {
            extra = 0;
            chosen = a.change2.1;
        } else {
            second = a.change2.1;
            extra = a.change2.1;
        }
    }
    let off = 1 - a.bar;
    let hit = |p: i32| 0 < off + p && (fl(p - 1) > 0.0 || fl(p - 2) > 0.0 || pb(off + p - 1) != 0);
    if gap != 0 && chosen == gap {
        // Among the gap and the 4/8/12 beats after it, the start with the most hits.
        let mut best = 0;
        let mut at = gap;
        let mut stage = 0;
        if gap > 1 {
            if hit(gap) {
                best = low(gap, &mut last)?.max(0);
            }
        } else if gap > -3 {
        } else if (gap as u32) > 0xffff_fff9 {
            stage = 1;
        } else if (gap as u32) > 0xffff_fff5 {
            stage = 2;
        } else {
            return Some((0, vec![a.intro], last));
        }
        for (k, p) in [(0, gap + 4), (1, gap + 8), (2, gap + 12)] {
            if k < stage || !hit(p) {
                continue;
            }
            let n = low(p, &mut last)?;
            if !(n <= best) {
                at = p;
            }
            if k < 2 && best <= n {
                best = n;
            }
        }
        chosen = at;
    }
    let (mut kind, mut list) = if chosen == gap {
        (0, vec![a.intro])
    } else {
        extra = second;
        (1, vec![a.intro, chosen])
    };
    if extra != 0 {
        kind = 2;
        list.push(second);
    }
    Some((kind, list, last))
}

/// The five curves `RatioCalc_Verse` reads (copies owned by `VerseHookJudge_All`; zeros are
/// replaced in place as they are read).
pub struct RatioCurves {
    /// Beat peaks (per beat).
    pub peaks: Vec<f64>,
    /// Low-band and mid-band bar sums, full-band bar means (per bar).
    pub low: Vec<f64>,
    pub mid: Vec<f64>,
    pub all: Vec<f64>,
    /// `IntroEndFind_b`'s level (per beat).
    pub level: Vec<f64>,
}

fn spread(r: f64) -> f64 {
    if 1.0 <= r { r } else { 1.0 / r }
}

/// `RatioCalc_Verse(pos, …, flags, weight)` @0x101b70794: how much the music changes at beat
/// `pos`: the sum of the change factors (≥ 1) of the beat peak (×0.7 on a 0.4/0.8 jump flag,
/// ×1.2 after four flagged beats), the low, mid and full bands and the level, times `weight`.
pub fn ratio_calc_verse(pos: i32, c: &mut RatioCurves, flags: &[f64], weight: f64) -> f64 {
    let p = pos;
    for k in [p - 1, p - 2, p - 3] {
        nz_at(&mut c.peaks, k);
    }
    let iv = (f64::from(p)).mul_add(0.25, 0.5) as i32;
    nz_at(&mut c.low, iv - 1);
    nz_at(&mut c.low, iv);
    nz_at(&mut c.mid, iv - 1);
    nz_at(&mut c.mid, iv);
    nz_at(&mut c.level, p - 1);
    nz_at(&mut c.level, p - 2);
    let a0 = nz_at(&mut c.all, iv - 1);
    let a1 = nz_at(&mut c.all, iv);
    let fl = |i: i32| get(flags, i);
    let (f1, f2) = (fl(p - 1), fl(p - 2));
    let (r, f) = if f1 == 0.0 && f2 != 0.0 {
        (get(&c.peaks, p - 2) / get(&c.peaks, p - 3), f2)
    } else {
        (get(&c.peaks, p - 1) / get(&c.peaks, p - 2), f1)
    };
    let r = if f == 0.8 || f == 0.4 { r * 0.7 } else { r };
    let mut rp = spread(r);
    if (fl(p - 4) + fl(p - 5)) + (fl(p - 3) + fl(p - 2)) == 3.0 {
        rp *= 1.2;
    }
    let low = spread(get(&c.low, iv) / get(&c.low, iv - 1));
    let mid = spread(get(&c.mid, iv) / get(&c.mid, iv - 1));
    let lev = spread(get(&c.level, p - 1) / get(&c.level, p - 2));
    let all = spread(a1 / a0);
    (((low + rp) + (mid + lev)) + all) * weight
}

/// Beat steps tried by `VerseHookJudge_All` per state (1–8); later states step by 16.
fn all_step(state: i32) -> Option<i32> {
    Some(match state {
        1 => 32,
        2 => 64,
        3 => 16,
        4 => 96,
        5 => 48,
        6 => 80,
        7 => 128,
        8 => 112,
        _ => return None,
    })
}

/// `VerseHookJudge_All(count, starts, intro, bar, beat_flags, low_flags, mid_flags,
/// hihat_flags, peaks, low, mid, level, all)` @0x101bc8620: between consecutive starts, the
/// next phrase starts 16–128 beats apart where a band changes (beat flag, or a low/mid/hi-hat
/// bar flag); at 32 and 64 beats the half-way point competes by `RatioCalc_Verse`. Returns
/// the starts (absolute beats).
#[allow(clippy::too_many_arguments)]
pub fn verse_hook_judge_all(
    count: i32,
    starts: &[i32],
    bar: i32,
    flags: &[f64],
    low_flags: &[u8],
    mid_flags: &[u8],
    hihat: &[u8],
    curves: &mut RatioCurves,
) -> Option<Vec<i32>> {
    let from = (bar - 1).max(0) as usize;
    let mut c8: Vec<f64> = flags.get(from..).unwrap_or(&[]).to_vec();
    let c8_len = c8.len() as i32;
    let q = |n: usize| (n as f64).mul_add(0.25, 0.5) as i32;
    let m = (low_flags.len() as i32)
        .min(mid_flags.len() as i32)
        .min(hihat.len() as i32)
        .min(q(flags.len()))
        .min(q(curves.level.len()))
        .max(0) as usize;
    let u1: Vec<u8> = low_flags[..m].iter().flat_map(|&v| [v; 4]).collect();
    let u2: Vec<u8> = mid_flags[..m].iter().flat_map(|&v| [v; 4]).collect();
    let u3: Vec<u8> = hihat[..m].iter().flat_map(|&v| [v; 4]).collect();
    c8.resize(4 * m, 0.0);
    let mut list: Vec<i32> = starts.iter().map(|&v| v + 1 - bar).collect();
    list.push(4 * m as i32);
    let size = c8_len.max(0) as usize;
    let mut out = vec![0i32; size];
    let mut run: Vec<i32> = Vec::new();
    let half = f64::from(c8_len) * 0.5;
    let fc = |i: i32| get(&c8, i);
    let ub = |v: &[u8], i: i32| i >= 0 && v.get(i as usize).copied() == Some(1);
    let hit = |j: i32| fc(j) == 1.0 || fc(j - 1) == 1.0 || ub(&u1, j) || ub(&u2, j) || ub(&u3, j);
    let mut pos = 0i32;
    let mut prev = 1i32;
    if count >= 0 {
        for u in 0..=count as usize {
            let mut start = list.get(u).copied().unwrap_or(0);
            let next = list.get(u + 1).copied().unwrap_or(0);
            let idx = if out.first().copied().unwrap_or(0) != 0 { pos } else { 0 };
            if let Some(d) = out.get_mut(idx as usize) {
                *d = start;
            }
            let mut written = pos + 1;
            let mut state = 1;
            let mut last = prev;
            run.clear();
            loop {
                let cand = start + all_step(state).unwrap_or(state * 16);
                if next <= cand {
                    prev = if next - 4 <= last { last } else { cand };
                    break;
                }
                prev = last;
                if next - 4 <= last {
                    break;
                }
                let mut now = cand;
                let mut got = false;
                if hit(cand - 1) {
                    got = true;
                    if state == 2 {
                        if hit(start + 47) {
                            let r48 = ratio_calc_verse(start + 48, curves, &c8, 1.0);
                            let r64 = ratio_calc_verse(start + 64, curves, &c8, 1.02);
                            now = if r48 <= r64 { start + 64 } else { start + 48 };
                        } else {
                            now = start + 64;
                        }
                    } else if state == 1 {
                        if hit(start + 15) {
                            let r16 = ratio_calc_verse(start + 16, curves, &c8, 1.0);
                            let w = if f64::from(bar - 1 + start + 32) <= half { 1.25 } else { 1.23 };
                            let r32 = ratio_calc_verse(start + 32, curves, &c8, w);
                            now = if r16 <= r32 { start + 32 } else { start + 16 };
                        } else {
                            now = start + 32;
                        }
                    }
                    start = now;
                    run.push(start);
                }
                state = if got { 1 } else { state + 1 };
                last = now;
                prev = now;
                if state >= 12 {
                    break;
                }
            }
            if run.first().copied().unwrap_or(0) != 0 {
                for (t, &v) in run.iter().enumerate() {
                    if let Some(d) = out.get_mut(written as usize + t) {
                        *d = v;
                    }
                }
                written += run.len() as i32;
            }
            pos = written;
        }
    }
    out.resize(pos.max(0) as usize, 0);
    for v in &mut out {
        *v += bar - 1;
    }
    Some(out)
}

/// `CandidateAdjust(starts, bar, all_bars, low_bars)` @0x1010e9068: drops starts (but the
/// first) in the last 4 bars, where the full band is quiet on both beats before (≤ 0.32), or
/// medium (0.32–0.56) with a weak low band (≤ 300). Returns the remaining valid starts, or
/// `[0]`.
pub fn candidate_adjust(starts: &[i32], all: &[f64], low: &[f64]) -> Vec<i32> {
    let mut c = starts.to_vec();
    let na = all.len() as i32;
    let lim = 4 * na - 16;
    let n = c.len();
    for k in (1..n).rev() {
        if c[k] > lim {
            c[k] = 0;
        }
    }
    let expand = |v: &[f64]| -> Vec<f64> {
        let mut o = vec![0.0f64; 4 * v.len()];
        for b in 0..v.len().saturating_sub(1) {
            o[4 * b..4 * b + 4].fill(v[b]);
        }
        o
    };
    let a = expand(all);
    let mut l = vec![0.0f64; a.len()];
    for b in 0..all.len().saturating_sub(1) {
        l[4 * b..4 * b + 4].fill(low.get(b).copied().unwrap_or(0.0));
    }
    let la = a.len() as i32;
    let ll = l.len() as i32;
    let ok = |v: i32| 0 < v && v <= la && v <= ll;
    let at = |x: &[f64], i: i32| if i >= 0 { x.get(i as usize).copied().unwrap_or(0.0) } else { 0.0 };
    for k in (1..n).rev() {
        let v = c[k];
        if 0 < v && v <= la && at(&a, v - 1) <= 0.32 && at(&a, v - 2) <= 0.32 {
            c[k] = 0;
        }
    }
    let mid = |x: f64| (0.32..0.56).contains(&x);
    for k in (1..n).rev() {
        let v = c[k];
        if ok(v) && mid(at(&a, v - 1)) && mid(at(&a, v - 2)) && at(&l, v - 1) <= 300.0 && at(&l, v - 2) <= 300.0 {
            c[k] = 0;
        }
    }
    let out: Vec<i32> = c.into_iter().filter(|&v| ok(v)).collect();
    if out.is_empty() { vec![0] } else { out }
}

/// `RhythmPatternReject`'s outputs: the starts, the mean similarity, the similarities, the
/// repeat flags.
type Reject = (Vec<i32>, f64, Vec<f64>, Vec<u8>);

/// `RhythmPatternReject(starts, pattern, bar)` @0x100b85f88: for starts 32 beats after the
/// previous one, the rhythm-pattern similarity of the 80 steps around the start with the 80
/// steps 32 beats earlier; starts that repeat the pattern more than the average are dropped
/// (×1.02, or ×1.06 where the bar before also repeats). Returns (the starts, the mean
/// similarity, the similarities, the repeat flags).
pub fn rhythm_pattern_reject(starts: &[i32], pattern: &[u8]) -> Option<Reject> {
    let mut list = starts.to_vec();
    let n = list.len();
    let p = |i: i32| if i >= 0 { pattern.get(i as usize).copied() } else { None };
    let same = |a: i32, b: i32| p(a).is_some() && p(a) == p(b);
    let mut sims = Vec::new();
    let mut vals = Vec::new();
    let mut flags = Vec::new();
    for k in 1..n {
        let v = list[k];
        if (pattern.len() as f64).mul_add(0.25, -f64::from(v)) < 32.0 {
            break;
        }
        if v - list[k - 1] != 32 {
            continue;
        }
        vals.push(v);
        let b = 4 * v;
        let mut cnt = 0;
        for t in 0..40 {
            cnt += i32::from(same(b - 121 + t, b + 7 + t));
            cnt += i32::from(same(b - 57 + t, b + 71 + t));
        }
        sims.push(f64::from(cnt) * 0.0125);
        let mut flag = 0u8;
        if k != 1 && k != n - 1 {
            let c1 = (0..16).filter(|&j| same(b - 49 + j, b - 17 + j)).count();
            let c2 = (0..16).filter(|&j| same(b - 113 + j, b - 49 + j)).count();
            if c1 < c2 {
                flag = 1;
            }
        }
        flags.push(flag);
    }
    let mean = if sims.is_empty() { 0.0 } else { mat::sum(&sims) / sims.len() as f64 };
    let full_flags = {
        let mut f = vec![0u8; n];
        f[..flags.len()].copy_from_slice(&flags);
        f
    };
    if vals.is_empty() {
        return Some((list, mean, sims, full_flags));
    }
    for (u, &val) in vals.iter().enumerate() {
        let pos: Vec<i32> = (0..list.len()).filter(|&j| list[j] == val).map(|j| j as i32 + 1).collect();
        let regular = pos.iter().all(|&e| {
            let a = list.get((e - 1) as usize).copied().unwrap_or(0);
            let b = if e >= 2 { list.get((e - 2) as usize).copied().unwrap_or(0) } else { 0 };
            a - b == 32
        });
        if !regular {
            continue;
        }
        let drop = if u == 0 {
            full_flags[0] == 0 && mean < sims[0]
        } else {
            let w = if full_flags[u] != 0 { 1.06 } else { 1.02 };
            w * mean < sims[u]
        };
        if drop {
            for &e in &pos {
                let i = (e - 1) as usize;
                if i >= list.len() {
                    return None;
                }
                list.remove(i);
            }
        }
    }
    Some((list, mean, sims, full_flags))
}

/// `VerseHookJudge23Sub(a, b, c, bar, out)` @0x1005a95d8 on three per-bar curves of one block
/// (`b` the level, `a` and `c` its changes): the four highest bars of `b` (adjacent ones
/// merged unless the step is large) become phrase starts (`bar − 1 + p + 3`) where the level
/// rises above the block mean and the bars before are quiet. `out` is left as it is for blocks
/// under 10 bars.
pub fn verse_hook_judge23_sub(a: &[f64], b: &[f64], c: &[f64], bar: i32, out: &mut Vec<i32>) {
    let na = a.len() as i32;
    if na < 10 {
        return;
    }
    let m = na - 6;
    let mean = {
        let v = sum_ij(b, 3, na - 4) / f64::from(m);
        if v != 0.0 { v } else { EPS }
    };
    let win = |v: &[f64]| -> Vec<f64> { (3..3 + m).map(|i| get(v, i)).collect() };
    let (_, mx) = mat::max_idx(&win(a));
    let max_at = mx as i32 + 1;
    let mut w = win(b);
    let order = mat::sort_descend_idx(&mut w);
    let mut top: Vec<i32> = (0..4).map(|k| order.get(k).copied().unwrap_or(0)).collect();
    top.sort_by(|x, y| y.cmp(x));
    let mut keep = top.clone();
    let mut cnt = 0i32;
    let bv = |p: i32| get(b, p + 2);
    let cv = |p: i32| get(c, p + 2);
    let ratio = |p: i32, q: i32| if bv(q) == 0.0 { bv(p) * 333.3333333333333 } else { bv(p) / bv(q) };
    for k in 0..3 {
        let (p, q) = (top[k], top[k + 1]);
        let r = ratio(p, q);
        if p - q == 1 && cnt + 4 < m {
            if r < 1.7 && cv(p) <= 1.8 {
                keep[k] = 0;
                cnt += 1;
            }
            if 1.8 < cv(p) {
                keep[k + 1] = 0;
                cnt += 1;
            }
        }
    }
    let mut cand = order.clone();
    let k4 = 4.min(cand.len());
    cand[..k4].copy_from_slice(&keep[..k4]);
    let th = mean * 1.0573000000000001;
    let mut res = vec![0i32; (cnt + 4) as usize];
    let mut count = 0;
    for t in 0..(cnt + 4) as usize {
        let p = cand.get(t).copied().unwrap_or(0);
        if p == 0 {
            continue;
        }
        let s01 = sum_ij(b, p, p + 1);
        let s3 = sum_ij(b, p - 1, p + 1);
        let a3 = {
            let v = sum_ij(a, p - 1, p + 1) * 0.3333333333333333;
            if v != 0.0 { v } else { EPS }
        };
        let after = {
            let v = sum_ij(b, p + 3, p + 4) * 0.5;
            if v != 0.0 { v } else { EPS }
        };
        let after2 = {
            let v = sum_ij(b, p + 4, p + 5) * 0.5;
            if v != 0.0 { v } else { EPS }
        };
        let a1 = {
            let v = get(a, p + 1);
            if v != 0.0 { v } else { EPS }
        };
        let level = bv(p);
        if !(mean * 1.05 < level || max_at == p) {
            continue;
        }
        let av = get(a, p + 2);
        let q = av / a3;
        let jump = !(q <= mean * 1.09) || q < 0.9174311926605504 / mean;
        if !(jump || max_at == p) {
            continue;
        }
        let quiet = s01 * 0.5 < th && s3 * 0.3333333333333333 < th;
        if !(quiet || (max_at == p && 1.1 < av / a1)) {
            continue;
        }
        if 1.2 <= after {
            let steady = level / after <= mean && after2 >= 1.2 && level / after2 <= mean;
            if steady && (level <= 1.4 || 1.2 <= (get(b, p + 5) + get(b, p + 3)) * 0.5) {
                continue;
            }
        }
        res[t] = bar - 1 + p + 3;
        count += 1;
    }
    while res.last() == Some(&0) {
        res.pop();
    }
    if count == 0 {
        *out = vec![0];
        return;
    }
    res.sort();
    for _ in 0..3 {
        if res.first() == Some(&0) {
            res.remove(0);
        }
    }
    if res.first() == Some(&0) {
        res = vec![0];
    }
    *out = res;
}

/// `MatArrayInt::sort_val_and_delete0(true)`: ascending, then the leading zeros removed.
pub(super) fn sort_delete0(v: &mut Vec<i32>) {
    v.sort();
    if v.first() == Some(&0) {
        let k = v.iter().position(|&x| x != 0).unwrap_or(v.len());
        v.drain(..k);
    }
}

/// `VerseHookJudge23(peak, low, mid_bar, estimates, intro, bar, mood)` @0x1014412f4: the bar
/// numbers of the intro estimates (the intro bar itself dropped when it follows), then per
/// 16-bar window `VerseHookJudge23Sub` on the beat-peak, low-band and mid-band curves (the
/// beat-peak and mid-band results only for moods other than 1); bars one apart are thinned,
/// and of close bars whose combined change differs by 7 % the weaker is dropped (not bars 5,
/// 9 or the intro bar). Returns phrase starts as beats.
pub fn verse_hook_judge23(
    peak: &Band,
    low: &Band,
    mid: &Band,
    estimates: &[i32],
    intro: i32,
    bar: i32,
    mood: i32,
) -> Option<Vec<i32>> {
    let m = peak.norm.len().min(low.norm.len()).min(mid.norm.len()) as i32;
    let cut = |v: &[f64]| v[..m as usize].to_vec();
    let (pn, ps, pr) = (cut(&peak.norm), cut(&peak.spread), cut(&peak.raw));
    let (ln, ls, lr) = (cut(&low.norm), cut(&low.spread), cut(&low.raw));
    let (mn, ms, mr) = (cut(&mid.norm), cut(&mid.spread), cut(&mid.raw));
    let bar_of = |v: i32| f64::from(v - bar).mul_add(0.25, 1.5) as i32;
    let mut e: Vec<i32> = estimates.iter().map(|&v| {
        let b = bar_of(v);
        if b <= m { b } else { 0 }
    }).collect();
    e.sort();
    let mut d = e.clone();
    for k in 0..e.len().saturating_sub(1) {
        if e[k] == e[k + 1] {
            d[k] = 0;
        }
    }
    d.sort();
    let mut lead = d.clone();
    let mut kept = 4;
    if lead.first().is_some_and(|&v| v < 1) {
        kept = 0;
        for left in [3, 2, 1] {
            if lead.is_empty() {
                return None;
            }
            lead.remove(0);
            if lead.first().is_some_and(|&v| v > 0) {
                kept = left;
                break;
            }
        }
        if kept == 0 {
            if lead.is_empty() {
                return None;
            }
            lead.remove(0);
        }
    }
    if kept == 0 || lead.iter().sum::<i32>() == 0 {
        lead = vec![0];
    }
    let iv = bar_of(intro);
    if kept >= 2 && iv - lead[0] == 1 {
        lead.remove(0);
    }
    let mut out = lead;
    let last = out.last().copied().unwrap_or(0);
    let last = if last < 4 { 3 } else { last };
    let mut end = (last + 19).min(m);
    let mut start = if last - 2 <= end { last - 2 } else { m };
    let (mut r1, mut r2, mut r3) = (vec![0i32], vec![0i32], vec![0i32]);
    let mut sub = Vec::new();
    let span = m - last - 1;
    if span > -16 {
        let windows = mat::cdiv(span, 16) + 1;
        let mut off = start - 1;
        let mut sbar = start;
        for _ in 0..windows {
            let len = (end + 1 - start).max(0) as usize;
            let w = |v: &[f64]| -> Vec<f64> {
                (0..len).map(|i| get(v, off + i as i32)).collect()
            };
            verse_hook_judge23_sub(&w(&pn), &w(&ps), &w(&pr), sbar, &mut sub);
            r1.extend_from_slice(&sub);
            if mood == 1 {
                r1 = vec![0];
            }
            verse_hook_judge23_sub(&w(&ln), &w(&ls), &w(&lr), sbar, &mut sub);
            r2.extend_from_slice(&sub);
            verse_hook_judge23_sub(&w(&mn), &w(&ms), &w(&mr), sbar, &mut sub);
            r3.extend_from_slice(&sub);
            if mood == 1 {
                r3 = vec![0];
            }
            if m - end < 10 {
                break;
            }
            end += 16;
            if m <= end {
                end = m - 1;
            }
            off += 16;
            sbar += 16;
            start += 16;
        }
    }
    let mut all = r1;
    all.extend_from_slice(&r2);
    all.extend_from_slice(&r3);
    all.sort();
    let mut dd = all.clone();
    for k in 0..all.len().saturating_sub(1) {
        if all[k + 1] == all[k] {
            dd[k + 1] = 0;
        }
    }
    sort_delete0(&mut dd);
    out.extend_from_slice(&dd);
    let mut z = out.clone();
    let n = out.len();
    for k in 0..n.saturating_sub(1) {
        let (v, w) = (out[k], out[k + 1]);
        if v != 0 && v == w - 1 {
            // A run of two or three consecutive bars: every other one from the first goes.
            z[k] = 0;
        }
    }
    sort_delete0(&mut z);
    let mut list = z;
    if list.first() != Some(&1) {
        list.insert(0, 1);
    }
    // The combined change at each bar, and the thinning of close pairs.
    let n = list.len();
    let mut score = vec![0.0f64; n];
    let at = |v: &[f64], i: i32| get(v, i);
    for i in 0..n {
        let v = list[i];
        let s = (at(&ls, v - 1) + at(&ps, v - 1)).mul_add(1.05, at(&ms, v - 1) * 0.9);
        score[i] = if v > 2 {
            (at(&ls, v - 2) + s) * 0.25
        } else if v == 1 {
            0.0
        } else {
            s * 0.3333333333333333
        };
    }
    let mut keep = list.clone();
    if n > 2 {
        for k in 1..n - 1 {
            if score[k] == 0.0 {
                score[k] = EPS;
            }
            if score[k + 1] == 0.0 {
                score[k + 1] = EPS;
            }
            let (cur, next) = (score[k], score[k + 1]);
            let r = next / cur;
            let f = if r < 1.0 { cur / next } else { r };
            if f < 1.07 || list[k + 1] - list[k] > 3 {
                continue;
            }
            let (idx, weak) = if next > cur { (k, cur) } else { (k + 1, next) };
            let b = keep[idx];
            if b == 5 || b == 9 || b == iv {
                continue;
            }
            if weak < 1.6 {
                keep[idx] = 0;
            }
        }
    }
    let mut res: Vec<i32> = Vec::with_capacity(n);
    for (i, &v) in list.iter().enumerate() {
        if i == 0 || keep[i] != 0 {
            res.push(v);
        }
    }
    for v in &mut res {
        *v = bar - 4 + *v * 4;
    }
    let _ = &mut out;
    Some(res)
}

/// `MatArrayFloat::sort_val_and_delete0(true)` on a buffer: ascending, the leading zeros
/// moved out; the tail keeps its old values (the binary's memmove), `n` is the new length.
pub(super) fn sort_delete0_buf(v: &[f64]) -> (Vec<f64>, usize) {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let z = if s.first() == Some(&0.0) { s.iter().position(|&x| x != 0.0).unwrap_or(s.len()) } else { 0 };
    let n = s.len() - z;
    let mut buf = s.clone();
    buf.copy_within(z.., 0);
    (buf, n)
}

/// `ShortBreakDetection(level, mid, n, q, bar)` @0x101beec5c: beats (on the second half of
/// a bar) where both the low-band level and the mid band drop below their typical falls and
/// come back 2.5× within four beats. Returns `q` bar numbers (1-based; 0 = none).
pub fn short_break_detection(level: &[f64], mid: &[f64], n: i32, q: i32) -> Vec<i32> {
    let mut a = level.to_vec();
    let s = {
        let mx = mat::max_val(&a);
        if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 }
    };
    for v in &mut a {
        *v *= s;
    }
    let nn = n.max(0) as usize;
    a.resize(nn, 0.0);
    let mut b = mid.to_vec();
    b.resize(nn, 0.0);
    let (mut ra, mut rb) = (vec![0.0f64; nn], vec![0.0f64; nn]);
    let (mut da, mut db) = (vec![0.0f64; nn], vec![0.0f64; nn]);
    let (mut fall_a, mut fall_b) = (EPS, EPS);
    if n >= 6 {
        let (mut ua, mut ub) = (0, 0);
        for k in 0..(n - 5) as usize {
            for (x, r, d, up) in [(&mut a, &mut ra, &mut da, &mut ua), (&mut b, &mut rb, &mut db, &mut ub)] {
                if x[k + 1] == 0.0 {
                    x[k + 1] = EPS;
                }
                if x[k] == 0.0 {
                    x[k] = EPS;
                }
                let v = x[k + 1] / x[k];
                r[k + 1] = v;
                d[k + 1] = v;
                if 1.0 <= v {
                    d[k + 1] = 0.0;
                    *up += 1;
                }
            }
        }
        fall_a = if ua == 0 { EPS } else { mat::sum(&da) / f64::from(ua) };
        fall_b = if ub == 0 { EPS } else { mat::sum(&db) / f64::from(ub) };
    }
    let (sorted, _) = sort_delete0_buf(&ra);
    let mut th = fall_a * 0.65;
    let mut stop = nn;
    for (u, &v) in sorted.iter().enumerate().take(nn) {
        if th < v || v <= 0.0 {
            stop = u;
            break;
        }
    }
    if stop > 13 {
        th = sorted[12];
    }
    let qn = q.max(0) as usize;
    let mut hits = vec![0i32; qn];
    let mut cnt = 0usize;
    if n > 20 {
        for k in 0..(n - 20) as usize {
            let ok = ra[k + 16] < th
                && rb[k + 16] < fall_b * 0.95
                && ((0.5 < a[k + 15] && (0.2 < b[k + 15] || 0.2 < b[k + 14]))
                    || (0.2 < a[k + 15] && (0.5 < b[k + 15] || 0.5 < b[k + 14])));
            if ok {
                if cnt < qn {
                    hits[cnt] = k as i32 + 17;
                }
                cnt += 1;
            }
        }
    }
    for v in &mut hits {
        let r = mat::crem(*v, 4);
        if ((r - 1) as u32) < 2 {
            *v = 0;
        }
    }
    let at = |i: i32| if i >= 0 { a.get(i as usize).copied().unwrap_or(0.0) } else { 0.0 };
    for v in &mut hits {
        if *v != 0 {
            let base = at(*v - 1);
            let s = if base != 0.0 { 1.0 / base } else { 333.3333333333333 };
            if (0..4).all(|k| at(*v + k) * s <= 2.5) {
                *v = 0;
            }
        }
    }
    sort_delete0(&mut hits);
    if hits.is_empty() {
        return vec![0; qn];
    }
    hits.iter().map(|&v| mat::cdiv(v, 4) + if v & 3 != 0 { 2 } else { 1 }).collect()
}

/// `SnareFillinDet(high_peaks, n, q)` @0x1002ef650: per beat the sum of the high-band peaks
/// (`findpeaks_ex` on 32 eighths), and where that rises or falls well beyond its typical
/// change for one beat and then holds, a fill-in at that beat. Returns `q` beats (0 = none).
pub fn snare_fillin_det(hp: &[f64], q: i32) -> Option<Vec<i32>> {
    let qn = q.max(0) as usize;
    let mut out = vec![0i32; qn];
    let mut sums = vec![0.0f64; qn];
    if q > 4 {
        for k in 0..(q - 4) as usize {
            let w: Vec<f64> = (0..32).map(|i| hp.get(32 * k + i).copied().unwrap_or(0.0)).collect();
            let p = super::d_feat::findpeaks_ex(&w)?;
            sums[k] = mat::sum(&p.vals);
        }
    }
    let s = {
        let mx = mat::max_val(&sums);
        if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 }
    };
    let (mut up, mut down) = (0.00345, 0.00255);
    let mut ratio = vec![0.0f64; qn];
    let mut ups = vec![0.0f64; qn];
    let mut downs = vec![0.0f64; qn];
    if q >= 1 {
        for v in &mut sums {
            let x = *v * s;
            *v = if x != 0.0 { x } else { EPS };
        }
        if q >= 2 {
            for i in 1..qn {
                ratio[i] = if sums[i - 1] != 0.0 { sums[i] / sums[i - 1] } else { 0.0 };
            }
            let (mut nu, mut nd) = (0, 0);
            for i in 1..qn {
                let r = ratio[i];
                if 1.0 <= r || r == 0.0 {
                    if 1.0 <= r {
                        ups[i] = r;
                        nu += 1;
                    }
                } else {
                    downs[i] = r;
                    nd += 1;
                }
            }
            up = if nu == 0 { 0.00345 } else { (mat::sum(&ups) * 1.15) / f64::from(nu) };
            down = if nd == 0 { 0.00255 } else { (mat::sum(&downs) * 0.85) / f64::from(nd) };
        }
    }
    let k16 = mat::cdiv(q, 16);
    ups.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    if q > 15 && k16 <= ups.len() as i32 && k16 >= 1 && up <= ups[(k16 - 1) as usize] {
        up = ups[(k16 - 1) as usize];
    }
    let (asc, an) = sort_delete0_buf(&ups);
    if q < 16 || (an as i32) < k16 {
        if q < 6 {
            return Some(out);
        }
    } else if k16 >= 1 {
        let v = asc[(k16 - 1) as usize];
        if v <= down {
            down = v;
        }
    }
    let r = |i: usize| ratio.get(i).copied().unwrap_or(0.0);
    let sm = |i: usize| sums.get(i).copied().unwrap_or(0.0);
    let mut flag = false;
    let mut cnt = 0usize;
    let mut put = |out: &mut Vec<i32>, v: i32| {
        if cnt < out.len() {
            out[cnt] = v;
        }
        cnt += 1;
    };
    let mut u = 1usize;
    while (u as i32) != q - 4 && q - 4 > 1 {
        if flag {
            flag = false;
        } else {
            let (r1, r3, r2) = (r(u), r(u + 1), r(u + 2));
            let big = !(r1 <= up) || r1 < down;
            let steady = r2 < 1.15 && !(r2 <= 0.85);
            if big && steady {
                flag = false;
                if r1 < 1.0 && r3 > down {
                    let prev = sm(u - 1);
                    let ok = if r1 <= 0.5 { prev > 0.5 } else { prev > 0.3 };
                    if ok {
                        let v = if r3 * r1 < 0.8 { u as i32 + 1 } else { u as i32 + 2 };
                        put(&mut out, v);
                        flag = true;
                    }
                }
                if 1.0 < r1 && r3 < up {
                    let cur = sm(u);
                    let skip = if 2.0 <= r1 { cur <= 0.5 && !flag } else { !flag && cur <= 0.4 };
                    if !skip {
                        let v = if 0.8 < r3 * r1 { u as i32 + 1 } else { u as i32 + 2 };
                        put(&mut out, v);
                        flag = true;
                    }
                }
            }
        }
        u += 1;
    }
    Some(out)
}

/// The bar number (1-based, rounded) of a beat: `round((v − bar)·0.25) + 1`.
fn beat_bar(v: i32, bar: i32) -> i32 {
    let d = f64::from(v - bar) * 0.25;
    (if 0.0 <= d { 0.5 } else { -0.5 } + d) as i32 + 1
}

/// `MatArrayInt::delete_consecutive0_from_first`.
fn delete_leading0(v: &mut Vec<i32>) {
    let k = v.iter().position(|&x| x != 0).unwrap_or(v.len());
    v.drain(..k);
}

/// `VerseHookAdd_genre1(low_norm, low_spread, low_raw, level, mid, high, a, b, bar)`
/// @0x100b454f8 for mood 1: merges the two candidate lists (`a` from `VerseHookJudge_All`,
/// `b` from `VerseHookJudge23`) as bar numbers, moves `b`'s bars onto a neighbouring `a` bar
/// where the low band does not jump, adds short breaks and snare fill-ins, and thins bars that
/// are off the 4-bar grid. Returns phrase starts as beats.
#[allow(clippy::too_many_arguments)]
pub fn verse_hook_add_genre1(
    low_norm: &[f64],
    low_spread: &[f64],
    low_raw: &[f64],
    level: &[f64],
    mid: &[f64],
    high: &[f64],
    a: &[i32],
    b: &[i32],
    bar: i32,
) -> Option<Vec<i32>> {
    let n_a = a.len();
    let mut aa = a.to_vec();
    sort_delete0(&mut aa);
    let mut ab = vec![0i32; n_a];
    for (d, &v) in ab.iter_mut().zip(&aa) {
        *d = beat_bar(v, bar);
    }
    delete_leading0(&mut ab);
    let mut bb = b.to_vec();
    sort_delete0(&mut bb);
    for v in &mut bb {
        *v = beat_bar(*v, bar);
    }
    delete_leading0(&mut bb);
    let g = |v: &[f64], i: i32| get(v, i);
    if !bb.is_empty() && !ab.is_empty() {
        for k in 0..bb.len() {
            let mut flag = false;
            let mut cur = bb[k];
            for &x in &ab {
                if (cur - x).abs() != 1 {
                    continue;
                }
                if g(low_spread, cur - 1) < 1.5 || g(low_norm, cur - 1) < 0.5 {
                    flag = true;
                    bb[k] = x;
                    cur = x;
                } else if flag {
                } else if x - cur == 1 {
                    flag = false;
                    if g(low_raw, cur - 1) <= 0.5 && 1.8 <= g(low_raw, x - 1) {
                        bb[k] = x;
                        cur = x;
                    }
                } else {
                    flag = false;
                }
            }
        }
    }
    let (_, nz) = sort_delete0_buf(mid);
    let nz = nz as i32;
    let q = mat::cdiv(nz, 4);
    let brk = short_break_detection(level, mid, nz, q);
    bb.extend_from_slice(&brk);
    let fills = snare_fillin_det(high, q)?;
    let mut c: Vec<i32> = bb.clone();
    c.extend_from_slice(&fills);
    let n2 = c.len();
    c.sort();
    for i in 0..n2 {
        for j in 0..n2 {
            if i != j && c[i] == c[j] {
                c[j] = 0;
            }
        }
    }
    sort_delete0(&mut c);
    for &x in &ab {
        for v in c.iter_mut() {
            if x == *v {
                *v = 0;
            }
        }
    }
    let mut list = ab.clone();
    list.extend_from_slice(&c);
    sort_delete0(&mut list);
    sort_delete0(&mut c);
    let n = list.len();
    let mut mark = vec![0u8; n];
    if n > 1 && !c.is_empty() {
        for i in 1..n {
            if c.contains(&list[i]) {
                mark[i] = 1;
            }
        }
    }
    let mut out = list.clone();
    if n > 1 {
        let mut del = 0usize;
        for i in 1..n {
            let j = i - del;
            let v = out[j];
            let d1 = v - out[j - 1];
            let r1 = mat::crem(d1, 4);
            let (mut d2, mut r2) = (0, 0);
            if i < n - 1 {
                d2 = out.get(j + 1).copied().unwrap_or(0) - v;
                r2 = mat::crem(d2, 4);
            }
            if mark[i] != 1 || r1 == 0 || i == n - 1 {
                continue;
            }
            let act = if d1 == 1 || d2 == 1 {
                2
            } else {
                let x = g(low_raw, v - 1);
                let first = if 0.83 < x { 1 } else { 2 };
                let i1 = if d1 >= 5 || x <= 0.83 { first } else { 2 };
                let i29 = if d1 == 2 || r1 == 2 { i1 } else { 0 };
                let i1b = if d2 == 2 { i29 } else { 0 };
                if x <= 0.83 { i1b } else { 2 }
            };
            let shift = d1 >= 4
                && r1 == 3
                && d2 >= 4
                && r2 == 1
                && !(0.83 < g(low_raw, v - 1))
                && !(g(low_raw, v) * g(low_raw, v - 1) < 0.9);
            if shift {
                out[j] = v + 1;
            } else if act == 2 {
                out.remove(j);
                del += 1;
            }
        }
    }
    if let Some(&last) = out.last()
        && low_norm.len() as i32 - 4 < last {
            out.pop();
        }
    Some(out.iter().map(|&v| bar - 4 + v * 4).collect())
}

/// `MatArrayInt::sum(i, j)`: 0 for an invalid range.
fn isum(v: &[i32], i: i32, j: i32) -> i32 {
    let n = v.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0;
    }
    v[i as usize..=j as usize].iter().sum()
}

/// `SouonChangeDet(level)` @0x1005dffc8: bars after which the level (bar means) jumps by
/// 1.8× and then holds (the next two bars change less than 1.11×). Returns the bars.
pub fn souon_change_det(level: &[f64]) -> Vec<i32> {
    let n = level.len() as i32;
    let nb = mat::cdiv(n, 4).max(0) as usize;
    let mut mean = vec![0.0f64; nb];
    let mut spread = vec![0.0f64; nb];
    if n >= 8 {
        let count = (nb as i32 - 1).max(1) as usize;
        for k in 0..count.min(nb) {
            mean[k] = sum_ij(level, 4 * k as i32, 4 * k as i32 + 3) * 0.25;
        }
        if n >= 12 {
            let c = (nb as i32 - 1).max(2) - 1;
            for k in 1..=c as usize {
                if k >= nb {
                    break;
                }
                if mean[k - 1] == 0.0 {
                    mean[k - 1] = EPS;
                }
                if mean[k] == 0.0 {
                    mean[k] = EPS;
                }
                let r = mean[k] / mean[k - 1];
                spread[k] = if r < 1.0 { 1.0 / r } else { r };
            }
        }
    }
    let mut out = vec![0i32; nb];
    if n > 19 {
        let lim = (nb as i32).max(5);
        let mut cnt = 0;
        for k in 0..(lim - 4) {
            let r = get(&spread, k + 1);
            if r != 0.0 && !(r < 1.8) && sum_ij(&spread, k + 3, k + 4) * 0.5 <= 1.11 {
                if let Some(d) = out.get_mut(cnt) {
                    *d = k + 2;
                }
                cnt += 1;
            }
        }
    }
    out
}

/// `VocalInDetection(uta)` @0x1004ebda0: vocal entries — a voiced line after at least eight
/// unvoiced ones (two of the next three voiced), and, when the track is mostly unvoiced, a
/// strong line after a short gap where the bar level rises. Returns bar numbers.
pub fn vocal_in_detection(uta: &[f64]) -> Vec<i32> {
    let n = uta.len();
    let mx = mat::max_val(uta);
    let s = if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 };
    let a: Vec<f64> = uta.iter().map(|&v| v * s).collect();
    let nb = mat::cdiv(n as i32, 4).max(0) as usize;
    let mut bars = vec![0.0f64; nb];
    let mut ratio = vec![0.0f64; nb];
    if n > 7 {
        let count = (nb as i32 - 1).max(1) as usize;
        for k in 0..count.min(nb) {
            bars[k] = sum_ij(&a, 4 * k as i32, 4 * k as i32 + 3);
        }
        if n > 11 {
            let c = (nb as i32 - 1).max(2) - 1;
            for k in 1..=c as usize {
                if k >= nb {
                    break;
                }
                ratio[k] = if bars[k - 1] == 0.0 { 0.0 } else { bars[k] / bars[k - 1] };
            }
        }
    }
    let mut on = vec![0i32; n];
    if n > 4 {
        for i in 0..n - 4 {
            if 0.0 < a[i] {
                on[i] = 1;
            }
        }
    }
    let sparse = if n == 4 {
        true
    } else {
        let share = f64::from(on.iter().sum::<i32>()) / f64::from(n as i32 - 4);
        if share < 0.8 { !(0.6..0.8).contains(&share) } else { false }
    };
    let mut out = vec![0i32; n];
    let mut cnt = 0usize;
    let mut put = |out: &mut Vec<i32>, v: i32| {
        if cnt < out.len() {
            out[cnt] = v;
        }
        cnt += 1;
    };
    if n >= 5 {
        let mut armed = false;
        let mut run = 0i32;
        for i in 0..n - 4 {
            let v = on[i];
            let long = 6 < run;
            if v == 0 {
                run += 1;
            }
            armed = (v == 0 && long) || armed;
            let next;
            if v == 1 && armed {
                if isum(&on, i as i32, i as i32 + 2) >= 2 {
                    put(&mut out, i as i32 + 1);
                }
                armed = false;
                next = 0;
            } else {
                next = if armed { run } else { 0 };
            }
            run = if on[i] != 1 { run } else { next };
        }
    }
    if sparse && n > 8 {
        let end = n - 8;
        let (mut i, mut run, mut armed, mut pz) = (0usize, 0i32, false, false);
        loop {
            let v = on[i];
            if v == 0 {
                let r20 = run + 1;
                if run > 6 {
                    run = r20;
                    armed = false;
                    pz = true;
                } else {
                    armed = run > 1 || armed;
                    run = r20;
                }
            } else {
                let i2 = if pz { 0 } else { run };
                let b6 = v != 1;
                let r20 = if b6 { run } else { i2 };
                pz = b6 && pz;
                let b1 = b6 && armed;
                if b6 || !armed {
                    let i2b = if b1 { r20 } else { 0 };
                    run = if v == 1 && !pz { i2b } else { r20 };
                    armed = b1;
                } else {
                    let q = i >> 2;
                    if isum(&on, i as i32, i as i32 + 2) > 2
                        && 0.7 <= a[i]
                        && (1.8 <= get(&ratio, q as i32) || 3.0 <= get(&ratio, q as i32 + 1))
                    {
                        put(&mut out, i as i32 + 1);
                    }
                    run = 0;
                    armed = false;
                    pz = false;
                }
            }
            i += 1;
            if i == end {
                break;
            }
        }
    }
    sort_delete0(&mut out);
    for v in &mut out {
        *v = mat::cdiv(*v, 4) + 1;
    }
    out
}

/// `CandidatesAdjustment(starts, candidates, bar, q)` @0x10019d064: the starts as bar
/// numbers, plus the candidates that are neither consecutive nor within a bar of a start,
/// without bars in the last four; returned as beats.
pub fn candidates_adjustment(starts: &[i32], cands: &[i32], bar: i32, q: i32) -> Vec<i32> {
    let mut c = cands.to_vec();
    for k in 0..cands.len().saturating_sub(1) {
        if cands[k + 1] == cands[k] + 1 {
            c[k] = 0;
        }
    }
    sort_delete0(&mut c);
    let l: Vec<i32> = starts.iter().map(|&v| mat::cdiv(v - bar, 4) + 1).collect();
    if l.len() > 1 && !c.is_empty() {
        for &x in &l[1..] {
            for v in c.iter_mut() {
                if *v == x || *v == x - 1 || *v == x + 1 {
                    *v = 0;
                }
            }
        }
    }
    let mut out = l;
    out.extend_from_slice(&c);
    sort_delete0(&mut out);
    while out.last().is_some_and(|&v| v > q - 4) {
        out.pop();
    }
    out.iter().map(|&v| bar - 4 + v * 4).collect()
}

/// `VerseHookAdd_genre23(level, mid, high, uta, level_b, starts, bar)` @0x1013f1fbc for moods
/// 2/3: short breaks, level jumps, snare fill-ins and vocal entries as candidates, merged
/// into the starts by `CandidatesAdjustment`.
#[allow(clippy::too_many_arguments)]
pub fn verse_hook_add_genre23(
    level: &[f64],
    mid: &[f64],
    high: &[f64],
    uta: &[f64],
    level_b: &[f64],
    starts: &[i32],
    bar: i32,
) -> Option<Vec<i32>> {
    let (_, nz) = sort_delete0_buf(mid);
    let nz = nz as i32;
    let q = mat::cdiv(nz, 4);
    let mut all = vec![0i32; q.max(0) as usize];
    let brk = short_break_detection(level, mid, nz, q);
    all.extend_from_slice(&brk);
    let jumps = souon_change_det(level_b);
    all.extend_from_slice(&jumps);
    let fills = snare_fillin_det(high, q)?;
    all.extend_from_slice(&fills);
    let vocal = vocal_in_detection(uta);
    let mut out = all;
    out.extend_from_slice(&vocal);
    let n2 = out.len();
    let mut c = out;
    c.sort();
    for i in 0..n2 {
        for j in 0..n2 {
            if i != j && c[i] == c[j] {
                c[j] = 0;
            }
        }
    }
    sort_delete0(&mut c);
    Some(candidates_adjustment(starts, &c, bar, q))
}

/// Phrase labels: 10 intro, 30 "verse/up", 40 hook (chorus), 50 "down", 60 outro.
pub const L_INTRO: i32 = 10;
pub const L_UP: i32 = 30;
pub const L_HOOK: i32 = 40;
pub const L_DOWN: i32 = 50;
pub const L_OUTRO: i32 = 60;

/// Values scaled by `1/max` (333.3 when the maximum is 0), zeros replaced by 0.003.
fn norm_nz(v: &[f64], n: usize) -> Vec<f64> {
    let mx = mat::max_val(v);
    let s = if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 };
    (0..n)
        .map(|i| {
            let x = get(v, i as i32) * s;
            if x != 0.0 { x } else { EPS }
        })
        .collect()
}

/// `SongStructureJudge(blocks, level, peaks)` @0x10025dcf0: labels the blocks from their
/// mean level and beat peak (both normalised in place where zero): hooks (40) where the level
/// jumps or is high and steady, then up (30) / down (50) blocks around them, intro (10) at the
/// start and outro (60) at the end. Returns (labels, 1 unless the level swings more than twice
/// by 0.3 between local extrema).
pub fn song_structure_judge(blocks: &[i32], level: &mut [f64], peaks: &mut [f64]) -> (Vec<i32>, i32) {
    let n = blocks.len();
    if n == 2 {
        return (vec![L_INTRO, L_OUTRO], 1);
    }
    if n == 1 {
        return (vec![L_INTRO], 1);
    }
    let n2 = level.len();
    let b8 = norm_nz(level, n2);
    let d8 = norm_nz(peaks, n2);
    let mut f8 = vec![0.0f64; n2];
    for i in 1..n2 {
        if level[i - 1] == 0.0 {
            level[i - 1] = EPS;
        }
        if level[i] == 0.0 {
            level[i] = EPS;
        }
        f8[i] = level[i] / level[i - 1];
    }
    let mut flag = 1;
    // Local extrema of the normalised level (blocks under 16 beats only when loud enough).
    let mut ext = vec![b8.get(1).copied().unwrap_or(0.0)];
    if n >= 4 {
        let mut rising = b8[1] < b8[2];
        let mean = if b8.is_empty() { 0.0 } else { mat::sum(&b8) / b8.len() as f64 };
        for k in 0..=n - 4 {
            let long = blocks[k + 3] - blocks[k + 2] > 15;
            if !long && b8[k + 2] <= mean * 0.65 {
                continue;
            }
            let (d0, d1) = (b8[k + 2], b8[k + 3]);
            if rising {
                if d0 > d1 {
                    rising = false;
                    ext.push(d0);
                } else {
                    rising = true;
                }
            } else if d0 < d1 {
                rising = true;
                ext.push(d0);
            } else {
                rising = false;
            }
        }
    }
    if ext.len() >= 4 {
        let swings = ext.windows(2).filter(|w| (w[0] - w[1]).abs() >= 0.3).count();
        if swings > 2 {
            flag = 0;
        }
    }
    let mut out = vec![0i32; n];
    out[0] = L_INTRO;
    if n2 >= 1 {
        out[n2 - 1] = L_OUTRO;
    }
    let mut r178 = vec![0.0f64; n2];
    for i in 1..n2 {
        if peaks[i - 1] == 0.0 {
            peaks[i - 1] = EPS;
        }
        r178[i] = peaks[i] / peaks[i - 1];
    }
    let mean2 = if level.is_empty() { 0.0 } else { mat::sum(level) / level.len() as f64 };
    let s = if mean2 == 0.0 { 333.3333333333333 } else { 1.0 / mean2 };
    let mut r198 = vec![0.0f64; n2];
    let mut sum198 = 0.0;
    if n2 >= 3 {
        for k in 1..=n2 - 2 {
            let v = level[k];
            r198[k] = if v != 0.0 && 0.84 <= d8[k] {
                if level[k - 1] == 0.0 {
                    level[k - 1] = EPS;
                }
                if level[k + 1] == 0.0 {
                    level[k + 1] = EPS;
                }
                (v / level[k + 1] + v / level[k - 1]) * v * s
            } else {
                1.0
            };
        }
        sum198 = mat::sum(&r198[1..n2 - 1]);
    }
    let avg = sum198 / (n2 as f64 - 2.0);
    let s2 = if avg != 0.0 { 1.0 / avg } else { 333.3333333333333 };
    let mut r1b8 = vec![0.0f64; n2];
    if n2 >= 3 {
        for k in 1..n2 - 1 {
            r1b8[k] = r198[k] * s2;
        }
    }
    if n2 >= 2 {
        for k in 1..n2 - 1 {
            if out[k - 1] == L_HOOK {
                if 0.7 < d8[k] && 0.7 < f8[k] && f8[k] < 1.9 {
                    out[k] = L_HOOK;
                }
            } else {
                let r = r178[k];
                if (r > 4.0 && d8[k] > 0.8 && level[k] > 0.3)
                    || (r > 2.0 && d8[k] > 0.5 && level[k] > 0.75)
                    || (r > 1.2 && d8[k] > 0.65 && level[k] > 0.45)
                {
                    out[k] = L_HOOK;
                }
                if out[k] == 0 {
                    let lim = if r <= 1.5 { 1.25 } else { 1.0 };
                    if 0.8 < d8[k] && lim < r1b8[k] {
                        out[k] = L_HOOK;
                    }
                }
            }
        }
    }
    // Weak hooks (relative to the other hooks) go, and of two hooks in a row the first when
    // the second rises sharply.
    let m = n2.saturating_sub(1);
    let mut hp = vec![0.0f64; m];
    let mut hl = vec![0.0f64; m];
    for i in 0..m {
        if out[i] == L_HOOK {
            hp[i] = peaks[i];
            hl[i] = level[i];
        }
    }
    let inv = |v: &[f64]| {
        let mx = mat::max_val(v);
        if mx != 0.0 { 1.0 / mx } else { 333.3333333333333 }
    };
    let (s_p, s_l) = (inv(&hp), inv(&hl));
    for i in 0..m {
        if out[i] == L_HOOK {
            hp[i] *= s_p;
            hl[i] *= s_l;
        }
    }
    for i in 0..m {
        if out[i] == L_HOOK && ((hp[i] < 0.48 && r198[i] < 1.1) || (hl[i] < 0.48 && r198[i] < 1.1)) {
            out[i] = 0;
        }
    }
    for i in 0..m {
        if out[i] == L_HOOK && out[i + 1] == L_HOOK && 1.4 < f8[i + 1] {
            out[i] = 0;
        }
    }
    let has_hook = if n2 < 3 {
        false
    } else if out[1] == L_HOOK {
        true
    } else {
        let mut k = 2;
        while k != n2 - 1 && out[k] != L_HOOK {
            k += 1;
        }
        k < n2 - 1
    };
    if n2 >= 3 {
        if !has_hook {
            // The (up to four) largest peak rises become hooks.
            let mut sorted: Vec<f64> = r178[1..n2 - 1].to_vec();
            sorted.resize(m, 0.0);
            sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            while sorted.last() == Some(&0.0) {
                sorted.pop();
            }
            let (mut j, mut cnt) = (0usize, 0);
            for k in 1..n2 - 1 {
                if r178[k] == sorted.get(j).copied().unwrap_or(f64::NAN) {
                    if 0.84 < d8[k] {
                        out[k] = L_HOOK;
                        if cnt > 2 {
                            break;
                        }
                        cnt += 1;
                    }
                    j += 1;
                }
            }
        }
        for k in 1..n2 - 1 {
            if out[k] == 0 && out[k + 1] == L_HOOK {
                out[k] = L_UP;
            }
        }
        for k in 1..n2 - 1 {
            if out[k] == 0 && out[k - 1] == L_HOOK {
                out[k] = L_DOWN;
            }
        }
        for k in 1..n2 - 1 {
            if out[k] == 0 && out[k - 1] == L_INTRO && b8[k] < 0.5 && f8[k] < 1.1 {
                out[k] = L_INTRO;
            }
        }
        for k in 1..n2 - 1 {
            if out[k] == 0
                && ((1.0 < f8[k] && out[k + 1] != L_OUTRO)
                    || (blocks[k + 1] - blocks[k] < 32 && out[k - 1] == L_UP && out[k + 1] == L_UP))
            {
                out[k] = L_UP;
            }
        }
        for k in 1..n2 - 1 {
            if out[k] == 0 && (out[k + 1] == L_OUTRO || f8[k] < 1.0) {
                out[k] = L_DOWN;
            }
        }
    }
    if n2 >= 2 && out[n2 - 2] == 0 {
        out[n2 - 2] = L_DOWN;
        if n2 > 3 {
            let mut k = n2 - 1;
            for _ in 0..n2 - 3 {
                if out[k] != L_UP {
                    break;
                }
                out[k] = L_DOWN;
                k -= 1;
            }
        }
    }
    if n2 >= 3 {
        let hooks = (1..n2 - 1).filter(|&k| out[k] == L_HOOK).count();
        for k in 1..n2 - 1 {
            let (prev, next, d, b) = (out[k - 1], out[k + 1], d8[k], b8[k]);
            let mut c = out[k];
            if c == 0 {
                if d <= 0.75 {
                    out[k] = if prev != L_INTRO {
                        if next == L_DOWN || next == L_OUTRO { L_DOWN } else { prev }
                    } else {
                        L_UP
                    };
                    continue;
                }
                if 0.7 < b && hooks < 4 {
                    out[k] = L_HOOK;
                    continue;
                }
                if b <= 0.7 || hooks < 4 {
                    out[k] = if prev != L_INTRO {
                        if next == L_DOWN || next == L_OUTRO { L_DOWN } else { prev }
                    } else {
                        L_UP
                    };
                    continue;
                }
                if next == L_OUTRO || next == L_DOWN {
                    out[k] = L_DOWN;
                    c = L_DOWN;
                } else {
                    out[k] = L_UP;
                    continue;
                }
            }
            if c == L_DOWN && prev == L_UP && 0.75 < d {
                if 0.7 < b && hooks < 4 {
                    out[k] = L_HOOK;
                } else if !(b <= 0.7 || hooks < 4) {
                    out[k] = if next != L_OUTRO && next != L_DOWN { L_UP } else { L_DOWN };
                }
            }
        }
    }
    for k in 0..m {
        if out[k] == L_DOWN && out[k + 1] != L_OUTRO && 0.9 < d8[k] && 0.9 < r178[k] {
            out[k] = L_UP;
        }
    }
    (out, flag)
}

/// `Beat4Ana(blocks, k, bar_level, beat_peaks)` @0x101c65a50: whether block `k` keeps a steady
/// four-beat feel: the loudest beat of each bar stays in place (or moves with similar level)
/// and the bar peaks change little; 0 for quiet blocks.
pub fn beat4_ana(blocks: &[i32], k: i32, level: &[f64], peaks: &[f64]) -> i32 {
    let (a, b) = (blocks[(k - 1) as usize], blocks[k as usize]);
    let mean = if b - a - 1 < 0 { 0.0 } else { sum_ij(level, a - 1, b - 2) / f64::from(b - a) };
    let len = (b - a) * 4;
    let mut u3 = len - 1;
    if u3 > 48 && blocks.len() as i32 - 1 != k {
        u3 = len - 33;
    }
    let nb = mat::cdiv(u3, 4).max(0) as usize;
    let mut out = 0;
    if u3 <= 3 {
        return out;
    }
    let mut pos = vec![0i32; nb.max(1)];
    let mut ratio = vec![0.0f64; nb];
    let mut tops = vec![0.0f64; nb];
    if u3 > 7 {
        let cnt = (nb as i32).max(2) - 1;
        for t in 0..cnt as usize {
            let start = 4 * a + 4 * t as i32;
            let w: Vec<f64> = (0..4).map(|i| get(peaks, start + i)).collect();
            let (mx, at) = mat::max_idx(&w);
            let idx = at as i32 + 1;
            let v = if mx != 0.0 { mx } else { EPS };
            let prev = pos.get(t).copied().unwrap_or(0);
            let d = (idx - prev).abs();
            if let Some(p) = pos.get_mut(t + 1) {
                *p = idx;
            }
            if let Some(x) = tops.get_mut(t + 1) {
                *x = mx;
            }
            if let Some(r) = ratio.get_mut(t) {
                if idx == prev {
                    *r = 1.0;
                } else if d & 0x7fff_fffd == 1 {
                    *r = get(peaks, 4 * a - 1 + 4 * t as i32 + prev) / v;
                }
            }
        }
    }
    let avg = if ratio.is_empty() { 0.0 } else { mat::sum(&ratio) / ratio.len() as f64 };
    if ratio.is_empty() || avg <= 0.8 {
        let u1 = nb as i32 - 4;
        let size = if u3 > 15 { u1.max(0) as usize } else { 0 };
        let mut d8 = vec![0.0f64; size];
        if u3 > 27 {
            let cnt = u1.max(3) - 2;
            for t in 0..cnt as usize {
                let (i1, i0) = (t + 2, t + 1);
                if tops.get(i1) == Some(&0.0) {
                    tops[i1] = EPS;
                }
                if tops.get(i0) == Some(&0.0) {
                    tops[i0] = EPS;
                }
                let (x1, x0) = (get(&tops, i1 as i32), get(&tops, i0 as i32));
                let r = if x1 < x0 { x0 / x1 } else { x1 / x0 };
                if let Some(d) = d8.get_mut(i1) {
                    *d = r;
                }
            }
        }
        let n = d8.len() as i32;
        if n > 2 && sum_ij(&d8, 2, n - 1) / f64::from(n - 2) < 1.12 {
            out = 1;
        }
    } else {
        out = 1;
    }
    if mean < 0.22 {
        out = 0;
    }
    out
}

/// `OutroKickoutAna(blocks, labels, pattern)` @0x100fc3dc4: for outro blocks, the last bar
/// with any rhythm-pattern hit (0 when the hits reach the end or stop more than 12 bars early).
pub fn outro_kickout_ana(blocks: &[i32], labels: &[i32], pattern: &[u8]) -> Vec<i32> {
    let mut out = vec![0i32; blocks.len()];
    for u in 0..blocks.len() {
        if labels.get(u) != Some(&L_OUTRO) {
            continue;
        }
        let b = blocks[u];
        let span = mat::cdiv(pattern.len() as i32, 4) - b;
        let n1 = span + 1;
        let mut last = n1;
        if span >= 0 {
            let mut on = vec![0u8; n1 as usize];
            for t in 0..n1 {
                let i0 = 4 * b - 3 + 4 * t - 1;
                let s: u32 = (0..4)
                    .map(|i| if i0 + i >= 0 { u32::from(pattern.get((i0 + i) as usize).copied().unwrap_or(0)) } else { 0 })
                    .sum();
                if s != 0 {
                    on[t as usize] = 1;
                }
            }
            let mut seen = 0;
            for j in (1..=n1).rev() {
                let first = seen == 0;
                if on[(j - 1) as usize] == 1 {
                    seen += 1;
                    if first {
                        last = j;
                    }
                }
            }
        }
        out[u] = if last <= span - 12 || last == n1 { 0 } else { last + b };
    }
    out
}

/// `FillinSnareAna(blocks, k, level)` @0x1007e20c4: a fill-in in the last 8 or 12 beats of
/// block `k` where the 4-beat level means rise by 25 % at the end. Returns (start, end) or 0s.
pub fn fillin_snare_ana(blocks: &[i32], k: i32, level: &[f64]) -> (i32, i32) {
    let b = blocks[k as usize];
    if b - blocks[(k - 1) as usize] < 16 {
        return (0, 0);
    }
    let m: Vec<f64> = (0..4)
        .map(|j| {
            let s = b - 17 + 4 * j;
            let w: Vec<f64> = (0..4).map(|i| get(level, s + i)).collect();
            mat::sum(&w) / 4.0
        })
        .collect();
    let len = if m[0] * 0.5 <= m[1] {
        if m[1] * 0.5 <= m[2] || m[3] <= m[2] * 1.25 {
            return (0, 0);
        }
        12
    } else if m[2] <= m[1] * 1.25 {
        if m[3] <= m[2] * 1.25 {
            return (0, 0);
        }
        12
    } else {
        8
    };
    (b - 16 + len, b - 1)
}

/// `FillinCutAna(blocks, k, level, x)` @0x100461680: a break (cut) in the last 16 beats of
/// block `k`: the 4-beat level means drop by half (start 12, 8 or 4 beats before the end), or
/// after a steady stretch of `x` one of the last beats drops below 0.66× or jumps above 2×.
/// Returns (start, end) or 0s.
pub fn fillin_cut_ana(blocks: &[i32], k: i32, level: &[f64], x: &[f64]) -> (i32, i32) {
    let mut x = x.to_vec();
    let b = blocks[k as usize];
    if b - blocks[(k - 1) as usize] < 16 {
        return (0, 0);
    }
    let m: Vec<f64> = (0..4).map(|j| sum_ij(level, b - 17 + 4 * j, b - 14 + 4 * j) * 0.25).collect();
    let end = b - 1;
    if !(m[0] * 0.5 <= m[1]) {
        return (b - 12, end);
    }
    if m[2] < m[1] * 0.5 {
        return (b - 8, end);
    }
    if !(m[2] * 0.5 <= m[3]) {
        return (b - 4, end);
    }
    // A steady stretch from b − 13 to b − 6.
    for p in (b - 12)..=(b - 6) {
        if x[(p - 1) as usize] == 0.0 {
            x[(p - 1) as usize] = EPS;
        }
        let r = x[p as usize] / x[(p - 1) as usize];
        if !(0.7..=1.5).contains(&r) {
            return (0, 0);
        }
    }
    let v = |i: i32| x[i as usize];
    let s = |i: i32, j: i32| sum_ij(&x, i, j);
    let fill = |start: i32| (start, end);
    // Position b − 5 (fill from b − 4).
    let (p6, p5, p4) = (v(b - 6), v(b - 5), v(b - 4));
    if !(p6 * 0.66 <= p5) {
        let sm = s(b - 9, b - 6);
        if !(sm * 0.165 <= p5) && !(p5 * 0.8 * p6 <= p4 * p5) {
            return fill(b - 4);
        }
    }
    if !(p5 <= p6 + p6 || p5 <= s(b - 9, b - 6) * 0.5 || p4 * p5 <= p5 * (p6 + p6)) {
        return fill(b - 4);
    }
    // Position b − 4 (fill from b − 3).
    let p3 = v(b - 3);
    if p4 < p5 * 0.66 {
        let sm = s(b - 8, b - 5);
        if !(sm * 0.165 <= p4) && p3 * p4 < p4 * 0.8 * p5 {
            return fill(b - 3);
        }
    }
    if p5 + p5 < p4 && s(b - 8, b - 5) * 0.5 < p4 && p4 * (p5 + p5) < p3 * p4 {
        return fill(b - 3);
    }
    // Position b − 3 (fill from b − 2).
    let p2 = v(b - 2);
    if p3 < p4 * 0.66 {
        let sm = s(b - 7, b - 4);
        if !(sm * 0.165 <= p3) && p2 * p3 < p3 * 0.8 * p4 {
            return fill(b - 2);
        }
    }
    if !(p3 <= p4 + p4 || p3 <= s(b - 7, b - 4) * 0.5 || p2 * p3 <= p3 * (p4 + p4)) {
        return fill(b - 2);
    }
    // Position b − 2 (fill from b − 1).
    let p1 = v(b - 1);
    if p2 < p3 * 0.66 {
        let sm = s(b - 6, b - 3);
        if !(sm * 0.165 <= p2) && p1 * p2 < p2 * 0.8 * p3 {
            return fill(b - 1);
        }
    }
    if !(p2 <= p3 + p3 || p2 <= s(b - 6, b - 3) * 0.5 || p1 * p2 <= p2 * (p3 + p3)) {
        return fill(b - 1);
    }
    (0, 0)
}

/// `FillinSnareAnaEX(blocks, high_peaks, n, k)` @0x100308cb8: per bar, a regular run of
/// high-band peaks (spacing 2–4 eighths, similar heights) as in a snare roll; a roll in the
/// last bar before block `k` after a bar without one ends a fill at `blocks[k] − 2`.
pub fn fillin_snare_ana_ex(blocks: &[i32], hp: &[f64], n: i32, k: i32) -> Option<i32> {
    let nb = mat::cdiv(n, 4).max(0) as usize;
    let mut rolls = vec![0i32; nb];
    if n >= 20 {
        let cnt = (nb as i32 - 4).max(1) as usize;
        for u in 0..cnt {
            let w: Vec<f64> = (0..16).map(|i| hp.get(32 * u + 16 + i).copied().unwrap_or(0.0)).collect();
            let p = super::d_feat::findpeaks_ex(&w)?;
            let np = p.vals.len();
            let mut val = vec![0.0f64; 8];
            let mut at = [0i32; 8];
            let mut gap = [0i32; 8];
            let mut run = 0usize;
            let mut res = 0;
            if np >= 3 {
                for j in 0..np - 1 {
                    let d = p.locs[j + 1] - p.locs[j];
                    if ((d - 2) as u32) < 3 {
                        let r = p.vals[j + 1] / p.vals[j];
                        if 0.7 < r && r < 1.43 {
                            if run < 8 {
                                gap[run] = d;
                                val[run] = p.vals[j];
                                at[run] = j as i32 + 1;
                            }
                            run += 1;
                            continue;
                        }
                    }
                    run = 0;
                }
                if run >= 2 {
                    let last = p.vals.get(at[(run - 1).min(7)] as usize).copied().unwrap_or(0.0);
                    val.push(last);
                    let mx = mat::max_val(&val);
                    res = if 0.1 <= mx { run as i32 } else { 0 };
                    if res != 0 && mx < 0.35 && !(0..run - 1).all(|i| gap.get(i) == gap.get(i + 1)) {
                        res = 0;
                    }
                }
            }
            rolls[u] = res;
        }
    }
    let d = blocks[k as usize] - blocks[0];
    let q = mat::cdiv(d, 4);
    let r = |i: i32| if i >= 0 { rolls.get(i as usize).copied().unwrap_or(0) } else { 0 };
    if nb > 3 && d > 7 && r(q - 1) > 1 && r(q - 2) < 2 {
        return Some(blocks[k as usize] - 2);
    }
    Some(0)
}

/// `IntroFeatureAna(blocks, labels, peaks, quarters, x, fills, high, n)` @0x101dcb7dc: a fill
/// at the end of the intro (`FillinCutAna` on block 1, else a snare roll) marks the intro
/// 110; intros with a steady four-beat feel become 11 / 111. Returns (start, end) of the cut.
#[allow(clippy::too_many_arguments)]
pub fn intro_feature_ana(
    blocks: &[i32],
    labels: &mut [i32],
    peaks: &[f64],
    quarters: &[f64],
    x: &[f64],
    fills: &mut [i32],
    high: &[f64],
    n: i32,
) -> Option<(i32, i32)> {
    let m = labels.len() as i32 - 1;
    if m <= 0 {
        return Some((0, 0));
    }
    let (mut s, mut e) = (0, 0);
    for u in 0..m as usize {
        if labels[u] != L_INTRO {
            continue;
        }
        let (cs, ce) = fillin_cut_ana(blocks, 1, peaks, x);
        s = cs;
        e = ce;
        let mut start = cs;
        if cs == 0 {
            start = fillin_snare_ana_ex(blocks, high, n, u as i32 + 1)?;
            s = 0;
            if start == 0 {
                continue;
            }
        }
        labels[0] = 110;
        fills[0] = start;
    }
    for u in 0..m as usize {
        if (labels[u] == L_INTRO || labels[u] == 110)
            && beat4_ana(blocks, u as i32 + 1, peaks, quarters) == 1 {
                labels[u] += 1;
            }
    }
    Some((s, e))
}

/// `UpSimpleloopAna_b(blocks, labels, before, level)` @0x1020b8bd4: in each run of "up" (31)
/// blocks only the loudest keeps the label; the others get their label from `before` back.
pub fn up_simpleloop_b(blocks: &[i32], labels: &mut [i32], before: &[i32], level: &[f64]) {
    let n1 = blocks.len();
    let mut run = vec![0i32; n1];
    if n1 < 2 {
        return;
    }
    let m = n1 - 1;
    let mut left = 0i32;
    let mut end = n1 as i32;
    for u in 0..m {
        if labels[u] != 31 {
            continue;
        }
        let mut v = u;
        if u + 1 < n1 {
            while v != n1 - 1 {
                if labels[v + 1] != 31 {
                    end = v as i32 + 2;
                    break;
                }
                v += 1;
            }
        }
        if left == 0 {
            left = end - (u as i32 + 1);
            run[u] = left;
        }
        left -= 1;
    }
    for u in 0..m {
        let len = run[u];
        if len <= 1 {
            continue;
        }
        let w: Vec<f64> = (0..len).map(|i| get(level, u as i32 + i)).collect();
        let (_, at) = mat::max_idx(&w);
        let top = at as i32 + 1 + u as i32;
        for p in u..u + len as usize {
            if p as i32 + 1 != top
                && let (Some(l), Some(&b)) = (labels.get_mut(p), before.get(p)) {
                    *l = b;
                }
        }
    }
}

/// `UpFeatureAna(blocks, labels, peaks, x, fills, high, n)` @0x10109e414: up blocks (30–33)
/// with a fill at their end (`FillinSnareAna`, `FillinCutAna`, snare roll) get +300 and the
/// fill start. Returns the `FillinSnareAna`/`FillinCutAna` (start, end) per block.
pub fn up_feature_ana(
    blocks: &[i32],
    labels: &mut [i32],
    peaks: &[f64],
    x: &[f64],
    fills: &mut [i32],
    high: &[f64],
    n: i32,
) -> Option<(Vec<i32>, Vec<i32>)> {
    let l = labels.len();
    let mut starts = vec![0i32; l.saturating_sub(1)];
    let mut ends = vec![0i32; l.saturating_sub(1)];
    if l > 2 {
        for t in 0..l - 2 {
            let lab = labels[t + 1];
            if !((lab - 34) as u32 > 0xffff_fffb) {
                continue;
            }
            let k = t as i32 + 2;
            let (s, e) = fillin_snare_ana(blocks, k, peaks);
            starts[t + 1] = s;
            ends[t + 1] = e;
            let mut start = s;
            if s == 0 {
                let (cs, ce) = fillin_cut_ana(blocks, k, peaks, x);
                starts[t + 1] = cs;
                ends[t + 1] = ce;
                start = cs;
                if cs == 0 {
                    start = fillin_snare_ana_ex(blocks, high, n, k)?;
                    if start == 0 {
                        continue;
                    }
                }
            }
            labels[t + 1] += 300;
            fills[t + 1] = start;
        }
    }
    Some((starts, ends))
}

/// `HookFeatureAna(blocks, labels, peaks, quarters, x, fills, high, n)` @0x1020b35f4: hook
/// blocks (40) with a fill at their end become 440 (+ fill start); hooks with a steady
/// four-beat feel get +1. Returns the `FillinCutAna` (start, end) per block.
#[allow(clippy::too_many_arguments)]
pub fn hook_feature_ana(
    blocks: &[i32],
    labels: &mut [i32],
    peaks: &[f64],
    quarters: &[f64],
    x: &[f64],
    fills: &mut [i32],
    high: &[f64],
    n: i32,
) -> Option<(Vec<i32>, Vec<i32>)> {
    let l = labels.len();
    let m = l.saturating_sub(1);
    let mut starts = vec![0i32; m];
    let mut ends = vec![0i32; m];
    if l > 2 {
        for u in 1..m {
            if labels[u] != L_HOOK {
                continue;
            }
            let (s, e) = fillin_cut_ana(blocks, u as i32 + 1, peaks, x);
            starts[u] = s;
            ends[u] = e;
            let mut start = s;
            if s == 0 {
                start = fillin_snare_ana_ex(blocks, high, n, u as i32 + 1)?;
                if start == 0 {
                    continue;
                }
            }
            labels[u] = 440;
            fills[u] = start;
        }
        for u in 1..m {
            if (labels[u] == L_HOOK || labels[u] == 440) && beat4_ana(blocks, u as i32 + 1, peaks, quarters) == 1 {
                labels[u] += 1;
            }
        }
    }
    Some((starts, ends))
}

/// `OutroFeatureAna(blocks, labels, peaks, pattern, quarters, bar)` @0x10079c4a4: where the
/// outro (last block) ends: the rhythm kick-out, a steady fall of the beat peaks to silence,
/// or a jump that then decays; rounded off the bar's last beat. Returns (labels with the
/// outro +1 for a steady four-beat feel, the end per block: 0 but the last).
pub fn outro_feature_ana(
    blocks: &[i32],
    labels: &[i32],
    peaks: &[f64],
    pattern: &[u8],
    quarters: &[f64],
    bar: i32,
) -> (Vec<i32>, Vec<i32>) {
    let mut labs = labels.to_vec();
    let mut x = peaks.to_vec();
    let nb = blocks.len();
    let nl = labs.len();
    let kick_all = outro_kickout_ana(blocks, &labs, pattern);
    let last = nl - 1;
    let mut kick = kick_all.get(last).copied().unwrap_or(0);
    if kick != 0 {
        let a = nz_at(&mut x, kick - 1);
        let b = nz_at(&mut x, kick - 2);
        if 0.8 < a / b || sum_ij(&x, kick - 5, kick - 2) * 0.25 < 0.05 {
            kick = 0;
        }
    }
    let nf = x.len() as i32;
    let e = blocks[nb - 1];
    let start = e + 9;
    let mut j = 0i32;
    let end = loop {
        if nf + j < start {
            break nf;
        }
        let v = get(&x, nf - 1 + j);
        j -= 1;
        if !(v <= 0.0007) {
            break nf + j + 2;
        }
    };
    let n3 = if start <= end { (end - start) as usize } else { 0 };
    let mut r1 = vec![0.0f64; n3];
    let mut r2 = vec![0.0f64; n3];
    let ylen = (nf - e - 8).max(0) as usize;
    let mut y: Vec<f64> = (0..ylen).map(|i| get(&x, e + 8 + i as i32)).collect();
    if n3 > 1 {
        let b = nz_at(&mut y, 1);
        let a = nz_at(&mut y, 0);
        r1[1] = b / a;
        for i in 2..n3 {
            let b = nz_at(&mut y, i as i32);
            let a = nz_at(&mut y, i as i32 - 1);
            r1[i] = b / a;
            r2[i] = (b / a) / r1[i - 1];
        }
    }
    let yv = |i: usize| y.get(i).copied().unwrap_or(0.0);
    if n3 > 1 {
        for u in 1..n3 {
            if !(r1[u] < 0.5) {
                continue;
            }
            let tail: Vec<f64> = (u + 1..n3).filter(|&i| 0.001 < yv(i)).map(|i| r1[i]).collect();
            if !tail.is_empty() && mat::sum(&tail) / tail.len() as f64 > 0.86 {
                continue;
            }
            if (u + 1..n3).any(|i| 1.1 < r1[i] && 0.001 < yv(i)) {
                continue;
            }
            let u2 = u as i32 + start;
            if kick == 0 || u2 < kick {
                kick = u2;
                if u2 < end - 3 {
                    let r = mat::crem(1 - bar + u2, 4);
                    if r == 2 {
                        if u > 1 && (1.3 < r1[u - 1] || 1.3 < r1[u - 2]) {
                            kick = start + u as i32 - 2;
                        }
                    } else if r == 3 && u > 1 && 1.3 < r1[u - 2] && 1.3 < get(&r1, u as i32 - 3) {
                        kick = start + u as i32 - 3;
                    }
                }
            }
        }
    }
    let mut found = kick;
    if n3 > 1 {
        'b: for u in 1..n3 {
            if !(0.2 < yv(u) && 2.0 < r1[u]) {
                continue;
            }
            let mut k = u + 1;
            loop {
                if n3 <= k || yv(k) < 0.0007 {
                    let v = start + u as i32;
                    found = if kick != 0 && kick <= v { kick } else { v };
                    break 'b;
                }
                let d = r1[k];
                k += 1;
                if !(d <= 1.0) {
                    break;
                }
            }
        }
    }
    let mut res = found;
    if n3 > 3 {
        for u in 1..n3 - 3 {
            if 1.3 < r2[u] && r2[u + 1] < 1.03 && r2[u + 2] < 1.03 {
                if n3 <= u + 4 {
                    break;
                }
                if (u + 3..n3).all(|i| r2[i] <= 0.91) {
                    let v = start + u as i32;
                    res = if found != 0 && found <= v { found } else { v };
                    break;
                }
            }
        }
    }
    let mut fin = if res != 0 { res } else { end };
    if (fin - bar + 1) & 3 == 0 && fin < end {
        fin += 1;
    }
    let mut ends = vec![0i32; nb];
    ends[last] = fin;
    let mut b2 = blocks.to_vec();
    b2.push(fin);
    if beat4_ana(&b2, nl as i32, &x, quarters) == 1 {
        labs[last] += 1;
    }
    (labs, ends)
}
