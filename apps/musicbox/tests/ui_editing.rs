//! Headless UI test: drives the real App through egui_kittest (no OS mouse/keyboard, no window).
//! Works on a temp copy of fixtures/oracle-a; skipped when fixtures are absent.
//! Screenshots: set MB_UI_SHOTS=<dir> to save rendered frames for review.

use egui::accesskit::Role;
use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use musicbox::{App, Args};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// A private copy of the oracle-a library: (options.json, master.db, analysis root, temp root).
fn temp_library() -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        return None;
    }
    let root = std::env::temp_dir().join(format!("mb-ui-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&fixture.join("rekordbox"), &root.join("rekordbox"));
    std::fs::copy(fixture.join("options.json"), root.join("options.json")).unwrap();
    Some((root.join("options.json"), root.join("rekordbox/master.db"), root.join("rekordbox/share"), root))
}

fn wait(h: &mut Harness<'_, App>, what: &str, cond: impl Fn(&Harness<'_, App>) -> bool) {
    let start = Instant::now();
    while !cond(h) {
        assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    h.run_steps(2);
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    if let Ok(img) = h.render() {
        img.save(Path::new(&dir).join(format!("{name}.png"))).unwrap();
    }
}

fn entries(db: &Path, options: &Path, playlist: &str) -> usize {
    let paths = mb_masterdb::LibraryPaths::from_options_json(options).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    let snap = mb_masterdb::MasterDb::open_read_only(db, &pass).unwrap().load_snapshot(&paths).unwrap();
    let node = snap.playlists.iter().find(|p| p.name == playlist).expect("playlist exists");
    snap.playlist_entries.get(&node.id).map_or(0, Vec::len)
}

#[test]
fn create_playlist_drag_track_undo_redo() {
    let Some((options, db, share, root)) = temp_library() else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let args = Args {
        options: Some(options.clone()),
        db: Some(db.clone()),
        analysis_root: Some(share),
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        ..Default::default()
    };
    let mut h = Harness::builder().with_size(Vec2::new(1280.0, 832.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });

    wait(&mut h, "library loaded", |h| h.query_by_label("oracle-small").is_some());
    shot(&mut h, "01-loaded");

    h.get_by_label("Read-only").click();
    wait(&mut h, "editing on", |h| h.query_by_label_contains("Editing on").is_some());
    assert!(root.join("backups").read_dir().unwrap().next().is_some(), "backup written before editing");

    h.get_by_label("Playlists").click_secondary();
    h.run_steps(3);
    h.get_by_label("New Playlist").click();
    h.run_steps(3);
    let input = h
        .get_all_by_role(Role::TextInput)
        .find(|n| n.value().as_deref() == Some("New Playlist"))
        .expect("name field");
    input.focus();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_all_by_role(Role::TextInput).find(|n| n.is_focused()).unwrap().type_text("kittest list");
    h.run_steps(2);
    shot(&mut h, "02-dialog");
    h.key_press(Key::Enter);
    wait(&mut h, "playlist created", |h| h.query_by_label("kittest list").is_some());
    assert_eq!(entries(&db, &options, "kittest list"), 0);

    // Drag the first track row onto the new playlist.
    let from = h.get_all_by_label("chords_Am").next().unwrap().rect().center();
    let to = h.get_by_label("kittest list").rect().center();
    h.drag_at(from);
    h.step();
    for i in 1..=10 {
        h.hover_at(from + (to - from) * (i as f32 / 10.0));
        h.step();
    }
    shot(&mut h, "03-dragging");
    h.drop_at(to);
    wait(&mut h, "track added", |h| h.query_by_label_contains("Added 1 track").is_some());
    assert_eq!(entries(&db, &options, "kittest list"), 1);

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "undo", |h| h.query_by_label("Done").is_some());
    assert_eq!(entries(&db, &options, "kittest list"), 0, "undo removed the entry");

    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    wait(&mut h, "redo", |h| h.query_by_label_contains("Added 1 track").is_some());
    assert_eq!(entries(&db, &options, "kittest list"), 1, "redo re-added the entry");

    h.get_by_label("kittest list").click();
    h.run_steps(3);
    shot(&mut h, "04-playlist");
    let _ = std::fs::remove_dir_all(&root);
}
