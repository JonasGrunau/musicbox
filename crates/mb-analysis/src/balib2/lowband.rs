//! `UnitBeatAdjust` v2's low band (`putSample` @0x10113ad58, read through `get_pdata_low`
//! @0x10113f4e4 and `calcLowInteg` @0x10113d760) and its two readers: test A of the verdict
//! (`BeatInfoAnalyzer::judgeHendouDynamicPeak` @0x1011a6714) and the offset
//! (`Analyzer::getOffsetCalc` @0x101182100). `analysis-beats-hp.md` § 8.

use crate::beats::BeatInfo;
use crate::beats::filters::Filter2D;

/// `UnitBeatAdjust` v2's low band: every fourth frame of each block, the mid signal through a
/// 100 Hz low-pass, rectified, then a 25 Hz low-pass (both designed for a quarter of the rate,
/// `create` @0x10113aabc), clamped to 0..32767. The unit's other two bands (2 kHz high-pass,
/// full band) feed nothing the constant analyser keeps.
pub struct LowBand {
    /// `+0xc`: rate / 4.
    rate4: i32,
    total: i32,
    lp: Filter2D,
    smooth: Filter2D,
    data: Vec<i16>,
}

impl LowBand {
    pub fn new(rate: u32, total: i32) -> Self {
        let rate4 = rate as i32 / 4;
        let fs = f64::from(rate4);
        Self {
            rate4,
            total,
            lp: Filter2D::low(100.0, fs),
            smooth: Filter2D::low(25.0, fs),
            data: Vec::new(),
        }
    }

    /// One block (the frames `UnitEnvelopeMaker` gets).
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        let mut take = pcm.len() as i32;
        if take < 1 {
            return;
        }
        let have = self.data.len() as i32;
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
            let y = self.smooth.step(self.lp.step(x).abs());
            self.data.push(if y > 32767.0 {
                i16::MAX
            } else if y >= 0.0 {
                y as i32 as i16
            } else {
                0
            });
            i += 4;
        }
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
            .map(|k| f64::from(self.data.get(k as usize).copied().unwrap_or(0)))
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

/// Test A: for each sensor onset before 15 minutes, the low band summed over half a beat from
/// the onset (`rate · 7.5 / bpm` quarter-rate samples); a window past the band's end stops the
/// scan. A holds when onsets × the largest sum × the mean sum is below 2e15: the onsets carry
/// little low-frequency energy. `onsets` is the sensor's list as `analyze` received it, `bpm` the
/// constant pass's 60000 / first beat length.
pub fn judge(rate: i32, onsets: &[BeatInfo], bpm: f32, total: i32, low: &LowBand) -> bool {
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
                        .map(|k| f64::from(low.data.get(k as usize).copied().unwrap_or(0)))
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
