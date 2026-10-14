//! BALib v2 (`BeatAnalyzer_2_0`), the beat analyser rekordbox's high-precision modes use
//! (`BALibWrapper::SetVersion(2)`, `analysis-beats-hp.md`). The onset sensor, the envelope units and
//! the filters are instruction-identical to v1 and run as v1's code (`crate::beats`); the beat
//! information analysis and `UnitBeatAdjust` are v2's own.

use crate::beats::BeatInfo;
use crate::beats::{envelope, info, sensor};

/// The constant-tempo `Analyzer` (`BA_CreateConst44k`/`48k`): blocks of `rate/75` frames into the
/// sensor and the envelope (`Analyzer::setSample` @0x10116bae8).
pub struct Analyzer {
    rate: u32,
    total: i32,
    block: usize,
    seen: i64,
    mono: Vec<f32>,
    pcm: Vec<[i16; 2]>,
    sensor: sensor::BeatSensor,
    envelope: envelope::UnitEnvelope,
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
        }
    }

    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        for &[l, r] in pcm {
            if self.seen >= i64::from(self.total) {
                return;
            }
            self.mono.push((f32::from(r) + f32::from(l)) * 1.525_878_9e-5);
            self.pcm.push([l, r]);
            self.seen += 1;
            if self.mono.len() >= self.block || self.seen >= i64::from(self.total) {
                self.sensor.put(&self.mono);
                self.envelope.put(&self.pcm);
                self.mono.clear();
                self.pcm.clear();
            }
        }
    }

    /// The stages for development comparisons: the sensor's list, then the beat information
    /// analysis (BPM, list) before the envelope.
    #[doc(hidden)]
    pub fn stages(mut self) -> (Vec<BeatInfo>, Pass, Pass) {
        let raw = std::mem::take(&mut self.sensor.beats);
        let mut work = raw.clone();
        let c = info::analyze(self.rate as i32, &mut work, self.total).map(|r| (r.rep, r.beats));
        let mut work = raw.clone();
        let v = info::analyze_varied(self.rate as i32, &mut work, self.total).map(|r| (r.rep, r.beats));
        (raw, c, v)
    }

    /// `Analyzer::analyze`: the beat information analysis, then the envelope's downbeat choice.
    /// *Stage:* v1's constant-path analysis stands in for v2's `BeatInfoAnalyzer::analyze`.
    pub fn finish(mut self) -> Option<Result> {
        let mut raw = std::mem::take(&mut self.sensor.beats);
        let r = info::analyze(self.rate as i32, &mut raw, self.total)?;
        let mut list: Vec<BeatInfo> = r.beats;
        self.envelope.analyze(r.bpm, &mut list, false);
        // v2 returns the representative BPM (the longest segment's, `fdiv` at 0x10119a110), where
        // v1's constant path returns 60000 / the first beat's length.
        let bpm100 = r.rep.mul_add(100.0, 0.5) as i32;
        let beats = list.iter().map(|b| (b.pos, b.stat, (6_000_000.0f32 / b.tempo + 0.5) as i32)).collect();
        Some(Result { bpm100, beats })
    }
}
