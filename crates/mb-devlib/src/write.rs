//! exportLibrary.db writer: rekordbox 7.2.19's `db::DevSqliteDBController` statement by statement.
//! Spec: `docs/re/devlib.md#writer`; statement sequences: `docs/re/ops/export-devlib.md`.
//!
//! Every method names the rekordbox function it reproduces. IDs, sequence numbers and the
//! NULL / 0 / '' choice per column follow that function, so a database built with this writer
//! matches the one rekordbox would build from the same calls row for row. Values are bound as
//! parameters; rekordbox inlines them as literals, which stores the same values (strings are
//! double-quoted with `""` escaping, numbers are plain, see `SQLiteDB::toSqlString`).
//!
//! rekordbox runs every statement in autocommit mode. [`DevLibWriter::begin`] /
//! [`DevLibWriter::commit`] may wrap a batch for speed; the stored rows are the same.
//!
//! Speed (musicbox's own, no effect on the stored rows): statements are prepared once and
//! reused, and the writer keeps in memory what rekordbox re-reads with a table scan per track
//! (see [`Caches`]): the name → ID maps of the lookup tables and albums, `count(*)` of
//! `content` for `numberOfContents`, and the entry count of each playlist being filled. Every
//! cache gives the value the SQL would; writes through [`DevLibWriter::connection`] that insert
//! or delete rows of those tables must be followed by [`DevLibWriter::invalidate_caches`].

use crate::model::ListAttribute;
use crate::schema::{DB_VERSION_FRESH_STICK, DB_VERSION_INIT, INDEXES, MenuSeed, TABLES};
use crate::{Error, Result, passphrase};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, ToSql, params, params_from_iter};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Values for the seed rows written when the database is created.
#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// `property.deviceName`. A fresh stick exported by rekordbox has "" (`updateDeviceName`
    /// fills it later only for some flows).
    pub device_name: String,
    /// `property.dbVersion`: [`DB_VERSION_FRESH_STICK`] ("1000") is what a fresh 7.2.19 stick
    /// ends with; [`DB_VERSION_INIT`] ("10000") is what `initPropertyTable` itself writes.
    pub db_version: String,
    /// `property.createdDate` (`YYYY-MM-DD`); `None` = today (local time, like
    /// `DBToolBox::getDateString`).
    pub created_date: Option<String>,
    /// `property.backGroundColorType`, 0 on creation.
    pub background_color_type: u32,
    /// `property.myTagMasterDBID`, 0 on creation (set by [`DevLibWriter::write_my_tags`]).
    pub my_tag_master_db_id: u32,
    /// `color` / `menuItem` / `category` / `sort` rows, inserted in the given order.
    pub menus: MenuSeed,
    /// Replay what follows the creation on a fresh stick: rekordbox seeds the new file
    /// (`initPropertyTable`, `dbVersion` "10000"), then the Device Library Plus conversion drops
    /// and recreates every table and index and seeds it again (export-devlib.md phases 1–2).
    /// The rows end up the same either way; the replay also reproduces rekordbox's page layout
    /// (index root pages). Default `true`.
    pub replay_conversion: bool,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            device_name: String::new(),
            db_version: DB_VERSION_FRESH_STICK.to_string(),
            created_date: None,
            background_color_type: 0,
            my_tag_master_db_id: 0,
            menus: MenuSeed::defaults(),
            replay_conversion: true,
        }
    }
}

/// A track as `rekordboxDBController::addTrack` (@0x1017E1EE4) receives it: names instead of
/// lookup IDs. Field widths follow `RowDataTrack`; [`DevLibWriter::add_track`] truncates the
/// same way rekordbox does (`length`, `bitDepth`, `discNo`, `releaseYear`, `djPlayCount` are
/// 16-bit, `fileType`, `rating`, `color_id` are 8-bit, `fileSize` is 32-bit).
///
/// The USB export transforms some source fields before the insert
/// (`export_track_data_to_db`, `docs/re/export.md` §1.2); see [`export_analysed_bits`] and
/// [`export_content_link`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewTrack {
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub remixer: String,
    pub original_artist: String,
    pub composer: String,
    pub lyricist: String,
    pub album: String,
    /// Album artist name; looked up/added in `artist` and stored in `album.artist_id`.
    pub album_artist: String,
    /// `album.isComplation`.
    pub compilation: u8,
    pub genre: String,
    pub label: String,
    /// Key name (master `djmdKey.ScaleName`).
    pub key: String,
    pub bpm_x100: i64,
    /// Seconds.
    pub length: i64,
    pub track_no: i64,
    pub disc_no: i64,
    pub release_year: i64,
    /// `YYYY-MM-DD` or "".
    pub release_date: String,
    pub rating: i64,
    /// 0 = none, 1..=8.
    pub color_id: i64,
    pub comment: String,
    pub date_created: String,
    /// "" = today (`insertTrack` fills it with `DBToolBox::getDateString`).
    pub date_added: String,
    /// Device path, `/Contents/…`.
    pub path: String,
    /// "" = the part of `path` after the last `/`.
    pub file_name: String,
    pub file_size: u64,
    pub file_type: i64,
    pub bitrate: i64,
    pub bit_depth: i64,
    pub sample_rate: u32,
    pub isrc: String,
    pub play_count: i64,
    pub hot_cue_auto_load: bool,
    pub kuvo_deliver: bool,
    pub kuvo_comment: String,
    pub master_db_id: u32,
    pub master_content_id: u32,
    /// Device path of `ANLZnnnn.DAT`.
    pub analysis_path: String,
    pub analysed_bits: u8,
    pub content_link: u32,
    pub has_modified: bool,
    /// `content.image_id` written by the insert itself (0 = not written). The export leaves it 0
    /// and adds the artwork afterwards ([`DevLibWriter::add_image`]).
    pub image_id: u32,
}

/// `analysedBits` the export inserts: the source `Analysed & 0x3F`
/// (`export_track_data_to_db`, RowDataTrack+0x3FC).
pub fn export_analysed_bits(analysed: i64) -> u8 {
    (analysed & 0x3F) as u8
}

/// `contentLink` the export inserts: `(source & 0xC1600) | 0x100` (`export_track_data_to_db`).
pub fn export_content_link(source: i64) -> u32 {
    ((source as u32) & 0xC1600) | 0x100
}

/// `contentLink` after a re-export of a track whose analysis or cues changed
/// (`DatabaseMediator::overwrite_export` @0x1009B8748): the device value gains 0x100 and the
/// source's 0x200, 0x400, 0x40000 and 0x80000 bits; 0x10000 and 0x100000 are taken from the
/// source only when it comes from the collection (`source_from_collection`, RowDataTrack+0x50
/// = 1 or 6). Returns `None` when nothing changes (rekordbox then skips `modifyContentLink`).
pub fn reexport_content_link(
    device: u32,
    source: u32,
    source_from_collection: bool,
) -> Option<u32> {
    let mut link = device | 0x100 | (source & (0x200 | 0x400 | 0x40000 | 0x80000));
    if source_from_collection {
        link |= source & (0x10000 | 0x100000);
    }
    (link != device).then_some(link)
}

/// Which row an image belongs to (`add_image` @0x100709C08 updates the owner's `image_id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageOwner {
    Content(u32),
    Playlist(u32),
    HotCueBankList(u32),
}

/// One `myTag` row as `writeMyTag` (@0x1007121E4) receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyTagRow {
    /// Groups: 1..; tags: master `djmdMyTag.ID`.
    pub id: u32,
    /// Already 0-based (`syncMyTagList` passes master `Seq - 1`).
    pub seq: i64,
    pub name: String,
    pub attribute: ListAttribute,
    /// 0 = top level.
    pub parent: u32,
}

/// One cue point as `setHotCue` (@0x10071D6C4) + `addCueOption2` (@0x100716058) write it.
/// Positions are in the units of the columns (`docs/re/devlib.md#cue`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewCue {
    pub content_id: u32,
    /// 0 memory cue / loop, 1..=17 hot cue slot.
    pub kind: i64,
    pub color_table_index: i64,
    pub comment: String,
    /// `isActiveLoop` is not written by `addCueOption2`; `Some` writes it
    /// (`enableActiveLoop`), `None` leaves NULL.
    pub active_loop: Option<bool>,
    pub beat_loop_numerator: i64,
    pub beat_loop_denominator: i64,
    pub in_usec: i64,
    /// -1 = no out point.
    pub out_usec: i64,
    pub in_150: i64,
    pub out_150: i64,
    pub in_mpeg_frame: i64,
    pub out_mpeg_frame: i64,
    pub in_mpeg_abs: i64,
    pub out_mpeg_abs: i64,
    pub in_decoding_start: i64,
    pub out_decoding_start: i64,
    pub in_file_offset: i64,
    pub out_file_offset: i64,
    pub in_samples: i64,
    pub out_samples: i64,
}

/// The lookup tables `getAndAdd*ID` fill on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Artist,
    Genre,
    Label,
    Key,
}

impl Lookup {
    fn table(self) -> &'static str {
        match self {
            Lookup::Artist => "artist",
            Lookup::Genre => "genre",
            Lookup::Label => "label",
            Lookup::Key => "key",
        }
    }

    fn index(self) -> usize {
        match self {
            Lookup::Artist => 0,
            Lookup::Genre => 1,
            Lookup::Label => 2,
            Lookup::Key => 3,
        }
    }
}

/// Today's date as `DBToolBox::getDateString` writes it (local time, `YYYY-MM-DD`).
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Prepared statements kept per connection. An export uses a few dozen distinct statements
/// (one per table for `max(id)`, the inserts, the `modify<Column>` updates).
const STATEMENT_CACHE: usize = 128;

/// Page cache of the writer's connection, in KiB (`PRAGMA cache_size = -N`): 64 MiB keeps the
/// `content` table of a large stick in memory (SQLCipher decrypts every page it reads in).
/// Connection-only; nothing is stored in the file.
const PAGE_CACHE_KIB: i64 = 65_536;

/// A cache slot: not read yet, loaded, or off because the table's DDL is not the one from
/// [`TABLES`] (the SQL then runs as is, so a foreign schema cannot change a match).
#[derive(Debug, Default)]
enum Slot<T> {
    #[default]
    Unloaded,
    Loaded(T),
    Off,
}

/// A stored integer-affinity value as SQL `=` against a bound integer sees it: equal only to
/// an INTEGER of the same value or a REAL that equals it numerically; NULL, TEXT and BLOB never.
#[derive(Debug, Clone, Copy)]
enum Num {
    Int(i64),
    Real(f64),
    Other,
}

impl Num {
    fn from_ref(v: ValueRef<'_>) -> Self {
        match v {
            ValueRef::Integer(i) => Num::Int(i),
            ValueRef::Real(r) => Num::Real(r),
            _ => Num::Other,
        }
    }

    fn equals(self, bound: i64) -> bool {
        match self {
            Num::Int(i) => i == bound,
            // `bound` is a u32/u8 value, exact as f64.
            Num::Real(r) => r == bound as f64,
            Num::Other => false,
        }
    }
}

/// One `album` row as `getAndAddAlbumID`'s select matches it.
#[derive(Debug, Clone, Copy)]
struct AlbumRow {
    id: i64,
    artist_id: Num,
    compilation: Num,
}

/// What the writer remembers between statements instead of scanning the table again. Each
/// entry reproduces one select exactly:
/// - `lookups[Lookup::index]`: `select <t>_id from <t> where name = ?` (first row in rowid
///   order, BINARY text comparison) → name bytes to the lowest rowid with that name;
/// - `albums`: `getAndAddAlbumID`'s select → the rows per name in rowid order, matched on
///   `artist_id` / `isComplation` like SQL `=`;
/// - `content_count`: `select count(*) from content` (`update_property_num_content`);
/// - `entry_counts`: `select count(content_id) from playlist_content where playlist_id = ?`.
///
/// Every write path of the writer keeps them in step or drops them; an error drops them all
/// (a statement may have rolled the transaction back).
#[derive(Debug, Default)]
struct Caches {
    lookups: [Slot<HashMap<Vec<u8>, i64>>; 4],
    albums: Slot<HashMap<Vec<u8>, Vec<AlbumRow>>>,
    content_count: Option<i64>,
    entry_counts: HashMap<u32, i64>,
    /// `content` has the DDL of [`TABLES`] (see [`DevLibWriter::add_track`]).
    content_standard: Option<bool>,
}

/// `insert into <table>(<columns>) values (?, …)`.
fn insert_sql<'a>(table: &str, columns: impl Iterator<Item = &'a str>) -> String {
    let columns: Vec<&str> = columns.collect();
    format!(
        "insert into {table}({}) values ({})",
        columns.join(", "),
        vec!["?"; columns.len()].join(", ")
    )
}

/// A lookup ID as `insertTrack` writes it: only when non-zero (NULL otherwise).
fn non_zero(id: u32) -> Option<u32> {
    (id != 0).then_some(id)
}

/// A read-write connection to exportLibrary.db.
pub struct DevLibWriter {
    conn: Connection,
    path: PathBuf,
    today: String,
    caches: RefCell<Caches>,
    /// A transaction opened with [`DevLibWriter::begin`] is open.
    in_tx: Cell<bool>,
}

impl DevLibWriter {
    /// Creates a new database like `DevSqliteDBController::startupDeviceDB` on a stick without
    /// one (parent directories included): open, key, `synchronous = NORMAL`, `journal_mode =
    /// WAL`, the 22 tables and 4 indexes, then the seed rows. Fails if the file exists.
    ///
    /// With [`CreateOptions::replay_conversion`] the initial seed and the conversion's drop,
    /// recreate and re-seed are replayed as well. The final seed order is that of the
    /// conversion (export-devlib.md phase 2): color, menuItem, category, sort, property.
    pub fn create(path: &Path, options: &CreateOptions) -> Result<Self> {
        if path.exists() {
            return Err(Error::Exists(path.display().to_string()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::Io(path.display().to_string(), e))?;
        }
        let mut writer = Self::open_with_flags(path, OpenFlags::SQLITE_OPEN_CREATE)?;
        if let Some(date) = &options.created_date {
            writer.today = date.clone();
        }
        writer.create_tables()?;
        writer.create_indexes()?;
        if options.replay_conversion {
            writer.seed_initial(options)?;
            writer.recreate_all()?;
        }
        writer.seed(options)?;
        Ok(writer)
    }

    /// Creates a new database for the Device Library Plus conversion of a stick that has only
    /// export.pdb (`docs/re/export.md` § 6.4): `startupDeviceDB` creates the file with the
    /// `initPropertyTable` seed (from `options`), then `execDbConversion` runs `clearAllData`.
    /// The tables are left empty; the caller inserts the converted rows with
    /// [`DevLibWriter::insert_all`].
    pub fn create_for_conversion(path: &Path, options: &CreateOptions) -> Result<Self> {
        if path.exists() {
            return Err(Error::Exists(path.display().to_string()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::Io(path.display().to_string(), e))?;
        }
        let mut writer = Self::open_with_flags(path, OpenFlags::SQLITE_OPEN_CREATE)?;
        if let Some(date) = &options.created_date {
            writer.today = date.clone();
        }
        writer.create_tables()?;
        writer.create_indexes()?;
        writer.seed_initial(options)?;
        writer.recreate_all()?;
        Ok(writer)
    }

    /// Opens an existing database read-write (`startupDeviceDB` on a stick that has one). Tables
    /// and indexes are only created if missing, as rekordbox's `createTables(false)` /
    /// `createIndexes(false)` do.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::Path(format!("{} does not exist", path.display())));
        }
        let writer = Self::open_with_flags(path, OpenFlags::empty())?;
        writer.create_tables()?;
        writer.create_indexes()?;
        Ok(writer)
    }

    fn open_with_flags(path: &Path, extra: OpenFlags) -> Result<Self> {
        // `SQLiteDB::open` @0x1010FC674: READWRITE | CREATE | FULLMUTEX, then `sqlite3_key`.
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_FULL_MUTEX | extra,
        )?;
        conn.pragma_update(None, "key", passphrase())?;
        // musicbox's own, connection-only (not stored in the file): a larger page cache and
        // statement cache.
        conn.execute_batch(&format!("PRAGMA cache_size = -{PAGE_CACHE_KIB}"))?;
        conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
        // `DevSQLiteDB::openDB` @0x1007399D4.
        conn.execute_batch("PRAGMA synchronous = NORMAL")?;
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Invalid(format!("journal_mode is {mode}, not wal")));
        }
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            today: today(),
            caches: RefCell::default(),
            in_tx: Cell::new(false),
        })
    }

    /// The raw connection. A write through it that inserts or deletes rows of `artist`,
    /// `genre`, `label`, `key`, `album`, `content` or `playlist_content` (or a `ROLLBACK`)
    /// must be followed by [`DevLibWriter::invalidate_caches`]; updates of other columns
    /// (e.g. a `content` lookup ID) need nothing.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Forgets every in-memory map and counter; the next use reads them from the database
    /// again.
    pub fn invalidate_caches(&self) {
        *self.caches.borrow_mut() = Caches::default();
    }

    /// Drops the caches when the transaction opened with [`DevLibWriter::begin`] is gone
    /// without [`DevLibWriter::commit`]: SQLite rolls it back by itself after some errors
    /// (SQLITE_FULL, SQLITE_IOERR, …), and a `ROLLBACK` may have come through
    /// [`DevLibWriter::connection`]. Called before every cache read.
    fn check_transaction(&self) {
        if self.in_tx.get() && self.conn.is_autocommit() {
            self.in_tx.set(false);
            self.invalidate_caches();
        }
    }

    /// Drops every cache when `result` is an error: the failed statement may have rolled the
    /// transaction back, or left a cache ahead of the database.
    fn guarded<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let result = f();
        if result.is_err() {
            self.invalidate_caches();
        }
        result
    }

    /// `Connection::execute` through the prepared-statement cache; returns the changed rows.
    fn exec_cached<P: rusqlite::Params>(&self, sql: &str, params: P) -> Result<usize> {
        Ok(self.conn.prepare_cached(sql)?.execute(params)?)
    }

    /// Overrides the date used for `createdDate` / empty `dateAdded` (default: today).
    pub fn set_today(&mut self, date: &str) {
        self.today = date.to_string();
    }

    /// Closes like `DevSqliteDBController::closeDB` → `SQLiteDB::close` (`sqlite3_close`): the
    /// last connection checkpoints the WAL into the main file and deletes `-wal` and `-shm`.
    /// Returns an error if either file is still there (another connection is open).
    pub fn close(self) -> Result<()> {
        let path = self.path.clone();
        self.conn.close().map_err(|(_, e)| Error::Sqlite(e))?;
        for suffix in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{suffix}", path.display()));
            if side.exists() {
                return Err(Error::Invalid(format!(
                    "{} still exists after close",
                    side.display()
                )));
            }
        }
        Ok(())
    }

    pub fn begin(&self) -> Result<()> {
        self.check_transaction();
        self.conn.execute_batch("BEGIN")?;
        self.in_tx.set(true);
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        self.check_transaction();
        self.in_tx.set(false);
        self.guarded(|| Ok(self.conn.execute_batch("COMMIT")?))
    }

    /// Rolls back the transaction opened with [`DevLibWriter::begin`] (and drops the caches).
    pub fn rollback(&self) -> Result<()> {
        self.in_tx.set(false);
        self.invalidate_caches();
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }

    fn exists(&self, kind: &str, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "select name from sqlite_master where type = ?1 and name = ?2",
                params![kind, name],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// `createTables(false)` @0x1006F4DF4: `create table` for every missing table.
    fn create_tables(&self) -> Result<()> {
        for (name, ddl) in TABLES {
            if !self.exists("table", name)? {
                self.conn.execute_batch(ddl)?;
            }
        }
        Ok(())
    }

    /// `createIndexes(false)` @0x1006F6BA8: no drop (only `createIndexes(true)` drops), and
    /// `SQLiteDB::createIndex` creates only the missing ones.
    fn create_indexes(&self) -> Result<()> {
        for (name, ddl) in INDEXES {
            if !self.exists("index", name)? {
                self.conn.execute_batch(ddl)?;
            }
        }
        Ok(())
    }

    /// Multi-row insert, as `SQLiteDB::insertIntoMultiple` sends it. Small statements (a
    /// track's My Tag links) go through the statement cache; the large ones of
    /// [`DevLibWriter::insert_all`] are prepared once and dropped.
    fn insert_multiple(&self, table: &str, columns: &[&str], rows: &[Vec<Value>]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let placeholders = format!("({})", vec!["?"; columns.len()].join(", "));
        let sql = format!(
            "insert into {table}({}) values {}",
            columns.join(", "),
            vec![placeholders; rows.len()].join(", ")
        );
        let params = params_from_iter(rows.iter().flatten());
        if rows.len() * columns.len() <= 64 {
            self.exec_cached(&sql, params)?;
        } else {
            self.conn.execute(&sql, params)?;
        }
        Ok(())
    }

    /// `insertAllDev*` (`DevSqliteDBController`, @0x100729AD0…@0x1007393B8): all rows of one
    /// table in one `insertIntoMultiple`. rekordbox inlines the values as literals; bound
    /// parameters store the same values, so the statement is split into chunks that stay below
    /// SQLite's parameter limit (the rows and their rowids come out the same).
    pub fn insert_all(&self, table: &str, columns: &[&str], rows: &[Vec<Value>]) -> Result<()> {
        self.invalidate_caches();
        let per_chunk = (32_000 / columns.len().max(1)).max(1);
        self.guarded(|| {
            for chunk in rows.chunks(per_chunk) {
                self.insert_multiple(table, columns, chunk)?;
            }
            Ok(())
        })
    }

    /// `initPropertyTable` @0x1006F26A8 on a new file: the property row first (`dbVersion`
    /// "10000"), then colours and menu items, then categories and sorts in master `Seq` order.
    fn seed_initial(&self, options: &CreateOptions) -> Result<()> {
        self.conn.execute(
            "insert into property(deviceName, dbVersion, numberOfContents, createdDate, \
             backGroundColorType, myTagMasterDBID) values ('', ?1, 0, ?2, 0, 0)",
            params![DB_VERSION_INIT, self.today],
        )?;
        let mut menus = options.menus.clone();
        menus.categories.sort_by_key(|c| (c.seq, c.id));
        menus.sorts.sort_by_key(|c| (c.seq, c.id));
        self.seed_menus(&menus)
    }

    /// `clearAllData` @0x1007284B0 = `createTables(true)` + `createIndexes(true)`, run by the
    /// Device Library Plus conversion (trace phase 2): `drop table if exists` + `create table`
    /// per table in creation order, then `drop index if exists` + `create index` per index.
    fn recreate_all(&self) -> Result<()> {
        self.invalidate_caches();
        for (name, ddl) in TABLES {
            self.conn
                .execute_batch(&format!("drop table if exists {name}"))?;
            self.conn.execute_batch(ddl)?;
        }
        for (name, ddl) in INDEXES {
            self.conn
                .execute_batch(&format!("drop index if exists {name}"))?;
            self.conn.execute_batch(ddl)?;
        }
        Ok(())
    }

    fn seed(&self, options: &CreateOptions) -> Result<()> {
        self.seed_menus(&options.menus)?;
        self.conn.execute(
            "insert into property(deviceName, dbVersion, numberOfContents, createdDate, \
             backGroundColorType, myTagMasterDBID) values (?1, ?2, 0, ?3, ?4, ?5)",
            params![
                options.device_name,
                options.db_version,
                self.today,
                options.background_color_type,
                options.my_tag_master_db_id,
            ],
        )?;
        Ok(())
    }

    fn seed_menus(&self, menus: &MenuSeed) -> Result<()> {
        self.insert_multiple(
            "color",
            &["color_id", "name"],
            &menus
                .colors
                .iter()
                .map(|(id, name)| vec![Value::from(*id), Value::from(name.clone())])
                .collect::<Vec<_>>(),
        )?;
        self.insert_multiple(
            "menuItem",
            &["menuItem_id", "kind", "name"],
            &menus
                .menu_items
                .iter()
                .map(|m| vec![m.id.into(), m.kind.into(), m.name.clone().into()])
                .collect::<Vec<_>>(),
        )?;
        self.insert_multiple(
            "category",
            &["category_id", "menuItem_id", "sequenceNo", "isVisible"],
            &menus
                .categories
                .iter()
                .map(|c| {
                    vec![
                        c.id.into(),
                        c.menu_item_id.into(),
                        c.seq.into(),
                        i64::from(c.visible).into(),
                    ]
                })
                .collect::<Vec<_>>(),
        )?;
        self.insert_multiple(
            "sort",
            &[
                "sort_id",
                "menuItem_id",
                "sequenceNo",
                "isVisible",
                "isSelectedAsSubColumn",
            ],
            &menus
                .sorts
                .iter()
                .map(|s| {
                    vec![
                        s.id.into(),
                        s.menu_item_id.into(),
                        s.seq.into(),
                        i64::from(s.visible).into(),
                        i64::from(s.sub_column).into(),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    }

    // ---- property -------------------------------------------------------------------------

    /// `update_property_num_content` @0x10070669C: `numberOfContents = (select count(*) from
    /// content)`. The count is kept in memory once it has been read (one scan, then +1 per
    /// [`DevLibWriter::add_track`]); the stored value is the same.
    pub fn update_number_of_contents(&self) -> Result<()> {
        self.guarded(|| {
            let count = self.content_count()?;
            self.conn
                .prepare_cached("update property set numberOfContents = ?1")?
                .execute([count])?;
            Ok(())
        })
    }

    /// `select count(*) from content`, read once and then kept in step.
    fn content_count(&self) -> Result<i64> {
        self.check_transaction();
        if let Some(count) = self.caches.borrow().content_count {
            return Ok(count);
        }
        let count: i64 = self
            .conn
            .prepare_cached("select count(*) from content")?
            .query_row([], |r| r.get(0))?;
        self.caches.borrow_mut().content_count = Some(count);
        Ok(count)
    }

    /// [`Caches::content_standard`], read once.
    fn content_is_standard(&self) -> Result<bool> {
        if let Some(standard) = self.caches.borrow().content_standard {
            return Ok(standard);
        }
        let standard = self.standard_table("content")?;
        self.caches.borrow_mut().content_standard = Some(standard);
        Ok(standard)
    }

    /// `updateDeviceName` @0x1006F7D3C.
    pub fn set_device_name(&self, name: &str) -> Result<()> {
        self.conn
            .execute("update property set deviceName = ?1", [name])?;
        Ok(())
    }

    /// `selectDeviceColor` @0x1007189D8.
    pub fn set_background_color_type(&self, value: u32) -> Result<()> {
        self.conn
            .execute("update property set backGroundColorType = ?1", [value])?;
        Ok(())
    }

    /// `updateColorComment` @0x1006FD0B4: rename a colour.
    pub fn set_color_name(&self, color_id: u32, name: &str) -> Result<()> {
        self.conn.execute(
            "update color set name = ?1 where color_id = ?2",
            params![name, color_id],
        )?;
        Ok(())
    }

    // ---- IDs ------------------------------------------------------------------------------

    /// `get_next_id` @0x100703FF4: `max(<id>) + 1`, 1 for an empty table. On the integer
    /// primary key this is a single b-tree lookup, so it is not cached.
    fn next_id(&self, table: &str, id_column: &str) -> Result<i64> {
        let max: Option<i64> = self
            .conn
            .prepare_cached(&format!("select max({id_column}) as maxId from {table}"))?
            .query_row([], |r| r.get(0))?;
        Ok(max.unwrap_or(0) + 1)
    }

    // ---- lookups --------------------------------------------------------------------------

    /// Whether `table` has exactly the DDL of [`TABLES`] (as `sqlite_master` stores it), so
    /// its column affinities and collations are the ones the caches assume.
    fn standard_table(&self, table: &str) -> Result<bool> {
        let Some((_, ddl)) = TABLES.iter().find(|(name, _)| *name == table) else {
            return Ok(false);
        };
        let sql: Option<Option<String>> = self
            .conn
            .prepare_cached("select sql from sqlite_master where type = 'table' and name = ?1")?
            .query_row([table], |r| r.get(0))
            .optional()?;
        Ok(sql.flatten().as_deref()
            == Some(ddl.replacen("create table", "CREATE TABLE", 1).as_str()))
    }

    /// Loads the name map of `lookup` unless it is loaded or off.
    fn ensure_lookup(&self, lookup: Lookup) -> Result<()> {
        self.check_transaction();
        let i = lookup.index();
        if !matches!(self.caches.borrow().lookups[i], Slot::Unloaded) {
            return Ok(());
        }
        let table = lookup.table();
        let slot = if self.standard_table(table)? {
            // Rowid order = the order of the full scan `where name = ?` runs, so the first
            // row kept per name is the one the select returns. NULL and BLOB names never
            // equal a text value.
            let mut map: HashMap<Vec<u8>, i64> = HashMap::new();
            let mut stmt = self.conn.prepare(&format!(
                "select {table}_id, name from {table} order by rowid"
            ))?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let id: i64 = r.get(0)?;
                if let ValueRef::Text(name) = r.get_ref(1)? {
                    map.entry(name.to_vec()).or_insert(id);
                }
            }
            Slot::Loaded(map)
        } else {
            Slot::Off
        };
        self.caches.borrow_mut().lookups[i] = slot;
        Ok(())
    }

    /// `get{Artist,Genre,Label,Key}ID`: exact (case-sensitive) name match, 0 when not found.
    pub fn lookup_id(&self, lookup: Lookup, name: &str) -> Result<u32> {
        self.guarded(|| {
            self.ensure_lookup(lookup)?;
            if let Slot::Loaded(map) = &self.caches.borrow().lookups[lookup.index()] {
                return Ok(map.get(name.as_bytes()).map_or(0, |&id| id as u32));
            }
            let table = lookup.table();
            Ok(self
                .conn
                .prepare_cached(&format!("select {table}_id from {table} where name = ?1"))?
                .query_row([name], |r| r.get::<_, Option<i64>>(0))
                .optional()?
                .flatten()
                .unwrap_or(0) as u32)
        })
    }

    /// `getAndAdd{Artist,Genre,Label,Key}ID` (@0x10070421C, @0x100703C3C, …): the existing ID
    /// for `name`, else a new row `(max+1, name)`. An empty name gives 0 and touches nothing.
    /// rekordbox inlines the name as a double-quoted literal, so a name equal to a column name
    /// of the table (`name`, `genre_id`, …) matches every row there; this writer compares the
    /// value instead.
    pub fn get_or_add(&self, lookup: Lookup, name: &str) -> Result<u32> {
        if name.is_empty() {
            return Ok(0);
        }
        self.guarded(|| {
            let found = self.lookup_id(lookup, name)?;
            if found != 0 {
                return Ok(found);
            }
            let table = lookup.table();
            let id = self.next_id(table, &format!("{table}_id"))?;
            self.conn
                .prepare_cached(&format!(
                    "insert into {table}({table}_id, name) values (?1, ?2)"
                ))?
                .execute(params![id, name])?;
            // max + 1 is above every ID, so a name already in the map keeps its lower one.
            if let Slot::Loaded(map) = &mut self.caches.borrow_mut().lookups[lookup.index()] {
                map.entry(name.as_bytes().to_vec()).or_insert(id);
            }
            Ok(id as u32)
        })
    }

    /// Loads the album map unless it is loaded or off.
    fn ensure_albums(&self) -> Result<()> {
        self.check_transaction();
        if !matches!(self.caches.borrow().albums, Slot::Unloaded) {
            return Ok(());
        }
        let slot = if self.standard_table("album")? {
            let mut map: HashMap<Vec<u8>, Vec<AlbumRow>> = HashMap::new();
            let mut stmt = self.conn.prepare(
                "select album_id, name, artist_id, isComplation from album order by rowid",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                if let ValueRef::Text(name) = r.get_ref(1)? {
                    map.entry(name.to_vec()).or_default().push(AlbumRow {
                        id: r.get(0)?,
                        artist_id: Num::from_ref(r.get_ref(2)?),
                        compilation: Num::from_ref(r.get_ref(3)?),
                    });
                }
            }
            Slot::Loaded(map)
        } else {
            Slot::Off
        };
        self.caches.borrow_mut().albums = slot;
        Ok(())
    }

    /// `getAndAddAlbumID(name, compilation, albumArtist, …)` @0x100704388:
    /// 1. the album artist is looked up/added in `artist` first;
    /// 2. the album is matched on `name` plus `artist_id` when there is an album artist and
    ///    `isComplation` when the flag is set;
    /// 3. a new row gets `(album_id, name, [artist_id,] isComplation)`; `image_id` and
    ///    `nameForSearch` stay NULL.
    ///
    /// An empty name gives 0 (rekordbox's `addTrack` never calls it then).
    pub fn get_or_add_album(&self, name: &str, compilation: u8, album_artist: &str) -> Result<u32> {
        if name.is_empty() {
            return Ok(0);
        }
        self.guarded(|| {
            let artist_id = self.get_or_add(Lookup::Artist, album_artist)?;
            self.ensure_albums()?;
            let cached: Option<Option<i64>> = match &self.caches.borrow().albums {
                Slot::Loaded(map) => Some(
                    map.get(name.as_bytes())
                        .and_then(|rows| {
                            rows.iter().find(|a| {
                                (artist_id == 0 || a.artist_id.equals(i64::from(artist_id)))
                                    && (compilation == 0
                                        || a.compilation.equals(i64::from(compilation)))
                            })
                        })
                        .map(|a| a.id),
                ),
                _ => None,
            };
            let found: Option<i64> = match cached {
                Some(found) => found,
                None => {
                    let mut sql = String::from("select album_id from album where name = ?1");
                    let mut args: Vec<Value> = vec![name.to_string().into()];
                    if artist_id != 0 {
                        sql.push_str(" and artist_id = ?2");
                        args.push(artist_id.into());
                    }
                    if compilation != 0 {
                        sql.push_str(&format!(" and isComplation = ?{}", args.len() + 1));
                        args.push(i64::from(compilation).into());
                    }
                    self.conn
                        .prepare_cached(&sql)?
                        .query_row(params_from_iter(args), |r| r.get::<_, Option<i64>>(0))
                        .optional()?
                        .flatten()
                }
            };
            if let Some(id) = found.filter(|&id| id != 0) {
                return Ok(id as u32);
            }
            let id = self.next_id("album", "album_id")?;
            if artist_id != 0 {
                self.conn
                    .prepare_cached(
                        "insert into album(album_id, name, artist_id, isComplation) \
                         values (?1, ?2, ?3, ?4)",
                    )?
                    .execute(params![id, name, artist_id, compilation])?;
            } else {
                self.conn
                    .prepare_cached(
                        "insert into album(album_id, name, isComplation) values (?1, ?2, ?3)",
                    )?
                    .execute(params![id, name, compilation])?;
            }
            if let Slot::Loaded(map) = &mut self.caches.borrow_mut().albums {
                map.entry(name.as_bytes().to_vec())
                    .or_default()
                    .push(AlbumRow {
                        id,
                        artist_id: if artist_id != 0 {
                            Num::Int(i64::from(artist_id))
                        } else {
                            Num::Other
                        },
                        compilation: Num::Int(i64::from(compilation)),
                    });
            }
            Ok(id as u32)
        })
    }

    // ---- content --------------------------------------------------------------------------

    /// `rekordboxDBController::addTrack` @0x1017E1EE4 followed by
    /// `DevSqliteDBController::insertTrack` @0x10070562C:
    /// 1. lookups, in this order and only for non-empty names: artist, genre, album (with its
    ///    album artist), remixer, label, original artist, key, composer;
    /// 2. `content_id = max + 1`; the lyricist is looked up/added while the row is built;
    /// 3. one INSERT; `artist_id_artist`, `album_id`, `genre_id`, `artist_id_remixer`,
    ///    `label_id`, `artist_id_originalArtist`, `artist_id_composer` and `image_id` are only
    ///    written when non-zero (NULL otherwise), `key_id` and `artist_id_lyricist` always;
    ///    `titleForSearch` and the three `*UpdateCount` markers are never written;
    /// 4. `numberOfContents` is refreshed.
    pub fn add_track(&self, t: &NewTrack) -> Result<u32> {
        self.guarded(|| {
            let artist = self.get_or_add(Lookup::Artist, &t.artist)?;
            let genre = self.get_or_add(Lookup::Genre, &t.genre)?;
            let album = self.get_or_add_album(&t.album, t.compilation, &t.album_artist)?;
            let remixer = self.get_or_add(Lookup::Artist, &t.remixer)?;
            let label = self.get_or_add(Lookup::Label, &t.label)?;
            let original_artist = self.get_or_add(Lookup::Artist, &t.original_artist)?;
            let key = self.get_or_add(Lookup::Key, &t.key)?;
            let composer = self.get_or_add(Lookup::Artist, &t.composer)?;

            let content_id = self.next_id("content", "content_id")?;
            let file_name = if t.file_name.is_empty() {
                t.path.rsplit('/').next().unwrap_or_default()
            } else {
                t.file_name.as_str()
            };
            let date_added = if t.date_added.is_empty() {
                self.today.as_str()
            } else {
                t.date_added.as_str()
            };
            let lyricist = self.get_or_add(Lookup::Artist, &t.lyricist)?;

            // Widths as RowDataTrack stores them.
            let bpm = t.bpm_x100 as i32;
            let length = i64::from(t.length as i16);
            let track_no = t.track_no as i32;
            let bitrate = t.bitrate as i32;
            let bit_depth = i64::from(t.bit_depth as i16);
            let file_type = i64::from(t.file_type as i8);
            let rating = i64::from(t.rating as i8);
            let release_year = i64::from(t.release_year as i16);
            let color_id = i64::from(t.color_id as i8);
            let play_count = i64::from(t.play_count as i16);
            let file_size = t.file_size as u32;
            let disc_no = i64::from(t.disc_no as i16);
            let analysed_bits = i64::from(t.analysed_bits as i8);
            let has_modified = i64::from(t.has_modified);
            let hot_cue_auto_load = i64::from(t.hot_cue_auto_load);
            let kuvo_deliver = i64::from(t.kuvo_deliver);
            // (column, value, written by rekordbox). Column order = the NamedValueSet insertion
            // order in insertTrack (SQL trace); lookup IDs and image_id are only put into the
            // row when non-zero.
            let row: [(&str, &dyn ToSql, bool); 42] = [
                ("content_id", &content_id, true),
                ("path", &t.path, true),
                ("fileName", &file_name, true),
                ("title", &t.title, true),
                ("artist_id_artist", &non_zero(artist), artist != 0),
                ("album_id", &non_zero(album), album != 0),
                ("genre_id", &non_zero(genre), genre != 0),
                ("bpmx100", &bpm, true),
                ("length", &length, true),
                ("trackNo", &track_no, true),
                ("bitrate", &bitrate, true),
                ("bitDepth", &bit_depth, true),
                ("djComment", &t.comment, true),
                ("fileType", &file_type, true),
                ("rating", &rating, true),
                ("releaseYear", &release_year, true),
                ("artist_id_remixer", &non_zero(remixer), remixer != 0),
                ("label_id", &non_zero(label), label != 0),
                (
                    "artist_id_originalArtist",
                    &non_zero(original_artist),
                    original_artist != 0,
                ),
                ("key_id", &key, true),
                ("dateCreated", &t.date_created, true),
                ("color_id", &color_id, true),
                ("djPlayCount", &play_count, true),
                ("image_id", &non_zero(t.image_id), t.image_id != 0),
                ("masterDbId", &t.master_db_id, true),
                ("masterContentId", &t.master_content_id, true),
                ("analysisDataFilePath", &t.analysis_path, true),
                ("fileSize", &file_size, true),
                ("discNo", &disc_no, true),
                ("artist_id_composer", &non_zero(composer), composer != 0),
                ("subtitle", &t.subtitle, true),
                ("samplingRate", &t.sample_rate, true),
                ("analysedBits", &analysed_bits, true),
                ("releaseDate", &t.release_date, true),
                ("dateAdded", &date_added, true),
                ("contentLink", &t.content_link, true),
                ("hasModified", &has_modified, true),
                ("isHotCueAutoLoadOn", &hot_cue_auto_load, true),
                ("isKuvoDeliverStatusOn", &kuvo_deliver, true),
                ("kuvoDeliveryComment", &t.kuvo_comment, true),
                ("artist_id_lyricist", &lyricist, true),
                ("isrc", &t.isrc, true),
            ];
            if self.content_is_standard()? {
                // Every column named, NULL bound where rekordbox leaves one out: the same row
                // (no column of the standard DDL has a default) with one statement text for
                // every track, so it is prepared once.
                static SQL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
                let sql = SQL.get_or_init(|| insert_sql("content", row.iter().map(|c| c.0)));
                let values: Vec<&dyn ToSql> = row.iter().map(|c| c.1).collect();
                self.conn.prepare_cached(sql)?.execute(&values[..])?;
            } else {
                let kept: Vec<_> = row.iter().filter(|c| c.2).collect();
                let values: Vec<&dyn ToSql> = kept.iter().map(|c| c.1).collect();
                self.conn
                    .prepare_cached(&insert_sql("content", kept.iter().map(|c| c.0)))?
                    .execute(&values[..])?;
            }
            if let Some(count) = &mut self.caches.borrow_mut().content_count {
                *count += 1;
            }
            self.update_number_of_contents()?;
            Ok(content_id as u32)
        })
    }

    /// Content row of a track already on the device, matched like
    /// `getContentIDBsByMasterIDs` (@0x1006FA6D8): `masterDbId` and `masterContentId`.
    pub fn content_ids_by_master(
        &self,
        master_db_id: u32,
        master_content_id: u32,
    ) -> Result<Vec<u32>> {
        let mut stmt = self.conn.prepare_cached(
            "select content_id from content where masterDbId = ?1 and masterContentId = ?2",
        )?;
        let ids = stmt
            .query_map(params![master_db_id, master_content_id], |r| {
                r.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids.into_iter().map(|id| id as u32).collect())
    }

    /// The device copy of a source track, as `isCollectionSongForDuplicateSongId`
    /// (@0x1017E1330) finds it: same master IDs and the same length, bitrate, bit depth, file
    /// type, file size and sample rate (`RowDataTrack::isSameTrackForDuplicateId`, compared at
    /// the widths `insertTrack` stores). A re-encoded file therefore counts as a new track.
    pub fn find_exported_track(&self, t: &NewTrack) -> Result<Option<u32>> {
        let mut stmt = self.conn.prepare_cached(
            "select content_id, length, bitrate, bitDepth, fileType, fileSize, samplingRate \
             from content where masterDbId = ?1 and masterContentId = ?2 order by content_id",
        )?;
        let want = [
            i64::from(t.length as i16),
            i64::from(t.bitrate as i32),
            i64::from(t.bit_depth as i16),
            i64::from(t.file_type as i8),
            i64::from(t.file_size as u32),
            i64::from(t.sample_rate),
        ];
        let mut rows = stmt.query(params![t.master_db_id, t.master_content_id])?;
        while let Some(r) = rows.next()? {
            let have: Vec<i64> = (1..=6)
                .map(|i| r.get::<_, Option<i64>>(i).map(|v| v.unwrap_or(0)))
                .collect::<rusqlite::Result<_>>()?;
            if have == want {
                return Ok(Some(r.get::<_, i64>(0)? as u32));
            }
        }
        Ok(None)
    }

    /// The three change markers of a device row as rekordbox compares them
    /// (`compareTrackUpdated` @0x1006F1AF4 & co. read the column as `var::toString`, so NULL is
    /// ""): (information, analysis, cue).
    pub fn track_markers(&self, content_id: u32) -> Result<(String, String, String)> {
        let as_text = |v: Value| match v {
            Value::Null | Value::Blob(_) => String::new(),
            Value::Integer(i) => i.to_string(),
            Value::Real(r) => r.to_string(),
            Value::Text(t) => t,
        };
        Ok(self
            .conn
            .prepare_cached(
                "select informationUpdateCount, analysisDataUpdateCount, cueUpdateCount \
                 from content where content_id = ?1",
            )?
            .query_row([content_id], |r| {
                Ok((as_text(r.get(0)?), as_text(r.get(1)?), as_text(r.get(2)?)))
            })?)
    }

    /// `contentLink` of a device row (`get_content_link` @0x1007251EC); NULL reads as 0.
    pub fn content_link(&self, content_id: u32) -> Result<u32> {
        Ok(self
            .conn
            .prepare_cached("select contentLink from content where content_id = ?1")?
            .query_row([content_id], |r| r.get::<_, Option<i64>>(0))?
            .unwrap_or(0) as u32)
    }

    /// `setTrackUpdated` @0x1006F1FEC: stores the info change marker (`informationUpdateCount`,
    /// bound as text, so a number gets integer affinity and "" stays text).
    pub fn set_track_updated(&self, content_id: u32, info_marker: &str) -> Result<()> {
        self.exec_cached(
            "update content set informationUpdateCount = ?1 where content_id = ?2",
            params![info_marker, content_id],
        )?;
        Ok(())
    }

    /// `setAnalysisUpdated` @0x1006F21EC: `analysedBits` (the full source `Analysed` byte),
    /// `cueUpdateCount` and `analysisDataUpdateCount` (both bound as text).
    pub fn set_analysis_updated(
        &self,
        content_id: u32,
        analysed: u8,
        analysis_marker: &str,
        cue_marker: &str,
    ) -> Result<()> {
        self.exec_cached(
            "update content set analysedBits = ?1, cueUpdateCount = ?2, \
             analysisDataUpdateCount = ?3 where content_id = ?4",
            params![analysed, cue_marker, analysis_marker, content_id],
        )?;
        Ok(())
    }

    /// `modifyContentLink` @0x10071EAAC.
    pub fn set_content_link(&self, content_id: u32, content_link: u32) -> Result<()> {
        self.exec_cached(
            "update content set contentLink = ?1 where content_id = ?2",
            params![content_link, content_id],
        )?;
        Ok(())
    }

    /// `setAnalysisDataPath` @0x100722A24.
    pub fn set_analysis_path(&self, content_id: u32, path: &str) -> Result<()> {
        self.exec_cached(
            "update content set analysisDataFilePath = ?1 where content_id = ?2",
            params![path, content_id],
        )?;
        Ok(())
    }

    /// The `modify<Column>` family for plain columns (`modifyTitle`, `modifyBPM`,
    /// `modifyComment`, …, each `update content set <col> = v where content_id = id`).
    /// Lookup columns go through [`DevLibWriter::modify_lookup`] instead.
    pub fn modify_column(
        &self,
        content_id: u32,
        column: ContentColumn,
        value: Value,
    ) -> Result<()> {
        self.exec_cached(
            &format!(
                "update content set {} = ?1 where content_id = ?2",
                column.name()
            ),
            params![value, content_id],
        )?;
        Ok(())
    }

    /// `modifyGenre` @0x10071FB08 and its siblings (`modifyLabel`, `modifyKeyScaleName`,
    /// `modifyRemixer`, `modifyOrgArtist`, `modifyComposer`, `modifyLyricist`,
    /// `modifyArtistByContentId`): look up/add the name, write its ID (**0**, not NULL, for an
    /// empty name), then delete lookup rows no track uses any more (`delete_unused_record`
    /// @0x100719890).
    pub fn modify_lookup(&self, content_id: u32, column: LookupColumn, name: &str) -> Result<()> {
        self.guarded(|| {
            let id = self.get_or_add(column.lookup(), name)?;
            self.exec_cached(
                &format!(
                    "update content set {} = ?1 where content_id = ?2",
                    column.name()
                ),
                params![id, content_id],
            )?;
            self.delete_unused(column.lookup())
        })
    }

    /// `delete_unused_record` @0x100719890 for one lookup table: rows that no `content`
    /// column (and for artists no `album.artist_id`) references with a value > 0.
    pub fn delete_unused(&self, lookup: Lookup) -> Result<()> {
        let sql = match lookup {
            Lookup::Artist => {
                "delete from artist where artist_id not in (select artist_id_artist from content where artist_id_artist > 0) \
                 and artist_id not in (select artist_id_remixer from content where artist_id_remixer > 0) \
                 and artist_id not in (select artist_id_composer from content where artist_id_composer > 0) \
                 and artist_id not in (select artist_id_lyricist from content where artist_id_lyricist > 0) \
                 and artist_id not in (select artist_id_originalArtist from content where artist_id_originalArtist > 0) \
                 and artist_id not in (select artist_id from album where artist_id > 0)"
            }
            Lookup::Genre => {
                "delete from genre where genre_id not in (select genre_id from content where genre_id > 0)"
            }
            Lookup::Label => {
                "delete from label where label_id not in (select label_id from content where label_id > 0)"
            }
            Lookup::Key => {
                "delete from key where key_id not in (select key_id from content where key_id > 0)"
            }
        };
        let deleted = self.guarded(|| self.exec_cached(sql, []))?;
        if deleted > 0 {
            self.caches.borrow_mut().lookups[lookup.index()] = Slot::Unloaded;
        }
        Ok(())
    }

    // ---- images ---------------------------------------------------------------------------

    /// `addImage` → `calc_next_device_image_path` (@0x10070A018) → `add_image` (@0x100709C08):
    /// `image_id = max + 1`, path `/PIONEER/Artwork/%05d/b<id>.jpg` with folder `id / 20 + 1`
    /// (`/.PIONEER` when `hidden_pioneer`), then the owner's `image_id` is set. The JPEG files
    /// (`b<id>.jpg` 80×80 and `b<id>_m.jpg` 240×240) are the caller's job.
    pub fn add_image(&self, owner: ImageOwner, hidden_pioneer: bool) -> Result<(u32, String)> {
        let id = self.next_id("image", "image_id")? as u32;
        let path = device_image_path(id, hidden_pioneer);
        self.exec_cached(
            "insert into image(image_id, path) values (?1, ?2)",
            params![id, path],
        )?;
        let (table, owner_id) = match owner {
            ImageOwner::Content(id) => ("content", id),
            ImageOwner::Playlist(id) => ("playlist", id),
            ImageOwner::HotCueBankList(id) => ("hotCueBankList", id),
        };
        self.exec_cached(
            &format!("update {table} set image_id = ?1 where {table}_id = ?2"),
            params![id, owner_id],
        )?;
        Ok((id, path))
    }

    // ---- playlists ------------------------------------------------------------------------

    fn parent_clause(column: &str, parent: u32) -> String {
        if parent == 0 {
            format!("{column} = 0 or {column} is null")
        } else {
            format!("{column} = {parent}")
        }
    }

    /// `existsListName` (@0x1006FFB34): a list or folder with this exact name under `parent`.
    pub fn find_playlist(&self, parent: u32, name: &str) -> Result<Option<(u32, ListAttribute)>> {
        let sql = format!(
            "select playlist_id, attribute from playlist where name = ?1 and ({})",
            Self::parent_clause("playlist_id_parent", parent)
        );
        Ok(self
            .conn
            .query_row(&sql, [name], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u32,
                    ListAttribute::from_code(r.get::<_, Option<i64>>(1)?.unwrap_or(0)),
                ))
            })
            .optional()?)
    }

    /// `rekordboxDBController::createNewList` @0x1017E6808 for playlists and folders:
    /// 1. `sequenceNo` = 0 when `parent` has no lists yet, else `max(sequenceNo) + 1`
    ///    (`getListCountInFolder`, `getPlaylistFolderSeqMax` @0x100720A80);
    /// 2. `insertPlaylist` @0x100720DB4: `(playlist_id = max+1, sequenceNo, name, attribute,
    ///    playlist_id_parent)`, `image_id` NULL;
    /// 3. with `position = Some(p)` and `p <= sequenceNo` the list is then moved to `p`
    ///    ([`DevLibWriter::move_playlist`]). The USB export passes position 0, so each newly
    ///    exported list lands on top.
    pub fn create_playlist(
        &self,
        name: &str,
        parent: u32,
        folder: bool,
        position: Option<i64>,
    ) -> Result<u32> {
        let where_parent = Self::parent_clause("playlist_id_parent", parent);
        let count: i64 = self.conn.query_row(
            &format!("select count(playlist_id) as numList from playlist where {where_parent}"),
            [],
            |r| r.get(0),
        )?;
        let seq = if count == 0 {
            0
        } else {
            let max: Option<i64> = self.conn.query_row(
                &format!("select max(sequenceNo) as maxSeq from playlist where {where_parent}"),
                [],
                |r| r.get(0),
            )?;
            max.unwrap_or(0) + 1
        };
        let id = self.next_id("playlist", "playlist_id")?;
        self.conn.execute(
            "insert into playlist(playlist_id, sequenceNo, name, attribute, playlist_id_parent) \
             values (?1, ?2, ?3, ?4, ?5)",
            params![id, seq, name, i64::from(folder), parent],
        )?;
        if let Some(position) = position
            && position <= seq
        {
            self.move_playlist(id as u32, parent, position)?;
        }
        Ok(id as u32)
    }

    /// `movePlaylist` @0x100721AB0 → `move_sequence` @0x100710230 within one parent: siblings
    /// between the old and the new position shift by one, then the list takes `position`. Runs
    /// in a transaction like rekordbox (`transactionStart(2)`).
    pub fn move_playlist(&self, playlist_id: u32, parent: u32, position: i64) -> Result<()> {
        self.move_sequence(
            "playlist",
            "playlist_id",
            "playlist_id_parent",
            playlist_id,
            parent,
            position,
        )
    }

    fn move_sequence(
        &self,
        table: &str,
        id_col: &str,
        parent_col: &str,
        id: u32,
        parent: u32,
        position: i64,
    ) -> Result<()> {
        let current: Option<Option<i64>> = self
            .conn
            .query_row(
                &format!(
                    "select sequenceNo from {table} where {id_col} = ?1 and {parent_col} = ?2"
                ),
                params![id, parent],
                |r| r.get(0),
            )
            .optional()?;
        let Some(current) = current.map(|s| s.unwrap_or(0)) else {
            return Ok(());
        };
        let shift = if current < position {
            format!(
                "update {table} set sequenceNo = sequenceNo - 1 where sequenceNo > {current} \
                 and sequenceNo <= {position} and {parent_col} = {parent}"
            )
        } else if current > position {
            format!(
                "update {table} set sequenceNo = sequenceNo + 1 where sequenceNo >= {position} \
                 and sequenceNo < {current} and {parent_col} = {parent}"
            )
        } else {
            return Ok(());
        };
        let in_tx = !self.conn.is_autocommit();
        if !in_tx {
            self.conn.execute_batch("BEGIN")?;
        }
        let result = (|| {
            self.conn.execute(&shift, [])?;
            self.conn.execute(
                &format!(
                    "update {table} set sequenceNo = ?1 where {id_col} = ?2 and {parent_col} = ?3"
                ),
                params![position, id, parent],
            )?;
            Ok::<_, rusqlite::Error>(())
        })();
        if !in_tx {
            self.conn
                .execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        }
        Ok(result?)
    }

    /// Appends a track to a playlist like the export (`add_track_to_device_playlist`): entry
    /// `sequenceNo = count(entries) + 1` (1-based). The count (`count(content_id)` of the
    /// list's entries) is read once per list and then kept in memory.
    pub fn append_playlist_entry(&self, playlist_id: u32, content_id: u32) -> Result<()> {
        self.check_transaction();
        self.guarded(|| {
            let cached = self.caches.borrow().entry_counts.get(&playlist_id).copied();
            let count = match cached {
                Some(count) => count,
                None => self
                    .conn
                    .prepare_cached(
                        "select count(content_id) as numSongs from playlist_content \
                         where playlist_id = ?1",
                    )?
                    .query_row([playlist_id], |r| r.get::<_, i64>(0))?,
            };
            self.exec_cached(
                "insert into playlist_content(playlist_id, content_id, sequenceNo) values (?1, ?2, ?3)",
                params![playlist_id, content_id, count + 1],
            )?;
            // content_id is never NULL, so the new entry counts.
            self.caches
                .borrow_mut()
                .entry_counts
                .insert(playlist_id, count + 1);
            Ok(())
        })
    }

    /// `deleteAllTracksFromPlaylist` @0x1007089BC (a changed list is emptied before re-export).
    pub fn clear_playlist(&self, playlist_id: u32) -> Result<()> {
        self.caches.borrow_mut().entry_counts.remove(&playlist_id);
        self.guarded(|| {
            self.exec_cached(
                "delete from playlist_content where playlist_id = ?1",
                [playlist_id],
            )?;
            Ok(())
        })
    }

    /// Runs `f` in a transaction unless the caller already opened one (rekordbox's
    /// `transactionStart(2)` … `transactionFinish`). Any error drops the caches.
    fn in_transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.guarded(|| {
            let own = self.conn.is_autocommit();
            if own {
                self.conn.execute_batch("BEGIN")?;
            }
            let result = f();
            if own {
                self.conn
                    .execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
            }
            result
        })
    }

    /// `deleteList` @0x100708C04 for a playlist or folder (`docs/re/export.md` § 4.3): the list
    /// and its descendants (`delete_lists` @0x10072563C), their entries (`delete_listsongs`
    /// @0x100725A30, whose content IDs are returned, list by list in entry order), then the
    /// remaining children of the parent renumbered `sequenceNo` 0, 1, 2, …
    /// (`renumber_as_delete_children` @0x10072684C → `renumber_sequence` @0x10070F3B8). Returns
    /// `(deleted list IDs: descendants depth first, the list itself last; content IDs)`.
    pub fn delete_playlist(&self, playlist_id: u32) -> Result<(Vec<u32>, Vec<u32>)> {
        self.guarded(|| self.delete_playlist_inner(playlist_id))
    }

    fn delete_playlist_inner(&self, playlist_id: u32) -> Result<(Vec<u32>, Vec<u32>)> {
        let parent: Option<Option<i64>> = self
            .conn
            .query_row(
                "select playlist_id_parent from playlist where playlist_id = ?1",
                [playlist_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(parent) = parent else {
            return Ok((Vec::new(), Vec::new()));
        };
        let parent = parent.unwrap_or(0) as u32;
        let mut lists = Vec::new();
        self.collect_child_lists(playlist_id, &mut lists)?;
        lists.push(playlist_id);
        let ids = join_ids(&lists);
        self.conn.execute(
            &format!("delete from playlist where playlist_id in ({ids})"),
            [],
        )?;
        let mut contents = Vec::new();
        for l in &lists {
            let mut stmt = self.conn.prepare_cached(
                "select content_id from playlist_content where playlist_id = ?1 order by sequenceNo",
            )?;
            for c in stmt.query_map([l], |r| r.get::<_, Option<i64>>(0))? {
                contents.push(c?.unwrap_or(0) as u32);
            }
        }
        self.conn.execute(
            &format!("delete from playlist_content where playlist_id in ({ids})"),
            [],
        )?;
        self.caches.borrow_mut().entry_counts.clear();
        self.renumber_children(parent)?;
        Ok((lists, contents))
    }

    fn collect_child_lists(&self, parent: u32, out: &mut Vec<u32>) -> Result<()> {
        let kids: Vec<u32> = {
            let mut stmt = self.conn.prepare_cached(
                "select playlist_id from playlist where playlist_id_parent = ?1 and playlist_id != ?1 \
                 order by sequenceNo, playlist_id",
            )?;
            stmt.query_map([parent], |r| r.get::<_, i64>(0))?
                .map(|r| r.map(|v| v as u32))
                .collect::<rusqlite::Result<_>>()?
        };
        for k in kids {
            out.push(k);
            self.collect_child_lists(k, out)?;
        }
        Ok(())
    }

    /// `renumber_sequence` with offset 0: the children of `parent` in `sequenceNo` order get
    /// 0, 1, 2, … (rekordbox numbers them in a temp table and updates every child).
    fn renumber_children(&self, parent: u32) -> Result<()> {
        self.in_transaction(|| {
            let kids: Vec<u32> = {
                let mut stmt = self.conn.prepare(&format!(
                    "select playlist_id from playlist where {} order by sequenceNo, playlist_id",
                    Self::parent_clause("playlist_id_parent", parent)
                ))?;
                stmt.query_map([], |r| r.get::<_, i64>(0))?
                    .map(|r| r.map(|v| v as u32))
                    .collect::<rusqlite::Result<_>>()?
            };
            for (i, id) in kids.iter().enumerate() {
                self.exec_cached(
                    "update playlist set sequenceNo = ?1 where playlist_id = ?2",
                    params![i as i64, id],
                )?;
            }
            Ok(())
        })
    }

    /// `DevSqliteDBController::removeTrack` @0x100718AEC (`docs/re/export.md` § 4.3), in one
    /// transaction: the `content` rows, `numberOfContents`, the tracks' cues (and their hot cue
    /// bank list links), playlist / history / My Tag links (not renumbered), the tracks' image
    /// rows (unconditionally), "like" relations; then `delete_unused_record` for artist, album,
    /// genre, label and key.
    pub fn remove_tracks(&self, content_ids: &[u32]) -> Result<()> {
        if content_ids.is_empty() {
            return Ok(());
        }
        let ids = join_ids(content_ids);
        self.in_transaction(|| {
            let images: Vec<u32> = {
                let mut stmt = self.conn.prepare(&format!(
                    "select image_id from content where content_id in ({ids}) and image_id > 0"
                ))?;
                stmt.query_map([], |r| r.get::<_, i64>(0))?
                    .map(|r| r.map(|v| v as u32))
                    .collect::<rusqlite::Result<_>>()?
            };
            self.conn
                .execute(&format!("delete from content where content_id in ({ids})"), [])?;
            self.caches.borrow_mut().content_count = None;
            self.update_number_of_contents()?;
            let cues: Vec<u32> = {
                let mut stmt = self.conn.prepare(&format!(
                    "select cue_id from cue where content_id in ({ids})"
                ))?;
                stmt.query_map([], |r| r.get::<_, i64>(0))?
                    .map(|r| r.map(|v| v as u32))
                    .collect::<rusqlite::Result<_>>()?
            };
            if !cues.is_empty() {
                let cue_ids = join_ids(&cues);
                self.conn.execute(
                    &format!("delete from hotCueBankList_cue where cue_id in ({cue_ids})"),
                    [],
                )?;
                self.conn
                    .execute(&format!("delete from cue where cue_id in ({cue_ids})"), [])?;
            }
            for table in ["playlist_content", "history_content", "myTag_content"] {
                self.conn.execute(
                    &format!("delete from {table} where content_id in ({ids})"),
                    [],
                )?;
            }
            self.caches.borrow_mut().entry_counts.clear();
            if !images.is_empty() {
                self.conn.execute(
                    &format!("delete from image where image_id in ({})", join_ids(&images)),
                    [],
                )?;
            }
            self.conn.execute(
                &format!(
                    "delete from recommendedLike where content_id_1 in ({ids}) or content_id_2 in ({ids})"
                ),
                [],
            )?;
            Ok(())
        })?;
        self.delete_unused(Lookup::Artist)?;
        self.delete_unused_albums()?;
        self.delete_unused(Lookup::Genre)?;
        self.delete_unused(Lookup::Label)?;
        self.delete_unused(Lookup::Key)
    }

    /// `delete_unused_record` @0x100719890 for albums: rows no `content.album_id` > 0 references.
    pub fn delete_unused_albums(&self) -> Result<()> {
        let deleted = self.guarded(|| {
            self.exec_cached(
                "delete from album where album_id not in (select album_id from content where album_id > 0)",
                [],
            )
        })?;
        if deleted > 0 {
            self.caches.borrow_mut().albums = Slot::Unloaded;
        }
        Ok(())
    }

    // ---- My Tag ---------------------------------------------------------------------------

    /// `writeMyTag` @0x1007121E4: set `property.myTagMasterDBID` (inserting the property row
    /// with `dbVersion` "10000" if there is none), delete every `myTag` row and insert `rows`
    /// in one statement. `myTag_content` is untouched; `syncMyTagList` clears it separately
    /// ([`DevLibWriter::clear_my_tag_content`]) on a full rewrite.
    pub fn write_my_tags(&self, master_db_id: u32, rows: &[MyTagRow]) -> Result<()> {
        let has_property: i64 = self
            .conn
            .query_row("select count(*) from property", [], |r| r.get(0))?;
        if has_property == 0 {
            self.conn.execute(
                "insert into property(deviceName, dbVersion, numberOfContents, createdDate, \
                 backGroundColorType, myTagMasterDBID) values ('', ?1, 0, ?2, 0, ?3)",
                params![DB_VERSION_INIT, self.today, master_db_id],
            )?;
        } else {
            self.conn
                .execute("update property set myTagMasterDBID = ?1", [master_db_id])?;
        }
        self.conn.execute("delete from myTag", [])?;
        self.insert_multiple(
            "myTag",
            &[
                "myTag_id",
                "sequenceNo",
                "name",
                "attribute",
                "myTag_id_parent",
            ],
            &rows
                .iter()
                .map(|t| {
                    vec![
                        t.id.into(),
                        t.seq.into(),
                        t.name.clone().into(),
                        list_attribute_code(t.attribute).into(),
                        t.parent.into(),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    }

    /// `clearMyTagTrack` @0x100711FE0: `delete from myTag_content`.
    pub fn clear_my_tag_content(&self) -> Result<()> {
        self.conn.execute("delete from myTag_content", [])?;
        Ok(())
    }

    /// `insertMyTagOnTrack` @0x10070EC20: one `(myTag_id, content_id)` row per tag.
    pub fn add_my_tags_to_track(&self, content_id: u32, tag_ids: &[u32]) -> Result<()> {
        self.insert_multiple(
            "myTag_content",
            &["myTag_id", "content_id"],
            &tag_ids
                .iter()
                .map(|t| vec![(*t).into(), content_id.into()])
                .collect::<Vec<_>>(),
        )
    }

    // ---- histories ------------------------------------------------------------------------

    /// `addNewHistory` @0x10071313C: `history_id = max + 1`, `sequenceNo = max(sequenceNo of
    /// the siblings) + 1` (`get_next_seq` @0x10070E954, so the first child gets **1**).
    pub fn add_history(&self, name: &str, attribute: u8, parent: u32) -> Result<u32> {
        let id = self.next_id("history", "history_id")?;
        let max: Option<i64> = self.conn.query_row(
            "select max(sequenceNo) as maxId from history where history_id_parent = ?1",
            [parent.to_string()],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "insert into history(history_id, sequenceNo, name, attribute, history_id_parent) \
             values (?1, ?2, ?3, ?4, ?5)",
            params![id, max.unwrap_or(0) + 1, name, attribute, parent],
        )?;
        Ok(id as u32)
    }

    /// `insertTrackToHistory` @0x100713578: entries continue after the highest `sequenceNo`
    /// of the history (1 for an empty one).
    pub fn add_tracks_to_history(&self, history_id: u32, content_ids: &[u32]) -> Result<()> {
        let last: Option<Option<i64>> = self
            .conn
            .query_row(
                "select sequenceNo from history_content where history_id = ?1 \
                 order by sequenceNo desc",
                [history_id],
                |r| r.get(0),
            )
            .optional()?;
        let first = last.map_or(1, |s| s.unwrap_or(0) + 1);
        for (i, content_id) in content_ids.iter().enumerate() {
            self.exec_cached(
                "insert into history_content(history_id, content_id, sequenceNo) values (?1, ?2, ?3)",
                params![history_id, content_id, first + i as i64],
            )?;
        }
        Ok(())
    }

    // ---- cues -----------------------------------------------------------------------------

    /// `setHotCue` @0x10071D6C4 (insert: `cue_id = max + 1`, content, kind and the position
    /// columns) followed by `addCueOption2` @0x100716058 (update of colour, comment, beat loop,
    /// microsecond positions and block seek columns). `isActiveLoop` is only written for
    /// `active_loop: Some(_)` (`enableActiveLoop`).
    pub fn add_cue(&self, cue: &NewCue) -> Result<u32> {
        let id = self.next_id("cue", "cue_id")?;
        self.exec_cached(
            "insert into cue(cue_id, content_id, kind, inUsec, outUsec, in150FramePerSec, \
             out150FramePerSec, inMpegFrameNumber, outMpegFrameNumber, inMpegAbs, outMpegAbs) \
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                cue.content_id,
                cue.kind,
                cue.in_usec,
                cue.out_usec,
                cue.in_150,
                cue.out_150,
                cue.in_mpeg_frame,
                cue.out_mpeg_frame,
                cue.in_mpeg_abs,
                cue.out_mpeg_abs,
            ],
        )?;
        self.exec_cached(
            "update cue set kind = ?1, colorTableIndex = ?2, cueComment = ?3, inUsec = ?4, \
             outUsec = ?5, in150FramePerSec = ?6, out150FramePerSec = ?7, \
             inMpegFrameNumber = ?8, outMpegFrameNumber = ?9, inMpegAbs = ?10, outMpegAbs = ?11, \
             beatLoopNumerator = ?12, beatLoopDenominator = ?13, \
             inDecodingStartFramePosition = ?14, outDecodingStartFramePosition = ?15, \
             inFileOffsetInBlock = ?16, OutFileOffsetInBlock = ?17, \
             inNumberOfSampleInBlock = ?18, outNumberOfSampleInBlock = ?19 where cue_id = ?20",
            params![
                cue.kind,
                cue.color_table_index,
                cue.comment,
                cue.in_usec,
                cue.out_usec,
                cue.in_150,
                cue.out_150,
                cue.in_mpeg_frame,
                cue.out_mpeg_frame,
                cue.in_mpeg_abs,
                cue.out_mpeg_abs,
                cue.beat_loop_numerator,
                cue.beat_loop_denominator,
                cue.in_decoding_start,
                cue.out_decoding_start,
                cue.in_file_offset,
                cue.out_file_offset,
                cue.in_samples,
                cue.out_samples,
                id,
            ],
        )?;
        if let Some(active) = cue.active_loop {
            self.exec_cached(
                "update cue set isActiveLoop = ?1 where cue_id = ?2",
                params![i64::from(active), id],
            )?;
        }
        Ok(id as u32)
    }

    // ---- hot cue bank lists ---------------------------------------------------------------

    /// `createNewList` for hot cue bank lists (list type 4): like
    /// [`DevLibWriter::create_playlist`] with `addHotCueBanklist` @0x100721F08 as the insert
    /// (`hotCueBankList_id, sequenceNo, name, attribute, hotCueBankList_id_parent`).
    pub fn create_hot_cue_bank_list(
        &self,
        name: &str,
        parent: u32,
        folder: bool,
        position: Option<i64>,
    ) -> Result<u32> {
        let where_parent = Self::parent_clause("hotCueBankList_id_parent", parent);
        let count: i64 = self.conn.query_row(
            &format!("select count(hotCueBankList_id) from hotCueBankList where {where_parent}"),
            [],
            |r| r.get(0),
        )?;
        let seq = if count == 0 {
            0
        } else {
            let max: Option<i64> = self.conn.query_row(
                &format!("select max(sequenceNo) from hotCueBankList where {where_parent}"),
                [],
                |r| r.get(0),
            )?;
            max.unwrap_or(0) + 1
        };
        let id = self.next_id("hotCueBankList", "hotCueBankList_id")?;
        self.conn.execute(
            "insert into hotCueBankList(hotCueBankList_id, sequenceNo, name, attribute, \
             hotCueBankList_id_parent) values (?1, ?2, ?3, ?4, ?5)",
            params![id, seq, name, i64::from(folder), parent],
        )?;
        if let Some(position) = position
            && position <= seq
        {
            self.move_sequence(
                "hotCueBankList",
                "hotCueBankList_id",
                "hotCueBankList_id_parent",
                id as u32,
                parent,
                position,
            )?;
        }
        Ok(id as u32)
    }

    /// `setHotCueBankCue` @0x10071DC08: the cue row ([`DevLibWriter::add_cue`]) and the link
    /// row `(hotCueBankList_id, cue_id, sequenceNo)`, in one transaction.
    pub fn add_hot_cue_bank_cue(&self, list_id: u32, seq: i64, cue: &NewCue) -> Result<u32> {
        let in_tx = !self.conn.is_autocommit();
        if !in_tx {
            self.conn.execute_batch("BEGIN")?;
        }
        let result = (|| {
            let cue_id = self.add_cue(cue)?;
            self.exec_cached(
                "insert into hotCueBankList_cue(hotCueBankList_id, cue_id, sequenceNo) \
                 values (?1, ?2, ?3)",
                params![list_id, cue_id, seq],
            )?;
            Ok::<_, Error>(cue_id)
        })();
        if !in_tx {
            self.conn
                .execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        }
        result
    }

    // ---- relations ------------------------------------------------------------------------

    /// `addTracksRelation` @0x10070D17C: `(content_id_1, content_id_2, createdDate)` with the
    /// time in ms since the Unix epoch; `rating` is not written (NULL).
    pub fn add_recommended_like(
        &self,
        content_id_1: u32,
        content_id_2: u32,
        created_ms: i64,
    ) -> Result<()> {
        self.exec_cached(
            "insert into recommendedLike(content_id_1, content_id_2, createdDate) values (?1, ?2, ?3)",
            params![content_id_1, content_id_2, created_ms],
        )?;
        Ok(())
    }
}

/// `calc_next_device_image_path` @0x10070A018.
/// IDs as rekordbox inlines them into `… in (…)`: joined with ", ".
fn join_ids(ids: &[u32]) -> String {
    ids.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn device_image_path(image_id: u32, hidden_pioneer: bool) -> String {
    let root = if hidden_pioneer {
        "/.PIONEER"
    } else {
        "/PIONEER"
    };
    format!("{root}/Artwork/{:05}/b{image_id}.jpg", image_id / 20 + 1)
}

fn list_attribute_code(attribute: ListAttribute) -> i64 {
    match attribute {
        ListAttribute::List => 0,
        ListAttribute::Folder => 1,
        ListAttribute::Other(code) => code,
    }
}

/// Plain `content` columns the `modify*` functions write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentColumn {
    Title,
    Subtitle,
    BpmX100,
    Length,
    TrackNo,
    DiscNo,
    ColorId,
    Comment,
    Rating,
    ReleaseYear,
    ReleaseDate,
    FileType,
    Bitrate,
    BitDepth,
    SampleRate,
    PlayCount,
    HotCueAutoLoad,
    KuvoDeliver,
    KuvoComment,
    HasModified,
    AnalysedBits,
    InformationUpdateCount,
    AnalysisUpdateCount,
    CueUpdateCount,
}

impl ContentColumn {
    pub fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Subtitle => "subtitle",
            Self::BpmX100 => "bpmx100",
            Self::Length => "length",
            Self::TrackNo => "trackNo",
            Self::DiscNo => "discNo",
            Self::ColorId => "color_id",
            Self::Comment => "djComment",
            Self::Rating => "rating",
            Self::ReleaseYear => "releaseYear",
            Self::ReleaseDate => "releaseDate",
            Self::FileType => "fileType",
            Self::Bitrate => "bitrate",
            Self::BitDepth => "bitDepth",
            Self::SampleRate => "samplingRate",
            Self::PlayCount => "djPlayCount",
            Self::HotCueAutoLoad => "isHotCueAutoLoadOn",
            Self::KuvoDeliver => "isKuvoDeliverStatusOn",
            Self::KuvoComment => "kuvoDeliveryComment",
            Self::HasModified => "hasModified",
            Self::AnalysedBits => "analysedBits",
            Self::InformationUpdateCount => "informationUpdateCount",
            Self::AnalysisUpdateCount => "analysisDataUpdateCount",
            Self::CueUpdateCount => "cueUpdateCount",
        }
    }
}

/// `content` columns that hold a lookup ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupColumn {
    Artist,
    Remixer,
    OriginalArtist,
    Composer,
    Lyricist,
    Genre,
    Label,
    Key,
}

impl LookupColumn {
    pub fn name(self) -> &'static str {
        match self {
            Self::Artist => "artist_id_artist",
            Self::Remixer => "artist_id_remixer",
            Self::OriginalArtist => "artist_id_originalArtist",
            Self::Composer => "artist_id_composer",
            Self::Lyricist => "artist_id_lyricist",
            Self::Genre => "genre_id",
            Self::Label => "label_id",
            Self::Key => "key_id",
        }
    }

    fn lookup(self) -> Lookup {
        match self {
            Self::Genre => Lookup::Genre,
            Self::Label => Lookup::Label,
            Self::Key => Lookup::Key,
            _ => Lookup::Artist,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_paths() {
        assert_eq!(device_image_path(1, false), "/PIONEER/Artwork/00001/b1.jpg");
        assert_eq!(
            device_image_path(19, false),
            "/PIONEER/Artwork/00001/b19.jpg"
        );
        assert_eq!(
            device_image_path(20, false),
            "/PIONEER/Artwork/00002/b20.jpg"
        );
        assert_eq!(
            device_image_path(95, true),
            "/.PIONEER/Artwork/00005/b95.jpg"
        );
    }

    #[test]
    fn content_link_rules() {
        // Fixture (oracle-a): master 1902094 → inserted 788224, re-exported 1902336.
        assert_eq!(export_content_link(1_902_094), 788_224);
        assert_eq!(
            reexport_content_link(788_224, 1_902_094, true),
            Some(1_902_336)
        );
        assert_eq!(reexport_content_link(1_902_336, 1_902_094, true), None);
        assert_eq!(export_analysed_bits(105), 41);
    }
}
