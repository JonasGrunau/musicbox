//! Smart list ops (`docs/re/smartlist.md`) on a temp copy of `fixtures/oracle-a`: create with
//! rules, the stored row and XML, evaluation through the snapshot loader, rule changes with undo,
//! rename/move/delete/restore. Skipped without the fixtures.

mod common;

use common::*;
use mb_core::smartlist::{self, Clock, Condition, MATCH_ALL, MATCH_ANY, SmartList, op};
use mb_library::playlist;
use mb_library::{Command, DbId, Effect, History, Library};
use mb_masterdb::MasterPlaylists;
use mb_masterdb::rusqlite::OptionalExtension;

fn open(dir: &TempLibrary) -> Library {
    Library::open_unguarded(&dir.db, &dir.passphrase).unwrap()
}

/// Content IDs (rowid order) from a query over the fixture, as the expectation.
fn expected(lib: &Library, filter: &str) -> Vec<String> {
    let sql = format!(
        "SELECT c.ID FROM djmdContent c LEFT JOIN djmdGenre g ON g.ID = c.GenreID \
         WHERE c.rb_local_deleted = 0 AND ({filter}) ORDER BY c.rowid"
    );
    let mut stmt = lib.connection().prepare(&sql).unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect()
}

fn smart_row(lib: &Library, id: &str) -> (i64, Option<String>, i64) {
    lib.connection()
        .query_row(
            "SELECT Attribute, SmartList, rb_local_usn FROM djmdPlaylist WHERE ID = ?1 AND rb_local_deleted = 0",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

fn evaluated(dir: &TempLibrary, id: &DbId) -> Vec<String> {
    let snap = dir.verify_loads();
    let all = smartlist::evaluate_all(&snap, Clock::utc_now());
    all.get(id).unwrap().iter().map(|d| d.0.clone()).collect()
}

#[test]
fn create_evaluate_edit_undo() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let mut history = History::default();
    let conditions = vec![
        Condition::new("bpm", op::IN_RANGE, "", "120", "130"),
        Condition::new("genre", op::CONTAINS, "", "house", ""),
    ];
    let out = history
        .run(
            &mut lib,
            Command::CreateSmartList { parent: None, name: "Smart".into(), logical_operator: MATCH_ALL, conditions: conditions.clone() },
        )
        .unwrap();
    let Effect::PlaylistCreated { id } = out.effect.clone() else { panic!("{:?}", out.effect) };

    // The row: Attribute 4, the dialog's XML with this list's ID, written by the last USN.
    let (attribute, xml, usn) = smart_row(&lib, id.as_str());
    assert_eq!(attribute, 4);
    let want = SmartList::new(id.as_str(), MATCH_ALL, conditions).to_xml();
    assert_eq!(xml.as_deref(), Some(want.as_str()));
    assert_eq!(usn, out.usn_after);
    assert!(xml.unwrap().starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<NODE Id=\""));
    // On top, with a cloud filter row and a masterPlaylists6.xml node (Attribute 4).
    assert_eq!(playlist::children(lib.connection(), playlist::ROOT, None).unwrap()[0].id, id.0);
    let uuid: String = lib.connection().query_row("SELECT UUID FROM djmdPlaylist WHERE ID = ?1", [id.as_str()], |r| r.get(0)).unwrap();
    let filter: Option<String> = lib
        .connection()
        .query_row("SELECT ID FROM djmdCloudFilterPlaylist WHERE PlaylistUUID = ?1 AND rb_local_deleted = 0", [&uuid], |r| r.get(0))
        .optional()
        .unwrap();
    assert!(filter.is_some());
    let doc = MasterPlaylists::read(&dir.xml_path()).unwrap().unwrap();
    let node = doc.nodes.iter().find(|n| n.id == id.as_str().parse::<u64>().unwrap()).unwrap();
    assert_eq!(node.attribute, 4);
    // No entries are stored.
    let entries: i64 = lib
        .connection()
        .query_row("SELECT count(*) FROM djmdSongPlaylist WHERE PlaylistID = ?1", [id.as_str()], |r| r.get(0))
        .unwrap();
    assert_eq!(entries, 0);

    // Evaluation through the snapshot loader.
    let house = expected(&lib, "c.BPM BETWEEN 12000 AND 13000 AND lower(g.Name) LIKE '%house%'");
    assert!(!house.is_empty());
    assert_eq!(evaluated(&dir, &id), house);

    // New rules (any of two), then undo back to the first ones.
    let any = SmartList::new(
        id.as_str(),
        MATCH_ANY,
        vec![
            Condition::new("genre", op::IS, "", "techno", ""),
            Condition::new("genre", op::STARTS_WITH, "", "psy", ""),
        ],
    );
    let out = history.run(&mut lib, Command::SetSmartList { id: id.clone(), xml: Some(any.to_xml()) }).unwrap();
    assert_eq!(out.usn_spent(), 1);
    assert_eq!(evaluated(&dir, &id), expected(&lib, "lower(g.Name) = 'techno' OR lower(g.Name) LIKE 'psy%'"));
    let same = lib.execute(Command::SetSmartList { id: id.clone(), xml: Some(any.to_xml()) }).unwrap();
    assert_eq!(same.effect, Effect::Unchanged);
    history.undo(&mut lib).unwrap();
    assert_eq!(smart_row(&lib, id.as_str()).1.as_deref(), Some(want.as_str()));

    // Rename, move into a folder, delete and restore keep the rules.
    lib.execute(Command::RenamePlaylist { id: id.clone(), name: "Smart 2".into() }).unwrap();
    let folder = match lib.create_playlist(None, "F", mb_library::NodeKind::Folder).unwrap().effect {
        Effect::PlaylistCreated { id } => id,
        e => panic!("{e:?}"),
    };
    lib.execute(Command::MovePlaylist { id: id.clone(), parent: Some(folder.clone()), position: 1 }).unwrap();
    let out = history.run(&mut lib, Command::DeletePlaylist { id: id.clone() }).unwrap();
    assert!(out.inverse.is_some());
    history.undo(&mut lib).unwrap();
    let restored = playlist::children(lib.connection(), folder.as_str(), None).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].name, "Smart 2");
    assert_eq!(restored[0].attribute, 4);
    assert_eq!(restored[0].smart_list.as_deref(), Some(want.as_str()));
    dir.verify_loads();
}

#[test]
fn set_smart_list_refuses_other_nodes() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let before = lib.usn().unwrap();
    let r = lib.execute(Command::SetSmartList { id: DbId("1036096362".into()), xml: Some("<NODE/>".into()) });
    assert!(r.is_err());
    assert_eq!(lib.usn().unwrap(), before);
}
