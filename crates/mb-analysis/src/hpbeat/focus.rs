//! BALib v2's Focus pass (`BeatAnalyzer_2_0::Analyzer::analyzeFocus` @0x10116c050): turns the
//! neural beat list into rekordbox's high-precision grid. It finds stretches where the beat
//! intervals jump (missed, doubled or triplet beats), rebuilds them as evenly spaced beats, and
//! then smooths the result. Spec: `analysis-beats-hp.md` § 6.
//!
//! The port keeps the binary's arithmetic: interval and time rounding (`frinta`/`frintz`), the
//! summation order of its vectorised loops, and fused multiply-adds where the binary uses them.

// Float tests follow the binary's branches; `!(a < b)` and `a >= b` differ on NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]
#![allow(clippy::too_many_arguments)]

use super::madmom::ListBeat;
use crate::balib2::adjust::UnitBeatAdjust;

mod constant;
mod vote;

/// `peakss::BeatInfo` (24 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BeatInfo {
    pub pos: i32,
    pub beats: f32,
    /// Beat length in ms.
    pub tempo: f32,
    /// Beat in bar.
    pub stat: i32,
    pub band: i32,
    pub flag: u8,
}

impl BeatInfo {
    /// A beat the pass inserts: position only, band −1.
    fn new(pos: i32) -> Self {
        Self { pos, beats: 0.0, tempo: 0.0, stat: 0, band: -1, flag: 0 }
    }
}

/// A BPM and beat list at one point of the constant path (development aid).
pub type Stage = (f32, Vec<BeatInfo>);

/// The analyser state `analyzeFocus` reads.
#[derive(Debug, Clone, Copy)]
pub struct FocusInput {
    /// Analysis rate (Analyzer +0x14).
    pub rate: i32,
    /// Samples fed (`L − 1`, +0x1c).
    pub total: i32,
    /// Dynamic ("varied") analyser (+0x2c).
    pub varied: bool,
    /// BALib's tempo-change flag (+0x2d): always set in Dynamic mode.
    pub hendou: bool,
    /// `getOffsetCalc` (+0x50): start of the madmom window.
    pub offset: i32,
}

/// The beat vector with an optional trace of its edits (`INS`/`MOV`, as the dev harness logs
/// rekordbox's own vector).
pub struct Beats {
    pub v: Vec<BeatInfo>,
    pub trace: Option<Vec<String>>,
}

impl Beats {
    fn len(&self) -> usize {
        self.v.len()
    }
    fn insert(&mut self, at: usize, b: BeatInfo) {
        if let Some(t) = &mut self.trace {
            t.push(format!(
                "INS {at} n={} pos={} beats={} tempo={} stat={} band={} flag={}",
                self.v.len(),
                b.pos,
                b.beats,
                b.tempo,
                b.stat,
                b.band,
                b.flag
            ));
        }
        self.v.insert(at, b);
    }
    /// Removes `[from, to)` in one move.
    fn erase_range(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        if let Some(t) = &mut self.trace {
            t.push(format!("MOV dst={from} src={to} n={}", self.v.len()));
        }
        self.v.drain(from..to);
    }
    fn erase(&mut self, at: usize) {
        self.erase_range(at, at + 1);
    }
}

/// Sum of `a` the way the binary's 8-wide loops add it: four two-lane accumulators over the
/// first `len & !7` values, reduced lane-wise, then the rest one by one.
pub(crate) fn neon_sum8(a: &[f64]) -> f64 {
    let n8 = a.len() & !7;
    let mut s = 0.0;
    if n8 > 0 {
        let mut acc = [[0.0f64; 2]; 4];
        for c in a[..n8].chunks_exact(8) {
            for (k, lane) in acc.iter_mut().enumerate() {
                lane[0] += c[2 * k];
                lane[1] += c[2 * k + 1];
            }
        }
        let l0 = ((acc[0][0] + acc[1][0]) + acc[2][0]) + acc[3][0];
        let l1 = ((acc[0][1] + acc[1][1]) + acc[2][1]) + acc[3][1];
        s = l0 + l1;
    }
    for &x in &a[n8..] {
        s += x;
    }
    s
}

/// Sum of squared deviations from `m`, 8-wide with fused multiply-adds like `neon_sum8`.
pub(crate) fn neon_sumsq8(a: &[f64], m: f64) -> f64 {
    let n8 = a.len() & !7;
    let mut s = 0.0;
    if n8 > 0 {
        let mut acc = [[0.0f64; 2]; 4];
        for c in a[..n8].chunks_exact(8) {
            for (k, lane) in acc.iter_mut().enumerate() {
                let (d0, d1) = (c[2 * k] - m, c[2 * k + 1] - m);
                lane[0] = d0.mul_add(d0, lane[0]);
                lane[1] = d1.mul_add(d1, lane[1]);
            }
        }
        let l0 = ((acc[0][0] + acc[1][0]) + acc[2][0]) + acc[3][0];
        let l1 = ((acc[0][1] + acc[1][1]) + acc[2][1]) + acc[3][1];
        s = l0 + l1;
    }
    for &x in &a[n8..] {
        let d = x - m;
        s = d.mul_add(d, s);
    }
    s
}

/// Ratio windows the run fixes accept for a neighbouring beat: (1.07, 1.425) or (1.575, 1.8).
fn near_ratio(r: f64) -> bool {
    (1.07 < r && r < 1.424_999_999_999_999_8) || (1.575_000_000_000_000_2 < r && r < 1.8)
}

/// The per-pass state of `analyzeFocus`.
struct Pass {
    /// Beat times in s, rounded to ms.
    t: Vec<f64>,
    /// Interval / reference interval.
    x: Vec<f64>,
    /// `max(x, 1/x)`.
    r: Vec<f64>,
    /// Reference interval in ms.
    ref_ms: f64,
}

/// Interval-ratio sub-ranges inside one irregular run, per category: half (≈0.5), two thirds
/// (≈0.667), one and a half (≈1.5) and double (≈2.0); first and last index, −1 when absent.
#[derive(Debug, Clone, Copy)]
struct Cats {
    first: [i32; 4],
    last: [i32; 4],
}

/// Category windows on `x`.
const CAT_LO: [f64; 4] = [0.4425, 0.59, 1.327_500_000_000_000_1, 1.77];
const CAT_HI: [f64; 4] = [0.5575, 0.743_333_333_333_333_3, 1.672_499_999_999_999_9, 2.23];

pub struct Focus {
    pub inp: FocusInput,
    pub b: Beats,
    /// Analyzer +0x48: the tempo (BPM).
    pub bpm: f32,
    /// The anchor beat chosen in the first pass: position and beat in bar.
    anchor_pos: i32,
    anchor_stat: i32,
    starts: [i32; 128],
    ends: [i32; 128],
    /// Patterns met that are not ported yet (development aid).
    pub unported: Vec<&'static str>,
    /// Run fixes taken, in order (development aid).
    pub hits: Vec<&'static str>,
    /// The madmom list did not start with beat 4 among its first four (main loop condition).
    top_is_not_four: bool,
    /// `analyzeFocus`'s out flag: a reference tempo above 180 was halved (constant path).
    pub halved: bool,
    /// The constant path's BPM and beats as they went into `UnitBeatAdjust::analyze`, and as
    /// they came out (development aid).
    pub adjust_io: Option<(Stage, Stage)>,
    /// Beats the constant path cut from the end of the vector (still in its memory).
    ghost: Vec<BeatInfo>,
}

impl Focus {
    /// Times in s of the current beats, rounded to the ms (`frinta`); the binary's vector loop
    /// (pairs) scales by `1000/rate`, the scalar tail by `1000` then `1/rate`.
    fn times(&self) -> Vec<f64> {
        let n = self.b.len();
        let rate = f64::from(self.inp.rate);
        let vec_n = if n < 3 { 0 } else { n - if n & 1 != 0 { 1 } else { 2 } };
        let scale = 1000.0 / rate;
        let inv = 1.0 / rate;
        self.b
            .v
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let p = f64::from(b.pos);
                if i < vec_n { (p * scale).round() * 0.001 } else { (p * 1000.0 * inv).round() * 0.001 }
            })
            .collect()
    }

    /// The tempo estimate of a pass: the mean interval of the first stable stretch (or of the
    /// first beats), checked against the madmom tempo. Returns the reference interval in ms.
    fn estimate(&mut self, iv: &[f64], mad_bpm: i32) -> f64 {
        let n = self.b.len() as i32;
        let mad = f64::from(mad_bpm);
        let skip = if n < 0x16 { if n - 1 == 0 || n < 1 { 0 } else { n - 2 } } else { 0x14 };
        let min_run = n.min(0x25);
        let m = n - skip;
        let nd = m - 1;
        let dd: Vec<f64> = (0..nd.max(0) as usize).map(|k| (iv[k + 1] - iv[k]).abs()).collect();
        // Three scans for a stretch of at least `min_run` intervals whose neighbours differ by
        // at most 11, 20 and 30 ms; the stretch must end at a larger jump.
        let mut found: Option<(f64, i32)> = None;
        if nd > 0 && m >= 1 {
            'search: for thr in [11.0, 20.0, 30.0] {
                let (mut sum, mut count) = (0.0, 0);
                for k in 0..nd as usize {
                    if thr < dd[k] {
                        if min_run <= count {
                            found = Some((sum, count));
                            break 'search;
                        }
                        sum = 0.0;
                        count = 0;
                    } else {
                        sum += iv[k];
                        count += 1;
                    }
                }
            }
        }
        let avg = match found {
            Some((sum, count)) if count != 0 => sum / f64::from(count),
            Some(_) => self.median_interval(iv, mad, m, nd),
            None if n < 1 => self.median_interval(iv, mad, m, nd),
            None => {
                // the first min(n, 37) intervals, in two accumulators (even/odd)
                let cnt = min_run.max(1) as usize;
                let pairs = if cnt < 2 { 0 } else { cnt & 0x7fff_fffe };
                let mut s = 0.0;
                if pairs > 0 {
                    let (mut e, mut o) = (0.0, 0.0);
                    for k in (0..pairs).step_by(2) {
                        e += iv[k];
                        o += iv[k + 1];
                    }
                    s = e + o;
                }
                for &v in &iv[pairs..cnt] {
                    s += v;
                }
                if min_run == 0 { self.median_interval(iv, mad, m, nd) } else { s / f64::from(min_run) }
            }
        };
        let bpm = 60000.0 / avg;
        let ok = mad * 0.008 <= bpm && bpm <= mad * 0.0125;
        self.bpm = if ok { bpm as f32 } else { mad_bpm as f32 * 0.01 };
        if ok { avg } else { (60.0 / mad) * 100000.0 }
    }

    /// Fallback interval: the median of the intervals (only reached for degenerate input).
    fn median_interval(&self, iv: &[f64], mad: f64, m: i32, nd: i32) -> f64 {
        let mut v = (60.0 / mad) * 1000.0;
        if m > 1 {
            let mut c: Vec<f64> = iv[..nd.max(1) as usize].to_vec();
            if m > 2 {
                for i in 0..(m - 2) as usize {
                    for j in i + 1..nd as usize {
                        if c[i] < c[j] {
                            c.swap(i, j);
                        }
                    }
                }
            }
            v = if nd & (0x8000_0001u32 as i32) == 1 {
                c[(nd >> 1) as usize]
            } else {
                let h = if nd < 0 { m } else { nd } >> 1;
                (c[h as usize] + c[(h - 1) as usize]) * 0.5
            };
        }
        v
    }
}

/// Statistics of one long run and of its four category sub-ranges (index 0 = whole run, then
/// half, two thirds, one and a half, double).
#[derive(Debug, Clone, Copy, Default)]
struct RunStats {
    /// Mean `x` of the whole run.
    mean: f64,
    /// Variance of `x` (whole run and categories).
    var: [f64; 5],
    /// Drift: mean of three end-to-end differences of `x`.
    drift: [f64; 5],
}

/// The three drift terms of a range `[first, last]` (length `d = last − first`): the binary
/// combines them as `(j3 + j1) + j2`, except for the double category.
fn drift_terms(x: &[f64], first: i32, last: i32, n: i32) -> (f64, f64, f64) {
    let d = last - first;
    let (mut j1, mut j2, mut j3) = (0.0, 0.0, 0.0);
    if 2 < d && first + 1 < n && 0 < last {
        j1 = (x[(last - 1) as usize] - x[(first + 1) as usize]).abs() / f64::from(d - 2);
    }
    if d - 3 != 0 && 2 < d && first + 2 < n && 1 < last {
        let dd = f64::from(d - 3);
        j2 = (x[(last - 2) as usize] - x[(first + 1) as usize]).abs() / dd;
        j3 = (x[(last - 1) as usize] - x[(first + 2) as usize]).abs() / dd;
    }
    (j1, j2, j3)
}

impl Focus {
    /// `analyzeFocus` @0x10116c050.
    pub fn run(inp: FocusInput, balib_beats: i32, balib_bpm100: i32, mad: &[ListBeat], mad_bpm: i32, trace: bool) -> Option<Focus> {
        Self::run_with(inp, balib_beats, balib_bpm100, mad, mad_bpm, trace, None)
    }

    /// `analyzeFocus` with the analyser's `UnitBeatAdjust`, which the constant path needs.
    pub fn run_with(inp: FocusInput, balib_beats: i32, balib_bpm100: i32, mad: &[ListBeat], mad_bpm: i32, trace: bool, adjust: Option<&mut UnitBeatAdjust>) -> Option<Focus> {
        if balib_beats == 0 || balib_bpm100 == 0 || mad.is_empty() || mad_bpm == 0 || inp.rate == 0 {
            return None;
        }
        let mut f = Focus {
            inp,
            b: Beats { v: Vec::with_capacity(mad.len()), trace: trace.then(Vec::new) },
            bpm: balib_bpm100 as f32 * 0.01,
            anchor_pos: -1,
            anchor_stat: 0,
            starts: [0; 128],
            ends: [0; 128],
            unported: Vec::new(),
            hits: Vec::new(),
            top_is_not_four: false,
            halved: false,
            adjust_io: None,
            ghost: Vec::new(),
        };
        // the largest beat number among the first four madmom beats
        let top = mad.iter().take(4).map(|b| b.num_bar).fold(0, |a, b| if a <= b { b } else { a }).max(0);
        let mut tempo = 0.0f32;
        for m in mad {
            if m.bpm != 0 {
                tempo = 6e6f32 / m.bpm as f32;
            }
            f.b.v.push(BeatInfo { pos: m.pos, beats: 0.0, tempo, stat: m.num_bar, band: -1, flag: 0 });
        }
        f.top_is_not_four = top != 4;
        if f.inp.hendou || top != 4 {
            for _ in 0..2 {
                f.pass(mad_bpm);
            }
            f.post(mad_bpm);
        } else if let Some(a) = adjust {
            f.constant(a);
        } else {
            f.unported.push("constant path");
        }
        Some(f)
    }

    fn pass(&mut self, mad_bpm: i32) {
        let n = self.b.len() as i32;
        let t = self.times();
        // interval ms: trunc(Δt·1000 + 0.5) (fused)
        let mut iv = vec![0.0f64; n.max(0) as usize];
        for i in 0..(n - 1).max(0) as usize {
            iv[i] = (t[i + 1] - t[i]).mul_add(1000.0, 0.5).trunc();
        }
        let ref_ms = self.estimate(&iv, mad_bpm);
        if n < 2 {
            return;
        }
        let inv = 1.0 / ref_ms;
        let mut x = vec![0.0f64; n as usize];
        let mut r = vec![0.0f64; n as usize];
        for i in 0..(n - 1) as usize {
            let v = iv[i] * inv;
            x[i] = v;
            r[i] = if v.abs() > f64::EPSILON { if 1.0 <= v { v } else { 1.0 / v } } else { 0.0 };
        }
        x[n as usize - 1] = x[n as usize - 2];
        r[n as usize - 1] = r[n as usize - 2];
        // runs of at least two ratios beyond 1.18
        self.starts = [0; 128];
        self.ends = [0; 128];
        let len = n as usize;
        let (mut i, mut k) = (0usize, 0usize);
        loop {
            if 1.18 < r[i] {
                let mut j = i + 1;
                if 1.18 < r[j] {
                    self.starts[k] = i as i32;
                    if j < len {
                        let mut jj = i + 2;
                        loop {
                            j = jj;
                            if len <= j {
                                break;
                            }
                            jj = j + 1;
                            if !(1.18 < r[j]) {
                                break;
                            }
                        }
                    }
                    i = j;
                    self.ends[k] = i as i32 - 1;
                    k += 1;
                }
            }
            i += 1;
            if !(i < (n - 1) as usize && k < 0x80) {
                break;
            }
        }
        let nruns = self.ends.iter().position(|&e| e < 1).unwrap_or(128);
        if self.anchor_pos < 0 {
            self.set_anchor(nruns, n);
        }
        let pass = Pass { t, x, r, ref_ms };
        let cats = self.categories(&pass, nruns);
        self.fix_runs(&pass, cats, nruns, n);
    }

    /// First pass only: the beat in the middle(ish) of the longest regular stretch becomes the
    /// anchor. As in the binary, the index is half the stretch length, not its midpoint.
    fn set_anchor(&mut self, nruns: usize, n: i32) {
        let (lo, hi) = if nruns == 0 {
            (0, n - 1)
        } else {
            let mut hi = self.starts[0] - 1;
            let mut best = self.starts[0];
            if self.starts[0] < 1 {
                best = 0;
                hi = n - 1;
            }
            let mut lo = 0;
            if nruns >= 2 {
                for j in 0..nruns - 1 {
                    let gap = self.starts[j + 1] - self.ends[j];
                    let prev = best;
                    if prev <= gap {
                        best = gap;
                    }
                    if prev < gap {
                        lo = self.ends[j] + 1;
                        hi = self.starts[j + 1] - 1;
                    }
                }
            }
            let last = self.ends[nruns - 1];
            if last < n - 1 && best < (n - 1) - last {
                lo = last + 1;
                hi = n - 1;
            }
            (lo, hi)
        };
        let idx = (hi - lo) / 2;
        let b = self.b.v[idx as usize];
        self.anchor_pos = b.pos;
        self.anchor_stat = b.stat;
    }

    /// Category sub-ranges of every run.
    fn categories(&self, p: &Pass, nruns: usize) -> Vec<Cats> {
        let x = &p.x;
        let mut out = Vec::with_capacity(nruns);
        for k in 0..nruns {
            let mut c = Cats { first: [-1; 4], last: [-1; 4] };
            let (s, e) = (self.starts[k], self.ends[k]);
            if s <= e {
                for i in s..=e {
                    let v = x[i as usize];
                    let prev = if i >= 1 { x[i as usize - 1] } else { f64::NAN };
                    let inside = |cat: usize, v: f64| CAT_LO[cat] < v && v < CAT_HI[cat];
                    let open = |cat: usize, c: &mut Cats| {
                        if c.first[cat] == -1 {
                            c.first[cat] = i;
                        }
                        if e <= i && c.last[cat] == -1 {
                            c.last[cat] = i;
                        }
                    };
                    if inside(0, v) {
                        open(0, &mut c);
                        continue;
                    }
                    if c.first[0] >= 0 && c.last[0] == -1 && inside(0, prev) {
                        c.last[0] = i - 1;
                        continue;
                    }
                    if inside(1, v) {
                        open(1, &mut c);
                        continue;
                    }
                    if c.first[1] >= 0 && c.last[1] == -1 && inside(1, prev) {
                        c.last[1] = i - 1;
                        continue;
                    }
                    if inside(2, v) {
                        open(2, &mut c);
                        continue;
                    }
                    if c.first[2] >= 0 && c.last[2] == -1 && inside(2, prev) {
                        c.last[2] = i - 1;
                        continue;
                    }
                    if inside(3, v) {
                        open(3, &mut c);
                    } else if c.first[3] >= 0 && c.last[3] == -1 && inside(3, prev) {
                        c.last[3] = i - 1;
                    }
                }
            }
            out.push(c);
        }
        out
    }
}

impl Focus {
    /// Rebuilds the irregular runs, last to first. `nv` is the beat count the binary keeps in a
    /// register across runs (some branches leave another value in it).
    fn fix_runs(&mut self, p: &Pass, cats: Vec<Cats>, nruns: usize, n: i32) {
        let mut nv = n;
        for k in (0..nruns).rev() {
            let (s, e) = (self.starts[k], self.ends[k]);
            let len1 = e - s;
            if len1 < 1 {
                continue;
            }
            if len1 < 8 {
                self.short_run(p, k, nv);
                continue;
            }
            let st = self.run_stats(p, k, &cats[k], nv);
            nv = self.long_run(p, k, &cats[k], &st, nv);
        }
    }

    /// A short run (2–8 beats): if its ratios stand out from up to 8 neighbours on each side by
    /// more than 0.16, it is widened over neighbours beyond 1.07 and refilled with evenly spaced
    /// beats when its span is close to a whole number of reference intervals.
    fn short_run(&mut self, p: &Pass, k: usize, nv: i32) {
        let r = &p.r;
        let (s, e) = (self.starts[k], self.ends[k]);
        let inside = neon_sum8(&r[s as usize..=e as usize]);
        let nl = s.min(8);
        let mut nb = 0.0;
        if 0 < s {
            nb = r[(s - 1) as usize];
            for j in 2..=nl {
                nb += r[(s - j) as usize];
            }
        }
        let right = nv + !e;
        let nr = right.min(8);
        if 0 < right {
            for j in 1..=nr.max(1) {
                nb += r[(e + j) as usize];
            }
        }
        if nl + nr != 0 {
            nb /= f64::from(nl + nr);
        }
        if inside / f64::from(e - s + 1) - nb <= 0.16 {
            return;
        }
        // widen left over up to three neighbours beyond 1.07
        let mut lo = s;
        if 0 < s && 1.07 < r[(s - 1) as usize] {
            self.starts[k] = s - 1;
            lo = s - 1;
            if s >= 2 && 1.07 < r[(s - 2) as usize] {
                self.starts[k] = s - 2;
                lo = s - 2;
                if s - 1 > 1 && 1.07 < r[(s - 3) as usize] {
                    self.starts[k] = s - 3;
                    lo = s - 3;
                }
            }
        }
        // widen right likewise, or clamp to n − 2
        let mut hi = e;
        if nv - 2 <= e {
            if nv - 2 < e {
                self.ends[k] = nv - 2;
                hi = nv - 2;
            }
        } else {
            for j in 1..=3 {
                if !(1.07 < r[(e + j) as usize]) {
                    break;
                }
                self.ends[k] = e + j;
                hi = e + j;
                if e + j >= nv - 2 {
                    break;
                }
            }
        }
        let t0 = p.t[lo as usize];
        let span = p.t[hi as usize + 1] - t0;
        let q = span * 1000.0 * (1.0 / p.ref_ms);
        let qp = q + 0.5;
        let count = qp as i32;
        let qr = qp.trunc();
        if !(count >= 2 && (q - qr).abs() < 0.25) {
            return;
        }
        if lo != hi {
            self.b.erase_range(lo as usize + 1, hi as usize + 1);
        }
        let inv = 1.0 / qr;
        let rate = f64::from(self.inp.rate);
        for j in 1..count {
            let at = (span * f64::from(j)).mul_add(inv, t0);
            let pos = at.mul_add(rate, 0.5) as i32;
            self.b.insert(lo as usize + j as usize, BeatInfo::new(pos));
        }
        self.anchor_pos = self.b.v[0].pos;
        self.anchor_stat = self.b.v[0].stat;
    }

    /// Mean, variance and drift of a long run and of its category sub-ranges.
    fn run_stats(&self, p: &Pass, k: usize, c: &Cats, nv: i32) -> RunStats {
        let x = &p.x;
        let (s, e) = (self.starts[k], self.ends[k]);
        let cnt = f64::from(e - s + 1);
        let mut st = RunStats::default();
        let slice = &x[s as usize..=e as usize];
        st.mean = neon_sum8(slice) / cnt;
        st.var[0] = neon_sumsq8(slice, st.mean) / cnt;
        let (j1, j2, j3) = drift_terms(x, s, e, nv);
        st.drift[0] = ((j3 + j1) + j2) * 0.333_333_333_333_333_3;
        let len_x = x.len() as i64;
        for cat in 0..4 {
            let (first, last) = (c.first[cat], c.last[cat]);
            if first <= last {
                let count = f64::from(last + 1 - first);
                // which indices each category's loops read
                let ok = |i: i32| -> bool {
                    match cat {
                        0 => (i as u32).wrapping_sub(1) <= 126,
                        2 => i64::from(i) >= 0 && i64::from(i) < len_x,
                        _ => i >= 0 && (i as i64) < len_x,
                    }
                };
                let mut sum = 0.0;
                if cat == 2 {
                    // runs from `first` while inside the vector
                    let start = first as i64;
                    let avail = if (start as u64) <= len_x as u64 { len_x - start } else { 0 };
                    let take = avail.min(i64::from(last + 1 - first));
                    for i in 0..take {
                        sum += x[(start + i) as usize];
                    }
                    let mean = sum / count;
                    let mut v = 0.0;
                    for i in 0..take {
                        let d = x[(start + i) as usize] - mean;
                        v = d.mul_add(d, v);
                    }
                    st.var[cat + 1] = v / count;
                } else {
                    for i in first..=last {
                        if ok(i) {
                            sum += x[i as usize];
                        }
                    }
                    let mean = sum / count;
                    let mut v = 0.0;
                    for i in first..=last {
                        if ok(i) {
                            let d = x[i as usize] - mean;
                            v = d.mul_add(d, v);
                        }
                    }
                    st.var[cat + 1] = v / count;
                }
            }
            let (j1, j2, j3) = drift_terms(x, first, last, nv);
            st.drift[cat + 1] = if cat == 3 { (j2 + j1) + j3 } else { (j3 + j1) + j2 } * 0.333_333_333_333_333_3;
        }
        st
    }
}

/// Walks left from `i` over up to five ratios accepted by `acc`; returns the new start and
/// whether all five were taken.
fn ext_left(r: &[f64], i: i32, acc: &dyn Fn(f64) -> bool) -> (i32, bool) {
    let mut j = 0;
    while j < 5 && i - j > 0 && acc(r[(i - j - 1) as usize]) {
        j += 1;
    }
    (i - j, j == 5)
}

/// Walks right from `i` over up to five accepted ratios, staying below `nv − 1`.
fn ext_right(r: &[f64], i: i32, nv: i32, acc: &dyn Fn(f64) -> bool) -> (i32, bool) {
    let mut j = 0;
    while j < 5 && i <= nv - 2 - j && acc(r[(i + j + 1) as usize]) {
        j += 1;
    }
    (i + j, j == 5)
}

/// The constants of the alignment tests, derived from the reference interval (ms).
#[derive(Clone, Copy)]
struct Tol {
    /// One, two and three intervals in s.
    step: [f64; 3],
    /// Tolerance in s (a quarter interval).
    tol: f64,
    /// 1 / reference interval.
    inv_ref: f64,
}

impl Tol {
    fn new(ref_ms: f64) -> Self {
        Self { step: [ref_ms * 0.001, ref_ms * 0.002, ref_ms * 0.003], tol: ref_ms * 0.00025, inv_ref: 1.0 / ref_ms }
    }
}

impl Focus {
    fn rate_f(&self) -> f64 {
        f64::from(self.inp.rate)
    }

    /// Erases `hi`, `hi − 1`, …, `lo + 1` one at a time.
    fn erase_down(&mut self, lo: i32, hi: i32) {
        let mut i = hi;
        while lo < i {
            self.b.erase(i as usize);
            i -= 1;
        }
    }

    /// A beat one, two or three intervals after `lo` among the next five (`n` = beat count).
    /// The whole-run copies in the binary can read one slot past the end when `lo` is within
    /// five beats of it; the sub-range copies check. Both are treated as checked here.
    fn align_start(&self, lo: i32, n: i32, tl: &Tol) -> Option<i32> {
        let mut span = n - lo;
        if span == 0 || n < lo {
            return None;
        }
        if lo <= n - 5 {
            span = 5;
        }
        if span <= 0 {
            return None;
        }
        let rate = self.rate_f();
        let t0 = f64::from(self.b.v[lo as usize].pos) / rate;
        let inv = 1.0 / rate;
        for step in tl.step {
            let target = t0 + step;
            for j in 1..=span {
                if n <= lo + j {
                    continue;
                }
                let t = f64::from(self.b.v[(lo + j) as usize].pos) * inv;
                if t - tl.tol < target && target < t + tl.tol {
                    return Some(lo + j);
                }
            }
        }
        None
    }

    /// The end-side alignment: a beat one, two or three intervals before the range end; the
    /// beats from there are erased the way the binary does it (always at `end − 1`, so the end
    /// beat and the ones after it go). Returns whether anything was erased.
    fn align_end(&mut self, rg: &mut Rg, tl: &Tol) -> bool {
        let end = rg.e;
        if end < 1 {
            return false;
        }
        let rate = self.rate_f();
        let t_end = f64::from(self.b.v[end as usize].pos) / rate;
        let lim = end.min(5);
        let hit = |t: f64, target: f64| t - tl.tol < target && target < t + tl.tol;
        let mut found = None;
        let target = t_end - tl.step[0];
        for j in 1..=lim {
            let t = f64::from(self.b.v[(end - j) as usize].pos) / rate;
            if hit(t, target) {
                found = Some(end - j);
                break;
            }
        }
        if found.is_none() {
            let inv = 1.0 / rate;
            'tiers: for step in [tl.step[1], tl.step[2]] {
                let target = t_end - step;
                for j in 1..=lim {
                    let t = f64::from(self.b.v[(end - j) as usize].pos) * inv;
                    if hit(t, target) {
                        found = Some(end - j);
                        break 'tiers;
                    }
                }
            }
        }
        let m = found.unwrap_or(end);
        if m < end - 1 {
            let at = (end - 1) as usize;
            for _ in 0..(end - 1 - m) {
                self.b.erase(at);
            }
            rg.e = m + 1;
            return true;
        }
        false
    }

    /// Fills the gap after beat `i` with evenly spaced beats when it spans several intervals;
    /// returns how many were inserted.
    fn fill_gap(&mut self, i: i32, tl: &Tol) -> i32 {
        let rate_i = self.inp.rate;
        let t0 = f64::from(self.b.v[i as usize].pos) / f64::from(rate_i);
        let gap = f64::from(self.b.v[i as usize + 1].pos) / f64::from(rate_i) - t0;
        let count = (gap * 1000.0).mul_add(tl.inv_ref, 0.5) as i32;
        if count < 2 {
            return 0;
        }
        let inv = 1.0 / f64::from(count);
        for j in 1..count {
            let at = (gap * f64::from(j)).mul_add(inv, t0);
            let pos = at.mul_add(f64::from(self.inp.rate), 0.5) as i32;
            self.b.insert((i + j) as usize, BeatInfo::new(pos));
        }
        count - 1
    }

    /// One beat before the first, one interval earlier, when it stays after 0.
    fn extend_first(&mut self) -> bool {
        let (p0, p1) = (self.b.v[0].pos, self.b.v[1].pos);
        let d = p1 - p0;
        let p = p0 - d;
        if p != 0 && d <= p0 {
            self.b.insert(0, BeatInfo::new(p));
            return true;
        }
        false
    }

    /// Beats after the last one, at the last interval, up to the end of the track.
    fn extend_last(&mut self) {
        let n = self.b.len();
        if n < 2 {
            return;
        }
        let d = self.b.v[n - 1].pos - self.b.v[n - 2].pos;
        if d == 0 {
            return;
        }
        let q = f64::from(self.inp.total - self.b.v[n - 1].pos) / f64::from(d);
        if !(1.0 < q) {
            return;
        }
        let count = q as i32;
        for j in 0..count.max(0) as usize {
            let pos = self.b.v[n - 1 + j].pos + d;
            self.b.insert(n + j, BeatInfo::new(pos));
        }
    }

    /// After a run fix that widened all five steps somewhere: the first gap beyond five times
    /// its neighbours gets filled at the following interval.
    fn fill_wide_gap(&mut self) {
        let n = self.b.len() as i32;
        let lim = (n - 2).max(1);
        let mut i = 1;
        loop {
            if i == lim {
                return;
            }
            let a = self.b.v[i as usize].pos;
            let c = self.b.v[i as usize + 1].pos;
            let d = self.b.v[i as usize + 2].pos;
            let d1 = d - c;
            let d0 = c - a;
            let prev = a - self.b.v[i as usize - 1].pos;
            if !(d0 <= prev * 5 && d0 <= d1 * 5) {
                if d != c {
                    let q = f64::from(d0) / f64::from(d1);
                    if 1.0 < q && 1 < q as i32 {
                        let step = (f64::from(d0) / q.trunc()) as i32;
                        let mut p = step;
                        for j in 0..(q as i32 - 1) {
                            self.b.insert((i + 1 + j) as usize, BeatInfo::new(p + self.b.v[i as usize].pos));
                            p += step;
                        }
                    }
                }
                return;
            }
            i += 1;
        }
    }
}

/// Where the widened run boundaries ended up.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    lo_s: i32,
    hi_s: i32,
    lo_e: i32,
    hi_e: i32,
    /// Any of the four walks took all five steps.
    five: bool,
}

/// The beat range a long-run fix works on: the whole run (`starts`/`ends`) or the sub-range of
/// one ratio category inside it.
#[derive(Debug, Clone, Copy)]
struct Rg {
    s: i32,
    e: i32,
}

/// The long-run patterns, by the ratio of the run's intervals to the reference interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fix {
    /// ≈ 0.5: a beat between every two.
    Half,
    /// ≈ 2/3: three beats where two belong.
    TwoThirds,
    /// ≈ 1.5: two beats where three belong.
    OneHalf,
    /// ≈ 2: every other beat missing.
    Double,
}

/// Ratio windows the half and double fixes accept for a neighbouring beat: (1.07, 1.8) or
/// beyond 2.2.
fn double_ratio(r: f64) -> bool {
    (1.07 < r && r < 1.8) || 2.2 < r
}

impl Focus {
    fn widen(r: &[f64], s: i32, e: i32, nv: i32, acc: &dyn Fn(f64) -> bool) -> Bounds {
        let (lo_s, f1) = ext_left(r, s, acc);
        let (hi_s, f2) = ext_right(r, s, nv, acc);
        let (lo_e, f3) = ext_left(r, e, acc);
        let (hi_e, f4) = ext_right(r, e, nv, acc);
        Bounds { lo_s, hi_s, lo_e, hi_e, five: f1 || f2 || f3 || f4 }
    }

    /// Erases the widened zones around both ends of the range. Returns `(hi_e, hi_s)` as
    /// clamped to the beat count (the binary compares them unsigned).
    fn clear_zones(&mut self, rg: &mut Rg, bd: &mut Bounds) -> (i32, i32) {
        bd.lo_e = bd.lo_e.max(0);
        let nb = self.b.len() as i32;
        let mut lim = nb - 1;
        let hi_e = if (bd.hi_e as i64 as u64) <= lim as i64 as u64 { bd.hi_e } else { lim };
        let mut end = hi_e;
        if bd.lo_e < hi_e {
            self.erase_down(bd.lo_e, hi_e);
            lim = self.b.len() as i32 - 1;
            end = bd.lo_e;
        }
        rg.e = end;
        bd.lo_s = bd.lo_s.max(0);
        let hi_s = if (bd.hi_s as i64 as u64) <= lim as i64 as u64 { bd.hi_s } else { lim };
        if bd.lo_s < hi_s {
            self.erase_down(bd.lo_s, hi_s);
            rg.e -= hi_s - bd.lo_s;
        }
        rg.s = bd.lo_s;
        (hi_e, hi_s)
    }

    /// Aligns the range start: drops the beats up to the one an interval multiple after it.
    fn align_start_cut(&mut self, rg: &mut Rg, tl: &Tol) -> bool {
        let lo = rg.s;
        if let Some(m) = self.align_start(lo, self.b.len() as i32, tl)
            && lo + 1 < m
        {
            for _ in 0..(m - lo - 1) {
                self.b.erase(lo as usize + 1);
                rg.e -= 1;
            }
            return true;
        }
        false
    }

    /// Thins the range after its second beat: drops two of every three beats (2/3) or every
    /// other beat (half, 1.5).
    fn thin(&mut self, rg: &mut Rg, fix: Fix) {
        let mut j = rg.s + 1;
        if j >= rg.e {
            return;
        }
        let per_step = if fix == Fix::TwoThirds { 2 } else { 1 };
        loop {
            j += 1;
            for _ in 0..per_step {
                if j < self.b.len() as i32 {
                    self.b.erase(j as usize);
                    rg.e -= 1;
                }
            }
            if j >= rg.e {
                break;
            }
        }
    }

    /// The common end of every long-run fix: fill the gaps next to the range ends, extend the
    /// grid before the first and after the last beat, and fill a wide gap when a widening took
    /// five steps. Returns the beat count the binary carries to the next run.
    fn finish_run(&mut self, rg: &mut Rg, end_open: bool, start_open: bool, five: bool, tl: &Tol) -> i32 {
        let nb = self.b.len() as i32;
        // the sub-range copies check `0 <= e < nb` too (overlapping zones can push e below 0)
        if end_open && 0 <= rg.e && rg.e + 1 < nb {
            rg.e += self.fill_gap(rg.e, tl);
        }
        if start_open && rg.s + 1 < nb {
            rg.e += self.fill_gap(rg.s, tl);
        }
        if self.extend_first() {
            rg.e += 1;
        }
        let mut nv = self.b.len() as i32;
        self.extend_last();
        if five {
            nv = self.b.len() as i32;
            self.fill_wide_gap();
        }
        nv
    }

    /// Long runs: pick the pattern from the run statistics and rebuild it, over the whole run
    /// or over the sub-range of the matching ratio category.
    fn long_run(&mut self, p: &Pass, k: usize, c: &Cats, st: &RunStats, nv: i32) -> i32 {
        let (s, e) = (self.starts[k], self.ends[k]);
        let len = e - s;
        let m = st.mean;
        let whole = |lo: f64, hi: f64| m < hi && lo < m && st.drift[0] < 0.008 && -1 < s && 7 < len && st.var[0] < 0.015;
        let sub = |cat: usize| st.drift[cat + 1] < 0.008 && -1 < c.first[cat] && 7 < c.last[cat] - c.first[cat] && st.var[cat + 1] < 0.015;
        let tl = Tol::new(p.ref_ms);
        let (fs, ls) = (c.first, c.last);
        if whole(0.485, 0.515) {
            return self.fix_span(p, s, e, nv, &tl, Fix::Half);
        }
        if sub(0) {
            return self.fix_span(p, fs[0], ls[0], nv, &tl, Fix::Half);
        }
        if whole(0.646_666_666_666_666_6, 0.686_666_666_666_666_6) {
            return self.fix_span(p, s, e, nv, &tl, Fix::TwoThirds);
        }
        if sub(1) {
            return self.fix_span(p, fs[1], ls[1], nv, &tl, Fix::TwoThirds);
        }
        if whole(1.455, 1.545) {
            return self.fix_span(p, s, e, nv, &tl, Fix::OneHalf);
        }
        if sub(2) {
            return self.fix_span(p, fs[2], ls[2], nv, &tl, Fix::OneHalf);
        }
        if whole(1.94, 2.06) {
            return self.fix_span(p, s, e, nv, &tl, Fix::Double);
        }
        if sub(3) {
            return self.fix_span(p, fs[3], ls[3], nv, &tl, Fix::Double);
        }
        nv
    }

    /// One long-run fix over `s..=e`: widen both ends over neighbouring ratios and erase the
    /// widened zones; except for double, re-align both ends and thin the beats; then put one
    /// beat (2/3, double) or two (1.5) into each remaining gap, and fill and extend around it.
    fn fix_span(&mut self, p: &Pass, s: i32, e: i32, nv: i32, tl: &Tol, fix: Fix) -> i32 {
        self.hits.push(match fix {
            Fix::Half => "half",
            Fix::TwoThirds => "two thirds",
            Fix::OneHalf => "1.5",
            Fix::Double => "double",
        });
        let acc: &dyn Fn(f64) -> bool = if matches!(fix, Fix::Half | Fix::Double) { &double_ratio } else { &near_ratio };
        let mut rg = Rg { s, e };
        let mut bd = Self::widen(&p.r, s, e, nv, acc);
        let (hi_e, hi_s) = self.clear_zones(&mut rg, &mut bd);
        let (mut start_cut, mut end_cut) = (false, false);
        if fix != Fix::Double {
            start_cut = self.align_start_cut(&mut rg, tl);
            end_cut = self.align_end(&mut rg, tl);
            if 1 <= rg.e {
                self.thin(&mut rg, fix);
            }
        }
        let nb = self.b.len() as i32;
        let last_one = if hi_e <= bd.lo_e && !end_cut { (rg.e == nb - 1) as i32 } else { 1 };
        let upto = (rg.e - last_one).min(nb - 2);
        let start_open = bd.lo_s < hi_s || start_cut;
        let from = rg.s + start_open as i32;
        if fix != Fix::Half && from <= upto {
            let rate = self.rate_f();
            for i in (from..=upto).rev() {
                let a = self.b.v[i as usize].pos;
                let d = self.b.v[i as usize + 1].pos - a;
                let at = i as usize + 1;
                if fix == Fix::OneHalf {
                    // thirds: the later beat goes in first, both at i + 1
                    let d3 = f64::from(d) * 0.333_333_333_333_333_3 / rate;
                    let base = f64::from(a) + 0.5;
                    self.b.insert(at, BeatInfo::new((d3 + d3).mul_add(rate, base) as i32));
                    self.b.insert(at, BeatInfo::new(d3.mul_add(rate, base) as i32));
                    rg.e += 2;
                } else {
                    let pos = f64::from(d).mul_add(0.5, f64::from(a) + 0.5) as i32;
                    self.b.insert(at, BeatInfo::new(pos));
                    rg.e += 1;
                }
            }
        }
        self.finish_run(&mut rg, hi_e > bd.lo_e || end_cut, start_open, bd.five, tl)
    }
}

/// `(x·0.5 + 0.5)` rounded the binary's way: `fcvtzs(fma(x, 0.5, 0.5))`.
fn half_round(sum: i32) -> i32 {
    f64::from(sum).mul_add(0.5, 0.5) as i32
}

/// `fcvtzs(fma(d, 1/3, 0.5))`.
fn third(d: i32) -> i32 {
    f64::from(d).mul_add(0.333_333_333_333_333_3, 0.5) as i32
}

impl Focus {
    /// The two clean-up rounds after the passes, then the final tempo and numbering.
    fn post(&mut self, mad_bpm: i32) {
        let _ = mad_bpm;
        if self.inp.hendou || self.top_is_not_four {
            for _ in 0..2 {
                let n = self.b.len() as i32;
                if 2 < n {
                    self.post_gaps(n);
                    self.post_bars();
                }
            }
        }
        self.finish();
    }

    /// Gaps of about two or more neighbouring intervals get evenly spaced beats (`n` is the
    /// count at the start; the vector grows meanwhile).
    fn post_gaps(&mut self, n: i32) {
        let mut i = 1;
        while i != n - 1 {
            let a = self.b.v[i as usize].pos;
            let prev = self.b.v[i as usize - 1].pos;
            let nx = self.b.v[i as usize + 1].pos;
            let d = nx - a;
            let mut m: Option<u32> = None;
            if i < n - 2 {
                let after = self.b.v[i as usize + 2].pos - nx;
                let r1 = if a - prev != 0 { f64::from(d) / f64::from(a - prev) } else { 0.0 };
                let r2 = if after != 0 { f64::from(d) / f64::from(after) } else { 0.0 };
                let k = (r1 + 0.5) as i32 as u32;
                if 0 < k as i32 && 0 < (r2 + 0.5) as i32 && r1 > 1.8 && r2 > 1.8 && 1 < k {
                    m = Some(k);
                }
            } else if a - prev != 0 {
                let r = f64::from(d) / f64::from(a - prev);
                let k = (r + 0.5) as i32 as u32;
                if 0 < k as i32 && r > 1.8 && 1 < k {
                    m = Some(k);
                }
            }
            if let Some(k) = m {
                let step = (f64::from(d) / f64::from(k as i32) + 0.5) as i32;
                self.b.insert(i as usize + 1, BeatInfo::new(a + step));
                for j in 2..k as i32 {
                    let pos = self.b.v[i as usize].pos + step * j;
                    self.b.insert((i + j) as usize, BeatInfo::new(pos));
                }
            }
            i += 1;
        }
    }

    /// Bars whose four-beat span is about x.5 times the neighbouring bars (n ≥ 65) are spaced
    /// at `q + 0.5` beats, in pairs (start and end of an odd-length stretch).
    fn post_bars(&mut self) {
        let n = self.b.len() as i32;
        if n < 0x41 {
            return;
        }
        let mut rec: Vec<(i32, i32)> = Vec::new();
        let (mut since, mut inside) = (0, false);
        let mut i = 0x20;
        loop {
            let p = |j: i32| self.b.v[j as usize].pos;
            let span = p(i + 4) - p(i);
            let before = p(i) - p(i - 4);
            let after = p(i + 8) - p(i + 4);
            let r1 = if before != 0 { f64::from(span) * 4.0 / f64::from(before) } else { 0.0 };
            let r2 = if after != 0 { f64::from(span) * 4.0 / f64::from(after) } else { 0.0 };
            let f1 = r1 - r1.trunc();
            let f2 = r2 - r2.trunc();
            if !(0.4 < f1 && f1 < 0.6 && 0.4 < f2 && f2 < 0.6) {
                inside = false;
            } else {
                let (q1, q2) = (r1 as i32, r2 as i32);
                if q1 != q2 || since < 5 || inside {
                    inside = q1 == q2 && since >= 5;
                } else {
                    rec.push((i, q1));
                    since = 0;
                    inside = true;
                    if rec.len() > 127 {
                        break;
                    }
                }
            }
            since += 1;
            i += 1;
            if i == n - 0x20 {
                break;
            }
        }
        let cnt = rec.len();
        let mut j = 0;
        while cnt > 1 && j < cnt / 2 {
            let (a, qa) = rec[2 * j];
            let (bq, qb) = rec[2 * j + 1];
            self.space_from_start(a, qa);
            self.space_from_end(bq, qb);
            match (qa, qb) {
                (4, 3) | (4, 4) => {
                    self.smooth_down(a + 4, bq);
                    let v = self.b.v[a as usize + 4].pos;
                    self.b.v[a as usize + 4].pos = v - third(v - self.b.v[a as usize + 3].pos);
                    if qb == 4 {
                        let at = bq as usize;
                        let pos = half_round(self.b.v[at].pos + self.b.v[at + 1].pos);
                        self.b.insert(at + 1, BeatInfo::new(pos));
                        if 2 < cnt as i64 - 2 * j as i64 && 2 * j + 2 < cnt {
                            for r in rec.iter_mut().skip(2 * j + 2) {
                                r.0 += 1;
                            }
                        }
                    }
                }
                (3, 4) => {
                    if a + 4 < bq {
                        self.smooth_up(a + 4, bq);
                    }
                    let at = bq as usize;
                    let v = self.b.v[at].pos;
                    self.b.v[at].pos = v + third(self.b.v[at + 1].pos - v);
                }
                (3, 3) => {
                    if a + 4 < bq - 1 {
                        self.smooth_up(a + 4, bq - 1);
                    }
                    self.b.erase(bq as usize);
                    let at = rec[2 * j + 1].0 as usize;
                    let v = self.b.v[at - 1].pos;
                    self.b.v[at - 1].pos = v + third(self.b.v[at].pos - v);
                    if 2 < cnt as i64 - 2 * j as i64 && 2 * j + 2 < cnt {
                        for r in rec.iter_mut().skip(2 * j + 2) {
                            r.0 -= 1;
                        }
                    }
                }
                _ => {}
            }
            j += 1;
        }
        if cnt & 1 == 1 {
            let (a, q) = rec[cnt - 1];
            self.space_from_start(a, q);
            let n = self.b.len() as i32;
            if q == 4 {
                if a + 4 < n - 1 {
                    self.smooth_down(a + 4, n - 1);
                }
                let v = self.b.v[a as usize + 4].pos;
                self.b.v[a as usize + 4].pos = v - third(v - self.b.v[a as usize + 3].pos);
                if 1 < n {
                    let p = self.b.v[n as usize - 1].pos * 2 - self.b.v[n as usize - 2].pos;
                    if p <= self.inp.total {
                        self.b.v.push(BeatInfo::new(p));
                    }
                }
            } else if q == 3 {
                if a + 4 < n - 1 {
                    self.smooth_up(a + 4, n - 1);
                }
                if 1 < n {
                    let p = self.b.v[n as usize - 1].pos * 2 - self.b.v[n as usize - 2].pos;
                    if self.inp.total < p {
                        self.b.v.pop();
                    } else {
                        self.b.v[n as usize - 1].pos = p;
                    }
                }
            }
        }
    }

    /// Beats `a+1..a+3` at `(q + 0.5)`-ths of the bar span from `a`.
    fn space_from_start(&mut self, a: i32, q: i32) {
        let a = a as usize;
        let d = f64::from(self.b.v[a + 4].pos - self.b.v[a].pos);
        let p = f64::from(self.b.v[a].pos) + 0.5;
        self.b.v[a].pos = p as i32;
        let base = p.trunc();
        let inv = 1.0 / (f64::from(q) + 0.5);
        self.b.v[a + 1].pos = (d.mul_add(inv, 0.5) + base) as i32;
        self.b.v[a + 2].pos = (d.mul_add(inv + inv, 0.5) + base) as i32;
        self.b.v[a + 3].pos = ((d * 3.0).mul_add(inv, 0.5) + base) as i32;
    }

    /// Beats `b+1..b+3` back from `b+4` at `(q + 0.5)`-ths of the span.
    fn space_from_end(&mut self, b: i32, q: i32) {
        let b = b as usize;
        let d = f64::from(self.b.v[b + 4].pos - self.b.v[b].pos);
        let p = f64::from(self.b.v[b + 4].pos) + 0.5;
        self.b.v[b + 4].pos = p as i32;
        let base = p.trunc();
        let inv = 1.0 / (f64::from(q) + 0.5);
        self.b.v[b + 3].pos = ((-d).mul_add(inv, 0.5) + base) as i32;
        self.b.v[b + 2].pos = ((d * -2.0).mul_add(inv, 0.5) + base) as i32;
        self.b.v[b + 1].pos = ((d * -3.0).mul_add(inv, 0.5) + base) as i32;
    }

    /// From `hi` down to `lo + 1`: each beat moves to the middle of itself and the beat before.
    fn smooth_down(&mut self, lo: i32, hi: i32) {
        if lo >= hi {
            return;
        }
        let mut cur = self.b.v[hi as usize].pos;
        let mut l = hi;
        loop {
            l -= 1;
            let prev = self.b.v[l as usize].pos;
            self.b.v[l as usize + 1].pos = half_round(prev + cur);
            cur = prev;
            if lo >= l {
                break;
            }
        }
    }

    /// From `lo` up to `hi − 1`: each beat moves to the middle of itself and the next one.
    fn smooth_up(&mut self, lo: i32, hi: i32) {
        let mut cur = self.b.v[lo as usize].pos;
        for l in lo..hi {
            let next = self.b.v[l as usize + 1].pos;
            self.b.v[l as usize].pos = half_round(cur + next);
            cur = next;
        }
    }

    /// The final stage: median tempo, beat numbers from the anchor, per-beat tempo over ±8
    /// beats, averaged per bar.
    fn finish(&mut self) {
        let n = self.b.len() as i32;
        let rate = self.inp.rate;
        if 1 < n {
            let mut list: Vec<f32> = Vec::with_capacity(n as usize);
            for i in 0..(n - 1) as usize {
                let d = self.b.v[i + 1].pos - self.b.v[i].pos;
                if d != 0 {
                    list.push((rate as f32 * 60.0) / d as f32);
                }
            }
            let d = self.b.v[n as usize - 1].pos - self.b.v[n as usize - 2].pos;
            if d != 0 {
                list.push((rate as f32 * 60.0) / d as f32);
            }
            // the binary sorts n entries even when fewer were pushed (zero gaps)
            list.resize(n as usize, 0.0);
            for i in 0..(n - 1) as usize {
                for j in i + 1..n as usize {
                    if list[j] < list[i] {
                        list.swap(i, j);
                    }
                }
            }
            self.bpm = if n & (0x8000_0001u32 as i32) == 1 {
                list[(n >> 1) as usize]
            } else {
                (list[(n >> 1) as usize] + list[(n >> 1) as usize - 1]) * 0.5
            };
        } else if n & (0x8000_0001u32 as i32) == 1 {
            // one beat: the binary reads an empty list here
            self.bpm = 0.0;
        }
        let mut closest = 0i32;
        if n >= 2 {
            let anchor = self.anchor_pos;
            let dist = |p: i32| (p.wrapping_sub(anchor)).unsigned_abs();
            let d0 = dist(self.b.v[0].pos);
            let mut d = dist(self.b.v[1].pos);
            if d < d0 {
                let mut j = 0i32;
                loop {
                    closest = n - 1;
                    if n - 2 == j {
                        break;
                    }
                    let nd = dist(self.b.v[(j + 2) as usize].pos);
                    closest = j + 1;
                    j += 1;
                    let better = nd < d;
                    d = nd;
                    if !better {
                        break;
                    }
                }
            }
            if 4 < n {
                for i in 2..(n - 2) as usize {
                    let s1 = if rate != 0 { self.b.v[i].pos / rate } else { 0 };
                    let s2 = if rate != 0 { self.b.v[i + 1].pos / rate } else { 0 };
                    if s1 / 60 != s2 / 60 {
                        let st = |j: usize| self.b.v[j].stat;
                        let okd = |d: i32| d == 1 || d == -3;
                        if !(okd(st(i - 1) - st(i - 2)) && okd(st(i) - st(i - 1)) && okd(st(i + 1) - st(i)) && okd(st(i + 2) - st(i + 1))) {
                            closest = 0;
                            self.anchor_stat = self.b.v[0].stat;
                        }
                    }
                }
            }
        }
        if 0 < n {
            for i in 0..n {
                let v = self.anchor_stat - closest - 1 + i;
                self.b.v[i as usize].stat = v.rem_euclid(4) + 1;
            }
            for i in 0..n {
                let lo = if i < 9 { 8 } else { i } - 8;
                let hi = if n <= i + 8 { n - 1 } else { i + 8 };
                let cnt = hi - lo;
                if cnt != 0 {
                    let d = (self.b.v[hi as usize].pos - self.b.v[lo as usize].pos) as f32;
                    self.b.v[i as usize].tempo = (d * 1000.0) / (rate as f32 * cnt as f32);
                }
            }
        }
        if n < 4 {
            return;
        }
        let first_down = (0..4).find(|&j| self.b.v[j].stat == 1);
        let (start, at_zero) = match first_down {
            Some(0) | None => (0usize, true),
            Some(j) => (j, false),
        };
        let mut next = 0usize;
        if (start as i32) < n - 4 {
            let mut s = start;
            loop {
                let e = s + 4;
                let cnt = if e as i32 <= n { 4 } else { n as usize - s };
                let mut sum = 0.0f32;
                for j in s..s + cnt {
                    sum += self.b.v[j].tempo;
                }
                let avg = sum / cnt as f32;
                for j in s..s + cnt {
                    self.b.v[j].tempo = avg;
                }
                s = e;
                next = e;
                if e as i32 >= n - 4 {
                    break;
                }
            }
        }
        if !at_zero {
            let t = self.b.v[start].tempo;
            for j in 0..start {
                self.b.v[j].tempo = t;
            }
        }
        if 0 < next && (next as i32) < n {
            let t = self.b.v[next - 1].tempo;
            for j in next..n as usize {
                self.b.v[j].tempo = t;
            }
        }
    }
}

impl Focus {
    /// `BA_GetBeatPos_Focus` @0x1011a7664: `(pos, beat in bar, BPM × 100)` per beat.
    pub fn beat_list(&self) -> Vec<(i32, i32, i32)> {
        self.b.v.iter().map(|b| (b.pos, b.stat, (6e6f32 / b.tempo + 0.5) as i32)).collect()
    }

    /// `getBpm_Focus` @0x1011820d4: `fcvtzs(fmaf(bpm, 100, 0.5))` (single precision).
    pub fn bpm100(&self) -> i32 {
        self.bpm.mul_add(100.0, 0.5) as i32
    }

    /// The edit trace (development aid).
    pub fn trace(&self) -> &[String] {
        self.b.trace.as_deref().unwrap_or(&[])
    }
}
