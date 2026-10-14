# High-precision beat analysis (madmom + BALib v2 Focus)

Status: **verified for all three high-precision modes.** With `Options { high_precision: Some(mode), .. }`, `mb_analysis::analyze` reproduces rekordbox 7.2.19's grids (phrase pass on):
- Dynamic (mode 4), `fixtures/oracle-b`: PQTZ, PQT2 and `djmdContent.BPM` 154/154;
- Normal (mode 3), `fixtures/oracle-c`: 162/162, PSSI 161/161 (the four sampler loops on which rekordbox crashes are excluded, and so is X-Ray's PSSI, written by the crashing process; `fixtures/manifest.toml`);
- Auto (mode 2), `fixtures/oracle-d`: 154/154, PSSI 166/166.

In modes 4 and 2 the other 12 tracks (sweeps) get no high-precision grid, in rekordbox as in musicbox (§ 6). The BPM-range rebuild for grids outside 70–180 is not ported (§ 6, § 9). Scorecards: `docs/scorecards/beats-hp.json` (mode 4), `docs/scorecards/beats-hp-v2.json` (modes 3 and 2).

Code: `crates/mb-nn` (TensorFlow runtime); `crates/mb-analysis/src/hpbeat/` (`madmom.rs`, `hmm.rs`, `focus.rs`, `focus/constant.rs`, `focus/vote.rs`, `mod.rs`); `crates/mb-analysis/src/balib2/` (BALib v2 for modes 2 and 3).

## 1. Modes, preferences and flags
- **Mode selection** (`DatabaseMediator::execAnalyze` @0x1014900ec):
  - `HighPrecisionBeatAnalyze` = 0 (default 0, `_kHighPrecisionBeatAnalyzeDefaultValue` @0x1037bbc99): `trackAnalysisMode` 1 gives mode 1 (Dynamic), else mode 0 (Normal).
  - `HighPrecisionBeatAnalyze` = 1: `SettingIF::getCurrentTackHighPrecisionAnalysisMode` @0x1015bce80 reads `trackHiPrecisionBeatAnalysisMode` (default 2, `_kTrackHighPrecisionAnalysisModeDefaultValue` @0x1037bbc9c). 0 gives mode 3 (Normal), 1 gives mode 4 (Dynamic), anything else mode 2 (Auto).
  - rekordbox's analysis dialog shows "Analysis Mode: Dynamisch" in mode 4.
- **Dispatch.** `AnalyzeJobBeatDetector::Detect` @0x102327c80 runs `highPrecisionBeatAnalyze` @0x102327e50 for modes 2–4 and `legacyBeatAnalyze` @0x102328684 otherwise.
- **BALib.** Version 2 in every mode. The varied analyser is used for modes 1 and 4 (`CBeatAnalyzeThread::startAnalyze` @0x100151d54 sets +0x3940 = mode ∈ {1, 4}). `SetBpmHendou`: false in mode 3, true in mode 4; mode 2 takes `GetBpmHendou`. So the analyser's tempo-change flag (+0x2d) is 1 in mode 4, 0 in mode 3, and BALib's verdict in mode 2.
- **No phrase grid shift in Dynamic modes.**
  - The shift flag `CBeatAnalyzeThread` +0x3908 is `AnalyzeJobManager` (+0x38f8) +0x10, written by `AnalyzeJobManager::SetUrahaku` @0x1022fb028.
  - `CBeatAnalyzeThread::analyze` @0x1001528c4 sets it to: beat analysis on (+0x3900) and mode ∉ {1, 4}.
  - It gates the phrase pass's grid move (`analysis-beats.md` § 8). `SongStructureAnalyze::Initialize` also receives it, negated, as its 9th argument.
  - musicbox: `hpbeat::HpGrid.urahaku`, passed to `phrase::run`.
- **No phrase grid shift once madmom found beats.** Besides the thread's setting, `highPrecisionBeatAnalyze` @0x102327e50 calls `AnalyzeJobManager::SetUrahaku(false)` after parsing madmom's JSON when the beat list is not empty. So modes 2 and 3 do not shift either, unless madmom returned no beats. oracle-c and oracle-d confirm it: their PQTZ are the unshifted Focus grids.
- **PQT2 type.** `CBeatAnalyzeThread::notifyBeat` @0x1001595c8 passes the PQT2 `analysis_type` to `notify_detailed_beat_info` (then `MstStoreExtBeatGrid`): 2 for modes 1 and 4, 2 for mode 2 when +0x3940 is set, else 1.
  - In mode 2, `startAnalyze` sets +0x3940 from the request's beat-analysis mode (1 or 4), yet in oracle-d the type follows BALib v2's verdict: the 22 grids with tempo changes (hendou) have type 2, the 132 constant ones type 1, and the 12 sweeps keep their old type-1 grid. Where +0x3940 is rewritten is not located.
- **oracle-b is mode 4.** 154 PQT2 have type 2. The 12 sweeps keep type 1 from the earlier pass (§ 6).

## 2. TensorFlow runtime (`mb-nn`)
- **Library.** rekordbox ships TensorFlow 2.16.1: `Contents/Frameworks/libtensorflow_cc.2.dylib` plus `libtensorflow_framework.2.dylib`.
  - musicbox `dlopen`s the user's installed copy; `MB_REKORDBOX_APP` overrides the app path.
  - It requires `TF_Version` = "2.16.1" and checks the model files' MD5 before use.
  - Nothing is copied or bundled.
- **Models** (`Contents/Resources/models/`):

  | Model | Files (MD5) | Used by |
  |---|---|---|
  | `detect_beat/model7` (SavedModel, tag `serve`) | `saved_model.pb` 2fcaa488335e8e9ee035997bc9ec19da, `variables.data-00000-of-00001` 4f65199efecdd3d862ce9433bb4fca60, `variables.index` af86d1862acff1f79143b33d8a223799 | mode 4 (`GetBeatDetectModelNo7DirectoryPath`) |
  | `detect_beat/model8` | 0ec3f9d3a0253050c4d535c9c93fe8a1 / dfb57914726bf38e6dac829a2f82c1fb / f4835efea6bc04a64ca1103b9b95729a | modes 2 and 3 |
  | `vd_param.pb` (frozen graph) | 13217b009062598a322f3427bb7cd21b | vocal detector (`analyzer::Discriminator::inference` @0x100b812d0) |

  The analyser keeps the two beat model paths at `CAnalyzerIF` +0xb8 (model7) and +0xc0 (model8).
- **Session.** `TFWrapper::Model::Pimpl` @0x10340520c loads the SavedModel with tag `serve`. `TFWrapper::defaultSessionOptions` @0x103404d18 sets intra- and inter-op pools of `hardware_concurrency()` threads and allows soft placement.
- **Tensors.** Input `serving_default_fltp:0`: f32 `[1, samples]`. Output `StatefulPartitionedCall:1`: f32 `[frames, 2]` (beat, downbeat) at 100 frames/s. Activations are bit-identical to rekordbox's, compared with a native harness outside the repo.
- **Logging.** TensorFlow logs INFO lines to stderr unless `TF_CPP_MIN_LOG_LEVEL` is set. Each of its two libraries reads the variable once, at its first log statement.
  - `mb-nn` sets it to 1 from before the load until the first session run, then removes it.
  - A value the caller set is left alone.

## 3. `madmom::analyze` @0x10230a820 (inner part @0x102307f8c)
- **Input.** `(L + R)·0.5` of the decoded float samples at the analysis rate, samples `0 … L − 2` (`decode.md` § 2).
- **Arguments.** The model 7 and model 8 paths, the file path, the rate, BALib's `GetOffsetCalc`, and the hendou flag. An output JSON gets `numChannels, sampleRate, decodeSampleRate, bitRate, bitDepth, decodeSamples, bpm, beatList[{index, bpm, pos, msec, numBar}]`.
- **Without hendou** (modes 2/3). Model 8 runs once over 60 s from `offset`. When less than 60 s remains after `offset`, it runs over the last 60 s instead (the whole track when shorter).
- **With hendou** (mode 4). Model 7 runs over the whole track in 60 s chunks.
  - When the part after the last full chunk is shorter than 30 s, the last full chunk and that remainder are run as two halves of their combined span.
  - Each chunk's times are offset by its start.
  - Each later segment is merged in after the beats so far. Its beats up to the one closest to the last beat so far are dropped (at least its first).
- **`downbeat_tracking`** @0x102300d6c (4 beats per bar, 70–180 BPM, 60 tempi, λ 100 / 16, threshold 0.05, correction on, 100 fps):
  1. Only the rows from the first to the last with an activation ≥ 0.05 are kept.
  2. Viterbi over the bar HMM (§ 4).
  3. One beat per run of beat-range states, at the strongest activation in the run (first maximum over the flattened rows). Its bar position comes from the state.
  - Quirk: a path that never enters or leaves the beat range gives no beats. madmom itself would emit one.
- **Rate.** At rates other than 44.1 kHz, times are scaled by 44100/rate. The vector part multiplies by that ratio; the scalar tail multiplies by 44100, then by 1/rate.
- **Hendou clean-up** (seconds, f32).
  1. At each 60 s boundary and at the split point, take the first beat `c` at or after it, with `r1 = (c − p)/(p − pp)` and `r2 = (n − c)/(c − p)`. Then:
     - if `round(r1) > 1`, 1.35 ≤ r1 ≤ 3.5 and 0.25 ≤ r2 ≤ 0.75, fill the gap before `c`;
     - else if 0.9 ≤ r1 ≤ 1.1 and 1.35 ≤ r2 ≤ 3.5, fill the gap after `c`;
     - otherwise drop `c` when r1 < 0.3, or the beat after it when r2 < 0.3.
  2. Every gap longer than 1.8× both neighbouring gaps gets `round(ratio) − 1` evenly spaced beats.
- **`get_beat_list`** @0x102305908 and **`get_bpm`** @0x102306b34.
  - The list is extended back towards 0 at the first interval, with bar numbers counting down.
  - `msec = round(t·1000)` and `pos = (int)((msec·rate)·0.001f)`.
  - Each beat's `bpm = (int)(100·60000/Δmsec)` to the next beat; the last beat repeats the previous value.
  - The list is extended to the end of the track at the last step.
  - `bpm` is the median (`nc::percentile`, linear) of the per-beat values.
- **Result.** 166/166 oracle-b beat lists are identical to rekordbox's.

## 4. The bar HMM (`hmm.rs`)
- **State space.** `BarStateSpace`, 4 beats. 60 log-spaced tempi between 70 and 180 BPM are rounded (`nc::round`, ties to even) and made unique: 54 beat intervals of 33–86 frames, giving 12852 states.
- **Transitions.** Exponential tempo transitions, λ = 100, 20036 transitions.
  - rekordbox does not compute them: `TransitionModel::make_sparse` @0x102311cd0 returns a CSR table compiled into the binary (`param_pointers` @0x10384599c, `param_states` @0x10383208c, `param_probabilities` @0x103852270).
  - The port rebuilds the table numpy-style (exp, pairwise row sums, division, sorted CSR).
  - SHA-256 `55a72d1b5c2593a0b6c25289787c9bce28c4545e8cd1f59c2bbc8cc53c0117fe` equals rekordbox's table (test `transition_table_matches_rekordbox`).
- **Observations.** `RNNDownBeatTrackingObservationModel::log_densities` @0x10231725c, λ = 16, in f32.
- **Viterbi.** `HiddenMarkovModel::viterbi` @0x1023035c4, u8 back-pointer offsets.

## 5. Focus (`BALibWrapper::CalcBeatPos_Focus` → `BeatAnalyzer_2_0::Analyzer::analyzeFocus` @0x10116c050)
- **Inputs.**
  - BALib's beat count and BPM. In mode 4 they are only checked for zero; zero gives no result.
  - The madmom list (pos, bar number, BPM × 100) and its median BPM.
  - The analyser's rate, samples (`L − 1`), hendou flag and offset.
  - An empty madmom list returns at the entry check, before anything is written.
- **Outputs.**
  - `getBpm_Focus` @0x1011820d4: `fcvtzs(fmaf(+0x4c, 100, 0.5))`, single precision. +0x4c is set from +0x48 at the end of `analyzeFocus`; after an early return it still holds BALib's own BPM.
  - `GetBeatPos_Focus` @0x1011a7664: per beat, position, beat in bar and `(int)(6e6/tempo + 0.5f)`, with the tempo in ms.
- **Paths.** With the hendou flag, or when the largest bar number among the first four madmom beats is not 4: two passes, then two clean-up rounds. Otherwise the constant path runs (only modes 2/3 can reach it).
- **Constant path** (code 0x10117996c–0x101181240; `hpbeat/focus/constant.rs`; ported up to UnitBeatAdjust: its input and output identical on 166/166 tracks in mode 3, the final list on 96/166, the rest differing only in bar numbers).
  1. Times are rounded to the ms; intervals are `trunc(fma(Δt, 1000, 0.5))`.
  2. **Reference interval:** the first stretch (among the first n − 21 differences) of at least min(n, 37) intervals whose neighbours differ by ≤ 11, else 20, else 30 ms. It must end at a larger jump. Otherwise the reference is the mean of the first min(n, 37) intervals. The stretch's first and last index are kept.
  3. **Tempo check.** The analyser's tempo (+0x48, BALib's BPM) is checked against 60000/reference:
     - snapped to a whole BPM within 0.05 %;
     - at ≈2× and ≤ 90 BPM: doubled;
     - at ≈½× and ≥ 140: halved;
     - outside 0.8–1.25×: replaced by the reference tempo (halved above 180, which sets the out flag; doubled below 70).
  4. **Irregular stretches.** Beats deviating from the reference by > 10 ms on two neighbouring intervals form stretches.
     - If they cover ≥ half the track, they are rebuilt as `round(span/ref)` evenly spaced beats between their outer beats (rounded to the ms).
     - Otherwise the same is done with 20 ms stretches, and the count change per stretch is recorded.
     - The beat vector takes the new length (extra copies of the last madmom beat, or cut). Quirk kept: when it gets shorter, the anchor region can read one slot past its end, i.e. the cut madmom beat still in memory (Breaks2); the port keeps the cut beats as a tail for those reads.
  5. **Half-beat test.** For the midpoints of the stable segments between stretches (10 ms and 20 ms separately), count the segment pairs whose distance is 0.4–0.6 beats off a whole beat. More than a quarter of pairs marks the segments as disagreeing.
  6. **Anchor.**
     - If they disagree: the anchor is where the low band comes in: the first beat whose low-band sum over two beats (`calcLowInteg` at pos + BALib offset, scaled to the loudest) is > 0.6, or > 0.2 after a rise > 0.1. The region is the stable gap holding it, or 16 beats.
     - Otherwise: the anchor is the middle of the reference stretch (shifted by the 20 ms count changes before it), moved back to its bar's beat 1. Beats outside the region are then compared with the grid through the anchor; each takes the closest of its ±5 neighbours when that is 25 % closer. If their mean distance exceeds 0.3 beat over more than half the beats, the grid moves half a beat toward their mean deviation.
  7. **Grid.**
     - The rebuilt times become positions; bar numbers continue from the beat nearest the anchor's old position.
     - The grid is `fma(beat, k − anchor, (mean residual over the anchor region + flip) + anchor position)`, rounded `trunc(x + 0.5)`.
     - Beat lengths come from the unrounded grid; BALib's offset is added.
     - Whole beats are added before the first beat (≥ 0) and after the last (≤ the sample count), with tempo 60000/BPM (single precision).
  8. `UnitBeatAdjust::analyze` runs on the grid (§ 8).
  9. Then the bar-number vote below.
- **Bar numbering after UnitBeatAdjust** (0x10117db28–0x101180744; `hpbeat/focus/vote.rs`). The adjusted grid (n beats) keeps the anchor's bar numbers until this vote; it runs whether or not `UnitBeatAdjust` ran (+0x2c).
  1. **Band sums.** Index k+1 holds beat k: L = `calcLowInteg(bpm·0.5, pos)` (the low band over one beat), A = `calcAllInteg(bpm, pos)` @0x10113d9ac (the full band, +0x80, `rate/4·60/bpm` values from `pos/4`, −1 in the same cases as `calcLowInteg`). A negative sum becomes 0 except at beat 0. Index n+1 is 0.
     - Index 0 is the lead-in: when the first beat lies at least half a beat in (`bpm·pos₀/(rate·60)` ≥ 0.5), the same sums from 0 at the tempo B = `rate·60/pos₀` (120 when pos₀ = 0), times B/bpm; else 0.
  2. **Rises.** dX[0] = X[0], dX[i] = X[i] − X[i−1]. dX[0..n] are scaled by their largest value (taken together with 0) when that is over `DBL_EPSILON`: dX[0] by division, the others by the reciprocal. dX[n+1] stays unscaled.
  3. **Irregular falls.** Per band: the running maximum of |dX| from the end; from one past the last index with |dX| > 0.7 up to n−1, the indices where that maximum grows by more than 0.005. More than four, or two of the first four not on neighbouring indices, mark the band irregular.
  4. **Head votes.** Without a lead-in (q < 0.5): the first k in 1..n with dL[k] > 0.35 votes beat k−1's bar number; the first k with dA[k] > mean + 1.5σ of |dA[1..w]| (w = min(n, 48)) does the same. With a lead-in the scans start at 0 and the statistics take |dA[0..w]| (w+1 values); k = 0 votes the bar before beat 0. 0 when nothing qualifies.
  5. **Tail votes.** Per band: mean and σ of the last w values |dX[n+1−w+1 .. n+1]|, summed from the end. Scanning k from n+1 down to 2, the first dX[k] < −0.5σ − mean votes beat k−1's bar number, or the bar after the last beat when k = n+1. A band's tail vote counts only when the band is not irregular. The mean and spread sums are 8-wide with four 2-lane accumulators and fused multiply-adds, like the constant path's mean residual.
  6. **Median votes.** With m = the median of |dL[0..n+1]| (n+2 values): every k in 1..n+1 with dL[k] > m + 0.2 (m + 0.1 when that gives fewer than five) is a candidate for beat k−1's bar number (the bar after the last beat at k = n+1). A candidate is kept when it lies at least 5 beats after each of the previous four candidates. Kept votes are tallied per bar (k₁..k₄) only when at least two were kept.
  7. **Loudest beats.** L[0..n+1] sorted descending (exchange sort, so ties keep a fixed order) with the bar numbers [beat 0..n−1, the bar after the last beat, the one after that]; the first 16 are tallied (t₁..t₄).
  8. **Winner.** The leader of k₁..k₄ (ties to the lower bar). When its lead over the runner-up is at most 10 and the runner-up has more than 4 votes, step 6 is redone on L itself, scaled by its largest value over 0..n; that vote's leader wins (0 when it has none).
  9. **Cases** (first match; v₁..v₄ tallies the head and tail votes, "lead" = first or last leader of v; each case rotates every beat's bar number or keeps it):
     - only k₁ non-zero → keep; only k₂ → one back (2→1, 1→4); only k₃ → half a bar (s ± 2); only k₄ → one on (4→1);
     - lead 1 and k₁ ≥ 1, others ≤ 1 → keep; lead 2, k₂ ≥ 1, others ≤ 1 → one back; lead 3, k₃ ≥ 1, others ≤ 1 → half a bar;
     - (last leader 4 or v₄ above v₁..v₃), k₁ ≤ 1, k₂ ≤ 1, k₃ ≤ 10, k₄ ≥ 1 → one on;
     - head low vote h ≠ 1 with h = head full vote, and h equal to each tail vote or the bar before it (any combination) → shift by 1 − h;
     - first and last leader 1 and winner 1 → keep; a leader i ≥ 2 equal to the winner → shift by 1 − winner;
     - v_i ≥ 1 and the loudest 16 only on bar i → keep / one back / half a bar / one on for i = 1..4;
     - first leader i ≥ 2 equal to the last leader → shift by 1 − i;
     - else keep.
     - "Shift by d" adds d and adds 4 below 1, with no wrap at the top. With all four head/tail votes 0 the fourth case would give bar 5; not seen.
  10. The list is copied to the result (+0x88) and +0x4c = +0x48.
  - **Result.** With harness inputs (BALib and madmom from rekordbox), Focus's final list and BPM match rekordbox on 166/166 constant-path tracks; every case above occurs at least twice.
- **A pass.**
  1. Beat times are rounded to the ms (`frinta`). Intervals in ms are `trunc(Δt·1000 + 0.5)`.
  2. **Reference interval:** the mean interval of the first stretch of at least `min(n, 37)` intervals whose neighbours differ by ≤ 11 ms, else ≤ 20 ms, else ≤ 30 ms; failing all three, the mean of the first `min(n, 37)` intervals. It is kept when `60000/mean` is within 0.8–1.25× the madmom BPM. Otherwise the madmom BPM sets it.
  3. `x` = interval/reference and `r = max(x, 1/x)`.
  4. **Runs:** at least two consecutive `r > 1.18`, at most 128.
  5. **Anchor** (first pass): in the longest regular stretch, the beat at start + half its length. That is half the length, not the midpoint.
  6. **Categories** per run: the first and last index of `x` in each window. Half 0.4425–0.5575, 2/3 0.59–0.7433, 1.5 1.3275–1.6725, double 1.77–2.23.
- **Short runs** (< 8 intervals; all runs are processed last to first).
  - The run must stand out: its mean `r` exceeds the mean of up to 8 neighbours on each side by more than 0.16.
  - It is widened over up to three neighbours with `r > 1.07` on each side.
  - It is refilled with `round(span/reference)` evenly spaced beats when that count is ≥ 2 and within 0.25 of a whole number.
- **Long runs.**
  - **Statistics** for the run and for each category range: mean, variance, and drift (mean of three end-to-end differences of `x`).
  - **Pattern order** (the first match wins): half (mean 0.485–0.515), sub half, 2/3 (0.6467–0.6867), sub 2/3, 1.5 (1.455–1.545), sub 1.5, double (1.94–2.06), sub double.
  - **Conditions:** drift < 0.008, variance < 0.015, and the range spans more than 7.
- **Fixing a long run** (or a category range):

  | Pattern | Neighbour window | Align ends | Thinning | New beats per gap |
  |---|---|---|---|---|
  | half | 1.07–1.8 or > 2.2 | yes | every other beat | none |
  | 2/3 | 1.07–1.425 or 1.575–1.8 | yes | two of every three | 1 (midpoint) |
  | 1.5 | 1.07–1.425 or 1.575–1.8 | yes | every other beat | 2 (thirds) |
  | double | 1.07–1.8 or > 2.2 | no | none | 1 (midpoint) |

  1. **Widen** both range ends over up to five accepted neighbours each way, and erase the widened zones.
  2. **Align:** the start keeps the first beat 1, 2 or 3 reference intervals later (±¼ interval, among the next five) and drops the beats in between. The end does the same backwards.
  3. **Thin** the range after its second beat.
  4. **Insert** new beats, from the end back to the start.
  5. **Fill** the gaps next to the range ends with `round(gap/reference)` beats.
  6. **Extend:** add one beat an interval before the first if that stays ≥ 0, and beats at the last interval up to the end of the track.
  7. When any widening took all five steps, fill the first gap longer than five times both neighbours.

  Quirks kept:
  - The end alignment always erases at `end − 1`.
  - The whole-run copies of the start alignment can read one slot past the vector's end. The sub-range copies check; the port treats both as checked.
  - The sub-range copies check `0 ≤ end` before filling.
  - The beat count carried between runs is the count before "extend last", or after it when a widening took five steps.
- **Clean-up rounds** (twice):
  1. Gaps longer than 1.8× both neighbours are filled (`k = round(ratio)`).
  2. With n ≥ 65, from beat 32 to n − 32: four-beat spans whose ratio to the neighbouring bars has a fraction in 0.4–0.6 are recorded in pairs and respaced at `q + 0.5` beats, with neighbour smoothing. The pair cases are (4,3), (4,4), (3,4) and (3,3).
- **Finish.**
  - The median BPM (f32).
  - Beat numbers from the anchor, reset at minute boundaries.
  - Per-beat tempo over ±8 beats (f32), averaged per bar.
  - Head and tail filled.
- **Validation.**
  - A harness runs rekordbox's own `analyzeFocus` on a given madmom list. It agrees with the port on all 166 oracle-b inputs.
  - It also agrees on 5100 synthetic lists out of 6000 seeds: tempo drift, jitter, half/2/3/1.5/double stretches, mixed runs, gaps, short runs. Each long-run pattern was taken about 900–1100 times. The other 900 take the constant path and are excluded.

## 6. The job (`highPrecisionBeatAnalyze`)
1. BALib v2 `GetBpm` / `GetBeatNum` / `GetOffsetCalc`.
2. `madmom::analyze`; the JSON is parsed into `{pos, numBar, bpm}`.
3. `CalcBeatPos_Focus(NULL, beatNum, bpm, list, n, madmomBpm)`, then `GetBpm_Focus` into job +0x68, then `GetBeatNum_Focus`.
4. **No beats.**
   - The job clears +0x64/+0x58/+0x60/+0x74, notifies, and returns status 4. `notifyBeat` is not reached, so the old PQTZ/PQT2 stay.
   - `djmdContent.BPM` becomes 0 (+0x64).
   - The thread then reads the stored grid back. `CBeatAnalyzeThread::analyze` @0x1001528c4 calls `getBeatListFromDatabase` when the beat count is 0 and `AnalyzeJobManager` +9 is set. Key and phrase then run on the previous grid with its BPM.
   - This is oracle-b's 12 sweeps: madmom finds no beats; the old 51-beat type-1 grid stays; BPM is 0; the PSSI comes from the old grid with BPM 102.50 (`analysis-phrase.md` § 10).
5. **Otherwise:** `GetBeatPos_Focus`, then `getFirstBeatPos` @0x102328a00, then `convertToMicrosec` @0x102328b20.
   - `µs = pos·10⁶/rate` (64-bit integer); `ms = µs/1000`; sub-ms = the remainder (`analysis-beats.md` § 7).
   - The BPM-range rebuild (`bpmToDouble`/`bpmToHalf`) is skipped because the job's hendou flag (job +0x28 → +0x30) is set.
6. `djmdContent.BPM` = `GetBpm_Focus`. PQT2 type 2 (§ 1).
- **Modes 2 and 3.** hendou = BALib v2's verdict in mode 2, false in mode 3 (`SetBpmHendou`, job +0x30). madmom runs model 7 over the whole track with hendou, else model 8 over the 60 s window. Focus gets BALib v2's beat count and BPM, madmom's list and BPM, and the analyser's `UnitBeatAdjust` bands.
- **`getFirstBeatPos`** @0x102328a00 reads the job's previous `DetailedBeatInfo` array (+0x58, when it has more than 7 beats): +0x88 = its first beat's ms, +0x90 = that beat's bar through `convFirstBar`. When the stored tempo differs from the new BPM and the beat lies between one and two beats in, both move one beat back. It does not touch the new grid.
- **BPM-range rebuild** (`convertToMicrosec` without hendou). The BPM (`GetBpm_Focus`, else the first beat's tempo) is checked against `bpmRangeList[+0x7c]` (index 1 → 70–180). Below the range `bpmToDouble` @0x102329060 inserts a beat between each pair at double tempo, repeatedly; above it `bpmToHalf` @0x10232967c keeps every second beat at half tempo; both renumber the bars (`renumberBeat`) from the first beat's bar through a table, and the job BPM is doubled or halved with them. Not ported: no oracle-c/d track leaves 70–180 (min 73.97, max 180.00).

## 7. musicbox
- **Options.** `Options.high_precision: Option<HpMode>`.
  - `hpbeat::grid` runs madmom on the `(L + R)/2` samples gathered during the decode pass.
  - Focus is then seeded with the Normal-mode grid's beat count and BPM. Only non-zero matters, which stands in for BALib v2's count (§ 8).
  - `Analysis.grid_type` is the PQT2 type.
  - The model is loaded once per process.
- **No beats.** `Analysis.beats` is `None`. `mb-library` then sets `TrackAnalysis.keep_old_grid`: `ApplyAnalysis` carries the previous PQTZ/PQT2 over and writes `BPM` 0 (`ops/analyse-track.md`).
  - Key and phrase run on the stored grid, as in rekordbox (§ 6): `mb_analysis::analyze_with` / `mb_library::analyse_track_with` take it from the caller. The 12 sweeps then get oracle-b's key and PSSI.
- **Preferences.** `AnalysisSettings::high_precision_from(HighPrecisionBeatAnalyze, trackHiPrecisionBeatAnalysisMode)`, with the binary's defaults 0 and 2.
- **Normal and Auto.** `mb_analysis::analyze` feeds BALib v2's constant analyser (`balib2::Analyzer`) during the decode pass. `hpbeat::grid_v2` then runs the job: `finish_with_adjust` (BPM, offset, verdict, and the `UnitBeatAdjust` bands), madmom, Focus (`run_with`), `convertToMicrosec`. A BPM outside 70–180 without hendou is an error (the Normal-mode grid stays, `high_precision_error`).
  - `HpGrid { grid, grid_type, urahaku }`; no beats keeps the stored grid for key and phrase, as in Dynamic.
- **No phrase shift once madmom found beats** (urahaku, § 1). Without this gate, 57 of oracle-b's 154 grids would move.
- **Cost.** About 4–5 s per track, with three tracks in parallel on this machine.
- **Tools.**
  - `cargo run --release -p mb-analysis --example hp_score`: oracle-b scoring.
  - `hp_check {table, madmom, focus, focusin, window}`: developer checks against harness dumps.
  - `hp_v2_score <normal|auto> [--full]`: oracle-c / oracle-d scoring; `--full` goes through `analyze_with` and scores PSSI too.

## 8. BALib v2 beat analysis (modes 2/3)
Status: **in progress.** Ported and checked against rekordbox's own BALib v2 (harness outside the repo) on the 169 oracle tracks: the verdict flags and returned BPM 169/169, `getOffsetCalc` 169/169, the beat list 169/169 in mode 3 and 168/169 in mode 2 (Timelock (Red Sun, Harmonika Remix): v2's copy of the varied pipeline differs from v1's from beat 575). Model 8's 60 s window: 20/20. UnitBeatAdjust v2's `analyze` 166/166; Focus's constant path up to UnitBeatAdjust 166/166 (§ 5). Wired into `analyze()` through `hpbeat::grid_v2` (§ 7). Code: `crates/mb-analysis/src/balib2/` (`mod.rs`, `verdict.rs`, `adjust.rs`); examples `balib2_score`, `balib2_stages`, `balib2_uba`, `hp_check window`.

- **Shared with v1.** The sensor, envelope and filters are instruction-identical to v1 (`beats::sensor`, `beats::envelope`, `beats::filters`); the sensor list is identical on every track.
- **Two passes.** `BeatInfoAnalyzer::analyze` @0x101198e4c runs v1's constant path, then v1's varied (Dynamic) path, on the sensor list. It keeps two BPMs:
  - constant: 60000 / the constant list's first beat length (0x10119bd94);
  - varied: the varied list's longest-segment tempo (0x10119dc14).
- **Verdict** (0x10119fe0c–0x1011a53c0).
  - Skipped (no list, BPM 0) when either BPM is 0, the varied list is empty or the rate is 0.
  - Otherwise four tests run on the varied list. Any of them true keeps the varied list and its BPM, and sets the hendou flag (+0x2d). If all are false, the constant list and its BPM are kept.
  - **A**, `judgeHendouDynamicPeak` @0x1011a6714:
    - input is the sensor list. For each onset before 15 min, sum UnitBeatAdjust v2's low band over half a beat from the onset (rate·7.5/BPM quarter-rate samples);
    - the scan stops at the first window past the band's end;
    - A = n × (max sum × mean sum) < 2e15.
  - **D** (two tempo segments):
    - intervals (ms, `fma(|Δt|, 1000, 0.5)` with times rounded to the ms) are split at stretches where neighbours differ by more than 11 ms (up to 32 stretches);
    - each segment's BPM (single precision) must be within 0.2 of a whole BPM;
    - no segment may cover 65 % of the track. Segments longer than 16 beats must each cover more than 7.5 %, and the largest and smallest of those may differ by less than 55 %;
    - two such segments must have different whole BPMs.
  - **B** (spread):
    - reference interval: the mean of the longest stretch of ≥ min(n, 37) intervals whose neighbours differ by ≤ 11/20/30 ms, else the median. Segments at a harmonic of the longest segment (0.5, 2/3, 1.5, 2) are rescaled to it;
    - take the deviation of the 16-beat mean intervals from the reference, and the share of stable stretches over 23 intervals;
    - B = (dev ≥ 2.25 and share ≤ 0.19) or (dev > 1.56 and share < 0.057).
  - **C**, only when A, B and D are false (position drift):
    - beats between 5 % and 80 % of the track, outside 10 ms-irregular stretches, are compared with a uniform grid. The grid's beat length comes from the varied BPM after an octave/whole-BPM check against the reference;
    - the linear trend is removed, short excursions between jumps over 1/16 beat are cut, and residuals are classified per range by their mean phase (whole, half or quarter beat);
    - C needs all four 32-beat-mean measures above their bounds. Fewer than 65 usable beats counts as C;
    - whatever its outcome, the trend refines both BPMs: a drift-corrected BPM replaces a stored one when it rounds to the same whole BPM and is closer to it; each BPM then snaps to its tenth when within 0.1 % (e.g. a constant BPM of 119.998 becomes 120.0).
  - Fired on the 169 tracks: C 23, D 7, A 2, A + B 2, none 132 (+ 3 loops without beats).
- **Low band.** UnitBeatAdjust v2 `putSample` @0x10113ad58 takes every fourth frame of each block: mid signal → 100 Hz low-pass → |·| → 25 Hz low-pass, clamped to 0…32767. Its filters are designed for rate/4 (`create` @0x10113aabc divides the rate by 4).
- **Offset** (`getOffsetCalc` @0x101182100, `BA_GetOffsetCalc`):
  - for each beat of the final list, `calcLowInteg` @0x10113d760 gives the low band over two beats (a failure copies the previous value). The values are scaled to the loudest;
  - a scan finds where the low end comes in: a beat over 0.4 (or three over 0.2) after a rise > 0.15 starts a loud stretch; the first stretch reaching 12 loud beats wins; otherwise the first loud start, or the one after more than 32 quiet beats;
  - offset = rate·60·(index − 4)/BPM samples, at least 0.
- **Hendou to the job.** Mode 3 then sets `SetBpmHendou(false)`; mode 2 reads `GetBpmHendou`.
- **UnitBeatAdjust v2** (`balib2/adjust.rs`; `analyze` 166/166 against rekordbox's, output list, BPM and band checksums, on every track whose Focus reaches it in mode 3).
  - **Setup.** `create` @0x10113aabc passes rate/4, so all filters are designed for the quarter rate. The unit keeps three bands: high (+0x70: 2 kHz high-pass, |·|, 1 kHz low-pass), low (+0x78: 100 Hz low-pass, |·|, 25 Hz low-pass) and full (+0x80: |·|, 1 kHz low-pass).
  - **`analyze` @0x10113af38.** Beat count k: the beats of the first 15 minutes, `trunc(bpm/60 · (rate/4·900 − pos0/4) / (rate/4))` with a fused multiply-add, used only when the band reaches 15 minutes; otherwise k is the list length. It then runs `bpmAdjust(bpm, list, k)` and `beatPositionAdjust`.
  - **`bpmAdjust` @0x10113aff8.** Two searches: the first starts at the first beat (quarter rate), the second half a beat later (`trunc(rate/4·30/bpm + 0.5)` samples added to each position).
    - Each search steps from bpm − 0.05 in 0.05 BPM steps: up if the fraction is above .5, down if below, both ways if exactly .5. It continues while `getPeakBpm` asks for more.
    - It then rescans once just below the whole BPM when the last result is within 0.05 % of it.
    - The half-beat phase wins when the first phase scores under 96 % of it; every beat then moves by that half beat.
    - `reflectBeatData` @0x10113c670 rebuilds the list on the winning grid only when the BPM changes at 0.001 resolution: positions are the grid ×4 and the tempo is 60000/BPM; bar numbers continue from the beat nearest the grid's first beat; the list is resized to the grid's length.
  - **`getPeakBpm` @0x10113c028.** Scores 51·32 = 1632 tempos from the start BPM in steps of 6.25e-5 (fused multiply-add, kept within 70–180).
    - Each score comes from `createBeatData` + `manageWave`: the grid runs from the start (positions `trunc(start + k·len + 0.5)`, fused) to the end of the track; the high band is summed in ±rate/4·0.05 windows around the inner beats of its first n positions; the score is the sum's peak.
    - A MATLAB-style `findpeaks_x` @0x1011685c4 then runs over the score curve (peaks, prominences, half-prominence widths). With 21 or more peaks, any prominence beyond the tenth above 7.5 % of the best stops the search; the most prominent peak then sets the BPM, and the search continues only if more than half the peaks exceed 75 % of the best.
    - The search also stops when a whole BPM falls within [start, start + 0.1], or when the best leaves 70.007–179.982.
  - **`beatPositionAdjust` @0x10113b7d4.** Sums the high band around every inner beat, overall and per beat index mod 4.
    - The overall sum's 32-sample moving average gives the rise start (`findBeatPositionForBeatSubAll`): the 8th consecutive rising sample after the last dip before the main peak, between the prominent peaks; else the sum's maximum.
    - Each per-bar sum gives a candidate (`SubEach`) with reliability checks: a dip below 70 %, 20+ samples over a quarter of the range or a ratio ≥ 10, a slope/variance floor, and a min/max shape.
    - The latest reliable per-bar rise replaces the overall one when it is more than 34 samples later.
    - `offsetBeatPosition` @0x10113d304 shifts all beats by (index − w)·4 samples, drops beats before 0 and beats whose next shift would pass the end, and refills whole beat lengths at both ends; the beat length is `trunc(rate/4 · (0.004 · tempo))`, single precision.

## 9. Open
- **Modes 2 and 3:** the BPM-range rebuild (`bpmToDouble`/`bpmToHalf`/`renumberBeat`); Timelock's varied BALib list in mode 2 (Focus only takes the count and BPM there, so the grid is unaffected); where +0x3940 follows the verdict.
- **BALib v2's beat count** is approximated by the presence of our Normal-mode (v1) grid. This only matters if one finds beats and the other does not.
- **Mode 1** (Normal-mode Dynamic, the BALib v1 varied analyser) is ported and verified separately: `analysis-beats.md` § 9.
- **Stale slot:** what the whole-run start alignment's one-past-the-end read holds. This happens only near the end of the beat vector.
