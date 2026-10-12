//! How rekordbox reads a rekordbox.xml track back (`db::DjPlaylistsAnalyzer`, docs/re/ops/import-xml.md):
//! TEMPO entries become a per-beat grid, POSITION_MARKs become cues, `Colour`/`Rating`/`AverageBpm`
//! become library values. Everything here is from the binary; no fixture has an imported track.

use crate::export::GridBeat;
use crate::model::{PositionMark, Tempo};

/// `getTrackBeatInfo` @0x101052944: the beat grid rekordbox builds from a track's TEMPO entries.
///
/// Every TEMPO is one beat at `Inizio`. Between two entries, and from the last one to
/// `TotalTime`, beats are filled in at `60 / Bpm` seconds with the bar position advancing modulo
/// the `Metro` numerator. A filled beat closer than half a beat to the next entry is not added.
/// Entries with a non-positive `Bpm`, `Battito` or `Metro` are skipped (they end the fill), as is
/// an entry that is not after the previous beat or lies past `TotalTime` (except a first entry at
/// 0). Times and tempos are rounded to ms and BPM × 100 (`(long)(v + 0.5)`).
pub fn beats_from_tempos(total_time_s: f64, tempos: &[Tempo]) -> Vec<GridBeat> {
    let mut beats = Vec::new();
    let (mut bpm, mut battito, mut metro, mut last) = (0.0f64, 0i32, 0i32, 0.0f64);
    let beat = |bpm: f64, at: f64, battito: i32| GridBeat {
        beat_in_bar: battito as u16,
        tempo_x100: (bpm * 100.0 + 0.5) as i64 as u32 as u16,
        time_ms: (at * 1000.0 + 0.5) as i64 as u32,
    };
    // The fill between beats (`do … while (next < until)`).
    let fill = |beats: &mut Vec<GridBeat>,
                bpm: f64,
                battito: &mut i32,
                metro: i32,
                last: &mut f64,
                until: f64,
                gap: bool| loop {
        let next = *last + 60.0 / bpm;
        if next <= *last || total_time_s < next || (gap && until <= next + 30.0 / bpm) {
            break;
        }
        *battito = *battito % metro + 1;
        beats.push(beat(bpm, next, *battito));
        *last = next;
        if next >= until {
            break;
        }
    };
    for t in tempos {
        let inizio = t.inizio;
        if bpm > 0.0 && battito > 0 && metro > 0 && last < inizio {
            fill(
                &mut beats,
                bpm,
                &mut battito,
                metro,
                &mut last,
                inizio,
                true,
            );
        }
        bpm = t.bpm;
        battito = t.battito as i32;
        metro = metro_numerator(&t.metro);
        if bpm > 0.0 && battito > 0 && metro > 0 {
            if (inizio <= last || total_time_s < inizio)
                && (!beats.is_empty() || inizio < 0.0 || total_time_s < inizio)
            {
                continue;
            }
            beats.push(beat(bpm, inizio, battito));
            last = inizio;
        }
    }
    if bpm > 0.0 && battito > 0 && metro > 0 && last < total_time_s {
        fill(
            &mut beats,
            bpm,
            &mut battito,
            metro,
            &mut last,
            total_time_s,
            false,
        );
    }
    beats
}

/// `Metro`'s numerator (`substring(0, indexOf("/")).getIntValue()`; `juce::String::getIntValue`
/// reads leading digits and yields 0 for none).
fn metro_numerator(metro: &str) -> i32 {
    let head = metro.split('/').next().unwrap_or("").trim_start();
    let (sign, digits) = match head.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, head.strip_prefix('+').unwrap_or(head)),
    };
    let n: String = digits.chars().take_while(char::is_ascii_digit).collect();
    sign * n.parse::<i32>().unwrap_or(0)
}

/// One cue as `getTrackCueInfo` @0x101053228 builds it (`analyzer::Cue`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlCue {
    /// Player-side kind: −1 memory loop, 0 memory cue, 1–3 hot cues A–C, 5–9 D–H.
    pub kind: i32,
    pub start_ms: u32,
    /// 0 = none (a `Type="0"` mark, or no `End`).
    pub end_ms: u32,
    pub name: String,
    /// `Red`/`Green`/`Blue` of a hot cue (`Num` ≥ 0) when all three are present.
    pub rgb: Option<[u8; 3]>,
}

impl XmlCue {
    pub fn is_hot(&self) -> bool {
        (1..=3).contains(&self.kind) || (5..=17).contains(&self.kind)
    }
}

/// The cues rekordbox reads from a track's POSITION_MARKs, in document order.
///
/// A mark is skipped unless `Type` is 0 or 4, `0 ≤ Start ≤ TotalTime`, for loops
/// `Start ≤ End ≤ TotalTime`, and `Num` is −1…7. Kind: `Num` −1 with `Type` 4 → −1 (memory loop),
/// `Num` < 3 → `Num` + 1 (so a −1 cue point is a memory cue, 0), else `Num` + 2. Positions are
/// `(int)(s × 1000 + 0.5)` ms. A hot cue replaces an earlier mark of the same pad; a memory cue
/// with the in and out point of an earlier one is dropped (*hypothesis*: the second check of
/// `getTrackCueInfo` read as "skip").
pub fn cues_from_marks(total_time_s: f64, marks: &[PositionMark]) -> Vec<XmlCue> {
    let mut cues: Vec<XmlCue> = Vec::new();
    for m in marks {
        let ty = m.mark_type;
        let start = m.start;
        let end = m.end.unwrap_or(0.0);
        if ty & !4 != 0
            || start < 0.0
            || total_time_s < start
            || (ty == 4 && (end < start || total_time_s < end))
            || !(-1..=7).contains(&m.num)
        {
            continue;
        }
        let kind = if m.num == -1 && ty == 4 {
            -1
        } else if m.num < 3 {
            m.num + 1
        } else {
            m.num + 2
        };
        let ms = |s: f64| (s * 1000.0 + 0.5) as i64 as u32;
        let cue = XmlCue {
            kind,
            start_ms: ms(start),
            end_ms: if ty == 4 { ms(end) } else { 0 },
            name: m.name.clone().unwrap_or_default(),
            rgb: if m.num >= 0 { m.rgb } else { None },
        };
        if cue.is_hot() {
            cues.retain(|c| c.kind != cue.kind);
        } else if cues
            .iter()
            .any(|c| !c.is_hot() && c.start_ms == cue.start_ms && c.end_ms == cue.end_ms)
        {
            continue;
        }
        cues.push(cue);
    }
    cues
}

/// The pad colour code (1–64) nearest to `rgb` in `palette` (index 1–64; Euclidean distance over
/// R, G, B; the first of equally near codes wins), as `getTrackCueInfo` picks a hot cue's colour
/// with `djplay::getPadColor`.
pub fn nearest_pad_color(rgb: [u8; 3], palette: &[[u8; 3]]) -> u8 {
    let mut best = (f64::MAX, 0u8);
    for (code, p) in palette.iter().enumerate().skip(1).take(64) {
        let d = rgb
            .iter()
            .zip(p)
            .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
            .sum::<f64>()
            .sqrt();
        if d < best.0 {
            best = (d, code as u8);
        }
    }
    best.1
}

/// `Colour` → `djmdColor` ID 1–8 (`analyzeTrack` compares the attribute text, ignoring case, with
/// `0xFF007F` … `0x660099`); anything else is no colour.
pub fn colour_id_from_text(colour: &str) -> Option<u8> {
    const COLOURS: [&str; 8] = [
        "0xFF007F", "0xFF0000", "0xFFA500", "0xFFFF00", "0x00FF00", "0x25FDE9", "0x0000FF",
        "0x660099",
    ];
    COLOURS
        .iter()
        .position(|c| c.eq_ignore_ascii_case(colour))
        .map(|i| i as u8 + 1)
}

/// `AverageBpm` → BPM × 100 as `analyzeTrack` stores it: `(int)(long)(bpm × 100)` (truncated).
pub fn bpm_x100_from_xml(bpm: f64) -> u32 {
    (bpm * 100.0) as i64 as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Extra;

    fn tempo(inizio: f64, bpm: f64, battito: u32) -> Tempo {
        Tempo {
            inizio,
            bpm,
            metro: "4/4".into(),
            battito,
            extra: Extra::default(),
        }
    }

    fn mark(ty: u8, start: f64, end: Option<f64>, num: i32) -> PositionMark {
        PositionMark {
            name: Some(String::new()),
            mark_type: ty,
            start,
            end,
            num,
            rgb: None,
            extra: Extra::default(),
        }
    }

    #[test]
    fn one_tempo_fills_the_track() {
        let beats = beats_from_tempos(3.0, &[tempo(0.019, 120.0, 3)]);
        let times: Vec<u32> = beats.iter().map(|b| b.time_ms).collect();
        assert_eq!(times, [19, 519, 1019, 1519, 2019, 2519]);
        let bars: Vec<u16> = beats.iter().map(|b| b.beat_in_bar).collect();
        assert_eq!(bars, [3, 4, 1, 2, 3, 4]);
        assert!(beats.iter().all(|b| b.tempo_x100 == 12000));
    }

    #[test]
    fn second_tempo_cuts_the_fill_half_a_beat_early() {
        // 120 BPM from 0, 100 BPM from 1.2 s: 0.5 is filled, 1.0 is not (within half a beat of
        // the 1.2 entry); 100 BPM then fills 1.8.
        let beats = beats_from_tempos(2.0, &[tempo(0.0, 120.0, 1), tempo(1.2, 100.0, 4)]);
        let got: Vec<(u32, u16, u16)> = beats
            .iter()
            .map(|b| (b.time_ms, b.tempo_x100, b.beat_in_bar))
            .collect();
        assert_eq!(
            got,
            [
                (0, 12000, 1),
                (500, 12000, 2),
                (1200, 10000, 4),
                (1800, 10000, 1)
            ]
        );
    }

    #[test]
    fn bad_tempos_are_skipped() {
        assert!(beats_from_tempos(10.0, &[tempo(1.0, 0.0, 1)]).is_empty());
        // An entry past TotalTime is not a beat, but its tempo still drives the end fill from 0
        // (the attributes are taken before the check).
        let past = beats_from_tempos(10.0, &[tempo(11.0, 120.0, 1)]);
        assert_eq!((past[0].time_ms, past[0].beat_in_bar), (500, 2));
        let mut t = tempo(0.0, 120.0, 1);
        t.metro = "x".into();
        assert!(beats_from_tempos(10.0, &[t]).is_empty());
        assert_eq!(metro_numerator("3/4"), 3);
        assert_eq!(metro_numerator(" 7/8"), 7);
    }

    #[test]
    fn marks_become_cues() {
        let mut hot_a = mark(0, 1.0, None, 0);
        hot_a.rgb = Some([255, 0, 0]);
        let marks = [
            mark(0, 0.5, None, -1),      // memory cue
            mark(4, 2.0, Some(4.0), -1), // memory loop
            hot_a,                       // A
            mark(4, 3.0, Some(3.5), 3),  // D loop
            mark(0, 9.0, None, 7),       // H
            mark(1, 1.0, None, 1),       // fade-in: skipped
            mark(0, 100.0, None, 1),     // past TotalTime: skipped
            mark(0, 1.0, None, 8),       // Num 8: skipped
            mark(0, 0.5, None, -1),      // duplicate memory cue: dropped
            mark(0, 5.0, None, 0),       // A again: replaces the first A
        ];
        let cues = cues_from_marks(10.0, &marks);
        let got: Vec<(i32, u32, u32)> = cues
            .iter()
            .map(|c| (c.kind, c.start_ms, c.end_ms))
            .collect();
        assert_eq!(
            got,
            [
                (0, 500, 0),
                (-1, 2000, 4000),
                (5, 3000, 3500),
                (9, 9000, 0),
                (1, 5000, 0)
            ]
        );
        assert_eq!(cues[0].rgb, None);
    }

    #[test]
    fn colours_bpm_and_palette() {
        assert_eq!(colour_id_from_text("0xff007f"), Some(1));
        assert_eq!(colour_id_from_text("0x660099"), Some(8));
        assert_eq!(colour_id_from_text("0x123456"), None);
        assert_eq!(bpm_x100_from_xml(127.999), 12799);
        let palette = [[0, 0, 0], [0, 0, 255], [255, 0, 0], [250, 0, 0]];
        assert_eq!(nearest_pad_color([254, 1, 1], &palette), 2);
        assert_eq!(nearest_pad_color([249, 0, 0], &palette), 3);
        assert_eq!(nearest_pad_color([0, 0, 200], &palette), 1);
    }
}
