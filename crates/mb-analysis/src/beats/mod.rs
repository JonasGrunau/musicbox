//! The beat grid of rekordbox's Normal analysis mode: BALib v1 (`BeatAnalyzer_1_0`, the
//! constant-tempo Analyzer) driven by `AnalyzeJobBeatDetector::legacyBeatAnalyze` @0x102328684.
//! Spec: `analysis-beats.md`.
//!
//! `Analyzer::setSample` @0x1011364ec feeds the mono mix to the six-band onset sensor
//! (`sensor`) and the stereo shorts to two envelopes (`envelope`, `adjust`); at the end
//! `Analyzer::analyze` @0x101136890 picks the tempo and a first grid (`info`), lets the onset
//! envelope choose the downbeat (`envelope`), refines the tempo to 0.002 BPM against the
//! quarter-rate envelope and moves the grid onto its beat peak (`adjust`). The job converts the
//! beat positions to milliseconds (`convertToMicrosec` @0x102328b20).

// Float tests are written as the binary branches (`b.le`, `b.lt`, … are taken on NaN), so
// `!(a > b)` and `a <= b` are not interchangeable here.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

mod adjust;
pub(crate) mod envelope;
pub(crate) mod filters;
pub(crate) mod info;
pub(crate) mod sensor;

/// `peakss::BeatInfo` (24 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BeatInfo {
    /// Sample position.
    pub pos: i32,
    /// +0x04: beats since the previous peak (sensor), 0 in grids.
    pub beats: f32,
    /// +0x08: beat length in ms.
    pub tempo: f32,
    /// +0x0c: sensor call stamp, later the beat's position in the bar (1–4).
    pub stat: i32,
    /// +0x10: onset band (0–5) or −1.
    pub band: i32,
    /// +0x14: anchor flag.
    pub flag: u8,
}

/// One PQTZ beat (`analyzer::DetailedBeatInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beat {
    /// Position in the bar, 1–4.
    pub beat_in_bar: u16,
    /// BPM × 100 at this beat.
    pub tempo: u16,
    /// Time in ms (truncated).
    pub time_ms: u32,
    /// The µs remainder (`time_us − 1000·time_ms`).
    pub sub_ms: u16,
}

/// The analysed grid: `djmdContent.BPM` and the PQTZ beats.
#[derive(Debug, Clone, PartialEq)]
pub struct BeatGrid {
    /// BPM × 100 (`BA_GetBpm`).
    pub bpm: i32,
    pub beats: Vec<Beat>,
}

/// `Analyzer` for the beat side: blocks of `rate/75` frames into the three units.
pub struct BeatAnalyzer {
    rate: u32,
    total: i32,
    block: usize,
    seen: i64,
    mono: Vec<f32>,
    pcm: Vec<[i16; 2]>,
    sensor: sensor::BeatSensor,
    envelope: envelope::UnitEnvelope,
    adjust: adjust::UnitBeatAdjust,
}

impl BeatAnalyzer {
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
            adjust: adjust::UnitBeatAdjust::new(rate, total),
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
                self.adjust.put(&self.pcm);
                self.mono.clear();
                self.pcm.clear();
            }
        }
    }

    /// `Analyzer::analyze` + `legacyBeatAnalyze`.
    pub fn finish(mut self) -> Option<BeatGrid> {
        let rate = self.rate as i32;
        let mut raw = std::mem::take(&mut self.sensor.beats);
        let r = info::analyze(rate, &mut raw, self.total)?;
        let mut bpm = r.bpm;
        let mut list = r.beats;
        self.envelope.analyze(bpm, &mut list, false);
        self.adjust.analyze(&mut bpm, &mut list);
        if list.is_empty() {
            return None;
        }
        let bpm100 = bpm.mul_add(100.0, 0.5) as i32;
        let beats = list
            .iter()
            .map(|b| {
                let tempo = (6_000_000.0f32 / b.tempo + 0.5) as i32;
                let us = i64::from(b.pos) * 1_000_000 / i64::from(rate);
                let ms = us / 1000;
                Beat {
                    beat_in_bar: b.stat as u16,
                    tempo: tempo as u16,
                    time_ms: ms as u32,
                    sub_ms: (us - ms * 1000) as u16,
                }
            })
            .collect();
        Some(BeatGrid { bpm: bpm100, beats })
    }
}
