//! The Device Library Plus conversion: builds `exportLibrary.db` from the classic `export.pdb` /
//! `exportExt.pdb` of a stick that has no Plus database yet, as rekordbox 7.2.19 does.
//! Spec: `docs/re/export.md` § 6.4.
//!
//! `DatabaseMediator::execDbConversion` @0x1010e1a38 runs `clearAllData` on the new Plus database
//! and then 22 pairs `rekordboxDBController::getAllDev*` → `DevSqliteDBController::insertAllDev*`,
//! one per Plus table, in this module's table order. Each `getAllDev*` (`dsqlGetAllDev*`
//! @0x1019cb0b4…) reads one classic table sorted by its first column; each `insertAllDev*`
//! (@0x100729ad0…) turns the rows into one multi-row insert, keeping the classic IDs. The value
//! types follow the `juce::var`s rekordbox builds (integers from numeric columns and casts, text
//! from string columns), so SQLite's column affinity gives the same storage classes.
//!
//! `deviceSqliteDbConversion` @0x1010def78 then copies the artwork: for every Plus `image` row
//! with a path, `a<id>.jpg` → `b<id>.jpg` and, when that worked, `a<id>_m.jpg` → `b<id>_m.jpg`.
//!
//! Planning is pure: [`plan_conversion`] maps the classic rows; [`Conversion::devlib_state`]
//! builds the planner's model of the converted database (the same rows in an in-memory SQLite
//! database, read back like a real stick); [`Conversion::write`] inserts the rows through
//! [`DevLibWriter::insert_all`]; [`copy_artwork`] does the file work.

use crate::device::{DevlibState, PdbFiles, load_devlib_from};
use crate::{Error, Result};
use mb_devlib::{DevLibWriter, MenuSeed};
use mb_pdb::tables::{CueOption, HotCueBankPoint, Track};
use mb_pdb::{ExportDb, ExtDb};
use rusqlite::types::Value;
use std::path::{Path, PathBuf};

/// The rows of one Plus table, in insert order.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertedTable {
    pub table: &'static str,
    pub columns: &'static [&'static str],
    pub rows: Vec<Vec<Value>>,
}

/// One artwork copy of the conversion (device paths, e.g. `/PIONEER/Artwork/00001/a1.jpg` →
/// `.../b1.jpg`); the `_m` variant is copied after the base file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkCopy {
    pub from: String,
    pub to: String,
}

/// Everything the conversion writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversion {
    /// In `execDbConversion` order; a table whose classic source is empty is absent
    /// (`dsqlGetAllDev*` does not set its key, so `insertAllDev*` inserts nothing).
    pub tables: Vec<ConvertedTable>,
    pub artwork: Vec<ArtworkCopy>,
    /// The `initPropertyTable` seed `startupDeviceDB` writes before the conversion drops every
    /// table again (only the page layout depends on it): the local library's menus.
    pub menus: MenuSeed,
    /// `createdDate` of that dropped seed row.
    pub today: String,
}

const CONTENT: &[&str] = &[
    "content_id",
    "title",
    "titleForSearch",
    "subtitle",
    "bpmx100",
    "length",
    "trackNo",
    "discNo",
    "artist_id_artist",
    "artist_id_remixer",
    "artist_id_originalArtist",
    "artist_id_composer",
    "artist_id_lyricist",
    "album_id",
    "genre_id",
    "label_id",
    "key_id",
    "color_id",
    "image_id",
    "djComment",
    "rating",
    "releaseYear",
    "releaseDate",
    "dateCreated",
    "dateAdded",
    "path",
    "fileName",
    "fileSize",
    "fileType",
    "bitrate",
    "bitDepth",
    "samplingRate",
    "isrc",
    "djPlayCount",
    "isHotCueAutoLoadOn",
    "isKuvoDeliverStatusOn",
    "kuvoDeliveryComment",
    "masterDbId",
    "masterContentId",
    "analysisDataFilePath",
    "analysedBits",
    "contentLink",
    "hasModified",
    "cueUpdateCount",
    "analysisDataUpdateCount",
    "informationUpdateCount",
];

const CUE: &[&str] = &[
    "cue_id",
    "content_id",
    "kind",
    "colorTableIndex",
    "cueComment",
    "isActiveLoop",
    "beatLoopNumerator",
    "beatLoopDenominator",
    "inUsec",
    "outUsec",
    "in150FramePerSec",
    "out150FramePerSec",
    "inMpegFrameNumber",
    "outMpegFrameNumber",
    "inMpegAbs",
    "outMpegAbs",
    "inDecodingStartFramePosition",
    "outDecodingStartFramePosition",
    "inFileOffsetInBlock",
    "OutFileOffsetInBlock",
    "inNumberOfSampleInBlock",
    "outNumberOfSampleInBlock",
];

fn int(v: impl Into<i64>) -> Value {
    Value::Integer(v.into())
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

/// `juce::String::getLargeIntValue`: optional leading whitespace and sign, then decimal digits up
/// to the first other character; 0 when there are none.
fn juce_large_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        v = v.wrapping_mul(10).wrapping_add(i64::from(b - b'0'));
    }
    if neg { v.wrapping_neg() } else { v }
}

/// `juce::var::operator int` on a string `var` (`String::getIntValue`).
fn juce_int(s: &str) -> i64 {
    i64::from(juce_large_int(s) as i32)
}

/// A classic byte column as `dsqlGetAllDev*` reads it: `(int)*(char *)` (signed on arm64 Apple).
fn signed_byte(v: u8) -> i64 {
    i64::from(v as i8)
}

/// The ISRC as `dsqlGetAllDevContent` reads RESERVED9: the special form `0x03` + 12 bytes, or
/// nothing.
fn content_isrc(t: &Track) -> String {
    match t.isrc_raw.isrc() {
        Some(isrc) if isrc.len() == 12 => isrc,
        _ => String::new(),
    }
}

/// `insertAllDevContent` @0x100729ad0 from `dsqlGetAllDevContent` @0x1019cb0b4.
fn content_row(t: &Track) -> Vec<Value> {
    vec![
        int(t.id),
        text(&t.title),
        text(&t.search_str),
        text(&t.subtitle),
        // `operator int` of the u32/u16 columns.
        int(t.bpm_x100 as i32),
        int(t.length_s),
        int(t.track_no as i32),
        int(t.disc_no),
        int(t.artist_id),
        int(t.remixer_id),
        int(t.original_artist_id),
        int(t.composer_id),
        // RESERVED8 (lyricist ID, text) through `operator int`.
        int(juce_int(&t.lyricist)),
        int(t.album_id),
        int(t.genre_id),
        int(t.label_id),
        int(t.key_id),
        int(signed_byte(t.color_id)),
        int(t.artwork_id),
        text(&t.comment),
        int(signed_byte(t.rating)),
        int(t.release_year),
        text(&t.release_date),
        text(&t.date_created),
        // `dateAdded` ← STOCKDATE.
        text(&t.stock_date),
        text(&t.folder_path),
        text(&t.file_name),
        int(t.file_size as i32),
        int(signed_byte(t.file_type)),
        int(t.bitrate as i32),
        int(t.bit_depth),
        int(t.sample_rate as i32),
        text(&content_isrc(t)),
        int(t.play_count),
        // RESERVED2 "ON" → 1.
        int(i64::from(t.hot_cue_auto_load == "ON")),
        // RESERVED3 through `operator int` ("ON" → 0).
        int(juce_int(&t.delivery_control)),
        text(&t.delivery_comment),
        int(t.master_db_id),
        int(t.master_content_id),
        text(&t.analysis_path),
        int(signed_byte(t.analysed)),
        int(t.content_link as i32),
        // RESERVED1 non-empty → 1.
        int(i64::from(!t.modified_by_rbm.is_empty())),
        int(juce_int(&t.cue_updated)),
        int(juce_int(&t.analysis_updated)),
        int(juce_int(&t.track_info_updated)),
    ]
}

/// `insertAllDevImage` @0x10073337c: a path whose file name starts with `a` becomes the same
/// folder with `b`; any other path becomes `""`.
fn plus_image_path(classic: &str) -> String {
    let (dir, name) = match classic.rfind('/') {
        Some(i) => (&classic[..=i], &classic[i + 1..]),
        None => ("", classic),
    };
    match name.strip_prefix('a') {
        Some(rest) => format!("{dir}b{rest}"),
        None => String::new(),
    }
}

/// `insertAllDevMenuItems` @0x100735cf8: `getCategoryItemName` strips the wrapper, the insert
/// wraps the key again in U+FFFA … U+FFFB.
fn plus_menu_name(classic: &str) -> String {
    let key = classic.strip_prefix('\u{FFFA}').unwrap_or(classic);
    let key = key.strip_suffix('\u{FFFB}').unwrap_or(key);
    mb_devlib::schema::menu_item_name(key)
}

/// `insertAllDevCue` @0x100733b28: a hot cue bank point with the cue option at the same index
/// (rekordbox compares `djdbexCueOption[i]` with point `i` only, both sorted by ID).
fn cue_row(p: &HotCueBankPoint, opt: Option<&CueOption>) -> Vec<Value> {
    let seek = |s: &str| -> [i64; 3] {
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() == 3 {
            [
                juce_large_int(parts[0]),
                juce_large_int(parts[1]),
                juce_large_int(parts[2]),
            ]
        } else {
            [0; 3]
        }
    };
    let (color_index, comment, active_loop, beat_size, micro, sin, sout) = match opt {
        Some(o) => (
            signed_byte(o.reserved1),
            o.comment.clone(),
            i64::from(o.reserved2 as i32),
            o.beat_size,
            o.cue_micro,
            seek(&o.in_seek_info),
            seek(&o.out_seek_info),
        ),
        None => (0, String::new(), 0, 0, 0, [0; 3], [0; 3]),
    };
    let (num, den) = if beat_size < 0x10000 || beat_size & 0xffff == 0 {
        (0, 0)
    } else {
        (i64::from(beat_size >> 16), i64::from(beat_size & 0xffff))
    };
    let in_usec = (p.in_msec as i32)
        .wrapping_mul(1000)
        .wrapping_add((micro & 0xffff) as i32) as u32;
    let out_msec = p.out_msec as i32;
    let out = if out_msec == -1 {
        u32::MAX
    } else {
        ((micro >> 16) & 0xffff).wrapping_add(out_msec.wrapping_mul(1000) as u32)
    };
    let out_usec = if out == u32::MAX { -1 } else { i64::from(out) };
    vec![
        int(p.id),
        int(p.track_id),
        int(signed_byte(p.kind)),
        int(color_index),
        Value::Text(comment),
        int(active_loop),
        int(num),
        int(den),
        int(in_usec),
        int(out_usec),
        int(p.in_frame),
        int(p.out_frame),
        int(p.in_mpeg_frame),
        int(p.out_mpeg_frame),
        int(p.in_mpeg_abs),
        int(p.out_mpeg_abs),
        int(sin[0]),
        int(sout[0]),
        int(sin[1]),
        int(sout[1]),
        int(sin[2] & 0xffff_ffff),
        int(sout[2] & 0xffff_ffff),
    ]
}

/// Sorted by the first column, like `_edb_dyn_SortRowSet_(set, 1, &col0, &ascending)`.
/// **Hypothesis:** the sort is stable over the table scan order (only visible in the rowids of
/// tables that repeat the first column, e.g. `playlist_content`).
fn sorted<T: Clone, K: Ord>(rows: &[T], key: impl Fn(&T) -> K) -> Vec<T> {
    let mut v = rows.to_vec();
    v.sort_by_key(key);
    v
}

/// Maps the classic databases to the Plus rows (pure).
pub fn plan_conversion(pdb: &PdbFiles, menus: MenuSeed, today: &str) -> Conversion {
    let e: &ExportDb = &pdb.export;
    let empty = ExtDb::default();
    let x: &ExtDb = pdb.ext.as_ref().unwrap_or(&empty);
    let mut tables = Vec::new();
    let mut push =
        |table: &'static str, columns: &'static [&'static str], rows: Vec<Vec<Value>>| {
            if !rows.is_empty() {
                tables.push(ConvertedTable {
                    table,
                    columns,
                    rows,
                });
            }
        };

    push(
        "content",
        CONTENT,
        sorted(&e.tracks, |t| t.id)
            .iter()
            .map(content_row)
            .collect(),
    );
    push(
        "genre",
        &["genre_id", "name"],
        sorted(&e.genres, |g| g.0.id)
            .iter()
            .map(|g| vec![int(g.0.id), text(&g.0.name)])
            .collect(),
    );
    push(
        "artist",
        &["artist_id", "name", "nameForSearch"],
        sorted(&e.artists, |a| a.id)
            .iter()
            .map(|a| vec![int(a.id), text(&a.name), text(&a.search_str)])
            .collect(),
    );
    push(
        "album",
        &[
            "album_id",
            "name",
            "artist_id",
            "image_id",
            "isComplation",
            "nameForSearch",
        ],
        sorted(&e.albums, |a| a.id)
            .iter()
            .map(|a| {
                vec![
                    int(a.id),
                    text(&a.name),
                    int(a.album_artist_id),
                    int(a.artwork_id),
                    int(signed_byte(a.compilation)),
                    text(&a.search_str),
                ]
            })
            .collect(),
    );
    push(
        "label",
        &["label_id", "name"],
        sorted(&e.labels, |l| l.0.id)
            .iter()
            .map(|l| vec![int(l.0.id), text(&l.0.name)])
            .collect(),
    );
    push(
        "key",
        &["key_id", "name"],
        sorted(&e.keys, |k| k.id)
            .iter()
            .map(|k| vec![int(k.id), text(&k.name)])
            .collect(),
    );
    push(
        "color",
        &["color_id", "name"],
        sorted(&e.colors, |c| signed_byte(c.id))
            .iter()
            .map(|c| vec![int(signed_byte(c.id)), text(&c.name)])
            .collect(),
    );
    push(
        "playlist",
        &[
            "playlist_id",
            "sequenceNo",
            "name",
            "image_id",
            "attribute",
            "playlist_id_parent",
        ],
        sorted(&e.playlists, |p| p.0.id)
            .iter()
            .map(|p| {
                let n = &p.0;
                vec![
                    int(n.id),
                    int(n.seq as i32),
                    text(&n.name),
                    int(n.artwork_id),
                    int(signed_byte(n.attribute)),
                    int(n.parent_id),
                ]
            })
            .collect(),
    );
    push(
        "playlist_content",
        &["playlist_id", "content_id", "sequenceNo"],
        sorted(&e.playlist_entries, |p| p.playlist_id)
            .iter()
            .map(|p| {
                vec![
                    int(p.playlist_id),
                    int(p.track_id),
                    int(p.entry_index as i32),
                ]
            })
            .collect(),
    );
    push(
        "hotCueBankList",
        &[
            "hotCueBankList_id",
            "sequenceNo",
            "name",
            "image_id",
            "attribute",
            "hotCueBankList_id_parent",
        ],
        sorted(&e.hot_cue_bank_lists, |p| p.0.id)
            .iter()
            .map(|p| {
                let n = &p.0;
                vec![
                    int(n.id),
                    int(n.seq as i32),
                    text(&n.name),
                    int(n.artwork_id),
                    int(signed_byte(n.attribute)),
                    int(n.parent_id),
                ]
            })
            .collect(),
    );
    push(
        "hotCueBankList_cue",
        &["hotCueBankList_id", "cue_id", "sequenceNo"],
        sorted(&e.hot_cue_bank_list_entries, |h| h.bank_list_id)
            .iter()
            .map(|h| {
                vec![
                    int(h.bank_list_id),
                    int(h.cue_id),
                    int(h.entry_index as i32),
                ]
            })
            .collect(),
    );
    // `insertAllDevHistory` @0x1007321c8: name from the classic list, sequence, attribute and
    // parent from `djdbexHistory[i]` when its ID equals the classic list `i`'s, else 0.
    let ext_histories = sorted(&x.histories, |h| h.id);
    push(
        "history",
        &[
            "history_id",
            "sequenceNo",
            "name",
            "attribute",
            "history_id_parent",
        ],
        sorted(&e.histories, |h| h.0.id)
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let (seq, attribute, parent) = match ext_histories.get(i) {
                    Some(x) if x.id == h.0.id => (
                        i64::from(x.seq as i32),
                        signed_byte(x.attribute),
                        i64::from(x.parent_id),
                    ),
                    _ => (0, 0, 0),
                };
                vec![
                    int(h.0.id),
                    int(seq),
                    text(&h.0.name),
                    int(attribute),
                    int(parent),
                ]
            })
            .collect(),
    );
    push(
        "history_content",
        &["history_id", "content_id", "sequenceNo"],
        sorted(&e.history_entries, |h| h.history_id)
            .iter()
            .map(|h| vec![int(h.history_id), int(h.track_id), int(h.seq)])
            .collect(),
    );
    let images: Vec<Vec<Value>> = sorted(&e.artwork, |a| a.id)
        .iter()
        .map(|a| vec![int(a.id), text(&plus_image_path(&a.path))])
        .collect();
    // `deviceSqliteDbConversion`: `getAllImagePath` (path not NULL and not "") of the new
    // database, each copied from the `a` file next to it.
    let artwork = sorted(&e.artwork, |a| a.id)
        .iter()
        .filter_map(|a| {
            let to = plus_image_path(&a.path);
            (!to.is_empty()).then(|| ArtworkCopy {
                from: a.path.clone(),
                to,
            })
        })
        .collect();
    push("image", &["image_id", "path"], images);
    let points = sorted(&e.hot_cue_bank_points, |p| p.id);
    let options = sorted(&x.cue_options, |o| o.cue_id);
    push(
        "cue",
        CUE,
        points
            .iter()
            .enumerate()
            .map(|(i, p)| cue_row(p, options.get(i).filter(|o| o.cue_id == p.id)))
            .collect(),
    );
    push(
        "menuItem",
        &["menuItem_id", "kind", "name"],
        sorted(&e.menu_items, |m| m.id)
            .iter()
            .map(|m| {
                vec![
                    int(m.id),
                    int(signed_byte(m.class) & 0xff),
                    text(&plus_menu_name(&m.name)),
                ]
            })
            .collect(),
    );
    push(
        "category",
        &["category_id", "menuItem_id", "sequenceNo", "isVisible"],
        sorted(&e.categories, |c| c.id)
            .iter()
            .map(|c| {
                vec![
                    int(c.id),
                    int(c.menu_item_id),
                    int(signed_byte(c.seq)),
                    int(!signed_byte(c.disable) & 1),
                ]
            })
            .collect(),
    );
    push(
        "sort",
        &[
            "sort_id",
            "menuItem_id",
            "sequenceNo",
            "isVisible",
            "isSelectedAsSubColumn",
        ],
        sorted(&e.sorts, |s| s.id)
            .iter()
            .map(|s| {
                let d = signed_byte(s.disable);
                vec![
                    int(s.id),
                    int(s.menu_item_id),
                    int(signed_byte(s.seq)),
                    int(!d & 1),
                    int((d >> 1) & 1),
                ]
            })
            .collect(),
    );
    // `insertAllDevProperty` @0x100737658: the first classic property row and
    // `djdbexDBPropertyOption[0].MyTagMasterDBID` (0 without one).
    let my_tag_master = x
        .db_property_options
        .first()
        .map_or(0, |o| i64::from(o.my_tag_master_db_id));
    push(
        "property",
        &[
            "deviceName",
            "dbVersion",
            "numberOfContents",
            "createdDate",
            "backGroundColorType",
            "myTagMasterDBID",
        ],
        e.properties
            .first()
            .map(|p| {
                vec![
                    text(&p.device_name),
                    text(&p.db_version),
                    int(p.track_count as i32),
                    text(&p.date_created),
                    int(signed_byte(p.color_id)),
                    int(my_tag_master),
                ]
            })
            .into_iter()
            .collect(),
    );
    // `insertAllDevRecommendLike` @0x100738168: `createdDate` is `(H << 32) | H | L` (the extra
    // `| H` is in the binary: `lsl x19, x26, #32; orr x8, x19, x26; orr x26, x8, x0`).
    push(
        "recommendedLike",
        &["content_id_1", "content_id_2", "rating", "createdDate"],
        x.recommend_likes
            .iter()
            .map(|l| {
                let h = i64::from(l.date_created_hi);
                let lo = i64::from(l.date_created_lo);
                vec![
                    int(l.track_id1),
                    int(l.track_id2),
                    int(signed_byte(l.like_rate)),
                    int((h << 32) | h | lo),
                ]
            })
            .collect(),
    );
    push(
        "myTag",
        &[
            "myTag_id",
            "sequenceNo",
            "name",
            "attribute",
            "myTag_id_parent",
        ],
        sorted(&x.tags, |t| t.id)
            .iter()
            .map(|t| {
                vec![
                    int(t.id),
                    int(t.seq as i32),
                    text(&t.name),
                    int(signed_byte(t.attribute)),
                    int(t.parent_id),
                ]
            })
            .collect(),
    );
    push(
        "myTag_content",
        &["myTag_id", "content_id"],
        sorted(&x.tag_tracks, |t| t.tag_id)
            .iter()
            .map(|t| vec![int(t.tag_id), int(t.track_id)])
            .collect(),
    );

    Conversion {
        tables,
        artwork,
        menus,
        today: today.to_owned(),
    }
}

/// Inserts like [`DevLibWriter::insert_all`] (chunked multi-row inserts).
fn insert_chunked(
    conn: &rusqlite::Connection,
    table: &str,
    columns: &[&str],
    rows: &[Vec<Value>],
) -> Result<()> {
    let per_chunk = (32_000 / columns.len().max(1)).max(1);
    for chunk in rows.chunks(per_chunk) {
        let placeholders = format!("({})", vec!["?"; columns.len()].join(", "));
        let sql = format!(
            "insert into {table}({}) values {}",
            columns.join(", "),
            vec![placeholders; chunk.len()].join(", ")
        );
        conn.execute(&sql, rusqlite::params_from_iter(chunk.iter().flatten()))?;
    }
    Ok(())
}

impl Conversion {
    /// The planner's model of the converted database: the rows in an in-memory SQLite database
    /// with the real schema, read back like a stick's `exportLibrary.db`.
    pub fn devlib_state(&self) -> Result<DevlibState> {
        let conn = rusqlite::Connection::open_in_memory()?;
        for (_, ddl) in mb_devlib::schema::TABLES {
            conn.execute_batch(ddl)?;
        }
        for t in &self.tables {
            insert_chunked(&conn, t.table, t.columns, &t.rows)?;
        }
        load_devlib_from(&conn)
    }

    /// Inserts every converted table into a database made by
    /// [`DevLibWriter::create_for_conversion`] (each insert commits on its own, as in rekordbox).
    pub fn write(&self, w: &DevLibWriter) -> Result<()> {
        for t in &self.tables {
            w.insert_all(t.table, t.columns, &t.rows)?;
        }
        Ok(())
    }

    /// Rows of one table.
    pub fn table(&self, name: &str) -> Option<&ConvertedTable> {
        self.tables.iter().find(|t| t.table == name)
    }
}

/// `upToLastOccurrenceOf(".") + suffix + fromLastOccurrenceOf(".", inclusive)`.
fn with_suffix(path: &str, suffix: &str) -> String {
    match path.rfind('.') {
        Some(i) => format!("{}{suffix}{}", &path[..i], &path[i..]),
        None => format!("{path}{suffix}"),
    }
}

/// The artwork copies of `deviceSqliteDbConversion`: `a<id>.jpg` → `b<id>.jpg` (replacing an
/// existing file, like `juce::File::copyFileTo`) and, only if that worked, `a<id>_m.jpg` →
/// `b<id>_m.jpg`. A missing `a` file is skipped. Returns the files written.
pub fn copy_artwork(root: &Path, conv: &Conversion) -> Result<Vec<PathBuf>> {
    let fs = |p: &str| root.join(p.trim_start_matches('/'));
    let mut written = Vec::new();
    for copy in &conv.artwork {
        for (i, suffix) in ["", "_m"].iter().enumerate() {
            let from = fs(&with_suffix(&copy.from, suffix));
            let to = fs(&with_suffix(&copy.to, suffix));
            if !from.is_file() {
                break;
            }
            let bytes = std::fs::read(&from).map_err(|e| Error::io(&from, e))?;
            let ok = std::fs::write(&to, &bytes).is_ok();
            if ok {
                written.push(to);
            } else if i == 0 {
                break;
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn juce_number_parsing() {
        assert_eq!(juce_int("ON"), 0);
        assert_eq!(juce_int(""), 0);
        assert_eq!(juce_int("12"), 12);
        assert_eq!(juce_int(" -3x"), -3);
        assert_eq!(juce_large_int("2328226057"), 2_328_226_057);
        assert_eq!(juce_int("2328226057"), 2_328_226_057_i64 as i32 as i64);
    }

    #[test]
    fn image_paths() {
        assert_eq!(
            plus_image_path("/PIONEER/Artwork/00001/a1.jpg"),
            "/PIONEER/Artwork/00001/b1.jpg"
        );
        assert_eq!(plus_image_path("/PIONEER/Artwork/00001/x1.jpg"), "");
        assert_eq!(
            with_suffix("/PIONEER/Artwork/00001/a1.jpg", "_m"),
            "/PIONEER/Artwork/00001/a1_m.jpg"
        );
    }

    #[test]
    fn menu_names_are_rewrapped() {
        assert_eq!(
            plus_menu_name("\u{FFFA}GENRE\u{FFFB}"),
            "\u{FFFA}GENRE\u{FFFB}"
        );
        assert_eq!(plus_menu_name("GENRE"), "\u{FFFA}GENRE\u{FFFB}");
    }

    #[test]
    fn cue_times() {
        let p = HotCueBankPoint {
            id: 7,
            track_id: 3,
            in_msec: 1500,
            in_frame: 0,
            in_mpeg_frame: 0,
            in_mpeg_abs: 0,
            out_msec: u32::MAX,
            out_frame: 0,
            out_mpeg_frame: 0,
            out_mpeg_abs: 0,
            kind: 0,
        };
        let row = cue_row(&p, None);
        assert_eq!(row[8], Value::Integer(1_500_000));
        assert_eq!(row[9], Value::Integer(-1));
    }
}
