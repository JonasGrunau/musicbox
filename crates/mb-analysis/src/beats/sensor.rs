//! BALib v1's onset side (`BeatAnalyzer_1_0::peakss`): `BeatSensor::put` @0x101137234 splits the
//! mono mix into six bands, tracks peaks on an 8-sample envelope, and turns the gaps between
//! peaks into tempo candidates (`PeakBuffer::calcBeat` @0x10118ef24, `calcBeat_s` @0x10118f010).
//! `selectBand` @0x1011378d8 keeps the bands that agree with the tempo histogram, and
//! `BeatListSignal::changeStat` @0x101136ba0 files their peaks into the raw beat list.
//! Spec: `analysis-beats.md` § 2.

use mb_dsp::IirPassFilter;

use super::BeatInfo;

/// Bands of the Analyzer's sensor (`BeatSensor::create(rate, 0, total, 8, 6)` @0x10118f508).
const BANDS: usize = 6;
/// Envelope decimation (samples per tick).
const STEP: i32 = 8;
/// Peak ring capacity per band.
const RING: usize = 32;
/// Interval patterns `(lo ms, hi ms, beats)` of `calcBeat_s` for the current gap (0x1037c86c0)
/// and for the earlier gaps (0x1037c872c).
const GAP_NOW: [(f32, f32, f32); 9] = [
    (500.0, 1276.0, 1.5),
    (468.0, 1071.0, 1.25),
    (923.0, 2142.0, 2.5),
    (290.0, 638.0, 0.75),
    (320.0, 860.0, 1.0),
    (290.0, 428.0, 0.5),
    (666.0, 1714.0, 2.0),
    (1500.0, 3333.0, 4.0),
    (1125.0, 2580.0, 3.0),
];
const GAP_BEFORE: [(f32, f32, f32); 9] = [
    (1500.0, 3333.0, 4.0),
    (923.0, 2142.0, 2.5),
    (666.0, 1714.0, 2.0),
    (500.0, 1276.0, 1.5),
    (468.0, 1071.0, 1.25),
    (320.0, 860.0, 1.0),
    (290.0, 638.0, 0.75),
    (290.0, 428.0, 0.5),
    (1125.0, 2580.0, 3.0),
];

/// The tempo tolerance BALib uses everywhere: `(float)fminnm((double)t · 0.04, 20)`.
pub(crate) fn tol(t: f32) -> f32 {
    (f64::from(t) * 0.04).min(20.0) as f32
}

/// `!(|a − t| > tol) || !(|2a − t| > tol) || !(|a/2 − t| > tol)`: `a` is `t`, double or half
/// `t` within `tol` (the `fcmp`/`fccmp … gt` chains).
pub(crate) fn related(a: f32, t: f32, tol: f32) -> bool {
    !((a - t).abs() > tol && ((a + a) - t).abs() > tol && ((a * 0.5) - t).abs() > tol)
}

/// One ring entry (`PeakInfo`, 12 bytes; the peak value at +4 is never read back).
#[derive(Debug, Clone, Copy, Default)]
struct Peak {
    pos: u32,
    gap: f32,
}

/// `peakss::PeakBuffer` (0x50 bytes): the peak ring of one band and its tempo estimate.
#[derive(Debug, Clone)]
struct PeakBuffer {
    band: i32,
    /// +0x04: gap of the last peak, replaced by the tempo `calcBeat` found (ms per beat).
    tempo: f32,
    ring: [Peak; RING],
    widx: i32,
    count: i32,
    /// +0x20: position of the previous peak.
    prev: u64,
    /// +0x39: this tick's peak fits a tempo.
    hit: bool,
    /// +0x3c: tempo error of the best pattern.
    err: f32,
    /// +0x40: earlier gaps that agree with the tempo.
    agree: i32,
    /// +0x44 / +0x48: value and position of the last fitting peak.
    value: f32,
    pos: u64,
}

impl PeakBuffer {
    fn new(band: i32) -> Self {
        Self {
            band,
            tempo: 0.0,
            ring: [Peak::default(); RING],
            widx: 0,
            count: 0,
            prev: 0,
            hit: false,
            err: f32::MAX,
            agree: 0,
            value: 0.0,
            pos: 0,
        }
    }

    fn gap(&self, i: i32) -> f32 {
        let cap = RING as i32;
        self.ring[(i % cap) as usize].gap
    }

    /// `calcBeat_s(gap, ring, base, start, &tempo, &err, &agree)`: reads the gap as one of the
    /// patterns `GAP_NOW[start..]` and walks back through the earlier gaps until their beats add
    /// up to `base` (or `2·base`, doubling up to `4·base`). Returns the next pattern index.
    fn calc_beat_s(&self, gap: f32, base: f32, start: i32) -> Option<(i32, f32, f32, i32)> {
        let cap = RING as i32;
        let find = |from: i32| {
            (from.max(0)..9).find(|&j| {
                let (lo, hi, _) = GAP_NOW[j as usize];
                lo < gap && hi > gap
            })
        };
        if start > 8 {
            return None;
        }
        let mut j = find(start)?;
        let base2 = base + base;
        let base4 = f64::from(base) * 4.0;
        'pattern: loop {
            let mut beats = GAP_NOW[j as usize].2;
            let unit = gap / beats;
            let tl = (f64::from(unit) * 0.04).min(20.0) as f32;
            let mut target = if beats < base { base } else { base2 };
            let mut sum = gap;
            let mut m = 0i32;
            let fail = 'walk: loop {
                // L110 / L114
                let e = self.gap(cap + self.widx - 1 - m);
                let mut k = 0usize;
                let mult = loop {
                    if k > 8 {
                        break 'walk true;
                    }
                    let Some(i) = (k..9).find(|&i| {
                        let (lo, hi, _) = GAP_BEFORE[i];
                        lo < e && hi > e
                    }) else {
                        break 'walk true;
                    };
                    k = i + 1;
                    let mult = GAP_BEFORE[i].2;
                    if (e / mult - unit).abs() >= tl {
                        continue;
                    }
                    break mult;
                };
                sum += e;
                beats += mult;
                if beats == target {
                    break 'walk false;
                }
                if beats > target {
                    if !(target > base)
                        || !(base4 > f64::from(target))
                        || !(base4 > f64::from(beats))
                        || m + 2 >= self.count
                    {
                        break 'walk true;
                    }
                    target += target;
                    if m >= 29 {
                        break 'walk true;
                    }
                    m += 1;
                    continue;
                }
                if m > 28 || m + 2 >= self.count {
                    break 'walk true;
                }
                m += 1;
            };
            if fail {
                if j >= 8 {
                    return None;
                }
                j = find(j + 1)?;
                continue 'pattern;
            }
            // L298: the tempo, then how many further gaps agree with it.
            let tempo = sum / target;
            let err = (tempo - unit).abs();
            let tl2 = tol(tempo);
            let inv = 1.0 / tempo;
            let mut n = m + 1;
            let mut w = cap + self.widx - 2 - m;
            let agree = loop {
                let e = self.gap(w);
                let q = (e * 4.0).mul_add(inv, 0.5).trunc() * 0.25;
                let r = (q + 0.5).trunc();
                let q = if q > 2.5 { r } else { q }.min(4.0);
                if (e / q - tempo).abs() >= tl2 {
                    break n;
                }
                if n > 28 {
                    break n + 1;
                }
                let more = n + 2 < self.count;
                w -= 1;
                n += 1;
                if !more {
                    break n;
                }
            };
            return Some((j + 1, tempo, err, agree));
        }
    }

    /// `calcBeat(gap)`: tries every pattern and keeps the tempo (≥ 333 ms) with the most
    /// agreeing gaps.
    fn calc_beat(&mut self, gap: f32) -> bool {
        let mut best = 0;
        let mut found = false;
        let mut start = 0;
        while let Some((next, tempo, err, agree)) = self.calc_beat_s(gap, 4.0, start) {
            if tempo >= 333.0 && agree > best {
                self.tempo = tempo;
                self.err = err;
                self.agree = agree;
                best = agree;
                found = true;
            }
            start = next;
        }
        found
    }
}

/// One band's peak tracker (`BeatSensor` +0x20, 0xa0 bytes each).
#[derive(Debug, Clone)]
struct Tracker {
    /// 0 = waiting for a peak, 1 = following a peak.
    state: i32,
    /// +0x08: how long a peak must stay the maximum (0.29 s).
    hold: u64,
    thr: f32,
    slope: f32,
    tick_slope: f32,
    floor: f32,
    /// +0x20 / +0x24: decay phase and its length in samples.
    decay: i32,
    wait: i32,
    k_peak: f32,
    k_first: f32,
    k_next: f32,
    /// +0x38 / +0x40 / +0x48: decay timings (1.7 s, 1.3 s, 1.3 s).
    t_reset: u64,
    t_first: u64,
    t_next: u64,
    decay_at: i64,
    pos: i64,
    peak: f32,
    peak_start: u64,
    peak_pos: u64,
    rate: u64,
    /// +0x80: band to copy instead of filtering (−1 = own filter).
    src: i32,
    /// +0x84: subtract a moving average.
    detrend: Option<Detrend>,
}

#[derive(Debug, Clone)]
struct Detrend {
    ring: Vec<f32>,
    at: i32,
    sum: f32,
}

impl Tracker {
    fn new(rate: u32) -> Self {
        let fs = rate as f32;
        let floor = mb_dsp::libm::exp10(-3.0) as f32;
        Self {
            state: 0,
            hold: ((rate as u64) as f32 * 0.29000002) as u64,
            thr: floor,
            slope: 0.0,
            tick_slope: 0.0,
            floor,
            decay: 0,
            wait: 0,
            k_peak: 0.75,
            k_first: 0.5,
            k_next: 0.5,
            t_reset: (fs * 1.7) as u64,
            t_first: (fs * 1.3) as u64,
            t_next: (fs * 1.3) as u64,
            decay_at: 0,
            pos: 0,
            peak: 0.0,
            peak_start: 0,
            peak_pos: 0,
            rate: rate as u64,
            src: -1,
            detrend: None,
        }
    }

    fn with_detrend(mut self, src: i32, rate: u32, scale: f32) -> Self {
        let n = (((rate as u64) as f32 * scale + 0.5) as i32).max(2);
        let n = if n < 3 { 2 } else { n };
        self.src = src;
        self.detrend = Some(Detrend {
            ring: vec![0.0; n as usize],
            at: 0,
            sum: 0.0,
        });
        self
    }
}

/// `BeatSensor` + `BeatListSignal` for the Analyzer's six bands.
pub struct BeatSensor {
    filters: Vec<IirPassFilter>,
    trackers: Vec<Tracker>,
    buffers: Vec<PeakBuffer>,
    bufs: Vec<Vec<f32>>,
    acc: [f32; BANDS],
    sub: i32,
    count: i32,
    total: i32,
    hist: [i32; 256],
    lo: i32,
    hi: i32,
    /// `BeatListSignal` +0x1c: call number << 8 | strength of the current call.
    stat: i32,
    /// The raw beat list (`Analyzer` +0xb0).
    pub beats: Vec<BeatInfo>,
}

impl BeatSensor {
    pub fn new(rate: u32, total: i32) -> Self {
        let fs = rate as f32;
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let filters = vec![
            IirPassFilter::low_path(283.0, fs, q),
            IirPassFilter::band_path_w(150.0, fs, 1.5),
            IirPassFilter::mid_path_b(283.0, 7900.0, fs, q),
            IirPassFilter::hi_path(7900.0, fs, q),
            IirPassFilter::through(),
            IirPassFilter::through(),
        ];
        let mut trackers: Vec<Tracker> = (0..BANDS).map(|_| Tracker::new(rate)).collect();
        // Band 4: the 283 Hz low band minus its 1 ms average; band 5: the 7900 Hz high band
        // minus its 0.2 ms average.
        trackers[4] = Tracker::new(rate).with_detrend(0, rate, 0.001);
        trackers[5] = Tracker::new(rate).with_detrend(3, rate, 0.000_200_000_01);
        Self {
            filters,
            trackers,
            buffers: (0..BANDS as i32).map(PeakBuffer::new).collect(),
            bufs: vec![Vec::new(); BANDS],
            acc: [0.0; BANDS],
            sub: 0,
            count: 0,
            total,
            hist: [0; 256],
            lo: 200,
            hi: 40,
            stat: 0,
            beats: Vec::new(),
        }
    }

    /// `put(mono, n, lvm)` for one block of the mono mix (`(L + R) / 65536`).
    pub fn put(&mut self, mono: &[f32]) {
        let n = mono.len();
        for b in 0..BANDS {
            let mut buf = std::mem::take(&mut self.bufs[b]);
            buf.resize(n, 0.0);
            let src = self.trackers[b].src;
            if src < 0 {
                self.filters[b].operate(mono, &mut buf);
                for x in &mut buf {
                    *x = x.abs();
                }
            } else {
                buf.copy_from_slice(&self.bufs[src as usize][..n]);
            }
            if let Some(d) = &mut self.trackers[b].detrend {
                let len = d.ring.len() as i32;
                let k = 1.0 / len as f32;
                for x in &mut buf {
                    let v = *x;
                    let at = d.at as usize;
                    d.sum += v - d.ring[at];
                    // fmsub: v − k·sum, then fmaxnm with 0.
                    let y = (-k).mul_add(d.sum, v);
                    d.ring[at] = v;
                    d.at = (d.at + 1) % len;
                    *x = if y > 0.0 { y } else { 0.0 };
                }
            }
            self.bufs[b] = buf;
        }
        for j in 0..n {
            self.count += 1;
            if self.count >= self.total {
                break;
            }
            for b in 0..BANDS {
                self.acc[b] += self.bufs[b][j];
            }
            self.sub += 1;
            if self.sub < STEP {
                continue;
            }
            self.sub = 0;
            for b in 0..BANDS {
                let v = self.acc[b] / STEP as f32;
                self.acc[b] = 0.0;
                self.tick(b, v);
            }
            if let Some(sel) = self.select_band() {
                let _ = sel;
                for b in 0..BANDS {
                    if self.buffers[b].hit {
                        let t = self.buffers[b].tempo;
                        self.change_stat(t, b);
                    }
                }
            }
        }
    }

    /// One envelope tick of band `b` (the per-band body of `put`'s tick loop).
    fn tick(&mut self, b: usize, v: f32) {
        let e = &mut self.trackers[b];
        let p = &mut self.buffers[b];
        p.hit = false;
        e.pos += i64::from(STEP);
        let pos = e.pos;
        e.thr -= e.tick_slope;
        let mut thr = e.thr;
        match e.state {
            1 => {
                let peak = e.peak;
                let confirm = if peak >= v {
                    (pos as u64).wrapping_sub(e.peak_pos) >= e.hold
                } else {
                    e.peak_start = pos as u64;
                    e.peak_pos = pos as u64;
                    e.peak = v;
                    e.thr = e.k_peak * v;
                    e.decay = 0;
                    e.wait = e.t_reset as i32;
                    e.slope = 0.0;
                    e.tick_slope = 0.0;
                    e.decay_at = pos;
                    e.hold == 0
                };
                if !confirm {
                    return;
                }
                e.state = 0;
                let prev = p.prev;
                p.prev = e.peak_start;
                let gap = if prev == 0 {
                    0.0
                } else {
                    let gap = ((e.peak_start - prev) as f32 * 1000.0) / e.rate as f32;
                    p.tempo = gap;
                    p.hit = p.calc_beat(gap);
                    gap
                };
                if p.count < RING as i32 {
                    p.count += 1;
                }
                p.ring[p.widx as usize] = Peak {
                    pos: e.peak_start as u32,
                    gap,
                };
                p.widx = (p.widx + 1) % RING as i32;
                if p.hit {
                    p.value = e.peak;
                    p.pos = e.peak_start;
                }
            }
            0 => {
                if thr < e.floor || thr.is_nan() {
                    // b.ge skips only when thr >= floor.
                    e.thr = e.floor;
                    thr = e.floor;
                }
                if e.wait < (pos as i32).wrapping_sub(e.decay_at as i32) {
                    let len = match e.decay {
                        1 => {
                            e.wait = e.t_next as i32;
                            Some((e.t_next, e.k_next * thr))
                        }
                        0 => {
                            e.decay = 1;
                            e.wait = e.t_first as i32;
                            Some((e.t_first, e.k_first * e.peak))
                        }
                        _ => None,
                    };
                    if let Some((len, to)) = len {
                        let s = (thr - to) / len as f32;
                        e.slope = s;
                        e.tick_slope = s * STEP as f32;
                        e.decay_at = pos;
                    }
                }
                if !(thr > v) {
                    e.peak_start = pos as u64;
                    e.peak_pos = pos as u64;
                    e.peak = v;
                    e.thr = e.k_peak * v;
                    e.wait = e.t_reset as i32;
                    e.decay = 0;
                    e.slope = 0.0;
                    e.tick_slope = 0.0;
                    e.decay_at = pos;
                    e.state = 1;
                }
            }
            _ => {}
        }
    }

    /// `selectBand`: votes the fitting bands' tempos into the BPM histogram, picks its mode
    /// (folding double/half tempos), and clears the bands that disagree with it.
    fn select_band(&mut self) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_err = f32::MAX;
        let mut best_bpm = 0i32;
        let mut best_tempo = 0f32;
        for (i, p) in self.buffers.iter().enumerate() {
            if !p.hit {
                continue;
            }
            let bpm = (60000.0 / p.tempo + 0.5) as i32;
            let w = (4.0 / (p.err + 1.0)) as i32;
            let w = if w > 1 { w } else { 1 };
            if let Some(h) = self.hist.get_mut(bpm as usize) {
                *h += w;
            }
            if self.lo > bpm {
                self.lo = bpm;
            }
            if self.hi < bpm {
                self.hi = bpm;
            }
            if p.err < best_err {
                best_err = p.err;
                best_bpm = bpm;
                best_tempo = p.tempo;
                best = Some(i);
            }
        }
        let mut mode = self.mode();
        let best = best?;
        let mut t = 60000.0 / mode as f32;
        if mode < 1 {
            if mode < 0 {
                return None;
            }
            mode = 0;
        } else {
            let tl = tol(best_tempo);
            if (t - best_tempo).abs() > tl {
                loop {
                    if !(((t + t) - best_tempo).abs() > tl && ((t * 0.5) - best_tempo).abs() > tl) {
                        break;
                    }
                    if self.hist.get(best_bpm as usize).copied().unwrap_or(0) < 3 {
                        return None;
                    }
                    let (lo, hi) = (self.lo, self.hi);
                    self.lo = 200;
                    self.hi = 40;
                    if lo > hi {
                        return None;
                    }
                    for i in lo..=hi {
                        let ti = 60000.0 / i as f32;
                        if !related(ti, best_tempo, tl) {
                            self.hist[i as usize] = 0;
                        } else if self.hist[i as usize] != 0 {
                            if i < self.lo {
                                self.lo = i;
                            }
                            if i > self.hi {
                                self.hi = i;
                            }
                        }
                    }
                    if self.lo > self.hi {
                        return None;
                    }
                    mode = self.mode();
                    t = 60000.0 / mode as f32;
                    if mode <= 0 {
                        if mode < 0 {
                            return None;
                        }
                        mode = 0;
                        break;
                    }
                    if !((t - best_tempo).abs() > tl) {
                        break;
                    }
                }
            }
        }
        if self.hist[mode as usize] < 3 {
            return None;
        }
        for p in &mut self.buffers {
            if p.hit && !related(t, p.tempo, tol(p.tempo)) {
                p.hit = false;
            }
        }
        Some(best)
    }

    /// Histogram mode over `lo..=hi` (first maximum; −1 when empty).
    fn mode(&self) -> i32 {
        let mut m = -1;
        let mut c = 0;
        if self.lo <= self.hi {
            for i in self.lo..=self.hi {
                if self.hist[i as usize] > c {
                    m = i;
                    c = self.hist[i as usize];
                }
            }
        }
        m
    }

    /// `BeatListSignal::changeStat(tempo, pos, value, buffer)`: files the fitting peaks of band
    /// `b` (the current one and the `agree + 1` before it) into the beat list.
    fn change_stat(&mut self, t: f32, b: usize) {
        let p = &self.buffers[b];
        let agree = p.agree;
        self.stat = self.stat.wrapping_add(agree).wrapping_add(2);
        if agree < -1 {
            self.stat = (self.stat + 0x100) & !0xff;
            return;
        }
        let bpm10 = (600_000.0 / t + 0.5) as i32;
        let tl = tol(t);
        let inv = 1.0 / t;
        let cap = RING as i32;
        let mut k = agree + 1;
        loop {
            let idx = (p.widx + !k + cap) % cap;
            let pk = p.ring[idx as usize];
            let pos = pk.pos as i32;
            let beats = (pk.gap * 4.0).mul_add(inv, 0.5).trunc() * 0.25;
            let list = &mut self.beats;
            let n = list.len() as i32;
            // Last entry at or before `pos`.
            let mut i = n;
            while i >= 1 && list[(i - 1) as usize].pos > pos {
                i -= 1;
            }
            let mut at = if n < 0 { n } else { 0 };
            let mut skip = false;
            if i >= 1 {
                at = i;
                let mut o = -1i32;
                loop {
                    let ei = i + o;
                    if ei < 0 || list[ei as usize].pos != pos {
                        at = i;
                        break;
                    }
                    let et = list[ei as usize].tempo;
                    let eb = (600_000.0 / et + 0.5) as i32;
                    let dup =
                        eb == bpm10 || eb * 2 == bpm10 || bpm10 * 2 == eb || !related(et, t, tl);
                    if dup {
                        if (self.stat as u8) <= list[ei as usize].stat as u8 {
                            skip = true;
                        } else {
                            list.remove(ei as usize);
                            at = i - 1;
                        }
                        break;
                    }
                    o -= 1;
                }
            }
            if !skip {
                let info = BeatInfo {
                    pos,
                    beats,
                    tempo: t,
                    stat: self.stat,
                    band: p.band,
                    flag: 0,
                };
                list.insert(at as usize, info);
            }
            if k <= 0 {
                break;
            }
            k -= 1;
        }
        self.stat = (self.stat + 0x100) & !0xff;
    }
}
