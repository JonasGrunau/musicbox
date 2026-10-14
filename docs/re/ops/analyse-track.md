# Op: track analysis (request, start, completion)

Status: **verified** for Normal mode with BPM, key, phrase and vocal analysis on (two full passes over 161 tracks: the automatic pass after import, L3694–~17300, and a manual "Analyse track" pass, L17355–29590). Building blocks: `common.md` (UPDATE-COL, DELETE-ROWS, agent `contentFile`). **Files (2026-10-12): verified** — musicbox's `mb_library::analyse_track` rebuilds rekordbox's local `.DAT`/`.EXT`/`.2EX` of all 161 oracle-a tracks section by section, PSSI included, apart from the 96 kHz probes (the click probe's grid, its and the tempo ramp's PSSI; `analysis-phrase.md` § 10) (§ Files). PVDI (`analysis-vocal.md`) and the `.3EX` (`analysis-embedding.md`) are verified separately.

All analysis writes run in **autocommit** mode, one statement at a time, on several worker threads in parallel. There is no transaction around a track.

## Phases and triggers
| Phase | Trigger | Thread |
|---|---|---|
| A. request | user runs "Analyse track" on a selection (manual pass only) | UI |
| B. start | the analysis queue hands the track to a worker (about 7 run in parallel) | queue thread |
| C. completion | the analyser has written the ANLZ files | worker |
| D. file registration | rekordboxAgent picks up the new files (untraced) | agent process |

The automatic pass after import has no phase A.

## A. Request (manual re-analysis)
For each selected track, in selection order:
```
UPDATE-COL Analysed    = <Analysed with bit 0x01 cleared>      -- 105 → 104
UPDATE-COL ContentLink = <ContentLink | 0x50>                  -- 1902094 → 1902174
```

## B. Start
```
[only if the file has embedded artwork and this is the first analysis]
select UUID from djmdContent where rb_local_deleted = 0 and ID = <CID>
UPDATE-COL ImagePath = "/PIONEER/Artwork/<u[0:3]>/<u[3:]>/artwork.jpg"         -- u = content UUID
select ID from djmdContent where rb_local_deleted = 0 and UUID ="<u>"
        → the agent registers artwork.jpg in contentFile (1 USN; common.md)
[always]
UPDATE-COL Analysed    = <see table>        -- first pass 0 → 74; manual pass 104 → 106
UPDATE-COL ContentLink = <ContentLink | 0x50> -- first pass 14 → 94; manual pass rewrites 1902174
networkAnalyze6.db:  select * from manage_tbl where SongID = <CID>              -- read only
networkRecommend.db: select * from manage_tbl where SongID = <CID>              -- read only
```
- Artwork files written (not traced) under `<analysis root>/PIONEER/Artwork/<u[0:3]>/<u[3:]>/`: `artwork.jpg` (500×500 in the sample), `artwork_m.jpg` (240×240), `artwork_s.jpg` (80×80). 95 of 161 tracks had artwork; the manual pass wrote no `ImagePath`.

## C. Completion
```
select … from djmdCue where rb_local_deleted = 0 and ContentID = <CID>          -- cue read(s) during analysis
select distinct PlaylistID from djmdSongPlaylist where rb_local_deleted = 0 and ContentID in (<CID>)
select * from djmdPlaylist where rb_local_deleted = 0 and ID = <P>              -- per containing playlist (masterPlaylists6 Timestamp, playlist-create.md §5)
UPDATE-COL AnalysisDataPath = "/PIONEER/USBANLZ/<u[0:3]>/<u[3:]>/ANLZ0000.DAT"   -- rewritten on every pass, same value
COUNTER    AnalysisUpdated  += 1
UPDATE-COL BPM        = <bpm × 100, integer>
UPDATE-COL SampleRate = <Hz>
UPDATE-COL BitRate    = <kbit/s>
UPDATE-COL BitDepth   = <bits>
UPDATE-COL Length     = <seconds>
COUNTER    TrackInfoUpdated += 1
select * from djmdMixerParam where rb_local_deleted = 0 and ContentID = <CID>
  [no row] [USN bump B]
           insert into djmdMixerParam (ID, ContentID, GainHigh, GainLow, PeakHigh, PeakLow, UUID, rb_data_status, rb_local_deleted, rb_local_usn, rb_local_synced, created_at, updated_at)
             values ("<UUIDv4>", "<CID>", <gainHi>, <gainLo>, <peakHi>, <peakLo>, "<UUIDv4>", 0, 0, <U>, 0, "<TS>", "<TS>")
  [row exists] nothing (manual pass)
select * from djmdKey where rb_local_deleted = 0 and ScaleName = "<key>"
  [no row] SELECT * FROM djmdKey WHERE ID = '<K>' AND rb_local_deleted = 0      -- ORM collision check
           [USN bump O]
           INSERT INTO djmdKey (ID,ScaleName,UUID,created_at,rb_data_status,rb_local_deleted,rb_local_synced,rb_local_usn,updated_at)
             VALUES ('<K>','<key>','<UUIDv4>','<TS>',0,0,0,<U>,'<TS>')
UPDATE-COL KeyID = "<K>"                                                         -- written on every pass, even if unchanged
select ID from djmdKey where rb_local_deleted = 0 and ID not in (select KeyID from djmdContent where rb_local_deleted = 0 and KeyID notnull)
  [orphans] DELETE-ROWS(djmdKey, ID, <orphan IDs quoted>)                        -- e.g. the tag key the analysis replaced
select ID from djmdKey where rb_local_deleted = 0 and ScaleName = "<key>"
COUNTER    TrackInfoUpdated += 1
[first pass only] UPDATE-COL ContentLink = <ContentLink | 0x10000>              -- 94 → 65630
COUNTER    AnalysisUpdated  += 1
COUNTER    AnalysisUpdated  += 1
UPDATE-COL Analysed    = 105
UPDATE-COL ContentLink = 1902094
select ID from djmdContent where rb_local_deleted = 0 and AnalysisDataPath like '%/PIONEER/USBANLZ/<u[0:3]>/<u[3:]>/ANLZ0000%'   (×3)
```
`COUNTER x += 1` = `select CueUpdated, AnalysisUpdated, TrackInfoUpdated from djmdContent where rb_local_deleted = 0 and ID = <CID>`, then UPDATE-COL with the decimal string old+1 (NULL = 0).

## D. File registration (agent, untraced)
About 100 ms after completion rekordboxAgent inserts the `contentFile` rows for `.2EX`, `.DAT`, `.EXT` (in that order), one USN each (`common.md`). The main app's next bumps skip those values. The manual pass produced identical files and the agent wrote nothing.

## Values
| Column | Value | Notes |
|---|---|---|
| `AnalysisDataPath` | `/PIONEER/USBANLZ/<u[0:3]>/<u[3:]>/ANLZ0000.DAT`, relative to `analysis-data-root-path` | 322/322 |
| `BPM` | BPM × 100, integer | |
| `SampleRate` | Hz (22050, 44100, 48000, 96000 seen) | |
| `BitRate` | PCM (WAV/AIFF): `SampleRate × BitDepth × channels / 1000`, truncated (1411, 2304, 6144, 352 for 22.05 kHz mono). MP3: kbit/s; CBR files get their nominal rate (128, 320), VBR files get a per-file value (seen: 32, 160, 224, 256, 320; probably the first frame's rate, *open*). FLAC, ALAC and AAC (`.m4a`): **0** | |
| `BitDepth` | 16, 24, 32 (32-bit float WAV); 16 for lossy formats | |
| `Length` | whole seconds from the decoder; 44/161 are 1 s lower than the import value | |
| `KeyID` | ID of the `djmdKey` row whose `ScaleName` is the detected key | |
| `ScaleName` (analysis) | rekordbox notation: `C Db D Eb E F F# G Ab A Bb B`, minor with `m` (`Abm`, `F#m`, `Dbm`, `Ebm`, `Bbm`) | tag keys stay verbatim (`Dmin`, `G#maj`) |
| `djmdMixerParam.GainHigh/GainLow` | the auto-gain factor as an IEEE-754 float32, split into its high and low 16-bit words | e.g. 16249/20046 → 0x3F794E4E = 0.974 |
| `djmdMixerParam.PeakHigh/PeakLow` | the peak level, same float32 split | 16128/0 → 0.5 (a −6 dBFS probe); 16256/0 → 1.0 |
| `Analysed` | state, see below | |
| `ContentLink` | flag word, see below | |
| `AnalysisUpdated` | +3 per pass (`"1"…"3"`, then `"4"…"6"`) | text |
| `TrackInfoUpdated` | +2 per pass (`"1","2"`, then `"3","4"`) | text |
| `CueUpdated` | not touched (stays NULL) | |

### `Analysed` states (observed)
| Value | Hex | When |
|---|---|---|
| 0 | 0x00 | after import |
| 104 | 0x68 | re-analysis requested (done bit cleared) |
| 74 | 0x4A | first start: 0x40 \| 0x08 \| 0x02 |
| 106 | 0x6A | later start: 0x40 \| 0x20 \| 0x08 \| 0x02 |
| 105 | 0x69 | done: 0x40 \| 0x20 \| 0x08 \| 0x01 |

So bit 0x01 = analysed, 0x02 = queued/in progress, and 0x08, 0x20, 0x40 describe what was analysed (0x20 appears only once an analysis has completed). The meaning of the individual content bits is *open*. The USB export stores `Analysed & 0x3F` on the device (105 → 41, `export-devlib.md`).

### `ContentLink` transitions (observed)
| Value | Hex | When |
|---|---|---|
| 14 | 0x0E | import |
| 94 | 0x5E | queued (\| 0x50) |
| 65630 | 0x1005E | first completion, after key/track info (\| 0x10000) |
| 1902094 | 0x1D060E | done: 0x50 cleared, 0x600 and 0x1C0000 set |
| 1902174 | 0x1D065E | re-analysis requested (\| 0x50) |

Three of the eight demo tracks that an untraced session re-added at 22:23 end at 1836558 (0x1C060E, no 0x10000); the other five at 1902094. Bit meanings are *open*. The device copy is `(ContentLink & 0xC1600) | 0x100` = 0xC0700 (verified against the trace, `export-devlib.md`).

## Counters per track
- First pass, main app: 2 (start) + 16 (completion) + 1 if artwork + 1 per created key + 1 per orphan-key cleanup. Agent: +3 (ANLZ files) + 1 if artwork.
- Manual pass: 2 (request) + 2 (start) + 14 (completion; no mixer insert, no 0x10000 step).
- The agent's increments interleave with the main app's (gaps of 1–2 per track during the first pass; none in the manual pass).

## Notes for musicbox
- To be indistinguishable from rekordbox after an analysis, write: the ANLZ files, `AnalysisDataPath`, BPM/SampleRate/BitRate/BitDepth/Length, `KeyID` (creating the `djmdKey` row if needed, removing orphans), a `djmdMixerParam` row if missing, the counters, `Analysed = 105`, `ContentLink = 1902094`, plus the `contentFile` rows (rekordbox leaves those to the agent, which may not be running).
- `networkAnalyze6.db` was only read (`manage_tbl` stayed empty). The file's change counter moved during the untraced 22:23 session, not during this one.
- *Open:* behaviour when no key is detected, when only some analysis types are enabled, Dynamic mode, and whether an existing `djmdMixerParam` row is updated when the gain changes.

## Files (verified 2026-10-12 on all 161 rekordbox-analysed oracle-a tracks)
`CMusicAnalyzeFile::write_pmai` @0x100f5ab40, `write_pmai_ext` @0x1013d0230 and `write_pmai_ex2` @0x1007869f0 write the sections in a fixed order. The `.DAT` sections are unconditional. An `.EXT`/`.2EX` section is written only when its data exists (a null pointer in the info struct is skipped), so a missing analysis just means a missing section.

| File | Sections | Notes |
|---|---|---|
| `.DAT` | PPTH PVBR PQTZ PWAV PWV2 PCOB PCOB | PCOB hot (kind 1) first, then memory (kind 0), both empty: collection cues live in `djmdCue` only |
| `.EXT` | PPTH PWV3 PCOB PCOB PCO2 PCO2 PQT2 PWV5 PWV4 [PVB2] [PSSI] | PVB2 for FLAC only; PSSI with phrase analysis |
| `.2EX` | PPTH PWV7 PWV6 PWVC [PVDI] | PVDI with vocal analysis |

- **PMAI header:** 16 bytes `00000001 00010000 00010000 00000000` in all three files.
- **PPTH:** `?/<FileNameL>`.
- **PVBR:** always 1604 bytes (400-entry index + sample count). MP3: count = decoded samples; the index is filled only when the first frame carries a `Xing` header (37/37 MP3s). Other formats: zeros.
- **PVB2:** the FLAC seek table, `decode.md` § 7; `mb_audio::flac_seek` reproduces it on 23/23 FLACs.
- **Waveforms:** PWAV/PWV2 headers `len, 1, 0`; PWV3/PWV7 150 columns/s; PWV5 150 columns/s, colour bits 3, height bits 5; PWV4 1200 × 6; PWV6 1200 × 3.
- **Grids:** PQTZ from the beat grid; PQT2 from the same beats plus their µs remainders (`analysis-beats.md`).
- **Columns:**
  - `Length` = whole seconds of the analysed samples `0 .. L − 1` at the analysis rate, so an exact 64 s file stores 63.
  - `BitRate` for MP3 = the first frame's rate (VBR files included).
  - All five columns match on 161/161.

## Status bits (from the binary)
- **`modAnalyzeStatus` @0x1017e37fc** takes one status word:
  - `Analysed` = status & 0x7b, plus 0x80 when the lock flag is passed;
  - `ContentLink` = (old & ~0x8670, with 0xE kept) | ((status >> 8) & 0x3c8670).
- **Analysis lock = `Analysed` bit 0x80.** `analyze_complete` @0x101494740 leaves a track alone when that byte is negative. The lock is set by `DatabaseMediator::modifyAnalysisLock` @0x100c4903c. There is no lock column.
- **Completion** (`analyze_complete`): status `& 0xf379ffce | 0xc060021`, so `Analysed` |= 0x21 (final 105) and `ContentLink` |= 0xC0600.
- **Optional `ContentLink` bits:**
  - **0x10000 = phrase:** `setAnalyzedSongStructToContentLink` (vtable +0x4b0) sets it when the song-structure result exists.
  - **0x100000 = vocal:** status 0x10000000, from the BeatAnalyzeInfo vocal flag.
  - **0x200000 = embeddings:** status 0x20000000, when the network/AI embedding data is an object.

  Each is followed by `updateAnalysisUpdated`, which explains the trace's two extra `AnalysisUpdated` bumps after the key block.
- **Fixtures agree:** `sandbox-initial` (phrase/vocal off, network analysis on) ends at 0x2C060E; oracle-a/oracle-b (phrase/vocal on) at 0x1D060E.

## musicbox (`mb_library::analyse`, verified 2026-10-12)
- **`analyse_track` (pure, no database).** Decodes with `mb-audio` and runs `mb-analysis` (8 waveform tags, auto-gain, beat grid, phrase shift and PSSI, key). It builds the three files in the order above, plus the `djmdContent` columns, the `djmdMixerParam` words and the key's `ScaleName`. `analyse_track_with` also takes the track's stored grid (§ High precision); `stored_grid(dat, bpm)` reads it from the current `.DAT`.
  - Test: `crates/mb-library/tests/analyse_ops.rs` (`MB_ORACLE_FULL=1` for all tracks), with phrase analysis on as for oracle-a. All 161 tracks match apart from PVDI; the 96 kHz click probe's grid is placed differently by the phrase pass, and the PSSI of the two 96 kHz probes differs. Mixer rows, columns and keys are equal.
  - `phrase_pass_writes_pssi`: a re-analysis with phrase on writes our PSSI and `ContentLink` 0x10000. `high_precision_without_beats_uses_stored_grid`: oracle-b's sweep gets its key and PSSI from the stored grid.
- **`Command::ApplyAnalysis`.** Replays the trace in one transaction:
  1. The request (re-analysis only).
  2. The start: `Analysed` 74/106, `ContentLink` | 0x50.
  3. The files.
  4. The completion: `AnalysisDataPath`, `AnalysisUpdated`, BPM/SampleRate/BitRate/BitDepth/Length, `TrackInfoUpdated`, a `djmdMixerParam` insert when missing, then the key block (`djmdKey` row by `ScaleName`, inserted when missing; `KeyID`; orphaned key rows removed; `TrackInfoUpdated`), `Analysed` 105, the final `ContentLink`.
  5. The playlist timestamps.
  6. The agent's three `contentFile` rows (MD5, size; only new or changed files).

  It refuses a locked track. The previous files go to the session backup.
- **Phrase.** With `trackAnalysisSettingsSongStruct` on, the `.EXT` gets our PSSI and `ContentLink` 0x10000 when the pass yields a result; an earlier PSSI is not carried over (without a result: no section, bit cleared, *hypothesis*). With it off, an earlier PSSI and its bit are kept. The phrase pass's two `AnalysisUpdated` bumps after the key block are skipped.
- **Vocal** (`trackAnalysisSettingsVocalDetect`, default 0): with the setting on, `analyse_track` writes our PVDI after PWVC (`analysis-vocal.md`) and `ApplyAnalysis` sets `ContentLink` 0x100000. When the setting is off, or the pass cannot run because rekordbox's install or model is missing (`TrackAnalysis::vocal_error`), PVDI is carried over from the previous `.2EX` when the track had it, and its `ContentLink` bit is kept only then.
- **Embedding** (`TrackAnalysisSettingsEmbedding`, default 1): `analyse_track` computes the track embedding (`analysis-embedding.md`). With it, the vocal detector runs even when the vocal preference is off and the track has no PVDI; its levels then feed only the vocal share, and no PVDI is written. `ApplyAnalysis` writes `ANLZ0000.3EX` before the other files, inserts its `contentFile` row first (`.3EX`, `.2EX`, `.DAT`, `.EXT`) and sets `ContentLink` 0x200000. Outside 1…1800 whole seconds, or without rekordbox's models (`TrackAnalysis::embedding_error`), the pass is skipped and an earlier `.3EX` stays.
- **Inverse `Command::RestoreAnalysis`.** Puts back the previous files (or removes them), the columns and the `contentFile` hash/size. It deletes the inserted mixer and `contentFile` rows, and bumps `AnalysisUpdated` once (counters only move forward). The previous `KeyID` comes back; when the analysis's orphan cleanup removed its row, a row with the old `ScaleName` is found or created (as an undone key edit does), the analysis's key row goes if nothing uses it, and `TrackInfoUpdated` is bumped.
- **High precision.** `HighPrecisionBeatAnalyze` = 1 (default 0) with `trackHiPrecisionBeatAnalysisMode` (default 2) selects the high-precision grid (`analysis-beats-hp.md`); all three modes are ported (Dynamic, Normal and Auto, with the BPM-range rebuild in Normal/Auto). PQT2 `analysis_type` comes from `notifyBeat`: 2 for analysis modes 1 and 4, 1 otherwise. When the high-precision pass finds no beats, rekordbox writes no grid and the previous PQTZ/PQT2 stay, while `BPM` becomes 0 (oracle-b's 12 sweeps: the old 51-beat type-1 grid, `BPM` 0); `ApplyAnalysis` carries the sections over from the previous files, like PVDI, and writes `BPM` 0. Key and phrase then run on the stored grid with its BPM (`getBeatListFromDatabase`, `CBeatAnalyzeThread::analyze` @0x1001528c4). `analyse_track_with` takes that grid from the caller (`stored_grid`: the PQTZ of the current `.DAT` and the `BPM` column, beat 0's tempo where the column is 0, *hypothesis*; both agree on oracle-b). The app's analysis jobs and `accept_check` pass it. With it, the 12 sweeps get oracle-b's key and PSSI.
- **Analysis settings.** Normal mode (BALib v1), Normal-mode Dynamic (`trackAnalysisMode` 1, the varied analyser, `analysis-beats.md` § 9) or high precision (Dynamic, Normal or Auto). The BPM range is the 70–180 preset. `AutoGain` from `rekordbox3.settings` decides the mixer row; `enableKeyAnalysis` (on when missing, *hypothesis*) the key block; `trackAnalysisSettingsSongStruct` the phrase pass.
- **CUE analysis** (`DetectCue` with Manual, `AutoDetectCue` ≠ 0; default off): after the completion, `ApplyAnalysis` removes and adds cues through the cue ops, as rekordbox's `CreateCue`/`RemoveCue` (`analysis-cue.md`); undo removes them and restores removed ones. Auto (`AutoDetectCue` = 0) takes the scored candidates from `mb_analysis::cue` (§ 4 of the spec); without rekordbox's models (`TrackAnalysis::cue_error`) it makes no cues.

## Artwork (first analysis)
- **When** (`DatabaseMediator::execAnalyze` @0x1014900ec): once the job's status exists, and unless the job flags (& 0x48) say otherwise, a file with an embedded picture (`TagLib::ParseTag::hasArtwork` @0x100e51890) gets `add_track_artwork` @0x100c45e4c. With flag 0x08 alone and an `ImagePath` already set, only the `_s`/`_m` files are made again from `artwork.jpg` (`CAnalyzerIF::writeFileImage`). *musicbox:* artwork is added when the track has no `ImagePath`; otherwise it is left alone. In the import trace (`sqltrace-37549`) every first analysis of a file with a picture sets `ImagePath`, a manual re-analysis does not.
- **Order** (trace): `ImagePath` is written **before** the analysis starts (before `Analysed` = 74); the agent then adds the `contentFile` row of `artwork.jpg` (only that file).
- **Path:** `/PIONEER/Artwork/<uuid[0:3]>/<uuid[3:]>/artwork.jpg` under the analysis root, the content UUID's folder as for the ANLZ files. Local tracks get `artwork.jpg`, `artwork_s.jpg`, `artwork_m.jpg` (no 320 px file; `writeTagImage` @0x10149d90c with the large flag off).
- **Picture** (`TagLib::ParseTag::parseCoverArt` @0x100e507e0): MP4 `covr`; FLAC the first JPEG or PNG picture of type 3 (front cover); ID3v2 (MP3, AIFF, WAV) the first `APIC` of type 0 (other) or 3. `writeTagImage` @0x10149d90c decodes it once and calls `write_image` with 800, 80 and 240 (and 320 with the large flag).
- **Decoding** (`ImageFileFormat::loadFrom` → `JPEGImageFormat::decodeImage` @0x102551518 / `PNGImageFormat::decodeImage` → `juce_loadWithCoreImage` @0x102546e38): ImageIO (`CGImageSourceCreateWithDataProvider`, `CGImageSourceCreateImageAtIndex`), then one `CGContextDrawImage` at 1:1 into a new image. JUCE 8 keeps every image as a `CoreGraphicsPixelData` (@0x10259e36c; `Image::Image` and `NativeImageType::create` turn RGB into ARGB): 8 bits per channel, rows of (4w + 3) & ~3 bytes, `kCGColorSpaceSRGB`, bitmap info 0x2002 (alpha premultiplied first, 32-bit little-endian: bytes B, G, R, A). That draw converts tagged pictures to sRGB.
- **Sizes** (`write_image` @0x10149e0c8, `create_rescaled_image` @0x10149e3ac):
  - 800 (`artwork.jpg`): the decoded image as is when both sides are ≤ 800. Otherwise it is fitted: the long side becomes 800 and the short side `roundToInt(short × 800 / long)` (double, round half to even); a square becomes 800 × 800.
  - 80, 240, 320 (`_s`, `_m`, `_l`): a square image is scaled straight to the size. A non-square one first goes onto a cleared ARGB square of side max(w, h): `Graphics::fillAll` with the colour global @0x10553a088 (opaque black), then `drawImageWithin(0, 0, side, side, 0x24 = xMid | yMid)`. That is scale 1 and offset ((side − w)/2, (side − h)/2) in float, so a half pixel when the margin is odd, drawn with the context's default interpolation. Then it is scaled to size × size.
  - Scaling is `Image::rescaled(w, h, highResamplingQuality)` @0x102545338 (nothing is done at the same size):
    - It makes a cleared ARGB image and a `CoreGraphicsContext` on it (@0x10257b030: save state, font smoothing off, antialias on, blend mode normal).
    - `setInterpolationQuality` @0x10257cc24 maps low/medium/high to `kCGInterpolationNone`/`Medium`/`High` (table @0x10388a698).
    - `CoreGraphicsContext::drawImage` @0x10257d190: save state; alpha = the fill opacity (1.0); concat the flip (1, 0, 0, −1, 0, height); concat the JUCE transform as (m00, m10, −m01, −m11, m02 + ih·m01, m12 + ih·m11), from single-precision values; `CGContextDrawImage` of the rect (0, 0, iw, ih) with a CGImage of the source pixels (same colour space and bitmap info, interpolate on; `getCGImage` @0x10259ed04); restore.
- **Alpha** (`rb::ImageHelper::ignoreAlpha` @0x102413d34): every alpha byte below 255 becomes 255. The colour bytes stay premultiplied, so transparent parts come out as the colour over black.
- **Encoder** (`JPEGImageFormat::writeImageToStream` @0x102551520, JUCE's embedded IJG libjpeg 6b):
  - Settings: `jpeg_set_defaults` with RGB input (YCbCr 4:2:0: 2×2, 1×1, 1×1), then `dct_method = JDCT_FLOAT` and `optimize_coding = TRUE`. JUCE's default quality 0.85 → 85 → `jpeg_set_linear_quality(30, force_baseline)` on the Annex K tables.
  - Output: one sequential scan, no restart interval; JFIF 1.01, density unit 0, 1:1 (`jpeg_set_defaults` overwrites the 72 × 72 that JUCE stores). Each pixel is read as R, G, B, un-premultiplied only when alpha < 255.
  - Arithmetic in the arm64 build:
    - The DCT (`jpeg_fdct_float` @0x102549d90) is plain single precision: vectorised adds, subtracts and multiplies, no fused multiply-add.
    - The quantiser (`forward_DCT_float` @0x1025496b8) computes sample × reciprocal + 16384.5 as one fused multiply-add, then truncates and subtracts 16384.
    - The reciprocals (`start_pass_fdctmgr` @0x1025482d4) are precomputed constants divided by q; for every q in 1–255 they equal libjpeg's `(float)(1 / (q·a_r·a_c·8))`.
    - Measured: without the fused rounding 2 of 95 `artwork.jpg` differ; with fused multiply-adds inside the DCT, 3.
- **Files** (fixture): SOI, APP0 JFIF, DQT 0, DQT 1, SOF0, DHT DC 0, AC 0, DC 1, AC 1 (optimised per file), SOS, data, EOI; no EXIF, ICC or thumbnail.
- **musicbox** (`mb_usb::artwork`, `mb_library::tags::embedded_picture`, `ApplyAnalysis`):
  - It writes the same files, `ImagePath` and the `contentFile` row; undo removes them.
  - On macOS, `artwork/cg.rs` makes the same ImageIO and CoreGraphics calls with the same parameters.
  - `artwork/jpeg.rs` is our own baseline encoder that reproduces libjpeg 6b for this configuration: libjpeg's fixed-point colour conversion; edge padding (last column and row repeated) and dummy blocks (zero, with the DC of the block before); h2v2 downsampling with the alternating 1/2 bias; Annex K.2 Huffman tables with libjpeg's tie-breaking; the fused rounding.
  - Other platforms use the `image` crate for decoding and scaling (Lanczos3), with the same geometry and encoder.
  - `embedded_picture` reads an AIFF/WAV whose other chunks lofty rejects from its `ID3 ` chunk alone, as the tag import does (`Cosmic Boys - Titan`).
  - Score (`cargo run --release -p mb-usb --example artwork_score`, `docs/scorecards/artwork.json`): on oracle-a, 95/95 `artwork.jpg`, 95/95 `_s` and 95/95 `_m` are byte-identical, and so are the 190 + 190 USB copies.
  - Tests: `analyse_ops` `first_analysis_adds_artwork` (byte identity on an m4a, mp3, flac and aiff track) and `tags_calibration` `embedded_pictures_where_rekordbox_made_artwork` (169/169).
  - **Limits:** the CoreGraphics resampler and the ImageIO decoders are private system code, so identity holds for the macOS build the fixtures came from (27.0). Transparent pictures, the 320 px `_l` file and `_s`/`_m` remade from `artwork.jpg` run through the same code but have no rekordbox file to compare with yet.

## Analysis lock
- **Menu:** "Analysis Lock On" / "Analysis Lock Off" (German "Analysesperre Ein/Aus") → `DatabaseMediator::modifyAnalysisLock` @0x100c4903c → `mod_analysis_lock` @0x100c4921c → per track `rekordboxDBController::changeAnalysisLock` @0x1017d8b1c (vtable +0x260) → `AppSyncDBController::modAnalysisLockIfNeed` @0x100a5d2d8 (vtable +0x980).
- **Write:** `Analysed = (lock ? 0x80 : 0) | (Analysed & 0x7F)`, only when that differs from the stored value, through `updateAnalysed` @0x100a5d1c0 (one `djmdContent` column update: `rb_local_usn`, `updated_at`; no counters). Locking a track that is being analysed cancels its analysis (`CAnalyzerIF::cancel_analyze`). More than one track runs in a progress window; then `notifyTrackListModified`.
- **Effect:** locked tracks are skipped by the analysis ("(Locked tracks will not be analyzed.)") and grid edits are refused ("Beatgrid adjustment error. Turn off Analysis Lock.").
- **musicbox:** `Command::SetAnalysisLock { ids, locked }` (`mb_library::analysis_lock`), undo = the opposite on the changed tracks; `ApplyAnalysis` and `SetBeatGrid` refuse locked tracks; the track menu offers On/Off. Tests: `cargo test -p mb-library --test analysis_lock`, `cargo test -p musicbox --test ui_analysis`. Not checked against a trace.

