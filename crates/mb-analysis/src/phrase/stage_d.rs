//! Stage D: `Sub_Main_D` @0x102379edc (`analysis-phrase.md` § 7). From stage C's features it
//! picks the genre and mood, finds the phrase starts, labels the blocks between them and fills
//! `SongStructureAnalyzeResult` (§ 8.1).

use super::adjust::{self, PhraseResult};
use super::stage_c::COut;
use super::{blocks, d_feat, genre, intro, melo, snare, verse};

/// What `Sub_Main_D` reads from the analyser besides stage C's outputs.
#[derive(Debug, Clone)]
pub struct DIn<'a> {
    /// +0xa8: BPM.
    pub bpm: f64,
    /// +0x80: sample rate.
    pub rate: f64,
    /// +0x90: the decimated rate (rate / 8).
    pub fs8: f64,
    /// +0xb0: the key index (the thread passes 0 as far as observed: PSSI byte 0x1d is 1).
    pub key: i32,
    /// +0xb8: analysed samples.
    pub total: i32,
    /// +0xa4: the first downbeat (ms) before stage B.
    pub fb0: i32,
    /// +0x200 / +0x204: stage B's first downbeat (ms) and its bar.
    pub fb: i32,
    pub bar: i32,
    /// +0x208: `BeatposCorrection2016`'s kick-on-6 flag.
    pub kick6: bool,
    /// +0x210 / +0x250 / +0x254: `DetectSnarePositionsForGenre`.
    pub snares: &'a snare::GenreSnares,
    /// +0x258: `BarPositionJudge`.
    pub bar_pos: i32,
}

/// Reports a callee's arguments to the replay test (nothing outside tests).
macro_rules! probe {
    ($name:expr; $($v:expr),* $(,)?) => {
        #[cfg(test)]
        {
            #[allow(unused_imports)]
            use probe::{f, i, int, real, u, Val::Skip};
            probe::call($name, vec![$($v),*]);
        }
    };
}

/// The result D writes when the track never left silence (+0x10f0 = 0): no phrases, a start
/// beat of −1.
fn failed() -> PhraseResult {
    let mut r = PhraseResult::default();
    for c in r.cols.iter_mut() {
        *c = vec![0];
    }
    r.cols[0][0] = -1;
    r
}

/// Deletes the zero entries of `v` from the back, as the binary's `delete_element` loops do.
fn delete_zeros(v: &mut Vec<i32>) {
    v.retain(|&x| x != 0);
}

/// `Sub_Main_D`. `None` where the binary fails (no result is written then).
pub fn sub_main_d(c: &COut, d: &DIn) -> Option<PhraseResult> {
    if c.nonzero != 1 {
        return Some(failed());
    }
    let bar = d.bar;
    let n = c.n;
    let dur = d.total as f64 / d.rate;

    // Features per bar and beat.
    let mut cqt = c.cqt.clone();
    probe!("IntroEndFind_b"; real(d.bpm), real(d.fs8), Skip, int(c.cqt_frames), int(c.cqt_first), int(n));
    let (_strong, _jump, level) = d_feat::intro_end_find_b(&mut cqt, c.cqt_frames, c.cqt_first, n);
    probe!("LowLevelAnalyze_b"; f(&c.low_level), int(bar));
    let (low_bars, low_flags) = d_feat::low_level_b(&c.low_level, bar);
    probe!("MiddleLevelAnalyze_b"; f(&c.mid_level), int(bar));
    let (mid_flags, mid_bars) = d_feat::middle_level_b(&c.mid_level, bar);
    let mut mid_peak = c.mid_peak.clone();
    probe!("MiddlePeakAnalyze_b"; f(&mid_peak), int(n), int(bar));
    let mid_means = d_feat::middle_peak_b(&mut mid_peak, n, bar);
    probe!("AllLevelAnalyze_b"; f(&c.all_level), int(bar));
    let (all_bars, _all_norm) = d_feat::all_level_b(&c.all_level, bar);
    probe!("HiHatAnalyze_b"; u(&c.hihat), int(bar));
    let hihat_flags = d_feat::hihat_b(&c.hihat, bar);
    probe!("HighPeakAnalyze32_b"; f(&c.high_peak), int(bar));
    let (high, high_n) = d_feat::high_peak_b(&c.high_peak, bar);
    let uta = d_feat::uta_umu(&c.fft1)?;

    // Genre.
    probe!("DetectComp"; f(&c.low_power), f(&c.all_power), int(n), u(&c.pattern), int(bar));
    let (_comp_share, comp_level) = genre::detect_comp(&c.low_power, &c.all_power, &c.pattern, bar);
    let fadeout = genre::fade_out(&c.low_power, &c.all_power);
    let _max_ratio = genre::max_level_ratio(&c.all_peak);
    let b128 = genre::bpm128(d.bpm);
    let tenkei = genre::tenkei_genre1(d.bpm, d.kick6);
    probe!("DetectSlowrock"; u(&c.pattern), u(&c.pattern2), real(d.bpm));
    let slowrock = genre::detect_slowrock(&c.pattern, &c.pattern2);
    let r1 = genre::track_low_power_ratio(&c.low_power, &c.all_power, n);
    let _r2 = genre::track_low_power_ratio(&c.low40, &c.all_power, n);
    probe!("DetectSlowTempo"; real(d.fs8), f(&r1.bar_low), real(d.bpm), i(&d.snares.pos), Skip,
        int(d.fb0), int(d.fb), i(&c.beats8), int(n), int(d.snares.count), int(d.snares.first));
    let (slow, _half) = genre::slow_tempo(
        d.fs8,
        &r1.bar_low,
        d.bpm,
        &d.snares.pos,
        d.fb0,
        d.fb,
        &c.beats8,
        n,
        d.snares.count,
        d.snares.first,
    );
    let k4 = genre::four_kicks(&c.pattern, d.bpm, &r1.bar_low);
    let mut mood = genre::genre_devide(slowrock, slow, r1.ratio, k4, fadeout, comp_level, b128, tenkei);
    probe!("Genre12FinalJudge"; int(mood), int(n), f(&c.low_peak), f(&c.low_peak_q));
    genre::genre12_final(&mut mood, n, &c.low_peak, &c.low_peak_q);
    let secure = genre::secure_points(&c.low_peak, bar);

    // Secure points as beats; a copy keeps only 1..=199.
    let mut sb: Vec<i32> = if secure[0] == 0 {
        vec![0; secure.len()]
    } else {
        secure.iter().map(|&s| bar - 4 + 4 * s).collect()
    };
    let sb_all = sb.clone();
    let mut sb2 = sb.clone();
    for v in sb2.iter_mut() {
        if !(1..=199).contains(v) {
            *v = 0;
        }
    }
    if secure[0] != 0 {
        delete_zeros(&mut sb2);
    }

    // The type: 1 for genre 1, 2 for genre 3, judged for genre 2.
    let mut kind;
    if mood == 1 {
        kind = 1;
    } else if mood != 2 {
        kind = 2;
    } else {
        kind = 0;
        let ns = 2 * secure.len() + 1;
        let (mut b1, mut l1) = (vec![0i32; ns], vec![0i32; ns]);
        let (mut b2, mut l2) = (vec![0i32; ns], vec![0i32; ns]);
        if secure[0] != 0 {
            genre2_blocks(&secure, bar, 10, 9, -36, &mut b1, &mut l1);
            genre2_blocks(&secure, bar, 18, 17, -68, &mut b2, &mut l2);
            probe!("BlockAverage"; i(&b1), f(&level), f(&c.low_peak), int(bar), u(&c.pattern2), f(&c.low_peak_q));
            let m = blocks::block_average(&b1, &level, &c.low_peak, bar, &c.low_peak_q)?;
            probe!("UpSimpleloopAna"; i(&b1), i(&l1), f(&c.band_lo), f(&c.band_hi), f(&m.peak));
            blocks::up_simpleloop(&b1, &mut l1, &c.band_lo, &c.band_hi, &m.peak);
            probe!("Up4_8_16Ana"; i(&b2), i(&l2), u(&c.pattern2));
            l2 = blocks::up_4_8_16(&b2, &l2, &c.pattern2).labels;
        }
        probe!("SongStructureAna_Type_Genre_Judge"; f(&c.low_peak), int(bar), real(dur), i(&secure), i(&l1), i(&l2), int(mood), int(kind));
        genre::type_genre_judge(&c.low_peak, bar, dur, &l1, &l2, &mut mood, &mut kind)?;
    }

    probe!("IntroEndJudge_main"; f(&c.low_peak), f(&low_bars), f(&mid_means), int(bar), f(&uta), i(&sb2));
    let (intro_list, intro_end, found) =
        intro::intro_end_judge_main(&c.low_peak, &low_bars, &mid_means, bar, &uta, &sb2)?;
    probe!("VerseHookAnaData23"; f(&c.low_peak), f(&low_bars), f(&mid_peak), f(&mid_means), int(bar));
    let vd = verse::verse_hook_ana_data23(&c.low_peak, &low_bars, &mid_peak, &mid_means, bar);

    let low_peak = &c.low_peak;

    // Phrase starts (D's +0x820). It starts as one zero entry.
    let mut starts: Vec<i32> = vec![0];
    let mut count = 0i32;
    let mut fills: Vec<i32> = vec![0];
    if kind == 1 {
        if found == 1 {
            starts = mood1_starts(
                c,
                d,
                &vd,
                &intro_list,
                intro_end,
                &low_flags,
                &mid_flags,
                &hihat_flags,
                low_peak,
                &low_bars,
                &mid_bars,
                &level,
                &all_bars,
                &mut sb,
                &sb_all,
                &mid_peak,
                &high,
                kind,
                dur,
            )?;
        } else if found == 0 {
            starts = vec![d.bar_pos];
        }
        count = starts.len() as i32;
        fills = vec![0; starts.len()];
    } else {
        if found == 1 {
            probe!("VerseHookJudge23"; f(&vd.peak.norm), f(&vd.peak.spread), f(&vd.peak.raw), f(&vd.low.norm), f(&vd.low.spread), f(&vd.low.raw),
                f(&vd.mid_bar.norm), f(&vd.mid_bar.spread), f(&vd.mid_bar.raw), i(&intro_list), int(intro_end), int(bar), int(kind));
            starts = verse::verse_hook_judge23(&vd.peak, &vd.low, &vd.mid_bar, &intro_list, intro_end, bar, kind)?;
            probe!("VerseHookAdd_genre23"; f(&c.low_level), f(&mid_peak), f(&high), f(&uta), f(&level), i(&starts), int(bar));
            starts = verse::verse_hook_add_genre23(&c.low_level, &mid_peak, &high, &uta, &level, &starts, bar)?;
            if starts.is_empty() {
                return None;
            }
            // Drop starts past the analysed range, and those up to the bar position.
            while let Some(&last) = starts.last() {
                if last <= c.mid_level.len() as i32 && last <= level.len() as i32 {
                    for v in starts.iter_mut() {
                        if d.bar_pos + 3 < *v {
                            break;
                        }
                        *v = 0;
                    }
                    while starts.first() == Some(&0) {
                        starts.remove(0);
                    }
                    break;
                }
                starts.pop();
            }
            starts.insert(0, d.bar_pos);
        }
        if found == 0 {
            starts = vec![d.bar_pos];
        }
        if starts.len() > 1 && starts[0] == starts[1] {
            starts.remove(0);
        }
    }

    // Labels.
    let mut bank = 0;
    let mut labels: Vec<i32>;
    let ends: Vec<i32>;
    let up: blocks::UpOut;
    let mut keep: Vec<i32>;
    if mood == 1 {
        probe!("BlockAverage"; i(&starts), f(&level), f(low_peak), int(bar), u(&c.pattern2), f(&c.low_peak_q));
        let mut m = blocks::block_average(&starts, &level, low_peak, bar, &c.low_peak_q)?;
        probe!("SongStructureJudge"; i(&starts), f(&m.level), f(&m.peak), f(&level), f(&c.mid_level));
        let (l, flag) = verse::song_structure_judge(&starts, &mut m.level, &mut m.peak);
        labels = l;
        bank = (flag != 1) as i32;
        probe!("IntroFeatureAna"; i(&starts), i(&labels), f(low_peak), f(&c.low_peak_q), f(&m.beat_peak), i(&fills), f(&high), int(high_n));
        verse::intro_feature_ana(&starts, &mut labels, low_peak, &c.low_peak_q, &m.beat_peak, &mut fills, &high, high_n)?;
        let before = labels.clone();
        probe!("UpSimpleloopAna"; i(&starts), i(&labels), f(&c.band_lo), f(&c.band_hi), f(&m.peak));
        let (_, loop_peak) = blocks::up_simpleloop(&starts, &mut labels, &c.band_lo, &c.band_hi, &m.peak);
        probe!("Up4_8_16Ana"; i(&starts), i(&labels), u(&c.pattern2));
        up = blocks::up_4_8_16(&starts, &labels, &c.pattern2);
        labels.clone_from(&up.labels);
        probe!("UpSimpleloopAna_b"; i(&starts), i(&labels), i(&before), f(&loop_peak));
        verse::up_simpleloop_b(&starts, &mut labels, &before, &loop_peak);
        probe!("UpFeatureAna"; i(&starts), i(&labels), f(low_peak), f(&m.beat_peak), i(&fills), f(&high), int(high_n));
        verse::up_feature_ana(&starts, &mut labels, low_peak, &m.beat_peak, &mut fills, &high, high_n)?;
        probe!("HookFeatureAna"; i(&starts), i(&labels), f(low_peak), f(&c.low_peak_q), f(&m.beat_peak), i(&fills), f(&high), int(high_n));
        verse::hook_feature_ana(&starts, &mut labels, low_peak, &c.low_peak_q, &m.beat_peak, &mut fills, &high, high_n)?;
        probe!("OutroFeatureAna"; i(&starts), i(&labels), f(low_peak), u(&c.pattern2), f(&c.low_peak_q), int(bar));
        let (l, e) = verse::outro_feature_ana(&starts, &labels, low_peak, &c.pattern2, &c.low_peak_q, bar);
        labels = l;
        ends = e;
        keep = vec![0; count.max(0) as usize];
        probe!("FillAdjustAna"; i(&starts), i(&labels), i(&fills), int(count), f(low_peak), f(&c.all_power), i(&keep));
        adjust::fill_adjust_ana(&starts, &mut labels, &mut fills, count, low_peak, &c.all_power, &mut keep);
        probe!("FeatureLastAdjust"; i(&starts), i(&labels), i(&fills), i(&ends), int(1), f(&m.peak));
        adjust::feature_last_adjust(&starts, &mut labels, &mut fills, &ends, 1, &m.peak);
    } else {
        count = starts.len() as i32;
        fills = vec![0; starts.len()];
        up = blocks::UpOut {
            labels: Vec::new(),
            a: vec![0; starts.len()],
            b: vec![0; starts.len()],
            c: vec![0; starts.len()],
            d: vec![0; starts.len()],
        };
        let mut m_peak: Vec<f64> = Vec::new();
        let melo_labels: Vec<i32>;
        if starts.len() == 1 {
            labels = vec![10];
            melo_labels = vec![10];
        } else {
            let similar = melo::ab_melo_analyze(&c.fft2, &starts, bar, c.n)?;
            probe!("HooksoloAnalyze"; f(&mid_bars), f(&level), i(&starts), f(&uta), int(mood));
            let mut hl = melo::hooksolo_analyze(&mid_bars, &level, &starts, &uta, mood);
            probe!("TenkaimeiAnalyze"; Skip, i(&hl));
            if !melo::tenkaimei_analyze(&similar, &mut hl) {
                return None;
            }
            melo_labels = hl;
            probe!("BlockAverage"; i(&starts), f(&level), f(low_peak), int(bar), u(&c.pattern2), f(&c.low_peak_q));
            let mut m = blocks::block_average(&starts, &level, low_peak, bar, &c.low_peak_q)?;
            probe!("SongStructureJudge"; i(&starts), f(&m.level), f(&m.peak), f(&level), f(&c.mid_level));
            let (l, _) = verse::song_structure_judge(&starts, &mut m.level, &mut m.peak);
            labels = l;
            m_peak = m.peak;
            probe!("YokuyoAnalyze"; f(&level), i(&starts), int(bar));
            bank = melo::yokuyo_analyze(&level, &starts).0;
        }
        probe!("OutroFeatureAna"; i(&starts), i(&labels), f(low_peak), u(&c.pattern2), f(&c.low_peak_q), int(bar));
        let (_, e) = verse::outro_feature_ana(&starts, &labels, low_peak, &c.pattern2, &c.low_peak_q, bar);
        ends = e;
        labels = melo_labels;
        probe!("FeatureLastAdjust"; i(&starts), i(&labels), i(&fills), i(&ends), int(mood), f(&m_peak));
        adjust::feature_last_adjust(&starts, &mut labels, &mut fills, &ends, mood, &m_peak);
        keep = vec![0];
    }

    // The result.
    let nres = count.max(0) as usize;
    let mut res = PhraseResult {
        count,
        fb: d.fb,
        mood,
        bar,
        bank,
        unk_14: 0,
        end_beat: ends.last().copied().unwrap_or(0),
        cols: Default::default(),
    };
    for col in res.cols.iter_mut() {
        *col = vec![0; nres.max(1)];
    }
    for k in 0..nres {
        res.cols[0][k] = starts.get(k).copied().unwrap_or(0);
        res.cols[10][k] = fills.get(k).copied().unwrap_or(0);
    }
    adjust::data_format_convert(mood, &labels, d.key, &up.b, &up.c, &up.d, &up.a, &mut res, &mut keep);
    Some(res)
}

/// The genre-2 trial blocks from the secure points: a block of label 40 per point, preceded
/// by a 30 block `pre` beats earlier where the gap to the previous point is at least `gap`
/// bars (`first` bars for the first point).
fn genre2_blocks(secure: &[i32], bar: i32, first: i32, gap: i32, pre: i32, b: &mut Vec<i32>, l: &mut Vec<i32>) {
    b[0] = 1;
    l[0] = 10;
    let s0 = secure[0];
    let (mut c40, mut c30) = if s0 < first {
        b[1] = bar + 4 * s0 - 4;
        l[1] = 40;
        (1usize, 0usize)
    } else {
        b[1] = bar + 4 * s0 + pre;
        b[2] = bar + 4 * s0 - 4;
        l[1] = 30;
        l[2] = 40;
        (0, 1)
    };
    for k in 0..secure.len().saturating_sub(1) {
        let e = secure[k + 1] * 4;
        if secure[k + 1] - secure[k] >= gap {
            let at = c40 + 2 * c30;
            b[at + 1] = bar + pre + e;
            b[at + 2] = bar - 4 + e;
            l[at + 1] = 30;
            l[at + 2] = 40;
            c30 += 1;
        } else {
            c40 += 1;
            let at = c40 + 2 * c30;
            b[at] = bar - 4 + e;
            l[at] = 40;
        }
    }
    let mut k = b.len();
    while k > 0 {
        if b[k - 1] == 0 {
            b.remove(k - 1);
            l.remove(k - 1);
        }
        k -= 1;
    }
}

/// The mood-1 phrase starts (D's `kind == 1`, intro found).
#[allow(clippy::too_many_arguments)]
fn mood1_starts(
    c: &COut,
    d: &DIn,
    vd: &verse::VerseData,
    intro_list: &[i32],
    intro_end: i32,
    low_flags: &[u8],
    mid_flags: &[u8],
    hihat_flags: &[u8],
    low_peak: &[f64],
    low_bars: &[f64],
    mid_bars: &[f64],
    level: &[f64],
    all_bars: &[f64],
    sb: &mut Vec<i32>,
    sb_all: &[i32],
    mid_peak: &[f64],
    high: &[f64],
    kind: i32,
    dur: f64,
) -> Option<Vec<i32>> {
    let bar = d.bar;
    probe!("VerseHookJudge_LowPeak"; int(intro_end), f(&c.low_flags), int(bar), u(low_flags));
    let (cnt, list) = verse::verse_hook_low_peak(intro_end, &c.low_flags, bar, low_flags)?;
    // The first candidate followed by a gap of at least 113 beats (the last one against the
    // end of the flags).
    let cand: Vec<i32> = list.iter().take(cnt.max(0) as usize).copied().collect();
    let mut gap = 0;
    for (k, &v) in cand.iter().enumerate() {
        let next = cand.get(k + 1).copied().unwrap_or(c.low_flags.len() as i32);
        if next - v >= 113 {
            gap = v;
            break;
        }
    }
    probe!("BreakFind"; int(cnt), i(&list), int(bar), f(low_bars), f(mid_bars), f(all_bars));
    let brk = verse::break_find(cnt, &list, bar, low_bars, mid_bars, all_bars);
    probe!("BigChangeFind"; int(cnt), i(&list), int(bar), f(low_bars), f(mid_bars), f(all_bars), f(level), f(low_peak));
    let chg = verse::big_change_find(cnt, &list, bar, low_bars, mid_bars, all_bars, level, low_peak);
    let (mut last, mut phr) = if gap < 2 && brk.0 < 2 && chg.0 < 2 {
        (0, vec![intro_end])
    } else {
        let a = verse::Irregular {
            intro: intro_end,
            bar,
            gap,
            brk: (brk.0, brk.1),
            deep: brk.2,
            change1: (chg.0, chg.1),
            change2: (chg.2, chg.3),
        };
        probe!("IrregularJudge"; int(intro_end), int(cnt), i(&list), f(&c.low_flags), int(bar), int(gap), int(brk.0), int(brk.1),
            int(chg.0), int(chg.1), int(chg.2), int(chg.3), u(low_flags), int(brk.2));
        // Its third output (the candidate list, cut) is not read again.
        let (k, p, _) = verse::irregular_judge(a, &list, &c.low_flags, low_flags)?;
        (k, p)
    };
    if !sb.is_empty() && sb[0] >= 1 {
        let mut u = last;
        while u >= 0 {
            let mut v = sb.len();
            while v >= 1 {
                let x = phr.get(u as usize).copied().unwrap_or(0);
                let s = sb[v - 1];
                if x == s {
                    sb.remove(v - 1);
                } else if x + 4 == s || x - 4 == s {
                    // (The binary also counts `last` down; it is recomputed below.)
                    if let Some(p) = phr.get_mut(u as usize) {
                        *p = 0;
                    }
                }
                v -= 1;
            }
            u -= 1;
        }
        phr.extend_from_slice(sb);
        phr.sort_unstable_by(|a, b| b.cmp(a));
        delete_zeros(&mut phr);
        phr.sort_unstable();
        last = phr.len() as i32 - 1;
    }
    // `VerseHookJudge_All` works on copies: D's arrays keep their zeros.
    let mut cv = verse::RatioCurves {
        peaks: low_peak.to_vec(),
        low: low_bars.to_vec(),
        mid: mid_bars.to_vec(),
        level: level.to_vec(),
        all: all_bars.to_vec(),
    };
    probe!("VerseHookJudge_All"; int(last), i(&phr), int(intro_end), int(bar), f(&c.low_flags), u(low_flags), u(mid_flags), u(hihat_flags),
        f(&cv.peaks), f(&cv.low), f(&cv.mid), f(&cv.level), f(&cv.all));
    let all = verse::verse_hook_judge_all(last, &phr, bar, &c.low_flags, low_flags, mid_flags, hihat_flags, &mut cv);
    let all = all?;
    probe!("CandidateAdjust"; i(&all), int(bar), f(all_bars), f(low_bars));
    let adj = verse::candidate_adjust(&all, all_bars, low_bars);
    let mut starts = if adj.len() as f64 <= dur * 0.052000000000000005 {
        adj
    } else {
        probe!("RhythmPatternReject"; i(&adj), u(&c.pattern), int(bar));
        verse::rhythm_pattern_reject(&adj, &c.pattern)?.0
    };
    starts.insert(0, d.bar_pos);
    let a = starts.clone();
    probe!("VerseHookJudge23"; f(&vd.peak.norm), f(&vd.peak.spread), f(&vd.peak.raw), f(&vd.low.norm), f(&vd.low.spread), f(&vd.low.raw),
        f(&vd.mid_bar.norm), f(&vd.mid_bar.spread), f(&vd.mid_bar.raw), i(intro_list), int(intro_end), int(bar), int(kind));
    let est = verse::verse_hook_judge23(&vd.peak, &vd.low, &vd.mid_bar, intro_list, intro_end, bar, kind)?;
    probe!("VerseHookAdd_genre1"; f(&vd.low.norm), f(&vd.low.spread), f(&vd.low.raw), f(&c.low_level), f(mid_peak), f(high), i(&a), i(&est), int(bar));
    let mut starts = verse::verse_hook_add_genre1(&vd.low.norm, &vd.low.spread, &vd.low.raw, &c.low_level, mid_peak, high, &a, &est, bar)?;
    for v in starts.iter_mut() {
        if d.bar_pos + 3 < *v {
            break;
        }
        *v = 0;
    }
    while starts.first() == Some(&0) {
        starts.remove(0);
    }
    starts.insert(0, d.bar_pos);
    // Merge the secure beats: insert the missing ones in order, then drop starts within 8
    // beats of one.
    for &s in sb_all.iter().rev() {
        let mut k = starts.len();
        let mut at = None;
        while k >= 1 {
            if s == starts[k - 1] {
                break;
            }
            k -= 1;
            if !(s <= starts[k]) {
                at = Some(k + 1);
                break;
            }
        }
        if let Some(at) = at {
            starts.insert(at, s);
        }
    }
    for &s in sb_all.iter().rev() {
        let mut k = starts.len();
        while k >= 2 {
            if let Some(&e) = starts.get(k - 1)
                && s - 8 <= e && e <= s + 8 && s != e {
                    starts.remove(k - 1);
                }
            k -= 1;
        }
    }
    if starts.len() >= 2 && starts[0] == starts[1] {
        starts.remove(0);
    }
    Some(starts)
}

/// Arguments as the replay test compares them with the traced calls.
#[cfg(test)]
pub(super) mod probe {
    use std::cell::RefCell;

    #[derive(Debug, Clone)]
    pub enum Val {
        I(Vec<i32>),
        F(Vec<f64>),
        U(Vec<u8>),
        Int(i64),
        Real(f64),
        Skip,
    }

    type Hook = Box<dyn FnMut(&str, Vec<Val>)>;

    thread_local! {
        pub static HOOK: RefCell<Option<Hook>> = RefCell::new(None);
    }

    pub fn call(name: &str, args: Vec<Val>) {
        HOOK.with(|h| {
            if let Some(f) = h.borrow_mut().as_mut() {
                f(name, args)
            }
        })
    }

    pub fn i(v: &[i32]) -> Val {
        Val::I(v.to_vec())
    }
    pub fn f(v: &[f64]) -> Val {
        Val::F(v.to_vec())
    }
    pub fn u(v: &[u8]) -> Val {
        Val::U(v.to_vec())
    }
    pub fn int(v: i32) -> Val {
        Val::Int(v as i64)
    }
    pub fn real(v: f64) -> Val {
        Val::Real(v)
    }
}
