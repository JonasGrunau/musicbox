# master.db operation specs (from the 2026-10-11 SQL trace)

Source: `fixtures/oracle-a/traces/sqltrace-37549.log` (rekordbox 7.2.19 main process, 30,638 statements, 21:52–22:22) plus three startup-then-quit traces (27865, 29204, 44426), the before/after databases (`fixtures/sandbox-initial`, `fixtures/oracle-a`) and the RBTEST stick copy (`fixtures/oracle-a/rbtest`). Method: `../tooling.md` § SQL trace. Session script: `../../oracle-session.md`.

Read `common.md` first: it defines the placeholders (`<U>`, `<TS>`, `<CID>`, …), the two SQL dialects, the USN bump, UPDATE-COL, DELETE-ROWS and ORPHAN-CLEANUP, the ID rules and the agent-written `contentFile` rows.

## Ops
| Spec | Operation | Status | master.db writes | USN cost |
|---|---|---|---|---|
| [common.md](common.md) | shared building blocks | verified | — | — |
| [startup.md](startup.md) | launch | verified (4 traces) | 5 ORM updates of `djmdSampler` | +5 per launch |
| [import-track.md](import-track.md) | import one file | verified (161 files) | artist/genre/album/label/key on demand, 1 `djmdContent` | +1 per new row |
| [remove-tracks.md](remove-tracks.md) | remove tracks from the collection | verified (hard-delete branch) | DELETE-ROWS over 10 tables, orphan cleanup | +11, +1 per orphan table |
| [playlist-create.md](playlist-create.md) | create, reorder, rename a playlist; `masterPlaylists6.xml` | verified | `djmdPlaylist`, `djmdCloudFilterPlaylist` | 1 + reordered siblings + 1; rename +1 |
| [playlist-add-tracks.md](playlist-add-tracks.md) | add tracks to a playlist | verified | N × `djmdSongPlaylist` | **+1 per batch** |
| [playlist-edit.md](playlist-edit.md) | folders, tree move/reorder, delete, remove/reorder/insert entries | **hypothesis** (binary templates + traced blocks) | `djmdPlaylist`, `djmdSongPlaylist`, `djmdCloudFilterPlaylist` | see spec |
| [edit-track.md](edit-track.md) | track metadata edits | **hypothesis** (traced blocks) | `djmdContent` UPDATE-COL, lookups, orphan step, `TrackInfoUpdated` | 2 + created/orphaned rows |
| [analyse-track.md](analyse-track.md) | request, start, completion of analysis | verified (Normal mode, all options on) | `djmdContent` field by field, `djmdMixerParam`, `djmdKey`; agent `contentFile` | 18 per track on the first pass (+1 with artwork, +1 per new/orphaned key; agent +3, +1 with artwork), 18 per re-analysis |
| [settings.md](settings.md) | analysis settings change | verified | none | 0 |
| [export-xml.md](export-xml.md) | rekordbox.xml export | verified | none | 0 |
| [export-usb.md](export-usb.md) | USB export, master.db side | verified | none | 0 |
| [export-devlib.md](export-devlib.md) | USB export, `exportLibrary.db` statement sequence | verified (first export to an empty stick) | — (device DB only) | — |
| [shutdown.md](shutdown.md) | quit: `sqlcipher_export` backup | verified + binary | none (backup file only) | 0 |

## Implementation (`crates/mb-library`)
One module per op, replaying the statements above through `mb_masterdb::WriteTxn`; each command returns its inverse (undo = forward command with new USNs). Replay checks against the trace and the oracle-a after-DB: `session_oracle.rs` (remove → import 161 files → create/rename/add playlists on `sandbox-initial`: every traced column, every USN = rekordbox's − 5, `masterPlaylists6.xml` byte-exact after ID/timestamp substitution), `tags_calibration.rs` (tag values 161/161). The hypothesis ops are pinned by `playlist_ops.rs` and `track_ops.rs`. Every modified copy is re-opened with the snapshot loader and passes `PRAGMA integrity_check`.

## USN rules: verdicts on the `../masterdb.md` hypotheses
| Hypothesis (masterdb.md, before this trace) | Verdict | Evidence |
|---|---|---|
| `agentRegistry('localUpdateCount').int_1` is the global change counter | **confirmed** | every write is preceded by a bump; rekordboxAgent uses the same counter (its rows fill the gaps) |
| each change increments the counter and stores the new value in the row's `rb_local_usn` | **confirmed, refined**: one bump per *statement*, not per row; multi-row statements and the add-to-playlist batch share one USN; a bump is spent even when the statement changes no row | L506–508, L19134–19296, L550 |
| on a fresh library the counter equals `max(rb_local_usn)` | **holds in both fixtures** (505, 7762) but **not guaranteed**: USNs spent on no-op statements leave no row, and rows later rewritten take a new USN | |
| `usn` stays NULL when cloud sync is off | **confirmed** | no statement writes `usn`; 169/169 content rows NULL |
| `created_at`/`updated_at` are `YYYY-MM-DD HH:MM:SS.mmm +00:00` UTC | **confirmed** | local 21:59 → `19:59 … +00:00` |
| IDs are random u32 decimal strings; UUIDs v4 | **refined**: content IDs are random in [1, 2^28−1] (binary + 183 samples); other numeric IDs random u32; `djmdMixerParam.ID` and `djmdSongPlaylist.ID` are **UUID strings**; all `UUID` columns v4 | `common.md` § IDs |
| deletes are soft (`rb_local_deleted = 1`) | **refuted as stated**: never-synced rows are **hard-deleted**; only rows with sync history are soft-deleted (`rb_data_status` 2 or 258) | `remove-tracks.md` |
| (new) `agentRegistry.updated_at` | never changes on a bump | 27 ORM bumps write back the old value |
| (new) bump atomicity | the main app reads and writes the counter in two autocommit statements during analysis; with the agent writing concurrently, duplicate USNs occur (one in the after-DB) | `common.md` rule 5 |
| (new) update status | updates set `rb_data_status = case when usn != 0 then 257 else 0 end` (0 when never synced) | |

## Other `masterdb.md` claims
| Claim | Verdict |
|---|---|
| `MasterDBID` = `djmdProperty.DBID`, `DeviceID` = `djmdDevice.ID` | confirmed (`djmdProperty.DeviceID` = `djmdDevice.ID`) |
| `AnalysisDataPath` = `/PIONEER/USBANLZ/<UUID[0:3]>/<UUID[3:]>/ANLZ0000.DAT` | confirmed (322 writes) |
| `BPM` = bpm × 100 | confirmed |
| `FileType` 1 mp3, 11 wav, 12 aiff | confirmed and extended: 4 m4a/AAC, 5 flac, 6 m4a/ALAC |
| `contentFile`: one row per ANLZ file; `ID` = `<content UUID>_<url-encoded path>`; `Hash` = MD5; `Size` | confirmed on 602/602 rows; also one row for `artwork.jpg`; no `.3EX` here; written by rekordboxAgent, not the main app |
| `masterPlaylists6.xml` format | confirmed; `Timestamp` = last change of the list or of a member track |
| does rekordbox 7 rewrite `datafile.edb` / `ExtData.edb` on playlist edits? | **no** (byte-identical before and after) |
| `ContentLink`, `rb_file_id`, `SearchStr`, `FileNameS` derivation | `rb_file_id` = file inode; `FileNameS` = `""`; `SearchStr` never written (NULL); `ContentLink` transitions documented, bit meanings open |
| backups `master.backup.YYYYMMDD.db` via `sqlcipher_export` | confirmed: via `tmp.db` + rename, one per day, `journal_mode = delete` |
| rekordbox checkpoints and deletes `-wal`/`-shm` on exit | not visible (implicit at close); no explicit checkpoint statement |

## Contradictions with other specs (not edited here)
- `../export.md` §6.3 step 5 / `../devlib.md` § Startup step 4: "`createIndexes` drops and recreates the 4 indexes on every open". The trace shows existence checks only; drops happen only in the full rebuild (`export-devlib.md` phase 1/2).
- `../devlib.md` § property: "7.2.19 writes `dbVersion` "10000"". True for `initPropertyTable`, but a fresh stick is rebuilt right away and ends with `"1000"` and `deviceName ""`.
- `../export.md` §2.5: `myTagMasterDBID` = DBID + 1. In this library it was the `ID` of the `djmdCloudProperty` row `Reserved1 = 'SyncService'`.
- `../devlib.md` § playlist: new device lists at `max(sequenceNo)+1`. The untraced second export put the new list at 0 and moved the first to 1 (*hypothesis*).

## Coverage gaps
- **rekordboxAgent writes are not traced** (`contentFile`, `agentNotification*`). A second shim for the agent's node-sqlcipher, or before/after diffs, is needed for exact statements.
- An **untraced writer at 22:23:24** (between this trace and trace 27865) re-added 8 rekordbox demo tracks with ready-made analysis, a sampler pack and their `contentFile` rows (USN 7483–7742). Probably a normal-app launch; not analysed.
- The later `oracle-all` export with the normal app is only visible as final state.
- Not exercised: playlist folders, smart lists, playlist delete/reorder by the user, track edits (title, rating, colour, cues, My Tags), cue writes, history, duplicate import, sync with an already-filled stick, Dynamic/high-precision analysis (pass B has only before/after DBs).

## Open questions
- [ ] Meaning of `ContentLink` bits (0x0E at import, 0x50 queued, 0x10000, 0x600 | 0x1C0000 done) and of `Analysed` bits 0x08, 0x20, 0x40.
- [ ] Whether `djmdMixerParam` is updated when a re-analysis changes the gain.
- [ ] Order of original-artist/composer/lyricist lookups at import; compilation albums.
- [ ] `StockDate`/`DateCreated`: local or UTC date.
- [ ] The source of `myTagMasterDBID` when no sync service row exists.
- [ ] Whether rekordbox deletes ANLZ/artwork files when tracks are removed.
- [ ] Backup retention count.
- [ ] Trace the ops of `playlist-edit.md` and `edit-track.md` (all hypotheses today), duplicate import, and the removal of tracks that are in playlists.
- [ ] Import behaviour for composer, original artist and lyricist tags (none in the corpus).
