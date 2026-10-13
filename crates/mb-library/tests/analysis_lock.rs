//! "Analysis Lock On/Off" (`docs/re/ops/analyse-track.md` § Analysis lock): `Analysed` bit 0x80,
//! one row update per changed track, grid edits refused while locked, undo.

mod common;
use common::*;
use mb_library::{Command, DbId, History, Library, grid};


fn analysed(lib: &Library, id: &str) -> i64 {
    lib.connection().query_row("SELECT Analysed FROM djmdContent WHERE ID = ?1", [id], |r| r.get(0)).unwrap()
}

#[test]
fn lock_unlock_undo() {
    let Some(dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(&dir.paths.analysis_data_root);
    let ids: Vec<String> = lib
        .connection()
        .prepare("SELECT ID FROM djmdContent WHERE rb_local_deleted = 0 AND Analysed = 105 ORDER BY ID LIMIT 2")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let ids: Vec<DbId> = ids.into_iter().map(DbId).collect();
    let mut history = History::default();

    let out = history.run(&mut lib, Command::SetAnalysisLock { ids: ids.clone(), locked: true }).unwrap();
    assert_eq!(out.usn_after - out.usn_before, 2, "one row update per track");
    for id in &ids {
        assert_eq!(analysed(&lib, &id.0), 105 | 0x80);
    }
    // Locking again changes nothing.
    let again = lib.execute(Command::SetAnalysisLock { ids: ids.clone(), locked: true }).unwrap();
    assert_eq!(again.usn_after, again.usn_before);
    assert!(again.inverse.is_none());

    // A locked track's grid is not edited.
    let beats = vec![mb_library::Beat { beat_in_bar: 1, tempo: 12800, time_ms: 100 }, mb_library::Beat { beat_in_bar: 2, tempo: 12800, time_ms: 569 }];
    let err = lib.execute(Command::SetBeatGrid { content: ids[0].clone(), beats: grid::shift(&beats, 1, 10_000) }).unwrap_err();
    assert!(err.to_string().contains("Analysis Lock"), "{err}");

    history.undo(&mut lib).unwrap();
    for id in &ids {
        assert_eq!(analysed(&lib, &id.0), 105);
    }
    dir.verify_loads();
}
