//! Headless design review (egui_kittest; no OS mouse/keyboard, no window): walks the main views of
//! a temp copy of fixtures/oracle-cue/A3 (hot cues and artwork on every real track) with the
//! oracle-a RBTEST export as a device, and saves a render of each.
//! `MB_UI_SHOTS=<dir> cargo test -p musicbox --test ui_design`; skipped without fixtures.

use egui::accesskit::Role;
use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use musicbox::{App, Args};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TRACK: &str = "Lift Off (Original Mix)";

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

fn wait(h: &mut Harness<'_, App>, what: &str, cond: impl Fn(&Harness<'_, App>) -> bool) {
    let start = Instant::now();
    while !cond(h) {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    h.run_steps(4);
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    // Artwork loads on a worker thread; give it a few frames.
    for _ in 0..10 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Ok(img) = h.render() {
        img.save(Path::new(&dir).join(format!("design-{name}.png"))).unwrap();
    }
}

/// Selects `TRACK` through the search field (the table only lays out visible rows), then clears
/// the search again; the selection stays.
fn select_track(h: &mut Harness<'_, App>) {
    let search = h.get_all_by_role(Role::TextInput).find(|n| n.accesskit_node().placeholder() == Some("Search")).expect("search field");
    search.focus();
    h.run_steps(1);
    h.get_all_by_role(Role::TextInput).find(|n| n.is_focused()).unwrap().type_text("Lift Off");
    h.run_steps(3);
    h.get_all_by_label(TRACK).next().expect("track row").click();
    wait(h, "deck", |h| h.query_by_label("Memory Cue").is_some());
    let search = h.get_all_by_role(Role::TextInput).find(|n| n.value().as_deref() == Some("Lift Off")).expect("search field with text");
    search.focus();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.key_press(Key::Backspace);
    h.key_press(Key::Escape);
    h.run_steps(4);
}

/// A plain left click at a position (press + release).
fn click_at(h: &mut Harness<'_, App>, pos: egui::Pos2) {
    h.hover_at(pos);
    h.step();
    h.drag_at(pos);
    h.step();
    h.drop_at(pos);
    h.step();
}

fn harness(size: Vec2, args: Args) -> Harness<'static, App> {
    Harness::builder().with_size(size).build_eframe(move |cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    })
}

struct Fixture {
    root: PathBuf,
    args: Args,
}

fn fixture() -> Option<Fixture> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let library = fixtures.join("oracle-cue/A3");
    let device = fixtures.join("oracle-a/rbtest");
    if !library.exists() || !device.exists() {
        return None;
    }
    let root = std::env::temp_dir().join(format!("mb-ui-design-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&library.join("rekordbox"), &root.join("rekordbox"));
    copy_dir(&device, &root.join("RBTEST"));
    std::fs::copy(library.join("options.json"), root.join("options.json")).unwrap();
    let args = Args {
        options: Some(root.join("options.json")),
        db: Some(root.join("rekordbox/master.db")),
        analysis_root: Some(root.join("rekordbox/share")),
        devices: vec![root.join("RBTEST")],
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        ..Default::default()
    };
    Some(Fixture { root, args })
}

#[test]
fn design_views() {
    let Some(fx) = fixture() else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let mut h = harness(Vec2::new(1440.0, 900.0), fx.args.clone());
    wait(&mut h, "library loaded", |h| h.query_all_by_label("oracle-small").next().is_some());
    shot(&mut h, "01-empty-deck");

    select_track(&mut h);
    // Move the cursor into the track without playing: each click near the right end of the zoomed
    // strip moves it about 4 s on (a click on the overview would start the preview player).
    let play = h.get_by_label("Play").rect();
    for _ in 0..12 {
        click_at(&mut h, egui::pos2(1420.0, play.top() - 70.0));
    }
    h.run_steps(4);
    assert!(h.query_by_label("Pause").is_none(), "moving the cursor must not start the preview player");
    shot(&mut h, "02-collection");

    h.get_all_by_label("oracle-small").next().unwrap().click();
    h.run_steps(4);
    shot(&mut h, "03-playlist");

    h.get_by_label("Read-only").click();
    wait(&mut h, "editing on", |h| h.query_by_label_contains("Editing on").is_some());
    h.get_by_label("Collection").click();
    h.run_steps(2);
    select_track(&mut h);
    shot(&mut h, "04-editing");

    h.get_all_by_label("Playlists").next().unwrap().click_secondary();
    h.run_steps(2);
    shot(&mut h, "05-menu");
    h.get_by_label("New Playlist").click();
    h.run_steps(4);
    shot(&mut h, "06-dialog");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    let device = h.query_all_by_label("All Tracks").next().is_some();
    if device {
        h.get_all_by_label("All Tracks").next().unwrap().click();
        h.run_steps(4);
        shot(&mut h, "07-device");
    }
    drop(h);

    // The smallest window size the app allows.
    let mut h = harness(Vec2::new(960.0, 600.0), fx.args.clone());
    wait(&mut h, "library loaded", |h| h.query_all_by_label("oracle-small").next().is_some());
    select_track(&mut h);
    shot(&mut h, "08-small");
    drop(h);
    let _ = std::fs::remove_dir_all(&fx.root);
}
