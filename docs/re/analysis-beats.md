# Beat grid and BPM, Normal mode (BALib v1)

Status: **verified** for BALib's own output. `mb_analysis::analyze(…, Options { beats: true, .. })` returns `BeatGrid { bpm, beats }`, i.e. `djmdContent.BPM` and the PQTZ/PQT2 beats. Against `fixtures/oracle-a` (Normal mode, high precision off, BPM range 70–180), on all 161 tracks rekordbox analysed itself (those with a PQT2):
- BPM × 100 is exact on 161/161;
- the beat count is exact on 161/161;
- PQTZ and PQT2 are identical on 160/161, every beat to the millisecond and every µs remainder.

BALib's own output matches on 140/161. In the other 21, rekordbox's **phrase pass** then moved the whole grid or renumbered its bars (§ 8). That pass's downbeat stages are ported (`analysis-phrase.md`, `Options.phrase`, on by default), and 20 of those 21 now match. The one miss is `wav_96k32f/click_128_offset1234`: we move it, rekordbox did not (`analysis-phrase.md` § 6).

Normal-mode Dynamic (analysis mode 1) is verified on oracle-e: PQTZ, PQT2 and BPM 166/166 (§ 9).

- Scorecard: `docs/scorecards/beats.json` (Dynamic: `docs/scorecards/beats-dynamic.json`).
- Code: `crates/mb-analysis/src/beats/` (`sensor.rs`, `info.rs`, `envelope.rs`, `adjust.rs`, `filters.rs`, `mod.rs`).
- Tests: `tests/oracle.rs` `beats_match_rekordbox`, `tests/synthetic.rs` `click_track_grid`.
- Scorer: `examples/beat_score.rs`.

## 1. Where it runs
1. **Job dispatch.** `AnalyzeJobBeatDetector::Detect` @0x102327c80 reads the special-info mode (+0x18 → +0x70):
   - modes 2, 3 and 4 run `highPrecisionBeatAnalyze` @0x102327e50;
   - every other mode runs `legacyBeatAnalyze` @0x102328684.
   Normal mode is mode 0 (thread +0x38d0 = 0), so it takes the legacy path.
2. **BALib.** `BALibWrapper::SetVersion(2)` runs only in modes 2–4 (`analysis-waveforms.md` § 1). Normal mode therefore uses `BeatAnalyzer_1_0`.
   - "Varied" (dynamic) analyzers belong to modes 1 and 4. Normal mode creates the constant one: `BA_CreateConst44k` @0x10115629c / `BA_CreateConst48k` @0x10115618c.
   - These call `Analyzer::create(rate, total = L − 1, beatFlag, varied = false, loud, dots, zoom)` @0x101136258.
   - The analyzer accepts only 44100 and 48000 Hz; other rates arrive already resampled (`decode.md` § 2).
3. **Input.** `Analyzer::setSample` @0x1011364ec takes the same 16-bit analysis samples as the waveforms, in blocks of `rate/75` frames:
   - `BeatSensor::put` @0x101137234 gets the mono mix `(R + L) · 1.5258789e-05` (f32);
   - `UnitEnvelopeMaker::putSample` @0x101138168 and `UnitBeatAdjust::putSample` @0x101133300 get the interleaved shorts.
4. **End of input.** When the sample count reaches `total`, `Analyzer::analyze` @0x101136890 runs:
   1. `BeatInfoAnalyzer::analyze(info, out, total, 4, 16, 12, varied)` @0x10118ff2c returns the BPM (f32) and the beat list. The three ints come from Analyzer +0x28 = 32.
   2. `UnitEnvelopeMaker::analyze(bpm, out, varied)` @0x101138480.
   3. `UnitBeatAdjust::analyze(&bpm, out)` @0x101133528 (only when not varied).
   The local `BeatInfoAnalyzer` is built with `{1024, 48000}`, and `analyze` then stores the real rate at its +4 (`str w8, [sp, #0x1c]`).
5. **Job side.** `legacyBeatAnalyze` reads the results:
   - `BA_GetBpm` = `(int)fmaf(bpm, 100, 0.5)` (`getBpm` @0x101136a00);
   - `BA_GetBeatList` @0x101156380 gives `{pos, beatInBar (+0xc), (int)(6e6f/tempo + 0.5f)}` per beat;
   - then `convertToMicrosec` @0x102328b20 and `getFirstBeatPos` @0x102328a00 (§ 7).

`peakss::BeatInfo` (24 bytes) fields:

| Offset | Field |
|---|---|
| 0 | int `pos` (samples) |
| 4 | f32 `beats` |
| 8 | f32 `tempo` (ms per beat) |
| 0xc | int `stat` (sensor stamp; later the beat's place in the bar, 1–4) |
| 0x10 | int `band` (0–5 or −1) |
| 0x14 | u8 `flag` |

## 2. Onsets: `peakss::BeatSensor` (`sensor.rs`)
`BeatSensor::create(rate, 0, total, 8, 6)` @0x10118f508 sets up six bands. The BALib v1 `IirPassFilter` designs and `operate` are instruction-identical to v2 (`mb_dsp::IirPassFilter`, `analysis-waveforms.md` § 3.2).

| Band | Source |
|---|---|
| 0 | low-pass 283 Hz, Q 1/√2 |
| 1 | band-pass W 150 Hz, 1.5 octaves |
| 2 | 283–7900 Hz |
| 3 | high-pass 7900 Hz |
| 4 | copy of band 0 (rectified) minus its moving average over `max(2, (int)(rate·0.001 + 0.5))` samples, floored at 0 (`fmsub` then `fmaxnm`) |
| 5 | the same on band 3, over `rate·0.0002` samples |

Bands 0–3 are rectified after filtering.

**Envelope tracker.** Every 8 samples (a tick), each band's 8-sample mean feeds a peak tracker. Its constants:
- hold 0.29 s;
- floor `(float)exp10(−3)`;
- threshold `0.75·peak` after a new peak;
- decay to `0.5·peak` over 1.3 s, then to `0.5·threshold` over 1.3 s, restarted 1.7 s after a peak.

A peak is confirmed when it has stayed the maximum for the hold time. Its sample position goes into that band's 32-entry ring, along with the gap since the previous peak in ms: `(float)(u64)Δ · 1000 / (float)rate`. The tick at sample index `total − 1` is never processed.

**Tempo from a gap: `PeakBuffer::calcBeat` @0x10118ef24 / `calcBeat_s` @0x10118f010.**
1. The gap is read as `k` beats under one of nine `(lo, hi, k)` patterns (table @0x1037c86c0). The earlier gaps are matched against the table @0x1037c872c, each within `tol(u) = (float)min(u·0.04, 20)` of the unit `u = gap/k`.
2. The beats add up until they equal 4, or 8 when the pattern has `k ≥ 4`. They may double once to 16.
3. The tempo is `Σgaps / target`.
4. Then the count of further earlier gaps whose quarter-beat-quantised length fits that tempo. The quantisation uses `trunc(fma(4e, 1/t, 0.5))/4`, rounded to whole beats above 2.5 and capped at 4.
5. `calcBeat` tries every pattern and keeps the tempo ≥ 333 ms with the most agreeing gaps.

**Voting: `selectBand` @0x1011378d8.** Each band with a fitting peak votes into a 256-bin BPM histogram (`(int)(60000/t + 0.5)`), weighted `max(1, (int)(4/(err + 1)))`. The band with the smallest error is the candidate.
- When the histogram mode is not the candidate's tempo, or its double or half, bins unrelated to the candidate are cleared. This repeats until they agree, or it gives up when the candidate's bin holds fewer than 3 votes.
- Bands that disagree with the result lose their flag.
- Histogram bounds start at (min 200, max 40).

**Raw beat list: `BeatListSignal::changeStat` @0x101136ba0.** For each remaining band, it files the current peak and the `agree + 1` peaks before it into the sorted list:
- `beats` = quarter-quantised gap;
- `tempo` = the band's tempo;
- `stat` = call number << 8 | (agree + 2);
- `band` = the band.
At the same position, an entry whose tempo is unrelated or integer-related is replaced only by a stronger call; related ones are kept side by side. Reading back past the ring's filled part yields zero entries, so a beat at position 0 appears, as in rekordbox.

## 3. Tempo and first grid: `BeatInfoAnalyzer::analyze`, constant path (`info.rs`)
1. **Coarse tempo.** Integer BPM histogram (400 bins) over all beats, then a BPM × 10 histogram (4000 bins) within ±1 BPM of the winner. Both take the first maximum. `T0 = (float)(600000.0/bpm10)`.
2. **Filter.** A beat is kept when its tempo, double or half lies strictly inside `T0 ± tol(T0)`, or inside `p ± tol(p)` of the previous beat's tempo `p`. The first `p` is `tol(T0)` itself.
3. **One segment.** The constant path always has exactly one segment, starting at the first kept beat. Its tempo is re-estimated from the filtered beats (`beats = 0`, `stat = 4`). The segment-merge loop (@0x101191610) only ever removes segments.
4. **Merge close beats.** Beats closer than `(int)((double)(T·(float)rate)·0.000125)` samples are merged. The one from the stronger sensor call wins (strength = low byte of the first `stat` with that call number).
5. **The returned BPM** is `60000 / first beat's tempo` after `gapCorrect` (step 8). A pre-`segAdjust` value is computed and discarded.
6. **`segAdjust` @0x1011944ec.** It searches:
   - tempo offsets 0, +1, −1, +2, −2 ms;
   - spacings of 4, 2 and 1 beats, scored at the same scale;
   - phases in 96-sample steps.
   For each grid it walks the beats, snapping to the nearest raw beat within `w = max(rate/20, (int)(P/8))` (`P = rate·0.001·T`). The score is `Σ(w − |d|)`.
   - The first tempo stops early once `score·m > w·(len/2)/P`.
   - The result is phase (mod P), tempo, spacing·4 (16/8/4) and the number of periods to the first matched beat.
7. **Grid walk** (@0x101192090–0x101193a48).
   1. **Anchor.** The first matched beat plus its band's onset lag. The lags are in ms: `{−30, −20, −3, −3, −20, −3}` by band, else −3.
   2. **Cursor.** Quarter-beat steps. At multiples of the spacing, the cursor snaps to the nearest raw beat.
   3. **Every 32 beats.** The beat length is re-estimated from the drift: `P' = fma(lag, rate/1000, cur − anchor)/32`, `T' = (float)(P'·1000/rate)`.
   4. **Output.** The first window writes:
      - back-fill beats before the anchor;
      - the anchor (`flag` 1, band of the snapped beat);
      - 16 beats, then 4 more.
      Each later window writes 4 beats. The anchor moves 4 beats on, and the band table slides by 16 quarters from index 64 on (as in the binary). A partial last window uses `·4/quarters` instead of `/32`.
   5. **Bands.** Beats on a spacing multiple carry the band snapped there. The first and the last of them (indices `first`, `last`) feed step 8.
8. **`gapCorrect` @0x101194e7c.** Replaces the list with a constant grid through beats `first` and `last`:
   - `period = (x_last − x_first)/(last − first)`;
   - start `x_first − period·trunc(x_first/period)` (fused);
   - the grid is laid from there to `total`, with `T = (float)(period/(rate·0.001))`.
9. **Bars.** The beat at the first band-confirmed position is beat 1. Beats outside `[0, total]` are dropped.

## 4. Envelope filters (`filters.rs`)
**`LowPathFilter2D` / `HighPathFilter2D`** (double), from `(s, c) = sincos(f·2π/fs)` and `α = s·0x3fe6a09e667f3bcd`:
- low-pass: `b0 = b2 = fmsub(c, ½, ½)·inv`, `b1 = fmsub(c, inv, inv)`;
- high-pass: `b0 = b2 = fma(c, ½, ½)·inv`, `b1 = −(c + 1)·inv`;
- both: `a1 = (c·−2)·inv`, `a2 = fmsub(α, inv, inv)`, with `inv = 1/(α + 1)`;
- output: `y = fma(x2, b2, fma(x1, b1, x·b0)) − fma(y2, a2, y1·a1)`.
The two `operate` functions are instruction-identical.

**`HighPathFilter1D`**: `w = f·2π/fs`, `b0 = 2/(w + 2)`, `b1 = −b0`, `a1 = (w − 2)/(w + 2)`, `y = fma(−y1, a1, fma(x1, b1, x·b0))`.

## 5. Downbeat and half-beat check: `UnitEnvelopeMaker` (`envelope.rs`)
1. **Envelope.** Blocks of 128 frames of `(R + L)/2` (double) go through `LowPathFilter2D(150 Hz)` twice. Only the **first** sample of each block continues: rectified, then `LowPathFilter2D(5 Hz, rate/128)`, then `HighPathFilter1D(0.5 Hz, rate/128)`. It is stored as `(short)(int)y`.
2. **`analyze`.**
   1. **Grid.** The beats go to envelope indices `(int)(pos/128 + 0.5)` with lengths `((t·0.001)·rate)/128`. When the first beat is later than 0.9 beat (`pos/128 > (rate·0.0009·t)/128`), index 1 is prepended and the last beat dropped ("late" layout).
   2. **Onsets.** Peaks of the envelope: a run is entered when rising and ended when it falls below max/1.5 or hits 0. Peaks at least as high as their mean are onsets.
   3. **Patterns.** Gaps of 2.5 + 1.5 or 2.5 + 2 beats count as "swing" (≥ 11 needed). Gaps of 1.5 beats count as "dotted" (≥ 81 needed; one-beat-one-beat pairs count against).
   4. **Half-beat check.** The onsets (or the pattern onsets), each taken 110 ms earlier (`(int)fma(rate·0.11, 1/128, 0.5)`), are compared with the nearest grid beat; "nearest" uses beat midpoints. If more than half are over ¼ beat away, the grid moves by half a beat: forward when the first beat is ≤ 0.45 beat + 0.5, else back.
   5. **Downbeat.** Each onset adds its peak height to the slot `beat index mod 4` of the nearest grid beat:
      - the first onset ×2 when the next onsets sit one or two beats later;
      - onsets after a gap of over 7 beats, if followed by a 2-beat gap or above 0.6× the loudest;
      - 983.04 per pattern onset.
      The best slot (first maximum) becomes beat 1.
   6. **Write-back.** Positions move only after a half-beat shift, by `128·pos_e[1 or 2] − pos[1]`. Bar numbers are rewritten. One beat is appended or dropped at the end, one dropped (< 0) or prepended at the start.

## 6. Tempo refinement and grid placement: `UnitBeatAdjust` (`adjust.rs`)
1. **Envelope.** Every fourth frame (`rate/4`), `(R + L)/2` → `HighPathFilter2D(2 kHz)` → abs → `LowPathFilter2D(1 kHz)`, clamped to 0..32767. At most 900 s are kept (`cap = min(total/4, rate/4·900)`).
2. **Beat count.** `analyze` takes the beat count. When the envelope is full, it uses the beats in the first 900 s instead.
3. **`bpmAdjust` @0x1011335f0.** Candidates are clamped to 70–180 BPM (fmaxnm/fminnm). Each candidate `c` gets a grid from `pos[0]/4` at `period = rate/4·60/c` (`createBeatData` @0x101134c14). `manageWave` @0x101135098 scores it: the maximum of the envelope summed over ±`(int)(rate/4·0.05)` around beats `1 … n − 2`. The best is the first strictly greater score.
   1. 17 steps of 0.002 from −0.016 (`fma(j, 0.002, bpm − 0.016)`).
   2. If the best index ≤ 2: 9 steps from −0.034, then if the best index is 17–19: −0.05 … −0.036.
   3. If the best index is 15–17: 9 steps from +0.018, then if it is 24–26: +0.036 … +0.05.
   4. Finally ±0.001 around the winner, each only within ±0.05 BPM of the start.
4. **`reflectBeatData`.** When `(int)(bpm·1000)` changes, the winning grid replaces the beats (`pos = q << 2`, `tempo = (float)(60000/bpm)`), extended or trimmed to its length. The new BPM is `(float)best`.
5. **`beatPositionAdjust` → `offsetBeatPosition`.** The beats are moved onto the peak of the summed envelope window: `d = (peak − w)·4` samples. When that start would fall under one beat, or `d` exceeds 20·`(int)(rate/4·−0.001)`, a 1 ms bias is applied. Then beats < 0 and past the end are dropped, and beats are prepended and appended a beat apart while they fit (`beat = (int)((float)(rate/4)·(0.004f·t))`).

## 7. Job and thread side (`mod.rs`)
1. **`convertToMicrosec`.** `µs = pos·10⁶/rate` (64-bit integer), `ms = µs/1000`, `sub = µs − 1000·ms`. Each `analyzer::DetailedBeatInfo` is `{u16 beatInBar, u16 tempo (BPM×100), u32 ms, u16 sub}`.
   - **BPM range.** `bpmRangeList` @0x10387968c holds pairs, index = special info +0x7c (reset to 1 when outside 1–10):
     - 0 and 1: 70–180;
     - then 48–95, 58–115, 68–135, 78–155, 88–175, 98–195, 108–215, 118–235, 128–255.
   - When the BPM falls outside the range, `bpmToDouble` @0x102329060 / `bpmToHalf` @0x10232967c rebuild the list. With range 70–180 this cannot happen: `bpmAdjust` clamps to 70–180 and `BeatInfoAnalyzer`'s tempos lie in 333–860 ms. So neither is ported.
2. **Writing.** `CBeatAnalyzeThread::notifyBeat` @0x1001595c8 writes:
   - PQTZ: `{beatInBar, tempo, ms}` per beat;
   - PQT2 with type 1 (`analysis_type` 1 for modes 0 and 3; 2 for modes 1 and 4, and for mode 2 with +0x3940). The PQT2 per-beat u16 is the µs remainder `sub` (now verified, `anlz.md` § PQT2). Header first/last/checksum as `ExtBeatGrid::from_beats`.
3. **DB BPM.** `djmdContent.BPM` is `BA_GetBpm` (BPM × 100). In oracle-a it equals every beat's PQTZ tempo on all 169 tracks, and ours do the same.

## 8. The phrase pass moves the grid (ported; full spec in `analysis-phrase.md`)
In oracle-a, phrase analysis was on. After the beat job, `CBeatAnalyzeThread::analyze` @0x1001528c4 (around 0x100155xxx, decompile lines ~1890–2150) does the following.

1. **Inputs from the job result** (`AnalyzeJobController::GetResult(…, 3)`, an `AnalyzeJobBeatDetectResult`):
   - the `DetailedBeatInfo` array;
   - BPM × 100 (+0x18);
   - first beat ms `fb` and first bar `fbar`. When the beat count is < 8 these are the result's +0x38/+0x40. Otherwise `fb` is beat 0's ms and `fbar = convFirstBar[beatInBar − 1]` with `convFirstBar = {1, 4, 3, 2}` (0x10387967c; `getFirstBeatPos` is the job-side twin). When beat 0's tempo differs from the BPM and `fb` lies between 1 and 2 beats, `fb` moves back one beat and `fbar` steps back.
2. **`SongStructureAnalyze`.** `Initialize(total, rate, bpm×100, fb, fbar, …)`, then `Analyze_AB` on the decoded audio again, in `rate·128/75`-frame chunks, until it reports stage ≥ 3 (`SS+0xa0`). It runs only when more than 7 beats follow `fb`, `0 ≤ fb ≤` one beat, `fbar > 0`, the track has at most 2 channels and is under 7201 s.
3. **The shift.** When the thread flag +0x3908 is set (inferred: phrase analysis on; the setter is not located) and the grid is not empty, it compares the result's first downbeat `SS+0x200` (ms) and bar `SS+0x204` with `fb`/`fbar`:
   - **Same bar** (or `SS+0x204 − 1 > 3`) and same ms: nothing.
   - **Different ms:** every beat's `time_ms += SS+0x200 − fb`. The µs remainders, tempos and count stay.
   - **Different bar:** `beatInBar` is renumbered from `convFirstBar[SS+0x204 − 1]`.
   - **`Analyze_AB` unfinished** (`SS+0xa0 < 3`): as decompiled, the target becomes −1 ms (skipped when `fb` is −1).
   Then `notifyBeat` writes PQTZ/PQT2 again.
4. **Effect on oracle-a.** 21 of 161 grids moved this way:
   - `click_128_offset1234` ×7: +936 ms, two beats, onto the first kick at 1.234 s. Our grid starts at 296 ms, rekordbox's at 1232 ms.
   - `sweep_20_20k` ×9: −438 ms and renumbered.
   - 22.05 kHz `tempo_120_to_126.wav`: +236 ms.
   - `bands_100_1k_8k.wav`: −320 ms.
   - `Oliver Schories – Circles` (m4a): −366 ms.
   - `Aura Vortex – Futura` (flac): renumbered only.
   The 5 `click_128_offset1234` copies rekordbox did not move equal ours exactly: the three MP3s, the AAC m4a and the 96 kHz WAV. So this pass is the only remaining difference. Why it left those five alone is part of the phrase analyser.
5. **Port.** `mb_analysis::phrase` runs `SongStructureAnalyze` stages A/B on the decoded audio with `(bpm, fb, fbar)` from `BeatGrid` and applies `(first ms, first bar)` to `BeatGrid::beats` (`phrase::run`). With it, 160/161 grids are identical. Our port also leaves the three MP3 copies and the AAC copy of the click probe alone, but moves the 96 kHz WAV, which rekordbox left. The rest of the analyser (stages C/D, PSSI) is paused; see `analysis-phrase.md` § 9.

## 9. Dynamic mode (analysis mode 1, "varied")

Status: **verified.** Against rekordbox's own BALib v1 analyser run on our decoded stream, on all 166 oracle-e tracks: every intermediate list of `BeatInfoAnalyzer::analyze`, its output (exact floats) and the final `BA_GetBeatList` with `BA_GetBpm` are identical. The same check on the constant path is also 166/166. That analyser reproduces oracle-e's PQTZ, PQT2 and `djmdContent.BPM` on 166/166, so the port does too (`docs/scorecards/beats-dynamic.json`).

- **Mode.** `trackAnalysisMode` = 1 with `HighPrecisionBeatAnalyze` = 0 is analysis mode 1. `BALibWrapper::Create` @0x101f543dc then builds `BA_CreateVaried44k/48k` @0x101156214/0x101156104, i.e. `Analyzer::create(…, varied = true)`. The window argument is 32 as in Normal mode.
- **`Analyzer::analyze` @0x101136890.**
  - `BeatInfoAnalyzer::analyze(…, varied = true)` returns the BPM of the longest tempo segment.
  - `UnitEnvelopeMaker::analyze(bpm, beats, true)` skips the half-beat check (§ 5).
  - There is no `UnitBeatAdjust`, so `BA_GetBpm` is that segment BPM, ×100 rounded.
- **Thread side.**
  - PQT2 gets `analysis_type` 2 (§ 7).
  - The BPM range check is skipped; with 70–180 it cannot trigger anyway.
  - The phrase pass does not shift the grid (`urahaku` off in modes 1 and 4, `analysis-phrase.md` § 1.5).
- **Code.**
  - `beats::BeatAnalyzer::new_dynamic`; `beats/varied.rs` (the front end); `beats/info.rs` (the shared middle, now over several segments). The constant path is the one-segment case of the same code.
  - `Options.dynamic`: high precision wins over it; `Analysis.grid_type` is 2.
  - `mb_library::AnalysisSettings.dynamic` comes from `trackAnalysisMode` = 1 (`dynamic_from`).
- **End to end against oracle-e** (`mb_analysis::analyze`, `mb_library::analyse_track`):
  - PQTZ, PQT2 and BPM identical on 166/166;
  - with the phrase pass, PSSI identical on 160/161. The miss is the 96 kHz click probe, the input-side difference of `analysis-phrase.md` § 10.

### 9.1 Front end (@0x1011900d0 … @0x101191588)
1. **Call groups.** Every raw beat belongs to the sensor call that filed it (`stat >> 8`). A group's tempo is its first beat's tempo. An empty group takes a later beat of its own whose tempo (or double, or half) lies strictly inside `t ± tol(t)` of the next group that has a tempo (`tol` as in § 2). Leading groups without a tempo are dropped with their beats.
2. **Group runs.** The groups after the first are walked with a run tempo `a`, a run length `c` and the previous run (tempo `pa`, length `pc`):
   - a group of tempo 0 is dropped;
   - a group fitting `a` extends the run (`a` becomes its tempo);
   - otherwise, a run of 3–8 groups whose next group does not fit `pa`, or any longer run, stands and becomes the previous run;
   - a shorter run, or a 3–8 group run whose next group returns to `pa`, is dropped. The walk then continues from the previous run, or starts a new run at the group;
   - at the end, a last run of one group after a standing run is dropped.
3. **Beat runs.** The same walk over the beats themselves, with these thresholds: runs of up to 8 beats are dropped; runs of 9–32 beats stand unless the next beat returns to the previous run's tempo; longer runs always stand. A trailing run of up to 8 beats after a standing one is cut off.
4. **Segments.** From each beat `i`, the integer-BPM histogram (400 bins, as § 3 step 1) of the beats within `8 · rate` samples gives the window's BPM `b` and beat length `T = 60000/b`. When `T` fits the previous window's `T`, the walk moves to `i + 1`. Otherwise a segment starts:
   - its tempo is that of the first beat from `i` whose own integer BPM is `b`;
   - its start is the earliest beat from `i` whose integer BPM lies in the BPM range of `T/2`, `T` or `2T` (for a centre `x`: `[60000/(x + tol(T) + ½), 60000/(x − tol(T) + ½))`, truncated);
   - the walk continues after the start.

### 9.2 Shared middle (both paths)
1. **Segment tempos.** Each segment's tempo comes from the BPM histograms (§ 3 step 1) of the beats from its start, within its span: up to the next segment, or `total` for the last. When it fits the previous segment's tempo, the segment is merged into the previous one, which is then estimated again.
2. **Close beats** (§ 3 step 4) are merged per segment, with that segment's tempo, in one pass over the list.
3. **Representative BPM.** That of the longest segment; the first one's length counts from sample 0. The constant path computes and discards it.
4. **`segAdjust`** runs per segment. For each tempo trial with beat length `P`, the search runs:
   - from one eighth beat after the beat before the last beat at or before the segment's start, clamped to the previous segment's start, the segment's start and 0 (0 for the first segment);
   - to the same bound built at the next segment's start, clamped to `[start of this segment, start of the next]` (`total` for the last).
   The adjusted segment is stored in place, so the next segment's search starts from it.
5. **Grid walk** (§ 3 step 7) per segment, up to the segment's end:
   - back-fill stops at the last beat of the earlier segments;
   - the window bookkeeping starts at the output count so far;
   - the first/last band-confirmed beats and the first anchor of segment 0 are kept across segments.
6. **Varied `gapCorrect`** @0x101194b00 replaces § 3 step 8. It walks four consecutive beats `a b c d` with gaps `g1 g2 g3`, `M = max(g1, g3)`, `m = min(g1, g3)`:
   - **`g2/M > 1.1`:**
     - `M = g1` and `(g2 − M)/(M − m) < 4`: `c = b + M`;
     - else `m = g1` and `fma(M, 3, g2) > 5m`: `c = b + m`, and a beat `b + 2m` is inserted before `c` when `b + 3m < d`;
     - else the span `a…d` is split into thirds (`(g1+g2+g3)/M < 3.3`), or into quarters with a beat inserted before `c`.
   - **`g2/m < 0.9`:**
     - `m = g1` and `(m − g2)/(M − m) < 4`: `c = b + m`;
     - else `M = g1` and `fma(m, 4, g2) < 4M`: `c = b + M`, or `c` is dropped when that reaches `d`;
     - else `(g1+g2+g3)/m ≤ 2.7`: `b = (a + d)/2` and `c` is dropped;
     - otherwise thirds.
   The insertions put the new beat before `c`, as the binary does. None occur in oracle-e.
7. **Bars** (§ 3 step 9). The mark is segment 0's first anchor in the varied path (`out[first]` in the constant one). Beats outside `[0, total]` are dropped.

### 9.3 Verification
- A harness outside the repo runs rekordbox's BALib v1 (`BA_CreateVaried*` / `BA_CreateConst*`, `BA_SetSample`, `BA_GetBeatList`) on our decoded stream. A hook turns on the analyser's own debug log, which dumps 9 intermediate lists.
- `beats::vtest` (`MB_V1_DIR`, `cfg(test)`) compares every stage and the final list with it: 166/166 in both modes.
- `mb-library` `analyse_ops::dynamic_mode_matches_oracle_e` checks the files (`MB_ORACLE_FULL=1`: 166/166).
- 138 oracle-e tracks have one segment, 19 two, 7 three, one four, one five.

## 10. High precision (modes 2–4)
`highPrecisionBeatAnalyze` @0x102327e50 runs BALib v2 and `madmom::analyze` @0x10230a820: a TensorFlow beat activation (TFWrapper, dlopen of the user's install) plus the downbeat HMM. Mode 4 (Dynamic) is ported and verified (`analysis-beats-hp.md`). Modes 2 and 3 need BALib v2's own analysis in full; that port is in progress (`analysis-beats-hp.md` § 8).

## 11. Open questions
- Where the thread flag +0x3908 is set (phrase analysis setting assumed).
- `SongStructureAnalyze`'s first-downbeat logic (§ 8).
- Uninitialised memory: the grid walk's band table is `new int[129]` without zeroing, and a partial first window can read past the quarters written. We use −1. Only possible on grids shorter than ~32 beats after the anchor.
