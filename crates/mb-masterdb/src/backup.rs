//! Session backups before the first write (plan § Safety rule 4).
//!
//! master.db is copied with `sqlcipher_export` into an encrypted file with the same passphrase —
//! the sequence rekordbox itself runs at quit (trace: ATTACH … AS backup KEY …; BEGIN EXCLUSIVE;
//! SELECT sqlcipher_export('backup'); COMMIT; DETACH). Side files that belong to the library are
//! copied byte for byte. Backups go to musicbox's own folder and never use rekordbox's
//! `master.backup.*` names.

use crate::{Error, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Library side files copied next to the database backup when present.
const SIDE_FILES: &[&str] = &[
    "masterPlaylists6.xml",
    "automixPlaylist6.xml",
    "datafile.edb",
    "ExtData.edb",
];

/// `~/Library/Application Support/musicbox/backups`.
pub fn default_backup_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/musicbox/backups"))
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io { path: path.display().to_string(), source }
}

/// Backs up `db_path` (and its side files) into a new timestamped folder under `backup_root`.
/// Returns the folder. The source is opened read-only, so this is safe on a live library copy.
pub fn backup_library(db_path: &Path, passphrase: &str, backup_root: &Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f").to_string();
    let dir = backup_root.join(stamp);
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;

    let target = dir.join("master.db");
    // The source is opened `mode=ro` through a URI so that only the attached backup is writable.
    let uri = format!("file:{}?mode=ro", uri_path(db_path));
    let conn = Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.pragma_update(None, "key", passphrase)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS backup KEY ?2",
        (target.to_string_lossy().as_ref(), passphrase),
    )?;
    conn.query_row("SELECT sqlcipher_export('backup')", [], |_| Ok(()))?;
    conn.execute("DETACH DATABASE backup", [])?;

    if let Some(library_dir) = db_path.parent() {
        for name in SIDE_FILES {
            let src = library_dir.join(name);
            if src.is_file() {
                std::fs::copy(&src, dir.join(name)).map_err(io(&src))?;
            }
        }
    }
    Ok(dir)
}

/// Escapes the characters SQLite URIs treat specially in a path.
fn uri_path(path: &Path) -> String {
    let mut out = String::new();
    for c in path.to_string_lossy().chars() {
        match c {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3f"),
            '#' => out.push_str("%23"),
            ' ' => out.push_str("%20"),
            c => out.push(c),
        }
    }
    out
}
