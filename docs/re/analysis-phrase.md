# Phrase analysis (`SongStructureAnalyze`)

Status: **verified: all four stages ported; PSSI produced and written.**
- **Stages A and B are verified.** The downbeat shift moves the beat grid the way rekordbox does: 160/161 oracle-a grids are identical (PQTZ + PQT2) (§ 6).
- **Stages C and D are verified bit for bit** against rekordbox's own routines run on the same input (§ 9): stage C's outputs on 161/161 tracks, every stage-D callee on every recorded call, and the whole result struct on 161/161 tracks.
- **PSSI against the fixtures** (`docs/scorecards/phrase.json`): oracle-a 159/161 identical with our own analysis (the two misses are 96 kHz probes); oracle-b (high-precision Dynamic, with the stored grid where high precision finds no beats, § 1.1) 159/160 with our own grid and 159/160 on rekordbox's stored grid.

Code: `crates/mb-analysis/src/phrase/`.
- Driver and stages A/B: `mod.rs` (grid anchor, preconditions, shift, [`run`]), `filter.rs`, `mat.rs`, `rhythm.rs`, `snare.rs`, `power.rs`, `beatpos.rs`.
- Stage C: `stage_c.rs`, `feat.rs`, `hp.rs`, `spec.rs`.
- Stage D: `stage_d.rs` (`Sub_Main_D`), `d_feat.rs`, `genre.rs`, `blocks.rs`, `intro.rs`, `verse.rs`, `melo.rs`, `adjust.rs` (labels → result struct, `PhraseResult`).
- `Options.phrase` (default on, needs `beats`) runs A/B and the shift inside `mb_analysis::analyze`, then C and D on a second read of the file; `Analysis.phrase` holds the result when rekordbox would store it.
- `mb_library::analyse_track` writes it as PSSI into the `.EXT` and sets `ContentLink` bit 0x10000 (§ 8).

Tools:
- `examples/beat_score.rs`: grids against oracle-a.
- `examples/phrase_score.rs`: PSSI against oracle-a/b, from our own analysis and from rekordbox's stored grid.
- `examples/phrase_debug.rs`: runs B and C on one file; dumps the analysis stream and C's outputs for the harness.
- Tests: `tests/oracle.rs` `beats_match_rekordbox` (`MB_ORACLE_FULL=1` for all 161); `phrase::replay` (§ 9); `mb-library` `analyse_ops` (PSSI in the `.EXT`).

## 1. Where it runs
`CBeatAnalyzeThread::analyze` @0x1001528c4 runs this after the beat job has written its grid.

### 1.1 Anchor
- `fb` = beat 0's ms; `fbar = convFirstBar[beatInBar − 1]`, with `convFirstBar = {1, 4, 3, 2}` (0x10387967c).
- With fewer than 8 beats both come from the job result instead.
- When the high-precision job finds no beats, the thread takes the stored grid and its BPM (`getBeatListFromDatabase`); key and phrase run on it (oracle-b's 12 sweeps: the old Normal-mode grid, BPM 102.50, while the BPM column becomes 0). Code: `mb_analysis::analyze_with`, which takes the stored grid from the caller.
- When beat 0's tempo field differs from the BPM × 100 and `beat < fb < 2·beat` (`beat = 6e6 / bpm100` ms), `fb` drops by one beat (truncated) and `fbar` steps back one (1 → 4).
- Code: `phrase::grid_anchor`.

### 1.2 Preconditions
The SS object (0x1d78 bytes, ctor @0x100441b98) is built only when all of these hold:
- the track has at most 2 channels;
- `(int)(bpm100/6000 · (total − 32 − rate·fb/1000) / rate) > 7`, i.e. more than 7 beats after `fb`;
- `total ≠ 0`, `rate ≠ 0`, `bpm100 > 0`, `fb ≥ 0`, `fbar > 0`, and `fb ≤` one beat;
- `total / rate < 7201`.

`total` is the analysed length `L − 1`, as for the beat job. Code: `phrase::runs`.

### 1.3 `Initialize` @0x1004436b8
Arguments are `(total, rate, bpm100, fb, fbar, key, !C/D-enabled, urahaku == 0, urahaku == 0, &result)` (`urahaku` = thread +0x3908, § 1.5). It sets:
- stage +0xa0 = 1;
- decimation ratios (+0x70, +0x74, +0x78) = (2, 8, 32);
- rates: fs = rate (+0x80), fs/2 (+0x88), fs/8 (+0x90), fs/32 (+0x98);
- total (+0xb8), fb (+0xa4), bpm = bpm100·0.01 (+0xa8), key 0–23 (+0xb0, `balibKeyIndex`), fbar (+0xb4);
- chunk +0xbc = `(int)(rate·1.7066666666666668 + 0.5)` (= rate·128/75);
- flags +0xc1..+0xc3.

It fails when `fb > 6e6/bpm100`, `fbar < 1` or `key > 23`.

### 1.4 Feeding
- The thread rewinds the reader and calls `Analyze_AB` @0x1004438d8 with `getStreamAt` blocks (L and R as int16 values in floats), until stage +0xa0 ≥ 3.
- Then, if C/D is enabled (thread +0x3902), A/B finished and +0xc0 is set (more than 30 beats after the first downbeat), it rewinds again and feeds `Analyze_CD` @0x100443c30 until stage +0xa0 > 4.
- Both buffer a mono mix and process it in chunks of +0xbc samples. Only the last chunk is short.
- The mix is `fma(R, 0.01/32768, L · 0.99/32768)` (constants 0x3e947ae147ae147b and 0x3effae147ae147ae).

### 1.5 Thread flags
- **+0x3908** ("urahaku", the grid-shift flag) is `AnalyzeJobManager`+0x10, written by `AnalyzeJobManager::SetUrahaku` @0x1022fb028. The thread sets it when beats are analysed and the analysis mode is neither 1 (Dynamic) nor 4 (high-precision Dynamic). When set, the downbeat shift is applied and `Initialize` clears +0xc2/+0xc3, so B uses its corrections. When clear (oracle-b), the grid is not shifted and B keeps `fb` and `fbar` (§ 4, steps 10 and 12). Code: `run`'s `urahaku` argument; `mb_analysis::analyze` passes `grid_type != 2`.
- **+0x3902**: C/D enabled.
- **+0x38e1**: `notifySongStruct` writes PSSI (§ 8).

## 2. Shared pieces
- **`c_filter`** (@0x100351a00–0x100351ba8): coefficients b0, b1, b2, a1, a2 and a two-value state.
  - `prewarp(w, fs) = tan(w/2fs)·2fs` (system `tan`).
  - `make2(p1…p6, fs)` is the bilinear transform of `(p1 s² + p2 s + p3)/(p4 s² + p5 s + p6)`, evaluated in the binary's order. `make1` is the first-order twin.
  - `filter2`: `y = fma(x, b0, s1)`, `s1' = fma(−y, a1, fma(x, b1, s2))`, `s2' = fma(x, b2, −(y·a2))`.
  - `filter1`: `y = fma(x, b0, s)`, `s' = fma(x, b1, −(y·a1))`.
  - Code: `filter.rs`.
- **Decimators**:
  - `resample2` (init @0x1002ca5b4) is four low-pass sections at `fc` with k = 1.96157…, 1.66294…, 1.11114…, 0.39018…;
  - `resample_hpf` (@0x102148ed4) is the same as high-pass;
  - `filter_8th_ds::downsample` @0x100ac3ea8 filters a chunk and keeps samples `r(k+1) − 1`, so every chunk restarts the decimation phase.
  - Code: `filter::Decimator`.
- **`MatArray` helpers** (`mat.rs`):
  - `sum` adds up to 7 values in order. Longer arrays go through 8 lanes reduced as `((a6+(a4+(a2+a0))) + (a7+(a5+(a3+a1))))`, then the tail in order. `square_sum` is the same with fused lanes.
  - `max_val`/`max_idx` take the first maximum; `min_val` the first minimum.
  - The `sort_*_idx` functions are stable sorts that also return 1-based original indices.
  - `link_from` fails only for a negative count.
  - `MatArrayFloat2` is row-major. Its `allocate` reuses the buffer only if the new size is strictly smaller.

## 3. Stage A: `Sub_Main_A` @0x101c28728
- `Sub_Main_A_init` @0x101c28678 sets the target length `min((int)(fs8·210), total/8)` and inits the resampler at `fs8·0.4`.
- Each chunk is decimated by 8 and appended (+0x1e0). After 210 s or the end of the track the buffer is resized to the target and stage B runs in the same call.
- Code: `PhraseAnalyzer::{new, put}`.

## 4. Stage B: `Sub_Main_B` @0x100b80b04
Works on the first 210 s at fs/8 (`x`). In order:

1. **`BeatsSamplesCalc`** @0x1008d4778.
   - `P = fs·60/bpm`, `off = (int)fma(fs·0.001, fb, 0.5)`, `n = (int)((len − off)/P)`.
   - 1-based beats `(int)fma(P, i, off + 1.5)`; the last is clamped to `len`.
2. **`RhythmPatternAnalyze`** @0x10222f334 (init @0x10222f000) → 16 steps per bar.
   - Envelope:
     - low-pass at 200 Hz: 4 sections at fs8 = 5512.5, 6000 or 4000; 1 section at 1378.125, 1500 or 1000; none at other rates;
     - then |·|, then a 10 Hz low-pass.
   - **Pre-roll search** (8 tries): take the strongest envelope samples after the first bar. Snap to the bar line. Follow the rise to its peak (or, for falling edges, back to it), then walk back to a valley:
     - the valley must be below `(min + max)/2`;
     - it must stay within 1.12× for `Pf/16` samples;
     - the peak must exceed 2.11× the valley.
     The result is how far before the bar line the onset starts.
   - The first-differenced envelope is shifted by that offset.
   - Per bar, 16 window maxima (window `⌊Pf/4 + ½⌋`, `Pf = ⌊P + ½⌋`) go through up to 6 picks of the first largest value. Each pick needs `> 0.0001`, `> 0.596×` the bar maximum and `> 0.178×` the running bar maximum.
3. **`LowPowerAnalyze16`** @0x1010c8db0 (100 Hz low-pass, quarter-beat energies).
   - Quirk: beat `i`'s 4th quarter ends at `beats[i+1]` only while `i + 2 < n`.
   - Values are ×4 unless fs ≈ 5512.5. Per-bar sums go to +0x3f0.
4. **`Detect4KicksPopRockRhythm`** @0x101afaaa0. From the first bar `j ≥ 17` (or ≥ 1) whose energy exceeds the mean, bars 2, 3, 6 and 7 of each 8-bar group (at most 8 groups) are matched against five 16-step templates:
   - T0 `1000000010000000`, T1 `1000001010000000`, T2 `0010001000100010`, T3 `1000100010001000`, T4 `1000000010100000`.
   - Outputs, first rule wins:
     - p4 = more than one group with T2/T3;
     - else p7 = T4 groups > threshold;
     - else p5 = T1 groups > threshold and bpm < 165;
     - else p6 = T0 groups > threshold.
   - The threshold is 1 when more than 63 bars follow the start, else 0.
5. **`DetectSnarePositionsFilter`** @0x101306748: 300 Hz high-pass (4 sections), |·|, 5 Hz first-order low-pass, first difference.
6. **`DetectSnarePositions`** @0x101306b98. For the bars of the first half: sort each bar's envelope descending. Pick values that drop more than 1.14 % to the next. Skip ones within a quarter-beat−20 of an earlier sorted entry. Keep picks with a partner 2 beats away (±P/32). Positions end up relative to `fb`.
7. **`…ForGenre`** @0x1008dbc34.
   - Picking is the same as `…ForBar` (below) with the 1.14 % drop.
   - It keeps onsets with a partner 2 beats away (±P/8) or a bar away (−P/4 … +P/8).
   - Without the four-kick flag, onsets on a low-band pattern hit are dropped.
   - Outputs: +0x210 positions, +0x250 count, +0x254 first index.
8. **`…ForBar`** @0x100e916d0.
   - Picks with a 0.115 % drop, de-duplicated against all picks so far.
   - It keeps a pick only with a partner 2 beats away (±P/8). The pick must also either sit near the edges or look like a backbeat: step `s` not hit, with `s−4 & s+2`, `s−6 & s+5` or `s−4 & s+4` hit.
9. **`BeatposCorrection2016`** @0x101967184 (needs at least 16 beats and 2 bars of pattern). This is the downbeat move.
   - It scans the first half's pattern for four shapes:
     - A: kick on steps 0 and 6, nothing on 7–15;
     - B: kick on 0 and 6, nothing on 8–9;
     - C: a fixed 24-step figure;
     - D: kick, rest, (3 or 7), (6 or 10).
     A hit sets +0x208 (`kick6`).
   - **Onset move:** `x = fb mod (16th-note ms)`. Two quarter-energy tracks are built (`LowPowerAnalyze16_b`: 400 Hz from `x`, 100 Hz from `fb`). If the 400 Hz track's first strong jump (> 0.48·max and > 60 % step) has only digital silence before it, or comes within 3 quarters, and its value is > 70, the first beat moves to `x + u·16th` (+0x204 = 2, or 3 if unchanged).
   - **Half-beat move:** otherwise, the first beat moves back half a beat (+0x204 = 1) when any of these hold:
     - the D shape mostly lands on step 3 of 4 (an off-beat grid);
     - the snares of the chosen shape fall mostly between beats (threshold 0.2/0.4/0.45 by shape);
     - the 100 Hz track holds a steady run (16 beats, or three 64-quarter blocks with a 12-beat run) starting on step 3.
   - It also returns `steady` (+0x1b0): steady run found and no move. Code: `beatpos::beatpos_correction`.
10. With +0xc2 clear, the new first beat replaces `fb`. `BeatsSamplesCalc` and `RhythmPatternAnalyze` run again. With +0xc2 set, `fb` stays.
11. **`BarPositionJudge`** @0x1012f35f0 (first 30 s) → +0x258: beats from the first beat to the first sample above 0.0005, rounded, + 1.
12. **Bar.** `BarStartCorrection` @0x1004f089c runs unless `…ForBar` found nothing, +0xc3 is set, or `steady`:
    - the snares (shifted to the new first beat) decide which beat is bar beat 2/4 (`odd`);
    - 100 Hz quarter energies find a loud quarter in bar positions 5–8 (add 2);
    - the bar becomes `1 + add + (odd == 1)`, or keeps `fbar` / 1 by the move type.

Outputs: +0x200 first beat (ms), +0x204 bar, +0x208, +0x210/+0x230/+0x250/+0x254, +0x258.
- `RhythmPatternAnalyze2`/`DetectSlowrock` also run here. Their flag lands in an unused `BarStartCorrection` argument, so they are not ported for B.
- `downbeat()` skips the D-only parts; `stage_b()` computes them.
- `RhythmPatternAnalyze2`/`DetectSlowrock` run again in D on stage C's patterns (§ 7.1).

## 5. Stage C: `Sub_Main_C_init` @0x1007d24e0 / `Sub_Main_C` @0x1007d2a80
The whole track is analysed in **blocks of about 35 s on bar lines**. Each block runs every feature over its samples. Filter states carry across blocks; envelope followers and rhythm patterns are block-aware. Code: `stage_c.rs`.

### 5.1 Init
Three decimated streams per chunk:
- fs/2 (high-pass at 0.275·rate, then ÷2: the top band folded down);
- fs/8;
- fs/32.

`BeatsSamplesCalcFX` @0x101eae3e4 gives `n` beats and 1-based beat arrays at each rate (+0x1118, +0x1138, +0x1158, n+1 entries):
- `P_k = (60/bpm)·fs_k`;
- `off_k = trunc(fma(fb·0.001, fs_k, 0.5))`;
- positions `(int)fma(P_k, i, off_k + 1.5)`;
- `n` is the minimum count over the three rates; index `n−1` is clamped to each stream length (`total/2`, `/8`, `/32` as doubles).

### 5.2 Block timing (asm-checked)
- `fb_s = fbBlock·0.001`.
- `bars = (int)((35.11 − fma(beat_s, bar−1, fb_s)) / (4·beat_s))`.
- `nBlock = bar − 1 + 4·bars`.
- Per chunk, the elapsed time (+0xb50, and a total +0xb58) grows by `chunk/rate`.

When `elapsed ≥ fma(beat_s, nBlock, fb_s)` and enough beats remain:
- the block ends (+0xb38 = 1);
- each stream is cut at `beats_k[nBlock] − 1` and the rest is carried;
- elapsed becomes `elapsed + fma(−(4·bars + bar − 1), beat_s, −fb_s)`.

At the end of the track the last block takes `(uint)((elapsed − fb_s if first block)/beat_s)` beats (+0xb38 = 2).

After a full block:
- `fb = 0`, `bar = 1`, block counter +1;
- every beat array has `beats[nb]` subtracted and is shifted left by `nb` (+1).
- The analysed count `nb = min(nBlock, n − done)`.

### 5.3 Features
Each result is written so it ends at the analysed-beat count. Inputs: x2/x8/x32 are the block streams, b2/b8/b32 the beat arrays.

| Function | Input | What it measures | Output → C-out |
|---|---|---|---|
| `LowPeakAnalyze` @0x1010f58c4 | x32, b32 | 100 Hz LP, \|·\|, 5 Hz: peak per beat and per quarter; jump flags (×1.77 up → 1, ÷2 down → 0.5, ×0.8 when both ≤ 0.09) against the previous beat (across blocks) | +0xeb8, +0xfb8, +0xf18 |
| `LowLevelAnalyze_a` @0x10043cbe0 | x32, b32 | 150 Hz LP envelope sum per beat ×4 | +0xf78 |
| `LowPowerAnalyze16` | x32, b32 | as in B (filter state continues) | +0xe50 |
| `LowPowerAnalyze16_40Hz` @0x1003a5038 | x32, b32 | 4×40 Hz LP quarter energies (beat end at `beats[i+1]` while `i+1 < n`) | +0x1070 |
| `MiddleLevelAnalyze_a` @0x102073cb8 | x8, b8 | 500 Hz HP envelope sum per beat | +0xfd8 |
| `MiddlePeakAnalyze` @0x1019977e0 | x8, b8 | 400 Hz–1 kHz band, 10 Hz smoothing, peak per beat | +0xef8 |
| `AllLevelAnalyze_a` @0x1000eef38 | x8, b8 | \|x\| peak per beat | +0x10b0 |
| `AllPowerAnalyze16` @0x100fc3ff8 | x8, b8 | raw quarter energies and \|x\| peaks | +0xe70, +0xe98 |
| `RhythmPatternAnalyze_HP` @0x1004bcae8 | x8, b8 | B's pattern, block-wise, windows at the real beat positions | +0xf58 (16/bar) |
| `RhythmPatternAnalyze2_HP` @0x10147e4c8 | x8, b8 | same pre-roll on the low band (rising edges only), then 12 picks at ratio 0.5 on a 110–200 Hz band envelope | +0xf98 |
| `HiHatAnalyze_a` @0x100ba9828 | x2, b2 | fast/slow followers from 0 each block; onsets (fast > slow and > 0.06, ≥ fs·13.33/bpm apart) per beat | +0xf38 |
| `IntroEndFind_a` @0x100dafb90 | x8 | constant-Q frames (hop 256): 72 semitone bins from 27.5 Hz, Hann windows of `⌊2fs/(f_{k+1}−f_k)⌋`, bins 23–71 computed; unused tail carried to the next block | +0x1090 (72 × frames), +0x10a8, +0x10ac |
| `FFT_calc_2` @0x1000ee7a8 | x8, b8 | 4096-point Hann FFT at beats 1 and 3 of each bar, 20·log10\|X\| (−200 below 1e-10), 2048 bins | +0x1050 |
| `FFT_calc_1` @0x101cab8b4 | x8, b8 | 2048-point at each beat, 1024 bins | +0xee0 |
| `DividedBandAnalyze` @0x100258794 | x32, x8 | per beat: envelope sum below 500 Hz (fs/32) and above 500 Hz (fs/8) | +0xff8, +0x1018 |
| `HighPeakAnalyze32` @0x101f9c554 | x2, b2 | 5–7 kHz band, 10 Hz smoothing, peak per eighth of a beat | +0x10d0 |

`Set_Sub_Main_C_Out` @0x1007d394c then copies everything to stage D's inputs, plus:
- n (+0xe90, +0x1068) and the current block bar (+0xed8);
- +0x10f0, set once a non-zero fs/32 sample was seen (`ZeroDataCheck` @0x101fd5320);
- the rebased fs/8 beat array (+0x10f8).

### 5.4 Exactness notes for C (asm-checked)
- **libm calls.** Vector paths and scalar tails call different libm entries:
  - the constant-Q kernel uses `__sincos_stret` for pairs and `cos`/`sin` for an odd tail;
  - octave lines are complex squares: vector `fma(−b, b, a²)`, scalar `fma(a, a, −b²)`;
  - `c_fft`'s table is `sin(k·2π/N)`;
  - its Hann window is `fma(cos((k − m)·π/(m+1)), 0.5, 0.5)` with `m = (N−1)/2`.
- **FFT.** Radix-2 DIT. Twiddle `tr = fma(re, c, im·s)`, `ti = fma(im, c, −re·s)`. Magnitude `sqrt(fma(im, im, re²))`.
- **Dot products.** The constant-Q dot product uses fused 8-lane accumulation with `sum`'s reduction order. Magnitude `sqrt(fma(im, im, re²))`.
- **HiHat follower.** `v += fma(c·(x − v), 1/fs, v)`.

## 6. The shift and its result
`phrase::apply` follows the thread:
- **Same bar** (or bar outside 1–4) and same ms: no change.
- **Different ms:** every beat moves by `new − fb` ms.
- **Different bar:** `beatInBar` is renumbered from `convFirstBar[bar − 1]`.
- **A/B failed:** the target becomes −1 ms, unless `fb` was −1.

`beat_score` against oracle-a (161 rekordbox-analysed tracks), with phrase on:
- BPM × 100: 161/161;
- beat count: 161/161;
- bars: 161/161;
- PQTZ + PQT2 identical: 160/161;
- beats within ±1 ms: 94 769/94 897 (99.87 %), all exact.

All 21 tracks the pass moved now match: `click_128_offset1234` ×7 (+936 ms onto the first kick), `sweep_20_20k` ×9, the 22.05 kHz tempo ramp, `bands_100_1k_8k`, Oliver Schories – Circles, and Aura Vortex – Futura (renumbered).

The one miss is `wav_96k32f/click_128_offset1234`: we move it, rekordbox did not, although it ran the pass (its PSSI exists). At 96 → 48 kHz the 16th-note window just before the click ends exactly at the click (sample 59232 at 48 kHz). Our SRC output is zero up to there, so the silence test passes. Most likely rekordbox's re-read stream is not zero there: priming of the resampler after the rewind, or a one-sample SRC offset.

## 7. Stage D: `Sub_Main_D` @0x102379edc

Code: `stage_d.rs` (`sub_main_d`). Every step below is checked against rekordbox's own routines (§ 9).

### 7.1 Inputs and flow
Registers and stack slots:
- bpm `d8`, rate `d10`, fs8 `d9`, key, total;
- n (+0xe90), kick6 (+0x208), original fb (+0xa4), +0x250/+0x254, bar (+0x204), new fb (+0x200);
- constant-Q frame count/offset (+0x10a8/+0x10ac), non-zero flag (+0x10f0), `BarPositionJudge` (+0x258).

When +0x10f0 is 0, D writes a "failed" result: count 0, each array one entry, beat array `[−1]`.

Otherwise, in order (inputs → what the code keeps):

| Call | Inputs | Kept |
|---|---|---|
| `IntroEndFind_b` @0x100fbc2c0 | constant-Q (normalised in place), frames, first, n | strong-bar flags, jump flags, per-beat level (`d_feat::intro_end_find_b`) |
| `LowLevelAnalyze_b` @0x101c893d0 | low level, bar | bar sums, jump flags |
| `MiddleLevelAnalyze_b` @0x101c63fc8 | mid level, bar | jump flags, bar sums |
| `MiddlePeakAnalyze_b` @0x100b45378 | mid peak (scaled in place), n, bar | bar means |
| `AllLevelAnalyze_b` @0x10126f618 | all level, bar | bar means, normalised level |
| `HiHatAnalyze_b` @0x1022584b0 | hi-hat counts, bar | per-bar change flags (counts quantised 0/4/8/16, 1/2/3-bar islands smoothed) |
| `HighPeakAnalyze32_b` @0x1001be9f0 | eighth peaks, bar | shifted/normalised peaks, bar count |
| `UtaUmuAnalyze_a` @0x101116194 | FFT_calc_1 lines | per line, the largest spectral peak above bin 112 that is > 38.8·mean level, width/position > 0.11, prominence > 10 dB (`findpeaks_ex` @0x101bc81c4: MATLAB-like peaks, left-side prominence, half-prominence width) |
| `DetectComp` @0x100508cec | low power, all power, pattern, bar | per 8-bar window, the off-hit share of full-band energy and the low-band level at hits |
| `DetectFadeOut` @0x101847454 | low power, all power | fade-out flag (last 512 quarters) |
| `CalcMaxLevelRatio` @0x10234b8cc | all peaks | share of 16-quarter windows reaching −1 dB |
| `DetectBPM128` @0x102354448, `DetectTenkeiGenre1` @0x100acea7c | bpm, kick6 | flags |
| `DetectSlowrock` @0x10111d2f8 | C patterns (+0xf58, +0xf98), bpm | slow-rock flag: the figure shared hit · 5 empty 16ths · an off-beat hit in the second pattern only · 5 empty · shared hit, found at least twice one bar (16 steps ± 1) apart (`genre::detect_slowrock`) |
| `CalcTrackLowPowerRatio` @0x100f28d60 ×2 | low power or 40 Hz power, all power | `√(Σlow/Σall)`, per-bar ratio and sum (16-term sums in a fixed tree), mean or best 16-bar mean |
| `DetectSlowTempo` @0x101475134 | first call's bar sums, B's genre snares (shifted to the new fb), fs/8 beats | (slow, half-time) |
| `Detect4KicksPopRockRhythm` | C pattern, first call's bar sums | p4–p7 |
| `GenreDevideOverall` @0x10194e7b0 | the flags above | genre: 3 if low ratio < 0.3979 or slow-rock, or comp < 0.6128 and slow; 2 on fade-out; else `fma((p4−p5)+comp, 0.6, fma(tenkei+b128b, 0.1, fma(b128a−p7, 0.3, −0.05·p6))) > 1 ? 1 : 2` |
| `Genre12FinalJudge` @0x100364368 | genre, n, beat peaks, quarter peaks | 2 → 1 for steady, changing low-band peaks |
| `FindSecurePoint` @0x101982480 | beat peaks, bar | bars where the level jumps and holds (`[0]` if none) |

Then:
- secure points become beat indices `bar − 4 + 4·s`;
- a copy keeps only values 1–199, without zeros.

Mood and type. D keeps two values: the **mood** (PSSI word 0x12, `GenreDevideOverall`'s genre 1/2/3 at first) and the **type**, which picks the candidate chain:
- mood 1 → type 1; mood 3 → type 2;
- mood 2: two trial block lists are built from the secure points `s` (block starts 1, then per point a 40 block at `bar − 4 + 4s`, preceded by a 30 block 9 bars earlier when the gap to the previous point is at least 9 bars, or 10 for the first point; the second list uses 17 bars, 18 for the first, and 17 bars earlier). Zero entries are dropped. Then:
  - `BlockAverage` @0x1004acc90 on the first list;
  - `UpSimpleloopAna` @0x100eab528 on it (30/50 → 31 on a steep high-band rise);
  - `Up4_8_16Ana` @0x1008011d0 on the second list (32/33 on quarter→eighth→roll build-ups);
  - `SongStructureAna_Type_Genre_Judge` @0x1005ed3b4, which sets the type and may change the mood.
  With no secure point, the judge gets the two zero lists.

Next comes `IntroEndJudge_main` @0x100e5536c (beat peaks, low-level bar sums, mid-peak bar means, bar, vocal lines, the filtered secure beats → an intro list, the intro end and a found flag), then `VerseHookAnaData23` @0x100b86a38 (four curves, each normalised, with per-bar raw and spread change).

**Phrase starts (+0x820).** The list starts as one zero entry.
- **Type 1, intro found:**
  1. `VerseHookJudge_LowPeak` gives candidates. The first one followed by a gap of at least 113 beats (the last one against the length of the low-peak flags) is noted.
  2. `BreakFind`, `BigChangeFind`; if the gap, the break and the change are all below 2, the phrase list is just the intro end, else `IrregularJudge` builds it.
  3. When the secure beats start at 1 or more, they are merged: a phrase equal to a secure beat removes that secure beat, one 4 beats from it is zeroed; the rest are appended, sorted, zeros dropped.
  4. `VerseHookJudge_All` (on copies of the curves: D's arrays keep their zeros), `CandidateAdjust`, and `RhythmPatternReject` when the candidates outnumber 5.2 % of the duration in seconds.
  5. The `BarPositionJudge` beat is put first; `VerseHookJudge23` gives estimates; `VerseHookAdd_genre1` merges both.
  6. Starts up to the bar position + 3 are dropped; the bar position goes first again.
  7. Every secure beat not yet in the list is inserted in order (values below the first entry are not inserted); then starts within 8 beats of a secure beat (not equal to it) are removed, never the first; a duplicate first entry is removed.
- **Type 1, no intro:** the bar position alone.
- **Other types, intro found:** `VerseHookJudge23`, `VerseHookAdd_genre23`; starts past the mid-level or level length are removed from the end; then as step 6.
- **Other types, no intro:** the bar position alone. A duplicate first entry is removed.
For type 1 the phrase count is fixed here.

**Labels.**
- **Mood 1:** `BlockAverage`, `SongStructureJudge` (bank = its flag ≠ 1), `IntroFeatureAna`, `UpSimpleloopAna`, `Up4_8_16Ana`, `UpSimpleloopAna_b`, `UpFeatureAna`, `HookFeatureAna`, `OutroFeatureAna` (labels and the end beat), `FillAdjustAna`, `FeatureLastAdjust` (mood 1). Labels encode the fill as hundreds (110, 330–333, 440/441), the section as tens (10 intro, 20, 30 up, 40 hook, 50 down, 60 outro) and a variant as units.
- **Moods 2/3:** the count is the list length.
  - One block: label 10.
  - Otherwise `ABmeloAnalyze` (which blocks sound alike: § 7.1.1), `HooksoloAnalyze` (first labels 10/40/50 from block levels), `TenkaimeiAnalyze` (similar blocks share labels 61, 62, …; the last block becomes 60 unless it is a hook), `BlockAverage`, `SongStructureJudge` (only its level/peak side effects are used) and `YokuyoAnalyze` (bank = whether block levels vary).
  - Then `OutroFeatureAna` for the end beat (its labels are discarded) and `FeatureLastAdjust`.

**Result** (§ 8.1): count, first downbeat, mood, bar, bank, end beat = the last entry of the end array; per phrase the start beat and the fill start. `DataFortmatConvert` @0x101840ab8 then fills the kinds and flags:
- mood 1: per label, hundreds → fill flag (word 10, cleared when `FillAdjustAna` kept a loud fill), tens → kind (1 intro→1, 2→4, 3 up→2, 4 hook→5, 5 down→3, 6 outro→6), units → variant words: 1 → word 3 (intro, hook, outro) or word 9 (up); up 2 → words 4, 5 = 1 and words 6–8 from `Up4_8_16Ana`; up 3 → word 4 = 1, word 5 = 0, word 6 from it;
- moods 2/3: 10 → 1, 40 → 9, 50 → 8, 60 → 10; each new label from 61 on takes the next of 2, 3, 4, 5, 6, 7, 2, …; repeats take their first occurrence's kind;
- byte 0x1d = `key < 12` with `Initialize`'s key.

#### 7.1.1 `ABmeloAnalyze`
- Rows of `FFT_calc_2` (+0x1050, one per two beats) are compared pairwise: `1/Σ((mean_j + a_i) − (mean_i + a_j))²`, 65535 for a zero distance. The sum runs as clang vectorised it (four 2-lane fused accumulators, reduced `((a1+a0)+a2)+a3`, lanes added, then a fused scalar tail).
- Block boundaries become rows `max(1, (int)fma(beat − bar, 0.5, 1))`; a block pair's similarity is the mean along the diagonal over the shorter block.
- Pairs at or above mean + standard deviation of the positive pairs are similar (same vectorised order for both sums; the deviation is the sum itself with fewer than 2 values).

### 7.2 Call-site map (first callees)
Stack offsets are from the D frame (`CFA = sp + 0xad0`):
- `IntroEndFind_b` → sp+0x428 / sp+0x408 / x29−0x98;
- `LowLevel_b` → x29−0xb8, sp+0x3e8;
- `MiddleLevel_b` → sp+0x3c8, x29−0xd8;
- `MiddlePeak_b` → x29−0xf8;
- `AllLevel_b` → sp+0x9a8, sp+0x988;
- `HiHat_b` → sp+0x3a8;
- `HighPeak_b` → sp+0x968, sp+0x38c;
- `UtaUmu` → sp+0x948;
- `DetectComp` → sp+0x2f0, sp+0x2e8;
- `FadeOut` → sp+0x388;
- `MaxLevel` → sp+0x318;
- `BPM128` → sp+0x384, sp+0x380;
- `Tenkei` → sp+0x37c;
- `Slowrock` → sp+0x324;
- `CalcTrackLowPowerRatio` #1 → sp+0x310 (ratio), sp+0x928, sp+0x908 (bar sums), sp+0x308, [sp];
- #2 → sp+0xe8, sp+0x150, sp+0x130, sp+0xe0;
- `SlowTempo` → sp+0x378 (slow), sp+0xdc (half);
- `Detect4Kicks` → sp+0x334/330/32c/328 (p4/p5/p6/p7);
- genre → sp+0xcc, mood → sp+0xc8;
- secure points → sp+0x6e8.

`GenreDevideOverall` reads only `d0` (ratio) and `d5` (comp level) of its float arguments. Its stack ints are 0, 0, BPM128a, BPM128b, Tenkei.

### 7.3 Function map
| Function | Address | Bytes | Status |
|---|---|---|---|
| `ABmeloAnalyze` | 0x100c019d8 | 2612 | ported (D) |
| `AllLevelAnalyze_a` | 0x1000eef38 | 476 | ported (C) |
| `AllLevelAnalyze_b` | 0x10126f618 | 364 | ported (D) |
| `AllPowerAnalyze16` | 0x100fc3ff8 | 640 | ported (C) |
| `Analyze_AB` | 0x1004438d8 | 856 | ported (A/B) |
| `Analyze_CD` | 0x100443c30 | 852 | ported (C) |
| `BarPositionJudge` | 0x1012f35f0 | 372 | ported (A/B) |
| `BarStartCorrection` | 0x1004f089c | 1516 | ported (A/B) |
| `Beat4Ana` | 0x101c65a50 | 1072 | ported (D) |
| `BeatposCorrection2016` | 0x101967184 | 3056 | ported (A/B) |
| `BeatsSamplesCalc` | 0x1008d4778 | 232 | ported (A/B) |
| `BeatsSamplesCalcFX` | 0x101eae3e4 | 540 | ported (C) |
| `BigChangeFind` | 0x1015adac0 | 1744 | ported (D) |
| `BlockAverage` | 0x1004acc90 | 932 | ported (D) |
| `BreakFind` | 0x101391d04 | 1356 | ported (D) |
| `c_fft::c_fft` | 0x100b8752c | 28 | not needed: constructor |
| `c_fft::fft` | 0x100b87898 | 300 | ported (C) |
| `c_fft::init` | 0x100b875a8 | 752 | ported (C) |
| `c_fft::make_hanning_window` | 0x100b879c4 | 348 | ported (C) |
| `c_fft::window` | 0x100b87b20 | 200 | ported (C) |
| `c_filter::clear` | 0x100351a00 | 8 | ported (A/B) |
| `c_filter::filter1` | 0x100351b28 | 64 | ported (A/B) |
| `c_filter::filter2` | 0x100351b68 | 76 | ported (A/B) |
| `c_filter::make1` | 0x100351a64 | 68 | ported (A/B) |
| `c_filter::make2` | 0x100351aa8 | 128 | ported (A/B) |
| `c_filter::prewarp` | 0x100351a3c | 40 | ported (A/B) |
| `CalcMaxLevelRatio` | 0x10234b8cc | 220 | ported (D) |
| `CalcTrackLowPowerRatio` | 0x100f28d60 | 1268 | ported (D) |
| `CandidateAdjust` | 0x1010e9068 | 1804 | ported (D) |
| `CandidatesAdjustment` | 0x10019d064 | 1716 | ported (D) |
| `DataFortmatConvert` | 0x101840ab8 | 1260 | ported (D) |
| `Detect4KicksPopRockRhythm` | 0x101afaaa0 | 1484 | ported (A/B) |
| `DetectBPM128` | 0x102354448 | 252 | ported (D) |
| `DetectComp` | 0x100508cec | 2520 | ported (D) |
| `DetectFadeOut` | 0x101847454 | 2224 | ported (D) |
| `DetectSlowrock` | 0x10111d2f8 | 824 | ported (D) |
| `DetectSlowTempo` | 0x101475134 | 1164 | ported (D) |
| `DetectSnarePositions` | 0x101306b98 | 1312 | ported (A/B) |
| `DetectSnarePositionsFilter` | 0x101306748 | 1008 | ported (A/B) |
| `DetectSnarePositionsForBar` | 0x100e916d0 | 1368 | ported (A/B) |
| `DetectSnarePositionsForGenre` | 0x1008dbc34 | 2024 | ported (A/B) |
| `DetectTenkeiGenre1` | 0x100acea7c | 80 | ported (D) |
| `DividedBandAnalyze` | 0x100258794 | 552 | ported (C) |
| `DividedBandAnalyze_init` | 0x100258620 | 372 | ported (C) |
| `FeatureLastAdjust` | 0x101075aec | 1856 | ported (D) |
| `FFT_calc_1` | 0x101cab8b4 | 2836 | ported (C) |
| `FFT_calc_1_init` | 0x101cab874 | 64 | ported (C) |
| `FFT_calc_2` | 0x1000ee7a8 | 1884 | ported (C) |
| `FFT_calc_2_init` | 0x1000ee768 | 64 | ported (C) |
| `FillAdjustAna` | 0x1000c137c | 1488 | ported (D) |
| `FillinCutAna` | 0x100461680 | 1996 | ported (D) |
| `FillinSnareAna` | 0x1007e20c4 | 608 | ported (D) |
| `FillinSnareAnaEX` | 0x100308cb8 | 944 | ported (D) |
| `filter_8th_ds::downsample` | 0x100ac3ea8 | 404 | ported (A/B) |
| `filter_8th_ds::filter_8th_ds` | 0x100ac3dfc | 84 | not needed: constructor |
| `findpeaks_ex` | 0x101bc81c4 | 1076 | ported (D) |
| `FindSecurePoint` | 0x101982480 | 896 | ported (D) |
| `Genre12FinalJudge` | 0x100364368 | 1104 | ported (D) |
| `GenreDevideOverall` | 0x10194e7b0 | 264 | ported (D) |
| `hanning_ex` | 0x1004ae7b8 | 416 | ported (C) |
| `HighPeakAnalyze32` | 0x101f9c554 | 1060 | ported (C) |
| `HighPeakAnalyze32_b` | 0x1001be9f0 | 356 | ported (D) |
| `HighPeakAnalyze32_init` | 0x101f9c384 | 464 | ported (C) |
| `HiHatAnalyze_a` | 0x100ba9828 | 812 | ported (C) |
| `HiHatAnalyze_a_init` | 0x100ba9818 | 16 | ported (C) |
| `HiHatAnalyze_b` | 0x1022584b0 | 1680 | ported (D) |
| `HookFeatureAna` | 0x1020b35f4 | 600 | ported (D) |
| `HooksoloAnalyze` | 0x1019169cc | 3188 | ported (D) |
| `Initialize` | 0x1004436b8 | 496 | ported (A/B) |
| `InputData_init` | 0x1004438a8 | 48 | ported (C) |
| `IntroEndFind_a` | 0x100dafb90 | 1108 | ported (C) |
| `IntroEndFind_a_init` | 0x100daf408 | 1840 | ported (C) |
| `IntroEndFind_b` | 0x100fbc2c0 | 2184 | ported (D) |
| `IntroEndJudge` | 0x10234508c | 25812 | ported (D) |
| `IntroEndJudge_main` | 0x100e5536c | 1964 | ported (D) |
| `IntroFeatureAna` | 0x101dcb7dc | 448 | ported (D) |
| `IrregularJudge` | 0x101e46988 | 1344 | ported (D) |
| `is_equal` | 0x100443f84 | 92 | ported (A/B) |
| `LowLevelAnalyze_a` | 0x10043cbe0 | 632 | ported (C) |
| `LowLevelAnalyze_a_init` | 0x10043cb08 | 216 | ported (C) |
| `LowLevelAnalyze_b` | 0x101c893d0 | 492 | ported (D) |
| `LowPeakAnalyze` | 0x1010f58c4 | 1488 | ported (C) |
| `LowPeakAnalyze_init` | 0x1010f57e4 | 224 | ported (C) |
| `LowPowerAnalyze16` | 0x1010c8db0 | 984 | ported (A/B) |
| `LowPowerAnalyze16_40Hz` | 0x1003a5038 | 1052 | ported (C) |
| `LowPowerAnalyze16_40Hz_init` | 0x1003a4f48 | 240 | ported (C) |
| `LowPowerAnalyze16_b` | 0x101bbce64 | 636 | ported (A/B) |
| `LowPowerAnalyze16_b_init` | 0x101bbcdd4 | 144 | ported (A/B) |
| `LowPowerAnalyze16_init` | 0x1010c8d24 | 140 | ported (A/B) |
| `LowPowerAnalyze16forBarCorr` | 0x101365734 | 644 | ported (A/B) |
| `LowPowerAnalyze16forBarCorr_init` | 0x1013656a8 | 140 | ported (A/B) |
| `MiddleLevelAnalyze_a` | 0x102073cb8 | 528 | ported (C) |
| `MiddleLevelAnalyze_a_init` | 0x102073bd8 | 224 | ported (C) |
| `MiddleLevelAnalyze_b` | 0x101c63fc8 | 692 | ported (D) |
| `MiddlePeakAnalyze` | 0x1019977e0 | 640 | ported (C) |
| `MiddlePeakAnalyze_b` | 0x100b45378 | 384 | ported (D) |
| `MiddlePeakAnalyze_init` | 0x101997608 | 472 | ported (C) |
| `OutroFeatureAna` | 0x10079c4a4 | 2340 | ported (D) |
| `OutroKickoutAna` | 0x100fc3dc4 | 520 | ported (D) |
| `RatioCalc_Verse` | 0x101b70794 | 868 | ported (D) |
| `resample_hpf::init` | 0x102148ed4 | 396 | ported (C) |
| `resample_hpf::resample` | 0x102149060 | 24 | ported (C) |
| `RhythmPatternAnalyze` | 0x10222f334 | 4108 | ported (A/B) |
| `RhythmPatternAnalyze_HP` | 0x1004bcae8 | 4404 | ported (C) |
| `RhythmPatternAnalyze_HP_init` | 0x1004bc768 | 896 | ported (C) |
| `RhythmPatternAnalyze_init` | 0x10222f000 | 820 | ported (A/B) |
| `RhythmPatternAnalyze2` | 0x10198a45c | 4064 | not needed: as `RhythmPatternAnalyze2_init` |
| `RhythmPatternAnalyze2_HP` | 0x10147e4c8 | 4444 | ported (C) |
| `RhythmPatternAnalyze2_HP_init` | 0x10147e044 | 1156 | ported (C) |
| `RhythmPatternAnalyze2_init` | 0x10198a040 | 1052 | not needed: B only feeds DetectSlowrock, whose flag lands in an unused BarStartCorrection argument |
| `RhythmPatternReject` | 0x100b85f88 | 2588 | ported (D) |
| `Set_Sub_Main_C_Out` | 0x1007d394c | 920 | ported (C) |
| `ShortBreakDetection` | 0x101beec5c | 1812 | ported (D) |
| `SnareFillinDet` | 0x1002ef650 | 1892 | ported (D) |
| `SongStructureAna_Type_Genre_Judge` | 0x1005ed3b4 | 4372 | ported (D) |
| `SongStructureAnalyze_resample2::init` | 0x1002ca5b4 | 396 | ported (A/B) |
| `SongStructureAnalyze_resample2::resample` | 0x1002ca740 | 24 | ported (A/B) |
| `SongStructureAnalyze_resample2::SongStructureAnalyze_resample2` | 0x1002ca598 | 24 | not needed: constructor |
| `SongStructureJudge` | 0x10025dcf0 | 5364 | ported (D) |
| `SouonAnalyze_a2_init` | 0x100f2fb30 | 64 | not needed: only builds a 256-point FFT; no stage C caller uses it |
| `SouonChangeDet` | 0x1005dffc8 | 1272 | ported (D) |
| `Sub_Main_A` | 0x101c28728 | 176 | ported (A/B) |
| `Sub_Main_A_init` | 0x101c28678 | 176 | ported (A/B) |
| `Sub_Main_B` | 0x100b80b04 | 1764 | ported (A/B) |
| `Sub_Main_C` | 0x1007d2a80 | 3788 | ported (C) |
| `Sub_Main_C_init` | 0x1007d24e0 | 1440 | ported (C) |
| `Sub_Main_D` | 0x102379edc | 8664 | ported (D) |
| `Sub_Main_D()::$_0::operator()` | 0x10237c5f4 | 120 | not needed: result-array allocation check |
| `TenkaimeiAnalyze` | 0x100540924 | 736 | ported (D) |
| `Up4_8_16Ana` | 0x1008011d0 | 3816 | ported (D) |
| `UpFeatureAna` | 0x10109e414 | 556 | ported (D) |
| `UpSimpleloopAna` | 0x100eab528 | 1376 | ported (D) |
| `UpSimpleloopAna_b` | 0x1020b8bd4 | 468 | ported (D) |
| `UtaUmuAnalyze_a` | 0x101116194 | 2152 | ported (D) |
| `VerseFeatureAna` | 0x101063bf4 | 16 | not needed: sets its two outputs to 0; nothing reads them |
| `VerseHookAdd_genre1` | 0x100b454f8 | 2828 | ported (D) |
| `VerseHookAdd_genre23` | 0x1013f1fbc | 648 | ported (D) |
| `VerseHookAnaData23` | 0x100b86a38 | 2008 | ported (D) |
| `VerseHookJudge_All` | 0x101bc8620 | 2564 | ported (D) |
| `VerseHookJudge_LowPeak` | 0x1020ded04 | 952 | ported (D) |
| `VerseHookJudge23` | 0x1014412f4 | 3328 | ported (D) |
| `VerseHookJudge23Sub` | 0x1005a95d8 | 1940 | ported (D) |
| `VocalInDetection` | 0x1004ebda0 | 1708 | ported (D) |
| `YokuyoAnalyze` | 0x100f097fc | 956 | ported (D) |
| `ZeroDataCheck` | 0x101fd5320 | 80 | ported (C) |

### 7.4 What remains
Nothing: every `SongStructureAnalyze` function is ported or not needed (table above).

## 8. From the result to PSSI
When thread flag +0x38e1 is set, the result goes to the `.EXT` file through this chain:
1. `notifySongStruct` @0x100159fac;
2. `CAnalyzerIF::notify_song_struct` @0x10162b8b0;
3. `set_pmai_ext_song_struct_data` @0x100f26a7c;
4. `write_pmai_ext_song_struct` @0x1013d10b4.

The PSSI layout is in `docs/re/anlz.md` § PSSI (`mb_anlz::tags::SongStructure`).

### 8.1 Result struct (0x78 bytes)
| Offset | Field | PSSI |
|---|---|---|
| +0x00 | phrase count | 0x10 |
| +0x04 | first downbeat (ms, stage B's +0x200) | not written |
| +0x08 | mood (§ 7.1) | 0x12 |
| +0x0c | bar | not written |
| +0x10 | bank | byte 0x1c |
| +0x14 | `Initialize`'s key < 12 | byte 0x1d |
| +0x18 | end beat | 0x1a |
| +0x20 | int array: phrase start beat | word 1 |
| +0x28 | int array: kind | word 2 |
| +0x30, +0x38 | int arrays (0/1) | words 3, 4 |
| +0x40 … +0x58 | int arrays | words 5–8 |
| +0x60 | int array (0/1) | word 9 |
| +0x68 | int array: fill flag | word 10 |
| +0x70 | int array: fill start beat | word 11 |

Word 0 is the 1-based index.
- A failed analysis (+0x10f0 = 0) writes count 0 and a beat array `[−1]`.

What reaches the file:
- `notifySongStruct` clears the section instead when any array is missing, the first start is negative or the count is 0.
- `set_pmai_ext_song_struct_data` refuses a count, end beat or bar of 0x10000 or more; array values are cut to 16 bits. Byte 0x1e stays 0 (from `init_pmai_ext_song_struct`), 0x0c/0x14/0x18/0x1f are written as 0.
- Code: `PhraseResult::stored` and `PhraseResult::rows`.
- The thread passes key 0 to `Initialize` as far as observed: byte 0x1d is 1 on every oracle track, including minor keys. musicbox passes 0.

**Library (`mb_library::analyse_track`).** With the phrase setting on, the `.EXT` gets our PSSI last (`… PWV4 [PVB2] PSSI`) and `ContentLink` gets 0x10000 (`setAnalyzedSongStructToContentLink`). An earlier PSSI is not carried over then; without a result the section is left out and the bit cleared (hypothesis: rekordbox's behaviour for a phrase pass without result is not observed). With the setting off, an earlier PSSI and its bit are kept as before.

## 9. Verification
A harness outside the repo (`~/re-work/harness`) loads rekordbox's analyser and runs its `Initialize`/`Analyze_AB`/`Analyze_CD` on the stream our decoder produces. It records every call of the `SongStructureAnalyze` functions (arguments before and after). It reproduces oracle-a's PSSI on 159/161 tracks (the two misses are the 96 kHz probes, an input-side difference). Nothing from it enters the repo; musicbox does not link or call it.
- **Stage C:** our outputs equal rekordbox's on 161/161 tracks (after making `IntroEndFind_a`'s frame index round up, `fcvtps`).
- **Replay tests** (`phrase/replay.rs`, `MB_PHRASE_TRACE=<dir of traces>`): each ported D function is run on every recorded call and compared bit for bit. About 400 000 calls in 60+ functions, all identical.
- **End to end** (`replay::stage_d_end_to_end`, `MB_PHRASE_D=<dir>`): stages A–D from the audio. Every probed callee input matches the trace in order, and the result struct equals rekordbox's on 161/161 tracks.
- **Fixtures:** `examples/phrase_score.rs`, `docs/scorecards/phrase.json`.

Porting notes that mattered:
- Ghidra drops `this` from some signatures (argument k is the real k − 2) and loses stores in fused sequences. The asm decided every case.
- Vectorised loops have their own summation order (`ABmeloAnalyze`, `FillAdjustAna`): copy it from the asm.
- `VerseHookJudge_All` works on copies of D's curves. Writing its 0.003 replacements back changes later inputs (caught by the end-to-end probe, not by the result).

## 10. Open questions
- **96 kHz misses.** `wav_96k32f/click_128_offset1234`: we move the grid, rekordbox does not (§ 6), so the phrases differ. `wav_96k32f/tempo_120_to_126`: one phrase more or fewer than rekordbox on both fixtures. Both look input-side: the stream after the rewind, or the SRC phase.
- **Key argument.** What the thread passes to `Initialize` as key: 0 fits all fixtures (byte 0x1d = 1).
- **Phrase pass without result.** Whether rekordbox removes an earlier PSSI when a new phrase pass yields none is not observed (§ 8.1).
