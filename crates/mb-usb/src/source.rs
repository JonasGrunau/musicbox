//! The source side of an export: everything the planner needs from the local library, read once
//! up front so that planning itself does no I/O. Reads master.db (through
//! [`mb_masterdb::MasterDb::connection`]) and checks the files the export would read: the
//! audio, the local ANLZ set and the artwork variants. Spec: `docs/re/export.md` § 1.2,
//! `docs/re/ops/export-usb.md` (the reads rekordbox does), `docs/re/ops/export-devlib.md`.

use crate::{Error, Result};
use mb_devlib::schema::{MasterMenuItem, MasterMenuSetting};
use mb_devlib::{ListAttribute, MenuSeed, MyTagRow};
use rusqlite::types::Value;
use rusqlite::{Connection, Row};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A file the export would read, as found on disk when the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub path: PathBuf,
    pub size: u64,
}

/// The local analysis set of a track (`docs/re/anlz.md` § Files). `.3EX` is never exported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalAnalysis {
    /// Where master.db says the `.DAT` is (`AnalysisDataPath` under the analysis root).
    pub dat_path: Option<PathBuf>,
    pub dat: bool,
    pub ext: bool,
    pub ex2: bool,
}

impl LocalAnalysis {
    pub fn with_extension(&self, ext: &str) -> Option<PathBuf> {
        self.dat_path.as_ref().map(|p| p.with_extension(ext))
    }
}

/// The local artwork of a track: `<art>.jpg` plus the `_s` (80×80), `_m` (240×240) and `_l`
/// (320×320) variants (`docs/re/export.md` § 3). Missing variants are regenerated from the base
/// image by the executor (`create_small_arts_if_not_exists`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalArtwork {
    pub base: PathBuf,
    pub base_exists: bool,
    pub small: Option<PathBuf>,
    pub medium: Option<PathBuf>,
    pub large: Option<PathBuf>,
}

/// One `djmdContent` row with the joined names, as `getRowDataTrack` provides it to the export.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceTrack {
    /// `djmdContent.ID`.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub remixer: String,
    pub original_artist: String,
    pub composer: String,
    pub lyricist: String,
    pub album: String,
    pub album_artist: String,
    pub compilation: u8,
    pub genre: String,
    pub label: String,
    /// `djmdKey.ScaleName`.
    pub key: String,
    pub bpm_x100: i64,
    pub length: i64,
    pub track_no: i64,
    pub disc_no: i64,
    pub release_year: i64,
    pub release_date: String,
    pub rating: i64,
    pub color_id: i64,
    pub comment: String,
    pub date_created: String,
    /// `StockDate` (date added).
    pub stock_date: String,
    pub file_size: i64,
    pub file_type: i64,
    pub bitrate: i64,
    pub bit_depth: i64,
    pub sample_rate: i64,
    pub isrc: String,
    pub play_count: i64,
    pub hot_cue_auto_load: bool,
    pub delivery_control: bool,
    pub delivery_comment: String,
    /// Raw `MasterDBID` / `MasterSongID` (0 when empty).
    pub master_db_id: i64,
    pub master_song_id: i64,
    pub analysed: i64,
    pub content_link: i64,
    /// `TrackInfoUpdated`, `AnalysisUpdated`, `CueUpdated` as text (NULL → "").
    pub info_marker: String,
    pub analysis_marker: String,
    pub cue_marker: String,
    /// `FolderPath`: absolute path of the audio file.
    pub folder_path: String,
    /// `FileNameL`.
    pub file_name: String,
    /// `ImagePath` (relative to the analysis root, "" = no artwork).
    pub image_path: String,
    /// The audio file, if it exists.
    pub audio: Option<SourceFile>,
    pub analysis: LocalAnalysis,
    pub artwork: Option<LocalArtwork>,
    /// My Tag IDs (`djmdSongMyTag`, `TrackNo` order).
    pub my_tags: Vec<u32>,
    /// `djmdCue` rows in row order (`getContentCueInfo`); the device cue tags are built from
    /// them (`crate::cues`).
    pub cues: Vec<crate::cues::SourceCue>,
}

impl SourceTrack {
    /// `RowDataTrack::getMasterDBID` @0x1004e0d2c: the row's `MasterDBID`, else the library's.
    pub fn device_master_db_id(&self, library_dbid: u32) -> u32 {
        if self.master_db_id != 0 {
            self.master_db_id as u32
        } else {
            library_dbid
        }
    }

    /// `getMasterContentID` @0x1004e0d44: `MasterSongID`, else the row's own ID.
    pub fn device_master_content_id(&self) -> u32 {
        if self.master_song_id != 0 {
            self.master_song_id as u32
        } else {
            self.id.parse::<i64>().unwrap_or(0) as u32
        }
    }

    /// The audio file name rekordbox builds the device name from (`File::getFileName` of the
    /// source path).
    pub fn source_file_name(&self) -> String {
        Path::new(&self.folder_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.file_name.clone())
    }
}

/// A `djmdPlaylist` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePlaylist {
    pub id: String,
    /// `None` = top level (`ParentID = 'root'`).
    pub parent: Option<String>,
    pub name: String,
    /// 0 playlist, 1 folder, 4 smart.
    pub attribute: i64,
    pub seq: i64,
}

impl SourcePlaylist {
    pub fn is_folder(&self) -> bool {
        self.attribute == 1
    }
}

/// One `djmdMyTag` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMyTag {
    pub id: u32,
    pub seq: i64,
    pub name: String,
    pub attribute: i64,
    /// 0 = top level (`'root'`).
    pub parent: u32,
}

/// Browse menus rekordbox copies into a new device database (`initPropertyTable`; master
/// `djmdColor`, `djmdMenuItems`, `djmdCategory`, `djmdSort`).
#[derive(Debug, Clone, Default)]
pub struct SourceMenus {
    /// (ID, ColorCode, SortKey, Commnt) by `cast(ID as integer)`.
    pub colors: Vec<(u32, u32, u32, String)>,
    /// Row order of `select * from djmdMenuItems`.
    pub menu_items: Vec<MasterMenuItem>,
    /// `order by Seq` (the export.pdb insertion order), with `InfoOrder`.
    pub categories: Vec<(MasterMenuSetting, i64)>,
    pub sorts: Vec<MasterMenuSetting>,
}

impl SourceMenus {
    /// The exportLibrary.db seed (`MenuSeed::from_master`; categories and sorts in ID order, the
    /// order of the Device Library Plus conversion, export-devlib.md phase 2).
    pub fn devlib_seed(&self) -> MenuSeed {
        let colors: Vec<(u32, String)> = self
            .colors
            .iter()
            .map(|(id, _, _, name)| (*id, name.clone()))
            .collect();
        let mut cats: Vec<MasterMenuSetting> =
            self.categories.iter().map(|(c, _)| c.clone()).collect();
        cats.sort_by_key(|c| c.id);
        let mut sorts = self.sorts.clone();
        sorts.sort_by_key(|s| s.id);
        let mut items = self.menu_items.clone();
        items.sort_by_key(|i| i.id);
        MenuSeed::from_master(&colors, &items, &cats, &sorts)
    }

    /// The export.pdb rows (`DevInitPropertyTable_` copies them in the master read order).
    pub fn pdb_rows(&self) -> mb_pdb::ExportDb {
        use mb_pdb::tables::{Category, Color, MenuItem, Sort};
        mb_pdb::ExportDb {
            colors: self
                .colors
                .iter()
                .map(|(id, code, sort_key, name)| Color {
                    id: *id as u8,
                    color_code: *code,
                    sort_key: *sort_key as u8,
                    name: name.clone(),
                })
                .collect(),
            menu_items: self
                .menu_items
                .iter()
                .map(|m| MenuItem {
                    id: m.id as u16,
                    class: (m.class & 0xff) as u8,
                    name: format!("\u{fffa}{}\u{fffb}", m.name),
                })
                .collect(),
            categories: self
                .categories
                .iter()
                .map(|(c, info)| Category {
                    id: c.id as u16,
                    menu_item_id: c.menu_item_id as u16,
                    seq: c.seq as u8,
                    disable: c.disable as u8,
                    info_order: *info as u8,
                })
                .collect(),
            sorts: self
                .sorts
                .iter()
                .map(|s| Sort {
                    id: s.id as u16,
                    menu_item_id: s.menu_item_id as u16,
                    seq: s.seq as u8,
                    disable: s.disable as u8,
                })
                .collect(),
            ..Default::default()
        }
    }
}

/// Everything the planner reads from the source library.
#[derive(Debug, Clone, Default)]
pub struct SourceLibrary {
    /// `djmdProperty.DBID`.
    pub dbid: u32,
    /// `property.myTagMasterDBID` / `DBPROPERTYOPTION.MYTAGMASTERDBID` (`syncMyTagLocal`
    /// @0x100c52988): the cloud library's DBID when cloud sync is set up, else DBID + 1.
    pub my_tag_master_db_id: u32,
    pub menus: SourceMenus,
    /// The whole My Tag tree in the order rekordbox writes it: each group (by `Seq`) followed by
    /// its tags (by `Seq`).
    pub my_tags: Vec<SourceMyTag>,
    /// Every live playlist node.
    pub playlists: Vec<SourcePlaylist>,
    /// Entries (content IDs in `TrackNo` order) of the playlists the selection needs.
    pub entries: HashMap<String, Vec<String>>,
    /// Every track the selection needs, plus every other track of this library that the device
    /// might already hold (for the My Tag relink), keyed by `djmdContent.ID`.
    pub tracks: HashMap<String, SourceTrack>,
    /// Content IDs by device master content ID, for mapping device rows back to the library.
    pub by_master_content_id: HashMap<u32, String>,
}

/// A library directory as captured in fixtures: `options.json` + `rekordbox/master.db` (with
/// `-wal`/`-shm`) + `rekordbox/share`. master.db is copied into a temp directory and opened
/// read-only there, so the library itself is never opened (SQLite would create side files).
pub struct LibraryDir {
    pub master: mb_masterdb::MasterDb,
    /// `rekordbox/share` when present, else options.json `analysis-data-root-path`.
    pub analysis_root: PathBuf,
    /// options.json `settings-root-path` (rekordbox's local settings folder).
    pub settings_root: PathBuf,
    tmp: PathBuf,
}

impl LibraryDir {
    pub fn open(dir: &Path) -> Result<Self> {
        let src = [dir.join("rekordbox/master.db"), dir.join("master.db")]
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| Error::Selection(format!("no master.db under {}", dir.display())))?;
        let paths = mb_masterdb::LibraryPaths::from_options_json(&dir.join("options.json"))?;
        let passphrase = mb_masterdb::derive_passphrase(&paths.dp)?;
        let share = src.parent().map(|p| p.join("share"));
        let analysis_root = share
            .filter(|s| s.is_dir())
            .unwrap_or(paths.analysis_data_root);
        Self::open_paths(&src, &passphrase, analysis_root, paths.settings_root)
    }

    /// Opens a library given its parts (the live library: master.db, its passphrase, the
    /// analysis-data root and rekordbox's settings folder). master.db is copied to a temp dir
    /// first, so the live file is never opened by the export.
    pub fn open_paths(src: &Path, passphrase: &str, analysis_root: PathBuf, settings_root: PathBuf) -> Result<Self> {
        let tmp = std::env::temp_dir().join(format!(
            "mb-usb-master-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&tmp).map_err(|e| Error::io(&tmp, e))?;
        for suffix in ["", "-wal", "-shm"] {
            let from = PathBuf::from(format!("{}{suffix}", src.display()));
            if from.exists() {
                let to = tmp.join(format!("master.db{suffix}"));
                std::fs::copy(&from, &to).map_err(|e| Error::io(&from, e))?;
            }
        }
        let master = mb_masterdb::MasterDb::open_read_only(&tmp.join("master.db"), passphrase)?;
        Ok(Self {
            master,
            analysis_root,
            settings_root,
            tmp,
        })
    }

    pub fn connection(&self) -> &Connection {
        self.master.connection()
    }
}

impl Drop for LibraryDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

/// What to export, in order. Each item becomes one or more export jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionItem {
    /// A playlist or folder by `djmdPlaylist.ID`. A folder is exported with its contents.
    Playlist(String),
    /// Loose tracks by `djmdContent.ID` (no device playlist).
    Tracks(Vec<String>),
}

fn text(r: &Row, i: usize) -> rusqlite::Result<String> {
    Ok(match r.get::<_, Value>(i)? {
        Value::Null | Value::Blob(_) => String::new(),
        Value::Integer(v) => v.to_string(),
        Value::Real(v) => v.to_string(),
        Value::Text(t) => t,
    })
}

fn int(r: &Row, i: usize) -> rusqlite::Result<i64> {
    Ok(match r.get::<_, Value>(i)? {
        Value::Integer(v) => v,
        Value::Real(v) => v as i64,
        Value::Text(t) => t.trim().parse().unwrap_or(0),
        Value::Null | Value::Blob(_) => 0,
    })
}

const TRACK_SQL: &str = "
SELECT c.ID, c.Title, c.Subtitle, ar.Name, rm.Name, oa.Name, co.Name, c.Lyricist,
       al.Name, aa.Name, al.Compilation, ge.Name, la.Name, k.ScaleName,
       c.BPM, c.Length, c.TrackNo, c.DiscNo, c.ReleaseYear, c.ReleaseDate, c.Rating, c.ColorID,
       c.Commnt, c.DateCreated, c.StockDate, c.FileSize, c.FileType, c.BitRate, c.BitDepth,
       c.SampleRate, c.ISRC, c.DJPlayCount, c.HotCueAutoLoad, c.DeliveryControl,
       c.DeliveryComment, c.MasterDBID, c.MasterSongID, c.Analysed, c.ContentLink, c.ImagePath,
       c.TrackInfoUpdated, c.AnalysisUpdated, c.CueUpdated, c.FileNameL, c.FolderPath,
       c.AnalysisDataPath
FROM djmdContent c
LEFT JOIN djmdArtist ar ON ar.ID = c.ArtistID
LEFT JOIN djmdArtist rm ON rm.ID = c.RemixerID
LEFT JOIN djmdArtist oa ON oa.ID = c.OrgArtistID
LEFT JOIN djmdArtist co ON co.ID = c.ComposerID
LEFT JOIN djmdAlbum al ON al.ID = c.AlbumID
LEFT JOIN djmdArtist aa ON aa.ID = al.AlbumArtistID
LEFT JOIN djmdGenre ge ON ge.ID = c.GenreID
LEFT JOIN djmdLabel la ON la.ID = c.LabelID
LEFT JOIN djmdKey k ON k.ID = c.KeyID
WHERE c.rb_local_deleted = 0";

fn track_row(r: &Row) -> rusqlite::Result<SourceTrack> {
    Ok(SourceTrack {
        id: text(r, 0)?,
        title: text(r, 1)?,
        subtitle: text(r, 2)?,
        artist: text(r, 3)?,
        remixer: text(r, 4)?,
        original_artist: text(r, 5)?,
        composer: text(r, 6)?,
        lyricist: text(r, 7)?,
        album: text(r, 8)?,
        album_artist: text(r, 9)?,
        compilation: int(r, 10)? as u8,
        genre: text(r, 11)?,
        label: text(r, 12)?,
        key: text(r, 13)?,
        bpm_x100: int(r, 14)?,
        length: int(r, 15)?,
        track_no: int(r, 16)?,
        disc_no: int(r, 17)?,
        release_year: int(r, 18)?,
        release_date: text(r, 19)?,
        rating: int(r, 20)?,
        color_id: int(r, 21)?,
        comment: text(r, 22)?,
        date_created: text(r, 23)?,
        stock_date: text(r, 24)?,
        file_size: int(r, 25)?,
        file_type: int(r, 26)?,
        bitrate: int(r, 27)?,
        bit_depth: int(r, 28)?,
        sample_rate: int(r, 29)?,
        isrc: text(r, 30)?,
        play_count: int(r, 31)?,
        hot_cue_auto_load: text(r, 32)?.eq_ignore_ascii_case("on"),
        delivery_control: text(r, 33)?.eq_ignore_ascii_case("on"),
        delivery_comment: text(r, 34)?,
        master_db_id: int(r, 35)?,
        master_song_id: int(r, 36)?,
        analysed: int(r, 37)?,
        content_link: int(r, 38)?,
        image_path: text(r, 39)?,
        info_marker: text(r, 40)?,
        analysis_marker: text(r, 41)?,
        cue_marker: text(r, 42)?,
        file_name: text(r, 43)?,
        folder_path: text(r, 44)?,
        analysis: LocalAnalysis {
            dat_path: Some(PathBuf::from(text(r, 45)?)).filter(|p| !p.as_os_str().is_empty()),
            ..Default::default()
        },
        ..SourceTrack::default()
    })
}

/// Every live `djmdCue` row by `ContentID`, in row order: `getContentCueInfo` selects them
/// without ORDER BY through the `(ContentID, rb_local_deleted)` index.
fn load_cues(master: &Connection) -> Result<HashMap<String, Vec<crate::cues::SourceCue>>> {
    let mut stmt = master.prepare(
        "select ContentID, Kind, InMsec, InMpegFrame, InMpegAbs, OutMsec, OutMpegFrame, OutMpegAbs, \
         Color, ColorTableIndex, ActiveLoop, Comment, BeatLoopSize, CueMicrosec, InPointSeekInfo, \
         OutPointSeekInfo from djmdCue where rb_local_deleted = 0 order by rowid",
    )?;
    let mut out: HashMap<String, Vec<crate::cues::SourceCue>> = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            text(r, 0)?,
            crate::cues::SourceCue {
                kind: int(r, 1)?,
                in_msec: int(r, 2)?,
                in_mpeg_frame: int(r, 3)?,
                in_mpeg_abs: int(r, 4)?,
                out_msec: int(r, 5)?,
                out_mpeg_frame: int(r, 6)?,
                out_mpeg_abs: int(r, 7)?,
                color: int(r, 8)?,
                color_table_index: int(r, 9)?,
                active_loop: int(r, 10)?,
                comment: text(r, 11)?,
                beat_loop_size: int(r, 12)?,
                cue_microsec: int(r, 13)?,
                in_point_seek_info: text(r, 14)?,
                out_point_seek_info: text(r, 15)?,
            },
        ))
    })?;
    for row in rows {
        let (content, cue) = row?;
        out.entry(content).or_default().push(cue);
    }
    Ok(out)
}

/// Paths in master.db that start with `/PIONEER/` are relative to the analysis-data root.
fn under_root(root: &Path, stored: &str) -> PathBuf {
    root.join(stored.trim_start_matches('/'))
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

impl SourceLibrary {
    /// Reads what an export of `selection` needs. `analysis_root` is the directory the
    /// `/PIONEER/...` paths in master.db are relative to (options.json
    /// `analysis-data-root-path`, or a fixture's `rekordbox/share`).
    pub fn load(
        master: &Connection,
        analysis_root: &Path,
        selection: &[SelectionItem],
    ) -> Result<Self> {
        let dbid = master.query_row("select DBID from djmdProperty", [], |r| int(r, 0))? as u32;
        let cloud: Option<i64> = master
            .query_row(
                "select ID from djmdCloudProperty where Reserved1 = 'SyncService' \
                 and rb_local_deleted = 0",
                [],
                |r| int(r, 0),
            )
            .ok();
        let my_tag_master_db_id = cloud
            .filter(|id| *id != 0)
            .map_or(dbid.wrapping_add(1), |id| id as u32);

        let mut lib = SourceLibrary {
            dbid,
            my_tag_master_db_id,
            menus: load_menus(master)?,
            my_tags: load_my_tags(master)?,
            playlists: load_playlists(master)?,
            ..Default::default()
        };

        // Lists to export (folders expanded), then their entries.
        let mut needed_lists = Vec::new();
        let mut needed_tracks: Vec<String> = Vec::new();
        for item in selection {
            match item {
                SelectionItem::Playlist(id) => {
                    if !lib.playlists.iter().any(|p| &p.id == id) {
                        return Err(Error::Selection(format!("no playlist with ID {id}")));
                    }
                    lib.collect_lists(id, &mut needed_lists);
                }
                SelectionItem::Tracks(ids) => needed_tracks.extend(ids.iter().cloned()),
            }
        }
        let mut stmt = master.prepare(
            "select ContentID from djmdSongPlaylist where PlaylistID = ?1 and rb_local_deleted = 0 \
             order by TrackNo",
        )?;
        for list in &needed_lists {
            let ids = stmt
                .query_map([list], |r| text(r, 0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            needed_tracks.extend(ids.iter().cloned());
            lib.entries.insert(list.clone(), ids);
        }

        // Every live track: the selection's tracks get their files checked; all of them are
        // needed to map device rows back (My Tag relink, playlist comparison).
        let mut song_tags: HashMap<String, Vec<u32>> = HashMap::new();
        let mut stmt = master.prepare(
            "select ContentID, MyTagID from djmdSongMyTag where rb_local_deleted = 0 \
             order by ContentID, TrackNo",
        )?;
        for row in stmt.query_map([], |r| Ok((text(r, 0)?, int(r, 1)?)))? {
            let (content, tag) = row?;
            song_tags.entry(content).or_default().push(tag as u32);
        }
        let needed: HashSet<&String> = needed_tracks.iter().collect();
        let mut cues = load_cues(master)?;
        let mut stmt = master.prepare(TRACK_SQL)?;
        let rows = stmt
            .query_map([], track_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for mut t in rows {
            t.my_tags = song_tags.remove(&t.id).unwrap_or_default();
            t.cues = cues.remove(&t.id).unwrap_or_default();
            if needed.contains(&t.id) {
                resolve_files(&mut t, analysis_root);
            }
            lib.by_master_content_id
                .entry(t.device_master_content_id())
                .or_insert_with(|| t.id.clone());
            lib.tracks.insert(t.id.clone(), t);
        }
        for id in &needed_tracks {
            if !lib.tracks.contains_key(id) {
                return Err(Error::Selection(format!("no track with ID {id}")));
            }
        }
        Ok(lib)
    }

    fn collect_lists(&self, id: &str, out: &mut Vec<String>) {
        let Some(node) = self.playlists.iter().find(|p| p.id == id) else {
            return;
        };
        if node.is_folder() {
            for child in self.children(id) {
                self.collect_lists(&child.id, out);
            }
        } else if !out.iter().any(|o| o == id) {
            out.push(id.to_owned());
        }
    }

    /// Children of a playlist folder in `Seq` order.
    pub fn children(&self, id: &str) -> Vec<&SourcePlaylist> {
        let mut v: Vec<&SourcePlaylist> = self
            .playlists
            .iter()
            .filter(|p| p.parent.as_deref() == Some(id))
            .collect();
        v.sort_by_key(|p| p.seq);
        v
    }

    pub fn playlist(&self, id: &str) -> Option<&SourcePlaylist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    /// Finds a playlist by name, or by `Folder/Sub/List` path. Errors when the name is
    /// ambiguous.
    pub fn find_playlist(&self, name: &str) -> Result<&SourcePlaylist> {
        let parts: Vec<&str> = name.split('/').collect();
        let matches: Vec<&SourcePlaylist> = self
            .playlists
            .iter()
            .filter(|p| p.name == *parts.last().unwrap_or(&""))
            .filter(|p| {
                if parts.len() == 1 {
                    return true;
                }
                // Walk up the parents and compare the path from the end.
                let mut node = *p;
                for part in parts.iter().rev().skip(1) {
                    match node.parent.as_deref().and_then(|id| self.playlist(id)) {
                        Some(parent) if parent.name == *part => node = parent,
                        _ => return false,
                    }
                }
                node.parent.is_none()
            })
            .collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => Err(Error::Selection(format!("no playlist named {name:?}"))),
            _ => Err(Error::Selection(format!(
                "{} playlists are named {name:?}; use the Folder/Name path",
                matches.len()
            ))),
        }
    }

    /// The My Tag tree as exportLibrary.db rows (`syncMyTagList`: `sequenceNo = Seq − 1`).
    pub fn devlib_my_tags(&self) -> Vec<MyTagRow> {
        self.my_tags
            .iter()
            .map(|t| MyTagRow {
                id: t.id,
                seq: t.seq - 1,
                name: t.name.clone(),
                attribute: ListAttribute::from_code(t.attribute),
                parent: t.parent,
            })
            .collect()
    }

    /// The My Tag tree as exportExt.pdb rows (same values; `docs/re/pdb.md` § My Tags).
    pub fn pdb_my_tags(&self) -> Vec<mb_pdb::tables::MyTag> {
        self.my_tags
            .iter()
            .map(|t| mb_pdb::tables::MyTag {
                id: t.id,
                seq: (t.seq - 1).max(0) as u32,
                attribute: t.attribute as u8,
                reserved1: 0,
                reserved2: 0,
                parent_id: t.parent,
                reserved3: 0,
                reserved4: 0,
                reserved5: String::new(),
                name: t.name.clone(),
                reserved6: String::new(),
            })
            .collect()
    }
}

fn resolve_files(t: &mut SourceTrack, analysis_root: &Path) {
    let audio = PathBuf::from(&t.folder_path);
    t.audio = file_size(&audio).map(|size| SourceFile { path: audio, size });
    if let Some(stored) = t.analysis.dat_path.take() {
        let dat = under_root(analysis_root, &stored.to_string_lossy());
        t.analysis.dat = dat.is_file();
        t.analysis.ext = dat.with_extension("EXT").is_file();
        t.analysis.ex2 = dat.with_extension("2EX").is_file();
        t.analysis.dat_path = Some(dat);
    }
    if !t.image_path.is_empty() {
        let base = under_root(analysis_root, &t.image_path);
        let variant = |suffix: &str| {
            let p = PathBuf::from(crate::naming::artwork_variant(
                &base.to_string_lossy(),
                suffix,
            ));
            p.is_file().then_some(p)
        };
        t.artwork = Some(LocalArtwork {
            base_exists: base.is_file(),
            small: variant("_s"),
            medium: variant("_m"),
            large: variant("_l"),
            base,
        });
    }
}

fn load_menus(master: &Connection) -> Result<SourceMenus> {
    let mut stmt = master.prepare(
        "select ID, ColorCode, SortKey, Commnt from djmdColor where rb_local_deleted = 0 \
         order by cast(ID as integer)",
    )?;
    let colors = stmt
        .query_map([], |r| {
            Ok((
                int(r, 0)? as u32,
                int(r, 1)? as u32,
                int(r, 2)? as u32,
                text(r, 3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt =
        master.prepare("select ID, Class, Name from djmdMenuItems where rb_local_deleted = 0")?;
    let menu_items = stmt
        .query_map([], |r| {
            Ok(MasterMenuItem {
                id: int(r, 0)? as u32,
                class: int(r, 1)?,
                name: text(r, 2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let setting = |r: &Row| -> rusqlite::Result<MasterMenuSetting> {
        Ok(MasterMenuSetting {
            id: int(r, 0)? as u32,
            menu_item_id: int(r, 1)? as u32,
            seq: int(r, 2)?,
            disable: int(r, 3)?,
        })
    };
    let mut stmt = master.prepare(
        "select ID, MenuItemID, Seq, Disable, InfoOrder from djmdCategory \
         where rb_local_deleted = 0 order by Seq",
    )?;
    let categories = stmt
        .query_map([], |r| Ok((setting(r)?, int(r, 4)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = master.prepare(
        "select ID, MenuItemID, Seq, Disable from djmdSort where rb_local_deleted = 0 order by Seq",
    )?;
    let sorts = stmt
        .query_map([], setting)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(SourceMenus {
        colors,
        menu_items,
        categories,
        sorts,
    })
}

/// `syncMyTagList`'s input: each group (`ParentID = 'root'`, by `Seq`) followed by its tags.
fn load_my_tags(master: &Connection) -> Result<Vec<SourceMyTag>> {
    let mut stmt = master.prepare(
        "select ID, Seq, Name, Attribute, ParentID from djmdMyTag where rb_local_deleted = 0 \
         order by Seq",
    )?;
    let all = stmt
        .query_map([], |r| {
            Ok((
                int(r, 0)? as u32,
                int(r, 1)?,
                text(r, 2)?,
                int(r, 3)?,
                text(r, 4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let node = |(id, seq, name, attr, parent): &(u32, i64, String, i64, String)| SourceMyTag {
        id: *id,
        seq: *seq,
        name: name.clone(),
        attribute: *attr,
        parent: parent.parse().unwrap_or(0),
    };
    let mut out = Vec::new();
    for group in all.iter().filter(|t| t.4 == "root") {
        out.push(node(group));
        let gid = group.0.to_string();
        out.extend(all.iter().filter(|t| t.4 == gid).map(node));
    }
    Ok(out)
}

fn load_playlists(master: &Connection) -> Result<Vec<SourcePlaylist>> {
    let mut stmt = master.prepare(
        "select ID, ParentID, Name, Attribute, Seq from djmdPlaylist where rb_local_deleted = 0",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let parent = text(r, 1)?;
            Ok(SourcePlaylist {
                id: text(r, 0)?,
                parent: Some(parent).filter(|p| !p.is_empty() && p != "root"),
                name: text(r, 2)?,
                attribute: int(r, 3)?,
                seq: int(r, 4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
