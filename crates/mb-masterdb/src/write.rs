//! master.db writes, the single place that applies rekordbox's change bookkeeping.
//! Spec: `docs/re/ops/common.md` (verified against rekordbox's own SQL trace).
//!
//! Every write statement goes through [`WriteTxn`], which
//! 1. bumps `agentRegistry('localUpdateCount').int_1` first and stamps the row with the new value
//!    as `rb_local_usn` — one bump per statement, spent even when the statement changes nothing;
//! 2. sets the row-change columns exactly like rekordbox's builder dialect;
//! 3. deletes the way rekordbox does: hard delete for never-synced rows, soft delete otherwise.

use crate::{Error, Result, guard};
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params_from_iter};
use std::path::Path;

/// A read-write connection. Opening refuses while rekordbox or rekordboxAgent runs.
pub struct MasterDbWriter {
    conn: Connection,
}

impl MasterDbWriter {
    pub fn open(path: &Path, passphrase: &str) -> Result<Self> {
        let running = guard::running_rekordbox();
        if !running.is_empty() {
            return Err(Error::RekordboxRunning(running));
        }
        Self::open_unguarded(path, passphrase)
    }

    /// Opens without the process check. Only for fixture copies in tests and tools.
    pub fn open_unguarded(path: &Path, passphrase: &str) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.pragma_update(None, "key", passphrase)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// `begin exclusive`, as rekordbox wraps imports, removals and playlist batches.
    pub fn transaction(&mut self) -> Result<WriteTxn<'_>> {
        Ok(WriteTxn { tx: self.conn.transaction_with_behavior(TransactionBehavior::Exclusive)? })
    }
}

/// `<TS>`: UTC with milliseconds and an explicit offset, e.g. `2026-10-11 19:59:26.763 +00:00`.
pub fn timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f +00:00").to_string()
}

/// `<DATE>` for StockDate/DateCreated. Local date (rekordbox's choice is open, see common.md).
pub fn date_today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// `<UUIDv4>`: lower-case dashed random UUID.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn random_u32() -> Result<u32> {
    getrandom::u32().map_err(|e| Error::Random(e.to_string()))
}

/// `<RID32>`: random u32 (lookup tables, playlists), as decimal text by the caller.
pub fn new_rid32() -> Result<u32> {
    loop {
        let r = random_u32()?;
        if r != 0 && r != u32::MAX {
            return Ok(r);
        }
    }
}

/// `djmdContent.ID`: random in [1, 2^28 − 1] (`AppSyncDBController::insertTrack` @0x100a56e3c).
pub fn new_content_id() -> Result<u32> {
    loop {
        let r = new_rid32()?;
        if r <= 0x0FFF_FFFF {
            return Ok(r);
        }
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The `djmdContent` change counters (`*Updated`, stored as decimal text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterColumn {
    CueUpdated,
    AnalysisUpdated,
    TrackInfoUpdated,
}

impl CounterColumn {
    pub fn name(self) -> &'static str {
        match self {
            Self::CueUpdated => "CueUpdated",
            Self::AnalysisUpdated => "AnalysisUpdated",
            Self::TrackInfoUpdated => "TrackInfoUpdated",
        }
    }
}

/// The lookup tables ORPHAN-CLEANUP visits (common.md), in rekordbox's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanTable {
    Artist,
    Album,
    Genre,
    Label,
    Key,
}

impl OrphanTable {
    pub const ALL: [OrphanTable; 5] = [Self::Artist, Self::Album, Self::Genre, Self::Label, Self::Key];

    pub fn name(self) -> &'static str {
        match self {
            Self::Artist => "djmdArtist",
            Self::Album => "djmdAlbum",
            Self::Genre => "djmdGenre",
            Self::Label => "djmdLabel",
            Self::Key => "djmdKey",
        }
    }

    fn orphan_query(self) -> &'static str {
        match self {
            Self::Artist => "SELECT ID FROM djmdArtist WHERE rb_local_deleted = 0 \
                AND ID NOT IN (SELECT ArtistID FROM djmdContent WHERE rb_local_deleted = 0 AND ArtistID NOTNULL) \
                AND ID NOT IN (SELECT RemixerID FROM djmdContent WHERE rb_local_deleted = 0 AND RemixerID NOTNULL) \
                AND ID NOT IN (SELECT OrgArtistID FROM djmdContent WHERE rb_local_deleted = 0 AND OrgArtistID NOTNULL) \
                AND ID NOT IN (SELECT ComposerID FROM djmdContent WHERE rb_local_deleted = 0 AND ComposerID NOTNULL) \
                AND ID NOT IN (SELECT AlbumArtistID FROM djmdAlbum WHERE rb_local_deleted = 0 AND AlbumArtistID NOTNULL)",
            Self::Album => "SELECT ID FROM djmdAlbum WHERE rb_local_deleted = 0 \
                AND ID NOT IN (SELECT AlbumID FROM djmdContent WHERE rb_local_deleted = 0 AND AlbumID NOTNULL)",
            Self::Genre => "SELECT ID FROM djmdGenre WHERE rb_local_deleted = 0 \
                AND ID NOT IN (SELECT GenreID FROM djmdContent WHERE rb_local_deleted = 0 AND GenreID NOTNULL)",
            Self::Label => "SELECT ID FROM djmdLabel WHERE rb_local_deleted = 0 \
                AND ID NOT IN (SELECT LabelID FROM djmdContent WHERE rb_local_deleted = 0 AND LabelID NOTNULL)",
            Self::Key => "SELECT ID FROM djmdKey WHERE rb_local_deleted = 0 \
                AND ID NOT IN (SELECT KeyID FROM djmdContent WHERE rb_local_deleted = 0 AND KeyID NOTNULL)",
        }
    }
}

/// One write transaction. Dropping without [`WriteTxn::commit`] rolls back.
pub struct WriteTxn<'c> {
    tx: rusqlite::Transaction<'c>,
}

impl WriteTxn<'_> {
    pub fn connection(&self) -> &Connection {
        &self.tx
    }

    /// USN bump: `<U>` = current `int_1` + 1, stored back, returned for the following statement.
    /// `agentRegistry.updated_at` is left unchanged, as rekordbox does.
    pub fn bump_usn(&self) -> Result<i64> {
        let current: Option<i64> = self.tx.query_row(
            "SELECT int_1 FROM agentRegistry WHERE registry_id = 'localUpdateCount'",
            [],
            |r| r.get(0),
        )?;
        let next = current.unwrap_or(0) + 1;
        self.tx.execute("UPDATE agentRegistry SET int_1 = ?1 WHERE registry_id = 'localUpdateCount'", [next])?;
        Ok(next)
    }

    /// True if `table` has a live row with this ID (collision check before inserting a new ID).
    pub fn id_exists(&self, table: &str, id: &str) -> Result<bool> {
        let sql = format!("SELECT 1 FROM {} WHERE ID = ?1 AND rb_local_deleted = 0", quote_ident(table));
        Ok(self.tx.query_row(&sql, [id], |_| Ok(())).map(|_| true).or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            e => Err(e),
        })?)
    }

    /// Insert with the row-change columns of an insert: `rb_data_status = 0, rb_local_deleted = 0,
    /// rb_local_synced = 0, rb_local_usn = <U>, created_at = updated_at = <TS>`. Returns `<U>`.
    pub fn insert(&self, table: &str, columns: &[(&str, Value)]) -> Result<i64> {
        let usn = self.bump_usn()?;
        self.insert_with_usn(table, columns, usn)?;
        Ok(usn)
    }

    /// Builder-dialect update of live rows matching `key_col = key` (common.md § Row-change
    /// columns): sets `columns`, `rb_data_status` (257 for synced rows, else 0), `rb_local_usn`,
    /// `updated_at`. rekordbox updates one column per statement; callers that replay an op pass
    /// one column at a time. Returns `<U>`.
    pub fn update(&self, table: &str, key_col: &str, key: Value, columns: &[(&str, Value)]) -> Result<i64> {
        let usn = self.bump_usn()?;
        let mut sets: Vec<String> = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        for (c, v) in columns {
            values.push(v.clone());
            sets.push(format!("{} = ?{}", quote_ident(c), values.len()));
        }
        values.push(Value::Integer(usn));
        let usn_idx = values.len();
        values.push(Value::Text(timestamp()));
        let ts_idx = values.len();
        values.push(key);
        let key_idx = values.len();
        let sql = format!(
            "UPDATE {} SET {}, rb_data_status = CASE WHEN usn != 0 THEN 257 ELSE 0 END, \
             rb_local_usn = ?{usn_idx}, updated_at = ?{ts_idx} WHERE rb_local_deleted = 0 AND {} = ?{key_idx}",
            quote_ident(table),
            sets.join(", "),
            quote_ident(key_col)
        );
        self.tx.execute(&sql, params_from_iter(values))?;
        Ok(usn)
    }

    /// Builder-dialect update with caller-supplied SQL fragments, for statements that are not a
    /// plain `col = value` (e.g. `TrackNo = TrackNo - 1`) or that match rows by a range:
    /// `UPDATE <table> SET <assignments>, rb_data_status = …, rb_local_usn = <U>, updated_at = <TS>
    /// WHERE rb_local_deleted = 0 AND (<filter>)`. Fragments use anonymous `?` placeholders, bound
    /// in order: `assignment_params`, then `filter_params`. One bump, spent even when no row
    /// matches (common.md rule 3). Returns `(<U>, rows changed)`.
    pub fn update_where(
        &self,
        table: &str,
        assignments: &str,
        assignment_params: &[Value],
        filter: &str,
        filter_params: &[Value],
    ) -> Result<(i64, usize)> {
        let usn = self.bump_usn()?;
        let mut values: Vec<Value> = assignment_params.to_vec();
        values.push(Value::Integer(usn));
        values.push(Value::Text(timestamp()));
        values.extend(filter_params.iter().cloned());
        let sql = format!(
            "UPDATE {} SET {assignments}, rb_data_status = CASE WHEN usn != 0 THEN 257 ELSE 0 END, \
             rb_local_usn = ?, updated_at = ? WHERE rb_local_deleted = 0 AND ({filter})",
            quote_ident(table)
        );
        let changed = self.tx.execute(&sql, params_from_iter(values))?;
        Ok((usn, changed))
    }

    /// ORM-dialect update of one row by `ID` (common.md § Row-change columns, "O update"), as
    /// rekordbox writes `djmdPlaylist` moves: sets `columns`, `rb_local_usn = <U>`,
    /// `updated_at = <TS>`, `WHERE ID = <id>` (no `rb_local_deleted` filter).
    /// rekordbox also lists `rb_data_status, rb_local_data_status, rb_local_deleted,
    /// rb_local_synced` in the statement; they were 0 in every traced row.
    // hypothesis: those four are written back from the row just read (`SELECT *` precedes every
    // ORM update), so they are left unchanged here. Forced-to-0 is the alternative (common.md, open).
    pub fn update_orm(&self, table: &str, id: &str, columns: &[(&str, Value)]) -> Result<i64> {
        let usn = self.bump_usn()?;
        let mut sets: Vec<String> = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        for (c, v) in columns {
            values.push(v.clone());
            sets.push(format!("{} = ?{}", quote_ident(c), values.len()));
        }
        values.push(Value::Integer(usn));
        sets.push(format!("rb_local_usn = ?{}", values.len()));
        values.push(Value::Text(timestamp()));
        sets.push(format!("updated_at = ?{}", values.len()));
        values.push(Value::Text(id.to_owned()));
        let sql = format!("UPDATE {} SET {} WHERE ID = ?{}", quote_ident(table), sets.join(", "), values.len());
        self.tx.execute(&sql, params_from_iter(values))?;
        Ok(usn)
    }

    /// Several rows inserted under **one** bump, as rekordbox's add-to-playlist batch does
    /// (`ops/playlist-add-tracks.md`): every row carries the same `<U>`, each its own `<TS>`.
    /// Returns `<U>`.
    pub fn insert_batch(&self, table: &str, rows: &[Vec<(&str, Value)>]) -> Result<i64> {
        let usn = self.bump_usn()?;
        for columns in rows {
            self.insert_with_usn(table, columns, usn)?;
        }
        Ok(usn)
    }

    fn insert_with_usn(&self, table: &str, columns: &[(&str, Value)], usn: i64) -> Result<()> {
        let ts = timestamp();
        let mut names: Vec<String> = columns.iter().map(|(c, _)| quote_ident(c)).collect();
        let mut values: Vec<Value> = columns.iter().map(|(_, v)| v.clone()).collect();
        for (c, v) in [
            ("rb_data_status", Value::Integer(0)),
            ("rb_local_deleted", Value::Integer(0)),
            ("rb_local_synced", Value::Integer(0)),
            ("rb_local_usn", Value::Integer(usn)),
            ("created_at", Value::Text(ts.clone())),
            ("updated_at", Value::Text(ts)),
        ] {
            names.push(quote_ident(c));
            values.push(v);
        }
        let placeholders = (1..=values.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let sql = format!("INSERT INTO {} ({}) VALUES ({placeholders})", quote_ident(table), names.join(", "));
        self.tx.execute(&sql, params_from_iter(values))?;
        Ok(())
    }

    /// Plain hard delete without a bump and without the soft-delete branch, as rekordbox removes
    /// `djmdCue` rows by `ContentUUID` (`ops/remove-tracks.md`). Returns rows deleted.
    pub fn delete_unversioned(&self, table: &str, key_col: &str, keys: &[Value]) -> Result<usize> {
        if keys.is_empty() {
            return Ok(0);
        }
        let list = (1..=keys.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let sql = format!("DELETE FROM {} WHERE {} IN ({list})", quote_ident(table), quote_ident(key_col));
        Ok(self.tx.execute(&sql, params_from_iter(keys.iter().cloned()))?)
    }

    /// COUNTER macro (common.md § UPDATE-COL): reads `CueUpdated, AnalysisUpdated,
    /// TrackInfoUpdated` of the content row, then UPDATE-COL `<column>` = decimal text of old + 1
    /// (NULL counts as 0). Returns `(<U>, new value)`.
    pub fn increment_counter(&self, content_id: &str, column: CounterColumn) -> Result<(i64, i64)> {
        let (cue, analysis, track_info): (Option<String>, Option<String>, Option<String>) = self.tx.query_row(
            "SELECT CueUpdated, AnalysisUpdated, TrackInfoUpdated FROM djmdContent WHERE rb_local_deleted = 0 AND ID = ?1",
            [content_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let old = match column {
            CounterColumn::CueUpdated => cue,
            CounterColumn::AnalysisUpdated => analysis,
            CounterColumn::TrackInfoUpdated => track_info,
        };
        let next = old.and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0) + 1;
        let usn = self.update(
            "djmdContent",
            "ID",
            Value::Text(content_id.to_owned()),
            &[(column.name(), Value::Text(next.to_string()))],
        )?;
        Ok((usn, next))
    }

    /// DELETE-ROWS macro: one bump; never-synced rows are hard-deleted, rows with sync history are
    /// soft-deleted (`rb_local_deleted = 1`, `rb_data_status` 258 if it was non-zero else 2).
    pub fn delete_rows(&self, table: &str, key_col: &str, keys: &[Value]) -> Result<i64> {
        let usn = self.bump_usn()?;
        if keys.is_empty() {
            return Ok(usn);
        }
        let list = (1..=keys.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let (t, k) = (quote_ident(table), quote_ident(key_col));
        self.tx.execute(
            &format!(
                "DELETE FROM {t} WHERE rb_local_deleted = 0 AND rb_data_status = 0 AND rb_local_synced = 0 AND {k} IN ({list})"
            ),
            params_from_iter(keys.iter().cloned()),
        )?;
        let mut values = keys.to_vec();
        values.push(Value::Integer(usn));
        let usn_idx = values.len();
        values.push(Value::Text(timestamp()));
        let ts_idx = values.len();
        self.tx.execute(
            &format!(
                "UPDATE {t} SET rb_local_deleted = 1, \
                 rb_data_status = CASE WHEN rb_data_status != 0 THEN 258 ELSE 2 END, \
                 rb_local_usn = ?{usn_idx}, updated_at = ?{ts_idx} \
                 WHERE rb_local_deleted = 0 AND (rb_data_status != 0 OR rb_local_synced != 0) AND {k} IN ({list})"
            ),
            params_from_iter(values),
        )?;
        Ok(usn)
    }

    /// ORPHAN-CLEANUP macro: removes artist/album/genre/label/key rows no live content (or album,
    /// for album artists) refers to, in that table order. An empty result costs no USN.
    /// Returns the removed IDs per table (tables without orphans are left out).
    pub fn cleanup_orphans(&self) -> Result<Vec<(&'static str, Vec<String>)>> {
        let mut removed = Vec::new();
        for table in OrphanTable::ALL {
            let ids = self.cleanup_orphan_table(table)?;
            if !ids.is_empty() {
                removed.push((table.name(), ids));
            }
        }
        Ok(removed)
    }

    /// One step of ORPHAN-CLEANUP: the orphan query for `table`, then one DELETE-ROWS over the
    /// result (quoted IDs). Analysis runs this step alone for `djmdKey` after a `KeyID` change.
    pub fn cleanup_orphan_table(&self, table: OrphanTable) -> Result<Vec<String>> {
        let mut stmt = self.tx.prepare(table.orphan_query())?;
        let ids: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        if !ids.is_empty() {
            let keys: Vec<Value> = ids.iter().cloned().map(Value::Text).collect();
            self.delete_rows(table.name(), "ID", &keys)?;
        }
        Ok(ids)
    }

    pub fn commit(self) -> Result<()> {
        self.tx.commit()?;
        Ok(())
    }
}
