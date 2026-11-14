//! Headless UI tests: export into an empty folder pinned as a device, through the tree's
//! "Export to" menu and by dragging tracks or a playlist onto the device, then check the stick
//! with our own readers. No OS input, no window.

use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use musicbox::{App, Args};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let to = dst.join(entry.file_name());
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to);
        } else if !name.ends_with("-shm") && !name.ends_with("-wal") {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn wait(h: &mut Harness<'_, App>, what: &str, secs: u64, cond: impl Fn(&Harness<'_, App>) -> bool) {
    let start = Instant::now();
    while !cond(h) {
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(25));
    }
    h.run_steps(2);
}

/// A copy of the oracle-a library and an empty folder `MBSTICK` pinned as a device, both under
/// a temp folder of this test (`name`: tests in one binary run in parallel). `None` without
/// fixtures.
fn setup(name: &str) -> Option<(Harness<'static, App>, PathBuf, PathBuf)> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        eprintln!("no fixtures, skipping");
        return None;
    }
    let root: PathBuf = std::env::temp_dir().join(format!("mb-ui-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&fixture.join("rekordbox"), &root.join("lib/rekordbox"));
    std::fs::copy(fixture.join("options.json"), root.join("lib/options.json")).unwrap();
    let stick = root.join("MBSTICK");
    std::fs::create_dir_all(&stick).unwrap();

    let args = Args {
        options: Some(root.join("lib/options.json")),
        db: Some(root.join("lib/rekordbox/master.db")),
        analysis_root: Some(root.join("lib/rekordbox/share")),
        devices: vec![stick.clone()],
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        import_folder: None,
    };
    let mut h = Harness::builder().with_size(Vec2::new(1280.0, 832.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "library + device", 30, |h| h.query_by_label("oracle-small").is_some() && h.query_by_label("MBSTICK").is_some());
    Some((h, root, stick))
}

/// Saves a render when `MB_UI_SHOTS` names a folder.
fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    if let Ok(img) = h.render() {
        img.save(Path::new(&dir).join(format!("{name}.png"))).unwrap();
    }
}

/// Press at `from`, move to `to` in steps (past egui's drag threshold), release. `shot` names
/// the render taken over the target, before the release.
fn drag(h: &mut Harness<'_, App>, from: egui::Pos2, to: egui::Pos2, shot_name: &str) {
    h.hover_at(from);
    h.step();
    h.drag_at(from);
    h.step();
    for i in 1..=10 {
        h.hover_at(from + (to - from) * (i as f32 / 10.0));
        h.step();
    }
    shot(h, shot_name);
    h.drop_at(to);
    h.step();
}

fn wait_export(h: &mut Harness<'_, App>) {
    wait(h, "export finished", 120, |h| h.query_by_label_contains("to MBSTICK").is_some() && h.query_by_label_contains("Exporting").is_none());
}

/// Every track on the stick in both databases, with its audio and ANLZ file; returns the
/// export.pdb snapshot.
fn check_stick(stick: &Path, tracks: usize) -> mb_core::LibrarySnapshot {
    let pdb = mb_pdb::DeviceLibrary::open(&stick.join("PIONEER/rekordbox")).unwrap().snapshot();
    assert_eq!(pdb.tracks.len(), tracks);
    let devlib = mb_devlib::DevLibDb::open_immutable(&stick.join("PIONEER/rekordbox/exportLibrary.db")).unwrap().load_snapshot(stick).unwrap();
    assert_eq!(devlib.tracks.len(), tracks);
    for t in &pdb.tracks {
        assert!(t.path.is_file(), "audio copied: {}", t.path.display());
        assert!(t.analysis_path.as_ref().is_some_and(|p| p.is_file()), "ANLZ written for {}", t.title);
    }
    pdb
}

#[test]
fn export_playlist_to_empty_device() {
    let Some((mut h, root, stick)) = setup("menu") else { return };
    h.get_by_label("oracle-small").click_secondary();
    h.run_steps(3);
    // egui's submenu button names itself "Export to ⏵".
    h.get_by_label_contains("Export to").click();
    h.run_steps(3);
    h.get_all_by_label("MBSTICK").last().unwrap().click();
    wait_export(&mut h);
    assert!(h.query_by_label_contains("Exported 5 track(s) to MBSTICK").is_some(), "status shows the export summary");
    check_stick(&stick, 5);
    let _ = std::fs::remove_dir_all(&root);
}

/// Editing stays off: exporting leaves the library as it is.
#[test]
fn drag_track_onto_device() {
    let Some((mut h, root, stick)) = setup("drag-track") else { return };
    let from = h.get_all_by_label("chords_Am").next().unwrap().rect().center();
    let to = h.get_by_label("MBSTICK").rect().center();
    drag(&mut h, from, to, "export-drag-track");
    wait_export(&mut h);
    assert!(h.query_by_label_contains("Exported 1 track(s) to MBSTICK").is_some(), "status shows the export summary");
    let pdb = check_stick(&stick, 1);
    assert_eq!(pdb.tracks[0].title, "chords_Am");
    assert!(pdb.playlists.is_empty(), "loose tracks get no device playlist");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn drag_playlist_onto_device() {
    let Some((mut h, root, stick)) = setup("drag-list") else { return };
    let from = h.get_by_label("oracle-small").rect().center();
    let to = h.get_by_label("MBSTICK").rect().center();
    drag(&mut h, from, to, "export-drag-list");
    wait_export(&mut h);
    assert!(h.query_by_label_contains("Exported 5 track(s) to MBSTICK").is_some(), "status shows the export summary");
    let pdb = check_stick(&stick, 5);
    let list = pdb.playlists.iter().find(|p| p.name == "oracle-small").expect("device playlist");
    assert_eq!(pdb.playlist_entries.get(&list.id).map_or(0, Vec::len), 5);
    let _ = std::fs::remove_dir_all(&root);
}
