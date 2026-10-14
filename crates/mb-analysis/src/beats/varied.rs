//! The front end of `BeatInfoAnalyzer::analyze` @0x10118ff2c on the varied (Dynamic) path
//! (@0x1011900d0 to @0x101191588): the raw beats are grouped by the sensor call that filed them,
//! groups and beats outside steady tempo runs are dropped, and tempo segments are cut where the
//! BPM of an 8-second window changes. Spec: `analysis-beats.md` § 9.

use super::BeatInfo;
use super::sensor::tol;

/// The sensor call that filed a raw beat (`stat >> 8`).
fn call(b: &BeatInfo) -> i32 {
    b.stat >> 8
}

/// `t`, `2t` or `t/2` strictly inside `a ± tol(a)`.
fn fits(t: f32, a: f32) -> bool {
    let tl = tol(a);
    let (lo, hi) = (a - tl, a + tl);
    let within = |x: f32| x > lo && x < hi;
    within(t) || within(t + t) || within(t * 0.5)
}

fn remove_calls(list: &mut Vec<BeatInfo>, from: usize, to: usize) {
    for g in from..to {
        list.retain(|b| call(b) != g as i32);
    }
}

/// Integer BPM bin of a beat length: `(int)(600000/t + 5) / 10`.
fn bin(t: f32) -> i32 {
    (600_000.0 / f64::from(t) + 5.0) as i32 / 10
}

/// Drops the call groups (and beats) outside steady tempo runs. Returns `false` when the list
/// is empty (the analysis stops).
pub(crate) fn filter(list: &mut Vec<BeatInfo>) -> bool {
    if list.is_empty() {
        return false;
    }
    // One tempo per call group: the group's first beat, or a later one fitting the next group.
    let mut ct: Vec<f32> = Vec::new();
    for e in list.iter() {
        let id = call(e);
        if id < 0 {
            continue;
        }
        let id = id as usize;
        if id >= ct.len() {
            ct.resize(id + 1, 0.0);
            ct[id] = e.tempo;
        } else if ct[id] == 0.0
            && let Some(&t2) = ct[id + 1..].iter().find(|&&v| v != 0.0)
            && fits(e.tempo, t2)
        {
            ct[id] = e.tempo;
        }
    }
    // Leading groups without a tempo go.
    let len = ct.len();
    let mut s = 0usize;
    while s < len && ct[s] == 0.0 {
        remove_calls(list, s, s + 1);
        s += 1;
    }
    if s + 1 < len {
        runs(list, &ct, s);
    }
    true
}

/// The group runs: a group that fits the run's tempo extends it; runs of up to two groups, and
/// runs of up to eight followed by a return to the previous tempo, are deleted.
fn runs(list: &mut Vec<BeatInfo>, ct: &[f32], mut s: usize) {
    let len = ct.len();
    let (mut c, mut pc) = (0i32, 0i32);
    let mut a = ct[s];
    let mut pa = 0.0f32;
    let mut j = s + 1;
    loop {
        let t = ct[j];
        if t == 0.0 {
            remove_calls(list, j, j + 1);
            j += 1;
            if j >= len {
                break;
            }
            continue;
        }
        if fits(t, a) {
            j += 1;
            c += 1;
            a = t;
            if j >= len {
                break;
            }
            continue;
        }
        if c > 1 && (c > 7 || !fits(t, pa)) {
            // The run stands: it becomes the previous one.
            s = j;
            pa = a;
            pc = c;
            c = 0;
            a = t;
            j += 1;
            if j >= len {
                break;
            }
            continue;
        }
        // A short run (or one returning to the previous tempo) goes.
        let x0 = if s < j {
            remove_calls(list, s, j);
            j
        } else {
            s
        };
        s = x0;
        if pc != 0 {
            a = pa;
            c = pc;
        } else {
            c = 0;
            a = ct[j];
        }
    }
    if pc != 0 && c <= 0 && s < j {
        remove_calls(list, s, j);
    }
}

/// Drops beats outside runs of more than eight beats of fitting tempo (32 and more always
/// stand; 9–32 unless the next beat returns to the previous run's tempo).
pub(crate) fn runs8(list: &mut Vec<BeatInfo>) {
    if list.len() < 2 {
        return;
    }
    let mut s = 0usize;
    let mut cur = 1usize;
    let (mut c, mut pc) = (0i32, 0i32);
    let mut a = list[0].tempo;
    let mut pa = 0.0f32;
    loop {
        let t = list[cur].tempo;
        let next_a;
        if fits(t, a) {
            cur += 1;
            c += 1;
            next_a = t;
        } else if c <= 7 || (c <= 31 && fits(t, pa)) {
            list.drain(s..cur);
            cur = s;
            if pc != 0 {
                next_a = pa;
                c = pc;
            } else {
                c = 0;
                next_a = list.get(s).map_or(0.0, |b| b.tempo);
            }
        } else {
            s = cur;
            cur += 1;
            pa = a;
            pc = c;
            c = 0;
            next_a = t;
        }
        a = next_a;
        if cur >= list.len() {
            break;
        }
    }
    if pc != 0 && c <= 7 && s != cur {
        list.truncate(s);
    }
}

/// The tempo segments: the integer BPM of each 8 s window of beats (`8·rate` samples from a
/// beat); where it changes, a segment starts at the first beat whose own BPM matches the
/// window's, its half or double (within the tolerance), taking the tempo of the first beat with
/// the window's BPM.
pub(crate) fn segments(rate: i32, list: &[BeatInfo]) -> Vec<BeatInfo> {
    let span = rate << 3;
    let n = list.len() as i32;
    let bins: Vec<i32> = list.iter().map(|b| bin(b.tempo)).collect();
    let first_from = |from: i32, b: i32| -> i32 {
        (from.max(0)..n).find(|&j| bins[j as usize] == b).unwrap_or(-1)
    };
    let mut segs = Vec::new();
    let mut i = 0i32;
    let mut win_t = 0.0f32;
    'outer: loop {
        let prev_t = win_t;
        let bpm = loop {
            if i >= n {
                break 'outer;
            }
            let mut hist = [0i32; 400];
            if let Some(h) = hist.get_mut(bins[i as usize] as usize) {
                *h += 1;
            }
            let p0 = list[i as usize].pos;
            for e in &list[i as usize + 1..] {
                if e.pos.wrapping_sub(p0) > span {
                    break;
                }
                let v = (600_000.0 / f64::from(e.tempo) + 5.0) as i32;
                if (v.wrapping_add(9) as u32) < 4009 {
                    hist[((v as i16) / 10) as u16 as usize] += 1;
                }
            }
            let mut best = -1i32;
            let mut most = 0;
            for (k, &h) in hist.iter().enumerate() {
                if h > most {
                    most = h;
                    best = k as i32;
                }
            }
            if best >= 0 {
                break best;
            }
            i += 1;
        };
        win_t = 60000.0 / bpm as f32;
        if fits(win_t, prev_t) {
            i += 1;
            continue;
        }
        let tempo_at = if bins[i as usize] == bpm { i } else { first_from(i + 1, bpm) };
        let tl = tol(win_t);
        let (up, down) = (tl + 0.5, 0.5 - tl);
        let mut start = tempo_at;
        for centre in [win_t * 0.5, win_t, win_t + win_t] {
            let lo = (60000.0 / (up + centre)) as i32;
            let hi = (60000.0 / (down + centre)) as i32;
            if lo < hi {
                for b in lo..hi {
                    let j = first_from(i, b);
                    if start > j && j != -1 {
                        start = j;
                    }
                }
            }
        }
        let (Some(at), Some(t)) = (list.get(start as usize), list.get(tempo_at as usize)) else {
            break;
        };
        segs.push(BeatInfo { tempo: t.tempo, ..*at });
        i = start + 1;
    }
    segs
}
