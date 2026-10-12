//! Snare onsets: `DetectSnarePositionsFilter` @0x101306748, `DetectSnarePositions`
//! @0x101306b98 and `DetectSnarePositionsForBar` @0x100e916d0 (`analysis-phrase.md` § 4.4).

use super::filter::{CFilter, prewarp};
use super::mat;

/// One bar of the snare envelope, sorted descending, with the 1-based sample offsets
/// (`DetectSnarePositions`' per-bar `MatArrayFloat` after `sort_descend_idx`).
#[derive(Debug, Clone, Default)]
pub struct BarPeaks {
    pub vals: Vec<f64>,
    pub idx: Vec<i32>,
}

/// Picked positions (samples from the first beat) and where the non-zero ones start.
#[derive(Debug, Clone, Default)]
pub struct Snares {
    /// Ascending; the leading entries are 0 (unused slots).
    pub pos: Vec<i32>,
    /// How many entries are non-zero.
    pub count: i32,
    /// 1-based index of the first non-zero entry.
    pub first: i32,
}

/// `DetectSnarePositionsFilter(x, fs)`: 300 Hz 8th-order high-pass, rectify, 5 Hz smoothing,
/// first difference (the first value 0).
pub fn envelope(x: &[f64], fs: f64) -> Vec<f64> {
    let fs = if fs != 0.0 { fs } else { 5512.5 };
    let w = prewarp(1884.9555921538758, fs);
    let ww = w * w;
    let a = CFilter::make2(1.0, 0.0, 0.0, 1.0, w * 1.8477590650225735, ww, fs);
    let b = CFilter::make2(1.0, 0.0, 0.0, 1.0, w * 0.7653668647301797, ww, fs);
    let w = prewarp(31.41592653589793, fs);
    let mut lp = CFilter::make1(0.0, w, 1.0, w, fs);
    let mut y = x.to_vec();
    for mut f in [a, b, a, b] {
        f.filter2(&mut y);
    }
    for v in &mut y {
        *v = v.abs();
    }
    lp.filter1(&mut y);
    for i in (1..y.len()).rev() {
        y[i] -= y[i - 1];
    }
    if let Some(v) = y.first_mut() {
        *v = 0.0;
    }
    y
}

fn period(fs: f64, bpm: f64) -> (f64, f64) {
    let bpm = if bpm != 0.0 { bpm } else { 120.0 };
    let fs = if fs != 0.0 { fs } else { 5512.5 };
    (fs, (fs * 60.0) / bpm)
}

/// Sorts the positions ascending and counts the non-zero tail, then makes them relative to
/// the first beat (the end of both `DetectSnarePositions*`).
fn finish(mut pos: Vec<i32>, nbeats: i32, fs: f64, fb_ms: i32) -> Snares {
    pos.sort_unstable();
    let cap = pos.len() as i32;
    let mut first0 = 0i32;
    if nbeats >= 8 {
        let lim = cap.max(1);
        match (0..lim).find(|&i| pos.get(i as usize).copied().unwrap_or(0) != 0) {
            Some(i) => first0 = i,
            None => {
                return Snares {
                    pos,
                    count: cap - lim,
                    first: lim + 1,
                };
            }
        }
    }
    let count = cap - first0;
    if count != 0 && first0 <= cap {
        let off = (fs * 0.001).mul_add(fb_ms as f64, 0.5) as i32;
        for p in &mut pos[first0 as usize..] {
            *p -= off;
        }
    }
    Snares {
        pos,
        count,
        first: first0 + 1,
    }
}

/// `DetectSnarePositions`: per bar of the first half, the envelope peaks that stand out
/// (a drop of more than 1.14 % to the next value), then keeps the ones with a partner two
/// beats away. Returns the bars' sorted peaks for `…ForBar` too.
pub fn detect(
    env: &[f64],
    fs: f64,
    bpm: f64,
    fb_ms: i32,
    beats: &[i32],
) -> Option<(Snares, Vec<BarPeaks>)> {
    let nbeats = beats.len() as i32;
    let cap = (nbeats / 8 * 8).max(0);
    let nbars = (nbeats / 8).max(0) as usize;
    let (fs, p) = period(fs, bpm);
    let mut pos = vec![0i32; cap as usize];
    let mut bars = Vec::with_capacity(nbars);
    let mut total = 0i32;
    if nbeats > 7 {
        let near = (p.mul_add(0.25, 0.5) as i32) - 20;
        for u in 0..nbars {
            let b = beats[4 * u];
            let e = beats[4 * u + 4];
            if e - b < 0 {
                return None;
            }
            let lo = (b - 1) as usize;
            let mut vals = env.get(lo..lo + (e - b) as usize)?.to_vec();
            if vals.is_empty() {
                return None;
            }
            let idx = mat::sort_descend_idx(&mut vals);
            let n = vals.len();
            let mut picked = 0usize;
            if n > 1 {
                let mut k = 0usize;
                while k != n - 1 {
                    let close = idx[..picked]
                        .iter()
                        .any(|&j| j - near <= idx[k] && idx[k] <= j + near);
                    if !close {
                        let v = vals[k] * 0.9886;
                        let next = vals[k + 1];
                        if next < v && total < cap {
                            pos[total as usize] = b - 1 + idx[k];
                            total += 1;
                            picked += 1;
                        } else if (n as i32) < k as i32 + 3
                            || cap <= total
                            || !(next * 0.9886 > vals[k + 2])
                        {
                            break;
                        }
                    }
                    k += 1;
                }
            }
            bars.push(BarPeaks { vals, idx });
        }
    }
    pos.sort_unstable();
    if nbeats > 7 {
        let two = (p + p + 0.5) as i32;
        let tol = (p * 0.03125 + 0.5) as i32;
        let lim = (cap as usize).max(1);
        for i in 0..lim {
            let v = pos[i];
            if v == 0 {
                continue;
            }
            let (a, b) = (v - two, v + two);
            let partner = pos[..lim].iter().any(|&w| {
                w != 0 && ((w - tol <= a && a <= w + tol) || (w - tol <= b && b <= w + tol))
            });
            if !partner {
                pos[i] = 0;
            }
        }
    }
    Some((finish(pos, nbeats, fs, fb_ms), bars))
}

/// `DetectSnarePositionsForBar`: the same peaks with a 0.115 % drop, de-duplicated against
/// everything picked so far, then kept only with a two-beat partner and a pattern that does
/// not look like a backbeat fill.
pub fn detect_for_bar(
    bars: &[BarPeaks],
    fs: f64,
    bpm: f64,
    fb_ms: i32,
    beats: &[i32],
    pattern: &[u8],
) -> Snares {
    let nbeats = beats.len() as i32;
    let cap = ((nbeats / 8) * 16).max(0);
    let (fs, p) = period(fs, bpm);
    let quarter = p.mul_add(0.25, 0.5) as i32;
    let near = quarter - 20;
    let mut pos = vec![0i32; cap as usize];
    let mut total = 0i32;
    const DROP: f64 = 0.998849369936505;
    if nbeats > 7 {
        for (u, bar) in bars.iter().enumerate().take((nbeats / 8) as usize) {
            let n = bar.vals.len();
            if n < 2 {
                continue;
            }
            let b = beats[4 * u];
            let mut k = 0usize;
            while k != n - 1 {
                let at = bar.idx[k] + b;
                let close = pos[..total as usize]
                    .iter()
                    .any(|&j| j - near <= at && at <= j + near);
                if !close && total < cap {
                    let next = bar.vals[k + 1];
                    if next < bar.vals[k] * DROP {
                        pos[total as usize] = b - 1 + bar.idx[k];
                        total += 1;
                    } else if (n as i32) < k as i32 + 3 || !(next * DROP > bar.vals[k + 2]) {
                        break;
                    }
                }
                k += 1;
            }
        }
    }
    pos.sort_unstable();
    if nbeats > 7 {
        let two = (p + p + 0.5) as i32;
        let tol = (p * 0.125 + 0.5) as i32;
        let off = (fs * 0.001).mul_add(fb_ms as f64, 0.5) as i32;
        let lim = (cap as usize).max(1);
        let plen = pattern.len() as i32;
        let pat = |i: i32| pattern.get(i as usize).copied().unwrap_or(0);
        for i in 0..lim {
            let v = pos[i];
            if v == 0 {
                continue;
            }
            let (a, b2) = (v - two, v + two);
            let partner = pos[..lim].iter().any(|&w| {
                w != 0 && ((w - tol <= a && a <= w + tol) || (w - tol <= b2 && b2 <= w + tol))
            });
            let keep = partner && {
                let s = mat::cdiv(v - off, quarter);
                s < 6
                    || plen <= s + 5
                    || (((pat(s - 4) == 1 && pat(s + 2) == 1)
                        || (pat(s - 6) == 1 && pat(s + 5) == 1)
                        || (pat(s - 4) == 1 && pat(s + 4) == 1))
                        && pat(s) != 1)
            };
            if !keep {
                pos[i] = 0;
            }
        }
    }
    finish(pos, nbeats, fs, fb_ms)
}

/// `DetectSnarePositionsForGenre`'s result: the positions that have a partner two beats or a
/// bar away (relative to the first beat, ascending, zeros first) and their count / first index
/// (+0x210, +0x250, +0x254).
#[derive(Debug, Clone, Default)]
pub struct GenreSnares {
    pub pos: Vec<i32>,
    pub count: i32,
    pub first: i32,
}

/// `DetectSnarePositionsForGenre`: like `…ForBar` (1.14 % drops, de-duplicated against all
/// picks), keeping onsets with a partner two beats away or a bar away; without the four-kick
/// flag, onsets on a low-band pattern hit are dropped too.
#[allow(clippy::too_many_arguments)]
pub fn detect_for_genre(
    bars: &[BarPeaks],
    fs: f64,
    bpm: f64,
    fb_ms: i32,
    beats: &[i32],
    k4: i32,
    pattern: &[u8],
) -> GenreSnares {
    let nbeats = beats.len() as i32;
    let cap = (nbeats / 8 * 8).max(0);
    let (fs, p) = period(fs, bpm);
    let quarter = p.mul_add(0.25, 0.5) as i32;
    let near = quarter - 20;
    let mut pos = vec![0i32; cap as usize];
    let mut total = 0i32;
    if nbeats > 7 {
        for (u, bar) in bars.iter().enumerate().take((nbeats / 8) as usize) {
            let n = bar.vals.len();
            if n < 2 {
                continue;
            }
            let b = beats[4 * u];
            let mut k = 0usize;
            while k != n - 1 {
                let at = bar.idx[k] + b;
                let close = pos[..total as usize]
                    .iter()
                    .any(|&j| j - near <= at && at <= j + near);
                if !close && total < cap {
                    let next = bar.vals[k + 1];
                    if next < bar.vals[k] * 0.9886 {
                        pos[total as usize] = b - 1 + bar.idx[k];
                        total += 1;
                    } else if (n as i32) < k as i32 + 3 || !(next * 0.9886 > bar.vals[k + 2]) {
                        break;
                    }
                }
                k += 1;
            }
        }
    }
    pos.sort_unstable();
    let mut out = pos.clone();
    let off = (fs * 0.001).mul_add(fb_ms as f64, 0.5) as i32;
    let lim = (cap as usize).max(1);
    if nbeats > 7 {
        let two = (p + p + 0.5) as i32;
        let bar4 = (p * 4.0 + 0.5) as i32;
        let eighth = (p * 0.125 + 0.5) as i32;
        let plen = pattern.len() as i32;
        let pat = |i: i32| pattern.get(i as usize).copied().unwrap_or(0);
        for i in 0..lim {
            let v = pos.get(i).copied().unwrap_or(0);
            if v == 0 {
                continue;
            }
            let near2 = |w: i32| {
                (w - eighth <= v - two && v - two <= w + eighth)
                    || (w - eighth <= v + two && v + two <= w + eighth)
            };
            let near4 = |w: i32| {
                (w - quarter <= v - bar4 && v - bar4 <= w + eighth)
                    || (w - quarter <= v + bar4 && v + bar4 <= w + eighth)
            };
            let found2 = (0..lim).any(|j| {
                let w = pos.get(j).copied().unwrap_or(0);
                w != 0 && near2(w)
            });
            let found4 = (0..lim).any(|j| {
                let w = pos.get(j).copied().unwrap_or(0);
                w != 0 && near4(w)
            });
            if found2 {
                let s = mat::cdiv(v - off, quarter);
                let fill = ((pat(s - 4) != 1 || pat(s + 2) != 1)
                    && (pat(s - 6) != 1 || pat(s + 5) != 1)
                    && (pat(s - 4) != 1 || pat(s + 4) != 1))
                    || pat(s) == 1;
                if 5 < s && s + 5 < plen && fill {
                    pos[i] = 0;
                }
            } else {
                pos[i] = 0;
                if !found4 && let Some(o) = out.get_mut(i) {
                    *o = 0;
                }
            }
        }
        if k4 != 1 {
            for v in out.iter_mut() {
                if *v != 0 {
                    let s = mat::cdiv(*v - off - 1, quarter);
                    if s >= 0 && s + 1 < plen && pat(s) == 1 {
                        *v = 0;
                    }
                }
            }
        }
    }
    out.sort_unstable();
    let (count, first) = if nbeats < 8 {
        (cap, 1)
    } else {
        let f = (0..lim)
            .find(|&i| out.get(i).copied().unwrap_or(0) != 0)
            .unwrap_or(lim) as i32;
        (cap - f, f + 1)
    };
    if first <= cap {
        for v in out.iter_mut().skip((first - 1) as usize) {
            *v -= off;
        }
    }
    GenreSnares {
        pos: out,
        count,
        first,
    }
}
