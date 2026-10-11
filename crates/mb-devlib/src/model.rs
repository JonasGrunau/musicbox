//! Typed rows of exportLibrary.db. Column meanings: `docs/re/devlib.md#tables`.
//!
//! IDs are the device's own integer keys (`content_id`, `playlist_id`, …), not master.db IDs.
//! Paths are stored relative to the stick root and start with `/` (`/Contents/…`, `/PIONEER/…`).

use std::collections::HashMap;

/// The single `property` row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceProperty {
    /// Volume label of the stick (`updateDeviceName`).
    pub device_name: String,
    /// rekordbox 7.2.19 writes "10000" when it creates the row; older exports wrote "1000".
    pub db_version: String,
    /// Kept equal to `count(content)` (`update_property_num_content`).
    pub number_of_contents: u32,
    /// `YYYY-MM-DD` of creation.
    pub created_date: String,
    /// Device colour chosen in rekordbox (`selectDeviceColor`); 0 = none.
    pub background_color_type: u32,
    /// DBID of the library whose My Tag tree was written (`writeMyTag`).
    pub my_tag_master_db_id: Option<u32>,
}

/// One `content` row with its lookup names resolved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceTrack {
    pub id: u32,
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub remixer: String,
    pub original_artist: String,
    pub composer: String,
    pub lyricist: String,
    pub album: String,
    pub genre: String,
    pub label: String,
    /// Key name as stored in the `key` table (free text copied from the collection).
    pub key: String,
    /// `color.color_id` 1..=8, if set.
    pub color: Option<u8>,
    pub bpm_x100: Option<u32>,
    pub length_seconds: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub release_year: Option<u32>,
    pub rating: u8,
    /// `djComment`.
    pub comment: String,
    pub play_count: u32,
    /// kbit/s.
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub file_size: Option<u64>,
    /// Same codes as master.db `FileType`.
    pub file_type: i64,
    pub isrc: String,
    /// Device path of the audio file, e.g. `/Contents/Artist/Album/file.aiff`.
    pub path: String,
    pub file_name: String,
    /// Device path of `ANLZ0000.DAT`, e.g. `/PIONEER/USBANLZ/P004/00008C70/ANLZ0000.DAT`.
    pub analysis_path: String,
    /// Device path of the small artwork (`image.path`), e.g. `/PIONEER/Artwork/00001/b1.jpg`.
    pub artwork_path: Option<String>,
    /// `YYYY-MM-DD`.
    pub date_added: String,
    pub date_created: String,
    pub release_date: String,
    /// Copy of master.db `djmdContent.Analysed`.
    pub analysed_bits: u32,
    pub content_link: u32,
    pub has_modified: bool,
    pub hot_cue_auto_load: bool,
    pub kuvo_deliver: bool,
    pub kuvo_comment: String,
    /// master.db `djmdProperty.DBID` of the source library.
    pub master_db_id: Option<u32>,
    /// `djmdContent.ID` in the source library.
    pub master_content_id: Option<u32>,
    /// Change markers copied from `djmdContent.CueUpdated` / `AnalysisUpdated` /
    /// `TrackInfoUpdated`; may be empty.
    pub cue_update_count: String,
    pub analysis_update_count: String,
    pub information_update_count: String,
    pub ids: TrackRefs,
}

/// Raw foreign keys of a track (0 / NULL = none), kept for writers and joins.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackRefs {
    pub artist: u32,
    pub remixer: u32,
    pub original_artist: u32,
    pub composer: u32,
    pub lyricist: u32,
    pub album: u32,
    pub genre: u32,
    pub label: u32,
    pub key: u32,
    pub image: u32,
}

/// `playlist.attribute`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListAttribute {
    List,
    Folder,
    Other(i64),
}

impl ListAttribute {
    pub fn from_code(code: i64) -> Self {
        match code {
            0 => Self::List,
            1 => Self::Folder,
            other => Self::Other(other),
        }
    }
}

/// A node of the playlist, history, My Tag or hot cue bank list tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceListNode {
    pub id: u32,
    /// `None` for top-level nodes (stored parent 0).
    pub parent: Option<u32>,
    pub name: String,
    /// Position among siblings, 0-based.
    pub seq: i64,
    pub attribute: ListAttribute,
    /// `image_id` where the table has one (playlist, hotCueBankList).
    pub image_id: Option<u32>,
}

/// `cue.kind` as interpreted on read (`db::dsql_conv_cue_kind` @0x1019AA930).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueKind {
    /// Kind 0 without an out point.
    MemoryCue,
    /// Kind 0 with an out point.
    MemoryLoop,
    /// Kind 1..=17: hot cue slot (1 = A).
    HotCue(u8),
    /// Kind 101: passed through unchanged; meaning open.
    Kind101,
    /// Anything else; rekordbox drops such rows.
    Invalid(i64),
}

impl CueKind {
    pub fn from_row(kind: i64, has_out: bool) -> Self {
        match kind {
            0 if has_out => Self::MemoryLoop,
            0 => Self::MemoryCue,
            1..=17 => Self::HotCue(kind as u8),
            101 => Self::Kind101,
            other => Self::Invalid(other),
        }
    }
}

/// One cue point position in every encoding the table carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CuePosition {
    /// Microseconds = milliseconds × 1000 + sub-millisecond part.
    pub usec: i64,
    /// Position in 1/150 s units.
    pub frame_150: i64,
    /// MPEG frame number and absolute byte offset (MP3 seeking).
    pub mpeg_frame: i64,
    pub mpeg_abs: i64,
    /// Block seek information for block-based codecs.
    pub decoding_start_frame_position: i64,
    pub file_offset_in_block: i64,
    pub number_of_sample_in_block: i64,
}

impl CuePosition {
    pub fn ms(&self) -> i64 {
        self.usec.div_euclid(1000)
    }
}

/// One `cue` row (memory cue/loop, hot cue, or hot cue bank cue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCue {
    pub id: u32,
    pub content_id: u32,
    pub raw_kind: i64,
    pub kind: CueKind,
    pub color_table_index: Option<i64>,
    pub comment: String,
    pub active_loop: bool,
    pub beat_loop_numerator: Option<i64>,
    pub beat_loop_denominator: Option<i64>,
    pub in_point: CuePosition,
    /// `None` when `outUsec` is -1 (no out point).
    pub out_point: Option<CuePosition>,
}

/// `menuItem` row. Names are wrapped in U+FFFA … U+FFFB (a translation key for the player).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: u32,
    pub kind: i64,
    pub name: String,
}

impl MenuItem {
    /// The name without the U+FFFA/U+FFFB wrapper.
    pub fn label(&self) -> &str {
        self.name
            .trim_start_matches('\u{FFFA}')
            .trim_end_matches('\u{FFFB}')
    }
}

/// `category` (browse menu) or `sort` (sort menu) row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSetting {
    pub id: u32,
    pub menu_item_id: u32,
    pub seq: i64,
    pub visible: bool,
    /// `sort.isSelectedAsSubColumn`; always false for `category`.
    pub sub_column: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendedLike {
    pub content_id_1: u32,
    pub content_id_2: u32,
    pub rating: Option<i64>,
    /// Milliseconds since the Unix epoch.
    pub created_ms: Option<i64>,
}

/// Everything in one exportLibrary.db, loaded in one pass.
#[derive(Debug, Clone, Default)]
pub struct DeviceLibrary {
    pub property: Option<DeviceProperty>,
    pub tracks: Vec<DeviceTrack>,
    pub playlists: Vec<DeviceListNode>,
    /// Track IDs per playlist, in `sequenceNo` order.
    pub playlist_entries: HashMap<u32, Vec<u32>>,
    pub cues: Vec<DeviceCue>,
    pub my_tags: Vec<DeviceListNode>,
    /// (myTag_id, content_id) pairs.
    pub my_tag_content: Vec<(u32, u32)>,
    pub histories: Vec<DeviceListNode>,
    pub history_entries: HashMap<u32, Vec<u32>>,
    pub hot_cue_bank_lists: Vec<DeviceListNode>,
    /// Cue IDs per hot cue bank list, in `sequenceNo` order.
    pub hot_cue_bank_entries: HashMap<u32, Vec<u32>>,
    pub menu_items: Vec<MenuItem>,
    pub categories: Vec<MenuSetting>,
    pub sorts: Vec<MenuSetting>,
    /// (color_id, name).
    pub colors: Vec<(u32, String)>,
    pub recommended_likes: Vec<RecommendedLike>,
}
