//! Removing playlists and tracks from a device (`docs/re/export.md` § 4.3): a synthetic stick
//! exported by mb-usb, then (with fixtures) a clone of the reference stick's `PIONEER` folder.

use mb_pdb::schema::DbKind;
use mb_pdb::tables::{
    HistoryEntry, HistoryPlaylist, MyTagTrack, Named, PlaylistEntry, PlaylistNode, RecommendLike,
    TreeNode,
};
use mb_pdb::writer::{Exporter, insert_row};
use mb_pdb::{ExportDb, ExtDb, Pdb};
use mb_usb::source::{LocalAnalysis, LocalArtwork, SourceFile, SourcePlaylist, SourceTrack};
use mb_usb::{
    CancelToken, DeviceState, ExportOptions, PlanOptions, RemoveFile, RemoveRequest, SelectionItem,
    SourceLibrary, execute, execute_remove, plan_export, plan_remove,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mb-usb-remove-{tag}-{}", std::process::id()));
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
        ..Default::default()
    }
}

fn add(lib: &mut SourceLibrary, t: SourceTrack) {
    lib.by_master_content_id
        .insert(t.device_master_content_id(), t.id.clone());
    lib.tracks.insert(t.id.clone(), t);
}

fn list(lib: &mut SourceLibrary, id: &str, name: &str, seq: i64, tracks: &[&str]) {
    lib.playlists.push(SourcePlaylist {
        id: id.into(),
        parent: None,
        name: name.into(),
        attribute: 0,
        seq,
    });
    lib.entries
        .insert(id.into(), tracks.iter().map(|s| s.to_string()).collect());
}

fn library(dir: &Path) -> SourceLibrary {
    let mut lib = SourceLibrary {
        dbid: 4242,
        my_tag_master_db_id: 4243,
        ..Default::default()
    };
    add(
        &mut lib,
        track(dir, "101", "Artist", "Album", "Song.mp3", 1000),
    );
    add(
        &mut lib,
        track(dir, "102", "Artist", "Album", "Song.mp3", 1200),
    );
    let mut art = track(dir, "104", "Cover Artist", "Cover", "Cover Track.mp3", 800);
    let base = dir.join("share/art104/artwork.jpg");
    std::fs::create_dir_all(base.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(100, 100, image::Rgb([200, 30, 30]))
        .save(&base)
        .unwrap();
    art.artwork = Some(LocalArtwork {
        base,
        base_exists: true,
        small: None,
        medium: None,
        large: None,
    });
    add(&mut lib, art);
    add(
        &mut lib,
        track(dir, "105", "Solo", "Single", "Solo.mp3", 700),
    );
    list(&mut lib, "1", "L1", 1, &["101", "102", "104"]);
    list(&mut lib, "2", "L2", 2, &["101", "105"]);
    list(&mut lib, "3", "L3", 3, &["105"]);
    lib
}

fn rb(root: &Path) -> PathBuf {
    root.join("PIONEER/rekordbox")
}

fn pdb_rows(root: &Path) -> ExportDb {
    let bytes = std::fs::read(rb(root).join("export.pdb")).unwrap();
    let report = mb_pdb::lint::lint(&bytes, DbKind::Export);
    assert!(report.is_clean(), "export.pdb lint: {report:?}");
    ExportDb::read(&Pdb::parse(bytes).unwrap()).unwrap()
}

fn ext_rows(root: &Path) -> ExtDb {
    let bytes = std::fs::read(rb(root).join("exportExt.pdb")).unwrap();
    let report = mb_pdb::lint::lint(&bytes, DbKind::ExportExt);
    assert!(report.is_clean(), "exportExt.pdb lint: {report:?}");
    ExtDb::read(&Pdb::parse(bytes).unwrap()).unwrap()
}

fn devlib_count(root: &Path, sql: &str) -> i64 {
    let db = mb_devlib::DevLibDb::open_immutable(&rb(root).join("exportLibrary.db")).unwrap();
    db.connection().query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Top-level lists of export.pdb as (name, SEQ), in SEQ order.
fn pdb_top(rows: &ExportDb) -> Vec<(String, u32)> {
    let mut v: Vec<_> = rows
        .playlists
        .iter()
        .filter(|p| p.0.parent_id == 0)
        .map(|p| (p.0.name.clone(), p.0.seq))
        .collect();
    v.sort_by_key(|(_, s)| *s);
    v
}

/// Both databases hold the same tracks with the same paths, every referenced file exists, and
/// sibling positions are contiguous from 0.
fn assert_consistent(root: &Path) {
    let pdb = pdb_rows(root);
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let devlib = dev.devlib.as_ref().unwrap();
    let mut a: Vec<_> = pdb
        .tracks
        .iter()
        .map(|t| (t.id, t.folder_path.clone()))
        .collect();
    let mut b: Vec<_> = devlib
        .tracks
        .iter()
        .map(|t| (t.id, t.path.clone()))
        .collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);
    for t in &pdb.tracks {
        assert!(root.join(t.folder_path.trim_start_matches('/')).is_file());
        assert!(root.join(t.analysis_path.trim_start_matches('/')).is_file());
    }
    assert_eq!(pdb.properties[0].track_count as usize, pdb.tracks.len());
    assert_eq!(
        devlib_count(root, "select numberOfContents from property") as usize,
        devlib.tracks.len()
    );
    let mut by_parent: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for p in &pdb.playlists {
        by_parent.entry(p.0.parent_id).or_default().push(p.0.seq);
    }
    for seqs in by_parent.values_mut() {
        seqs.sort();
        assert_eq!(*seqs, (0..seqs.len() as u32).collect::<Vec<_>>());
    }
    let mut by_parent: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
    for p in &devlib.playlists {
        by_parent.entry(p.parent).or_default().push(p.seq);
    }
    for seqs in by_parent.values_mut() {
        seqs.sort();
        assert_eq!(*seqs, (0..seqs.len() as i64).collect::<Vec<_>>());
    }
    for f in [
        ".exportLibrary.db.musicbox",
        "exportLibrary.db-wal",
        "exportLibrary.db-shm",
    ] {
        assert!(!rb(root).join(f).exists(), "{f} left behind");
    }
}

/// Player-written rows: a tag list holding track 1 and a history with tracks 2, 1, 2, 4 in
/// export.pdb; a "like" between 2 and 1 and a My Tag link of 2 in exportExt.pdb.
fn add_player_rows(root: &Path) {
    let path = rb(root).join("export.pdb");
    let mut ex = Exporter::load(&std::fs::read(&path).unwrap()).unwrap();
    let top = ex.playlists().filter(|p| p.0.parent_id == 0).count() as u32;
    ex.insert_playlist(&PlaylistNode(TreeNode {
        id: 16385,
        seq: top,
        name: "TAG LIST 001".into(),
        artwork_id: 0,
        attribute: 0,
        parent_id: 0,
    }))
    .unwrap();
    ex.add_entry(&PlaylistEntry {
        playlist_id: 16385,
        track_id: 1,
        entry_index: 1,
    })
    .unwrap();
    ex.db
        .txn(|db| {
            insert_row(
                db,
                &HistoryPlaylist(Named {
                    id: 1,
                    name: "HISTORY 001".into(),
                }),
            )?;
            for (seq, track) in [(1, 2), (2, 1), (3, 2), (4, 4)] {
                insert_row(
                    db,
                    &HistoryEntry {
                        history_id: 1,
                        seq,
                        track_id: track,
                        on_air: 0,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&path, ex.finish()).unwrap();

    let path = rb(root).join("exportExt.pdb");
    let mut db =
        mb_pdb::engine::Db::load(DbKind::ExportExt, &std::fs::read(&path).unwrap()).unwrap();
    db.txn(|db| {
        insert_row(
            db,
            &RecommendLike {
                track_id1: 2,
                track_id2: 1,
                like_rate: 1,
                reserved1: 0,
                reserved2: 0,
                date_created_hi: 0,
                date_created_lo: 0,
                reserved3: String::new(),
            },
        )?;
        insert_row(
            db,
            &MyTagTrack {
                tag_id: 7,
                track_id: 2,
                reserved1: 0,
                reserved2: String::new(),
            },
        )?;
        Ok(())
    })
    .unwrap();
    db.close();
    std::fs::write(&path, db.to_bytes()).unwrap();
}

fn export(lib: &SourceLibrary, root: &Path, lists: &[&str]) {
    let sel: Vec<_> = lists
        .iter()
        .map(|l| SelectionItem::Playlist((*l).into()))
        .collect();
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let opts = PlanOptions {
        today: "2026-10-12".into(),
        ..PlanOptions::default()
    };
    let plan = plan_export(lib, &dev, &sel, &opts).unwrap();
    execute(
        &plan,
        &dev,
        &ExportOptions::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
}

#[test]
fn remove_lists_tracks_and_files() {
    let src = Tmp::new("src");
    let dev_dir = Tmp::new("dev");
    let root = dev_dir.0.as_path();
    let lib = library(&src.0);
    export(&lib, root, &["1", "2", "3"]);
    add_player_rows(root);
    let before = pdb_rows(root);
    let ids: Vec<(u32, String)> = before
        .tracks
        .iter()
        .map(|t| (t.id, t.title.clone()))
        .collect();
    assert_eq!(
        ids,
        [
            (1, "Title 101".into()),
            (2, "Title 102".into()),
            (3, "Title 104".into()),
            (4, "Title 105".into())
        ]
    );
    let art = before
        .artwork
        .iter()
        .find(|a| a.id != 0)
        .unwrap()
        .path
        .clone();
    let cover_art = root.join(art.trim_start_matches('/'));
    assert!(cover_art.is_file());
    let anlz_of = |id: u32| {
        let t = before.tracks.iter().find(|t| t.id == id).unwrap();
        root.join(t.analysis_path.trim_start_matches('/'))
    };

    // 1. L1 with its orphan tracks (102 and 104; 101 is still in L2 and the tag list).
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let req = RemoveRequest {
        playlists: vec![vec!["L1".into()]],
        delete_orphan_tracks: true,
        ..Default::default()
    };
    let plan = plan_remove(&dev, &req).unwrap();
    assert_eq!(
        plan.track_paths,
        [
            "/Contents/Artist/Album/Song-1.mp3",
            "/Contents/Cover Artist/Cover/Cover Track.mp3"
        ]
    );
    assert_eq!(plan.pdb_tracks, [2, 3]);
    assert_eq!(plan.devlib_tracks, [2, 3]);
    assert!(plan.files.contains(&RemoveFile::Audio(
        "/Contents/Artist/Album/Song-1.mp3".into()
    )));
    let (anlz2, anlz3) = (anlz_of(2), anlz_of(3));
    let report = execute_remove(&plan, &dev).unwrap();
    assert_eq!(report.tracks, 2);
    assert_eq!(report.files_missing, 0);
    assert_consistent(root);
    assert!(root.join("Contents/Artist/Album/Song.mp3").is_file());
    assert!(!root.join("Contents/Artist/Album/Song-1.mp3").exists());
    assert!(
        !root
            .join("Contents/Cover Artist/Cover/Cover Track.mp3")
            .exists()
    );
    for f in [&anlz2, &anlz3] {
        for ext in ["DAT", "EXT", "2EX"] {
            assert!(!f.with_extension(ext).exists());
        }
    }
    assert!(!cover_art.exists());
    let name = cover_art
        .file_name()
        .unwrap()
        .to_string_lossy()
        .replace(".jpg", "_m.jpg");
    assert!(!cover_art.with_file_name(name).exists());
    let b_art = cover_art.with_file_name(
        cover_art
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replacen('a', "b", 1),
    );
    assert!(!b_art.exists());

    let rows = pdb_rows(root);
    assert_eq!(
        pdb_top(&rows),
        [
            ("L3".into(), 0),
            ("L2".into(), 1),
            ("TAG LIST 001".into(), 2)
        ]
    );
    let tag_entries: Vec<u32> = rows
        .playlist_entries
        .iter()
        .filter(|e| e.playlist_id == 16385)
        .map(|e| e.track_id)
        .collect();
    assert_eq!(tag_entries, [1]);
    // History: the two plays of track 2 are gone, the others moved up.
    let mut hist: Vec<(u16, u32)> = rows
        .history_entries
        .iter()
        .map(|e| (e.seq, e.track_id))
        .collect();
    hist.sort();
    assert_eq!(hist, [(1, 1), (2, 4)]);
    assert_eq!(rows.histories.len(), 1);
    // Lookups: the cover artist and album went; genres stay while used; keys are never deleted.
    let artists: HashSet<_> = rows.artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(artists, HashSet::from(["Artist", "Solo"]));
    let albums: HashSet<_> = rows.albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(albums, HashSet::from(["Album", "Single"]));
    assert_eq!(rows.keys.len(), 1);
    assert!(rows.artwork.is_empty());
    let ext = ext_rows(root);
    assert!(ext.recommend_likes.is_empty());
    assert!(ext.tag_tracks.is_empty());
    assert_eq!(devlib_count(root, "select count(*) from image"), 0);
    assert_eq!(devlib_count(root, "select count(*) from artist"), 2);
    assert_eq!(devlib_count(root, "select count(*) from album"), 2);
    assert_eq!(devlib_count(root, "select count(*) from playlist"), 2);

    // 2. Track 105 explicitly: out of L2 and L3 (L3 stays, empty) and out of the history.
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let req = RemoveRequest {
        tracks: vec!["/Contents/Solo/Single/Solo.mp3".into()],
        ..Default::default()
    };
    let plan = plan_remove(&dev, &req).unwrap();
    execute_remove(&plan, &dev).unwrap();
    assert_consistent(root);
    let rows = pdb_rows(root);
    assert_eq!(rows.tracks.len(), 1);
    let l2: Vec<(u32, u32)> = rows
        .playlist_entries
        .iter()
        .filter(|e| e.playlist_id != 16385)
        .map(|e| (e.track_id, e.entry_index))
        .collect();
    assert_eq!(l2, [(1, 1)]);
    assert_eq!(rows.history_entries.len(), 1);
    assert!(!rows.artists.iter().any(|a| a.name == "Solo"));
    assert_eq!(
        devlib_count(root, "select count(*) from playlist_content"),
        1
    );
    assert!(!root.join("Contents/Solo/Single/Solo.mp3").exists());

    // 3. Unknown names are refused; the local library keeps its audio.
    let dev = DeviceState::load(root, Some(false)).unwrap();
    assert!(
        plan_remove(
            &dev,
            &RemoveRequest {
                playlists: vec![vec!["nope".into()]],
                ..Default::default()
            }
        )
        .is_err()
    );
    let song = root.join("Contents/Artist/Album/Song.mp3");
    let req = RemoveRequest {
        tracks: vec!["/Contents/Artist/Album/Song.mp3".into()],
        local_paths: HashSet::from([song.clone()]),
        ..Default::default()
    };
    let plan = plan_remove(&dev, &req).unwrap();
    assert!(!plan.files.iter().any(|f| matches!(f, RemoveFile::Audio(_))));
    execute_remove(&plan, &dev).unwrap();
    assert!(song.is_file());
    let rows = pdb_rows(root);
    assert!(rows.tracks.is_empty());
    assert_eq!(rows.properties[0].track_count, 0);

    // 4. The stick takes a new export afterwards.
    export(&lib, root, &["1"]);
    assert_consistent(root);
    assert_eq!(pdb_rows(root).tracks.len(), 3);
}

#[test]
fn remove_folder_with_children() {
    let src = Tmp::new("src-f");
    let dev_dir = Tmp::new("dev-f");
    let root = dev_dir.0.as_path();
    let mut lib = library(&src.0);
    lib.playlists.push(SourcePlaylist {
        id: "10".into(),
        parent: None,
        name: "Folder".into(),
        attribute: 1,
        seq: 4,
    });
    for p in lib
        .playlists
        .iter_mut()
        .filter(|p| p.id == "2" || p.id == "3")
    {
        p.parent = Some("10".into());
    }
    export(&lib, root, &["1", "10"]);
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let rows = &dev.pdb.as_ref().unwrap().export;
    assert_eq!(rows.playlists.len(), 4, "{:?}", rows.playlists);
    let req = RemoveRequest {
        playlists: vec![vec!["Folder".into()], vec!["Folder".into(), "L2".into()]],
        delete_orphan_tracks: true,
        ..Default::default()
    };
    let plan = plan_remove(&dev, &req).unwrap();
    assert_eq!(plan.pdb_lists.len(), 1);
    assert_eq!(plan.pdb_lists_all.len(), 3);
    assert_eq!(plan.track_paths, ["/Contents/Solo/Single/Solo.mp3"]);
    execute_remove(&plan, &dev).unwrap();
    assert_consistent(root);
    let rows = pdb_rows(root);
    assert_eq!(pdb_top(&rows), [("L1".into(), 0)]);
    assert_eq!(rows.tracks.len(), 3);
    assert_eq!(devlib_count(root, "select count(*) from playlist"), 1);
}

/// The reference stick's `PIONEER` folder, cloned (APFS `clonefile`, no extra space) into a
/// temp dir: one list and its orphan tracks removed; every other row, the player-created tag
/// lists and the histories survive.
#[test]
fn reference_stick_clone_keeps_player_data() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stick/PIONEER");
    if !src.join("rekordbox/export.pdb").is_file() {
        eprintln!("fixtures/stick missing; skipped");
        return;
    }
    let dev_dir = Tmp::new("stick");
    let root = dev_dir.0.as_path();
    let ok = std::process::Command::new("/bin/cp")
        .arg("-Rc")
        .arg(&src)
        .arg(root.join("PIONEER"))
        .status()
        .unwrap()
        .success();
    assert!(ok, "clone of fixtures/stick failed");
    let before = pdb_rows(root);
    let before_ext = ext_rows(root);
    let dev = DeviceState::load(root, Some(false)).unwrap();
    // Every list rekordbox exported (all but the players' tag lists), with their orphans.
    let targets: Vec<String> = pdb_top(&before)
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| !n.starts_with("TAG LIST"))
        .collect();
    let req = RemoveRequest {
        playlists: targets.iter().map(|n| vec![n.clone()]).collect(),
        delete_orphan_tracks: true,
        ..Default::default()
    };
    let plan = plan_remove(&dev, &req).unwrap();
    eprintln!(
        "removing {} lists: {} tracks, {} files",
        targets.len(),
        plan.pdb_tracks.len(),
        plan.files.len()
    );
    assert!(!plan.devlib_lists.is_empty() && !plan.pdb_tracks.is_empty());
    assert_eq!(plan.pdb_tracks, plan.devlib_tracks);
    let removed: HashSet<u32> = plan.pdb_tracks.iter().copied().collect();
    let report = execute_remove(&plan, &dev).unwrap();
    assert_eq!(report.tracks, removed.len());

    let after = pdb_rows(root);
    let after_ext = ext_rows(root);
    // Every other list (tag lists included) is still there, in the same order.
    let names =
        |rows: &ExportDb| -> Vec<String> { pdb_top(rows).into_iter().map(|(n, _)| n).collect() };
    let mut expected = names(&before);
    expected.retain(|n| !targets.contains(n));
    assert_eq!(names(&after), expected);
    let seqs: Vec<u32> = pdb_top(&after).into_iter().map(|(_, s)| s).collect();
    assert_eq!(seqs, (0..seqs.len() as u32).collect::<Vec<_>>());
    // Entries of the other lists are unchanged.
    let deleted = plan.pdb_lists_all.clone();
    let entries = |rows: &ExportDb| -> Vec<(u32, u32, u32)> {
        let mut v: Vec<_> = rows
            .playlist_entries
            .iter()
            .filter(|e| !deleted.contains(&e.playlist_id))
            .map(|e| (e.playlist_id, e.entry_index, e.track_id))
            .collect();
        v.sort();
        v
    };
    assert_eq!(entries(&after), entries(&before));
    // Remaining tracks are untouched; the removed ones are gone.
    let kept: Vec<_> = before
        .tracks
        .iter()
        .filter(|t| !removed.contains(&t.id))
        .cloned()
        .collect();
    let mut got = after.tracks.clone();
    got.sort_by_key(|t| t.id);
    let mut want = kept;
    want.sort_by_key(|t| t.id);
    assert_eq!(got, want);
    assert_eq!(
        after.properties[0].track_count,
        before.properties[0].track_count - removed.len() as u32
    );
    // Histories: every history list stays; only the removed tracks' plays go, the rest keep
    // their order.
    assert_eq!(after.histories, before.histories);
    for h in &before.histories {
        let plays = |rows: &ExportDb| -> Vec<u32> {
            let mut v: Vec<_> = rows
                .history_entries
                .iter()
                .filter(|e| e.history_id == h.0.id)
                .map(|e| (e.seq, e.track_id))
                .collect();
            v.sort();
            v.into_iter().map(|(_, t)| t).collect()
        };
        let mut want = plays(&before);
        want.retain(|t| !removed.contains(t));
        assert_eq!(plays(&after), want, "history {}", h.0.name);
    }
    // exportExt.pdb: the My Tag tree and the property row are untouched.
    assert_eq!(after_ext.tags, before_ext.tags);
    assert_eq!(
        after_ext.db_property_options,
        before_ext.db_property_options
    );
    // exportLibrary.db lost the same tracks and the list.
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let devlib = dev.devlib.as_ref().unwrap();
    assert!(!devlib.playlists.iter().any(|l| targets.contains(&l.name)));
    assert!(!devlib.tracks.iter().any(|t| removed.contains(&t.id)));
    // The removed tracks' ANLZ sets are gone, the others are still there.
    for t in &after.tracks {
        assert!(root.join(t.analysis_path.trim_start_matches('/')).is_file());
    }
    for t in before.tracks.iter().filter(|t| removed.contains(&t.id)) {
        let still_used = after
            .tracks
            .iter()
            .any(|a| a.analysis_path == t.analysis_path);
        assert_eq!(
            root.join(t.analysis_path.trim_start_matches('/')).exists(),
            still_used
        );
    }
}
