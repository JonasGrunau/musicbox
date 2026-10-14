//! Focus's constant path (`analyzeFocus` from 0x10116c47c, code 0x10117996c–0x101181240): taken
//! without the tempo-change flag when the madmom list starts on a bar (its largest beat number
//! among the first four is 4), so only in Normal and Auto mode (`analysis-beats-hp.md` § 5).
//!
//! It turns the madmom beats into one constant grid: a reference interval from the longest
//! stable stretch, the analyser's tempo checked against it, irregular stretches rebuilt as even
//! beats, a phase anchor (from a stable stretch, or from where the low band comes in when the
//! stretches disagree by half a beat), a uniform grid through the anchor, BALib's offset added,
//! the grid run out to both ends of the track, `UnitBeatAdjust::analyze` on the result, and the
//! bar-number vote (`vote.rs`).

use super::{BeatInfo, Focus};
use crate::balib2::adjust::UnitBeatAdjust;
use crate::hpbeat::focus::neon_sum8;

/// Up to 128 stretches of at least two intervals that differ from the reference by more than
/// `limit` ms: first beat and one past the last (1-based like the binary's arrays).
fn stretches(e: &[f64], n: i32, limit: f64) -> ([i32; 128], [i32; 128]) {
    let (mut st, mut en) = ([0i32; 128], [0i32; 128]);
    if n < 3 {
        return (st, en);
    }
    let (mut k, mut i) = (0usize, 0i32);
    loop {
        let j = i;
        i = j + 1;
        if e[j as usize] > limit && e[(j + 1) as usize] > limit {
            st[k] = j + 2;
            let mut p = j + 1;
            if p < n - 2 {
                loop {
                    p += 1;
                    if !(e[p as usize] > limit && p < n - 2) {
                        break;
                    }
                }
            }
            i = p + 1;
            en[k] = p + 1;
            k += 1;
        }
        if !(i < n - 2 && k < 128) {
            break;
        }
    }
    (st, en)
}

/// The number of stretches before the first empty slot (`cap` when the scan runs out).
fn stretch_count(st: &[i32; 128], cap: usize) -> usize {
    st.iter().position(|&s| s == 0).unwrap_or(cap)
}

/// Whether more than a quarter of the stable segments between stretches sit half a beat apart
/// (0.4–0.6 of a beat) from one another (0x10117bc1c–0x10117be3c).
fn half_beat_apart(mids: &[i32], pos: &[i32], beat: f64) -> bool {
    let k = mids.len();
    let (mut apart, mut not) = (0i32, 0i32);
    for i in 0..k.saturating_sub(1) {
        for j in i + 1..k {
            let p = |m: i32| pos.get(m as usize).map_or(0.0, |&x| f64::from(x));
            let x = ((p(mids[i]) - p(mids[j])) * (1.0 / beat)).abs();
            let frac = x - x.trunc();
            if !(frac >= 0.6) && frac > 0.4 {
                apart += 1;
            } else {
                not += 1;
            }
        }
    }
    f64::from(apart + not) * 0.25 < f64::from(apart)
}

impl Focus {
    /// Beat `i`, reading past the end into the beats a shorter vector cut (zero beyond them).
    fn at(&self, i: i32) -> BeatInfo {
        let n = self.b.len() as i32;
        if i < n {
            self.b.v[i as usize]
        } else {
            self.ghost
                .get((i - n) as usize)
                .copied()
                .unwrap_or_default()
        }
    }

    /// The constant path. `adjust` is the analyser's `UnitBeatAdjust` (its bands).
    pub(crate) fn constant(&mut self, adjust: &mut UnitBeatAdjust) {
        let n = self.b.len() as i32;
        let nu = n as usize;
        let rate = f64::from(self.inp.rate);
        // Beat times rounded to the ms (`(p·1000)·(1/rate)` throughout) and whole-ms intervals.
        let inv = 1.0 / rate;
        let t: Vec<f64> = self
            .b
            .v
            .iter()
            .map(|b| (f64::from(b.pos) * 1000.0 * inv).round() * 0.001)
            .collect();
        let mut iv = vec![0.0f64; nu];
        for i in 0..nu.saturating_sub(1) {
            iv[i] = (t[i + 1] - t[i]).mul_add(1000.0, 0.5).trunc();
        }
        let (r, ref_start, ref_end) = reference(&iv, n);
        let halved = self.check_tempo(r);
        self.halved |= halved;
        let tempo = self.bpm;

        // Interval deviations from the reference (the last repeated), times in ms.
        let mut e = vec![0.0f64; nu];
        for i in 0..nu.saturating_sub(1) {
            e[i] = (iv[i] - r).abs();
        }
        if nu >= 2 {
            e[nu - 1] = e[nu - 2];
        }
        let ms: Vec<f64> = t.iter().map(|&x| x * 1000.0).collect();
        let (st10, en10) = stretches(&e, n, 10.0);
        let beat = (rate * 60.0) / f64::from(tempo);
        let covered: i32 = st10
            .iter()
            .zip(&en10)
            .take_while(|(s, _)| **s >= 1)
            .map(|(s, e)| e - s + 1)
            .sum();

        // Rebuild the times: irregular stretches become evenly spaced beats between their ends.
        let mut times: Vec<f64> = Vec::with_capacity(nu);
        let (mut st20, mut en20) = ([0i32; 128], [0i32; 128]);
        let mut len20 = [0i32; 128];
        let mut added20 = [0i32; 128];
        let use10 = 0.5 <= f64::from(covered) / f64::from(n);
        let (st, en) = if use10 {
            (st10, en10)
        } else {
            let s = stretches(&e, n, 20.0);
            st20 = s.0;
            en20 = s.1;
            (st20, en20)
        };
        times.push(t[0]);
        for s in 0..128 {
            let (a, b) = (st[s], en[s]);
            if a < 1 {
                break;
            }
            if !use10 {
                len20[s] = b - a + 1;
            }
            let (hi, lo) = (
                if b < n { ms[b as usize] } else { ms[nu - 1] },
                if a == 1 { ms[0] } else { ms[(a - 2) as usize] },
            );
            let span = (hi - lo) * (1.0 / r);
            let (count, step) = if b < n {
                let g = span.round() as i32;
                if g == 0 {
                    (-1, r)
                } else {
                    (g - 1, (hi - lo) / f64::from(g))
                }
            } else {
                (span.round() as i32, r)
            };
            if !use10 {
                added20[s] = count;
            }
            let fill = |times: &mut Vec<f64>, from: usize| {
                for j in 1..=count {
                    times.push(f64::from(j).mul_add(step, ms[from]).round() * 0.001);
                }
            };
            if s == 0 {
                if a > 1 {
                    if a != 2 {
                        times.extend_from_slice(&t[1..(a - 1) as usize]);
                    } else {
                        if count < 1 {
                            continue;
                        }
                        fill(&mut times, 0);
                        if b < n - 1 {
                            times.push(t[b as usize]);
                        }
                        continue;
                    }
                } else {
                    if count < 1 || n - 1 <= b {
                        continue;
                    }
                    times.push(t[b as usize]);
                    continue;
                }
            } else {
                let from = en[s - 1] + 1;
                if from < a - 1 {
                    times.extend_from_slice(&t[from as usize..(a - 1) as usize]);
                }
            }
            if count > 0 {
                if a > 1 {
                    fill(&mut times, (a - 2) as usize);
                }
                if b < n - 1 {
                    times.push(t[b as usize]);
                }
            }
        }
        if st[0] == 0 {
            if n > 1 {
                times.extend_from_slice(&t[1..]);
            }
        } else {
            let last = en[stretch_count(&st, 128).max(1) - 1];
            if last < n && last + 1 < n {
                times.extend_from_slice(&t[(last + 1) as usize..]);
            }
        }

        // The beat vector takes the new count (copies of the last madmom beat, or cut).
        // (A shorter vector keeps the cut beats in memory past its end; the binary reads them
        // when an anchor region reaches past the last beat.)
        let last_mad = *self.b.v.last().unwrap();
        let count = times.len();
        if count > nu {
            self.b.v.resize(count, last_mad);
        } else if count < nu {
            self.ghost = self.b.v.split_off(count);
        }
        let ci = count as i32;

        // Do the stable segments between stretches disagree by half a beat?
        let n10 = stretch_count(&st10, 128) % 128;
        let n20 = if st20[0] == 0 {
            0
        } else {
            stretch_count(&st20, 128) % 128
        };
        let mids = |st: &[i32; 128], en: &[i32; 128], k: usize| -> Vec<i32> {
            (0..k)
                .map(|s| {
                    if s == 0 {
                        st[0] / 2
                    } else {
                        (st[s] + en[s - 1]) / 2
                    }
                })
                .collect()
        };
        // positions as in memory: the vector, then what a cut left behind
        let pos_now: Vec<i32> = self.b.v.iter().chain(&self.ghost).map(|b| b.pos).collect();
        let odd10 = n10 > 0 && half_beat_apart(&mids(&st10, &en10, n10), &pos_now, beat);
        let odd20 = n20 > 0 && half_beat_apart(&mids(&st20, &en20, n20), &pos_now, beat);

        let (mut anchor, mut lo_reg, mut hi_reg) = (0i32, 0i32, 0i32);
        let (anchor_pos, anchor_stat);
        if odd10 || odd20 {
            // The anchor is where the low band comes in.
            let idx = self.low_start(adjust, &pos_now, ci);
            anchor_pos = self.at(idx).pos;
            anchor_stat = self.at(idx).stat;
            let (r_lo, r_hi) = region(idx, ci, &st10, &en10, n10, &st20, &en20, n20);
            lo_reg = r_lo;
            hi_reg = r_hi;
            anchor = idx;
        } else {
            anchor_pos = self.at(ref_start).pos;
            anchor_stat = self.at(ref_start).stat;
        }

        // The rebuilt times become the positions (tempo cleared).
        let pairs = if count >= 2 { count & !1 } else { 0 };
        let rk = rate * 0.001;
        for (i, b) in self.b.v.iter_mut().enumerate() {
            let m = (times[i] * 1000.0).round();
            b.pos = if i < pairs {
                (m * rk) as i64 as i32
            } else {
                (m * 0.001 * rate) as i32
            };
            b.tempo = 0.0;
        }

        // Bar numbers continue from the beat nearest the anchor's old position.
        let near = nearest(&self.b.v, anchor_pos, ci);
        let mut base = anchor_stat - 1 - (near & 3);
        if base < 0 {
            base += 4;
        }
        for (k, b) in self.b.v.iter_mut().enumerate() {
            b.stat = (base + k as i32) % 4 + 1;
        }
        for k in 0..(ci - 1).max(0) as usize {
            let d = self.b.v[k + 1].pos - self.b.v[k].pos;
            self.b.v[k].tempo = ((f64::from(d) * 1000.0) / rate) as f32;
        }
        if count >= 2 {
            self.b.v[count - 1].tempo = self.b.v[count - 2].tempo;
        }

        // Half-beat check of the grid through a stable-stretch anchor.
        let mut flip = 0.0f64;
        if !odd10 && !odd20 {
            let mut shift = 0;
            if st20[0] >= 1 {
                for s in 0..128 {
                    if en20[s] <= ref_start {
                        shift += added20[s] - len20[s];
                    }
                    if s == 127 || st20[s + 1] <= 0 {
                        break;
                    }
                }
            }
            lo_reg = ref_start + shift;
            hi_reg = ref_end + shift;
            anchor = (lo_reg + hi_reg) / 2;
            let s = self.at(anchor).stat;
            if s != 1 {
                anchor = anchor - s + 1;
            }
            flip = self.half_flip(anchor, lo_reg, hi_reg, beat, ci);
        }

        // Mean residual of the anchor region against the grid through the anchor.
        let pa = f64::from(self.at(anchor).pos);
        let mut mean = 0.0;
        if hi_reg - lo_reg >= 0 {
            let cnt = (hi_reg - lo_reg + 1) as usize;
            let res: Vec<f64> = (lo_reg..=hi_reg)
                .map(|k| f64::from(self.at(k).pos) - beat.mul_add(f64::from(k - anchor), pa))
                .collect();
            let vec_n = if hi_reg - lo_reg > 7 {
                let r8 = cnt & 7;
                cnt - if r8 == 0 { 8 } else { r8 }
            } else {
                0
            };
            let mut sum = if vec_n > 0 {
                neon_sum8(&res[..vec_n])
            } else {
                0.0
            };
            for &x in &res[vec_n..] {
                sum += x;
            }
            mean = sum / cnt as f64;
        }

        // The grid: rounded, bar numbers from `base`, beat lengths from the unrounded grid.
        let start = (mean + flip) + pa;
        let grid: Vec<f64> = (0..ci)
            .map(|k| beat.mul_add(f64::from(k - anchor), start))
            .collect();
        for (k, b) in self.b.v.iter_mut().enumerate() {
            b.pos = (grid[k] + 0.5) as i32;
            b.stat = (base + k as i32) % 4 + 1;
        }
        if ci >= 2 {
            for k in 0..count - 1 {
                self.b.v[k].tempo = (((grid[k + 1] - grid[k]) * 1000.0) / rate) as f32;
            }
            self.b.v[count - 1].tempo = self.b.v[count - 2].tempo;
        }
        for b in self.b.v.iter_mut() {
            b.pos += self.inp.offset;
        }

        // Run the grid out to both ends of the track.
        let first = self.b.v[0];
        let last = *self.b.v.last().unwrap();
        let tempo_ms = 60000.0f32 / self.bpm;
        let before = (f64::from(first.pos) / beat) as i32;
        for j in 1..=before.max(0) {
            let p = ((-beat).mul_add(f64::from(j), 0.5) + f64::from(first.pos)) as i32;
            let rem = (first.stat - 1 - j) % 4;
            let stat = if rem < 0 { rem + 5 } else { rem + 1 };
            self.b.insert(
                0,
                BeatInfo {
                    pos: p.max(0),
                    beats: 0.0,
                    tempo: tempo_ms,
                    stat,
                    band: -1,
                    flag: 0,
                },
            );
        }
        let after = ((f64::from(self.inp.total) - f64::from(last.pos)) / beat) as i32;
        for j in 1..=after.max(0) {
            let p = (beat.mul_add(f64::from(j), 0.5) + f64::from(last.pos)) as i32;
            let stat = (j + last.stat - 1) % 4 + 1;
            let at = self.b.len();
            self.b.insert(
                at,
                BeatInfo {
                    pos: p.min(self.inp.total),
                    beats: 0.0,
                    tempo: tempo_ms,
                    stat,
                    band: -1,
                    flag: 0,
                },
            );
        }
        if !self.inp.varied {
            let before = (self.bpm, self.b.v.clone());
            let mut bpm = self.bpm;
            let mut v: Vec<crate::beats::BeatInfo> = self
                .b
                .v
                .iter()
                .map(|b| crate::beats::BeatInfo {
                    pos: b.pos,
                    beats: b.beats,
                    tempo: b.tempo,
                    stat: b.stat,
                    band: b.band,
                    flag: b.flag,
                })
                .collect();
            adjust.analyze(&mut bpm, &mut v);
            self.b.v = v
                .iter()
                .map(|b| BeatInfo {
                    pos: b.pos,
                    beats: b.beats,
                    tempo: b.tempo,
                    stat: b.stat,
                    band: b.band,
                    flag: b.flag,
                })
                .collect();
            self.bpm = bpm;
            self.adjust_io = Some((before, (self.bpm, self.b.v.clone())));
        }
        self.vote_bars(adjust);
    }

    /// The analyser's tempo (+0x48) checked against the reference interval (0x10117a320–
    /// 0x10117a47c): snapped to a whole BPM within 0.05 %, doubled or halved at an octave, or
    /// replaced by the reference tempo outside 0.8–1.25×. Returns the halved flag (`analyze`'s
    /// out flag).
    fn check_tempo(&mut self, r: f64) -> bool {
        let mut v = self.bpm;
        let w = v.round();
        let (vd, wd) = (f64::from(v), f64::from(w));
        if wd * 0.9995 <= vd && !(wd * 1.0005 < vd) {
            v = w;
            self.bpm = w;
        }
        let vd = f64::from(v);
        let rt = 60000.0 / r;
        if vd * 1.98 <= rt && rt <= vd * 2.02 {
            if v <= 90.0 {
                self.bpm = v + v;
            }
        } else if vd * 0.495 <= rt && rt <= vd * 0.505 {
            if 140.0 <= v {
                self.bpm = v * 0.5;
            }
        } else if !(vd * 0.8 <= rt && rt <= vd * 1.25) {
            if 70.0 <= rt {
                let s = rt as f32;
                if rt > 180.0 {
                    self.bpm = s * 0.5;
                    return true;
                }
                self.bpm = s;
            } else {
                self.bpm = rt as f32 + rt as f32;
            }
        }
        false
    }

    /// The first beat where the low band (two beats from the beat plus BALib's offset, scaled to
    /// the loudest) is over 0.6, or over 0.2 after a rise above 0.1 (0x10117c1e0–0x10117c720).
    fn low_start(&self, adjust: &UnitBeatAdjust, pos: &[i32], count: i32) -> i32 {
        if count <= 1 {
            return 0;
        }
        let m = (count - 1) as usize;
        let mut v = vec![0.0f64; count as usize];
        for i in 0..m {
            v[i] = adjust.low_integ(self.bpm, self.inp.offset + pos[i]);
            if i != 0 && v[i] < 0.0 {
                v[i] = v[i - 1];
            }
        }
        let peak = v[..m].iter().fold(0.0f64, |a, &x| a.max(x));
        if peak.abs() > f64::EPSILON {
            let k = 1.0 / peak;
            v[..m].iter_mut().for_each(|x| *x *= k);
        }
        let mut d = vec![0.0f64; count as usize];
        for i in 1..m {
            d[i] = (v[i] - v[i - 1]).abs();
        }
        (0..m)
            .find(|&i| v[i] > 0.6 || (i != 0 && v[i] > 0.2 && d[i] > 0.1))
            .unwrap_or(0) as i32
    }

    /// The half-beat flip (0x10117ca00–0x10117d0e0): beats outside the anchor region are compared
    /// with the grid through the anchor (each the closest of its ±5 neighbours when that is a
    /// quarter better); when their mean distance exceeds 0.3 beat over more than half the beats,
    /// the grid moves half a beat toward the mean deviation.
    fn half_flip(&self, anchor: i32, lo: i32, hi: i32, beat: f64, count: i32) -> f64 {
        let pos = |i: i32| f64::from(self.at(i).pos);
        let size = self.b.len() as i32;
        let pa = pos(anchor);
        let (mut sum, mut dev) = (0.0f64, 0.0f64);
        let mut one = |k: i32, from: i32, to: i32, bounded: bool| {
            let g = beat * f64::from(anchor - k);
            let base = g - pa;
            let mut best_r = base + pos(from);
            let mut best = best_r.abs();
            let mut j = from;
            let limit = if bounded {
                if from <= size { size - from } else { 0 }
            } else {
                i32::MAX
            };
            let mut used = 0;
            while j <= to {
                if bounded && used == limit {
                    break;
                }
                let r = base + pos(j);
                let a = r.abs();
                if !(best <= a) {
                    best_r = r;
                }
                best = best.min(a);
                j += 1;
                used += 1;
            }
            let mut own = (g + pos(k)) - pa;
            let mut own_a = own.abs();
            if best * 1.25 <= own_a {
                own = best_r;
                own_a = best;
            }
            sum += own;
            dev += own_a;
        };
        for k in 0..lo.max(0) {
            let from = if k < 6 { 0 } else { k - 5 };
            let to = if lo <= k + 5 { lo - 1 } else { k + 5 };
            one(k, from, to, false);
        }
        let mut k = hi + 1;
        while k < count && k < size {
            let from = if k - 5 <= hi { hi + 1 } else { k - 5 };
            let to = if count <= k + 5 { count - 1 } else { k + 5 };
            one(k, from, to, true);
            k += 1;
        }
        let outside = lo + count - hi - 1;
        let avg = if outside != 0 {
            dev / f64::from(outside)
        } else {
            0.0
        };
        if beat * 0.3 < avg && 0.5 < f64::from(outside) / f64::from(count) {
            if 0.0 < sum { beat * 0.5 } else { beat * -0.5 }
        } else {
            0.0
        }
    }
}

/// The reference interval (ms) with the first and last index of its stretch: the first stretch
/// of at least min(n, 37) intervals with neighbour differences up to 11, then 20, then 30 ms that
/// ends at a larger jump; else the mean of the first min(n, 37) intervals.
fn reference(iv: &[f64], n: i32) -> (f64, i32, i32) {
    let min_run = n.min(37);
    let m = if n < 22 {
        if n < 2 { n - 1 } else { 1 }
    } else {
        n - 21
    };
    let dd: Vec<f64> = (0..m.max(0) as usize)
        .map(|i| (iv[i + 1] - iv[i]).abs())
        .collect();
    let (mut sum, mut cnt, mut start, mut end) = (0.0f64, 0i32, -1i32, -1i32);
    let mut found = false;
    if m >= 1 {
        'scan: for thr in [11.0, 20.0, 30.0] {
            sum = 0.0;
            cnt = 0;
            start = -1;
            for i in 0..m as usize {
                if thr < dd[i] {
                    if min_run <= cnt {
                        end = i as i32 - 1;
                        if start < 0 {
                            end = -1;
                        }
                        found = true;
                        break 'scan;
                    }
                    sum = 0.0;
                    cnt = 0;
                    start = -1;
                } else {
                    sum += iv[i];
                    cnt += 1;
                    if start < 0 {
                        start = i as i32;
                    }
                }
            }
        }
    }
    if !found {
        if n < 1 {
            sum = 0.0;
        } else {
            let c = min_run.max(1) as usize;
            let pairs = if c < 2 { 0 } else { c & 0x7fff_fffe };
            let mut s = 0.0;
            if pairs > 0 {
                let (mut ev, mut od) = (0.0, 0.0);
                for k in (0..pairs).step_by(2) {
                    ev += iv[k];
                    od += iv[k + 1];
                }
                s = ev + od;
            }
            for &x in &iv[pairs..c] {
                s += x;
            }
            sum = s;
        }
        start = 0;
        end = min_run - 1;
        cnt = min_run;
    }
    let start = start.max(0);
    let end = end.max(0);
    let r = if cnt == 0 {
        // the median interval (degenerate input only)
        let k = (n - 1).max(1) as usize;
        let mut c: Vec<f64> = iv[..k.min(iv.len())].to_vec();
        if n > 2 {
            for i in 0..(n - 2) as usize {
                for j in i + 1..(n - 1) as usize {
                    if c[i] < c[j] {
                        c.swap(i, j);
                    }
                }
            }
        }
        let nd = n - 1;
        if nd & (0x8000_0001u32 as i32) == 1 {
            c[(nd >> 1) as usize]
        } else {
            let h = (if nd < 0 { n } else { nd } >> 1) as usize;
            (c[h] + c[h.saturating_sub(1)]) * 0.5
        }
    } else {
        sum / f64::from(cnt)
    };
    (r, start, end)
}

/// The anchor region around the low-band start `idx` (0x10117c720–0x10117c7f8): the stable gap
/// between stretches that holds it (10 ms stretches first, then 20 ms), up to the start or the
/// end of the track; 16 beats from it when none applies.
#[allow(clippy::too_many_arguments)]
fn region(
    idx: i32,
    count: i32,
    st10: &[i32; 128],
    en10: &[i32; 128],
    n10: usize,
    st20: &[i32; 128],
    en20: &[i32; 128],
    n20: usize,
) -> (i32, i32) {
    let (mut g_lo, mut g_hi) = (0i32, 0i32);
    for s in 0..n20.saturating_sub(1) {
        if en20[s] <= idx && idx <= st20[s + 1] {
            g_lo = en20[s];
            g_hi = st20[s + 1];
            break;
        }
    }
    let (mut h_lo, mut h_hi) = (g_lo, g_hi);
    for s in 0..n10.saturating_sub(1) {
        if en10[s] <= idx && idx <= st10[s + 1] {
            h_lo = en10[s];
            h_hi = st10[s + 1];
            break;
        }
    }
    let mut hi20 = st20[0];
    let mut lo20 = 0;
    if st20[0] < idx {
        hi20 = h_hi;
        lo20 = h_lo;
        if n20 != 0 && en20[n20 - 1] <= idx {
            hi20 = count - 1;
            lo20 = en20[n20 - 1];
        }
    }
    let mut hi = st10[0];
    let mut lo = 0;
    if st10[0] < idx {
        hi = hi20;
        lo = lo20;
        if n10 != 0 && en10[n10 - 1] <= idx {
            hi = count - 1;
            lo = en10[n10 - 1];
        }
    }
    if lo == 0 && hi == 0 {
        lo = idx;
        hi = if count <= idx + 16 {
            count - 1
        } else {
            idx + 16
        };
    }
    (lo, hi)
}

/// The index of the beat nearest to `p`, walking from the start while the distance shrinks.
fn nearest(v: &[BeatInfo], p: i32, count: i32) -> i32 {
    if count < 2 {
        return 0;
    }
    let d = |i: usize| (v[i].pos - p).unsigned_abs();
    let mut prev = d(1);
    if d(0) <= prev {
        return 0;
    }
    let mut k = 0i32;
    loop {
        if k == count - 2 {
            return count - 1;
        }
        let x = d((k + 2) as usize);
        k += 1;
        if x >= prev {
            return k;
        }
        prev = x;
    }
}
