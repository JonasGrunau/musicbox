# Waveform analysis (PWAV, PWV2, PWV3, PWV4, PWV5, PWV6, PWV7, PWVC)

Status: **verified**. `mb-analysis::analyze_waveforms` reproduces rekordbox 7.2.19's waveform tags byte for byte: 169/169 corpus tracks in all eight tags against `fixtures/oracle-b`. AAC is now stable too: the ±1 (PWV7 ±2) columns some runs showed came from the AAC decode's dependence on stack alignment, fixed in `mb-audio` (`decode.md` § 6). Scorecard: `docs/scorecards/waveforms.json`.

Code: `crates/mb-analysis` (`zoom.rs` PWV3, `loud.rs` PWAV/PWV2, `wavecreator.rs` PWV4–PWVC) and `crates/mb-dsp` (filters, libm calls). Tag layouts are in `anlz.md`; decoding and the analysis input are in `decode.md`.

All addresses are in the arm64 slice of rekordbox 7.2.19.

## 1. Where the samples go
`analyzer::CBeatAnalyzeThread::analyze` @0x1001528c4:

1. **Input.** The decoded track (`decode.md` § 2) at positions `0 .. L − 2`, so `L − 1` samples. Each sample is quantised to 16 bit and handed on as `(float)short`, left and right separately (`getStreamAt` @0x100158f08).
2. **Blocks.** The loop reads `2 × rate` samples per step (the last step is shorter).
3. **Two consumers per block:**
   - `BALibWrapper::SetSample` → `BeatAnalyzer_2_0::Analyzer::setSample` @0x10116bae8: BALib, which makes PWAV, PWV2 and PWV3 besides beats.
   - `WaveCreator::readWaveData` @0x100d564c4 when any of the four colour/3-band waveforms is enabled.
4. **After the last block:**
   - `WaveCreator::createWave` @0x100d54974 builds PWV4, PWV5, PWV6 + PWVC and PWV7, in that order.
   - BALib's results are fetched with `BA_GetLoudWave`, `BA_GetDotsWave` and `BA_GetZoomWave` and quantised by `notifyLoud` @0x1001592cc, `notifyDots` @0x1001593d4 and `notifyZoom` @0x10015980c.

**BALib version.** `BALibWrapper::SetVersion(2)` runs when the analysis mode field (thread +0x38d0) is 2, 3 or 4, otherwise version 1 is used. The `BeatAnalyzer_1_0` view makers have the same sizes and structure as the `_2_0` ones; only `_2_0` has been ported and checked.

**Lengths.**
- BALib gets `total = L − 1` (`Analyzer::create`, second argument).
- WaveCreator sizes its arrays from the reader length `L`:
  - milliseconds `N = ⌈L·1000 / rate⌉`;
  - PWV5/PWV7 columns `C = ⌈L·150 / rate⌉`, computed in double and rounded up with `fcvtpu`.
- PWV3 has `⌈(L − 1)/(rate/150)⌉` columns, so it can be one column shorter than PWV5 (`anlz.md`).

## 2. PWV3: `BeatAnalyzer_2_0::ZoomViewMaker`
Created by `Analyzer::create` as `create(rate, total, 150, normalise = true)` @0x1011568c4. Fed by `putSample` @0x101156a80 with interleaved shorts.

**Columns.** A column is `rate/150` samples (294 or 320). It closes when it is full or when `total` samples have been seen; the last column is usually partial.

**Mono value.** Per sample, `m = (int)((double)R + (double)L) · 0.5)`. Its magnitude is `|m|` masked to 16 bits and clamped to 0x7fff. Two maxima are tracked:
- the column peak `p`;
- the track peak `P`, across all samples, used for normalisation.

**Whiteness filter.** The mono values go through a second-order low-pass, run per column with two samples of history carried over:
- `k = tan(150π / rate)`, `t = √2·k`, `inv = 1/((t + 1) + k²)`
- `b0 = b2 = k²·inv`, `b1 = 2k²·inv`, `a1 = (2k² − 2)·inv`, `a2 = ((1 − t) + k²)·inv`
- Evaluation: `y = b0·x0`, then `y = fma(b1, x1, y)`, `y = fma(b2, x2, y)`, `y −= y1·a1` (fused) and `y −= y2·a2` (fused).

**Whiteness level.** The filtered column peak `q` is formed like `p` (truncate, absolute value, clamp to 0x7fff). The level is `r = q/p` in double:
- `r ≥ 0.875` → 0, `≥ 0.75` → 1, `≥ 0.625` → 2, … `≥ 0.125` → 6, otherwise 7.
- `p = 0` → 7.

**Normalisation** at the end. Each column height `h` becomes `v²·K` with `K = 2.9327451233027466e-08`, so `32767 → 31`. The height loop is vectorised, and the vector part and the scalar tail round differently:
- **Columns `0 .. n − r`**, where `r = n & 3` or 4 when `n` is a multiple of 4; only when `n > 4`: `v = (int)(long)(h · (32767/P))`, then `(short)(long)(v·v·K)`.
- **The remaining columns:** `v = trunc((h·32767)·(1/P))`, then `(short)(int)(v·v·K)`.
- If `P = 0`, every height is 0.

**Byte.** `(height & 0x1f) | whiteness << 5` (`set_pmai_ext_zoom_wave` @0x100f262b4).

## 3. PWAV and PWV2: `BeatAnalyzer_2_0::LoudViewMaker`

### 3.1 Feed
**Analyzer blocks.** `Analyzer::create` sets the block length to `rate/75` (588 or 640 samples), using a multiply-shift for the division. `setSample` fills one block with:
- the float mono `(R + L) · 2⁻¹⁶`;
- interleaved shorts.

A full block, or the last samples once `total` is reached, goes on to the consumers below.

**BeatSensor bands.** The block goes to `peakss::BeatSensor::put` @0x101182eb4 (6 bands, `BeatSensor::create` @0x101198414). Each band filters the block (§ 3.2) and rectifies it:

| Band | Filter |
|---|---|
| 0 | `LowPath(283)` |
| 1 | `BandPathW(150, bw 1.5)` |
| 2 | `MidPathB(283, 7900)` |
| 3 | `HiPath(7900)` |
| 4 | copy of band 0, detrended |
| 5 | copy of band 3, detrended |

`Q` is `1/√2` (float) for every filter.

**LoudViewMaker call.** With 6 bands, BeatSensor calls `LoudViewMaker::analyzePeak(mono, NULL, band2, band3, n)` @0x101195dd8 through the vtable slot +0x10.

**Paths not taken.** `Analyzer+0xa` is never set, so the "simple envelope" path is dead. When BPM analysis is off, BALib is created with a zero flag and `analyzePeak(mono, NULL, NULL, NULL)` is called directly; LoudViewMaker then uses its own 6 kHz filters for mid and high. That path is not ported.

**Inside `analyzePeak`.**
- The NULL low band is computed with LoudViewMaker's own `BandPathQ(150, Q 0.9)` and rectified.
- The chunk is processed in pieces of `rate/75`; one piece per call here.
- Every value is multiplied by 32768: full (mono), low, mid and high.

### 3.2 BALib filters (`IirPassFilter`)
Coefficients are computed in double from float parameters, using the libm `sincos`, `sinh` and the constant `2π = 6.28318530717958` (0x401921fb54442d11):
- `w = f·2π/fs`
- `α = sin w / (Q + Q)`
- `inv = 1/(α + 1)`
- `a1 = (cos·−2)·inv`
- `a2 = inv − α·inv` (fused)

| Design | b0 | b1 | b2 | Sections |
|---|---|---|---|---|
| LowPath | `(0.5 − cos·0.5)·inv` (fused) | `inv − cos·inv` (fused) | b0 | 2 identical |
| HiPath | `(cos·0.5 + 0.5)·inv` (fused) | `−(cos + 1)·inv` | b0 | 2 identical |
| MidPathB(lo, hi) | — | — | — | HP(lo), LP(hi), HP(lo), LP(hi) |
| BandPathQ | `α·inv` | 0 | `−α·inv` | 2 identical |
| BandPathW(bw) | `α'·inv` | 0 | `−α'·inv` | 2 identical, with `α' = sinh(ln2/2 · bw · w / sin w) · sin w` |

`IirPassFilter::operate` @0x101143a2c runs as follows:
1. Widen the float input to double.
2. Run the sections in turn. Each is the direct-form-I biquad of `IirFilter::operate` @0x101143184: `ff = fma(x2, b2, fma(x1, b1, x·b0))`, `fb = fma(y2, a2, y1·a1)`, `y = ff − fb`, with state kept across calls.
3. Narrow the result to float.

WaveCreator's `BandPassFilter` evaluates the same way (§ 4.1).

### 3.3 `PeakDetect` @0x101197780 (onsets per band; bands low, mid, high)
**Per-band state:** a sample counter `c`, a peak index `k`, a call counter, and a done flag. The peak list holds 36000 entries of `{value, value900, pos, max_at}`. Entry 0 starts as `{0.01, 0.01, 0, 0}`.

**Thresholds.** `thr = 0.66` for the high band, 0.75 otherwise. The running level `cur` is restored from `peaks[k].value` (or `peaks[0]` when `k < 1`).

**Per sample** (`c += 1`):
1. **Waiting.** While `peaks[0].value == 0.01`, the first sample with `(double)x > 0.01` starts peak 0: `value = x`, `pos = c`, `c = 0`, `k = 1`. The low band also raises the "onset" flag.
2. **Below threshold.** If `x < cur·thr` (when `cur > 0.01`), or `x < 0.01` (when `cur ≤ 0.01`):
   - If `c > 1.714·rate`, every `(int)(rate·0.011609977…)` such samples (512 at 44.1 kHz, counted per call) multiply `cur` by 0.9938303. `cur` does not drop below 0.01.
   - Otherwise the counter for that is reset.
3. **Above threshold, `c < 0.32·rate`.** A larger `x` becomes `cur` and `peaks[k].max_at = c`.
4. **Above threshold, `c ≥ 0.32·rate`: new peak.**
   1. `peaks[k].pos = c` and `peaks[k − 1].value = cur`.
   2. Then `k += 1`, `c = 0`, `cur = x`.
   3. The low band raises the onset flag.
5. **End of track** (sample `total − 1`): `peaks[k] = {cur, pos c}`, `k += 1`, done = 1.

**At return:** `peaks[k].value = cur`. The function returns `k`.

### 3.4 `PeakDetect900` @0x101197ab4 (slow onsets, low and mid bands)
Like § 3.3, with these differences:
- It acts only on every fourth sample (`c & 3 == 0`), using the maximum over those samples (`fmaxnm`). The running maximum restarts at 0 at each call.
- Levels are kept in `value900`. The start level and the floor are 327.68 instead of 0.01. The decay is counted in steps of 4 samples and floored with `fmaxnm`.
- A new peak stores `peaks[k].value900 = cur` before `k += 1`.
- A sample off the 4-grid is still processed when it is the track's last sample, but only when the final chunk is short. A track whose last chunk is exactly full ends without the done flag.

### 3.5 Columns (`analyzePeak` after the detectors)
**Constants.**
- `U = ⌈total/(rate/75)⌉` (float `frintp`)
- `step = U·0.005`
- `g = 1/U`
- `chunks` counts calls (1/75 s each)

**Full-band mean.**
- Sum `|full|`, carried over from previous chunks, in float. It uses 16 lanes for the first `n & ~15` samples, then the scalar tail. The lane sums are combined as `((l0+l4)+l8)+l12` per lane group, then `(a+b)+(c+d)`.
- The mean is emitted when more than 8 chunks have accumulated or the low band raised its onset flag: `sum / (block · chunks)`.
- An onset-triggered mean is written into the column of the previous emission once more, through a two-flag delay.

**Full columns (400).**
- The target column is `(int)(chunks·400·g)`, capped at 399, or 400 once the low band is done.
- When the column advances, the carried value is rescaled by `15000·g` and the skipped columns are zeroed.
- An emitted mean is added to the current column. A delayed one goes to the remembered column, scaled by `15000·g` unless it is the current one.

**Band columns (low, mid, high; 400 each).** Pairs of columns hold the sum of `trunc(peak value)` for the peaks completed in that stretch of the track. Peaks are consumed only when new ones appear: up to `count − 1`, or `count` once the band is done. On each consumption, `frac += chunks − last`. While `frac ≥ step`:
1. The pair is written as `carry·7500·g`.
2. Further whole steps write zero pairs.
3. One more zero follows the last pair.
4. `carry` restarts at 0.

The peak being consumed is then added. The last one is left out when the band is done.

**Column values.** Once all bands have reached a column, or are done, that column gets:
- dB levels: `low·0.5`, `mid·0.25` and `high` each become `(float)(log10(x·2⁻¹⁵)·20) + 30.5`, clamped to [0, 30] and truncated (0 for `x ≤ 0`);
- the height `min(fma(low·0.75, 1/32768, 0.25), 1) · full`.

**Dots (100).**
- Per detector (low, mid), `dot = chunks / D`, where `D = (total/(rate/75))/100`, or 1 for short tracks. It is capped at 99, or 100 once that detector is done.
- When the dot advances, the carried value becomes `(carry·75)/(2D)` (integer).
- From chunk 51 on, a chunk in which the detector found new peaks adds `(long)value900` of the first new one.
- Dot value: the low count, or 1.0 when only the mid count is non-zero, or 0.

### 3.6 Output
**Colour** (`analyzeLoudColor` @0x101197d64), from the dB levels `a` (low), `b` (mid) and `h` (high):

| Condition | Colour |
|---|---|
| `a < 14`, `b − h < 4` | 5 |
| `a < 14`, `b − h ≥ 4` | 4 |
| `a ≥ 14`, `b < h`, `a > h` | 2 |
| `a ≥ 14`, `b < h`, `a ≤ h` | 3 |
| `a ≥ 14`, `b ≥ h`, `a > b` | 0 |
| `a ≥ 14`, `b ≥ h`, `a ≤ b` | 1 |

Before this runs, an entry has colour 5.

**PWAV height:** `CBeatAnalyzeLib::V_Devide` @0x10015c18c compares the height with 23 float thresholds (table @0x1037528d0, 50790.4 down to 2195.456) and returns 25 down to 3, or 2. PWAV byte = `height | colour << 5`.

**PWV2:** `notifyDots` maps `(long)value` to a 1–15 scale:

| Value | Dot |
|---|---|
| 0 | 1 |
| > 20674 | 15 |
| > 16421 | 14 |
| … | … |
| > 1303 | 3 |
| otherwise | 2 |

The thresholds in between are in `loud.rs`. PWV2 has no colour.

Columns that were never computed would stay zero bytes. On a complete analysis every detector finishes, except the § 3.4 quirk.

## 4. PWV4–PWV7, PWVC: `analyzer::WaveCreator`

### 4.1 Filters (constructor @0x100d53c70)
**Design.** Eighteen `BandPassFilter` biquads (stride 0x58, coefficients at +8):
- `w = (1/fs)·2πf`, with the `2πf` constants stored in the binary
- `(sin, cos) = __sincos_stret(w)`
- `α = sin·(1/√2)` and `inv = 1/(α + 1)`

**Coefficients.**
- Low-pass: `b0 = b2 = (0.5 − cos·0.5)·inv` (fused) and `b1 = inv − cos·inv` (fused).
- High-pass: `b0 = b2 = (cos·0.5 + 0.5)·inv` (fused) and `b1 = −(cos + 1)·inv`.
- Both: `a1 = (cos·−2)·inv` and `a2 = inv − α·inv` (fused).

**Filters in use:**

| Offset | Filter | Offset | Filter |
|---|---|---|---|
| +0x30 | LP 400 | +0x348 | LP 100 |
| +0x88, +0xe0 | LP 200 (×2) | +0x3a0 | HP 300 |
| +0x138, +0x190 | HP 200 (×2) | +0x3f8 | LP 3000 |
| +0x1e8, +0x240 | LP 2000 (×2) | +0x450 | HP 1500 |
| +0x298, +0x2f0 | HP 2000 (×2) | +0x4a8 | LP 300 |
| | | +0x500 | HP 250 |
| | | +0x558 | LP 1200 |
| | | +0x5b0 | HP 3000 |
| | | +0x608 | LP 9000 |

### 4.2 `readWaveData`: millisecond records
**Mono mix.** Each sample is scaled by `1/32767` (0x3f00002000400080) to `l, r` in double:
- `||l| − |r|| < 0.001` → the louder of the two, with `r` on a tie;
- otherwise `(r + l)·0.5`.

**Millisecond grid.** Milliseconds are counted **from the start of each block**:
- `d = 1000/gcd(rate, 1000)` and `q = rate/gcd`
- millisecond `i` starts at sample `⌈i·q/d⌉` and runs to the next start (at least one sample)
- a block yields `⌊n·d/q⌋` milliseconds; a partial millisecond at the block end is dropped
- with 2-second blocks this matters only for the last block
- records beyond the last summarised millisecond stay zero

**Records per millisecond.** "Q15" means `trunc(x·2¹⁵)`, with special cases for ±1 and NaN.

| Record | Filters | Fields |
|---|---|---|
| A | low = LP200², mid = HP200² → LP2000², high = HP2000², plus LP400 | max and min of the mono (signed Q15); max \|LP400\|; per band, `peak·32768·won/samples`, where `won` counts the samples in which that band had the largest signed value |
| B | LP100; HP300 → LP3000; HP1500 | max, min, then the three band peaks (Q15, or 0x7fff from 1 up) |
| C | LP300; HP250 → LP1200; HP3000 → LP9000 | the three band peaks |

### 4.3 PWV4 (`createWholeWaveData` @0x100d549fc), 1200 points from record A
**Bucketing.** Milliseconds are distributed with an accumulator: `acc += 1200`, and a point closes when `acc ≥ N`. Each closing point is copied over any points skipped (short tracks).

**Values per point** (floats):
- running max of `max`
- min of `min`
- max of LP400
- the largest low share in a ±12 ms window. The window keeps the position of its maximum and only rescans when that falls out. The value is not reset on a rescan, only at the point boundary.
- max of the mid share over the previous, current and next millisecond
- max of the high share

**Bytes:** `(int)(v·128·(1/32768))` in float, as `[max, min, lp400, low, mid, high]`.

### 4.4 PWV5 (`createColorWaveData` @0x100d54c70), C columns from record B
**Bucketing.** As in § 4.3, with C columns, and the same ±12 ms low window and ±1 ms mid window.

**Track peak.** The peak used for normalisation is updated each millisecond from the *current* column's arrays **before** they are written. So it never sees the last millisecond of a column. The arrays come from an uninitialised allocation, read as zero here (§ 6).

**Colour** (`calcZoomWaveColour` @0x100d573d4, float) from the low, mid and high peaks:
1. Scale to the largest of the three, ×255.
2. When high < 64, pull low and mid down by `low·mid·(1/765)·(1 − high/64)`.
3. `mid −= mid·0.00234375·min(max(low, high), 128)`.
4. `high ×= 1.3`.
5. Clamp to [0, 255] → RGB.

All-zero input → white.

**Fields.** Each channel is reduced to its top 3 bits. When every channel is ≤ 31, the brightest channel(s) get 1 and the others 0 instead.

**Height:** `max(|max|, |min|)` (clamped to 0x7fff), normalised like PWV3's scalar tail: `trunc((h·32767)·(1/peak))²·K`. If `peak < 1`, the height is 0.

**Word:** `r << 13 | g << 10 | b << 7 | height << 2`.

### 4.5 PWV6 and PWVC (`create3BandsWholeWaveData` @0x100d55210), from record C
**Averages.** 1200 points of per-band averages over a look-back window that grows from the point start:
- `N/1000` milliseconds for low (300 ms for a 5-minute track), two thirds of that for mid, `N/3000` for high;
- negative indices count as zeros;
- sums and counts reset per point.

The per-band **global maximum** of the record values is kept over the whole track.

**Totals.** Each band's 1200 averages are summed with eight accumulators, combined as `((s0+s2)+s4)+s6 + ((s1+s3)+s5)+s7`.

**Gains.** Each band gets a gain `∝ 1/total`, scaled so that the smallest gain is 1. A band whose total is 0 gets gain 1; the code has explicit cases for that.

**Scale.** With `peak = max over points of fma(high, gH, fma(mid, gM, low·gL))`, each gain becomes `gain·32768/peak`.

**PWVC.**
- low = `min(g, 32768/maxLow)`
- mid = `min(g, 32768/maxMid)`
- high = `min(g, 32768/(16384 − cos(maxHigh·π/32768)·16384))` (fused)
- each is then `80` if `< 0.8`, else `min(·, 3 or 5)·100`, truncated to u16.

**PWV6:** `trunc(avg · min(max(g, 0.8), 3 or 5)/256)` as bytes (low, mid, high).

### 4.6 PWV7 (`create3BandsZoomWaveData` @0x100d55fbc), C columns from record C
**Envelopes.** Decaying peak envelopes per band (decay 0.99 low, 0.98 mid, 0.97 high):
- above the envelope: jump to the value;
- below: `(env − x)·decay + x` (fused).

At each column start the envelope restarts 300/200/100 ms back and is run up to the current millisecond.

**Bytes.**
- low: `(int)(((float)PWVC₀·0.01)·(1/256)·(double)(float)env)`
- mid: the same with PWVC₁
- high: `(int)((64 − cos(env·π/32768)·64) · ((float)PWVC₂·0.01))`

The last millisecond of a column wins; skipped columns are copies.

## 5. Verification (2026-10-12)
`cargo run --release -p mb-analysis --example waveform_score`, plus `MB_ORACLE=oracle-b` for the second fixture. The test is `cargo test -p mb-analysis` (`tests/oracle.rs`: probes and sampler loops against oracle-b; `MB_ORACLE_FULL=1` for all 169 tracks).

Against **oracle-b**, all 169 tracks are byte-exact in all eight tags: MP3 37, AAC 19, ALAC 14, FLAC 23, WAV 34, AIFF 34 and JUCE 8, including the 22.05 kHz and 96 kHz SRC probes. The results were the same with 1 and with 14 analysis threads.

In runs earlier the same day, the same code left 4 to 10 AAC tracks off by 1 (PWV7: 2) in a few columns. The cause was the AAC decode, not this port: `AudioConverterFillComplexBuffer` entered at SP ≡ 48 (mod 64) decodes slightly differently (`decode.md` § 6). With `mb-audio` entering it at a fixed alignment, all 19 AAC tracks are byte-exact in every tag against both oracles, in every run.
- Against **oracle-a**, the only additional differences are PWAV colours on four sampler loops and PWV2 on one. oracle-a holds an older analysis of those demo files, and rekordbox's re-analysis in oracle-b agrees with ours.

## 6. Open questions
- ~~**AAC decoding is not always reproducible.**~~ Resolved (`decode.md` § 6): the decode depends on the stack alignment at `AudioConverterFillComplexBuffer`, which followed the caller's stack (on the main thread also argv and the environment). `mb-audio` now enters it at a fixed alignment that matches rekordbox.
- **Uninitialised read.** The peak scan in `createColorWaveData` reads each column before its first write. We model that memory as zero; rekordbox's allocation is large enough to come from fresh pages, but a recycled block could differ.
- **BALib version 1** (`BeatAnalyzer_1_0`, analysis modes other than 2–4) and the LoudViewMaker path without BeatSensor (BPM analysis off) are not ported.
- **The end of `PeakDetect900`.** A track whose last chunk is exactly `rate/75` samples and whose final sample is off the 4-grid never sets the mid/low dot "done" flag. The port reproduces this, but no fixture exercises it.

## 7. The player's zoomed waveform (not stored)
rekordbox does not draw its deck's zoomed strip from PWV5 or PWV7. The player engine builds a waveform of its own from the audio when it loads a track, and the stored tags are only drawn for the other data class (below). Read from the binary; not compared pixel by pixel.

**Building it.** `djplay::WaveZoom::readWaveZoom` @0x10198b4c4 runs the same mono mix (§ 4.2, the 0.001 rule) and the same eighteen biquads as WaveCreator, at the same per-millisecond grid:
- For the RGB and blue styles it filters with record A's set (LP200², HP200 → LP2000², HP2000², LP400).
- For the 3-band style it uses record C's set (LP300; HP250 → LP1200; HP3000 → LP9000).
- `djplay::WaveDataZoom::getItem` @0x1006ab694 reads six 16-bit values per millisecond (record A's order: max, min, |LP400|, then the low, mid and high shares). The data's full scale (`getResolution`) is 32768.

**RGB drawing** (`djplay::WaveView::createWaveInfo` @0x1021916c0), per pixel column:
- The low share is the largest within ±12 ms and the mid share the largest of the previous, this and the next millisecond, as in PWV4 (§ 4.3). The high share is the millisecond's own.
- The colour is the PWV5 colour formula (§ 4.4, steps 1–5) on these three values; white when all are 0.
- The height is `max(|max|, |min|) / 32768` of the half height, linear, plus one pixel.

**3-band drawing** (`create3BandsWaveInfo` @0x102192b48) runs PWV7's decaying envelopes (§ 4.6: 0.99, 0.98, 0.97 per millisecond, restarted 300, 200 and 100 ms back) on the record C values. It scales low and mid by the band gains and draws high through `0.5 − 0.5·cos(π·v)`.

**Stored data.** `drawWaveCDJ` @0x10218ff6c and `draw3BandsWaveCDJ` @0x102190bf0 draw a `WaveDataCDJ` loaded from the stored tags (`WaveCDJ::load` @0x100277d5c). Which tracks rekordbox draws this way is open.

**Why the stored tags look sparse.** PWV5 heights are squared (§ 4.4) and PWV7 values are decaying band envelopes, with the high band through a cosine. Quiet material between kicks therefore drops to 0. Example: the intro of a 125 BPM track has hits at −15 to −25 dBFS between −8 dBFS kicks. They come out as 0 in PWV5 (5-bit heights) and 2–8 of 127 in PWV7.

**musicbox.**
- `mb_analysis::view::build_zoom_view` builds record A only (`WaveCreator::new_view`): about 1.4 s for a 7-minute AAC file in a release build, most of it decoding. It hands over each millisecond once its ±12 ms low window is summarised, block by block. The result equals building it in one go (`tests/synthetic.rs`).
- The app builds it on a worker thread from the start of the selected track on, draws each part as it arrives, cancels the build when the selection changes, and keeps the last eight.
- Its strip draws heights on a square-root scale, not rekordbox's linear one, so the quiet parts between kicks stay visible (a design choice, `docs/design.md`).
