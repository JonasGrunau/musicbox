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
    /// 0 → analysis mode 3 (model 8 over a 60 s window, constant tempo).
    Normal,
    /// 1 → analysis mode 4 (model 7 over the whole track, tempo changes followed).
    Dynamic,
    /// Any other value → analysis mode 2 (BALib's tempo-change flag decides).
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
    load(&MODEL, Model::BeatNo7)
}

/// The model-8 network (the 60 s window without hendou), loaded once.
fn model8() -> Result<&'static Session, String> {
    static MODEL: OnceLock<Result<Session, String>> = OnceLock::new();
    load(&MODEL, Model::BeatNo8)
}

fn load(cell: &'static OnceLock<Result<Session, String>>, model: Model) -> Result<&'static Session, String> {
    cell.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        Session::open(model, threads).map_err(|e| e.to_string())
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
    Ok(to_grid(&f, rate, f.bpm100()))
}

/// `convertToMicrosec` @0x102328b20 without the BPM-range rebuild: `µs = pos·10⁶/rate`.
fn to_grid(f: &focus::Focus, rate: u32, bpm100: i32) -> Option<BeatGrid> {
    let beats: Vec<Beat> = f
        .beat_list()
        .into_iter()
        .map(|(pos, stat, tempo)| {
            let us = i64::from(pos) * 1_000_000 / i64::from(rate);
            let ms = us / 1000;
            Beat { beat_in_bar: stat as u16, tempo: tempo as u16, time_ms: ms as u32, sub_ms: (us - ms * 1000) as u16 }
        })
        .collect();
    (!beats.is_empty()).then_some(BeatGrid { bpm: bpm100, beats })
}

/// What the high-precision job hands to the thread.
#[derive(Debug, Clone)]
pub struct HpGrid {
    /// `None`: no beats (the job's status 4). The previous grid stays and the BPM becomes 0.
    pub grid: Option<BeatGrid>,
    /// PQT2 `analysis_type` (`notifyBeat`): 2 in Dynamic, and in Auto when BALib v2 found tempo
    /// changes; else 1.
    pub grid_type: u8,
    /// Whether the phrase pass may move the grid (the thread's urahaku flag). The job clears it
    /// once madmom returned beats.
    pub urahaku: bool,
}

/// `highPrecisionBeatAnalyze` in Normal and Auto mode. `balib` is BALib v2's constant analyser
/// fed with the track (its `UnitBeatAdjust` bands serve Focus's constant path), `mono` the
/// `(L + R)/2` samples `0 .. total` (`analysis-beats-hp.md` § 6).
/// - BALib v2's BPM, beat count and offset; hendou is off in Normal and BALib's verdict in Auto.
/// - madmom: model 8 over the 60 s window, or model 7 over the whole track with hendou.
/// - Focus: the constant path without hendou when madmom's list starts on a bar.
/// - `convertToMicrosec`; without hendou the BPM must lie in the 70–180 range (range 1, the
///   only one musicbox has), else `bpmToDouble`/`bpmToHalf` would rebuild the list. They are
///   not ported, so that case is an error.
pub fn grid_v2(mode: HpMode, mono: &[f32], rate: u32, total: u64, balib: crate::balib2::Analyzer) -> Result<HpGrid, String> {
    let none = |urahaku| HpGrid { grid: None, grid_type: 1, urahaku };
    let hendou_mode = match mode {
        HpMode::Normal => Some(false),
        HpMode::Auto => None,
        HpMode::Dynamic => return Err("grid_v2 serves Normal and Auto".into()),
    };
    let (r, mut adjust) = balib.finish_with_adjust();
    let Some(r) = r.filter(|r| !r.beats.is_empty()) else {
        return Ok(none(true));
    };
    let hendou = hendou_mode.unwrap_or(r.hendou);
    let model = if hendou { model7()? } else { model8()? };
    let total = total.min(mono.len() as u64);
    let list = madmom::analyze(model, mono, rate, total as usize, r.offset, hendou).map_err(|e| e.to_string())?;
    // `SetUrahaku(false)` once madmom's list has a beat.
    let urahaku = list.beats.is_empty();
    let inp = focus::FocusInput { rate: rate as i32, total: total as i32, varied: false, hendou, offset: r.offset };
    let Some(f) = focus::Focus::run_with(inp, r.beats.len() as i32, r.bpm100, &list.beats, list.bpm, false, Some(&mut adjust)) else {
        return Ok(none(urahaku));
    };
    if !f.unported.is_empty() {
        return Err(format!("Focus: not ported: {}", f.unported.join(", ")));
    }
    let mut bpm100 = f.bpm100();
    if !hendou {
        let first = f.beat_list().first().map_or(0, |b| b.2);
        let v = if bpm100 < 1 { first } else { bpm100 };
        if !(7000..=18000).contains(&v) {
            return Err(format!("BPM {v} outside 70–180: bpmToDouble/bpmToHalf not ported"));
        }
        bpm100 = v;
    }
    let grid_type = if hendou { 2 } else { 1 };
    Ok(HpGrid { grid: to_grid(&f, rate, bpm100), grid_type, urahaku })
}
