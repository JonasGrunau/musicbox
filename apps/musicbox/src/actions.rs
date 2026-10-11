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
    AskRemoveFromCollection(Vec<DbId>),
    /// Export a local playlist to the device at this index of `Devices::list`.
    Export { playlist: DbId, device: usize },
    CancelExport,
}
