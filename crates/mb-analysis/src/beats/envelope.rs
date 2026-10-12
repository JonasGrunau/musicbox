//! BALib v1's `UnitEnvelopeMaker` (create @0x101137f5c, putSample @0x101138168, analyze
//! @0x101138480): a 1/128-rate onset envelope (150 Hz low-pass twice, rectified, 5 Hz low-pass,
//! 0.5 Hz high-pass) whose peaks decide whether the grid sits half a beat off and which beat of
//! the bar is the downbeat. Spec: `analysis-beats.md` § 5.

use super::BeatInfo;
use super::filters::{Filter1D, Filter2D};

/// Frames per envelope sample (`+0x18`).
const BLOCK: i32 = 128;

pub struct UnitEnvelope {
    rate: i32,
    total: i32,
    lp1: Filter2D,
    lp2: Filter2D,
    lp3: Filter2D,
    hp: Filter1D,
    buf: Vec<f64>,
    count: i32,
    /// The envelope (`+0x58`) and its picked peaks (`+0x60`).
    env: Vec<i16>,
    peaks: Vec<i16>,
}

impl UnitEnvelope {
    pub fn new(rate: u32, total: i32) -> Self {
        let fs = f64::from(rate);
        let fe = fs / f64::from(BLOCK);
        let n = ((total + BLOCK - 1) / BLOCK).max(0) as usize;
        Self {
            rate: rate as i32,
            total,
            lp1: Filter2D::low(150.0, fs),
            lp2: Filter2D::low(150.0, fs),
            lp3: Filter2D::low(5.0, fe),
            hp: Filter1D::high(0.5, fe),
            buf: Vec::with_capacity(BLOCK as usize),
            count: 0,
            env: vec![0; n],
            peaks: vec![0; n],
        }
    }

    /// `putSample(shorts, n)`.
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        for &[l, r] in pcm {
            if self.count >= self.total {
                return;
            }
            self.buf.push((f64::from(r) + f64::from(l)) * 0.5);
            self.count += 1;
            if self.buf.len() as i32 >= BLOCK || self.count >= self.total {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        let fill = self.buf.len() as i32;
        self.lp1.run(&mut self.buf);
        self.lp2.run(&mut self.buf);
        // Only the first sample of each block goes on.
        let x = self.buf[0].abs();
        let y = self.hp.step(self.lp3.step(x));
        let idx = ((self.count - fill) / BLOCK) as usize;
        if let Some(e) = self.env.get_mut(idx) {
            *e = (y as i32) as i16;
        }
        self.buf.clear();
    }
}

/// Last index with `v[i] <= key`, or −1.
fn last_le(v: &[i32], key: i32) -> i32 {
    v.partition_point(|&p| p <= key) as i32 - 1
}

/// The grid beat nearest to `key`: the last index whose midpoint with its predecessor is at or
/// before `key` (−1 when even the first beat is after it).
fn near_mid(pos: &[i32], key: i32) -> i32 {
    let mut lo = 0i32;
    let mut hi = pos.len() as i32 - 1;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let after = if mid == 0 {
            pos[0] > key
        } else {
            let s = pos[(mid - 1) as usize].wrapping_add(pos[mid as usize]);
            key < s / 2
        };
        if mid == 0 && after {
            hi = -1;
            break;
        }
        if after {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    hi
}

impl UnitEnvelope {
    /// The beat length (in envelope samples) at envelope index `at`.
    fn per_at(pos: &[i32], per: &[f64], at: i32) -> f64 {
        if per.is_empty() || pos.is_empty() {
            return 0.0;
        }
        per[last_le(pos, at).max(0) as usize]
    }

    /// `analyze(bpm, beats, varied)`: shifts the grid by half a beat when most onsets fall between
    /// the beats, and numbers the beats so the strongest onsets land on beat 1.
    pub fn analyze(&mut self, bpm: f32, list: &mut Vec<BeatInfo>, varied: bool) {
        if !(bpm > 0.0) || list.len() < 2 {
            return;
        }
        let n = list.len();
        let block = f64::from(BLOCK);
        let inv_b = 1.0 / block;
        let rate = f64::from(self.rate);
        let p0 = f64::from(list[0].pos) * inv_b;
        let thr0 =
            ((rate * f64::from_bits(0x3f4d_7dbf_487f_cb93)) * f64::from(list[0].tempo)) * inv_b;
        let late = !(p0 <= thr0);
        let to_env = |b: &BeatInfo| (f64::from(b.pos) / block + 0.5) as i32;
        let to_per = |b: &BeatInfo| ((f64::from(b.tempo) * 0.001) * rate) / block;
        let (mut pos, per): (Vec<i32>, Vec<f64>) = if late {
            let mut pos = vec![1];
            let mut per = vec![to_per(&list[0])];
            for b in &list[..n - 1] {
                pos.push(to_env(b));
                per.push(to_per(b));
            }
            (pos, per)
        } else {
            (
                list.iter().map(to_env).collect(),
                list.iter().map(to_per).collect(),
            )
        };
        let nenv = (self.total + BLOCK - 1) / BLOCK;
        let mut first = pos[0];
        if first < 1 {
            pos[0] = 1;
            first = 1;
        }

        // Peaks of the envelope, kept when at least as high as the mean peak.
        let mut onsets: Vec<i32> = Vec::new();
        if nenv > 1 {
            let env = &self.env;
            let peaks = &mut self.peaks;
            let (mut sum, mut cnt) = (0u64, 0u64);
            let (mut w14, mut w12, mut inside) = (0i32, 0i32, false);
            for i in 0..(nenv - 1) {
                let w17 = i32::from(env[(i + 1) as usize]);
                let w0;
                let end;
                if inside {
                    let cur = i32::from(w14 as i16);
                    if cur < w17 {
                        w12 = i + 1;
                    }
                    w14 = if cur > w17 { cur } else { w17 };
                    w0 = i32::from(w14 as i16);
                    end = w17 == 0 || (f64::from(w0) / f64::from(w17)) > 1.5;
                } else {
                    if i32::from(env[i as usize]) > w17 {
                        w14 = w17;
                    }
                    let cur = i32::from(w14 as i16);
                    w14 = if cur > w17 { cur } else { w17 };
                    if cur >= w17 {
                        inside = false;
                        continue;
                    }
                    w12 = i + 1;
                    w0 = i32::from(w14 as i16);
                    end = w17 == 0 || (f64::from(w0) / f64::from(w17)) > 1.5;
                }
                if end {
                    inside = false;
                    if let Some(p) = peaks.get_mut(w12 as usize) {
                        *p = w14 as i16;
                    }
                    let v = if w0 > 0 { w0 } else { 0 };
                    sum += u64::from(v as u16);
                    if w0 > 0 {
                        cnt += 1;
                    }
                    w14 = i32::from(env[(i + 1) as usize] as u16);
                } else {
                    inside = true;
                }
            }
            let mean = sum.checked_div(cnt).map_or(0, |m| m as i32);
            let mean = i32::from(mean as i16);
            for i in 1..nenv {
                if mean > i32::from(peaks[i as usize]) {
                    peaks[i as usize] = 0;
                } else {
                    onsets.push(i);
                }
            }
        }
        let n_on = onsets.len();

        // Swing (2.5 + 1.5/2 beats) and dotted (1.5 beats) onset patterns.
        let mut list_a: Vec<i32> = Vec::new();
        let mut list_b: Vec<i32> = Vec::new();
        let (mut count_a, mut count_b) = (0i32, BLOCK);
        if n_on >= 3 {
            count_b = 0;
            let k135 = f64::from_bits(0x3ff5_9999_9999_999a);
            let k165 = f64::from_bits(0x3ffa_6666_6666_6667);
            for k in 0..n_on - 2 {
                let o0 = onsets[k];
                let d1 = onsets[k + 1] - o0;
                let d2 = onsets[k + 2] - onsets[k + 1];
                let p = Self::per_at(&pos, &per, o0);
                if ((p * 2.25) as i32) < d1 && d1 <= (p * 2.75) as i32 {
                    let a = ((p * k135) as i32) < d2 && d2 <= (p * k165) as i32;
                    let b = ((p * 1.8) as i32) < d2 && d2 <= (p * 2.2) as i32;
                    if a || b {
                        count_a += 1;
                        list_a.push(o0);
                    }
                }
                if ((p * k135) as i32) < d1 && d1 <= (p * k165) as i32 {
                    count_b += 1;
                    list_b.push(o0);
                } else {
                    let g = (p * 0.9) as i32;
                    if d1 > g {
                        let h = (p * 1.1) as i32;
                        if d1 <= h && d2 > g && d2 <= h {
                            count_b -= 1;
                        }
                    }
                }
            }
        }
        let w8 = (rate * 0.11).mul_add(inv_b, 0.5) as i32;

        // Half a beat off?
        let mut shifted = false;
        if !varied {
            let pb = (rate * 60.0) / (block * f64::from(bpm));
            let chosen: &[i32] = if count_a >= 11 {
                &list_a
            } else if count_b >= 81 {
                &list_b
            } else {
                &onsets
            };
            let q = (pb * 0.25) as i32;
            let mut off = 0i32;
            for &o in chosen {
                if pos.is_empty() {
                    continue;
                }
                let key = o - w8;
                let i = near_mid(&pos, key).max(0) as usize;
                if pos[i].wrapping_sub(key).wrapping_abs() > q {
                    off += 1;
                }
            }
            let nl = chosen.len() as i32;
            if off > nl / 2 && nl >= 1 {
                let half = (pb * 0.5 + 0.5) as i32;
                if pb * f64::from_bits(0x3fdc_cccc_cccc_cccd) + 0.5 >= f64::from(first) {
                    for p in pos.iter_mut().take(n) {
                        *p += half;
                    }
                } else {
                    for p in pos.iter_mut().take(n) {
                        *p -= half;
                    }
                }
                if pos[0] <= 0 {
                    pos[0] = 1;
                }
                shifted = true;
            }
        }

        // Downbeat: onset strength per beat-of-bar.
        let mut c = [0f64; 4];
        let env2 = &self.peaks;
        let pk = |i: i32| f64::from(env2[i as usize]);
        if n_on == 0 {
            return;
        }
        let o0 = onsets[0];
        let w12 = if o0 < w8 { o0 } else { w8 };
        let p = Self::per_at(&pos, &per, o0);
        let w13 = p.mul_add(0.9, 0.5) as i32;
        let w14 = p.mul_add(1.1, 0.5) as i32;
        let w17 = p.mul_add(1.8, 0.5) as i32;
        let slot = |key: i32| (near_mid(&pos, key).max(0) & 3) as usize;
        let w16 = if n_on >= 2 { onsets[1] - o0 } else { 0 };
        if w13 <= w16 && w16 <= w14 && !pos.is_empty() {
            c[slot(o0 - w12)] = pk(o0) + pk(o0);
        }
        if w16 >= w17 {
            let w17b = p.mul_add(2.2, 0.5) as i32;
            if w16 <= w17b && !pos.is_empty() {
                c[slot(o0 - w12)] = pk(o0) + pk(o0);
            }
        }
        let w15 = if n_on >= 3 { onsets[2] - o0 } else { 0 };
        if w15 >= w13 && w15 <= w14 && !pos.is_empty() {
            c[slot(o0 - w12)] = pk(o0) + pk(o0);
        }
        let mut maxv = pk(o0);
        for &o in &onsets[1..] {
            maxv = maxv.max(pk(o));
        }
        if n_on > 2 {
            let thr = maxv * f64::from_bits(0x3fe3_3333_3333_3333);
            for k in 1..n_on - 1 {
                let x4 = onsets[k];
                let pp = Self::per_at(&pos, &per, x4);
                let gp = f64::from(x4) - f64::from(onsets[k - 1]);
                if !(gp > pp * 7.0) {
                    continue;
                }
                let hi = pp.mul_add(2.2, 0.5) as i32;
                let lo = pp.mul_add(1.8, 0.5) as i32;
                let gn = onsets[k + 1] - x4;
                if gn >= lo && gn <= hi {
                    if !pos.is_empty() {
                        let s = slot(x4 - w8);
                        c[s] += pk(x4);
                    }
                } else {
                    let v = pk(x4);
                    if thr >= v || pos.is_empty() {
                        continue;
                    }
                    let s = slot(x4 - w8);
                    c[s] += v;
                }
            }
        }
        let bonus = f64::from_bits(0x408e_b851_eb85_1eb8);
        let pattern: &[i32] = if count_a >= 11 {
            &list_a
        } else if count_b >= 81 {
            &list_b
        } else {
            &[]
        };
        for &o in pattern {
            if !pos.is_empty() {
                let s = slot(o - w8);
                c[s] += bonus;
            }
        }
        let mut d0 = c[0].max(0.0);
        let mut best = u32::from(d0 < c[1]);
        if d0 < c[1] {
            d0 = c[1];
        }
        if d0 < c[2] {
            best = 2;
        }
        d0 = d0.max(c[2]);
        let top = d0.max(c[3]);
        if d0 < c[3] {
            best = 3;
        }
        if top == 0.0 && !pos.is_empty() {
            best = (near_mid(&pos, o0).max(0) & 3) as u32;
        }
        let best = best as i32;
        let mut w10 = 4 - best;
        let at = |v: &[i32], i: usize| v.get(i).copied().unwrap_or(0);
        let shift;
        if shifted {
            let p1 = list.get(1).map_or(0, |b| b.pos);
            if late {
                shift = BLOCK * at(&pos, 2) - p1;
                w10 = (1 - best) & 3;
            } else {
                shift = BLOCK * at(&pos, 1) - p1;
            }
        } else {
            shift = 0;
            if late {
                w10 = (1 - best) & 3;
            }
        }
        for (i, b) in list.iter_mut().enumerate().take(n) {
            b.pos = b.pos.wrapping_add(shift);
            b.stat = ((w10 + i as i32) & 3) + 1;
        }

        // Ends: one more beat when it still fits, none past the end or before the start.
        let ms = rate * 0.001;
        let last = list[n - 1];
        let mut check_front = true;
        if last.pos < self.total {
            let next = last.pos + f64::from(last.tempo).mul_add(ms, 0.5) as i32;
            if next < self.total {
                let mut b = last;
                b.pos = next;
                b.stat = last.stat % 4 + 1;
                list.push(b);
            }
        } else {
            list.pop();
        }
        if list.is_empty() {
            return;
        }
        if list[0].pos < 0 {
            list.remove(0);
            check_front = false;
        }
        if check_front {
            let f = list[0];
            let step = ms.mul_add(f64::from(f.tempo), 0.5) as i32;
            if f.pos >= step {
                let mut b = f;
                b.pos = f.pos - step;
                b.stat = (f.stat + 2) % 4 + 1;
                list.insert(0, b);
            }
        }
    }
}
