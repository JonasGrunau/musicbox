//! rekordbox USB export databases `export.pdb` and `exportExt.pdb` (Encirq DeviceSQL).
//! Spec: `docs/re/pdb.md`.
//!
//! Layers, bottom up:
//! - [`page`]: the file header, the per-table page chains, data pages with their row-slot
//!   footers, and the per-table header ("index") pages. Every header field is exposed, including
//!   bookkeeping a reader does not need, and [`page::Pdb::as_bytes`] returns the input unchanged.
//! - [`string`]: DeviceSQL strings (short ASCII, long ASCII, UTF-16LE, the ISRC special case).
//! - [`schema`]: the column lists rekordbox 7.2.19 passes to `TETableCreate`, and the physical row
//!   layout the runtime derives from them (`TE_ComputeRealPositions` @0x102f747b0).
//! - [`row`]: generic, schema-driven row decoding.
//! - [`tables`]: typed rows for every table of both files.
//! - [`library`]: [`DeviceLibrary`], which loads both files and converts them to
//!   [`mb_core::LibrarySnapshot`].
//!
//! Writing (spec: `docs/re/pdb.md` § Storage engine, § Writer):
//! - [`encode`]: the row formatter (`TE_RowFormSmgr`), inverse of [`row`].
//! - [`engine`]: a model of the DeviceSQL storage layer (sessions, transaction ids, page
//!   allocation, insert/delete/update with the reuse list and dirty bits).
//! - [`writer`]: typed rows → files: compact layout, rekordbox's export sequence, and the
//!   step-by-step [`writer::Exporter`] (also for incremental updates of an existing stick).
//! - [`defaults`]: the rows of a new stick. [`lint`]: structural checks (`pdb-lint`).
//!   [`compare`]: page-by-page diff. [`replay`]: the oracle stick's export history, replayed
//!   byte for byte.

pub mod compare;
pub mod defaults;
pub mod encode;
pub mod engine;
pub mod library;
pub mod lint;
pub mod page;
pub mod replay;
pub mod row;
pub mod schema;
pub mod string;
pub mod tables;
pub mod writer;

pub use library::{DeviceHistory, DeviceLibrary, ExportDb, ExtDb};
pub use page::{DataPage, FileHeader, HeaderPage, PageHeader, Pdb, Slot, TableDirEntry};
pub use row::{Row, Value};
pub use schema::{Column, ColumnType, DbKind, TableSchema};
pub use string::DeviceString;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("file is {len} bytes, too short for a {need}-byte structure at offset {offset}")]
    Truncated {
        offset: usize,
        need: usize,
        len: usize,
    },
    #[error("unsupported page size {0}")]
    BadPageSize(u32),
    #[error("table type {table}: page chain is broken at page {page}: {reason}")]
    BadChain {
        table: u32,
        page: u32,
        reason: &'static str,
    },
    #[error("page {page}: {reason}")]
    BadPage { page: u32, reason: String },
    #[error("page {page} slot {slot}: {reason}")]
    BadRow {
        page: u32,
        slot: u16,
        reason: String,
    },
    #[error("bad DeviceSQL string at row offset {offset}: {reason}")]
    BadString { offset: usize, reason: &'static str },
    #[error("table type {0} is not part of the schema")]
    UnknownTable(u32),
    #[error("{table}: cannot encode row: {reason}")]
    BadValue { table: &'static str, reason: String },
    #[error("writer: {0}")]
    Write(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn le_u16(data: &[u8], offset: usize) -> Result<u16> {
    data.get(offset..offset + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(Error::Truncated {
            offset,
            need: 2,
            len: data.len(),
        })
}

pub(crate) fn le_u32(data: &[u8], offset: usize) -> Result<u32> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Error::Truncated {
            offset,
            need: 4,
            len: data.len(),
        })
}
