# CUE analysis (Intelligent Cue Creation)

Status: **Manual verified on oracle-cue M1–M6** (9 runs, 169/169 tracks each, all compared `djmdCue` columns; scorecard `docs/scorecards/cue-manual.json`). **Auto mapped, not implemented**; its reference runs A1/A2 exist.

Code: `crates/mb-library/src/cue_analysis.rs`, step E of `ApplyAnalysis` in `crates/mb-library/src/analyse.rs`.

## 1. Preferences (Preferences > Analysis > CUE Analysis)
Keys are `juce::String` objects built in `__GLOBAL__sub_I_SettingIF_key.cpp` @0x10206ccbc. Defaults are at 0x1037bbebe…0x1037bbed9.

| key | default | meaning |
|---|---|---|
| `DetectCue` | 0 | "Set CUE during analysis" |
| `AutoDetectCue` | 0 | 0 = Auto (intelligent), ≠ 0 = Manual |
| `AutoDetectCueType` | 0 | 0 = HOT CUE, ≠ 0 = MEMORY CUE |
| `MaxHotCueNumber` / `MaxMemoryCueNumber` | 4 / 4 | "Upper limit"; the one for the selected type is used |
| `FirstCue` | 1 | "Set to 1.1Bars" |
| `LoopCue` | 0 | "Repeat CUE settings per setting value of bars from 1.1Bars" |
| `LoopCueCount` | 8 | the bars for the repeat |
| `OverwriteCue` | 0 | remove earlier cues of the type first (the UI asks before switching it) |
| `WriteComment` | 1 | "Add a comment to CUE" |
| `allowAutoMemoryCue` | 0 | old key, migrated by `initIntelligentCue` |

- **Gate.** `RbAppBackdoor::canAnalyzeIntelligentCue` @0x1005c1d50 = `DetectCue` AND `ActivationComponent::canUseCueAnalisysFunction` @0x101331094 (the cloud plan's Core product).
  - `execAnalyze` @0x1014900ec forces Manual without repeat when `DetectCue` is on but the gate is false (0x10149134c–0x101491360). That is the "only some functions of [Manual]" plan limit.
  - The analysis dialog shows a lock next to "CUE(Auto)" on the 7.2.19 trial, yet Auto runs there (A1/A2 have "CUE(Auto)" cues).
  - musicbox has no plan check and follows the stored values.
- **First launch.** `SettingIF::initIntelligentCue` @0x1015b8748 sets `AutoDetectCue` = 1 when the plan cannot use cue analysis or `allowAutoMemoryCue` is set, else 0.
- **The job.** `DatabaseMediator::getDetectCueParam` @0x1014997a4 fills `AnalyzeDetectCue`:
  - +0 `DetectCue`, +1 `OverwriteCue`, +2 `WriteComment`;
  - +4 type ≠ 0;
  - +8 limit for the type;
  - +12 `FirstCue`;
  - +16 Manual;
  - +20 `LoopCue`;
  - +24 step in beats = `LoopCueCount · 4` in Manual, 1 otherwise.
  - The CueManager copies this block to +0x38…+0x50.

## 2. When cues are made
- After the analysis, `analyze_complete` @0x101494740 calls `createDetectCue` @0x101495900(track, `BeatAnalyzeInfo`, `AnalyzeDetectCue`).
- **Inputs.** The beat list is `BeatAnalyzeInfo`+0x30, entries {u16 beat in bar, u16 tempo, u32 ms}. Without beats no cues are made. Auto also takes the scored candidates at +0x20.
- **First beat.** `MusicAnalyzeInfo`+0x50 comes from PQT2's `beat_num_offset` (`get_pmai_ext_beat_num_offset` @0x100f23980, via `CAnalyzerIF::load_info` @0x10161ef04).
  - 0, which is every fixture (486 files), → `AnalyzeFirstBeat` @0x100058308: the first downbeat's 1-based index from the first beat's beat-in-bar, 1/2/3/4 → 1/4/3/2 (table 0x10374e31c).
  - Otherwise `SetFirstBeatPosition` @0x100058668 with that value.
- **Bars.** `recalcBeat` @0x100058398 renumbers from the first downbeat f: bars before it count from −1, f is bar 1 (0 is skipped), beat-in-bar cycles 1–4 (`convFirstBar` 0x10374e320 = 1, 4, 3, 2).
- **Existing cues.** Loaded into the CueManager's hot (+0x78) and memory (+0x68) arrays (`Initialize` @0x1000567d0 sorts both).
- **Branch.** Manual → `ManualCueCreation` @0x100056e94; Auto → `IntelligentCueCreation` @0x100057c7c.
- **Writes.** Each cue goes through `DatabaseMediator::CreateCue` @0x101496840: `rekordboxDBController::setCue` (vtable +0x448), then `update_playlist_timestamp`. Each removal goes through `DatabaseMediator::RemoveCue` @0x101496a74. In mb-library these are the same paths as the `AddCue` and `DeleteCue` ops.
  - Rows (M1/M2): `usn` and `rb_local_usn` NULL, `ContentUUID` set; `contentCue` one row per track with `rb_local_usn`; `CueUpdated` + 1 per cue.
  - *Hypothesis:* after the completion statements. The timestamps cannot place them, and the instrumented copy (`tooling.md` § SQL trace) now exits right after its startup backup, so no trace exists for this.

## 3. Manual (`ManualCueCreation` @0x100056e94)
1. **Nothing** when neither `FirstCue` nor `LoopCue` is on, or the limit < 1.
2. **Start.** The first downbeat (index f − 1); without `FirstCue` it moves one step on. Count n = the limit with `LoopCue`, else 1. Positions are taken every `LoopCueCount · 4` beats until n are found or the grid ends.
3. **Overwrite** (`removeCues` @0x1000573b8, k = the number of positions found):
   - hot: the existing hot cues on pads below k (pad from `Kind` via the table at 0x10374e34c);
   - memory: the first k memory cues in the CueManager's order (*hypothesis:* time order; M5d, with one seeded memory cue per track, cannot tell).
4. **Each position** (`create` @0x1000579c8):
   - **memory:** skipped when a memory cue from before this pass lies at the same ms or within ±5 ms (`findSame` @0x100058994, unsigned bounds); none once the track has 10 memory cues (`findEmpty` @0x100058c34);
   - **hot:** the first free pad in the order A…P (`Kind` 1, 2, 3, 5, …, 17, constructor @0x100055f80); none when all 16 are taken;
   - the cue: InMsec = the beat's ms, InFrame = `(int)(ms · 0.15)`, no colour (255 / 0); the MP3 VBR columns from the MPEG listener. rekordbox writes none for these cues: `InMpegFrame` and `InMpegAbs` are 0 on every row, MP3s included, as in musicbox.
5. **Comment** (`createComment` @0x100058db0, with `WriteComment`): "1.1Bars" for bar 1, "1.1Bars+%d" with bar − 1 from bar 2 on (unsigned comparison). Auto writes "CUE(Auto)".

## 4. Auto (mapped, not implemented)
- **The job.** `execAnalyze` builds `analyzer::DetectCue` (ctor @0x10149e7a4, call 0x101491a24). It holds the samplecnn path (`GetCnnModelFilePath` @0x1014930a0), the cue-prediction model path, the personal-trend matrix (3 × 36 × 6) and position distribution (3) from `cue_personal_trend.json`, and the Auto flag at +0x50 (= `AutoDetectCue` == 0).
  - A missing JSON file is first written all zeros (`GetCuePersonalTrendFile` @0x101492b40 → `common::PropertyPath()/cue_personal_trend.json`).
- **`CuePointPredictor`** (ctor @0x101b48d08, sessions `initilaize_session` @0x101b49614):
  - Environment log level 2; session options: CPU arena off, graph optimisation level 2, default threads, no CoreML. The ctor also calls ONNX API table index 145 on the environment (open).
  - Tables: mood one-hot `{"1":[1,0,0],"2":[0,1,0],"3":[0,0,1]}` (+0x28) and phrase-kind one-hots "1"…"10" (+0x38).
- **`setData`** @0x101b4b874, per decode block (0x100155530): mono × 2⁻¹⁵, an IIR low-pass, then JUCE Lagrange resampling to 22050 Hz. This is the vocal detector's front end (`analysis-vocal.md` § 2), collected into a buffer.
- **`getScores`** @0x101b49f08(rate, beat positions, `SongStructureAnalyzeResult`), after the phrase pass (0x1001562c8):
  - nothing without a phrase result or with a mood outside 1…3;
  - the candidates are the phrase starts: their beat's sample position × 22050 / rate, with the phrase kind's one-hot;
  - the waveform around each candidate (±59049 samples) goes through samplecnn (`onnx_get_features` @0x101b4b454: `waveform` [n, 59049] → `features` [n, 512]);
  - the regressor `cue_prediction_model_no_abs_position` (input f64, output `variable`) scores each candidate. *Hypothesis:* its input is the CNN features, mood one-hot and phrase one-hot.
  - It prints timings to a `std::ostream` (`ScopedTimeMeasure` @0x101b4ccb4), not to a file. A1/A2 left no `csv` directory.
- **`ScoreCalculator::getTotalScores`** @0x1019190dc (0x1001563c8): combines the scores with the personal trend (`getPersonalTrendScoreMap` @0x101919a68), `minMaxNormalization` @0x101919d98 and `adjustScoreNearBeat` @0x101919f00, giving `BeatAnalyzeInfo`+0x20.
- **`IntelligentCueCreation`** @0x100057c7c places cues from that list (not read).
- **Personal trend.** Built from the cues of the tracks in [CUE Analysis Playlist]: `StartPersonalTrend` @0x1014978e4 → `CAnalyzerIF::AnalyzePersonalTrend` @0x1016332c8 → `PersonalTrendAnalyzer::analyzeTrack` @0x1019178bc, `updatePersonalTrend` @0x101918a70, `MinMaxNormalization` @0x1019186b8.
- **Reference runs** (all-zero personal trend, empty [CUE Analysis Playlist]): A1 hot cues, limit 8: 962 cues on 161 tracks (Kind 1/2/3/5/6/7/8/9 = 161/146/134/112/104/102/102/101), all "CUE(Auto)". A2 memory cues, limit 8: the same 962 positions as Kind 0.

## 5. Reference runs (`fixtures/oracle-cue/`)
Each run starts from `fixtures/oracle-a` (no cues; Normal mode, high precision off, key/phrase/vocal on), re-analyses all 169 tracks in rekordbox with the preferences in its `cue-settings.txt`, and keeps the library, the settings and `cue_personal_trend.json`.

| run | settings | rekordbox's cues |
|---|---|---|
| M1 | hot, limit 4, 1.1Bars, no repeat | 169 |
| M2 | hot, limit 8, repeat every 8 bars | 1032 |
| M3 | memory, limit 10, repeat every 16 bars | 1121 |
| M4 | hot, limit 4, 1.1Bars off, repeat every 4 bars | 629 |
| M5a/M5b | M2 on a seeded library, overwrite off/on | 1044 / 1037 |
| M5c/M5d | M3 on the seeded library, overwrite off/on | 1128 / 1128 |
| M6 | M2 with comments off | 1032 |
| A1/A2 | Auto, hot/memory, limit 8 | 962 / 962 |

- **Seed** (`crates/mb-library/examples/cue_seed.rs`, `seed/master.db` in each M5 run): hot cue A on 3 tracks, A and B on 2, a memory cue on the first downbeat on 3, a memory cue at 0:30 on 2.

## 6. Open
- The statement order of the cue writes against the completion statements (§ 2).
- Auto in full: the regressor's input layout, `getTotalScores`, `IntelligentCueCreation`, the personal trend ([CUE Analysis Playlist] run not made).
