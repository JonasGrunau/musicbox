//! CUE analysis, Manual mode (`docs/re/analysis-cue.md` § 3): after an analysis, rekordbox can
//! set hot cues or memory cues on the new beat grid, at 1.1 bars and then every N bars
//! (`DatabaseMediator::createDetectCue` @0x101495900 → `CueManager::ManualCueCreation`
//! @0x100056e94). Auto mode (the cue-point models and the personal CUE tendency) is not
//! implemented; with it selected no cues are made.

use crate::cues::{Cue, CueSlot, NewCue};
use mb_anlz::tags::Beat;

/// The CUE analysis preferences as `DatabaseMediator::getDetectCueParam` @0x1014997a4 reads them
/// (Preferences > Analysis > CUE Analysis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CueAnalysisSettings {
    /// `DetectCue` (default 0): "Set CUE during analysis".
    pub enabled: bool,
    /// `AutoDetectCue` ≠ 0: Manual. 0 (the default) is Auto, which musicbox does not implement.
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

    /// Manual cue creation will run.
    pub fn runs(&self) -> bool {
        self.enabled && self.manual
    }
}

/// What a Manual pass does to one track: earlier cues removed (overwrite), then new cues in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManualPlan {
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
fn comment(bar: i32) -> String {
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
pub fn manual_plan(beats: &[Beat], beat_num_offset: u32, existing: &[Cue], s: &CueAnalysisSettings) -> ManualPlan {
    let mut plan = ManualPlan::default();
    // `createDetectCue` makes no cues without beats; `ManualCueCreation` returns 1 when neither
    // the 1.1-bar cue nor the repeat is on.
    if beats.is_empty() || !(s.first_cue || s.loop_cue) {
        return plan;
    }
    let first = if beat_num_offset == 0 { first_downbeat(beats) } else { beat_num_offset };
    let list = bar_beats(beats, first);
    let limit = if s.memory { s.max_memory } else { s.max_hot };
    if limit < 1 {
        return plan;
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

    let is_target = |c: &Cue| matches!(c.slot, CueSlot::Memory) == s.memory;
    let mut kept: Vec<&Cue> = existing.iter().filter(|c| is_target(c)).collect();
    if s.memory {
        // hypothesis: the CueManager keeps memory cues in time order (`Initialize` sorts them)
        kept.sort_by_key(|c| c.in_ms);
    }
    // Overwrite: `removeCues(type, k)` @0x1000573b8, k = the cues about to be made. Hot cues on
    // pads below k go; for memory cues the first k.
    if s.overwrite {
        let k = picked.len().min(n);
        let removed: Vec<&Cue> = if s.memory {
            kept.iter().take(k).copied().collect()
        } else {
            kept.iter().filter(|c| matches!(c.slot, CueSlot::Hot(p) if usize::from(p) < k)).copied().collect()
        };
        plan.remove = removed.iter().map(|c| c.id.clone()).collect();
        kept.retain(|c| !plan.remove.contains(&c.id));
    }
    // The memory-cue duplicate check sees the cues before this pass only.
    let before: Vec<u32> = kept.iter().map(|c| c.in_ms).collect();
    let mut used_pads: Vec<u8> = kept.iter().filter_map(|c| if let CueSlot::Hot(p) = c.slot { Some(p) } else { None }).collect();
    let mut memory_count = kept.len();
    for b in picked {
        let slot = if s.memory {
            if before.iter().any(|&ms| same_time(b.ms, ms)) || memory_count >= MAX_MEMORY_CUES {
                continue;
            }
            memory_count += 1;
            CueSlot::Memory
        } else {
            let Some(pad) = (0..HOT_PADS).find(|p| !used_pads.contains(p)) else { continue };
            used_pads.push(pad);
            CueSlot::Hot(pad)
        };
        plan.create.push(NewCue {
            slot,
            in_ms: b.ms,
            out_ms: None,
            color: None,
            color_table_index: None,
            comment: if s.comment { comment(b.bar) } else { String::new() },
        });
    }
    plan
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
        assert_eq!(comment(-1), "1.1Bars+-2");
    }

    #[test]
    fn preferences_with_defaults() {
        let s = CueAnalysisSettings::from_prefs(|n| match n {
            "DetectCue" => Some("1".into()),
            "AutoDetectCue" => Some("1".into()),
            "LoopCueCount" => Some("16".into()),
            _ => None,
        });
        assert!(s.runs());
        assert_eq!((s.loop_bars, s.max_hot, s.first_cue, s.comment), (16, 4, true, true));
        assert!(!CueAnalysisSettings::default().runs());
    }
}
