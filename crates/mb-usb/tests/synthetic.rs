//! Export runs on a synthetic library (no fixtures needed): a new stick, an incremental export
//! onto it (skipped identical list, in-place update, file-name clash with a file already on the
//! stick, stale ANLZ set), artwork generated from a base image, and cancellation.

use mb_pdb::schema::DbKind;
use mb_pdb::{ExportDb, Pdb};
use mb_usb::source::{LocalAnalysis, LocalArtwork, SourceFile, SourcePlaylist, SourceTrack};
use mb_usb::{
    CancelToken, DeviceState, ExportOptions, PlanOptions, SelectionItem, SourceLibrary,
    TrackResult, execute, plan_export,
};
use std::path::{Path, PathBuf};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mb-usb-synth-{tag}-{}", std::process::id()));
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

/// A source track with real files below `dir`.
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
        ..Default::default()
    }
}

fn add(lib: &mut SourceLibrary, t: SourceTrack) {
    lib.by_master_content_id
        .insert(t.device_master_content_id(), t.id.clone());
    lib.tracks.insert(t.id.clone(), t);
}

fn list(lib: &mut SourceLibrary, id: &str, name: &str, seq: i64, tracks: &[&str]) {
    lib.playlists.retain(|p| p.id != id);
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

fn base_library(dir: &Path) -> SourceLibrary {
    let mut lib = SourceLibrary {
        dbid: 4242,
        my_tag_master_db_id: 4243,
        ..Default::default()
    };
    add(
        &mut lib,
        track(dir, "101", "Artist", "Album", "Song.mp3", 1000),
    );
    // Same name, different file: becomes `Song-1.mp3`.
    add(
        &mut lib,
        track(dir, "102", "Artist", "Album", "Song.mp3", 1200),
    );
    let mut wav = track(dir, "103", "", "", "low?.wav", 500);
    wav.file_type = 11;
    wav.sample_rate = 22_050;
    add(&mut lib, wav);
    let mut art = track(dir, "104", "Art. ", "Cover", "Cover Track.mp3", 800);
    let base = dir.join("share/art104/artwork.jpg");
    std::fs::create_dir_all(base.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(300, 200, image::Rgb([200, 30, 30]))
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
    list(&mut lib, "1", "L1", 1, &["101", "102", "103", "104"]);
    lib
}

fn pdb_rows(root: &Path) -> ExportDb {
    let bytes = std::fs::read(root.join("PIONEER/rekordbox/export.pdb")).unwrap();
    assert!(mb_pdb::lint::lint(&bytes, DbKind::Export).is_clean());
    ExportDb::read(&Pdb::parse(bytes).unwrap()).unwrap()
}

fn assert_clean(root: &Path) {
    let rb = root.join("PIONEER/rekordbox");
    for f in [
        "sync.temp",
        "sync.devlibplus.temp",
        "exportLibrary.db-wal",
        "exportLibrary.db-shm",
        ".exportLibrary.db.musicbox",
        ".export.pdb.musicbox",
    ] {
        assert!(!rb.join(f).exists(), "{f} left behind");
    }
    // Both databases describe the same tracks with the same paths.
    let pdb = pdb_rows(root);
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let devlib = dev.devlib.unwrap();
    assert_eq!(pdb.tracks.len(), devlib.tracks.len());
    for t in &pdb.tracks {
        let d = devlib.tracks.iter().find(|d| d.id == t.id).unwrap();
        assert_eq!(d.path, t.folder_path);
        assert_eq!(d.analysis_path, t.analysis_path);
        assert_eq!(d.meta.title, t.title);
        let artist = pdb
            .artists
            .iter()
            .find(|a| a.id == t.artist_id)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        assert_eq!(d.meta.artist, artist);
        assert!(root.join(t.folder_path.trim_start_matches('/')).is_file());
        let dat = root.join(t.analysis_path.trim_start_matches('/'));
        let ppth = mb_anlz::AnlzFile::parse(&std::fs::read(&dat).unwrap())
            .unwrap()
            .path();
        assert_eq!(ppth.as_deref(), Some(t.folder_path.as_str()));
    }
}

#[test]
fn new_stick_then_incremental_export() {
    let src_dir = Tmp::new("src");
    let dev_dir = Tmp::new("dev");
    let root = dev_dir.0.as_path();
    let mut lib = base_library(&src_dir.0);
    let opts = PlanOptions {
        today: "2026-10-12".into(),
        ..PlanOptions::default()
    };
    let sel = [SelectionItem::Playlist("1".into())];
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(&lib, &dev, &sel, &opts).unwrap();
    let log_dir = dev_dir.0.join("logs");
    let exec_opts = ExportOptions {
        log_dir: Some(log_dir.clone()),
        ..ExportOptions::default()
    };
    let r = execute(&plan, &dev, &exec_opts, &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(
        r.results,
        [
            TrackResult::Exported,
            TrackResult::Exported,
            TrackResult::Unsupported,
            TrackResult::Exported
        ]
    );
    assert!(root.join("Contents/Artist/Album/Song.mp3").is_file());
    assert!(root.join("Contents/Artist/Album/Song-1.mp3").is_file());
    assert!(root.join("Contents/Art_/Cover/Cover Track.mp3").is_file());
    // Artwork generated from the 300×200 base: 80×80 and 240×240, letterboxed.
    for (f, px) in [
        ("a1.jpg", 80),
        ("a1_m.jpg", 240),
        ("b1.jpg", 80),
        ("b1_m.jpg", 240),
    ] {
        let img = image::open(root.join("PIONEER/Artwork/00001").join(f)).unwrap();
        assert_eq!((img.width(), img.height()), (px, px), "{f}");
    }
    let log = std::fs::read(r.log_file.unwrap()).unwrap();
    assert_eq!(&log[..2], &[0xff, 0xfe]);
    assert_clean(root);
    let rows = pdb_rows(root);
    assert_eq!(rows.tracks.len(), 3);
    assert_eq!(rows.playlist_entries.len(), 3);
    assert_eq!(rows.properties[0].track_count, 3);
    assert_eq!(rows.properties[0].date_created, "2026-10-12");

    // Session 2: L1 unchanged (skipped), new list L2 with a new track whose path clashes with a
    // file already on the stick, and track 101 with a new analysis marker.
    let clash_dir = root.join("Contents/Other/Album");
    std::fs::create_dir_all(&clash_dir).unwrap();
    std::fs::write(clash_dir.join("Clash.mp3"), b"not ours").unwrap();
    let new_path = "/Contents/Other/Album/Clash-1.mp3";
    let folder = mb_usb::naming::anlz_folder(new_path, false);
    let stale = root
        .join(folder.trim_start_matches('/'))
        .join("ANLZ0000.DAT");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, anlz_file(new_path)).unwrap();

    add(
        &mut lib,
        track(&src_dir.0, "105", "Other", "Album", "Clash.mp3", 900),
    );
    lib.tracks.get_mut("101").unwrap().analysis_marker = "7".into();
    {
        // A library edit: new title and artist, new info marker.
        let t = lib.tracks.get_mut("102").unwrap();
        t.title = "Renamed".into();
        t.artist = "New Artist".into();
        t.info_marker = "5".into();
    }
    list(&mut lib, "2", "L2", 2, &["105", "101", "102"]);
    // L1's device list equals the master list once the unsupported track is left out, so the
    // analysis change of 101 alone does not make rekordbox refill L1.
    let sel = [
        SelectionItem::Playlist("1".into()),
        SelectionItem::Playlist("2".into()),
    ];
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(&lib, &dev, &sel, &opts).unwrap();
    assert_eq!(plan.jobs[0].pdb_list, 0, "L1 is identical in export.pdb");
    assert_eq!(
        plan.jobs[0].devlib_list, 0,
        "L1 is identical in exportLibrary.db"
    );
    assert!(plan.jobs[0].steps.is_empty());
    let steps: Vec<_> = plan.jobs[1].steps.iter().collect();
    assert_eq!(steps[0].device_path.as_deref(), Some(new_path));
    assert_eq!(steps[1].result, TrackResult::Unchanged);
    let r = execute(&plan, &dev, &exec_opts, &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(r.completed, 3);
    assert_eq!(
        std::fs::read(clash_dir.join("Clash.mp3")).unwrap(),
        b"not ours"
    );
    // The stale set was replaced by ANLZ0001 for the new track.
    assert!(!stale.exists());
    assert!(stale.with_file_name("ANLZ0001.DAT").is_file());
    assert_clean(root);
    let rows = pdb_rows(root);
    assert_eq!(rows.tracks.len(), 4);
    let t102 = rows.tracks.iter().find(|t| t.id == 2).unwrap();
    assert_eq!(t102.title, "Renamed");
    assert_eq!(t102.track_info_updated, "5");
    // The path stays: an existing track is never copied again.
    assert_eq!(t102.folder_path, "/Contents/Artist/Album/Song-1.mp3");
    let t101 = rows.tracks.iter().find(|t| t.id == 1).unwrap();
    assert_eq!(t101.analysis_updated, "7");
    assert_eq!(t101.analysed, 105);
    let lists: Vec<(String, u32)> = rows
        .playlists
        .iter()
        .map(|p| (p.0.name.clone(), p.0.seq))
        .collect();
    assert!(lists.contains(&("L2".into(), 0)) && lists.contains(&("L1".into(), 1)));
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let devlib = dev.devlib.as_ref().unwrap();
    let d101 = devlib.tracks.iter().find(|t| t.id == 1).unwrap();
    assert_eq!(d101.analysis_marker, "7");
    assert_eq!(d101.content_link, 0x1D0700);
    let d102 = devlib.tracks.iter().find(|t| t.id == 2).unwrap();
    assert_eq!(d102.meta.title, "Renamed");
    assert_eq!(d102.meta.artist, "New Artist");
    assert_eq!(d102.info_marker, "5");

    // Exporting the same selection again changes nothing.
    let plan = plan_export(&lib, &dev, &sel, &opts).unwrap();
    assert!(plan.steps().next().is_none(), "{:?}", plan.jobs);
}

#[test]
fn cancellation_keeps_the_stick_consistent() {
    let src_dir = Tmp::new("src-c");
    let dev_dir = Tmp::new("dev-c");
    let root = dev_dir.0.as_path();
    let lib = base_library(&src_dir.0);
    let sel = [SelectionItem::Playlist("1".into())];
    let dev = DeviceState::load(root, Some(false)).unwrap();
    let plan = plan_export(&lib, &dev, &sel, &PlanOptions::default()).unwrap();
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let r = execute(&plan, &dev, &ExportOptions::default(), &cancel, &mut |p| {
        if p.tracks_done == 1 {
            c.cancel();
        }
    })
    .unwrap();
    assert!(r.cancelled);
    assert_eq!(r.completed, 1);
    assert!(!root.join("Contents/Artist/Album/Song-1.mp3").exists());
    assert_clean(root);
    let rows = pdb_rows(root);
    assert_eq!(rows.tracks.len(), 1);
    assert_eq!(rows.playlist_entries.len(), 1);
}
