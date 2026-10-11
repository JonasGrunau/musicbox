//! Rows rekordbox writes into a new device database before any track is exported.
//! Spec: `docs/re/pdb.md` § Writer, § Default rows.
//!
//! rekordbox copies colours, menu items, categories and sorts from the source library
//! (`djmdColor`, `djmdMenuItems`, `djmdCategory … order by Seq`, `djmdSort … order by Seq`;
//! `docs/re/ops/export-usb.md`) when it creates the stick's database. The values below are those
//! of a fresh rekordbox 7.2.19 library, in the order rekordbox inserted them on the
//! `fixtures/oracle-a/rbtest` stick (verified there: same rows, same slot order). A library whose
//! user changed the browse menus has other category/sort rows; callers with access to master.db
//! should pass those instead.

use crate::library::{ExportDb, ExtDb};
use crate::tables::{Category, Color, DbPropertyOption, MenuItem, Property, Sort};

/// `djmdColor`: the eight track colours (ID, name); SORTKEY = ID, COLORCODE = 0.
pub fn colors() -> Vec<Color> {
    [
        "Pink", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple",
    ]
    .iter()
    .enumerate()
    .map(|(i, name)| Color {
        id: i as u8 + 1,
        color_code: 0,
        sort_key: i as u8 + 1,
        name: (*name).to_owned(),
    })
    .collect()
}

/// `djmdMenuItems`: browse menu items (ID, CLASS, name), names wrapped in U+FFFA/U+FFFB.
pub fn menu_items() -> Vec<MenuItem> {
    const ITEMS: [(u16, u8, &str); 27] = [
        (1, 128, "GENRE"),
        (2, 129, "ARTIST"),
        (3, 130, "ALBUM"),
        (4, 131, "TRACK"),
        (5, 133, "BPM"),
        (6, 134, "RATING"),
        (7, 135, "YEAR"),
        (8, 136, "REMIXER"),
        (9, 137, "LABEL"),
        (10, 138, "ORIGINAL ARTIST"),
        (11, 139, "KEY"),
        (12, 141, "CUE"),
        (13, 142, "COLOR"),
        (14, 146, "TIME"),
        (15, 147, "BITRATE"),
        (16, 148, "FILE NAME"),
        (17, 132, "PLAYLIST"),
        (18, 152, "HOT CUE BANK"),
        (19, 149, "HISTORY"),
        (20, 145, "SEARCH"),
        (21, 150, "COMMENTS"),
        (22, 140, "DATE ADDED"),
        (23, 151, "DJ PLAY COUNT"),
        (24, 144, "FOLDER"),
        (25, 161, "DEFAULT"),
        (26, 162, "ALPHABET"),
        (27, 170, "MATCHING"),
    ];
    ITEMS
        .iter()
        .map(|&(id, class, name)| MenuItem {
            id,
            class,
            name: format!("\u{fffa}{name}\u{fffb}"),
        })
        .collect()
}

/// `djmdCategory` in `Seq` order: (ID, MENUITEMID, SEQ, DISABLE, INFOORDER).
pub fn categories() -> Vec<Category> {
    const ROWS: [(u16, u16, u8, u8, u8); 22] = [
        (1, 1, 0, 1, 99),
        (6, 5, 0, 1, 5),
        (7, 6, 0, 1, 99),
        (8, 7, 0, 1, 99),
        (9, 8, 0, 1, 99),
        (10, 9, 0, 1, 99),
        (11, 10, 0, 1, 99),
        (15, 13, 0, 1, 99),
        (19, 14, 0, 1, 4),
        (20, 15, 0, 1, 6),
        (21, 16, 0, 1, 99),
        (23, 18, 0, 1, 99),
        (2, 2, 1, 0, 2),
        (3, 3, 2, 0, 3),
        (4, 4, 3, 0, 1),
        (12, 11, 4, 0, 99),
        (5, 17, 5, 0, 99),
        (22, 19, 6, 0, 99),
        (18, 20, 7, 0, 99),
        (26, 27, 8, 2, 99),
        (17, 24, 9, 0, 99),
        (27, 22, 10, 0, 99),
    ];
    ROWS.iter()
        .map(|&(id, menu_item_id, seq, disable, info_order)| Category {
            id,
            menu_item_id,
            seq,
            disable,
            info_order,
        })
        .collect()
}

/// `djmdSort` in `Seq` order: (ID, MENUITEMID, SEQ, DISABLE).
pub fn sorts() -> Vec<Sort> {
    const ROWS: [(u16, u16, u8, u8); 17] = [
        (6, 1, 0, 1),
        (7, 21, 0, 1),
        (8, 14, 0, 1),
        (9, 8, 0, 1),
        (10, 9, 0, 1),
        (11, 10, 0, 1),
        (13, 15, 0, 1),
        (15, 13, 0, 1),
        (16, 23, 0, 1),
        (17, 22, 0, 1),
        (0, 25, 1, 0),
        (1, 26, 2, 0),
        (2, 2, 3, 0),
        (3, 3, 4, 0),
        (4, 5, 5, 0),
        (5, 6, 6, 0),
        (12, 11, 7, 0),
    ];
    ROWS.iter()
        .map(|&(id, menu_item_id, seq, disable)| Sort {
            id,
            menu_item_id,
            seq,
            disable,
        })
        .collect()
}

/// The property row of a new stick (`DevInitPropertyTable_`): no device name, DBVERSION
/// "1000", the creation date, colour 0, not protected. SONGNUMBER = number of tracks.
pub fn property(date_created: &str, track_count: u32) -> Property {
    Property {
        device_name: String::new(),
        db_version: "1000".into(),
        track_count,
        date_created: date_created.into(),
        color_id: 0,
        protected: 0,
    }
}

/// The DBPROPERTYOPTION row of exportExt.pdb. `my_tag_master_db_id` is the value rekordbox
/// writes when it syncs My Tags (`docs/re/export.md` § 2.5; on the oracle stick the ID of the
/// `djmdCloudProperty` row `SyncService`).
pub fn db_property_option(my_tag_master_db_id: u32) -> DbPropertyOption {
    DbPropertyOption {
        my_tag_master_db_id,
        reserved: [0; 5],
        db_version: String::new(),
        reserved_strings: Default::default(),
    }
}

/// An export.pdb with only the rows of a new stick.
pub fn export_db(date_created: &str) -> ExportDb {
    ExportDb {
        colors: colors(),
        menu_items: menu_items(),
        categories: categories(),
        sorts: sorts(),
        properties: vec![property(date_created, 0)],
        ..Default::default()
    }
}

/// An exportExt.pdb with only the DBPROPERTYOPTION row.
pub fn ext_db(my_tag_master_db_id: u32) -> ExtDb {
    ExtDb {
        db_property_options: vec![db_property_option(my_tag_master_db_id)],
        ..Default::default()
    }
}
