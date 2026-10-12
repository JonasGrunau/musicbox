//! Stage C's rhythm patterns: `RhythmPatternAnalyze_HP` @0x1004bcae8 (low band, 6 picks a
//! bar) and `RhythmPatternAnalyze2_HP` @0x10147e4c8 (110–200 Hz band, 12 picks), the
//! block-wise twins of `RhythmPatternAnalyze` (`analysis-phrase.md` § 6.3).

use super::filter::{CFilter, SQRT2, prewarp};
use super::mat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `RhythmPatternAnalyze_HP` (+0x1598 … +0x16d0).
    Low,
    /// `RhythmPatternAnalyze2_HP` (+0x16f0 … +0x1940).
    Band,
}

#[derive(Debug, Clone)]
pub struct RhythmHp {
    kind: Kind,
    low: Vec<CFilter>,
    smooth: CFilter,
    band: [CFilter; 4],
    band_smooth: CFilter,
    /// +0x16b8 / +0x1928: the previous block's last envelope value.
    last: f64,
    /// +0x16c0 / +0x1930: the pre-roll offset of the first block.
    offset: i32,
    /// +0x16c8 / +0x1938: the running bar maximum.
    runmax: f64,
}

fn near(fs: f64, lo: f64, hi: f64) -> bool {
    lo < fs && fs < hi
}

impl RhythmHp {
    /// `RhythmPatternAnalyze{,2}_HP_init(0.0, 0, 0.0001, fs)`.
    pub fn new(kind: Kind, fs: f64) -> Self {
        let w = prewarp(1256.6370614359173, fs);
        let low = if near(fs, 5512.49, 5512.51)
            || near(fs, 5999.99, 6000.01)
            || near(fs, 3999.99, 4000.01)
        {
            let a = CFilter::low2(w, 1.8477590650225735, fs);
            let b = CFilter::low2(w, 0.7653668647301797, fs);
            vec![a, b, a, b]
        } else if near(fs, 1378.115, 1378.135)
            || near(fs, 1499.99, 1500.01)
            || near(fs, 999.99, 1000.01)
        {
            vec![CFilter::low2(w, SQRT2, fs)]
        } else {
            Vec::new()
        };
        let ws = prewarp(62.83185307179586, fs);
        let smooth_fs = match kind {
            Kind::Low if fs == 0.0 => 5512.5,
            _ => fs,
        };
        let smooth = CFilter::low2(ws, SQRT2, smooth_fs);
        let wb = prewarp(691.1503837897545, fs);
        let hi = |k: f64| CFilter::make2(1.0, 0.0, 0.0, 1.0, wb * k, wb * wb, fs);
        let (h1, h2) = (hi(1.8477590650225735), hi(0.7653668647301797));
        Self {
            kind,
            low,
            smooth,
            band: [h1, h2, h1, h2],
            band_smooth: CFilter::low2(ws, SQRT2, fs),
            last: 0.0,
            offset: 0,
            runmax: 0.0001,
        }
    }

    /// One block: `x` at fs/8, `fb_ms`/`bar` of the block's first beat, `block` = +0xb78,
    /// `beats` the block's 1-based beat starts (the whole array), `n` its beat count. Returns
    /// the pattern bytes (16 per bar).
    #[allow(clippy::too_many_arguments)]
    pub fn analyze(
        &mut self,
        x: &[f64],
        bpm: f64,
        fb_ms: i32,
        bar: i32,
        fs: f64,
        block: i32,
        beats: &[i32],
        n: usize,
    ) -> Option<Vec<u8>> {
        let p = (fs * 60.0) / bpm;
        let len = x.len() as i32;
        let endb = beats[n - 1];
        let e2 = p as i32 + endb - 1;
        let end = if e2 <= len { e2 } else { endb };
        let nbr = (n as i32 - bar + 1) / 4;
        let mut runmax = if block < 2 { 0.0001 } else { self.runmax };
        let old_offset = self.offset;
        if self.kind == Kind::Band {
            self.offset = 0;
        }
        let mut lowpassed = x.to_vec();
        for f in &mut self.low {
            f.filter2(&mut lowpassed);
        }
        let mut env: Vec<f64> = lowpassed.iter().map(|v| v.abs()).collect();
        self.smooth.filter2(&mut env);
        let off = (fb_ms as f64 * 0.001).mul_add(fs, 0.5) as i32;
        let start = p.mul_add(f64::from(bar - 1), f64::from(off + 1)) as i32;
        let s0 = start - 1;
        let barlen = (p * 4.0) as i32;
        let nb = nbr - i32::from(len < s0 + nbr * barlen);
        if nb < 1 {
            self.last = env[(len - 1) as usize];
            self.offset = old_offset;
            self.runmax = runmax;
            return Some(vec![0]);
        }
        let win = (p * 0.25) as i32;
        let found = if nb < 2 {
            0
        } else {
            self.preroll(&env, p, start, barlen, nb, win)
        };
        self.offset = found;
        let u6 = if block < 2 { found } else { old_offset };
        if self.kind == Kind::Band {
            let mut y = lowpassed;
            for f in &mut self.band {
                f.filter2(&mut y);
            }
            env = y.iter().map(|v| v.abs()).collect();
            self.band_smooth.filter2(&mut env);
        }
        let prev = self.last;
        self.last = env[(len - 1) as usize];
        for i in (1..env.len()).rev() {
            env[i] -= env[i - 1];
        }
        env[0] -= prev;
        let span = end - start;
        if u6 < start && end - u6 <= len {
            let from = (start - 1 - u6) as usize;
            env = env[from..from + (span + 1) as usize].to_vec();
        } else if end - 1 - u6 <= len && start < u6 {
            let head = env[..(end - 1 - u6) as usize].to_vec();
            if len < span {
                env = vec![0.0; span as usize];
            }
            let pad = (u6 - s0) as usize;
            if u6 - s0 > 0 {
                env[..pad].fill(0.0);
            }
            env[pad..pad + head.len()].copy_from_slice(&head);
        }
        let mut pattern = vec![0u8; nb as usize * 16];
        let q = p * 0.25;
        let offs = [0.0, q, p * 0.5, p * 0.75];
        let (picks, ratio) = match self.kind {
            Kind::Low => (6, 0.596),
            Kind::Band => (12, 0.5),
        };
        'bars: for b in 0..nb {
            let ib = bar + 4 * b;
            let mut v = [0.0f64; 16];
            for j in 0..4 {
                if (beats.len() as i32) < ib + j {
                    break 'bars;
                }
                let base = f64::from(beats[(ib - 1 + j) as usize] - start) + 0.5;
                for (k, o) in offs.iter().enumerate() {
                    let at = (base + o) as i32;
                    if (env.len() as i32) < at + win {
                        break 'bars;
                    }
                    let w = &env[at as usize..(at + win) as usize];
                    v[4 * j as usize + k] = if w.is_empty() { 0.0 } else { mat::max_val(w) };
                }
            }
            let thr_run = runmax * 0.178;
            let mut bar_max = 0.0;
            for pick in 0..picks {
                let (m, idx) = argmax16(&v);
                if pick == 0 {
                    bar_max = m;
                }
                if m > 0.0001 && m > bar_max * ratio && m > thr_run {
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

    /// The pre-roll search (as `RhythmPatternAnalyze`'s, on actual-beat geometry). The band
    /// variant only follows rising envelopes.
    fn preroll(&self, env: &[f64], p: f64, start: i32, barlen: i32, nb: i32, win: i32) -> i32 {
        let len = env.len() as i32;
        let s0 = start - 1;
        let n = (nb - 1) * barlen;
        let from = (start + barlen - 1) as usize;
        let mut seg = env[from..from + n as usize].to_vec();
        let at = |i: i32| env[i as usize];
        let mut idx = [0i32; 8];
        let mut i4 = 0i32;
        let mut i8 = 0i32;
        let mut peak = 0.0f64;
        for l in 0..8 {
            if l != 0 && idx[l - 1] <= n {
                seg[(idx[l - 1] - 1) as usize] = 0.0;
            }
            let (mx, i) = mat::max_idx(&seg);
            idx[l] = i as i32 + 1;
            let bi = if p * 4.0 == 0.0 {
                0xffff
            } else {
                ((idx[l] as i64 as f64 * 0.25) * (1.0 / p)) as i32
            };
            let i3 = barlen + bi * barlen;
            let l23 = i3 + s0;
            let edge = at(l23.min(len) - 1);
            if edge <= mx * 0.2 || n - barlen <= i3 {
                continue;
            }
            let (x0, x1, x2) = (at(l23), at(l23 + 1), at(l23 + 2));
            if edge <= x0 {
                if !(x0 <= x1 && x1 <= x2) {
                    continue;
                }
                for u in 0..win {
                    let a = s0 + i3 + u;
                    if at(a + 1) - at(a) < 0.0 {
                        i4 = u + 1 + i3;
                        peak = at(start - 2 + i4);
                        break;
                    }
                }
            } else {
                if self.kind == Kind::Band || x0 <= x1 || x1 <= x2 {
                    continue;
                }
                for u in 0..win {
                    let a = start - 4 + i3 - u;
                    if at(a) - at(a + 1) < 0.0 {
                        i4 = i3 - u - 1;
                        peak = at(start - 2 + i4);
                        break;
                    }
                }
            }
            let count = (p * 1.25) as i32;
            if !(mx * 0.595662 < peak) || count <= 0 {
                continue;
            }
            let lo = (s0 + i3 - 1) as usize;
            let mn = mat::min_val(&env[lo..(lo + barlen as usize + 1).min(env.len())]);
            let thr = (mn + mx) * 0.5;
            let mut level = mx;
            let mut low = false;
            let mut run = 0i32;
            let mut i20 = i4;
            let mut left = count;
            loop {
                i20 -= 1;
                let mut next = level;
                if low {
                    if at(start + i20 - 2) <= level * 1.12 {
                        run += 1;
                        if (p * 0.0625) as i32 <= run {
                            if level == 0.0 {
                                return 0;
                            }
                            if 2.11 < peak / level {
                                let r = (((i3 as f64) + 0.5) - i8 as f64) as i32;
                                return if start <= r { start } else { r };
                            }
                            break;
                        }
                        low = true;
                    } else {
                        run = 0;
                        low = false;
                    }
                } else {
                    let v = at(start + i20 - 2);
                    if v <= at(start + i20 - 3) && v < thr {
                        low = true;
                        i8 = i20;
                        next = v;
                    } else {
                        low = false;
                    }
                }
                left -= 1;
                level = next;
                if left == 0 {
                    break;
                }
            }
        }
        0
    }
}

/// The bar's argmax as compiled: an `fmaxnm` chain, first largest index.
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
