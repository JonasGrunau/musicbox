//! BALib v2 (`BeatAnalyzer_2_0`), the beat analyser rekordbox's high-precision modes use
//! (`BALibWrapper::SetVersion(2)`, `analysis-beats-hp.md`). The onset sensor, the envelope units and
//! the filters are instruction-identical to v1 and run as v1's code (`crate::beats`); the beat
//! information analysis and `UnitBeatAdjust` are v2's own.

use crate::beats::BeatInfo;
use crate::beats::{envelope, info, sensor};

pub mod lowband;
pub mod verdict;

/// The constant-tempo `Analyzer` (`BA_CreateConst44k`/`48k`): blocks of `rate/75` frames into the
/// sensor, the envelope and `UnitBeatAdjust`'s low band (`Analyzer::setSample` @0x10116bae8).
pub struct Analyzer {
    rate: u32,
    total: i32,
    block: usize,
    seen: i64,
    mono: Vec<f32>,
    pcm: Vec<[i16; 2]>,
    sensor: sensor::BeatSensor,
    envelope: envelope::UnitEnvelope,
    low: lowband::LowBand,
}

/// One analysis pass's result for the development comparisons: BPM and beat list.
pub type Pass = Option<(f32, Vec<BeatInfo>)>;

/// What BALib reports after `Analyzer::analyze` @0x10116be98.
#[derive(Debug, Clone, PartialEq)]
pub struct Result {
    /// `BA_GetBpm`: BPM × 100.
    pub bpm100: i32,
    /// `BA_GetBeatList`: (position, beat in bar, BPM × 100) per beat.
    pub beats: Vec<(i32, i32, i32)>,
    /// `BA_GetOffsetCalc`: where the low end comes in, in samples.
    pub offset: i32,
    /// `BA_GetBpmHendou`: the verdict.
    pub hendou: bool,
}

impl Analyzer {
    /// `total` = samples fed (`L − 1`).
    pub fn new(rate: u32, total: u64) -> Self {
        let total = total.min(i32::MAX as u64) as i32;
        Self {
            rate,
            total,
            block: (rate / 75) as usize,
            seen: 0,
            mono: Vec::new(),
            pcm: Vec::new(),
            sensor: sensor::BeatSensor::new(rate, total),
            envelope: envelope::UnitEnvelope::new(rate, total),
            low: lowband::LowBand::new(rate, total),
        }
    }

    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        for &[l, r] in pcm {
            if self.seen >= i64::from(self.total) {
                return;
            }
            self.mono
                .push((f32::from(r) + f32::from(l)) * 1.525_878_9e-5);
            self.pcm.push([l, r]);
            self.seen += 1;
            if self.mono.len() >= self.block || self.seen >= i64::from(self.total) {
                self.sensor.put(&self.mono);
                self.envelope.put(&self.pcm);
                self.low.put(&self.pcm);
                self.mono.clear();
                self.pcm.clear();
            }
        }
    }

    /// The stages for development comparisons: the sensor's list, then the beat information
    /// analysis (BPM, list) before the envelope, constant then varied, and the verdict. The
    /// constant pass's BPM is 60000 / its first beat's length (0x10119bd94), the varied pass's
    /// its longest segment's (0x10119dc14); the verdict starts from these.
    #[doc(hidden)]
    pub fn stages(mut self) -> (Vec<BeatInfo>, Pass, Pass, Option<verdict::Verdict>) {
        let raw = std::mem::take(&mut self.sensor.beats);
        let c =
            info::analyze(self.rate as i32, &mut raw.clone(), self.total).map(|r| (r.bpm, r.beats));
        let v = info::analyze_varied(self.rate as i32, &mut raw.clone(), self.total)
            .map(|r| (r.rep, r.beats));
        let verdict = self.verdict(&raw, &c, &v);
        (raw, c, v, verdict)
    }

    /// Test A on the sensor's list, then the verdict on the varied list.
    fn verdict(&self, raw: &[BeatInfo], c: &Pass, v: &Pass) -> Option<verdict::Verdict> {
        let rate = self.rate as i32;
        let cb = c.as_ref().map_or(0.0, |c| c.0);
        let a = !raw.is_empty() && lowband::judge(rate, raw, cb, self.total, &self.low);
        let (vb, vl) = v.as_ref().map_or((0.0, &[][..]), |v| (v.0, &v.1[..]));
        verdict::run(rate, cb, vb, vl, a)
    }

    /// `Analyzer::analyze`: the beat information analysis (both passes and the verdict between
    /// them), then the envelope's downbeat choice.
    pub fn finish(mut self) -> Option<Result> {
        let raw = std::mem::take(&mut self.sensor.beats);
        let c =
            info::analyze(self.rate as i32, &mut raw.clone(), self.total).map(|r| (r.bpm, r.beats));
        let v = info::analyze_varied(self.rate as i32, &mut raw.clone(), self.total)
            .map(|r| (r.rep, r.beats));
        let verdict = self.verdict(&raw, &c, &v)?;
        let (bpm, mut list) = if verdict.hendou() {
            (verdict.varied_bpm, v?.1)
        } else {
            (verdict.const_bpm, c?.1)
        };
        self.envelope.analyze(bpm, &mut list, false);
        let bpm100 = bpm.mul_add(100.0, 0.5) as i32;
        let positions: Vec<i32> = list.iter().map(|b| b.pos).collect();
        let offset = self.low.offset(self.rate, &positions, bpm);
        let beats = list
            .iter()
            .map(|b| (b.pos, b.stat, (6_000_000.0f32 / b.tempo + 0.5) as i32))
            .collect();
        Some(Result {
            bpm100,
            beats,
            offset,
            hendou: verdict.hendou(),
        })
    }
}
