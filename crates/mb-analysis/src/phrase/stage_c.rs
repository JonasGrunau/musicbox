//! Stage C: `Sub_Main_C_init` @0x1007d2... and `Sub_Main_C` @0x1007d2a80, fed by `Analyze_CD`
//! from a second read of the track. The track is cut into blocks of about 35 s on bar lines;
//! each block's features are written into per-beat arrays for stage D
//! (`analysis-phrase.md` § 6).

use super::feat;
use super::filter::Decimator;
use super::hp::{Kind, RhythmHp};
use super::power::LowPower16;
use super::spec::{self, Cqt, Fft};

/// `Set_Sub_Main_C_Out`'s arrays (+0xe50 … +0x10f8).
#[derive(Debug, Clone, Default)]
pub struct COut {
    /// +0xe90 / +0x1068: beats.
    pub n: i32,
    /// +0xed8: the bar number of the last block's first beat (+0xb90 when `Set_Sub_Main_C_Out`
    /// ran).
    pub bar: i32,
    /// +0xe50: `LowPowerAnalyze16` quarters (4 per beat).
    pub low_power: Vec<f64>,
    /// +0xe70 / +0xe98: `AllPowerAnalyze16` quarter energies and peaks.
    pub all_power: Vec<f64>,
    pub all_peak: Vec<f64>,
    /// +0xeb8 / +0xfb8 / +0xf18: `LowPeakAnalyze` per beat, per quarter, jump flags.
    pub low_peak: Vec<f64>,
    pub low_peak_q: Vec<f64>,
    pub low_flags: Vec<f64>,
    /// +0xee0: `FFT_calc_1` lines (4 per bar × 1024).
    pub fft1: Vec<Vec<f64>>,
    /// +0xef8: `MiddlePeakAnalyze`.
    pub mid_peak: Vec<f64>,
    /// +0xf38: `HiHatAnalyze_a` onset counts.
    pub hihat: Vec<u8>,
    /// +0xf58 / +0xf98: `RhythmPatternAnalyze{,2}_HP` (16 per bar).
    pub pattern: Vec<u8>,
    pub pattern2: Vec<u8>,
    /// +0xf78: `LowLevelAnalyze_a`.
    pub low_level: Vec<f64>,
    /// +0xfd8: `MiddleLevelAnalyze_a`.
    pub mid_level: Vec<f64>,
    /// +0xff8 / +0x1018: `DividedBandAnalyze`.
    pub band_lo: Vec<f64>,
    pub band_hi: Vec<f64>,
    /// +0x1050: `FFT_calc_2` lines (2 per bar × 2048).
    pub fft2: Vec<Vec<f64>>,
    /// +0x1070: `LowPowerAnalyze16_40Hz` quarters.
    pub low40: Vec<f64>,
    /// +0x1090: the constant-Q frames (72 lines); +0x10a8 their count, +0x10ac the first
    /// block's downbeat frame.
    pub cqt: Vec<Vec<f64>>,
    pub cqt_frames: i32,
    pub cqt_first: i32,
    /// +0x10b0: `AllLevelAnalyze_a`.
    pub all_level: Vec<f64>,
    /// +0x10d0: `HighPeakAnalyze32` (8 per beat).
    pub high_peak: Vec<f64>,
    /// +0x10f0: 1 once a non-zero fs/32 sample was seen.
    pub nonzero: i32,
    /// +0x10f8: the fs/8 beat array as the last block left it.
    pub beats8: Vec<i32>,
}

/// One block's split point and the samples carried to the next block.
fn split(buf: &mut Vec<f64>, carry: &mut Vec<f64>, pos: i32, count: i32, guard: bool) -> i32 {
    if pos <= count && guard {
        let lo = (pos - 1) as usize;
        *carry = buf[lo..count as usize].to_vec();
        buf.truncate(lo);
        pos - 1
    } else {
        count
    }
}

/// Writes `src` so that it ends at `end` (the `memcpy`s after each feature).
fn put<T: Copy>(dst: &mut [T], end: usize, src: &[T]) {
    // The binary has no bounds check here; out-of-range parts are dropped.
    for (i, &v) in src.iter().enumerate() {
        if let Some(d) = (end + i)
            .checked_sub(src.len())
            .and_then(|k| dst.get_mut(k))
        {
            *d = v;
        }
    }
}

pub struct StageC {
    rate: f64,
    fs2: f64,
    fs8: f64,
    fs32: f64,
    bpm: f64,
    /// +0xb40: seconds per beat.
    beat_s: f64,
    total: i64,
    chunk: usize,
    fed: i64,
    mono: Vec<f64>,
    res2: Decimator,
    res8: Decimator,
    res32: Decimator,
    buf2: Vec<f64>,
    buf8: Vec<f64>,
    buf32: Vec<f64>,
    carry2: Vec<f64>,
    carry8: Vec<f64>,
    carry32: Vec<f64>,
    /// +0xb38: 0 filling, 1 block complete, 2 last block.
    state: i32,
    /// +0xb50 / +0xb58: seconds into the block / into the track.
    elapsed: f64,
    elapsed_total: f64,
    /// +0xb60 / +0xb64 / +0xb68.
    cnt2: i32,
    cnt8: i32,
    cnt32: i32,
    /// +0xb74: input samples consumed.
    consumed: i64,
    /// +0xb6c: bars in the current block.
    bars_blk: i32,
    /// +0xb78: block number (1-based).
    block: i32,
    /// +0xb80: the last beat peak of the previous block.
    prev_peak: f64,
    /// +0xb88 / +0xb90: first beat (ms) and its bar for the current block; +0xb8c the first.
    fb_blk: i32,
    bar_blk: i32,
    bar0: i32,
    /// +0x1178 and the beat starts at fs/2, fs/8, fs/32 (+0x1118, +0x1138, +0x1158).
    n: i32,
    beats2: Vec<i32>,
    beats8: Vec<i32>,
    beats32: Vec<i32>,
    /// +0xdd8: beats analysed so far.
    done_beats: i32,
    low_peak: feat::LowPeak,
    low_level: feat::LowLevel,
    low16: LowPower16,
    low40: feat::LowPower40,
    mid_level: feat::MiddleLevel,
    mid_peak: feat::MiddlePeak,
    rhythm: RhythmHp,
    rhythm2: RhythmHp,
    cqt: Cqt,
    fft1: Fft,
    fft2: Fft,
    band: feat::DividedBand,
    high: feat::HighPeak,
    out: COut,
    pub finished: bool,
    pub failed: bool,
}

impl StageC {
    /// `Sub_Main_C_init(fb_ms, bar)`; `None` when the binary's init fails.
    pub fn new(rate: u32, total: u64, bpm100: i32, fb_ms: i32, bar: i32) -> Option<Self> {
        let rate_f = f64::from(rate);
        let fs2 = rate_f * 0.5;
        let fs8 = rate_f * 0.125;
        let fs32 = rate_f * 0.03125;
        let bpm = f64::from(bpm100) * 0.01;
        let total = total.min(i32::MAX as u64) as i64;
        let beat_s = 60.0 / bpm;
        let tf = total as f64;
        // BeatsSamplesCalcFX.
        let fbs = f64::from(fb_ms) * 0.001;
        let rates = [(tf / 2.0, fs2), (tf / 8.0, fs8), (tf / 32.0, fs32)];
        let geo: Vec<(f64, f64, f64)> = rates
            .iter()
            .map(|&(len, fs)| {
                let p = beat_s * fs;
                let off = fbs.mul_add(fs, 0.5).trunc();
                (len, p, off)
            })
            .collect();
        let counts: Vec<i32> = geo
            .iter()
            .map(|&(len, p, off)| ((len - off) / p) as i32)
            .collect();
        let mut n = counts[0];
        if counts[1] < n {
            n = counts[1];
        }
        if !(n < counts[2]) {
            n = counts[2];
        }
        if n < 0 {
            return None;
        }
        let mut arrays: Vec<Vec<i32>> = geo
            .iter()
            .map(|&(_, p, off)| {
                (0..=n)
                    .map(|i| p.mul_add(f64::from(i), off + 1.5) as i32)
                    .collect()
            })
            .collect();
        if n >= 1 {
            for (a, &(len, _, _)) in arrays.iter_mut().zip(&geo) {
                let l = (n - 1) as usize;
                if (len as i32) < a[l] {
                    a[l] = len as i32;
                }
            }
        }
        let beats32 = arrays.pop()?;
        let beats8 = arrays.pop()?;
        let beats2 = arrays.pop()?;
        let nbars = (n - bar + 1) / 4;
        let nu = n.max(0) as usize;
        let cqt_cols = (((tf * 0.125) - (fs8 * 1.2230919765166333).trunc()) * 0.00390625) as i32;
        let out = COut {
            n,
            low_power: vec![0.0; 4 * nu],
            all_power: vec![0.0; 4 * nu],
            all_peak: vec![0.0; 4 * nu],
            low_peak: vec![0.0; nu],
            low_peak_q: vec![0.0; 4 * nu],
            low_flags: vec![0.0; nu],
            fft1: vec![vec![0.0; 0x400]; (nbars.max(0) * 4) as usize],
            mid_peak: vec![0.0; nu],
            hihat: vec![0; nu],
            pattern: vec![0; (nbars.max(0) * 16) as usize],
            pattern2: vec![0; (nbars.max(0) * 16) as usize],
            low_level: vec![0.0; nu],
            mid_level: vec![0.0; nu],
            band_lo: vec![0.0; nu],
            band_hi: vec![0.0; nu],
            fft2: vec![vec![0.0; 0x800]; (nbars.max(0) * 2) as usize],
            low40: vec![0.0; 4 * nu],
            cqt: vec![vec![0.0; cqt_cols.max(0) as usize]; 0x48],
            all_level: vec![0.0; nu],
            high_peak: vec![0.0; 8 * nu],
            ..COut::default()
        };
        Some(Self {
            rate: rate_f,
            fs2,
            fs8,
            fs32,
            bpm,
            beat_s,
            total,
            chunk: (rate_f * 1.7066666666666668 + 0.5) as usize,
            fed: 0,
            mono: Vec::new(),
            res2: Decimator::high(fs2 * 0.55, rate_f, 2),
            res8: Decimator::low(fs8 * 0.4, rate_f, 8),
            res32: Decimator::low(fs32 * 0.4, rate_f, 32),
            buf2: Vec::new(),
            buf8: Vec::new(),
            buf32: Vec::new(),
            carry2: Vec::new(),
            carry8: Vec::new(),
            carry32: Vec::new(),
            state: 0,
            elapsed: 0.0,
            elapsed_total: 0.0,
            cnt2: 0,
            cnt8: 0,
            cnt32: 0,
            consumed: 0,
            bars_blk: 0,
            block: 1,
            prev_peak: 0.0,
            fb_blk: fb_ms,
            bar_blk: bar,
            bar0: bar,
            n,
            beats2,
            beats8,
            beats32,
            done_beats: 0,
            low_peak: feat::LowPeak::new(fs32),
            low_level: feat::LowLevel::new(fs32),
            low16: LowPower16::new(fs32),
            low40: feat::LowPower40::new(fs32),
            mid_level: feat::MiddleLevel::new(fs8),
            mid_peak: feat::MiddlePeak::new(fs8),
            rhythm: RhythmHp::new(Kind::Low, fs8),
            rhythm2: RhythmHp::new(Kind::Band, fs8),
            cqt: Cqt::new(fs8),
            fft2: Fft::new(0x1000),
            fft1: Fft::new(0x800),
            band: feat::DividedBand::new(fs32, fs8),
            high: feat::HighPeak::new(fs2),
            out,
            finished: false,
            failed: false,
        })
    }

    /// +0xc0: stage C/D runs only with more than 30 beats after the first downbeat.
    pub fn enough_beats(&self) -> bool {
        self.n - self.bar0 > 30
    }

    /// `Analyze_CD`: buffers the mono mix and runs `Sub_Main_C` per chunk.
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        for &[l, r] in pcm {
            if self.finished || self.failed || self.fed >= self.total {
                break;
            }
            let l = f64::from(l);
            self.mono
                .push(f64::from(r).mul_add(super::GAIN_R, l * super::GAIN_L));
            self.fed += 1;
            if self.mono.len() >= self.chunk {
                self.step();
            }
        }
        if self.fed >= self.total {
            while !self.finished && !self.failed {
                self.step();
            }
        }
    }

    fn step(&mut self) {
        let n = self.mono.len().min(self.chunk);
        let chunk: Vec<f64> = self.mono.drain(..n).collect();
        if self.sub_main_c(&chunk).is_none() {
            self.failed = true;
        }
    }

    /// The stage C result once the last block is through.
    pub fn take(&mut self) -> Option<COut> {
        (self.finished && !self.failed).then(|| std::mem::take(&mut self.out))
    }

    fn sub_main_c(&mut self, chunk: &[f64]) -> Option<()> {
        let x2 = self.res2.downsample(chunk);
        let x8 = self.res8.downsample(chunk);
        let x32 = self.res32.downsample(chunk);
        self.buf2.extend_from_slice(&x2);
        self.buf8.extend_from_slice(&x8);
        self.buf32.extend_from_slice(&x32);
        self.consumed += chunk.len() as i64;
        let bar_in = self.bar_blk;
        let fb_in = self.fb_blk;
        let fb_s = f64::from(fb_in) * 0.001;
        self.bars_blk = ((35.11 - self.beat_s.mul_add(f64::from(bar_in - 1), fb_s))
            / (self.beat_s * 4.0)) as i32;
        let mut beats_blk = bar_in - 1 + self.bars_blk * 4;
        let dsec = chunk.len() as f64 / self.rate;
        self.cnt2 += x2.len() as i32;
        self.cnt8 += x8.len() as i32;
        self.cnt32 += x32.len() as i32;
        self.elapsed += dsec;
        self.elapsed_total += dsec;
        let t0 = if self.block == 1 { fb_s } else { 0.0 };
        let (mut i17, mut i19, mut i16) = (0, 0, 0);
        if self.elapsed >= self.beat_s.mul_add(f64::from(beats_blk), fb_s)
            && self.n >= self.done_beats + beats_blk
        {
            self.state = 1;
            let k = beats_blk as usize;
            let (p2, p8, p32) = (self.beats2[k], self.beats8[k], self.beats32[k]);
            i17 = split(&mut self.buf2, &mut self.carry2, p2, self.cnt2, true);
            let len8 = self.buf8.len() as i32;
            i19 = split(
                &mut self.buf8,
                &mut self.carry8,
                p8,
                self.cnt8,
                p8 <= len8 && self.cnt8 <= len8,
            );
            let len32 = self.buf32.len() as i32;
            i16 = split(
                &mut self.buf32,
                &mut self.carry32,
                p32,
                self.cnt32,
                p32 <= len32 && self.cnt32 <= len32,
            );
            let k = -(4.0 * f64::from(self.bars_blk) + f64::from(bar_in - 1));
            self.elapsed += k.mul_add(self.beat_s, -fb_s);
        }
        if self.total <= self.consumed {
            if self.state == 0 {
                i17 = self.cnt2;
                i19 = self.cnt8;
                i16 = self.cnt32;
                beats_blk = ((self.elapsed - t0) / self.beat_s) as u32 as i32;
            }
            self.state = 2;
        }
        let state = self.state;
        if state < 1 || beats_blk < 1 || self.n <= self.done_beats {
            if state == 2 {
                self.set_out(bar_in);
            }
            return Some(());
        }
        if self.out.nonzero == 0 && self.buf32.iter().any(|&v| v != 0.0) {
            self.out.nonzero = 1;
        }
        let nb = (beats_blk.min(self.n - self.done_beats)).max(0) as usize;
        self.done_beats = (self.done_beats + beats_blk).min(self.n);
        let done = self.done_beats as usize;
        self.features(nb, done, fb_in, bar_in)?;
        if state == 2 {
            self.set_out(bar_in);
            return Some(());
        }
        // Block complete: keep the carried samples, rebase the beat arrays.
        let restore = |buf: &mut Vec<f64>, carry: &mut Vec<f64>, cut: i32, cnt: i32| {
            *buf = if cut < cnt {
                std::mem::take(carry)
            } else {
                Vec::new()
            };
        };
        restore(&mut self.buf2, &mut self.carry2, i17, self.cnt2);
        restore(&mut self.buf8, &mut self.carry8, i19, self.cnt8);
        restore(&mut self.buf32, &mut self.carry32, i16, self.cnt32);
        self.cnt2 -= i17;
        self.cnt8 -= i19;
        self.cnt32 -= i16;
        self.state = 0;
        self.fb_blk = 0;
        self.bar_blk = 1;
        let total_n = (self.n + 1) as usize;
        for a in [&mut self.beats2, &mut self.beats8, &mut self.beats32] {
            let o = a[nb];
            for v in a.iter_mut() {
                *v -= o;
            }
            for i in 0..total_n - nb {
                a[i] = a[i + nb] + 1;
            }
        }
        self.block += 1;
        Some(())
    }

    /// The block's analyses (`Sub_Main_C` from `LowPeakAnalyze` to `HighPeakAnalyze32`).
    fn features(&mut self, nb: usize, done: usize, fb: i32, bar: i32) -> Option<()> {
        let (bpm, fs2, fs8, fs32) = (self.bpm, self.fs2, self.fs8, self.fs32);
        let o = &mut self.out;
        let lp = self
            .low_peak
            .analyze(&self.buf32, bpm, fs32, &self.beats32, nb, self.prev_peak);
        put(&mut o.low_peak, done, &lp.peaks);
        put(&mut o.low_peak_q, 4 * done, &lp.quarters);
        put(&mut o.low_flags, done, &lp.flags);
        self.prev_peak = o.low_peak[done - 1];
        let ll = self
            .low_level
            .analyze(&self.buf32, bpm, fs32, &self.beats32, nb);
        put(&mut o.low_level, done, &ll);
        let (q16, _) = self
            .low16
            .analyze(&self.buf32, bpm, fs32, &self.beats32, nb)?;
        let (q40, _) = self
            .low40
            .analyze(&self.buf32, bpm, fs32, &self.beats32, nb)?;
        put(&mut o.low_power, 4 * done, &q16);
        put(&mut o.low40, 4 * done, &q40);
        let ml = self.mid_level.analyze(&self.buf8, &self.beats8, nb);
        put(&mut o.mid_level, done, &ml);
        let mp = self.mid_peak.analyze(&self.buf8, &self.beats8, nb);
        put(&mut o.mid_peak, done, &mp);
        let al = feat::all_level(&self.buf8, &self.beats8, nb);
        put(&mut o.all_level, done, &al);
        let (ae, apk) = feat::all_power(&self.buf8, bpm, fs8, &self.beats8, nb)?;
        put(&mut o.all_power, 4 * done, &ae);
        put(&mut o.all_peak, 4 * done, &apk);
        let bars_done = ((f64::from(done as i32 - self.bar0) + 1.0) * 0.25) as u32 as i64;
        for (pat, which) in [
            (
                self.rhythm
                    .analyze(&self.buf8, bpm, fb, bar, fs8, self.block, &self.beats8, nb)?,
                1,
            ),
            (
                self.rhythm2.analyze(
                    &self.buf8,
                    bpm,
                    fb,
                    bar,
                    fs8,
                    self.block,
                    &self.beats8,
                    nb,
                )?,
                2,
            ),
        ] {
            let dst = if which == 1 {
                &mut o.pattern
            } else {
                &mut o.pattern2
            };
            let off = bars_done * 16 - pat.len() as i64;
            if off < 0 || bars_done < 1 {
                *dst = vec![0];
            } else {
                let off = off as usize;
                dst[off..off + pat.len()].copy_from_slice(&pat);
            }
        }
        let hh = feat::hihat(&self.buf2, bpm, fs2, &self.beats2, nb);
        put(&mut o.hihat, done, &hh);
        let (lines, count) = self.cqt.frames(&self.buf8, self.state == 2);
        if self.block == 1 {
            // `IntroEndFind_a` @0x100dafb90 rounds up (`fcvtps`); its ms argument is 0 here.
            o.cqt_first = (fs8
                * 0.00390625
                * 0.0f64.mul_add(0.001, (f64::from(bar - 1) * 60.0) / bpm))
                .ceil() as i32;
        }
        o.cqt_frames += count as i32;
        let end = o.cqt_frames as usize;
        for (dst, src) in o.cqt.iter_mut().zip(&lines) {
            put(dst, end, src);
        }
        let n_blk = nb as i32;
        let f2 = spec::bar_spectra(
            &self.fft2,
            &self.buf8,
            bpm,
            fs8,
            bar,
            &self.beats8,
            n_blk,
            &[0, 2],
        )?;
        let base2 =
            ((f64::from(done as i32 - self.bar0) + 1.0) * 0.25) as i32 * 2 - f2.len() as i32;
        for (i, line) in f2.into_iter().enumerate() {
            o.fft2[(base2 + i as i32) as usize] = line;
        }
        let f1 = spec::bar_spectra(
            &self.fft1,
            &self.buf8,
            bpm,
            fs8,
            bar,
            &self.beats8,
            n_blk,
            &[0, 1, 2, 3],
        )?;
        let d = done as i32 - self.bar0;
        let q = if d + 1 < 0 { d + 4 } else { d + 1 };
        let base1 = (q & !3) - f1.len() as i32;
        for (i, line) in f1.into_iter().enumerate() {
            o.fft1[(base1 + i as i32) as usize] = line;
        }
        let (blo, bhi) =
            self.band
                .analyze(&self.buf32, &self.buf8, &self.beats32, &self.beats8, nb);
        put(&mut o.band_lo, done, &blo);
        put(&mut o.band_hi, done, &bhi);
        let hp = self.high.analyze(&self.buf2, bpm, fs2, &self.beats2, nb)?;
        put(&mut o.high_peak, 8 * done, &hp);
        Some(())
    }

    /// `Set_Sub_Main_C_Out(n, bar)`.
    fn set_out(&mut self, bar: i32) {
        self.out.bar = bar;
        self.out.beats8 = self.beats8.clone();
        self.finished = true;
    }
}
