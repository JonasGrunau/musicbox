//! rekordbox's phrase analysis, `analyzer::SongStructureAnalyze`, as run by
//! `CBeatAnalyzeThread::analyze` @0x1001528c4 after the beat job. Spec: `analysis-phrase.md`.
//!
//! Stage A (`Sub_Main_A` @0x101c28728) mixes the track to mono and decimates the first 210 s
//! by 8. Stage B (`Sub_Main_B` @0x100b80b04) finds the low-band rhythm pattern and the snare
//! onsets, corrects the first downbeat (`BeatposCorrection2016`) and the bar it starts
//! (`BarStartCorrection`); the thread then moves the beat grid onto that downbeat.

// Float tests are written as the binary branches, so `!(a > b)` and `a <= b` differ here.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::nonminimal_bool)]
// Ported loops index several parallel arrays the way the binary does.
#![allow(clippy::needless_range_loop)]

mod adjust;
mod beatpos;
mod blocks;
mod d_feat;
mod feat;
mod filter;
mod hp;
mod intro;
mod spec;
mod genre;
mod mat;
mod melo;
mod power;
#[cfg(test)]
mod replay;
mod rhythm;
mod snare;
pub mod stage_c;
pub mod stage_d;
mod verse;

use crate::beats::BeatGrid;
pub use adjust::PhraseResult;

/// `CBeatAnalyzeThread::analyze()::convFirstBar` (0x10387967c).
const CONV_FIRST_BAR: [i32; 4] = [1, 4, 3, 2];

/// The mono mix `Analyze_AB` builds from the int16 stream: `0.99·L + 0.01·R`, scaled by
/// 1/32768 (0x3effae147ae147ae, 0x3e947ae147ae147b), as one fused multiply-add.
pub(crate) const GAIN_L: f64 = f64::from_bits(0x3eff_ae14_7ae1_47ae);
pub(crate) const GAIN_R: f64 = f64::from_bits(0x3e94_7ae1_47ae_147b);

/// Stage A: collects the decimated mono signal while the track is decoded.
pub struct PhraseAnalyzer {
    rate: u32,
    total: i64,
    /// +0xbc: samples per `Sub_Main_A` call (`rate · 128/75`, rounded).
    chunk: usize,
    fed: i64,
    buf: Vec<f64>,
    res: filter::Decimator,
    /// +0x1e0.
    dec: Vec<f64>,
    /// +0x1d4: `min(210 s, total / 8)` decimated samples.
    target: usize,
    done: bool,
}

/// What stage B decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Downbeat {
    /// SS+0x200: the first downbeat (ms).
    pub fb_ms: i32,
    /// SS+0x204: its bar number in `convFirstBar` terms (1–4).
    pub bar: i32,
}

impl PhraseAnalyzer {
    /// `Initialize` + `Sub_Main_A_init` for a track of `total` analysed samples (`L − 1`).
    pub fn new(rate: u32, total: u64) -> Self {
        let fs8 = rate as f64 * 0.125;
        let total = total.min(i32::MAX as u64) as i64;
        let target = ((fs8 * 210.0) as i64).min(total / 8).max(0) as usize;
        Self {
            rate,
            total,
            chunk: (rate as f64 * 1.7066666666666668 + 0.5) as usize,
            fed: 0,
            buf: Vec::new(),
            res: filter::Decimator::low(fs8 * 0.4, rate as f64, 8),
            dec: Vec::new(),
            target,
            done: false,
        }
    }

    /// `Analyze_AB`'s stage 1 on the next frames.
    pub fn put(&mut self, pcm: &[[i16; 2]]) {
        if self.done {
            return;
        }
        for &[l, r] in pcm {
            if self.fed >= self.total {
                break;
            }
            let l = f64::from(l);
            self.buf.push(f64::from(r).mul_add(GAIN_R, l * GAIN_L));
            self.fed += 1;
            if self.buf.len() >= self.chunk {
                self.step();
                if self.done {
                    return;
                }
            }
        }
        if self.fed >= self.total {
            while !self.done {
                self.step();
            }
        }
    }

    /// One `Sub_Main_A` call on `min(buffered, chunk)` samples.
    fn step(&mut self) {
        let n = self.buf.len().min(self.chunk);
        let chunk: Vec<f64> = self.buf.drain(..n).collect();
        let d = self.res.downsample(&chunk);
        self.dec.extend_from_slice(&d);
        let consumed = self.fed - self.buf.len() as i64;
        if self.target <= self.dec.len() || self.total <= consumed {
            self.dec.resize(self.target, 0.0);
            self.buf = Vec::new();
            self.done = true;
        }
    }

    /// Stage B for the grid's `(bpm, fb, fbar)`; `None` where `Sub_Main_B` fails. Only the
    /// parts the downbeat depends on.
    pub fn downbeat(&self, bpm100: i32, fb_ms: i32, fbar: i32) -> Option<Downbeat> {
        self.sub_main_b(bpm100, fb_ms, fbar, false, true).map(|b| b.down)
    }

    /// `Sub_Main_B` (@0x100b80b04) with the outputs stage D reads. `urahaku` is the thread's
    /// grid-shift flag (+0x3908): without it `Initialize` sets +0xc2/+0xc3 and B keeps `fb` and
    /// `fbar` (no first-beat or bar correction).
    pub fn stage_b(&self, bpm100: i32, fb_ms: i32, fbar: i32, urahaku: bool) -> Option<BOut> {
        self.sub_main_b(bpm100, fb_ms, fbar, true, urahaku)
    }

    fn sub_main_b(
        &self,
        bpm100: i32,
        fb_ms: i32,
        fbar: i32,
        for_d: bool,
        corrections: bool,
    ) -> Option<BOut> {
        if !self.done {
            return None;
        }
        let fs = self.rate as f64 * 0.125;
        let bpm = bpm100 as f64 * 0.01;
        let n = ((fs * 210.0) as usize).min(self.dec.len());
        let x = &self.dec[..n];
        let beats = rhythm::beats_samples(n, fs, bpm, fb_ms);
        let pattern = rhythm::RhythmPattern::new(fs).analyze(x, bpm, fb_ms, fs)?;
        let k4 = if for_d {
            let (_, bar_energy) =
                power::LowPower16::new(fs).analyze(&self.dec, bpm, fs, &beats, beats.len())?;
            genre::four_kicks(&pattern, bpm, &bar_energy).0
        } else {
            0
        };
        let env = snare::envelope(x, fs);
        let (snares, bars) = snare::detect(&env, fs, bpm, fb_ms, &beats)?;
        let genre_snares = if for_d {
            snare::detect_for_genre(&bars, fs, bpm, fb_ms, &beats, k4, &pattern)
        } else {
            snare::GenreSnares::default()
        };
        let bar_snares = snare::detect_for_bar(&bars, fs, bpm, fb_ms, &beats, &pattern);
        let bp =
            beatpos::beatpos_correction(x, fs, bpm, fb_ms, &pattern, beats.len() as i32, &snares)?;
        // +0xc2 is clear when the thread's grid-shift flag (+0x3908) is set: the corrected
        // first beat replaces `fb`.
        let (fb2, beats2) = if corrections {
            let beats2 = rhythm::beats_samples(n, fs, bpm, bp.fb_ms);
            rhythm::RhythmPattern::new(fs).analyze(x, bpm, bp.fb_ms, fs)?;
            (bp.fb_ms, beats2)
        } else {
            (fb_ms, beats)
        };
        let bar_pos = if for_d {
            let n30 = ((fs * 30.0) as usize).min(self.dec.len());
            beatpos::bar_position(&self.dec[..n30], bpm, fb2, fs)
        } else {
            0
        };
        // +0xc3 (set without the grid-shift flag) skips the bar correction too.
        let bar = if !corrections || bar_snares.count < 1 || bp.steady {
            fbar
        } else {
            beatpos::bar_start_correction(
                x,
                fs,
                bpm,
                fb_ms,
                fb2,
                &beats2,
                &bar_snares,
                fbar,
                bp.moved,
            )?
        };
        Some(BOut {
            down: Downbeat { fb_ms: fb2, bar },
            kick6: bp.kick6,
            snares: genre_snares,
            bar_pos,
        })
    }
}

/// `Sub_Main_B`'s outputs (+0x200 … +0x258).
#[derive(Debug, Clone)]
pub struct BOut {
    pub down: Downbeat,
    /// +0x208: `BeatposCorrection2016`'s kick-on-6 pattern flag.
    pub kick6: bool,
    /// +0x210 / +0x250 / +0x254: `DetectSnarePositionsForGenre`.
    pub snares: snare::GenreSnares,
    /// +0x258: `BarPositionJudge`.
    pub bar_pos: i32,
}

/// The thread's inputs to `Initialize`: BPM × 100, first beat (ms) and its bar, adjusted when
/// beat 0's tempo differs from the BPM (decompile ~1897–1938).
pub fn grid_anchor(grid: &BeatGrid) -> Option<(i32, i64, i32)> {
    let b0 = grid.beats.first()?;
    let bpm100 = grid.bpm;
    let mut fb = i64::from(b0.time_ms);
    let mut fbar = match b0.beat_in_bar {
        1..=4 => CONV_FIRST_BAR[b0.beat_in_bar as usize - 1],
        _ => 1,
    };
    if grid.beats.len() >= 8 && bpm100 != i32::from(b0.tempo) {
        let beat = 6_000_000.0 / bpm100 as f64;
        let prev = if fbar - 1 != 0 { fbar - 1 } else { 4 };
        if beat < fb as f64 && (fb as f64) < beat + beat {
            fbar = prev;
            fb -= beat as i64;
        }
    }
    Some((bpm100, fb, fbar))
}

/// Whether the thread runs `SongStructureAnalyze` for this grid (decompile ~2034–2052).
pub fn runs(rate: u32, total: u64, bpm100: i32, fb: i64, fbar: i32) -> bool {
    if rate == 0 || total == 0 || bpm100 <= 0 || fb < 0 || fbar <= 0 {
        return false;
    }
    let r = rate as f64;
    let after = (bpm100 as i64 as f64
        * 0.00016666666666666666
        * ((total as f64 - 32.0) + r * -0.001 * fb as f64))
        / r;
    (after as i32) > 7 && fb as f64 <= 6_000_000.0 / bpm100 as f64 && total / u64::from(rate) < 7201
}

/// Applies stage B's downbeat to the grid as the thread does (decompile ~2103–2140).
pub fn apply(grid: &mut BeatGrid, fb: i64, fbar: i32, down: Option<Downbeat>) {
    if grid.beats.is_empty() {
        return;
    }
    let (target, same_ms, renumber, mut bar) = match down {
        None => {
            if fb == -1 {
                return;
            }
            (-1i64, false, false, fbar)
        }
        Some(d) => {
            let t = i64::from(d.fb_ms);
            let bi = d.bar - 1;
            if fbar == d.bar || !(0..=3).contains(&bi) {
                if fb == t {
                    return;
                }
                (t, false, false, fbar)
            } else {
                (t, fb == t, true, CONV_FIRST_BAR[bi as usize])
            }
        }
    };
    bar -= 1;
    for b in &mut grid.beats {
        if !same_ms {
            b.time_ms = (i64::from(b.time_ms) + (target - fb)) as u32;
        }
        if renumber {
            b.beat_in_bar = (mat::crem(bar, 4) + 1) as u16;
        }
        bar += 1;
    }
}

/// The thread's phrase step for one analysed grid: anchor, preconditions, stage B, shift. With
/// `path`, stages C and D follow on a second read of the file (`Analyze_CD`); the result is
/// returned when rekordbox would store it as PSSI ([`PhraseResult::stored`]).
///
/// `urahaku` is the thread's grid-shift flag (+0x3908, `AnalyzeJobManager::SetUrahaku`
/// @0x1022fb028): set when beats are analysed outside the Dynamic modes. Without it the grid
/// is not shifted and stage B keeps the anchor.
pub fn run(
    p: &PhraseAnalyzer,
    grid: &mut BeatGrid,
    rate: u32,
    total: u64,
    path: Option<&std::path::Path>,
    urahaku: bool,
) -> Option<PhraseResult> {
    let (bpm100, fb, fbar) = grid_anchor(grid)?;
    if !runs(rate, total, bpm100, fb, fbar) {
        return None;
    }
    if urahaku {
        let down = p.downbeat(bpm100, fb as i32, fbar);
        apply(grid, fb, fbar, down);
        down?;
    }
    let b = p.stage_b(bpm100, fb as i32, fbar, urahaku)?;
    let c = stage_c(path?, bpm100, b.down)?;
    let d = stage_d::DIn {
        bpm: bpm100 as f64 * 0.01,
        rate: rate as f64,
        fs8: rate as f64 * 0.125,
        // `Initialize`'s key argument: 0 as far as observed (PSSI byte 0x1d is 1 on every
        // oracle track, `analysis-phrase.md` § 8.1).
        key: 0,
        total: total as i32,
        fb0: fb as i32,
        fb: b.down.fb_ms,
        bar: b.down.bar,
        kick6: b.kick6,
        snares: &b.snares,
        bar_pos: b.bar_pos,
    };
    stage_d::sub_main_d(&c, &d).filter(PhraseResult::stored)
}

/// The phrase analysis of the file at `path` for an existing grid (e.g. one rekordbox
/// stored): stage A on a first read, then [`run`] on a copy of the grid.
pub fn analyze_with_grid(path: &std::path::Path, grid: &BeatGrid, urahaku: bool) -> Option<PhraseResult> {
    let mut dec = mb_audio::open(path).ok()?;
    let info = *dec.info();
    if info.total_samples < 2 {
        return None;
    }
    let total = info.total_samples - 1;
    let mut p = PhraseAnalyzer::new(info.sample_rate, total);
    let block = 2 * info.sample_rate as usize;
    let mut frames = vec![[0.0f32; 2]; block];
    let mut pcm = vec![[0i16; 2]; block];
    let mut pos = 0u64;
    while pos < total {
        let n = ((total - pos) as usize).min(block);
        let got = dec.read_frames(&mut frames[..n]);
        if got == 0 {
            break;
        }
        for (dst, src) in pcm[..got].iter_mut().zip(&frames[..got]) {
            *dst = [
                mb_audio::analysis_i16(src[0]),
                mb_audio::analysis_i16(src[1]),
            ];
        }
        p.put(&pcm[..got]);
        pos += got as u64;
    }
    run(&p, &mut grid.clone(), info.sample_rate, total, Some(path), urahaku)
}

/// Stage C on a second read of the track (as the thread re-reads it for `Analyze_CD`).
pub fn stage_c(path: &std::path::Path, bpm100: i32, down: Downbeat) -> Option<stage_c::COut> {
    let mut dec = mb_audio::open(path).ok()?;
    let info = *dec.info();
    let len = info.total_samples;
    if len < 2 {
        return None;
    }
    let total = len - 1;
    let mut c = stage_c::StageC::new(info.sample_rate, total, bpm100, down.fb_ms, down.bar)?;
    if !c.enough_beats() {
        return None;
    }
    let block = 2 * info.sample_rate as usize;
    let mut frames = vec![[0.0f32; 2]; block];
    let mut pcm = vec![[0i16; 2]; block];
    let mut pos = 0u64;
    while pos < total && !c.finished && !c.failed {
        let n = ((total - pos) as usize).min(block);
        let got = dec.read_frames(&mut frames[..n]);
        if got == 0 {
            break;
        }
        for (dst, src) in pcm[..got].iter_mut().zip(&frames[..got]) {
            *dst = [
                mb_audio::analysis_i16(src[0]),
                mb_audio::analysis_i16(src[1]),
            ];
        }
        c.put(&pcm[..got]);
        pos += got as u64;
    }
    c.take()
}
