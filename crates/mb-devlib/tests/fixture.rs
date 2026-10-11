//! Reads the stick fixture (skipped when fixtures are absent).
//! The database is copied to a temp dir first: a WAL database grows `-shm`/`-wal` files when
//! opened, and the fixture must stay byte-identical to the stick.

use mb_devlib::{CueKind, DevLibDb, ListAttribute};
use std::path::{Path, PathBuf};

fn stick() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stick");
    root.join("PIONEER/rekordbox/exportLibrary.db")
        .exists()
        .then_some(root)
}

/// A copy of the fixture database in a fresh temp directory, removed again on drop.
/// Declare it before the connection so the connection closes first.
struct TempDb {
    dir: PathBuf,
    path: PathBuf,
}

impl TempDb {
    fn new(root: &Path, tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mb-devlib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("exportLibrary.db");
        std::fs::copy(root.join("PIONEER/rekordbox/exportLibrary.db"), &path).unwrap();
        Self { dir, path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn opens_and_passes_quick_check() {
    let Some(root) = stick() else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let tmp = TempDb::new(&root, "check");
    let db = DevLibDb::open_read_only(&tmp.path).unwrap();
    let check: String = db
        .connection()
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    // SQLCipher answers these as text. All are SQLCipher 4 defaults.
    let pragma = |name: &str| -> String {
        db.connection()
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(pragma("cipher_page_size"), "4096");
    assert_eq!(pragma("kdf_iter"), "256000");
    assert_eq!(pragma("cipher_hmac_algorithm"), "HMAC_SHA512");
    assert_eq!(pragma("cipher_kdf_algorithm"), "PBKDF2_HMAC_SHA512");
    assert_eq!(pragma("journal_mode"), "wal");
    let counts = db.table_counts().unwrap();
    assert_eq!(counts.len(), 22);
    assert_eq!(counts[0].0, "content", "creation order starts with content");
    let count = |name: &str| counts.iter().find(|(n, _)| n == name).unwrap().1;
    assert_eq!(count("content"), 704);
    assert_eq!(count("playlist"), 12);
    assert_eq!(count("playlist_content"), 981);
    assert_eq!(count("cue"), 0);
}

#[test]
fn wrong_passphrase_fails() {
    let Some(root) = stick() else { return };
    let tmp = TempDb::new(&root, "wrong");
    assert!(DevLibDb::open_read_only_with(&tmp.path, "not the key").is_err());
}

#[test]
fn immutable_open_creates_no_side_files() {
    let Some(root) = stick() else { return };
    let tmp = TempDb::new(&root, "immutable");
    let db = DevLibDb::open_immutable(&tmp.path).unwrap();
    assert_eq!(db.tracks().unwrap().len(), 704);
    drop(db);
    let files: Vec<_> = std::fs::read_dir(&tmp.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
}

#[test]
fn library_contents() {
    let Some(root) = stick() else { return };
    let tmp = TempDb::new(&root, "library");
    let db = DevLibDb::open_read_only(&tmp.path).unwrap();
    let lib = db.load_library().unwrap();

    let prop = lib.property.as_ref().unwrap();
    assert_eq!(prop.device_name, "REKORDBOX");
    assert_eq!(prop.db_version, "1000");
    assert_eq!(prop.number_of_contents, 704);

    assert_eq!(lib.tracks.len(), 704);
    let t = &lib.tracks[0];
    assert_eq!(t.id, 1);
    assert_eq!(t.title, "The Underdog (Original Mix)");
    assert_eq!(t.artist, "Eddie Bitar");
    assert_eq!(t.album, "The Underdog");
    assert_eq!(t.genre, "Psy-Trance");
    assert_eq!(t.label, "Blue Tunes Records");
    assert_eq!(t.key, "Gmin");
    assert_eq!(t.bpm_x100, Some(13800));
    assert_eq!(t.length_seconds, Some(396));
    assert_eq!(t.file_type, 12);
    assert_eq!(t.bitrate, Some(1411));
    assert_eq!(t.sample_rate, Some(44100));
    assert_eq!(t.comment, "6A");
    assert_eq!(t.isrc, "DEGA22003230");
    assert_eq!(t.date_added, "2020-08-30");
    assert_eq!(
        t.path,
        "/Contents/Eddie Bitar/The Underdog/Eddie Bitar - The Underdog (Original Mix).aiff"
    );
    assert_eq!(
        t.analysis_path,
        "/PIONEER/USBANLZ/P004/00008C70/ANLZ0000.DAT"
    );
    assert_eq!(
        t.artwork_path.as_deref(),
        Some("/PIONEER/Artwork/00001/b1.jpg")
    );
    assert_eq!(t.master_db_id, Some(2328226057));
    assert_eq!(t.information_update_count, "", "stored as text ''");

    // Rows from the 7.x insert path leave unknown lookups NULL instead of 0.
    let funky = lib.tracks.iter().find(|t| t.id == 692).unwrap();
    assert_eq!(funky.title, "Funky Mushrooms");
    assert_eq!(funky.genre, "");
    assert_eq!(funky.ids.album, 0);
    assert_eq!(funky.key, "9A");

    let flac = lib.tracks.iter().find(|t| t.file_type == 5).unwrap();
    assert!(flac.path.ends_with(".flac"));

    assert_eq!(lib.playlists.len(), 12);
    assert!(
        lib.playlists
            .iter()
            .all(|p| p.attribute == ListAttribute::List && p.parent.is_none())
    );
    let u130 = lib
        .playlists
        .iter()
        .find(|p| p.name == "U 130 BPM")
        .unwrap();
    let entries = &lib.playlist_entries[&u130.id];
    assert_eq!(entries.len(), 197);
    assert_eq!(entries[0], 226);
    assert_eq!(
        lib.playlist_entries.values().map(Vec::len).sum::<usize>(),
        981
    );

    assert_eq!(lib.menu_items.len(), 27);
    assert_eq!(lib.menu_items[0].label(), "GENRE");
    assert_eq!(lib.colors.len(), 8);
    assert_eq!(lib.colors[0], (1, "Pink".to_string()));
    assert_eq!(lib.my_tags.len(), 28);
    assert_eq!(
        lib.my_tags
            .iter()
            .filter(|t| t.attribute == ListAttribute::Folder)
            .count(),
        4
    );
    assert!(
        lib.cues
            .iter()
            .all(|c| !matches!(c.kind, CueKind::Invalid(_)))
    );
}

#[test]
fn snapshot() {
    let Some(root) = stick() else { return };
    let tmp = TempDb::new(&root, "snapshot");
    let db = DevLibDb::open_read_only(&tmp.path).unwrap();
    let snap = db.load_snapshot(&root).unwrap();
    assert_eq!(snap.tracks.len(), 704);
    assert_eq!(snap.playlists.len(), 12);
    assert_eq!(
        snap.playlist_entries.values().map(Vec::len).sum::<usize>(),
        981
    );
    let t = snap.tracks.iter().find(|t| t.id.as_str() == "1").unwrap();
    assert_eq!(t.file_type, mb_core::FileType::Aiff);
    assert_eq!(t.bpm(), Some(138.0));
    let anlz = t.analysis_path.as_ref().unwrap();
    assert!(anlz.exists(), "{anlz:?}");
    assert!(t.artwork_path.as_ref().unwrap().exists());
}
