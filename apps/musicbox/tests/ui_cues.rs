//! Headless UI test (egui_kittest; no OS mouse/keyboard, no window): cue and beat-grid editing on
//! the deck, with undo, against a temp copy of fixtures/oracle-a. Skipped without fixtures.
//! Screenshots: set MB_UI_SHOTS=<dir>.

use egui::accesskit::Role;
use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use mb_anlz::AnlzFile;
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

fn temp_library() -> Option<Lib> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        return None;
    }
    let root = std::env::temp_dir().join(format!("mb-ui-cues-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&fixture.join("rekordbox"), &root.join("rekordbox"));
    std::fs::copy(fixture.join("options.json"), root.join("options.json")).unwrap();
    Some(Lib { options: root.join("options.json"), db: root.join("rekordbox/master.db"), share: root.join("rekordbox/share"), root })
}

fn wait(h: &mut Harness<'_, App>, what: &str, mut cond: impl FnMut(&Harness<'_, App>) -> bool) {
    let start = Instant::now();
    while !cond(h) {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(30));
    }
    h.run_steps(3);
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    if let Ok(img) = h.render() {
        img.save(Path::new(&dir).join(format!("cues-{name}.png"))).unwrap();
    }
}

fn click_button(h: &Harness<'_, App>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
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

/// Query helper on a fresh read-only connection.
fn query<T>(lib: &Lib, f: impl FnOnce(&mb_masterdb::rusqlite::Connection) -> T) -> T {
    let paths = mb_masterdb::LibraryPaths::from_options_json(&lib.options).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    let db = mb_masterdb::MasterDb::open_read_only(&lib.db, &pass).unwrap();
    f(db.connection())
}

/// (ID, Kind, InMsec, Comment) of the track's cues in row order.
fn cues(lib: &Lib, content: &str) -> Vec<(String, i64, i64, Option<String>)> {
    query(lib, |c| {
        let mut stmt = c.prepare("SELECT ID, Kind, InMsec, Comment FROM djmdCue WHERE ContentID = ?1 ORDER BY rowid").unwrap();
        stmt.query_map([content], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
    })
}

fn bpm(lib: &Lib, content: &str) -> i64 {
    query(lib, |c| c.query_row("SELECT BPM FROM djmdContent WHERE ID = ?1", [content], |r| r.get(0)).unwrap())
}

#[test]
fn cues_and_grid_from_the_deck() {
    let Some(lib) = temp_library() else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let (content, rel): (String, String) =
        query(&lib, |c| c.query_row("SELECT ID, AnalysisDataPath FROM djmdContent WHERE Title = ?1", [TRACK], |r| Ok((r.get(0)?, r.get(1)?))).unwrap());
    let dat = lib.share.join(rel.trim_start_matches('/'));
    let dat0 = std::fs::read(&dat).unwrap();
    let bpm0 = bpm(&lib, &content);
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

    wait(&mut h, "library loaded", |h| h.query_by_label("oracle-small").is_some());
    h.get_all_by_label(TRACK).next().unwrap().click();
    wait(&mut h, "deck controls", |h| h.query_by_label("Memory Cue").is_some());
    h.get_by_label("Read-only").click();
    wait(&mut h, "editing on", |h| h.query_by_label_contains("Editing on").is_some());
    shot(&mut h, "01-deck");

    // Move the cursor by clicking the detail strip right of its centre line (it sits just above
    // the cue row), then add a memory cue there.
    let button = h.get_by_label("Memory Cue").rect();
    click_at(&mut h, egui::pos2(button.left() + 700.0, button.top() - 30.0));
    h.run_steps(3);
    click_button(&h, "Memory Cue");
    wait(&mut h, "memory cue", |_| cues(&lib, &content).len() == 1);
    let first = cues(&lib, &content);
    assert_eq!(first[0].1, 0, "memory cue kind");
    assert!(first[0].2 > 0, "cue at the clicked position, got {} ms", first[0].2);

    // Hot cue A at the same position.
    click_button(&h, "A");
    wait(&mut h, "hot cue", |_| cues(&lib, &content).len() == 2);
    assert_eq!(cues(&lib, &content)[1].1, 1, "hot cue A kind");

    // Select pad A, comment it, delete it, undo the delete.
    click_button(&h, "A");
    h.run_steps(3);
    let field = h.get_all_by_role(Role::TextInput).find(|n| n.accesskit_node().placeholder() == Some("Comment")).expect("comment field");
    field.focus();
    h.run_steps(1);
    h.get_all_by_role(Role::TextInput).find(|n| n.is_focused()).unwrap().type_text("kittest drop");
    h.run_steps(2);
    click_button(&h, "Save Comment");
    wait(&mut h, "comment saved", |_| cues(&lib, &content)[1].3.as_deref() == Some("kittest drop"));
    shot(&mut h, "02-cues");
    click_button(&h, "Delete Cue");
    wait(&mut h, "cue deleted", |_| cues(&lib, &content).len() == 1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "delete undone", |_| cues(&lib, &content).len() == 2);
    assert_eq!(cues(&lib, &content)[1].3.as_deref(), Some("kittest drop"));

    // Grid: shift right by 1 ms, then double the tempo; undo both.
    let first_beat = |p: &Path| AnlzFile::parse(&std::fs::read(p).unwrap()).unwrap().beat_grid().unwrap().beats[0].time_ms;
    let t0 = first_beat(&dat);
    click_button(&h, "Grid ▶");
    wait(&mut h, "grid shifted", |_| first_beat(&dat) != t0);
    click_button(&h, "BPM ×2");
    wait(&mut h, "tempo doubled", |_| bpm(&lib, &content) == bpm0 * 2);
    shot(&mut h, "03-grid");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "tempo undone", |_| bpm(&lib, &content) == bpm0);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "shift undone", |_| std::fs::read(&dat).unwrap() == dat0);
    assert!(lib.root.join("backups").read_dir().unwrap().flatten().any(|d| d.path().join("anlz").exists()), "analysis files backed up");
    shot(&mut h, "04-undone");
    let _ = std::fs::remove_dir_all(&lib.root);
}
