//! Exports `oracle-small`, then `oracle-all` from `fixtures/oracle-a` into a temp device and
//! compares the result with rekordbox's own export of the same two playlists
//! (`fixtures/oracle-a/rbtest`, `docs/oracle-session.md`). Skipped without the fixtures or the
//! corpus audio (`~/re-work/corpus`, read-only).
//!
//! The second session replays what rekordbox did on the oracle stick (`docs/re/pdb.md` § The
//! oracle stick's history): ten close/open cycles before it, `oracle-small` emptied and its
//! five entries re-added, `oracle-all` inserted at position 1 and moved to the top after that,
//! one more cycle afterwards. With it both `.pdb` files come out byte for byte.
//!
//! `MB_USB_ORACLE_OUT=<dir>` keeps the exported device in `<dir>` (must not exist yet), e.g.
//! for `cargo run -p rbdiff -- device <dir>`.

use mb_pdb::schema::DbKind;
use mb_pdb::tables::PlaylistNode;
use mb_pdb::{ExportDb, ExtDb, Pdb};
use mb_usb::log::LogStrings;
use mb_usb::source::LibraryDir;
use mb_usb::{
    CancelToken, DeviceState, ExportOptions, ExtOp, PdbOp, PlanOptions, SelectionItem,
    SourceLibrary, TrackResult, execute, plan_export,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

fn fixture() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    let needed = [
        "options.json",
        "rekordbox/master.db",
        "rekordbox/share",
        "rbtest/PIONEER/rekordbox/export.pdb",
        "rbtest/Contents.sha256",
        "export-log.txt",
    ];
    needed.iter().all(|p| dir.join(p).exists()).then_some(dir)
}

struct Out {
    dir: PathBuf,
    keep: bool,
}

impl Drop for Out {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

fn out_dir() -> Out {
    if let Some(dir) = std::env::var_os("MB_USB_ORACLE_OUT") {
        let dir = PathBuf::from(dir);
        assert!(!dir.exists(), "{} exists", dir.display());
        std::fs::create_dir_all(&dir).unwrap();
        return Out { dir, keep: true };
    }
    let dir = std::env::temp_dir().join(format!("mb-usb-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Out { dir, keep: false }
}

/// Relative paths (NFC, `/`-separated) and full paths of every regular file below `dir`,
/// without AppleDouble `._` files.
fn files(dir: &Path) -> BTreeMap<String, PathBuf> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().nfc().collect::<String>();
            if name.starts_with("._") || name == ".DS_Store" {
                continue;
            }
            let rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let ty = e.file_type().unwrap();
            if ty.is_dir() {
                walk(&e.path(), &rel, out);
            } else if ty.is_file() {
                out.insert(rel, e.path());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

/// SHA-256 of a file (the system `shasum` is much faster than an unoptimised test build).
fn sha256(path: &Path) -> String {
    if let Ok(o) = std::process::Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        && o.status.success()
    {
        return String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
    }
    use sha2::Digest;
    let bytes = std::fs::read(path).unwrap();
    sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn same_files(ours: &Path, theirs: &Path, what: &str) -> usize {
    let (a, b) = (files(ours), files(theirs));
    let ka: BTreeSet<_> = a.keys().collect();
    let kb: BTreeSet<_> = b.keys().collect();
    assert_eq!(
        ka.symmetric_difference(&kb).collect::<Vec<_>>(),
        Vec::<&&String>::new(),
        "{what}: file sets differ"
    );
    let differing: Vec<&String> = a
        .iter()
        .filter(|(k, p)| std::fs::read(p).unwrap() != std::fs::read(&b[*k]).unwrap())
        .map(|(k, _)| k)
        .collect();
    assert!(differing.is_empty(), "{what}: files differ: {differing:?}");
    a.len()
}

#[test]
fn oracle_a_export_matches_rekordbox() -> anyhow::Result<()> {
    let Some(fx) = fixture() else {
        eprintln!("skipped: fixtures/oracle-a is not present");
        return Ok(());
    };
    let reference = fx.join("rbtest");
    let library = LibraryDir::open(&fx)?;
    let conn = library.connection();
    let all = SourceLibrary::load(conn, &library.analysis_root, &[])?;
    let small_id = all.find_playlist("oracle-small")?.id.clone();
    let all_id = all.find_playlist("oracle-all")?.id.clone();
    let probe = SourceLibrary::load(
        conn,
        &library.analysis_root,
        &[SelectionItem::Playlist(all_id.clone())],
    )?;
    let audio_ok = probe.entries[&all_id]
        .iter()
        .filter(|id| probe.tracks[*id].sample_rate != 22_050)
        .all(|id| probe.tracks[id].audio.is_some());
    if !audio_ok {
        eprintln!("skipped: the corpus audio (~/re-work/corpus) is not present");
        return Ok(());
    }

    // The stick's creation date (the only input not derivable from the library).
    let ref_pdb = std::fs::read(reference.join("PIONEER/rekordbox/export.pdb"))?;
    let ref_ext = std::fs::read(reference.join("PIONEER/rekordbox/exportExt.pdb"))?;
    let ref_rows = ExportDb::read(&Pdb::parse(ref_pdb.clone())?)?;
    let date = ref_rows.properties[0].date_created.clone();
    let plan_opts = PlanOptions {
        today: date.clone(),
        ..PlanOptions::default()
    };

    let out = out_dir();
    let root = out.dir.join("device");
    let log_dir = out.dir.join("log");
    let settings_dir = out.dir.join("settings");
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(&settings_dir)?;
    // Stand-in for the local settings folder: only the KUVO profile (the DAT files on the
    // oracle stick are rekordbox's defaults, see settings.rs).
    std::fs::copy(
        reference.join("PIONEER/djprofile.nxs"),
        settings_dir.join("djprofile.nxs"),
    )?;
    let exec_opts = ExportOptions {
        settings_dir: Some(settings_dir),
        log_dir: Some(log_dir.clone()),
        log_strings: LogStrings::german(),
        ..ExportOptions::default()
    };

    // Session 1: oracle-small onto an empty stick.
    let sel1 = [SelectionItem::Playlist(small_id.clone())];
    let src1 = SourceLibrary::load(conn, &library.analysis_root, &sel1)?;
    let dev1 = DeviceState::load(&root, None)?;
    let plan1 = plan_export(&src1, &dev1, &sel1, &plan_opts)?;
    let r1 = execute(&plan1, &dev1, &exec_opts, &CancelToken::new(), &mut |_| {})?;
    assert_eq!(r1.completed, 5);
    assert!(r1.results.iter().all(|r| *r == TrackResult::Exported));
    assert!(r1.log_file.is_none());

    // Session 2: oracle-all, as rekordbox ran it on the oracle stick.
    let sel2 = [SelectionItem::Playlist(all_id.clone())];
    let src2 = SourceLibrary::load(conn, &library.analysis_root, &sel2)?;
    let dev2 = DeviceState::load(&root, None)?;
    let plan2 = plan_export(&src2, &dev2, &sel2, &plan_opts)?;
    let counts = plan2.counts();
    assert_eq!(counts.get(&0), Some(&152), "{counts:?}");
    assert_eq!(counts.get(&1), Some(&5), "{counts:?}");
    assert_eq!(counts.get(&5), Some(&4), "{counts:?}");

    // The default operation sequence gives rekordbox's rows (and a lint-clean file).
    let default_pdb = mb_usb::exec::build_pdb(
        None,
        Some(&dev2.pdb.as_ref().unwrap().export_bytes),
        &plan2.pdb_ops(None),
        &plan2.pdb_lookups,
    )?;
    assert!(mb_pdb::lint::lint(&default_pdb, DbKind::Export).is_clean());
    assert_eq!(
        sorted_rows(&ExportDb::read(&Pdb::parse(default_pdb)?)?),
        sorted_rows(&ref_rows)
    );

    let small_node = dev2.pdb.as_ref().unwrap().export.playlists[0].clone();
    let all_node = match &plan2.jobs[0].list_pdb[..] {
        [PdbOp::CreatePlaylistAtTop(n)] => n.clone(),
        other => panic!("unexpected list ops {other:?}"),
    };
    let at = |n: &PlaylistNode, seq: u32| {
        let mut n = n.clone();
        n.0.seq = seq;
        n
    };
    let mut pdb_ops = vec![PdbOp::Reopen; 10];
    pdb_ops.push(PdbOp::ClearPlaylist(small_node.0.id));
    pdb_ops.push(PdbOp::TouchPlaylist(small_node.0.id));
    pdb_ops.push(PdbOp::InsertPlaylist(at(&all_node, 1)));
    pdb_ops.extend(
        plan1
            .pdb_ops(None)
            .into_iter()
            .filter(|o| matches!(o, PdbOp::AddEntry(_))),
    );
    pdb_ops.push(PdbOp::TouchPlaylist(small_node.0.id));
    pdb_ops.push(PdbOp::UpdatePlaylists(vec![
        at(&small_node, 1),
        at(&all_node, 0),
    ]));
    pdb_ops.extend(plan2.steps().flat_map(|s| s.pdb.clone()));
    pdb_ops.push(PdbOp::Reopen);
    let mut ext_ops = vec![ExtOp::Reopen; 10];
    ext_ops.extend(plan2.ext_ops(None));
    ext_ops.push(ExtOp::Reopen);
    let opts2 = ExportOptions {
        pdb_ops: Some(pdb_ops),
        ext_ops: Some(ext_ops),
        ..exec_opts.clone()
    };
    let r2 = execute(&plan2, &dev2, &opts2, &CancelToken::new(), &mut |_| {})?;
    assert_eq!(r2.completed, 161);
    assert_eq!(r2.exported(), 157);

    let ours = root.join("PIONEER");
    let theirs = reference.join("PIONEER");

    // (a) + (b): /Contents paths and audio bytes against rekordbox's list.
    let listed: BTreeMap<String, String> =
        std::fs::read_to_string(reference.join("Contents.sha256"))?
            .lines()
            .filter_map(|l| l.split_once("  "))
            .map(|(h, p)| (p.nfc().collect::<String>(), h.to_owned()))
            .collect();
    let contents: BTreeMap<String, PathBuf> = files(&root.join("Contents"))
        .into_iter()
        .map(|(k, v)| (format!("Contents/{k}"), v))
        .collect();
    assert_eq!(
        contents.keys().collect::<Vec<_>>(),
        listed.keys().collect::<Vec<_>>(),
        "(a) /Contents paths"
    );
    for (path, file) in &contents {
        assert_eq!(&sha256(file), &listed[path], "(b) audio bytes of {path}");
    }

    // (c) ANLZ, (d) artwork: identical file sets and bytes.
    assert_eq!(
        same_files(&ours.join("USBANLZ"), &theirs.join("USBANLZ"), "(c) ANLZ"),
        471
    );
    assert_eq!(
        same_files(
            &ours.join("Artwork"),
            &theirs.join("Artwork"),
            "(d) artwork"
        ),
        380
    );

    // (e) export.pdb / exportExt.pdb byte for byte.
    let our_pdb = std::fs::read(ours.join("rekordbox/export.pdb"))?;
    let our_ext = std::fs::read(ours.join("rekordbox/exportExt.pdb"))?;
    assert!(mb_pdb::lint::lint(&our_pdb, DbKind::Export).is_clean());
    assert!(mb_pdb::lint::lint(&our_ext, DbKind::ExportExt).is_clean());
    assert_eq!(
        ExtDb::read(&Pdb::parse(our_ext.clone())?)?,
        ExtDb::read(&Pdb::parse(ref_ext.clone())?)?
    );
    let cmp = mb_pdb::compare::compare(&our_pdb, &ref_pdb);
    assert!(cmp.is_identical(), "(e) export.pdb: {cmp:?}");
    assert!(our_ext == ref_ext, "(e) exportExt.pdb differs");

    // (f) exportLibrary.db tables.
    let tmp = out.dir.join("compare");
    std::fs::create_dir_all(&tmp)?;
    std::fs::copy(ours.join("rekordbox/exportLibrary.db"), tmp.join("ours.db"))?;
    std::fs::copy(
        theirs.join("rekordbox/exportLibrary.db"),
        tmp.join("theirs.db"),
    )?;
    let a = mb_devlib::DevLibDb::open_read_only(&tmp.join("ours.db"))?;
    let b = mb_devlib::DevLibDb::open_read_only(&tmp.join("theirs.db"))?;
    let diff = mb_devlib::diff::compare(a.connection(), b.connection())?;
    assert!(diff.content_identical(), "(f) exportLibrary.db:\n{diff}");
    for side in ["-wal", "-shm"] {
        assert!(
            !ours
                .join(format!("rekordbox/exportLibrary.db{side}"))
                .exists()
        );
    }

    // (g) settings files.
    for name in [
        "MYSETTING.DAT",
        "MYSETTING2.DAT",
        "DEVSETTING.DAT",
        "DJMMYSETTING.DAT",
        "djprofile.nxs",
    ] {
        assert_eq!(
            std::fs::read(ours.join(name))?,
            std::fs::read(theirs.join(name))?,
            "(g) {name}"
        );
    }
    // DeviceLibBackup: same name, same shape (the UUID is random).
    let json = std::fs::read_to_string(ours.join("DeviceLibBackup/rbDevLibBaInfo_417703505.json"))?;
    let theirs_json =
        std::fs::read_to_string(theirs.join("DeviceLibBackup/rbDevLibBaInfo_417703505.json"))?;
    assert_eq!(json.len(), theirs_json.len());
    assert_eq!(json[..14], theirs_json[..14]);
    assert_eq!(json[46..], theirs_json[46..]);
    assert!(json[14..46].bytes().all(|b| b.is_ascii_hexdigit()));

    // No markers or temp files left; the export log equals rekordbox's.
    let rb = files(&ours.join("rekordbox"));
    assert_eq!(
        rb.keys().collect::<Vec<_>>(),
        ["export.pdb", "exportExt.pdb", "exportLibrary.db"]
    );
    let log = r2.log_file.expect("export log");
    assert_eq!(
        std::fs::read(&log)?,
        std::fs::read(fx.join("export-log.txt"))?,
        "export log"
    );
    Ok(())
}

/// Every table's rows, each table sorted (row order differs between operation sequences).
fn sorted_rows(db: &ExportDb) -> Vec<Vec<String>> {
    fn rows<T: std::fmt::Debug>(v: &[T]) -> Vec<String> {
        let mut out: Vec<String> = v.iter().map(|r| format!("{r:?}")).collect();
        out.sort();
        out
    }
    vec![
        rows(&db.tracks),
        rows(&db.genres),
        rows(&db.artists),
        rows(&db.albums),
        rows(&db.labels),
        rows(&db.keys),
        rows(&db.colors),
        rows(&db.playlists),
        rows(&db.playlist_entries),
        rows(&db.artwork),
        rows(&db.menu_items),
        rows(&db.categories),
        rows(&db.sorts),
        rows(&db.properties),
    ]
}
