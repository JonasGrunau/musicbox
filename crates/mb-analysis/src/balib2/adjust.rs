//! BALib v2's `UnitBeatAdjust` (`create` @0x10113aabc, `putSample` @0x10113ad58, `analyze`
//! @0x10113af38) and the two readers of its low band: test A of the verdict
//! (`BeatInfoAnalyzer::judgeHendouDynamicPeak` @0x1011a6714) and the offset
//! (`Analyzer::getOffsetCalc` @0x101182100). `analysis-beats-hp.md` § 8.
//!
//! The unit keeps three quarter-rate envelopes of the mid signal. `analyze` (called by Focus's
//! constant path) searches the BPM whose grid best lines up with the 2 kHz band (`bpmAdjust`),
//! then moves the grid onto the band's beat-synchronous rise (`beatPositionAdjust`).

// Negated comparisons keep the binary's NaN behaviour.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use crate::beats::BeatInfo;
use crate::beats::filters::Filter2D;

/// `UnitBeatAdjust` v2. `putSample` takes every fourth frame of each block and keeps three
/// envelopes of the mid signal, each clamped to 0..32767 (filters designed for a quarter of the
/// rate):
/// - high (+0x70): 2 kHz high-pass, rectified, 1 kHz low-pass;
/// - low (+0x78, `get_pdata_low`): 100 Hz low-pass, rectified, 25 Hz low-pass;
/// - full (+0x80): rectified, 1 kHz low-pass.
pub struct UnitBeatAdjust {
    /// `+0xc`: rate / 4.
    rate4: i32,
    /// `+0x8`: samples.
    total: i32,
    hp: Filter2D,
    hp_env: Filter2D,
    lp: Filter2D,
    lp_env: Filter2D,
    full_env: Filter2D,
    high: Vec<i16>,
    low: Vec<i16>,
    full: Vec<i16>,
    /// `+0x20`: beats in the list being adjusted.
    beats: i32,
    /// `+0x18`: candidate grid counter, the index into `grids`.
    counter: i32,
    /// `+0x58`: every candidate grid `createBeatData` made, as (BPM, start); the positions are
    /// rebuilt on demand.
    grids: Vec<(f64, i32)>,
    /// `+0x30`: the high band summed around the beats, `2·w` values.
    acc: Vec<f64>,
    /// `+0x38`, `+0x40`, `+0x48`, `+0x50`: the same per beat index modulo 4 (1, 2, 3, 0).
    bars: [Vec<f64>; 4],
    /// `+0x28`: the largest value of `acc` (`findBeatPositionForBeatSubAll`).
    acc_max: f64,
}

fn clamp16(y: f64) -> i16 {
    if y > 32767.0 {
        i16::MAX
    } else if y >= 0.0 {
        y as i32 as i16
    } else {
        0
    }
}

impl UnitBeatAdjust {
    pub fn new(rate: u32, total: i32) -> Self {
        let rate4 = rate as i32 / 4;
        let fs = f64::from(rate4);
        Self {
            rate4,
            total,
            hp: Filter2D::high(2000.0, fs),
            hp_env: Filter2D::low(1000.0, fs),
            lp: Filter2D::low(100.0, fs),
            lp_env: Filter2D::low(25.0, fs),
            full_env: Filter2D::low(1000.0, fs),
            high: Vec::new(),
            low: Vec::new(),
            full: Vec::new(),
            beats: 0,
            counter: 0,
            grids: Vec::new(),
            acc: Vec::new(),
            bars: Default::default(),
            acc_max: 0.0,
        }
    }

    /// One block (the frames `UnitEnvelopeMaker` gets).
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        let mut take = pcm.len() as i32;
        if take < 1 {
            return;
        }
        let have = self.low.len() as i32;
        let left = self.total - have * 4;
        if left < take {
            take = left / 4 * 4;
        }
        let room = (self.rate4 * 900 - have) * 4;
        if room <= take {
            take = room;
        }
        let mut i = 0;
        while i < take {
            let [l, r] = pcm[i as usize];
            let x = (f64::from(r) + f64::from(l)) * 0.5;
            let h = self.hp_env.step(self.hp.step(x).abs());
            let lo = self.lp_env.step(self.lp.step(x).abs());
            let f = self.full_env.step(x.abs());
            self.high.push(clamp16(h));
            self.low.push(clamp16(lo));
            self.full.push(clamp16(f));
            i += 4;
        }
    }

    /// Development check: (count, sum, weighted sum, samples 1000/5000/20000) per band, as the
    /// `hp` harness prints them.
    #[doc(hidden)]
    pub fn band_checks(&self) -> Vec<(usize, i64, i64, [i16; 3])> {
        [&self.high, &self.low, &self.full]
            .iter()
            .map(|d| {
                let sum = d.iter().map(|&x| i64::from(x)).sum();
                let wsum = d
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| i64::from(x) * (i % 997) as i64)
                    .sum();
                let at = |i: usize| d.get(i).copied().unwrap_or(0);
                (d.len(), sum, wsum, [at(1000), at(5000), at(20000)])
            })
            .collect()
    }

    /// `+0x1c`: the band's capacity, `min(rate/4 · 900, total / 4)`.
    fn cap(&self) -> i32 {
        (self.rate4 * 900).min(self.total.max(0) >> 2)
    }

    /// `calcLowInteg(bpm, pos)`: the band summed over two beats from `pos`; −1 when the window
    /// passes the capacity, the BPM is zero or `pos` < −3.
    fn integ(&self, bpm: f32, pos: i32) -> f64 {
        if bpm.abs() <= f32::EPSILON || pos < -3 {
            return -1.0;
        }
        let len = ((f64::from(self.rate4) * 30.0) / f64::from(bpm) + 0.5) as i32;
        let start = f64::from(pos).mul_add(0.25, 0.5) as i32;
        if start + len - 1 > self.cap() {
            return -1.0;
        }
        (start..start + len.max(0))
            .map(|k| f64::from(self.low.get(k as usize).copied().unwrap_or(0)))
            .sum()
    }

    /// `getOffsetCalc`: where the low end comes in. Each beat's two-beat low-band sum, scaled to
    /// the loudest; a scan finds the first stretch of loud beats (over 0.4, or three over 0.2)
    /// that starts with a rise and lasts 12 beats, else the first loud start (or one after more
    /// than 32 quiet beats). The offset is four beats before it, in samples (≥ 0).
    pub fn offset(&self, rate: u32, positions: &[i32], bpm: f32) -> i32 {
        let n = positions.len();
        if n == 0 {
            return 0;
        }
        let m = n - 1;
        let mut v = vec![0.0f64; n];
        for i in 0..m {
            v[i] = self.integ(bpm, positions[i]);
            if i != 0 && v[i] < 0.0 {
                v[i] = v[i - 1];
            }
        }
        let peak = v[..m].iter().fold(0.0f64, |a, &x| a.max(x));
        if peak.abs() > f64::EPSILON {
            let inv = 1.0 / peak;
            v[..m].iter_mut().for_each(|x| *x *= inv);
        }
        let first = if n < 3 { -1 } else { loud_start(&v) };
        let idx = first.max(0);
        let off = ((f64::from(rate) * 60.0) * f64::from(idx - 4)) / f64::from(bpm);
        (off as i32).max(0)
    }
}

/// `getOffsetCalc`'s scan (0x101182394–0x10118257c) over the scaled sums `v`.
fn loud_start(v: &[f64]) -> i32 {
    let n = v.len();
    let mut d = vec![0.0f64; n];
    for i in 1..n - 1 {
        d[i] = v[i] - v[i - 1];
    }
    enum Step {
        Count,
        Loud,
        Low,
        Next2,
        Fall,
        Quiet,
    }
    let (mut loud, mut beats, mut quiet) = (false, 0i32, 0i32);
    let (mut cand, mut start) = (-1i32, -1i32);
    for i in 0..n - 2 {
        let x = v[i];
        let mut step = if i == 0 {
            if x > 0.4 {
                start = 0;
                cand = 0;
                Step::Count
            } else {
                Step::Low
            }
        } else if loud {
            if x > 0.4 || (x > 0.2 && v[i + 1] > 0.2 && v[i + 2] > 0.2) {
                Step::Count
            } else if x < 0.2 {
                Step::Next2
            } else {
                Step::Fall
            }
        } else if x <= 0.2 {
            Step::Low
        } else if d[i] <= 0.15 {
            if x < 0.2 { Step::Next2 } else { Step::Fall }
        } else {
            // A rise into a loud beat starts a stretch; it becomes the candidate when none was
            // taken yet or after more than 32 quiet beats.
            if quiet > 32 || start < 0 {
                cand = i as i32;
            }
            quiet = 0;
            start = i as i32;
            if beats > 11 {
                return start;
            }
            Step::Loud
        };
        loop {
            step = match step {
                Step::Count => {
                    if beats > 11 {
                        return start;
                    }
                    Step::Loud
                }
                Step::Loud => {
                    beats += 1;
                    loud = true;
                    if x < 0.2 { Step::Next2 } else { Step::Fall }
                }
                Step::Low => {
                    loud = false;
                    if x >= 0.2 { Step::Fall } else { Step::Next2 }
                }
                Step::Next2 => {
                    if v[i + 1] >= 0.2 || v[i + 2] >= 0.2 {
                        Step::Fall
                    } else {
                        Step::Quiet
                    }
                }
                Step::Fall => {
                    if d[i] < -0.4 && d[i + 1] < 0.2 && d[i + 2] < 0.2 {
                        Step::Quiet
                    } else {
                        break;
                    }
                }
                Step::Quiet => {
                    beats = 0;
                    loud = false;
                    quiet += 1;
                    break;
                }
            };
        }
    }
    if beats > 12 { start } else { cand }
}

/// A score curve's peaks (`findpeaks_x` @0x1011685c4, MATLAB style): 1-based positions, values,
/// prominences and widths at half prominence.
#[derive(Default)]
struct Peaks {
    locs: Vec<i32>,
    vals: Vec<f64>,
    proms: Vec<f64>,
}

fn find_peaks(x: &[f64]) -> Peaks {
    let n = x.len();
    let mut p = Peaks::default();
    if n < 2 {
        return p;
    }
    // A peak is a rise followed by a fall or a flat step (a plateau counts at its first point).
    let mut rising = false;
    for i in 0..n - 1 {
        let d = x[i] - x[i + 1];
        if d < 0.0 {
            rising = true;
            continue;
        }
        if rising {
            p.locs.push(i as i32 + 1);
            p.vals.push(x[i]);
        }
        rising = false;
    }
    let nn = n as i32;
    let at = |i: i32| x[i as usize];
    // Left and right markers carry over between peaks as in the binary.
    let (mut lm, mut rm) = (0i32, 0i32);
    for k in 0..p.locs.len() {
        let (l, v) = (p.locs[k], p.vals[k]);
        // Left: back to the first higher point (or the start); the minimum from there through
        // the sample after the peak.
        let mut j = l - 1;
        if j >= 0 {
            loop {
                if v < at(j) {
                    lm = j + 1;
                    break;
                }
                if j == 0 {
                    lm = 1;
                }
                j -= 1;
                if j < 0 {
                    break;
                }
            }
        }
        let mut left = at(lm - 1);
        for i in lm..=l {
            if i >= 0 && i <= l {
                left = left.min(at(i));
            }
        }
        // Right: on to the first higher point (or the end); the minimum from the sample after the
        // peak through the sample after that point.
        if l <= nn {
            let mut t = l;
            let mut rem = nn - l;
            loop {
                if v < at(t - 1) {
                    rm = t;
                    break;
                }
                if rem == 0 {
                    rm = nn;
                }
                t += 1;
                rem -= 1;
                if t == nn + 1 {
                    break;
                }
            }
        }
        let mut right = at(l - 1);
        if l <= rm {
            let mut limit = if nn >= l { nn - l } else { 0 };
            let mut c = rm - l + 1;
            let mut i = l;
            while limit != 0 {
                right = right.min(at(i));
                limit -= 1;
                c -= 1;
                i += 1;
                if c == 0 {
                    break;
                }
            }
        }
        p.proms.push(v - left.max(right));
    }
    p
}

impl UnitBeatAdjust {
    /// Half the scoring window: `(int)(rate/4 · 0.05)` quarter-rate samples.
    fn half(&self) -> i32 {
        (f64::from(self.rate4) * 0.05) as i32
    }

    /// The window `[c − w, c + w)` of a band, when it fits (`manageWave` @0x10113ce60).
    fn window(&self, c: i32, w: i32) -> Option<usize> {
        let lo = c.wrapping_sub(w);
        if lo < 0 {
            return None;
        }
        let hi = c.wrapping_add(w);
        let span = hi.wrapping_sub(lo);
        let cap = self.cap();
        if span <= 0 || cap < hi || !(span >= 1 && span <= 2 * w && span <= cap) {
            return None;
        }
        Some(lo as usize)
    }

    /// Adds the high band's window at `lo` into `acc` (and a bar buffer).
    fn add_window(acc: &mut [f64], band: &[i16], lo: usize) {
        for (j, a) in acc.iter_mut().enumerate() {
            *a += f64::from(band.get(lo + j).copied().unwrap_or(0));
        }
    }

    /// A candidate grid's quarter-rate positions (`createBeatData` @0x10113c9d8): `beats`
    /// positions from `start` at the BPM's beat length (fused multiply-add), then on to the end
    /// of the track by repeated addition; each `trunc(x + 0.5)`. `limit` caps the count.
    fn grid(&self, bpm: f64, start: i32, limit: usize) -> Vec<i32> {
        let len = (f64::from(self.rate4) * 60.0) / bpm;
        let end = (f64::from(self.total) * 0.25).floor();
        let mut out = Vec::new();
        let mut x = 0.0;
        for k in 0..self.beats {
            x = len.mul_add(f64::from(k), f64::from(start));
            if x < end {
                out.push((x + 0.5) as i32);
                if out.len() >= limit {
                    return out;
                }
            }
        }
        x += len;
        while x < end && out.len() < limit {
            out.push((x + 0.5) as i32);
            x += len;
        }
        out
    }

    /// `createBeatData`: records the candidate grid under the next counter value.
    fn create_beat_data(&mut self, bpm: f64, start: i32) {
        if !(bpm > 0.0) || start < 0 || self.cap() <= start || self.beats < 1 {
            return;
        }
        self.grids.push((bpm, start));
    }

    /// `manageWave`: the high band summed around the grid's beats (all but the first and last of
    /// the first `n`); the score is the sum's peak.
    fn manage_wave(&mut self, n: i32, counter: i32, score: &mut f64) {
        if n < 1 || counter < 0 || counter as usize >= self.grids.len() {
            return;
        }
        let w = self.half();
        let (bpm, start) = self.grids[counter as usize];
        let g = self.grid(bpm, start, n as usize);
        let m = g.len().min(n as usize);
        let mut sums = vec![0i64; (2 * w).max(0) as usize];
        if m > 2 {
            for &c in &g[1..m - 1] {
                if let Some(lo) = self.window(c, w) {
                    for (j, s) in sums.iter_mut().enumerate() {
                        *s += i64::from(self.high.get(lo + j).copied().unwrap_or(0));
                    }
                }
            }
        }
        if w < 1 {
            return;
        }
        // The values are whole numbers, so integer sums equal the binary's double sums.
        *score = sums.iter().fold(0.0f64, |a, &s| a.max(s as f64));
    }

    /// `getPeakBpm` @0x10113c028: scores `count · 32` tempos from `from` up in 0.0000625 BPM
    /// steps (kept within 70–180), tracking the best. `more` says whether the search should go
    /// on: the score curve's peaks are not yet clear, no whole BPM was crossed and the best stays
    /// within 70.007–179.982.
    #[allow(clippy::too_many_arguments)]
    fn get_peak_bpm(
        &mut self,
        from: f64,
        count: i32,
        start: i32,
        n: i32,
        out_bpm: &mut f64,
        best: &mut f64,
        out_cnt: &mut i32,
        more: &mut bool,
    ) {
        if start < 0 || !(from > 0.0) || !(1..=51).contains(&count) || n < 1 || self.cap() <= start
        {
            return;
        }
        let len = (count * 32) as usize;
        let mut bpms = vec![0.0f64; len];
        let mut scores = vec![0.0f64; len];
        let mut score = 0.0;
        for i in 0..len {
            // fmaxnm then fminnm, as in the binary (not a clamp: NaN gives 70)
            #[allow(clippy::manual_clamp)]
            let b = (i as f64).mul_add(6.25e-5, from).max(70.0).min(180.0);
            self.create_beat_data(b, start);
            self.manage_wave(n, self.counter, &mut score);
            bpms[i] = b;
            scores[i] = score;
            if *best < score {
                *best = score;
                *out_bpm = b;
                *out_cnt = self.counter;
            }
            self.counter += 1;
        }
        let pk = find_peaks(&scores);
        let k = pk.locs.len();
        *more = true;
        if k < len && k >= 21 && pk.proms[10..k].iter().any(|&p| p > *best * 0.075) {
            *more = false;
            let (mut m, mut idx) = (0.0f64, 0i32);
            for j in 0..k {
                if m < pk.proms[j] {
                    idx = pk.locs[j] - 1;
                    m = pk.proms[j];
                }
            }
            *out_bpm = bpms[idx as usize];
            let thr = *best * 0.75;
            let high = pk.vals[..k].iter().filter(|&&v| v > thr).count();
            if high as f64 / k as f64 > 0.5 {
                *more = true;
            }
        }
        let r = (from + 0.05).round();
        if r >= from && r <= from + 0.1 {
            *more = false;
        }
        if !(*out_bpm > 70.007 && *out_bpm < 179.982) {
            *more = false;
        }
    }

    /// The first beat (from the start) whose position plus `off` is above −4, in quarter-rate
    /// samples (C division); −1 for an empty list.
    fn first_start(list: &[BeatInfo], off: i32) -> i32 {
        if list.is_empty() {
            return -1;
        }
        let mut v = 0;
        for (i, b) in list.iter().enumerate() {
            v = b.pos.wrapping_add(off);
            if !(v <= -4 && i + 1 < list.len()) {
                break;
            }
        }
        v / 4
    }

    /// One search for `bpmAdjust`: from `start` BPM in 0.05 steps up, down, or both ways
    /// (by the BPM's fraction) while `getPeakBpm` asks for more, then once more just below the
    /// whole BPM when the last result is within 0.05 % of it. Returns (BPM, counter, score).
    #[allow(clippy::too_many_arguments)]
    fn search(
        &mut self,
        bpm: f64,
        frac_half: Option<bool>,
        from: i32,
        n: i32,
        out: &mut f64,
        cnt: &mut i32,
        more: &mut bool,
    ) -> (f64, i32, f64) {
        const STEP: f64 = 0.05;
        let run = |s: &mut Self, up: bool, out: &mut f64, cnt: &mut i32, more: &mut bool| {
            let (mut best_bpm, mut best_cnt, mut best) = (0.0f64, 0i32, 0.0f64);
            let mut x = bpm - STEP;
            loop {
                let mut score = 0.0;
                s.get_peak_bpm(x, 51, from, n, out, &mut score, cnt, more);
                x += if up { STEP } else { -STEP };
                if best < score {
                    best_bpm = *out;
                    best_cnt = *cnt;
                }
                best = best.max(score);
                if !*more {
                    break;
                }
            }
            (best_bpm, best_cnt, best)
        };
        let (mut b, mut c, mut s) = match frac_half {
            // exactly .5: both ways, the second with its own outputs
            None => {
                let up = run(self, true, out, cnt, more);
                let (mut out2, mut cnt2, mut more2) = (0.0, 0, true);
                let down = run(self, false, &mut out2, &mut cnt2, &mut more2);
                if up.2 < down.2 {
                    (down.0, down.1, down.2)
                } else {
                    (up.0, up.1, up.2.max(down.2))
                }
            }
            Some(up) => run(self, up, out, cnt, more),
        };
        let last = *out;
        let r = last.round();
        if !(last == r || last < r * 0.9995 || last > r * 1.0005) {
            let mut score = 0.0;
            self.get_peak_bpm(r - STEP, 51, from, n, out, &mut score, cnt, more);
            if s < score {
                b = *out;
                c = *cnt;
                s = score;
            }
        }
        (b, c, s)
    }

    /// `bpmAdjust` @0x10113aff8: the best grid tempo from the first beat, and from half a beat
    /// later; the later phase wins when the first scores under 96 % of it, which also moves every
    /// beat by that half beat. `reflectBeatData` then rebuilds the list on the winning grid.
    fn bpm_adjust(&mut self, bpm: &mut f32, list: &mut Vec<BeatInfo>, n: i32) {
        if n < 1 || *bpm <= 0.05f32 || list.is_empty() {
            return;
        }
        let s0 = Self::first_start(list, 0);
        let frac = *bpm - bpm.floor();
        let dir = if frac == 0.5f32 {
            None
        } else {
            Some(frac > 0.5f32)
        };
        let (mut out, mut cnt, mut more) = (0.0f64, 0i32, true);
        let (mut best_bpm, mut best_cnt, best) =
            self.search(f64::from(*bpm), dir, s0, n, &mut out, &mut cnt, &mut more);
        let half = if best_bpm.abs() > f64::EPSILON {
            ((f64::from(self.rate4) * 30.0) / best_bpm + 0.5) as i32
        } else {
            0
        };
        let s1 = Self::first_start(list, half);
        let frac2 = best_bpm - best_bpm.floor();
        let dir2 = if frac2 == 0.5 {
            None
        } else {
            Some(frac2 > 0.5)
        };
        let (mut out2, mut cnt2, mut more2) = (0.0f64, 0i32, true);
        let (b2, c2, s2) = self.search(best_bpm, dir2, s1, n, &mut out2, &mut cnt2, &mut more2);
        if best < s2 * 0.96 {
            for b in list.iter_mut() {
                b.pos = b.pos.wrapping_add(half);
            }
            best_cnt = c2;
            best_bpm = b2;
        }
        self.reflect_beat_data(best_bpm, bpm, list, best_cnt);
    }

    /// `reflectBeatData` @0x10113c670: when the BPM changed (to the 0.001), the list takes the
    /// winning grid's positions (×4), its beat length and bar numbers continued from the beat
    /// nearest to the grid's start; extra beats copy the last one.
    fn reflect_beat_data(
        &mut self,
        best: f64,
        bpm: &mut f32,
        list: &mut Vec<BeatInfo>,
        counter: i32,
    ) {
        if !(best > 0.0)
            || counter < 0
            || counter as usize >= self.grids.len()
            || !(*bpm > 0.0)
            || list.is_empty()
        {
            return;
        }
        if (f64::from(*bpm) * 1000.0) as i32 == (best * 1000.0) as i32 {
            return;
        }
        let (gb, gs) = self.grids[counter as usize];
        let g = self.grid(gb, gs, usize::MAX);
        let m = g.len();
        let mut n = self.beats as usize;
        while n != m {
            if n < m {
                let mut b = list[n - 1];
                b.stat = if b.stat == 4 { 1 } else { b.stat + 1 };
                b.flag = 0;
                list.push(b);
                n += 1;
            } else {
                list.pop();
                n -= 1;
            }
            self.beats = n as i32;
        }
        let mut maxbar = list[0].stat;
        let mut phase = list[0].stat - 1;
        if m > 1 {
            let p0 = g[0].wrapping_mul(4);
            let mut nearest = list[0].pos.wrapping_sub(p0).wrapping_abs();
            for b in &list[1..m] {
                maxbar = maxbar.max(b.stat);
                let d = b.pos.wrapping_sub(p0).wrapping_abs();
                if nearest > d {
                    phase = b.stat - 1;
                }
                nearest = nearest.min(d);
            }
        }
        let tempo = (60000.0 / best) as f32;
        for (j, b) in list.iter_mut().take(m).enumerate() {
            let ph = phase + j as i32;
            b.pos = g[j].wrapping_mul(4);
            b.tempo = tempo;
            b.stat = if maxbar != 0 { ph % maxbar } else { ph } + 1;
        }
        *bpm = best as f32;
    }

    /// `analyze` @0x10113af38: `bpmAdjust` over the beats of the first 15 minutes (all of them
    /// in a shorter track), then `beatPositionAdjust`.
    pub fn analyze(&mut self, bpm: &mut f32, list: &mut Vec<BeatInfo>) {
        if !(*bpm > 0.0) || list.is_empty() {
            return;
        }
        let n = list.len() as i32;
        self.beats = n;
        let quarter = self.rate4 * 900;
        let k = if quarter <= self.cap() {
            let span = f64::from(list[0].pos).mul_add(-0.25, f64::from(quarter));
            ((f64::from(*bpm) * f64::from_bits(0x3f91_1111_1111_1111) * span)
                / f64::from(self.rate4)) as i32
        } else {
            n
        };
        self.bpm_adjust(bpm, list, k);
        self.beat_position_adjust(list);
    }

    /// `beatPositionAdjust` @0x10113b7d4: sums the high band around every inner beat (and per
    /// beat index modulo 4), finds where the beat's rise starts in the sum and shifts the list
    /// there (`offsetBeatPosition`).
    fn beat_position_adjust(&mut self, list: &mut Vec<BeatInfo>) {
        if list.is_empty() {
            return;
        }
        let w = self.half();
        let size = (2 * w).max(1) as usize;
        self.acc = vec![0.0; size];
        self.bars = std::array::from_fn(|_| vec![0.0; size]);
        let n = list.len();
        if n >= 3 && w > 0 {
            let mut acc = std::mem::take(&mut self.acc);
            let mut bars = std::mem::take(&mut self.bars);
            for k in 1..n - 1 {
                if let Some(lo) = self.window(list[k].pos / 4, w) {
                    Self::add_window(&mut acc[..(2 * w) as usize], &self.high, lo);
                    Self::add_window(&mut bars[(k + 3) % 4][..(2 * w) as usize], &self.high, lo);
                }
            }
            self.acc = acc;
            self.bars = bars;
        }
        let idx = self.find_beat_position(2 * w);
        self.offset_beat_position(w, idx, list);
    }

    /// `findBeatPositionForBeat` @0x10113d18c: the rise in the whole sum, overridden by the
    /// latest reliable per-bar rise when that is more than 34 samples later.
    fn find_beat_position(&mut self, m: i32) -> i32 {
        let (mut val, mut idx) = (0.0f64, 0i32);
        if 0 < m && m <= 2 * self.half() {
            self.sub_all(m, &mut val, &mut idx);
            let mut best = 0;
            for k in 0..4 {
                let (_, i, unsure) = self.sub_each(k, m);
                if k == 0 {
                    best = if unsure || i < 1 { 0 } else { i };
                } else if !(unsure || i <= best) {
                    best = i;
                }
            }
            if idx <= best - 35 {
                idx = best;
            }
        }
        idx
    }

    /// The 32-sample moving average of `buf[..m]` (`m − 32` values), its maximum and first
    /// maximal index.
    fn moving_avg(buf: &[f64], m: usize) -> (Vec<f64>, f64, usize) {
        let k = m - 32;
        let (mut top, mut at) = (0.0f64, 0usize);
        let mut ma = Vec::with_capacity(k);
        for i in 0..k {
            let s: f64 = buf[i..i + 32].iter().sum();
            let v = s * 0.03125;
            if top < v {
                at = i;
            }
            top = top.max(v);
            ma.push(v);
        }
        (ma, top, at)
    }

    /// `findBeatPositionForBeatSubAll` @0x10113dbfc on the whole sum.
    fn sub_all(&mut self, m: i32, val: &mut f64, idx: &mut i32) {
        if m < 34 || 2 * self.half() < m {
            return;
        }
        let mu = m as usize;
        let acc = &self.acc;
        let (ma, top, at) = Self::moving_avg(acc, mu);
        let d: Vec<f64> = (0..mu - 33).map(|i| ma[i + 1] - ma[i]).collect();
        let dthr = d.iter().fold(0.0f64, |a, &x| a.max(x)) * 0.25;
        let pk = find_peaks(&ma);
        let k = pk.locs.len();
        let (mut hi, mut lo) = (0i32, 0i32);
        if k > 0 && k < ma.len() {
            let maxval = pk.vals.iter().fold(0.0f64, |a, &x| a.max(x));
            let maxprom = pk.proms.iter().fold(0.0f64, |a, &x| a.max(x));
            if let Some(u) =
                (0..k).find(|&u| pk.vals[u] > maxval * 0.65 && pk.proms[u] > maxprom * 0.25)
            {
                hi = pk.locs[u];
                for j in (0..u).rev() {
                    if pk.vals[j] > maxval * 0.25 && pk.proms[j] > maxprom * 0.25 {
                        lo = pk.locs[j];
                    }
                }
            }
        }
        // The last fall before the maximum: where the final rise starts.
        let mut rise = 0usize;
        if at > 1 {
            let (mut r, mut cur, mut fall) = (ma[0], ma[0], false);
            for (u, &v) in ma.iter().enumerate().take(at).skip(1) {
                if fall {
                    r = cur;
                }
                cur = v;
                fall = r > cur;
                if fall {
                    rise = u;
                }
            }
        }
        *val = 0.0;
        *idx = 0;
        if lo < hi {
            let mut c = 0;
            for l in lo as usize..hi as usize {
                if d[l] > dthr && ma[l] > top * 0.4 && l > rise {
                    if c >= 7 {
                        *idx = l as i32 + 8;
                        *val = acc[*idx as usize];
                        break;
                    }
                    c += 1;
                } else {
                    c = 0;
                }
            }
        }
        let (mut gmax, mut gidx) = (0.0f64, 0usize);
        for (u, &v) in acc[..mu].iter().enumerate() {
            if gmax < v {
                gidx = u;
            }
            gmax = gmax.max(v);
        }
        self.acc_max = gmax;
        let dip = acc[..mu].iter().any(|&v| v < gmax * 0.7);
        if (*idx == 0 && *val == 0.0) || !dip {
            *val = gmax;
            *idx = gidx as i32;
        }
    }

    /// `findBeatPositionForBeatSubEach` @0x10113e538 on one bar buffer: (value, rise index,
    /// unreliable).
    fn sub_each(&self, bar: usize, m: i32) -> (f64, i32, bool) {
        if m < 34 || 2 * self.half() < m {
            return (0.0, 0, false);
        }
        let mu = m as usize;
        let buf = &self.bars[bar];
        let (ma, top, at) = Self::moving_avg(buf, mu);
        let cnt = mu - 33;
        let d: Vec<f64> = (0..cnt).map(|i| ma[i + 1] - ma[i]).collect();
        let dmax = d.iter().fold(0.0f64, |a, &x| a.max(x));
        let dsum = d.iter().fold(0.0f64, |a, &x| x + a);
        let var = crate::hpbeat::focus::neon_sumsq8(&d, dsum / cnt as f64);
        let (r1, r2) = if self.acc_max.abs() > f64::EPSILON {
            (dmax / self.acc_max, var / (self.acc_max * cnt as f64))
        } else {
            (0.0, 0.0)
        };
        let pk = find_peaks(&ma);
        let k = pk.locs.len();
        let mut sel = 0i32;
        if k > 0 && k < ma.len() {
            let maxval = pk.vals.iter().fold(0.0f64, |a, &x| a.max(x));
            let maxprom = pk.proms.iter().fold(0.0f64, |a, &x| a.max(x));
            if let Some(u) =
                (0..k).find(|&u| pk.vals[u] > maxval * 0.75 && pk.proms[u] > maxprom * 0.25)
            {
                sel = pk.locs[u];
            }
        }
        // The moving average's minimum before its maximum.
        let (mut mn, mut mi) = (ma[0], 0i32);
        if at > 1 {
            for (u, &v) in ma.iter().enumerate().take(at).skip(1) {
                if mn > v {
                    mi = u as i32;
                }
                mn = mn.min(v);
            }
        }
        let lo4 = top * 0.4;
        let thr = if lo4 < mn {
            (top - mn).mul_add(0.4, mn)
        } else {
            lo4
        };
        let (mut val, mut first, mut second) = (0.0f64, 0i32, 0i32);
        if mi + 1 < sel {
            let mut c = 0;
            for i in (mi + 1)..sel {
                let iu = i as usize;
                if d[iu] > dmax * 0.25 && ma[iu] > thr {
                    if c >= 4 {
                        first = i + 11;
                        val = buf[first as usize];
                        break;
                    }
                    c += 1;
                } else {
                    c = 0;
                }
            }
            let mut c = 0;
            for i in (mi + 1)..sel {
                if ma[i as usize] > thr {
                    if c >= 9 {
                        second = i + 6;
                        break;
                    }
                    c += 1;
                } else {
                    c = 0;
                }
            }
        }
        // The bar buffer's own shape.
        let (mut bmax, mut barg, mut bmin) = (buf[0], 0usize, buf[0]);
        for (u, &v) in buf[..mu].iter().enumerate().skip(1) {
            if bmax < v {
                barg = u;
            }
            bmax = bmax.max(v);
            bmin = bmin.min(v);
        }
        let (r3, r4) = if bmax.abs() > f64::EPSILON {
            (bmin / bmax, (bmax - bmin) / bmax)
        } else {
            (0.0, 0.0)
        };
        let min2 = buf[barg..mu].iter().fold(buf[barg], |a, &x| a.min(x));
        let range = bmax - min2;
        let (mut above, mut sum) = (0, 0.0f64);
        let thr4 = range * 0.25;
        for &v in &buf[barg..mu] {
            let e = v - min2;
            if e > thr4 {
                above += 1;
                sum += e;
            } else {
                sum += -0.0;
            }
        }
        let rng = if range.abs() > f64::EPSILON {
            range
        } else {
            1.0
        };
        let dip = buf[..mu].iter().any(|&v| v < bmax * 0.7);
        let unsure = (first == 0 && val == 0.0)
            || !dip
            || second < first - 20
            || (above <= 19 && sum / rng < 10.0)
            || (r1 < 0.0015 && r2 < 0.1)
            || !(r3 <= 0.35 || r4 >= 0.65);
        (val, first, unsure)
    }

    /// `offsetBeatPosition` @0x10113d304: moves every beat by `(idx − w)·4` samples, drops beats
    /// before the start (and those whose next shift would pass the end), then refills whole beat
    /// lengths (`rate/4 · 0.004 · tempo`, single precision) at both ends.
    fn offset_beat_position(&mut self, w: i32, idx: i32, list: &mut Vec<BeatInfo>) {
        let m2 = (f64::from(self.rate4) * 0.05) as i32 * 2;
        if w < 0 || m2 < idx || idx < 0 || m2 < w || list.is_empty() {
            return;
        }
        let beat = (self.rate4 as f32 * (0.004f32 * list[0].tempo)) as i32;
        let shift = (idx - w) * 4;
        let (mut neg, mut over) = (0usize, 0usize);
        for b in list.iter_mut() {
            b.pos = b.pos.wrapping_add(shift);
            if b.pos < 0 {
                neg += 1;
            }
            if self.total < b.pos.wrapping_add(shift) {
                over += 1;
            }
        }
        list.drain(..neg.min(list.len()));
        let keep = list.len().saturating_sub(over);
        list.truncate(keep);
        if list.is_empty() {
            self.beats = 0;
            return;
        }
        while beat <= list[0].pos {
            let mut b = list[0];
            b.stat = if b.stat - 1 != 0 { b.stat - 1 } else { 4 };
            b.pos -= beat;
            b.flag = 0;
            list.insert(0, b);
        }
        let mut last = *list.last().unwrap();
        if last.pos + beat <= self.total {
            loop {
                let mut b = last;
                b.pos = last.pos + beat;
                b.stat = if b.stat != 4 { b.stat + 1 } else { 1 };
                b.flag = 0;
                list.push(b);
                let p = last.pos;
                last = b;
                if p + beat + beat > self.total {
                    break;
                }
            }
        }
        self.beats = list.len() as i32;
    }
}

/// Test A: for each sensor onset before 15 minutes, the low band summed over half a beat from
/// the onset (`rate · 7.5 / bpm` quarter-rate samples); a window past the band's end stops the
/// scan. A holds when onsets × the largest sum × the mean sum is below 2e15: the onsets carry
/// little low-frequency energy. `onsets` is the sensor's list as `analyze` received it, `bpm` the
/// constant pass's 60000 / first beat length.
pub fn judge(rate: i32, onsets: &[BeatInfo], bpm: f32, total: i32, low: &UnitBeatAdjust) -> bool {
    let n = onsets.len() as i32;
    let (mut peak, mut mean) = (0.0f64, 0.0f64);
    if n >= 2 {
        let mut sums = vec![0.0f64; onsets.len()];
        let mut limit = n - 1;
        let half = (f64::from(rate) * 7.5) / f64::from(bpm);
        for i in 0..(n - 1) as usize {
            let p = onsets[i].pos;
            if rate * 900 <= p {
                limit = i as i32;
                break;
            }
            let mut v = -1.0;
            if !(bpm.abs() <= f32::EPSILON || p < -3) {
                let len = (half + 0.5) as i32;
                let start = f64::from(p).mul_add(0.25, 0.5) as i32;
                let end = start + len - 1;
                if !(total / 4 < end || rate * 225 < end) {
                    if len <= 0 {
                        sums[i] = 0.0;
                        continue;
                    }
                    v = (start..start + len)
                        .map(|k| f64::from(low.low.get(k as usize).copied().unwrap_or(0)))
                        .sum();
                }
            }
            sums[i] = v;
            if i != 0 && v < 0.0 {
                limit = i as i32;
                break;
            }
        }
        if limit != 0 {
            let s = &sums[..limit as usize];
            peak = s.iter().fold(0.0, |m, &x| if m <= x { x } else { m });
            mean = s.iter().sum::<f64>() / f64::from(limit);
        }
    }
    f64::from(n) * (peak * mean) < 2e15
}
