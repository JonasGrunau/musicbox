# Op: edit the beat grid of a collection track

Status: **verified for "move grid" (2026-10-12)**, the rest binary-derived (rekordbox 7.2.19 arm64, Ghidra). Implementation: `crates/mb-library/src/grid.rs`.

**rekordbox oracle** (`fixtures/oracle-grid-edit`, test `grid_shift_matches_rekordbox_byte_for_byte`): on the oracle-b (high-precision) analysis of one track, the user clicked "move grid right" once in rekordbox's grid edit, left grid edit and quit. rekordbox then:
- moved every PQTZ beat by **+1 ms** (one click = 1 ms) and kept all 1178 beats (the last at 521.261 s, inside the track's 521.400 s scroll-wave length, past its 521 s `Length`);
- set `BPM` to the **first beat's tempo** (137.40 → 70.87, displayed so in rekordbox), and bumped `TrackInfoUpdated` and `AnalysisUpdated` by 1;
- emptied PQT2 (56-byte header) and left every other section unchanged.

musicbox's `grid::shift(beats, 1, grid::duration_ms(ext, Length))` + `Command::SetBeatGrid` writes the same `.DAT` and `.EXT` **byte for byte** and the same row values. The M5 user check (`m5_check`) also passed in rekordbox: cues, hot cues, loop, comment, shifted grid and doubled tempo all show. On a high-precision grid the first beat's tempo can be far from the track BPM (149/166 tracks of the sandbox), so a doubled grid shows 2 × the first beat's tempo (253.64 for a 125.00 track whose first beat is 126.82); rekordbox does the same.

## Save sequence (`djplay::BeatGridAdjustment::saveBeatPosition` @0x1010bfdc0)
The grid editor works on an in-memory beat list (16-byte entries: beat number, tempo, position) and saves it in three steps:
1. **BPM column.** If the first beat's tempo differs from the track's `BPM`, `DatabaseIF::modifyTrackData(row, 0x1a, tempo)` stores it (BPM × 100).
2. **Analysis files.** `DatabaseMediator::setBeatInfo` @0x100c5b398 (browse type 1) → `CAnalyzerIF::setBeatInfoWithExtBeat` @0x10162d5e4 on the local `.DAT`/`.EXT`:
   - `set_pmai_beat_position(beats, count, 0)`: PQTZ gets the new beats; the adjust offset (0x12) is 0.
   - If the `.EXT` has a PQT2 (or the caller passes a non-zero beat-number offset): the PQT2 is **kept** only when the new grid equals the old PQTZ beat for beat (same count, adjust offset, times, tempos) and the old PQT2 header matches (first/last beat, checksum). Otherwise it is **emptied**: `set_pmai_ext_ext_beat_position(0, NULL)` @0x100f2667c zeroes first/last beat, count and checksum and drops the per-beat entries. When kept, the per-beat sub-ms values are copied by index (`get_pmai_ext_beat_position` @0x100f237d4 only copies when the stored count equals the requested one). Then `set_pmai_ext_beat_num_offset` stores the caller's offset.
   - An `.EXT` without PQT2 stays without one; no new `.EXT` is created unless that offset is non-zero.
   - `write_pmai` and `write_pmai_ext` rewrite both files; all other tags (and unknown EXT atoms) are written back unchanged.
3. **500 ms later** (lambda `setBeatInfo::$_10` @0x100c612cc): `updateAnalysisUpdated` (vtable +0x470 → `modifyAnalysisUpdated`: `AnalysisUpdated` += 1, COUNTER pattern), `update_playlist_timestamp([track])`, a cloud upload request and `updateAnalysisFiles` (the `contentFile` registration job; not reimplemented).

The `.2EX` and every other tag are untouched. `djmdCue` rows are not moved by a grid save.

## Grid edit functions (`djplay::BeatGridAdjustment`)
| UI action | Function | musicbox (`grid.rs`) |
|---|---|---|
| move grid left/right | `moveBeatToLeft` @0x1010bd784 / `moveBeatToRight` @0x1010bd880 → `moveBeatPos` @0x1010ba2bc | `shift`: every beat ± Δ; beats outside the track are dropped and the gap is refilled at the edge tempo |
| set downbeat | `renumberBeat` @0x1010bb878 | `set_downbeat`: the chosen beat becomes 1, the others cycle 1–4 around it |
| BPM ×2 | `doubleBpm` @0x1010bbbdc | `double`: a beat in the middle of every interval, tempo × 2 |
| BPM ÷2 | `halveBpm` @0x1010bc410 | `halve`: every second beat counted from the first downbeat, tempo ÷ 2 |
| enter BPM | `applyNewBpm` @0x1010c056c, `ApplyNewBpm::perform` @0x100af8918, `setBpmAt` @0x1010bfc2c | `set_tempo`: a constant grid through the anchor beat over the whole track |

`moveBeatPos` drops beats that move before 0 or past the track end; `doubleBpm` writes the doubled tempo into every beat and inserts midpoints (towards the track end after the last beat). The exact refill (`expandBeatPos` @0x1010ba838), step sizes and BPM limits (`isBpmNotOutOfRange` @0x1010bdf84) are not decoded; musicbox's versions are hypotheses that keep the grid valid (times increasing, bar positions 1–4, one tempo per beat).

## musicbox
- `Command::SetBeatGrid { content, beats }` runs the save sequence in one database transaction around the file writes: `BPM` (+ `TrackInfoUpdated`, assuming `modifyTrackData` takes the metadata-edit path of `edit-track.md`), then the `.DAT`/`.EXT` (temp file + rename), then `AnalysisUpdated`, then `masterPlaylists6.xml`.
- USN cost: 1 when the tempo is unchanged, 3 when the BPM column changes.
- Before the first change of a file in a session, the `.DAT` and `.EXT` are copied to `<session backup>/anlz/<AnalysisDataPath folder>/` (plan, safety rule 4).
- Undo (`Command::RestoreBeatGrid`) puts the previous PQTZ and PQT2 sections and BPM back; the files become byte-identical to before (tested on oracle-a).

## Open questions
- [x] Verify against rekordbox: done for "move grid right" (above). ×2/÷2, downbeat and tempo entry are still unverified.
- [x] `modifyTrackData(0x1a)` is the BPM column and bumps `TrackInfoUpdated` (oracle above).
- [ ] The beat-number offset `saveBeatPosition` passes (its argument, else `this+0x250`); musicbox keeps the stored value.
- [ ] `expandBeatPos`, `shrinkBeatInterval`/`expandBeatInterval` (fine tempo steps), `tapTap`, and the phrase editing on the same panel.
- [x] High-precision grids: an edit empties PQT2, so the per-beat sub-ms values are lost (verified by the oracle above).
