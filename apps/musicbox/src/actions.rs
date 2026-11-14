//! User intents produced by the tree and the track table, applied by the app.

use mb_core::DbId;
use mb_library::{Command, NodeKind};

/// Payload while local collection tracks are dragged from the table.
#[derive(Clone)]
pub struct TrackDrag(pub Vec<DbId>);

/// Payload while a local playlist, intelligent playlist or folder is dragged from the tree.
#[derive(Clone)]
pub struct PlaylistDrag {
    pub id: DbId,
    pub name: String,
}

/// What an export writes to a device.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportItem {
    /// A local playlist, intelligent playlist or folder, with everything in it.
    Playlist(DbId),
    /// Collection tracks, written to the device without a playlist (rekordbox's `export_track`).
    Tracks(Vec<DbId>),
}

pub enum Action {
    Run(Command),
    Undo,
    Redo,
    /// Opens the name dialog for a new playlist/folder under `parent`.
    AskCreate { parent: Option<DbId>, kind: NodeKind },
    AskRename { id: DbId, name: String },
    /// Opens the intelligent playlist editor: a new list under `parent`, or the list `id`.
    AskSmartList { parent: Option<DbId>, id: Option<DbId> },
    AskRemoveFromCollection(Vec<DbId>),
    /// Export local lists or tracks to the device at this index of `Devices::list`.
    Export { item: ExportItem, device: usize },
    CancelExport,
    /// Confirm, then remove lists (name paths from the top level) and/or tracks (absolute
    /// paths) from the device at this index of `Devices::list`.
    AskRemoveFromDevice { device: usize, playlists: Vec<Vec<String>>, tracks: Vec<std::path::PathBuf> },
    /// Import a device list (name path from the top level, as a playlist) or single tracks
    /// (absolute paths on the device) into the collection.
    ImportFromDevice { device: usize, playlist: Option<Vec<String>>, tracks: Vec<std::path::PathBuf> },
    /// Import the device's player histories.
    ImportHistories { device: usize },
    /// Open the device's My Settings (player, mixer and DJ System settings files).
    AskMySettings { device: usize },
    /// Import from the opened rekordbox.xml: a list (tree ID, with its tracks) or collection rows.
    ImportFromXml { list: Option<DbId>, tracks: Vec<usize> },
    /// Analyse these collection tracks with musicbox's own analysis ("Analyze Track").
    AnalyzeTracks(Vec<DbId>),
    CancelAnalysis,
}
