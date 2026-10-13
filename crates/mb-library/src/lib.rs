//! musicbox's command layer: one module per rekordbox library operation, each replaying the
//! statement pattern of its spec in `docs/re/ops/` through mb-masterdb's [`WriteTxn`] (same tables,
//! columns, values, USN bump count and order). Every command returns its inverse, so undo is a
//! forward command with new USNs, never a rollback of committed data.
//!
//! | Module | Op | Spec | Status |
//! |---|---|---|---|
//! | [`import`] | import files | `import-track.md` | traced (duplicate check: hypothesis) |
//! | [`remove`] | remove tracks from the collection | `remove-tracks.md` | traced (playlist renumbering: hypothesis) |
//! | [`playlist`] | create, rename | `playlist-create.md` | traced |
//! | [`playlist`] | add tracks (append) | `playlist-add-tracks.md` | traced |
//! | [`playlist`] | folders, delete, move, remove/reorder entries, insert at a position | `playlist-edit.md` | hypothesis |
//! | [`playlist`] | smart lists: create, set rules | `smartlist.md` | binary-derived (no trace) |
//! | [`edit`] | track metadata edits | `edit-track.md` | hypothesis (built from traced macros) |
//! | [`xml_sync`] | `masterPlaylists6.xml` after playlist changes | `playlist-create.md` §5 | traced format, hypothesis for folders |
//! | [`cues`] | add, delete, edit memory/hot cues and loops | `edit-cues.md` | binary-derived (no trace) |
//! | [`grid`] | replace a beat grid (shift, downbeat, tempo, ×2, ÷2) | `edit-grid.md` | binary-derived (no trace) |
//! | [`device_import`] | import tracks / a playlist from a device | `import-device.md` | binary-derived (no trace) |
//! | [`device_history`] | import player histories from a device | `import-device.md` § Histories | binary-derived (no trace) |
//! | [`xml`] | rekordbox.xml export; import tracks / lists from it | `../xml.md`, `import-xml.md` | export verified byte-exact; import binary-derived (no trace) |
//! | [`analyse`] | analyse tracks (musicbox's own analysis) | `analyse-track.md` | traced statements; files verified tag by tag against rekordbox |
//!
//! [`WriteTxn`]: mb_masterdb::WriteTxn

pub mod analyse;
pub mod analysis_lock;
pub mod cues;
pub mod device_history;
pub mod device_import;
pub mod edit;
pub mod grid;
pub mod import;
mod lookup;
pub mod playlist;
pub mod remove;
pub mod tags;
pub mod xml;
pub mod xml_sync;

pub use analyse::{AnalysisSettings, AnalysisSnapshot, TrackAnalysis, analyse_track};
pub use cues::{Cue, CueEdit, CueSlot, NewCue};
pub use device_history::{DeviceHistory, HistoryImportReport, ImportedHistory, LocalTime};
pub use device_import::{DeviceImport, DeviceImportKind, DeviceImportReport, DeviceImportResult, DeviceTrack};
pub use edit::TrackEdit;
pub use mb_anlz::tags::Beat;
pub use import::{ImportReport, SkipReason};
pub use mb_core::DbId;
pub use playlist::{EntrySnapshot, NodeKind, PlaylistSnapshot};
pub use xml::{XmlExportOptions, XmlImport, XmlImportReport, XmlList, XmlTrackResult};

use mb_masterdb::MasterDbWriter;
use mb_masterdb::rusqlite::Connection;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    MasterDb(#[from] mb_masterdb::Error),
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("reading tags of {path}: {message}")]
    Tags { path: String, message: String },
    #[error("no live {what} with ID {id}")]
    NotFound { what: &'static str, id: String },
    #[error("{0}")]
    Invalid(String),
    #[error("smart playlist {0} is read-only")]
    SmartPlaylistReadOnly(String),
}

impl From<mb_masterdb::rusqlite::Error> for Error {
    fn from(e: mb_masterdb::rusqlite::Error) -> Self {
        Error::MasterDb(e.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One library operation. [`Library::execute`] runs it; its [`Outcome::inverse`] undoes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Import audio files into the collection (`import-track.md`).
    ImportFiles { paths: Vec<PathBuf> },
    /// Remove tracks from the collection (`remove-tracks.md`). Not undoable.
    RemoveTracks { ids: Vec<DbId> },
    /// Create a playlist or folder under `parent` (`None` = root); it becomes the first child.
    CreatePlaylist { parent: Option<DbId>, name: String, kind: NodeKind },
    RenamePlaylist { id: DbId, name: String },
    /// Move a node to `position` (1-based `Seq`) under `parent`.
    MovePlaylist { id: DbId, parent: Option<DbId>, position: u32 },
    /// Delete a playlist, or a folder with everything below it.
    DeletePlaylist { id: DbId },
    /// Re-create a deleted subtree (inverse of [`Command::DeletePlaylist`]).
    RestorePlaylist { snapshot: PlaylistSnapshot },
    /// Create a smart list under `parent` with these rules (`docs/re/smartlist.md`).
    CreateSmartList {
        parent: Option<DbId>,
        name: String,
        /// [`mb_core::smartlist::MATCH_ALL`] or [`mb_core::smartlist::MATCH_ANY`].
        logical_operator: i32,
        conditions: Vec<mb_core::smartlist::Condition>,
    },
    /// Replace a smart list's rule XML (`None` = NULL).
    SetSmartList { id: DbId, xml: Option<String> },
    /// Add tracks to a playlist, appended (`position = None`) or inserted before `position`.
    AddTracks { playlist: DbId, content_ids: Vec<DbId>, position: Option<u32> },
    /// Remove entries (`djmdSongPlaylist.ID`s) from a playlist.
    RemoveFromPlaylist { playlist: DbId, entry_ids: Vec<String> },
    /// Put removed entries back at their old positions (inverse of [`Command::RemoveFromPlaylist`]).
    RestoreEntries { playlist: DbId, entries: Vec<EntrySnapshot> },
    /// Reorder a playlist to exactly this entry order (a permutation of its entry IDs).
    ReorderPlaylist { playlist: DbId, order: Vec<String> },
    /// Change one metadata field of a track.
    EditTrack { id: DbId, edit: TrackEdit },
    /// Add a memory cue, hot cue or loop to a track (`edit-cues.md`).
    AddCue { content: DbId, cue: NewCue },
    /// Delete one cue (`djmdCue.ID`).
    DeleteCue { id: String },
    /// Put a deleted cue row back unchanged (inverse of [`Command::DeleteCue`]).
    RestoreCue { row: Vec<(String, mb_masterdb::rusqlite::types::Value)> },
    /// Change the colour, colour code, comment or active-loop flag of one cue.
    EditCue { id: String, edit: CueEdit },
    /// Replace a track's beat grid (`edit-grid.md`); build `beats` with the [`grid`] functions.
    SetBeatGrid { content: DbId, beats: Vec<Beat> },
    /// Import tracks (and optionally a playlist of them) from a device (`import-device.md`).
    ImportFromDevice { import: DeviceImport },
    /// Inverse of [`Command::ImportFromDevice`]: the playlist, the new rows, the created files.
    UndoDeviceImport { ids: Vec<DbId>, playlist: Option<DbId>, files: Vec<PathBuf> },
    /// Import device histories not yet listed in `already` (`import-device.md` § Histories).
    ImportHistories { histories: Vec<DeviceHistory>, already: ImportedHistory, now: LocalTime },
    /// Inverse of [`Command::ImportHistories`].
    UndoHistoryImport { histories: Vec<DbId>, play_counts: Vec<(DbId, i64)> },
    /// Put the previous grid sections and BPM back (inverse of [`Command::SetBeatGrid`]).
    RestoreBeatGrid { content: DbId, pqtz: mb_anlz::Section, pqt2: Option<mb_anlz::Section>, bpm_x100: Option<i64> },
    /// Import tracks and lists from a rekordbox.xml (`import-xml.md`).
    ImportXml { import: XmlImport },
    /// Several commands in order, as one undo step; the inverse runs their inverses backwards.
    Batch { commands: Vec<Command> },
    /// Write the result of [`analyse_track`] for a track (`analyse-track.md`). Refused for a track
    /// whose analysis is locked.
    ApplyAnalysis { content: DbId, analysis: Box<TrackAnalysis> },
    /// Inverse of [`Command::ApplyAnalysis`]: the previous files, columns and rows.
    RestoreAnalysis { snapshot: Box<AnalysisSnapshot> },
    /// "Analysis Lock On/Off" (`analysis_lock`): `Analysed` bit 0x80 on the tracks whose state
    /// differs. Locked tracks are not analysed and their beat grid is not edited.
    SetAnalysisLock { ids: Vec<DbId>, locked: bool },
}

/// What a command did.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Imported(ImportReport),
    Removed { ids: Vec<DbId> },
    PlaylistCreated { id: DbId },
    /// Playlist IDs of a restored subtree (pre-order; the root first).
    PlaylistsRestored { ids: Vec<DbId> },
    /// New `djmdSongPlaylist.ID`s of added or restored entries.
    EntriesAdded { entry_ids: Vec<String> },
    ImportedFromDevice(DeviceImportReport),
    HistoriesImported(HistoryImportReport),
    /// `djmdCue.ID` of an added or restored cue.
    CueAdded { id: String },
    ImportedXml(XmlImportReport),
    /// A track's analysis was written ([`Command::ApplyAnalysis`]).
    Analysed { content: DbId },
    Done,
    /// Nothing to do (e.g. an edit to the current value); no statement was written.
    Unchanged,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub effect: Effect,
    /// The command that undoes this one, if the op is undoable.
    pub inverse: Option<Command>,
    /// `localUpdateCount` before and after: the number of USN bumps the op spent.
    pub usn_before: i64,
    pub usn_after: i64,
}

impl Outcome {
    pub fn usn_spent(&self) -> i64 {
        self.usn_after - self.usn_before
    }
}

/// A writable library: master.db plus `masterPlaylists6.xml` next to it.
pub struct Library {
    pub(crate) writer: MasterDbWriter,
    pub(crate) xml_path: PathBuf,
    /// The analysis root (`analysis-data-root-path` of options.json); needed by grid edits.
    pub(crate) analysis_root: Option<PathBuf>,
    /// Session backup folder; analysis files are copied there before their first change.
    pub(crate) anlz_backup: Option<PathBuf>,
}

impl Library {
    /// Opens for writing; refuses while rekordbox or rekordboxAgent runs.
    pub fn open(db_path: &Path, passphrase: &str) -> Result<Self> {
        Ok(Self::with_writer(MasterDbWriter::open(db_path, passphrase)?, db_path))
    }

    /// Opens without the process check. Only for fixture copies in tests and tools.
    pub fn open_unguarded(db_path: &Path, passphrase: &str) -> Result<Self> {
        Ok(Self::with_writer(MasterDbWriter::open_unguarded(db_path, passphrase)?, db_path))
    }

    fn with_writer(writer: MasterDbWriter, db_path: &Path) -> Self {
        Self { writer, xml_path: mb_masterdb::playlists_xml::path_for_db(db_path), analysis_root: None, anlz_backup: None }
    }

    /// Where `AnalysisDataPath` (`/PIONEER/USBANLZ/…`) resolves; grid edits fail without it.
    pub fn set_analysis_root(&mut self, root: impl Into<PathBuf>) {
        self.analysis_root = Some(root.into());
    }

    /// The session backup folder (from `backup_library`); analysis files touched by this session
    /// are copied below `<dir>/anlz/` first.
    pub fn set_anlz_backup(&mut self, dir: impl Into<PathBuf>) {
        self.anlz_backup = Some(dir.into());
    }

    pub fn connection(&self) -> &Connection {
        self.writer.connection()
    }

    pub fn xml_path(&self) -> &Path {
        &self.xml_path
    }

    /// Current `localUpdateCount`.
    pub fn usn(&self) -> Result<i64> {
        Ok(self
            .connection()
            .query_row("SELECT int_1 FROM agentRegistry WHERE registry_id = 'localUpdateCount'", [], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .unwrap_or(0))
    }

    pub fn execute(&mut self, command: Command) -> Result<Outcome> {
        let usn_before = self.usn()?;
        let (effect, inverse) = match command {
            Command::ImportFiles { paths } => import::import_files(self, &paths)?,
            Command::RemoveTracks { ids } => remove::remove_tracks(self, &ids)?,
            Command::CreatePlaylist { parent, name, kind } => playlist::create(self, parent.as_ref(), &name, kind)?,
            Command::RenamePlaylist { id, name } => playlist::rename(self, &id, &name)?,
            Command::MovePlaylist { id, parent, position } => playlist::move_node(self, &id, parent.as_ref(), position)?,
            Command::DeletePlaylist { id } => playlist::delete(self, &id)?,
            Command::RestorePlaylist { snapshot } => playlist::restore(self, &snapshot)?,
            Command::CreateSmartList { parent, name, logical_operator, conditions } => {
                playlist::create_smart(self, parent.as_ref(), &name, logical_operator, &conditions)?
            }
            Command::SetSmartList { id, xml } => playlist::set_smart_list(self, &id, xml.as_deref())?,
            Command::AddTracks { playlist, content_ids, position } => {
                playlist::add_tracks(self, &playlist, &content_ids, position)?
            }
            Command::RemoveFromPlaylist { playlist, entry_ids } => playlist::remove_entries(self, &playlist, &entry_ids)?,
            Command::RestoreEntries { playlist, entries } => playlist::restore_entries(self, &playlist, &entries)?,
            Command::ReorderPlaylist { playlist, order } => playlist::reorder(self, &playlist, &order)?,
            Command::EditTrack { id, edit } => edit::edit_track(self, &id, &edit)?,
            Command::AddCue { content, cue } => cues::add_cue(self, &content, &cue)?,
            Command::DeleteCue { id } => cues::delete_cue(self, &id)?,
            Command::RestoreCue { row } => cues::restore_cue(self, &row)?,
            Command::EditCue { id, edit } => cues::edit_cue(self, &id, &edit)?,
            Command::SetBeatGrid { content, beats } => grid::set_beat_grid(self, &content, &beats)?,
            Command::ImportFromDevice { import } => device_import::import(self, &import)?,
            Command::UndoDeviceImport { ids, playlist, files } => device_import::undo(self, &ids, playlist.as_ref(), &files)?,
            Command::ImportHistories { histories, already, now } => device_history::import(self, &histories, &already, &now)?,
            Command::UndoHistoryImport { histories, play_counts } => device_history::undo(self, &histories, &play_counts)?,
            Command::RestoreBeatGrid { content, pqtz, pqt2, bpm_x100 } => {
                grid::restore_beat_grid(self, &content, &pqtz, pqt2.as_ref(), bpm_x100)?
            }
            Command::ImportXml { import } => xml::import(self, &import)?,
            Command::ApplyAnalysis { content, analysis } => analyse::apply(self, &content, &analysis)?,
            Command::RestoreAnalysis { snapshot } => analyse::restore(self, &snapshot)?,
            Command::SetAnalysisLock { ids, locked } => analysis_lock::set_lock(self, &ids, locked)?,
            Command::Batch { commands } => {
                let mut inverses = Vec::new();
                for c in commands {
                    inverses.extend(self.execute(c)?.inverse);
                }
                inverses.reverse();
                (Effect::Done, (!inverses.is_empty()).then_some(Command::Batch { commands: inverses }))
            }
        };
        Ok(Outcome { effect, inverse, usn_before, usn_after: self.usn()? })
    }

    // Convenience wrappers.

    pub fn import_files(&mut self, paths: &[PathBuf]) -> Result<Outcome> {
        self.execute(Command::ImportFiles { paths: paths.to_vec() })
    }

    pub fn remove_tracks(&mut self, ids: &[DbId]) -> Result<Outcome> {
        self.execute(Command::RemoveTracks { ids: ids.to_vec() })
    }

    pub fn create_playlist(&mut self, parent: Option<&DbId>, name: &str, kind: NodeKind) -> Result<Outcome> {
        self.execute(Command::CreatePlaylist { parent: parent.cloned(), name: name.to_owned(), kind })
    }

    pub fn add_tracks(&mut self, playlist: &DbId, content_ids: &[DbId]) -> Result<Outcome> {
        self.execute(Command::AddTracks { playlist: playlist.clone(), content_ids: content_ids.to_vec(), position: None })
    }

    pub fn edit_track(&mut self, id: &DbId, edit: TrackEdit) -> Result<Outcome> {
        self.execute(Command::EditTrack { id: id.clone(), edit })
    }
}

/// Undo/redo stacks of inverse commands. Undo runs the inverse as a new forward command.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
}

impl History {
    /// Runs `command` and records its inverse. A new command clears the redo stack.
    pub fn run(&mut self, lib: &mut Library, command: Command) -> Result<Outcome> {
        let out = lib.execute(command)?;
        if let Some(inv) = &out.inverse {
            self.undo.push(inv.clone());
            self.redo.clear();
        }
        Ok(out)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Runs the last inverse; its own inverse goes onto the redo stack.
    pub fn undo(&mut self, lib: &mut Library) -> Result<Option<Outcome>> {
        let Some(cmd) = self.undo.pop() else { return Ok(None) };
        let out = lib.execute(cmd)?;
        if let Some(inv) = &out.inverse {
            self.redo.push(inv.clone());
        }
        Ok(Some(out))
    }

    pub fn redo(&mut self, lib: &mut Library) -> Result<Option<Outcome>> {
        let Some(cmd) = self.redo.pop() else { return Ok(None) };
        let out = lib.execute(cmd)?;
        if let Some(inv) = &out.inverse {
            self.undo.push(inv.clone());
        }
        Ok(Some(out))
    }
}

/// Milliseconds since the Unix epoch, for `masterPlaylists6.xml` timestamps.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
