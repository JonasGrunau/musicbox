//! High-precision beat analysis (rekordbox's analysis modes 2–4: `highPrecisionBeatAnalyze`
//! @0x102327e50). Spec: `docs/re/analysis-beats-hp.md`.

pub mod hmm;
pub mod focus;
pub mod madmom;

use crate::beats::{Beat, BeatGrid};
use mb_nn::{Model, Session};
use std::sync::OnceLock;

/// The `trackHiPrecisionBeatAnalysisMode` preference, read when `HighPrecisionBeatAnalyze` is 1
/// (`analysis-beats-hp.md` § 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpMode {
    /// 0 → analysis mode 3 (model 8 over a 60 s window, constant tempo). Not ported.
    Normal,
    /// 1 → analysis mode 4 (model 7 over the whole track, tempo changes followed).
    Dynamic,
    /// Any other value → analysis mode 2 (BALib's tempo-change flag decides). Not ported.
    Auto,
}

impl HpMode {
    pub fn from_pref(value: i64) -> Self {
        match value {
            0 => Self::Normal,
            1 => Self::Dynamic,
            _ => Self::Auto,
        }
    }
}

/// The model-7 network, loaded once (`TFWrapper::Model`; rekordbox uses
/// `hardware_concurrency()` threads).
fn model7() -> Result<&'static Session, String> {
    static MODEL: OnceLock<Result<Session, String>> = OnceLock::new();
    MODEL
        .get_or_init(|| {
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
            Session::open(Model::BeatNo7, threads).map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// `highPrecisionBeatAnalyze` in Dynamic mode: madmom over the whole track (`mono` = the
/// `(L + R)/2` samples `0 .. total`), BALib's Focus pass seeded with the Normal grid's beat count
/// and BPM (only checked for zero in this mode), then `convertToMicrosec` without the BPM range
/// check (the job's hendou flag is set). `Ok(None)` when no beat is left; rekordbox then leaves
/// the previous grid in place (`analysis-beats-hp.md` § 6).
pub fn grid(mode: HpMode, mono: &[f32], rate: u32, total: u64, balib: &BeatGrid) -> Result<Option<BeatGrid>, String> {
    if mode != HpMode::Dynamic {
        return Err(format!("high-precision mode {mode:?} is not ported"));
    }
    let model = model7()?;
    let total = total.min(mono.len() as u64);
    let list = madmom::analyze(model, mono, rate, total as usize, 0, true).map_err(|e| e.to_string())?;
    let inp = focus::FocusInput { rate: rate as i32, total: total as i32, varied: true, hendou: true, offset: 0 };
    let Some(f) = focus::Focus::run(inp, balib.beats.len() as i32, balib.bpm, &list.beats, list.bpm, false) else {
        return Ok(None);
    };
    let beats: Vec<Beat> = f
        .beat_list()
        .into_iter()
        .map(|(pos, stat, tempo)| {
            let us = i64::from(pos) * 1_000_000 / i64::from(rate);
            let ms = us / 1000;
            Beat { beat_in_bar: stat as u16, tempo: tempo as u16, time_ms: ms as u32, sub_ms: (us - ms * 1000) as u16 }
        })
        .collect();
    if beats.is_empty() {
        return Ok(None);
    }
    Ok(Some(BeatGrid { bpm: f.bpm100(), beats }))
}
