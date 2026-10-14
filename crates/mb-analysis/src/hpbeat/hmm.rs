//! The bar-tracking hidden Markov model of rekordbox's downbeat tracker (`madmom::` C++ port in
//! the binary, spec `analysis-beats-hp.md` § 4): a bar state space of 4 beats × the beat
//! intervals for 70–180 BPM at 100 frames/s, exponential tempo transitions, and the RNN
//! observation model.
//!
//! rekordbox does not compute the transition model at run time: `TransitionModel::make_sparse`
//! @0x102311cd0 returns a CSR table that is compiled into the binary. The table equals what the
//! original Python model computes for these parameters, so it is rebuilt here with the same float
//! operations (numpy's `exp`, pairwise row sums and division, scipy's sorted CSR); a test checks it
//! against the table's SHA-256.

use std::sync::OnceLock;

/// Frames per second of the activations.
pub const FPS: f64 = 100.0;
const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 180.0;
const NUM_TEMPI: usize = 60;
const BEATS_PER_BAR: usize = 4;
const TRANSITION_LAMBDA: f64 = 100.0;
const OBSERVATION_LAMBDA: i32 = 16;

/// `BarStateSpace` for one bar length plus the model built on it.
pub struct Hmm {
    /// Position of each state in the bar (beat index + fraction).
    pub state_positions: Vec<f64>,
    /// CSR rows: for state `s`, `prev[pointers[s]..pointers[s + 1]]` are its predecessors.
    pub pointers: Vec<u32>,
    pub prev: Vec<u32>,
    /// Transition probabilities (the compiled-in table's values).
    pub prob: Vec<f64>,
    /// `log(probabilities)`, as `viterbi` takes them.
    pub log_prob: Vec<f64>,
    /// Observation density column per state: 0 no beat, 1 beat, 2 downbeat.
    pub om_pointers: Vec<u8>,
    pub num_states: usize,
}

/// `nc::round` (`frinti`: to nearest, ties to even).
fn round(x: f64) -> f64 {
    x.round_ties_even()
}

/// `BeatStateSpace` @0x10230cc78: the beat intervals (in frames) and the per-state position
/// within a beat and interval.
struct BeatStateSpace {
    intervals: Vec<i32>,
    positions: Vec<f64>,
    state_intervals: Vec<i32>,
    first: Vec<u32>,
    last: Vec<u32>,
}

/// `nc::linspace(start, stop, num, endpoint)` @0x1008b8760: element `i` is
/// `fma((stop − start)·i, 1/div, start)`.
fn linspace(start: f64, stop: f64, num: u32, endpoint: bool) -> Vec<f64> {
    match num {
        0 => return Vec::new(),
        1 => return vec![start],
        _ => {}
    }
    let d = stop - start;
    if endpoint {
        if num == 2 {
            return vec![start, stop];
        }
        let inv = 1.0 / f64::from(num - 1);
        let mut v: Vec<f64> = (0..num).map(|i| (d * f64::from(i)).mul_add(inv, start)).collect();
        v[0] = start;
        v[num as usize - 1] = stop;
        v
    } else {
        if num == 2 {
            return vec![start, start + d * 0.5];
        }
        let inv = 1.0 / f64::from(num);
        let mut v: Vec<f64> = (0..num).map(|i| (d * f64::from(i)).mul_add(inv, start)).collect();
        v[0] = start;
        v
    }
}

impl BeatStateSpace {
    fn new(min_interval: f64, max_interval: f64, num_intervals: usize) -> Self {
        let lo = round(min_interval);
        let hi = round(max_interval) + 1.0;
        let mut intervals: Vec<f64> = Vec::new();
        let mut x = lo;
        while x < hi {
            intervals.push(x);
            x += 1.0;
        }
        if num_intervals != 0 && num_intervals < intervals.len() {
            // log-spaced intervals, approached iteratively (madmom's BeatStateSpace)
            let (l0, l1) = (min_interval.log2(), max_interval.log2());
            let mut k = num_intervals as u32;
            loop {
                let mut v: Vec<f64> = linspace(l0, l1, k, true).into_iter().map(|e| round(e.exp2())).collect();
                v.sort_by(f64::total_cmp);
                v.dedup_by(|b, a| (*a - *b).abs() <= a.abs().min(b.abs()) * f64::EPSILON);
                intervals = v;
                k += 1;
                if intervals.len() >= num_intervals {
                    break;
                }
            }
        }
        let intervals: Vec<i32> = intervals.iter().map(|&v| v as i32).collect();
        let mut positions = Vec::new();
        let mut state_intervals = Vec::new();
        let (mut first, mut last) = (Vec::new(), Vec::new());
        let mut at = 0u32;
        for &i in &intervals {
            first.push(at);
            positions.extend(linspace(0.0, 1.0, i as u32, false));
            state_intervals.extend(std::iter::repeat_n(i, i as usize));
            at += i as u32;
            last.push(at - 1);
        }
        Self { intervals, positions, state_intervals, first, last }
    }
}

/// numpy's pairwise sum of a contiguous float64 row of at most 128 values (the row sums of
/// `exponential_transition`).
fn np_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut s = 0.0;
        for &x in a {
            s += x;
        }
        return s;
    }
    debug_assert!(n <= 128);
    let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
    let full = n - n % 8;
    let mut i = 8;
    while i < full {
        for j in 0..8 {
            r[j] += a[i + j];
        }
        i += 8;
    }
    let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
    for &x in &a[full..] {
        res += x;
    }
    res
}

/// `exponential_transition(from, to, λ)`: row = from interval, column = to interval.
fn exponential_transition(from: &[i32], to: &[i32], lambda: f64) -> Vec<Vec<f64>> {
    from.iter()
        .map(|&f| {
            let mut row: Vec<f64> = to
                .iter()
                .map(|&t| {
                    let ratio = f64::from(t) / f64::from(f);
                    let p = (-lambda * (ratio - 1.0).abs()).exp();
                    if p <= f64::EPSILON { 0.0 } else { p }
                })
                .collect();
            let s = np_sum(&row);
            for p in &mut row {
                *p /= s;
            }
            row
        })
        .collect()
}

impl Hmm {
    fn build() -> Self {
        let min_interval = 60.0 * FPS / MAX_BPM;
        let max_interval = 60.0 * FPS / MIN_BPM;
        let bss = BeatStateSpace::new(min_interval, max_interval, NUM_TEMPI);
        let per_beat = bss.positions.len();
        let num_states = per_beat * BEATS_PER_BAR;
        let mut state_positions = Vec::with_capacity(num_states);
        let mut state_intervals = Vec::with_capacity(num_states);
        let (mut firsts, mut lasts) = (Vec::new(), Vec::new());
        for b in 0..BEATS_PER_BAR {
            state_positions.extend(bss.positions.iter().map(|&p| p + b as f64));
            state_intervals.extend_from_slice(&bss.state_intervals);
            let off = (b * per_beat) as u32;
            firsts.push(bss.first.iter().map(|&s| s + off).collect::<Vec<_>>());
            lasts.push(bss.last.iter().map(|&s| s + off).collect::<Vec<_>>());
        }
        let _ = &bss.intervals;
        // Same-tempo transitions (every state but a beat's first follows its predecessor),
        // then the tempo changes from the last states of beat b−1 to the first states of beat b.
        let is_first: Vec<bool> = {
            let mut v = vec![false; num_states];
            for f in firsts.iter().flatten() {
                v[*f as usize] = true;
            }
            v
        };
        let mut entries: Vec<(u32, u32, f64)> = (0..num_states as u32)
            .filter(|&s| !is_first[s as usize])
            .map(|s| (s, s - 1, 1.0))
            .collect();
        for beat in 0..BEATS_PER_BAR {
            let to = &firsts[beat];
            let from = &lasts[(beat + BEATS_PER_BAR - 1) % BEATS_PER_BAR];
            let fi: Vec<i32> = from.iter().map(|&s| state_intervals[s as usize]).collect();
            let ti: Vec<i32> = to.iter().map(|&s| state_intervals[s as usize]).collect();
            let prob = exponential_transition(&fi, &ti, TRANSITION_LAMBDA);
            for (r, row) in prob.iter().enumerate() {
                for (c, &p) in row.iter().enumerate() {
                    if p != 0.0 {
                        entries.push((to[c], from[r], p));
                    }
                }
            }
        }
        // CSR by state, predecessors ascending (scipy's canonical form).
        entries.sort_by_key(|e| (e.0, e.1));
        let mut pointers = vec![0u32; num_states + 1];
        for e in &entries {
            pointers[e.0 as usize + 1] += 1;
        }
        for i in 0..num_states {
            pointers[i + 1] += pointers[i];
        }
        let prev = entries.iter().map(|e| e.1).collect();
        let prob: Vec<f64> = entries.iter().map(|e| e.2).collect();
        let log_prob = prob.iter().map(|p| p.ln()).collect();
        // RNNDownBeatTrackingObservationModel @0x102316950.
        let border = 1.0 / f64::from(OBSERVATION_LAMBDA);
        let om_pointers = state_positions
            .iter()
            .map(|&p: &f64| {
                if p < border {
                    2
                } else if p % 1.0 < border {
                    1
                } else {
                    0
                }
            })
            .collect();
        Self { state_positions, pointers, prev, prob, log_prob, om_pointers, num_states }
    }

    /// The model for rekordbox's parameters (built once).
    pub fn get() -> &'static Hmm {
        static HMM: OnceLock<Hmm> = OnceLock::new();
        HMM.get_or_init(Hmm::build)
    }

    /// `RNNDownBeatTrackingObservationModel::log_densities` @0x10231725c, in f32.
    fn log_densities(obs: &[[f32; 2]]) -> Vec<[f32; 3]> {
        let scale = 1.0f32 / (OBSERVATION_LAMBDA as f32 - 1.0);
        obs.iter()
            .map(|&[b, d]| [((1.0 - (b + d)) * scale).ln(), b.ln(), d.ln()])
            .collect()
    }

    /// `HiddenMarkovModel::viterbi` @0x1023035c4. Returns the state path and its log probability.
    pub fn viterbi(&self, obs: &[[f32; 2]]) -> (Vec<u32>, f64) {
        let n = self.num_states;
        let frames = obs.len();
        let dens = Self::log_densities(obs);
        // uniform initial distribution: ones · (1/n), then log
        let init = (1.0 * (1.0 / n as f64)).ln();
        let mut previous = vec![init; n];
        let mut current = vec![f64::NEG_INFINITY; n];
        // back pointers as offsets into each state's CSR row (≤ 54 predecessors)
        let mut bt = vec![0u8; frames * n];
        for (f, d) in dens.iter().enumerate() {
            let row = &mut bt[f * n..(f + 1) * n];
            for s in 0..n {
                let mut best = f64::NEG_INFINITY;
                let density = f64::from(d[self.om_pointers[s] as usize]);
                let (a, b) = (self.pointers[s] as usize, self.pointers[s + 1] as usize);
                for p in a..b {
                    let v = previous[self.prev[p] as usize] + density + self.log_prob[p];
                    if best < v {
                        best = v;
                        row[s] = (p - a) as u8;
                    }
                }
                current[s] = best;
            }
            previous.copy_from_slice(&current);
        }
        if frames == 0 {
            return (Vec::new(), f64::NEG_INFINITY);
        }
        // NdArray::argmax: the first maximum
        let mut state = 0usize;
        for s in 1..n {
            if current[s] > current[state] {
                state = s;
            }
        }
        let logp = current[state];
        let mut path = vec![0u32; frames];
        for f in (0..frames).rev() {
            path[f] = state as u32;
            let off = bt[f * n + state] as usize;
            state = self.prev[self.pointers[state] as usize + off] as usize;
        }
        (path, logp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_space_shape() {
        let h = Hmm::get();
        assert_eq!(h.num_states, 12852);
        assert_eq!(h.pointers.len(), 12853);
        assert_eq!(h.prev.len(), 20036);
        assert_eq!(*h.pointers.last().unwrap(), 20036);
    }

    /// The table compiled into rekordbox 7.2.19 (`param_pointers` @0x10384599c, `param_states`
    /// @0x10383208c, `param_probabilities` @0x103852270, little endian, in that order) has this
    /// SHA-256; the rebuilt model must match it bit for bit.
    #[test]
    fn transition_table_matches_rekordbox() {
        use sha2::{Digest, Sha256};
        let h = Hmm::get();
        let mut d = Sha256::new();
        for v in &h.pointers {
            d.update(v.to_le_bytes());
        }
        for v in &h.prev {
            d.update(v.to_le_bytes());
        }
        for v in &h.prob {
            d.update(v.to_le_bytes());
        }
        let hex: String = d.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "55a72d1b5c2593a0b6c25289787c9bce28c4545e8cd1f59c2bbc8cc53c0117fe");
    }

    #[test]
    fn linspace_endpoint() {
        assert_eq!(linspace(0.0, 1.0, 4, false), vec![0.0, 0.25, 0.5, 0.75]);
        assert_eq!(linspace(1.0, 2.0, 3, true), vec![1.0, 1.5, 2.0]);
    }
}
