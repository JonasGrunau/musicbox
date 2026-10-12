//! Headless UI test (egui_kittest, no OS input): create an intelligent playlist with two rules,
//! check the stored row and the evaluated tracks, switch it to "any", undo. Works on a temp copy
//! of fixtures/oracle-a; skipped without the fixtures. `MB_UI_SHOTS=<dir>` saves renders.

use egui::accesskit::Role;
use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mb_core::smartlist::{MATCH_ALL, MATCH_ANY, SmartList};
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

fn temp_library(tag: &str) -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        return None;
    }
    let root = std::env::temp_dir().join(format!("mb-ui-smart-{tag}-{}", std::process::id()));
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
        img.save(Path::new(&dir).join(format!("smart-{name}.png"))).unwrap();
    }
}

/// (ID, SmartList XML) of the smart list with this name.
fn smart_row(db: &Path, options: &Path, name: &str) -> Option<(String, String)> {
    let paths = mb_masterdb::LibraryPaths::from_options_json(options).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    let db = mb_masterdb::MasterDb::open_read_only(db, &pass).unwrap();
    db.connection()
        .query_row(
            "SELECT ID, SmartList FROM djmdPlaylist WHERE Name = ?1 AND Attribute = 4 AND rb_local_deleted = 0",
            [name],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())),
        )
        .ok()
}

/// Content IDs matching an SQL filter over the copy, as the expectation.
fn expected(db: &Path, options: &Path, filter: &str) -> usize {
    let paths = mb_masterdb::LibraryPaths::from_options_json(options).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    let db = mb_masterdb::MasterDb::open_read_only(db, &pass).unwrap();
    db.connection()
        .query_row(
            &format!(
                "SELECT count(*) FROM djmdContent c LEFT JOIN djmdGenre g ON g.ID = c.GenreID WHERE c.rb_local_deleted = 0 AND ({filter})"
            ),
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap() as usize
}

/// Opens the last combo box currently showing `current` (other panels have combo boxes too) and
/// picks `option` from its popup: the node with that label that appeared when the popup opened
/// (the label may also exist elsewhere, e.g. as a column header).
fn choose(h: &mut Harness<'_, App>, current: &str, option: &str) {
    let before: Vec<egui::Rect> = h.query_all_by_label(option).map(|n| n.rect()).collect();
    h.get_all_by_role(Role::ComboBox)
        .rfind(|n| n.value().as_deref() == Some(current))
        .unwrap_or_else(|| panic!("combo box showing {current}"))
        .click();
    h.run_steps(3);
    h.query_all_by_label(option)
        .find(|n| !before.contains(&n.rect()))
        .unwrap_or_else(|| panic!("option {option}"))
        .click();
    h.run_steps(3);
}

/// Types into the `index`-th text field of the dialog (0 = the list name).
fn type_into(h: &mut Harness<'_, App>, index: usize, text: &str) {
    let field = h.get_all_by_role(Role::TextInput).nth(index).expect("text field");
    field.focus();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let values: Vec<_> = h.get_all_by_role(Role::TextInput).map(|n| (n.value(), n.is_focused())).collect();
    h.get_all_by_role(Role::TextInput)
        .find(|n| n.is_focused())
        .unwrap_or_else(|| panic!("no focused field after focusing {index}: {values:?}"))
        .type_text(text);
    h.run_steps(2);
}

#[test]
fn create_edit_undo_smart_list() {
    let Some((options, db, share, root)) = temp_library("edit") else {
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
    h.get_by_label("Read-only").click();
    wait(&mut h, "editing on", |h| h.query_by_label_contains("Editing on").is_some());

    h.get_by_label("Playlists").click_secondary();
    h.run_steps(3);
    h.get_by_label("New Intelligent Playlist").click();
    wait(&mut h, "editor open", |h| h.query_by_label("Add condition").is_some());
    // The search field is the first text input of the window; the dialog's come after it.
    let first = h
        .get_all_by_role(Role::TextInput)
        .position(|n| n.value().as_deref() == Some("New Intelligent Playlist"))
        .expect("name field");
    type_into(&mut h, first, "kittest smart");
    // Row 1: Genre contains "house".
    choose(&mut h, "Artist", "Genre");
    choose(&mut h, "is", "contains");
    type_into(&mut h, first + 1, "house");
    // Row 2: BPM is in the range 120 to 130.
    h.get_by_label("Add condition").click();
    h.run_steps(3);
    choose(&mut h, "Artist", "BPM");
    choose(&mut h, "is", "is in the range");
    type_into(&mut h, first + 2, "120");
    type_into(&mut h, first + 3, "130");
    shot(&mut h, "01-editor");
    h.get_by_label("OK").click();
    wait(&mut h, "list created", |h| h.query_by_label("kittest smart").is_some());

    let (id, xml) = smart_row(&db, &options, "kittest smart").expect("smart list row");
    let list = SmartList::parse(&xml).unwrap();
    assert_eq!(list.logical_operator, MATCH_ALL);
    assert_eq!(list.id, id.parse::<u32>().unwrap() as i32);
    let props: Vec<_> = list.conditions.iter().map(|c| (c.property.as_str(), c.operator, c.left.as_str(), c.right.as_str())).collect();
    assert_eq!(props, [("genre", 8, "house", ""), ("bpm", 5, "12000", "13000")]);

    // The table shows the evaluated tracks.
    h.get_by_label("kittest smart").click();
    let all = expected(&db, &options, "lower(g.Name) LIKE '%house%' AND c.BPM BETWEEN 12000 AND 13000");
    assert!(all > 0);
    wait(&mut h, "smart rows shown", |h| h.state().browser.visible_count() == all);
    shot(&mut h, "02-tracks");

    // Edit: match any of the conditions.
    h.get_by_label("kittest smart").click_secondary();
    h.run_steps(3);
    h.get_by_label("Edit Intelligent Playlist…").click();
    wait(&mut h, "editor open", |h| h.query_by_label("Add condition").is_some());
    choose(&mut h, "all of the", "any of the");
    h.get_by_label("OK").click();
    let any = expected(&db, &options, "lower(g.Name) LIKE '%house%' OR c.BPM BETWEEN 12000 AND 13000");
    assert!(any > all);
    wait(&mut h, "any rows shown", |h| h.state().browser.visible_count() == any);
    assert_eq!(SmartList::parse(&smart_row(&db, &options, "kittest smart").unwrap().1).unwrap().logical_operator, MATCH_ANY);

    // Undo restores "all".
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "undo applied", |h| h.state().browser.visible_count() == all);
    assert_eq!(smart_row(&db, &options, "kittest smart").unwrap().1, xml);
    shot(&mut h, "03-undone");
    let _ = std::fs::remove_dir_all(&root);
}

/// "Export to ▸" on a smart list: its current tracks land on the device as a plain playlist.
#[test]
fn export_smart_list_to_device() {
    let Some((options, db, share, root)) = temp_library("export") else {
        eprintln!("no fixtures, skipping");
        return;
    };
    // Two small WAV probes: Title is chords_Eb or chords_C.
    let paths = mb_masterdb::LibraryPaths::from_options_json(&options).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    {
        use mb_core::smartlist::{Condition, op};
        let mut lib = mb_library::Library::open_unguarded(&db, &pass).unwrap();
        lib.execute(mb_library::Command::CreateSmartList {
            parent: None,
            name: "mb chords".into(),
            logical_operator: MATCH_ANY,
            conditions: vec![Condition::new("name", op::IS, "", "chords_Eb", ""), Condition::new("name", op::IS, "", "chords_C", "")],
        })
        .unwrap();
    }
    let stick = root.join("MBSTICK");
    std::fs::create_dir_all(&stick).unwrap();
    let args = Args {
        options: Some(options.clone()),
        db: Some(db.clone()),
        analysis_root: Some(share),
        devices: vec![stick.clone()],
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        ..Default::default()
    };
    let mut h = Harness::builder().with_size(Vec2::new(1280.0, 832.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "library + device", |h| h.query_by_label("mb chords").is_some() && h.query_by_label("MBSTICK").is_some());
    h.get_by_label("mb chords").click_secondary();
    h.run_steps(3);
    h.get_by_label_contains("Export to").click();
    h.run_steps(3);
    h.get_all_by_label("MBSTICK").last().unwrap().click();
    wait(&mut h, "export finished", |h| h.query_by_label_contains("to MBSTICK").is_some() && h.query_by_label_contains("Exporting").is_none());
    shot(&mut h, "04-exported");
    assert!(h.query_by_label_contains("Exported 2 track(s) to MBSTICK").is_some(), "status shows the export summary");

    let pdb = mb_pdb::DeviceLibrary::open(&stick.join("PIONEER/rekordbox")).unwrap().snapshot();
    let list = pdb.playlists.iter().find(|p| p.name == "mb chords").expect("device list");
    assert_eq!(list.kind, mb_core::PlaylistKind::Playlist);
    let titles: Vec<String> = pdb.playlist_entries[&list.id]
        .iter()
        .map(|id| pdb.tracks.iter().find(|t| &t.id == id).unwrap().title.clone())
        .collect();
    let mut sorted = titles.clone();
    sorted.sort();
    assert_eq!(sorted, ["chords_C", "chords_Eb"]);
    let _ = std::fs::remove_dir_all(&root);
}
