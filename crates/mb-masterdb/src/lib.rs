//! rekordbox `master.db` storage. Spec: `docs/re/masterdb.md`.

mod key;
mod paths;
mod snapshot;

pub use key::derive_passphrase;
pub use paths::LibraryPaths;

use rusqlite::{Connection, OpenFlags};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("options.json: {0}")]
    Options(String),
    #[error("dp field: {0}")]
    Passphrase(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A read-only connection to master.db. Reads go through the WAL like rekordbox's own readers.
pub struct MasterDb {
    conn: Connection,
}

impl MasterDb {
    pub fn open_read_only(path: &Path, passphrase: &str) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "key", passphrase)?;
        // The key is only checked on first page access; fail here rather than on first query.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Row count of every table, in schema order.
    pub fn table_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        names
            .into_iter()
            .map(|name| {
                let count = self.conn.query_row(
                    &format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\"")),
                    [],
                    |row| row.get(0),
                )?;
                Ok((name, count))
            })
            .collect()
    }
}
