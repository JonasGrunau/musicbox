//! Replays a rekordbox USB export into a new exportLibrary.db with [`mb_devlib::DevLibWriter`].
//! Shared by `examples/devlib_rebuild` and `tests/writer.rs`.
//!
//! Input: a library directory (`options.json` + `rekordbox/master.db`, or `master.db` directly)
//! and the root of a stick rekordbox exported from it. The stick supplies only what belongs to
//! the export layer, not to exportLibrary.db: the device audio path and ANLZ path per source
//! track (matched by `masterContentId`), the order the playlists were exported in
//! (`playlist_id` order) and the stick's `createdDate`. Everything else comes from master.db
//! and the writer's rules. Spec: `docs/re/ops/export-devlib.md`.
//!
//! The export of each playlist is replayed like `DatabaseMediator::exportPlaylist` +
//! `execExport` (`docs/re/export.md` §1): the device list is created on top (position 0); each
//! source track in list order is skipped if the format gate refuses it, updated in place if
//! it is already on the device (`overwrite_export`), or inserted with its artwork; then it is
//! appended to the list.

use mb_devlib::schema::{MasterMenuItem, MasterMenuSetting};
use mb_devlib::{
    CreateOptions, DevLibDb, DevLibWriter, ImageOwner, ListAttribute, MenuSeed, MyTagRow, NewTrack,
    export_analysed_bits, export_content_link, reexport_content_link,
};
use rusqlite::types::Value;
use rusqlite::{Connection, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the replay did.
#[derive(Debug, Default)]
pub struct Summary {
    /// (playlist name, tracks inserted, tracks updated in place, tracks skipped).
    pub exports: Vec<(String, usize, usize, usize)>,
    /// Source tracks refused by the format gate: (master ID, file name).
    pub refused: Vec<(String, String)>,
    pub my_tag_master_db_id: u32,
}

/// A temp directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> std::io::Result<Self> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mb-devlib-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// master.db of a library directory, copied (with `-wal`/`-shm`) into `tmp` so the source is
/// never opened in place, and opened read-only with the passphrase from `options.json`.
fn open_master(library_dir: &Path, tmp: &Path) -> anyhow::Result<mb_masterdb::MasterDb> {
    let src = [
        library_dir.join("rekordbox/master.db"),
        library_dir.join("master.db"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .ok_or_else(|| anyhow::anyhow!("no master.db under {}", library_dir.display()))?;
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", src.display()));
        if from.exists() {
            std::fs::copy(&from, tmp.join(format!("master.db{suffix}")))?;
        }
    }
    let options = mb_masterdb::LibraryPaths::from_options_json(&library_dir.join("options.json"))?;
    let passphrase = mb_masterdb::derive_passphrase(&options.dp)?;
    Ok(mb_masterdb::MasterDb::open_read_only(
        &tmp.join("master.db"),
        &passphrase,
    )?)
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

/// `initPropertyTable`'s sources, in the order rekordbox reads them.
fn menus(master: &Connection) -> anyhow::Result<MenuSeed> {
    let mut stmt = master.prepare(
        "select ID, Commnt from djmdColor where rb_local_deleted = 0 order by cast(ID as integer)",
    )?;
    let colors = stmt
        .query_map([], |r| Ok((int(r, 0)? as u32, text(r, 1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = master.prepare(
        "select ID, Class, Name from djmdMenuItems where rb_local_deleted = 0 order by cast(ID as integer)",
    )?;
    let items = stmt
        .query_map([], |r| {
            Ok(MasterMenuItem {
                id: int(r, 0)? as u32,
                class: int(r, 1)?,
                name: text(r, 2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let settings = |table: &str| -> anyhow::Result<Vec<MasterMenuSetting>> {
        // ID order: the order of the rebuild a fresh stick ends with (export-devlib.md phase 2).
        let mut stmt = master.prepare(&format!(
            "select ID, MenuItemID, Seq, Disable from {table} where rb_local_deleted = 0 \
             order by cast(ID as integer)"
        ))?;
        Ok(stmt
            .query_map([], |r| {
                Ok(MasterMenuSetting {
                    id: int(r, 0)? as u32,
                    menu_item_id: int(r, 1)? as u32,
                    seq: int(r, 2)?,
                    disable: int(r, 3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    };
    Ok(MenuSeed::from_master(
        &colors,
        &items,
        &settings("djmdCategory")?,
        &settings("djmdSort")?,
    ))
}

/// `syncMyTagList`'s input: every top-level group (by `Seq`) followed by its tags (by `Seq`),
/// `sequenceNo = Seq - 1`, parent `root` → 0.
fn my_tag_rows(master: &Connection) -> anyhow::Result<Vec<MyTagRow>> {
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
    let row = |(id, seq, name, attr, parent): &(u32, i64, String, i64, String)| MyTagRow {
        id: *id,
        seq: seq - 1,
        name: name.clone(),
        attribute: ListAttribute::from_code(*attr),
        parent: parent.parse().unwrap_or(0),
    };
    let mut out = Vec::new();
    for group in all.iter().filter(|t| t.4 == "root") {
        out.push(row(group));
        let gid = group.0.to_string();
        out.extend(all.iter().filter(|t| t.4 == gid).map(row));
    }
    Ok(out)
}

/// `syncMyTagLocal` @0x100C52988: the cloud library's DBID (`djmdCloudProperty` row
/// `SyncService`) when cloud sync is set up, otherwise the master DBID + 1.
fn my_tag_master_db_id(master: &Connection, dbid: u32) -> anyhow::Result<u32> {
    let cloud: Option<i64> = master
        .query_row(
            "select ID from djmdCloudProperty where Reserved1 = 'SyncService' and rb_local_deleted = 0",
            [],
            |r| int(r, 0),
        )
        .ok();
    Ok(cloud
        .filter(|id| *id != 0)
        .map_or(dbid.wrapping_add(1), |id| id as u32))
}

/// One source track with the fields the export reads (`getRowDataTrack` + `getUpdatedInfo`).
struct Source {
    id: String,
    track: NewTrack,
    analysed: i64,
    content_link: u32,
    has_artwork: bool,
    info_marker: String,
    analysis_marker: String,
    cue_marker: String,
    file_name: String,
    sample_rate: i64,
}

const SOURCE_SQL: &str = "
SELECT c.ID, c.Title, c.Subtitle, ar.Name, rm.Name, oa.Name, co.Name, c.Lyricist,
       al.Name, aa.Name, al.Compilation, ge.Name, la.Name, k.ScaleName,
       c.BPM, c.Length, c.TrackNo, c.DiscNo, c.ReleaseYear, c.ReleaseDate, c.Rating, c.ColorID,
       c.Commnt, c.DateCreated, c.StockDate, c.FileSize, c.FileType, c.BitRate, c.BitDepth,
       c.SampleRate, c.ISRC, c.DJPlayCount, c.HotCueAutoLoad, c.DeliveryControl,
       c.DeliveryComment, c.MasterDBID, c.MasterSongID, c.Analysed, c.ContentLink, c.ImagePath,
       c.TrackInfoUpdated, c.AnalysisUpdated, c.CueUpdated, c.FileNameL
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
WHERE c.ID = ?1";

fn source(master: &Connection, id: &str, dbid: u32) -> anyhow::Result<Source> {
    Ok(master.query_row(SOURCE_SQL, [id], |r| {
        let nonzero = |v: i64| (v != 0).then_some(v as u32);
        let track = NewTrack {
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
            date_added: text(r, 24)?,
            file_size: int(r, 25)? as u64,
            file_type: int(r, 26)?,
            bitrate: int(r, 27)?,
            bit_depth: int(r, 28)?,
            sample_rate: int(r, 29)? as u32,
            isrc: text(r, 30)?,
            play_count: int(r, 31)?,
            hot_cue_auto_load: text(r, 32)? == "on",
            kuvo_deliver: text(r, 33)? == "on",
            kuvo_comment: text(r, 34)?,
            // `getMasterDBID` / `getMasterContentID` (export.md §2.5).
            master_db_id: nonzero(int(r, 35)?).unwrap_or(dbid),
            master_content_id: nonzero(int(r, 36)?).unwrap_or(int(r, 0)? as u32),
            analysed_bits: export_analysed_bits(int(r, 37)?),
            content_link: export_content_link(int(r, 38)?),
            ..NewTrack::default()
        };
        Ok(Source {
            id: text(r, 0)?,
            analysed: int(r, 37)?,
            content_link: int(r, 38)? as u32,
            has_artwork: !text(r, 39)?.is_empty(),
            info_marker: text(r, 40)?,
            analysis_marker: text(r, 41)?,
            cue_marker: text(r, 42)?,
            file_name: text(r, 43)?,
            sample_rate: int(r, 29)?,
            track,
        })
    })?)
}

/// The USB branch of the format gate (`check_support_export` @0x1009BFCE4 / `execExport`):
/// FLAC, ALAC, WAV and AIFF only at 44.1, 48, 88.2 or 96 kHz; DRUM CAPTURE (0x40) never.
fn exportable(s: &Source) -> bool {
    match s.track.file_type {
        0x40 => false,
        5 | 6 | 11 | 12 => matches!(s.sample_rate, 44100 | 48000 | 88200 | 96000),
        _ => true,
    }
}

/// Replays the exports found on `device_root` from the library in `library_dir` into a new
/// database at `out` (replaced if present), and closes it cleanly.
pub fn rebuild(library_dir: &Path, device_root: &Path, out: &Path) -> anyhow::Result<Summary> {
    let tmp = TempDir::new("rebuild-master")?;
    let master_db = open_master(library_dir, &tmp.0)?;
    let master = master_db.connection();
    let device = DevLibDb::open_immutable(&device_root.join("PIONEER/rekordbox/exportLibrary.db"))?;
    let reference = device.load_library()?;
    let by_master: HashMap<u32, (String, String)> = reference
        .tracks
        .iter()
        .filter_map(|t| {
            Some((
                t.master_content_id?,
                (t.path.clone(), t.analysis_path.clone()),
            ))
        })
        .collect();
    let mut lists: Vec<_> = reference
        .playlists
        .iter()
        .filter(|p| p.parent.is_none() && p.attribute == ListAttribute::List)
        .collect();
    lists.sort_by_key(|p| p.id);

    let dbid: u32 = master.query_row("select DBID from djmdProperty", [], |r| int(r, 0))? as u32;
    let created_date = reference
        .property
        .as_ref()
        .map(|p| p.created_date.clone())
        .filter(|d| !d.is_empty());

    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", out.display()));
    }
    let mut writer = DevLibWriter::create(
        out,
        &CreateOptions {
            created_date: created_date.clone(),
            menus: menus(master)?,
            ..CreateOptions::default()
        },
    )?;
    if let Some(date) = &created_date {
        writer.set_today(date);
    }

    // My Tag tree (`syncMyTag` after the DB is created; export-devlib.md phase 4).
    let mut summary = Summary {
        my_tag_master_db_id: my_tag_master_db_id(master, dbid)?,
        ..Summary::default()
    };
    writer.write_my_tags(summary.my_tag_master_db_id, &my_tag_rows(master)?)?;
    writer.clear_my_tag_content()?;

    for list in lists {
        let master_list: String = master.query_row(
            "select ID from djmdPlaylist where Name = ?1 and ParentID = 'root' and Attribute = 0 \
             and rb_local_deleted = 0",
            [&list.name],
            |r| text(r, 0),
        )?;
        let mut stmt = master.prepare(
            "select ContentID from djmdSongPlaylist where PlaylistID = ?1 and rb_local_deleted = 0 \
             order by TrackNo",
        )?;
        let entries = stmt
            .query_map([&master_list], |r| text(r, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let playlist = writer.create_playlist(&list.name, 0, false, Some(0))?;
        let (mut inserted, mut updated, mut skipped) = (0, 0, 0);
        for id in entries {
            let mut src = source(master, &id, dbid)?;
            if !exportable(&src) {
                skipped += 1;
                summary
                    .refused
                    .push((src.id.clone(), src.file_name.clone()));
                continue;
            }
            let content_id = match writer.find_exported_track(&src.track)? {
                Some(content_id) => {
                    // `overwrite_export` @0x1009B8748.
                    let (info, analysis, cue) = writer.track_markers(content_id)?;
                    if info != src.info_marker {
                        writer.set_track_updated(content_id, &src.info_marker)?;
                    }
                    if analysis != src.analysis_marker || cue != src.cue_marker {
                        writer.set_analysis_updated(
                            content_id,
                            src.analysed as u8,
                            &src.analysis_marker,
                            &src.cue_marker,
                        )?;
                        let device_link = writer.content_link(content_id)?;
                        if let Some(link) =
                            reexport_content_link(device_link, src.content_link, true)
                        {
                            writer.set_content_link(content_id, link)?;
                        }
                    }
                    updated += 1;
                    content_id
                }
                None => {
                    let (path, anlz) = by_master
                        .get(&src.track.master_content_id)
                        .cloned()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "track {} ({}) is not on the reference stick",
                                src.id,
                                src.file_name
                            )
                        })?;
                    src.track.path = path;
                    src.track.analysis_path = anlz;
                    let content_id = writer.add_track(&src.track)?;
                    if src.has_artwork {
                        writer.add_image(ImageOwner::Content(content_id), false)?;
                    }
                    // `replaceMyTagOnTrack`: the track's master My Tags.
                    let mut stmt = master.prepare(
                        "select MyTagID from djmdSongMyTag where ContentID = ?1 \
                         and rb_local_deleted = 0 order by TrackNo",
                    )?;
                    let tags = stmt
                        .query_map([&src.id], |r| int(r, 0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let tags: Vec<u32> = tags.into_iter().map(|t| t as u32).collect();
                    writer.add_my_tags_to_track(content_id, &tags)?;
                    inserted += 1;
                    content_id
                }
            };
            writer.append_playlist_entry(playlist, content_id)?;
        }
        summary
            .exports
            .push((list.name.clone(), inserted, updated, skipped));
    }
    writer.close()?;
    Ok(summary)
}
