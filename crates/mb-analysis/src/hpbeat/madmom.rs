//! rekordbox's madmom port (`madmom::analyze` @0x10230a820 / @0x102307f8c): the beat/downbeat
//! network on the mono mix, the bar HMM, and the beat list handed to BALib's Focus pass.
//! Spec: `analysis-beats-hp.md` § 3–5.

use super::hmm::{FPS, Hmm};
use mb_nn::{Session, Tensor};

const INPUT: &str = "serving_default_fltp:0";
const OUTPUT: &str = "StatefulPartitionedCall:1";
/// `downbeat_tracking`'s activation threshold.
const THRESHOLD: f32 = 0.05;
/// Float epsilon of the gap tests in the post-processing.
const EPS: f32 = 1.192_092_9e-7;

/// One beat of `madmom::analyze`'s inner result: time (s at 44.1 kHz) and beat in bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawBeat {
    pub time: f32,
    pub num_bar: i32,
}

/// One entry of the JSON beat list (`get_beat_list` @0x102305908).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListBeat {
    /// Sample position.
    pub pos: i32,
    /// Beat in bar, 1–4.
    pub num_bar: i32,
    /// BPM × 100 of the interval to the next beat.
    pub bpm: i32,
}

/// The result `madmom::analyze` returns to `highPrecisionBeatAnalyze` (JSON `bpm`/`beatList`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeatList {
    pub bpm: i32,
    pub beats: Vec<ListBeat>,
}

/// Runs the network on mono samples: `[frames][beat, downbeat]`.
pub fn activations(model: &Session, mono: &[f32]) -> mb_nn::Result<Vec<[f32; 2]>> {
    let input = Tensor { dims: vec![1, mono.len() as i64], data: mono.to_vec() };
    let out = model.run(&[(INPUT, &input)], &[OUTPUT])?;
    let t = &out[0];
    Ok(t.data.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
}

/// `downbeat_tracking` @0x102300d6c with rekordbox's arguments (4 beats per bar, 70–180 BPM,
/// 60 tempi, λ 100/16, threshold 0.05, correction on, 100 fps): `(time s, beat in bar)` rows.
pub fn downbeat_tracking(act: &[[f32; 2]]) -> Vec<(f32, f32)> {
    // threshold_activations @0x102302f00: the rows from the first to the last with any value
    // ≥ the threshold.
    let rows: Vec<usize> = act
        .iter()
        .enumerate()
        .filter(|(_, r)| r[0] >= THRESHOLD || r[1] >= THRESHOLD)
        .map(|(i, _)| i)
        .collect();
    let (first, last) = match (rows.first(), rows.last()) {
        (Some(&f), Some(&l)) => (f, (l + 1).min(act.len())),
        _ => return Vec::new(),
    };
    if first == 0 && last == 0 {
        return Vec::new();
    }
    let act = &act[first..last];
    if !act.iter().any(|r| r[0] != 0.0 || r[1] != 0.0) {
        return Vec::new();
    }
    let hmm = Hmm::get();
    let (path, _) = hmm.viterbi(act);
    if path.is_empty() {
        return Vec::new();
    }
    let beat_numbers: Vec<i32> = path.iter().map(|&s| hmm.state_positions[s as usize] as i32 + 1).collect();
    // correct = true: one beat per run of beat-range states, at the strongest activation.
    let in_range: Vec<bool> = path.iter().map(|&s| hmm.om_pointers[s as usize] >= 1).collect();
    let mut idx: Vec<usize> = Vec::new();
    for i in 1..in_range.len() {
        if in_range[i] != in_range[i - 1] {
            idx.push(i);
        }
    }
    // The port returns no beats when the path never enters or leaves the beat range (madmom
    // would still emit one beat for a path inside it throughout).
    if idx.is_empty() {
        return Vec::new();
    }
    if in_range[0] {
        idx.insert(0, 0);
    }
    if in_range[in_range.len() - 1] {
        idx.push(in_range.len());
    }
    if !idx.iter().any(|&i| i != 0) {
        return Vec::new();
    }
    let mut beats = Vec::new();
    for pair in idx.chunks_exact(2) {
        let (left, right) = (pair[0], pair[1]);
        // argmax over the flattened [left, right) rows: first maximum
        let mut best = 0usize;
        let mut best_v = f32::NEG_INFINITY;
        for (k, v) in act[left..right].iter().flatten().enumerate() {
            if k == 0 || *v > best_v {
                best_v = *v;
                best = k;
            }
        }
        beats.push(best / 2 + left);
    }
    let inv = 1.0 / FPS;
    beats
        .iter()
        .map(|&b| (((b + first) as f64 * inv) as f32, beat_numbers[b] as f32))
        .collect()
}

/// Mono input of one model run: `(R + L)·0.5` over `[start, start + len)`.
fn chunk(mono: &[f32], start: usize, len: usize) -> &[f32] {
    &mono[start..start + len]
}

/// Appends a later segment's beats (`madmom::analyze` @0x102307f8c): the new beats up to the one
/// closest to the last beat so far (at least the first one) are dropped.
fn merge(acc: &mut Vec<(f32, f32)>, mut new: Vec<(f32, f32)>, offset: f32) {
    if new.is_empty() {
        return;
    }
    for b in &mut new {
        b.0 += offset;
    }
    if acc.is_empty() {
        *acc = new;
        return;
    }
    let last = acc[acc.len() - 1].0;
    let rows = new.len();
    let k = if rows < 2 || (last - new[0].0).abs() <= (last - new[1].0).abs() {
        0
    } else {
        let mut l = 0usize;
        let mut d = (last - new[1].0).abs();
        loop {
            if l == rows - 2 {
                break rows - 1;
            }
            let e = (last - new[l + 2].0).abs();
            l += 1;
            let closer = e < d;
            d = e;
            if !closer {
                break l;
            }
        }
    };
    if rows > k + 1 {
        acc.extend_from_slice(&new[k + 1..]);
    }
}

/// The inner `madmom::analyze` with the hendou (Dynamic) flag: 60 s model runs over the whole
/// `n` samples, the last full chunk split in two when the remainder is under 30 s.
pub fn track_hendou(model: &Session, mono: &[f32], rate: u32, n: usize) -> mb_nn::Result<Vec<RawBeat>> {
    let chunk_len = rate as usize * 60;
    let full = n / chunk_len;
    let mut acc: Vec<(f32, f32)> = Vec::new();
    let run = |start: usize, len: usize| -> mb_nn::Result<Vec<(f32, f32)>> {
        Ok(downbeat_tracking(&activations(model, chunk(mono, start, len))?))
    };
    let (seg2_start, seg2_len, seg3_start, seg3_len);
    if full < 1 {
        (seg2_start, seg2_len, seg3_start, seg3_len) = (0, n, n, 0);
    } else {
        let last_full = (full - 1) * chunk_len;
        let rem = n - full * chunk_len;
        let span = n - last_full;
        if rem > 0 && rem < rate as usize * 30 {
            let half = span >> 1;
            (seg2_start, seg2_len, seg3_start, seg3_len) = (last_full, half, last_full + half, n - (last_full + half));
        } else {
            (seg2_start, seg2_len, seg3_start, seg3_len) = (last_full, chunk_len, full * chunk_len, rem);
        }
        for k in 0..full.saturating_sub(1) {
            let mut r = run(k * chunk_len, chunk_len)?;
            let off = rate as f32 * 0.001_360_544_2 * k as f32;
            for b in &mut r {
                b.0 += off;
            }
            acc.extend(r);
        }
    }
    let inv = 2.267_573_7e-5f32;
    if seg2_len > 0 {
        let r = run(seg2_start, seg2_len)?;
        if seg2_start < 1 {
            acc = r;
        } else {
            merge(&mut acc, r, seg2_start as f32 * inv);
        }
    }
    if seg3_len > 0 {
        let r = run(seg3_start, seg3_len)?;
        if seg3_start < 1 {
            acc = r;
        } else {
            merge(&mut acc, r, seg3_start as f32 * inv);
        }
    }
    Ok(acc.into_iter().map(|(t, b)| RawBeat { time: t, num_bar: b as i32 }).collect())
}

/// The inner `madmom::analyze` without hendou: one model run over `[start, start + n)`.
pub fn track_window(model: &Session, mono: &[f32], start: usize, n: usize) -> mb_nn::Result<Vec<RawBeat>> {
    let r = downbeat_tracking(&activations(model, chunk(mono, start, n))?);
    Ok(r.into_iter().map(|(t, b)| RawBeat { time: t, num_bar: b as i32 }).collect())
}

/// One gap test of the hendou clean-up around a chunk boundary: with `p = t[i−1]`, `c = t[i]`
/// the first beat at or after the boundary, it fills a doubled gap or drops a beat that came
/// too early.
#[allow(clippy::neg_cmp_op_on_partial_ord, reason = "the binary's test: a NaN time counts as at or after the boundary")]
fn fix_boundary(b: &mut Vec<RawBeat>, boundary: f32) {
    let count = b.len();
    let Some(i) = b.iter().position(|x| !(x.time < boundary)) else { return };
    if i < 2 || i + 1 >= count {
        return;
    }
    let (pp, p, c, nx) = (b[i - 2].time, b[i - 1].time, b[i].time, b[i + 1].time);
    let dp = p - pp;
    let d1 = c - p;
    let r1 = if dp <= EPS { 0.0 } else { d1 / dp };
    let r2 = if EPS < d1 { (nx - c) / d1 } else { 0.0 };
    let n1 = (r1 + 0.5) as i32;
    let c2 = n1 > 1 && (1.35..=3.5).contains(&r1);
    if c2 && (0.25..=0.75).contains(&r2) {
        // the gap before c is a multiple of the beat: fill it
        let inv = 1.0 / n1 as f32;
        for k in 1..n1 {
            let t = (d1 * k as f32).mul_add(inv, p);
            b.insert(i - 1 + k as usize, RawBeat { time: t, num_bar: 1 });
        }
        return;
    }
    let n2 = (r2 + 0.5) as i32;
    if n2 > 1 && (0.9..=1.1).contains(&r1) && (1.35..=3.5).contains(&r2) {
        // the gap after c is a multiple: fill it
        let d2 = nx - c;
        let inv = 1.0 / n2 as f32;
        for k in 1..n2 {
            let t = (d2 * k as f32).mul_add(inv, c);
            b.insert(i + k as usize, RawBeat { time: t, num_bar: 1 });
        }
        return;
    }
    if 0.3 <= r1 {
        if r2 < 0.3 {
            b.remove(i + 1);
        }
    } else {
        b.remove(i);
    }
}

/// The hendou clean-up in `madmom::analyze` @0x10230a820 (times already in seconds): fixes the
/// chunk boundaries, then fills every gap of about two or more beats.
fn hendou_fix(b: &mut Vec<RawBeat>, rate: u32, n: usize) {
    let rate_i = rate as i64;
    let win = rate_i * 60;
    let n = n as i64;
    let nwin = if win != 0 { n / win } else { 0 };
    let (base, win_) = if nwin < 1 { (0, n) } else { ((nwin - 1) * win, win) };
    let rem = n - (win_ + base);
    let split = if rem > 0 && rem < rate_i * 30 && nwin != 0 { (n - base) as u64 >> 1 } else { win_ as u64 } as i64;
    if nwin > 1 {
        for w in 0..nwin - 1 {
            if b.is_empty() {
                continue;
            }
            fix_boundary(b, (w + 1) as f32 * 60.0);
        }
    }
    if split > 0 && !b.is_empty() {
        fix_boundary(b, (split + base) as i32 as f32 / rate as f32);
    }
    let count0 = b.len();
    if count0 > 2 {
        for i in 1..count0 - 1 {
            let (a, c) = (b[i - 1].time, b[i].time);
            if i < count0 - 2 {
                let d0 = c - a;
                let d1 = b[i + 1].time - c;
                let r1 = if EPS < d0 { d1 / d0 } else { 0.0 };
                let d2 = b[i + 2].time - b[i + 1].time;
                let r2 = if EPS < d2 { d1 / d2 } else { 0.0 };
                let n = (r1 + 0.5) as i32 as u32;
                if (n as i32) > 0 && ((r2 + 0.5) as i32) > 0 && r1 > 1.8 && r2 > 1.8 && n > 1 {
                    let inv = 1.0 / n as i32 as f32;
                    for k in 2..=n {
                        let t = (d1 * (k - 1) as i32 as f32).mul_add(inv, c);
                        b.insert(i + k as usize - 1, RawBeat { time: t, num_bar: 1 });
                    }
                }
            } else {
                let d0 = c - a;
                if EPS < d0 {
                    let nx = b[i + 1].time;
                    let r = (nx - c) / d0;
                    let n = (r + 0.5) as i32 as u32;
                    if (n as i32) > 0 && r > 1.8 && n > 1 {
                        let inv = 1.0 / n as i32 as f32;
                        for k in 2..=n {
                            let t = ((nx - c) * (k - 1) as i32 as f32).mul_add(inv, c);
                            b.insert(i + k as usize - 1, RawBeat { time: t, num_bar: 1 });
                        }
                    }
                }
            }
        }
    }
}

/// `nc::percentile(v, 50, NONE, "linear")` @0x10230715c.
fn median(v: &[i32]) -> f64 {
    let mut a: Vec<f64> = v.iter().map(|&x| f64::from(x)).collect();
    if a.len() == 1 {
        return a[0];
    }
    a.sort_by(f64::total_cmp);
    let n1 = (a.len() - 1) as f64;
    let p = 50.0 * 0.01;
    let i = ((p * n1).floor() as u32).min(a.len() as u32 - 2) as usize;
    let pi = i as f64 / n1;
    (a[i + 1] - a[i]) * (p - pi) / ((i + 1) as f64 / n1 - pi) + a[i]
}

/// `get_beat_list` @0x102305908 + `get_bpm` @0x102306b34.
pub fn beat_list(beats: &[RawBeat], n: i64, rate: u32) -> BeatList {
    let mut v: Vec<RawBeat> = beats.to_vec();
    if v.len() > 1 {
        // extend backwards to 0 with the first interval
        let d = v[0].time - v[1].time;
        let mut t = v[0].time + d;
        let mut nb = v[0].num_bar;
        while 0.0 <= t {
            nb = if nb > 1 { nb - 1 } else { 4 };
            v.insert(0, RawBeat { time: t, num_bar: nb });
            t += d;
        }
    }
    let mut out: Vec<ListBeat> = Vec::with_capacity(v.len());
    let mut msecs: Vec<i64> = Vec::with_capacity(v.len());
    for (i, b) in v.iter().enumerate() {
        let msec = (b.time * 1000.0f32).round() as i64;
        let mut bpm_f = 0.0f32;
        if i > 0 {
            let prev = msecs[i - 1];
            let f = 60000.0f32 / (msec - prev) as f32;
            out[i - 1].bpm = (f * 100.0f32) as i32;
            bpm_f = f;
            if i == v.len() - 1 {
                bpm_f = out[i - 1].bpm as f32;
            }
        }
        let pos = ((msec * i64::from(rate)) as f32 * 0.001f32) as i32;
        out.push(ListBeat { pos, num_bar: b.num_bar, bpm: bpm_f as i32 });
        msecs.push(msec);
    }
    if out.len() > 1 {
        let last = out[out.len() - 1];
        let step = i64::from(last.pos) - i64::from(out[out.len() - 2].pos);
        let mut pos = i64::from(last.pos) + step;
        let mut nb = last.num_bar;
        while pos < n && step > 0 {
            out.push(ListBeat { pos: pos as i32, num_bar: nb % 4 + 1, bpm: last.bpm });
            nb += 1;
            pos += step;
        }
    }
    let bpm = if out.is_empty() { 0.0 } else { median(&out.iter().map(|b| b.bpm).collect::<Vec<_>>()) };
    BeatList { bpm: bpm as i32, beats: out }
}

/// `madmom::analyze` @0x10230a820: the window, the model runs, the scale to seconds, the hendou
/// clean-up and the beat list. `mono` holds the whole track (`(L + R)·0.5`, analysis rate),
/// `total` = `L − 1`, `offset` = BALib's `GetOffsetCalc`.
pub fn analyze(model: &Session, mono: &[f32], rate: u32, total: usize, offset: i32, hendou: bool) -> mb_nn::Result<BeatList> {
    let sixty = rate as usize * 60;
    let (start, n) = if hendou || total == 0 {
        (0, total)
    } else {
        let (len4, start17) = if sixty < total { (sixty, total - sixty) } else { (total, 0) };
        if (total as u64).wrapping_sub(offset as i64 as u64) <= sixty as u64 { (start17, len4) } else { (offset as usize, sixty) }
    };
    let mut beats = if hendou {
        track_hendou(model, &mono[start..], rate, n)?
    } else {
        track_window(model, mono, start, n)?
    };
    if rate != 44100 && !beats.is_empty() {
        let len = beats.len();
        let vec_n = if len < 5 { 0 } else { len - if len & 3 != 0 { len & 3 } else { 4 } };
        let s = 44100.0f32 / rate as f32;
        let inv = 1.0f32 / rate as f32;
        for (i, b) in beats.iter_mut().enumerate() {
            b.time = if i < vec_n { b.time * s } else { b.time * 44100.0 * inv };
        }
    }
    if hendou {
        hendou_fix(&mut beats, rate, n);
    }
    Ok(beat_list(&beats, n as i64, rate))
}
