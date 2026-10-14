# Vocal detection (PVDI)

Status: **verified**. With `Options { vocal: true, .. }`, `mb_analysis::analyze` reproduces rekordbox 7.2.19's PVDI on 169/169 tracks of `fixtures/oracle-a` and 169/169 of `fixtures/oracle-b` (969,928 values each, headers identical). Scorecard: `docs/scorecards/vocal.json`.

Code: `crates/mb-analysis/src/vocal/` (`mod.rs`, `dsp.rs`, `features.rs`, `infer.rs`), with `crates/mb-nn` for the network and for reading the installed executable.

## 1. Where it runs
- **Preference.** `trackAnalysisSettingsVocalDetect` (`SettingIF::getCurrentAnalysisSettingsVocalDetect` @0x1015bda68). Default off (`_kTrackAnalysisSettingsVocalDetectValue` @0x1037bbcb0 = 0). `CBeatAnalyzeThread::analyze` @0x1001528c4 also clears the vocal flag below an OS-version check (`SystemStats::getOperatingSystemType() < 0x10d`).
- **The thread.** It creates `VocalDetector(rate)` (ctor @0x1005acb08) and feeds every decode block.
  - Mono = `(R + L)·0.5` of the 16-bit samples as floats for stereo, `L` otherwise. `CBeatAnalyzeThread::getStreamAt` @0x100158f08 turns the shorts into floats.
  - The block (2 s) goes to `setData(mono, n, rate)` @0x1005acda8. At the end the thread calls `applyEndProcess` @0x1005ad82c and `getResult` @0x1005adfe4 (the level array at +0x8838, count +0x8844).
  - From there `notifyVocalDetect` @0x10015b300 → `notify_vocal_detect` @0x10162c574 → PVDI.

## 2. Input stage (`dsp.rs`)
1. The samples are scaled by 2⁻¹⁵ (f32).
2. **Low-pass** JUCE `IIRCoefficients::makeLowPass(rate, 11025)` @0x10243b8dc, Q = 1/√2, in double:
   - `n = 1/tan(11025·π/rate)`, `c1 = 1/((n√2 + 1) + n²)`;
   - b = (c1, 2c1, c1), a1 = 2c1 − n²·2c1 (fused), a2 = c1·((1 − n√2) + n²);
   - stored as f32.

   `IIRFilterBase::processSamples` @0x10243b858 is transposed direct form II, fused as in the binary, with no denormal snapping.
3. **Resampling to 22.05 kHz** with JUCE's 5-point Lagrange interpolator (`LagrangeTraits::valueAtOffset` @0x10243b978, transcribed with its lane order and fusions).
   - Each block gives `trunc(n·22050/rate)` outputs. The step is `rate·(1/22050)`, the start position 1.0, with a 5-sample history ring.
   - Input is consumed as the position demands.

## 3. Frames (`mod.rs`)
- **Buffer.** 2048 samples plus a count.
- **Start padding.** The first block with at least 1024 outputs puts its first 1024 samples in front, mirrored (`x[1023 − i]`), and sets the count to 1024. So the padding is symmetric: the first sample is repeated.
- **Framing.** Frames of 2048 samples, hop 1024, over buffer ++ block output, while `start ≤ count + m − 2048`.
  - Window: BALib v1 `hanning(2048)` @0x10113f634. FFT: `fft_less_normalize` @0x10118678c (both in `key::fft`).
  - Power `fma(im, im, re·re)` in f32, stored as f64, 1025 bins.
- **Keeping samples.** `(count + m) mod 1024 + 1024` samples stay in the buffer.
  - Quirk kept: when the block is shorter than that (the last one), the old part is taken from `buf[m..]` rather than from its end, and zeros follow.
- **End.** `applyEndProcess` adds one more frame when the count exceeds 1024, mirrored past the end (index `2·count − 1 − v`).

## 4. Features (`features.rs`; rekordbox's own `analyzer::Librosa`, ctor @0x100779958, n_fft 2048 at 22050 Hz)
- **Mel spectrogram.** `make_mel_filter` @0x100778a18 builds 128 Slaney bands, 0 Hz to 11025 Hz, Slaney normalisation. The compiler-folded constants are used as their exact bit patterns, the mel→Hz exponential is fused, and enorm is `2·(1/(f[i+2] − f[i]))`.
  - `get_melspec` @0x100779a28: dot products in 4 × 2-lane fused accumulators, `((a1 + a0) + a2) + a3`, the lanes added, the last bin fused.
- **dB.** `power_to_db` @0x100779c30 over the whole track: `10·(log10(max(S, 1e-10)) − log10(max S))`, floored 80 dB below the maximum.
- **MFCC.** `make_mfcc_cos_table` @0x100779120 holds 18 DCT-II rows, `cos((2j+1)·kπ/256)`. `get_mfcc` @0x10077a014: c0 = Σ·(1/√128) in the unrolled code's summation order; c1–c17 = dot·0.125.
- **Deltas.** `get_mfcc_delta` @0x10077a308, width 9: `fma(d4, 4, fma(d3, 3, d1 + (d2 + d2)))·(1/60)`. The first and last four frames copy their neighbour. Fewer than 9 frames give no result.
- **Fluctogram.** `get_fluctogram` @0x10077a9cc:
  1. Magnitudes are interpolated linearly (`stft_interp` @0x10077b3a0) onto 540 log bins, `233.08 Hz·2^(i/120)`.
  2. 11 bands of 240 bins, every 30 bins.
  3. **Tonal test** per frame and band: the Chebyshev-weighted energy share (`chebwin` @0x10077b558, a 240-point table) must exceed 0.1, and the spectral flatness (exp-mean-log over mean, ε = 2⁻⁵²) must stay under 0.6.
  4. The band spectrum is then multiplied by a triangular window.
  5. For a tonal frame, `calc_fluctogram` @0x10077baa0 gives the shift −5…5 of the best normalised cross-correlation with the next frame's band. The first maximum wins, and NaN (silent bands) runs on to +5. Non-tonal frames give 0.
  6. The state (last spectrum and flag per band) carries across blocks. The last frame has no value.
- **Feature row.** 18 MFCC deltas plus 11 fluctogram values (the MFCCs themselves are not used).
- **Normalisation.** `normalize_feature` @0x10077c4ec: `fma(x, a_f, b_f)` per feature, with the learned statistics from § 6.

## 5. Network (`infer.rs`, `Discriminator::inference` @0x100b812d0)
- **Windows.** `⌈frames/180⌉` windows of 360 frames, hop 180 (`fcvtps`, so the last window is short).
  - `InputBatch`: f32 [windows, 29, 360], feature-major, zero padded.
  - `LengthBatch`: i32 [windows, 1].
  - Output `Softmax`: [windows, 360, 2]; class 1 is taken.
  - Session config: intra-op 0, inter-op 1.
- **Averaging.** The per-frame probability is the f32 sum over the overlapping windows, divided by their count.
- **Level** (`applyEndProcess`): `> 0.2` → 1, `> 0.4` → 2, `> 0.6` → 3, `> 0.8` → 4, compared in f32. NaN → 0.
- **Model file.** rekordbox loads `Contents/Resources/vd_param.pb` (MD5 13217b009062598a322f3427bb7cd21b), an encrypted file that `tf_utils::LoadGraph` @0x1013a3d08 decrypts.
  - The app also ships `vd_param_raw.pb` (MD5 93bff1a226507e1cffea8020921a5ab8), a plain GraphDef of the same network. Our outputs with it are bit-identical to rekordbox's.
  - musicbox uses only the plain file and never decrypts anything.

## 6. Values read from the installed rekordbox at run time
The learned feature statistics and the Chebyshev window come from the user's installed executable (`Contents/MacOS/rekordbox`, arm64 slice). The VM address is mapped through the segment load commands and the bytes are MD5-checked; a mismatch refuses the vocal pass (`mb_nn::executable_bytes`, `MB_REKORDBOX_APP` override). No value is in the repository.

| Region | VM address | Bytes | MD5 | Content |
|---|---|---|---|---|
| normalisation table | 0x10378f6f0 | 448 | 89bc4e6967c1ddd0cfe059636108509c | scale/offset of features 0–27, per pair: scale[2m], scale[2m+1], offset[2m], offset[2m+1] |
| feature 28 | 0x10077c5f0 | 40 | 9b0a4ca8931d565c6c7a6fa54fbfeff1 | `normalize_feature` code; movz/movk into x10 (scale, first 4 instructions) and x11 (offset, last 4) |
| Chebyshev window | 0x10378ef70 | 1920 | 8f667e8b7ba347db966dd043c3b57eba | 240 doubles copied by `chebwin` |

When the app, model or regions are unavailable, the vocal pass is skipped. `Analysis::vocal_error` and `TrackAnalysis::vocal_error` say why, and the analysis still completes.

## 7. PVDI and the library
- **PVDI** (`.2EX`, `anlz.md`): version 0, 1024, 22050, entry size 1, one level per frame. Local plain; USB scrambled.
- **`mb-library`:**
  - `AnalysisSettings::vocal` comes from the preference.
  - `analyse_track` appends PVDI after PWVC.
  - `ApplyAnalysis` sets `ContentLink` 0x100000 when our PVDI is written.
  - When the pass did not run, it keeps an earlier PVDI and its bit.

## 8. Verification
- **Harness.** rekordbox's own `VocalDetector`, run natively outside the repo on our decode, equals the stored PVDI on every track tried. Against it, our tables, power frames, mel, fluctogram, features, normalised features and probabilities are bit-identical.
- **Scores.** `cargo run --release -p mb-analysis --example vocal_score -- score` (`MB_ORACLE=oracle-b` for the second fixture).

## 9. Open
- The OS-version gate's exact meaning (§ 1).
- Tracks shorter than 9 frames: no deltas, so no PVDI. rekordbox's handling is not observed.
