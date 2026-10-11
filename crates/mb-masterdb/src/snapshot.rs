//! Loads a [`LibrarySnapshot`] (collection + playlist tree) from master.db.
//! Rows with `rb_local_deleted = 1` are soft-deleted and skipped (docs/re/masterdb.md).

use crate::{LibraryPaths, MasterDb, Result};
use mb_core::{DbId, FileType, LibrarySnapshot, PlaylistKind, PlaylistNode, Track};
use rusqlite::Row;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const TRACKS_SQL: &str = "
SELECT c.ID, c.Title, ar.Name, al.Name, ge.Name, la.Name, rm.Name, co.Name, oa.Name,
       k.ScaleName, c.ColorID, c.BPM, c.Length, c.TrackNo, c.DiscNo, c.ReleaseYear, c.Rating,
       c.Commnt, c.DJPlayCount, c.BitRate, c.SampleRate, c.BitDepth, c.FileSize, c.FileType,
       c.FolderPath, c.StockDate, c.ReleaseDate, c.AnalysisDataPath, c.ImagePath, c.Analysed
FROM djmdContent c
LEFT JOIN djmdArtist ar ON ar.ID = c.ArtistID
LEFT JOIN djmdAlbum al ON al.ID = c.AlbumID
LEFT JOIN djmdGenre ge ON ge.ID = c.GenreID
LEFT JOIN djmdLabel la ON la.ID = c.LabelID
LEFT JOIN djmdArtist rm ON rm.ID = c.RemixerID
LEFT JOIN djmdArtist co ON co.ID = c.ComposerID
LEFT JOIN djmdArtist oa ON oa.ID = c.OrgArtistID
LEFT JOIN djmdKey k ON k.ID = c.KeyID
WHERE c.rb_local_deleted = 0";

fn text(row: &Row, i: usize) -> rusqlite::Result<String> {
    Ok(row.get::<_, Option<String>>(i)?.unwrap_or_default())
}

fn positive_u32(row: &Row, i: usize) -> rusqlite::Result<Option<u32>> {
    Ok(row.get::<_, Option<i64>>(i)?.filter(|v| *v > 0).map(|v| v as u32))
}

/// Paths in master.db that start with `/PIONEER/` are relative to the analysis-data root.
fn under_root(root: &Path, stored: Option<String>) -> Option<PathBuf> {
    let stored = stored.filter(|s| !s.is_empty())?;
    Some(root.join(stored.trim_start_matches('/')))
}

impl MasterDb {
    pub fn load_snapshot(&self, paths: &LibraryPaths) -> Result<LibrarySnapshot> {
        let conn = self.connection();
        let root = &paths.analysis_data_root;

        let mut stmt = conn.prepare(TRACKS_SQL)?;
        let tracks = stmt
            .query_map([], |r| {
                Ok(Track {
                    id: DbId(r.get(0)?),
                    title: text(r, 1)?,
                    artist: text(r, 2)?,
                    album: text(r, 3)?,
                    genre: text(r, 4)?,
                    label: text(r, 5)?,
                    remixer: text(r, 6)?,
                    composer: text(r, 7)?,
                    original_artist: text(r, 8)?,
                    key: text(r, 9)?,
                    color: text(r, 10)?.parse::<u8>().ok().filter(|c| (1..=8).contains(c)),
                    bpm_x100: positive_u32(r, 11)?,
                    length_seconds: positive_u32(r, 12)?,
                    track_no: positive_u32(r, 13)?,
                    disc_no: positive_u32(r, 14)?,
                    release_year: positive_u32(r, 15)?,
                    rating: r.get::<_, Option<i64>>(16)?.unwrap_or(0).clamp(0, 255) as u8,
                    comment: text(r, 17)?,
                    play_count: r.get::<_, Option<i64>>(18)?.unwrap_or(0).max(0) as u32,
                    bitrate: positive_u32(r, 19)?,
                    sample_rate: positive_u32(r, 20)?,
                    bit_depth: positive_u32(r, 21)?,
                    file_size: r.get::<_, Option<i64>>(22)?.filter(|v| *v > 0).map(|v| v as u64),
                    file_type: FileType::from_code(r.get::<_, Option<i64>>(23)?.unwrap_or(0)),
                    path: PathBuf::from(text(r, 24)?),
                    date_added: text(r, 25)?,
                    release_date: text(r, 26)?,
                    analysis_path: under_root(root, r.get(27)?),
                    artwork_path: under_root(root, r.get(28)?),
                    analysed: r.get::<_, Option<i64>>(29)?.unwrap_or(0).max(0) as u32,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut stmt = conn.prepare(
            "SELECT ID, ParentID, Name, Attribute, Seq FROM djmdPlaylist WHERE rb_local_deleted = 0",
        )?;
        let playlists = stmt
            .query_map([], |r| {
                let parent: Option<String> = r.get(1)?;
                Ok(PlaylistNode {
                    id: DbId(r.get(0)?),
                    parent: parent.filter(|p| p != "root" && !p.is_empty()).map(DbId),
                    name: text(r, 2)?,
                    // Attribute: 0 playlist, 1 folder, 4 smart (to be confirmed by M2 traces).
                    kind: match r.get::<_, Option<i64>>(3)?.unwrap_or(0) {
                        1 => PlaylistKind::Folder,
                        4 => PlaylistKind::Smart,
                        _ => PlaylistKind::Playlist,
                    },
                    seq: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut stmt = conn.prepare(
            "SELECT PlaylistID, ContentID FROM djmdSongPlaylist \
             WHERE rb_local_deleted = 0 ORDER BY PlaylistID, TrackNo",
        )?;
        let mut playlist_entries: HashMap<DbId, Vec<DbId>> = HashMap::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (playlist, content) = row?;
            playlist_entries.entry(DbId(playlist)).or_default().push(DbId(content));
        }

        Ok(LibrarySnapshot { tracks, playlists, playlist_entries })
    }
}
