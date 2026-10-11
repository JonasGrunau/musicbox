//! Lookup-or-create of artist/genre/album/label/key rows (`docs/re/ops/import-track.md`
//! § Per-file transaction, `common.md` § Lookups are exact). A found row is reused and costs no
//! USN; a missing one costs one bump and one builder-dialect insert.

use crate::Result;
use mb_masterdb::WriteTxn;
use mb_masterdb::rusqlite::OptionalExtension;
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::write::{new_rid32, new_uuid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameTable {
    Artist,
    Genre,
    Label,
    Key,
}

impl NameTable {
    pub(crate) fn table(self) -> &'static str {
        match self {
            Self::Artist => "djmdArtist",
            Self::Genre => "djmdGenre",
            Self::Label => "djmdLabel",
            Self::Key => "djmdKey",
        }
    }

    fn name_column(self) -> &'static str {
        if self == Self::Key { "ScaleName" } else { "Name" }
    }
}

/// A new random u32 ID not used by any row of `table` (live or soft-deleted, since `ID` is the
/// primary key). rekordbox's builder inserts do not check (import-track.md); the redraw only
/// avoids a primary-key failure and does not change what is written.
pub(crate) fn fresh_id(tx: &WriteTxn<'_>, table: &str) -> Result<String> {
    loop {
        let id = new_rid32()?.to_string();
        if !any_row_with_id(tx, table, &id)? {
            return Ok(id);
        }
    }
}

pub(crate) fn any_row_with_id(tx: &WriteTxn<'_>, table: &str, id: &str) -> Result<bool> {
    Ok(tx
        .connection()
        .query_row(&format!("SELECT 1 FROM {table} WHERE ID = ?1"), [id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// `select ID from <T> where rb_local_deleted = 0 and Name = "<name>"` (exact, case-sensitive).
pub(crate) fn find(tx: &WriteTxn<'_>, t: NameTable, name: &str) -> Result<Option<String>> {
    Ok(tx
        .connection()
        .query_row(
            &format!("SELECT ID FROM {} WHERE rb_local_deleted = 0 AND {} = ?1 ORDER BY rowid LIMIT 1", t.table(), t.name_column()),
            [name],
            |r| r.get(0),
        )
        .optional()?)
}

/// Lookup, and on a miss: `[USN bump] insert into <T> (ID, UUID, Name|ScaleName, rb_*…)`.
pub(crate) fn find_or_create(tx: &WriteTxn<'_>, t: NameTable, name: &str) -> Result<String> {
    if let Some(id) = find(tx, t, name)? {
        return Ok(id);
    }
    let id = fresh_id(tx, t.table())?;
    tx.insert(
        t.table(),
        &[
            ("ID", Value::Text(id.clone())),
            ("UUID", Value::Text(new_uuid())),
            (t.name_column(), Value::Text(name.to_owned())),
        ],
    )?;
    Ok(id)
}

/// Album lookup. The key is `(Name, AlbumArtistID, Compilation = 0)`; without an album artist the
/// lookup has no `AlbumArtistID` condition at all (import-track.md, both forms traced).
pub(crate) fn find_or_create_album(tx: &WriteTxn<'_>, name: &str, album_artist_id: Option<&str>) -> Result<String> {
    let conn = tx.connection();
    let found: Option<String> = match album_artist_id {
        Some(aa) => conn
            .query_row(
                "SELECT ID FROM djmdAlbum WHERE rb_local_deleted = 0 AND Name = ?1 AND AlbumArtistID = ?2 AND Compilation = 0 ORDER BY rowid LIMIT 1",
                [name, aa],
                |r| r.get(0),
            )
            .optional()?,
        None => conn
            .query_row(
                "SELECT ID FROM djmdAlbum WHERE rb_local_deleted = 0 AND Name = ?1 AND Compilation = 0 ORDER BY rowid LIMIT 1",
                [name],
                |r| r.get(0),
            )
            .optional()?,
    };
    if let Some(id) = found {
        return Ok(id);
    }
    let id = fresh_id(tx, "djmdAlbum")?;
    let mut cols = vec![
        ("ID", Value::Text(id.clone())),
        ("UUID", Value::Text(new_uuid())),
        ("Name", Value::Text(name.to_owned())),
    ];
    if let Some(aa) = album_artist_id {
        cols.push(("AlbumArtistID", Value::Text(aa.to_owned())));
    }
    cols.push(("Compilation", Value::Integer(0)));
    tx.insert("djmdAlbum", &cols)?;
    Ok(id)
}

/// The name of a lookup row (for inverse edits).
pub(crate) fn name_of(tx: &WriteTxn<'_>, table: &str, id: &str) -> Result<Option<String>> {
    let col = if table == "djmdKey" { "ScaleName" } else { "Name" };
    Ok(tx
        .connection()
        .query_row(&format!("SELECT {col} FROM {table} WHERE ID = ?1"), [id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}
