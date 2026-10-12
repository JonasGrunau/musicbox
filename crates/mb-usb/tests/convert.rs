//! Device Library Plus conversion (`docs/re/export.md` § 6.4).
//!
//! 1. `stick_conversion_matches_stick`: the classic databases of the reference stick snapshot
//!    (`fixtures/stick`, copied to a temp dir with its `a` artwork) are converted and compared
//!    with the stick's own `exportLibrary.db`, table by table. Every difference is one the
//!    stick's later history explains (see the test).
//! 2. `synthetic_export_then_conversion`: a musicbox export to an empty folder, exportLibrary.db
//!    deleted, then an export onto the old-style stick converts it again.

use mb_devlib::{DevLibDb, DevLibWriter};
use mb_usb::DeviceState;
use mb_usb::convert::{copy_artwork, plan_conversion};
use rusqlite::types::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mb-usb-convert-{tag}-{}", std::process::id()));
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

fn fixture(rel: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    p.exists().then_some(p)
}

fn copy_dir(from: &Path, to: &Path, keep: &dyn Fn(&str) -> bool) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to.join(&name), keep);
        } else if keep(&name) {
            std::fs::copy(e.path(), to.join(&name)).unwrap();
        }
    }
}

type Table = BTreeMap<String, Vec<Value>>;

/// Rows keyed by the first column (tables with an integer primary key) or by the whole row
/// with a running number for duplicates.
fn table(conn: &rusqlite::Connection, name: &str) -> (Vec<String>, Table) {
    let mut stmt = conn.prepare(&format!("select * from {name}")).unwrap();
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let n = cols.len();
    let keyed = conn
        .query_row(
            "select sql from sqlite_master where type = 'table' and name = ?1",
            [name],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
        .contains("primary key");
    let mut out = Table::new();
    let rows = stmt
        .query_map([], |r| {
            (0..n)
                .map(|i| r.get::<_, Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap();
    for row in rows {
        let row = row.unwrap();
        let mut key = if keyed {
            format!("{:?}", row[0])
        } else {
            format!("{row:?}")
        };
        while out.contains_key(&key) {
            key.push('+');
        }
        out.insert(key, row);
    }
    (cols, out)
}

#[derive(Debug, Default)]
struct TableDiff {
    only_ours: Vec<String>,
    only_stick: Vec<String>,
    /// Column → number of rows where it differs.
    columns: BTreeMap<String, usize>,
    /// Column → (key, ours, stick) of the differing rows.
    cells: BTreeMap<String, Vec<(String, Value, Value)>>,
    /// Keys of rows that differ.
    rows: Vec<String>,
    same: usize,
}

fn diff(ours: &rusqlite::Connection, stick: &rusqlite::Connection, name: &str) -> TableDiff {
    let (cols, a) = table(ours, name);
    let (_, b) = table(stick, name);
    let mut d = TableDiff::default();
    for (k, ra) in &a {
        match b.get(k) {
            None => d.only_ours.push(k.clone()),
            Some(rb) => {
                let mut differs = false;
                for (i, c) in cols.iter().enumerate() {
                    if ra[i] != rb[i] {
                        *d.columns.entry(c.clone()).or_default() += 1;
                        d.cells.entry(c.clone()).or_default().push((
                            k.clone(),
                            ra[i].clone(),
                            rb[i].clone(),
                        ));
                        differs = true;
                    }
                }
                if differs {
                    d.rows.push(k.clone());
                } else {
                    d.same += 1;
                }
            }
        }
    }
    d.only_stick = b.keys().filter(|k| !a.contains_key(*k)).cloned().collect();
    d
}

const TABLES: [&str; 22] = [
    "content",
    "genre",
    "artist",
    "album",
    "label",
    "key",
    "color",
    "playlist",
    "playlist_content",
    "hotCueBankList",
    "hotCueBankList_cue",
    "history",
    "history_content",
    "image",
    "cue",
    "menuItem",
    "category",
    "sort",
    "property",
    "recommendedLike",
    "myTag",
    "myTag_content",
];

#[test]
fn stick_conversion_matches_stick() {
    let Some(stick) = fixture("fixtures/stick/PIONEER") else {
        eprintln!("fixtures/stick missing; skipped");
        return;
    };
    let tmp = Tmp::new("stick");
    let root = tmp.0.join("root");
    // The classic databases and the `a` artwork only (no exportLibrary.db, no `b` files).
    copy_dir(
        &stick.join("rekordbox"),
        &root.join("PIONEER/rekordbox"),
        &|n| n == "export.pdb" || n == "exportExt.pdb",
    );
    copy_dir(
        &stick.join("Artwork"),
        &root.join("PIONEER/Artwork"),
        &|n| n.starts_with('a'),
    );
    let dev = DeviceState::load(&root, Some(false)).unwrap();
    assert!(dev.devlib.is_none());
    let conv = plan_conversion(
        dev.pdb.as_ref().unwrap(),
        mb_devlib::MenuSeed::defaults(),
        "2026-10-12",
    );
    let db = root.join("PIONEER/rekordbox/exportLibrary.db");
    let w = DevLibWriter::create_for_conversion(&db, &Default::default()).unwrap();
    conv.write(&w).unwrap();
    w.close().unwrap();
    let written = copy_artwork(&root, &conv).unwrap();

    let ours = DevLibDb::open_immutable(&db).unwrap();
    let theirs = DevLibDb::open_immutable(&stick.join("rekordbox/exportLibrary.db")).unwrap();
    let d: BTreeMap<&str, TableDiff> = TABLES
        .iter()
        .map(|&name| (name, diff(ours.connection(), theirs.connection(), name)))
        .collect();
    for (name, t) in &d {
        eprintln!(
            "{name:20} same {:4} differ {:4} only-ours {:4} only-stick {:4} {:?}",
            t.same,
            t.rows.len(),
            t.only_ours.len(),
            t.only_stick.len(),
            t.columns
        );
    }
    let id = |k: &str| -> i64 {
        k.trim_start_matches("Integer(")
            .trim_end_matches(')')
            .parse()
            .unwrap()
    };

    // Identical tables (rows, values and storage classes).
    for name in [
        "genre",
        "label",
        "key",
        "color",
        "image",
        "menuItem",
        "category",
        "sort",
        "myTag",
        "hotCueBankList",
        "hotCueBankList_cue",
        "cue",
        "recommendedLike",
        "myTag_content",
    ] {
        let t = &d[name];
        assert!(
            t.rows.is_empty() && t.only_ours.is_empty() && t.only_stick.is_empty(),
            "{name}: {t:?}"
        );
    }
    assert_eq!(d["image"].same, 702);
    assert_eq!(d["myTag"].same, 28);

    // content: 704 rows on both sides. Rows 1–620 come from the conversion and only differ in
    // what later re-exports rewrote in exportLibrary.db alone (`docs/re/export.md` § 4.2: full
    // `Analysed`, text markers). Rows 621–704 were inserted later by lock-step exports, which
    // write NULL for missing lookups and `titleForSearch` and take the KUVO flag from the source.
    let content = &d["content"];
    assert!(content.only_ours.is_empty() && content.only_stick.is_empty());
    assert_eq!(content.same + content.rows.len(), 704);
    for (col, cells) in &content.cells {
        for (k, _, _) in cells {
            assert!(
                id(k) > 620
                    || ["analysedBits", "cueUpdateCount", "informationUpdateCount"]
                        .contains(&col.as_str()),
                "content {k}: {col}"
            );
        }
    }
    // artist / album: the converted rows are identical; the later inserts (artists 512–572,
    // albums 529–604) have NULL `nameForSearch` / NULL album artist and image. Album 605 exists
    // only in export.pdb: the album IDs of the two databases diverge at 604 because track 692
    // has its album (and genre) in export.pdb only.
    let artist = &d["artist"];
    assert!(artist.only_ours.is_empty() && artist.only_stick.is_empty());
    assert!(artist.rows.iter().all(|k| (512..=572).contains(&id(k))));
    assert_eq!(artist.columns.keys().collect::<Vec<_>>(), ["nameForSearch"]);
    let album = &d["album"];
    assert_eq!(album.only_ours, ["Integer(605)"]);
    assert!(album.only_stick.is_empty());
    assert!(album.rows.iter().all(|k| id(k) >= 529));
    // playlist: the lists that already existed when the stick was converted (4–7, TAG LIST
    // 001–005) are there with the same name, parent, attribute and image; only their position
    // moved, because each database got its own later lists (players add TAG LISTs to
    // export.pdb only, and the two databases number new lists separately, § 2.3).
    let playlist = &d["playlist"];
    for (col, cells) in &playlist.cells {
        for (k, _, _) in cells {
            let converted = [4, 5, 6, 7, 16385, 16386, 16387, 16388, 16389].contains(&id(k));
            assert!(
                !converted || col == "sequenceNo",
                "playlist {k}: {col} differs"
            );
        }
    }
    // Player histories were written to export.pdb after the conversion.
    assert_eq!(d["history"].only_ours.len(), 21);
    assert_eq!(d["history_content"].only_ours.len(), 160);
    assert!(d["history"].only_stick.is_empty() && d["history_content"].only_stick.is_empty());
    // property: all equal but the background colour, a per-database device setting (classic 8,
    // Plus 2).
    let (_, ours_p) = table(ours.connection(), "property");
    let (_, theirs_p) = table(theirs.connection(), "property");
    let (a, b) = (
        ours_p.values().next().unwrap(),
        theirs_p.values().next().unwrap(),
    );
    for i in 0..a.len() {
        assert!(i == 4 || a[i] == b[i], "property column {i}");
    }
    assert_eq!((&a[4], &b[4]), (&Value::Integer(8), &Value::Integer(2)));

    // Artwork: every `b` file the conversion wrote equals the stick's.
    assert_eq!(written.len(), 1404);
    for to in &written {
        let rel = to.strip_prefix(root.join("PIONEER")).unwrap();
        assert_eq!(
            std::fs::read(to).unwrap(),
            std::fs::read(stick.join(rel)).unwrap(),
            "{}",
            rel.display()
        );
    }
}

// ---- synthetic: export, drop exportLibrary.db, convert --------------------------------------

use mb_devlib::schema::{MasterMenuItem, MasterMenuSetting};
use mb_pdb::schema::DbKind;
use mb_usb::source::{
    LocalAnalysis, LocalArtwork, SourceFile, SourceMenus, SourcePlaylist, SourceTrack,
};
use mb_usb::{
    CancelToken, ExportOptions, PlanOptions, SelectionItem, SourceLibrary, execute, plan_export,
};

fn anlz_file(ppth: &str) -> Vec<u8> {
    mb_anlz::AnlzFile {
        header: mb_anlz::FileHeader::default().encode(),
        sections: vec![mb_anlz::tags::Path::new(ppth).encode()],
    }
    .to_bytes()
}

fn track(dir: &Path, id: &str, artist: &str, album: &str, file: &str, bytes: usize) -> SourceTrack {
    let audio = dir.join("music").join(id).join(file);
    std::fs::create_dir_all(audio.parent().unwrap()).unwrap();
    std::fs::write(&audio, vec![id.as_bytes()[0]; bytes]).unwrap();
    let dat = dir.join("share").join(id).join("ANLZ0000.DAT");
    std::fs::create_dir_all(dat.parent().unwrap()).unwrap();
    for ext in ["DAT", "EXT", "2EX"] {
        std::fs::write(dat.with_extension(ext), anlz_file(&format!("?/{file}"))).unwrap();
    }
    let base = dir.join("share").join(id).join("artwork.jpg");
    image::RgbImage::from_pixel(120, 120, image::Rgb([id.as_bytes()[2], 90, 30]))
        .save(&base)
        .unwrap();
    SourceTrack {
        id: id.into(),
        title: format!("Title {id}"),
        artist: artist.into(),
        album: album.into(),
        genre: "Techno".into(),
        key: "Am".into(),
        bpm_x100: 12800,
        length: 300,
        bitrate: 320,
        bit_depth: 16,
        sample_rate: 44_100,
        file_type: 1,
        file_size: bytes as i64,
        analysed: 105,
        content_link: 1902094,
        info_marker: "4".into(),
        analysis_marker: "6".into(),
        stock_date: "2026-01-02".into(),
        date_created: "2026-01-01".into(),
        hot_cue_auto_load: true,
        folder_path: audio.to_string_lossy().into_owned(),
        file_name: file.into(),
        audio: Some(SourceFile {
            path: audio,
            size: bytes as u64,
        }),
        analysis: LocalAnalysis {
            dat_path: Some(dat),
            dat: true,
            ext: true,
            ex2: true,
        },
        artwork: Some(LocalArtwork {
            base,
            base_exists: true,
            small: None,
            medium: None,
            large: None,
        }),
        ..Default::default()
    }
}

/// master.db menu tables as a library has them (rekordbox's built-in defaults).
fn menus() -> SourceMenus {
    let seed = mb_devlib::MenuSeed::defaults();
    let setting = |m: &mb_devlib::MenuSetting| MasterMenuSetting {
        id: m.id,
        menu_item_id: m.menu_item_id,
        seq: m.seq,
        disable: i64::from(!m.visible) | (i64::from(m.sub_column) << 1),
    };
    SourceMenus {
        colors: seed
            .colors
            .iter()
            .map(|(id, name)| (*id, *id, *id, name.clone()))
            .collect(),
        menu_items: seed
            .menu_items
            .iter()
            .map(|m| MasterMenuItem {
                id: m.id,
                class: i64::from(m.kind as u8 as i8),
                name: m.label().to_owned(),
            })
            .collect(),
        categories: seed.categories.iter().map(|c| (setting(c), 0)).collect(),
        sorts: seed.sorts.iter().map(setting).collect(),
    }
}

fn library(dir: &Path) -> SourceLibrary {
    let mut lib = SourceLibrary {
        dbid: 4242,
        my_tag_master_db_id: 4243,
        menus: menus(),
        ..Default::default()
    };
    for t in [
        track(dir, "101", "Artist A", "Album A", "One.mp3", 1000),
        track(dir, "102", "Artist B", "", "Two.mp3", 1200),
        track(dir, "103", "Artist A", "Album C", "Three.mp3", 900),
    ] {
        lib.by_master_content_id
            .insert(t.device_master_content_id(), t.id.clone());
        lib.tracks.insert(t.id.clone(), t);
    }
    for (id, name, seq, tracks) in [
        ("1", "First", 1, vec!["101", "102"]),
        ("2", "Second", 2, vec!["103"]),
    ] {
        lib.playlists.push(SourcePlaylist {
            id: id.into(),
            parent: None,
            name: name.into(),
            attribute: 0,
            seq,
        });
        lib.entries
            .insert(id.into(), tracks.into_iter().map(String::from).collect());
    }
    lib
}

fn export(lib: &SourceLibrary, root: &Path, lists: &[&str]) {
    let opts = PlanOptions {
        today: "2026-10-12".into(),
        ..PlanOptions::default()
    };
    let sel: Vec<_> = lists
        .iter()
        .map(|id| SelectionItem::Playlist(id.to_string()))
        .collect();
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(lib, &dev, &sel, &opts).unwrap();
    let report = execute(
        &plan,
        &dev,
        &ExportOptions::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    assert!(report.error.is_none() && !report.cancelled);
}

#[test]
fn synthetic_export_then_conversion() {
    let src = Tmp::new("syn-src");
    let dev_dir = Tmp::new("syn-dev");
    let root = dev_dir.0.join("stick");
    std::fs::create_dir_all(&root).unwrap();
    let lib = library(&src.0);
    export(&lib, &root, &["1"]);

    // Make it an old stick: no exportLibrary.db, no `b` artwork.
    let rb = root.join("PIONEER/rekordbox");
    let original = dev_dir.0.join("original.db");
    std::fs::rename(rb.join("exportLibrary.db"), &original).unwrap();
    let art = root.join("PIONEER/Artwork/00001");
    let mut b_files = Vec::new();
    for e in std::fs::read_dir(&art).unwrap() {
        let p = e.unwrap().path();
        if p.file_name().unwrap().to_string_lossy().starts_with('b') {
            b_files.push((p.clone(), std::fs::read(&p).unwrap()));
            std::fs::remove_file(&p).unwrap();
        }
    }
    assert_eq!(b_files.len(), 4, "two images, plain and _m");

    // 1. The conversion alone, compared with the database the export wrote.
    let dev = DeviceState::load(&root, Some(false)).unwrap();
    let conv = plan_conversion(
        dev.pdb.as_ref().unwrap(),
        mb_devlib::MenuSeed::defaults(),
        "2026-10-12",
    );
    let converted = dev_dir.0.join("converted.db");
    let w = DevLibWriter::create_for_conversion(&converted, &Default::default()).unwrap();
    conv.write(&w).unwrap();
    w.close().unwrap();
    let ours = DevLibDb::open_immutable(&converted).unwrap();
    let theirs = DevLibDb::open_immutable(&original).unwrap();
    for name in TABLES {
        let t = diff(ours.connection(), theirs.connection(), name);
        assert!(
            t.only_ours.is_empty() && t.only_stick.is_empty(),
            "{name}: rows differ {t:?}"
        );
        // What the conversion stores differently from the export's own inserts
        // (`insertAllDevContent` vs `insertTrack`, `devlib.md` § content): '' instead of NULL
        // for `titleForSearch`, 0 instead of NULL for missing lookups, and the classic update
        // markers (RESERVED5–7) as integers where the export's insert leaves them NULL.
        let allowed: &[&str] = match name {
            "content" => &[
                "titleForSearch",
                "artist_id_remixer",
                "artist_id_originalArtist",
                "artist_id_composer",
                "album_id",
                "label_id",
                "cueUpdateCount",
                "analysisDataUpdateCount",
                "informationUpdateCount",
            ],
            // `getAndAddArtist` / `getAndAddAlbum` leave the search name (and the album's
            // artist and image) NULL; the conversion copies '' and 0 from export.pdb.
            "artist" => &["nameForSearch"],
            "album" => &["nameForSearch", "artist_id", "image_id"],
            // A new list's `image_id` is NULL; export.pdb stores 0.
            "playlist" => &["image_id"],
            _ => &[],
        };
        for col in t.columns.keys() {
            assert!(
                allowed.contains(&col.as_str()),
                "{name}.{col}: {:?}",
                t.cells[col]
            );
        }
    }

    // 2. The whole flow: an export onto the old stick converts it first.
    export(&lib, &root, &["1", "2"]);
    for (p, bytes) in &b_files {
        assert_eq!(&std::fs::read(p).unwrap(), bytes, "{}", p.display());
    }
    for f in [
        "sync.temp",
        "sync.devlibplus.temp",
        "exportLibrary.db-wal",
        "exportLibrary.db-shm",
        ".exportLibrary.db.musicbox",
    ] {
        assert!(!rb.join(f).exists(), "{f} left behind");
    }
    let bytes = std::fs::read(rb.join("export.pdb")).unwrap();
    assert!(mb_pdb::lint::lint(&bytes, DbKind::Export).is_clean());
    let dev = DeviceState::load(&root, Some(false)).unwrap();
    let pdb = &dev.pdb.as_ref().unwrap().export;
    let devlib = dev.devlib.as_ref().unwrap();
    assert_eq!(pdb.tracks.len(), 3);
    assert_eq!(devlib.tracks.len(), 3);
    for t in &pdb.tracks {
        let d = devlib.tracks.iter().find(|d| d.id == t.id).unwrap();
        assert_eq!(d.path, t.folder_path);
        assert_eq!(d.analysis_path, t.analysis_path);
        assert_eq!(d.image_id, t.artwork_id);
        let b = root.join(devlib.images[&d.image_id].trim_start_matches('/'));
        assert!(b.is_file(), "{}", b.display());
    }
    let mut names: Vec<_> = devlib.playlists.iter().map(|p| p.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["First", "Second"]);
}
