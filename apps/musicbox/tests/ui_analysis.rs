//! Headless UI test (egui_kittest; no OS mouse/keyboard, no window): "Analyze Track" on a track
//! whose analysis was removed, against a temp copy of fixtures/oracle-a, then undo. Skipped without
//! the fixtures or the corpus audio. Screenshots: set MB_UI_SHOTS=<dir>.

use egui::accesskit::Role;
use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mb_anlz::{AnlzFile, Tag};
use musicbox::{App, Args};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TRACK: &str = "chords_Am";

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to);
        } else if !entry.file_name().to_string_lossy().ends_with("-shm") && !entry.file_name().to_string_lossy().ends_with("-wal") {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

struct Lib {
    options: PathBuf,
    db: PathBuf,
    share: PathBuf,
    root: PathBuf,
}

impl Drop for Lib {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn temp_library(name: &str) -> Option<Lib> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        return None;
    }
    let root = std::env::temp_dir().join(format!("mb-ui-analysis-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&fixture.join("rekordbox"), &root.join("rekordbox"));
    std::fs::copy(fixture.join("options.json"), root.join("options.json")).unwrap();
    Some(Lib { options: root.join("options.json"), db: root.join("rekordbox/master.db"), share: root.join("rekordbox/share"), root })
}

fn passphrase(lib: &Lib) -> String {
    let paths = mb_masterdb::LibraryPaths::from_options_json(&lib.options).unwrap();
    mb_masterdb::derive_passphrase(&paths.dp).unwrap()
}

fn query<T>(lib: &Lib, f: impl FnOnce(&mb_masterdb::rusqlite::Connection) -> T) -> T {
    let db = mb_masterdb::MasterDb::open_read_only(&lib.db, &passphrase(lib)).unwrap();
    f(db.connection())
}

fn analysed(lib: &Lib, id: &str) -> (i64, Option<String>) {
    query(lib, |c| c.query_row("SELECT Analysed, AnalysisDataPath FROM djmdContent WHERE ID = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap())
}

fn wait(h: &mut Harness<'_, App>, what: &str, secs: u64, mut cond: impl FnMut(&Harness<'_, App>) -> bool) {
    let start = Instant::now();
    while !cond(h) {
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(50));
    }
    h.run_steps(3);
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    if let Ok(img) = h.render() {
        img.save(Path::new(&dir).join(format!("analysis-{name}.png"))).unwrap();
    }
}

#[test]
fn analyze_track_then_undo() {
    let Some(lib) = temp_library("analyze") else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let (content, rel, audio): (String, String, String) = query(&lib, |c| {
        c.query_row("SELECT ID, AnalysisDataPath, FolderPath FROM djmdContent WHERE Title = ?1", [TRACK], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
    });
    if !Path::new(&audio).exists() {
        eprintln!("corpus audio missing, skipping");
        return;
    }
    // Back to the state after import: no analysis files, no analysis columns, no gain row.
    let dat = lib.share.join(rel.trim_start_matches('/'));
    for ext in ["DAT", "EXT", "2EX"] {
        std::fs::remove_file(dat.with_extension(ext)).unwrap();
    }
    {
        let w = mb_masterdb::MasterDbWriter::open_unguarded(&lib.db, &passphrase(&lib)).unwrap();
        w.connection()
            .execute_batch(&format!(
                "UPDATE djmdContent SET Analysed = 0, ContentLink = 14, AnalysisDataPath = NULL WHERE ID = '{content}';
                 DELETE FROM djmdMixerParam WHERE ContentID = '{content}';
                 DELETE FROM contentFile WHERE ContentID = '{content}';"
            ))
            .unwrap();
    }
    let args = Args {
        options: Some(lib.options.clone()),
        db: Some(lib.db.clone()),
        analysis_root: Some(lib.share.clone()),
        backup_root: Some(lib.root.join("backups")),
        no_volume_scan: true,
        ..Default::default()
    };
    let mut h = Harness::builder().with_size(Vec2::new(1400.0, 900.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "library loaded", 60, |h| h.query_by_label("oracle-small").is_some());
    h.get_by_label("Read-only").click();
    wait(&mut h, "editing on", 60, |h| h.query_by_label_contains("Editing on").is_some());

    h.get_all_by_label(TRACK).next().unwrap().click_secondary();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Analyze Track").click();
    h.run_steps(2);
    shot(&mut h, "01-running");
    wait(&mut h, "analysis written", 240, |_| analysed(&lib, &content).0 == 105);
    let (_, path) = analysed(&lib, &content);
    assert_eq!(path.as_deref(), Some(rel.as_str()));
    let file = AnlzFile::parse(&std::fs::read(&dat).unwrap()).unwrap();
    assert!(file.beat_grid().is_some_and(|g| !g.beats.is_empty()), "PQTZ written");
    let ext = AnlzFile::parse(&std::fs::read(dat.with_extension("EXT")).unwrap()).unwrap();
    assert!(ext.sections_with(Tag::PWV5).next().is_some(), "colour waveform written");
    assert!(dat.with_extension("2EX").exists());
    wait(&mut h, "status", 30, |h| h.query_by_label_contains("Analysed 1 track").is_some());
    shot(&mut h, "02-done");

    // Undo puts the imported state back.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "undo", 60, |_| analysed(&lib, &content).0 == 0);
    assert_eq!(analysed(&lib, &content).1, None);
    assert!(!dat.exists() && !dat.with_extension("EXT").exists() && !dat.with_extension("2EX").exists());
    shot(&mut h, "03-undone");
}

/// "Analysis Lock On" / "Analysis Lock Off" in the track menu set and clear `Analysed` bit 0x80.
#[test]
fn analysis_lock_menu() {
    let Some(lib) = temp_library("lock") else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let content: String = query(&lib, |c| c.query_row("SELECT ID FROM djmdContent WHERE Title = ?1", [TRACK], |r| r.get(0)).unwrap());
    let args = Args {
        options: Some(lib.options.clone()),
        db: Some(lib.db.clone()),
        analysis_root: Some(lib.share.clone()),
        backup_root: Some(lib.root.join("backups")),
        no_volume_scan: true,
        ..Default::default()
    };
    let mut h = Harness::builder().with_size(Vec2::new(1400.0, 900.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "library loaded", 60, |h| h.query_by_label("oracle-small").is_some());
    h.get_by_label("Read-only").click();
    wait(&mut h, "editing on", 60, |h| h.query_by_label_contains("Editing on").is_some());
    let before = analysed(&lib, &content).0;
    assert_eq!(before & 0x80, 0);

    h.get_all_by_label(TRACK).next().unwrap().click_secondary();
    h.run_steps(2);
    assert!(h.query_by_label("Analysis Lock Off").is_none());
    h.get_by_role_and_label(Role::Button, "Analysis Lock On").click();
    wait(&mut h, "locked", 60, |_| analysed(&lib, &content).0 == before | 0x80);
    h.run_steps(3);

    // The table reloads after the write; reopen the menu until it shows the locked state.
    let start = std::time::Instant::now();
    loop {
        // After the reload the title also shows on the deck; try each match until the table
        // row's menu opens.
        let n = h.get_all_by_label(TRACK).count();
        for i in 0..n {
            h.key_press(Key::Escape);
            h.run_steps(2);
            h.get_all_by_label(TRACK).nth(i).unwrap().click_secondary();
            h.run_steps(2);
            if h.query_by_label("Analyze Track").is_some() {
                break;
            }
        }
        if h.query_by_label("Analysis Lock Off").is_some() {
            break;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(60),
            "menu never offered Analysis Lock Off (menu open: {}, lock on offered: {}, matches: {})",
            h.query_by_label("Analyze Track").is_some(),
            h.query_by_label("Analysis Lock On").is_some(),
            h.get_all_by_label(TRACK).count()
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(h.query_by_label("Analysis Lock On").is_none());
    shot(&mut h, "lock-menu");
    h.get_by_role_and_label(Role::Button, "Analysis Lock Off").click();
    wait(&mut h, "unlocked", 60, |_| analysed(&lib, &content).0 == before);
}
