//! Pure domain types shared by every musicbox crate. No I/O lives here.

pub mod key;
pub mod smartlist;

use std::path::PathBuf;

/// Row ID as stored by rekordbox: master.db keeps random u32 values as decimal strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DbId(pub String);

impl DbId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DbId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Audio container/codec as rekordbox classifies it (`djmdContent.FileType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Mp3,
    M4a,
    Flac,
    Alac,
    Wav,
    Aiff,
    Other(i64),
}

impl FileType {
    /// Codes observed in master.db and export.pdb (docs/re/masterdb.md).
    pub fn from_code(code: i64) -> Self {
        match code {
            1 => Self::Mp3,
            4 => Self::M4a,
            5 => Self::Flac,
            6 => Self::Alac,
            11 => Self::Wav,
            12 => Self::Aiff,
            other => Self::Other(other),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::M4a => "M4A",
            Self::Flac => "FLAC",
            Self::Alac => "ALAC",
            Self::Wav => "WAV",
            Self::Aiff => "AIFF",
            Self::Other(_) => "?",
        }
    }
}

/// One collection entry, flattened with its lookup names for display.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: DbId,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub label: String,
    pub remixer: String,
    pub composer: String,
    pub original_artist: String,
    /// The album's artist (`djmdAlbum.AlbumArtistID`).
    pub album_artist: String,
    /// "Mix name" (`djmdContent.Subtitle`).
    pub mix_name: String,
    /// `djmdContent.FileNameL`; on devices the last path component.
    pub file_name: String,
    /// Key name as stored (`djmdKey.ScaleName`), e.g. `Am`.
    pub key: String,
    /// rekordbox colour slot 1..=8, if set.
    pub color: Option<u8>,
    /// BPM × 100.
    pub bpm_x100: Option<u32>,
    pub length_seconds: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub release_year: Option<u32>,
    pub rating: u8,
    pub comment: String,
    pub play_count: u32,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub file_size: Option<u64>,
    pub file_type: FileType,
    pub path: PathBuf,
    /// `StockDate`, YYYY-MM-DD.
    pub date_added: String,
    /// `DateCreated`, YYYY-MM-DD.
    pub date_created: String,
    pub release_date: String,
    /// Absolute path of `ANLZ0000.DAT`, if analysed.
    pub analysis_path: Option<PathBuf>,
    pub artwork_path: Option<PathBuf>,
    pub analysed: u32,
}

impl Track {
    pub fn bpm(&self) -> Option<f64> {
        self.bpm_x100.map(|b| b as f64 / 100.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistKind {
    Playlist,
    Folder,
    Smart,
}

/// A node of the playlist tree. `parent` is `None` for top-level nodes.
#[derive(Debug, Clone)]
pub struct PlaylistNode {
    pub id: DbId,
    pub parent: Option<DbId>,
    pub name: String,
    pub kind: PlaylistKind,
    pub seq: i64,
}

/// Everything the browser shows, loaded in one pass.
#[derive(Debug, Clone, Default)]
pub struct LibrarySnapshot {
    pub tracks: Vec<Track>,
    /// Playlist tree nodes, unordered; order siblings by `seq`.
    pub playlists: Vec<PlaylistNode>,
    /// Track IDs per playlist, in playlist order.
    pub playlist_entries: std::collections::HashMap<DbId, Vec<DbId>>,
    /// Row IDs of those entries (`djmdSongPlaylist.ID`), parallel to `playlist_entries`; empty for
    /// libraries whose entries have no own ID (devices).
    pub playlist_entry_ids: std::collections::HashMap<DbId, Vec<String>>,
    /// History lists (folders and playlists), same shape as `playlists`.
    pub histories: Vec<PlaylistNode>,
    pub history_entries: std::collections::HashMap<DbId, Vec<DbId>>,
    /// My Tag tree: groups are `Folder`s, tags are `Playlist`s.
    pub my_tags: Vec<PlaylistNode>,
    /// Track IDs per My Tag.
    pub my_tag_entries: std::collections::HashMap<DbId, Vec<DbId>>,
    /// Rule XML of each smart list (`djmdPlaylist.SmartList`, [`smartlist`]); its tracks are
    /// computed by [`smartlist::evaluate_all`], never stored.
    pub smart_lists: std::collections::HashMap<DbId, String>,
}
