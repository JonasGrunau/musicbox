//! Headless UI test: "Import to Collection" on a device playlist, undo, and "Import Histories".
//! The library is a temp copy of fixtures/sandbox-initial; the devices are APFS clones of
//! rekordbox's RBTEST export (with the oracle-small audio put back from the corpus) and of the
//! reference stick. Skipped without fixtures or corpus. No OS input, no window.
//! Screenshots: set MB_UI_SHOTS=<dir> to save rendered frames for review.

use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use musicbox::{App, Args};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn fixture(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
    p.exists().then_some(p)
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

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(img) = h.render() {
        let _ = img.save(Path::new(&dir).join(format!("{name}.png")));
    }
}

fn clone(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    assert!(std::process::Command::new("/bin/cp").arg("-cR").arg(src).arg(dst).status().unwrap().success());
}

fn open(options: &Path, db: &Path) -> mb_masterdb::MasterDb {
    let paths = mb_masterdb::LibraryPaths::from_options_json(options).unwrap();
    mb_masterdb::MasterDb::open_read_only(db, &mb_masterdb::derive_passphrase(&paths.dp).unwrap()).unwrap()
}

/// RBTEST clone with the oracle-small audio under /Contents. `None` without the corpus.
fn rbtest_with_audio(dst: &Path) -> Option<()> {
    let oracle = fixture("oracle-a")?;
    clone(&oracle.join("rbtest/PIONEER"), &dst.join("PIONEER"));
    let db = mb_pdb::ExportDb::read(&mb_pdb::Pdb::parse(std::fs::read(dst.join("PIONEER/rekordbox/export.pdb")).ok()?).ok()?).ok()?;
    let list = db.playlists.iter().find(|p| p.0.name == "oracle-small")?.0.id;
    let local = open(&oracle.join("options.json"), &oracle.join("rekordbox/master.db"));
    for e in db.playlist_entries.iter().filter(|e| e.playlist_id == list) {
        let t = db.tracks.iter().find(|t| t.id == e.track_id)?;
        let src: String = local
            .connection()
            .query_row("SELECT FolderPath FROM djmdContent WHERE ID = ?1", [t.master_content_id.to_string()], |r| r.get(0))
            .ok()?;
        if !Path::new(&src).is_file() {
            return None;
        }
        let dest = dst.join(t.folder_path.trim_start_matches('/'));
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        assert!(std::process::Command::new("/bin/cp").arg("-c").arg(&src).arg(&dest).status().unwrap().success());
    }
    Some(())
}

#[test]
fn import_playlist_undo_and_histories() {
    let (Some(sandbox), Some(stick)) = (fixture("sandbox-initial"), fixture("stick")) else {
        eprintln!("skipped: fixtures missing");
        return;
    };
    let root = std::env::temp_dir().join(format!("mb-ui-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let device = root.join("RBCLONE");
    if rbtest_with_audio(&device).is_none() {
        eprintln!("skipped: fixtures/oracle-a or the corpus audio is missing");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    clone(&stick.join("PIONEER"), &root.join("STICKCLONE/PIONEER"));
    // The library: master.db (+ playlists XML) of sandbox-initial, a fresh analysis root.
    std::fs::create_dir_all(root.join("lib/rekordbox")).unwrap();
    for f in ["master.db", "masterPlaylists6.xml"] {
        let src = sandbox.join("rekordbox").join(f);
        if src.exists() {
            std::fs::copy(src, root.join("lib/rekordbox").join(f)).unwrap();
        }
    }
    std::fs::copy(sandbox.join("options.json"), root.join("lib/options.json")).unwrap();
    let (options, db, share) = (root.join("lib/options.json"), root.join("lib/rekordbox/master.db"), root.join("lib/share"));
    std::fs::create_dir_all(&share).unwrap();
    let count = |sql: &str| -> i64 { open(&options, &db).connection().query_row(sql, [], |r| r.get(0)).unwrap() };
    let tracks_before = count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0");

    let args = Args {
        options: Some(options.clone()),
        db: Some(db.clone()),
        analysis_root: Some(share.clone()),
        devices: vec![device.clone(), root.join("STICKCLONE")],
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        import_folder: Some(root.join("Imported")),
    };
    let mut h = Harness::builder().with_size(Vec2::new(1280.0, 1800.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "library and devices", 60, |h| h.query_by_label("Read-only").is_some() && h.query_by_label("oracle-small").is_some());
    // The switch does nothing until the local library has loaded; click until editing starts.
    let start = Instant::now();
    while h.query_by_label_contains("Editing on").is_none() {
        assert!(start.elapsed() < Duration::from_secs(180), "editing did not turn on");
        if h.query_by_label("Read-only").is_some() {
            h.get_by_label("Read-only").click();
        }
        h.run_steps(3);
        std::thread::sleep(Duration::from_millis(100));
    }
    shot(&mut h, "import-0-editing");

    // 1. Import the device playlist.
    h.get_by_label("oracle-small").click_secondary();
    h.run_steps(3);
    shot(&mut h, "import-1-menu");
    h.get_by_label("Import to Collection").click();
    wait(&mut h, "import done", 60, |h| h.query_by_label_contains("Imported 5 track(s) from the device").is_some());
    shot(&mut h, "import-2-done");
    assert!(h.query_by_label_contains("5 device row(s) linked to the collection").is_some(), "stick rows stamped");
    assert_eq!(count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0"), tracks_before + 5);
    assert_eq!(count("SELECT count(*) FROM djmdPlaylist WHERE rb_local_deleted = 0 AND Name = 'oracle-small'"), 1);
    assert_eq!(count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0 AND Analysed = 105 AND AnalysisDataPath LIKE '/PIONEER/USBANLZ/%' AND FolderPath LIKE '%/Imported/%'"), 5);
    let pdb = mb_pdb::ExportDb::read(&mb_pdb::Pdb::parse(std::fs::read(device.join("PIONEER/rekordbox/export.pdb")).unwrap()).unwrap()).unwrap();
    let stamped = pdb
        .tracks
        .iter()
        .filter(|t| count(&format!("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0 AND ID = '{}' AND FolderPath LIKE '%Imported%'", t.master_content_id)) == 1)
        .count();
    assert_eq!(stamped, 5);
    wait(&mut h, "local playlist shown", 30, |h| h.get_all_by_label("oracle-small").count() == 2);

    // 2. Undo removes the tracks, the playlist and the copied files.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "undone", 60, |h| h.query_by_label_contains("Removed 5 track(s)").is_some());
    assert_eq!(count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0"), tracks_before);
    fn files(dir: &Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |d| {
            d.flatten().map(|e| if e.path().is_dir() { files(&e.path()) } else { 1 }).sum()
        })
    }
    assert_eq!(files(&root.join("Imported")), 0, "undo deleted the copied audio");
    assert_eq!(files(&share), 0, "undo deleted the local analysis and artwork files");

    // 3. Histories of the reference stick clone: none of its tracks is in this collection. (The
    // window is tall enough to show the clone's 30 lists and its Histories header.)
    // Let the layout settle (the status line changes height), then right-click the header.
    h.run_steps(5);
    let r = h.get_by_label("Histories").rect();
    h.hover_at(r.center());
    h.run_steps(2);
    let pos = h.get_by_label("Histories").rect().center();
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    shot(&mut h, "import-3-menu");
    h.get_by_label("Import Histories").click();
    wait(&mut h, "history import", 60, |h| h.query_by_label_contains("Imported 0 history list(s), skipped 21").is_some());
    shot(&mut h, "import-3-histories");
    assert_eq!(count("SELECT count(*) FROM djmdHistory WHERE rb_local_deleted = 0"), 0);
    let _ = std::fs::remove_dir_all(&root);
}
