//! `playlists3.sync` / `playlists3Plus.sync` round trip on the reference stick's files
//! (`docs/re/export.md` § 4.4). Skipped without `fixtures/`.

use mb_usb::syncfile::{CLASSIC, PLUS, SyncFile};
use std::path::Path;

#[test]
fn reference_stick_files_round_trip() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stick/PIONEER/rekordbox");
    if !dir.is_dir() {
        eprintln!("skipped: no fixtures/stick");
        return;
    }
    for name in [CLASSIC, PLUS] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        let file = SyncFile::parse(&bytes).unwrap();
        assert_eq!(file.dbid, -1966741239, "{name}");
        assert_eq!(file.nodes.len(), 8, "{name}");
        assert_eq!(file.to_bytes(), bytes, "{name}");
    }
}
