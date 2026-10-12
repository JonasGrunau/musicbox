//! First-downbeat and bar corrections: `BeatposCorrection2016` @0x101967184 and
//! `BarStartCorrection` @0x1004f089c (`analysis-phrase.md` § 5).

use super::mat;
use super::power;
use super::snare::Snares;

/// `BeatposCorrection2016`'s outputs (+0x200 / +0x208 inputs of `Sub_Main_B`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeatPos {
    /// The corrected first beat (ms).
    pub fb_ms: i32,
    /// 0 unchanged, 1 moved by half a beat, 2 moved onto the first onset, 3 onset check passed
    /// without a move.
    pub moved: i32,
    /// A pattern with a kick on step 0 and the next kick on step 6 (the 1a8 flag).
    pub kick6: bool,
    /// The low band holds a steady four-on-the-floor from the start and `fb` stayed.
    pub steady: bool,
}

/// `is_equal(p, pat, n)`.
fn starts_with(p: &[u8], at: usize, pat: &[u8]) -> bool {
    pat.iter().enumerate().all(|(i, &b)| p.get(at + i).copied().unwrap_or(0) == b)
}

const P7: [u8; 7] = [1, 0, 0, 0, 0, 0, 1];
const P24: [u8; 24] = [1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0];

/// `BeatposCorrection2016(x, fs, bpm, fb, 1, pattern, beats, snares)`.
pub fn beatpos_correction(
    x: &[f64],
    fs: f64,
    bpm: f64,
    fb: i32,
    pattern: &[u8],
    nbeats: i32,
    snares: &Snares,
) -> Option<BeatPos> {
    let mut out = BeatPos { fb_ms: fb, moved: 0, kick6: false, steady: false };
    if nbeats < 16 || ((pattern.len() as i64 as f64 * 0.03125) as i32) < 2 {
        return Some(out);
    }
    let nb8 = nbeats / 8;
    let at = |i: usize| pattern.get(i).copied().unwrap_or(0);
    let (mut a, mut b, mut c, mut d) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let beat_s = 60.0 / bpm;
    let p = beat_s * fs;
    let steps = (nb8 * 16).max(1) as usize;
    for u in 0..steps {
        if starts_with(pattern, u, &P7) {
            if starts_with(pattern, u + 7, &[0; 9]) {
                a.push(u as i32);
            }
            if starts_with(pattern, u + 8, &[0; 2]) {
                b.push(u as i32);
            }
        }
        if starts_with(pattern, u, &P24) {
            c.push(u as i32);
        }
        if at(u) == 1 && at(u + 1) == 0 && (at(u + 3) == 1 || at(u + 7) == 1) && (at(u + 6) == 1 || at(u + 10) == 1) {
            d.push(u as i32);
        }
    }
    out.kick6 = !a.is_empty();
    // Which pattern list to test the snares against, and the threshold.
    let mut half = false;
    let (sel, thr): (Option<&[i32]>, f64) = if c.len() >= 5 {
        (Some(&c), 0.2)
    } else if a.len() >= 5 {
        (Some(&a), 0.4)
    } else {
        let mut offbeat = false;
        if d.len() >= 17 {
            let t = if d.len() > 25 {
                if d.len() > 60 { 0.30000001192092896 } else { 0.4099999964237213 }
            } else {
                0.6499999761581421
            };
            for v in &mut d {
                *v = mat::crem(*v + 1, 4);
            }
            let m = d.iter().copied().max().unwrap_or(0);
            let hits = d.iter().filter(|&&v| v == m).count() as f64;
            if m == 3 && t < hits / d.len() as f64 {
                offbeat = true;
            }
        }
        if offbeat {
            half = true;
            (None, 0.5)
        } else if b.len() >= 5 {
            (Some(&b), 0.45)
        } else {
            (None, 0.5)
        }
    };
    let step = if bpm == 0.0 { 125 } else { (beat_s * 250.0) as i32 };
    let mut x0 = step + fb;
    loop {
        x0 -= step;
        if !(step < x0) {
            break;
        }
    }
    let l1 = power::low_power16_b(x, 400.0, bpm, x0, fs)?;
    let l2 = power::low_power16_b(x, 100.0, bpm, fb, fs)?;
    // The first strong low-band jump after silence.
    let (mut u, mut silent) = (0usize, false);
    if !l1.is_empty() {
        let thr_a = mat::max_val(&l1) * 0.48;
        if let Some(i) = (0..l1.len()).find(|&i| {
            let v = l1[i];
            v > thr_a && (i == 0 || (l1[i - 1] - v).abs() > v * 0.6000000000000001)
        }) {
            u = i;
            silent = i != 0 && l1[..i].iter().all(|&v| v == 0.0);
        }
    }
    let (steady, offbeat_start) = steady_kicks(&l2);
    let mut onset = false;
    if !(u > 3 && !silent) && l1.get(u).copied().unwrap_or(0.0) > 70.0 {
        x0 += u as i32 * step;
        onset = true;
    }
    let mut snare_off = false;
    if let Some(sel) = sel {
        let cap = nb8 * 8;
        let mut found: Vec<i32> = Vec::new();
        if snares.count > 0 && snares.first <= cap {
            let q = p * 0.25;
            for &s in sel {
                let t = q * s as i64 as f64;
                for j in (snares.first - 1)..cap {
                    let v = snares.pos.get(j as usize).copied().unwrap_or(0);
                    if t < v as f64 && !found.contains(&v) {
                        found.push(v);
                    }
                }
            }
        }
        let pi = (p + 0.5) as i32;
        let (lo, hi) = (p * 0.25, p * 0.75);
        let off = found
            .iter()
            .filter(|&&v| {
                let r = mat::crem(v, pi) as f64;
                lo < r && r < hi
            })
            .count() as i32;
        snare_off = !found.is_empty() && off as f64 / found.len() as f64 > thr;
    }
    if onset {
        out.moved = if x0 == fb { 3 } else { 2 };
        out.fb_ms = x0;
    } else if half || snare_off || offbeat_start {
        let mut v = if fb < 250 { fb } else { fb - 250 };
        if bpm != 0.0 {
            v = fb - (beat_s * 500.0 + 0.5) as i32;
            if v < 0 {
                v += (beat_s * 1000.0 + 0.5) as i32;
            }
        }
        out.moved = 1;
        out.fb_ms = v;
    }
    out.steady = steady && out.fb_ms == fb;
    Some(out)
}

/// The second half of the low-band check (@0x1019678cc … 0x101967a44): a run of 16 beats
/// above 0.605·max from the first loud beat, or three 16-beat blocks each holding a 12-beat
/// run. Returns (found, found with the first loud quarter on step 3 of 4).
fn steady_kicks(l2: &[f64]) -> (bool, bool) {
    let n = l2.len();
    if n == 0 {
        return (false, false);
    }
    let m = mat::max_val(l2);
    let Some(k) = (0..n).find(|&i| l2[i] > m * 0.5) else {
        return (false, false);
    };
    let thr = m * 0.605;
    let odd = (k + 1) & 3 == 3;
    let mut run = 0i32;
    let mut i = k;
    loop {
        if thr >= l2[i] {
            run = 0;
        } else {
            if run > 14 {
                return (true, odd);
            }
            run += 1;
        }
        let stop = i + 5 > n;
        i += 4;
        if stop {
            break;
        }
    }
    let mut blocks = 0i32;
    let mut b = k + 1;
    let mut base = k;
    loop {
        let end = (b + 63).min(n);
        if end >= b {
            let mut j = 0usize;
            loop {
                if thr >= l2[base + j] {
                    run = 0;
                } else {
                    let r = run;
                    run += 1;
                    if r >= 11 {
                        blocks += 1;
                        break;
                    }
                }
                j += 4;
                if b + j > end {
                    break;
                }
            }
            if blocks > 2 {
                return (true, odd);
            }
        }
        if run < 12 {
            blocks = 0;
        }
        b += 64;
        base += 64;
        run = 0;
        if n < b {
            return (false, false);
        }
    }
}

/// `BarStartCorrection`: which beat of the bar the corrected first beat is, from where the
/// snares fall (beats 2/4) and from where the low band peaks within 16 beats.
#[allow(clippy::too_many_arguments)]
pub fn bar_start_correction(
    x: &[f64],
    fs: f64,
    bpm: f64,
    fb: i32,
    fb2: i32,
    beats: &[i32],
    snares: &Snares,
    fbar: i32,
    moved: i32,
) -> Option<i32> {
    let first0 = snares.first - 1;
    let mut s: Vec<f64> = snares.pos.iter().map(|&v| v as f64).collect();
    if snares.first <= first0 + snares.count {
        let d = (fs * 0.001) * (fb - fb2) as f64;
        for v in s.iter_mut().skip(first0 as usize).take(snares.count as usize) {
            *v += d;
        }
    }
    let bpm = if bpm != 0.0 { bpm } else { 0.1 };
    let (mut on, mut mid) = (0i32, 0i32);
    if snares.count >= 1 {
        let pd = (fs * 60.0) / bpm + 0.5;
        let pi = pd as i32;
        let q = pd.trunc().mul_add(0.25, 0.5) as i32;
        let two = pi * 2;
        for i in 0..snares.count {
            let v = s.get((first0 + i) as usize).copied().unwrap_or(0.0);
            let r = mat::crem((v + 0.5) as i32, two);
            if q < r && r <= two - q {
                if !(r < pi - q || q + pi < r) {
                    mid += 1;
                }
            } else {
                on += 1;
                mid += 1;
            }
        }
    }
    // The snare values are never filled in by `…ForBar`, so both magnitude sums are 0.
    let parity = |v: i32| mat::crem(v, 2);
    let odd = if mid < 1 || mid < 7 {
        parity(fbar - 1)
    } else {
        let ratio = on as f64 / mid as f64;
        if ratio > 0.886 { 1 } else { 0 }
    };
    let e = power::low_power16_bar(x, bpm, fs, beats)?;
    let m = mat::max_val(&e);
    let ne = e.len() as i32;
    let avg = if ne < 1 { 0.0 } else { (mat::sum(&e) * 0.6) / ne as i64 as f64 };
    let thr = m.mul_add(0.4, avg);
    let mut add = 0;
    if ne > 4 {
        for k in 0..(ne - 4) {
            if e[(odd * 4 + k) as usize] > thr {
                let b = k & 15;
                if (0xe007u32 >> b) & 1 != 0 {
                    break;
                }
                if (5..=8).contains(&b) {
                    add = 2;
                    break;
                }
            }
        }
    }
    let one = 1;
    let bar = if moved == 1 || parity(fbar + 1) != odd {
        add + one + i32::from(odd == 1)
    } else if moved & !1 == 2 {
        one
    } else {
        fbar
    };
    Some(bar)
}
