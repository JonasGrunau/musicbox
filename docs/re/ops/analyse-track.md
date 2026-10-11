# Op: track analysis (request, start, completion)

Status: **verified** for Normal mode with BPM, key, phrase and vocal analysis on (two full passes over 161 tracks: the automatic pass after import, L3694–~17300, and a manual "Analyse track" pass, L17355–29590). Building blocks: `common.md` (UPDATE-COL, DELETE-ROWS, agent `contentFile`).

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
