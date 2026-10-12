//! Exports `oracle-small` (`fixtures/oracle-a`) onto a copy of the reference stick
//! (`fixtures/stick`), a stick that belongs to another library and carries years of player and
//! rekordbox history: player-created TAG LISTs, histories, another library's My Tags and Sync
//! Manager files. Everything that was there must survive (`docs/re/export.md` § 6.5, § 7, § 2.5):
//! - every export.pdb / exportExt.pdb / exportLibrary.db row is still present; only the property
//!   row and list positions (the new list goes on top) may change;
//! - the other library's My Tag tree is left alone (rekordbox asks before overwriting it);
//! - every file outside the databases and settings files is byte-identical;
//! - both `.pdb` files are lint-clean.
//!
//! The copy is an APFS clone in a temp dir; `fixtures/` is never written. Skipped without the
//! fixtures or the corpus audio.

use mb_pdb::schema::DbKind;
use mb_pdb::{ExportDb, ExtDb, Pdb};
use mb_usb::source::LibraryDir;
use mb_usb::{
    CancelToken, DeviceState, ExportOptions, PlanOptions, SelectionItem, SourceLibrary,
    TrackResult, execute, plan_export,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixtures() -> Option<(PathBuf, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let stick = root.join("stick");
    let oracle = root.join("oracle-a");
    (stick.join("PIONEER/rekordbox/exportLibrary.db").is_file()
        && oracle.join("rekordbox/master.db").is_file())
    .then_some((stick, oracle))
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Relative path → bytes of every file below `dir` (AppleDouble files skipped).
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("._") {
                continue;
            }
            let rel = format!("{rel}/{name}");
            if e.file_type().unwrap().is_dir() {
                walk(&e.path(), &rel, out);
            } else {
                out.insert(rel, std::fs::read(e.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

/// Every `before` row is in `after`, comparing `key(row)`.
fn all_kept<T, K: PartialEq + std::fmt::Debug>(
    what: &str,
    before: &[T],
    after: &[T],
    key: impl Fn(&T) -> K,
) {
    let after: Vec<K> = after.iter().map(&key).collect();
    let lost: Vec<K> = before.iter().map(&key).filter(|k| !after.contains(k)).collect();
    assert!(lost.is_empty(), "{what}: {} rows lost, e.g. {:?}", lost.len(), lost.first());
}

fn read_pdb(path: &Path) -> ExportDb {
    ExportDb::read(&Pdb::parse(std::fs::read(path).unwrap()).unwrap()).unwrap()
}

fn read_ext(path: &Path) -> ExtDb {
    ExtDb::read(&Pdb::parse(std::fs::read(path).unwrap()).unwrap()).unwrap()
}

#[test]
fn export_keeps_everything_on_a_foreign_stick() -> anyhow::Result<()> {
    let Some((stick, oracle)) = fixtures() else {
        eprintln!("skipped: fixtures/stick or fixtures/oracle-a is not present");
        return Ok(());
    };
    let library = LibraryDir::open(&oracle)?;
    let conn = library.connection();
    let all = SourceLibrary::load(conn, &library.analysis_root, &[])?;
    let small = all.find_playlist("oracle-small")?.id.clone();
    let sel = [SelectionItem::Playlist(small.clone())];
    let src = SourceLibrary::load(conn, &library.analysis_root, &sel)?;
    if !src.entries[&small].iter().all(|id| src.tracks[id].audio.is_some()) {
        eprintln!("skipped: the corpus audio (~/re-work/corpus) is not present");
        return Ok(());
    }

    let tmp = TempDir(std::env::temp_dir().join(format!("mb-usb-foreign-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&tmp.0);
    let root = tmp.0.join("device");
    std::fs::create_dir_all(&root)?;
    // APFS clone of the snapshot (`cp -c`): no extra disk space, and fixtures/ stays untouched.
    let status = std::process::Command::new("/bin/cp")
        .arg("-cR")
        .arg(stick.join("PIONEER"))
        .arg(root.join("PIONEER"))
        .status()?;
    assert!(status.success());
    let rb = root.join("PIONEER/rekordbox");
    let before_files = tree(&root.join("PIONEER"));
    let pdb0 = read_pdb(&rb.join("export.pdb"));
    let ext0 = read_ext(&rb.join("exportExt.pdb"));
    let devlib0 = tmp.0.join("before.db");
    std::fs::copy(rb.join("exportLibrary.db"), &devlib0)?;

    let dev = DeviceState::load(&root, Some(false))?;
    assert_ne!(dev.my_tag_master_db_id(), 0);
    assert_ne!(dev.my_tag_master_db_id(), src.my_tag_master_db_id);
    let plan = plan_export(&src, &dev, &sel, &PlanOptions::default())?;
    assert!(plan.ext_preamble.is_empty() && plan.devlib_preamble.is_empty());
    let report = execute(&plan, &dev, &ExportOptions::default(), &CancelToken::new(), &mut |_| {})?;
    assert_eq!(report.completed, 5);
    assert!(report.results.iter().all(|r| *r == TrackResult::Exported));

    // Both .pdb files are lint-clean.
    for (name, kind) in [("export.pdb", DbKind::Export), ("exportExt.pdb", DbKind::ExportExt)] {
        let rep = mb_pdb::lint::lint(&std::fs::read(rb.join(name))?, kind);
        assert_eq!(rep.errors(), 0, "{name}: {:?}", rep.findings);
    }

    // export.pdb: every row kept; new list on top, so only list positions may change.
    let pdb1 = read_pdb(&rb.join("export.pdb"));
    all_kept("tracks", &pdb0.tracks, &pdb1.tracks, |r| r.clone());
    all_kept("genres", &pdb0.genres, &pdb1.genres, |r| r.clone());
    all_kept("artists", &pdb0.artists, &pdb1.artists, |r| r.clone());
    all_kept("albums", &pdb0.albums, &pdb1.albums, |r| r.clone());
    all_kept("labels", &pdb0.labels, &pdb1.labels, |r| r.clone());
    all_kept("keys", &pdb0.keys, &pdb1.keys, |r| r.clone());
    all_kept("colors", &pdb0.colors, &pdb1.colors, |r| r.clone());
    all_kept("playlists", &pdb0.playlists, &pdb1.playlists, |r| {
        (r.0.id, r.0.parent_id, r.0.name.clone(), r.0.attribute, r.0.artwork_id)
    });
    all_kept("entries", &pdb0.playlist_entries, &pdb1.playlist_entries, |r| *r);
    all_kept("hcb lists", &pdb0.hot_cue_bank_lists, &pdb1.hot_cue_bank_lists, |r| r.clone());
    all_kept("histories", &pdb0.histories, &pdb1.histories, |r| r.clone());
    all_kept("history entries", &pdb0.history_entries, &pdb1.history_entries, |r| *r);
    all_kept("artwork", &pdb0.artwork, &pdb1.artwork, |r| r.clone());
    all_kept("menu items", &pdb0.menu_items, &pdb1.menu_items, |r| r.clone());
    all_kept("categories", &pdb0.categories, &pdb1.categories, |r| *r);
    all_kept("sorts", &pdb0.sorts, &pdb1.sorts, |r| *r);
    assert_eq!(pdb1.tracks.len(), pdb0.tracks.len() + 5);
    assert_eq!(pdb1.playlists.len(), pdb0.playlists.len() + 1);

    // exportExt.pdb: the other library's My Tags untouched, nothing lost.
    let ext1 = read_ext(&rb.join("exportExt.pdb"));
    assert_eq!(ext1.tags, ext0.tags);
    assert_eq!(ext1.tag_tracks, ext0.tag_tracks);
    assert_eq!(ext1.db_property_options, ext0.db_property_options);
    all_kept("likes", &ext0.recommend_likes, &ext1.recommend_likes, |r| r.clone());
    all_kept("ext histories", &ext0.histories, &ext1.histories, |r| r.clone());
    all_kept("ext history entries", &ext0.history_entries, &ext1.history_entries, |r| r.clone());
    all_kept("content options", &ext0.content_options, &ext1.content_options, |r| r.clone());

    // exportLibrary.db: no row lost; only the property row and list positions changed.
    let before = mb_devlib::DevLibDb::open_immutable(&devlib0)?;
    let after = mb_devlib::DevLibDb::open_immutable(&rb.join("exportLibrary.db"))?;
    let cmp = mb_devlib::diff::compare(before.connection(), after.connection())?;
    for t in &cmp.tables {
        assert!(!t.columns_differ, "{}", t.table);
        assert!(t.only_left.is_empty(), "{}: rows lost {:?}", t.table, t.only_left);
        for d in &t.differing {
            let ok = match t.table.as_str() {
                "property" => true,
                "playlist" => d.cells.iter().all(|c| c.column == "sequenceNo"),
                _ => false,
            };
            assert!(ok, "{}: row {} changed: {:?}", t.table, d.rowid, d.cells);
        }
    }
    let added: BTreeMap<_, _> = cmp.tables.iter().map(|t| (t.table.as_str(), t.only_right.len())).collect();
    assert_eq!(added["content"], 5);
    assert_eq!(added["myTag"], 0);
    assert_eq!(added["myTag_content"], 0);

    // Every other file is byte-identical (players' and rekordbox's side files, sync files,
    // ANLZ sets and artwork of the existing tracks).
    let after_files = tree(&root.join("PIONEER"));
    let rewritten = |p: &str| {
        p.starts_with("/rekordbox/export")
            || p.ends_with("SETTING.DAT")
            || p.ends_with("SETTING2.DAT")
            || p == "/djprofile.nxs"
    };
    for (path, bytes) in &before_files {
        if rewritten(path) {
            continue;
        }
        let now = after_files.get(path).unwrap_or_else(|| panic!("{path} deleted"));
        assert!(now == bytes, "{path} changed");
    }
    Ok(())
}
