//! User intents produced by the tree and the track table, applied by the app.

use mb_core::DbId;
use mb_library::{Command, NodeKind};

/// Payload while tracks are dragged from the table.
#[derive(Clone)]
pub struct TrackDrag(pub Vec<DbId>);

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
    /// Export a local playlist to the device at this index of `Devices::list`.
    Export { playlist: DbId, device: usize },
    CancelExport,
    /// Confirm, then remove lists (name paths from the top level) and/or tracks (absolute
    /// paths) from the device at this index of `Devices::list`.
    AskRemoveFromDevice { device: usize, playlists: Vec<Vec<String>>, tracks: Vec<std::path::PathBuf> },
}
