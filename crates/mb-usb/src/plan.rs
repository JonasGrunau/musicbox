//! The export planner: source library + device state + selection → [`ExportPlan`].
//! Spec: `docs/re/export.md` (algorithm summary, § 1–4, writer checklist).
//!
//! Planning is pure: it reads only the [`SourceLibrary`] and [`DeviceState`] snapshots. It
//! replays rekordbox's export in memory against a model of both device databases, so every
//! device ID, path and file is decided here, in rekordbox's order:
//!
//! 1. Jobs are built (UI thread): each selected playlist is matched on the device by parent and
//!    name (§ 1.7); an identical list is skipped, a different one is emptied and refilled, a
//!    missing one is created on top (position 0).
//! 2. Tracks run per job in list order (`execExport`, § 1.2): the format gate, the duplicate
//!    check against each database (§ 4.1), then either an in-place update (§ 4.2) or a new
//!    track: `/Contents` path (§ 1.3–1.4), ANLZ path (§ 1.6), lookup rows and `max + 1` IDs
//!    (§ 2), artwork (§ 3), then the playlist entry.
//!
//! The result is a list of operations per database plus the file work per track; the executor
//! ([`crate::exec`]) runs them.

use crate::device::{DeviceState, DevlibAlbum, DevlibList, DevlibTrack};
use crate::naming::{self, path_key};
use crate::source::{LocalArtwork, SelectionItem, SourceLibrary, SourceTrack};
use crate::{Error, Result};
use mb_devlib::{MenuSeed, MyTagRow, NewTrack};
use mb_pdb::string::DeviceString;
use mb_pdb::tables::{
    Album, Artist, Artwork, Genre, Key, Label, MyTag, MyTagTrack, Named, PlaylistEntry,
    PlaylistNode, Track, TreeNode,
};
use mb_pdb::writer::Lookups;
use mb_pdb::{ExportDb, ExtDb};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

/// rekordbox's per-track result codes (`DbBackGroundProcess::run`, `docs/re/export.md` § 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackResult {
    /// 0: exported as a new track.
    Exported,
    /// 1: already on the device (updated in place if needed).
    Unchanged,
    /// 4: the source file does not exist (log `[1]`).
    SourceMissing,
    /// 5: a format the device cannot play (log `[2]`).
    Unsupported,
    /// 7: the device database is gone (log `[5]`).
    DeviceGone,
    /// 8: copying the audio failed (log `[3]`).
    CopyFailed,
    /// 9: the analysis files could not be written (log `[4]`, with the ANLZ path).
    AnalysisFailed,
    /// 11: not enough free space (log `[5]`).
    NoSpace,
    /// 12: the database insert failed (log `[5]`).
    InsertFailed,
}

impl TrackResult {
    /// The numeric code rekordbox stores in `TrackAdditionResult` (+0x28).
    pub fn code(self) -> u32 {
        match self {
            TrackResult::Exported => 0,
            TrackResult::Unchanged => 1,
            TrackResult::SourceMissing => 4,
            TrackResult::Unsupported => 5,
            TrackResult::DeviceGone => 7,
            TrackResult::CopyFailed => 8,
            TrackResult::AnalysisFailed => 9,
            TrackResult::NoSpace => 11,
            TrackResult::InsertFailed => 12,
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, TrackResult::Exported | TrackResult::Unchanged)
    }
}

/// Planner settings.
#[derive(Debug, Clone)]
pub struct PlanOptions {
    /// `YYYY-MM-DD` used for a new stick's `createdDate` / `DATECREATED` and for an empty
    /// `dateAdded` (`DBToolBox::getDateString`). Default: today (local time).
    pub today: String,
    /// Also copy the 320×320 `_l` artwork (`dbConversionDevice` 11 + `create320x320Artwork`,
    /// § 3). Default off, as on every stick seen so far.
    pub artwork_320: bool,
    /// On a stick that already has databases, rekordbox syncs My Tags when the stick is mounted
    /// and again when the export opens the databases (`docs/re/pdb.md` § The oracle stick's
    /// history: two My Tag syncs in session 2). Default on.
    pub mount_my_tag_sync: bool,
    /// Tracks per playlist (`get_device_playlist_track_num_max`): 10000, 1000 for some devices.
    pub max_playlist_tracks: usize,
    /// The stick holds another library's My Tags (`syncMyTag` @0x100c51eec: device value neither
    /// 0 nor ours): rekordbox asks whether to overwrite them. `true` answers yes; default `false`
    /// leaves the stick's My Tags alone and exports no My Tag links (`docs/re/export.md` § 2.5).
    pub overwrite_foreign_my_tags: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            today: mb_devlib::today(),
            artwork_320: false,
            mount_my_tag_sync: true,
            max_playlist_tracks: 10_000,
            overwrite_foreign_my_tags: false,
        }
    }
}

/// One lookup row of export.pdb (`getAndAdd*ID`), inserted in its own transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdbLookup {
    Artist(u32),
    Genre(u32),
    Album(u32),
    Label(u32),
    Key(u32),
}

/// One export.pdb operation, run through [`mb_pdb::writer::Exporter`] in order. Tests insert
/// [`PdbOp::Reopen`] / [`PdbOp::TouchPlaylist`] to replay a recorded session byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdbOp {
    /// The first list of a stick without lists: inserted and rewritten in one transaction.
    CreateFirstPlaylist(PlaylistNode),
    /// A new list inserted at the end of its siblings, then moved to position 0.
    CreatePlaylistAtTop(PlaylistNode),
    /// A new list inserted with the position it carries.
    InsertPlaylist(PlaylistNode),
    /// Copying updates of several list rows in one transaction (a reorder).
    UpdatePlaylists(Vec<PlaylistNode>),
    /// A write transaction that leaves the list row unchanged.
    TouchPlaylist(u32),
    /// All entries of a list deleted in one transaction.
    ClearPlaylist(u32),
    /// A missing lookup row (the row is in [`ExportPlan::pdb_lookups`]).
    AddLookup(PdbLookup),
    /// A new track: its lookups, track + property row, image row, IMAGEID update.
    AddTrack(Track),
    /// A copying update of an existing track row.
    UpdateTrack(Track),
    /// A new image row for an existing track (re-export), then the track update.
    AddArtwork {
        track: Track,
    },
    AddEntry(PlaylistEntry),
    /// Close and reopen the file (a new session).
    Reopen,
}

/// One exportExt.pdb operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtOp {
    /// `syncMyTag`: all My Tag rows (and track links) deleted, the tree inserted, then the links
    /// of the tracks already on the device inserted.
    SyncMyTags {
        tags: Vec<MyTag>,
        links: Vec<MyTagTrack>,
    },
    /// `replaceMyTagOnTrack` for a new track.
    AddMyTagLinks(Vec<MyTagTrack>),
    Reopen,
}

/// One exportLibrary.db operation (a [`mb_devlib::DevLibWriter`] call). IDs the writer allocates
/// are given as `expect_*` and checked.
#[derive(Debug, Clone, PartialEq)]
pub enum DevlibOp {
    /// `syncMyTagLocal`: tree rewrite, links cleared, then re-added for tracks on the device.
    WriteMyTags {
        master_db_id: u32,
        rows: Vec<MyTagRow>,
        links: Vec<(u32, Vec<u32>)>,
    },
    CreatePlaylist {
        expect_id: u32,
        name: String,
        parent: u32,
        folder: bool,
        position: Option<i64>,
    },
    ClearPlaylist(u32),
    AddTrack {
        expect_id: u32,
        track: Box<NewTrack>,
        /// `Some(id)`: an image row follows (`add_image`).
        image: Option<u32>,
        my_tags: Vec<u32>,
    },
    /// Metadata of an existing track rewritten (re-export after a library edit).
    ModifyTrack {
        content_id: u32,
        changes: Vec<DevlibChange>,
    },
    SetTrackUpdated {
        content_id: u32,
        marker: String,
    },
    SetAnalysisUpdated {
        content_id: u32,
        analysed: u8,
        analysis_marker: String,
        cue_marker: String,
    },
    SetContentLink {
        content_id: u32,
        content_link: u32,
    },
    /// A new image row for an existing track.
    AddImage {
        content_id: u32,
        expect_id: u32,
    },
    AppendEntry {
        playlist_id: u32,
        content_id: u32,
    },
}

/// One metadata column a re-export rewrites in exportLibrary.db (`modify*`).
///
/// **Hypothesis:** which fields `checkAndSetChanges` compares is open (`docs/re/export.md`
/// § 4.2); the metadata columns below are compared and the differing ones written. Lookup
/// columns get the looked-up or added ID (0 for an empty name, as `modifyGenre` & co. write);
/// unused lookup rows are kept (rekordbox's `delete_unused_record` is not replayed, so IDs stay
/// `max + 1` like in export.pdb).
#[derive(Debug, Clone, PartialEq)]
pub enum DevlibChange {
    Column(mb_devlib::ContentColumn, rusqlite::types::Value),
    Lookup(mb_devlib::LookupColumn, String),
    Album {
        name: String,
        compilation: u8,
        album_artist: String,
    },
}

/// The [`DevlibMeta`] an insert of `t` stores (at the widths `insertTrack` writes).
fn devlib_meta(t: &SourceTrack) -> crate::device::DevlibMeta {
    crate::device::DevlibMeta {
        title: t.title.clone(),
        subtitle: t.subtitle.clone(),
        bpm_x100: i64::from(t.bpm_x100 as i32),
        track_no: i64::from(t.track_no as i32),
        disc_no: i64::from(t.disc_no as i16),
        color_id: i64::from(t.color_id as i8),
        comment: t.comment.clone(),
        rating: i64::from(t.rating as i8),
        release_year: i64::from(t.release_year as i16),
        release_date: t.release_date.clone(),
        play_count: i64::from(t.play_count as i16),
        hot_cue_auto_load: i64::from(t.hot_cue_auto_load),
        kuvo_deliver: i64::from(t.delivery_control),
        kuvo_comment: t.delivery_comment.clone(),
        artist: t.artist.clone(),
        remixer: t.remixer.clone(),
        original_artist: t.original_artist.clone(),
        composer: t.composer.clone(),
        lyricist: t.lyricist.clone(),
        genre: t.genre.clone(),
        label: t.label.clone(),
        key: t.key.clone(),
        album: t.album.clone(),
        album_artist: if t.album.is_empty() {
            String::new()
        } else {
            t.album_artist.clone()
        },
        compilation: if t.album.is_empty() {
            0
        } else {
            i64::from(t.compilation)
        },
    }
}

/// The columns that differ between a device row and the source, in a fixed order (plain
/// columns, then lookups in `LookupColumn` order, then the album).
fn devlib_changes(
    have: &crate::device::DevlibMeta,
    want: &crate::device::DevlibMeta,
) -> Vec<DevlibChange> {
    use mb_devlib::{ContentColumn as C, LookupColumn as L};
    use rusqlite::types::Value;
    let mut out = Vec::new();
    let mut text = |c: C, a: &str, b: &str| {
        if a != b {
            out.push(DevlibChange::Column(c, Value::Text(b.to_owned())));
        }
    };
    text(C::Title, &have.title, &want.title);
    text(C::Subtitle, &have.subtitle, &want.subtitle);
    text(C::Comment, &have.comment, &want.comment);
    text(C::ReleaseDate, &have.release_date, &want.release_date);
    text(C::KuvoComment, &have.kuvo_comment, &want.kuvo_comment);
    for (c, a, b) in [
        (C::BpmX100, have.bpm_x100, want.bpm_x100),
        (C::TrackNo, have.track_no, want.track_no),
        (C::DiscNo, have.disc_no, want.disc_no),
        (C::ColorId, have.color_id, want.color_id),
        (C::Rating, have.rating, want.rating),
        (C::ReleaseYear, have.release_year, want.release_year),
        (C::PlayCount, have.play_count, want.play_count),
        (
            C::HotCueAutoLoad,
            have.hot_cue_auto_load,
            want.hot_cue_auto_load,
        ),
        (C::KuvoDeliver, have.kuvo_deliver, want.kuvo_deliver),
    ] {
        if a != b {
            out.push(DevlibChange::Column(c, Value::Integer(b)));
        }
    }
    for (c, a, b) in [
        (L::Artist, &have.artist, &want.artist),
        (L::Remixer, &have.remixer, &want.remixer),
        (
            L::OriginalArtist,
            &have.original_artist,
            &want.original_artist,
        ),
        (L::Composer, &have.composer, &want.composer),
        (L::Lyricist, &have.lyricist, &want.lyricist),
        (L::Genre, &have.genre, &want.genre),
        (L::Label, &have.label, &want.label),
        (L::Key, &have.key, &want.key),
    ] {
        if a != b {
            out.push(DevlibChange::Lookup(c, b.clone()));
        }
    }
    if (&have.album, &have.album_artist, have.compilation)
        != (&want.album, &want.album_artist, want.compilation)
    {
        out.push(DevlibChange::Album {
            name: want.album.clone(),
            compilation: want.compilation as u8,
            album_artist: want.album_artist.clone(),
        });
    }
    out
}

/// Which artwork variant a device file is made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkSize {
    /// `_s`, 80×80 → `<a|b><id>.jpg`.
    Small,
    /// `_m`, 240×240 → `<a|b><id>_m.jpg`.
    Medium,
    /// `_l`, 320×320 → `<a|b><id>_l.jpg`.
    Large,
}

impl ArtworkSize {
    pub fn pixels(self) -> u32 {
        match self {
            ArtworkSize::Small => 80,
            ArtworkSize::Medium => 240,
            ArtworkSize::Large => 320,
        }
    }
}

/// File work of one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    /// Copy the audio (bytes only, no extended attributes).
    CopyAudio {
        from: PathBuf,
        to: String,
        size: u64,
    },
    /// Delete an ANLZ set (`.DAT`, `.EXT`, `.2EX` next to `dat`).
    DeleteAnlz { dat: String },
    /// Build the USB ANLZ set from the local one (`mb_anlz::usb::to_usb`).
    WriteAnlz {
        local_dat: PathBuf,
        /// Which of `.DAT`, `.EXT`, `.2EX` exist locally.
        parts: [bool; 3],
        dat: String,
        /// The PPTH value: the track's device path.
        ppth: String,
    },
    /// One artwork file: the local variant copied byte for byte, or generated from the base
    /// image when the variant is missing.
    Artwork {
        from: LocalArtwork,
        size: ArtworkSize,
        to: String,
    },
}

/// One track of a job.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackStep {
    /// `djmdContent.ID`.
    pub content_id: String,
    pub title: String,
    /// The source audio path (`FolderPath`).
    pub location: String,
    pub result: TrackResult,
    /// Where the track lives on the device.
    pub device_path: Option<String>,
    /// For [`TrackResult::AnalysisFailed`]: the local `.DAT` path rekordbox prints in the log.
    pub analysis_path: Option<PathBuf>,
    pub files: Vec<FileOp>,
    pub pdb: Vec<PdbOp>,
    pub ext: Vec<ExtOp>,
    pub devlib: Vec<DevlibOp>,
}

/// One `ExportDescription`: a playlist (or loose tracks) and its tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Playlist name ("" for loose tracks).
    pub name: String,
    /// The master playlist (`None` for loose tracks and for a folder's own job).
    pub master_playlist_id: Option<String>,
    /// Master content IDs in list order.
    pub tracks: Vec<String>,
    /// export.pdb / exportLibrary.db list IDs (0 = none, e.g. loose tracks or a skipped list).
    pub pdb_list: u32,
    pub devlib_list: u32,
    /// List operations done when the job is built (UI thread).
    pub list_pdb: Vec<PdbOp>,
    pub list_devlib: Vec<DevlibOp>,
    pub steps: Vec<TrackStep>,
}

/// How exportLibrary.db is created on a new stick.
#[derive(Debug, Clone)]
pub struct DevlibCreate {
    pub created_date: String,
    pub menus: MenuSeed,
}

/// Everything an export will do.
#[derive(Debug, Clone)]
pub struct ExportPlan {
    pub root: PathBuf,
    pub hidden: bool,
    pub today: String,
    pub source_dbid: u32,
    pub my_tag_master_db_id: u32,
    /// New stick: the rows export.pdb is created with (colours, menus, property). `None`: the
    /// existing file is continued.
    pub pdb_create: Option<ExportDb>,
    /// New exportExt.pdb: its DBPROPERTYOPTION row.
    pub ext_create: Option<ExtDb>,
    pub devlib_create: Option<DevlibCreate>,
    /// Old stick with export.pdb only: exportLibrary.db is built by the Device Library Plus
    /// conversion first (`docs/re/export.md` § 6.4), then the operations below run on it.
    pub devlib_convert: Option<crate::convert::Conversion>,
    /// Operations before the jobs (My Tag syncs).
    pub ext_preamble: Vec<ExtOp>,
    pub devlib_preamble: Vec<DevlibOp>,
    pub jobs: Vec<Job>,
    /// Every export.pdb row an [`PdbOp::AddLookup`] / [`PdbOp::AddTrack`] refers to.
    pub pdb_lookups: Lookups,
}

impl ExportPlan {
    /// All steps in execution order.
    pub fn steps(&self) -> impl Iterator<Item = &TrackStep> {
        self.jobs.iter().flat_map(|j| j.steps.iter())
    }

    /// Bytes of audio the export copies.
    pub fn bytes_to_copy(&self) -> u64 {
        self.steps()
            .flat_map(|s| s.files.iter())
            .map(|f| match f {
                FileOp::CopyAudio { size, .. } => *size,
                _ => 0,
            })
            .sum()
    }

    /// Steps per result.
    pub fn counts(&self) -> BTreeMap<u32, usize> {
        let mut m = BTreeMap::new();
        for s in self.steps() {
            *m.entry(s.result.code()).or_default() += 1;
        }
        m
    }

    /// The export.pdb operations in execution order: every job's list operations (jobs are
    /// built before the background thread starts), then every job's track operations. With
    /// `completed = Some(n)` only the first `n` steps are included (cancellation).
    pub fn pdb_ops(&self, completed: Option<usize>) -> Vec<PdbOp> {
        let mut ops: Vec<PdbOp> = self.jobs.iter().flat_map(|j| j.list_pdb.clone()).collect();
        ops.extend(
            self.steps()
                .take(completed.unwrap_or(usize::MAX))
                .flat_map(|s| s.pdb.clone()),
        );
        ops
    }

    pub fn ext_ops(&self, completed: Option<usize>) -> Vec<ExtOp> {
        let mut ops = self.ext_preamble.clone();
        ops.extend(
            self.steps()
                .take(completed.unwrap_or(usize::MAX))
                .flat_map(|s| s.ext.clone()),
        );
        ops
    }

    pub fn devlib_ops(&self, completed: Option<usize>) -> Vec<DevlibOp> {
        let mut ops = self.devlib_preamble.clone();
        ops.extend(self.jobs.iter().flat_map(|j| j.list_devlib.clone()));
        ops.extend(
            self.steps()
                .take(completed.unwrap_or(usize::MAX))
                .flat_map(|s| s.devlib.clone()),
        );
        ops
    }
}

// ---- the in-memory model of the device databases --------------------------------------------

/// Name → first ID (rows are matched by exact name; the lowest ID wins like a table scan).
#[derive(Debug, Clone, Default)]
struct NameIds {
    ids: HashMap<String, u32>,
    max: u32,
}

impl NameIds {
    fn from_rows<'a>(rows: impl Iterator<Item = (u32, &'a str)>) -> Self {
        let mut s = Self::default();
        for (id, name) in rows {
            s.max = s.max.max(id);
            s.ids
                .entry(name.to_owned())
                .and_modify(|e| *e = (*e).min(id))
                .or_insert(id);
        }
        s
    }

    /// `(id, true)` for a new row.
    fn get_or_add(&mut self, name: &str) -> (u32, bool) {
        if name.is_empty() {
            return (0, false);
        }
        if let Some(id) = self.ids.get(name) {
            return (*id, false);
        }
        self.max += 1;
        self.ids.insert(name.to_owned(), self.max);
        (self.max, true)
    }
}

#[derive(Debug, Clone, Default)]
struct Albums {
    rows: Vec<DevlibAlbum>,
    max: u32,
}

impl Albums {
    /// `getAndAddAlbumID`: matched by name, plus album artist when there is one, plus the
    /// compilation flag when it is set.
    fn get_or_add(&mut self, name: &str, artist_id: u32, compilation: u8) -> (u32, bool) {
        if name.is_empty() {
            return (0, false);
        }
        if let Some(a) = self.rows.iter().find(|a| {
            a.name == name
                && (artist_id == 0 || a.artist_id == artist_id)
                && (compilation == 0 || a.compilation == compilation)
        }) {
            return (a.id, false);
        }
        self.max += 1;
        self.rows.push(DevlibAlbum {
            id: self.max,
            name: name.to_owned(),
            artist_id,
            compilation,
        });
        (self.max, true)
    }
}

/// The lookup IDs of one track in one database.
#[derive(Debug, Clone, Copy, Default)]
struct TrackIds {
    artist: u32,
    genre: u32,
    album: u32,
    remixer: u32,
    original_artist: u32,
    composer: u32,
    label: u32,
    key: u32,
}

#[derive(Debug, Clone, Default)]
struct PdbModel {
    tracks: BTreeMap<u32, Track>,
    artists: NameIds,
    genres: NameIds,
    labels: NameIds,
    keys: NameIds,
    albums: Albums,
    max_artwork: u32,
    playlists: Vec<PlaylistNode>,
    entries: HashMap<u32, Vec<u32>>,
}

#[derive(Debug, Clone, Default)]
struct DevlibModel {
    tracks: BTreeMap<u32, DevlibTrack>,
    artists: NameIds,
    genres: NameIds,
    labels: NameIds,
    keys: NameIds,
    albums: Albums,
    max_image: u32,
    playlists: Vec<DevlibList>,
    entries: HashMap<u32, Vec<u32>>,
}

/// A blank export.pdb track row (every number 0, every string empty).
pub fn blank_pdb_track() -> Track {
    Track {
        id: 0,
        folder_path: String::new(),
        file_name: String::new(),
        file_name_short: String::new(),
        title: String::new(),
        artist_id: 0,
        album_id: 0,
        genre_id: 0,
        bpm_x100: 0,
        length_s: 0,
        track_no: 0,
        bitrate: 0,
        bit_depth: 0,
        comment: String::new(),
        file_type: 0,
        rating: 0,
        release_year: 0,
        remixer_id: 0,
        label_id: 0,
        original_artist_id: 0,
        key_id: 0,
        stock_date: String::new(),
        color_id: 0,
        play_count: 0,
        artwork_id: 0,
        master_db_id: 0,
        master_content_id: 0,
        analysis_path: String::new(),
        search_str: String::new(),
        file_size: 0,
        disc_no: 0,
        composer_id: 0,
        subtitle: String::new(),
        sample_rate: 0,
        disable_quantize: 0,
        analysed: 0,
        release_date: String::new(),
        date_created: String::new(),
        content_link: 0,
        tag: String::new(),
        modified_by_rbm: String::new(),
        hot_cue_auto_load: String::new(),
        delivery_control: String::new(),
        delivery_comment: String::new(),
        cue_updated: String::new(),
        analysis_updated: String::new(),
        track_info_updated: String::new(),
        lyricist: String::new(),
        isrc: String::new(),
        isrc_raw: DeviceString::from_text(""),
        reserved10: String::new(),
    }
}

fn on_off(v: bool) -> String {
    if v { "ON".into() } else { String::new() }
}

/// The export.pdb track row `export_track_data_to_db` builds from a source track
/// (`docs/re/export.md` § 1.2 step 7.4, `docs/re/pdb.md` writer rule 11).
fn pdb_track_row(
    t: &SourceTrack,
    dbid: u32,
    id: u32,
    ids: TrackIds,
    path: &str,
    anlz: &str,
    artwork_id: u32,
) -> Track {
    let file_name = path.rsplit('/').next().unwrap_or_default().to_owned();
    Track {
        id,
        folder_path: path.to_owned(),
        file_name,
        title: t.title.clone(),
        artist_id: ids.artist,
        album_id: ids.album,
        genre_id: ids.genre,
        bpm_x100: t.bpm_x100 as u32,
        length_s: t.length as u16,
        track_no: t.track_no as u32,
        bitrate: t.bitrate as u32,
        bit_depth: t.bit_depth as u16,
        comment: t.comment.clone(),
        file_type: t.file_type as u8,
        rating: t.rating as u8,
        release_year: t.release_year as u16,
        remixer_id: ids.remixer,
        label_id: ids.label,
        original_artist_id: ids.original_artist,
        key_id: ids.key,
        stock_date: t.stock_date.clone(),
        color_id: t.color_id as u8,
        play_count: t.play_count as u16,
        artwork_id,
        master_db_id: t.device_master_db_id(dbid),
        master_content_id: t.device_master_content_id(),
        analysis_path: anlz.to_owned(),
        file_size: t.file_size as u32,
        disc_no: t.disc_no as u16,
        composer_id: ids.composer,
        subtitle: t.subtitle.clone(),
        sample_rate: t.sample_rate as u32,
        analysed: mb_devlib::export_analysed_bits(t.analysed),
        release_date: t.release_date.clone(),
        date_created: t.date_created.clone(),
        content_link: mb_devlib::export_content_link(t.content_link),
        hot_cue_auto_load: on_off(t.hot_cue_auto_load),
        delivery_control: on_off(t.delivery_control),
        delivery_comment: t.delivery_comment.clone(),
        cue_updated: t.cue_marker.clone(),
        analysis_updated: t.analysis_marker.clone(),
        track_info_updated: t.info_marker.clone(),
        lyricist: t.lyricist.clone(),
        isrc: t.isrc.clone(),
        ..blank_pdb_track()
    }
}

/// The exportLibrary.db insert (`DevSqliteDBController::insertTrack`) for a source track.
fn devlib_new_track(t: &SourceTrack, dbid: u32, path: &str, anlz: &str) -> NewTrack {
    NewTrack {
        title: t.title.clone(),
        subtitle: t.subtitle.clone(),
        artist: t.artist.clone(),
        remixer: t.remixer.clone(),
        original_artist: t.original_artist.clone(),
        composer: t.composer.clone(),
        lyricist: t.lyricist.clone(),
        album: t.album.clone(),
        album_artist: t.album_artist.clone(),
        compilation: t.compilation,
        genre: t.genre.clone(),
        label: t.label.clone(),
        key: t.key.clone(),
        bpm_x100: t.bpm_x100,
        length: t.length,
        track_no: t.track_no,
        disc_no: t.disc_no,
        release_year: t.release_year,
        release_date: t.release_date.clone(),
        rating: t.rating,
        color_id: t.color_id,
        comment: t.comment.clone(),
        date_created: t.date_created.clone(),
        date_added: t.stock_date.clone(),
        path: path.to_owned(),
        file_name: String::new(),
        file_size: t.file_size as u64,
        file_type: t.file_type,
        bitrate: t.bitrate,
        bit_depth: t.bit_depth,
        sample_rate: t.sample_rate as u32,
        isrc: t.isrc.clone(),
        play_count: t.play_count,
        hot_cue_auto_load: t.hot_cue_auto_load,
        kuvo_deliver: t.delivery_control,
        kuvo_comment: t.delivery_comment.clone(),
        master_db_id: t.device_master_db_id(dbid),
        master_content_id: t.device_master_content_id(),
        analysis_path: anlz.to_owned(),
        analysed_bits: mb_devlib::export_analysed_bits(t.analysed),
        content_link: mb_devlib::export_content_link(t.content_link),
        has_modified: false,
        image_id: 0,
    }
}

/// The USB branch of the format gate (`check_support_export` @0x1009bfce4 / `execExport`
/// step 4, `docs/re/export.md` § 6.2): DRUM CAPTURE (0x40) never; FLAC, ALAC, WAV and AIFF
/// only at 44.1, 48, 88.2 or 96 kHz.
pub fn format_supported(file_type: i64, sample_rate: i64) -> bool {
    match file_type {
        0x40 => false,
        5 | 6 | 11 | 12 => matches!(sample_rate, 44_100 | 48_000 | 88_200 | 96_000),
        _ => true,
    }
}

/// Why a track cannot be exported before anything is written, if it cannot.
fn gate(t: &SourceTrack) -> Option<TrackResult> {
    if t.audio.is_none() {
        return Some(TrackResult::SourceMissing);
    }
    if !format_supported(t.file_type, t.sample_rate) {
        return Some(TrackResult::Unsupported);
    }
    // rekordbox analyses first and retries (code 10); musicbox cannot, so a track without its
    // local ANLZ0000.DAT is reported like a failed analysis export.
    if !t.analysis.dat {
        return Some(TrackResult::AnalysisFailed);
    }
    None
}

struct Planner<'a> {
    src: &'a SourceLibrary,
    opts: &'a PlanOptions,
    hidden: bool,
    pdb: PdbModel,
    devlib: DevlibModel,
    /// Path keys of every file under /Contents (existing + planned).
    contents: HashSet<String>,
    /// ANLZ folder key → (number, device DAT path, PPTH) of the sets there.
    anlz: HashMap<String, Vec<(u32, String, Option<String>)>>,
    artwork: HashSet<String>,
    lookups: Lookups,
    /// export.pdb already has a list (else the next new list is "the first list of a stick").
    pdb_first_list_done: bool,
    /// My Tag export enabled for this device (controller +0x40, set by `syncMyTag`): the tree
    /// is written and new tracks get their My Tag links.
    my_tags: bool,
    /// The device is FAT32 (`msdos`): files of 4 GiB or more cannot be stored.
    fat32: bool,
}

/// Plans exporting `selection` from `src` to the device described by `dev`.
pub fn plan_export(
    src: &SourceLibrary,
    dev: &DeviceState,
    selection: &[SelectionItem],
    opts: &PlanOptions,
) -> Result<ExportPlan> {
    // A stick with export.pdb but no exportLibrary.db is converted first (§ 6.4); the export is
    // then planned against the converted database.
    let devlib_convert = match (&dev.pdb, &dev.devlib) {
        (Some(pdb), None) => Some(crate::convert::plan_conversion(
            pdb,
            src.menus.devlib_seed(),
            &opts.today,
        )),
        _ => None,
    };
    let converted;
    let dev = match &devlib_convert {
        Some(conv) => {
            converted = DeviceState {
                devlib: Some(conv.devlib_state()?),
                ..dev.clone()
            };
            &converted
        }
        None => dev,
    };
    let mut p = Planner::new(src, dev, opts);
    let mut plan = ExportPlan {
        root: dev.root.clone(),
        hidden: dev.hidden,
        today: opts.today.clone(),
        source_dbid: src.dbid,
        my_tag_master_db_id: src.my_tag_master_db_id,
        pdb_create: None,
        ext_create: None,
        devlib_create: None,
        devlib_convert,
        ext_preamble: Vec::new(),
        devlib_preamble: Vec::new(),
        jobs: Vec::new(),
        pdb_lookups: Lookups::default(),
    };
    match (&dev.pdb, &dev.devlib) {
        (None, None) => {
            let mut rows = src.menus.pdb_rows();
            rows.properties = vec![mb_pdb::defaults::property(&opts.today, 0)];
            plan.pdb_create = Some(rows);
            plan.ext_create = Some(mb_pdb::defaults::ext_db(src.my_tag_master_db_id));
            plan.devlib_create = Some(DevlibCreate {
                created_date: opts.today.clone(),
                menus: src.menus.devlib_seed(),
            });
        }
        (Some(pdb), Some(_)) => {
            if pdb.ext.is_none() {
                plan.ext_create = Some(mb_pdb::defaults::ext_db(src.my_tag_master_db_id));
            }
        }
        (Some(_), None) => unreachable!("converted above"),
        (None, Some(_)) => {
            return Err(Error::Device(
                "the stick has exportLibrary.db but no export.pdb".into(),
            ));
        }
    }

    // My Tag syncs (`syncMyTag` on mount for an existing stick, then on `createDeviceDB`).
    let syncs = if !p.my_tags {
        0
    } else if dev.pdb.is_some() && opts.mount_my_tag_sync {
        2
    } else {
        1
    };
    for _ in 0..syncs {
        plan.ext_preamble.push(ExtOp::SyncMyTags {
            tags: src.pdb_my_tags(),
            links: p.pdb_my_tag_links(),
        });
    }
    if p.my_tags {
        plan.devlib_preamble.push(DevlibOp::WriteMyTags {
            master_db_id: src.my_tag_master_db_id,
            rows: src.devlib_my_tags(),
            links: p.devlib_my_tag_links(),
        });
    }

    // Jobs (UI thread): list rows first, for every job.
    let mut jobs = Vec::new();
    for item in selection {
        match item {
            SelectionItem::Playlist(id) => p.playlist_jobs(id, 0, 0, true, &mut jobs)?,
            SelectionItem::Tracks(ids) => jobs.push(Job {
                name: String::new(),
                master_playlist_id: None,
                tracks: ids.clone(),
                pdb_list: 0,
                devlib_list: 0,
                list_pdb: Vec::new(),
                list_devlib: Vec::new(),
                steps: Vec::new(),
            }),
        }
    }
    let lists = p.pdb.playlists.len().max(p.devlib.playlists.len());
    if lists > 1000 {
        return Err(Error::Plan(format!(
            "{lists} lists and folders on the device; rekordbox allows at most 1000"
        )));
    }

    // Tracks (background thread), job by job.
    for job in &mut jobs {
        if job.master_playlist_id.is_some() && job.pdb_list == 0 && job.devlib_list == 0 {
            continue; // identical on both databases (or a folder): nothing to do
        }
        if job.tracks.len() > opts.max_playlist_tracks && job.master_playlist_id.is_some() {
            return Err(Error::Plan(format!(
                "playlist {:?} has {} tracks; the device allows {}",
                job.name,
                job.tracks.len(),
                opts.max_playlist_tracks
            )));
        }
        job.steps = job
            .tracks
            .iter()
            .map(|id| p.track_step(id, job.pdb_list, job.devlib_list))
            .collect::<Result<Vec<_>>>()?;
    }
    plan.jobs = jobs;
    plan.pdb_lookups = p.lookups;
    Ok(plan)
}

fn empty_step() -> TrackStep {
    TrackStep {
        content_id: String::new(),
        title: String::new(),
        location: String::new(),
        result: TrackResult::Unchanged,
        device_path: None,
        analysis_path: None,
        files: Vec::new(),
        pdb: Vec::new(),
        ext: Vec::new(),
        devlib: Vec::new(),
    }
}

impl<'a> Planner<'a> {
    fn new(src: &'a SourceLibrary, dev: &DeviceState, opts: &'a PlanOptions) -> Self {
        let mut pdb = PdbModel::default();
        if let Some(files) = &dev.pdb {
            let e = &files.export;
            pdb.tracks = e.tracks.iter().map(|t| (t.id, t.clone())).collect();
            pdb.artists = NameIds::from_rows(e.artists.iter().map(|a| (a.id, a.name.as_str())));
            pdb.genres = NameIds::from_rows(e.genres.iter().map(|g| (g.0.id, g.0.name.as_str())));
            pdb.labels = NameIds::from_rows(e.labels.iter().map(|l| (l.0.id, l.0.name.as_str())));
            pdb.keys = NameIds::from_rows(e.keys.iter().map(|k| (k.id, k.name.as_str())));
            let mut rows: Vec<DevlibAlbum> = e
                .albums
                .iter()
                .map(|a| DevlibAlbum {
                    id: a.id,
                    name: a.name.clone(),
                    artist_id: a.album_artist_id,
                    compilation: a.compilation,
                })
                .collect();
            rows.sort_by_key(|a| a.id);
            pdb.albums = Albums {
                max: rows.iter().map(|a| a.id).max().unwrap_or(0),
                rows,
            };
            pdb.max_artwork = e.artwork.iter().map(|a| a.id).max().unwrap_or(0);
            pdb.playlists = e.playlists.clone();
            let mut entries: Vec<&PlaylistEntry> = e.playlist_entries.iter().collect();
            entries.sort_by_key(|x| (x.playlist_id, x.entry_index));
            for x in entries {
                pdb.entries
                    .entry(x.playlist_id)
                    .or_default()
                    .push(x.track_id);
            }
        }
        let mut devlib = DevlibModel::default();
        if let Some(d) = &dev.devlib {
            devlib.tracks = d.tracks.iter().map(|t| (t.id, t.clone())).collect();
            devlib.artists = NameIds::from_rows(d.artists.iter().map(|(i, n)| (*i, n.as_str())));
            devlib.genres = NameIds::from_rows(d.genres.iter().map(|(i, n)| (*i, n.as_str())));
            devlib.labels = NameIds::from_rows(d.labels.iter().map(|(i, n)| (*i, n.as_str())));
            devlib.keys = NameIds::from_rows(d.keys.iter().map(|(i, n)| (*i, n.as_str())));
            devlib.albums = Albums {
                max: d.albums.iter().map(|a| a.id).max().unwrap_or(0),
                rows: d.albums.clone(),
            };
            devlib.max_image = d.max_image_id;
            devlib.playlists = d.playlists.clone();
            devlib.entries = d.entries.clone();
        }
        let mut anlz: HashMap<String, Vec<(u32, String, Option<String>)>> = HashMap::new();
        for (folder, entries) in &dev.anlz {
            anlz.insert(
                folder.clone(),
                entries
                    .iter()
                    .map(|e| (e.number, e.file.clone(), e.ppth.clone()))
                    .collect(),
            );
        }
        let pdb_has_lists = !pdb.playlists.is_empty();
        // Paths a track row of either database still points at count as taken even when the
        // file is missing. rekordbox only tests the file (§ 1.4); on such a stick it would hand
        // the path to the new track and purge the old row's ANLZ set (§ 1.6). Not reproduced.
        let mut contents = dev.contents.clone();
        contents.extend(pdb.tracks.values().map(|t| path_key(&t.folder_path)));
        contents.extend(devlib.tracks.values().map(|t| path_key(&t.path)));
        Self {
            src,
            opts,
            hidden: dev.hidden,
            pdb,
            devlib,
            contents,
            anlz,
            artwork: dev.artwork.clone(),
            lookups: Lookups::default(),
            pdb_first_list_done: pdb_has_lists,
            fat32: dev.filesystem.as_deref() == Some("msdos"),
            my_tags: {
                let owner = dev.my_tag_master_db_id();
                owner == 0 || owner == src.my_tag_master_db_id || opts.overwrite_foreign_my_tags
            },
        }
    }

    // ---- My Tags ------------------------------------------------------------------------------

    /// Links for the tracks already on the device that come from this library.
    fn pdb_my_tag_links(&self) -> Vec<MyTagTrack> {
        let mut out = Vec::new();
        for t in self.pdb.tracks.values() {
            if t.master_db_id != self.src.dbid {
                continue;
            }
            if let Some(src) = self
                .src
                .by_master_content_id
                .get(&t.master_content_id)
                .and_then(|id| self.src.tracks.get(id))
            {
                out.extend(src.my_tags.iter().map(|tag| my_tag_link(*tag, t.id)));
            }
        }
        out
    }

    fn devlib_my_tag_links(&self) -> Vec<(u32, Vec<u32>)> {
        let mut out = Vec::new();
        for t in self.devlib.tracks.values() {
            if t.master_db_id != self.src.dbid {
                continue;
            }
            if let Some(src) = self
                .src
                .by_master_content_id
                .get(&t.master_content_id)
                .and_then(|id| self.src.tracks.get(id))
                && !src.my_tags.is_empty()
            {
                out.push((t.id, src.my_tags.clone()));
            }
        }
        out
    }

    // ---- playlists ----------------------------------------------------------------------------

    /// Master IDs of the tracks the export would put into a list, after the format gate.
    fn exportable_entries(&self, list: &str) -> Vec<String> {
        self.src
            .entries
            .get(list)
            .map(|ids| {
                ids.iter()
                    .filter(|id| self.src.tracks.get(*id).is_some_and(|t| gate(t).is_none()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn master_id_of(&self, master_db_id: u32, master_content_id: u32) -> Option<&String> {
        (master_db_id == self.src.dbid)
            .then(|| self.src.by_master_content_id.get(&master_content_id))
            .flatten()
    }

    /// Jobs for a selected playlist or folder; `top` = selected directly (moved to position 0).
    fn playlist_jobs(
        &mut self,
        id: &str,
        pdb_parent: u32,
        devlib_parent: u32,
        top: bool,
        jobs: &mut Vec<Job>,
    ) -> Result<()> {
        let node = self
            .src
            .playlist(id)
            .ok_or_else(|| Error::Selection(format!("no playlist with ID {id}")))?
            .clone();
        if node.attribute == 4 {
            return Err(Error::Plan(format!(
                "{:?} is an intelligent playlist; exporting those is not supported",
                node.name
            )));
        }
        let folder = node.is_folder();
        let wanted = self.exportable_entries(id);

        // export.pdb
        let mut list_pdb = Vec::new();
        let existing = self
            .pdb
            .playlists
            .iter()
            .filter(|p| p.0.parent_id == pdb_parent && p.0.name == node.name)
            .min_by_key(|p| p.0.id)
            .cloned();
        let pdb_list = match existing {
            Some(p) if (p.0.attribute == 1) != folder => {
                return Err(Error::Plan(format!(
                    "{:?} exists on the device as a {} (export.pdb)",
                    node.name,
                    if folder { "playlist" } else { "folder" }
                )));
            }
            Some(p) if folder => p.0.id,
            Some(p) => {
                let have: Vec<Option<&String>> = self
                    .pdb
                    .entries
                    .get(&p.0.id)
                    .map(|v| {
                        v.iter()
                            .map(|tid| {
                                self.pdb.tracks.get(tid).and_then(|t| {
                                    self.master_id_of(t.master_db_id, t.master_content_id)
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if have.len() == wanted.len()
                    && have.iter().zip(&wanted).all(|(h, w)| *h == Some(w))
                {
                    0 // identical: skipped
                } else {
                    list_pdb.push(PdbOp::ClearPlaylist(p.0.id));
                    self.pdb.entries.remove(&p.0.id);
                    p.0.id
                }
            }
            None => {
                let id = self.pdb.playlists.iter().map(|p| p.0.id).max().unwrap_or(0) + 1;
                let siblings = self
                    .pdb
                    .playlists
                    .iter()
                    .filter(|p| p.0.parent_id == pdb_parent)
                    .count() as u32;
                let mut row = PlaylistNode(TreeNode {
                    id,
                    seq: 0,
                    name: node.name.clone(),
                    artwork_id: 0,
                    attribute: u8::from(folder),
                    parent_id: pdb_parent,
                });
                if !self.pdb_first_list_done {
                    list_pdb.push(PdbOp::CreateFirstPlaylist(row.clone()));
                    self.pdb_first_list_done = true;
                } else if top {
                    list_pdb.push(PdbOp::CreatePlaylistAtTop(row.clone()));
                    for p in self
                        .pdb
                        .playlists
                        .iter_mut()
                        .filter(|p| p.0.parent_id == pdb_parent)
                    {
                        p.0.seq += 1;
                    }
                } else {
                    row.0.seq = siblings;
                    list_pdb.push(PdbOp::InsertPlaylist(row.clone()));
                }
                self.pdb.playlists.push(row);
                id
            }
        };

        // exportLibrary.db
        let mut list_devlib = Vec::new();
        let existing = self
            .devlib
            .playlists
            .iter()
            .filter(|p| p.parent == devlib_parent && p.name == node.name)
            .min_by_key(|p| p.id)
            .cloned();
        let devlib_list = match existing {
            Some(p) if (p.attribute == 1) != folder => {
                return Err(Error::Plan(format!(
                    "{:?} exists on the device as a {} (exportLibrary.db)",
                    node.name,
                    if folder { "playlist" } else { "folder" }
                )));
            }
            Some(p) if folder => p.id,
            Some(p) => {
                let have: Vec<Option<&String>> = self
                    .devlib
                    .entries
                    .get(&p.id)
                    .map(|v| {
                        v.iter()
                            .map(|cid| {
                                self.devlib.tracks.get(cid).and_then(|t| {
                                    self.master_id_of(t.master_db_id, t.master_content_id)
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if have.len() == wanted.len()
                    && have.iter().zip(&wanted).all(|(h, w)| *h == Some(w))
                {
                    0
                } else {
                    list_devlib.push(DevlibOp::ClearPlaylist(p.id));
                    self.devlib.entries.remove(&p.id);
                    p.id
                }
            }
            None => {
                let id = self
                    .devlib
                    .playlists
                    .iter()
                    .map(|p| p.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let siblings: Vec<i64> = self
                    .devlib
                    .playlists
                    .iter()
                    .filter(|p| p.parent == devlib_parent)
                    .map(|p| p.seq)
                    .collect();
                let seq = if siblings.is_empty() {
                    0
                } else {
                    siblings.iter().max().copied().unwrap_or(0) + 1
                };
                let position = top.then_some(0);
                list_devlib.push(DevlibOp::CreatePlaylist {
                    expect_id: id,
                    name: node.name.clone(),
                    parent: devlib_parent,
                    folder,
                    position,
                });
                if top && seq > 0 {
                    for p in self
                        .devlib
                        .playlists
                        .iter_mut()
                        .filter(|p| p.parent == devlib_parent)
                    {
                        p.seq += 1;
                    }
                }
                self.devlib.playlists.push(DevlibList {
                    id,
                    parent: devlib_parent,
                    name: node.name.clone(),
                    seq: if top { 0 } else { seq },
                    attribute: i64::from(folder),
                });
                id
            }
        };

        if folder {
            jobs.push(Job {
                name: node.name.clone(),
                master_playlist_id: None,
                tracks: Vec::new(),
                pdb_list: 0,
                devlib_list: 0,
                list_pdb,
                list_devlib,
                steps: Vec::new(),
            });
            let children: Vec<String> =
                self.src.children(id).iter().map(|c| c.id.clone()).collect();
            for child in children {
                self.playlist_jobs(&child, pdb_list, devlib_list, false, jobs)?;
            }
        } else {
            jobs.push(Job {
                name: node.name.clone(),
                master_playlist_id: Some(id.to_owned()),
                tracks: self.src.entries.get(id).cloned().unwrap_or_default(),
                pdb_list,
                devlib_list,
                list_pdb,
                list_devlib,
                steps: Vec::new(),
            });
        }
        Ok(())
    }

    // ---- tracks -------------------------------------------------------------------------------

    fn find_pdb(&self, t: &SourceTrack) -> Option<u32> {
        let (mdb, mcid) = (
            t.device_master_db_id(self.src.dbid),
            t.device_master_content_id(),
        );
        self.pdb
            .tracks
            .values()
            .find(|r| {
                r.master_db_id == mdb
                    && r.master_content_id == mcid
                    && r.length_s == t.length as u16
                    && r.bitrate == t.bitrate as u32
                    && r.bit_depth == t.bit_depth as u16
                    && r.file_type == t.file_type as u8
                    && r.file_size == t.file_size as u32
                    && r.sample_rate == t.sample_rate as u32
            })
            .map(|r| r.id)
    }

    /// `isCollectionSongForDuplicateSongId` against exportLibrary.db (compared at the widths
    /// `insertTrack` stores).
    fn find_devlib(&self, t: &SourceTrack) -> Option<u32> {
        let (mdb, mcid) = (
            t.device_master_db_id(self.src.dbid),
            t.device_master_content_id(),
        );
        self.devlib
            .tracks
            .values()
            .find(|r| {
                r.master_db_id == mdb
                    && r.master_content_id == mcid
                    && r.length == i64::from(t.length as i16)
                    && r.bitrate == i64::from(t.bitrate as i32)
                    && r.bit_depth == i64::from(t.bit_depth as i16)
                    && r.file_type == i64::from(t.file_type as i8)
                    && r.file_size == i64::from(t.file_size as u32)
                    && r.sample_rate == i64::from(t.sample_rate as u32)
            })
            .map(|r| r.id)
    }

    /// `calc_device_file_path`: the candidate, or the first free `-n` variant.
    fn device_path(&self, t: &SourceTrack) -> String {
        let dir = naming::contents_dir(&t.artist, &t.album);
        let name = naming::file_component(&t.source_file_name());
        let mut path = format!("{dir}/{name}");
        let mut n = 1;
        while self.contents.contains(&path_key(&path)) {
            path = format!("{dir}/{}", naming::clash_candidate(&name, n));
            n += 1;
        }
        path
    }

    /// `calc_device_analysis_path`: the folder hash of the device path, file number = highest
    /// `ANLZxxxx` there + 1. Also returns the stale sets `removeOldAnalysisFiles` deletes.
    fn anlz_path(&self, device_path: &str) -> (String, Vec<String>) {
        let folder = naming::anlz_folder(device_path, self.hidden);
        let entries = self.anlz.get(&path_key(&folder));
        let number = entries
            .and_then(|e| e.iter().map(|(n, _, _)| *n).max())
            .map_or(0, |n| n + 1);
        let stale = entries
            .map(|e| {
                e.iter()
                    .filter(|(_, file, ppth)| {
                        file.to_ascii_uppercase().ends_with(".DAT")
                            && ppth
                                .as_deref()
                                .is_some_and(|p| p.to_lowercase() == device_path.to_lowercase())
                    })
                    .map(|(_, file, _)| file.clone())
                    .collect()
            })
            .unwrap_or_default();
        (
            naming::anlz_dat_path(device_path, number, self.hidden),
            stale,
        )
    }

    fn note_anlz(&mut self, dat: &str, ppth: &str, stale: &[String]) {
        let folder = dat.rsplit_once('/').map_or("", |(f, _)| f).to_owned();
        let entries = self.anlz.entry(path_key(&folder)).or_default();
        let stale_keys: HashSet<String> = stale
            .iter()
            .filter_map(|s| s.rsplit_once('.').map(|(b, _)| b.to_lowercase()))
            .collect();
        entries.retain(|(_, f, _)| {
            f.rsplit_once('.')
                .is_none_or(|(b, _)| !stale_keys.contains(&b.to_lowercase()))
        });
        let number = naming::anlz_number(dat.rsplit('/').next().unwrap_or_default()).unwrap_or(0);
        for (i, ext) in ["DAT", "EXT", "2EX"].iter().enumerate() {
            let file = format!("{}.{ext}", &dat[..dat.len() - 4]);
            entries.push((number, file, (i == 0).then(|| ppth.to_owned())));
        }
    }

    /// pdb lookup IDs in `addTrack` order, recording each new row as an [`PdbOp::AddLookup`].
    fn pdb_ids(&mut self, t: &SourceTrack, ops: &mut Vec<PdbOp>) -> TrackIds {
        let mut ids = TrackIds::default();
        let artist = |s: &mut Self, name: &str, ops: &mut Vec<PdbOp>| -> u32 {
            let (id, new) = s.pdb.artists.get_or_add(name);
            if new {
                s.lookups.artists.insert(
                    id,
                    Artist {
                        id,
                        name: name.to_owned(),
                        search_str: String::new(),
                    },
                );
                ops.push(PdbOp::AddLookup(PdbLookup::Artist(id)));
            }
            id
        };
        ids.artist = artist(self, &t.artist, ops);
        let (genre, new) = self.pdb.genres.get_or_add(&t.genre);
        if new {
            self.lookups.genres.insert(
                genre,
                Genre(Named {
                    id: genre,
                    name: t.genre.clone(),
                }),
            );
            ops.push(PdbOp::AddLookup(PdbLookup::Genre(genre)));
        }
        ids.genre = genre;
        if !t.album.is_empty() {
            let album_artist = artist(self, &t.album_artist, ops);
            let (album, new) = self
                .pdb
                .albums
                .get_or_add(&t.album, album_artist, t.compilation);
            if new {
                self.lookups.albums.insert(
                    album,
                    Album {
                        id: album,
                        name: t.album.clone(),
                        album_artist_id: album_artist,
                        artwork_id: 0,
                        compilation: t.compilation,
                        search_str: String::new(),
                    },
                );
                ops.push(PdbOp::AddLookup(PdbLookup::Album(album)));
            }
            ids.album = album;
        }
        ids.remixer = artist(self, &t.remixer, ops);
        ids.original_artist = artist(self, &t.original_artist, ops);
        ids.composer = artist(self, &t.composer, ops);
        let (label, new) = self.pdb.labels.get_or_add(&t.label);
        if new {
            self.lookups.labels.insert(
                label,
                Label(Named {
                    id: label,
                    name: t.label.clone(),
                }),
            );
            ops.push(PdbOp::AddLookup(PdbLookup::Label(label)));
        }
        ids.label = label;
        let (key, new) = self.pdb.keys.get_or_add(&t.key);
        if new {
            // SEQ: the device key ID (master `djmdKey.Seq` is NULL; every fixture row has
            // SEQ = ID).
            self.lookups.keys.insert(
                key,
                Key {
                    id: key,
                    name: t.key.clone(),
                    seq: key as u8,
                },
            );
            ops.push(PdbOp::AddLookup(PdbLookup::Key(key)));
        }
        ids.key = key;
        ids
    }

    /// Advances the exportLibrary.db model like `DevLibWriter::add_track` would (lookups in
    /// `addTrack` order, lyricist last).
    fn devlib_ids(&mut self, t: &SourceTrack) {
        let d = &mut self.devlib;
        d.artists.get_or_add(&t.artist);
        d.genres.get_or_add(&t.genre);
        if !t.album.is_empty() {
            let (aa, _) = d.artists.get_or_add(&t.album_artist);
            d.albums.get_or_add(&t.album, aa, t.compilation);
        }
        d.artists.get_or_add(&t.remixer);
        d.labels.get_or_add(&t.label);
        d.artists.get_or_add(&t.original_artist);
        d.keys.get_or_add(&t.key);
        d.artists.get_or_add(&t.composer);
        d.artists.get_or_add(&t.lyricist);
    }

    /// Advances the exportLibrary.db model for a [`DevlibOp::ModifyTrack`] (new lookup rows).
    fn devlib_change_ids(&mut self, changes: &[DevlibChange]) {
        use mb_devlib::LookupColumn as L;
        let d = &mut self.devlib;
        for c in changes {
            match c {
                DevlibChange::Column(..) => {}
                DevlibChange::Lookup(col, name) => {
                    match col {
                        L::Genre => d.genres.get_or_add(name),
                        L::Label => d.labels.get_or_add(name),
                        L::Key => d.keys.get_or_add(name),
                        _ => d.artists.get_or_add(name),
                    };
                }
                DevlibChange::Album {
                    name,
                    compilation,
                    album_artist,
                } => {
                    if !name.is_empty() {
                        let (aa, _) = d.artists.get_or_add(album_artist);
                        d.albums.get_or_add(name, aa, *compilation);
                    }
                }
            }
        }
    }

    fn artwork_files(&self, art: &LocalArtwork, prefix: char, id: u32) -> Vec<FileOp> {
        let small = naming::artwork_path(prefix, id, self.hidden);
        let mut files = vec![
            FileOp::Artwork {
                from: art.clone(),
                size: ArtworkSize::Small,
                to: small.clone(),
            },
            FileOp::Artwork {
                from: art.clone(),
                size: ArtworkSize::Medium,
                to: naming::artwork_variant(&small, "_m"),
            },
        ];
        if self.opts.artwork_320 {
            files.push(FileOp::Artwork {
                from: art.clone(),
                size: ArtworkSize::Large,
                to: naming::artwork_variant(&small, "_l"),
            });
        }
        files
    }

    fn has_artwork(t: &SourceTrack) -> Option<&LocalArtwork> {
        t.artwork
            .as_ref()
            .filter(|a| a.base_exists || a.small.is_some() && a.medium.is_some())
    }

    fn track_step(&mut self, id: &str, pdb_list: u32, devlib_list: u32) -> Result<TrackStep> {
        let t = self
            .src
            .tracks
            .get(id)
            .ok_or_else(|| Error::Selection(format!("no track with ID {id}")))?
            .clone();
        let mut step = TrackStep {
            content_id: t.id.clone(),
            title: t.title.clone(),
            location: t.folder_path.clone(),
            ..empty_step()
        };
        if let Some(result) = gate(&t) {
            step.result = result;
            step.analysis_path = t.analysis.dat_path.clone();
            return Ok(step);
        }
        let dbid = self.src.dbid;
        let pdb_hit = self.find_pdb(&t);
        let devlib_hit = self.find_devlib(&t);

        // Device path and ANLZ path: reused from whichever database already holds the track
        // (the audio is copied once per stick), else new.
        let reuse = pdb_hit
            .and_then(|i| self.pdb.tracks.get(&i))
            .map(|r| (r.folder_path.clone(), r.analysis_path.clone()))
            .or_else(|| {
                devlib_hit
                    .and_then(|i| self.devlib.tracks.get(&i))
                    .map(|r| (r.path.clone(), r.analysis_path.clone()))
            });
        let (path, anlz) = match reuse {
            Some(x) => x,
            None => {
                let audio = t.audio.as_ref().expect("gate checked the audio");
                // FAT32 stores at most 4 GiB − 1 bytes per file. rekordbox has no check for it
                // (export.md § Open questions), so its copy fails with code 8 / log `[3]`; the
                // same result is planned here so the export goes on with the next track.
                if self.fat32 && audio.size > u64::from(u32::MAX) {
                    step.result = TrackResult::CopyFailed;
                    return Ok(step);
                }
                let path = self.device_path(&t);
                let (anlz, stale) = self.anlz_path(&path);
                step.files.push(FileOp::CopyAudio {
                    from: audio.path.clone(),
                    to: path.clone(),
                    size: audio.size,
                });
                for dat in &stale {
                    step.files.push(FileOp::DeleteAnlz { dat: dat.clone() });
                }
                step.files.push(FileOp::WriteAnlz {
                    local_dat: t.analysis.dat_path.clone().expect("gate checked the DAT"),
                    parts: [t.analysis.dat, t.analysis.ext, t.analysis.ex2],
                    dat: anlz.clone(),
                    ppth: path.clone(),
                });
                self.contents.insert(path_key(&path));
                self.note_anlz(&anlz, &path, &stale);
                (path, anlz)
            }
        };
        step.device_path = Some(path.clone());
        let mut rewrite_anlz = false;
        let art = Self::has_artwork(&t).cloned();

        // export.pdb (the primary handle).
        let pdb_id = match pdb_hit {
            Some(tid) => {
                let row = self.pdb.tracks[&tid].clone();
                let mut new = row.clone();
                let info = t.info_marker != row.track_info_updated;
                let analysis =
                    t.analysis_marker != row.analysis_updated || t.cue_marker != row.cue_updated;
                if info {
                    // `checkAndSetChanges` + `modifyTrack` + `setTrackUpdated`: the metadata is
                    // taken from the source again (**hypothesis**: which fields rekordbox
                    // compares is open, export.md § 4.2).
                    let mut lookups = Vec::new();
                    let ids = self.pdb_ids(&t, &mut lookups);
                    step.pdb.extend(lookups);
                    let fresh = pdb_track_row(
                        &t,
                        dbid,
                        tid,
                        ids,
                        &row.folder_path,
                        &row.analysis_path,
                        row.artwork_id,
                    );
                    new = Track {
                        analysed: row.analysed,
                        content_link: row.content_link,
                        analysis_updated: row.analysis_updated.clone(),
                        cue_updated: row.cue_updated.clone(),
                        ..fresh
                    };
                }
                if analysis {
                    // `setAnalysisUpdated` + `modContentLink`; the ANLZ set is rewritten.
                    new.analysed = t.analysed as u8;
                    new.analysis_updated = t.analysis_marker.clone();
                    new.cue_updated = t.cue_marker.clone();
                    if let Some(link) = mb_devlib::reexport_content_link(
                        row.content_link,
                        t.content_link as u32,
                        true,
                    ) {
                        new.content_link = link;
                    }
                    rewrite_anlz = true;
                }
                if new != row {
                    step.pdb.push(PdbOp::UpdateTrack(new.clone()));
                    self.pdb.tracks.insert(tid, new.clone());
                }
                // `export_art_when_overwrite`.
                if let Some(a) = &art {
                    if new.artwork_id == 0 {
                        self.pdb.max_artwork += 1;
                        let img = self.pdb.max_artwork;
                        self.lookups.artwork.insert(
                            img,
                            Artwork {
                                id: img,
                                path: naming::artwork_path('a', img, self.hidden),
                            },
                        );
                        let mut with_art = new.clone();
                        with_art.artwork_id = img;
                        step.pdb.push(PdbOp::AddArtwork {
                            track: with_art.clone(),
                        });
                        self.pdb.tracks.insert(tid, with_art);
                        step.files.extend(self.artwork_files(a, 'a', img));
                    } else {
                        let small = naming::artwork_path('a', new.artwork_id, self.hidden);
                        let m = naming::artwork_variant(&small, "_m");
                        if !self.artwork.contains(&path_key(&m)) {
                            step.files
                                .extend(self.artwork_files(a, 'a', new.artwork_id));
                        }
                    }
                }
                step.result = TrackResult::Unchanged;
                tid
            }
            None => {
                let mut ops = Vec::new();
                let ids = self.pdb_ids(&t, &mut ops);
                let tid = self.pdb.tracks.keys().max().copied().unwrap_or(0) + 1;
                let mut artwork_id = 0;
                if let Some(a) = &art {
                    self.pdb.max_artwork += 1;
                    artwork_id = self.pdb.max_artwork;
                    self.lookups.artwork.insert(
                        artwork_id,
                        Artwork {
                            id: artwork_id,
                            path: naming::artwork_path('a', artwork_id, self.hidden),
                        },
                    );
                    step.files.extend(self.artwork_files(a, 'a', artwork_id));
                }
                let row = pdb_track_row(&t, dbid, tid, ids, &path, &anlz, artwork_id);
                ops.push(PdbOp::AddTrack(row.clone()));
                step.pdb.extend(ops);
                self.pdb.tracks.insert(tid, row);
                if self.my_tags && !t.my_tags.is_empty() {
                    step.ext.push(ExtOp::AddMyTagLinks(
                        t.my_tags.iter().map(|tag| my_tag_link(*tag, tid)).collect(),
                    ));
                }
                step.result = TrackResult::Exported;
                tid
            }
        };

        // exportLibrary.db (a same-device database).
        let devlib_id = match devlib_hit {
            Some(cid) => {
                let row = self.devlib.tracks[&cid].clone();
                let mut new = row.clone();
                if t.info_marker != row.info_marker {
                    // `checkAndSetChanges` + `modifyTrack`, then `setTrackUpdated`.
                    let meta = devlib_meta(&t);
                    let changes = devlib_changes(&row.meta, &meta);
                    if !changes.is_empty() {
                        self.devlib_change_ids(&changes);
                        step.devlib.push(DevlibOp::ModifyTrack {
                            content_id: cid,
                            changes,
                        });
                        new.meta = meta;
                    }
                    step.devlib.push(DevlibOp::SetTrackUpdated {
                        content_id: cid,
                        marker: t.info_marker.clone(),
                    });
                    new.info_marker = t.info_marker.clone();
                }
                if t.analysis_marker != row.analysis_marker || t.cue_marker != row.cue_marker {
                    step.devlib.push(DevlibOp::SetAnalysisUpdated {
                        content_id: cid,
                        analysed: t.analysed as u8,
                        analysis_marker: t.analysis_marker.clone(),
                        cue_marker: t.cue_marker.clone(),
                    });
                    new.analysis_marker = t.analysis_marker.clone();
                    new.cue_marker = t.cue_marker.clone();
                    if let Some(link) = mb_devlib::reexport_content_link(
                        row.content_link,
                        t.content_link as u32,
                        true,
                    ) {
                        step.devlib.push(DevlibOp::SetContentLink {
                            content_id: cid,
                            content_link: link,
                        });
                        new.content_link = link;
                    }
                    rewrite_anlz = true;
                }
                if let Some(a) = &art {
                    if new.image_id == 0 {
                        self.devlib.max_image += 1;
                        let img = self.devlib.max_image;
                        step.devlib.push(DevlibOp::AddImage {
                            content_id: cid,
                            expect_id: img,
                        });
                        new.image_id = img;
                        step.files.extend(self.artwork_files(a, 'b', img));
                    } else {
                        let small = naming::artwork_path('b', new.image_id, self.hidden);
                        let m = naming::artwork_variant(&small, "_m");
                        if !self.artwork.contains(&path_key(&m)) {
                            step.files.extend(self.artwork_files(a, 'b', new.image_id));
                        }
                    }
                }
                self.devlib.tracks.insert(cid, new);
                cid
            }
            None => {
                self.devlib_ids(&t);
                let cid = self.devlib.tracks.keys().max().copied().unwrap_or(0) + 1;
                let mut image = None;
                if let Some(a) = &art {
                    self.devlib.max_image += 1;
                    let img = self.devlib.max_image;
                    image = Some(img);
                    step.files.extend(self.artwork_files(a, 'b', img));
                }
                let new = devlib_new_track(&t, dbid, &path, &anlz);
                step.devlib.push(DevlibOp::AddTrack {
                    expect_id: cid,
                    track: Box::new(new),
                    image,
                    my_tags: if self.my_tags {
                        t.my_tags.clone()
                    } else {
                        Vec::new()
                    },
                });
                self.devlib.tracks.insert(
                    cid,
                    DevlibTrack {
                        id: cid,
                        path: path.clone(),
                        analysis_path: anlz.clone(),
                        master_db_id: t.device_master_db_id(dbid),
                        master_content_id: t.device_master_content_id(),
                        length: i64::from(t.length as i16),
                        bitrate: i64::from(t.bitrate as i32),
                        bit_depth: i64::from(t.bit_depth as i16),
                        file_type: i64::from(t.file_type as i8),
                        file_size: i64::from(t.file_size as u32),
                        sample_rate: i64::from(t.sample_rate as u32),
                        info_marker: String::new(),
                        analysis_marker: String::new(),
                        cue_marker: String::new(),
                        content_link: mb_devlib::export_content_link(t.content_link),
                        image_id: image.unwrap_or(0),
                        meta: devlib_meta(&t),
                    },
                );
                if step.result == TrackResult::Unchanged {
                    step.result = TrackResult::Exported;
                }
                cid
            }
        };

        if rewrite_anlz && reuse_is_existing(&step) {
            step.files.push(FileOp::DeleteAnlz { dat: anlz.clone() });
            step.files.push(FileOp::WriteAnlz {
                local_dat: t.analysis.dat_path.clone().expect("gate checked the DAT"),
                parts: [t.analysis.dat, t.analysis.ext, t.analysis.ex2],
                dat: anlz.clone(),
                ppth: path.clone(),
            });
        }

        // Playlist entries (`add_track_to_device_playlist`, 1-based).
        if pdb_list != 0 {
            let list = self.pdb.entries.entry(pdb_list).or_default();
            list.push(pdb_id);
            step.pdb.push(PdbOp::AddEntry(PlaylistEntry {
                playlist_id: pdb_list,
                track_id: pdb_id,
                entry_index: list.len() as u32,
            }));
        }
        if devlib_list != 0 {
            self.devlib
                .entries
                .entry(devlib_list)
                .or_default()
                .push(devlib_id);
            step.devlib.push(DevlibOp::AppendEntry {
                playlist_id: devlib_list,
                content_id: devlib_id,
            });
        }
        Ok(step)
    }
}

/// True when the step did not already copy the audio and write a new ANLZ set.
fn reuse_is_existing(step: &TrackStep) -> bool {
    !step
        .files
        .iter()
        .any(|f| matches!(f, FileOp::CopyAudio { .. }))
}

fn my_tag_link(tag: u32, track: u32) -> MyTagTrack {
    MyTagTrack {
        tag_id: tag,
        track_id: track,
        reserved1: 0,
        reserved2: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{LocalAnalysis, SourceFile};

    fn track(id: &str, artist: &str, album: &str, file: &str) -> SourceTrack {
        SourceTrack {
            id: id.into(),
            title: format!("title {id}"),
            artist: artist.into(),
            album: album.into(),
            key: "Am".into(),
            file_type: 1,
            sample_rate: 44_100,
            length: 100,
            bitrate: 320,
            bit_depth: 16,
            file_size: 1000,
            analysed: 105,
            info_marker: "4".into(),
            analysis_marker: "6".into(),
            folder_path: format!("/music/{file}"),
            file_name: file.into(),
            audio: Some(SourceFile {
                path: format!("/music/{file}").into(),
                size: 1000,
            }),
            analysis: LocalAnalysis {
                dat_path: Some(format!("/share/{id}/ANLZ0000.DAT").into()),
                dat: true,
                ext: true,
                ex2: true,
            },
            ..Default::default()
        }
    }

    fn library(tracks: Vec<SourceTrack>, lists: &[(&str, &str, &[&str])]) -> SourceLibrary {
        let mut lib = SourceLibrary {
            dbid: 1000,
            my_tag_master_db_id: 1001,
            ..Default::default()
        };
        for t in tracks {
            lib.by_master_content_id
                .insert(t.device_master_content_id(), t.id.clone());
            lib.tracks.insert(t.id.clone(), t);
        }
        for (i, (id, name, entries)) in lists.iter().enumerate() {
            lib.playlists.push(crate::source::SourcePlaylist {
                id: (*id).into(),
                parent: None,
                name: (*name).into(),
                attribute: 0,
                seq: i as i64 + 1,
            });
            lib.entries.insert(
                (*id).into(),
                entries.iter().map(|e| (*e).to_string()).collect(),
            );
        }
        lib
    }

    #[test]
    fn new_stick_ids_paths_and_clashes() {
        let mut dup = track("2", "Artist", "Album", "Song.mp3");
        dup.file_size = 2000; // a different file with the same name
        let lib = library(
            vec![
                track("1", "Artist", "Album", "Song.mp3"),
                dup,
                track("3", "", "", "x?y.mp3"),
            ],
            &[("10", "list", &["1", "2", "3"])],
        );
        let dev = DeviceState::empty(std::path::Path::new("/dev"));
        let plan = plan_export(
            &lib,
            &dev,
            &[SelectionItem::Playlist("10".into())],
            &PlanOptions::default(),
        )
        .unwrap();
        let paths: Vec<_> = plan
            .steps()
            .map(|s| s.device_path.clone().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "/Contents/Artist/Album/Song.mp3",
                "/Contents/Artist/Album/Song-1.mp3",
                "/Contents/UnknownArtist/UnknownAlbum/x_y.mp3"
            ]
        );
        let ops = plan.pdb_ops(None);
        assert!(matches!(&ops[0], PdbOp::CreateFirstPlaylist(n) if n.0.id == 1));
        let tracks: Vec<u32> = ops
            .iter()
            .filter_map(|o| match o {
                PdbOp::AddTrack(t) => Some(t.id),
                _ => None,
            })
            .collect();
        assert_eq!(tracks, [1, 2, 3]);
        // One artist, one album, one key row for the first track; none for the second.
        let lookups = ops
            .iter()
            .filter(|o| matches!(o, PdbOp::AddLookup(_)))
            .count();
        assert_eq!(lookups, 3);
        assert_eq!(plan.counts().get(&0), Some(&3));
    }

    #[test]
    fn format_gate_and_missing_files() {
        let mut wav = track("1", "a", "b", "low.wav");
        wav.file_type = 11;
        wav.sample_rate = 22_050;
        let mut gone = track("2", "a", "b", "gone.mp3");
        gone.audio = None;
        let lib = library(vec![wav, gone], &[("10", "l", &["1", "2"])]);
        let dev = DeviceState::empty(std::path::Path::new("/dev"));
        let plan = plan_export(
            &lib,
            &dev,
            &[SelectionItem::Playlist("10".into())],
            &PlanOptions::default(),
        )
        .unwrap();
        let results: Vec<_> = plan.steps().map(|s| s.result).collect();
        assert_eq!(
            results,
            [TrackResult::Unsupported, TrackResult::SourceMissing]
        );
        assert!(format_supported(5, 96_000));
        assert!(!format_supported(12, 192_000));
        assert!(!format_supported(0x40, 44_100));
        assert!(format_supported(1, 22_050));
    }

    #[test]
    fn files_over_4_gib_fail_on_fat32_only() {
        let mut big = track("1", "a", "b", "big.wav");
        big.audio.as_mut().unwrap().size = 1 << 32;
        let small = track("2", "a", "b", "small.mp3");
        let lib = library(vec![big, small], &[("10", "l", &["1", "2"])]);
        let sel = [SelectionItem::Playlist("10".into())];
        let mut dev = DeviceState::empty(std::path::Path::new("/dev"));
        dev.filesystem = Some("msdos".into());
        let plan = plan_export(&lib, &dev, &sel, &PlanOptions::default()).unwrap();
        let steps: Vec<_> = plan.steps().collect();
        assert_eq!(steps[0].result, TrackResult::CopyFailed);
        assert!(steps[0].files.is_empty() && steps[0].pdb.is_empty());
        assert_eq!(steps[1].result, TrackResult::Exported);
        dev.filesystem = Some("exfat".into());
        let plan = plan_export(&lib, &dev, &sel, &PlanOptions::default()).unwrap();
        assert!(plan.steps().all(|s| s.result == TrackResult::Exported));
    }
}
