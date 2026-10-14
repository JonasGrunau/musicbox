//! Stage D's last label passes (`analysis-phrase.md` § 7): `FillAdjustAna`,
//! `FeatureLastAdjust` and the conversion into the result arrays.

use super::mat;
use super::verse::get;

fn is_hook(v: i32) -> bool {
    (40..42).contains(&v) || (440..442).contains(&v)
}

/// Drops a block's fill mark (110 → 10, 330–333 → 30–33, 440/441 → 40/41).
fn demote(v: i32) -> i32 {
    match v {
        110 | 111 => v - 100,
        330..=333 => v - 300,
        440 | 441 => v - 400,
        _ => v,
    }
}

/// `FillAdjustAna(blocks, labels, fills, count, level, quarters, out)` @0x1000c137c: a fill
/// whose block ends in near silence (level under 0.15 on three of the last four beats and the
/// quarter-beat energy under 0.04) is no fill: its mark is dropped (101+ → −100, 301+ → −300,
/// 401+ → −400) and its start cleared, unless the beats before the fill are loud (over 0.4 on
/// six of eight), which flags `out` instead.
pub fn fill_adjust_ana(blocks: &[i32], labels: &mut [i32], fills: &mut [i32], count: i32, level: &[f64], quarters: &[f64], out: &mut [i32]) {
    let n = level.len();
    let mut q = vec![0.0f64; n];
    for (i, v) in q.iter_mut().enumerate() {
        for k in 0..4 {
            *v += quarters.get(4 * i + k).copied().unwrap_or(0.0);
        }
    }
    let ml = mat::max_val(level);
    let mq = mat::max_val(&q);
    let sl = if ml != 0.0 { 1.0 / ml } else { 333.3333333333333 };
    let sq = if mq != 0.0 { 1.0 / mq } else { 333.3333333333333 };
    let a: Vec<f64> = level.iter().map(|&v| v * sl).collect();
    let c: Vec<f64> = q.iter().map(|&v| v * sq).collect();
    if count < 2 {
        return;
    }
    let m = (count - 1) as usize;
    let mut kind = vec![0i32; m];
    for t in 0..m {
        let s = fills[t];
        let b = blocks.get(t + 1).copied().unwrap_or(0);
        if s == 0 || b - s <= 2 {
            continue;
        }
        let quiet = [3, 2, 4, 5].iter().filter(|&&k| get(&a, b - k) < 0.15).count();
        if quiet > 2 && get(&c, b - 3) < 0.04 && get(&c, b - 4) < 0.04 && (get(&c, b - 2) < 0.04 || get(&c, b - 5) < 0.04) {
            kind[t] = 1;
            if s > 8 && (2..=9).filter(|&k| 0.4 < get(&a, s - k)).count() > 5 {
                kind[t] = 2;
                out[t] = 1;
            }
        }
    }
    for t in 0..m {
        if kind[t] == 1 {
            fills[t] = 0;
            let v = labels[t];
            labels[t] = if v > 400 {
                v - 400
            } else if v > 300 {
                v - 300
            } else if v > 100 {
                v - 100
            } else {
                v
            };
        }
    }
}

/// `FeatureLastAdjust(blocks, labels, fills, ends, mood, level)` @0x101075aec: short hooks
/// between non-hooks become intro/up/outro; for mood 1 the hook, up and fill marks are
/// smoothed against the neighbours, short blocks lose their fill, and fills start at most
/// four beats before their block's end.
pub fn feature_last_adjust(blocks: &[i32], labels: &mut [i32], fills: &mut [i32], ends: &[i32], mood: i32, level: &[f64]) {
    if labels.is_empty() || fills.is_empty() || level.is_empty() {
        return;
    }
    let l = labels.len();
    if l < 2 {
        return;
    }
    let mut b = blocks.to_vec();
    b.push(ends.last().copied().unwrap_or(0));
    let bi = |i: usize| b.get(i).copied().unwrap_or(0);
    let lab = labels;
    // Block 1.
    if bi(2) - bi(1) < 9 {
        let x = lab[1];
        let mut new = x;
        if is_hook(x) {
            let p = lab[0];
            let c = !is_hook(p);
            if l == 2 {
                if c {
                    new = if (x & !1) == 40 { 60 } else { 61 };
                }
            } else if !is_hook(lab[2]) && c {
                new = match x {
                    40 => 10,
                    41 => 11,
                    440 => 110,
                    441 => 111,
                    _ => x,
                };
                if p == 50 {
                    new = p;
                }
                if (330..334).contains(&p) || (30..34).contains(&p) {
                    new = if x == 40 { 30 } else { 330 };
                }
            }
        }
        lab[1] = new;
    }
    // Blocks 2..
    for t in 2..l {
        if bi(t + 1) - bi(t) >= 9 {
            continue;
        }
        let x = lab[t];
        let mut new = x;
        if is_hook(x) {
            let p = lab[t - 1];
            let last = t == l - 1;
            let mut c = !is_hook(p) as i32;
            if !last && !is_hook(lab[t + 1]) {
                c += 1;
            }
            if last && c == 1 {
                new = if (x & !1) == 40 { 60 } else { 61 };
            } else if !last && c == 2 {
                new = if p == 50 { p } else { x };
                if (330..334).contains(&p) || (30..34).contains(&p) {
                    new = if x == 40 { 30 } else { 330 };
                }
            }
        }
        lab[t] = new;
    }
    if mood != 1 {
        return;
    }
    for t in 1..l {
        let x = lab[t];
        let mut new = x;
        if bi(t + 1) - bi(t) < 32 && (x == 440 || x == 40) {
            let four = |v: i32| v == 441 || v == 41;
            if four(lab[t - 1]) {
                new = if x == 40 { 41 } else { 441 };
            }
            if t != l - 1 && four(lab[t + 1]) {
                new = if x == 40 { 41 } else { 441 };
            }
        }
        lab[t] = new;
    }
    for u in 0..l - 1 {
        let s = bi(u + 1);
        let d = s - bi(u);
        let y = lab[u + 1];
        let short = if bi(u + 2) - s < 5 {
            if is_hook(y) && d > 4 {
                u == l - 2 || !is_hook(lab[u + 2])
            } else {
                true
            }
        } else {
            d < 5
        };
        if y == 50 || y == 60 || short {
            fills[u] = 0;
            lab[u] = demote(lab[u]);
        }
    }
    if l > 3 {
        for t in 2..l - 1 {
            if bi(t + 1) - bi(t) < 13 {
                let p = lab[t - 1];
                if p == 331 || p == 31 {
                    if lab[t] == 330 {
                        lab[t] = 331;
                    } else if lab[t] == 30 {
                        lab[t] = 31;
                    }
                }
            }
        }
    }
    if l > 2 {
        for t in 1..l - 1 {
            if bi(t + 1) - bi(t) > 68 {
                if lab[t] == 31 {
                    lab[t] = 30;
                } else if lab[t] == 331 {
                    lab[t] = 330;
                }
            }
        }
    }
    if l >= 3 {
        if (31..34).contains(&lab[l - 2]) {
            lab[l - 2] = 30;
        }
        if l > 4 {
            if !(1..l - 1).any(|t| is_hook(lab[t])) {
                for t in 2..l - 2 {
                    let p = lab[t - 1];
                    if ((331..334).contains(&p) || (31..33).contains(&p)) && lab[t + 1] == 50 {
                        lab[t] = if lab[l - 1] == 61 { 41 } else { 40 };
                    }
                }
            }
            for t in 1..l - 1 {
                let x = lab[t];
                let v = level.get(t).copied().unwrap_or(0.0);
                if (x & !1) == 332 {
                    if 0.28 <= v {
                        lab[t] = 330;
                    }
                } else if (x & !1) == 32 && 0.28 <= v {
                    lab[t] = 30;
                }
            }
        }
    }
    for u in 0..l - 1 {
        let e = bi(u + 1);
        if fills[u] != 0 && e - fills[u] > 4 {
            fills[u] = e - 4;
        }
    }
}

/// `SongStructureAnalyzeResult` (0x78 bytes, `analysis-phrase.md` § 8.1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhraseResult {
    pub count: i32,
    /// First downbeat in ms (not written to PSSI).
    pub fb: i32,
    pub mood: i32,
    /// Bar of the first downbeat (not written to PSSI).
    pub bar: i32,
    pub bank: i32,
    /// +0x14, PSSI byte 0x1d.
    pub unk_14: i32,
    pub end_beat: i32,
    /// `cols[k]` is PSSI word `k + 1` of each phrase: start beat, kind, … fill, fill start.
    pub cols: [Vec<i32>; 11],
}

impl PhraseResult {
    /// Whether rekordbox stores the result as PSSI: `notifySongStruct` @0x100159fac clears the
    /// section for a result without phrases or with a negative first start, and
    /// `set_pmai_ext_song_struct_data` @0x100f26a7c refuses a count, end beat or bar of 2¹⁶ or
    /// more.
    pub fn stored(&self) -> bool {
        self.count > 0
            && self.cols.iter().all(|c| !c.is_empty())
            && self.cols[0][0] >= 0
            && self.count < 0x10000
            && self.end_beat < 0x10000
            && self.bar < 0x10000
    }

    /// The PSSI entries as `set_pmai_ext_song_struct_data` builds them: word 0 the 1-based
    /// index, words 1–11 the arrays, each cut to 16 bits.
    pub fn rows(&self) -> Vec<[u16; 12]> {
        (0..self.count.max(0) as usize)
            .map(|i| {
                let mut w = [0u16; 12];
                w[0] = (i + 1) as u16;
                for (k, col) in self.cols.iter().enumerate() {
                    w[k + 1] = col.get(i).copied().unwrap_or(0) as u16;
                }
                w
            })
            .collect()
    }
}

/// Kinds handed out to the mood-2/3 section labels (61, 62, …) in order of appearance.
const MELO_KINDS: [i32; 6] = [7, 2, 3, 4, 5, 6];

/// `DataFortmatConvert(mood, labels, p3, up_a, up_b, up_c, up_d, result, keep)` @0x101840ab8:
/// turns the block labels into PSSI kinds and flags. Mood 1 splits a label into its digits
/// (hundreds: a fill at the end; tens: the section; units: the variant); moods 2/3 map
/// 10/40/50/60 to 1/9/8/10 and number the other sections. `keep` (fills the
/// `FillAdjustAna` kept) is cleared for labels under 100.
#[allow(clippy::too_many_arguments)]
pub fn data_format_convert(
    mood: i32,
    labels: &[i32],
    p3: i32,
    up_a: &[i32],
    up_b: &[i32],
    up_c: &[i32],
    up_d: &[i32],
    res: &mut PhraseResult,
    keep: &mut [i32],
) {
    if labels.is_empty() || up_a.is_empty() || up_b.is_empty() || up_c.is_empty() || up_d.is_empty() {
        return;
    }
    let n = res.count.max(0) as usize;
    for c in res.cols.iter_mut() {
        if c.len() < n {
            c.resize(n, 0);
        }
    }
    res.unk_14 = (p3 < 12) as i32;
    let lab = |i: usize| labels.get(i).copied().unwrap_or(0);
    let geti = |v: &[i32], i: usize| v.get(i).copied().unwrap_or(0);
    if mood == 1 {
        for i in 0..n {
            if lab(i) < 100
                && let Some(k) = keep.get_mut(i) {
                    *k = 0;
                }
        }
        for i in 0..n {
            let l = lab(i);
            let lf = l as f64;
            let (major, sub, units) = if l > 99 {
                let h = (lf * 0.01) as i32;
                let r = l - 100 * h;
                let t = (r as f64 * 0.1) as i32;
                (h, t, r - 10 * t)
            } else {
                let t = (lf * 0.1) as i32;
                (t, l - 10 * t, 0)
            };
            let fill = (geti(keep, i) != 1) as i32;
            let c = &mut res.cols;
            match major {
                1 | 4 => {
                    c[1][i] = if major == 1 { 1 } else { 5 };
                    let d = if l >= 101 {
                        c[9][i] = fill;
                        units
                    } else {
                        sub
                    };
                    if d == 1 {
                        c[2][i] = 1;
                    }
                }
                2 => {
                    c[1][i] = 4;
                    if l > 100 {
                        c[9][i] = 1;
                    }
                }
                3 => {
                    c[1][i] = 2;
                    let d = if l >= 101 {
                        c[9][i] = fill;
                        units
                    } else {
                        sub
                    };
                    match d {
                        1 => c[8][i] = 1,
                        2 => {
                            c[3][i] = 1;
                            c[4][i] = 1;
                            c[5][i] = geti(up_a, i);
                            c[6][i] = geti(up_b, i);
                            c[7][i] = geti(up_c, i);
                        }
                        3 => {
                            c[3][i] = 1;
                            c[4][i] = 0;
                            c[5][i] = geti(up_d, i);
                        }
                        _ => {}
                    }
                }
                5 => c[1][i] = 3,
                6 => {
                    c[1][i] = 6;
                    if sub == 1 {
                        c[2][i] = 1;
                    }
                }
                _ => {}
            }
        }
        return;
    }
    let mut seen = vec![0i32; n];
    let mut next = 1usize;
    for i in 0..n {
        let l = lab(i);
        let kind = &mut res.cols[1];
        match l {
            10 => kind[i] = 1,
            40 => kind[i] = 9,
            50 => kind[i] = 8,
            60 => kind[i] = 10,
            _ if l < 61 => {}
            _ => {
                if let Some(j) = seen[..next.min(n)].iter().position(|&s| s == l) {
                    let k = labels.iter().position(|&x| x == seen[j]).unwrap_or(0);
                    kind[i] = kind.get(k).copied().unwrap_or(0);
                } else {
                    kind[i] = MELO_KINDS[next % 6];
                    if let Some(s) = seen.get_mut(next - 1) {
                        *s = l;
                    }
                    next += 1;
                }
            }
        }
    }
}
