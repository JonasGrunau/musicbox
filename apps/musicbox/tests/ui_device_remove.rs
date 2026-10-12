//! Headless UI test: remove a playlist (with its orphan tracks) and then a track from a device
//! through the tree and table context menus. The device is a temp folder exported by mb-usb
//! from a synthetic library; no fixtures, no OS input, no window.
//! Screenshots: set MB_UI_SHOTS=<dir> to save rendered frames for review.

use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mb_usb::source::{LocalAnalysis, SourceFile, SourcePlaylist, SourceTrack};
use mb_usb::{CancelToken, DeviceState, ExportOptions, PlanOptions, SelectionItem, SourceLibrary};
use musicbox::{App, Args};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn wait(h: &mut Harness<'_, App>, what: &str, secs: u64, cond: impl Fn(&Harness<'_, App>) -> bool) {
    let start = Instant::now();
    while !cond(h) {
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(25));
    }
    h.run_steps(2);
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(img) = h.render() {
        let _ = img.save(Path::new(&dir).join(format!("{name}.png")));
    }
}

fn track(dir: &Path, id: &str, artist: &str, file: &str) -> SourceTrack {
    let audio = dir.join("music").join(id).join(file);
    std::fs::create_dir_all(audio.parent().unwrap()).unwrap();
    std::fs::write(&audio, vec![id.as_bytes()[0]; 1000]).unwrap();
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
        artist: artist.into(),
        album: "Album".into(),
        bpm_x100: 12800,
        length: 300,
        bitrate: 320,
        bit_depth: 16,
        sample_rate: 44_100,
        file_type: 1,
        file_size: 1000,
        analysed: 105,
        info_marker: "1".into(),
        analysis_marker: "1".into(),
        folder_path: audio.to_string_lossy().into_owned(),
        file_name: file.into(),
        audio: Some(SourceFile { path: audio, size: 1000 }),
        analysis: LocalAnalysis { dat_path: Some(dat), dat: true, ext: true, ex2: true },
        ..Default::default()
    }
}

/// A stick with L1 = [101, 102] and L2 = [105].
fn make_stick(src: &Path, stick: &Path) {
    let mut lib = SourceLibrary { dbid: 4242, my_tag_master_db_id: 4243, ..Default::default() };
    for (id, artist, file) in [("101", "One", "One.mp3"), ("102", "Two", "Two.mp3"), ("105", "Five", "Five.mp3")] {
        let t = track(src, id, artist, file);
        lib.by_master_content_id.insert(t.device_master_content_id(), t.id.clone());
        lib.tracks.insert(t.id.clone(), t);
    }
    for (id, name, tracks) in [("1", "L1", vec!["101", "102"]), ("2", "L2", vec!["105"])] {
        lib.playlists.push(SourcePlaylist { id: id.into(), parent: None, name: name.into(), attribute: 0, seq: 0 });
        lib.entries.insert(id.into(), tracks.into_iter().map(String::from).collect());
    }
    let sel = [SelectionItem::Playlist("1".into()), SelectionItem::Playlist("2".into())];
    let dev = DeviceState::load(stick, Some(false)).unwrap();
    let plan = mb_usb::plan_export(&lib, &dev, &sel, &PlanOptions::default()).unwrap();
    mb_usb::execute(&plan, &dev, &ExportOptions::default(), &CancelToken::new(), &mut |_| {}).unwrap();
}

fn pdb(stick: &Path) -> mb_core::LibrarySnapshot {
    mb_pdb::DeviceLibrary::open(&stick.join("PIONEER/rekordbox")).unwrap().snapshot()
}

#[test]
fn remove_playlist_and_track_from_device() {
    let root: PathBuf = std::env::temp_dir().join(format!("mb-ui-remove-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let stick = root.join("MBREMOVE");
    std::fs::create_dir_all(&stick).unwrap();
    make_stick(&root.join("src"), &stick);
    assert_eq!(pdb(&stick).tracks.len(), 3);

    // No local library (the options file does not exist): only the pinned device is shown.
    let args = Args {
        options: Some(root.join("none/options.json")),
        db: Some(root.join("none/master.db")),
        devices: vec![stick.clone()],
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        ..Args::default()
    };
    let mut h = Harness::builder().with_size(Vec2::new(1280.0, 832.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "device lists", 30, |h| h.query_by_label("L1").is_some() && h.query_by_label("L2").is_some());

    // 1. The playlist L1 with its orphan tracks (the checkbox is on by default).
    h.get_by_label("L1").click_secondary();
    h.run_steps(3);
    h.get_by_label("Remove from device…").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Remove the playlist \"L1\" from MBREMOVE").is_some());
    assert!(h.query_by_label("Also delete tracks no other playlist uses").is_some());
    shot(&mut h, "remove-1-confirm");
    h.get_by_label("OK").click();
    wait(&mut h, "playlist removed", 30, |h| {
        h.query_by_label_contains("Removed").is_some() && h.query_by_label("L1").is_none()
    });
    assert!(h.query_by_label_contains("Removed 1 playlist(s) and 2 track(s)").is_some(), "status shows the summary");
    shot(&mut h, "remove-2-done");
    let snap = pdb(&stick);
    assert_eq!(snap.tracks.len(), 1);
    assert!(!stick.join("Contents/One/Album/One.mp3").exists());
    assert!(!stick.join("Contents/Two/Album/Two.mp3").exists());
    assert!(stick.join("Contents/Five/Album/Five.mp3").is_file());

    // 2. The last track, from the device's track table.
    h.get_by_label("All Tracks").click();
    wait(&mut h, "track row", 10, |h| h.query_by_label("Title 105").is_some());
    h.get_by_label("Title 105").click();
    h.run_steps(2);
    // The deck now shows the title too; the table row is the last match.
    h.get_all_by_label("Title 105").last().unwrap().click_secondary();
    h.run_steps(3);
    h.get_by_label("Remove from device…").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Remove 1 track(s) from MBREMOVE").is_some());
    h.get_by_label("OK").click();
    wait(&mut h, "track removed", 30, |h| h.query_by_label_contains("Removed 1 track(s)").is_some());
    let snap = pdb(&stick);
    assert!(snap.tracks.is_empty());
    assert!(!stick.join("Contents/Five/Album/Five.mp3").exists());
    let devlib = mb_devlib::DevLibDb::open_immutable(&stick.join("PIONEER/rekordbox/exportLibrary.db")).unwrap().load_snapshot(&stick).unwrap();
    assert!(devlib.tracks.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
