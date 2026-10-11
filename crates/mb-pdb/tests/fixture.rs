//! Checks against the real rekordbox 7.2.19 export in `fixtures/stick` (skipped when absent).

use std::path::{Path, PathBuf};

use mb_pdb::page::{HEADER_LIST_EMPTY, NONE_13, PageKind};
use mb_pdb::row::RowHeader;
use mb_pdb::schema::DbKind;
use mb_pdb::tables::read_rows;
use mb_pdb::{DeviceLibrary, Pdb};

fn dir() -> Option<PathBuf> {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stick/PIONEER/rekordbox");
    if d.join("export.pdb").exists() && d.join("exportExt.pdb").exists() {
        Some(d)
    } else {
        eprintln!("no fixtures, skipping");
        None
    }
}

fn open(name: &str) -> Option<Pdb> {
    dir().map(|d| Pdb::open(&d.join(name)).unwrap())
}

#[test]
fn row_counts_match_the_fixture() {
    let Some(export) = open("export.pdb") else {
        return;
    };
    let Some(ext) = open("exportExt.pdb") else {
        return;
    };
    let count = |pdb: &Pdb, db: DbKind, t: u32| read_rows(pdb, db, t).unwrap().len();
    let expected_export = [
        (0, 704),  // tracks
        (1, 14),   // genres
        (2, 572),  // artists
        (3, 605),  // albums
        (4, 209),  // labels
        (5, 58),   // keys
        (6, 8),    // colors
        (7, 30),   // playlist tree
        (8, 1355), // playlist entries (see legacy_counts_explain_848_and_159)
        (9, 0),
        (10, 0),
        (11, 21),  // history playlists
        (12, 160), // history entries
        (13, 702), // artwork
        (14, 0),
        (15, 0),
        (16, 27), // menu items ("columns")
        (17, 22), // categories
        (18, 17), // sorts
        (19, 1),  // property
    ];
    for (t, n) in expected_export {
        assert_eq!(count(&export, DbKind::Export, t), n, "export.pdb type {t}");
    }
    let expected_ext = [
        (0, 0),
        (1, 0),
        (2, 0),
        (3, 28),
        (4, 0),
        (5, 0),
        (6, 0),
        (7, 1),
        (8, 0),
    ];
    for (t, n) in expected_ext {
        assert_eq!(
            count(&ext, DbKind::ExportExt, t),
            n,
            "exportExt.pdb type {t}"
        );
    }
}

/// The task brief expected 848 playlist entries and 159 history entries. Those numbers come from
/// reading the slot count as the low byte of 0x18 and preferring the u16 at 0x22 when larger;
/// that u16 is really the first dirty slot. This test reproduces the old numbers to show where
/// they come from, so the 1355/160 above are the real counts.
#[test]
fn legacy_counts_explain_848_and_159() {
    let Some(export) = open("export.pdb") else {
        return;
    };
    let legacy = |t: u32| {
        let entry = export.header.table(t).unwrap();
        let mut n = 0;
        for page in export.data_pages(entry).unwrap() {
            let small = export.page(page.number).unwrap()[0x18] as u16;
            let large = page.header.first_dirty_slot;
            let rows = if large > small && large != NONE_13 {
                large
            } else {
                small
            } as usize;
            let slots = page.slots().unwrap();
            n += slots.iter().take(rows).filter(|s| s.present).count();
        }
        n
    };
    assert_eq!(legacy(8), 848);
    assert_eq!(legacy(12), 159);
}

#[test]
fn spot_check_known_tracks() {
    let Some(d) = dir() else { return };
    let lib = DeviceLibrary::open(&d).unwrap();
    let e = &lib.export;
    let track = |id: u32| e.tracks.iter().find(|t| t.id == id).unwrap();
    let artist = |id: u32| e.artists.iter().find(|a| a.id == id).unwrap().name.as_str();

    let t1 = track(1);
    assert_eq!(t1.title, "The Underdog (Original Mix)");
    assert_eq!(artist(t1.artist_id), "Eddie Bitar");
    assert_eq!(t1.bpm_x100, 13800);
    assert_eq!(t1.length_s, 396);
    assert_eq!(t1.file_type, 12);
    assert_eq!(
        t1.folder_path,
        "/Contents/Eddie Bitar/The Underdog/Eddie Bitar - The Underdog (Original Mix).aiff"
    );

    let t2 = track(2);
    assert_eq!(t2.title, "Digital God (Original)");
    assert_eq!(artist(t2.artist_id), "Spirit Architect, Djantrix");
    assert_eq!(t2.bpm_x100, 14600);

    let flac = track(682);
    assert_eq!(flac.title, "Give Me Ecstasy (Original Mix)");
    assert_eq!(flac.bpm_x100, 15500);
    assert_eq!(
        (
            flac.file_type,
            flac.bit_depth,
            flac.sample_rate,
            flac.bitrate
        ),
        (5, 24, 44100, 0)
    );
    assert_eq!(flac.file_name, "Lawbreaker - Give Me Ecstasy.flac");
    assert_eq!(
        flac.analysis_path,
        "/PIONEER/USBANLZ/P00D/00001CE1/ANLZ0000.DAT"
    );
    assert_eq!(
        (flac.stock_date.as_str(), flac.date_created.as_str()),
        ("2025-07-23", "2025-07-23")
    );
    assert_eq!(flac.release_year, 2025);
    assert_eq!(flac.comment, "8A");
    assert_eq!(flac.hot_cue_auto_load, "ON");

    let isrc = track(650);
    assert_eq!(isrc.title, "High Pressure");
    assert_eq!(isrc.isrc, "QZS662166224");
    assert_eq!(isrc.isrc_raw.kind, 0x90);

    let key = |id: u32| e.keys.iter().find(|k| k.id == id).unwrap().name.as_str();
    assert_eq!(key(t1.key_id), "Gmin");
    assert_eq!(
        e.genres
            .iter()
            .find(|g| g.0.id == t1.genre_id)
            .unwrap()
            .0
            .name,
        "Psy-Trance"
    );
    assert_eq!(
        e.colors.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        [
            "Pink", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple"
        ]
    );
    assert_eq!(e.menu_items[0].plain_name(), "GENRE");
    let p = &e.properties[0];
    assert_eq!(
        (p.device_name.as_str(), p.db_version.as_str(), p.track_count),
        ("REKORDBOX", "1000", 704)
    );

    let ext = lib.ext.as_ref().unwrap();
    let genre_col = ext.tags.iter().find(|t| t.id == 1).unwrap();
    assert_eq!((genre_col.name.as_str(), genre_col.attribute), ("Genre", 1));
    assert_eq!(ext.tags.iter().filter(|t| t.parent_id == 1).count(), 7);
}

#[test]
fn snapshot_resolves_names_and_orders_playlists() {
    let Some(d) = dir() else { return };
    let lib = DeviceLibrary::open(&d).unwrap();
    let snap = lib.snapshot();
    assert_eq!(snap.tracks.len(), 704);
    assert_eq!(snap.playlists.len(), 30);
    let t = snap.tracks.iter().find(|t| t.id.0 == "682").unwrap();
    assert_eq!(t.bpm(), Some(155.0));
    assert_eq!(t.file_type, mb_core::FileType::Flac);
    assert!(t.path.ends_with("Contents/Lawbreaker,Total Balance/Give Me Ecstasy (Original Mix)/Lawbreaker - Give Me Ecstasy.flac"));
    assert!(
        t.artwork_path
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .contains("PIONEER/Artwork/")
    );
    let total: usize = snap.playlist_entries.values().map(Vec::len).sum();
    assert_eq!(total, 1355);
    let prog = snap
        .playlists
        .iter()
        .find(|p| p.name == "PROG / PSY / FULLON")
        .unwrap();
    assert_eq!(snap.playlist_entries[&prog.id].len(), 504);
    let hist = lib.history();
    assert_eq!(hist.len(), 21);
    assert_eq!(hist.iter().map(|h| h.tracks.len()).sum::<usize>(), 160);
    assert_eq!(hist[0].name, "HISTORY 001");
    assert_eq!(hist[0].tracks[0].0, "704");
}

/// Header encoders reproduce the file bytes, and the structural invariants documented in
/// docs/re/pdb.md hold on every page.
#[test]
fn headers_round_trip_and_invariants_hold() {
    for (name, db, word_base) in [
        ("export.pdb", DbKind::Export, 1u16),
        ("exportExt.pdb", DbKind::ExportExt, 49),
    ] {
        let Some(pdb) = open(name) else { return };
        let raw = std::fs::read(dir().unwrap().join(name)).unwrap();
        assert_eq!(pdb.as_bytes(), raw.as_slice());
        let h = &pdb.header;
        assert_eq!(h.to_bytes(), pdb.page(0).unwrap(), "{name}: page 0");
        assert_eq!(
            (h.page_size, h.state, h.reserved0, h.xid_hi),
            (4096, 5, 0, 0)
        );
        assert_eq!(
            h.tables.iter().map(|t| t.table_type).collect::<Vec<_>>(),
            (0..h.table_count).collect::<Vec<_>>()
        );

        for entry in &h.tables {
            let chain = pdb.chain(entry).unwrap();
            assert!(entry.empty_candidate < h.next_unused_page);
            assert!(!chain.contains(&entry.empty_candidate));
            let last = pdb.page_header(entry.last_page).unwrap();
            assert_eq!(
                last.next_page, entry.empty_candidate,
                "{name} type {}: last page links to the candidate",
                entry.table_type
            );
            if entry.empty_candidate < pdb.page_count() {
                assert!(
                    pdb.page(entry.empty_candidate)
                        .unwrap()
                        .iter()
                        .all(|&b| b == 0)
                );
            }

            let hp = pdb.header_page(entry).unwrap();
            assert_eq!(hp.header.kind(), PageKind::TableHeader);
            assert_eq!(hp.header.flags, 0x64);
            assert_eq!((hp.self_page, hp.header.aux0), (entry.first_page, 1004));
            let reuse = hp.reuse_pages();
            assert_eq!(hp.reuse_count as usize, reuse.len());
            assert_eq!(
                (hp.free_list_head, hp.free_list_tail),
                (mb_pdb::page::NO_PAGE, 0)
            );
            // Header-page dirty counters track list entries with flag bit 0 set.
            let dirty_entries: Vec<u16> = hp
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| **e != HEADER_LIST_EMPTY && *e & 1 == 1)
                .map(|(i, _)| i as u16)
                .collect();
            assert_eq!(
                hp.header.dirty_count,
                if dirty_entries.is_empty() {
                    NONE_13
                } else {
                    dirty_entries.len() as u16
                }
            );
            assert_eq!(
                hp.header.first_dirty_slot,
                dirty_entries.first().copied().unwrap_or(NONE_13)
            );
            assert!(
                hp.entries[hp.header.aux1 as usize..]
                    .iter()
                    .all(|&e| e == HEADER_LIST_EMPTY)
            );

            let data = pdb.data_pages(entry).unwrap();
            assert_eq!(
                hp.first_data_page,
                data.first().map_or(mb_pdb::page::NO_PAGE, |p| p.number)
            );
            let mut flagged: Vec<u32> = data
                .iter()
                .filter(|p| p.header.in_reuse_list())
                .map(|p| p.number)
                .collect();
            let mut listed = reuse.clone();
            flagged.sort_unstable();
            listed.sort_unstable();
            assert_eq!(
                flagged, listed,
                "{name} type {}: reuse list = pages flagged 0x10",
                entry.table_type
            );

            let schema = db.table(entry.table_type).unwrap();
            let layout = schema.layout();
            for number in &chain {
                let page = pdb.page(*number).unwrap();
                let ph = pdb.page_header(*number).unwrap();
                assert_eq!(&ph.to_bytes()[..], &page[..0x28], "{name} page {number}");
                assert_eq!(ph.reserved0, 0);
                assert!((ph.xid_hi, ph.xid_lo) <= (h.xid_hi, h.xid_lo));
            }
            for page in &data {
                let ph = page.header;
                assert!(
                    ph.flags == 0x24 || ph.flags == 0x34,
                    "{name} page {}",
                    page.number
                );
                let slots = page.slots().unwrap();
                assert_eq!(
                    ph.live_count as usize,
                    slots.iter().filter(|s| s.present).count()
                );
                let dirty: Vec<u16> = slots.iter().filter(|s| s.dirty).map(|s| s.index).collect();
                assert_eq!(
                    ph.dirty_count,
                    if dirty.is_empty() {
                        NONE_13
                    } else {
                        dirty.len() as u16
                    }
                );
                assert_eq!(
                    ph.first_dirty_slot,
                    dirty.first().copied().unwrap_or(NONE_13)
                );
                assert_eq!(
                    ph.free_size as usize + ph.used_size as usize + page.footer_len(),
                    4096 - 0x28
                );
                for slot in &slots {
                    let row = mb_pdb::Row::decode(
                        schema,
                        &layout,
                        page.number,
                        *slot,
                        page.row_bytes(slot).unwrap(),
                    )
                    .unwrap();
                    assert!(row.end <= ph.used_size as usize - slot.offset as usize);
                    assert_eq!(slot.offset % 4, 0);
                    if let Some(RowHeader(raw)) = row.header {
                        let rh = RowHeader(raw);
                        assert_eq!(rh.slot(), slot.index);
                        assert_eq!(rh.table_word(), word_base + entry.table_type as u16);
                        assert!(!rh.deleted());
                        for (ci, off) in row.string_offsets.iter().enumerate() {
                            let (Some(off), Some(s)) = (off, row.values[ci].as_str()) else {
                                continue;
                            };
                            if !s.is_short() {
                                assert_eq!(off % 4, 0, "long strings are 4-aligned");
                            }
                        }
                    }
                }
            }
        }
    }
}
