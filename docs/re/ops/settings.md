# Op: change analysis settings

Status: **verified** (no DB writes) for the pass A settings change (21:53–21:59: track analysis mode Normal, BPM/key/phrase/vocal on, high-precision off, network analysis off).

## master.db
No INSERT, UPDATE or DELETE between the end of startup (L41) and the first track removal (L504). `localUpdateCount` stayed at 510. The analysis preferences are **not** stored in master.db.

## Other databases
`networkAnalyze6.db`, `networkRecommend.db`, `user.db3` and `product.db` saw only reads in the whole session.

## Where the settings go
Into rekordbox's settings files under `settings-root-path` (file writes, not traced). The keys seen in the pass B snapshot (`fixtures/oracle-b/analysis-settings.txt`): `trackAnalysisMode`, `enableKeyAnalysis`, `trackAnalysisProcessMode`, `AutoGain`, `HighPrecisionBeatAnalyze`, `trackAnalysisSettingsBpm`, `trackAnalysisBpmRange`, `trackAnalysisSettingsSongStruct`, `trackAnalysisSettingsVocalDetect`, `trackAnalysisSettingsNetworkAnalysis`. Which file and format holds them is *open* (outside this trace).

## Effect on later writes
The settings decide which ANLZ tags are produced and probably the content bits of `Analysed` / `ContentLink` (`analyse-track.md`; *open*).
