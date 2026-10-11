//! Typed rows for every table of export.pdb and exportExt.pdb. Spec: `docs/re/pdb.md` § Tables.
//!
//! Field names follow the DeviceSQL column names; doc comments give the meaning and its status.
//! Numbers are kept as stored (unsigned, no unit conversion). Reserved columns are kept so
//! nothing read from the file is dropped.

use crate::page::Pdb;
use crate::row::{Row, Value};
use crate::schema::{DbKind, TableSchema};
use crate::string::DeviceString;
use crate::{Error, Result};

/// A typed view of one table.
pub trait TypedRow: Sized {
    const DB: DbKind;
    const TABLE_TYPE: u32;
    fn from_row(row: &Row) -> Self;
    /// One value per schema column in declaration order, as the writer stores them
    /// ([`crate::encode::encode_row`]). Inverse of [`TypedRow::from_row`] for rows rekordbox
    /// wrote: strings are re-encoded with rekordbox's form choice ([`DeviceString::from_text`]).
    fn to_values(&self) -> Vec<Value>;
}

/// Builds a value list by column name, so the `to_values` implementations mirror `read`.
struct Values {
    schema: &'static TableSchema,
    values: Vec<Option<Value>>,
}

impl Values {
    fn new(db: DbKind, table_type: u32) -> Self {
        let schema = db.table(table_type).expect("static schema");
        Self {
            schema,
            values: vec![None; schema.columns.len()],
        }
    }

    fn index(&self, name: &str) -> usize {
        self.schema
            .column_index(name)
            .unwrap_or_else(|| panic!("{}.{name} is not a column", self.schema.name))
    }

    fn int(mut self, name: &str, v: u32) -> Self {
        let i = self.index(name);
        self.values[i] = Some(match self.schema.columns[i].ty.size() {
            Some(1) => Value::U8(v as u8),
            Some(2) => Value::U16(v as u16),
            Some(_) => Value::U32(v),
            None => panic!("{}.{name} is a string column", self.schema.name),
        });
        self
    }

    fn string(mut self, name: &str, s: DeviceString) -> Self {
        let i = self.index(name);
        assert!(
            self.schema.columns[i].ty.is_variable(),
            "{}.{name} is not a string column",
            self.schema.name
        );
        self.values[i] = Some(Value::Str(s));
        self
    }

    fn text(self, name: &str, s: &str) -> Self {
        self.string(name, DeviceString::from_text(s))
    }

    fn finish(self) -> Vec<Value> {
        let name = self.schema.name;
        self.values
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.unwrap_or_else(|| panic!("{name}.{} has no value", self.schema.columns[i].name))
            })
            .collect()
    }
}

/// Decodes every live row of a table with its schema.
pub fn read_rows(pdb: &Pdb, db: DbKind, table_type: u32) -> Result<Vec<Row>> {
    let schema = db
        .table(table_type)
        .ok_or(Error::UnknownTable(table_type))?;
    if pdb.header.table(table_type).is_none() {
        return Ok(Vec::new());
    }
    let layout = schema.layout();
    pdb.live_rows(table_type)?
        .into_iter()
        .map(|(page, slot, bytes)| Row::decode(schema, &layout, page, slot, bytes))
        .collect()
}

/// Decodes every live row of a table into its typed form.
pub fn read_table<T: TypedRow>(pdb: &Pdb) -> Result<Vec<T>> {
    Ok(read_rows(pdb, T::DB, T::TABLE_TYPE)?
        .iter()
        .map(T::from_row)
        .collect())
}

macro_rules! typed {
    ($ty:ident, $db:ident, $table:expr) => {
        impl TypedRow for $ty {
            const DB: DbKind = DbKind::$db;
            const TABLE_TYPE: u32 = $table;
            fn from_row(r: &Row) -> Self {
                Self::read(r)
            }
            fn to_values(&self) -> Vec<Value> {
                self.write(Values::new(DbKind::$db, $table)).finish()
            }
        }
    };
}

fn u8_of(r: &Row, name: &str) -> u8 {
    r.uint(name) as u8
}

fn u16_of(r: &Row, name: &str) -> u16 {
    r.uint(name) as u16
}

/// Type 0 `DJDBCONTENT`: one row per exported track. Columns mirror the first 49 columns of
/// master.db `djmdContent` (see `docs/re/pdb.md` § tracks for the provenance of each one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: u32,
    /// Device path of the audio file, e.g. `/Contents/<artist>/<album>/<file>`.
    pub folder_path: String,
    /// File name (`FileNameL`).
    pub file_name: String,
    /// Short file name (`FileNameS`); rekordbox 7 passes NULL, stored as "".
    pub file_name_short: String,
    pub title: String,
    pub artist_id: u32,
    pub album_id: u32,
    pub genre_id: u32,
    /// Tempo × 100.
    pub bpm_x100: u32,
    /// Duration in seconds.
    pub length_s: u16,
    pub track_no: u32,
    /// kbit/s (0 for FLAC in the fixture).
    pub bitrate: u32,
    pub bit_depth: u16,
    pub comment: String,
    /// `djmdContent.FileType`: 1 mp3, 4 m4a, 5 flac, 6 alac, 11 wav, 12 aiff.
    pub file_type: u8,
    /// 0..=5 stars.
    pub rating: u8,
    pub release_year: u16,
    pub remixer_id: u32,
    pub label_id: u32,
    pub original_artist_id: u32,
    pub key_id: u32,
    /// `djmdContent.StockDate` (YYYY-MM-DD): date added to the collection.
    pub stock_date: String,
    /// rekordbox colour slot 1..=8, 0 = none.
    pub color_id: u8,
    pub play_count: u16,
    /// `artwork` table ID (type 13), 0 = none.
    pub artwork_id: u32,
    /// `djmdContent.MasterDBID` of the source library.
    pub master_db_id: u32,
    /// `djmdContent.MasterSongID` of the source library.
    pub master_content_id: u32,
    /// Device path of `ANLZ0000.DAT`.
    pub analysis_path: String,
    /// `SearchStr`; NULL from rekordbox 7, stored as "".
    pub search_str: String,
    pub file_size: u32,
    pub disc_no: u16,
    pub composer_id: u32,
    /// `Subtitle` (mix name).
    pub subtitle: String,
    /// Hz.
    pub sample_rate: u32,
    pub disable_quantize: u8,
    /// `djmdContent.Analysed & 0x3f`.
    pub analysed: u8,
    pub release_date: String,
    /// `djmdContent.DateCreated` (YYYY-MM-DD).
    pub date_created: String,
    pub content_link: u32,
    /// `Tag`; NULL from rekordbox 7.
    pub tag: String,
    /// RESERVED1 = `ModifiedByRBM`.
    pub modified_by_rbm: String,
    /// RESERVED2 = `HotCueAutoLoad` ("ON" or "").
    pub hot_cue_auto_load: String,
    /// RESERVED3 = `DeliveryControl` ("ON" or "").
    pub delivery_control: String,
    /// RESERVED4 = `DeliveryComment`.
    pub delivery_comment: String,
    /// RESERVED5 = `CueUpdated` (decimal counter as text).
    pub cue_updated: String,
    /// RESERVED6 = `AnalysisUpdated` (decimal counter as text).
    pub analysis_updated: String,
    /// RESERVED7 = `TrackInfoUpdated` (decimal counter as text).
    pub track_info_updated: String,
    /// RESERVED8 = `Lyricist`.
    pub lyricist: String,
    /// RESERVED9 = `ISRC` (decoded from the special 0x90 form).
    pub isrc: String,
    /// RESERVED9 exactly as stored.
    pub isrc_raw: DeviceString,
    /// RESERVED10, unused ("").
    pub reserved10: String,
}

impl Track {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            folder_path: r.text("FOLDERPATH"),
            file_name: r.text("FILENAMEL"),
            file_name_short: r.text("FILENAMES"),
            title: r.text("TITLE"),
            artist_id: r.uint("ARTISTID"),
            album_id: r.uint("ALBUMID"),
            genre_id: r.uint("GENREID"),
            bpm_x100: r.uint("BPM"),
            length_s: u16_of(r, "LENGTH"),
            track_no: r.uint("TRACKNO"),
            bitrate: r.uint("BITRATE"),
            bit_depth: u16_of(r, "BITDEPTH"),
            comment: r.text("COMMNT"),
            file_type: u8_of(r, "FILETYPE"),
            rating: u8_of(r, "RATING"),
            release_year: u16_of(r, "RELEASEYEAR"),
            remixer_id: r.uint("REMIXERID"),
            label_id: r.uint("LABELID"),
            original_artist_id: r.uint("ORGARTISTID"),
            key_id: r.uint("KEYID"),
            stock_date: r.text("STOCKDATE"),
            color_id: u8_of(r, "COLORID"),
            play_count: u16_of(r, "DJPLAYCOUNT"),
            artwork_id: r.uint("IMAGEID"),
            master_db_id: r.uint("MASTERDBID"),
            master_content_id: r.uint("MASTERCONTENTID"),
            analysis_path: r.text("ANALYSISDATAPATH"),
            search_str: r.text("SEARCHSTR"),
            file_size: r.uint("FILESIZE"),
            disc_no: u16_of(r, "DISCNO"),
            composer_id: r.uint("COMPOSERID"),
            subtitle: r.text("SUBTITLE"),
            sample_rate: r.uint("SAMPLERATE"),
            disable_quantize: u8_of(r, "DISABLEQUANTIZE"),
            analysed: u8_of(r, "ANALYSED"),
            release_date: r.text("RELEASEDATE"),
            date_created: r.text("DATECREATED"),
            content_link: r.uint("CONTENTLINK"),
            tag: r.text("TAG"),
            modified_by_rbm: r.text("RESERVED1"),
            hot_cue_auto_load: r.text("RESERVED2"),
            delivery_control: r.text("RESERVED3"),
            delivery_comment: r.text("RESERVED4"),
            cue_updated: r.text("RESERVED5"),
            analysis_updated: r.text("RESERVED6"),
            track_info_updated: r.text("RESERVED7"),
            lyricist: r.text("RESERVED8"),
            isrc: r.text("RESERVED9"),
            isrc_raw: r.string("RESERVED9").clone(),
            reserved10: r.text("RESERVED10"),
        }
    }
}
typed!(Track, Export, 0);

/// Types 1 `DJDBGENRE`, 4 `DJDBLABEL`, 11 `DJDBHISTORY`: an ID and a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub id: u32,
    pub name: String,
}

impl Named {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            name: r.text("NAME"),
        }
    }
}

/// Type 1 `DJDBGENRE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre(pub Named);
impl Genre {
    fn read(r: &Row) -> Self {
        Self(Named::read(r))
    }
}
typed!(Genre, Export, 1);

/// Type 4 `DJDBLABEL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label(pub Named);
impl Label {
    fn read(r: &Row) -> Self {
        Self(Named::read(r))
    }
}
typed!(Label, Export, 4);

/// Type 11 `DJDBHISTORY`: one history playlist ("HISTORY 001", ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPlaylist(pub Named);
impl HistoryPlaylist {
    fn read(r: &Row) -> Self {
        Self(Named::read(r))
    }
}
typed!(HistoryPlaylist, Export, 11);

/// Type 2 `DJDBARTIST`. Used for artist, remixer, original artist, composer and album artist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub id: u32,
    pub name: String,
    /// NULL from rekordbox 7 ("").
    pub search_str: String,
}
impl Artist {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            name: r.text("NAME"),
            search_str: r.text("SEARCHSTR"),
        }
    }
}
typed!(Artist, Export, 2);

/// Type 3 `DJDBALBUM`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub id: u32,
    pub name: String,
    pub album_artist_id: u32,
    pub artwork_id: u32,
    pub compilation: u8,
    pub search_str: String,
}
impl Album {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            name: r.text("NAME"),
            album_artist_id: r.uint("ALBUMARTISTID"),
            artwork_id: r.uint("IMAGEID"),
            compilation: u8_of(r, "COMPILATION"),
            search_str: r.text("SEARCHSTR"),
        }
    }
}
typed!(Album, Export, 3);

/// Type 5 `DJDBKEY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub id: u32,
    /// Key name as stored in master.db (`djmdKey.ScaleName`), e.g. `Am`, `Gmin`, `9A`.
    pub name: String,
    /// `djmdKey.Seq`, stored in one byte.
    pub seq: u8,
}
impl Key {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            name: r.text("SCALENAME"),
            seq: u8_of(r, "SEQ"),
        }
    }
}
typed!(Key, Export, 5);

/// Type 6 `DJDBCOLOR`: the eight track colours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Color {
    pub id: u8,
    /// 0 for all eight rows in the fixture.
    pub color_code: u32,
    pub sort_key: u8,
    /// Colour name (`Pink`, `Red`, ...).
    pub name: String,
}
impl Color {
    fn read(r: &Row) -> Self {
        Self {
            id: u8_of(r, "ID"),
            color_code: r.uint("COLORCODE"),
            sort_key: u8_of(r, "SORTKEY"),
            name: r.text("COMMNT"),
        }
    }
}
typed!(Color, Export, 6);

/// Types 7 `DJDBPLAYLIST` and 9 `DJDBHOTCUEBANKLIST`: tree nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: u32,
    /// Position among siblings.
    pub seq: u32,
    pub name: String,
    pub artwork_id: u32,
    /// Node type: 0 = playlist, 1 = folder (hypothesis, from `djmdPlaylist.Attribute`).
    pub attribute: u8,
    /// 0 = top level.
    pub parent_id: u32,
}
impl TreeNode {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            seq: r.uint("SEQ"),
            name: r.text("NAME"),
            artwork_id: r.uint("IMAGEID"),
            attribute: u8_of(r, "ATTRIBUTE"),
            parent_id: r.uint("PARENTID"),
        }
    }
}

/// Type 7 `DJDBPLAYLIST`: the playlist tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistNode(pub TreeNode);
impl PlaylistNode {
    fn read(r: &Row) -> Self {
        Self(TreeNode::read(r))
    }
}
typed!(PlaylistNode, Export, 7);

/// Type 9 `DJDBHOTCUEBANKLIST`: hot cue bank list tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotCueBankList(pub TreeNode);
impl HotCueBankList {
    fn read(r: &Row) -> Self {
        Self(TreeNode::read(r))
    }
}
typed!(HotCueBankList, Export, 9);

/// Type 8 `DJDBSONGPLAYLIST`: playlist membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaylistEntry {
    pub playlist_id: u32,
    pub track_id: u32,
    /// 1-based position in the playlist.
    pub entry_index: u32,
}
impl PlaylistEntry {
    fn read(r: &Row) -> Self {
        Self {
            playlist_id: r.uint("PLAYLISTID"),
            track_id: r.uint("CONTENTID"),
            entry_index: r.uint("TRACKNO"),
        }
    }
}
typed!(PlaylistEntry, Export, 8);

/// Type 10 `DJDBSONGHOTCUEBANKLIST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotCueBankListEntry {
    pub bank_list_id: u32,
    pub track_id: u32,
    pub entry_index: u32,
    pub cue_id: u32,
}
impl HotCueBankListEntry {
    fn read(r: &Row) -> Self {
        Self {
            bank_list_id: r.uint("HOTCUEBANKLISTID"),
            track_id: r.uint("CONTENTID"),
            entry_index: r.uint("TRACKNO"),
            cue_id: r.uint("CUEID"),
        }
    }
}
typed!(HotCueBankListEntry, Export, 10);

/// Type 12 `DJDBSONGHISTORY`: history playlist membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    pub history_id: u32,
    /// 1-based position in the history playlist.
    pub seq: u16,
    pub track_id: u32,
    pub on_air: u8,
}
impl HistoryEntry {
    fn read(r: &Row) -> Self {
        Self {
            history_id: r.uint("HISTORYID"),
            seq: u16_of(r, "SEQ"),
            track_id: r.uint("CONTENTID"),
            on_air: u8_of(r, "ONAIR"),
        }
    }
}
typed!(HistoryEntry, Export, 12);

/// Type 13 `DJDBIMAGE`: artwork paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub id: u32,
    /// Device path, e.g. `/PIONEER/Artwork/00001/a1.jpg`.
    pub path: String,
}
impl Artwork {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            path: r.text("PATH"),
        }
    }
}
typed!(Artwork, Export, 13);

/// Type 14 `DJDBMIXERPARAM` (empty in the fixture).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixerParam {
    pub track_id: u32,
    pub gain: u16,
    pub high: u16,
    pub mid: u16,
    pub low: u16,
}
impl MixerParam {
    fn read(r: &Row) -> Self {
        Self {
            track_id: r.uint("CONTENTID"),
            gain: u16_of(r, "GAIN"),
            high: u16_of(r, "HIGH"),
            mid: u16_of(r, "MID"),
            low: u16_of(r, "LOW"),
        }
    }
}
typed!(MixerParam, Export, 14);

/// Type 15 `DJDBHOTCUEBANKPOINT` (empty in the fixture).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotCueBankPoint {
    pub id: u32,
    pub track_id: u32,
    pub in_msec: u32,
    pub in_frame: u32,
    pub in_mpeg_frame: u32,
    pub in_mpeg_abs: u32,
    pub out_msec: u32,
    pub out_frame: u32,
    pub out_mpeg_frame: u32,
    pub out_mpeg_abs: u32,
    pub kind: u8,
}
impl HotCueBankPoint {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            track_id: r.uint("CONTENTID"),
            in_msec: r.uint("INMSEC"),
            in_frame: r.uint("INFRAME"),
            in_mpeg_frame: r.uint("INMPEGFRAME"),
            in_mpeg_abs: r.uint("INMPEGABS"),
            out_msec: r.uint("OUTMSEC"),
            out_frame: r.uint("OUTFRAME"),
            out_mpeg_frame: r.uint("OUTMPEGFRAME"),
            out_mpeg_abs: r.uint("OUTMPEGABS"),
            kind: u8_of(r, "KIND"),
        }
    }
}
typed!(HotCueBankPoint, Export, 15);

/// Type 16 `DJDBMENUITEMS`: browse menu entries ("columns").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: u16,
    /// 0x80.. code of the item (meaning per value open).
    pub class: u8,
    /// UTF-16 name wrapped in U+FFFA … U+FFFB, e.g. `\u{fffa}GENRE\u{fffb}`.
    pub name: String,
}
impl MenuItem {
    fn read(r: &Row) -> Self {
        Self {
            id: u16_of(r, "ID"),
            class: u8_of(r, "CLASS"),
            name: r.text("NAME"),
        }
    }

    /// Name without the U+FFFA/U+FFFB markers.
    pub fn plain_name(&self) -> &str {
        self.name
            .trim_start_matches('\u{fffa}')
            .trim_end_matches('\u{fffb}')
    }
}
typed!(MenuItem, Export, 16);

/// Type 17 `DJDBCATEGORY`: top-level browse categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Category {
    pub id: u16,
    pub menu_item_id: u16,
    /// Display position (1-based) of enabled categories, 0 when disabled.
    pub seq: u8,
    /// Bit 0 = hidden; other bits seen (3, 5) are open.
    pub disable: u8,
    /// 99 for most rows; small numbers for some (meaning open).
    pub info_order: u8,
}
impl Category {
    fn read(r: &Row) -> Self {
        Self {
            id: u16_of(r, "ID"),
            menu_item_id: u16_of(r, "MENUITEMID"),
            seq: u8_of(r, "SEQ"),
            disable: u8_of(r, "DISABLE"),
            info_order: u8_of(r, "INFOORDER"),
        }
    }
}
typed!(Category, Export, 17);

/// Type 18 `DJDBSORT`: sort orders offered by the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub id: u16,
    pub menu_item_id: u16,
    pub seq: u8,
    pub disable: u8,
}
impl Sort {
    fn read(r: &Row) -> Self {
        Self {
            id: u16_of(r, "ID"),
            menu_item_id: u16_of(r, "MENUITEMID"),
            seq: u8_of(r, "SEQ"),
            disable: u8_of(r, "DISABLE"),
        }
    }
}
typed!(Sort, Export, 18);

/// Type 19 `DJDBPROPERTY`: one row describing the device library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// "REKORDBOX".
    pub device_name: String,
    /// "1000".
    pub db_version: String,
    /// Number of tracks (704 in the fixture).
    pub track_count: u32,
    /// YYYY-MM-DD.
    pub date_created: String,
    pub color_id: u8,
    pub protected: u8,
}
impl Property {
    fn read(r: &Row) -> Self {
        Self {
            device_name: r.text("DEVNAME"),
            db_version: r.text("DBVERSION"),
            track_count: r.uint("SONGNUMBER"),
            date_created: r.text("DATECREATED"),
            color_id: u8_of(r, "COLORID"),
            protected: u8_of(r, "PROTECTED"),
        }
    }
}
typed!(Property, Export, 19);

// ---- exportExt.pdb ----

/// Ext type 0 `DJDBEXRECOMMENDLIKE` (empty in the fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendLike {
    pub track_id1: u32,
    pub track_id2: u32,
    pub like_rate: u8,
    pub reserved1: u8,
    pub reserved2: u16,
    pub date_created_hi: u32,
    pub date_created_lo: u32,
    pub reserved3: String,
}
impl RecommendLike {
    fn read(r: &Row) -> Self {
        Self {
            track_id1: r.uint("CONTENTID1"),
            track_id2: r.uint("CONTENTID2"),
            like_rate: u8_of(r, "LIKERATE"),
            reserved1: u8_of(r, "RESERVED1"),
            reserved2: u16_of(r, "RESERVED2"),
            date_created_hi: r.uint("DATACREATEDH"),
            date_created_lo: r.uint("DATACREATEDL"),
            reserved3: r.text("RESERVED3"),
        }
    }
}
typed!(RecommendLike, ExportExt, 0);

/// Ext type 1 `DJDBEXHISTORY` (empty in the fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtHistory {
    pub id: u32,
    pub seq: u32,
    pub name: String,
    pub attribute: u8,
    pub reserved1: u8,
    pub reserved2: u16,
    pub parent_id: u32,
    pub date_created: String,
    pub reserved3: u32,
    pub reserved4: String,
}
impl ExtHistory {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            seq: r.uint("SEQ"),
            name: r.text("NAME"),
            attribute: u8_of(r, "ATTRIBUTE"),
            reserved1: u8_of(r, "RESERVED1"),
            reserved2: u16_of(r, "RESERVED2"),
            parent_id: r.uint("PARENTID"),
            date_created: r.text("DATECREATED"),
            reserved3: r.uint("RESERVED3"),
            reserved4: r.text("RESERVED4"),
        }
    }
}
typed!(ExtHistory, ExportExt, 1);

/// Ext type 2 `DJDBEXSONGHISTORY` (empty in the fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtHistoryEntry {
    pub history_id: u32,
    pub track_id: u32,
    pub entry_index: u32,
    pub reserved1: String,
}
impl ExtHistoryEntry {
    fn read(r: &Row) -> Self {
        Self {
            history_id: r.uint("HISTORYID"),
            track_id: r.uint("CONTENTID"),
            entry_index: r.uint("TRACKNO"),
            reserved1: r.text("RESERVED1"),
        }
    }
}
typed!(ExtHistoryEntry, ExportExt, 2);

/// Ext type 3 `DJDBEXMYTAG`: the My Tag tree (categories with `attribute` 1, tags below them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyTag {
    /// Small numbers (1..4) for categories, master.db random IDs for tags.
    pub id: u32,
    /// Position among siblings (0-based).
    pub seq: u32,
    /// 1 = category (column), 0 = tag.
    pub attribute: u8,
    pub reserved1: u8,
    pub reserved2: u16,
    /// Category ID for tags, 0 for categories.
    pub parent_id: u32,
    pub reserved3: u32,
    pub reserved4: u32,
    pub reserved5: String,
    pub name: String,
    pub reserved6: String,
}
impl MyTag {
    fn read(r: &Row) -> Self {
        Self {
            id: r.uint("ID"),
            seq: r.uint("SEQ"),
            attribute: u8_of(r, "ATTRIBUTE"),
            reserved1: u8_of(r, "RESERVED1"),
            reserved2: u16_of(r, "RESERVED2"),
            parent_id: r.uint("PARENTID"),
            reserved3: r.uint("RESERVED3"),
            reserved4: r.uint("RESERVED4"),
            reserved5: r.text("RESERVED5"),
            name: r.text("NAME"),
            reserved6: r.text("RESERVED6"),
        }
    }
}
typed!(MyTag, ExportExt, 3);

/// Ext type 4 `DJDBEXSONGMYTAG`: tag membership (empty in the fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyTagTrack {
    pub tag_id: u32,
    pub track_id: u32,
    pub reserved1: u32,
    pub reserved2: String,
}
impl MyTagTrack {
    fn read(r: &Row) -> Self {
        Self {
            tag_id: r.uint("MYTAGID"),
            track_id: r.uint("CONTENTID"),
            reserved1: r.uint("RESERVED1"),
            reserved2: r.text("RESERVED2"),
        }
    }
}
typed!(MyTagTrack, ExportExt, 4);

/// Ext type 5 `DJDBEXCUEOPTION` (empty in the fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueOption {
    pub cue_id: u32,
    pub color: u8,
    pub reserved1: u8,
    pub reserved2: u16,
    pub comment: String,
    pub beat_size: u32,
    pub cue_micro: u32,
    pub reserved5: u32,
    pub reserved6: u32,
    pub reserved7: u32,
    pub reserved8: u32,
    pub in_seek_info: String,
    pub out_seek_info: String,
}
impl CueOption {
    fn read(r: &Row) -> Self {
        Self {
            cue_id: r.uint("CUEID"),
            color: u8_of(r, "COLOR"),
            reserved1: u8_of(r, "RESERVED1"),
            reserved2: u16_of(r, "RESERVED2"),
            comment: r.text("COMMNT"),
            beat_size: r.uint("BEATSIZE"),
            cue_micro: r.uint("CUEMICRO"),
            reserved5: r.uint("RESERVED5"),
            reserved6: r.uint("RESERVED6"),
            reserved7: r.uint("RESERVED7"),
            reserved8: r.uint("RESERVED8"),
            in_seek_info: r.text("INSEEKINFO"),
            out_seek_info: r.text("OUTSEEKINFO"),
        }
    }
}
typed!(CueOption, ExportExt, 5);

/// Ext type 6 `DJDBEXCONTENTOPTION` (empty in the fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentOption {
    pub track_id: u32,
    pub reserved: [u32; 5],
    pub reserved_strings: [String; 5],
}
impl ContentOption {
    fn read(r: &Row) -> Self {
        Self {
            track_id: r.uint("CONTENTID"),
            reserved: std::array::from_fn(|i| r.uint(&format!("RESERVED{}", i + 1))),
            reserved_strings: std::array::from_fn(|i| r.text(&format!("RESERVEDSTRING{}", i + 1))),
        }
    }
}
typed!(ContentOption, ExportExt, 6);

/// Ext type 7 `DJDBEXDBPROPERTYOPTION`: one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbPropertyOption {
    /// 2328226058 in the fixture: the tracks' `MASTERDBID` + 1 (hypothesis: My Tag master DB ID).
    pub my_tag_master_db_id: u32,
    pub reserved: [u32; 5],
    pub db_version: String,
    pub reserved_strings: [String; 5],
}
impl DbPropertyOption {
    fn read(r: &Row) -> Self {
        Self {
            my_tag_master_db_id: r.uint("MYTAGMASTERDBID"),
            reserved: std::array::from_fn(|i| r.uint(&format!("RESERVED{}", i + 1))),
            db_version: r.text("DBVERSION"),
            reserved_strings: std::array::from_fn(|i| r.text(&format!("RESERVEDSTRING{}", i + 1))),
        }
    }
}
typed!(DbPropertyOption, ExportExt, 7);

/// Ext type 8 `DJDBEXGENERALOPTION` (empty in the fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneralOption {
    pub table_kind: u32,
    pub table_id: u32,
    pub reserved: [u32; 4],
    pub reserved_strings: [String; 3],
}
impl GeneralOption {
    fn read(r: &Row) -> Self {
        Self {
            table_kind: r.uint("TABLEKIND"),
            table_id: r.uint("TABLEID"),
            reserved: std::array::from_fn(|i| r.uint(&format!("RESERVED{}", i + 1))),
            reserved_strings: std::array::from_fn(|i| r.text(&format!("RESERVEDSTRING{}", i + 1))),
        }
    }
}
typed!(GeneralOption, ExportExt, 8);

// ---- writers: the inverse of the `read` functions above ----

impl Track {
    /// RESERVED9 as rekordbox stores it: `isrc_raw` when it still matches `isrc`, otherwise the
    /// `db::dsqlAddTrack` rule (exactly 12 bytes → the special 0x90 form, anything else → "").
    pub fn isrc_string(&self) -> DeviceString {
        let raw_matches = self.isrc_raw.text() == self.isrc;
        if raw_matches && (self.isrc.is_empty() || !self.isrc_raw.is_empty()) {
            return self.isrc_raw.clone();
        }
        if self.isrc.len() == 12 && self.isrc.is_ascii() {
            DeviceString::from_isrc(&self.isrc)
        } else {
            DeviceString::from_text("")
        }
    }

    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .text("FOLDERPATH", &self.folder_path)
            .text("FILENAMEL", &self.file_name)
            .text("FILENAMES", &self.file_name_short)
            .text("TITLE", &self.title)
            .int("ARTISTID", self.artist_id)
            .int("ALBUMID", self.album_id)
            .int("GENREID", self.genre_id)
            .int("BPM", self.bpm_x100)
            .int("LENGTH", self.length_s.into())
            .int("TRACKNO", self.track_no)
            .int("BITRATE", self.bitrate)
            .int("BITDEPTH", self.bit_depth.into())
            .text("COMMNT", &self.comment)
            .int("FILETYPE", self.file_type.into())
            .int("RATING", self.rating.into())
            .int("RELEASEYEAR", self.release_year.into())
            .int("REMIXERID", self.remixer_id)
            .int("LABELID", self.label_id)
            .int("ORGARTISTID", self.original_artist_id)
            .int("KEYID", self.key_id)
            .text("STOCKDATE", &self.stock_date)
            .int("COLORID", self.color_id.into())
            .int("DJPLAYCOUNT", self.play_count.into())
            .int("IMAGEID", self.artwork_id)
            .int("MASTERDBID", self.master_db_id)
            .int("MASTERCONTENTID", self.master_content_id)
            .text("ANALYSISDATAPATH", &self.analysis_path)
            .text("SEARCHSTR", &self.search_str)
            .int("FILESIZE", self.file_size)
            .int("DISCNO", self.disc_no.into())
            .int("COMPOSERID", self.composer_id)
            .text("SUBTITLE", &self.subtitle)
            .int("SAMPLERATE", self.sample_rate)
            .int("DISABLEQUANTIZE", self.disable_quantize.into())
            .int("ANALYSED", self.analysed.into())
            .text("RELEASEDATE", &self.release_date)
            .text("DATECREATED", &self.date_created)
            .int("CONTENTLINK", self.content_link)
            .text("TAG", &self.tag)
            .text("RESERVED1", &self.modified_by_rbm)
            .text("RESERVED2", &self.hot_cue_auto_load)
            .text("RESERVED3", &self.delivery_control)
            .text("RESERVED4", &self.delivery_comment)
            .text("RESERVED5", &self.cue_updated)
            .text("RESERVED6", &self.analysis_updated)
            .text("RESERVED7", &self.track_info_updated)
            .text("RESERVED8", &self.lyricist)
            .string("RESERVED9", self.isrc_string())
            .text("RESERVED10", &self.reserved10)
    }
}

impl Named {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id).text("NAME", &self.name)
    }
}

impl Genre {
    fn write(&self, v: Values) -> Values {
        self.0.write(v)
    }
}

impl Label {
    fn write(&self, v: Values) -> Values {
        self.0.write(v)
    }
}

impl HistoryPlaylist {
    fn write(&self, v: Values) -> Values {
        self.0.write(v)
    }
}

impl Artist {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .text("NAME", &self.name)
            .text("SEARCHSTR", &self.search_str)
    }
}

impl Album {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .text("NAME", &self.name)
            .int("ALBUMARTISTID", self.album_artist_id)
            .int("IMAGEID", self.artwork_id)
            .int("COMPILATION", self.compilation.into())
            .text("SEARCHSTR", &self.search_str)
    }
}

impl Key {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .text("SCALENAME", &self.name)
            .int("SEQ", self.seq.into())
    }
}

impl Color {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id.into())
            .int("COLORCODE", self.color_code)
            .int("SORTKEY", self.sort_key.into())
            .text("COMMNT", &self.name)
    }
}

impl TreeNode {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .int("SEQ", self.seq)
            .text("NAME", &self.name)
            .int("IMAGEID", self.artwork_id)
            .int("ATTRIBUTE", self.attribute.into())
            .int("PARENTID", self.parent_id)
    }
}

impl PlaylistNode {
    fn write(&self, v: Values) -> Values {
        self.0.write(v)
    }
}

impl HotCueBankList {
    fn write(&self, v: Values) -> Values {
        self.0.write(v)
    }
}

impl PlaylistEntry {
    fn write(&self, v: Values) -> Values {
        v.int("PLAYLISTID", self.playlist_id)
            .int("CONTENTID", self.track_id)
            .int("TRACKNO", self.entry_index)
    }
}

impl HotCueBankListEntry {
    fn write(&self, v: Values) -> Values {
        v.int("HOTCUEBANKLISTID", self.bank_list_id)
            .int("CONTENTID", self.track_id)
            .int("TRACKNO", self.entry_index)
            .int("CUEID", self.cue_id)
    }
}

impl HistoryEntry {
    fn write(&self, v: Values) -> Values {
        v.int("HISTORYID", self.history_id)
            .int("SEQ", self.seq.into())
            .int("CONTENTID", self.track_id)
            .int("ONAIR", self.on_air.into())
    }
}

impl Artwork {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id).text("PATH", &self.path)
    }
}

impl MixerParam {
    fn write(&self, v: Values) -> Values {
        v.int("CONTENTID", self.track_id)
            .int("GAIN", self.gain.into())
            .int("HIGH", self.high.into())
            .int("MID", self.mid.into())
            .int("LOW", self.low.into())
    }
}

impl HotCueBankPoint {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .int("CONTENTID", self.track_id)
            .int("INMSEC", self.in_msec)
            .int("INFRAME", self.in_frame)
            .int("INMPEGFRAME", self.in_mpeg_frame)
            .int("INMPEGABS", self.in_mpeg_abs)
            .int("OUTMSEC", self.out_msec)
            .int("OUTFRAME", self.out_frame)
            .int("OUTMPEGFRAME", self.out_mpeg_frame)
            .int("OUTMPEGABS", self.out_mpeg_abs)
            .int("KIND", self.kind.into())
    }
}

impl MenuItem {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id.into())
            .int("CLASS", self.class.into())
            .text("NAME", &self.name)
    }
}

impl Category {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id.into())
            .int("MENUITEMID", self.menu_item_id.into())
            .int("SEQ", self.seq.into())
            .int("DISABLE", self.disable.into())
            .int("INFOORDER", self.info_order.into())
    }
}

impl Sort {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id.into())
            .int("MENUITEMID", self.menu_item_id.into())
            .int("SEQ", self.seq.into())
            .int("DISABLE", self.disable.into())
    }
}

impl Property {
    fn write(&self, v: Values) -> Values {
        v.text("DEVNAME", &self.device_name)
            .text("DBVERSION", &self.db_version)
            .int("SONGNUMBER", self.track_count)
            .text("DATECREATED", &self.date_created)
            .int("COLORID", self.color_id.into())
            .int("PROTECTED", self.protected.into())
    }
}

impl RecommendLike {
    fn write(&self, v: Values) -> Values {
        v.int("CONTENTID1", self.track_id1)
            .int("CONTENTID2", self.track_id2)
            .int("LIKERATE", self.like_rate.into())
            .int("RESERVED1", self.reserved1.into())
            .int("RESERVED2", self.reserved2.into())
            .int("DATACREATEDH", self.date_created_hi)
            .int("DATACREATEDL", self.date_created_lo)
            .text("RESERVED3", &self.reserved3)
    }
}

impl ExtHistory {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .int("SEQ", self.seq)
            .text("NAME", &self.name)
            .int("ATTRIBUTE", self.attribute.into())
            .int("RESERVED1", self.reserved1.into())
            .int("RESERVED2", self.reserved2.into())
            .int("PARENTID", self.parent_id)
            .text("DATECREATED", &self.date_created)
            .int("RESERVED3", self.reserved3)
            .text("RESERVED4", &self.reserved4)
    }
}

impl ExtHistoryEntry {
    fn write(&self, v: Values) -> Values {
        v.int("HISTORYID", self.history_id)
            .int("CONTENTID", self.track_id)
            .int("TRACKNO", self.entry_index)
            .text("RESERVED1", &self.reserved1)
    }
}

impl MyTag {
    fn write(&self, v: Values) -> Values {
        v.int("ID", self.id)
            .int("SEQ", self.seq)
            .int("ATTRIBUTE", self.attribute.into())
            .int("RESERVED1", self.reserved1.into())
            .int("RESERVED2", self.reserved2.into())
            .int("PARENTID", self.parent_id)
            .int("RESERVED3", self.reserved3)
            .int("RESERVED4", self.reserved4)
            .text("RESERVED5", &self.reserved5)
            .text("NAME", &self.name)
            .text("RESERVED6", &self.reserved6)
    }
}

impl MyTagTrack {
    fn write(&self, v: Values) -> Values {
        v.int("MYTAGID", self.tag_id)
            .int("CONTENTID", self.track_id)
            .int("RESERVED1", self.reserved1)
            .text("RESERVED2", &self.reserved2)
    }
}

impl CueOption {
    fn write(&self, v: Values) -> Values {
        v.int("CUEID", self.cue_id)
            .int("COLOR", self.color.into())
            .int("RESERVED1", self.reserved1.into())
            .int("RESERVED2", self.reserved2.into())
            .text("COMMNT", &self.comment)
            .int("BEATSIZE", self.beat_size)
            .int("CUEMICRO", self.cue_micro)
            .int("RESERVED5", self.reserved5)
            .int("RESERVED6", self.reserved6)
            .int("RESERVED7", self.reserved7)
            .int("RESERVED8", self.reserved8)
            .text("INSEEKINFO", &self.in_seek_info)
            .text("OUTSEEKINFO", &self.out_seek_info)
    }
}

fn reserved_ints(mut v: Values, r: &[u32]) -> Values {
    for (i, x) in r.iter().enumerate() {
        v = v.int(&format!("RESERVED{}", i + 1), *x);
    }
    v
}

fn reserved_strings(mut v: Values, r: &[String]) -> Values {
    for (i, x) in r.iter().enumerate() {
        v = v.text(&format!("RESERVEDSTRING{}", i + 1), x);
    }
    v
}

impl ContentOption {
    fn write(&self, v: Values) -> Values {
        let v = v.int("CONTENTID", self.track_id);
        reserved_strings(reserved_ints(v, &self.reserved), &self.reserved_strings)
    }
}

impl DbPropertyOption {
    fn write(&self, v: Values) -> Values {
        let v = v
            .int("MYTAGMASTERDBID", self.my_tag_master_db_id)
            .text("DBVERSION", &self.db_version);
        reserved_strings(reserved_ints(v, &self.reserved), &self.reserved_strings)
    }
}

impl GeneralOption {
    fn write(&self, v: Values) -> Values {
        let v = v
            .int("TABLEKIND", self.table_kind)
            .int("TABLEID", self.table_id);
        reserved_strings(reserved_ints(v, &self.reserved), &self.reserved_strings)
    }
}
