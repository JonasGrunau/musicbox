//! Stage D's per-phrase-candidate measurements: `BlockAverage`, `UpSimpleloopAna`, … A block
//! list holds 1-based beat indices where candidate phrases start (`analysis-phrase.md` § 7.3).

use super::mat;

/// `MatArrayFloat::sum(i, j)`.
pub(super) fn sum_ij(y: &[f64], i: i32, j: i32) -> f64 {
    let n = y.len() as i32;
    if i < 0 || i > j || i >= n || j < 0 || j >= n {
        return 0.0;
    }
    mat::sum(&y[i as usize..=j as usize])
}

/// `BlockAverage(blocks, level, peaks, bar, _, quarter_peaks)`: per block the mean of `level`
/// (from the downbeat) and of `peaks`, leaving out the block's last `4·min(⌊0.045·len + ½⌋, 4)`
/// beats; and the mean quarter peak per beat. `None` where a window turns negative.
pub struct BlockMeans {
    pub level: Vec<f64>,
    pub peak: Vec<f64>,
    pub beat_peak: Vec<f64>,
}

pub fn block_average(
    blocks: &[i32],
    level: &[f64],
    peaks: &[f64],
    bar: i32,
    quarters: &[f64],
) -> Option<BlockMeans> {
    let lim = (level.len().min(peaks.len())) as i32;
    let nb = blocks.len();
    let mut cut = 0usize;
    for &v in blocks.iter().rev() {
        if v <= lim {
            break;
        }
        cut += 1;
    }
    let m = nb - cut;
    let mut b: Vec<i32> = blocks[..m].to_vec();
    b.push(lim + 1);
    let mut out_level = vec![0.0f64; nb];
    let mut out_peak = vec![0.0f64; nb];
    let base = bar - 1;
    let mut rel = vec![0i32; m + 1];
    rel[0] = (b[0] - base).max(1);
    for i in 1..=m {
        rel[i] = b[i] - base;
    }
    for j in 0..m {
        let (s, e) = (rel[j], rel[j + 1]);
        let t = (f64::from(e - s).mul_add(0.045, 0.5) as i32).min(4);
        let n1 = e - s - 4 * t;
        let n2 = b[j + 1] - (4 * t + b[j]);
        if n1 < 0 || n2 < 0 {
            return None;
        }
        let w1 = window(level, s - 1, n1);
        let w2 = window(peaks, b[j] - 1, n2);
        out_peak[j] = if n2 > 0 {
            mat::sum(&w2) / f64::from(n2)
        } else {
            0.0
        };
        out_level[j] = if n1 > 0 {
            mat::sum(&w1) / f64::from(n1)
        } else {
            0.0
        };
    }
    let nq = quarters.len() as i32;
    let beats = (nq / 4).max(0) as usize;
    let mut beat_peak = vec![0.0f64; beats];
    if nq > 3 {
        for (k, v) in beat_peak.iter_mut().enumerate() {
            *v = sum_ij(quarters, 4 * k as i32, 4 * k as i32 + 3) * 0.25;
        }
    }
    Some(BlockMeans {
        level: out_level,
        peak: out_peak,
        beat_peak,
    })
}

/// `y[lo .. lo + n]` as `link_from` sees it (values past the end read as 0).
fn window(y: &[f64], lo: i32, n: i32) -> Vec<f64> {
    (0..n)
        .map(|k| y.get((lo + k) as usize).copied().unwrap_or(0.0))
        .collect()
}

/// `UpSimpleloopAna(blocks, labels, low_band, high_band, block_level)`: for "verse" blocks
/// (labels 30 and 50) the rise of the high band from its quietest 8 beats (first half) to its
/// loudest 8 beats (second half); a steep rise in a quiet block relabels it 31 ("up").
/// Returns (start beats of relabelled blocks, peak high-band level per block).
pub fn up_simpleloop(
    blocks: &[i32],
    labels: &mut [i32],
    low: &[f64],
    high: &[f64],
    block_level: &[f64],
) -> (Vec<i32>, Vec<f64>) {
    let n = labels.len().min(blocks.len());
    let mut b = blocks.to_vec();
    b.push(low.len() as i32 + 1);
    let mut starts = vec![0i32; blocks.len()];
    let mut peak = vec![0.0f64; blocks.len()];
    let mean8 = |a: i32, z: i32| sum_ij(high, a, z) * 0.125;
    for j in 0..n {
        if labels[j] != 0x1e && labels[j] != 0x32 {
            continue;
        }
        let (s, e) = (b[j], b[j + 1]);
        let (diff, range, slope);
        if e <= s + 15 {
            diff = 0.0;
            range = 0.0;
            slope = 0.0;
        } else {
            let mut maxpos = e - 8;
            let mut maxv = 0.0f64;
            let (minv, minpos);
            if e > s + 31 {
                let len = e - s;
                let mid = s + len / 2;
                if mid <= e - 8 {
                    let mut t = mid + 7;
                    let mut cur = 0.0f64;
                    while t != e {
                        let v = mean8(t - 8, t - 1);
                        if v > cur {
                            maxpos = t - 7;
                        }
                        cur = v.max(cur);
                        t += 1;
                    }
                    maxv = cur;
                }
                if len >= 2 {
                    let mut cur = maxv;
                    let mut pos = s;
                    let mut u = s - 1;
                    loop {
                        let v = mean8(u, u + 7);
                        let next = u + 2;
                        u += 1;
                        if v < cur {
                            pos = u;
                        }
                        cur = v.min(cur);
                        if !(next < mid) {
                            break;
                        }
                    }
                    minv = cur;
                    minpos = pos;
                } else {
                    minv = maxv;
                    minpos = s;
                }
            } else if s > e - 8 {
                minv = 0.0;
                minpos = s;
            } else {
                let mut cur = 0.0f64;
                let mut t = s + 7;
                while t != e {
                    let v = mean8(t - 8, t - 1);
                    if v > cur {
                        maxpos = t - 7;
                    }
                    cur = v.max(cur);
                    t += 1;
                }
                maxv = cur;
                let mut lo = maxv;
                let mut pos = s;
                let mut t = s + 7;
                while t != e {
                    let v = mean8(t - 8, t - 1);
                    if v < lo {
                        pos = t - 7;
                    }
                    lo = v.min(lo);
                    t += 1;
                }
                minv = lo;
                minpos = pos;
            }
            peak[j] = maxv;
            let a1 = sum_ij(low, maxpos - 1, maxpos + 6);
            let a2 = sum_ij(low, minpos - 1, minpos + 6);
            diff = (a1 - a2) * 0.125;
            range = maxv - minv;
            slope = if maxpos - minpos != 0 {
                range / f64::from(maxpos - minpos)
            } else {
                0.0
            };
        }
        if !(block_level.get(j).copied().unwrap_or(0.0) < 0.16) {
            continue;
        }
        let mark = (diff < 10.0 && range > 85.0 && slope < 22.0 && slope > 3.5)
            || ((range - diff) > 150.0 && range > 85.0 && slope < 22.0)
            || (range > 85.0 && !(slope >= 22.0) && !(slope <= 6.0));
        if mark {
            labels[j] = 0x1f;
            starts[j] = s;
        }
    }
    (starts, peak)
}

/// The four per-block outputs of `Up4_8_16Ana` (+ the relabelled list).
#[derive(Debug, Clone, Default)]
pub struct UpOut {
    pub labels: Vec<i32>,
    pub a: Vec<i32>,
    pub b: Vec<i32>,
    pub c: Vec<i32>,
    pub d: Vec<i32>,
}

/// `Up4_8_16Ana(blocks, labels, pattern)`: build-ups. Per verse block, bars whose 16 steps
/// are quarters (4), eighths (8) or rolls (16); a run of eighths turning into a roll marks the
/// block 32 ("up") with its start; the block before each label-40 block is checked for a rising
/// hit count over its last bars (32, or 33 for a weaker rise).
pub fn up_4_8_16(blocks: &[i32], labels: &[i32], pattern: &[u8]) -> UpOut {
    const QUARTER: [u8; 16] = [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0];
    const EIGHTH: [u8; 16] = [1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0];
    let n = blocks.len();
    let mut lab = labels.to_vec();
    let (mut o5, mut o6, mut o7, mut o8) =
        (vec![0i32; n], vec![0i32; n], vec![0i32; n], vec![0i32; n]);
    let pat = |i: i32| pattern.get(i as usize).copied().unwrap_or(0);
    let eq = |at: i32, t: &[u8]| t.iter().enumerate().all(|(k, &v)| pat(at + k as i32) == v);
    let verse = |code: i32| (0..=0x33).contains(&code) && (0xc_0000_c000_0c00u64 >> code) & 1 != 0;
    if n > 1 {
        let (mut l268, mut u25, mut i13) = (0i32, 0i32, 0i32);
        for j in 0..n - 1 {
            let code = labels[j] % 100;
            if !verse(code) {
                continue;
            }
            let (s, e) = (blocks[j], blocks[j + 1]);
            let len = e - s;
            let nb = len / 4;
            let mut d = vec![0i32; (nb + 8).max(0) as usize];
            let i14 = e - 1 - s;
            let mut b7 = false;
            if i14 >= 3 {
                let cnt = nb.max(1);
                let off = 4 * s - 4;
                for k in 0..cnt {
                    let at = off + 16 * k;
                    if eq(at, &QUARTER) {
                        d[k as usize] = 4;
                    }
                    if eq(at, &EIGHTH) {
                        d[k as usize] = 8;
                    } else if k > 1 && d[(k - 1) as usize] == 8 {
                        let roll = if d[(k - 2) as usize] == 8 {
                            eq(at + 2, &[1, 1])
                        } else {
                            eq(at, &[1, 1, 1, 1])
                        };
                        if roll {
                            d[k as usize] = 16;
                        }
                    }
                }
                let mut found8 = false;
                let mut stop = false;
                let get = |k: i32| d.get(k as usize).copied().unwrap_or(0);
                for k in 0..cnt {
                    if get(k) != 8 {
                        continue;
                    }
                    if stop {
                        break;
                    }
                    u25 = k + 1;
                    if k + 1 < nb {
                        if get(k + 1) == 8 {
                            let i12 = if nb <= k + 3 { l268 } else { k + 3 };
                            let i28 = if nb <= k + 5 { l268 } else { k + 5 };
                            stop = true;
                            let reach = if get(k + 2) == 8 { k + 5 } else { k + 3 };
                            found8 |= reach < nb;
                            l268 = if get(k + 2) == 8 { i28 } else { i12 };
                        } else {
                            b7 |= k + 1 > 1;
                            if k + 1 >= 2 {
                                i13 = k;
                            }
                            stop = true;
                            found8 = true;
                            l268 = k + 2;
                        }
                    } else {
                        stop = true;
                    }
                }
                if found8 && l268 <= nb && get(l268 - 1) == 0x10 {
                    let dd = l268 - u25;
                    let back = if dd == 1 {
                        -8
                    } else if dd < 3 {
                        -16
                    } else {
                        -32
                    };
                    lab[j] = 0x20;
                    let i28 = s + u25 * 4;
                    let i8 = i28 - 4;
                    let i1 = back + i8;
                    let v = if blocks[j] <= i1 { i1 } else { i28 - 8 };
                    o5[j] = v;
                    o6[j] = v;
                    o7[j] = i8;
                    o8[j] = s + (l268 - 1) * 4;
                }
            }
            let get = |k: i32| d.get(k as usize).copied().unwrap_or(0);
            if lab[j] != 0x20 && b7 && i13 > 0 && i14 > 0x22 && get(i13 - 1) == 4 && i13 == nb - 2 {
                lab[j] = 0x20;
                let t = s + (i13 - 1) * 4;
                o5[j] = t;
                o6[j] = t;
                o7[j] = t + 4;
                o8[j] = t + 8;
            }
        }
    }
    let forties: Vec<usize> = (0..labels.len()).filter(|&j| labels[j] == 0x28).collect();
    for &j40 in &forties {
        if j40 == 0 {
            continue;
        }
        let jp = j40 - 1;
        let code = labels[jp] % 100;
        let thr = if code & !1 == 10 { 0.85 } else { 0.9 };
        let s = blocks[jp];
        let end = blocks.get(j40).copied().unwrap_or(s);
        let len = end - s;
        let cond = ((0..=0x33).contains(&code) && (0xc_0000_c000_0000u64 >> code) & 1 != 0)
            || (len > 0x23 && code & !1 == 10);
        if lab[jp] == 0x20 || !cond {
            continue;
        }
        let nb = len / 4;
        let mut c = vec![0i32; nb.max(0) as usize];
        if len > 3 {
            for (k, v) in c.iter_mut().enumerate() {
                let at = 4 * s - 4 + 16 * k as i32;
                *v = (0..16).map(|q| i32::from(pat(at + q))).sum();
            }
        }
        let cf = |i: i32| f64::from(c.get((nb + i) as usize).copied().unwrap_or(0));
        let mut set = |l: i32, a: i32, b: i32, cc: i32, dd: i32| {
            lab[jp] = l;
            o5[jp] = a;
            o6[jp] = b;
            o7[jp] = cc;
            o8[jp] = dd;
        };
        if len & !31 == 32 {
            let a = (((cf(-8) + cf(-7)) + cf(-6)) + cf(-5)) * 0.25;
            let b = (cf(-4) + cf(-3)) * 0.5;
            let cc = thr * cf(-2);
            if a <= b * thr && b <= cc {
                set(0x20, end - 32, end - 32, end - 16, end - 8);
            } else if a < (b + cf(-2)) * 0.5 {
                set(0x21, end - 32, 0, 0, 0);
            }
        } else if len < 64 {
            if len > 15 {
                let a = (cf(-4) + cf(-3)) * 0.5;
                if a <= thr * cf(-2) {
                    set(0x20, end - 16, end - 16, end - 16, end - 8);
                } else if a < cf(-2) {
                    set(0x21, end - 16, 0, 0, 0);
                }
            }
        } else {
            let a = (-16..-8).fold(0.0, |acc, i| acc + cf(i)) * 0.125;
            let b = (((cf(-6) + cf(-5)) + cf(-8)) + cf(-7)) * 0.25;
            let c3 = ((cf(-4) + cf(-3)) + cf(-2)) * 0.33333;
            let cc = c3 * thr;
            if a <= b * thr && b <= cc {
                set(0x20, end - 48, end - 48, end - 32, end - 16);
            } else {
                let w = &c[..(nb - 1).max(0) as usize];
                let mx = w.iter().copied().max().unwrap_or(0);
                let mn = w.iter().copied().min().unwrap_or(0);
                let rise = if mx - mn < 5 {
                    b <= cc
                } else {
                    a < (c3 + b) * 0.5
                };
                if rise {
                    set(0x21, end - 32, 0, 0, 0);
                }
            }
        }
    }
    if n > 1 {
        for j in 0..n - 1 {
            if (labels[j] % 100) & !1 == 10 && lab[j] & !1 == 0x20 {
                lab[j] = labels[j];
            }
        }
    }
    UpOut {
        labels: lab,
        a: o5,
        b: o6,
        c: o7,
        d: o8,
    }
}
