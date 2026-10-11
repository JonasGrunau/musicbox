//! rekordbox Device Library Plus / OneLibrary database (`PIONEER/rekordbox/exportLibrary.db`).
//! Spec: `docs/re/devlib.md`.
//!
//! The file is SQLCipher 4 with default parameters; the passphrase is a constant decoded from the
//! rekordbox binary ([`passphrase`]). [`DevLibDb`] reads it; [`DevLibWriter`] creates and writes
//! it the way rekordbox 7.2.19 does (spec: `docs/re/devlib.md#writer`). [`diff::compare`]
//! compares two databases table by table.

pub mod diff;
mod key;
pub mod model;
mod read;
pub mod schema;
mod snapshot;
mod write;

pub use key::{
    BLOWFISH_KEY, ENCODED_PASSPHRASE, decode_with_key, derive_passphrase, juce_base64_decode,
    passphrase,
};
pub use model::*;
pub use schema::MenuSeed;
pub use snapshot::on_stick;
pub use write::{
    ContentColumn, CreateOptions, DevLibWriter, ImageOwner, Lookup, LookupColumn, MyTagRow, NewCue,
    NewTrack, device_image_path, export_analysed_bits, export_content_link, reexport_content_link,
    today,
};

use rusqlite::{Connection, OpenFlags};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("passphrase: {0}")]
    Passphrase(String),
    #[error("path: {0}")]
    Path(String),
    #[error("{0} already exists")]
    Exists(String),
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A read-only connection to exportLibrary.db.
pub struct DevLibDb {
    conn: Connection,
}

impl DevLibDb {
    /// Opens read-only with the built-in passphrase ([`passphrase`]).
    ///
    /// The database is in WAL mode, so SQLite creates `-shm`/`-wal` next to it even for a
    /// read-only connection. Use [`DevLibDb::open_immutable`] on media that must stay untouched.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        Self::open_read_only_with(path, &passphrase())
    }

    pub fn open_read_only_with(path: &Path, passphrase: &str) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::keyed(conn, passphrase)
    }

    /// Opens with SQLite's `immutable=1` URI flag: no locking and no side files at all. Only valid
    /// when nothing else writes the file and no `-wal` holds committed data (a stick rekordbox
    /// closed cleanly has neither `-wal` nor `-shm`).
    pub fn open_immutable(path: &Path) -> Result<Self> {
        let path = path
            .to_str()
            .ok_or_else(|| Error::Path(format!("not valid UTF-8: {}", path.display())))?;
        let conn = Connection::open_with_flags(
            format!("file:{}?immutable=1", uri_escape(path)),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )?;
        Self::keyed(conn, &passphrase())
    }

    fn keyed(conn: Connection, passphrase: &str) -> Result<Self> {
        conn.pragma_update(None, "key", passphrase)?;
        // The key is only checked on first page access; fail here rather than on first query.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// Percent-escapes the characters that would end or alter the path part of an SQLite URI.
fn uri_escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '%' | '?' | '#' | ' ' => out.push_str(&format!("%{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn uri_escape() {
        assert_eq!(super::uri_escape("/a b/c?d#e%f"), "/a%20b/c%3Fd%23e%25f");
    }
}
