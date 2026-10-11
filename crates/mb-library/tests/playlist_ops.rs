//! Playlist ops beyond the traced session (folders, moves, delete + undo, entry removal,
//! reordering, insert at a position, smart lists) on a temp copy of `fixtures/oracle-a`
//! (3 playlists: oracle-small with 5 tracks, oracle-all with 161, the demo list 200000).
//! These ops are hypotheses (see docs/re/ops/playlist-edit.md); the tests pin the statement
//! pattern musicbox chose (USN counts), the invariants (contiguous Seq/TrackNo, IDs restored by
//! undo) and that rekordbox's own snapshot loader and `integrity_check` accept the result.

mod common;

use common::*;
use mb_library::playlist::{self, ROOT};
use mb_library::{Command, DbId, Effect, Error, History, Library, NodeKind};
use mb_masterdb::MasterPlaylists;

const SMALL: &str = "1036096362";
const ALL: &str = "2736485380";
const DEMO: &str = "200000";

fn open(dir: &TempLibrary) -> Library {
    Library::open_unguarded(&dir.db, &dir.passphrase).unwrap()
}

fn created(out: &mb_library::Outcome) -> DbId {
    match &out.effect {
        Effect::PlaylistCreated { id } => id.clone(),
        e => panic!("{e:?}"),
    }
}

/// (ID, Seq) of the children of `parent`, in Seq order.
fn tree(lib: &Library, parent: &str) -> Vec<(String, i64)> {
    playlist::children(lib.connection(), parent, None).unwrap().into_iter().map(|n| (n.id, n.seq)).collect()
}

fn ids(lib: &Library, parent: &str) -> Vec<String> {
    tree(lib, parent).into_iter().map(|(id, _)| id).collect()
}

fn assert_contiguous(lib: &Library, parent: &str) {
    let seqs: Vec<i64> = tree(lib, parent).into_iter().map(|(_, s)| s).collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>(), "Seq of {parent} children");
}

fn xml(dir: &TempLibrary) -> MasterPlaylists {
    MasterPlaylists::read(&dir.xml_path()).unwrap().unwrap()
}

fn timestamp_of(dir: &TempLibrary, id: &str) -> Option<i64> {
    let id: u64 = id.parse().unwrap();
    xml(dir).nodes.iter().find(|n| n.id == id).map(|n| n.timestamp)
}

fn entry_ids(lib: &Library, p: &str) -> Vec<String> {
    playlist::entries(lib.connection(), p).unwrap().into_iter().map(|e| e.id).collect()
}

fn assert_track_nos(lib: &Library, p: &str) {
    let nos: Vec<i64> = playlist::entries(lib.connection(), p).unwrap().into_iter().map(|e| e.track_no).collect();
    assert_eq!(nos, (1..=nos.len() as i64).collect::<Vec<_>>(), "TrackNo of {p}");
}

#[test]
fn folders_nesting_and_moves() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    assert_eq!(ids(&lib, ROOT), [SMALL, ALL, DEMO]);
    let demo_ts = timestamp_of(&dir, DEMO).unwrap();

    // Folder at root: insert at Seq 4, then the folder → 1 and the three siblings shift.
    let out = lib.create_playlist(None, "Folder", NodeKind::Folder).unwrap();
    assert_eq!(out.usn_spent(), 1 + 4 + 1, "insert + node + 3 shifted siblings + cloud filter");
    let f = created(&out);
    assert_eq!(ids(&lib, ROOT), [f.as_str(), SMALL, ALL, DEMO]);
    assert_contiguous(&lib, ROOT);
    let attr: i64 = lib.connection().query_row("SELECT Attribute FROM djmdPlaylist WHERE ID = ?1", [f.as_str()], |r| r.get(0)).unwrap();
    assert_eq!(attr, 1);

    // Playlists inside the folder; the newest goes to the top of the folder.
    let out = lib.create_playlist(Some(&f), "P", NodeKind::Playlist).unwrap();
    assert_eq!(out.usn_spent(), 3, "insert + node (no siblings) + cloud filter");
    let p = created(&out);
    let out = lib.create_playlist(Some(&f), "Q", NodeKind::Playlist).unwrap();
    assert_eq!(out.usn_spent(), 4, "insert + node + 1 shifted sibling + cloud filter");
    let q = created(&out);
    assert_eq!(ids(&lib, f.as_str()), [q.as_str(), p.as_str()]);
    let parent: String = lib.connection().query_row("SELECT ParentID FROM djmdPlaylist WHERE ID = ?1", [q.as_str()], |r| r.get(0)).unwrap();
    assert_eq!(parent, f.0);

    // A playlist cannot hold children; a folder cannot move into its own subtree.
    assert!(matches!(lib.create_playlist(Some(&p), "x", NodeKind::Playlist), Err(Error::Invalid(_))));
    let g = created(&lib.create_playlist(Some(&f), "G", NodeKind::Folder).unwrap());
    let err = lib.execute(Command::MovePlaylist { id: f.clone(), parent: Some(g.clone()), position: 1 });
    assert!(matches!(err, Err(Error::Invalid(_))), "{err:?}");

    // masterPlaylists6.xml: depth-first, ParentId = folder ID in hex, Attribute 1 for folders.
    let doc = xml(&dir);
    let order: Vec<u64> = doc.nodes.iter().map(|n| n.id).collect();
    let hex = |s: &DbId| s.0.parse::<u64>().unwrap();
    assert_eq!(order, vec![hex(&f), hex(&g), hex(&q), hex(&p), SMALL.parse().unwrap(), ALL.parse().unwrap(), DEMO.parse().unwrap()]);
    for n in &doc.nodes {
        if [hex(&g), hex(&q), hex(&p)].contains(&n.id) {
            assert_eq!(n.parent_id, hex(&f));
        }
    }
    assert_eq!(doc.nodes[0].attribute, 1);
    assert_eq!(timestamp_of(&dir, DEMO), Some(demo_ts), "Seq shifts keep the timestamp");

    // Move oracle-small into the folder at position 2; root closes the gap.
    let mut history = History::default();
    let before = lib.usn().unwrap();
    let out = history.run(&mut lib, Command::MovePlaylist { id: DbId(SMALL.into()), parent: Some(f.clone()), position: 2 }).unwrap();
    assert_eq!(ids(&lib, f.as_str()), [g.as_str(), SMALL, q.as_str(), p.as_str()]);
    assert_eq!(ids(&lib, ROOT), [f.as_str(), ALL, DEMO]);
    assert_contiguous(&lib, ROOT);
    assert_contiguous(&lib, f.as_str());
    // node + Q, P shifted down in the folder + ALL, DEMO up at root.
    assert_eq!(out.usn_spent(), 5);
    assert_eq!(lib.usn().unwrap(), before + 5);
    assert_eq!(out.inverse, Some(Command::MovePlaylist { id: DbId(SMALL.into()), parent: None, position: 2 }));

    // Undo puts it back; redo moves it again.
    history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(ids(&lib, ROOT), [f.as_str(), SMALL, ALL, DEMO]);
    assert_eq!(ids(&lib, f.as_str()), [g.as_str(), q.as_str(), p.as_str()]);
    assert_contiguous(&lib, f.as_str());
    history.redo(&mut lib).unwrap().unwrap();
    assert_eq!(ids(&lib, f.as_str()), [g.as_str(), SMALL, q.as_str(), p.as_str()]);
    history.undo(&mut lib).unwrap().unwrap();

    // Reorder within root: DEMO to the top.
    let out = lib.execute(Command::MovePlaylist { id: DbId(DEMO.into()), parent: None, position: 1 }).unwrap();
    assert_eq!(ids(&lib, ROOT), [DEMO, f.as_str(), SMALL, ALL]);
    assert_eq!(out.usn_spent(), 4, "node + 3 shifted siblings");
    let unchanged = lib.execute(Command::MovePlaylist { id: DbId(DEMO.into()), parent: None, position: 1 }).unwrap();
    assert_eq!((unchanged.usn_spent(), unchanged.effect), (0, Effect::Unchanged));

    // Rename + undo.
    let out = lib.execute(Command::RenamePlaylist { id: q.clone(), name: "Renamed".into() }).unwrap();
    assert_eq!(out.usn_spent(), 1);
    lib.execute(out.inverse.unwrap()).unwrap();
    assert_eq!(playlist::node(lib.connection(), q.as_str()).unwrap().unwrap().name, "Q");

    let snap = dir.verify_loads();
    assert_eq!(snap.playlists.len(), 7);
}

#[test]
fn delete_folder_recursively_and_undo() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let tracks: Vec<DbId> = playlist::entries(lib.connection(), ALL).unwrap().into_iter().map(|e| e.content_id).take(5).collect();
    let f = created(&lib.create_playlist(None, "F", NodeKind::Folder).unwrap());
    let p1 = created(&lib.create_playlist(Some(&f), "P1", NodeKind::Playlist).unwrap());
    let p2 = created(&lib.create_playlist(Some(&f), "P2", NodeKind::Playlist).unwrap());
    lib.add_tracks(&p1, &tracks[..3]).unwrap();
    lib.add_tracks(&p2, &tracks[3..]).unwrap();
    // Move the folder to the middle of root so that the delete has to close a gap.
    lib.execute(Command::MovePlaylist { id: f.clone(), parent: None, position: 2 }).unwrap();
    assert_eq!(ids(&lib, ROOT), [SMALL, f.as_str(), ALL, DEMO]);
    let p1_entries = playlist::entries(lib.connection(), p1.as_str()).unwrap();
    let uuids: Vec<String> = [&f, &p1, &p2].iter().map(|id| playlist::node(lib.connection(), id.as_str()).unwrap().unwrap().uuid).collect();
    let filters_before = count(lib.connection(), "SELECT count(*) FROM djmdCloudFilterPlaylist");

    let mut history = History::default();
    let out = history.run(&mut lib, Command::DeletePlaylist { id: f.clone() }).unwrap();
    // DELETE-ROWS entries, cloud filters, playlists (3) + ALL and DEMO shift up (2).
    assert_eq!(out.usn_spent(), 5);
    let c = lib.connection();
    for id in [&f, &p1, &p2] {
        assert!(row(c, "djmdPlaylist", "ID", id.as_str()).is_none(), "hard-deleted (never synced)");
        assert_eq!(count(c, &format!("SELECT count(*) FROM djmdSongPlaylist WHERE PlaylistID = '{id}'")), 0);
    }
    for u in &uuids {
        assert_eq!(count(c, &format!("SELECT count(*) FROM djmdCloudFilterPlaylist WHERE PlaylistUUID = '{u}'")), 0);
    }
    assert_eq!(ids(&lib, ROOT), [SMALL, ALL, DEMO]);
    assert_contiguous(&lib, ROOT);
    assert_eq!(xml(&dir).nodes.len(), 3);
    dir.verify_loads();

    // Undo: same IDs, UUIDs, position, entries (IDs, UUIDs, order) and cloud filter rows.
    let out = history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(out.effect, Effect::PlaylistsRestored { ids: vec![f.clone(), p2.clone(), p1.clone()] });
    // 3 inserts + node placed at 2 + ALL, DEMO shifted + 3 cloud filters + 2 entry batches.
    assert_eq!(out.usn_spent(), 3 + 3 + 3 + 2);
    assert_eq!(ids(&lib, ROOT), [SMALL, f.as_str(), ALL, DEMO]);
    assert_eq!(ids(&lib, f.as_str()), [p2.as_str(), p1.as_str()]);
    let restored = playlist::entries(lib.connection(), p1.as_str()).unwrap();
    assert_eq!(restored.iter().map(|e| (&e.id, &e.uuid, &e.content_id, e.track_no)).collect::<Vec<_>>(),
        p1_entries.iter().map(|e| (&e.id, &e.uuid, &e.content_id, e.track_no)).collect::<Vec<_>>());
    let uuids_after: Vec<String> = [&f, &p1, &p2].iter().map(|id| playlist::node(lib.connection(), id.as_str()).unwrap().unwrap().uuid).collect();
    assert_eq!(uuids_after, uuids);
    assert_eq!(count(lib.connection(), "SELECT count(*) FROM djmdCloudFilterPlaylist"), filters_before);
    assert_eq!(xml(&dir).nodes.len(), 6);

    // Redo deletes again.
    history.redo(&mut lib).unwrap().unwrap();
    assert_eq!(ids(&lib, ROOT), [SMALL, ALL, DEMO]);
    dir.verify_loads();
}

#[test]
fn entries_remove_restore_reorder_insert() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let original = entry_ids(&lib, SMALL);
    assert_eq!(original.len(), 5);
    let all_ts = timestamp_of(&dir, ALL).unwrap();
    let small_ts = timestamp_of(&dir, SMALL).unwrap();

    // Remove entries 2 and 4: DELETE-ROWS + one shift per removed entry.
    let mut history = History::default();
    let out = history
        .run(&mut lib, Command::RemoveFromPlaylist { playlist: DbId(SMALL.into()), entry_ids: vec![original[1].clone(), original[3].clone()] })
        .unwrap();
    assert_eq!(out.usn_spent(), 3);
    assert_eq!(entry_ids(&lib, SMALL), [original[0].clone(), original[2].clone(), original[4].clone()]);
    assert_track_nos(&lib, SMALL);
    assert!(timestamp_of(&dir, SMALL).unwrap() > small_ts, "list changed → Timestamp");
    assert_eq!(timestamp_of(&dir, ALL), Some(all_ts));

    // Undo: back at their positions with the same row IDs (shift + insert per entry).
    let out = history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(out.usn_spent(), 4);
    assert_eq!(entry_ids(&lib, SMALL), original);
    assert_track_nos(&lib, SMALL);

    // Reorder: last entry to the top (one shift + one set).
    let order = playlist::order_after_move(lib.connection(), SMALL, &[original[4].clone()], 1).unwrap();
    let out = history.run(&mut lib, Command::ReorderPlaylist { playlist: DbId(SMALL.into()), order: order.clone() }).unwrap();
    assert_eq!(out.usn_spent(), 2);
    assert_eq!(entry_ids(&lib, SMALL), order);
    assert_track_nos(&lib, SMALL);
    // Two entries to the end.
    let order2 = playlist::order_after_move(lib.connection(), SMALL, &[order[0].clone(), order[1].clone()], 6).unwrap();
    assert_eq!(&order2[3..], &order[..2]);
    history.run(&mut lib, Command::ReorderPlaylist { playlist: DbId(SMALL.into()), order: order2.clone() }).unwrap();
    assert_eq!(entry_ids(&lib, SMALL), order2);
    assert_track_nos(&lib, SMALL);
    history.undo(&mut lib).unwrap();
    history.undo(&mut lib).unwrap();
    assert_eq!(entry_ids(&lib, SMALL), original, "two undos restore the original order");
    assert!(matches!(
        lib.execute(Command::ReorderPlaylist { playlist: DbId(SMALL.into()), order: vec![original[0].clone()] }),
        Err(Error::Invalid(_))
    ));

    // Insert two tracks before position 2: tail shift (one USN) + one batch (one USN).
    let extra: Vec<DbId> = playlist::entries(lib.connection(), ALL).unwrap().into_iter().map(|e| e.content_id).rev().take(2).collect();
    let out = history
        .run(&mut lib, Command::AddTracks { playlist: DbId(SMALL.into()), content_ids: extra.clone(), position: Some(2) })
        .unwrap();
    assert_eq!(out.usn_spent(), 2);
    let now: Vec<DbId> = playlist::entries(lib.connection(), SMALL).unwrap().into_iter().map(|e| e.content_id).collect();
    assert_eq!(&now[1..3], extra.as_slice());
    assert_eq!(now.len(), 7);
    assert_track_nos(&lib, SMALL);
    let batch_usns = count(lib.connection(), &format!("SELECT count(DISTINCT rb_local_usn) FROM djmdSongPlaylist WHERE PlaylistID = '{SMALL}' AND ContentID IN ('{}', '{}')", extra[0], extra[1]));
    assert_eq!(batch_usns, 1, "one USN per batch");
    history.undo(&mut lib).unwrap();
    assert_eq!(entry_ids(&lib, SMALL), original);
    assert_track_nos(&lib, SMALL);

    // Folders hold no tracks; unknown tracks are refused.
    let f = created(&lib.create_playlist(None, "F", NodeKind::Folder).unwrap());
    assert!(matches!(lib.add_tracks(&f, &extra), Err(Error::Invalid(_))));
    assert!(matches!(lib.add_tracks(&DbId(SMALL.into()), &[DbId("1".into())]), Err(Error::NotFound { .. })));
    dir.verify_loads();
}

#[test]
fn smart_playlists_are_read_only() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    lib.connection()
        .execute(
            "INSERT INTO djmdPlaylist (ID, Seq, Name, Attribute, ParentID, SmartList, UUID, created_at, updated_at) \
             VALUES ('77', 4, 'smart', 4, 'root', '<NODE/>', '00000000-0000-4000-8000-000000000077', '2026-10-11 00:00:00.000 +00:00', '2026-10-11 00:00:00.000 +00:00')",
            [],
        )
        .unwrap();
    let smart = DbId("77".into());
    let before = lib.usn().unwrap();
    for cmd in [
        Command::RenamePlaylist { id: smart.clone(), name: "x".into() },
        Command::DeletePlaylist { id: smart.clone() },
        Command::MovePlaylist { id: smart.clone(), parent: None, position: 1 },
        Command::AddTracks { playlist: smart.clone(), content_ids: vec![], position: None },
        Command::RemoveFromPlaylist { playlist: smart.clone(), entry_ids: vec![] },
        Command::ReorderPlaylist { playlist: smart.clone(), order: vec![] },
    ] {
        assert!(matches!(lib.execute(cmd), Err(Error::SmartPlaylistReadOnly(_))));
    }
    assert_eq!(lib.usn().unwrap(), before, "refused ops write nothing");
}
