//! Op: import tracks (and a playlist) from a device into the collection. Spec:
//! `docs/re/ops/import-device.md` (binary-derived, no trace).
//!
//! Per device track, in selection order (`importFromDeviceManager::start` @0x1001dc470,
//! `importFromDeviceThread::run` @0x1001d9618):
//! 1. **Already in the collection?** A live row whose `FolderPath` is the device file's path
//!    (`isCollectionSong`), or, when the device row's `masterDbId` is this library's DBID, the
//!    live row whose `ID` is its `masterContentId` (`getMainTrackByDeviceTrack` @0x10052d18c).
//!    Such tracks are not imported again; their local ID is used.
//! 2. **Copy the audio** to `<copy folder>/<artist>/<album>/<file>`, named like the USB export
//!    (`calc_device_file_path`, `export.md` § 1.3–1.4; default folder
//!    `~/Music/PioneerDJ/Imported from Device`).
//! 3. **Insert the row** (`addTrackFromDevice` → `rekordboxDBController::addTrack` →
//!    `AppSyncDBController::insertTrack`): the import-track builder insert, filled from the
//!    device row. The device's analysis and artwork paths and master IDs are cleared first, so
//!    `MasterDBID` = this DBID and `MasterSongID` = the new ID; `ContentLink` =
//!    `(device & 0x26000) | 0xE`.
//! 4. **Analysis files** (`import_device_analysis` @0x1001db460): device `.DAT`/`.EXT` →
//!    [`mb_anlz::local::to_local`], `.2EX` copied unchanged, into the local folder of the new
//!    content UUID; then `AnalysisDataPath` and `Analysed = (device & 0xC060020) | 0x49`.
//! 5. **Cues** from the device analysis files go to `djmdCue` (`getDeviceContentCueInfo` →
//!    `updateCue`; hot cue bank cues, kind 101, are skipped).
//! 6. **Artwork** (`add_track_artwork` @0x1001dbf44): the device image becomes the local
//!    `artwork.jpg` / `_m` / `_s` set and `ImagePath`.
//! 7. **Device row** (`updateMasterDBInfo`, vtable +0x428 on the device controller): rekordbox
//!    stamps the new master IDs onto the stick's row. That write goes to the stick, so it is
//!    returned in the report ([`DeviceImportResult::stamp`]) for the caller to apply.
//!
//! With a playlist name, a playlist is created on top of the root and gets every selected
//! track, imported or already present, in device order (`importDevicePlaylist` @0x101a10670).

use crate::lookup::{NameTable, find_or_create, find_or_create_album};
use crate::{Command, DbId, Effect, Library, NewCue, Result, cues};
use mb_anlz::AnlzFile;
use mb_anlz::tags::{CueKind, CueType};
use mb_masterdb::WriteTxn;
use mb_masterdb::rusqlite::OptionalExtension;
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::write::{date_today, new_content_id, new_uuid};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One track as a device stores it (export.pdb row with its lookup names resolved).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceTrack {
    /// The track's ID in the device database.
    pub device_id: u32,
    /// Absolute path of the audio file on the mounted device.
    pub audio: PathBuf,
    /// Absolute path of the device `ANLZ0000.DAT`, if any.
    pub analysis_dat: Option<PathBuf>,
    /// Absolute path of the device artwork (`a<id>.jpg`), if any.
    pub artwork: Option<PathBuf>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub label: String,
    pub key: String,
    pub remixer: String,
    pub original_artist: String,
    pub composer: String,
    pub lyricist: String,
    pub comment: String,
    pub subtitle: String,
    pub release_date: String,
    pub isrc: String,
    pub delivery_comment: String,
    /// `StockDate` (YYYY-MM-DD).
    pub stock_date: String,
    pub date_created: String,
    pub bpm_x100: u32,
    pub length_s: u16,
    pub track_no: u32,
    pub disc_no: u16,
    pub bitrate: u32,
    pub bit_depth: u16,
    pub sample_rate: u32,
    pub file_size: u32,
    pub file_type: u8,
    pub rating: u8,
    pub color_id: u8,
    pub release_year: u16,
    pub play_count: u16,
    /// Device `Analysed` (export.pdb keeps `& 0x3F`).
    pub analysed: u8,
    pub content_link: u32,
    pub master_db_id: u32,
    pub master_content_id: u32,
}

/// The tracks of an export.pdb, by device ID, with absolute paths below `root`.
pub fn device_tracks(root: &Path, db: &mb_pdb::ExportDb) -> HashMap<u32, DeviceTrack> {
    let named = |rows: Vec<(u32, String)>| rows.into_iter().collect::<HashMap<u32, String>>();
    let artists = named(db.artists.iter().map(|a| (a.id, a.name.clone())).collect());
    let genres = named(db.genres.iter().map(|g| (g.0.id, g.0.name.clone())).collect());
    let labels = named(db.labels.iter().map(|l| (l.0.id, l.0.name.clone())).collect());
    let keys = named(db.keys.iter().map(|k| (k.id, k.name.clone())).collect());
    let albums: HashMap<u32, (String, u32)> =
        db.albums.iter().map(|a| (a.id, (a.name.clone(), a.album_artist_id))).collect();
    let artwork: HashMap<u32, String> = db.artwork.iter().map(|a| (a.id, a.path.clone())).collect();
    let name = |m: &HashMap<u32, String>, id: u32| m.get(&id).cloned().unwrap_or_default();
    let abs = |p: &str| root.join(p.trim_start_matches('/'));
    db.tracks
        .iter()
        .map(|t| {
            let (album, album_artist_id) = albums.get(&t.album_id).cloned().unwrap_or_default();
            let track = DeviceTrack {
                device_id: t.id,
                audio: abs(&t.folder_path),
                analysis_dat: (!t.analysis_path.is_empty()).then(|| abs(&t.analysis_path)),
                artwork: artwork.get(&t.artwork_id).filter(|p| !p.is_empty()).map(|p| abs(p)),
                title: t.title.clone(),
                artist: name(&artists, t.artist_id),
                album,
                album_artist: name(&artists, album_artist_id),
                genre: name(&genres, t.genre_id),
                label: name(&labels, t.label_id),
                key: name(&keys, t.key_id),
                remixer: name(&artists, t.remixer_id),
                original_artist: name(&artists, t.original_artist_id),
                composer: name(&artists, t.composer_id),
                lyricist: t.lyricist.clone(),
                comment: t.comment.clone(),
                subtitle: t.subtitle.clone(),
                release_date: t.release_date.clone(),
                isrc: t.isrc.clone(),
                delivery_comment: t.delivery_comment.clone(),
                stock_date: t.stock_date.clone(),
                date_created: t.date_created.clone(),
                bpm_x100: t.bpm_x100,
                length_s: t.length_s,
                track_no: t.track_no,
                disc_no: t.disc_no,
                bitrate: t.bitrate,
                bit_depth: t.bit_depth,
                sample_rate: t.sample_rate,
                file_size: t.file_size,
                file_type: t.file_type,
                rating: t.rating,
                color_id: t.color_id,
                release_year: t.release_year,
                play_count: t.play_count,
                analysed: t.analysed,
                content_link: t.content_link,
                master_db_id: t.master_db_id,
                master_content_id: t.master_content_id,
            };
            (t.id, track)
        })
        .collect()
}

/// What to import.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceImport {
    pub tracks: Vec<DeviceTrack>,
    /// Folder new audio files are copied into (rekordbox's default:
    /// `~/Music/PioneerDJ/Imported from Device`).
    pub copy_to: PathBuf,
    /// Also create a playlist of all `tracks` with this name (importing a device playlist).
    pub playlist: Option<String>,
}

/// rekordbox's default copy folder: the user's Music folder + `PioneerDJ/Imported from Device`
/// (`importFromDeviceCopySettingWindow` @0x1001d6af0, `getSpecialLocation(userMusicDirectory)`).
pub fn default_copy_folder() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Music/PioneerDJ/Imported from Device"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceImportKind {
    /// A new collection row; the audio was copied to `copied_to`.
    Imported { copied_to: PathBuf },
    /// The track was already in the collection.
    InCollection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceImportResult {
    pub device_id: u32,
    pub local_id: DbId,
    pub kind: DeviceImportKind,
    /// `(masterDbId, masterContentId)` to write onto the device row (`updateMasterDBInfo`), when
    /// they differ from what the device holds.
    pub stamp: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceImportReport {
    pub results: Vec<DeviceImportResult>,
    /// The playlist created for a playlist import.
    pub playlist: Option<DbId>,
    /// Device tracks skipped (audio missing or unreadable), with the reason.
    pub skipped: Vec<(u32, String)>,
    /// Files and folders the import created (copied audio, local analysis and artwork folders).
    pub created_files: Vec<PathBuf>,
}

pub(crate) fn import(lib: &mut Library, req: &DeviceImport) -> Result<(Effect, Option<Command>)> {
    let analysis_root = lib
        .analysis_root
        .clone()
        .ok_or_else(|| crate::Error::Invalid("importing from a device needs the analysis root".into()))?;
    let dbid: u32 = lib
        .connection()
        .query_row("SELECT DBID FROM djmdProperty", [], |r| r.get::<_, Option<String>>(0))?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut report = DeviceImportReport::default();
    for t in &req.tracks {
        if let Some(local) = find_in_collection(lib, t, dbid)? {
            let stamp = (dbid != 0 && (t.master_db_id, t.master_content_id) != (dbid, id_u32(&local)))
                .then(|| (dbid, id_u32(&local)));
            report.results.push(DeviceImportResult {
                device_id: t.device_id,
                local_id: local,
                kind: DeviceImportKind::InCollection,
                stamp,
            });
            continue;
        }
        if !t.audio.is_file() {
            report.skipped.push((t.device_id, format!("{} does not exist", t.audio.display())));
            continue;
        }
        let dest = copy_destination(&req.copy_to, t);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        }
        std::fs::copy(&t.audio, &dest).map_err(|e| io(&dest, e))?;
        report.created_files.push(dest.clone());

        let tx = lib.writer.transaction()?;
        let (id, uuid) = insert_track(&tx, &dest, t)?;
        tx.commit()?;

        // Analysis files, then the two columns (UPDATE-COL each, analyse-track.md macros).
        if let Some(dat) = &t.analysis_dat
            && dat.is_file()
        {
            let file_name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let rel = format!("PIONEER/USBANLZ/{}", mb_anlz::usb::local_anlz_folder(&uuid));
            let dir = analysis_root.join(&rel);
            std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
            report.created_files.push(dir.clone());
            write_local_anlz(dat, &dir, &file_name)?;
            let tx = lib.writer.transaction()?;
            tx.update(
                "djmdContent",
                "ID",
                Value::Text(id.0.clone()),
                &[("AnalysisDataPath", Value::Text(format!("/{rel}/ANLZ0000.DAT")))],
            )?;
            let analysed = (u32::from(t.analysed) & 0x0C06_0020) | 0x49;
            tx.update("djmdContent", "ID", Value::Text(id.0.clone()), &[("Analysed", Value::Integer(i64::from(analysed)))])?;
            tx.commit()?;
            for cue in device_cues(dat) {
                cues::add_cue(lib, &id, &cue)?;
            }
        }
        if let Some(art) = &t.artwork
            && art.is_file()
        {
            let rel = format!("PIONEER/Artwork/{}", mb_anlz::usb::local_anlz_folder(&uuid));
            let dir = analysis_root.join(&rel);
            std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
            report.created_files.push(dir.clone());
            write_local_artwork(art, &dir)?;
            let tx = lib.writer.transaction()?;
            tx.update(
                "djmdContent",
                "ID",
                Value::Text(id.0.clone()),
                &[("ImagePath", Value::Text(format!("/{rel}/artwork.jpg")))],
            )?;
            tx.commit()?;
        }
        let stamp = (dbid != 0 && (t.master_db_id, t.master_content_id) != (dbid, id_u32(&id))).then(|| (dbid, id_u32(&id)));
        report.results.push(DeviceImportResult {
            device_id: t.device_id,
            local_id: id,
            kind: DeviceImportKind::Imported { copied_to: dest },
            stamp,
        });
    }
    if let Some(name) = &req.playlist {
        let (effect, _) = crate::playlist::create(lib, None, name, crate::NodeKind::Playlist)?;
        if let Effect::PlaylistCreated { id } = effect {
            let ids: Vec<DbId> = report.results.iter().map(|r| r.local_id.clone()).collect();
            if !ids.is_empty() {
                crate::playlist::add_tracks(lib, &id, &ids, None)?;
            }
            report.playlist = Some(id);
        }
    }
    let imported: Vec<DbId> = report
        .results
        .iter()
        .filter(|r| matches!(r.kind, DeviceImportKind::Imported { .. }))
        .map(|r| r.local_id.clone())
        .collect();
    let inverse = (!imported.is_empty() || report.playlist.is_some()).then(|| Command::UndoDeviceImport {
        ids: imported,
        playlist: report.playlist.clone(),
        files: report.created_files.clone(),
    });
    Ok((Effect::ImportedFromDevice(report), inverse))
}

/// Inverse of an import: the playlist, the new rows, then the files the import created.
pub(crate) fn undo(lib: &mut Library, ids: &[DbId], playlist: Option<&DbId>, files: &[PathBuf]) -> Result<(Effect, Option<Command>)> {
    if let Some(p) = playlist {
        crate::playlist::delete(lib, p)?;
    }
    if !ids.is_empty() {
        crate::remove::remove_tracks(lib, ids)?;
    }
    for f in files.iter().rev() {
        let _ = if f.is_dir() { std::fs::remove_dir_all(f) } else { std::fs::remove_file(f) };
    }
    Ok((Effect::Removed { ids: ids.to_vec() }, None))
}

fn id_u32(id: &DbId) -> u32 {
    id.0.parse().unwrap_or(0)
}

fn io(path: &Path, source: std::io::Error) -> crate::Error {
    crate::Error::Io { path: path.display().to_string(), source }
}

/// `isCollectionSong` (exact `FolderPath`), then `getMainTrackByDeviceTrack` (this DBID →
/// `masterContentId` is a local `ID`).
fn find_in_collection(lib: &Library, t: &DeviceTrack, dbid: u32) -> Result<Option<DbId>> {
    let conn = lib.connection();
    let by_path: Option<String> = conn
        .query_row(
            "SELECT ID FROM djmdContent WHERE rb_local_deleted = 0 AND FolderPath = ?1",
            [t.audio.to_string_lossy()],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = by_path {
        return Ok(Some(DbId(id)));
    }
    if t.master_db_id != 0 && t.master_content_id != 0 && t.master_db_id == dbid {
        let by_master: Option<String> = conn
            .query_row(
                "SELECT ID FROM djmdContent WHERE rb_local_deleted = 0 AND ID = ?1",
                [t.master_content_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        return Ok(by_master.map(DbId));
    }
    Ok(None)
}

/// `<folder>/<artist>/<album>/<file>` with the USB export's naming rules and `-n` on a clash.
fn copy_destination(folder: &Path, t: &DeviceTrack) -> PathBuf {
    let source_name = t.audio.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = mb_usb::naming::contents_dir(&t.artist, &t.album);
    let rel = dir.trim_start_matches("/Contents/");
    let name = mb_usb::naming::file_component(&source_name);
    let mut candidate = folder.join(rel).join(&name);
    let mut n = 1;
    while candidate.exists() {
        candidate = folder.join(rel).join(mb_usb::naming::clash_candidate(&name, n));
        n += 1;
    }
    candidate
}

fn write_local_anlz(device_dat: &Path, dir: &Path, audio_file_name: &str) -> Result<()> {
    for ext in ["DAT", "EXT", "2EX"] {
        let src = device_dat.with_extension(ext);
        if !src.is_file() {
            continue;
        }
        let dest = dir.join(format!("ANLZ0000.{ext}"));
        let bytes = std::fs::read(&src).map_err(|e| io(&src, e))?;
        let out = if ext == "2EX" {
            bytes
        } else {
            match AnlzFile::parse(&bytes) {
                Ok(f) => mb_anlz::local::to_local(&f, audio_file_name).to_bytes(),
                Err(_) => bytes,
            }
        };
        std::fs::write(&dest, out).map_err(|e| io(&dest, e))?;
    }
    Ok(())
}

/// The local artwork set from the device's 80×80 and 240×240 files (*hypothesis*: rekordbox
/// re-encodes with `writeFileImage`; musicbox copies the bytes, 240 px for `artwork.jpg`/`_m`).
fn write_local_artwork(device_small: &Path, dir: &Path) -> Result<()> {
    let s = device_small.to_string_lossy();
    let medium = PathBuf::from(match s.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}_m.{ext}"),
        None => format!("{s}_m"),
    });
    let big = if medium.is_file() { medium } else { device_small.to_path_buf() };
    for (from, name) in [(&big, "artwork.jpg"), (&big, "artwork_m.jpg"), (&device_small.to_path_buf(), "artwork_s.jpg")] {
        let dest = dir.join(name);
        std::fs::copy(from, &dest).map_err(|e| io(&dest, e))?;
    }
    Ok(())
}

/// Cues in the device analysis files: the `.EXT` lists when present (they carry every hot cue,
/// colours and comments), else the `.DAT` lists. Memory cues first, then hot cues, each in
/// stored order.
pub fn device_cues(device_dat: &Path) -> Vec<NewCue> {
    let parse = |p: PathBuf| std::fs::read(p).ok().and_then(|b| AnlzFile::parse(&b).ok());
    let mut out = Vec::new();
    if let Some(ext) = parse(device_dat.with_extension("EXT")) {
        let lists = ext.cue_lists_ext();
        if lists.iter().any(|l| !l.entries.is_empty()) {
            for kind in [CueKind::Memory, CueKind::Hot] {
                for l in lists.iter().filter(|l| l.kind == kind) {
                    for e in &l.entries {
                        let slot = slot_of(e.hot_cue);
                        let is_loop = e.cue_type == CueType::from_raw(2);
                        let (color, color_table_index) = match slot {
                            crate::CueSlot::Memory => ((e.color_index != 0).then_some(e.color_index), None),
                            crate::CueSlot::Hot(_) => (None, e.color().map(|(code, _)| code).filter(|c| *c != 0)),
                        };
                        out.push(NewCue {
                            slot,
                            in_ms: e.time_ms(),
                            out_ms: if is_loop { e.loop_end_ms() } else { None },
                            color,
                            color_table_index,
                            comment: e.comment.clone().unwrap_or_default(),
                        });
                    }
                }
            }
            return out;
        }
    }
    if let Some(dat) = parse(device_dat.with_extension("DAT")) {
        let lists = dat.cue_lists();
        for kind in [CueKind::Memory, CueKind::Hot] {
            for l in lists.iter().filter(|l| l.kind == kind) {
                for e in &l.entries {
                    let is_loop = e.cue_type == CueType::from_raw(2);
                    out.push(NewCue {
                        slot: slot_of(e.hot_cue),
                        in_ms: e.time_ms(),
                        out_ms: if is_loop { e.loop_end_ms() } else { None },
                        color: None,
                        color_table_index: None,
                        comment: String::new(),
                    });
                }
            }
        }
    }
    out
}

fn slot_of(hot_cue: u16) -> crate::CueSlot {
    match hot_cue {
        0 => crate::CueSlot::Memory,
        n => crate::CueSlot::Hot((n - 1).min(15) as u8),
    }
}

/// The builder insert of `import-track.md`, filled from the device row (`insertTrack`
/// @0x100a56e3c). Lookup order as in the file import, then original artist and composer.
/// Returns the new content ID and UUID.
pub(crate) fn insert_track(tx: &WriteTxn<'_>, path: &Path, t: &DeviceTrack) -> Result<(DbId, String)> {
    let opt = |s: &str, table: NameTable| -> Result<Option<String>> {
        if s.is_empty() { Ok(None) } else { find_or_create(tx, table, s).map(Some) }
    };
    let artist_id = opt(&t.artist, NameTable::Artist)?;
    let genre_id = opt(&t.genre, NameTable::Genre)?;
    let album_id = if t.album.is_empty() {
        None
    } else {
        let album_artist_id = opt(&t.album_artist, NameTable::Artist)?;
        Some(find_or_create_album(tx, &t.album, album_artist_id.as_deref())?)
    };
    let remixer_id = opt(&t.remixer, NameTable::Artist)?;
    let org_artist_id = opt(&t.original_artist, NameTable::Artist)?;
    let composer_id = opt(&t.composer, NameTable::Artist)?;
    let label_id = opt(&t.label, NameTable::Label)?;
    let key_id = opt(&t.key, NameTable::Key)?.unwrap_or_else(|| "0".to_owned());

    let (dbid, device_id): (String, String) = tx.connection().query_row(
        "SELECT DBID, DeviceID FROM djmdProperty",
        [],
        |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default())),
    )?;
    let id = loop {
        let id = new_content_id()?.to_string();
        if !crate::lookup::any_row_with_id(tx, "djmdContent", &id)? {
            break id;
        }
    };
    let uuid = new_uuid();
    let today = date_today();
    let or_today = |s: &str| if s.is_empty() { today.clone() } else { s.to_owned() };
    let file_name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let inode = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).map(|m| m.ino()).unwrap_or(0)
    };
    let text = |s: &str| Value::Text(s.to_owned());
    let int = |v: i64| Value::Integer(v);
    let mut cols: Vec<(&str, Value)> = vec![
        ("ID", text(&id)),
        ("UUID", text(&uuid)),
        ("FolderPath", text(&path.to_string_lossy())),
        ("FileNameL", text(&file_name)),
        ("FileNameS", text("")),
        ("Title", text(&t.title)),
    ];
    if let Some(a) = &artist_id {
        cols.push(("ArtistID", text(a)));
    }
    if let Some(a) = &album_id {
        cols.push(("AlbumID", text(a)));
    }
    if let Some(g) = &genre_id {
        cols.push(("GenreID", text(g)));
    }
    cols.extend([
        ("BPM", int(i64::from(t.bpm_x100))),
        ("Length", int(i64::from(t.length_s))),
        ("TrackNo", int(i64::from(t.track_no))),
        ("BitRate", int(i64::from(t.bitrate))),
        ("BitDepth", int(i64::from(t.bit_depth))),
        ("Commnt", text(&t.comment)),
        ("FileType", int(i64::from(t.file_type))),
        ("Rating", int(i64::from(t.rating))),
        ("ReleaseYear", int(i64::from(t.release_year))),
    ]);
    if let Some(r) = &remixer_id {
        cols.push(("RemixerID", text(r)));
    }
    if let Some(l) = &label_id {
        cols.push(("LabelID", text(l)));
    }
    if let Some(o) = &org_artist_id {
        cols.push(("OrgArtistID", text(o)));
    }
    cols.extend([
        ("KeyID", text(&key_id)),
        ("StockDate", text(&or_today(&t.stock_date))),
        ("ColorID", text(&t.color_id.to_string())),
        ("DJPlayCount", int(i64::from(t.play_count))),
        ("ImagePath", text("")),
        ("MasterDBID", text(&dbid)),
        ("MasterSongID", text(&id)),
        ("AnalysisDataPath", text("")),
        ("FileSize", int(i64::from(t.file_size))),
        ("DiscNo", int(i64::from(t.disc_no))),
    ]);
    if let Some(c) = &composer_id {
        cols.push(("ComposerID", text(c)));
    }
    cols.extend([
        ("Subtitle", text(&t.subtitle)),
        ("SampleRate", int(i64::from(t.sample_rate))),
        ("Analysed", int(0)),
        ("ReleaseDate", text(&t.release_date)),
        ("DateCreated", text(&or_today(&t.date_created))),
        ("ContentLink", int(i64::from((t.content_link & 0x26000) | 0xE))),
        ("ModifiedByRBM", text("")),
        ("HotCueAutoLoad", text("on")),
        ("DeliveryControl", text("on")),
        ("DeliveryComment", text(&t.delivery_comment)),
        ("Lyricist", text(&t.lyricist)),
        ("ISRC", text(&t.isrc)),
        ("SamplerTrackInfo", int(0)),
        ("SamplerPlayOffset", int(0)),
        ("SamplerGain", int(0)),
        ("rb_file_id", int(inode as i64)),
        ("VideoAssociate", int(0)),
        ("LyricStatus", int(0)),
        ("ServiceID", int(0)),
        ("OrgFolderPath", text("")),
        ("ExtInfo", text("null")),
        ("DeviceID", text(&device_id)),
        ("Reserved1", text("")),
    ]);
    tx.insert("djmdContent", &cols)?;
    Ok((DbId(id), uuid))
}
