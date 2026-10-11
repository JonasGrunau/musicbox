//! `masterPlaylists6.xml` parse → write is byte-exact on every fixture file, and the parsed nodes
//! match the `djmdPlaylist` rows of the same library (skipped without fixtures).

use mb_masterdb::{LibraryPaths, MasterDb, MasterPlaylists, derive_passphrase};
use std::path::Path;

#[test]
fn fixture_files_round_trip_byte_exact() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut checked = 0;
    for name in ["sandbox-initial", "oracle-a", "oracle-b"] {
        let dir = root.join(name);
        let file = dir.join("rekordbox/masterPlaylists6.xml");
        if !file.exists() {
            continue;
        }
        let bytes = std::fs::read(&file).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        let doc = MasterPlaylists::parse(&text).unwrap();
        assert_eq!(doc.to_xml().as_bytes(), bytes.as_slice(), "{name}: byte-exact round trip");
        assert!(text.contains("\r\n") && !text.replace("\r\n", "").contains('\n'), "{name}: CRLF only");

        // Every live playlist has a node with its ID in hex, in Seq order (all are top level).
        let paths = LibraryPaths::from_options_json(&dir.join("options.json")).unwrap();
        let db = MasterDb::open_read_only(&dir.join("rekordbox/master.db"), &derive_passphrase(&paths.dp).unwrap()).unwrap();
        let mut stmt = db
            .connection()
            .prepare("SELECT ID, Attribute FROM djmdPlaylist WHERE rb_local_deleted = 0 AND ParentID = 'root' ORDER BY Seq")
            .unwrap();
        let rows: Vec<(u64, i64)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?.parse().unwrap(), r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let nodes: Vec<(u64, i64)> = doc.nodes.iter().map(|n| (n.id, n.attribute)).collect();
        assert_eq!(nodes, rows, "{name}: nodes = playlists in Seq order");
        assert!(doc.nodes.iter().all(|n| n.parent_id == 0 && n.lib_type == 0 && n.check_type == 0));
        checked += 1;
    }
    if checked == 0 {
        eprintln!("no fixtures, skipping");
    }
}

#[test]
fn write_is_atomic_and_round_trips() {
    let dir = std::env::temp_dir().join(format!("mb-playlists-xml-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(mb_masterdb::playlists_xml::FILE_NAME);
    let mut doc = MasterPlaylists::default();
    assert_eq!(MasterPlaylists::read(&path).unwrap(), None);
    doc.nodes.push(mb_masterdb::playlists_xml::PlaylistXmlNode::new(200000, 0, 0, 1791739907631));
    doc.write(&path).unwrap();
    assert_eq!(MasterPlaylists::read(&path).unwrap(), Some(doc));
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left.len(), 1, "no temporary file left behind: {left:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
