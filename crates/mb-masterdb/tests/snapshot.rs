//! Loads the sandbox fixture (skipped when fixtures are absent).

use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::Path;

#[test]
fn sandbox_initial_snapshot() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sandbox-initial");
    if !root.exists() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let mut paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    paths.analysis_data_root = root.join("rekordbox/share");
    let passphrase = derive_passphrase(&paths.dp).unwrap();
    let db = MasterDb::open_read_only(&root.join("rekordbox/master.db"), &passphrase).unwrap();
    let snap = db.load_snapshot(&paths).unwrap();
    assert_eq!(snap.tracks.len(), 14);
    let demo = snap.tracks.iter().find(|t| t.title == "Demo Track 1").expect("demo track");
    assert_eq!(demo.bpm_x100, Some(12800));
    assert!(demo.analysis_path.as_ref().unwrap().exists(), "{:?}", demo.analysis_path);
    assert_eq!(snap.playlists.len(), 1);
}
