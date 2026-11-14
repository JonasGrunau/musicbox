//! The device side of an export: what is already on the stick, read once before planning.
//! Spec: `docs/re/export.md` § 1.4 (name clashes), § 1.6 (ANLZ numbering and stale sets), § 2
//! (IDs), § 4.1 (tracks already exported), § 6.2 (`/PIONEER` vs `/.PIONEER`).
//!
//! Nothing here writes: export.pdb/exportExt.pdb are read as bytes, exportLibrary.db is opened
//! with `immutable=1` (no `-wal`/`-shm` side files).

use crate::naming::{anlz_number, path_key, pioneer_dir};
use crate::{Error, Result};
use mb_pdb::{ExportDb, ExtDb, Pdb};
use rusqlite::types::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// export.pdb and exportExt.pdb as found on the device.
#[derive(Debug, Clone)]
pub struct PdbFiles {
    pub export_bytes: Vec<u8>,
    pub export: ExportDb,
    pub ext_bytes: Option<Vec<u8>>,
    pub ext: Option<ExtDb>,
}

/// One `content` row of exportLibrary.db, with the columns the export compares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevlibTrack {
    pub id: u32,
    pub path: String,
    pub analysis_path: String,
    pub master_db_id: u32,
    pub master_content_id: u32,
    pub length: i64,
    pub bitrate: i64,
    pub bit_depth: i64,
    pub file_type: i64,
    pub file_size: i64,
    pub sample_rate: i64,
    /// `informationUpdateCount`, `analysisDataUpdateCount`, `cueUpdateCount` as text (NULL → "").
    pub info_marker: String,
    pub analysis_marker: String,
    pub cue_marker: String,
    pub content_link: u32,
    pub image_id: u32,
    pub meta: DevlibMeta,
}

/// The metadata columns of a `content` row that a re-export may rewrite, with the lookup IDs
/// resolved to names. Numbers are as stored (NULL → 0).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevlibMeta {
    pub title: String,
    pub subtitle: String,
    pub bpm_x100: i64,
    pub track_no: i64,
    pub disc_no: i64,
    pub color_id: i64,
    pub comment: String,
    pub rating: i64,
    pub release_year: i64,
    pub release_date: String,
    pub play_count: i64,
    pub hot_cue_auto_load: i64,
    pub kuvo_deliver: i64,
    pub kuvo_comment: String,
    pub artist: String,
    pub remixer: String,
    pub original_artist: String,
    pub composer: String,
    pub lyricist: String,
    pub genre: String,
    pub label: String,
    pub key: String,
    pub album: String,
    pub album_artist: String,
    pub compilation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevlibAlbum {
    pub id: u32,
    pub name: String,
    pub artist_id: u32,
    pub compilation: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevlibList {
    pub id: u32,
    /// 0 = top level.
    pub parent: u32,
    pub name: String,
    pub seq: i64,
    /// 0 playlist, 1 folder.
    pub attribute: i64,
}

/// exportLibrary.db as found on the device.
#[derive(Debug, Clone, Default)]
pub struct DevlibState {
    pub tracks: Vec<DevlibTrack>,
    /// (id, name) in ID order, per lookup table.
    pub artists: Vec<(u32, String)>,
    pub genres: Vec<(u32, String)>,
    pub labels: Vec<(u32, String)>,
    pub keys: Vec<(u32, String)>,
    pub albums: Vec<DevlibAlbum>,
    pub max_image_id: u32,
    /// `image` rows: ID → path.
    pub images: HashMap<u32, String>,
    pub playlists: Vec<DevlibList>,
    /// Playlist ID → content IDs in `sequenceNo` order.
    pub entries: HashMap<u32, Vec<u32>>,
    pub created_date: String,
    /// `property.myTagMasterDBID`: whose My Tag tree the stick holds (0 = none yet).
    pub my_tag_master_db_id: u32,
}

/// One file of an ANLZ folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnlzEntry {
    pub number: u32,
    /// Device path, e.g. `/PIONEER/USBANLZ/P046/00014454/ANLZ0000.DAT`.
    pub file: String,
    /// PPTH of a `.DAT` file (the track path it belongs to).
    pub ppth: Option<String>,
}

/// What is on the device before the export.
#[derive(Debug, Clone, Default)]
pub struct DeviceState {
    pub root: PathBuf,
    /// Use `/.PIONEER` (HFS volumes) instead of `/PIONEER`.
    pub hidden: bool,
    /// `statfs` file system type name (`msdos`, `exfat`, `hfs`, `apfs`, …).
    pub filesystem: Option<String>,
    pub pdb: Option<PdbFiles>,
    pub devlib: Option<DevlibState>,
    /// [`path_key`]s of every file below `/Contents`.
    pub contents: HashSet<String>,
    /// ANLZ folder (path key of `/PIONEER/USBANLZ/Pxxx/xxxxxxxx`) → its files.
    pub anlz: HashMap<String, Vec<AnlzEntry>>,
    /// [`path_key`]s of every file below `/PIONEER/Artwork`.
    pub artwork: HashSet<String>,
}

impl DeviceState {
    /// The library whose My Tag tree the stick holds, as `syncMyTag` @0x100c51eec reads it from
    /// the device's database (vtable +0x508): exportExt.pdb `DJDBEXDBPROPERTYOPTION`, else the
    /// exportLibrary.db `property` row. 0 = none written yet (or no database).
    pub fn my_tag_master_db_id(&self) -> u32 {
        let classic = self
            .pdb
            .as_ref()
            .and_then(|p| p.ext.as_ref())
            .and_then(|e| e.db_property_options.first())
            .map(|o| o.my_tag_master_db_id);
        classic
            .or_else(|| self.devlib.as_ref().map(|d| d.my_tag_master_db_id))
            .unwrap_or(0)
    }

    /// An empty device (nothing exported yet) at `root`, with the `PIONEER` folder.
    pub fn empty(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            ..Default::default()
        }
    }

    /// Reads the device at `root`. `hidden` forces `/.PIONEER` (`Some(true)`) or `/PIONEER`
    /// (`Some(false)`); `None` decides like rekordbox: `.PIONEER` on HFS volumes only.
    pub fn load(root: &Path, hidden: Option<bool>) -> Result<Self> {
        if !root.is_dir() {
            return Err(Error::Device(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        let filesystem = filesystem_type(root);
        let hidden = hidden.unwrap_or_else(|| filesystem.as_deref() == Some("hfs"));
        let pioneer = root.join(pioneer_dir(hidden));
        let rb = pioneer.join("rekordbox");
        let mut state = Self {
            root: root.to_path_buf(),
            hidden,
            filesystem,
            ..Default::default()
        };
        let art_prefix = format!("/{}/Artwork", pioneer_dir(hidden));
        let anlz_prefix = format!("/{}/USBANLZ", pioneer_dir(hidden));
        // The databases and the three folder walks are independent and run at the same time: on
        // FSKit FAT every directory read and file open is a round trip to the file-system
        // process, and concurrent requests overlap.
        let (dbs, contents, artwork, anlz) = std::thread::scope(|s| {
            let contents = s.spawn(|| file_keys(&root.join("Contents"), "/Contents"));
            let artwork = s.spawn(|| file_keys(&pioneer.join("Artwork"), &art_prefix));
            let anlz = s.spawn(|| load_anlz(&pioneer.join("USBANLZ"), &anlz_prefix));
            let dbs = load_databases(&rb);
            (dbs, join(contents), join(artwork), join(anlz))
        });
        (state.pdb, state.devlib) = dbs?;
        state.contents = contents?;
        state.artwork = artwork?;
        state.anlz = anlz?;
        Ok(state)
    }

    /// The `PIONEER` folder on this device.
    pub fn pioneer_path(&self) -> PathBuf {
        self.root.join(pioneer_dir(self.hidden))
    }
}

/// export.pdb, exportExt.pdb and exportLibrary.db from the `rekordbox` folder, when present.
fn load_databases(rb: &Path) -> Result<(Option<PdbFiles>, Option<DevlibState>)> {
    let (mut pdb, mut devlib) = (None, None);
    let export_path = rb.join("export.pdb");
    if export_path.is_file() {
        let export_bytes = read(&export_path)?;
        let export = ExportDb::read(&Pdb::parse(export_bytes.clone())?)?;
        let ext_path = rb.join("exportExt.pdb");
        let (ext_bytes, ext) = if ext_path.is_file() {
            let b = read(&ext_path)?;
            let e = ExtDb::read(&Pdb::parse(b.clone())?)?;
            (Some(b), Some(e))
        } else {
            (None, None)
        };
        pdb = Some(PdbFiles {
            export_bytes,
            export,
            ext_bytes,
            ext,
        });
    }
    let devlib_path = rb.join("exportLibrary.db");
    if devlib_path.is_file() {
        for side in ["-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{side}", devlib_path.display()));
            if p.exists() {
                return Err(Error::Device(format!(
                    "{} exists: exportLibrary.db was not closed cleanly",
                    p.display()
                )));
            }
        }
        devlib = Some(load_devlib(&devlib_path)?);
    }
    Ok((pdb, devlib))
}

fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))
}

/// [`path_key`]s of every file below `dir`.
fn file_keys(dir: &Path, prefix: &str) -> Result<HashSet<String>> {
    let mut keys = HashSet::new();
    walk_files(dir, prefix, &mut |dev, _| {
        keys.insert(path_key(dev));
    })?;
    Ok(keys)
}

/// Threads reading the `USBANLZ` folders.
const LOAD_THREADS: usize = 8;

/// The ANLZ files below `USBANLZ` (`dir`) by folder, with the PPTH of every `.DAT`: one file
/// read per track, the slow part of loading a big stick. The `Pxxx` folders are shared out to
/// [`LOAD_THREADS`] threads; the files of each folder keep their directory order.
fn load_anlz(dir: &Path, prefix: &str) -> Result<HashMap<String, Vec<AnlzEntry>>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(Error::io(dir, e)),
    };
    let mut anlz: HashMap<String, Vec<AnlzEntry>> = HashMap::new();
    let mut add = |dev: &str, fs_path: &Path| {
        if let Some((folder, entry)) = anlz_entry(dev, fs_path) {
            anlz.entry(folder).or_default().push(entry);
        }
    };
    let mut folders = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let ty = entry.file_type().map_err(|e| Error::io(&path, e))?;
        let dev = format!("{prefix}/{name}");
        if ty.is_dir() {
            folders.push((path, dev));
        } else if ty.is_file() {
            add(&dev, &path);
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let found = std::thread::scope(|s| {
        let threads: Vec<_> = (0..LOAD_THREADS.min(folders.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut found = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((path, dev)) = folders.get(i) else {
                            return Ok(found);
                        };
                        walk_files(path, dev, &mut |dev, fs_path| {
                            found.extend(anlz_entry(dev, fs_path));
                        })?;
                    }
                })
            })
            .collect();
        threads.into_iter().map(join).collect::<Result<Vec<_>>>()
    })?;
    for (folder, entry) in found.into_iter().flatten() {
        anlz.entry(folder).or_default().push(entry);
    }
    Ok(anlz)
}

/// One file below `USBANLZ`: its folder's [`path_key`] and entry (`None`: not an ANLZ file).
fn anlz_entry(dev: &str, fs_path: &Path) -> Option<(String, AnlzEntry)> {
    let (folder, name) = dev.rsplit_once('/')?;
    let number = anlz_number(name)?;
    let ppth = name
        .to_ascii_uppercase()
        .ends_with(".DAT")
        .then(|| read_ppth(fs_path))
        .flatten();
    Some((
        path_key(folder),
        AnlzEntry {
            number,
            file: dev.to_owned(),
            ppth,
        },
    ))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| Error::io(path, e))
}

fn read_ppth(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    mb_anlz::AnlzFile::parse(&bytes).ok()?.path()
}

/// Calls `f(device_path, fs_path)` for every regular file below `dir` (recursively); device
/// paths are `prefix` + the relative path with `/` separators. A missing `dir` is empty.
fn walk_files(dir: &Path, prefix: &str, f: &mut dyn FnMut(&str, &Path)) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::io(dir, e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let ty = entry.file_type().map_err(|e| Error::io(&path, e))?;
        let dev = format!("{prefix}/{name}");
        if ty.is_dir() {
            walk_files(&path, &dev, f)?;
        } else if ty.is_file() {
            f(&dev, &path);
        }
    }
    Ok(())
}

/// `statfs` file system type of the volume holding `path` (macOS `f_fstypename`).
pub fn filesystem_type(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a writable statfs buffer.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        let name: Vec<u8> = st
            .f_fstypename
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as u8)
            .collect();
        Some(String::from_utf8_lossy(&name).into_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(format!("{:#x}", st.f_type))
    }
}

/// Free bytes on the volume holding `path` (`File::getBytesFreeOnVolume`).
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a writable statvfs buffer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

fn as_text(v: Value) -> String {
    match v {
        Value::Null | Value::Blob(_) => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => r.to_string(),
        Value::Text(t) => t,
    }
}

fn as_int(v: Value) -> i64 {
    match v {
        Value::Integer(i) => i,
        Value::Real(r) => r as i64,
        Value::Text(t) => t.trim().parse().unwrap_or(0),
        Value::Null | Value::Blob(_) => 0,
    }
}

fn load_devlib(path: &Path) -> Result<DevlibState> {
    let db = mb_devlib::DevLibDb::open_immutable(path)?;
    load_devlib_from(db.connection())
}

/// [`DevlibState`] from an open exportLibrary.db (or an in-memory copy of its tables, as the
/// Device Library Plus conversion builds for planning, [`crate::convert`]).
pub(crate) fn load_devlib_from(c: &rusqlite::Connection) -> Result<DevlibState> {
    let mut state = DevlibState::default();
    let mut stmt = c.prepare(
        "select c.content_id, c.path, c.analysisDataFilePath, c.masterDbId, c.masterContentId, \
         c.length, c.bitrate, c.bitDepth, c.fileType, c.fileSize, c.samplingRate, \
         c.informationUpdateCount, c.analysisDataUpdateCount, c.cueUpdateCount, c.contentLink, \
         c.image_id, c.title, c.subtitle, c.bpmx100, c.trackNo, c.discNo, c.color_id, \
         c.djComment, c.rating, c.releaseYear, c.releaseDate, c.djPlayCount, \
         c.isHotCueAutoLoadOn, c.isKuvoDeliverStatusOn, c.kuvoDeliveryComment, ar.name, \
         rm.name, oa.name, co.name, ly.name, ge.name, la.name, k.name, al.name, aa.name, \
         al.isComplation \
         from content c \
         left join artist ar on ar.artist_id = c.artist_id_artist \
         left join artist rm on rm.artist_id = c.artist_id_remixer \
         left join artist oa on oa.artist_id = c.artist_id_originalArtist \
         left join artist co on co.artist_id = c.artist_id_composer \
         left join artist ly on ly.artist_id = c.artist_id_lyricist \
         left join genre ge on ge.genre_id = c.genre_id \
         left join label la on la.label_id = c.label_id \
         left join key k on k.key_id = c.key_id \
         left join album al on al.album_id = c.album_id \
         left join artist aa on aa.artist_id = al.artist_id \
         order by c.content_id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let v = |i: usize| r.get::<_, Value>(i);
        let meta = DevlibMeta {
            title: as_text(v(16)?),
            subtitle: as_text(v(17)?),
            bpm_x100: as_int(v(18)?),
            track_no: as_int(v(19)?),
            disc_no: as_int(v(20)?),
            color_id: as_int(v(21)?),
            comment: as_text(v(22)?),
            rating: as_int(v(23)?),
            release_year: as_int(v(24)?),
            release_date: as_text(v(25)?),
            play_count: as_int(v(26)?),
            hot_cue_auto_load: as_int(v(27)?),
            kuvo_deliver: as_int(v(28)?),
            kuvo_comment: as_text(v(29)?),
            artist: as_text(v(30)?),
            remixer: as_text(v(31)?),
            original_artist: as_text(v(32)?),
            composer: as_text(v(33)?),
            lyricist: as_text(v(34)?),
            genre: as_text(v(35)?),
            label: as_text(v(36)?),
            key: as_text(v(37)?),
            album: as_text(v(38)?),
            album_artist: as_text(v(39)?),
            compilation: as_int(v(40)?),
        };
        state.tracks.push(DevlibTrack {
            meta,
            id: as_int(v(0)?) as u32,
            path: as_text(v(1)?),
            analysis_path: as_text(v(2)?),
            master_db_id: as_int(v(3)?) as u32,
            master_content_id: as_int(v(4)?) as u32,
            length: as_int(v(5)?),
            bitrate: as_int(v(6)?),
            bit_depth: as_int(v(7)?),
            file_type: as_int(v(8)?),
            file_size: as_int(v(9)?),
            sample_rate: as_int(v(10)?),
            info_marker: as_text(v(11)?),
            analysis_marker: as_text(v(12)?),
            cue_marker: as_text(v(13)?),
            content_link: as_int(v(14)?) as u32,
            image_id: as_int(v(15)?) as u32,
        });
    }
    let names = |table: &str| -> Result<Vec<(u32, String)>> {
        let mut stmt = c.prepare(&format!(
            "select {table}_id, name from {table} order by {table}_id"
        ))?;
        let rows = stmt
            .query_map([], |r| Ok((as_int(r.get(0)?) as u32, as_text(r.get(1)?))))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    };
    state.artists = names("artist")?;
    state.genres = names("genre")?;
    state.labels = names("label")?;
    state.keys = names("key")?;
    let mut stmt =
        c.prepare("select album_id, name, artist_id, isComplation from album order by album_id")?;
    state.albums = stmt
        .query_map([], |r| {
            Ok(DevlibAlbum {
                id: as_int(r.get(0)?) as u32,
                name: as_text(r.get(1)?),
                artist_id: as_int(r.get(2)?) as u32,
                compilation: as_int(r.get(3)?) as u8,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    state.max_image_id = c.query_row("select max(image_id) from image", [], |r| {
        Ok(as_int(r.get(0)?) as u32)
    })?;
    let mut stmt = c.prepare("select image_id, path from image")?;
    state.images = stmt
        .query_map([], |r| Ok((as_int(r.get(0)?) as u32, as_text(r.get(1)?))))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = c.prepare(
        "select playlist_id, playlist_id_parent, name, sequenceNo, attribute from playlist \
         order by playlist_id",
    )?;
    state.playlists = stmt
        .query_map([], |r| {
            Ok(DevlibList {
                id: as_int(r.get(0)?) as u32,
                parent: as_int(r.get(1)?) as u32,
                name: as_text(r.get(2)?),
                seq: as_int(r.get(3)?),
                attribute: as_int(r.get(4)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = c.prepare(
        "select playlist_id, content_id from playlist_content order by playlist_id, sequenceNo",
    )?;
    for row in stmt.query_map([], |r| {
        Ok((as_int(r.get(0)?) as u32, as_int(r.get(1)?) as u32))
    })? {
        let (pl, content) = row?;
        state.entries.entry(pl).or_default().push(content);
    }
    state.created_date = c
        .query_row("select createdDate from property", [], |r| {
            Ok(as_text(r.get(0)?))
        })
        .unwrap_or_default();
    state.my_tag_master_db_id = c
        .query_row("select myTagMasterDBID from property", [], |r| {
            Ok(as_int(r.get(0)?) as u32)
        })
        .unwrap_or_default();
    Ok(state)
}
