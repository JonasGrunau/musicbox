# Track embedding (.3EX)

Status: **verified**. `mb_analysis::embed` with rekordbox's own ONNX Runtime and models reproduces all 14 `ANLZ0000.3EX` files of `fixtures/sandbox-initial` byte for byte, d2–d12 included. Through `ApplyAnalysis`, the demo track's file is identical too. Scorecard: `docs/scorecards/embedding.json`.

Code: `crates/mb-analysis/src/embed/` (`mod.rs`, `msgpack.rs`), `crates/mb-nn/src/ort.rs`, `crates/mb-library/src/analyse.rs`.

## 1. When it runs
- **Preference.** `TrackAnalysisSettingsEmbedding` (capital T; `SettingIF::getCurrentAnalysisSettingsEmbedding` @0x1015e6e04), default 1 (`_kTrackAnalysisSettingsEmbeddingValue` @0x1037bbeb6).
  - `execAnalyze` @0x1014900ec counts it among the reasons to analyse.
  - The job's flag (+0x15) becomes `CBeatAnalyzeThread`+0x272 (`startAnalyze` @0x100151d54). `analyze` @0x1001528c4 copies it to +0x281 and to `AnalyzeJobManager::SetEmbedding` @0x1022fb010 (manager +0xc).
- **Gates in `analyze`:**
  - Whole seconds `(L − 1) / rate` (integer division at the analysis rate, kept as u16, stored at 0x100153088) must be within 1…1800 (i64 pair at 0x103797f38).
  - Two track-provider checks must pass: vtable +0xa0 true, +0xd8 false. Their meaning is open; a local file passes.
  - With the vocal flag on, the vocal gate (0.4194 s < duration < 7200 s, `getOperatingSystemType() > 0x10c`) also clears the embedding.
- **Vocal detector.** With the vocal flag off, the thread looks up the track's stored PVDI (`CAnalyzerIF::getVocalDetectInfo`, 0x100154ce8–0x100154d90).
  - When there is none, it calls `SetVocal(true)`, so the detector runs for the embedding.
  - `notifyVocalDetect` is gated by the job's original vocal flag (thread +0x38e2), so PVDI is written only with `trackAnalysisSettingsVocalDetect`.
  - Evidence: sandbox-initial (vocal off) has no PVDI in any `.2EX`, yet its d12 values are non-zero, and our detector reproduces them.
- **Fixtures.** sandbox-initial (fresh install, embedding on) has a `.3EX` for all 14 tracks and `ContentLink` 0x2C060E. oracle-a and oracle-b were made with the embedding off: no `.3EX`, 0x1D060E.

## 2. ONNX Runtime and models (`crates/mb-nn/src/ort.rs`)
- **Library.** `dlopen` of `Contents/Frameworks/libonnxruntime.1.18.0.dylib` from the user's install, never bundled.
  - `OrtGetApiBase()->GetVersionString()` must be `1.18.0`, then `GetApi(18)`.
  - Function-table indices used: GetErrorMessage 2, CreateEnv 3, CreateSession 7, Run 9, CreateSessionOptions 10, SetSessionExecutionMode 13, DisableCpuMemArena 19, SetSessionGraphOptimizationLevel 23, SetIntraOpNumThreads 24, SetInterOpNumThreads 25, CreateTensorWithDataAsOrtValue 49, GetTensorMutableData 51, GetTensorElementType 60, GetDimensionsCount 61, GetDimensions 62, GetTensorTypeAndShape 65, CreateCpuMemoryInfo 69, ReleaseStatus 93, ReleaseSession 95, ReleaseValue 96, ReleaseTensorTypeAndShapeInfo 99, ReleaseSessionOptions 100.
- **Session setup** as `ONNXRunner` @0x1005a5ea0:
  - environment log level 3 (errors);
  - CPU memory arena off;
  - execution mode parallel when threads > 1, else sequential; intra-op = inter-op = threads;
  - graph optimisation level 99 (all);
  - no CoreML (every embedding runner is created with CoreML off).
- **Threads.** `getNumCpus() / 2`, at least 1, from `analyze` into `feature_extractor::embed`.
- **Models.** Under `Contents/Resources/models/`, MD5-checked at load, never copied:

| file | MD5 | inputs → outputs |
|---|---|---|
| `embed_model/cnn_embedding.onnx` | 8024c9bafacb11806213cbc718f92a7c | `signal` f32 [n, samples], `frame_step` i64[1], `frame_length` i64[1], `pads` i64[4] → `output` [n,50] (sigmoid tags), `features` [n,64]; STFT and Hann window inside |
| `embed_model/norm_before_umap.onnx` | 87f3ac3c0e0f2ca65da5673050f54e63 | `input_features` [?,64] → `normalized_features` |
| `embed_model/umap_embedder.onnx` | e806996a612d36614dd694b978afe83f | `input_1` [?,64] → `dense_6` [?,2] |
| `embed_model/norm_after_umap.onnx` | 67af0820c30de3a1f22492328942ea28 | `input_features` [?,2] → `normalized_features` |
| `embed_model/model_energy.onnx` | 97138d6f61b6b2de983634f48ddf05df | `input_features` [?,64] → `danceability` [?,1] |
| `embed_model/model_genre.onnx` | e5b6d44e7f3314958359c5649b66a69d | `input_features` [?,66] → `output_label` i64 [?] (+ `output_probability`) |
| `detect_cue_model/samplecnn.onnx` | 7f4e89752271601cd92cfbdb7818de65 | `waveform` [n,59049] → `tags` [n,50], `features` [n,512] (auto cue) |
| `detect_cue_model/cue_prediction_model_no_abs_position.onnx` | ba0f8a6f348d0c3a36315ce4f26329c7 | `input` f64 [?,?] → `variable` [?,1] (tree-ensemble regressor; auto cue) |

## 3. Audio (`load_audio` @0x10089b14c)
- `PcmReaderTemplate(path, 16000, …)`: the track at 16 kHz through rekordbox's sample-rate converter (`decode.md` § SRC; 44.1 → 16 kHz takes the default design, cut-off 0.45, β 12). `mb_audio::Decoder::open_at`.
- Samples `0 … L − 2` (`n = L − 1`, L = the 16 kHz length).
- Mono: `(R + L)·0.5` in f32 for two channels, channel 0 otherwise (float `getStreamAt` in 512-frame blocks).
- d4 = `n / 16000` (double).

## 4. Features (`feature_extractor::embed` @0x10089e85c)
1. `get_features` @0x10089b77c(audio, 464000, 512):
   - window 464000 rounded down to a multiple of 256 = 463872, hop half of that = 231936;
   - shorter audio is zero-padded to one window;
   - windows start at 0, hop, … while start + 463872 ≤ length (the tail is dropped).
2. Batches of 16 windows go through `cnn_embedding` (`cnn_run` @0x10089d2b4): `signal` [b, 463872], `frame_length` 512, `frame_step` 256, `pads` [0, 256, 0, 256] (i64 constant at 0x103797f70). `features` [b, 64] → d5, one row per window.
3. `avg_feats` @0x10089f5f4: the mean over rows 1…n−2 when there are 3 or more rows, else over all rows. Per column, the f32 values are summed in order into an f64, divided by the count and cast to f32 → d6.
4. `get_umap_2d_from_feats` @0x10089f784: `norm_before_umap` → `umap_embedder` → `norm_after_umap` → d7 (2 × f32).
5. `get_energy_from_feats` @0x10089f91c: `model_energy` on d6, clipped to 0…1 → d8.
6. `get_genre_from_feats` @0x10089f9c0: `model_genre` on [d6, d7] (66 values, d7 the normalised position) → the i64 label → d9.
7. d3 = MD5 of the audio file, lowercase hex.

## 5. The record (`analyze`, then `notifyEmbedding` @0x10015b398 → `notify_embedding` @0x10162c910)
| key | value |
|---|---|
| d2 | content ID as text (thread +0x258) |
| d3 | file MD5 (§ 4) |
| d4 | duration (§ 3) |
| d5 | window features (§ 4) |
| d6 | mean features |
| d7 | 2-D position |
| d8 | energy |
| d9 | genre label |
| d10 | key-job result, code 0–23 as in `analysis-key.md`, −1 on failure. With key analysis off: the library key's wheel position (1A = 0, 1B = 1, 2A = 2, … 12B = 23) mapped through the table at 0x103752aa0 to the same code (0x100154c98–0x100154cc8) |
| d11 | `BeatAnalyzeInfo` BPM × 100 (int, sp+0x6ac) × 0.01: the BPM the track holds after the pass |
| d12 | vocal share (§ 6) |

The placeholder d1 is removed. Key order in the file: d4 d5 d6 d7 d8 d9 d2 d3 d12 d11 d10.

## 6. Vocal share (`get_percentage_vocals` @0x1008a2308)
- **Levels.** The detector's levels (`VocalDetector::getResult`, lambda @0x10015a850) when the manager's vocal flag (+0xb) is on and +0x11 is clear (0x100156740). Otherwise the stored PVDI (lambda @0x10015a64c, `getVocalDetectInfo`).
- **Denominator.** The whole seconds of § 1, passed as int.
1. **Marks** at every frame i whose level (`& 7`) differs from the previous one (starting from 0), at `trunc(fma(i, 46439.90929705215, 0.5)) · 0.001` ms.
2. **`get_annotated_vocals`** @0x10089fc68(…, 6000):
   - the first mark is a start;
   - then, between levels a (previous) and b: a ≥ 1 and b ≥ 1 → continue; a = 0, b > 0 → start; a > 0, b = 0 → stop;
   - a second pass turns a stop followed by a start less than 6000 ms later into two continues.
3. **`get_segments`** @0x1008a04c4: a part runs from a start up to the next start. A part whose last mark is not a stop gets one at its last mark + 1000 ms with level 0. Per part: duration = last − first ms, max and sum of the levels, length (count).
4. **`filter_segments`** @0x1008a2140(…, 6000, 3): parts with max level ≥ 3 and duration > 6000 ms count.
5. **Share** = `min(Σ durations · 0.001 / seconds, 1)` (fminnm).
- Quirk: lengths are measured between marks, so a level that never changes makes a part of just the added second.

## 7. File format (`get_messagepack_from_json` @0x1008a7638)
- **JSON step.** The record is a `juce::var`, printed as JSON text with doubles at 15 decimal places. Trailing zeros are dropped and at least one decimal is kept (0 → `0.0`, stays a float). nlohmann `ordered_json` parses that text (insertion order) and `to_msgpack` packs it.
- **MessagePack:**
  - smallest forms: fixmap, fixstr, fixarray / array16 (0xdc), positive and negative fixint, then the 8/16/32-bit forms; big-endian;
  - a double is float32 (0xca) when a float holds it exactly, else float64 (0xcb). So most features come back one ulp off their f32 value and are stored as float64, e.g. 0x3fd29a857fffffff for the float 0x3e94d42c.
- **File.** `ANLZ0000.3EX` next to the `.DAT`, written with `replaceWithData`.

## 8. The library
- `AnalysisSettings::embedding` from `TrackAnalysisSettingsEmbedding` (default on), in the app and in `accept_check`.
- **`analyse_track`:**
  - runs the vocal detector along with the embedding, and writes PVDI only with the vocal preference;
  - returns `TrackEmbedding`: features, MD5, the detector's levels, whole seconds, and the key code (`None` with key analysis off).
- **`ApplyAnalysis`:**
  - completes the record: content ID; the `BPM` column the pass leaves; d12 from the PVDI the new `.2EX` carries (ours or kept), else from this pass's levels; d10 from the library key when key analysis is off;
  - writes the `.3EX` before the other files;
  - sets `ContentLink` 0x200000 (completion does not clear it, so an earlier bit stays);
  - inserts the `.3EX` `contentFile` row first (sandbox-initial's order: `.3EX`, `.2EX`, `.DAT`, `.EXT`; Hash = MD5 of the file);
  - undo restores or removes the file and the row.
- **Errors.** Without rekordbox's models, `TrackAnalysis::embedding_error` is set; the analysis completes and an earlier `.3EX` stays.

## 9. Verification
- `cargo run --release -p mb-analysis --example embed_score`: 14/14 identical values and files.
  - d4–d9 and d3: bit-exact.
  - d10 = our key: 14/14.
  - d12 = our detector: 14/14 (Demo Track 1 0.201419604651163, Demo Track 2 0.104489796875, the sampler loops 0).
  - d11 = the `BPM` column: 14/14. Our own grid differs from the library BPM on the 6 sampler loops, whose library grids are presets.
- `cargo test -p mb-library --test analyse_ops embedding_pass_writes_3ex`: byte-identical file, `contentFile` order, bit, undo.

## 10. Open
- The track-provider gates (vtable +0xa0, +0xd8) in § 1.
- Manager flag +0x11 (set by `SetNetAnalyze`) as the level-source condition in § 6. musicbox has no network analysis, so it is always clear.
- Whether the ONNX results depend on the thread count. All scored runs used CPUs/2, as rekordbox does.
- Only 44.1 kHz WAV and MP3 are scored. Other formats and rates through the 16 kHz converter are not.
- Tracks under 1 s or over 1800 s are not observed.
- What the genre labels and the energy value mean.
