//! BALib v1's `peakss::BeatInfoAnalyzer::analyze` @0x10118ff2c on the constant-tempo path
//! (`varied` false): picks the tempo from BPM histograms of the raw beats, aligns a grid to them
//! (`segAdjust` @0x1011944ec), follows the beats with a sliding 32-beat tempo estimate, and lays
//! a constant grid between the first and last band-confirmed beats (`gapCorrect` @0x101194e7c).
//! Spec: `analysis-beats.md` § 3.

use super::BeatInfo;
use super::sensor::tol;

/// `Analyzer::analyze` passes `(…, 4, 16, 12, varied)` (Analyzer +0x28 = 32): beats added after
/// the window, beats laid per window, beats kept behind.
const P4: i32 = 4;
const P5: i32 = 16;
const P6: i32 = 12;
const WINDOW: i32 = P4 + P5 + P6;
/// Per-band lag of the onsets in ms (`strBandOffset`, 0x1037c86a0 / 0x1037c86b0 and −3).
const OFFSETS: [f64; 6] = [-30.0, -20.0, -3.0, -3.0, -20.0, -3.0];

/// Index of the beat nearest to `key` within `w` samples, scanning from the last beat at or before
/// `key` to the first one after it (the search `findNearPos` @0x10118fd74 and `segAdjust` share).
pub(crate) fn nearest(list: &[BeatInfo], key: i32, w: i32) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    let last_le = |k: i32| list.partition_point(|b| b.pos <= k) as i32 - 1;
    let a = last_le(key).max(0);
    let mut b = last_le(key.wrapping_add(w));
    if b < 0 {
        if a > 0 {
            return None;
        }
        b = 0;
    }
    if a > b {
        return None;
    }
    let mut best_d = i32::MAX;
    let mut best = -1i32;
    let mut i = a;
    'scan: loop {
        let e = i.max(b) + 1;
        let d = loop {
            let d = key.wrapping_sub(list[i as usize].pos).wrapping_abs();
            if d <= w {
                break d;
            }
            i += 1;
            if i == e {
                break 'scan;
            }
        };
        if d < best_d {
            best = i;
        }
        if list[i as usize].pos > key {
            break;
        }
        if d < best_d {
            best_d = d;
        }
        if b <= i {
            break;
        }
        i += 1;
    }
    (best >= 0).then_some(best as usize)
}

/// First index of the histogram maximum (−1 when every bin is 0).
fn argmax(h: &[i32], from: usize) -> i32 {
    let mut m = -1;
    let mut c = 0;
    for (i, &v) in h.iter().enumerate() {
        if v > c {
            m = (from + i) as i32;
            c = v;
        }
    }
    m
}

/// The two-stage BPM histogram over the beats from `first` within `span` samples: integer BPM,
/// then BPM × 10 around the winner. Returns the beat length in ms.
fn coarse_tempo(list: &[BeatInfo], first: usize, span: i32) -> Option<f32> {
    let p0 = list[first].pos;
    let rest = || {
        list[first + 1..]
            .iter()
            .take_while(move |b| b.pos.wrapping_sub(p0) <= span)
    };
    let mut h1 = [0i32; 400];
    let b = (600_000.0 / f64::from(list[first].tempo) + 5.0) as i32 / 10;
    if let Some(c) = h1.get_mut(b as usize) {
        *c += 1;
    }
    for e in rest() {
        let v = (600_000.0 / f64::from(e.tempo) + 5.0) as i32;
        if (v.wrapping_add(9) as u32) < 4009 {
            h1[((v as i16) / 10) as u16 as usize] += 1;
        }
    }
    let bpm = argmax(&h1, 0);
    if bpm < 0 {
        return None;
    }
    let mut h2 = vec![0i32; 4000];
    let b = (6_000_000.0 / f64::from(list[first].tempo) + 5.0) as i32 / 10;
    if let Some(c) = h2.get_mut(b as usize) {
        *c += 1;
    }
    for e in rest() {
        let v = (6_000_000.0 / f64::from(e.tempo) + 5.0) as i32;
        if (v.wrapping_add(9) as u32) < 40009 {
            h2[(v / 10) as usize] += 1;
        }
    }
    let lo = f64::from(bpm).mul_add(10.0, -10.0) as i32;
    let lo = if lo > 0 { lo } else { 0 };
    let hi = f64::from(bpm).mul_add(10.0, 10.0) as i32;
    let hi = if hi < 4000 { hi } else { 4000 };
    if lo >= hi {
        return None;
    }
    let bpm10 = argmax(&h2[lo as usize..hi as usize], lo as usize);
    if bpm10 < 0 {
        return None;
    }
    Some((600_000.0 / f64::from(bpm10)) as f32)
}

/// `sdiv`/`msub` remainder: AArch64 division by zero yields 0, so the remainder is `a`.
fn rem(a: i32, b: i32) -> i32 {
    if b == 0 { a } else { a.wrapping_rem(b) }
}

fn within(x: f32, lo: f32, hi: f32) -> bool {
    x > lo && x < hi
}

/// `t`, `2t` or `t/2` within `c ± tol(c)` (or the given bounds).
fn near_tempo(t: f32, lo: f32, hi: f32) -> bool {
    within(t, lo, hi) || within(t + t, lo, hi) || within(t * 0.5, lo, hi)
}

pub(crate) struct Result {
    /// The returned BPM (`60000 / first beat's length`).
    pub bpm: f32,
    pub beats: Vec<BeatInfo>,
}

struct Builder<'a> {
    rate: i32,
    list: &'a [BeatInfo],
    out: Vec<BeatInfo>,
    /// First and last output beats that carry a band (`sp+0x160`, `sp+0x150`).
    first: i32,
    last: i32,
}

fn beat(pos: i32, tempo: f32, band: i32, anchor: bool) -> BeatInfo {
    BeatInfo {
        pos,
        beats: 0.0,
        tempo,
        stat: 0,
        band,
        flag: u8::from(anchor),
    }
}

impl Builder<'_> {
    /// The forward fills' bookkeeping: remembers the first and the latest band-confirmed beat.
    fn mark(&mut self, rem: i32, band: i32) {
        if rem == 0 && (band as u32) <= 5 {
            let idx = self.out.len() as i32 - 1;
            if self.first < 0 {
                self.first = idx;
            } else {
                self.last = idx;
            }
        }
    }

    fn push_anchor(&mut self, pos: f64, t: f32, band: i32) {
        self.out.push(beat(pos as i32, t, band, true));
        if (band as u32) < 6 && self.first < 0 {
            self.first = self.out.len() as i32 - 1;
        }
    }

    /// Beats `seg_band + 1 … 1` periods before the anchor, skipping those before `floor`.
    /// Returns the updated output count base (`local_1b0`).
    #[allow(clippy::too_many_arguments)]
    fn back_fill(
        &mut self,
        seg_band: i32,
        base: i32,
        anchor: f64,
        p: f64,
        t: f32,
        floor: i32,
    ) -> i32 {
        let mut w26 = base;
        if seg_band < 0 {
            return w26;
        }
        let mut w22 = -1i32;
        let mut w28 = 1i32;
        loop {
            let pos = (-p).mul_add(f64::from(seg_band + w28), anchor) as i32;
            if pos < floor {
                w26 += if w22 < 0 { 0 } else { -1 };
            } else {
                self.out.push(beat(pos, t, -1, false));
                w26 += i32::from(w22 < 0);
            }
            w22 += 1;
            w28 -= 1;
            if seg_band <= w22 {
                break;
            }
        }
        w26
    }
}

/// `analyze(info, out, total, 4, 16, 12, false)`: the constant-tempo beat list and its BPM.
pub(crate) fn analyze(rate: i32, list: &mut Vec<BeatInfo>, total: i32) -> Option<Result> {
    if list.is_empty() {
        return None;
    }
    // The tempo of the whole track, then drop the beats unrelated to it or to their predecessor.
    let t0 = coarse_tempo(list, 0, total)?;
    let tl = tol(t0);
    let (lo, hi) = (t0 - tl, t0 + tl);
    let mut prev = tl;
    let mut i = 0;
    while i < list.len() {
        let t = list[i].tempo;
        let p = prev;
        prev = t;
        let keep = near_tempo(t, lo, hi) || {
            let tp = tol(p);
            near_tempo(t, p - tp, p + tp)
        };
        if keep {
            i += 1;
        } else {
            list.remove(i);
        }
    }
    if list.is_empty() {
        return None;
    }
    // One segment over the whole track; its tempo from the filtered beats.
    let mut seg = list[0];
    seg.tempo = t0;
    if let Some(t1) = coarse_tempo(list, 0, total) {
        seg.tempo = t1;
        seg.beats = 0.0;
        seg.stat = 4;
    }
    // Merge beats closer than 1/8 beat, keeping the one from the stronger call.
    let mut strength: Vec<i32> = Vec::new();
    for b in list.iter() {
        let id = (b.stat >> 8) as usize;
        if strength.len() < id + 1 {
            strength.resize(id + 1, 0);
        }
        if strength[id] == 0 {
            strength[id] = b.stat & 0xff;
        }
    }
    let close = (f64::from(seg.tempo * rate as f32) * 0.000125) as i32;
    let (mut prev_pos, mut prev_stat) = (list[0].pos, list[0].stat);
    let mut i = 1;
    while i < list.len() {
        let e = list[i];
        if e.pos >= total {
            break;
        }
        if prev_pos.wrapping_sub(e.pos).wrapping_abs() >= close {
            prev_pos = e.pos;
            prev_stat = e.stat;
            i += 1;
        } else if strength[(prev_stat >> 8) as usize] < strength[(e.stat >> 8) as usize] {
            list.remove(i - 1);
            prev_pos = e.pos;
            prev_stat = e.stat;
        } else {
            list.remove(i);
        }
    }
    seg_adjust(rate, list, &mut seg, total);

    let mut b = Builder {
        rate,
        list,
        out: Vec::new(),
        first: -1,
        last: -1,
    };
    let anchor0 = b.build(&seg, total)?;
    let n = b.out.len();
    if n == 0 {
        return None;
    }
    let _ = anchor0;
    let mark = if b.first >= 0 {
        b.out[b.first as usize].pos
    } else {
        0
    };
    let mut out = gap_correct(rate, &b.out, total, b.first, b.last);
    let bpm = 60000.0 / out.first()?.tempo;
    // Bar positions: the beat at the first confirmed anchor is beat 1.
    let idx = if out.is_empty() {
        -1
    } else {
        (out.partition_point(|x| x.pos <= mark) as i32 - 1).max(0)
    };
    let c = 4 + (idx / 4) * 4 - idx;
    for (i, x) in out.iter_mut().enumerate() {
        x.stat = (c + i as i32) % 4 + 1;
    }
    out.retain(|x| x.pos >= 0 && x.pos <= total);
    Some(Result { bpm, beats: out })
}

impl Builder<'_> {
    /// The grid walk of `analyze` (0x101192090–0x101193a48) for the single segment.
    /// Returns the first anchor position (`sp+0x74`).
    fn build(&mut self, seg: &BeatInfo, total: i32) -> Option<i32> {
        let rate = self.rate;
        let p48 = f64::from(rate) * 0.001;
        let inv_rate = 1.0 / f64::from(rate);
        let inv_window = 1.0 / f64::from(WINDOW);
        let end = f64::from(total);
        let floor = 0i32;
        let base0 = 0i32;
        let mut base = base0;
        let mut tab = [-1i32; (WINDOW * 4 + 1) as usize];
        let mut g = f64::from(seg.pos);
        let t_seg = seg.tempo;
        let mut p = p48 * f64::from(t_seg);
        let mut cur = g;
        if seg.band >= 1 {
            let mut k = 1;
            loop {
                cur += p;
                g += p;
                if !(cur < end && k < seg.band) {
                    break;
                }
                k += 1;
            }
        }
        let mut maxoff = f64::MIN;
        for o in OFFSETS {
            maxoff = maxoff.max(o);
        }
        let spacing = seg.stat;
        tab[0] = -1;
        let w_beat = (p * 0.125) as i32;
        let win = (rate / 20).max(w_beat);
        let mut anchor = match nearest(self.list, cur as i32, win) {
            Some(idx) if self.list[idx].pos >= 0 => {
                let e = self.list[idx];
                tab[0] = e.band;
                cur = f64::from(e.pos);
                OFFSETS.get(e.band as usize).copied().unwrap_or(maxoff) * p48 + cur
            }
            _ => g + maxoff * p48,
        };
        let anchor0 = anchor as i32;
        let q = p * 0.25;
        if !(q > 0.0) {
            return None;
        }
        let mut t = t_seg;
        let mut w19 = 0i32;
        if cur >= end {
            return Some(anchor0);
        }
        let seg_band = seg.band;
        loop {
            let mut x21 = w19;
            for _ in 0..4 {
                cur += q;
                x21 += 1;
                tab[x21 as usize] = -1;
                if rem(x21, spacing) == 0 {
                    let w = (rate / 20).max(w_beat);
                    if let Some(idx) = nearest(self.list, cur as i32, w) {
                        let e = self.list[idx];
                        if e.pos >= 0 {
                            cur = f64::from(e.pos);
                            tab[x21 as usize] = e.band;
                        }
                    }
                }
            }
            if x21 % (WINDOW * 4) == 0 {
                // A full 32-beat window: re-estimate the beat length from the drift.
                let bb = tab[x21 as usize];
                let off = if (bb as u32) <= 5 {
                    OFFSETS[bb as usize]
                } else {
                    maxoff
                };
                p = off.mul_add(p48, cur - anchor) * inv_window;
                t = ((p * 1000.0) * inv_rate) as f32;
                if seg_band + base >= self.out.len() as i32 {
                    base = self.back_fill(seg_band, base, anchor, p, t, floor);
                    self.push_anchor(anchor, t, tab[0]);
                }
                if base + P5 + seg_band >= self.out.len() as i32 && P5 >= 1 {
                    for x28 in (0..P5 * 4).step_by(4) {
                        anchor += p;
                        if anchor >= end {
                            break;
                        }
                        let qi = x28 + 4;
                        let rem = rem(qi, spacing);
                        let band = if rem == 0 { tab[qi as usize] } else { -1 };
                        self.out.push(beat(anchor as i32, t, band, false));
                        self.mark(rem, band);
                    }
                }
                let last = self.out.last_mut()?;
                last.tempo = t;
                let mut d = f64::from(last.pos);
                let mut qi = P5 * 4 + 4;
                for _ in 0..P4 {
                    d += p;
                    if d >= end {
                        break;
                    }
                    let rem = rem(qi, spacing);
                    let band = if rem == 0 { tab[qi as usize] } else { -1 };
                    self.out.push(beat(d as i32, t, band, false));
                    self.mark(rem, band);
                    qi += 4;
                }
                let n = self.out.len() as i32;
                anchor = f64::from(self.out.get((n - P5 - 1).max(0) as usize)?.pos);
                w19 = x21 - P4 * 4;
                if P5 * 4 <= w19 {
                    // Slide the band table by P4 beats (from quarter P5·4 on, as rekordbox does).
                    for i in (P5 * 4) as usize..(x21 - 15) as usize {
                        tab[i] = tab[i + (P4 * 4) as usize];
                    }
                }
            } else {
                w19 = x21;
            }
            if cur >= end {
                break;
            }
        }
        // The partial window at the end of the track.
        let mut d;
        let ext_from_anchor = w19 % (WINDOW * 4) == 0 || (w19.wrapping_add(3) as u32) < 7;
        if ext_from_anchor {
            d = anchor;
        } else {
            let w9 = if w19 < 0 { w19 + 3 } else { w19 };
            let mut c8 = maxoff;
            if seg_band + base >= self.out.len() as i32 {
                let bb = tab[w19 as usize];
                let off = if (bb as u32) <= 5 {
                    OFFSETS[bb as usize]
                } else {
                    c8
                };
                p = (off.mul_add(p48, cur - anchor) * 4.0) / f64::from(w19);
                t = ((p * 1000.0) * inv_rate) as f32;
                base = self.back_fill(seg_band, base, anchor, p, t, floor);
                self.push_anchor(anchor, t, tab[0]);
            }
            if base + P5 + seg_band >= self.out.len() as i32 {
                let bb = tab[w19 as usize];
                if (bb as u32) <= 5 {
                    c8 = OFFSETS[bb as usize];
                }
                p = (c8.mul_add(p48, cur - anchor) * 4.0) / f64::from(w19);
                t = ((p * 1000.0) * inv_rate) as f32;
                if P5 >= 1 {
                    for x22 in (0..P5 * 4).step_by(4) {
                        anchor += p;
                        if anchor >= end {
                            break;
                        }
                        let qi = x22 + 4;
                        let rem = rem(qi, spacing);
                        let band = if rem == 0 { tab[qi as usize] } else { -1 };
                        self.out.push(beat(anchor as i32, t, band, false));
                        self.mark(rem, band);
                    }
                }
            }
            d = f64::from(self.out.last()?.pos);
            let extra = (w9 >> 2) - P5;
            let mut qi = P5 * 4 + 4;
            for _ in 0..extra.max(0) {
                d += p;
                if d >= end {
                    break;
                }
                let rem = rem(qi, spacing);
                let band = if rem == 0 { tab[qi as usize] } else { -1 };
                self.out.push(beat(d as i32, t, band, false));
                self.mark(rem, band);
                qi += 4;
            }
        }
        if d < end && p > 0.0 {
            loop {
                d += p;
                if d >= end {
                    break;
                }
                self.out.push(beat(d as i32, t, -1, false));
            }
        }
        if let Some(last) = self.out.last_mut() {
            last.tempo = (p / p48) as f32;
        }
        Some(anchor0)
    }
}

/// `segAdjust`: the grid phase (in 96-sample steps), beat spacing (16, 8 or 4) and tempo offset
/// (0, +1, −1, +2, −2 ms) whose grid meets the beats best.
fn seg_adjust(rate: i32, list: &[BeatInfo], seg: &mut BeatInfo, total: i32) {
    let d0 = f64::from(rate) * 0.001;
    let end = f64::from(total);
    let start = 0.0f64;
    let w12 = rate / 20;
    let t0 = f64::from(seg.tempo);
    let mut first = true;
    let mut best_ph = -1.0f64;
    let mut best = 0.0f64;
    let mut off = 0.0f64;
    let (mut r16, mut r17, mut r18) = (0.0f64, 0.0f64, 0.0f64);
    'tempo: loop {
        let t = off + t0;
        let p = d0 * t;
        let w13 = (p * 0.125) as i32;
        let w = if w12 > w13 { w12 } else { w13 };
        let thresh = (f64::from(w) * (0.5 * (end - start))) / p;
        let inv_p = 1.0 / p;
        let mut m = 4.0f64;
        loop {
            let step = m * p;
            let hi = (step + start).min(end);
            if start < hi {
                let m4 = m * 4.0;
                let mut ph = start;
                loop {
                    let mut rec: Option<(f64, f64)> = None;
                    if ph >= end {
                        if best < 0.0 {
                            rec = Some((ph, 0.0));
                        }
                    } else {
                        let mut score = 0.0f64;
                        let mut fm = -1.0f64;
                        let mut g = ph;
                        loop {
                            let key = g as i32;
                            if let Some(idx) = nearest(list, key, w) {
                                let pos = list[idx].pos;
                                if pos >= 0 {
                                    score += f64::from(w - pos.wrapping_sub(key).wrapping_abs());
                                    g = f64::from(pos);
                                    if !(fm >= 0.0) {
                                        fm = g;
                                    }
                                }
                            }
                            g += step;
                            if g >= end {
                                break;
                            }
                        }
                        if score > best {
                            rec = Some((if fm < 0.0 { ph } else { fm }, score));
                        }
                    }
                    if let Some((from, score)) = rec {
                        r16 = ((from - start) * inv_p).trunc();
                        best_ph = (-p).mul_add(r16, from);
                        r17 = m4;
                        r18 = t;
                        best = score;
                    }
                    ph += 96.0;
                    if ph >= hi {
                        break;
                    }
                }
            }
            if first && best_ph >= 0.0 && best * m > thresh {
                break 'tempo;
            }
            m *= 0.5;
            best += best;
            if m < 1.0 {
                break;
            }
        }
        first = false;
        while m < 4.0 {
            best *= 0.5;
            m += m;
        }
        off = if off != 0.0 { -off } else { off };
        off = if off < 0.0 { off } else { off + 1.0 };
        if !(off < 3.0) {
            break;
        }
    }
    if best_ph >= 0.0 {
        seg.pos = best_ph as i32;
        seg.tempo = r18 as f32;
        seg.stat = r17 as i32;
        seg.band = r16 as i32;
    }
}

/// `gapCorrect(out, total, first, last)`: a constant grid through the first and last
/// band-confirmed beats, from the start of the track to `total`.
fn gap_correct(rate: i32, out: &[BeatInfo], total: i32, first: i32, last: i32) -> Vec<BeatInfo> {
    let a = first.max(0);
    let b = if last < 0 { out.len() as i32 - 1 } else { last };
    let xa = f64::from(out[a as usize].pos);
    let xb = f64::from(out[b as usize].pos);
    let span = b - a;
    let period = (xb - xa) / f64::from(span);
    let k = (xa / period) as i32;
    let mut d = (-period).mul_add((xa / period).trunc(), xa);
    let band_a = out[a as usize].band;
    let band_b = out[b as usize].band;
    let t = (period / (f64::from(rate) * 0.001)) as f32;
    let mut grid = Vec::new();
    if k >= 0 {
        let mut left = k;
        grid.push(beat(d as i32, t, -1, false));
        loop {
            d += period;
            if left == 0 {
                break;
            }
            left -= 1;
            grid.push(beat(d as i32, t, -1, false));
        }
    }
    if let Some(x) = grid.get_mut(k as usize) {
        x.flag = 1;
        x.band = band_a;
    }
    for _ in 0..span.max(0) {
        grid.push(beat(d as i32, t, -1, false));
        d += period;
    }
    let m = ((f64::from(total) - xb) / period) as i32;
    if let Some(x) = grid.get_mut((span + k) as usize) {
        x.band = band_b;
    }
    for _ in 0..m.max(0) {
        grid.push(beat(d as i32, t, -1, false));
        d += period;
    }
    grid
}
