//! Headless UI test: export a playlist through the tree's "Export to" menu into an empty folder
//! pinned as a device, then check the stick with our own readers. No OS input, no window.

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

#[test]
fn export_playlist_to_empty_device() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let root: PathBuf = std::env::temp_dir().join(format!("mb-ui-export-{}", std::process::id()));
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
        ..Default::default()
    };
    let mut h = Harness::builder().with_size(Vec2::new(1280.0, 832.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "library + device", 30, |h| h.query_by_label("oracle-small").is_some() && h.query_by_label("MBSTICK").is_some());

    h.get_by_label("oracle-small").click_secondary();
    h.run_steps(3);
    // egui's submenu button names itself "Export to ⏵".
    h.get_by_label_contains("Export to").click();
    h.run_steps(3);
    h.get_all_by_label("MBSTICK").last().unwrap().click();
    wait(&mut h, "export finished", 120, |h| h.query_by_label_contains("to MBSTICK").is_some() && h.query_by_label_contains("Exporting").is_none());
    assert!(h.query_by_label_contains("Exported 5 track(s) to MBSTICK").is_some(), "status shows the export summary");

    let pdb = mb_pdb::DeviceLibrary::open(&stick.join("PIONEER/rekordbox")).unwrap().snapshot();
    assert_eq!(pdb.tracks.len(), 5);
    let devlib = mb_devlib::DevLibDb::open_immutable(&stick.join("PIONEER/rekordbox/exportLibrary.db")).unwrap().load_snapshot(&stick).unwrap();
    assert_eq!(devlib.tracks.len(), 5);
    for t in &pdb.tracks {
        assert!(t.path.is_file(), "audio copied: {}", t.path.display());
        assert!(t.analysis_path.as_ref().is_some_and(|p| p.is_file()), "ANLZ written for {}", t.title);
    }
    let _ = std::fs::remove_dir_all(&root);
}
