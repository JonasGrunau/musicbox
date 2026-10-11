//! USB export engine: rekordbox 7.2.19's export of playlists and tracks to a stick, reproduced
//! file for file. Spec: `docs/re/export.md` (algorithm, paths, IDs, artwork, sync, settings),
//! `docs/re/ops/export-usb.md`, `docs/re/ops/export-devlib.md`, `docs/re/pdb.md` § Writer,
//! `docs/re/devlib.md` § Writer, `docs/re/anlz.md` § Local vs USB.
//!
//! The export runs in three steps:
//! 1. **Snapshots.** [`SourceLibrary::load`] reads what the export needs from master.db (and
//!    checks the source audio, ANLZ and artwork files); [`DeviceState::load`] reads what is
//!    already on the stick (both databases, the `/Contents`, `USBANLZ` and `Artwork` listings).
//! 2. **Plan** ([`plan_export`], pure): replays rekordbox's export against an in-memory model
//!    of both device databases and decides every path, ID, row and file ([`ExportPlan`]).
//! 3. **Execute** ([`execute`]): copies the audio, writes the ANLZ sets, artwork and settings
//!    files, then the databases (exportLibrary.db, export.pdb, exportExt.pdb) last, with
//!    `F_FULLFSYNC`; writes the export log for tracks that were not exported. Reports progress
//!    and stops cleanly on cancellation.
//!
//! The three database formats are written by [`mb_pdb`], [`mb_devlib`] and [`mb_anlz`].

pub mod device;
pub mod exec;
pub mod log;
pub mod naming;
pub mod plan;
pub mod settings;
pub mod source;

pub use device::DeviceState;
pub use exec::{CancelToken, ExportOptions, ExportReport, Phase, Progress, execute};
pub use log::LogStrings;
pub use plan::{
    DevlibChange, DevlibOp, ExportPlan, ExtOp, FileOp, Job, PdbOp, PlanOptions, TrackResult,
    TrackStep, plan_export,
};
pub use source::{SelectionItem, SourceLibrary};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("master.db: {0}")]
    Master(#[from] mb_masterdb::Error),
    #[error("export.pdb: {0}")]
    Pdb(#[from] mb_pdb::Error),
    #[error("exportLibrary.db: {0}")]
    Devlib(#[from] mb_devlib::Error),
    #[error("ANLZ {path}: {source}")]
    Anlz {
        path: PathBuf,
        source: mb_anlz::Error,
    },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("artwork {path}: {message}")]
    Artwork { path: PathBuf, message: String },
    #[error("selection: {0}")]
    Selection(String),
    #[error("plan: {0}")]
    Plan(String),
    #[error("device: {0}")]
    Device(String),
    #[error("exportLibrary.db: writer returned {what} {got}, the plan expected {expected}")]
    IdMismatch {
        what: &'static str,
        got: u32,
        expected: u32,
    },
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
