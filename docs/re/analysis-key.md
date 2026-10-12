# Key detection (BALib `KeyCodeScale` + `KeyJudge2`)

Status: **verified**. `mb_analysis::analyze(…, Options { key: true, .. })` (or `key::detect` on a key buffer) gives rekordbox 7.2.19's key on 169/169 corpus tracks, both from rekordbox's own grid and from our own analysis pass (`fixtures/oracle-a`). Code: `crates/mb-analysis/src/key/`. Scorecard: `docs/scorecards/key.json`.

rekordbox stores the key only in the database (`djmdContent.KeyID` → `djmdKey.ScaleName`). The analysis is BALib's (`BeatAnalyzer_1_0`; `BeatAnalyzer_2_0` has byte-identical copies of every key function, same sizes, used in modes 2–4).

Key codes: 0–11 major, 12–23 minor, root `code % 12` with 0 = A (A, B♭, B, C, D♭, D, E♭, E, F, F♯, G, A♭; `key::scale_name` gives the stored `ScaleName`).

## 1. Where it runs
1. **Collection, during the decode pass** (`analyzer::CBeatAnalyzeThread::analyze` @0x1001528c4, around decompile line ~1750). Only when key analysis is on (thread +0x3901):
   - Before the loop: decimation `d = 8` (16 for 88.2/96 kHz, 32 for 176.4/192 kHz); key buffer length `K = min((L − 1)/d, 3 600 000)` floats.
   - Mix factors: when the track has 2 channels, `a = 0.99, b = 0.00999999` if `L − 1 ≥ 30·rate`, else `a = 0.999, b = 0.0009999871`. (Mono: the left channel as is.)
   - Per analysis block (the same `2·rate`-frame blocks of 16-bit samples as `sample/32768`, after the loudness meter has been fed): `left[i] = left[i]·a + b·right[i]` in place, then `BALibWrapper::Resample` → `resample2` @0x101146498 writes `frames/d` samples (the last block: what still fits) at the running key-buffer position.
2. **`resample2(dst, m, src, n, ratio, d, rate, scratch)`**: an 8th-order IIR low-pass (state reset to zero on **every** call, i.e. every block), then `dst[j] = scratch[7 + j·d]`.
   - Recursion (doubles, fused as in the binary): `acc = w1·a1; acc = fma(w2,a2,acc); … fma(w8,a8,acc); w0 = (float)(acc + (double)src[i])` with `a1..a8` = 6.390364646911621, −18.000337600708008, 29.171098709106445, −29.731374740600586, 19.505632400512695, −8.040995597839355, 1.9036688804626465, −0.1981000155210495 (f32 constants widened).
   - Output: `t = w4·b4; t = fma(w5+w3, b3, t); t = fma(w6+w2, b2, t); t = fma(w8, b8, t); t = fma(w7+w1, b1, t); out = (float)(t + (double)(w0·b0f))` with `b0f = 1.7625537e-07` (f32 product), `b1 = 1.410042955285462e-06, b2 = 4.935150627716212e-06, b3 = 9.870301255432423e-06, b4 = 1.2337875887169503e-05, b8 = 1.7625536941068276e-07`.
3. **The key job** (`analyzer::AnalyzeJobKey::Detect` @0x101b09f38 → `getKey` @0x101b0a1a8) runs after the beat job and the phrase pass, with `AnalyzeJobKeySpecialInfo` (0x48 bytes): BALib wrapper, the `DetailedBeatInfo` array (after the phrase shift), BPM (`BPM×100 · 0.01`), the key buffer and `K`, "has beats", `rate` and `L − 1`.

## 2. `AnalyzeJobKey::getKey`
Inputs: beats `{u16 beatInBar, u16 tempo, u32 ms, u16 sub}`, `bpm` (f32), buffer `x[K]`, `rate`, `len = L − 1`; `dur = (float)len / (float)rate`.
1. **Segment step** (beats per segment): 4 when `75 < bpm < 90 && 240 < dur < 501`, or `90 ≤ bpm < 95 && 210 < dur < 501`, or `119 ≤ bpm ≤ 125 && 360 < dur < 501`, or `125 ≤ bpm ≤ 127 && 405 < dur < 501`, or `135 < bpm ≤ 140 && 360 < dur < 501`; else 2 when `(bpm ≤ 141 && dur < 501) || (bpm ≥ 151 && dur < 180)`; else 4 when `dur < 1200`; else 8.
2. **Segment boundaries:** first index `s = 5 − beatInBar` when `beatInBar ∈ 2..4`, else 0. `pos[c] = beats[s + c·step].ms · (rate / 1000)` (integer division of the rate), for every such beat; then `pos[0] = −1`. Segments `nseg = max(count, 2) − 1`.
3. **Whole-track buffer** `y` of length `K'`: `K' = K` unless `K < rate·0.42875`; then "no segments" and `K' = ⌈T / K⌉·K` (`fcvtps`, rounded up) with `T = rate·0.1875` if `K < rate·0.0046875`, else `rate·0.42875`. `y` is zeroed, then `CutBeats(y, x, K, rate/8, 8, 1)` (fills only the first `K` samples).
4. `pitch = (float)PitchDetect(y, K', rate·0.125, 8, 1)` (§ 4).
5. `GetKey(y, K', rate/8, pitch, 8, 1, …, segment = 0, seg_index = 0)` → whole-track `(key1, key2)`.
6. Without segments: `keys[0] = (key1, key2)`, the other pairs stay `(0, 0)`. With segments, for `i in 0..nseg` (`prev = 0` at the start, not `pos[0]`): `len_i = (pos[i+1] − prev)/8` (C division), stop when `offset + 4·len_i > K'` (bytes vs samples: rekordbox's own unit mix-up), copy `y[offset..offset+len_i]`; when `len_i ≥ rate·0.5375` run `GetKey(…, segment = 1, seg_index = i)` on it, else tile it `⌈T/len_i⌉` times (rounded up, `fcvtps`) (`T = rate·0.5375`, or `rate·0.1875` when `len_i < rate·0.0046875`) and run on the tiles; `keys[i] = (key1, key2)`; `offset += len_i; prev = pos[i+1]`.
7. `KeyJudge(key1, key2, nseg, segments_on, keys, bpm)` → the key code (§ 7).

## 3. `CutBeats(dst, src, n, rate, d = 8, mode = 1)` @0x101144d00
Spectral suppression of percussive energy, then resynthesis.
- `N = 4096/d = 512`, `H = N/2 = 256`, hop `h = (rate·30 > n ? 512 : 1024) / d` (64 or 128), frames `F = (n − N)/h` (C division).
- Band tables for `k in 0..H`: `lo[k] = (int)(k·0.70711356f)`, `hi[k] = min((int)(k·1.4142f), H − 1)`, `w[k] = log10f(k + 1) / (hi − lo)` (divide by 1 when equal).
- Per frame `f`: `re = hann(N) · src[f·h ..]`, `im = 0`; forward FFT; `mag[f][k] = sqrt(fma(im, im, re·re))`, phase `(sin, cos)` per `calc_phase_sincos` for `k < H`. Then for `k in 4..H`: band sum `S[k] = Σ mag[f][lo..=hi]` (NEON order, § 8), and afterwards `mag[f][k] = 0` where `mag[f][k] < w[k]·S[k]`.
- Low bins `b in 0..20`: one-pole low-pass over frames (`make_lpf0(fc)`: `pole = (float)exp(fc·−2π)`, `gain = 1 − pole`, `fc = ((float)(N/h)·5)/rate`; `filter1`: `y = fma(x, gain, s); s = y·pole`), and `mag[f][b] = min(mag[f][b], filtered[f])`.
- Resynthesis: `dst[0..n] = 0`; per frame `re[k] = cos·mag`, `im[k] = sin·mag` for `k < H`, mirrored `re[N−k] = re[k]`, `im[N−k] = −(sin·mag)`, `re[H] = im[H] = 0`; inverse FFT; `dst[f·h + i] = fma((re[i]·(h·2.6666667))·hann[i], 1/(N·N), dst[f·h + i])`.

FFT `fft_less_normalize(±N, re, im, sintbl, bitrev)` @0x10118678c: radix-2, unnormalised, bit-reversal swap first; twiddles `c = sintbl[j·N/(2k) + N/4]`, `s = sintbl[j·N/(2k)]`. Forward: `tr = fma(re1, c, im1·s)`, `ti = im1·c − (re1·s)` (fnmsub). Inverse (`n < 0`): `tr = re1·c − (s·im1)`, `ti = fma(im1, c, s·re1)`. Then `x1 = x0 − t`, `x0 = t + x0`.
`make_sintbl(N)` @0x10118656c and `hanning(N)` @0x10113f634 (`w[i−1] = (float)(0.5 − cos(i·(2π/(N+1)))·0.5)`, the multiply-subtract fused).

## 4. `PitchDetect(x, n, rate, d = 8, mode = 1)` @0x101169898 → reference pitch 434…446 Hz
- `short = rate·30 > n`. Long tracks: references `u = 0..13` step 1, hop `0xc00/d = 384`. Short: `u = 2, 4, …, 10`, hop `0x800/d = 256`, plus a refinement pass.
- Per reference `f0 = 434 + u`: 25 tones `f_0 = f0`, `f_k = f0 · 2^(k/12)` (f32 table @0x1037c7680, `k = 1..24`). Kernel 0 has `N_0 = (int)(2·rate / (f0·0.059463143))` taps, kernel `k = 1..23` has `N_k = (int)(2·rate / (f_{k+1} − f_k))` taps at frequency `f_k`, stored at offset `k·N_0`. Taps: `t_i = (i+1)·(1/rate)`, `(s, c) = __sincosf_stret(t_i·(f·6.2831855))`, `cos_k[i] = hann_k[i]·c`, `sin_k[i] = hann_k[i]·s`. Weights `w_0 = 1`, `w_k = (float)N_0/(float)N_k`.
- Windows `p = 0, hop, …` while `p < n − N_0`: `mag_k = w_k · sqrt(fma(im, im, re·re))` with `re = Σ x·cos_k`, `im = Σ x·sin_k` (NEON order, § 8). Arg-max over `k` with strict `>` from `FLT_MIN`; when it equals the previous window's arg-max (the first window always counts), `score[u] += max`.
- Short tracks: stable descending sort of the 13 scores (`QSort3`, ties by ascending index): best `b`, second `c`. `b == 2` → refine `u = 0, 1`; `b == 10` → `u = 11, 12`; else `u = min(b,c)+1, +2, … < max(b,c)`.
- Result: `434 + argmax(score)` (first maximum).

## 5. `KeyCodeScale` @0x101158438: the profile
Called as `KeyCodeScale(res, x, n, rate, pitch, 8, 1, NULL, profile[12], segment, &key1, &key2, seg_index, &hop, Nk[72], cos[], sin[], w[72])`. The kernel tables are built when `seg_index == 0` (the whole track and the first segment) and reused otherwise. `short = (float)n < rate·30`.
1. **Hop** (mode 1): 256 (`0x800/8`) for a short track with `segment ≠ 1` (the whole-track call); otherwise 2048, except at `(int)rate == 4000`: 512 for a short segment call, 1024 for a long track.
2. **72 tones:** `f_i = exp2f(i·0.083333336) · (pitch·0.0625)` for `i = 0..72`, `f_72 = pitch·4`. `N_k = (int)(2·rate/(f_{k+1} − f_k))` stored as floats, kernel `k` at offset `(int)(N_0·k)`, taps like § 4 with `t_i` from `N_0`. Weights `w_0 = 1`, `w_k = N_0/N_k`.
3. **Constant-Q matrix:** frames `F = (uint)(((float)n − N_0)/(float)hop)`; frame `f` centre `c = f·hop + (int)(N_0·0.5)`; tone `k` starts at `c − (int)(N_k·0.5)` (0 when negative). `C[k][f] = sqrt(fma(re, re, im·im)) · w_k`. The raw matrix is kept for § 6.5; a copy is normalised by its maximum (when `≥ 1.1920929e-07`).
4. **Smoothing:** each tone row of the copy goes through `filter1_hat_mono` with `make_lpf1(1, rate/hop)` (`t = tanf((float)(π/(rate/hop)))`, `B = t/(t+1)`, `A1 = (t−1)/(t+1)`; `w = fma(−A1, w_prev, x)`, `y = fma(w, B, B·w_prev)`).
5. **Profile:** per pitch class `pc`, octave sums over frames `O_o = Σ_f C[pc + 12·o][f]` (4-lane order, § 8); `p[pc] = (O_0·0.65 + 0.5·O_1 + O_2 + O_3 + O_4 + O_5)²`, the sum fused left to right. Mode 1 with `segment ≥ 2` would zero the weights of octaves 0–3; rekordbox only passes 0 and 1. (Modes other than 0, 1, 4 use `0.65/0.63`, `0.9`, `0.75` depending on `short`.)

## 6. `KeyCodeScale`: the decision
Notation: the **ranking** `r[0..12]` is the 12 classes sorted by `p` (`QSort3`); `pos(v)` is the place (0–11) of pitch class `v` in it. The **degree table** gives each key's seven scale classes: major `{0,2,4,5,7,9,11}`, minor `{0,2,3,5,7,8,10}` semitones above the root; `pB(d)`, `pS(d)` are the places of degree `d` (0 = root … 6 = seventh) of keys `B` and `S`. `rel(k)` is the relative key, `par(k)` the parallel one, `H(k)` the **harmonic index** (`harmonic_mixing_table`): the key's place on the circle of fifths, minor keys 0–11 from A♭ minor, major keys 12–23 from B major, so that relative keys share `H % 12`.

`QSort3(v, idx, lo, hi)` @0x10113fb48 sorts descending (`QSort`, a Hoare quicksort) and then re-sorts every run of equal values by ascending index (`QSort2_Ascend`): a stable sort by value, ties by index.

### 6.1 Ten candidates
The five strongest classes `r[0..5]`, each as a major and a minor root (candidate `c` = root `r[c/2]`, minor when `c` is odd). Score = base score for the root's place, plus, for each of the seven strongest classes `r[j]`, a bonus when `r[j]` is the candidate's third, else its fourth, else its fifth (first match only). Scores are f32 sums in `j` order. Tables (mode 1; long track / short track):

| | place / `j` → | 0 | 1 | 2 | 3 | 4 | 5 | 6 |
|---|---|---|---|---|---|---|---|---|
| base, major | both | 3.2 | 2.7 | 2.1 | 1.6 | 1.1 | | |
| base, minor | long / short | 3.781 / 3.67 | 2.7 | 2.1 | 1.6 | 1.1 | | |
| third (whole track) | long | 2.4 | 1.92 | 1.42 | 1.229 | 0.84 | 0.4 | 0 |
| third (windows) | long | 2.4 | 1.92 | 1.536 | 1.229 | 0.8 | 0.4 | 0 |
| fifth | long | 2.4 | 1.92 | 1.536 | 1.229 | 0.8 | 0.48 | 0 |
| third, fifth | short | 2.4 | 2.0 | 1.6 | 1.2 | 0.8 | 0.4 | 0 |
| fourth | long | 0.2 | 0.16 | 0.128 | 0.102 | 0.072 | 0.043 | 0 |
| fourth | short | 0.2 | 0.17 | 0.13 | 0.1 | 0.07 | 0.03 | 0 |

Tables in the binary: `table_1st_maj` @0x1037c7718, `table_1st_min` @0x1037c77e0, `table_3rd` @0x1037c78a8, `table_5th` @0x1037c79c0, `table_1st_maj2` @0x1037c7b7c, `table_1st_min2` @0x1037c7c44, `table_3rd2` @0x1037c7d0c, `table_4th2` @0x1037c7e24 (both passes use it; there is no v1 `table_4th`), `table_5th2` @0x1037c7f3c (= `table_5th`). Layout `[mode 0..4][long, short][7 or 5]` f32.

The ten scores are ranked (`QSort3`): the best is `B`, the second `S`.

### 6.2 Whole track: `B` or `S`
1. **Short track, parallel pair** (`S = par(B)`): `B` becomes the minor one, unless the minor third ranks beyond place 2 and more than two places below the major third, and then `pos(min 3rd) + pos(min 6th) + pos(min 7th) > pos(maj 3rd) + 2 + pos(maj 6th) + pos(maj 7th)`; then the major. (`S` is not changed, so `B` may now equal `S`.)
2. **`b66`** (`S = B + 12`, its parallel minor): false when `B`'s 3rd, 6th and 7th all rank at least as high as `S`'s; else true when at least two of `S`'s rank higher, or `pS(2)+pS(5)+pS(6) < pB(2)+pB(5)+pB(6)`.
3. **`harm`**: `H(S) − 1 = H(B)` (unless `B = 16`), `H(S) + 1 = H(B)` (unless `B = 2`), or the wrap-arounds `H(S) ± 11` at the circle's ends; for a minor `B` and major `S` also `H(S) − 13`, `H(S) − 11` (unless `B = 2`), `H(S) − 1` at the ends and `H(S) − 23` for `B ∈ {2, 23}`; for a major `B` and minor `S` the mirror cases (`H(B) − 12 = H(S) + 1`, `H(B) − 13 = H(S)`, `H(B) − 23`, `H(B) − 1` with `H(B) = 12`). (`scale.rs::harmonic_whole` has the exact order.)
4. **`u110`** (`S` minor with its root ranked first): with `a = ΣpS − ΣpB` (all seven degrees), `c = pS(0) + pS(2) + pS(4) − u` where `u = pB(0) + pB(2) + pB(4)`, `e = pS(5) − pB(5) + pS(6) − pB(6)`: requires `Σ_{d∈0,1,2,4}(pS(d) − pB(d)) < (B minor ? 8 : 6)` and (`u > 7` or `c < 5`). Threshold `t = 0` when `B` and `S` are parallel, else `2 / 3 / 4` for `pB(0) < 2 / = 2 / > 2`. With `harm` all of `a, c, e` must be `≤ t`; without, at least one.
5. **Second-place root** (`S` minor with its root ranked second or third): `a` as above, `b = Σ_{d∈0,2,4}(pS(d) − pB(d))`, `c = pS(5) − pB(5) + pS(6) − pB(6)`. All three negative → swap outright. All positive → no vote. Else, when `a < 5 && c < 5`: vote when `a + b + c < 4 && b < 1 && (b < −4 || Σ_{d∈0,1,2,4}(pS(d) − pB(d)) < −4)`; otherwise vote when `(c < 1 || a < 1) && b < −5 && a + b + c < 4`. Without a second-place root there is no vote.
6. **Swap** when `b66 || u110 || vote`: `key1 = S`, `key2 = B`; else `key1 = B`, `key2 = S`. `key2` is final.

### 6.3 24-key ranking and harmony
Every key gets `W = Σ_d weight_d · (pos(degree d) + 1)` (each product rounded to f32, summed in degree order) with weights root 4, 2nd 0.9, 3rd 3, 4th 0.9, 5th 3.5, 6th 0.6, 7th 2. The 24 keys are ranked with `QSort3`, the arrays reversed (ascending, lowest `W` first, ties now by descending index) and one bubble pass swaps equal neighbours whose indices are descending. The same loop keeps two plain sums: `T(k)` over degrees 0, 2, 4 and `A(k)` over all degrees (of `pos + 1`).

Harmony score of a key `k` against a set of keys (in order): per other key `o`, add 1 when `H(o)` is a neighbour of `H(k)` (`H(o) − 1` unless `k = 16`, `H(o) + 1` unless `k = 2`, or the ends `H(o) + 11 = H(k) ∈ {11, 23}`, `H(o) − 11 = H(k) ∈ {0, 12}`), else 1.5 when `H(o) = H(k) ± 12` (the relative key), else 3 when `o = par(k)`. For minor `k` the first neighbour adds 1.6 instead of 1.

Whole track: the five best of the 24-key ranking are scored against the twelve best plus a bonus 9, 6, 5, 4, 1 by place; `QSort3` → `top1`, `top2`.

### 6.4 30-second windows
`win = (uint)(rate·30/hop)` frames, `step = (int)(rate·10/hop)`, `count = (uint)((float)(F − win)/(float)step)`. Window `w = 0..count` starts at frame `step·(w+1)` (the window at frame 0 is never used) and the loop stops when `start + win ≥ F`. Per window, on the **raw** matrix (not normalised, not smoothed):
1. Profile as § 5.5 over the window's frames; ranking `r'`.
2. Candidates with the window tables (§ 6.1) and the track's `short` flag (always long here) → `B'`, `S'`.
3. Window rules: `b66' = (S' = B' + 12) && (pS(2) < pB(2) || pS(5) < pB(5) || pS(6) < pB(6))`; `harm'`: `H(S') − 1 = H(B')` (unless `B' = 16`), `H(S') + 1 = H(B')` (unless `B' = 2`), `H(S') + 11 = H(B') ∈ {11, 23}`, or `H(S') − 11 = H(B')` with `B' ∈ {2, 23}`; `u110'` as § 6.2.4 but with `c < 6`, `t = 2 / 3 / 4` for `pB(0) < 3 / = 3 / > 3`, and the vote "at least two of `a, c, e` are `≤ t`" with `harm'`, "at least one" without; the second-place test as § 6.2.5. Result `pv = S'` on a swap, else `B'`.
4. 24-key ranking `o` on `r'` with `T`, `A`. Choice: if `o[0]` is minor, its relative major is among `o[1..5]`, `A(o[0])` and `T(o[0])` are the minima over the five and `o[0] = pv` → `pv`. Else if `o[1] = o[0] + 12` and `T(o[1])` is the minimum of `T` over the five → `o[1]`. Else the harmony score of `o[0..5]` against `o[0..5]` plus a bonus 5, 4, 3, 2, 1 → the best.
5. The choice is a **vote** when it equals `pv` and its root is `r'[0]` or `r'[1]`.

### 6.5 Final `key1`
- `vote` = the most frequent vote (first on ties), 24 when there are none.
- `best` = the key with the smallest `Σ_d pos(degree d)` among `key1`, the major and the minor on `r[0]` (ties to the earlier).
- The **list**: every key whose root, third and fifth classes are all in `{root(vote), best, top2, top1, key2, key1} mod 12`.
- `p1` = place of `key1`'s root, `pv` = place of the vote's root (0 when there is no vote).

Then: if `key1 = vote`, or `key1 ∈ {top1, top2}` with `p1 < 2`, the candidate is `key1`; otherwise an empty list returns `key1` when there is no vote and goes to the vote test below when there is one; a non-empty list keeps `key1` as the candidate only when `p1 < 3` and `key1` is on the list, else the candidate is "none". A vote on the list replaces the candidate. A candidate is the result. With no candidate and no vote, `key1`. With no candidate and a vote (the **vote test**): the vote when `pv > 3` and `vote ≠ key2`; otherwise the vote when it is `top1`, `top2` or `best`; otherwise `key2` when `key2 = best = top1`, else `key1`.

## 7. `Key_judge_with_keys` @0x100c2e5ac (`KeyJudge2.cpp`)
Inputs: the whole-track `key1` (`key1_current`), `key2` (only fed into state the function overwrites), `nseg`, the segments flag, the segment pairs and the BPM. The state lives in globals (`top5`, `scores`, `scores5`, `scores_c`, `rdp`, `top12r`, …), most reset at entry. With `nseg < 9` or segments off: `new_algo_skip`, straight to the final check (step 3) on an empty state, which returns `key1`.
1. **Top five** (`Key_judge_with_keys`): histogram of the segment `key1`s; seven times the first maximum (count 30 for an empty bin, key 0). When `key1 ≠` the most frequent key `t0`: `t0`'s count gains the segments in `rel(t0)`, and `rel(t0)` is removed from places 1–4 (later entries move up, key 30 / count 0 enter at the end); the new second key `t1` gains `rel(t1)`'s segments and `rel(t1)` is removed from places 2–4; if the second count is now larger, the first two places swap, using the second key as it was **before** the removals. `r_t1all = c0/nseg`, `r_t12all = (c1 + c0)/nseg`.
2. **`KeyJudge_step_1`** @0x100c2ee50: for each of the three best keys, its six relations (parallel ×1, dominant ×2, subdominant ×2, relative ×3, and for major keys the minor iii and ii, for minor keys the major VII and VI, ×3) are scored against the other two best keys (`scores`), the same plus places 4–5 (`scores5`), and the other two plus `key1` (`scores_c`); `key1`'s relations score against the three (start 1). `dt2` counts entries into the three keys along the segment sequence; `p_cntXY` counts `X → Y → not X` patterns; `rdp = dt2 / Σp_cnt` (1 when none), `top12r = c0/c1`, `top13r = c0/c2`, `top23r = c1/c2`; `key_no1/2` from the pattern rates; flags for parallel (`doshu12/13`) and relative (`heiko12/13`) pairs among the three. Then a decision tree over the totals `scores + scores5 + scores_c` per key, their ratios and `rdp` sets `pre_final` (1–4: first, second, third best, or `key1`) and `pre_judge_t`.
3. **`KeyJudge_step_2toFinal`** @0x100c30410: `final = top1`, then a tree by `top12r`, the score comparisons, the parallel/relative flags and the ratios (`judge_type` 0x0b … 0x30d), falling back to `pre_final`. Final check: `key1` wins outright when it is among the five, `scores_c` of `key1` beats the three, `rdp ≤ 3.8`, `top12r ≤ 1.47`, `c0/c4 ≤ 22` and the BPM is outside 121–128, unless the `judge_type` is 0x33, 0x54, 0x70 or 0x1f; when not all of that holds, `key1` still wins for type 0x52 with `rdp > 4` and `c0/c4 < 4`, and for type 0x0b with `rdp > 4.1` and `top12r ∉ (2, 3]`.

The full trees are in `judge.rs`, branch for branch. Literal constants that are not exact in f32 (1.2, 1.1, 1.8, 1.55, 0.26, …) are double; the float values are widened before the comparison.

## 8. Floating-point order
The binary is built with contraction and vectorised reductions. Dot products and band sums of 16+ elements run in 16-element chunks into four 4-lane accumulators (chunk quarter `q` → accumulator `q`); the reduction is `a3 + (a2 + (a1 + a0))` lane-wise, then `(l0 + l1) + (l2 + l3)`; the remaining elements are added one by one (fused multiply-add for products). Frame sums (§ 5.5, § 6.4) are 4-lane sums reduced `(l0 + l1) + (l2 + l3)`, then the rest one by one.

## 9. Verification (2026-10-12)
`cargo run --release -p mb-analysis --example key_score` (`MB_ORACLE=oracle-b` for the second fixture). Tests:
- `cargo test -p mb-analysis --test key`: triads, the judgement's short path, `QSort3` ties, and the oracle keys of the sampler loops (`MB_ORACLE_FULL=1` for all tracks).
- `cargo test -p mb-library --test analyse_ops`: the probes' keys through `analyse_track`, and the database write (`ops/analyse-track.md`).

| Fixture | rekordbox's grid + our key buffer | our whole pass (grid, phrase shift, key) |
|---|---|---|
| oracle-a | 169/169 | 169/169 |
| oracle-b | 169/169 | 160/169 |

The nine oracle-b differences are grid differences: oracle-b was analysed with high-precision beat analysis, and for all nine our Normal-mode result equals the key rekordbox stored in oracle-a.

Every stage was also compared against rekordbox's own routines, run natively from a copy of the arm64 slice in a harness outside the repo, on the 25 MP3 tracks of oracle-a: whole-track pairs 25/25, segment pairs 6339/6339, window choices 1018/1018, window votes 25/25; the judgement on 400 000 random inputs with its full internal state.

## 10. Open questions
- **Segments flag.** `getKey`'s segments argument is a byte of the job's special info (+0x3c, "has beats"); musicbox passes "the grid has beats". Without a grid musicbox still runs the whole-track estimate (no segments); what rekordbox does for a track whose beat analysis fails is untested.
- **Fewer than two boundaries.** With 0 or 1 segment boundaries `getKey` reads past the boundary list (uninitialised memory); musicbox stops instead. No fixture has segments on with so few beats.
- **Other modes.** Only mode 1 (`BeatAnalyzer_1_0`, `param_7 = 1`) is ported; high-precision analysis changes the grid (§ 9), not this code.
