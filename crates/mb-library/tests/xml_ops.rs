//! rekordbox.xml export and import (`docs/re/xml.md`, `docs/re/ops/import-xml.md`) on fixture
//! copies. Skipped without `fixtures/` (the import also needs the corpus audio in
//! `~/re-work/corpus`). Analysis folders that a test may write are APFS clones in temp dirs;
//! `fixtures/` is never written.

mod common;

use common::{TempLibrary, fixture};
use mb_anlz::AnlzFile;
use mb_library::xml::{XmlExportOptions, export_document, import_for_node};
use mb_library::{Command, CueSlot, Effect, History, Library, NewCue, TrackEdit, XmlTrackResult};
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use mb_xml::export::{KindLanguage, TempoMode};
use mb_xml::{Document, PlaylistNode, PositionMark};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// (Type, Start, End, Num, RGB, Name) of a POSITION_MARK.
type Mark = (u8, String, Option<String>, i32, Option<[u8; 3]>, String);

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("mb-xml-{tag}-{}-{}", std::process::id(), mb_masterdb::write::new_rid32().unwrap()));
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An APFS clone of oracle-a's analysis folder (writable, no extra space).
fn clone_share(into: &Path) -> PathBuf {
    let src = fixture("oracle-a").unwrap().join("rekordbox/share");
    let dst = into.join("share");
    let ok = std::process::Command::new("/bin/cp").arg("-cR").arg(&src).arg(&dst).status().unwrap().success();
    assert!(ok, "cp -c of the analysis folder");
    dst
}

fn snapshot(db: &Path, passphrase: &str, analysis_root: &Path) -> (MasterDb, mb_core::LibrarySnapshot) {
    let root = fixture("oracle-a").unwrap();
    let mut paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    paths.analysis_data_root = analysis_root.to_path_buf();
    paths.db_path = db.to_path_buf();
    let m = MasterDb::open_read_only(db, passphrase).unwrap();
    let s = m.load_snapshot(&paths).unwrap();
    (m, s)
}

fn oracle_export(lang: KindLanguage) -> Option<(Document, Document)> {
    let root = fixture("oracle-a")?;
    let reference = Document::parse(&std::fs::read_to_string(root.join("rekordbox.xml")).ok()?).unwrap();
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    let pass = derive_passphrase(&paths.dp).unwrap();
    let (db, snap) = snapshot(&root.join("rekordbox/master.db"), &pass, &root.join("rekordbox/share"));
    let opts = XmlExportOptions {
        product: reference.product.clone().unwrap(),
        language: lang,
        hot_cue_color_type: 4,
        tempo_mode: TempoMode::Changes,
    };
    Some((export_document(db.connection(), &snap, &opts).unwrap(), reference))
}

/// The writer rebuilds rekordbox's own export of oracle-a byte for byte, restricted to the 161
/// tracks that existed when rekordbox wrote it (8 sampler WAVs were added later).
#[test]
fn export_rebuilds_rekordboxs_file() {
    let Some((mut doc, reference)) = oracle_export(KindLanguage::German) else {
        eprintln!("skipped: no fixtures/oracle-a");
        return;
    };
    let ids: HashSet<i64> = reference.tracks().iter().filter_map(|t| t.track_id).collect();
    let all = doc.tracks().len();
    doc.collection.as_mut().unwrap().retain(|t| t.track_id.is_some_and(|id| ids.contains(&id)));
    assert_eq!((all, doc.tracks().len()), (169, 161));
    let ours = doc.to_xml();
    let theirs = std::fs::read_to_string(fixture("oracle-a").unwrap().join("rekordbox.xml")).unwrap();
    assert!(ours == theirs, "first difference at byte {}", ours.bytes().zip(theirs.bytes()).position(|(a, b)| a != b).unwrap_or(ours.len().min(theirs.len())));
}

/// Cues and colours set through `mb-library` come out as rekordbox's writer formats them.
#[test]
fn cues_and_colours_in_the_export() {
    let Some(tl) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: no fixtures/oracle-a");
        return;
    };
    let tmp = TempDir::new("cues");
    let share = clone_share(&tmp.0);
    let mut lib = Library::open_unguarded(&tl.db, &tl.passphrase).unwrap();
    lib.set_analysis_root(&share);
    let (_, snap) = snapshot(&tl.db, &tl.passphrase, &share);
    let id = snap.tracks.iter().find(|t| t.length_seconds.unwrap_or(0) > 20).unwrap().id.clone();
    let add = |lib: &mut Library, slot, in_ms, out_ms, code, comment: &str| {
        lib.execute(Command::AddCue {
            content: id.clone(),
            cue: NewCue { slot, in_ms, out_ms, color: None, color_table_index: code, comment: comment.into() },
        })
        .unwrap();
    };
    add(&mut lib, CueSlot::Memory, 1000, None, None, "memo");
    add(&mut lib, CueSlot::Memory, 2000, Some(4000), None, "");
    add(&mut lib, CueSlot::Hot(0), 3000, None, Some(5), "A");
    add(&mut lib, CueSlot::Hot(4), 5000, None, None, "");
    add(&mut lib, CueSlot::Hot(1), 6000, Some(7000), None, "");
    add(&mut lib, CueSlot::Hot(9), 8000, None, None, "J is not written");
    lib.edit_track(&id, TrackEdit::Color(3)).unwrap();
    drop(lib);

    let (db, snap) = snapshot(&tl.db, &tl.passphrase, &share);
    let doc = export_document(db.connection(), &snap, &XmlExportOptions::default()).unwrap();
    let x = doc.tracks().iter().find(|t| t.track_id == Some(mb_xml::export::xml_id(id.0.parse().unwrap()))).unwrap();
    assert_eq!(x.colour, Some(0xFFA500));
    assert_eq!(x.grouping.as_deref(), Some("Orange"));
    // rekordbox.xml uses the UI palette (`docs/re/xml.md`), not the device one.
    let pad = |code: u8| Some(mb_usb::cues::UI_PAD_COLORS[usize::from(code)]);
    let marks: Vec<Mark> = x
        .position_marks
        .iter()
        .map(|m: &PositionMark| (m.mark_type, mb_xml::model::fmt_seconds(m.start), m.end.map(mb_xml::model::fmt_seconds), m.num, m.rgb, m.name.clone().unwrap_or_default()))
        .collect();
    assert_eq!(
        marks,
        [
            (0, "1.000".into(), None, -1, None, "memo".into()),
            (4, "2.000".into(), Some("4.000".into()), -1, None, String::new()),
            (0, "3.000".into(), None, 0, pad(5), "A".into()),
            (0, "5.000".into(), None, 4, pad(mb_usb::cues::default_hot_cue_code(5, false, 4)), String::new()),
            (4, "6.000".into(), Some("7.000".into()), 1, pad(0x24), String::new()),
        ]
    );
    let text = doc.to_xml();
    assert!(text.contains("<POSITION_MARK Name=\"memo\" Type=\"0\" Start=\"1.000\" Num=\"-1\"/>"), "{}", &text[..200]);
}

/// The playlist `oracle-small` exported from oracle-a and imported into the initial sandbox:
/// rows like rekordbox's own import of the same files, the beat grid back from TEMPO, the list,
/// and undo.
#[test]
fn import_into_another_library() {
    let Some((doc, _)) = oracle_export(KindLanguage::English) else {
        eprintln!("skipped: no fixtures/oracle-a");
        return;
    };
    let Some(tl) = TempLibrary::new("sandbox-initial") else {
        eprintln!("skipped: no fixtures/sandbox-initial");
        return;
    };
    let node = match doc.playlists.as_deref() {
        Some([PlaylistNode::Folder { children, .. }]) => children.iter().find(|n| n.name() == "oracle-small").unwrap().clone(),
        other => panic!("{other:?}"),
    };
    let req = import_for_node(&doc, &node);
    assert_eq!(req.tracks.len(), 5);
    let audio: Vec<String> = req.tracks.iter().map(|t| mb_xml::export::location_from_uri(t.location.as_deref().unwrap()).unwrap()).collect();
    if !audio.iter().all(|p| Path::new(p).is_file()) {
        eprintln!("skipped: the corpus audio (~/re-work/corpus) is not present");
        return;
    }
    let tmp = TempDir::new("import");
    let mut lib = Library::open_unguarded(&tl.db, &tl.passphrase).unwrap();
    lib.set_analysis_root(&tmp.0);
    let mut history = History::default();
    let out = history.run(&mut lib, Command::ImportXml { import: req.clone() }).unwrap();
    let Effect::ImportedXml(report) = &out.effect else { panic!("{:?}", out.effect) };
    let ids: Vec<String> = report
        .results
        .iter()
        .map(|r| match r {
            XmlTrackResult::Imported(id) => id.0.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(report.lists.len(), 1);

    // Rows against rekordbox's own rows of the same files (oracle-a).
    let oracle = common::open_read_only("oracle-a").unwrap();
    let cols = "Title, BPM, Length, BitRate, SampleRate, FileSize, ReleaseYear, TrackNo, Commnt, StockDate, Rating, ColorID, FileType, \
                (SELECT Name FROM djmdArtist WHERE ID = ArtistID), (SELECT Name FROM djmdAlbum WHERE ID = AlbumID), \
                (SELECT Name FROM djmdGenre WHERE ID = GenreID), (SELECT ScaleName FROM djmdKey WHERE ID = KeyID)";
    let row = |c: &mb_masterdb::rusqlite::Connection, path: &str| -> Vec<String> {
        c.query_row(&format!("SELECT {cols} FROM djmdContent WHERE FolderPath = ?1 AND rb_local_deleted = 0"), [path], |r| {
            (0..17).map(|i| r.get::<_, mb_masterdb::rusqlite::types::Value>(i).map(|v| format!("{v:?}"))).collect()
        })
        .unwrap()
    };
    for p in &audio {
        assert_eq!(row(lib.connection(), p), row(oracle.connection(), p), "{p}");
    }
    // Grid columns and file. The beats come back close to rekordbox's PQTZ but not exactly: the
    // TEMPO reduction keeps a beat that matches the previous beat's own prediction without a new
    // entry, so the regular grid the import rebuilds can drift by a few ms (xml.md § TEMPO; the
    // same happens in rekordbox's own round trip, `getTrackBeatInfo` is ported 1:1).
    let oracle_root = fixture("oracle-a").unwrap().join("rekordbox/share");
    let (mut beats, mut exact, mut worst) = (0, 0, 0u32);
    for (id, p) in ids.iter().zip(&audio) {
        let (analysed, dat): (i64, String) = lib
            .connection()
            .query_row("SELECT Analysed, AnalysisDataPath FROM djmdContent WHERE ID = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(analysed, 0x10);
        let ours = AnlzFile::parse(&std::fs::read(tmp.0.join(dat.trim_start_matches('/'))).unwrap()).unwrap();
        assert_eq!(ours.path().unwrap(), format!("?/{}", Path::new(p).file_name().unwrap().to_string_lossy()));
        let theirs_rel: String = oracle
            .connection()
            .query_row("SELECT AnalysisDataPath FROM djmdContent WHERE FolderPath = ?1", [p], |r| r.get(0))
            .unwrap();
        let theirs = AnlzFile::parse(&std::fs::read(oracle_root.join(theirs_rel.trim_start_matches('/'))).unwrap()).unwrap();
        let (a, b) = (ours.beat_grid().unwrap().beats, theirs.beat_grid().unwrap().beats);
        // `TotalTime` is whole seconds: beats after it are not rebuilt.
        let total_ms = 1000 * req.tracks[ids.iter().position(|i| i == id).unwrap()].total_time.unwrap();
        assert_eq!(a.len(), b.iter().filter(|x| x.time_ms <= total_ms).count(), "{p}");
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((x.beat_in_bar, x.tempo), (y.beat_in_bar, y.tempo), "{p}");
            worst = worst.max(x.time_ms.abs_diff(y.time_ms));
            beats += 1;
            exact += usize::from(x.time_ms == y.time_ms);
        }
    }
    eprintln!("imported grids: {exact}/{beats} beats exact, worst {worst} ms");
    assert!(worst <= 10, "worst beat {worst} ms off");
    // The list, in XML order.
    let entries: Vec<String> = {
        let mut stmt = lib.connection().prepare("SELECT ContentID FROM djmdSongPlaylist WHERE PlaylistID = ?1 AND rb_local_deleted = 0 ORDER BY TrackNo").unwrap();
        stmt.query_map([report.lists[0].as_str()], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(entries, ids);

    // Undo: rows, list and analysis folders gone.
    history.undo(&mut lib).unwrap().unwrap();
    let left: i64 = lib
        .connection()
        .query_row(&format!("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0 AND ID IN ({})", ids.join(",")), [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
    let list_left: i64 = lib
        .connection()
        .query_row("SELECT count(*) FROM djmdPlaylist WHERE rb_local_deleted = 0 AND ID = ?1", [report.lists[0].as_str()], |r| r.get(0))
        .unwrap();
    assert_eq!(list_left, 0);
    assert!(report.created_files.iter().all(|f| !f.exists()));
    drop(lib);
    tl.verify_loads();
}

/// Tracks already in the collection: untouched by default; with `update_existing` they take the
/// XML's values and cues, and undo restores them.
#[test]
fn update_existing_tracks() {
    let Some(tl) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: no fixtures/oracle-a");
        return;
    };
    let tmp = TempDir::new("update");
    let share = clone_share(&tmp.0);
    let (db, snap) = snapshot(&tl.db, &tl.passphrase, &share);
    let mut doc = export_document(db.connection(), &snap, &XmlExportOptions::default()).unwrap();
    drop(db);
    let x = doc.collection.as_mut().unwrap().iter_mut().find(|t| t.total_time.unwrap_or(0) > 20 && !t.tempos.is_empty()).unwrap();
    let id = mb_xml::export::db_id(x.track_id.unwrap()).to_string();
    let old_title = x.name.clone().unwrap();
    x.name = Some("Changed by XML".into());
    x.position_marks.push(PositionMark {
        name: Some("from xml".into()),
        mark_type: 0,
        start: 1.5,
        end: None,
        num: 0,
        rgb: Some([250, 10, 10]),
        extra: Default::default(),
    });
    let mut req = mb_library::XmlImport { tracks: vec![x.clone()], ..Default::default() };

    let mut lib = Library::open_unguarded(&tl.db, &tl.passphrase).unwrap();
    lib.set_analysis_root(&share);
    let title = |lib: &Library| -> String { lib.connection().query_row("SELECT Title FROM djmdContent WHERE ID = ?1", [&id], |r| r.get(0)).unwrap() };
    let cues = |lib: &Library| -> Vec<(i64, i64, String, i64)> {
        let mut stmt = lib.connection().prepare("SELECT Kind, InMsec, ifnull(Comment, ''), ifnull(ColorTableIndex, 0) FROM djmdCue WHERE ContentID = ?1").unwrap();
        stmt.query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap()
    };

    let out = lib.execute(Command::ImportXml { import: req.clone() }).unwrap();
    assert!(matches!(&out.effect, Effect::ImportedXml(r) if matches!(r.results[0], XmlTrackResult::InCollection(_))));
    assert_eq!(title(&lib), old_title);
    assert!(cues(&lib).is_empty());

    req.update_existing = true;
    let mut history = History::default();
    history.run(&mut lib, Command::ImportXml { import: req }).unwrap();
    assert_eq!(title(&lib), "Changed by XML");
    let code = mb_xml::import::nearest_pad_color([250, 10, 10], &mb_usb::cues::PAD_COLORS);
    assert_eq!(cues(&lib), [(1, 1500, "from xml".into(), i64::from(code))]);
    let analysed: i64 = lib.connection().query_row("SELECT Analysed FROM djmdContent WHERE ID = ?1", [&id], |r| r.get(0)).unwrap();
    assert_eq!(analysed & 0x10, 0x10);

    history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(title(&lib), old_title);
    assert!(cues(&lib).is_empty());
    drop(lib);
    tl.verify_loads();
}
