//! Backup of a fixture library into a temp dir: the copy opens with the same passphrase and has
//! the same rows (skipped when fixtures are absent).

use mb_masterdb::{LibraryPaths, MasterDb, backup_library, derive_passphrase};
use std::path::Path;

#[test]
fn backup_round_trips_rows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !root.exists() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    let passphrase = derive_passphrase(&paths.dp).unwrap();
    let source = root.join("rekordbox/master.db");
    let tmp = std::env::temp_dir().join(format!("mb-backup-test-{}", std::process::id()));
    let dir = backup_library(&source, &passphrase, &tmp).unwrap();
    assert!(dir.join("masterPlaylists6.xml").is_file());
    let original = MasterDb::open_read_only(&source, &passphrase).unwrap().table_counts().unwrap();
    let copy = MasterDb::open_read_only(&dir.join("master.db"), &passphrase).unwrap().table_counts().unwrap();
    assert_eq!(original, copy);
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn guard_lists_no_foreign_processes() {
    // Only checks the function runs; whether rekordbox is running depends on the machine.
    for name in mb_masterdb::running_rekordbox() {
        assert!(mb_masterdb::REKORDBOX_PROCESSES.contains(&name.as_str()));
    }
}
