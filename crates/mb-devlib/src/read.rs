//! Typed reads of exportLibrary.db. Spec: `docs/re/devlib.md#tables`.
//!
//! Column storage is loose: rows written by older rekordbox versions store 0 where 7.2.19 leaves
//! NULL, and the `*UpdateCount` columns hold strings, so an empty marker is stored as text ''.
//! Every accessor here therefore accepts NULL, integer, real and text and normalises.

use crate::model::*;
use crate::{DevLibDb, Result};
use rusqlite::Row;
use rusqlite::types::ValueRef;
use std::collections::HashMap;

/// Integer value of a column; NULL, '' and non-numeric text give `None`.
fn int(row: &Row, i: usize) -> rusqlite::Result<Option<i64>> {
    Ok(match row.get_ref(i)? {
        ValueRef::Null | ValueRef::Blob(_) => None,
        ValueRef::Integer(v) => Some(v),
        ValueRef::Real(v) => Some(v as i64),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse().ok()),
    })
}

/// Text value of a column; NULL gives "" and numbers are formatted.
fn text(row: &Row, i: usize) -> rusqlite::Result<String> {
    Ok(match row.get_ref(i)? {
        ValueRef::Null | ValueRef::Blob(_) => String::new(),
        ValueRef::Integer(v) => v.to_string(),
        ValueRef::Real(v) => v.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
    })
}

/// A foreign key: 0, negative and NULL mean "none".
fn id(row: &Row, i: usize) -> rusqlite::Result<u32> {
    Ok(int(row, i)?
        .filter(|v| *v > 0 && *v <= u32::MAX as i64)
        .map_or(0, |v| v as u32))
}

fn positive(row: &Row, i: usize) -> rusqlite::Result<Option<u32>> {
    Ok(int(row, i)?
        .filter(|v| *v > 0 && *v <= u32::MAX as i64)
        .map(|v| v as u32))
}

fn flag(row: &Row, i: usize) -> rusqlite::Result<bool> {
    Ok(int(row, i)?.unwrap_or(0) != 0)
}

const TRACKS_SQL: &str = "
SELECT c.content_id, c.title, c.subtitle,
       ar.name, rm.name, oa.name, co.name, ly.name, al.name, ge.name, la.name, k.name,
       c.color_id, c.bpmx100, c.length, c.trackNo, c.discNo, c.releaseYear, c.rating,
       c.djComment, c.djPlayCount, c.bitrate, c.samplingRate, c.bitDepth, c.fileSize, c.fileType,
       c.isrc, c.path, c.fileName, c.analysisDataFilePath, im.path,
       c.dateAdded, c.dateCreated, c.releaseDate, c.analysedBits, c.contentLink, c.hasModified,
       c.isHotCueAutoLoadOn, c.isKuvoDeliverStatusOn, c.kuvoDeliveryComment,
       c.masterDbId, c.masterContentId,
       c.cueUpdateCount, c.analysisDataUpdateCount, c.informationUpdateCount,
       c.artist_id_artist, c.artist_id_remixer, c.artist_id_originalArtist, c.artist_id_composer,
       c.artist_id_lyricist, c.album_id, c.genre_id, c.label_id, c.key_id, c.image_id
FROM content c
LEFT JOIN artist ar ON ar.artist_id = c.artist_id_artist
LEFT JOIN artist rm ON rm.artist_id = c.artist_id_remixer
LEFT JOIN artist oa ON oa.artist_id = c.artist_id_originalArtist
LEFT JOIN artist co ON co.artist_id = c.artist_id_composer
LEFT JOIN artist ly ON ly.artist_id = c.artist_id_lyricist
LEFT JOIN album al ON al.album_id = c.album_id
LEFT JOIN genre ge ON ge.genre_id = c.genre_id
LEFT JOIN label la ON la.label_id = c.label_id
LEFT JOIN key k ON k.key_id = c.key_id
LEFT JOIN image im ON im.image_id = c.image_id
ORDER BY c.content_id";

fn track(r: &Row) -> rusqlite::Result<DeviceTrack> {
    Ok(DeviceTrack {
        id: id(r, 0)?,
        title: text(r, 1)?,
        subtitle: text(r, 2)?,
        artist: text(r, 3)?,
        remixer: text(r, 4)?,
        original_artist: text(r, 5)?,
        composer: text(r, 6)?,
        lyricist: text(r, 7)?,
        album: text(r, 8)?,
        genre: text(r, 9)?,
        label: text(r, 10)?,
        key: text(r, 11)?,
        color: int(r, 12)?.filter(|c| (1..=8).contains(c)).map(|c| c as u8),
        bpm_x100: positive(r, 13)?,
        length_seconds: positive(r, 14)?,
        track_no: positive(r, 15)?,
        disc_no: positive(r, 16)?,
        release_year: positive(r, 17)?,
        rating: int(r, 18)?.unwrap_or(0).clamp(0, 255) as u8,
        comment: text(r, 19)?,
        play_count: int(r, 20)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
        bitrate: positive(r, 21)?,
        sample_rate: positive(r, 22)?,
        bit_depth: positive(r, 23)?,
        file_size: int(r, 24)?.filter(|v| *v > 0).map(|v| v as u64),
        file_type: int(r, 25)?.unwrap_or(0),
        isrc: text(r, 26)?,
        path: text(r, 27)?,
        file_name: text(r, 28)?,
        analysis_path: text(r, 29)?,
        artwork_path: Some(text(r, 30)?).filter(|p| !p.is_empty()),
        date_added: text(r, 31)?,
        date_created: text(r, 32)?,
        release_date: text(r, 33)?,
        analysed_bits: int(r, 34)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
        content_link: int(r, 35)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
        has_modified: flag(r, 36)?,
        hot_cue_auto_load: flag(r, 37)?,
        kuvo_deliver: flag(r, 38)?,
        kuvo_comment: text(r, 39)?,
        master_db_id: positive(r, 40)?,
        master_content_id: positive(r, 41)?,
        cue_update_count: text(r, 42)?,
        analysis_update_count: text(r, 43)?,
        information_update_count: text(r, 44)?,
        ids: TrackRefs {
            artist: id(r, 45)?,
            remixer: id(r, 46)?,
            original_artist: id(r, 47)?,
            composer: id(r, 48)?,
            lyricist: id(r, 49)?,
            album: id(r, 50)?,
            genre: id(r, 51)?,
            label: id(r, 52)?,
            key: id(r, 53)?,
            image: id(r, 54)?,
        },
    })
}

/// Tree tables share the shape (id, sequenceNo, name, attribute, parent[, image_id]).
fn list_nodes(db: &DevLibDb, sql: &str, has_image: bool) -> Result<Vec<DeviceListNode>> {
    let mut stmt = db.connection().prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(DeviceListNode {
            id: id(r, 0)?,
            seq: int(r, 1)?.unwrap_or(0),
            name: text(r, 2)?,
            attribute: ListAttribute::from_code(int(r, 3)?.unwrap_or(0)),
            parent: positive(r, 4)?,
            image_id: if has_image { positive(r, 5)? } else { None },
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// (parent, child) link tables ordered by the parent and then `sequenceNo`.
fn ordered_links(db: &DevLibDb, sql: &str) -> Result<HashMap<u32, Vec<u32>>> {
    let mut stmt = db.connection().prepare(sql)?;
    let mut out: HashMap<u32, Vec<u32>> = HashMap::new();
    for row in stmt.query_map([], |r| Ok((id(r, 0)?, id(r, 1)?)))? {
        let (parent, child) = row?;
        out.entry(parent).or_default().push(child);
    }
    Ok(out)
}

fn cue_position(r: &Row, base: usize) -> rusqlite::Result<CuePosition> {
    // Column order per side: usec, 150fps, mpeg frame, mpeg abs, decoding start, offset, samples.
    Ok(CuePosition {
        usec: int(r, base)?.unwrap_or(0),
        frame_150: int(r, base + 1)?.unwrap_or(0),
        mpeg_frame: int(r, base + 2)?.unwrap_or(0),
        mpeg_abs: int(r, base + 3)?.unwrap_or(0),
        decoding_start_frame_position: int(r, base + 4)?.unwrap_or(0),
        file_offset_in_block: int(r, base + 5)?.unwrap_or(0),
        number_of_sample_in_block: int(r, base + 6)?.unwrap_or(0),
    })
}

const CUES_SQL: &str = "
SELECT cue_id, content_id, kind, colorTableIndex, cueComment, isActiveLoop,
       beatLoopNumerator, beatLoopDenominator,
       inUsec, in150FramePerSec, inMpegFrameNumber, inMpegAbs,
       inDecodingStartFramePosition, inFileOffsetInBlock, inNumberOfSampleInBlock,
       outUsec, out150FramePerSec, outMpegFrameNumber, outMpegAbs,
       outDecodingStartFramePosition, OutFileOffsetInBlock, outNumberOfSampleInBlock
FROM cue ORDER BY content_id, cue_id";

fn cue(r: &Row) -> rusqlite::Result<DeviceCue> {
    let raw_kind = int(r, 2)?.unwrap_or(0);
    // `outUsec` = -1 marks "no out point" (setHotCue @0x10071D6C4, set_cueInfo @0x1006F8C38).
    let out_usec = int(r, 15)?.unwrap_or(-1);
    let out_point = if out_usec == -1 {
        None
    } else {
        Some(cue_position(r, 15)?)
    };
    Ok(DeviceCue {
        id: id(r, 0)?,
        content_id: id(r, 1)?,
        raw_kind,
        kind: CueKind::from_row(raw_kind, out_point.is_some()),
        color_table_index: int(r, 3)?,
        comment: text(r, 4)?,
        active_loop: flag(r, 5)?,
        beat_loop_numerator: int(r, 6)?,
        beat_loop_denominator: int(r, 7)?,
        in_point: cue_position(r, 8)?,
        out_point,
    })
}

impl DevLibDb {
    /// Row count of every table, in creation order.
    pub fn table_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY rowid")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        names
            .into_iter()
            .map(|name| {
                let sql = format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\""));
                let count = self.connection().query_row(&sql, [], |row| row.get(0))?;
                Ok((name, count))
            })
            .collect()
    }

    pub fn property(&self) -> Result<Option<DeviceProperty>> {
        let mut stmt = self.connection().prepare(
            "SELECT deviceName, dbVersion, numberOfContents, createdDate, backGroundColorType, \
             myTagMasterDBID FROM property LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        let Some(r) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(DeviceProperty {
            device_name: text(r, 0)?,
            db_version: text(r, 1)?,
            number_of_contents: id(r, 2)?,
            created_date: text(r, 3)?,
            background_color_type: id(r, 4)?,
            my_tag_master_db_id: positive(r, 5)?,
        }))
    }

    pub fn tracks(&self) -> Result<Vec<DeviceTrack>> {
        let mut stmt = self.connection().prepare(TRACKS_SQL)?;
        let rows = stmt.query_map([], track)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn playlists(&self) -> Result<Vec<DeviceListNode>> {
        list_nodes(
            self,
            "SELECT playlist_id, sequenceNo, name, attribute, playlist_id_parent, image_id \
             FROM playlist ORDER BY playlist_id_parent, sequenceNo",
            true,
        )
    }

    /// Track IDs per playlist in `sequenceNo` order (1-based in rekordbox's output).
    pub fn playlist_entries(&self) -> Result<HashMap<u32, Vec<u32>>> {
        ordered_links(
            self,
            "SELECT playlist_id, content_id FROM playlist_content ORDER BY playlist_id, sequenceNo",
        )
    }

    pub fn cues(&self) -> Result<Vec<DeviceCue>> {
        let mut stmt = self.connection().prepare(CUES_SQL)?;
        let rows = stmt.query_map([], cue)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn my_tags(&self) -> Result<Vec<DeviceListNode>> {
        list_nodes(
            self,
            "SELECT myTag_id, sequenceNo, name, attribute, myTag_id_parent \
             FROM myTag ORDER BY myTag_id_parent, sequenceNo",
            false,
        )
    }

    pub fn my_tag_content(&self) -> Result<Vec<(u32, u32)>> {
        let mut stmt = self.connection().prepare(
            "SELECT myTag_id, content_id FROM myTag_content ORDER BY myTag_id, content_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((id(r, 0)?, id(r, 1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn histories(&self) -> Result<Vec<DeviceListNode>> {
        list_nodes(
            self,
            "SELECT history_id, sequenceNo, name, attribute, history_id_parent \
             FROM history ORDER BY history_id_parent, sequenceNo",
            false,
        )
    }

    pub fn history_entries(&self) -> Result<HashMap<u32, Vec<u32>>> {
        ordered_links(
            self,
            "SELECT history_id, content_id FROM history_content ORDER BY history_id, sequenceNo",
        )
    }

    pub fn hot_cue_bank_lists(&self) -> Result<Vec<DeviceListNode>> {
        list_nodes(
            self,
            "SELECT hotCueBankList_id, sequenceNo, name, attribute, hotCueBankList_id_parent, \
             image_id FROM hotCueBankList ORDER BY hotCueBankList_id_parent, sequenceNo",
            true,
        )
    }

    /// Cue IDs per hot cue bank list, in `sequenceNo` order.
    pub fn hot_cue_bank_entries(&self) -> Result<HashMap<u32, Vec<u32>>> {
        ordered_links(
            self,
            "SELECT hotCueBankList_id, cue_id FROM hotCueBankList_cue \
             ORDER BY hotCueBankList_id, sequenceNo",
        )
    }

    pub fn menu_items(&self) -> Result<Vec<MenuItem>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT menuItem_id, kind, name FROM menuItem ORDER BY menuItem_id")?;
        let rows = stmt.query_map([], |r| {
            Ok(MenuItem {
                id: id(r, 0)?,
                kind: int(r, 1)?.unwrap_or(0),
                name: text(r, 2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn categories(&self) -> Result<Vec<MenuSetting>> {
        let mut stmt = self.connection().prepare(
            "SELECT category_id, menuItem_id, sequenceNo, isVisible FROM category ORDER BY category_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MenuSetting {
                id: id(r, 0)?,
                menu_item_id: id(r, 1)?,
                seq: int(r, 2)?.unwrap_or(0),
                visible: flag(r, 3)?,
                sub_column: false,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn sorts(&self) -> Result<Vec<MenuSetting>> {
        let mut stmt = self.connection().prepare(
            "SELECT sort_id, menuItem_id, sequenceNo, isVisible, isSelectedAsSubColumn \
             FROM sort ORDER BY sort_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MenuSetting {
                id: int(r, 0)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
                menu_item_id: id(r, 1)?,
                seq: int(r, 2)?.unwrap_or(0),
                visible: flag(r, 3)?,
                sub_column: flag(r, 4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn colors(&self) -> Result<Vec<(u32, String)>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT color_id, name FROM color ORDER BY color_id")?;
        let rows = stmt.query_map([], |r| Ok((id(r, 0)?, text(r, 1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn recommended_likes(&self) -> Result<Vec<RecommendedLike>> {
        let mut stmt = self.connection().prepare(
            "SELECT content_id_1, content_id_2, rating, createdDate FROM recommendedLike",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RecommendedLike {
                content_id_1: id(r, 0)?,
                content_id_2: id(r, 1)?,
                rating: int(r, 2)?,
                created_ms: int(r, 3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Reads every table into a [`DeviceLibrary`].
    pub fn load_library(&self) -> Result<DeviceLibrary> {
        Ok(DeviceLibrary {
            property: self.property()?,
            tracks: self.tracks()?,
            playlists: self.playlists()?,
            playlist_entries: self.playlist_entries()?,
            cues: self.cues()?,
            my_tags: self.my_tags()?,
            my_tag_content: self.my_tag_content()?,
            histories: self.histories()?,
            history_entries: self.history_entries()?,
            hot_cue_bank_lists: self.hot_cue_bank_lists()?,
            hot_cue_bank_entries: self.hot_cue_bank_entries()?,
            menu_items: self.menu_items()?,
            categories: self.categories()?,
            sorts: self.sorts()?,
            colors: self.colors()?,
            recommended_likes: self.recommended_likes()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_kind_rules() {
        assert_eq!(CueKind::from_row(0, false), CueKind::MemoryCue);
        assert_eq!(CueKind::from_row(0, true), CueKind::MemoryLoop);
        assert_eq!(CueKind::from_row(1, false), CueKind::HotCue(1));
        assert_eq!(CueKind::from_row(17, true), CueKind::HotCue(17));
        assert_eq!(CueKind::from_row(101, false), CueKind::Kind101);
        assert_eq!(CueKind::from_row(18, false), CueKind::Invalid(18));
    }

    #[test]
    fn menu_label() {
        let item = MenuItem {
            id: 1,
            kind: 128,
            name: "\u{FFFA}GENRE\u{FFFB}".into(),
        };
        assert_eq!(item.label(), "GENRE");
    }
}
