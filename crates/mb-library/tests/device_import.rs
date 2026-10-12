//! Import from a device (`docs/re/ops/import-device.md`) on fixture copies. Skipped without
//! `fixtures/` (and, for the track import, without the corpus audio in `~/re-work/corpus`).
//! Devices are APFS clones in temp dirs; `fixtures/` is never written.

mod common;

use common::{TempLibrary, fixture};
use mb_anlz::AnlzFile;
use mb_anlz::tags::TypedSection;
use mb_library::device_history::{DeviceHistoryEntry, history_hash};
use mb_library::device_import::{DeviceImportKind, device_tracks};
use mb_library::{Command, DeviceHistory, DeviceImport, Effect, ImportedHistory, Library, LocalTime};
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use mb_pdb::{ExportDb, Pdb};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("mb-devimport-{tag}-{}-{}", std::process::id(), mb_masterdb::write::new_rid32().unwrap()));
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_pdb(path: &Path) -> ExportDb {
    ExportDb::read(&Pdb::parse(std::fs::read(path).unwrap()).unwrap()).unwrap()
}

/// oracle-a: local content ID → (FolderPath, absolute local DAT).
fn oracle_tracks() -> HashMap<String, (PathBuf, Option<PathBuf>)> {
    let root = fixture("oracle-a").unwrap();
    let mut paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    paths.analysis_data_root = root.join("rekordbox/share");
    let db = MasterDb::open_read_only(&root.join("rekordbox/master.db"), &derive_passphrase(&paths.dp).unwrap()).unwrap();
    db.load_snapshot(&paths).unwrap().tracks.into_iter().map(|t| (t.id.0, (t.path, t.analysis_path))).collect()
}

fn normalized(f: &AnlzFile) -> Vec<TypedSection> {
    f.typed_sections()
        .into_iter()
        .map(|mut s| {
            match &mut s {
                TypedSection::SongStructure(p) => p.scrambled = false,
                TypedSection::VocalDetect(v) => v.version &= 0x7fff,
                _ => {}
            }
            s
        })
        .collect()
}

/// `to_local(to_usb(local))` gives the local file back: rekordbox's own USB export of oracle-a
/// (`rbtest`) converted back and compared with oracle-a's local analysis files.
#[test]
fn to_local_reverses_rekordboxs_usb_files() {
    let Some(oracle) = fixture("oracle-a") else {
        eprintln!("skipped: no fixtures/oracle-a");
        return;
    };
    let rbtest = oracle.join("rbtest");
    let db = read_pdb(&rbtest.join("PIONEER/rekordbox/export.pdb"));
    let local = oracle_tracks();
    let (mut dat_exact, mut ext_exact, mut ext_same_after_unscramble, mut pairs) = (0, 0, 0, 0);
    let mut mismatches = Vec::new();
    for t in &db.tracks {
        let Some((_, Some(local_dat))) = local.get(&t.master_content_id.to_string()) else { continue };
        let device_dat = rbtest.join(t.analysis_path.trim_start_matches('/'));
        // The name the import gives the copied file: the device file name. The local PPTH holds
        // the original name; only compare where both are the same.
        let local_file = AnlzFile::parse(&std::fs::read(local_dat).unwrap()).unwrap();
        let device_name = Path::new(&t.folder_path).file_name().unwrap().to_string_lossy().into_owned();
        if local_file.path().as_deref() != Some(&format!("?/{device_name}")) {
            continue;
        }
        pairs += 1;
        let dev = AnlzFile::parse(&std::fs::read(&device_dat).unwrap()).unwrap();
        if mb_anlz::local::to_local(&dev, &device_name).to_bytes() == local_file.to_bytes() {
            dat_exact += 1;
        } else {
            mismatches.push(format!("DAT {}", t.folder_path));
        }
        let (le, de) = (local_dat.with_extension("EXT"), device_dat.with_extension("EXT"));
        if le.is_file() && de.is_file() {
            let lf = AnlzFile::parse(&std::fs::read(&le).unwrap()).unwrap();
            let conv = mb_anlz::local::to_local(&AnlzFile::parse(&std::fs::read(&de).unwrap()).unwrap(), &device_name);
            if conv.to_bytes() == lf.to_bytes() {
                ext_exact += 1;
            } else if normalized(&conv) == normalized(&lf) {
                ext_same_after_unscramble += 1;
            } else {
                mismatches.push(format!("EXT {}", t.folder_path));
            }
        }
    }
    eprintln!(
        "pairs {pairs}: DAT exact {dat_exact}; EXT exact {ext_exact}, equal once PSSI/PVDI are unscrambled {ext_same_after_unscramble}; mismatches {mismatches:?}"
    );
    assert!(pairs > 100, "too few comparable pairs: {pairs}");
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

/// Clone of rekordbox's RBTEST export with the audio of `list` put back under `/Contents` (from
/// the corpus, APFS clones). `None` when the corpus is missing.
fn device_with_audio(dir: &Path, list: &str) -> Option<(PathBuf, ExportDb, Vec<u32>)> {
    let rbtest = fixture("oracle-a")?.join("rbtest");
    let root = dir.join("device");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("/bin/cp").arg("-cR").arg(rbtest.join("PIONEER")).arg(root.join("PIONEER")).status().unwrap().success());
    let db = read_pdb(&root.join("PIONEER/rekordbox/export.pdb"));
    let list_id = db.playlists.iter().find(|p| p.0.name == list)?.0.id;
    let mut entries: Vec<_> = db.playlist_entries.iter().filter(|e| e.playlist_id == list_id).collect();
    entries.sort_by_key(|e| e.entry_index);
    let local = oracle_tracks();
    let mut ids = Vec::new();
    for e in entries {
        let t = db.tracks.iter().find(|t| t.id == e.track_id)?;
        let (src, _) = local.get(&t.master_content_id.to_string())?;
        if !src.is_file() {
            return None;
        }
        let dest = root.join(t.folder_path.trim_start_matches('/'));
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        assert!(std::process::Command::new("/bin/cp").arg("-c").arg(src).arg(&dest).status().unwrap().success());
        ids.push(t.id);
    }
    Some((root, db, ids))
}

#[test]
fn import_a_playlist_from_rekordboxs_export() {
    let Some(lib_dir) = TempLibrary::new("sandbox-initial") else {
        eprintln!("skipped: no fixtures/sandbox-initial");
        return;
    };
    let tmp = TempDir::new("tracks");
    let Some((root, db, ids)) = device_with_audio(&tmp.0, "oracle-small") else {
        eprintln!("skipped: fixtures/oracle-a or the corpus audio is missing");
        return;
    };
    let analysis_root = tmp.0.join("share");
    let mut lib = Library::open_unguarded(&lib_dir.db, &lib_dir.passphrase).unwrap();
    lib.set_analysis_root(&analysis_root);
    let all = device_tracks(&root, &db);
    let tracks: Vec<_> = ids.iter().map(|id| all[id].clone()).collect();
    let req = DeviceImport { tracks: tracks.clone(), copy_to: tmp.0.join("Imported"), playlist: Some("oracle-small".into()) };
    let out = lib.execute(Command::ImportFromDevice { import: req.clone() }).unwrap();
    let Effect::ImportedFromDevice(report) = &out.effect else { panic!("{:?}", out.effect) };
    assert_eq!(report.results.len(), 5, "{report:?}");
    let dbid: u32 = lib.connection().query_row("SELECT DBID FROM djmdProperty", [], |r| r.get::<_, String>(0)).unwrap().parse().unwrap();

    // Compare every imported row with rekordbox's own row for the same file (oracle-a).
    let oracle = common::open_read_only("oracle-a").unwrap();
    let cols = "Title, BPM, Length, BitRate, BitDepth, FileType, FileSize, SampleRate, Analysed, \
                (SELECT Name FROM djmdArtist a WHERE a.ID = c.ArtistID), (SELECT ScaleName FROM djmdKey k WHERE k.ID = c.KeyID)";
    for (r, t) in report.results.iter().zip(&tracks) {
        let DeviceImportKind::Imported { copied_to } = &r.kind else { panic!("{r:?}") };
        assert!(copied_to.is_file());
        assert_eq!(r.stamp, Some((dbid, r.local_id.0.parse().unwrap())));
        let ours: Vec<String> = row(lib.connection(), cols, &r.local_id.0);
        let theirs: Vec<String> = row(oracle.connection(), cols, &t.master_content_id.to_string());
        assert_eq!(ours, theirs, "{}", t.title);
        let (adp, master): (String, String) = lib
            .connection()
            .query_row("SELECT AnalysisDataPath, MasterSongID FROM djmdContent WHERE ID = ?1", [&r.local_id.0], |x| Ok((x.get(0)?, x.get(1)?)))
            .unwrap();
        assert_eq!(master, r.local_id.0);
        let dat = analysis_root.join(adp.trim_start_matches('/'));
        assert!(dat.is_file() && dat.with_extension("EXT").is_file() && dat.with_extension("2EX").is_file());
        let f = AnlzFile::parse(&std::fs::read(&dat).unwrap()).unwrap();
        assert_eq!(f.path().unwrap(), format!("?/{}", copied_to.file_name().unwrap().to_string_lossy()));
    }
    // The playlist holds the five tracks in device order.
    let p = report.playlist.clone().unwrap();
    let mut stmt = lib.connection().prepare("SELECT ContentID FROM djmdSongPlaylist WHERE PlaylistID = ?1 AND rb_local_deleted = 0 ORDER BY TrackNo").unwrap();
    let entries: Vec<String> = stmt.query_map([&p.0], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    drop(stmt);
    assert_eq!(entries, report.results.iter().map(|r| r.local_id.0.clone()).collect::<Vec<_>>());
    lib_dir.verify_loads();

    // The stamp goes onto both device databases; then the tracks are found again.
    let dev = mb_usb::DeviceState::load(&root, Some(false)).unwrap();
    let stamps: Vec<(u32, u32, u32)> =
        report.results.iter().map(|r| (r.device_id, r.stamp.unwrap().0, r.stamp.unwrap().1)).collect();
    mb_usb::stamp::stamp_master_ids(&dev, &stamps).unwrap();
    let pdb_bytes = std::fs::read(root.join("PIONEER/rekordbox/export.pdb")).unwrap();
    assert_eq!(mb_pdb::lint::lint(&pdb_bytes, mb_pdb::schema::DbKind::Export).errors(), 0);
    let db2 = read_pdb(&root.join("PIONEER/rekordbox/export.pdb"));
    let devlib = mb_devlib::DevLibDb::open_immutable(&root.join("PIONEER/rekordbox/exportLibrary.db")).unwrap();
    for (id, dbid, cid) in &stamps {
        let t = db2.tracks.iter().find(|t| t.id == *id).unwrap();
        assert_eq!((t.master_db_id, t.master_content_id), (*dbid, *cid));
        let plus: (i64, i64) = devlib
            .connection()
            .query_row("SELECT masterDbId, masterContentId FROM content WHERE path = ?1", [&t.folder_path], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(plus, (i64::from(*dbid), i64::from(*cid)));
    }
    assert_eq!(db2.tracks.len(), db.tracks.len());
    let all2 = device_tracks(&root, &db2);
    let stamped: Vec<_> = ids.iter().map(|id| all2[id].clone()).collect();
    let again = lib
        .execute(Command::ImportFromDevice { import: DeviceImport { tracks: stamped, playlist: None, ..req } })
        .unwrap();
    let Effect::ImportedFromDevice(r2) = &again.effect else { panic!() };
    assert!(r2.results.iter().all(|r| r.kind == DeviceImportKind::InCollection && r.stamp.is_none()));

    // Undo removes the rows, the playlist and the files.
    lib.execute(out.inverse.clone().unwrap()).unwrap();
    for r in &report.results {
        let n: i64 = lib.connection().query_row("SELECT count(*) FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [&r.local_id.0], |x| x.get(0)).unwrap();
        assert_eq!(n, 0);
        let DeviceImportKind::Imported { copied_to } = &r.kind else { unreachable!() };
        assert!(!copied_to.exists());
    }
    lib_dir.verify_loads();
}

fn row(conn: &mb_masterdb::rusqlite::Connection, cols: &str, id: &str) -> Vec<String> {
    use mb_masterdb::rusqlite::types::Value;
    let sql = format!("SELECT {cols} FROM djmdContent c WHERE c.ID = ?1");
    conn.query_row(&sql, [id], |r| {
        Ok((0..r.as_ref().column_count())
            .map(|i| match r.get::<_, Value>(i).unwrap() {
                Value::Null => "NULL".into(),
                Value::Integer(v) => v.to_string(),
                Value::Real(v) => v.to_string(),
                Value::Text(s) => s,
                Value::Blob(_) => "blob".into(),
            })
            .collect())
    })
    .unwrap()
}

fn now() -> LocalTime {
    LocalTime { year: 2026, month: 10, day: 12, hour: 9, minute: 30, second: 5 }
}

#[test]
fn import_histories_into_the_library_that_exported_the_tracks() {
    let Some(lib_dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: no fixtures/oracle-a");
        return;
    };
    let db = read_pdb(&fixture("oracle-a").unwrap().join("rbtest/PIONEER/rekordbox/export.pdb"));
    let mut lib = Library::open_unguarded(&lib_dir.db, &lib_dir.passphrase).unwrap();
    // Two player histories built from the stick's own track rows (masterDbId = this library).
    let entry = |t: &mb_pdb::tables::Track| DeviceHistoryEntry {
        device_content_id: t.id,
        master_db_id: t.master_db_id,
        master_content_id: t.master_content_id,
        title: t.title.clone(),
    };
    let h1 = DeviceHistory { id: 1, plus: false, name: "HISTORY 001".into(), entries: db.tracks[..3].iter().map(entry).chain([entry(&db.tracks[0])]).collect() };
    let h2 = DeviceHistory { id: 2, plus: false, name: "HISTORY 002".into(), entries: db.tracks[3..5].iter().map(entry).collect() };
    let first = db.tracks[0].master_content_id.to_string();
    let plays_before: i64 = lib.connection().query_row("SELECT DJPlayCount FROM djmdContent WHERE ID = ?1", [&first], |r| r.get(0)).unwrap();

    let out = lib
        .execute(Command::ImportHistories { histories: vec![h1.clone(), h2.clone()], already: ImportedHistory::default(), now: now() })
        .unwrap();
    let Effect::HistoriesImported(report) = &out.effect else { panic!() };
    assert_eq!(report.imported.len(), 2);
    fn q(lib: &Library, sql: &str) -> Vec<(String, String, i64, String)> {
        let mut stmt = lib.connection().prepare(sql).unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
    }
    let rows = q(&lib, "SELECT ID, Name, Attribute, ParentID FROM djmdHistory WHERE rb_local_deleted = 0 ORDER BY rowid");
    assert_eq!(&rows[0], &("2026".into(), "2026".into(), 1, "root".into()));
    assert_eq!(&rows[1], &("202610".into(), "10".into(), 1, "2026".into()));
    assert_eq!(rows[2].1, "HISTORY 2026-10-12");
    assert_eq!(rows[3].1, "HISTORY 2026-10-12 (1)");
    assert!(rows[2..].iter().all(|r| r.2 == 0 && r.3 == "202610" && r.0.parse::<u64>().unwrap() >= 3000));
    let n: i64 = lib.connection().query_row("SELECT count(*) FROM djmdSongHistory WHERE HistoryID = ?1", [&rows[2].0], |r| r.get(0)).unwrap();
    assert_eq!(n, 4);
    let plays: i64 = lib.connection().query_row("SELECT DJPlayCount FROM djmdContent WHERE ID = ?1", [&first], |r| r.get(0)).unwrap();
    assert_eq!(plays, plays_before + 2);
    let hash = history_hash(&h2.entries.iter().map(|e| (e.master_db_id, e.master_content_id)).collect::<Vec<_>>());
    assert_eq!(report.imported[1].3, hash);

    // Recorded in ImportedHistory.xml, the same histories are skipped next time.
    let mut ih = ImportedHistory::default();
    for (id, plus, _, hash) in &report.imported {
        ih.add(*plus, *id, hash.clone());
    }
    let again = lib
        .execute(Command::ImportHistories { histories: vec![h1, h2], already: ImportedHistory::parse(&ih.to_xml()), now: now() })
        .unwrap();
    let Effect::HistoriesImported(r2) = &again.effect else { panic!() };
    assert!(r2.imported.is_empty() && r2.skipped == vec![1, 2]);

    lib.execute(out.inverse.clone().unwrap()).unwrap();
    let plays: i64 = lib.connection().query_row("SELECT DJPlayCount FROM djmdContent WHERE ID = ?1", [&first], |r| r.get(0)).unwrap();
    assert_eq!(plays, plays_before);
    assert_eq!(q(&lib, "SELECT ID, Name, Attribute, ParentID FROM djmdHistory WHERE rb_local_deleted = 0 AND Attribute = 0").len(), 0);
    lib_dir.verify_loads();
}

/// The reference stick's 21 player histories belong to another library: none of their tracks is
/// in this collection, so nothing is imported (rekordbox imports histories, not tracks).
#[test]
fn histories_of_a_foreign_stick_are_skipped() {
    let (Some(stick), Some(lib_dir)) = (fixture("stick").or_else(|| {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stick");
        p.join("PIONEER/rekordbox/export.pdb").is_file().then_some(p)
    }), TempLibrary::new("oracle-a")) else {
        eprintln!("skipped: no fixtures/stick or fixtures/oracle-a");
        return;
    };
    let db = read_pdb(&stick.join("PIONEER/rekordbox/export.pdb"));
    let histories = mb_library::device_history::device_histories(&db);
    assert_eq!(histories.len(), 21);
    assert!(histories.iter().all(|h| !h.entries.is_empty()));
    let mut lib = Library::open_unguarded(&lib_dir.db, &lib_dir.passphrase).unwrap();
    let usn = lib.usn().unwrap();
    let out = lib.execute(Command::ImportHistories { histories, already: ImportedHistory::default(), now: now() }).unwrap();
    let Effect::HistoriesImported(r) = &out.effect else { panic!() };
    assert!(r.imported.is_empty() && r.skipped.len() == 21);
    assert_eq!(lib.usn().unwrap(), usn);
}
