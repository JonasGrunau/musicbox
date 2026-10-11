# Common building blocks for master.db writes

Status: **verified** against the SQL trace of rekordbox 7.2.19 (`fixtures/oracle-a/traces/sqltrace-37549.log`, 30,638 statements, 2026-10-11 21:52–22:22 local) and the before/after databases (`fixtures/sandbox-initial`, `fixtures/oracle-a`). `L<n>` below is a line number in that trace. Items marked *hypothesis* or *open* are not proven by the trace.

Every op spec in this folder is written in terms of the blocks below.

## Evidence and its limits
- The trace covers every statement that the **main rekordbox process** sends through `libsqlcipher` (see `../tooling.md`). One statement per line; no statement in this session spans lines.
- `sqlite3_key` is a C call, not a statement, so keying is not visible. `PRAGMA key` never appears.
- **rekordboxAgent (Electron) is not traced.** It writes to the same master.db with its own SQLite. In this session it inserted every `contentFile` row and the `agentNotification*` rows. Its writes are visible only as gaps in the USN sequence and as rows in the after-DB.
- Implicit work at `sqlite3_close` (the WAL checkpoint) is not a statement and is not visible.

## Placeholders
| Placeholder | Meaning |
|---|---|
| `<U>` | the next USN (see "USN bump"). A new `<U>` for every bump. |
| `<TS>` | UTC wall-clock time with milliseconds, `YYYY-MM-DD HH:MM:SS.mmm +00:00`, e.g. local 21:59:26.763 CEST → `2026-10-11 19:59:26.763 +00:00`. Taken fresh for every statement, so one op has several distinct `<TS>`. On inserts `created_at` = `updated_at` = the same `<TS>`. |
| `<DATE>` | `YYYY-MM-DD` (StockDate, DateCreated). The trace ran at 21:59 CEST = 19:59 UTC, so local and UTC dates agree; which one rekordbox uses is *open* (probably local). |
| `<UUIDv4>` | lower-case dashed random UUID, version nibble 4, variant 8–b (`juce::Uuid::toDashedString`). Verified on all 169 content, 166 song-playlist and 169 mixer rows. |
| `<RID32>` | random u32 as decimal text. Observed range 4,150,660 … 4,268,734,595 over 300+ rows (artist, album, genre, label, key, playlist, cloud filter playlist). Generator not decompiled. |
| `<CID>` | content ID, see "IDs". |
| `<DBID>` | `djmdProperty.DBID` (text). |
| `<DeviceID>` | `djmdProperty.DeviceID` = `djmdDevice.ID` (a UUID string). |

## Two SQL dialects
rekordbox uses two different statement builders. A writer does not have to reproduce the text, but the column sets, values and USN pattern below are what ends up in the file.

**Builder dialect (B)** — lower-case keywords, string literals in double quotes (`"..."`), explicit column lists in a fixed (schema-like) order. Used by import, track removal, analysis, rename, add-to-playlist.

**ORM dialect (O)** — upper-case keywords, `SELECT * FROM`, single-quoted literals, column lists sorted by ASCII (upper-case before lower-case: `Attribute,ID,Name,…,UUID,created_at,rb_data_status,…`). Used for `djmdSampler`, `djmdPlaylist` create/reorder, `djmdKey` created by analysis, `djmdCloudFilterPlaylist`.

## USN bump (`localUpdateCount`)
`agentRegistry` row `registry_id = 'localUpdateCount'`, column `int_1`, is the library-wide change counter.

**B form**
```
select int_1 as rb_local_usn from agentRegistry where registry_id = 'localUpdateCount'
update agentRegistry set int_1 = <U> where registry_id = 'localUpdateCount'      -- <U> = int_1 + 1
<one write statement carrying rb_local_usn = <U>>
```
**O form**
```
SELECT * FROM agentRegistry WHERE registry_id = 'localUpdateCount'
UPDATE agentRegistry SET int_1 = <U>,updated_at = '<agentRegistry.updated_at as just read>' WHERE registry_id = 'localUpdateCount'
<one ORM write carrying rb_local_usn = <U>>
```

Rules (all verified):
1. **Increment first, then write.** `<U>` = previous `int_1` + 1, and the row gets `rb_local_usn = <U>`. Fixture: `int_1` was 505 before the session; the first bump wrote 506 and the sampler row got 506 (L17–19).
2. **One bump per statement, not per row.** A multi-row statement shares one `<U>` (14-row soft delete L508; the 161-row add-to-playlist batch L19136–19296 all have 5091).
3. **A bump is spent even when the statement changes nothing** (the soft-delete half of a delete pair when every row was hard-deleted; `VideoAssociate` clearing that matches no row, L550).
4. **`agentRegistry.updated_at` never changes.** B leaves it alone; O writes back the value it just read (`2026-10-11 17:30:46.175 +00:00` in all 27 O bumps). `created_at` is untouched too.
5. **The bump is not atomic across processes.** B issues `select` and `update` as two separate statements, and during analysis they run in autocommit mode. rekordboxAgent increments the same counter concurrently (its contentFile inserts fill the gaps, e.g. main app 995 → 998). The after-DB has one USN shared by two agent rows (4061: a `.2EX` and a `.DAT` row). Consumers must not assume USNs are unique. A writer should bump and write inside one `BEGIN IMMEDIATE` transaction; the stored result is the same.
6. Within one statement-sequence the bump is usually inside the surrounding transaction, but **add-to-playlist bumps before `begin exclusive`** (see `playlist-add-tracks.md`).

`usn` is never written by the main app: inserts omit it (NULL) and no update sets it. All 169 content rows have `usn IS NULL`.

## Row-change columns
- **Insert (B and O):** `rb_data_status = 0, rb_local_deleted = 0, rb_local_synced = 0, rb_local_usn = <U>, created_at = <TS>, updated_at = <TS>`. `rb_local_data_status` and `usn` are omitted (defaults 0 and NULL).
- **B update:** `rb_data_status = case when usn != 0 then 257 else 0 end, rb_local_usn = <U>, updated_at = "<TS>"`, with `where rb_local_deleted = 0 and ID = <id>`. With `usn` NULL the CASE yields 0, so unsynced rows keep `rb_data_status = 0`.
- **O update:** sets every listed column explicitly, including `rb_data_status, rb_local_data_status, rb_local_deleted, rb_local_synced` (all 0 in this unsynced library; whether these are forced to 0 or written back from the row is *open*), `rb_local_usn = <U>`, `updated_at = '<TS>'`, `WHERE ID = '<id>'`.

## UPDATE-COL macro (B, one column of one content row)
```
[USN bump B]
update djmdContent set <Col> = <value>, rb_data_status = case when usn != 0 then 257 else 0 end, rb_local_usn = <U>, updated_at = "<TS>" where rb_local_deleted = 0 and ID = <CID>
```
- One column per statement; a field change is never batched with another field.
- `<value>` is written as a number for INTEGER columns and as `"text"` for VARCHAR columns (`KeyID = "890281741"`, `AnalysisUpdated = "1"`). The stored storage class follows the column affinity; bind with the affinity of the column (`masterdb/schema.sql`).
- **Counter columns** `AnalysisUpdated`, `TrackInfoUpdated` (and presumably `CueUpdated`) are read first with `select CueUpdated, AnalysisUpdated, TrackInfoUpdated from djmdContent where rb_local_deleted = 0 and ID = <CID>` and written as the decimal string of old+1 (NULL counts as 0).

## DELETE-ROWS macro (B, removes rows by a key list)
```
[USN bump B]
delete from <T> where rb_local_deleted = 0 and rb_data_status = 0 and rb_local_synced = 0 and <K> in (<list>)
update <T> set rb_local_deleted = 1, rb_data_status = case when rb_data_status != 0 then 258 else 2 end, rb_local_usn = <U>, updated_at = "<TS>" where rb_local_deleted = 0 and (rb_data_status != 0 or rb_local_synced != 0) and <K> in (<list>)
```
- Rows that were **never synced** (`rb_data_status = 0 AND rb_local_synced = 0`) are **hard-deleted**. Only rows with sync history are soft-deleted (`rb_local_deleted = 1`, `rb_data_status` 258 if it was non-zero, else 2).
- In this unsynced library every delete was a hard delete; the after-DB has no `rb_local_deleted = 1` rows.
- `<list>` is unquoted numbers for content IDs and `"quoted"` strings for artist/album/genre/label/key IDs.

## ORPHAN-CLEANUP macro (B)
For each of artist, album, genre, label, key (in that order), after content rows went away or a content row's reference changed:
```
select ID from djmdArtist where rb_local_deleted = 0 and ID not in (select ArtistID from djmdContent where rb_local_deleted = 0 and ArtistID notnull) and ID not in (select RemixerID …) and ID not in (select OrgArtistID …) and ID not in (select ComposerID …) and ID not in (select AlbumArtistID from djmdAlbum where rb_local_deleted = 0 and AlbumArtistID notnull)
select ID from djmdAlbum  where rb_local_deleted = 0 and ID not in (select AlbumID  from djmdContent where rb_local_deleted = 0 and AlbumID notnull)
select ID from djmdGenre  where … GenreID …
select ID from djmdLabel  where … LabelID …
select ID from djmdKey    where rb_local_deleted = 0 and ID not in (select KeyID from djmdContent where rb_local_deleted = 0 and KeyID notnull)
```
Each query that returns rows is followed by one DELETE-ROWS over the returned IDs (quoted). An empty result costs no USN.

## IDs
| What | Rule | Evidence |
|---|---|---|
| `djmdContent.ID` | random, redrawn until in **[1, 2^28 − 1]** (first draw rejects 0 and 0xFFFFFFFF; if the top 4 bits are set it redraws until 1 ≤ r ≤ 0x0FFFFFFF). Decimal text. `MasterSongID` = the same value. | `db::AppSyncDBController::insertTrack` @0x100a56e3c; all 183 content IDs seen (initial + after) are < 2^28 (max 264,899,681). |
| artist/album/genre/label/key/playlist/cloud-filter IDs | `<RID32>`. | observed range, see placeholders |
| `djmdMixerParam.ID`, `djmdSongPlaylist.ID` | a `<UUIDv4>` string, **not** a number | all 169 / 166 rows |
| `UUID` column (all tables) | `<UUIDv4>` | |
| collision check | O inserts first run `SELECT * FROM <T> WHERE ID = '<new>' AND rb_local_deleted = 0`. B inserts do not check. | L8919, L3859, L8934 |

## Lookups are exact
Name lookups (`select ID from djmdArtist where rb_local_deleted = 0 and Name = "<name>"`) use `=` on columns without a COLLATE clause, so they are case- and accent-sensitive byte comparisons. Deleted rows (`rb_local_deleted = 1`) are never matched. A lookup is skipped entirely for an empty tag value.

## Transactions
- `begin exclusive` / `commit` (B) wrap: each imported file, the track-removal batch, each add-to-playlist batch. There are also empty `begin exclusive; commit` pairs (L602, L3683, during USB export) and two stray `commit`s with no open transaction (L590, L3682); both are no-ops a writer can ignore.
- Analysis, rename, playlist create/reorder, sampler fix-ups and orphan cleanup run in **autocommit** mode, one statement at a time.
- No `BEGIN IMMEDIATE`, no savepoints. Of 13,000 write statements, 1,079 ran inside a transaction.

## Agent-written `contentFile` rows (state verified, statements not traced)
rekordboxAgent registers every file under the analysis root that belongs to a track. Verified on all 602 rows of the after-DB:

| Column | Value |
|---|---|
| `ID` | `<content UUID>_<Path percent-encoded>`. The paths only contain `[0-9a-zA-Z.-/]`, so in practice only `/` → `%2F` changes; how other characters would be encoded is *open*. |
| `ContentID` | content ID (text) |
| `Path` | `/PIONEER/USBANLZ/<u[0:3]>/<u[3:]>/ANLZ0000.{DAT,EXT,2EX}` or `/PIONEER/Artwork/<u[0:3]>/<u[3:]>/artwork.jpg`, `u` = content UUID |
| `Hash` | MD5 of the file, lower-case hex (602/602 match) |
| `Size` | file size in bytes (integer) |
| `rb_local_path` | `<analysis-data-root>` + `Path` (absolute) |
| `rb_insync_hash`, `rb_insync_local_usn`, `rb_temp_path` | NULL |
| `rb_file_hash_dirty`, `rb_local_file_status`, `rb_in_progress`, `rb_process_type`, `rb_file_size_dirty` | 0 |
| `rb_priority` | 50 |
| `UUID` | `<UUIDv4>` |
| `rb_data_status`, `rb_local_data_status`, `rb_local_deleted`, `rb_local_synced` | 0 |
| `usn` | NULL |
| `rb_local_usn` | its own USN from the shared counter |
| `created_at` = `updated_at` | time of registration |

- Only `artwork.jpg` is registered, not `artwork_m.jpg` / `artwork_s.jpg`. No `.3EX` exists for these analysis settings.
- Order per analysed track: `.2EX`, `.DAT`, `.EXT`, about 100 ms after the main app's last write for that track. Artwork is registered right after the `ImagePath` update.
- A re-analysis that produces byte-identical files does **not** touch the rows (all 602 rows have `created_at = updated_at`; second-pass USNs show no agent gaps).
- Whether the agent updates `Hash`/`Size` when a file changes is *open*.
