//! rekordbox.xml: "File → Export Collection in xml format" and the import from the
//! "rekordbox xml" tree. Specs: `docs/re/xml.md` (format, export), `docs/re/ops/import-xml.md`
//! (import; binary-derived, no trace).
//!
//! **Export** ([`export_document`], `db::DjPlaylistsWriter` @0x101054f0c): every live track in
//! `djmdContent` row order with its beat grid reduced to TEMPO entries, its cues as
//! POSITION_MARKs (the rows after `set_cueInfo` + `checkCueInfos`, hot-cue colours through
//! `getHotCueDefaultColor`), then the playlist tree. Byte layout and value rules are `mb_xml`'s.
//!
//! **Import** ([`Command::ImportXml`], `ListViewer::importTrackFromBridge` →
//! `DatabaseMediator::importTrack` → `ImportManager::run` → `addTrack` + `importRBXmlData`):
//! 1. A track whose file is already in the collection (`isCollectionSong`, same `FolderPath`) is
//!    not imported again. rekordbox asks whether to "load information in the tag of the library
//!    being imported"; with [`XmlImport::update_existing`] the track gets the XML's non-empty,
//!    differing values (`updateCollectionFromXml` @0x100c5a590), its beat grid and its cues.
//! 2. A new track is inserted in place (the file is not copied) with the XML's metadata; album
//!    artist, original artist, lyricist, ISRC and release date come from the file's tags, which
//!    the XML lacks (`addTrack` @0x101a1577c, source type 4).
//! 3. TEMPO entries → a per-beat grid ([`mb_xml::import::beats_from_tempos`]) → a local
//!    `ANLZ0000.DAT` with that PQTZ, then `AnalysisDataPath`, `BPM`, `Analysed` (bit 0x10) and
//!    `ContentLink` (`importRBXmlData` @0x101a161f0).
//! 4. POSITION_MARKs → cues ([`mb_xml::import::cues_from_marks`]), each through
//!    `rekordboxDBController::setCue` (vtable +0x448).
//! 5. Lists are created under the target (a folder tree is created with its children) and get
//!    their tracks in XML order (`ImportManager::run`: `addPlaylistInternal`,
//!    `addTrackToPlaylistInternal`).

use crate::device_import::{DeviceTrack, insert_track};
use crate::{Command, DbId, Effect, Error, Library, NewCue, Result, cues};
use mb_anlz::tags::{Beat, BeatGrid, CueKind, CueList, Path as PathTag, TypedSection, Vbr, WavePreview};
use mb_anlz::{AnlzFile, FileHeader, Tag};
use mb_core::LibrarySnapshot;
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::rusqlite::{Connection, OptionalExtension};
use mb_xml::export::{self, GridBeat, KindLanguage, TempoMode};
use mb_xml::import as ximport;
use mb_xml::{Document, PositionMark, Product};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---- export ------------------------------------------------------------------------------------

/// What the writer needs besides the library.
#[derive(Debug, Clone, PartialEq)]
pub struct XmlExportOptions {
    /// `PRODUCT` attributes. rekordbox writes its own name and version; its import never reads
    /// them (`analyzeDjPlaylists` only looks at COLLECTION and PLAYLISTS), so musicbox writes its
    /// own by default.
    pub product: Product,
    /// The UI language rekordbox would write `Kind` labels in (preference `language`).
    pub language: KindLanguage,
    /// Preference `HotCueColorType`: default colours of hot cues without a colour code.
    pub hot_cue_color_type: u8,
    /// Preference `bridgeExportBeatGrid`.
    pub tempo_mode: TempoMode,
}

impl Default for XmlExportOptions {
    fn default() -> Self {
        Self {
            product: Product {
                name: Some("musicbox".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                company: Some("musicbox".into()),
                extra: Default::default(),
            },
            language: KindLanguage::English,
            hot_cue_color_type: 4,
            tempo_mode: TempoMode::Changes,
        }
    }
}

impl XmlExportOptions {
    /// The defaults with rekordbox's preferences from `rekordbox3.settings` in `settings_dir`
    /// (`language`: 2 = German, *hypothesis* from the German oracle export; anything else
    /// English; `HotCueColorType`; `bridgeExportBeatGrid`: 1 changes, 2 every beat, else off).
    pub fn from_settings(settings_dir: &Path) -> Self {
        let pref = |name: &str| mb_usb::settings::preference(settings_dir, name);
        let mut o = Self::default();
        if let Some(lang) = pref("language") {
            o.language = if lang.trim() == "2" { KindLanguage::German } else { KindLanguage::English };
        }
        if let Some(t) = mb_usb::settings::hot_cue_color_type(settings_dir) {
            o.hot_cue_color_type = t;
        }
        if let Some(mode) = pref("bridgeExportBeatGrid").and_then(|v| v.trim().parse::<i64>().ok()) {
            o.tempo_mode = match mode {
                1 => TempoMode::Changes,
                2 => TempoMode::EveryBeat,
                _ => TempoMode::Off,
            };
        }
        o
    }
}

/// The whole document rekordbox writes for this library. `snapshot` must be the library's
/// current snapshot (tracks with their analysis paths, the playlist tree).
pub fn export_document(conn: &Connection, snapshot: &LibrarySnapshot, opts: &XmlExportOptions) -> Result<Document> {
    // `getAllCollection` order = djmdContent row order (xml.md § Collection order).
    let mut stmt = conn.prepare(
        "SELECT c.ID, c.FileType, c.Subtitle, c.ColorID, col.Commnt FROM djmdContent c \
         LEFT JOIN djmdColor col ON col.ID = c.ColorID AND col.rb_local_deleted = 0 \
         WHERE c.rb_local_deleted = 0 ORDER BY c.rowid",
    )?;
    let rows: Vec<(String, i64, String, u8, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                int(r.get(1)?),
                text(r.get(2)?),
                int(r.get(3)?) as u8,
                text(r.get(4)?),
            ))
        })?
        .collect::<mb_masterdb::rusqlite::Result<_>>()?;
    let cues = mb_usb::source::load_master_cues(conn).map_err(|e| Error::Invalid(e.to_string()))?;
    let tracks: HashMap<&str, &mb_core::Track> = snapshot.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut doc = Document::rekordbox("");
    doc.product = Some(opts.product.clone());
    let collection = doc.collection.as_mut().expect("rekordbox() has a collection");
    for (id, file_type, subtitle, color_id, colour_name) in rows {
        let Some(t) = tracks.get(id.as_str()) else { continue };
        let beats = load_beats(t.analysis_path.as_deref());
        let mut x = export::track_from_core(t, opts.language, &beats);
        x.tempos = export::tempos_from_beat_grid(&beats, opts.tempo_mode);
        x.kind = Some(export::kind_label(file_type, opts.language).to_owned());
        x.grouping = Some(colour_name);
        x.mix = Some(subtitle);
        x.colour = export::colour_rgb(color_id);
        x.position_marks = position_marks(cues.get(&id).map(Vec::as_slice).unwrap_or_default(), opts.hot_cue_color_type);
        collection.push(x);
    }
    doc.playlists = Some(vec![export::playlists_from_snapshot(snapshot)]);
    Ok(doc)
}

/// Writes `doc` next to `path` and renames it into place (rekordbox writes through a
/// `juce::TemporaryFile`).
pub fn write_document(doc: &Document, path: &Path) -> Result<()> {
    let tmp = path.with_extension("xml.musicbox-tmp");
    std::fs::write(&tmp, doc.to_xml()).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io(path, e))
}

/// `addPositionMarkNode` @0x1010574b0 for one track: the cues after `checkCueInfos`, in row order;
/// `Type` 4 when there is an out point; RGB for hot cues A–H from `getHotCueDefaultColor`
/// (`Num + 1`, `start < end`, the colour code, the colour type). Hot cues beyond H are not written.
pub fn position_marks(rows: &[mb_usb::cues::SourceCue], hot_cue_color_type: u8) -> Vec<PositionMark> {
    mb_usb::cues::checked_cues(rows)
        .into_iter()
        .filter_map(|c| {
            let num = export::position_mark_num(i64::from(c.kind))?;
            let rgb = (num >= 0).then(|| {
                mb_usb::cues::hot_cue_rgb((num + 1) as u16, c.in_ms < c.out_ms, c.color_table_index, hot_cue_color_type)
            });
            export::position_mark(i64::from(c.kind), &c.comment, c.in_ms, c.out_ms, rgb)
        })
        .collect()
}

fn load_beats(dat: Option<&Path>) -> Vec<GridBeat> {
    dat.and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| AnlzFile::parse(&bytes).ok())
        .and_then(|f| f.beat_grid())
        .map(|g| {
            g.beats
                .iter()
                .map(|b| GridBeat { beat_in_bar: b.beat_in_bar, tempo_x100: b.tempo, time_ms: b.time_ms })
                .collect()
        })
        .unwrap_or_default()
}

// ---- import ------------------------------------------------------------------------------------

/// A list to create from the XML: a playlist with tracks (indexes into [`XmlImport::tracks`], in
/// XML order) or a folder with its children.
#[derive(Debug, Clone, PartialEq)]
pub enum XmlList {
    Playlist { name: String, tracks: Vec<usize> },
    Folder { name: String, children: Vec<XmlList> },
}

impl XmlList {
    pub fn name(&self) -> &str {
        match self {
            XmlList::Playlist { name, .. } | XmlList::Folder { name, .. } => name,
        }
    }
}

/// What to import from a rekordbox.xml.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct XmlImport {
    /// The XML tracks, in import order.
    pub tracks: Vec<mb_xml::Track>,
    /// Lists to create (a dragged playlist or folder), first child of `parent`.
    pub lists: Vec<XmlList>,
    /// Target folder (`None` = the root of Playlists).
    pub parent: Option<DbId>,
    /// Update tracks already in the collection from the XML (rekordbox's "load information in
    /// the tag of the library being imported" question). Off: they are only added to the lists.
    pub update_existing: bool,
    /// Delete lists with the same name under `parent` first (rekordbox's "replace them with the
    /// one you're importing" question).
    pub replace_same_name: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlTrackResult {
    /// A new collection row.
    Imported(DbId),
    /// Already in the collection (updated when [`XmlImport::update_existing`]).
    InCollection(DbId),
    /// Not imported, with rekordbox's reason (`TrackAdditionError`).
    Skipped(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct XmlImportReport {
    /// One result per [`XmlImport::tracks`] entry.
    pub results: Vec<XmlTrackResult>,
    /// IDs of the lists created at the top level.
    pub lists: Vec<DbId>,
    /// Folders the import created (local analysis folders of new tracks).
    pub created_files: Vec<PathBuf>,
}

impl XmlImportReport {
    pub fn local_id(&self, index: usize) -> Option<&DbId> {
        match self.results.get(index)? {
            XmlTrackResult::Imported(id) | XmlTrackResult::InCollection(id) => Some(id),
            XmlTrackResult::Skipped(_) => None,
        }
    }
}

pub(crate) fn import(lib: &mut Library, req: &XmlImport) -> Result<(Effect, Option<Command>)> {
    let mut report = XmlImportReport::default();
    let mut undo: Vec<Command> = Vec::new();
    let mut new_ids: Vec<DbId> = Vec::new();
    for x in &req.tracks {
        let result = match import_track(lib, x, req.update_existing, &mut report.created_files, &mut undo) {
            Ok(r) => r,
            Err(Error::Invalid(reason)) => XmlTrackResult::Skipped(reason),
            Err(e) => return Err(e),
        };
        if let XmlTrackResult::Imported(id) = &result {
            new_ids.push(id.clone());
        }
        report.results.push(result);
    }
    if req.replace_same_name {
        for list in &req.lists {
            for id in same_name_lists(lib, req.parent.as_ref(), list.name())? {
                let (_, inverse) = crate::playlist::delete(lib, &id)?;
                undo.extend(inverse);
            }
        }
    }
    // rekordbox creates each list on top of its target, so the last top-level list ends up first.
    for list in &req.lists {
        let id = create_list(lib, req.parent.as_ref(), list, &report)?;
        undo.push(Command::DeletePlaylist { id: id.clone() });
        report.lists.push(id);
    }
    if !new_ids.is_empty() || !report.created_files.is_empty() {
        undo.push(Command::UndoDeviceImport { ids: new_ids, playlist: None, files: report.created_files.clone() });
    }
    undo.reverse();
    let inverse = (!undo.is_empty()).then_some(Command::Batch { commands: undo });
    Ok((Effect::ImportedXml(report), inverse))
}

fn create_list(lib: &mut Library, parent: Option<&DbId>, list: &XmlList, report: &XmlImportReport) -> Result<DbId> {
    let kind = match list {
        XmlList::Playlist { .. } => crate::NodeKind::Playlist,
        XmlList::Folder { .. } => crate::NodeKind::Folder,
    };
    let (effect, _) = crate::playlist::create(lib, parent, list.name(), kind)?;
    let Effect::PlaylistCreated { id } = effect else {
        return Err(Error::Invalid(format!("creating {:?} created nothing", list.name())));
    };
    match list {
        XmlList::Playlist { tracks, .. } => {
            let ids: Vec<DbId> = tracks.iter().filter_map(|i| report.local_id(*i).cloned()).collect();
            if !ids.is_empty() {
                crate::playlist::add_tracks(lib, &id, &ids, None)?;
            }
        }
        XmlList::Folder { children, .. } => {
            // Children keep the XML order: each new node becomes the first child, so create them
            // last to first.
            for child in children.iter().rev() {
                create_list(lib, Some(&id), child, report)?;
            }
        }
    }
    Ok(id)
}

fn same_name_lists(lib: &Library, parent: Option<&DbId>, name: &str) -> Result<Vec<DbId>> {
    let parent = parent.map_or("root", DbId::as_str).to_owned();
    let mut stmt = lib
        .connection()
        .prepare("SELECT ID FROM djmdPlaylist WHERE rb_local_deleted = 0 AND ParentID = ?1 AND Name = ?2")?;
    let ids = stmt
        .query_map([parent.as_str(), name], |r| r.get::<_, String>(0))?
        .collect::<mb_masterdb::rusqlite::Result<Vec<_>>>()?;
    Ok(ids.into_iter().map(DbId).collect())
}

/// One track: `addTrack` (or `prepareForImportRBXmlTrack` for a track already in the collection).
/// `Err(Error::Invalid)` = skipped with that reason.
fn import_track(
    lib: &mut Library,
    x: &mb_xml::Track,
    update_existing: bool,
    created: &mut Vec<PathBuf>,
    undo: &mut Vec<Command>,
) -> Result<XmlTrackResult> {
    let location = x.location.as_deref().unwrap_or_default();
    let path = export::location_from_uri(location).ok_or_else(|| Error::Invalid(format!("no file:// location ({location:?})")))?;
    if let Some(id) = find_by_folder_path(lib, &path)? {
        if update_existing {
            update_track(lib, &id, x, created, undo)?;
        }
        return Ok(XmlTrackResult::InCollection(id));
    }
    // `addTrack` checks (TrackAdditionError): absolute path (3), existing file (4), at most 255
    // characters (2), a known file type and a size below 2 GiB (5).
    let audio = PathBuf::from(&path);
    if !audio.is_absolute() {
        return Err(Error::Invalid(format!("{path}: not an absolute path")));
    }
    if !audio.is_file() {
        return Err(Error::Invalid(format!("{path}: file not found")));
    }
    if path.chars().count() > 0xff {
        return Err(Error::Invalid(format!("{path}: path longer than 255 characters")));
    }
    if std::fs::metadata(&audio).map(|m| m.len()).unwrap_or(0) > 0x7fff_ffff {
        return Err(Error::Invalid(format!("{path}: file of 2 GiB or more")));
    }
    let tags = crate::tags::read_file(&audio).map_err(|e| Error::Invalid(e.to_string()))?;
    let t = device_track_from_xml(x, &audio, &tags);
    let tx = lib.writer.transaction()?;
    let (id, uuid) = insert_track(&tx, &audio, &t)?;
    tx.commit()?;
    // Beat grid (`importRBXmlData`, TEMPO present).
    let total = f64::from(x.total_time.unwrap_or(0));
    let beats = to_beats(&ximport::beats_from_tempos(total, &x.tempos));
    if !beats.is_empty() {
        let dir = write_new_dat(lib, &uuid, &audio, &beats)?;
        created.push(dir.0.clone());
        grid_columns(lib, &id, &dir.1, t.bpm_x100, false)?;
    }
    add_cues(lib, &id, total, &x.position_marks, None)?;
    Ok(XmlTrackResult::Imported(id))
}

/// The XML's values in the shape of the device-import insert (`insertTrack` reads the same
/// `RowDataTrack` fields whatever filled them).
fn device_track_from_xml(x: &mb_xml::Track, audio: &Path, tags: &crate::tags::FileTags) -> DeviceTrack {
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let file_name_title = || audio.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    DeviceTrack {
        audio: audio.to_path_buf(),
        title: x.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(file_name_title),
        artist: s(&x.artist),
        album: s(&x.album),
        album_artist: tags.album_artist.clone(),
        genre: s(&x.genre),
        label: s(&x.label),
        key: s(&x.tonality),
        remixer: s(&x.remixer),
        original_artist: tags.original_artist.clone(),
        composer: s(&x.composer),
        lyricist: tags.lyricist.clone(),
        comment: s(&x.comments),
        subtitle: s(&x.mix),
        release_date: tags.release_date.clone(),
        isrc: tags.isrc.clone(),
        stock_date: s(&x.date_added),
        bpm_x100: x.average_bpm.map_or(0, ximport::bpm_x100_from_xml),
        length_s: x.total_time.unwrap_or(0) as u16,
        track_no: x.track_number.unwrap_or(0),
        disc_no: x.disc_number.unwrap_or(0) as u16,
        bitrate: x.bit_rate.unwrap_or(0),
        sample_rate: x.sample_rate.unwrap_or(0),
        file_size: x.size.unwrap_or(0) as u32,
        // `addTrack` sets the type from the path (`DBToolBox::getFileType`), not from `Kind`.
        file_type: tags.file_type as u8,
        rating: export::rating_from_xml(i64::from(x.rating.unwrap_or(0))).min(5),
        color_id: x.colour.and_then(|c| ximport::colour_id_from_text(&format!("0x{c:06X}"))).unwrap_or(0),
        release_year: x.year.unwrap_or(0) as u16,
        play_count: x.play_count.unwrap_or(0) as u16,
        ..DeviceTrack::default()
    }
}

/// `updateCollectionFromXml` @0x100c5a590 + `importRBXmlData(…, true)` for a track already in the
/// collection: the XML's non-empty, differing values (title, artist, composer, album, genre, disc
/// and track number, year, key, rating, remixer, comment, label, colour) through the track-edit
/// op, then the beat grid (unless the track is locked against analysis, `Analysed & 0x80`) and the
/// cues (the old cues are replaced).
fn update_track(lib: &mut Library, id: &DbId, x: &mb_xml::Track, created: &mut Vec<PathBuf>, undo: &mut Vec<Command>) -> Result<()> {
    use crate::TrackEdit as E;
    let mut edits: Vec<crate::TrackEdit> = Vec::new();
    let text_edit = |v: &Option<String>, f: fn(String) -> crate::TrackEdit| v.as_ref().filter(|s| !s.is_empty()).map(|s| f(s.clone()));
    edits.extend(text_edit(&x.name, E::Title));
    edits.extend(text_edit(&x.artist, E::Artist));
    edits.extend(text_edit(&x.composer, E::Composer));
    edits.extend(text_edit(&x.album, E::Album));
    edits.extend(text_edit(&x.genre, E::Genre));
    if let Some(n) = x.disc_number {
        edits.push(E::DiscNo(n));
    }
    if let Some(n) = x.track_number {
        edits.push(E::TrackNo(n));
    }
    if let Some(y) = x.year.filter(|y| *y != 0) {
        edits.push(E::ReleaseYear(y));
    }
    edits.extend(text_edit(&x.tonality, E::Key));
    if let Some(r) = x.rating {
        edits.push(E::Rating(export::rating_from_xml(i64::from(r)).min(5)));
    }
    edits.extend(text_edit(&x.remixer, E::Remixer));
    edits.extend(text_edit(&x.comments, E::Comment));
    edits.extend(text_edit(&x.label, E::Label));
    if let Some(c) = x.colour.and_then(|c| ximport::colour_id_from_text(&format!("0x{c:06X}"))) {
        edits.push(E::Color(c));
    }
    for edit in edits {
        let (effect, inverse) = crate::edit::edit_track(lib, id, &edit)?;
        if effect != Effect::Unchanged {
            undo.extend(inverse);
        }
    }
    let total = f64::from(x.total_time.unwrap_or(0));
    let beats = to_beats(&ximport::beats_from_tempos(total, &x.tempos));
    let analysed: i64 = lib
        .connection()
        .query_row("SELECT Analysed FROM djmdContent WHERE ID = ?1", [id.as_str()], |r| r.get::<_, Option<i64>>(0))?
        .unwrap_or(0);
    if !beats.is_empty() && analysed & 0x80 == 0 {
        let bpm = x.average_bpm.map_or(0, ximport::bpm_x100_from_xml);
        let (dat_rel, old) = existing_dat(lib, id)?;
        match (dat_rel, old) {
            (Some(rel), Some(file)) => {
                // The import leaves the .EXT alone; the restore must put its PQT2 back as it is.
                let root = lib.analysis_root.clone().unwrap_or_default();
                let ext = std::fs::read(root.join(rel.trim_start_matches('/')).with_extension("EXT"))
                    .ok()
                    .and_then(|b| AnlzFile::parse(&b).ok());
                let old_bpm: Option<i64> = lib
                    .connection()
                    .query_row("SELECT BPM FROM djmdContent WHERE ID = ?1", [id.as_str()], |r| r.get::<_, Option<i64>>(0))?;
                if let Some(pqtz) = file.sections_with(Tag::PQTZ).next().cloned() {
                    undo.push(Command::RestoreBeatGrid {
                        content: id.clone(),
                        pqtz,
                        pqt2: ext.and_then(|e| e.sections_with(Tag::PQT2).next().cloned()),
                        bpm_x100: old_bpm,
                    });
                }
                let mut file = file;
                replace_pqtz(&mut file, &beats);
                write_anlz(&lib.analysis_root.clone().unwrap_or_default().join(rel.trim_start_matches('/')), &file)?;
                grid_columns(lib, id, &rel, bpm, true)?;
            }
            _ => {
                let (uuid, audio): (String, String) = lib.connection().query_row(
                    "SELECT UUID, FolderPath FROM djmdContent WHERE ID = ?1",
                    [id.as_str()],
                    |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default())),
                )?;
                let (dir, rel) = write_new_dat(lib, &uuid, Path::new(&audio), &beats)?;
                created.push(dir);
                grid_columns(lib, id, &rel, bpm, true)?;
            }
        }
    }
    if !x.position_marks.is_empty() {
        let old: Vec<String> = {
            let mut stmt = lib.connection().prepare("SELECT ID FROM djmdCue WHERE ContentID = ?1 AND rb_local_deleted = 0 ORDER BY rowid")?;
            stmt.query_map([id.as_str()], |r| r.get::<_, String>(0))?.collect::<mb_masterdb::rusqlite::Result<_>>()?
        };
        for cue in old {
            let (_, inverse) = cues::delete_cue(lib, &cue)?;
            undo.extend(inverse);
        }
        add_cues(lib, id, total, &x.position_marks, Some(undo))?;
    }
    Ok(())
}

/// POSITION_MARKs → `setCue` per cue (kind mapping per `rekordboxDBController::setCue`: −1 and
/// 4 are memory loops, 0 a memory cue; the colour code is the pad colour nearest to the mark's
/// RGB).
fn add_cues(lib: &mut Library, id: &DbId, total_s: f64, marks: &[PositionMark], mut undo: Option<&mut Vec<Command>>) -> Result<()> {
    for c in ximport::cues_from_marks(total_s, marks) {
        let Some(slot) = crate::CueSlot::from_kind(i64::from(c.kind)) else { continue };
        let cue = NewCue {
            slot,
            in_ms: c.start_ms,
            out_ms: (c.end_ms > c.start_ms).then_some(c.end_ms),
            color: None,
            color_table_index: c.rgb.map(|rgb| ximport::nearest_pad_color(rgb, &mb_usb::cues::PAD_COLORS)),
            comment: c.name,
        };
        let (_, inverse) = cues::insert_cue(lib, id, &cue)?;
        if let Some(u) = undo.as_deref_mut() {
            u.extend(inverse);
        }
    }
    Ok(())
}

fn to_beats(grid: &[GridBeat]) -> Vec<Beat> {
    grid.iter().map(|b| Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo_x100, time_ms: b.time_ms }).collect()
}

/// A local `.DAT` for a track that has none (`createMusicAnalyzeFileFromAudioPath`, then
/// `set_pmai_beat_position` + `write_pmai`): the tag set of every local DAT with default
/// contents (*hypothesis*: `initialize_pmai` defaults: empty VBR index, zero waveform previews,
/// empty cue lists) and the imported PQTZ. Returns (folder, `AnalysisDataPath`).
fn write_new_dat(lib: &Library, uuid: &str, audio: &Path, beats: &[Beat]) -> Result<(PathBuf, String)> {
    let root = lib
        .analysis_root
        .clone()
        .ok_or_else(|| Error::Invalid("importing a beat grid needs the analysis root".into()))?;
    let rel = format!("PIONEER/USBANLZ/{}", mb_anlz::usb::local_anlz_folder(uuid));
    let dir = root.join(&rel);
    std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    let file_name = audio.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let preview = |tag: Tag, n: usize| TypedSection::WavePreview(WavePreview { tag, unk_0c: 0, unk_10: 1, unk_12: 0, data: vec![0; n] });
    let file = AnlzFile::from_typed(
        FileHeader::default(),
        &[
            TypedSection::Path(PathTag::new(mb_anlz::usb::local_ppth(&file_name))),
            TypedSection::Vbr(Vbr { unk_0c: 0, unk_0e: 0, index: vec![0; 400], sample_count: Some(0) }),
            TypedSection::BeatGrid(BeatGrid::new(beats.to_vec())),
            preview(Tag::PWAV, 400),
            preview(Tag::PWV2, 100),
            TypedSection::CueList(CueList::empty(CueKind::Hot)),
            TypedSection::CueList(CueList::empty(CueKind::Memory)),
        ],
    );
    write_anlz(&dir.join("ANLZ0000.DAT"), &file)?;
    Ok((dir, format!("/{rel}/ANLZ0000.DAT")))
}

/// The columns `importRBXmlData` writes after the DAT, one statement each: `modTrackAnalysisPath`,
/// `modTrackBpm`, `modAnalyzeStatus` (`Analysed` with bit 0x10 = "beat grid imported", then
/// `ContentLink` unchanged), and for a track already in the collection `ContentLink |= 0xE` when
/// a bit is missing.
fn grid_columns(lib: &mut Library, id: &DbId, analysis_path: &str, bpm_x100: u32, existing: bool) -> Result<()> {
    let (analysed, content_link, old_bpm): (i64, i64, i64) = lib.connection().query_row(
        "SELECT Analysed, ContentLink, BPM FROM djmdContent WHERE ID = ?1",
        [id.as_str()],
        |r| Ok((int(r.get(0)?), int(r.get(1)?), int(r.get(2)?))),
    )?;
    let bpm = if bpm_x100 != 0 { i64::from(bpm_x100) } else { old_bpm };
    let key = || Value::Text(id.0.clone());
    let tx = lib.writer.transaction()?;
    tx.update("djmdContent", "ID", key(), &[("AnalysisDataPath", Value::Text(analysis_path.to_owned()))])?;
    tx.update("djmdContent", "ID", key(), &[("BPM", Value::Integer(bpm))])?;
    let new_analysed = (analysed & 0x6b) | 0x10 | (analysed & 0x80);
    tx.update("djmdContent", "ID", key(), &[("Analysed", Value::Integer(new_analysed))])?;
    tx.update("djmdContent", "ID", key(), &[("ContentLink", Value::Integer(content_link))])?;
    if existing && content_link & 0xE != 0xE {
        tx.update("djmdContent", "ID", key(), &[("ContentLink", Value::Integer(content_link | 0xE))])?;
    }
    tx.commit()?;
    Ok(())
}

fn existing_dat(lib: &Library, id: &DbId) -> Result<(Option<String>, Option<AnlzFile>)> {
    let rel: Option<String> = lib
        .connection()
        .query_row("SELECT AnalysisDataPath FROM djmdContent WHERE ID = ?1", [id.as_str()], |r| r.get::<_, Option<String>>(0))?
        .filter(|p| !p.is_empty());
    let file = match (&rel, &lib.analysis_root) {
        (Some(rel), Some(root)) => std::fs::read(root.join(rel.trim_start_matches('/'))).ok().and_then(|b| AnlzFile::parse(&b).ok()),
        _ => None,
    };
    Ok((rel, file))
}

fn replace_pqtz(file: &mut AnlzFile, beats: &[Beat]) {
    let section = BeatGrid::new(beats.to_vec()).encode();
    match file.sections.iter_mut().find(|s| s.tag == Tag::PQTZ) {
        Some(s) => *s = section,
        None => file.sections.push(section),
    }
}

fn write_anlz(path: &Path, file: &AnlzFile) -> Result<()> {
    std::fs::write(path, file.to_bytes()).map_err(|e| io(path, e))
}

fn find_by_folder_path(lib: &Library, path: &str) -> Result<Option<DbId>> {
    Ok(lib
        .connection()
        .query_row("SELECT ID FROM djmdContent WHERE rb_local_deleted = 0 AND FolderPath = ?1", [path], |r| r.get::<_, String>(0))
        .optional()?
        .map(DbId))
}

fn int(v: Value) -> i64 {
    match v {
        Value::Integer(n) => n,
        Value::Real(f) => f as i64,
        Value::Text(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn text(v: Value) -> String {
    match v {
        Value::Text(s) => s,
        Value::Integer(n) => n.to_string(),
        Value::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io { path: path.display().to_string(), source }
}

/// Parses a rekordbox.xml file.
pub fn read_document(path: &Path) -> Result<Document> {
    let text = std::fs::read_to_string(path).map_err(|e| io(path, e))?;
    Document::parse(&text).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}

/// The XML tracks a playlist node refers to (`KeyType` 0: `TrackID`; 1: `Location`), in order;
/// unknown keys are left out.
pub fn playlist_tracks<'a>(doc: &'a Document, entries: &[mb_xml::PlaylistEntry], key_type: Option<mb_xml::KeyType>) -> Vec<&'a mb_xml::Track> {
    let tracks = doc.tracks();
    entries
        .iter()
        .filter_map(|e| match key_type {
            Some(mb_xml::KeyType::Location) => tracks.iter().find(|t| t.location.as_deref() == Some(e.key.as_str())),
            _ => {
                let id: i64 = e.key.trim().parse().ok()?;
                tracks.iter().find(|t| t.track_id == Some(id))
            }
        })
        .collect()
}

/// An [`XmlImport`] for one XML playlist or folder node: its tracks (each once, first
/// occurrence order) and the list tree.
pub fn import_for_node(doc: &Document, node: &mb_xml::PlaylistNode) -> XmlImport {
    let mut tracks: Vec<mb_xml::Track> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    fn walk(doc: &Document, node: &mb_xml::PlaylistNode, tracks: &mut Vec<mb_xml::Track>, index: &mut HashMap<String, usize>) -> XmlList {
        match node {
            mb_xml::PlaylistNode::Folder { name, children, .. } => XmlList::Folder {
                name: name.clone(),
                children: children.iter().map(|c| walk(doc, c, tracks, index)).collect(),
            },
            mb_xml::PlaylistNode::Playlist { name, key_type, entries, .. } => {
                let mut ids = Vec::new();
                for t in playlist_tracks(doc, entries, *key_type) {
                    let key = t.location.clone().unwrap_or_default();
                    let i = *index.entry(key).or_insert_with(|| {
                        tracks.push(t.clone());
                        tracks.len() - 1
                    });
                    ids.push(i);
                }
                XmlList::Playlist { name: name.clone(), tracks: ids }
            }
        }
    }
    let list = walk(doc, node, &mut tracks, &mut index);
    XmlImport { tracks, lists: vec![list], ..XmlImport::default() }
}
