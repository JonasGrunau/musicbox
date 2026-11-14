//! The editor thread when editing ends with edits still queued: turning editing off
//! (`Editor::finish`) saves them; rekordbox starting (`Editor::abort`) skips them and says how
//! many. Works on a temp copy of fixtures/oracle-a; skipped when fixtures are absent.

use mb_library::{Command, NodeKind};
use musicbox::editor::{Editor, Reply, Request};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const EDITS: usize = 8;

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

/// A private copy of the oracle-a library: (options.json, master.db, temp root).
fn temp_library(name: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        return None;
    }
    let root = std::env::temp_dir().join(format!("mb-editor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&fixture.join("rekordbox"), &root.join("rekordbox"));
    std::fs::copy(fixture.join("options.json"), root.join("options.json")).unwrap();
    Some((root.join("options.json"), root.join("rekordbox/master.db"), root))
}

fn passphrase(options: &Path) -> String {
    let paths = mb_masterdb::LibraryPaths::from_options_json(options).unwrap();
    mb_masterdb::derive_passphrase(&paths.dp).unwrap()
}

/// Names of the playlists in master.db that this test created.
fn created(db: &Path, options: &Path) -> Vec<String> {
    let paths = mb_masterdb::LibraryPaths::from_options_json(options).unwrap();
    let snap = mb_masterdb::MasterDb::open_read_only(db, &passphrase(options)).unwrap().load_snapshot(&paths).unwrap();
    snap.playlists.into_iter().map(|p| p.name).filter(|n| n.starts_with("queued ")).collect()
}

/// Starts an editor, queues `EDITS` new playlists at once, ends editing with `end` and collects
/// every reply until the thread is gone.
fn queue_then(name: &str, end: fn(&mut Editor)) -> Option<(Vec<Reply>, Vec<String>)> {
    let (options, db, root) = temp_library(name)?;
    let mut editor = Editor::start(&egui::Context::default(), &db, &passphrase(&options), Some(root.join("backups")), None);
    let start = Instant::now();
    while !editor.ready {
        assert!(start.elapsed() < Duration::from_secs(30), "editor did not start");
        for r in editor.poll() {
            if let Reply::Failed(e) = r {
                panic!("editor failed: {e}");
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for i in 0..EDITS {
        editor.send(Request::Run(Command::CreatePlaylist { parent: None, name: format!("queued {i}"), kind: NodeKind::Playlist }));
    }
    end(&mut editor);
    let mut replies = Vec::new();
    loop {
        assert!(start.elapsed() < Duration::from_secs(60), "editor did not end");
        let done = editor.finished();
        replies.extend(editor.poll());
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    editor.join();
    let names = created(&db, &options);
    let _ = std::fs::remove_dir_all(&root);
    Some((replies, names))
}

#[test]
fn turning_editing_off_saves_the_queued_edits() {
    let Some((replies, names)) = queue_then("finish", Editor::finish) else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let done = replies.iter().filter(|r| matches!(r, Reply::Done { changed: true, .. })).count();
    assert_eq!(done, EDITS);
    assert!(!replies.iter().any(|r| matches!(r, Reply::Skipped(_))));
    assert_eq!(names.len(), EDITS, "{names:?}");
}

#[test]
fn rekordbox_starting_skips_the_queued_edits_and_counts_them() {
    let Some((replies, names)) = queue_then("abort", Editor::abort) else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let done = replies.iter().filter(|r| matches!(r, Reply::Done { .. })).count();
    let skipped: usize = replies.iter().map(|r| if let Reply::Skipped(n) = r { *n } else { 0 }).sum();
    assert!(skipped > 0, "the edits were queued behind a running one");
    assert_eq!(done + skipped, EDITS);
    assert_eq!(names.len(), done, "{names:?}");
}
