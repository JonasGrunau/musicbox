//! Stage D's track-level classifiers that pick the phrase "mood" (rekordbox's genre 1–3):
//! `DetectComp`, `CalcMaxLevelRatio`, `DetectBPM128`, `DetectTenkeiGenre1`,
//! `CalcTrackLowPowerRatio`, `DetectSlowTempo`, `Detect4KicksPopRockRhythm` and
//! `GenreDevideOverall` (`analysis-phrase.md` § 7.2).

use super::mat;

/// The 16-term sum `CalcTrackLowPowerRatio` unrolls:
/// `((((a1+a0)+(a2+a3)) + ((a4+a5)+a6)) + (((a7+a8)+a9)+a10)) + ((((a11+a12)+a13)+a14)+a15)`.
fn sum16(a: &[f64]) -> f64 {
    let g1 = (a[1] + a[0]) + (a[2] + a[3]);
    let g2 = (a[4] + a[5]) + a[6];
    let g3 = ((a[7] + a[8]) + a[9]) + a[10];
    let g4 = (((a[11] + a[12]) + a[13]) + a[14]) + a[15];
    ((g1 + g2) + g3) + g4
}

/// `MatArrayFloat::sum(i, j)`.
fn sum_ij(y: &[f64], i: i32, j: i32) -> f64 {
    let n = y.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0.0;
    }
    mat::sum(&y[i as usize..=j as usize])
}

/// `DetectComp(low, all, n, pattern, bar)`: per 8-bar window from the first downbeat, the
/// low-band and full-band energy at the rhythm-pattern hits (and the quarter after each)
/// against the rest. Returns (max share of the full band off the hits, max mean low-band level
/// at the hits × 0.0075175301); (0.1, 0.3) for tracks under 8 bars.
pub fn detect_comp(low: &[f64], all: &[f64], pattern: &[u8], bar: i32) -> (f64, f64) {
    let base = bar * 4 - 4;
    let span = pattern.len() as i32 - base;
    if span < 0x80 {
        return (0.1, 0.3);
    }
    let windows = (span / 128) as usize;
    let len2 = all.len() as i64;
    let lim = len2 - i64::from(base);
    let mut share = vec![0.0f64; windows];
    let mut level = vec![0.0f64; windows];
    for (w, (sh, lv)) in share.iter_mut().zip(level.iter_mut()).enumerate() {
        let (mut sa, mut sq) = (0.0f64, 0.0f64);
        let mut cnt = 0i32;
        let mut last = 0i64;
        for p in 0..128i64 {
            let pp = 128 * w as i64 + p;
            if pattern.get(pp as usize).copied() != Some(1) {
                continue;
            }
            let q = i64::from(base) + pp;
            if pp + 1 != last && q < len2 {
                sa += all[q as usize];
                sq += low[q as usize];
            }
            cnt += 1;
            if lim < q + 1 {
                last = 0;
            } else {
                if q + 1 < len2 {
                    sa += all[(q + 1) as usize];
                    sq += low[(q + 1) as usize];
                }
                last = pp + 2;
            }
        }
        let i = base + w as i32 * 128;
        let total = sum_ij(all, i, i + 0x7f);
        if cnt > 0 {
            let mut other = 0.0;
            if cnt < 128 {
                if cnt < 4 {
                    continue;
                }
                other = (total - sa) / f64::from(128 - cnt);
            }
            let avg = sa / f64::from(cnt);
            let s = other + avg;
            *sh = if s <= 0.0 { 0.0 } else { other / s };
            *lv = sq / f64::from(cnt);
        }
    }
    (mat::max_val(&share), mat::max_val(&level) * 0.0075175301)
}

/// `CalcMaxLevelRatio(peaks)`: share of 16-quarter windows whose peak reaches 0.89125 (−1 dB).
pub fn max_level_ratio(peaks: &[f64]) -> f64 {
    let n = peaks.len() as i32;
    if n <= 0xf {
        return 0.0;
    }
    let w = (n / 16) as usize;
    let hits = (0..w)
        .filter(|&k| 0.89125 <= mat::max_val(&peaks[16 * k..16 * k + 16]))
        .count();
    hits as f64 / w as f64
}

/// `DetectBPM128(bpm)`: (in 127.04–128.96 or 132.33–134.33, else within 122.88–138.66).
pub fn bpm128(bpm: f64) -> (i32, i32) {
    if (127.04..=128.96).contains(&bpm) || (132.330025..=134.32997500000002).contains(&bpm) {
        return (1, 0);
    }
    let outside = !(122.88..=133.12).contains(&bpm);
    if outside && !(127.99680000000001..=138.66320000000002).contains(&bpm) {
        return (0, 0);
    }
    (0, 1)
}

/// `DetectTenkeiGenre1(bpm, kick6)`.
pub fn tenkei_genre1(bpm: f64, kick6: bool) -> i32 {
    i32::from((96.1..=105.01).contains(&bpm) && kick6)
}

/// `CalcTrackLowPowerRatio(low, all, n)`: √(Σlow/Σall), per bar (16 quarters from the start)
/// the low/full ratio and the low sum, and their average (under 128 beats) or the largest
/// 16-bar mean.
pub struct LowPowerRatio {
    pub ratio: f64,
    pub bar_ratio: Vec<f64>,
    pub bar_low: Vec<f64>,
    pub mean_ratio: f64,
    pub mean_low: f64,
}

pub fn track_low_power_ratio(low: &[f64], all: &[f64], n: i32) -> LowPowerRatio {
    let sl = mat::sum(low);
    let sa = mat::sum(all);
    let ratio = if 0.0 < sa { (sl / sa).sqrt() } else { 1.0 };
    let nb = (n / 4).max(0) as usize;
    if n < 4 {
        return LowPowerRatio {
            ratio,
            bar_ratio: vec![0.0],
            bar_low: vec![0.0],
            mean_ratio: 0.0,
            mean_low: 0.0,
        };
    }
    let mut bar_ratio = vec![0.0; nb];
    let mut bar_low = vec![0.0; nb];
    for j in 0..nb {
        let a = sum16(&low[16 * j..16 * j + 16]);
        let b = sum16(&all[16 * j..16 * j + 16]);
        bar_low[j] = a;
        bar_ratio[j] = if b <= 0.0 { 0.0 } else { a / b };
    }
    let (mean_ratio, mean_low) = if n < 0x80 {
        (
            mat::sum(&bar_ratio) / nb as f64,
            mat::sum(&bar_low) / nb as f64,
        )
    } else {
        let m = ((n as u32 >> 6) - 1) as usize;
        let r: Vec<f64> = (0..m)
            .map(|k| sum16(&bar_ratio[16 * k..16 * k + 16]) * 0.0625)
            .collect();
        let l: Vec<f64> = (0..m)
            .map(|k| sum16(&bar_low[16 * k..16 * k + 16]) * 0.0625)
            .collect();
        (mat::max_val(&r), mat::max_val(&l))
    };
    LowPowerRatio {
        ratio,
        bar_ratio,
        bar_low,
        mean_ratio,
        mean_low,
    }
}

/// `DetectSlowTempo`: votes over the snare onsets of loud bars whether they fall two beats
/// apart off the beat (half tempo) or one beat apart on it. Returns (slow, half_time).
#[allow(clippy::too_many_arguments)]
pub fn slow_tempo(
    fs: f64,
    bars: &[f64],
    bpm: f64,
    snares: &[i32],
    fb: i32,
    fb2: i32,
    beats8: &[i32],
    n: i32,
    count: i32,
    first: i32,
) -> (i32, i32) {
    let mut x: Vec<f64> = snares.iter().map(|&v| f64::from(v)).collect();
    let end = first + count;
    if count > 0 {
        let d = fs * 0.001 * f64::from(fb - fb2);
        let hi = first.max(end - 1);
        for i in (first - 1)..hi {
            if let Some(v) = x.get_mut(i as usize) {
                *v += d;
            }
        }
    }
    let p = if bpm == 0.0 {
        fs * 0.5
    } else {
        (fs * 60.0) / bpm
    };
    let nb = bars.len() as i32;
    let mean = mat::sum(bars) / f64::from(nb);
    let mut votes: Vec<i32> = Vec::new();
    let mut half = 0;
    if nb > 3 && first <= end - 2 {
        let q = (p * 0.25 + 0.5) as i64 as f64;
        let p1875 = p * 1.875;
        let pi = (p + 0.5) as i32;
        let at = |k: i64| x.get(k as usize).copied().unwrap_or(0.0);
        'outer: for j in 0..(nb - 3) as usize {
            if !(mean < bars[j]) {
                continue;
            }
            let u = 4 * j as i32;
            let e = if n <= u + 32 { n - 1 } else { u + 31 };
            let b0 = beats8.get(u as usize).copied().unwrap_or(0) as i64 as f64;
            let b1 = beats8.get(e as usize).copied().unwrap_or(0) as i64 as f64;
            let mut k = i64::from(first - 1);
            loop {
                let v = at(k);
                if b0 <= v && v <= b1 {
                    let mut r = (v - b0) * (1.0 / f64::from(pi));
                    r -= f64::from((r as i32).wrapping_mul(pi));
                    let big = q * 3.0 < r;
                    if k + 2 < i64::from(end) {
                        let d = at(k + 1) - v;
                        let off = p * 0.125 < d && !(d < p1875) && d != p1875 && d < p * 2.125;
                        if off && (r < q || big) && votes.len() < 64 {
                            votes.push(0);
                        }
                        if p * 0.125 < d
                            && p * 0.875 < d
                            && d < p * 1.125
                            && r >= q
                            && !big
                            && votes.len() < 64
                        {
                            votes.push(1);
                        }
                    }
                }
                k += 1;
                if !(v <= b1 && k < i64::from(end - 2)) {
                    break;
                }
            }
            if votes.len() > 63 {
                break 'outer;
            }
        }
        if !votes.is_empty() && 0.5 < f64::from(votes.iter().sum::<i32>()) / votes.len() as f64 {
            half = 1;
        }
    }
    let slow = i32::from(half == 0 && bpm < 83.9);
    (slow, half)
}

/// `Detect4KicksPopRockRhythm(pattern, bpm, bar_energy)` → (p4, p5, p6, p7): bars 2, 3, 6, 7
/// of each 8-bar group after the first loud bar are matched against five 16-step templates.
pub fn four_kicks(pattern: &[u8], bpm: f64, energy: &[f64]) -> (i32, i32, i32, i32) {
    const T: [[u8; 16]; 5] = [
        [1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        [1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0],
        [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0],
        [1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0],
    ];
    let n = energy.len() as i32;
    let start = if n == 0 {
        1
    } else {
        let mean = mat::sum(energy) / f64::from(n);
        let s0 = if n < 0x11 { 1 } else { 0x11 };
        let mut s = s0;
        if s0 <= n - 3 {
            let mut j = s0;
            while j != n - 2 {
                if mean < energy[(j - 1) as usize] {
                    s = j;
                    break;
                }
                j += 1;
            }
        }
        s
    };
    let span = (pattern.len() as i64 as f64 * 0.063) as i32 - start;
    let groups = if span > 0x3f { 8 } else { span / 8 };
    let thr = i32::from(span > 0x3f);
    let (mut g23, mut g1, mut g0, mut g4) = (0, 0, 0, 0);
    if span >= 1 && groups != 0 {
        for g in 0..groups {
            let (mut h23, mut h1, mut h0, mut h4) = (0, 0, 0, 0);
            for l in 0..8 {
                if !matches!(l, 1 | 2 | 5 | 6) || start - 1 + 8 * g + l < 1 {
                    continue;
                }
                let at = ((start - 1 + 8 * g + l) * 16) as usize;
                let bar: Vec<u8> = (0..16)
                    .map(|k| pattern.get(at + k).copied().unwrap_or(0))
                    .collect();
                let hit = |t: &[u8; 16]| bar.iter().zip(t).all(|(a, b)| a == b);
                if hit(&T[2]) || hit(&T[3]) {
                    h23 += 1;
                }
                if hit(&T[1]) {
                    h1 += 1;
                }
                if hit(&T[0]) {
                    h0 += 1;
                }
                if hit(&T[4]) {
                    h4 += 1;
                }
            }
            g23 += i32::from(h23 > 0);
            g1 += i32::from(h1 > 0);
            g0 += i32::from(h0 > 0);
            g4 += i32::from(h4 > 0);
        }
    }
    if g23 > 1 {
        return (1, 0, 0, 0);
    }
    if g4 > thr {
        return (0, 0, 0, 1);
    }
    if g1 > thr && bpm < 165.0 {
        return (0, 1, 0, 0);
    }
    if g0 > thr {
        return (0, 0, 1, 0);
    }
    (0, 0, 0, 0)
}

/// `GenreDevideOverall`: the track's genre 1–3 from the classifiers.
#[allow(clippy::too_many_arguments)]
pub fn genre_devide(
    slowrock: i32,
    slow: i32,
    low_ratio: f64,
    k: (i32, i32, i32, i32),
    fadeout: i32,
    comp2: f64,
    b128: (i32, i32),
    tenkei: i32,
) -> i32 {
    let (p4, p5, p6, p7) = k;
    if !(0.3979 <= low_ratio) || slowrock == 1 {
        return 3;
    }
    if comp2 < 0.6128 && slow == 1 {
        return 3;
    }
    if fadeout == 1 {
        return 2;
    }
    let a = f64::from(p6) * -0.05;
    let a = (f64::from(b128.0) - f64::from(p7)).mul_add(0.3, a);
    let a = (f64::from(tenkei) + f64::from(b128.1)).mul_add(0.1, a);
    let score = ((f64::from(p4) - f64::from(p5)) + comp2).mul_add(0.6, a);
    if score > 1.0 { 1 } else { 2 }
}

/// `DetectFadeOut(low, all)`: over the last 512 quarters (32 blocks of 16, paired), whether
/// the full-band energy falls away block by block at the end while the low share stays up.
pub fn fade_out(low: &[f64], all: &[f64]) -> i32 {
    let n = low.len() as i32;
    if n < 0x610 {
        return 0;
    }
    let sum = |v: &[f64], at: i32| -> f64 {
        let a = |k: i32| v.get((at + k) as usize).copied().unwrap_or(0.0);
        let mut s = 0.0 + a(0);
        s += a(1) + a(2);
        s += (a(3) + a(4)) + a(5);
        s += ((a(6) + a(7)) + a(8)) + a(9);
        s += (((a(10) + a(11)) + a(12)) + a(13)) + a(14);
        s + a(15)
    };
    let mut e = [0.0f64; 32];
    let mut l = [0.0f64; 32];
    for i in 0..32 {
        let at = (n & !15) + 16 * i as i32 - 0x210;
        l[i] = sum(low, at);
        e[i] = sum(all, at);
    }
    let e2: Vec<f64> = (0..16)
        .map(|j| {
            if j == 0 {
                e[0] + e[1]
            } else {
                (e[2 * j] + 0.0) + e[2 * j + 1]
            }
        })
        .collect();
    let mut r = [0.0f64; 15];
    if !e2.iter().any(|&v| v < 1e-07) {
        for j in 0..15 {
            let s = if j == 0 {
                l[0] + l[1]
            } else {
                (l[2 * j] + 0.0) + l[2 * j + 1]
            };
            r[j] = s / e2[j];
        }
    }
    let mut m = e2[0];
    for &v in &e2[1..] {
        if m <= v {
            m = v;
        }
    }
    if m * 0.001 <= e2[15] || !(e2[15] < e2[14]) || !(m <= r[14]) {
        return 0;
    }
    let mut rm = r[0];
    for &v in &r[2..12] {
        if rm <= v {
            rm = v;
        }
    }
    if r[14] <= rm && e2[14] < e2[13] && m <= r[13] && r[13] <= rm && e2[13] < e2[12] {
        i32::from(m <= r[12] && r[12] <= rm)
    } else {
        0
    }
}

/// `Genre12FinalJudge(genre, n, peaks, quarter_peaks)`: genre 2 becomes 1 when the low-band
/// peak sits on the same quarter (or a steady one) beat after beat and the bar-to-bar peak
/// level keeps changing by 1.5× on average.
pub fn genre12_final(genre: &mut i32, n: i32, peaks: &[f64], quarters: &[f64]) {
    if *genre != 2 || n < 0x60 {
        return;
    }
    let level = {
        let (i, j) = (7, n - 0x42);
        if j - i < 0 {
            0.0
        } else {
            super::genre::sum_range(peaks, i, j) / f64::from(j - i + 1)
        }
    };
    let nq = (n - 81) as usize;
    let mut idx = vec![0i32; nq.max(1)];
    let mut a = vec![0.0f64; nq];
    let mut c = vec![0.0f64; nq];
    let q = |i: i32| quarters.get(i as usize).copied().unwrap_or(0.0);
    for j in 1..nq {
        let base = 28 + 4 * j as i32;
        let w: Vec<f64> = (0..4).map(|k| q(base + k)).collect();
        let (m, k0) = mat::max_idx(&w);
        let k = k0 as i32 + 1;
        let m2 = if m != 0.0 { m } else { 0.003 };
        let prev = idx[j - 1];
        let d = (k - prev).abs();
        idx[j] = k;
        c[j] = m;
        if k == prev {
            a[j - 1] = 1.0;
        } else if d & 0x7ffffffd == 1 {
            a[j - 1] = q(31 + 4 * (j as i32 - 1) + prev) / m2;
        }
    }
    let steady = nq >= 1 && 0.91 < mat::sum(&a) / nq as i64 as f64;
    let nd = nq as i32 - 4;
    let mut dr = vec![0.0f64; nd.max(0) as usize];
    if nq > 6 {
        for j in 2..(nq - 4) {
            if c[j] == 0.0 {
                c[j] = 0.003;
            }
            if c[j - 1] == 0.0 {
                c[j - 1] = 0.003;
            }
            let (x, y) = (c[j], c[j - 1]);
            dr[j] = if x < y { y / x } else { x / y };
        }
    }
    if nd > 2 {
        let s = super::genre::sum_range(&dr, 2, nd - 1);
        if 1.5 <= s / f64::from(nd - 2) && 0.2 <= level && steady {
            *genre = 1;
        }
    }
}

pub(super) fn sum_range(y: &[f64], i: i32, j: i32) -> f64 {
    sum_ij(y, i, j)
}

/// `FindSecurePoint(peaks, bar)`: bars where the mean beat peak jumps up (above 0.6× the
/// largest jump and 0.15) and the next two bars hold that level within ±30 %. Returns the
/// ascending 1-based bar numbers + 1, or `[0]`.
pub fn secure_points(peaks: &[f64], bar: i32) -> Vec<i32> {
    let n = peaks.len() as i32;
    let nb = (n / 4).max(0) as usize;
    let mut m = vec![0.0f64; nb];
    let mut d = vec![0.0f64; nb];
    let last = (nb as i32 - 1).max(1) as usize;
    if n > 7 {
        for k in 0..last.min(nb) {
            let i = bar - 1 + 4 * k as i32;
            m[k] = sum_ij(peaks, i, i + 3) * 0.25;
        }
        for k in 0..last.min(nb) {
            d[k] = m.get(k + 1).copied().unwrap_or(0.0) - m[k];
        }
    }
    let dmax = mat::max_val(&d);
    let mut cand = vec![0i32; nb.max(1)];
    let mut nc = 0;
    if n > 11 {
        for k in 0..(nb.max(3) - 2) {
            let v = d.get(k).copied().unwrap_or(0.0);
            if dmax * 0.6 < v && v > 0.15 {
                cand[nc] = k as i32 + 1;
                nc += 1;
            }
        }
        let at = |i: i32| m.get(i as usize).copied().unwrap_or(0.0);
        for v in cand.iter_mut().take(nc) {
            let c = *v;
            let (hi, lo) = (at(c) * 1.3, at(c) * 0.7);
            let ok = |x: f64| x <= hi && !(x < lo);
            if !(ok(at(c + 1)) && ok(at(c + 2))) {
                *v = 0;
            }
        }
    }
    cand.sort_by(|a, b| b.cmp(a));
    let mut out = Vec::new();
    for (i, &v) in cand.iter().enumerate() {
        if v != 0 {
            out.push(v + 1);
        } else if i == 0 {
            out.push(0);
        }
    }
    out.sort_unstable();
    out
}
