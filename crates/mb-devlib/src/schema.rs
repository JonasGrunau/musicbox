//! exportLibrary.db DDL and seed rows exactly as rekordbox 7.2.19 writes them.
//! Spec: `docs/re/devlib.md#schema`, statement order: `docs/re/ops/export-devlib.md`.
//!
//! The statements are the text `DevSqliteDBController::createTables` (@0x1006F4DF4) and
//! `createIndexes` (@0x1006F6BA8) send (SQL trace, 2026-10-11). SQLite stores them in
//! `sqlite_master` with the leading keywords upper-cased (`CREATE TABLE content(…)`), which is
//! byte-identical to the fixture sticks.

use crate::model::{MenuItem, MenuSetting};

/// The 22 tables in creation order (= `sqlite_master` order). Names match `DevDBTables`.
pub const TABLES: [(&str, &str); 22] = [
    (
        "content",
        "create table content(content_id integer primary key, title varchar, titleForSearch varchar, subtitle varchar, bpmx100 integer, length integer, trackNo integer, discNo integer, artist_id_artist integer, artist_id_remixer integer, artist_id_originalArtist integer, artist_id_composer integer, artist_id_lyricist integer, album_id integer, genre_id integer, label_id integer, key_id integer, color_id integer, image_id integer, djComment varchar, rating integer, releaseYear integer, releaseDate varchar, dateCreated varchar, dateAdded varchar, path varchar, fileName varchar, fileSize integer, fileType integer, bitrate integer, bitDepth integer, samplingRate integer, isrc varchar, djPlayCount integer, isHotCueAutoLoadOn integer, isKuvoDeliverStatusOn integer, kuvoDeliveryComment varchar, masterDbId integer, masterContentId integer, analysisDataFilePath varchar, analysedBits integer, contentLink integer, hasModified integer, cueUpdateCount integer, analysisDataUpdateCount integer, informationUpdateCount integer)",
    ),
    (
        "genre",
        "create table genre(genre_id integer primary key, name varchar)",
    ),
    (
        "artist",
        "create table artist(artist_id integer primary key, name varchar, nameForSearch varchar)",
    ),
    (
        "album",
        "create table album(album_id integer primary key, name varchar, artist_id integer, image_id integer, isComplation integer, nameForSearch varchar)",
    ),
    (
        "label",
        "create table label(label_id integer primary key, name varchar)",
    ),
    (
        "key",
        "create table key(key_id integer primary key, name varchar)",
    ),
    (
        "color",
        "create table color(color_id integer primary key, name varchar)",
    ),
    (
        "playlist",
        "create table playlist(playlist_id integer primary key, sequenceNo integer, name varchar, image_id integer, attribute integer, playlist_id_parent integer)",
    ),
    (
        "playlist_content",
        "create table playlist_content(playlist_id integer, content_id integer, sequenceNo integer)",
    ),
    (
        "hotCueBankList",
        "create table hotCueBankList(hotCueBankList_id integer primary key, sequenceNo integer, name varchar, image_id integer, attribute integer, hotCueBankList_id_parent integer)",
    ),
    (
        "hotCueBankList_cue",
        "create table hotCueBankList_cue(hotCueBankList_id integer, cue_id integer, sequenceNo integer)",
    ),
    (
        "history",
        "create table history(history_id integer primary key, sequenceNo integer, name varchar, attribute integer, history_id_parent integer)",
    ),
    (
        "history_content",
        "create table history_content(history_id integer, content_id integer, sequenceNo integer)",
    ),
    (
        "image",
        "create table image(image_id integer primary key, path varchar)",
    ),
    (
        "cue",
        "create table cue(cue_id integer primary key, content_id integer, kind integer, colorTableIndex integer, cueComment varchar, isActiveLoop integer, beatLoopNumerator integer, beatLoopDenominator integer, inUsec integer, outUsec integer, in150FramePerSec integer, out150FramePerSec integer, inMpegFrameNumber integer, outMpegFrameNumber integer, inMpegAbs integer, outMpegAbs integer, inDecodingStartFramePosition integer, outDecodingStartFramePosition integer, inFileOffsetInBlock integer, OutFileOffsetInBlock integer, inNumberOfSampleInBlock integer, outNumberOfSampleInBlock integer)",
    ),
    (
        "menuItem",
        "create table menuItem(menuItem_id integer primary key, kind integer, name varchar)",
    ),
    (
        "category",
        "create table category(category_id integer primary key, menuItem_id integer, sequenceNo integer, isVisible integer)",
    ),
    (
        "sort",
        "create table sort(sort_id integer primary key, menuItem_id integer, sequenceNo integer, isVisible integer, isSelectedAsSubColumn integer)",
    ),
    (
        "property",
        "create table property(deviceName varchar, dbVersion varchar, numberOfContents integer, createdDate varchar, backGroundColorType integer, myTagMasterDBID integer)",
    ),
    (
        "recommendedLike",
        "create table recommendedLike(content_id_1 integer, content_id_2 integer, rating integer, createdDate integer)",
    ),
    (
        "myTag",
        "create table myTag(myTag_id integer primary key, sequenceNo integer, name varchar, attribute integer, myTag_id_parent integer)",
    ),
    (
        "myTag_content",
        "create table myTag_content(myTag_id integer, content_id integer)",
    ),
];

/// The 4 indexes in creation order.
pub const INDEXES: [(&str, &str); 4] = [
    (
        "index_playlist_content_playlist_id",
        "create index index_playlist_content_playlist_id on playlist_content(playlist_id)",
    ),
    (
        "index_myTag_content_myTag_id",
        "create index index_myTag_content_myTag_id on myTag_content(myTag_id)",
    ),
    (
        "index_myTag_content_content_id",
        "create index index_myTag_content_content_id on myTag_content(content_id)",
    ),
    (
        "index_hotCueBankList_cue_hotCueBankList_id",
        "create index index_hotCueBankList_cue_hotCueBankList_id on hotCueBankList_cue(hotCueBankList_id)",
    ),
];

/// `property.dbVersion` that `initPropertyTable` (@0x1006F26A8) and `writeMyTag` (@0x1007121E4)
/// write (cstring 0x104F8E832).
pub const DB_VERSION_INIT: &str = "10000";

/// `property.dbVersion` a stick freshly exported by 7.2.19 ends up with: the Device Library Plus
/// conversion (`insertAllDevProperty` @0x100737658) re-seeds the row from export.pdb's
/// `djdbProperty`, whose `DBVersion` is "1000" (SQL trace, `docs/re/ops/export-devlib.md`).
pub const DB_VERSION_FRESH_STICK: &str = "1000";

/// Wraps a menu key the way `initPropertyTable` / `initMenuItemsTable` do: U+FFFA KEY U+FFFB.
pub fn menu_item_name(key: &str) -> String {
    format!("\u{FFFA}{key}\u{FFFB}")
}

/// `menuItem` rows of `initMenuItemsTable` @0x10071B504: (id, kind, key).
const DEFAULT_MENU_ITEMS: [(u32, i64, &str); 27] = [
    (1, 0x80, "GENRE"),
    (2, 0x81, "ARTIST"),
    (3, 0x82, "ALBUM"),
    (4, 0x83, "TRACK"),
    (5, 0x85, "BPM"),
    (6, 0x86, "RATING"),
    (7, 0x87, "YEAR"),
    (8, 0x88, "REMIXER"),
    (9, 0x89, "LABEL"),
    (10, 0x8A, "ORIGINAL ARTIST"),
    (11, 0x8B, "KEY"),
    (12, 0x8D, "CUE"),
    (13, 0x8E, "COLOR"),
    (14, 0x92, "TIME"),
    (15, 0x93, "BITRATE"),
    (16, 0x94, "FILE NAME"),
    (17, 0x84, "PLAYLIST"),
    (18, 0x98, "HOT CUE BANK"),
    (19, 0x95, "HISTORY"),
    (20, 0x91, "SEARCH"),
    (21, 0x96, "COMMENTS"),
    (22, 0x8C, "DATE ADDED"),
    (23, 0x97, "DJ PLAY COUNT"),
    (24, 0x90, "FOLDER"),
    (25, 0xA1, "DEFAULT"),
    (26, 0xA2, "ALPHABET"),
    (27, 0xAA, "MATCHING"),
];

/// `category` rows of `initCategoryTable` @0x10071BDD0: (id, menuItem_id, sequenceNo, isVisible).
const DEFAULT_CATEGORIES: [(u32, u32, i64, bool); 22] = [
    (1, 1, 0, false),
    (2, 2, 1, true),
    (3, 3, 2, true),
    (4, 4, 3, true),
    (5, 17, 5, true),
    (6, 5, 0, false),
    (7, 6, 0, false),
    (8, 7, 0, false),
    (9, 8, 0, false),
    (10, 9, 0, false),
    (11, 10, 0, false),
    (12, 11, 4, true),
    (15, 13, 0, false),
    (17, 24, 9, true),
    (18, 20, 7, true),
    (19, 14, 0, false),
    (20, 15, 0, false),
    (21, 16, 0, false),
    (22, 19, 6, true),
    (23, 18, 0, false),
    (26, 27, 8, false),
    (27, 22, 10, true),
];

/// `sort` rows of `initSortTable` @0x10071D090: (id, menuItem_id, sequenceNo, isVisible);
/// `isSelectedAsSubColumn` is always 0 there.
const DEFAULT_SORTS: [(u32, u32, i64, bool); 17] = [
    (0, 25, 1, true),
    (1, 26, 2, true),
    (2, 2, 3, true),
    (3, 3, 4, true),
    (4, 5, 5, true),
    (5, 6, 6, true),
    (6, 1, 0, false),
    (7, 21, 0, false),
    (8, 14, 0, false),
    (9, 8, 0, false),
    (10, 9, 0, false),
    (11, 10, 0, false),
    (12, 11, 7, true),
    (13, 15, 0, false),
    (15, 13, 0, false),
    (16, 23, 0, false),
    (17, 22, 0, false),
];

/// master.db's stock `djmdColor` rows (ID, Commnt), which `initPropertyTable` copies. rekordbox
/// has no built-in fallback for colours; these are the rows every library ships with.
const DEFAULT_COLORS: [(u32, &str); 8] = [
    (1, "Pink"),
    (2, "Red"),
    (3, "Orange"),
    (4, "Yellow"),
    (5, "Green"),
    (6, "Aqua"),
    (7, "Blue"),
    (8, "Purple"),
];

/// The rows rekordbox seeds into `color`, `menuItem`, `category` and `sort` when it creates the
/// database. rekordbox copies them from master.db ([`MenuSeed::from_master`]); only when the
/// master tables are empty does it fall back to built-in defaults ([`MenuSeed::defaults`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSeed {
    /// (color_id, name).
    pub colors: Vec<(u32, String)>,
    pub menu_items: Vec<MenuItem>,
    pub categories: Vec<MenuSetting>,
    pub sorts: Vec<MenuSetting>,
}

/// One master.db `djmdMenuItems` row.
#[derive(Debug, Clone)]
pub struct MasterMenuItem {
    pub id: u32,
    /// `Class`, a signed byte in master.db (GENRE = -128).
    pub class: i64,
    /// `Name` without the U+FFFA/U+FFFB wrapper (`GENRE`).
    pub name: String,
}

/// One master.db `djmdCategory` / `djmdSort` row.
#[derive(Debug, Clone)]
pub struct MasterMenuSetting {
    pub id: u32,
    pub menu_item_id: u32,
    pub seq: i64,
    /// `Disable`: bit 0 = hidden, bit 1 = selected as sub column (sort only).
    pub disable: i64,
}

impl MenuSeed {
    /// The built-in fallback rows of `initMenuItemsTable` / `initCategoryTable` /
    /// `initSortTable`, plus master.db's stock colours.
    pub fn defaults() -> Self {
        Self {
            colors: DEFAULT_COLORS
                .iter()
                .map(|(id, name)| (*id, name.to_string()))
                .collect(),
            menu_items: DEFAULT_MENU_ITEMS
                .iter()
                .map(|(id, kind, key)| MenuItem {
                    id: *id,
                    kind: *kind,
                    name: menu_item_name(key),
                })
                .collect(),
            categories: DEFAULT_CATEGORIES
                .iter()
                .map(|(id, item, seq, visible)| MenuSetting {
                    id: *id,
                    menu_item_id: *item,
                    seq: *seq,
                    visible: *visible,
                    sub_column: false,
                })
                .collect(),
            sorts: DEFAULT_SORTS
                .iter()
                .map(|(id, item, seq, visible)| MenuSetting {
                    id: *id,
                    menu_item_id: *item,
                    seq: *seq,
                    visible: *visible,
                    sub_column: false,
                })
                .collect(),
        }
    }

    /// The conversion `initPropertyTable` (@0x1006F26A8) applies to master.db rows
    /// (`rb_local_deleted = 0` only):
    /// - `color` ← `djmdColor` (`ID`, `Commnt`);
    /// - `menuItem` ← `djmdMenuItems`: kind = `Class & 0xFF`, name = U+FFFA `Name` U+FFFB;
    /// - `category` ← `djmdCategory`: isVisible = !(Disable & 1);
    /// - `sort` ← `djmdSort`: isVisible = !(Disable & 1), isSelectedAsSubColumn = (Disable >> 1) & 1.
    ///
    /// Empty master tables fall back to [`MenuSeed::defaults`] per table, like rekordbox.
    pub fn from_master(
        colors: &[(u32, String)],
        menu_items: &[MasterMenuItem],
        categories: &[MasterMenuSetting],
        sorts: &[MasterMenuSetting],
    ) -> Self {
        let defaults = Self::defaults();
        let setting = |s: &MasterMenuSetting, with_sub: bool| MenuSetting {
            id: s.id,
            menu_item_id: s.menu_item_id,
            seq: s.seq,
            visible: s.disable & 1 == 0,
            sub_column: with_sub && (s.disable >> 1) & 1 != 0,
        };
        Self {
            colors: if colors.is_empty() {
                defaults.colors
            } else {
                colors.to_vec()
            },
            menu_items: if menu_items.is_empty() {
                defaults.menu_items
            } else {
                menu_items
                    .iter()
                    .map(|m| MenuItem {
                        id: m.id,
                        kind: m.class & 0xFF,
                        name: menu_item_name(&m.name),
                    })
                    .collect()
            },
            categories: if categories.is_empty() {
                defaults.categories
            } else {
                categories.iter().map(|s| setting(s, false)).collect()
            },
            sorts: if sorts.is_empty() {
                defaults.sorts
            } else {
                sorts.iter().map(|s| setting(s, true)).collect()
            },
        }
    }
}
