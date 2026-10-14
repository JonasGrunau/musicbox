//! CUE analysis (`docs/re/analysis-cue.md`): after an analysis, rekordbox can set hot cues or
//! memory cues on the new beat grid (`DatabaseMediator::createDetectCue` @0x101495900).
//! - Manual (§ 3, `CueManager::ManualCueCreation` @0x100056e94): at 1.1 bars, then every N bars.
//! - Auto (§ 4, `CueManager::IntelligentCueCreation` @0x100057c7c): at 1.1 bars, then at the
//!   best-scored phrase starts (`mb_analysis::cue`). The personal CUE tendency is not modelled.

use crate::cues::{Cue, CueSlot, NewCue};
use mb_anlz::tags::Beat;

/// The CUE analysis preferences as `DatabaseMediator::getDetectCueParam` @0x1014997a4 reads them
/// (Preferences > Analysis > CUE Analysis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CueAnalysisSettings {
    /// `DetectCue` (default 0): "Set CUE during analysis".
    pub enabled: bool,
    /// `AutoDetectCue` ≠ 0: Manual. 0 (the default) is Auto.
    pub manual: bool,
    /// `AutoDetectCueType` ≠ 0: memory cues instead of hot cues.
    pub memory: bool,
    /// `MaxHotCueNumber` (default 4): the upper limit for hot cues.
    pub max_hot: i32,
    /// `MaxMemoryCueNumber` (default 4): the upper limit for memory cues.
    pub max_memory: i32,
    /// `FirstCue` (default 1): a cue at 1.1 bars.
    pub first_cue: bool,
    /// `LoopCue` (default 0): repeat every [`CueAnalysisSettings::loop_bars`] bars.
    pub loop_cue: bool,
    /// `LoopCueCount` (default 8).
    pub loop_bars: i32,
    /// `OverwriteCue` (default 0): first remove earlier cues of the type.
    pub overwrite: bool,
    /// `WriteComment` (default 1): "1.1Bars", "1.1Bars+8", … as the cue comment.
    pub comment: bool,
}

impl Default for CueAnalysisSettings {
    /// rekordbox's defaults (0x1037bbebe…0x1037bbed9).
    fn default() -> Self {
        Self {
            enabled: false,
            manual: false,
            memory: false,
            max_hot: 4,
            max_memory: 4,
            first_cue: true,
            loop_cue: false,
            loop_bars: 8,
            overwrite: false,
            comment: true,
        }
    }
}

impl CueAnalysisSettings {
    /// From the named preferences of `rekordbox3.settings`, rekordbox's default where one is
    /// missing or unreadable. rekordbox also forces Manual without repeat when the plan lacks the
    /// cue functions (`execAnalyze` @0x1014900ec); musicbox has no plan check and follows the
    /// stored values.
    pub fn from_prefs(pref: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let int = |name: &str, default: i32| pref(name).and_then(|v| v.trim().parse().ok()).unwrap_or(default);
        let flag = |name: &str, default: bool| int(name, i32::from(default)) != 0;
        Self {
            enabled: flag("DetectCue", d.enabled),
            manual: flag("AutoDetectCue", d.manual),
            memory: flag("AutoDetectCueType", d.memory),
            max_hot: int("MaxHotCueNumber", d.max_hot),
            max_memory: int("MaxMemoryCueNumber", d.max_memory),
            first_cue: flag("FirstCue", d.first_cue),
            loop_cue: flag("LoopCue", d.loop_cue),
            loop_bars: int("LoopCueCount", d.loop_bars),
            overwrite: flag("OverwriteCue", d.overwrite),
            comment: flag("WriteComment", d.comment),
        }
    }

    /// Cue creation will run, Manual or Auto.
    pub fn runs(&self) -> bool {
        self.enabled
    }

    /// Auto cue creation will run: the analysis must score the phrase starts.
    pub fn auto(&self) -> bool {
        self.enabled && !self.manual
    }

    /// The upper limit for the selected cue type.
    fn limit(&self) -> i32 {
        if self.memory { self.max_memory } else { self.max_hot }
    }
}

/// The personal CUE tendency in `settings_root` (`cue_personal_trend.json`, the folder of
/// `rekordbox3.settings`), read only. `Ok(None)` when there is no file, as before rekordbox's
/// first analysis job writes its all-zero one.
pub fn read_trend(settings_root: &std::path::Path) -> Result<Option<mb_analysis::cue::Trend>, String> {
    match std::fs::read_to_string(settings_root.join("cue_personal_trend.json")) {
        Ok(text) => mb_analysis::cue::Trend::parse(&text).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// What a cue pass does to one track: earlier cues removed (overwrite), then new cues in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CuePlan {
    /// `djmdCue.ID`s, in removal order.
    pub remove: Vec<String>,
    pub create: Vec<NewCue>,
}

/// One entry of the CueManager's beat list: bar number, beat in bar, time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BarBeat {
    bar: i32,
    beat: u16,
    ms: u32,
}

/// `AnalyzeFirstBeat` @0x100058308: the 1-based index of the first downbeat from the first
/// beat's position in its bar (table at 0x10374e31c).
fn first_downbeat(beats: &[Beat]) -> u32 {
    match beats.first().map(|b| b.beat_in_bar) {
        Some(2) => 4,
        Some(3) => 3,
        Some(4) => 2,
        _ => 1,
    }
}

/// `recalcBeat` @0x100058398: bars and beats renumbered from the first downbeat `first` (1-based);
/// the bars before it count from −1, the first downbeat is bar 1 (0 is skipped).
fn bar_beats(beats: &[Beat], first: u32) -> Vec<BarBeat> {
    // `convFirstBar` @0x10374e320
    const CONV: [u16; 4] = [1, 4, 3, 2];
    let (mut bar, mut beat) = if first < 2 { (1i32, 1u16) } else { (!((first - 2) >> 2) as i32, CONV[((first - 1) & 3) as usize]) };
    beats
        .iter()
        .map(|b| {
            let out = BarBeat { bar, beat, ms: b.time_ms };
            let next = if beat != 4 { beat + 1 } else { 1 };
            if next == 1 {
                bar = if bar == -1 { 1 } else { bar + 1 };
            }
            beat = next;
            out
        })
        .collect()
}

/// `createComment` @0x100058db0 in Manual mode: "1.1Bars", then "+%d" with bar − 1 from bar 2
/// (the comparison is unsigned).
fn manual_comment(bar: i32) -> String {
    if (bar as u32) < 2 { "1.1Bars".to_owned() } else { format!("1.1Bars+{}", bar.wrapping_sub(1)) }
}

/// `findSame` @0x100058994: a memory cue at the same time, or within 5 ms (unsigned bounds).
fn same_time(ms: u32, cue_ms: u32) -> bool {
    cue_ms == ms || (ms.wrapping_sub(5) <= cue_ms && cue_ms <= ms.wrapping_add(5))
}

/// The hot-cue pad order of the CueManager (`Kind` 1, 2, 3, 5, …, 17 in its constructor
/// @0x100055f80): pads A–P.
const HOT_PADS: u8 = 16;
/// `findEmpty` @0x100058c34: no new memory cue once the track has this many.
const MAX_MEMORY_CUES: usize = 10;

/// `ManualCueCreation` on the analysed grid. `beat_num_offset` is PQT2's field of that name
/// (`MusicAnalyzeInfo`+0x50, read by `get_pmai_ext_beat_num_offset`): 0 lets the first beat's
/// position in its bar decide where 1.1 is. `existing` are the track's live cues.
pub fn manual_plan(beats: &[Beat], beat_num_offset: u32, existing: &[Cue], s: &CueAnalysisSettings) -> CuePlan {
    // `createDetectCue` makes no cues without beats; `ManualCueCreation` returns 1 when neither
    // the 1.1-bar cue nor the repeat is on.
    if beats.is_empty() || !(s.first_cue || s.loop_cue) {
        return CuePlan::default();
    }
    let first = first_position(beats, beat_num_offset);
    let list = bar_beats(beats, first);
    let limit = s.limit();
    if limit < 1 {
        return CuePlan::default();
    }
    let n = if s.loop_cue { limit as usize } else { 1 };
    // `getDetectCueParam`: the step is `LoopCueCount · 4` beats in Manual mode.
    let step = s.loop_bars.wrapping_mul(4);
    let mut at = first as i32 - 1;
    if !s.first_cue {
        at = at.wrapping_add(step);
    }
    let mut picked = Vec::new();
    while picked.len() < n && at >= 0 && (at as usize) < list.len() {
        picked.push(list[at as usize]);
        at = at.wrapping_add(step);
    }
    let mut m = Manager::new(existing, s);
    // Overwrite: `removeCues(type, k)` @0x1000573b8, k = the cues about to be made.
    if s.overwrite {
        m.remove(picked.len().min(n));
    }
    // The memory-cue duplicate check sees the cues before this pass only.
    let before = m.memory_times();
    for b in picked {
        m.create(b.ms, &before, if s.comment { manual_comment(b.bar) } else { String::new() });
    }
    m.plan
}

/// `IntelligentCueCreation` on the analysed grid: `candidates` are the scored phrase starts,
/// best first (`mb_analysis::Analysis::cue_candidates`); without any no cue is made, not even the
/// 1.1-bar one. The 1.1-bar cue comes first and drops a candidate at the same time; the best
/// of the rest up to the limit follow in time order.
pub fn auto_plan(beats: &[Beat], beat_num_offset: u32, candidates: &[Beat], existing: &[Cue], s: &CueAnalysisSettings) -> CuePlan {
    let mut limit = s.limit();
    if beats.is_empty() || candidates.is_empty() || limit < 1 {
        return CuePlan::default();
    }
    let first = first_position(beats, beat_num_offset);
    let list = bar_beats(beats, first);
    let mut m = Manager::new(existing, s);
    // `removeCues(type, min(limit, candidates))`
    if s.overwrite {
        m.remove((limit as usize).min(candidates.len()));
    }
    let comment = || if s.comment { "CUE(Auto)".to_owned() } else { String::new() };
    let mut rest: Vec<u32> = candidates.iter().map(|b| b.time_ms).collect();
    if s.first_cue {
        // the first downbeat; an index past the grid gives a zero beat
        let ms = list.get(first as usize - 1).map_or(0, |b| b.ms);
        if let Some(i) = rest.iter().position(|&c| c == ms) {
            rest.remove(i);
        }
        let before = m.memory_times();
        m.create(ms, &before, comment());
        limit -= 1;
    }
    // the cue list as it stands after the 1.1-bar cue
    let before = m.memory_times();
    rest.truncate(limit.max(0) as usize);
    // `HotCueOrder` 0: by time (`$_5` compares the times unsigned)
    rest.sort_unstable();
    for ms in rest {
        m.create(ms, &before, comment());
    }
    m.plan
}

/// The 1-based index of the first downbeat: PQT2's `beat_num_offset`, or from the first beat.
fn first_position(beats: &[Beat], beat_num_offset: u32) -> u32 {
    if beat_num_offset == 0 { first_downbeat(beats) } else { beat_num_offset }
}

/// The CueManager's view of the track's cues of the selected type during one pass.
struct Manager<'a> {
    s: &'a CueAnalysisSettings,
    kept: Vec<&'a Cue>,
    used_pads: Vec<u8>,
    memory: Vec<u32>,
    plan: CuePlan,
}

impl<'a> Manager<'a> {
    fn new(existing: &'a [Cue], s: &'a CueAnalysisSettings) -> Self {
        let mut kept: Vec<&Cue> = existing.iter().filter(|c| matches!(c.slot, CueSlot::Memory) == s.memory).collect();
        if s.memory {
            // hypothesis: the CueManager keeps memory cues in time order (`Initialize` sorts them)
            kept.sort_by_key(|c| c.in_ms);
        }
        let used_pads = kept.iter().filter_map(|c| if let CueSlot::Hot(p) = c.slot { Some(p) } else { None }).collect();
        let memory = if s.memory { kept.iter().map(|c| c.in_ms).collect() } else { Vec::new() };
        Self { s, kept, used_pads, memory, plan: CuePlan::default() }
    }

    /// `removeCues(type, k)` @0x1000573b8: hot cues on pads below k; for memory the first k.
    fn remove(&mut self, k: usize) {
        let removed: Vec<&Cue> = if self.s.memory {
            self.kept.iter().take(k).copied().collect()
        } else {
            self.kept.iter().filter(|c| matches!(c.slot, CueSlot::Hot(p) if usize::from(p) < k)).copied().collect()
        };
        self.plan.remove = removed.iter().map(|c| c.id.clone()).collect();
        self.kept.retain(|c| !self.plan.remove.contains(&c.id));
        self.used_pads = self.kept.iter().filter_map(|c| if let CueSlot::Hot(p) = c.slot { Some(p) } else { None }).collect();
        if self.s.memory {
            self.memory = self.kept.iter().map(|c| c.in_ms).collect();
        }
    }

    /// The memory cues' times now (the list `findSame` checks against).
    fn memory_times(&self) -> Vec<u32> {
        self.memory.clone()
    }

    /// `create` @0x1000579c8: a memory cue unless one in `before` is within 5 ms or the track has
    /// [`MAX_MEMORY_CUES`]; a hot cue on the first free pad.
    fn create(&mut self, ms: u32, before: &[u32], comment: String) {
        let slot = if self.s.memory {
            if before.iter().any(|&b| same_time(ms, b)) || self.memory.len() >= MAX_MEMORY_CUES {
                return;
            }
            self.memory.push(ms);
            CueSlot::Memory
        } else {
            let Some(pad) = (0..HOT_PADS).find(|p| !self.used_pads.contains(p)) else { return };
            self.used_pads.push(pad);
            CueSlot::Hot(pad)
        };
        self.plan.create.push(NewCue { slot, in_ms: ms, out_ms: None, color: None, color_table_index: None, comment });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(first_beat_in_bar: u16, n: usize) -> Vec<Beat> {
        (0..n)
            .map(|i| Beat { beat_in_bar: ((usize::from(first_beat_in_bar) - 1 + i) % 4 + 1) as u16, tempo: 12800, time_ms: 100 + i as u32 * 469 })
            .collect()
    }

    fn manual() -> CueAnalysisSettings {
        CueAnalysisSettings { enabled: true, manual: true, ..CueAnalysisSettings::default() }
    }

    fn cue(id: &str, slot: CueSlot, in_ms: u32) -> Cue {
        Cue { id: id.into(), content_id: "1".into(), slot, in_ms, out_ms: None, color: None, color_table_index: None, comment: String::new(), active_loop: false }
    }

    #[test]
    fn first_cue_lands_on_the_first_downbeat() {
        // the grid starts on beat 3: beats 3, 4, 1 → the downbeat is the third beat
        let beats = grid(3, 64);
        let p = manual_plan(&beats, 0, &[], &manual());
        assert_eq!(p.create.len(), 1);
        assert_eq!((p.create[0].slot, p.create[0].in_ms, p.create[0].comment.as_str()), (CueSlot::Hot(0), beats[2].time_ms, "1.1Bars"));
    }

    #[test]
    fn repeat_every_n_bars_up_to_the_limit() {
        let beats = grid(1, 200);
        let s = CueAnalysisSettings { loop_cue: true, max_hot: 8, ..manual() };
        let p = manual_plan(&beats, 0, &[], &s);
        // bars 1, 9, 17, …: every 32 beats, 7 fit in 200 beats
        let got: Vec<(CueSlot, u32, &str)> = p.create.iter().map(|c| (c.slot, c.in_ms, c.comment.as_str())).collect();
        assert_eq!(got.len(), 7);
        assert_eq!(got[0], (CueSlot::Hot(0), beats[0].time_ms, "1.1Bars"));
        assert_eq!(got[1], (CueSlot::Hot(1), beats[32].time_ms, "1.1Bars+8"));
        assert_eq!(got[6], (CueSlot::Hot(6), beats[192].time_ms, "1.1Bars+48"));
        // without the 1.1-bar cue the first is at bar 9
        let p = manual_plan(&beats, 0, &[], &CueAnalysisSettings { first_cue: false, ..s });
        assert_eq!(p.create[0].in_ms, beats[32].time_ms);
        assert_eq!(p.create.len(), 6);
    }

    #[test]
    fn hot_cues_take_free_pads_and_overwrite_clears_the_first_pads() {
        let beats = grid(1, 200);
        let s = CueAnalysisSettings { loop_cue: true, max_hot: 3, ..manual() };
        let existing = [cue("a", CueSlot::Hot(0), 5000), cue("c", CueSlot::Hot(2), 9000), cue("m", CueSlot::Memory, 100)];
        let p = manual_plan(&beats, 0, &existing, &s);
        assert!(p.remove.is_empty());
        assert_eq!(p.create.iter().map(|c| c.slot).collect::<Vec<_>>(), [CueSlot::Hot(1), CueSlot::Hot(3), CueSlot::Hot(4)]);
        let p = manual_plan(&beats, 0, &existing, &CueAnalysisSettings { overwrite: true, ..s });
        assert_eq!(p.remove, ["a", "c"]);
        assert_eq!(p.create.iter().map(|c| c.slot).collect::<Vec<_>>(), [CueSlot::Hot(0), CueSlot::Hot(1), CueSlot::Hot(2)]);
    }

    #[test]
    fn memory_cues_skip_taken_positions() {
        let beats = grid(1, 200);
        let s = CueAnalysisSettings { memory: true, loop_cue: true, max_memory: 4, comment: false, ..manual() };
        let existing = [cue("m", CueSlot::Memory, beats[32].time_ms + 4)];
        let p = manual_plan(&beats, 0, &existing, &s);
        assert_eq!(p.create.iter().map(|c| c.in_ms).collect::<Vec<_>>(), [beats[0].time_ms, beats[64].time_ms, beats[96].time_ms]);
        assert!(p.create.iter().all(|c| c.slot == CueSlot::Memory && c.comment.is_empty()));
    }

    #[test]
    fn nothing_without_beats_or_cue_kinds() {
        assert!(manual_plan(&[], 0, &[], &manual()).create.is_empty());
        let s = CueAnalysisSettings { first_cue: false, loop_cue: false, ..manual() };
        assert!(manual_plan(&grid(1, 64), 0, &[], &s).create.is_empty());
    }

    #[test]
    fn bars_before_the_first_downbeat_are_negative() {
        let b = bar_beats(&grid(2, 6), 4);
        assert_eq!(b.iter().map(|x| (x.bar, x.beat)).collect::<Vec<_>>(), [(-1, 2), (-1, 3), (-1, 4), (1, 1), (1, 2), (1, 3)]);
        assert_eq!(manual_comment(-1), "1.1Bars+-2");
    }

    fn auto() -> CueAnalysisSettings {
        CueAnalysisSettings { enabled: true, manual: false, max_hot: 4, ..CueAnalysisSettings::default() }
    }

    #[test]
    fn auto_puts_the_first_downbeat_first_then_the_best_in_time_order() {
        let beats = grid(1, 400);
        // best first; the first downbeat (beat 0) is among them and is dropped from the list
        let cands: Vec<Beat> = [192, 64, 0, 320, 128].iter().map(|&i| beats[i]).collect();
        let p = auto_plan(&beats, 0, &cands, &[], &auto());
        let got: Vec<(CueSlot, u32, &str)> = p.create.iter().map(|c| (c.slot, c.in_ms, c.comment.as_str())).collect();
        assert_eq!(
            got,
            [
                (CueSlot::Hot(0), beats[0].time_ms, "CUE(Auto)"),
                (CueSlot::Hot(1), beats[64].time_ms, "CUE(Auto)"),
                (CueSlot::Hot(2), beats[192].time_ms, "CUE(Auto)"),
                (CueSlot::Hot(3), beats[320].time_ms, "CUE(Auto)"),
            ]
        );
        // without the 1.1-bar cue the four best, in time order
        let p = auto_plan(&beats, 0, &cands, &[], &CueAnalysisSettings { first_cue: false, ..auto() });
        assert_eq!(p.create.iter().map(|c| c.in_ms).collect::<Vec<_>>(), [0, 64, 192, 320].map(|i| beats[i].time_ms));
    }

    #[test]
    fn auto_makes_nothing_without_candidates() {
        assert!(auto_plan(&grid(1, 64), 0, &[], &[], &auto()).create.is_empty());
    }

    #[test]
    fn auto_memory_cues_skip_taken_positions_and_overwrite_counts_the_candidates() {
        let beats = grid(1, 400);
        let s = CueAnalysisSettings { memory: true, max_memory: 3, comment: false, ..auto() };
        let cands: Vec<Beat> = [64, 128].iter().map(|&i| beats[i]).collect();
        let existing = [cue("m", CueSlot::Memory, beats[64].time_ms + 3), cue("n", CueSlot::Memory, 5)];
        let p = auto_plan(&beats, 0, &cands, &existing, &s);
        assert!(p.remove.is_empty());
        assert_eq!(p.create.iter().map(|c| c.in_ms).collect::<Vec<_>>(), [beats[0].time_ms, beats[128].time_ms]);
        // overwrite: min(limit 3, 2 candidates) = the first two memory cues in time order go
        let p = auto_plan(&beats, 0, &cands, &existing, &CueAnalysisSettings { overwrite: true, ..s });
        assert_eq!(p.remove, ["n", "m"]);
        assert_eq!(p.create.len(), 3);
    }

    #[test]
    fn preferences_with_defaults() {
        let s = CueAnalysisSettings::from_prefs(|n| match n {
            "DetectCue" => Some("1".into()),
            "AutoDetectCue" => Some("1".into()),
            "LoopCueCount" => Some("16".into()),
            _ => None,
        });
        assert!(s.runs() && !s.auto());
        assert_eq!((s.loop_bars, s.max_hot, s.first_cue, s.comment), (16, 4, true, true));
        assert!(!CueAnalysisSettings::default().runs());
        assert!(CueAnalysisSettings::from_prefs(|n| (n == "DetectCue").then(|| "1".into())).auto());
    }
}
