# Auto-gain loudness (`djmdMixerParam`)

Status: **verified**. `mb_analysis::analyze(…, Options { loudness: true, .. })` reproduces rekordbox 7.2.19's stored auto-gain bit for bit:
- the sample peak on 169/169 corpus tracks;
- the gain float on all 150 non-AAC tracks and on 12 of the 19 AAC tracks. The other seven are one float ulp off (≈ 5·10⁻⁷ dB), from the AAC decode (§ 5).

The results are the same against `fixtures/oracle-a` and `fixtures/oracle-b`. Scorecard: `docs/scorecards/loudness.json`.

Code: `crates/mb-analysis/src/loudness.rs`, plus `mb_dsp::libm::exp10`. Storage format: `ops/analyse-track.md`.

## 1. Where it runs
`analyzer::CBeatAnalyzeThread::analyze` @0x1001528c4. This step runs only when auto-gain is enabled (thread +0x27b, the `AutoGain` preference).

1. **Meter.** A `LoudnessAnalyzer` is created with `init(2, rate)` @0x1013566c4. This opens two libebur128 states for two channels at the analysis rate (44.1/48 kHz, after any SRC, `decode.md` § 2):
   - mode 5 (`EBUR128_MODE_I`, integrated loudness);
   - mode 0x11 (`EBUR128_MODE_SAMPLE_PEAK`).
2. **Input.** The same analysis blocks as everything else: `2 × rate` samples of 16-bit-truncated audio as `(float)short` (`analysis-waveforms.md` § 1). But **only every `s`-th block** goes to the meter:
   - `s = min(⌊(L − 1)/rate⌋ / 30 + 1, 16)`, from the block index starting at 0;
   - so a track under 30 s feeds every block, a 4-minute track every 9th, a track over 7.5 minutes every 16th.

   The fed blocks are joined into one stream; the filter state runs straight across the gaps.
3. **`addBuffer(l, r, n, 0x8000)` @0x101356720.** Interleaves the block as `sample / 32768` (exact) and passes it to `ebur128_add_frames_float` on both states.
4. **After the last block:**
   1. `calcLoudness` @0x10135691c calls `ebur128_loudness_global` (state 1).
   2. `calcSamplePeak` @0x101356974 takes the larger of the two channel peaks (state 2).
   3. `Loudness2AutoGainConverter::trackLoudness2AutoGaindB` @0x101356a30 returns `mStandardLoudnessLevel − LUFS`. The level @0x1053ab8e8 is the constant **−10.0** and is only ever read.
   4. The stored pair is `(float)__exp10(gain_dB · 0.05)` and `(float)peak`. If either call fails, the default pair @0x103752930 is stored instead.
5. **Storage.** Both floats go to `djmdMixerParam` as high and low 16-bit words: `GainHigh`/`GainLow` and `PeakHigh`/`PeakLow` (`db::AppSyncDBController::addAutoGainInfo` @0x100a6cfb0).

## 2. libebur128 (statically linked; `ebur128_*` @0x101ec0204–0x101ec160c)
The build is a 1.x libebur128 with the block-list/histogram option. Without `EBUR128_MODE_HISTOGRAM`, gating blocks are kept in a singly linked list, newest first.

**K-weighting** (`ebur128_init`). Computed in double from constants stored in the binary.

Shelf:
- `K = tan(5284.078578647628/rate)`, `t = K·1.414076664088621`
- `inv = 1/((t + 1) + K²)`, `Vb·K = K·1.7799278940417869`
- `pb0 = ((Vb·K + Vh) + K²)·inv`, `pb1 = ((K² + K²) − 2Vh)·inv`, `pb2 = ((Vh − Vb·K) + K²)·inv`, with `Vh = 1.5848647011308556`
- `pa1 = ((K² + K²) − 2)·inv`, `pa2 = ((1 − t) + K²)·inv`

High-pass:
- `K = tan(119.8061151453059/rate)`, `t = K·1.9986927057725021`
- `ra1 = ((K² + K²) − 2)/((t + 1) + K²)`, `ra2 = ((1 − t) + K²)/(…)`

Combined 4th-order filter:
- `b = [pb0, pb1 − (pb0 + pb0), (pb2 + pb0) − (pb1 + pb1), pb1 − (pb2 + pb2), pb2]`
- `a1 = ra1 + pa1`, `a2 = fma(ra1, pa1, ra2 + pa2)`, `a3 = fma(ra2, pa1, ra1·pa2)`, `a4 = ra2·pa2`

**Filter** (`ebur128_filter_float` @0x101ec0e90), per channel and sample:
- `v0 = x − fma(v4, a4, fma(v3, a3, fma(v2, a2, v1·a1)))`
- `y = fma(b4, v4, fma(b3, v3, fma(b2, v2, fma(b1, v1, b0·v0))))`

`y` goes into a 400 ms ring buffer. After each call, `v1 … v4` are flushed to 0 when `|v| < DBL_MIN`. In sample-peak mode, the channel's `max |x|` is taken first.

**Gating blocks** (`ebur128_add_frames_float` @0x101ec0c60, `ebur128_calc_gating_block` @0x101ec087c).
- The first block comes after 400 ms, then one every 100 ms (`(rate + 5)/10` samples).
- Each block takes the energy of the last 400 ms, per channel: `sum = fma(y, y, sum)` in ring order. When the window wraps, the part at the buffer start is summed first, then the tail.
- The channel sums are added left then right (weight 1), and the total is divided by the block length.
- Blocks below the absolute gate `10^((−70 + 0.691)/10)` (0x3e7f791ec6e1d5b7) are not stored.

**Integrated loudness** (`ebur128_gated_loudness` @0x101ec1268), walking the list newest first:
1. `sum = e + sum`, then `threshold = (0.1·sum)/count`.
2. A second walk adds `e` when `threshold ≤ e`, otherwise `−0.0`.
3. The result is `fma(ln(sum₂/count₂), 4.3429448190325175, −0.691)`.
4. If no block passes, the result is −∞.

## 3. Output
| Value | Formula | Column |
|---|---|---|
| gain | `(float)__exp10((−10 − LUFS)·0.05)` | `GainHigh = bits >> 16`, `GainLow = bits & 0xffff` |
| peak | `(float)max(peak_L, peak_R)`, as `max |sample| / 32768` (exact) | `PeakHigh`, `PeakLow` |

## 4. Verification (2026-10-12)
`cargo run --release -p mb-analysis --example loudness_score` (`MB_ORACLE=oracle-b` for the second fixture). Tests: `cargo test -p mb-analysis` (`tests/oracle.rs`: probes and sampler loops; `MB_ORACLE_FULL=1` for all tracks; `tests/synthetic.rs`: a −20 dBFS sine measures −20 LUFS).

| Fixture | Gain bit-exact | Peak bit-exact | Worst gain difference |
|---|---|---|---|
| oracle-a | 162/169 | 169/169 | 1 ulp (≈ 1·10⁻⁶ dB) |
| oracle-b | 162/169 | 169/169 | 1 ulp |

All seven gain differences are AAC tracks: two probes and five real tracks.

## 5. Open questions
- **AAC.** rekordbox stored identical values in both oracle sessions, so its AAC decode is stable. Ours misses by one float ulp on 7 of 19 AAC tracks, which means our AudioToolbox decode differs from rekordbox's by a few samples. The waveforms show the same thing in some runs (`analysis-waveforms.md` § 6). Candidates in `mb-audio`'s AAC path: the converter set-up (prime info, packet feeding) and the decode around the 5-frame restart. Loudness is the sensitive probe for comparing decoders.
- **Silence.** For a track whose blocks all fall below the absolute gate, the loudness is −∞ and the gain +∞. What rekordbox then stores (the default pair @0x103752930, or +∞) is untested; there is no fixture.
- **Track length.** The stride uses whole seconds of the analysed length `L − 1`. A track exactly at a multiple of 30 s switches stride at a one-sample boundary; no fixture sits there.
