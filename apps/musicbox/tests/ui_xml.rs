//! Headless UI test: the "rekordbox xml" library (rekordbox's bridge view), "Import to
//! Collection" on one of its playlists, undo, and "File → Export Collection in xml format".
//! The library is a temp copy of fixtures/sandbox-initial whose settings folder points the
//! bridge (`showRbXml`, `bridgeImportedLibraryFile`) at an XML exported from fixtures/oracle-a.
//! Skipped without fixtures or the corpus audio. No OS input, no window.
//! Screenshots: set MB_UI_SHOTS=<dir> to save rendered frames for review.

use egui::accesskit::Role;
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

fn open(options: &Path, db: &Path) -> mb_masterdb::MasterDb {
    let paths = mb_masterdb::LibraryPaths::from_options_json(options).unwrap();
    mb_masterdb::MasterDb::open_read_only(db, &mb_masterdb::derive_passphrase(&paths.dp).unwrap()).unwrap()
}

/// oracle-a's collection as rekordbox.xml (written by musicbox's exporter).
fn oracle_xml(to: &Path) -> Option<()> {
    let oracle = fixture("oracle-a")?;
    let mut paths = mb_masterdb::LibraryPaths::from_options_json(&oracle.join("options.json")).ok()?;
    paths.analysis_data_root = oracle.join("rekordbox/share");
    let db = open(&oracle.join("options.json"), &oracle.join("rekordbox/master.db"));
    let snapshot = db.load_snapshot(&paths).ok()?;
    let doc = mb_library::xml::export_document(db.connection(), &snapshot, &mb_library::XmlExportOptions::default()).ok()?;
    mb_library::xml::write_document(&doc, to).ok()
}

/// `options.json` with `settings-root-path` replaced.
fn with_settings_root(options: &str, settings: &Path) -> String {
    let key = "\"settings-root-path\",\"";
    let start = options.find(key).expect("settings-root-path") + key.len();
    let end = start + options[start..].find('"').unwrap();
    format!("{}{}{}", &options[..start], settings.display(), &options[end..])
}

#[test]
fn xml_bridge_import_undo_and_export() {
    let Some(sandbox) = fixture("sandbox-initial") else {
        eprintln!("skipped: fixtures missing");
        return;
    };
    let root = std::env::temp_dir().join(format!("mb-ui-xml-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("lib/rekordbox")).unwrap();
    let xml = root.join("oracle.xml");
    if oracle_xml(&xml).is_none() {
        eprintln!("skipped: fixtures/oracle-a missing");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    let doc = mb_xml::Document::parse(&std::fs::read_to_string(&xml).unwrap()).unwrap();
    let small_audio_present = doc
        .tracks()
        .iter()
        .filter_map(|t| mb_xml::export::location_from_uri(t.location.as_deref()?))
        .take(5)
        .all(|p| Path::new(&p).is_file());
    if !small_audio_present {
        eprintln!("skipped: the corpus audio is missing");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    for f in ["master.db", "masterPlaylists6.xml"] {
        let src = sandbox.join("rekordbox").join(f);
        if src.exists() {
            std::fs::copy(src, root.join("lib/rekordbox").join(f)).unwrap();
        }
    }
    let settings = root.join("settings");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(
        settings.join("rekordbox3.settings"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<PROPERTIES>\n  <VALUE name=\"showRbXml\" val=\"1\"/>\n  <VALUE name=\"bridgeImportedLibraryFile\" val=\"{}\"/>\n</PROPERTIES>\n",
            xml.display()
        ),
    )
    .unwrap();
    let options_text = std::fs::read_to_string(sandbox.join("options.json")).unwrap();
    std::fs::write(root.join("lib/options.json"), with_settings_root(&options_text, &settings)).unwrap();
    let (options, db, share) = (root.join("lib/options.json"), root.join("lib/rekordbox/master.db"), root.join("lib/share"));
    std::fs::create_dir_all(&share).unwrap();
    let count = |sql: &str| -> i64 { open(&options, &db).connection().query_row(sql, [], |r| r.get(0)).unwrap() };
    let tracks_before = count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0");

    let args = Args {
        options: Some(options.clone()),
        db: Some(db.clone()),
        analysis_root: Some(share.clone()),
        devices: Vec::new(),
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        import_folder: None,
    };
    let mut h = Harness::builder().with_size(Vec2::new(1280.0, 1400.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    wait(&mut h, "library and rekordbox xml", 60, |h| h.query_by_label("rekordbox xml").is_some() && h.query_by_label("oracle-small").is_some());
    let start = Instant::now();
    while h.query_by_label_contains("Editing on").is_none() {
        assert!(start.elapsed() < Duration::from_secs(180), "editing did not turn on");
        if h.query_by_label("Read-only").is_some() {
            h.get_by_label("Read-only").click();
        }
        h.run_steps(3);
        std::thread::sleep(Duration::from_millis(100));
    }
    shot(&mut h, "xml-0-bridge");

    // 1. Import the XML playlist (no track or list of that name in the sandbox: no questions).
    h.get_by_label("oracle-small").click_secondary();
    h.run_steps(3);
    shot(&mut h, "xml-1-menu");
    h.get_by_label("Import to Collection").click();
    wait(&mut h, "import done", 60, |h| h.query_by_label_contains("Imported 5 track(s) from rekordbox xml").is_some());
    shot(&mut h, "xml-2-imported");
    assert_eq!(count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0"), tracks_before + 5);
    assert_eq!(count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0 AND Analysed = 16 AND AnalysisDataPath LIKE '/PIONEER/USBANLZ/%'"), 5);
    assert_eq!(
        count("SELECT count(*) FROM djmdSongPlaylist s JOIN djmdPlaylist p ON p.ID = s.PlaylistID WHERE p.Name = 'oracle-small' AND p.rb_local_deleted = 0 AND s.rb_local_deleted = 0"),
        5
    );

    // 2. Undo removes the tracks and the list again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait(&mut h, "undo", 60, |_| count("SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0") == tracks_before);
    assert_eq!(count("SELECT count(*) FROM djmdPlaylist WHERE rb_local_deleted = 0 AND Name = 'oracle-small'"), 0);

    // 3. Export the collection.
    let out = root.join("export.xml");
    // "File → Export Collection in xml format" lives in the native menu bar; send its command.
    h.state_mut().menu_command(musicbox::MenuCommand::ExportXml);
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_all_by_role(Role::TextInput).find(|n| n.is_focused()).expect("path field").type_text(&out.display().to_string());
    h.run_steps(2);
    h.key_press(Key::Enter);
    wait(&mut h, "export done", 60, |h| h.query_by_label_contains("Exported").is_some());
    shot(&mut h, "xml-3-exported");
    let exported = mb_xml::Document::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(exported.tracks().len() as i64, tracks_before);
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}
