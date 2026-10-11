//! Op: import audio files into the collection. Spec: `docs/re/ops/import-track.md` (verified on
//! 161 traced imports).
//!
//! Per file, in one `begin exclusive` transaction: artist → genre → (album artist →) album →
//! remixer → label → key lookups, each creating a missing row with one USN bump, then
//! `select BaseDBDrive, CurrentDBDrive from djmdProperty` and one bump + `djmdContent` insert.
//! Import writes no `contentFile`, mixer, cue, artwork or analysis values.

use crate::lookup::{NameTable, find_or_create, find_or_create_album};
use crate::tags::{self, FileTags};
use crate::{Command, DbId, Effect, Library, Result};
use mb_masterdb::WriteTxn;
use mb_masterdb::rusqlite::OptionalExtension;
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::write::{date_today, new_content_id, new_uuid};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportReport {
    /// Imported files and their new content IDs, in input order.
    pub imported: Vec<(PathBuf, DbId)>,
    pub skipped: Vec<(PathBuf, SkipReason)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    /// A live `djmdContent` row already has this `FolderPath`.
    Duplicate(DbId),
    /// The file could not be read or is not a supported audio file.
    Unreadable(String),
}

/// `djmdContent.ContentLink` right after import (161/161).
pub const CONTENT_LINK_IMPORTED: i64 = 14;

pub(crate) fn import_files(lib: &mut Library, paths: &[PathBuf]) -> Result<(Effect, Option<Command>)> {
    let mut report = ImportReport::default();
    // The traced batch frame adds an empty `begin exclusive; commit` before and after the files;
    // empty transactions change nothing and are not replayed.
    for path in paths {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        // hypothesis: rekordbox skips a file whose path is already in the collection. Duplicate
        // import was not exercised (import-track.md § Open); the check is an exact FolderPath match.
        if let Some(existing) = find_by_folder_path(lib, &path)? {
            report.skipped.push((path, SkipReason::Duplicate(existing)));
            continue;
        }
        let tags = match tags::read_file(&path) {
            Ok(t) => t,
            Err(e) => {
                report.skipped.push((path, SkipReason::Unreadable(e.to_string())));
                continue;
            }
        };
        let tx = lib.writer.transaction()?;
        let id = import_one(&tx, &path, &tags)?;
        tx.commit()?;
        report.imported.push((path, id));
    }
    let inverse = (!report.imported.is_empty())
        .then(|| Command::RemoveTracks { ids: report.imported.iter().map(|(_, id)| id.clone()).collect() });
    Ok((Effect::Imported(report), inverse))
}

fn find_by_folder_path(lib: &Library, path: &Path) -> Result<Option<DbId>> {
    Ok(lib
        .connection()
        .query_row(
            "SELECT ID FROM djmdContent WHERE rb_local_deleted = 0 AND FolderPath = ?1",
            [path.to_string_lossy()],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(DbId))
}

/// The per-file transaction body. Returns the new content ID.
fn import_one(tx: &WriteTxn<'_>, path: &Path, t: &FileTags) -> Result<DbId> {
    let opt = |s: &str, table: NameTable| -> Result<Option<String>> {
        if s.is_empty() { Ok(None) } else { find_or_create(tx, table, s).map(Some) }
    };
    // Fixed order (import-track.md, all 100 tagged files): artist → genre → album artist → album
    // → remixer → label → key.
    let artist_id = opt(&t.artist, NameTable::Artist)?;
    let genre_id = opt(&t.genre, NameTable::Genre)?;
    let album_id = if t.album.is_empty() {
        None
    } else {
        let album_artist_id = opt(&t.album_artist, NameTable::Artist)?;
        Some(find_or_create_album(tx, &t.album, album_artist_id.as_deref())?)
    };
    let remixer_id = opt(&t.remixer, NameTable::Artist)?;
    // Original artist, composer and lyricist tags did not occur in the corpus; their lookups
    // (if any) are not replayed. hypothesis: import leaves OrgArtistID/ComposerID NULL and
    // Lyricist "" as observed on all 161 traced rows (import-track.md § Open).
    let label_id = opt(&t.label, NameTable::Label)?;
    // `KeyID` is "0" (never NULL) without a key tag.
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
    let date = date_today();
    let file_name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let text = |s: &str| Value::Text(s.to_owned());
    let int = Value::Integer;
    // Column list and values of the traced builder insert (import-track.md § djmdContent insert);
    // the bracketed lookup columns are omitted (NULL) when there is no tag.
    let mut cols: Vec<(&str, Value)> = vec![
        ("ID", text(&id)),
        ("UUID", Value::Text(new_uuid())),
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
        ("BPM", int(0)),
        ("Length", int(t.length)),
        ("TrackNo", int(t.track_no)),
        ("BitRate", int(0)),
        ("BitDepth", int(0)),
        ("Commnt", text(&t.comment)),
        ("FileType", int(t.file_type)),
        ("Rating", int(0)),
        ("ReleaseYear", int(t.release_year)),
    ]);
    if let Some(r) = &remixer_id {
        cols.push(("RemixerID", text(r)));
    }
    if let Some(l) = &label_id {
        cols.push(("LabelID", text(l)));
    }
    cols.extend([
        ("KeyID", text(&key_id)),
        ("StockDate", text(&date)),
        ("ColorID", text("0")),
        ("DJPlayCount", int(0)),
        ("ImagePath", text("")),
        ("MasterDBID", text(&dbid)),
        ("MasterSongID", text(&id)),
        ("AnalysisDataPath", text("")),
        ("FileSize", int(t.file_size)),
        ("DiscNo", int(t.disc_no)),
        ("Subtitle", text("")),
        ("SampleRate", int(0)),
        ("Analysed", int(0)),
        ("ReleaseDate", text(&t.release_date)),
        ("DateCreated", text(&date)),
        ("ContentLink", int(CONTENT_LINK_IMPORTED)),
        ("ModifiedByRBM", text("")),
        ("HotCueAutoLoad", text("on")),
        ("DeliveryControl", text("on")),
        ("DeliveryComment", text("")),
        ("Lyricist", text("")),
        ("ISRC", text(&t.isrc)),
        ("SamplerTrackInfo", int(0)),
        ("SamplerPlayOffset", int(0)),
        // Bound as the integer 0 like the trace; the FLOAT affinity stores REAL 0.0.
        ("SamplerGain", int(0)),
        // A bare number in the trace; the VARCHAR affinity stores the inode as text.
        ("rb_file_id", int(t.inode as i64)),
        ("VideoAssociate", int(0)),
        ("LyricStatus", int(0)),
        ("ServiceID", int(0)),
        ("OrgFolderPath", text("")),
        ("ExtInfo", text("null")),
        ("DeviceID", text(&device_id)),
        ("Reserved1", text("")),
    ]);
    tx.insert("djmdContent", &cols)?;
    Ok(DbId(id))
}
