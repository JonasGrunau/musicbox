//! Exporting intelligent playlists (`docs/re/export.md` § 1.7, `docs/re/smartlist.md`): the
//! tracks are evaluated when the source is read, the device list is a plain playlist, an
//! unchanged result is skipped and a changed one refilled. Synthetic library (no fixtures), plus
//! the evaluation on a copy of `fixtures/oracle-a` when present.

use mb_core::smartlist::{Clock, Condition, MATCH_ALL, SmartList, op};
use mb_pdb::{ExportDb, Pdb};
use mb_usb::source::{LibraryDir, LocalAnalysis, SourceFile, SourcePlaylist, SourceTrack};
use mb_usb::{
    CancelToken, DeviceState, ExportOptions, PlanOptions, SelectionItem, SourceLibrary,
    TrackResult, execute, plan_export,
};
use std::path::{Path, PathBuf};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mb-usb-smart-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const CLOCK: Clock = Clock { unix: 1_791_892_800, utc_offset: 0 };

fn track(dir: &Path, id: &str, genre: &str, bpm: i64) -> SourceTrack {
    let file = format!("{id}.mp3");
    let audio = dir.join("music").join(&file);
    std::fs::create_dir_all(audio.parent().unwrap()).unwrap();
    std::fs::write(&audio, vec![id.as_bytes()[2]; 500]).unwrap();
    let dat = dir.join("share").join(id).join("ANLZ0000.DAT");
    std::fs::create_dir_all(dat.parent().unwrap()).unwrap();
    let anlz = mb_anlz::AnlzFile {
        header: mb_anlz::FileHeader::default().encode(),
        sections: vec![mb_anlz::tags::Path::new(format!("?/{file}")).encode()],
    }
    .to_bytes();
    for ext in ["DAT", "EXT", "2EX"] {
        std::fs::write(dat.with_extension(ext), &anlz).unwrap();
    }
    SourceTrack {
        id: id.into(),
        title: format!("Title {id}"),
        artist: "Artist".into(),
        album: "Album".into(),
        genre: genre.into(),
        bpm_x100: bpm * 100,
        length: 300,
        bitrate: 320,
        bit_depth: 16,
        sample_rate: 44_100,
        file_type: 1,
        file_size: 500,
        analysed: 105,
        content_link: 1902094,
        info_marker: "4".into(),
        analysis_marker: "6".into(),
        stock_date: "2026-01-02".into(),
        folder_path: audio.to_string_lossy().into_owned(),
        file_name: file,
        audio: Some(SourceFile { path: audio, size: 500 }),
        analysis: LocalAnalysis { dat_path: Some(dat), dat: true, ext: true, ex2: true },
        ..Default::default()
    }
}

fn house_120_130(list_id: &str) -> String {
    SmartList::new(
        list_id,
        MATCH_ALL,
        vec![Condition::new("genre", op::CONTAINS, "", "house", ""), Condition::new("bpm", op::IN_RANGE, "", "120", "130")],
    )
    .to_xml()
}

fn library(dir: &Path) -> SourceLibrary {
    let mut lib = SourceLibrary { dbid: 4242, my_tag_master_db_id: 4243, ..Default::default() };
    for t in [
        track(dir, "201", "Deep House", 124),
        track(dir, "202", "Techno", 128),
        track(dir, "203", "House", 126),
        track(dir, "204", "House", 140),
    ] {
        lib.by_master_content_id.insert(t.device_master_content_id(), t.id.clone());
        lib.collection_order.push(t.id.clone());
        lib.tracks.insert(t.id.clone(), t);
    }
    lib.playlists.push(SourcePlaylist { id: "9".into(), parent: None, name: "Smart".into(), attribute: 4, seq: 1 });
    lib.smart_lists.insert("9".into(), house_120_130("9"));
    evaluate(&mut lib);
    lib
}

/// What `SourceLibrary::load` does for a selected smart list.
fn evaluate(lib: &mut SourceLibrary) {
    let ids = lib.smart_list_entries("9", CLOCK).unwrap();
    lib.entries.insert("9".into(), ids);
}

fn pdb(root: &Path) -> ExportDb {
    let bytes = std::fs::read(root.join("PIONEER/rekordbox/export.pdb")).unwrap();
    assert!(mb_pdb::lint::lint(&bytes, mb_pdb::schema::DbKind::Export).is_clean());
    ExportDb::read(&Pdb::parse(bytes).unwrap()).unwrap()
}

/// Titles of the device list "Smart" in entry order, from both databases.
fn device_list(root: &Path) -> (Vec<String>, Vec<String>) {
    let rows = pdb(root);
    let list = rows.playlists.iter().find(|p| p.0.name == "Smart").expect("pdb list");
    assert_eq!(list.0.attribute, 0, "a plain playlist on the device");
    let mut entries: Vec<_> = rows.playlist_entries.iter().filter(|e| e.playlist_id == list.0.id).collect();
    entries.sort_by_key(|e| e.entry_index);
    let title = |id: u32| rows.tracks.iter().find(|t| t.id == id).unwrap().title.clone();
    let pdb_titles = entries.iter().map(|e| title(e.track_id)).collect();

    let dev = DeviceState::load(root, Some(false)).unwrap().devlib.unwrap();
    let dl = dev.playlists.iter().find(|p| p.name == "Smart").expect("devlib list");
    assert_eq!(dl.attribute, 0);
    let devlib_titles = dev
        .entries
        .get(&dl.id)
        .map(|ids| ids.iter().map(|c| dev.tracks.iter().find(|t| t.id == *c).unwrap().meta.title.clone()).collect())
        .unwrap_or_default();
    (pdb_titles, devlib_titles)
}

#[test]
fn smart_list_export_and_refill() {
    let src = Tmp::new("src");
    let dev_dir = Tmp::new("dev");
    let root = dev_dir.0.as_path();
    let mut lib = library(&src.0);
    assert_eq!(lib.entries["9"], ["201", "203"]);
    let opts = PlanOptions { today: "2026-10-12".into(), ..PlanOptions::default() };
    let sel = [SelectionItem::Playlist("9".into())];

    // 1. New stick: the current result as a plain playlist.
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(&lib, &dev, &sel, &opts).unwrap();
    let r = execute(&plan, &dev, &ExportOptions::default(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(r.results, [TrackResult::Exported, TrackResult::Exported]);
    let want = vec!["Title 201".to_owned(), "Title 203".to_owned()];
    assert_eq!(device_list(root), (want.clone(), want));

    // 2. Same result: the list is identical and skipped.
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(&lib, &dev, &sel, &opts).unwrap();
    assert!(plan.jobs.iter().all(|j| j.steps.is_empty() && j.list_pdb.is_empty() && j.list_devlib.is_empty()));

    // 3. A library edit changes the result (202 becomes Tech House): emptied and refilled in
    //    collection order, the new track copied.
    lib.tracks.get_mut("202").unwrap().genre = "Tech House".into();
    evaluate(&mut lib);
    assert_eq!(lib.entries["9"], ["201", "202", "203"]);
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(&lib, &dev, &sel, &opts).unwrap();
    let r = execute(&plan, &dev, &ExportOptions::default(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(r.results, [TrackResult::Unchanged, TrackResult::Exported, TrackResult::Unchanged]);
    let want: Vec<String> = ["Title 201", "Title 202", "Title 203"].map(String::from).to_vec();
    assert_eq!(device_list(root), (want.clone(), want));
    assert_eq!(pdb(root).playlists.iter().filter(|p| p.0.name == "Smart").count(), 1);

    // 4. A rule change that empties the result empties the device list.
    lib.smart_lists.insert(
        "9".into(),
        SmartList::new("9", MATCH_ALL, vec![Condition::new("genre", op::IS, "", "trance", "")]).to_xml(),
    );
    evaluate(&mut lib);
    assert!(lib.entries["9"].is_empty());
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(&lib, &dev, &sel, &opts).unwrap();
    execute(&plan, &dev, &ExportOptions::default(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(device_list(root), (Vec::new(), Vec::new()));
}

/// The evaluation inside `SourceLibrary::load_at` on a copy of oracle-a with a smart list row
/// added: the entries equal the SQL selection, in rowid order, and the plan's job carries them.
#[test]
fn fixture_smart_list_entries() {
    let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fx.join("rekordbox/master.db").is_file() {
        eprintln!("skipped: fixtures/oracle-a is not present");
        return;
    }
    let tmp = Tmp::new("fixture");
    std::fs::create_dir_all(tmp.0.join("rekordbox")).unwrap();
    std::fs::copy(fx.join("options.json"), tmp.0.join("options.json")).unwrap();
    let db = tmp.0.join("rekordbox/master.db");
    std::fs::copy(fx.join("rekordbox/master.db"), &db).unwrap();
    let paths = mb_masterdb::LibraryPaths::from_options_json(&tmp.0.join("options.json")).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    let expected: Vec<String> = {
        let w = mb_masterdb::MasterDbWriter::open_unguarded(&db, &pass).unwrap();
        w.connection()
            .execute(
                "INSERT INTO djmdPlaylist (ID, Seq, Name, Attribute, ParentID, SmartList, UUID, created_at, updated_at) \
                 VALUES ('77', 9, 'Smart', 4, 'root', ?1, '00000000-0000-4000-8000-000000000077', \
                 '2026-10-12 00:00:00.000 +00:00', '2026-10-12 00:00:00.000 +00:00')",
                [house_120_130("77")],
            )
            .unwrap();
        let mut stmt = w
            .connection()
            .prepare(
                "SELECT c.ID FROM djmdContent c LEFT JOIN djmdGenre g ON g.ID = c.GenreID \
                 WHERE c.rb_local_deleted = 0 AND lower(g.Name) LIKE '%house%' AND c.BPM BETWEEN 12000 AND 13000 \
                 ORDER BY c.rowid",
            )
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert!(!expected.is_empty());
    let library = LibraryDir::open(&tmp.0).unwrap();
    let sel = [SelectionItem::Playlist("77".into())];
    let src = SourceLibrary::load_at(library.connection(), &library.analysis_root, &sel, CLOCK).unwrap();
    assert_eq!(src.entries["77"], expected);
    let dev_dir = Tmp::new("fixture-dev");
    let dev = DeviceState::load(&dev_dir.0, Some(false)).unwrap();
    let plan = plan_export(&src, &dev, &sel, &PlanOptions::default()).unwrap();
    let job = plan.jobs.iter().find(|j| j.name == "Smart").unwrap();
    assert_eq!(job.tracks, expected);
}
