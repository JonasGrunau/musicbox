//! `convertToMicrosec`'s BPM-range rebuild (@0x102328b20, `analysis-beats-hp.md` § 6). Without
//! hendou a BPM outside the range doubles (`bpmToDouble` @0x102329060) or halves
//! (`bpmToHalf` @0x10232967c) the grid until it fits; `renumberBeat` @0x1023297fc then numbers
//! the bars and adds a beat at either end where one fits. No fixture track leaves 70–180, so
//! this is ported from the code alone.

use crate::beats::Beat;

/// `bpmRangeList[1]` (BPM × 100): 70–180, the only range musicbox has.
pub const RANGE: (i32, i32) = (7000, 18000);

/// A beat's time in µs (`ms·1000 + sub`).
fn us(b: &Beat) -> i64 {
    i64::from(b.sub_ms) + i64::from(b.time_ms) * 1000
}

fn at(bar: u16, tempo: u16, t: i64) -> Beat {
    let ms = t / 1000;
    Beat {
        beat_in_bar: bar,
        tempo,
        time_ms: ms as u32,
        sub_ms: (t - ms * 1000) as u16,
    }
}

/// `convertToMicrosec` without hendou: `beats` and the Focus BPM × 100 as they leave the range
/// check. `total` is the track's sample count. An out-of-range BPM (or, when it is below 1, the
/// first beat's tempo) is doubled or halved with the grid; an in-range one leaves both as they
/// are, the BPM included.
pub fn rebuild(beats: Vec<Beat>, bpm100: i32, rate: u32, total: u64) -> (Vec<Beat>, i32) {
    let (lo, hi) = RANGE;
    let mut v = if bpm100 < 1 {
        beats.first().map_or(0, |b| i32::from(b.tempo))
    } else {
        bpm100
    };
    if (lo..=hi).contains(&v) || beats.is_empty() || rate == 0 {
        return (beats, bpm100);
    }
    let end = ((total as f64 * 1_000_000.0) / f64::from(rate)) as i64;
    // Each round passes the original beat count on, whatever the list holds by then.
    let n = beats.len();
    let mut list = beats;
    if v < lo {
        loop {
            list = double(&list, n, end);
            v *= 2;
            if v >= lo {
                break;
            }
        }
    }
    if v > hi {
        loop {
            // A later round would read past the halved list; this stops at its end.
            list = half(&list, n, end);
            v /= 2;
            if v <= hi {
                break;
            }
        }
    }
    (list, v)
}

/// `bpmToDouble`: each of the first `n` beats and one halfway to the next (the last one half
/// the previous interval on, or halfway to the end when it is the only beat), all at twice
/// the tempo; beats past the end are dropped.
fn double(list: &[Beat], n: usize, end: i64) -> Vec<Beat> {
    let n = n.min(list.len());
    let first = match list[0].beat_in_bar {
        1 | 3 => 1,
        2 | 4 => 3,
        s => s,
    };
    let mut out = Vec::with_capacity(2 * n + 2);
    for i in 0..n {
        let b = list[i];
        let tempo = b.tempo.wrapping_shl(1);
        out.push(Beat { tempo, ..b });
        let t = us(&b);
        let mut d = if i + 1 < n {
            us(&list[i + 1]) - t
        } else if i == 0 {
            end - t
        } else {
            t - us(&list[i - 1])
        };
        if d < 0 {
            d += 1;
        }
        out.push(at(b.beat_in_bar, tempo, t + (d >> 1)));
    }
    while out.last().is_some_and(|b| us(b) > end) {
        out.pop();
    }
    renumber(&mut out, first, end);
    out
}

/// `bpmToHalf`: every second of the first `n` beats at half the tempo.
fn half(list: &[Beat], n: usize, end: i64) -> Vec<Beat> {
    let n = n.min(list.len());
    let first = match list[0].beat_in_bar {
        1 | 4 => 1,
        2 | 3 => 4,
        s => s,
    };
    let mut out: Vec<Beat> = list[..n]
        .iter()
        .step_by(2)
        .map(|b| Beat {
            tempo: b.tempo >> 1,
            ..*b
        })
        .collect();
    renumber(&mut out, first, end);
    out
}

/// `renumberBeat`: bars counted on from `first`, then one beat a beat length before the first
/// (when it fits after 0) and one after the last (when it fits before `end`). The appended
/// beat's sub-ms part is the last one's plus the interval's, without a carry.
fn renumber(list: &mut Vec<Beat>, first: u16, end: i64) {
    let mut u = i32::from(first) - 1;
    for b in list.iter_mut() {
        u = u % 4 + 1;
        b.beat_in_bar = u as u16;
    }
    match list.len() {
        0 => list.push(at(3, 0, 0)),
        n => {
            let t0 = us(&list[0]);
            let (d, step) = if n == 1 {
                (-t0, t0)
            } else {
                let d = us(&list[1]) - t0;
                (d, -d)
            };
            if d <= t0 {
                let b = list[0];
                let bar = ((i32::from(b.beat_in_bar) + 2) & 3) + 1;
                list.insert(0, at(bar as u16, b.tempo, t0 + step));
            }
        }
    }
    let m = list.len();
    let last = list[m - 1];
    let tl = us(&last);
    if end < tl {
        return;
    }
    let d = if m > 1 { tl - us(&list[m - 2]) } else { tl };
    let room = end - tl;
    if room > 0 && d <= room {
        let q = d / 1000;
        list.push(Beat {
            beat_in_bar: (last.beat_in_bar & 3) + 1,
            tempo: last.tempo,
            time_ms: (i64::from(last.time_ms) + q) as u32,
            sub_ms: (i64::from(last.sub_ms) + d - q * 1000) as u16,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: u32, step_us: i64, tempo: u16) -> Vec<Beat> {
        (0..n)
            .map(|i| at((i % 4 + 1) as u16, tempo, 100_000 + i64::from(i) * step_us))
            .collect()
    }

    #[test]
    fn in_range_is_untouched() {
        let g = grid(8, 500_000, 12000);
        assert_eq!(rebuild(g.clone(), 12000, 44100, 44100 * 10), (g, 12000));
    }

    #[test]
    fn slow_grid_doubles() {
        // 60 BPM over 10 s: the doubled grid keeps a beat every 0.5 s and fills the ends.
        let g = grid(9, 1_000_000, 6000);
        let (d, bpm) = rebuild(g, 6000, 44100, 44100 * 10);
        assert_eq!(bpm, 12000);
        assert!(d.iter().all(|b| b.tempo == 12000));
        assert!(d.windows(2).all(|w| us(&w[1]) - us(&w[0]) == 500_000));
        assert_eq!(us(&d[0]), 100_000);
        assert_eq!(
            d.iter().map(|b| b.beat_in_bar).take(5).collect::<Vec<_>>(),
            [1, 2, 3, 4, 1]
        );
    }

    #[test]
    fn fast_grid_halves() {
        // 200 BPM: every second beat, the bars counted on from the first.
        let g = grid(20, 300_000, 20000);
        let (h, bpm) = rebuild(g, 20000, 44100, 44100 * 10);
        assert_eq!(bpm, 10000);
        assert!(h.iter().all(|b| b.tempo == 10000));
        assert!(h.windows(2).all(|w| us(&w[1]) - us(&w[0]) == 600_000));
        assert_eq!(h[0].beat_in_bar, 1);
    }
}
