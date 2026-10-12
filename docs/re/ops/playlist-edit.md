# Op: playlist tree and entry edits (folders, move, delete, remove/reorder/insert entries)

Status: **hypothesis**. None of these ops ran in the 2026-10-11 trace (`INDEX.md` § Coverage gaps). The patterns below are built from the traced building blocks (`common.md`, `playlist-create.md`, `playlist-add-tracks.md`) and from rekordbox's SQL templates in the binary. They are what `mb-library` (`crates/mb-library/src/playlist.rs`) writes, and each needs a trace before it can be called verified. Every place in the code that depends on one of them is marked `// hypothesis:`.

## Evidence from the binary (strings only, no xrefs yet)
File offsets in the arm64 slice (VA ≈ 0x100000000 + offset):

| Offset | Template |
|---|---|
| 0x4fa1911 | `delete from __TABLE__ where rb_local_deleted = 0 and rb_data_status = 0 and rb_local_synced = 0 and __ID_NAME__ in (__IDs__)` (the DELETE-ROWS macro; its soft-delete twin follows) |
| 0x4fa1ea1 | `update __TABLE_NAME__ set __COLUMN__ = __COLUMN__ - 1, rb_data_status = …, rb_local_usn = __RB_LOCAL_USN__, updated_at = CURRENT_TIMESTAMP where __COLUMN__ > %d and __COLUMN__ <= %d __WHERE_CLAUSE__;` |
| 0x4fa2054 | the same with `__COLUMN__ + 1 … where __COLUMN__ >= %d and __COLUMN__ < %d` |
| 0x4fa1f8f | `update __TABLE_NAME__ set __COLUMN__ = %d, rb_data_status = …, rb_local_usn = __RB_LOCAL_USN__, … where ID = %lu __WHERE_CLAUSE__;` |
| 0x4f8f384 | `update __TABLE_NAME__ set __SEQ_COLUMN__ = __SEQ_COLUMN__ - 1 where __SEQ_COLUMN__ > %d and __PARENT_ID_COLUMN__ = %d;` (no USN columns: probably the device library) |
| 0x4fa1d3d, 0x4f8ebae | a renumbering path through a `temp_table` (`… set __COLUMN__ = coalesce((select id from __TABLE_NAME___temp …), 0)`, no `rb_local_usn`) |
| 0x4f9f647 | `delete from djmdSongPlaylist where rb_data_status = 0 and rb_local_synced = 0 and ContentID = %lu` (no `rb_local_deleted` filter) |
| 0x4f9f6a9 | `update djmdSongPlaylist set rb_local_deleted = 1, rb_data_status = case when rb_data_status != 0 then 258 else 2 end, …` |

The trace showed that `CURRENT_TIMESTAMP` in these templates is replaced by the `<TS>` string before execution (`remove-tracks.md`, L508).

## Tree ops (ORM move routine)
The traced move-to-top after a create (`playlist-create.md` §2) and the startup sampler fix-up (`startup.md` step 3) are the same routine: write the moved node with its new `ParentID`/`Seq`, then each sibling, in the new order, whose `Seq` changes (one ORM update and one O bump each). musicbox uses it for every tree change.

| Op | musicbox pattern | USN cost |
|---|---|---|
| create folder | as `playlist-create.md` with `Attribute = 1`, including a `djmdCloudFilterPlaylist` row | 1 + moved nodes + 1 |
| create inside a folder | as `playlist-create.md` with `ParentID = <folder ID>`; the new node goes to the top of the folder | 1 + moved nodes + 1 |
| move / reorder (`MovePlaylist`) | ORM move to (parent, position); if the parent changes, the old parent's remaining children close the gap with the same ORM updates | 1 (node, always written) + shifted siblings in both parents |
| rename | traced (`playlist-create.md` §4) | 1 |
| delete (recursive) | `begin exclusive`; DELETE-ROWS `djmdSongPlaylist` by `PlaylistID` over the subtree; DELETE-ROWS `djmdCloudFilterPlaylist` by `PlaylistUUID`; DELETE-ROWS `djmdPlaylist` by `ID`; then the former siblings close the `Seq` gap (ORM) | 3 + shifted siblings |

Invariants musicbox keeps: siblings' `Seq` is 1…n; a node cannot move into its own subtree; only folders (`Attribute` 1) have children.

## Entry ops (`djmdSongPlaylist`)
| Op | musicbox pattern | USN cost |
|---|---|---|
| add, appended | traced (`playlist-add-tracks.md`) | 1 per batch |
| add before position p | builder update `TrackNo = TrackNo + k where PlaylistID = <P> and TrackNo >= p` (one statement, one bump), then the traced batch insert at p…p+k−1 | 2 |
| remove entries | DELETE-ROWS by `ID` (entry IDs), then per removed entry, highest `TrackNo` first: `TrackNo = TrackNo - 1 where PlaylistID = <P> and TrackNo > <t>` (bump spent even when no row follows) | 1 + entries |
| move one entry a → b | the `±1` range template over the rows between, then `TrackNo = b where ID = <entry>` | 2 per moved entry |
| reorder to an order | single-entry moves, top-down (each position gets the entry that belongs there) | 2 per moved entry |

Duplicates are allowed (rekordbox reads the playlist's `ContentID`s before an add, probably for its duplicate prompt; not traced).

## masterPlaylists6.xml
- Pure `Seq` shifts keep a node's `Timestamp` (verified: `30D40` kept its value in oracle-a through two creates that rewrote its `Seq`). musicbox therefore does not touch timestamps on moves.
- *Hypothesis:* rename, entry add/remove/reorder and edits of member tracks set the list's `Timestamp` to now (verified only for adds and analysis completion).
- *Hypothesis:* nested nodes are listed depth-first (folder, then its children in `Seq` order) with `ParentId` = the folder ID in upper-case hex; `Attribute` 1 for folders.
- *Hypothesis:* an empty tree is written as `<PLAYLISTS/>`.

## Smart playlists
`Attribute` 4 rows (`SmartList` holds the rule XML; `../smartlist.md`). Rename, move and delete work as for playlists (the row is the same; delete/restore keeps `SmartList`). Rules are written by one builder update of `SmartList` (`Command::SetSmartList`); a new smart list is the create of § Create with `Attribute` 4 followed by that update (`Command::CreateSmartList`). Entry ops on a smart list are refused: its tracks are evaluated, never stored.

## Undo (musicbox)
Inverse commands with new USNs, never a rollback: create ↔ delete; delete → restore (same IDs and UUIDs where free, original position, cloud filter rows, entries with their row IDs, one batch per playlist); move → move back; remove entries → restore entries (shift + insert per entry, original row IDs); reorder → reorder to the old order; insert → remove.

## Open questions
- [ ] Trace each op above: folder create, create inside a folder, drag-move/reorder in the tree, delete of a playlist and of a folder, remove from playlist, drag-reorder inside a playlist, drop at a position.
- [ ] Does delete close the `Seq` gap, and with which dialect? Does it delete the `djmdCloudFilterPlaylist` row?
- [ ] Do folders get a `djmdCloudFilterPlaylist` row?
- [ ] Is `TrackNo` renumbered after entry removal (per entry, or via the `temp_table` renumbering), and is one USN shared by the statements of one move?
- [ ] Is "top" for a new node fixed, or relative to the tree selection?
- [ ] masterPlaylists6.xml layout for nested nodes and for an empty tree; whether rename or reorder bumps `Timestamp`.
