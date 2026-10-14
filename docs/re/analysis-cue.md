# CUE analysis (Intelligent Cue Creation)

Status: **located, partly read; ground truth pending.** Manual mode is read far enough to implement once reference runs exist. The Auto pipeline is mapped but not read in detail.

## 1. Preferences (Preferences > Analysis > CUE Analysis)
Keys are `juce::String` objects built in `__GLOBAL__sub_I_SettingIF_key.cpp` @0x10206ccbc. Defaults are at 0x1037bbebe…0x1037bbed9.

| key | default | meaning |
|---|---|---|
| `DetectCue` | 0 | "Set CUE during analysis" |
| `AutoDetectCue` | 0 | 0 = Auto (intelligent), 1 = Manual (the label is inverted: non-zero selects `ManualCueCreation`) |
| `AutoDetectCueType` | 0 | 0 = HOT CUE, 1 = MEMORY CUE |
| `MaxHotCueNumber` / `MaxMemoryCueNumber` | 4 / 4 | "Upper limit"; the one for the selected type is used |
| `FirstCue` | 1 | "Set to 1.1Bars" |
| `LoopCue` | 0 | "Repeat CUE settings per setting value of bars from 1.1Bars" |
| `LoopCueCount` | 8 | the bars for the repeat |
| `OverwriteCue` | 0 | overwrite: earlier cues of the type may be removed (the UI asks first) |
| `WriteComment` | 1 | "Add a comment to CUE" |
| `allowAutoMemoryCue` | 0 | `_kTrackAnalysisAllowAutoMemoryCue` |
| `CueAnalysisPlaylist` | — | visibility of the [CUE Analysis Playlist] |

- **Gate.** `RbAppBackdoor::canAnalyzeIntelligentCue` @0x1005c1d50 = `DetectCue` AND `ActivationComponent::canUseCueAnalisysFunction` (the plan).
  - In `execAnalyze` @0x1014900ec, when `DetectCue` is on but the gate is false, the job is forced to Manual with `LoopCue` off (0x10149134c–0x101491360). That is the "only some functions of [Manual]" plan limit.
- **First launch.** `SettingIF::initIntelligentCue` @0x1015b8748 sets `AutoDetectCue` = 1 when the plan cannot use cue analysis or `allowAutoMemoryCue` is set, else 0. It also migrates `allowAutoMemoryCue` into `DetectCue`/`AutoDetectCueType`.
- **The job.** `DatabaseMediator::getDetectCueParam` @0x1014997a4 fills `AnalyzeDetectCue`. Byte offsets in the struct passed on:
  - 0 `DetectCue`, 1 `OverwriteCue`, 2 `WriteComment`;
  - 4 type ≠ 0;
  - 8 max count (memory or hot per type);
  - 12 `FirstCue`;
  - 16 Manual (`AutoDetectCue` ≠ 0);
  - 20 `LoopCue`;
  - 24 step in beats = `LoopCueCount · 4` in Manual, 1 otherwise.

## 2. When cues are made
- After the analysis, `analyze_complete` @0x101494740 calls `createDetectCue` @0x101495900 with the track, its `BeatAnalyzeInfo` and the `AnalyzeDetectCue`.
- **Inputs.** Manual takes `BeatAnalyzeInfo`+0x30 (the beat list); Auto takes +0x20 (the scored candidates). The beat list is also passed to `CueManager::SetBeatList` @0x1000586ac.
- **First beat.** When `MusicAnalyzeInfo`+0x50 = 0: `CueManager::AnalyzeFirstBeat` @0x100058308. Otherwise `SetFirstBeatPosition` @0x100058668 with it.
- **Existing cues.** They are loaded first (main DB vtable +0x438, `CueConverter::Convert`) into the CueManager's hot-cue (+0x78) and memory-cue (+0x68) arrays.
- **Branch.** Manual → `CueManager::ManualCueCreation` @0x100056e94. Auto → `CueManager::IntelligentCueCreation` @0x100057c7c with the candidate list.

## 3. Manual (`ManualCueCreation` @0x100056e94)
- **Status.** +0x9c: 0 ok, 1 nothing to do (neither FirstCue nor LoopCue), 2 no database listener, 3 no free slot.
1. **Start beat.** The first-beat index − 1 when set (+0x98 > 0). Otherwise the first entry of the beat list with beat-in-bar = 1.
2. **Count.** n = the upper limit, or 1 when `LoopCue` is off.
3. **Positions.** Without `FirstCue`, the first position is start + step (the 1.1-bar cue is skipped). Positions are taken every `step` = `LoopCueCount·4` beats until n positions or the end of the list.
4. **Overwrite.** With `OverwriteCue` (+0x39): `removeCues(type, min(found, n))` @0x1000573b8, then `refleshCues` @0x100056a00.
5. **Each position:** `CueManager::create` @0x1000579c8:
   - memory cues skip a position that already has one (`findSame` @0x100058994);
   - the slot comes from `findEmpty` @0x100058c34: memory → 0 while fewer than 10 exist, else none; hot → the first slot in the slot order (+0xa0, built in the constructor @0x100055f80 for 8 or 16 slots) that no existing hot cue uses, else none (status 3);
   - time = the beat's ms; frame = `trunc(ms · 0.15)`; an MP3 VBR seek position comes from the MPEG listener, FLAC from the FLAC listener;
   - the cue is written through the database listener (vtable +8) and added to the arrays.
6. **Comment** (`createComment` @0x100058db0, with `WriteComment`): "CUE(Auto)" in Auto. In Manual, "1.1Bars", plus "+%d" for the later positions (the number's meaning, bars or index, is open).

## 4. Auto (outline)
- **Predictor.** `execAnalyze` builds `analyzer::DetectCue` (ctor @0x10149e7a4, call at 0x101491a24) from:
  - the samplecnn path (`GetCnnModelFilePath` @0x1014930a0, `Contents/Resources/models/detect_cue_model/samplecnn.onnx`);
  - the cue-prediction model;
  - the personal-trend matrix (3 × 36 × 6) and position distribution (3) from `cue_personal_trend.json`;
  - a flag at +0x50 = Auto.
  - When the JSON file is missing, it writes an all-zero one first (`GetCuePersonalTrendFile` @0x101492b40 → `common::PropertyPath()/cue_personal_trend.json`, `replaceWithText`).
- **In the thread.** When the job's +0x50 flag is set, `CBeatAnalyzeThread::analyze` @0x1001528c4 runs:
  - `CuePointPredictor` (ctor via 0x100153c84 with the analysis rate; `initilaize_session` @0x101b49614);
  - `setData` @0x101b4b874 per decode block (0x100155530);
  - after the phrase pass, `getScores` @0x101b49f08 (beat times, `SongStructureAnalyzeResult`; `onnx_get_features` @0x101b4b454);
  - `ScoreCalculator::getTotalScores` @0x1019190dc, with `getPersonalTrendScoreMap` @0x101919a68, `minMaxNormalization` @0x101919d98 and `adjustScoreNearBeat` @0x101919f00. The result is the candidate list in `BeatAnalyzeInfo`+0x20.
- **Personal trend.** Built from the cues of the tracks in [CUE Analysis Playlist]: `DatabaseMediator::StartPersonalTrend` @0x1014978e4 → `CAnalyzerIF::AnalyzePersonalTrend` @0x1016332c8 → `PersonalTrendAnalyzer::analyzeTrack` @0x1019178bc, `updatePersonalTrend` @0x101918a70, `MinMaxNormalization` @0x1019186b8.
- Models: see the table in `analysis-embedding.md` § 2. Both run through ONNX Runtime 1.18 (`mb_nn::ort`).

## 5. Open
- All of § 3's output fields as stored: djmdCue columns, PCOB/PCO2, colours and the comment numbers. Reference runs are pending.
- The meaning of `OverwriteCue` versus the UI's "Prohibit overwriting CUEs".
- The hot-cue slot order (`HotCueOrder`).
- Auto in detail (§ 4).
