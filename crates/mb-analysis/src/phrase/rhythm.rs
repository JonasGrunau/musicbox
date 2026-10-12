//! Beat positions and the 16-step low-band rhythm pattern: `BeatsSamplesCalc` @0x1008d4778,
//! `RhythmPatternAnalyze_init` @0x10222f000 and `RhythmPatternAnalyze` @0x10222f334
//! (`analysis-phrase.md` § 4.1–4.2).

use super::filter::{prewarp, CFilter, SQRT2};
use super::mat;

/// `BeatsSamplesCalc(x, fs, bpm, fb_ms)`: 1-based beat positions at `fs`, the last one
/// clamped to the signal length.
pub fn beats_samples(len: usize, fs: f64, bpm: f64, fb_ms: i32) -> Vec<i32> {
    let period = (fs * 60.0) / bpm;
    let off = (fs * 0.001).mul_add(fb_ms as f64, 0.5) as i32;
    let n = ((len as i32 - off) as f64 / period) as i32;
    let n = n.max(0) as usize;
    let mut beats: Vec<i32> = (0..n)
        .map(|i| period.mul_add(i as f64, off as f64 + 1.5) as i32)
        .collect();
    if let Some(last) = beats.last_mut() {
        if *last > len as i32 {
            *last = len as i32;
        }
    }
    beats
}

/// `RhythmPatternAnalyze`'s state (+0x260 … +0x398).
#[derive(Debug, Clone)]
pub struct RhythmPattern {
    /// +0x260: the previous call's last envelope value.
    last: f64,
    /// +0x268: the pre-roll offset found by the last call.
    offset: i32,
    /// +0x270: the running bar maximum.
    runmax: f64,
    /// +0x278 … +0x358: the 200 Hz low-pass (0, 2 or 4 sections by rate).
    low: Vec<CFilter>,
    /// +0x360: the 10 Hz envelope smoother.
    smooth: CFilter,
}

/// `fs` within `±tol` of `centre` (the init's rate checks).
fn near(fs: f64, lo: f64, hi: f64) -> bool {
    lo < fs && fs < hi
}

impl RhythmPattern {
    /// `RhythmPatternAnalyze_init(0.0, 0, 0.0001, fs)`.
    pub fn new(fs: f64) -> Self {
        let w = prewarp(1256.6370614359173, fs);
        let low = if near(fs, 5512.49, 5512.51) || near(fs, 5999.99, 6000.01) || near(fs, 3999.99, 4000.01) {
            let a = CFilter::low2(w, 1.8477590650225735, fs);
            let b = CFilter::low2(w, 0.7653668647301797, fs);
            vec![a, b, a, b]
        } else if near(fs, 1378.115, 1378.135) || near(fs, 1499.99, 1500.01) || near(fs, 999.99, 1000.01) {
            vec![CFilter::low2(w, SQRT2, fs)]
        } else {
            Vec::new()
        };
        let w = prewarp(62.83185307179586, fs);
        let fs_lp = if fs != 0.0 { fs } else { 5512.5 };
        Self {
            last: 0.0,
            offset: 0,
            runmax: 0.0001,
            low,
            smooth: CFilter::low2(w, SQRT2, fs_lp),
        }
    }

    /// `RhythmPatternAnalyze(x, bpm, fb_ms, 1, fs, 1, pattern)`: for each bar after the first
    /// beat, 16 steps; a step is 1 when its envelope rise is among the bar's strongest (up to
    /// 6 picks). `None` where the binary returns 0.
    pub fn analyze(&mut self, x: &[f64], bpm: f64, fb_ms: i32, fs: f64) -> Option<Vec<u8>> {
        let p = ((fs * 60.0) / bpm + 0.5) as i32;
        if p == 0 {
            return None;
        }
        let len = x.len() as i32;
        let off = (fb_ms as f64 * 0.001).mul_add(fs, 0.5) as i32;
        // param_4 = 1: start = (1 − 1)·P + off.
        let start = off;
        let nraw = (len - start) / p;
        let mut env = x.to_vec();
        for f in &mut self.low {
            f.filter2(&mut env);
        }
        for v in &mut env {
            *v = v.abs();
        }
        self.smooth.filter2(&mut env);
        let pf = ((fs * 60.0) / bpm + 0.5).trunc();
        let barf = pf * 4.0;
        let bar = barf as i32;
        let nb = (nraw / 4) - i32::from(len < start + (nraw / 4) * bar);
        if nb < 1 {
            self.last = env[(len - 1) as usize];
            return Some(vec![0]);
        }
        let mut pattern = vec![0u8; nb as usize * 16];
        let q = pf * 0.25;
        let win = (q + 0.5) as i32;
        let s1 = start + 1;
        let found = if nb < 2 { 0 } else { preroll(&env, start, bar, nb, p, pf, barf, win) };
        let end = start + nraw * p;
        self.offset = found;
        // param_6 = 1: the call's own offset is used.
        let u6 = found;
        // First difference, backwards; the first value against the previous call's last.
        for i in (1..env.len()).rev() {
            env[i] -= env[i - 1];
        }
        env[0] -= self.last;
        let span = end - s1;
        if u6 < s1 && end - u6 <= len {
            let from = (s1 - 1 - u6) as usize;
            env = env[from..from + (span + 1) as usize].to_vec();
        } else if end - 1 - u6 <= len && s1 < u6 {
            let head = env[..(end - 1 - u6) as usize].to_vec();
            if len < span {
                env = vec![0.0; span as usize];
            }
            let pad = (u6 - start) as usize;
            env[..pad].fill(0.0);
            env[pad..pad + head.len()].copy_from_slice(&head);
        }
        let mut runmax = 0.0001f64;
        for b in 0..nb {
            let base = (b as f64).mul_add(barf, 1.0);
            let mut v = [0.0f64; 16];
            for (k, slot) in v.iter_mut().enumerate() {
                let at = base + (k / 4) as f64 * pf + (k % 4) as f64 * q;
                let lo = at as i32 - 1;
                *slot = window_max(&env, lo, win);
            }
            let thr_run = runmax * 0.178;
            let mut bar_max = 0.0;
            for pick in 0..6 {
                let (m, idx) = argmax16(&v);
                if pick == 0 {
                    bar_max = m;
                }
                if m > 0.0001 && m > bar_max * 0.596 && m > thr_run {
                    pattern[b as usize * 16 + idx] = 1;
                    v[idx] = 0.0;
                }
            }
            if runmax <= bar_max {
                runmax = bar_max;
            }
        }
        self.runmax = runmax;
        Some(pattern)
    }
}

/// `max_val` over `env[lo .. lo + n]` (the binary links a window of the array).
fn window_max(env: &[f64], lo: i32, n: i32) -> f64 {
    let lo = lo.max(0) as usize;
    let hi = (lo + n.max(0) as usize).min(env.len());
    if lo >= hi {
        return 0.0;
    }
    mat::max_val(&env[lo..hi])
}

/// The bar's argmax as compiled (@0x1022300a0): an `fmaxnm` chain with a strict-less index
/// select, i.e. the first largest of the 16 values.
fn argmax16(v: &[f64; 16]) -> (f64, usize) {
    let mut m = v[0];
    let mut idx = 0;
    for (k, &x) in v.iter().enumerate().skip(1) {
        if m < x {
            idx = k;
        }
        m = m.max(x);
    }
    (m, idx)
}

/// The pre-roll search (@0x10222f5c4 … 0x10222f9f4): the strongest envelope peaks after the
/// first bar, looking back from each for the onset valley; returns how many samples before
/// the bar line the rise starts (0 when none qualifies within 8 tries).
#[allow(clippy::too_many_arguments)]
fn preroll(env: &[f64], start: i32, bar: i32, nb: i32, p: i32, pf: f64, barf: f64, win: i32) -> i32 {
    let s1 = start + 1;
    let n = (nb - 1) * bar;
    let from = (s1 + bar - 1) as usize;
    let mut seg = env[from..from + n as usize].to_vec();
    let mut idx = [0i32; 8];
    let mut i41 = 0i32;
    let mut peak = 0.0f64;
    let mut valley_at = 0i32;
    let p5 = p * 5;
    let at = |i: i32| env[i as usize];
    for l in 0..8 {
        if l != 0 {
            let prev = idx[l - 1];
            if prev <= n {
                seg[(prev - 1) as usize] = 0.0;
            }
        }
        let (mx, i) = mat::max_idx(&seg);
        idx[l] = i as i32 + 1;
        let bi = if barf == 0.0 {
            0xffff
        } else {
            ((idx[l] as i64 as f64 * 0.25) * (1.0 / pf)) as i32
        };
        let i7 = bar + bar * bi;
        let i22 = i7 + start;
        let edge = at(i22.min(len_i32(env)) - 1);
        if !(mx * 0.2 < edge && i7 < n - bar) {
            continue;
        }
        let x0 = at(i22);
        let x1 = at(i22 + 1);
        let mut reached = false;
        if edge <= x0 {
            if x0 <= x1 && x1 <= at(i22 + 2) {
                // Rising: the first fall within a quarter beat marks the peak.
                for u in 0..win {
                    if at(i22 + u + 1) - at(i22 + u) < 0.0 {
                        i41 = u + 1 + i7;
                        peak = at(start - 1 + i41);
                        break;
                    }
                }
                reached = true;
            }
        } else if x1 < x0 && at(i22 + 2) < x1 {
            if win > 0 {
                // Falling: walk back to the last rise.
                let mut k = 0i32;
                loop {
                    if !(0.0 <= at(i22 + k - 3) - at(i22 + k - 2)) {
                        i41 = k - 1 + i7;
                        peak = at(start - 1 + i41);
                        break;
                    }
                    k -= 1;
                    if k == -win {
                        break;
                    }
                }
            }
            reached = true;
        }
        if !reached || !(mx * 0.595662 < peak) {
            continue;
        }
        let lo = (start + i7 - 1) as usize;
        let mn = mat::min_val(&env[lo..(lo + bar as usize + 1).min(env.len())]);
        if p5 <= 3 {
            continue;
        }
        let thr = (mn + mx) * 0.5;
        let mut level = mx;
        let mut low = false;
        let mut run = 0i32;
        let mut i31 = start - 2;
        let mut i35 = start - 3;
        let mut i26 = -1i32;
        let mut count = p5 / 4;
        loop {
            let mut next = level;
            if low {
                if at(i41 + i31) <= level * 1.12 {
                    run += 1;
                    if (pf * 0.0625) as i32 <= run {
                        if level == 0.0 {
                            return 0;
                        }
                        if level * 2.11 < peak {
                            let r = (((i7 as f64) + 0.5) - valley_at as f64) as i32;
                            return if s1 < r { s1 } else { r };
                        }
                        break;
                    }
                    low = true;
                } else {
                    run = 0;
                    low = false;
                }
            } else {
                let v = at(i41 + i31);
                if v <= at(i41 + i35) && v < thr {
                    valley_at = i41 + i26;
                    low = true;
                    next = v;
                } else {
                    low = false;
                }
            }
            i35 -= 1;
            i31 -= 1;
            i26 -= 1;
            count -= 1;
            level = next;
            if count == 0 {
                break;
            }
        }
    }
    0
}

fn len_i32(x: &[f64]) -> i32 {
    x.len() as i32
}
