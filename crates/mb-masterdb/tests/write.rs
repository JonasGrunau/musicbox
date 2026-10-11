//! WriteTxn bookkeeping on a temp copy of the oracle-a library (skipped without fixtures).
//! Expected values follow docs/re/ops/common.md.

use mb_masterdb::write::{new_rid32, timestamp};
use mb_masterdb::{LibraryPaths, MasterDbWriter, derive_passphrase};
use rusqlite::types::Value;
use std::path::{Path, PathBuf};

fn temp_library() -> Option<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !root.exists() {
        return None;
    }
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    let dir = std::env::temp_dir().join(format!("mb-write-test-{}-{}", std::process::id(), new_rid32().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(root.join("rekordbox/master.db"), dir.join("master.db")).unwrap();
    Some((dir, derive_passphrase(&paths.dp).unwrap()))
}

fn counter(w: &MasterDbWriter) -> i64 {
    w.connection()
        .query_row("SELECT int_1 FROM agentRegistry WHERE registry_id = 'localUpdateCount'", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn insert_update_delete_and_orphans_follow_rekordbox_rules() {
    let Some((dir, passphrase)) = temp_library() else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let mut w = MasterDbWriter::open_unguarded(&dir.join("master.db"), &passphrase).unwrap();
    let start = counter(&w);
    let id = new_rid32().unwrap().to_string();
    {
        let tx = w.transaction().unwrap();
        let usn = tx
            .insert("djmdArtist", &[
                ("ID", Value::Text(id.clone())),
                ("Name", Value::Text("musicbox test artist".into())),
                ("UUID", Value::Text(mb_masterdb::write::new_uuid())),
            ])
            .unwrap();
        assert_eq!(usn, start + 1, "increment first, then write");
        let (status, deleted, synced, row_usn, created, updated, cloud_usn): (i64, i64, i64, i64, String, String, Option<i64>) = tx
            .connection()
            .query_row(
                "SELECT rb_data_status, rb_local_deleted, rb_local_synced, rb_local_usn, created_at, updated_at, usn FROM djmdArtist WHERE ID = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .unwrap();
        assert_eq!((status, deleted, synced, row_usn), (0, 0, 0, start + 1));
        assert_eq!(created, updated);
        assert!(created.ends_with(" +00:00") && created.len() == timestamp().len());
        assert_eq!(cloud_usn, None, "usn is never written");

        let usn = tx.update("djmdArtist", "ID", Value::Text(id.clone()), &[("Name", Value::Text("renamed".into()))]).unwrap();
        assert_eq!(usn, start + 2);

        // The artist is referenced by no content row, so cleanup hard-deletes it (never synced).
        tx.cleanup_orphans().unwrap();
        let left: i64 = tx.connection().query_row("SELECT count(*) FROM djmdArtist WHERE ID = ?1", [&id], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
        tx.commit().unwrap();
    }
    assert_eq!(counter(&w), start + 3, "insert, update and one delete statement pair = 3 bumps");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rollback_on_drop_leaves_counter_unchanged() {
    let Some((dir, passphrase)) = temp_library() else {
        return;
    };
    let mut w = MasterDbWriter::open_unguarded(&dir.join("master.db"), &passphrase).unwrap();
    let start = counter(&w);
    {
        let tx = w.transaction().unwrap();
        tx.bump_usn().unwrap();
    }
    assert_eq!(counter(&w), start);
    std::fs::remove_dir_all(&dir).unwrap();
}
