//! BALib v1's `UnitBeatAdjust` (create @0x1011330c0, putSample @0x101133300, analyze
//! @0x101133528): a quarter-rate envelope (2 kHz high-pass, rectified, 1 kHz low-pass) against
//! which `bpmAdjust` @0x1011335f0 tries tempos in 0.002 BPM steps around the estimate
//! (`createBeatData` @0x101134c14, `manageWave` @0x101135098, `reflectBeatData` @0x101134948),
//! and `beatPositionAdjust` @0x1011344b8 / `offsetBeatPosition` @0x1011353dc move the grid onto
//! the envelope's beat-synchronous peak. Spec: `analysis-beats.md` § 6.

use super::BeatInfo;
use super::filters::Filter2D;

pub struct UnitBeatAdjust {
    total: i32,
    /// `+0xc`: rate / 4.
    rate4: i32,
    /// `+0x10`: envelope samples taken.
    count: i32,
    /// `+0x18`: candidate counter (index into `templates`).
    counter: i32,
    /// `+0x1c`: envelope capacity (`min(total / 4, rate4 · 900)`).
    cap: i32,
    /// `+0x20`: beat count.
    beats: i32,
    hp: Filter2D,
    lp: Filter2D,
    env: Vec<i16>,
    buf: Vec<f64>,
    templates: Vec<Vec<i32>>,
}

/// Best candidate of `bpmAdjust` so far.
#[derive(Debug, Clone, Copy, Default)]
struct Best {
    score: f64,
    bpm: f64,
    idx: i32,
}

/// `bpmAdjust`'s fixed offsets below (−0.05 … −0.036) and above (+0.036 … +0.05) the estimate.
const LOW: [f64; 8] = [
    f64::from_bits(0xbfa9_9999_9999_999a),
    f64::from_bits(0xbfa8_9374_bc6a_7efa),
    f64::from_bits(0xbfa7_8d4f_df3b_645a),
    f64::from_bits(0xbfa6_872b_020c_49bb),
    f64::from_bits(0xbfa5_8106_24dd_2f1b),
    f64::from_bits(0xbfa4_7ae1_47ae_147b),
    f64::from_bits(0xbfa3_74bc_6a7e_f9dc),
    f64::from_bits(0xbfa2_6e97_8d4f_df3c),
];
const HIGH: [f64; 8] = [
    f64::from_bits(0x3fa2_6e97_8d4f_df3c),
    f64::from_bits(0x3fa3_74bc_6a7e_f9dc),
    f64::from_bits(0x3fa4_7ae1_47ae_147c),
    f64::from_bits(0x3fa5_8106_24dd_2f1b),
    f64::from_bits(0x3fa6_872b_020c_49bb),
    f64::from_bits(0x3fa7_8d4f_df3b_645b),
    f64::from_bits(0x3fa8_9374_bc6a_7efa),
    f64::from_bits(0x3fa9_9999_9999_999a),
];

/// `(x + 3 if negative) >> 2`: C's `x / 4`.
fn div4(x: i32) -> i32 {
    x / 4
}

impl UnitBeatAdjust {
    pub fn new(rate: u32, total: i32) -> Self {
        let rate4 = div4(rate as i32);
        let cap = div4(total).min(rate4 * 900);
        let w = (f64::from(rate4) * 0.05) as i32;
        Self {
            total,
            rate4,
            count: 0,
            counter: 0,
            cap,
            beats: 0,
            hp: Filter2D::high(2000.0, f64::from(rate4)),
            lp: Filter2D::low(1000.0, f64::from(rate4)),
            env: Vec::with_capacity(cap.max(0) as usize),
            buf: vec![0.0; (2 * w).max(1) as usize],
            templates: Vec::with_capacity(36),
        }
    }

    /// `putSample(shorts, n)` for one block: every fourth frame while there is room.
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        let n = pcm.len() as i32;
        if n < 1 {
            return;
        }
        let used = self.count * 4;
        let mut take = if used + n > self.total {
            self.total - used
        } else {
            n
        };
        let len = self.env.len() as i32;
        let cap = self.rate4 * 900;
        if len + take / 4 > cap {
            take = (cap - len) * 4;
        }
        let mut i = 0;
        while i < take {
            let [l, r] = pcm[i as usize];
            let x = (f64::from(r) + f64::from(l)) * 0.5;
            let y = self.lp.step(self.hp.step(x).abs());
            let v = if y > 32767.0 {
                32767
            } else if y < 0.0 {
                0
            } else {
                y as i32 as i16
            };
            self.env.push(v);
            self.count += 1;
            i += 4;
        }
    }

    /// Half the scoring window (`(int)(rate4 · 0.05)`).
    fn half(&self) -> i32 {
        (f64::from(self.rate4) * 0.05) as i32
    }

    /// Adds the envelope around each of `centers[1 .. n − 1]` (± `w`) into `buf`.
    fn accumulate(&mut self, centers: impl Iterator<Item = i32>) {
        let w = self.half();
        let w2 = 2 * w;
        if w >= 1 {
            for v in &mut self.buf {
                *v = 0.0;
            }
        }
        for b in centers {
            let s = b - w;
            if s < 0 {
                continue;
            }
            let e = b + w;
            let len = e - s;
            if len <= 0 || self.cap < e || len > w2 {
                continue;
            }
            if len < 1 || (self.env.len() as u64) < len as u64 {
                continue;
            }
            for j in 0..w2.max(0) as usize {
                self.buf[j] += f64::from(self.env[s as usize + j]);
            }
        }
    }

    /// `createBeatData(bpm, start)`: the grid at `bpm` from `start` (quarter-rate samples).
    fn create_beat_data(&mut self, bpm: f64, start: i32) {
        if !(bpm > 0.0) || start < 0 || self.cap <= start || self.beats < 1 {
            return;
        }
        let period = (f64::from(self.rate4) * 60.0) / bpm;
        let s = f64::from(start);
        let end = f64::from(div4(self.total));
        let mut t = Vec::with_capacity(self.beats as usize);
        let mut x = 0.0;
        for i in 0..self.beats {
            x = period.mul_add(f64::from(i), s);
            if x < end {
                t.push((x + 0.5) as i32);
            }
        }
        x += period;
        while x < end {
            t.push((x + 0.5) as i32);
            x += period;
        }
        self.templates.push(t);
    }

    /// `manageWave(n, idx, &score)`: the peak of the beat-synchronous envelope sum.
    fn manage_wave(&mut self, n: i32, idx: i32, score: &mut f64) {
        if n < 1 || idx < 0 || self.templates.len() as u64 <= idx as u64 {
            return;
        }
        let tpl = std::mem::take(&mut self.templates[idx as usize]);
        let m = (tpl.len() as i32).min(n);
        let w = self.half();
        if m >= 3 {
            self.accumulate(tpl[1..(m - 1) as usize].iter().copied());
        } else if w >= 1 {
            for v in &mut self.buf {
                *v = 0.0;
            }
        }
        self.templates[idx as usize] = tpl;
        if w < 1 {
            return;
        }
        let mut best = 0.0f64;
        for &v in &self.buf[..(2 * w) as usize] {
            best = v.max(best);
        }
        *score = best;
    }

    /// Builds and scores the grid of one (clamped) candidate tempo.
    fn eval(&mut self, c: f64, start: i32, n: i32) -> f64 {
        self.create_beat_data(c, start);
        let mut score = 0.0;
        let idx = self.counter;
        self.manage_wave(n, idx, &mut score);
        score
    }

    /// `analyze(bpm, beats)`.
    pub fn analyze(&mut self, bpm: &mut f32, list: &mut Vec<BeatInfo>) {
        if !(*bpm > 0.0) || list.is_empty() {
            return;
        }
        let n = list.len() as i32;
        self.beats = n;
        let mut nb = n;
        let cap = self.rate4 * 900;
        if cap <= self.env.len() as i32 {
            let span = f64::from(list[0].pos).mul_add(-0.25, f64::from(cap));
            nb = (((f64::from(*bpm) * f64::from_bits(0x3f91_1111_1111_1111)) * span)
                / f64::from(self.rate4)) as i32;
        }
        self.bpm_adjust(bpm, list, nb);
        self.beat_position_adjust(list);
    }

    /// One candidate of `bpmAdjust`: clamp to 70–180, build and score its grid, keep it when it
    /// beats the best so far.
    fn try_bpm(&mut self, c: f64, start: i32, n: i32, best: &mut Best) {
        // fmaxnm/fminnm, not `clamp` (NaN goes to 70 here).
        #[allow(clippy::manual_clamp)]
        let c = c.max(70.0).min(180.0);
        let score = self.eval(c, start, n);
        if best.score < score {
            best.idx = self.counter;
            best.bpm = c;
        }
        best.score = best.score.max(score);
        self.counter += 1;
    }

    fn sweep(&mut self, from: f64, k: i32, start: i32, n: i32, best: &mut Best) {
        let step = f64::from_bits(0x3f60_624d_d2f1_a9fc);
        for j in 0..k {
            self.try_bpm(f64::from(j).mul_add(step, from), start, n, best);
        }
    }

    /// `bpmAdjust(bpm, beats, n)`: 0.002 BPM steps from −0.016 to +0.016, widened to ±0.05 when
    /// the best lies at an edge, then ±0.001 around the winner.
    fn bpm_adjust(&mut self, bpm: &mut f32, list: &mut Vec<BeatInfo>, n: i32) {
        if n < 1 || !(*bpm > 0.05f32) || list.is_empty() {
            return;
        }
        let pos0 = list[0].pos;
        let start = div4(pos0);
        let ok_pos = pos0 >= -3;
        let b = f64::from(*bpm);
        let mut best = Best::default();
        let a_from = b + f64::from_bits(0xbf90_624d_d2f1_a9fc);
        let mut go_low = false;
        let mut go_high = false;
        if a_from > 0.0 && ok_pos && self.cap > start {
            self.sweep(a_from, 17, start, n, &mut best);
            if (best.idx as u32) <= 2 {
                go_low = true;
            } else {
                go_high = true;
            }
        } else {
            go_low = ok_pos;
        }
        if go_low {
            let from = b + f64::from_bits(0xbfa1_6872_b020_c49c);
            if from > 0.0 && self.cap > start {
                self.sweep(from, 9, start, n, &mut best);
            }
        }
        if !go_high {
            if (best.idx.wrapping_sub(17) as u32) <= 2
                && ok_pos
                && b + LOW[0] > 0.0
                && self.cap > start
            {
                for &o in &LOW {
                    self.try_bpm(b + o, start, n, &mut best);
                }
            }
        } else if (best.idx.wrapping_sub(15) as u32) <= 2 {
            let from = b + f64::from_bits(0x3f92_6e97_8d4f_df3c);
            if from > 0.0 && self.cap > start {
                self.sweep(from, 9, start, n, &mut best);
            }
            if (best.idx.wrapping_sub(24) as u32) <= 2 && b + HIGH[0] > 0.0 && self.cap > start {
                for &o in &HIGH {
                    self.try_bpm(b + o, start, n, &mut best);
                }
            }
        }
        // ±0.001 BPM around the winner.
        let w26 = (best.bpm * 1000.0) as i32;
        let w27 = (b * 1000.0) as i32;
        let centre = best.bpm;
        let mut fin = centre;
        if w27 - 50 < w26 && ok_pos {
            let c = centre + f64::from_bits(0xbf50_624d_d2f1_a9fc);
            if c > 0.0 && self.cap > start {
                let mut probe = Best {
                    bpm: centre,
                    ..best
                };
                self.try_bpm(c, start, n, &mut probe);
                fin = probe.bpm;
                best = probe;
            }
        }
        if w27 + 50 > w26 && ok_pos {
            let c = centre + f64::from_bits(0x3f50_624d_d2f1_a9fc);
            if c > 0.0 && self.cap > start {
                let mut probe = Best { bpm: fin, ..best };
                self.try_bpm(c, start, n, &mut probe);
                fin = probe.bpm;
                best = probe;
            }
        }
        self.reflect(fin, bpm, list, best.idx);
    }

    /// `reflectBeatData(best, &bpm, beats, idx)`: the winning grid replaces the beats.
    fn reflect(&mut self, best: f64, bpm: &mut f32, list: &mut Vec<BeatInfo>, idx: i32) {
        if !(best > 0.0) || idx < 0 || self.templates.len() as u64 <= idx as u64 {
            return;
        }
        if !(*bpm > 0.0) || list.is_empty() {
            return;
        }
        if (f64::from(*bpm) * 1000.0) as i32 == (best * 1000.0) as i32 {
            return;
        }
        let m = self.templates[idx as usize].len() as i32;
        let mut c = self.beats;
        while c != m {
            if c > m {
                list.pop();
                c -= 1;
            } else {
                let mut b = list[(c - 1) as usize];
                b.stat = if b.stat == 4 { 1 } else { b.stat + 1 };
                b.flag = 0;
                list.push(b);
                c += 1;
            }
        }
        self.beats = c;
        let t = (60000.0 / best) as f32;
        for (b, &p) in list.iter_mut().zip(&self.templates[idx as usize]) {
            b.pos = p << 2;
            b.tempo = t;
        }
        *bpm = best as f32;
    }

    /// `beatPositionAdjust`: moves the grid onto the peak of the beat-synchronous envelope.
    fn beat_position_adjust(&mut self, list: &mut Vec<BeatInfo>) {
        if list.is_empty() {
            return;
        }
        let n = list.len();
        let w = self.half();
        if n >= 3 {
            let centers: Vec<i32> = list[1..n - 1].iter().map(|b| div4(b.pos)).collect();
            self.accumulate(centers.into_iter());
        } else if w >= 1 {
            for v in &mut self.buf {
                *v = 0.0;
            }
        }
        let mut peak = 0i32;
        if w >= 1 {
            let mut best = 0.0f64;
            for j in 0..(2 * w) as usize {
                let v = self.buf[j];
                if v > best {
                    peak = j as i32;
                }
                best = v.max(best);
            }
        }
        self.offset_beat_position(w, peak, list);
    }

    /// `offsetBeatPosition(w, peak, beats)`.
    fn offset_beat_position(&mut self, w: i32, peak: i32, list: &mut Vec<BeatInfo>) {
        if w < 0 {
            return;
        }
        let w2 = 2 * (f64::from(self.rate4) * 0.05) as i32;
        if w2 < peak || peak < 0 || w2 < w || list.is_empty() {
            return;
        }
        let mut count = list.len() as i32;
        let bias = (f64::from(self.rate4) * f64::from_bits(0xbf50_624d_d2f1_a9fc)) as i32;
        let beat = (self.rate4 as f32 * (0.004f32 * list[0].tempo)) as i32;
        let mut d = (peak - w) * 4;
        if !(list[0].pos + d >= beat && bias * 20 + d < 1) {
            d = ((peak - w) + bias) * 4;
        }
        let mut neg = 0;
        let mut past = 0;
        for b in list.iter_mut() {
            b.pos += d;
            if b.pos < 0 {
                neg += 1;
            }
            if b.pos + d > self.total {
                past += 1;
            }
        }
        if neg > 0 {
            list.drain(..(neg as usize).min(list.len()));
            count -= neg;
        }
        if past > 0 {
            let keep = list.len().saturating_sub(past as usize);
            list.truncate(keep);
            count -= past;
        }
        if list.is_empty() || beat <= 0 {
            self.beats = count;
            return;
        }
        while list[0].pos >= beat {
            let mut b = list[0];
            b.pos -= beat;
            b.stat = if b.stat - 1 == 0 { 4 } else { b.stat - 1 };
            b.flag = 0;
            list.insert(0, b);
            count += 1;
        }
        let last = list[list.len() - 1];
        if last.pos + beat <= self.total {
            loop {
                let mut b = list[list.len() - 1];
                b.pos += beat;
                b.stat = if b.stat == 4 { 1 } else { b.stat + 1 };
                b.flag = 0;
                let next = b.pos + beat;
                list.push(b);
                count += 1;
                if next > self.total {
                    break;
                }
            }
        }
        self.beats = count;
    }
}
