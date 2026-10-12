//! PWAV (400 columns) and PWV2 (100 dots): BALib's `LoudViewMaker` (`BeatAnalyzer_2_0`,
//! `create` @0x10119576c, `analyzePeak` @0x101195dd8, `PeakDetect` @0x101197780, `PeakDetect900`
//! @0x101197ab4, `analyzeLoudColor` @0x101197d64), fed by `Analyzer::setSample` @0x10116bae8 and
//! `peakss::BeatSensor::put` @0x101182eb4, quantised by `CBeatAnalyzeLib::V_Devide` @0x10015c18c
//! and `CBeatAnalyzeThread::notifyDots` @0x1001593d4. Spec: `analysis-waveforms.md` § 3.
//!
//! The track is cut into chunks of 1/75 s. Three bands (150 Hz band-pass, 283–7900 Hz, above
//! 7900 Hz) go through a peak tracker; each 1/200 of the track becomes one pair of PWAV columns
//! whose height mixes the full-band level with the low-band peaks and whose colour says which band
//! dominates. PWV2 counts the peaks of a slower tracker per 1/100 of the track.

use mb_dsp::IirPassFilter;

/// Peak list capacity (`LoudViewMaker` +0x20).
const CAP: i64 = 36000;
/// Peak value thresholds (f32 as stored in the binary).
const F001: f32 = 0.01;
const D001: f64 = f64::from_bits(0x3f84_7ae1_47ae_147b);
const F66: f32 = f32::from_bits(0x3f28_f5c3);
const DECAY: f32 = f32::from_bits(0x3f7e_6baa);
const FLOOR900: f32 = f32::from_bits(0x43a3_d70a);
/// Decay step: `(int)(rate · 0.011609977324263039)` samples (512 at 44.1 kHz).
const STEP: f64 = f64::from_bits(0x3f87_c6f8_c751_f177);

/// The 23 level thresholds of `V_Devide` (`CBeatAnalyzeLib` +0x8, the same table as
/// `LoudViewMaker` +0x3388, from 0x1037528d0), largest first: ≥ T[0] → 25 … ≥ T[22] → 3, else 2.
const LEVELS: [f32; 23] = {
    const BITS: [u32; 23] = [
        0x4746_6666,
        0x4719_999a,
        0x46f0_a3d7,
        0x46ca_3d71,
        0x46b5_c28f,
        0x46a6_6666,
        0x4697_0a3d,
        0x4687_ae14,
        0x4678_51ec,
        0x4668_f5c3,
        0x4659_999a,
        0x464a_3d71,
        0x463a_e148,
        0x462b_851f,
        0x461c_28f6,
        0x460c_cccd,
        0x45fa_e148,
        0x45dc_28f6,
        0x45bd_70a4,
        0x459e_b852,
        0x4580_0000,
        0x4542_8f5c,
        0x4509_374c,
    ];
    let mut out = [0f32; 23];
    let mut i = 0;
    while i < 23 {
        out[i] = f32::from_bits(BITS[i]);
        i += 1;
    }
    out
};

/// `CBeatAnalyzeLib::V_Devide`: PWAV column height 2..25.
pub fn v_devide(v: f32) -> i32 {
    // fmaxnm(v, 0): NaN and non-positive values fall to the bottom class.
    let m = if v.is_nan() { 0.0 } else { v.max(0.0) };
    if m == 0.0 {
        return 2;
    }
    for (i, &t) in LEVELS.iter().enumerate() {
        if m >= t {
            return 25 - i as i32;
        }
    }
    2
}

/// `notifyDots`: PWV2 dot height 1..15.
pub fn dot_class(v: f32) -> u8 {
    let x = v as i64;
    if x == 0 {
        return 1;
    }
    const T: [(i64, u8); 13] = [
        (20674, 15),
        (16421, 14),
        (13044, 13),
        (10361, 12),
        (8229, 11),
        (6537, 10),
        (5192, 9),
        (4124, 8),
        (3275, 7),
        (2601, 6),
        (2066, 5),
        (1641, 4),
        (1303, 3),
    ];
    for (t, c) in T {
        if x > t {
            return c;
        }
    }
    2
}

#[derive(Clone, Copy, Default)]
struct Peak {
    value: f32,
    v900: f32,
    pos: i64,
    max_at: i64,
}

/// State of one `PeakDetect` / `PeakDetect900` tracker.
#[derive(Default)]
struct Tracker {
    counter: i64,
    idx: i32,
    chunk: i64,
    done: i32,
}

/// Per-band 400-column accumulator of peak values (+0x190/+0x1a8/+0x1c0 and friends).
struct BandCols {
    cols: Vec<f32>,
    idx: i32,
    carry: f32,
    frac: f32,
    last: i64,
    consumed: i64,
    count: i64,
}

impl BandCols {
    fn new() -> Self {
        Self {
            cols: vec![0.0; 402],
            idx: 0,
            carry: 0.0,
            frac: 0.0,
            last: 0,
            consumed: 0,
            count: 0,
        }
    }
}

/// Per-list 100-dot accumulator (+0x1d8/+0x200).
struct Dots {
    dots: Vec<i64>,
    idx: i32,
    carry: i64,
    consumed: i64,
    count: i64,
}

impl Dots {
    fn new() -> Self {
        Self {
            dots: vec![0; 101],
            idx: 0,
            carry: 0,
            consumed: 0,
            count: 0,
        }
    }
}

pub struct LoudViewMaker {
    rate: f64,
    block: i32,
    /// 1.714 · rate and 0.32 · rate.
    long_gap: f64,
    min_gap: f64,
    total: i64,
    low_filter: IirPassFilter,
    full: Vec<f32>,
    low: Vec<f32>,
    mid: Vec<f32>,
    high: Vec<f32>,
    peaks: [Vec<Peak>; 3],
    pd: [Tracker; 3],
    pd900: [Tracker; 2],
    /// Low-band onset flag (+0x290), raised by `PeakDetect` and consumed by the full-band mean.
    flag: i32,
    /// Chunks processed (+0x18).
    chunks: i64,
    /// Last emitted full-band mean (+0x140[0]) and whether this chunk emitted one (+0x158).
    mean: f32,
    mean_emitted: i64,
    /// Running |x| sum (+0x164) and chunks in it (+0x2d0).
    mean_sum: f32,
    mean_n: i32,
    /// Two-step delay for onset-triggered means (+0x2a0, +0x2d4) and their column (+0x2d8).
    mean_pending: i32,
    mean_late: i32,
    mean_col: i32,
    /// Full-band columns (+0x250), current column (+0x16c) and its carried value (+0x258).
    full_cols: Vec<f32>,
    full_idx: i32,
    full_carry: f32,
    bands: [BandCols; 3],
    dots: [Dots; 2],
    /// Columns and dots already given their final values (+0x238, +0x248).
    loud_done: i32,
    dots_done: i32,
    /// Loud entries: (height, colour); colour 5 until `analyzeLoudColor`.
    loud: Vec<(f32, i32)>,
    /// Per column: dB levels of low, mid, high (+0x12c0).
    db: Vec<[i32; 3]>,
    dots_out: Vec<f32>,
}

/// `(float)(log10((double)x · 2⁻¹⁵) · 20) + 30.5`, clamped to [0, 30], truncated.
fn db(x: f32) -> i32 {
    if x.is_nan() || x <= 0.0 {
        return 0;
    }
    let d = (f64::from(x) * f64::from_bits(0x3f00_0000_0000_0000)).log10() * 20.0;
    let v = d as f32 + 30.5;
    let v = if v > 30.0 {
        30.0
    } else if v > 0.0 {
        v
    } else {
        0.0
    };
    v as i32
}

impl LoudViewMaker {
    /// `create(rate, total)`; `total` is the number of samples the analysis feeds (`L − 1`).
    pub fn new(rate: u32, total: i64) -> Self {
        let r = f64::from(rate);
        let block = rate as i32 / 75;
        Self {
            rate: r,
            block,
            long_gap: 1.714 * r,
            min_gap: 0.32 * r,
            total,
            low_filter: IirPassFilter::band_path_q(150.0, rate as f32, 0.9),
            full: vec![0.0; block as usize],
            low: vec![0.0; block as usize],
            mid: vec![0.0; block as usize],
            high: vec![0.0; block as usize],
            peaks: std::array::from_fn(|_| {
                let mut v = vec![Peak::default(); CAP as usize + 1];
                v[0] = Peak {
                    value: F001,
                    v900: F001,
                    pos: 0,
                    max_at: 0,
                };
                v
            }),
            pd: Default::default(),
            pd900: Default::default(),
            flag: 0,
            chunks: 0,
            mean: 0.0,
            mean_emitted: 0,
            mean_sum: 0.0,
            mean_n: 0,
            mean_pending: 0,
            mean_late: 0,
            mean_col: 0,
            full_cols: vec![0.0; 402],
            full_idx: 0,
            full_carry: 0.0,
            bands: [BandCols::new(), BandCols::new(), BandCols::new()],
            dots: [Dots::new(), Dots::new()],
            loud_done: 0,
            dots_done: 0,
            loud: vec![(0.0, 5); 400],
            db: vec![[0; 3]; 400],
            dots_out: vec![0.0; 100],
        }
    }

    /// `PeakDetect`: tracks onsets of one band; returns the number of peaks.
    fn peak_detect(&mut self, which: usize, n: usize) -> i64 {
        let band = which as i32;
        let thr = if band == 2 { F66 } else { 0.75f32 };
        let data: &[f32] = match which {
            0 => &self.low,
            1 => &self.mid,
            _ => &self.high,
        };
        let peaks = &mut self.peaks[which];
        let st = &mut self.pd[which];
        let mut k = i64::from(st.idx);
        let mut c = st.counter;
        let mut cur = if k < 1 {
            peaks[0].value
        } else if CAP > k {
            peaks[k as usize].value
        } else {
            0.75
        };
        let lim = (self.rate * STEP) as i32 as i64;
        let mut hold = 0i64;
        let n = n as i64;
        let mut i = 0i64;
        while i < n {
            c += 1;
            let x = data[i as usize];
            if peaks[0].value == F001 {
                if f64::from(x) > D001 {
                    peaks[0].value = x;
                    peaks[0].pos = c;
                    peaks[0].max_at = 0;
                    c = 0;
                    if band == 0 {
                        self.flag = 1;
                    }
                    k = 1;
                    cur = x;
                }
            } else {
                let above = (f64::from(cur) > D001 && x >= cur * thr)
                    || (f64::from(cur) <= D001 && f64::from(x) >= D001);
                if !above {
                    if self.long_gap >= c as f64 {
                        hold = 0;
                    } else {
                        hold += 1;
                        if hold >= lim {
                            hold = 0;
                            cur *= DECAY;
                            if f64::from(cur) <= D001 {
                                cur = F001;
                            }
                        }
                    }
                } else if (c as f64) < self.min_gap {
                    if x > cur {
                        if CAP > k {
                            peaks[k as usize].max_at = c;
                        }
                        cur = x;
                    }
                } else {
                    if CAP > k {
                        peaks[k as usize].pos = c;
                    }
                    if band == 0 {
                        self.flag = 1;
                    }
                    if k < 1 {
                        return 0;
                    }
                    if CAP >= k {
                        peaks[(k - 1) as usize].value = cur;
                    }
                    c = 0;
                    hold = 0;
                    k += 1;
                    cur = x;
                }
            }
            if st.chunk * i64::from(self.block) + i == self.total - 1 && i64::from(self.block) >= n
            {
                if CAP > k {
                    peaks[k as usize].value = cur;
                    peaks[k as usize].pos = c;
                }
                k += 1;
                st.done = 1;
            }
            i += 1;
        }
        if CAP > k {
            peaks[k as usize].value = cur;
        }
        st.counter = c;
        st.idx = k as i32;
        st.chunk += 1;
        k
    }

    /// `PeakDetect900`: a slower tracker on every fourth sample (peaks above 327.68).
    fn peak_detect900(&mut self, which: usize, n: usize) -> i64 {
        let band = which as i32;
        let thr = if band == 2 { F66 } else { 0.75f32 };
        let data: &[f32] = if which == 0 { &self.low } else { &self.mid };
        let peaks = &mut self.peaks[which];
        let st = &mut self.pd900[which];
        let mut k = i64::from(st.idx);
        let mut c = st.counter;
        let mut cur = if k < 1 {
            peaks[0].v900
        } else if CAP > k {
            peaks[k as usize].v900
        } else {
            0.0
        };
        let lim = (self.rate * STEP) as i32 as i64;
        let mut hold = 0i64;
        let mut acc = 0f32;
        let n = n as i64;
        let block = i64::from(self.block);
        let mut i = 0i64;
        while i < n {
            c += 1;
            let m = fmaxnm32(data[i as usize], acc);
            let last = st.chunk * block + i == self.total - 1;
            // A sample off the 4-grid is still processed when it is the track's last one, but
            // only in a short final chunk; the end test after processing also accepts a full one.
            let at_end = last && block >= n;
            if c & 3 != 0 && !(last && block > n) {
                acc = m;
                i += 1;
                continue;
            }
            let mut v = m;
            if peaks[0].v900 == F001 {
                if v > FLOOR900 {
                    c = 0;
                    peaks[0].v900 = v;
                    k = 1;
                } else {
                    v = cur;
                }
            } else {
                let above =
                    (cur > FLOOR900 && v >= cur * thr) || (cur <= FLOOR900 && v >= FLOOR900);
                if !above {
                    if self.long_gap >= c as f64 {
                        hold = 0;
                        v = cur;
                    } else {
                        hold += 4;
                        if hold < lim {
                            v = cur;
                        } else {
                            hold = 0;
                            v = fmaxnm32(cur * DECAY, FLOOR900);
                        }
                    }
                } else if (c as f64) < self.min_gap {
                    if v <= cur {
                        v = cur;
                    }
                } else {
                    if CAP > k {
                        peaks[k as usize].v900 = cur;
                    }
                    c = 0;
                    hold = 0;
                    k += 1;
                }
            }
            if at_end {
                if CAP > k {
                    peaks[k as usize].v900 = v;
                }
                k += 1;
                st.done = 1;
            }
            acc = 0.0;
            cur = v;
            i += 1;
        }
        if CAP > k {
            peaks[k as usize].v900 = cur;
        }
        st.counter = c;
        st.idx = k as i32;
        st.chunk += 1;
        k
    }

    /// `analyzePeak(mono, NULL, mid, high, n)` for one 1/75 s chunk. `mono` is the Analyzer's
    /// `(L + R) / 65536` mix; `mid` and `high` are BeatSensor's rectified bands 2 and 3.
    pub fn analyze_peak(&mut self, mono: &[f32], mid: &[f32], high: &[f32]) -> bool {
        let n = mono.len();
        let mut lowbuf = vec![0f32; n];
        self.low_filter.operate(mono, &mut lowbuf);
        for x in &mut lowbuf {
            *x = x.abs();
        }
        let block = i64::from(self.block);
        let per = self.total / block;
        let dot_div = if (per + 99) as u64 >= 199 {
            per / 100
        } else {
            1
        };
        let nunits = ((self.total as f32) / (self.block as f32)).ceil();
        let f61 = nunits * 0.005f32;
        let f42 = 1.0f32 / nunits;
        let mut remaining = n as i64;
        let mut off = 0usize;
        while remaining > 0 {
            self.chunks += 1;
            let m = remaining.min(block) as usize;
            for j in 0..m {
                self.full[j] = mono[off + j] * 32768.0;
                self.low[j] = lowbuf[off + j] * 32768.0;
                self.mid[j] = mid[off + j] * 32768.0;
                self.high[j] = high[off + j] * 32768.0;
            }
            let c0 = self.peak_detect(0, m);
            self.bands[0].count = c0;
            let d0 = self.peak_detect900(0, m);
            self.dots[0].count = d0;
            self.full_mean(m);
            let c1 = self.peak_detect(1, m);
            self.bands[1].count = c1;
            let d1 = self.peak_detect900(1, m);

            self.dots[1].count = d1;
            let c2 = self.peak_detect(2, m);
            self.bands[2].count = c2;
            if !self.full_columns(f42) {
                return false;
            }
            for b in 0..3 {
                if !self.band_columns(b, f61, f42) {
                    return false;
                }
            }
            self.loud_columns();
            for d in 0..2 {
                if !self.dot_columns(d, dot_div) {
                    return false;
                }
            }
            self.dot_values();
            remaining -= m as i64;
            off += m;
        }
        true
    }

    /// The full-band mean |x|, emitted at low-band onsets or every nine chunks.
    fn full_mean(&mut self, m: usize) {
        let x = &self.full[..m];
        let mut s = self.mean_sum;
        let mut j = 0;
        if m >= 16 {
            let end = m & !15;
            let mut v = [[0f32; 4]; 4];
            v[0][0] = s;
            while j < end {
                for (q, lane) in v.iter_mut().enumerate() {
                    for (l, acc) in lane.iter_mut().enumerate() {
                        *acc += x[j + q * 4 + l].abs();
                    }
                }
                j += 16;
            }
            // v1 += v2; v0 += v1; v0 = v3 + v0; pairwise: (a0 + a1) + (a2 + a3).
            let mut t = [0f32; 4];
            for l in 0..4 {
                let v1 = v[1][l] + v[0][l];
                let v0 = v[2][l] + v1;
                t[l] = v[3][l] + v0;
            }
            s = (t[0] + t[1]) + (t[2] + t[3]);
        }
        while j < m {
            s += x[j].abs();
            j += 1;
        }
        let k = self.mean_n;
        self.mean_n = k + 1;
        if k > 7 || self.flag == 1 {
            self.mean = s / ((self.block * (k + 1)) as f32);
            if self.mean_late == 1 {
                self.mean_pending = 1;
                self.mean_late = 0;
            }
            if self.flag == 1 {
                self.mean_late = 1;
            }
            self.mean_n = 0;
            self.flag = 0;
            s = 0.0;
            self.mean_emitted = 1;
        } else {
            self.mean_emitted = 0;
        }
        self.mean_sum = s;
    }

    /// Full-band columns (+0x250): the mean level, spread over 1/400 steps of the track.
    fn full_columns(&mut self, f42: f32) -> bool {
        let chunks = self.chunks;
        let mut col = ((chunks as f32) * 400.0 * f42) as i64;
        if col >= 399 {
            col = 399;
        }
        if self.pd[0].done == 1 {
            col = 400;
        }
        let mut idx = i64::from(self.full_idx);
        self.full_cols[idx as usize] = self.full_carry;
        if col > idx {
            self.full_cols[idx as usize] = self.full_carry * 15000.0 * f42;
            while idx != col {
                if idx <= 398 {
                    self.full_cols[(idx + 1) as usize] = 0.0;
                }
                idx += 1;
            }
        }
        if self.mean_emitted >= 1 && idx <= 399 {
            let m = self.mean;
            if self.mean_pending == 1 {
                let at = i64::from(self.mean_col);
                let v = if idx == at { m } else { m * 15000.0 * f42 };
                self.full_cols[at as usize] += v;
            } else {
                self.full_cols[idx as usize] += m;
                self.mean_col = idx as i32;
            }
            self.mean_pending = 0;
        }
        if idx <= 399 {
            self.full_carry = self.full_cols[idx as usize];
        }
        self.full_idx = idx as i32;
        idx <= 400
    }

    /// Peak sums per pair of columns for band `b` (low, mid, high).
    fn band_columns(&mut self, b: usize, f61: f32, f42: f32) -> bool {
        let chunks = self.chunks;
        let done = self.pd[b].done;
        let peaks = &self.peaks[b];
        let st = &mut self.bands[b];
        if st.idx > 399 {
            return true;
        }
        let count = st.count;
        let end = if done == 1 { count } else { count - 1 };
        if done != 1 && st.consumed >= end {
            return true;
        }
        let mut idx = i64::from(st.idx);
        let mut carry = st.carry;
        st.cols[idx as usize] = carry;
        let mut frac = st.frac + (chunks - st.last) as f32;
        let mut overflow = false;
        if st.consumed < end {
            let mut p = st.consumed;
            loop {
                let mut at = idx;
                if f61 <= frac {
                    let mut fell_through = true;
                    if idx < 400 {
                        let v = carry * 7500.0 * f42;
                        st.cols[idx as usize] = v;
                        st.cols[(idx + 1) as usize] = v;
                        at = idx + 2;
                        frac -= f61;
                        let mut b2 = idx < 398;
                        let mut zero_one = false;
                        if f61 <= frac {
                            if idx < 398 {
                                let mut j = at;
                                loop {
                                    st.cols[j as usize] = 0.0;
                                    st.cols[(j + 1) as usize] = 0.0;
                                    at = j + 2;
                                    frac -= f61;
                                    b2 = j < 398;
                                    if frac < f61 {
                                        break;
                                    }
                                    let b1 = j < 398;
                                    j = at;
                                    if !b1 {
                                        break;
                                    }
                                }
                            }
                            if b2 {
                                zero_one = true;
                            }
                        } else if b2 {
                            zero_one = true;
                        }
                        if zero_one {
                            st.cols[at as usize] = 0.0;
                            carry = 0.0;
                            fell_through = false;
                        }
                    }
                    if fell_through {
                        st.last = chunks;
                        overflow = true;
                        idx = at;
                        break;
                    }
                }
                let skip_last = if done == 1 {
                    p == count - 1
                } else {
                    p == count - 2 && done != 0
                };
                if !skip_last && at < 400 && p < CAP {
                    carry += (peaks[p as usize].value).trunc();
                    st.cols[at as usize] = carry;
                }
                p += 1;
                idx = at;
                if p == end {
                    break;
                }
            }
        }
        if !overflow {
            st.last = chunks;
            if idx < 400 {
                st.carry = carry;
            }
        }
        st.frac = frac;
        st.idx = idx as i32;
        st.consumed = end;
        !(done != 1 && idx > 400)
    }

    /// Height and dB levels of the columns every band has reached.
    fn loud_columns(&mut self) {
        let limit = if self.pd[0].done == 1 && self.pd[1].done == 1 && self.pd[2].done == 1 {
            400
        } else {
            self.full_idx
                .min(self.bands[0].idx)
                .min(self.bands[1].idx)
                .min(self.bands[2].idx)
        };
        let inv = 1.0f32 / 32768.0;
        for c in self.loud_done.max(0)..limit {
            let c = c as usize;
            let full = self.full_cols[c];
            let low = self.bands[0].cols[c];
            self.db[c] = [
                db(low * 0.5),
                db(self.bands[1].cols[c] * 0.25),
                db(self.bands[2].cols[c]),
            ];
            let h = fminnm32((low * 0.75).mul_add(inv, 0.25), 1.0) * full;
            self.loud[c].0 = h;
        }
        if limit > self.loud_done {
            self.loud_done = limit;
        }
    }

    /// Peak counts per dot (+0x1d8 low, +0x200 mid).
    fn dot_columns(&mut self, d: usize, dot_div: i64) -> bool {
        let chunks = self.chunks;
        let done = self.pd900[d].done;
        let peaks = &self.peaks[d];
        let st = &mut self.dots[d];
        if st.idx > 99 {
            return true;
        }
        let mut idx = i64::from(st.idx);
        let mut target = chunks / dot_div;
        if target >= 99 {
            target = 99;
        }
        if done == 1 {
            target = 100;
        }
        st.dots[idx as usize] = st.carry;
        if target > idx {
            st.dots[idx as usize] = (st.carry * 75) / (2 * dot_div);
            while idx != target {
                if idx <= 98 {
                    st.dots[(idx + 1) as usize] = 0;
                }
                idx += 1;
            }
        }
        let add = st.count != st.consumed && chunks >= 51 && st.count > st.consumed && idx <= 99;
        if add {
            if st.consumed < CAP {
                st.dots[idx as usize] += peaks[st.consumed as usize].v900 as i64;
            }
            st.carry = st.dots[idx as usize];
        } else if idx <= 99 {
            st.carry = st.dots[idx as usize];
        }
        st.idx = idx as i32;
        st.consumed = st.count;
        idx <= 100
    }

    fn dot_values(&mut self) {
        let limit = if self.pd900[0].done == 1 && self.pd900[1].done == 1 {
            100
        } else {
            self.dots[0].idx.min(self.dots[1].idx)
        };
        for c in self.dots_done.max(0)..limit {
            let c = c as usize;
            let a = self.dots[0].dots[c];
            let b = self.dots[1].dots[c];
            self.dots_out[c] = if a == 0 {
                if b == 0 { 0.0 } else { 1.0 }
            } else {
                a as f32
            };
        }
        if limit > self.dots_done {
            self.dots_done = limit;
        }
    }

    /// PWAV and PWV2 bytes.
    pub fn finish(mut self) -> (Vec<u8>, Vec<u8>) {
        let n = self.loud_done.clamp(0, 400) as usize;
        // analyzeLoudColor.
        for c in 0..n {
            let [a, b, h] = self.db[c];
            let d = b - h;
            self.loud[c].1 = if a < 14 {
                if d < 4 { 5 } else { 4 }
            } else if d < 0 {
                if a > h { 2 } else { 3 }
            } else if a > b {
                0
            } else {
                1
            };
        }
        let pwav = (0..400)
            .map(|c| {
                if c < n {
                    let (h, col) = self.loud[c];
                    ((v_devide(h) as u8) & 0x1f) | ((col as u8) << 5)
                } else {
                    0
                }
            })
            .collect();
        let nd = self.dots_done.clamp(0, 100) as usize;
        let pwv2 = (0..100)
            .map(|c| {
                if c < nd {
                    dot_class(self.dots_out[c])
                } else {
                    0
                }
            })
            .collect();
        (pwav, pwv2)
    }
}

#[inline]
fn fmaxnm32(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a == b {
        if a.is_sign_negative() { b } else { a }
    } else if a > b {
        a
    } else {
        b
    }
}

#[inline]
fn fminnm32(a: f32, b: f32) -> f32 {
    if a.is_nan() || (!b.is_nan() && b <= a) {
        b
    } else {
        a
    }
}

/// `Analyzer::setSample` + `BeatSensor::put` for the loud view: 1/75 s blocks of the mono mix,
/// BeatSensor's bands 2 (`createMidPathFilterB(283, 7900)`) and 3 (`createHiPathFilter(7900)`).
pub struct LoudAnalyzer {
    block: usize,
    total: u64,
    seen: u64,
    mono: Vec<f32>,
    band2: IirPassFilter,
    band3: IirPassFilter,
    b2: Vec<f32>,
    b3: Vec<f32>,
    lvm: LoudViewMaker,
    stopped: bool,
}

impl LoudAnalyzer {
    pub fn new(rate: u32, total: u64) -> Self {
        let fs = rate as f32;
        let q = std::f32::consts::FRAC_1_SQRT_2;
        Self {
            block: (rate / 75) as usize,
            total,
            seen: 0,
            mono: Vec::new(),
            band2: IirPassFilter::mid_path_b(283.0, 7900.0, fs, q),
            band3: IirPassFilter::hi_path(7900.0, fs, q),
            b2: Vec::new(),
            b3: Vec::new(),
            lvm: LoudViewMaker::new(rate, total as i64),
            stopped: false,
        }
    }

    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        for &[l, r] in pcm {
            if self.seen >= self.total || self.stopped {
                return;
            }
            self.mono
                .push((f32::from(r) + f32::from(l)) * 1.5258789e-05);
            self.seen += 1;
            if self.mono.len() >= self.block || self.seen >= self.total {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        let n = self.mono.len();
        self.b2.resize(n, 0.0);
        self.b3.resize(n, 0.0);
        self.band2.operate(&self.mono, &mut self.b2);
        self.band3.operate(&self.mono, &mut self.b3);
        for x in self.b2.iter_mut().chain(self.b3.iter_mut()) {
            *x = x.abs();
        }

        if !self.lvm.analyze_peak(&self.mono, &self.b2, &self.b3) {
            self.stopped = true;
        }
        self.mono.clear();
    }

    pub fn finish(self) -> (Vec<u8>, Vec<u8>) {
        self.lvm.finish()
    }
}
