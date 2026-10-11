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
//! | [`edit`] | track metadata edits | `edit-track.md` | hypothesis (built from traced macros) |
//! | [`xml_sync`] | `masterPlaylists6.xml` after playlist changes | `playlist-create.md` §5 | traced format, hypothesis for folders |
//!
//! [`WriteTxn`]: mb_masterdb::WriteTxn

pub mod edit;
pub mod import;
mod lookup;
pub mod playlist;
pub mod remove;
pub mod tags;
pub mod xml_sync;

pub use edit::TrackEdit;
pub use import::{ImportReport, SkipReason};
pub use mb_core::DbId;
pub use playlist::{EntrySnapshot, NodeKind, PlaylistSnapshot};

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
        Self { writer, xml_path: mb_masterdb::playlists_xml::path_for_db(db_path) }
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
            Command::AddTracks { playlist, content_ids, position } => {
                playlist::add_tracks(self, &playlist, &content_ids, position)?
            }
            Command::RemoveFromPlaylist { playlist, entry_ids } => playlist::remove_entries(self, &playlist, &entry_ids)?,
            Command::RestoreEntries { playlist, entries } => playlist::restore_entries(self, &playlist, &entries)?,
            Command::ReorderPlaylist { playlist, order } => playlist::reorder(self, &playlist, &order)?,
            Command::EditTrack { id, edit } => edit::edit_track(self, &id, &edit)?,
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
