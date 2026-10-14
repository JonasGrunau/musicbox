# High-precision beat analysis (madmom + BALib v2 Focus)

Status: **verified for Dynamic mode** (analysis mode 4: `HighPrecisionBeatAnalyze` = 1, `trackHiPrecisionBeatAnalysisMode` = 1). With `Options { high_precision: Some(HpMode::Dynamic), .. }`, `mb_analysis::analyze` reproduces rekordbox 7.2.19 on every `fixtures/oracle-b` track that rekordbox gave a high-precision grid (phrase pass on, as in oracle-b):
- PQTZ 154/154;
- PQT2 154/154;
- `djmdContent.BPM` 154/154.

The other 12 tracks (sweeps) get no high-precision grid, in rekordbox as in musicbox (§ 6). Normal (mode 3) and Auto (mode 2) are not ported (§ 8). Scorecard: `docs/scorecards/beats-hp.json`.

Code: `crates/mb-nn` (TensorFlow runtime); `crates/mb-analysis/src/hpbeat/` (`madmom.rs`, `hmm.rs`, `focus.rs`, `mod.rs`).

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
  - musicbox: `phrase::run(…, urahaku)` is called with `grid_type != 2`.
- **PQT2 type.** `CBeatAnalyzeThread::notifyBeat` @0x1001595c8 passes the PQT2 `analysis_type` to `notify_detailed_beat_info` (then `MstStoreExtBeatGrid`): 2 for modes 1 and 4, 2 for mode 2 when +0x3940 is set, else 1.
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
- **Without hendou** (modes 2/3, not ported). Model 8 runs once over 60 s from `offset`. When less than 60 s remains after `offset`, it runs over the last 60 s instead (the whole track when shorter).
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
- **Paths.** With the hendou flag, or when the largest bar number among the first four madmom beats is not 4: two passes, then two clean-up rounds. Otherwise a separate "constant" path runs. Only modes 2/3 can reach it, and it is not ported.
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

## 7. musicbox
- **Options.** `Options.high_precision: Option<HpMode>`.
  - `hpbeat::grid` runs madmom on the `(L + R)/2` samples gathered during the decode pass.
  - Focus is then seeded with the Normal-mode grid's beat count and BPM. Only non-zero matters, which stands in for BALib v2's count (§ 8).
  - `Analysis.grid_type` is the PQT2 type.
  - Normal and Auto keep the Normal-mode grid and set `Analysis.high_precision_error`.
  - The model is loaded once per process.
- **No beats.** `Analysis.beats` is `None`. `mb-library` then sets `TrackAnalysis.keep_old_grid`: `ApplyAnalysis` carries the previous PQTZ/PQT2 over and writes `BPM` 0 (`ops/analyse-track.md`).
- **Preferences.** `AnalysisSettings::high_precision_from(HighPrecisionBeatAnalyze, trackHiPrecisionBeatAnalysisMode)`, with the binary's defaults 0 and 2.
- **No phrase shift for a Dynamic grid** (urahaku, § 1). Without this gate, 57 of the 154 grids would move.
- **Cost.** About 4–5 s per track, with three tracks in parallel on this machine.
- **Tools.**
  - `cargo run --release -p mb-analysis --example hp_score`: oracle-b scoring.
  - `hp_check {table, madmom, focus, focusin}`: developer checks against harness dumps.

## 8. Open
- **Sweep fallback** (in progress): when high precision finds no beats, rekordbox runs key and phrase on the stored grid (§ 6). `analyse_track` has no previous grid today, so our own pass writes no PSSI and takes a whole-track key there.
- **Modes 2 and 3:**
  - BALib v2's own beat analysis (`GetBpm`, `GetOffsetCalc`, `GetBpmHendou`);
  - model 8 over the 60 s window;
  - Focus's constant path;
  - the BPM-range rebuild.
- **BALib v2's beat count** is approximated by the presence of our Normal-mode (v1) grid. This only matters if one finds beats and the other does not.
- **Mode 1** (Normal-mode Dynamic, the BALib v1 varied analyser): `analysis-beats.md` § 9.
- **Stale slot:** what the whole-run start alignment's one-past-the-end read holds. This happens only near the end of the beat vector.
