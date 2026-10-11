//! Writer verification ladder against rekordbox 7.2.19 files (`docs/re/pdb.md` § Writer
//! verification). Fixture-based tests skip when the fixtures are absent.
//! - `fixtures/oracle-a/rbtest/PIONEER/rekordbox`: a fresh export (oracle-small, then oracle-all);
//! - `fixtures/stick/PIONEER/rekordbox`: a years-old, incrementally updated stick.

use std::path::{Path, PathBuf};

use mb_pdb::compare::compare;
use mb_pdb::encode::encode_row;
use mb_pdb::engine::Db;
use mb_pdb::library::{ExportDb, ExtDb};
use mb_pdb::lint::{Severity, kind_of, lint};
use mb_pdb::row::Row;
use mb_pdb::schema::DbKind;
use mb_pdb::tables::*;
use mb_pdb::writer::{
    Exporter, Lookups, export_like_rekordbox, ext_like_rekordbox, write_export, write_ext,
};
use mb_pdb::{Pdb, defaults, replay};

fn fixture(rel: &str) -> Option<PathBuf> {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel);
    if d.join("export.pdb").exists() && d.join("exportExt.pdb").exists() {
        Some(d)
    } else {
        eprintln!("no fixture {rel}, skipping");
        None
    }
}

fn rbtest() -> Option<PathBuf> {
    fixture("oracle-a/rbtest/PIONEER/rekordbox")
}

fn stick() -> Option<PathBuf> {
    fixture("stick/PIONEER/rekordbox")
}

fn read(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap()
}

fn assert_lint_clean(bytes: &[u8], what: &str) {
    let kind = kind_of(bytes).expect("export or ext file");
    let rep = lint(bytes, kind);
    let errors: Vec<String> = rep
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .map(ToString::to_string)
        .collect();
    assert!(
        errors.is_empty(),
        "{what}: lint errors:\n{}",
        errors.join("\n")
    );
    assert_eq!(rep.warnings(), 0, "{what}: {:?}", rep.findings);
}

/// Every slot of every page (deleted ones included) re-encodes to the bytes rekordbox stored,
/// and the allocation formula never exceeds the space the slot has. Through the typed rows too.
#[test]
fn encoder_reproduces_every_stored_row() {
    for dir in [rbtest(), stick()].into_iter().flatten() {
        for (name, kind) in [
            ("export.pdb", DbKind::Export),
            ("exportExt.pdb", DbKind::ExportExt),
        ] {
            let pdb = Pdb::parse(read(&dir, name)).unwrap();
            let mut rows = 0;
            let mut exact_alloc = 0;
            for e in &pdb.header.tables {
                let schema = kind.table(e.table_type).unwrap();
                let layout = schema.layout();
                for page in pdb.data_pages(e).unwrap() {
                    let slots = page.slots().unwrap();
                    for (i, slot) in slots.iter().enumerate() {
                        let bytes = page.row_bytes(slot).unwrap();
                        let row = Row::decode(schema, &layout, page.number, *slot, bytes).unwrap();
                        let mut enc = encode_row(kind, schema, &layout, &row.values).unwrap();
                        enc.set_slot(slot.index);
                        for r in &enc.written {
                            assert_eq!(
                                &bytes[r.clone()],
                                &enc.bytes[r.clone()],
                                "{name} page {:#x} slot {}",
                                page.number,
                                slot.index
                            );
                        }
                        let next = slots.get(i + 1).map_or(page.header.used_size, |s| s.offset);
                        let space = usize::from(next - slot.offset);
                        assert!(
                            enc.alloc() <= space,
                            "{name} page {:#x} slot {}",
                            page.number,
                            slot.index
                        );
                        exact_alloc += usize::from(enc.alloc() == space);
                        rows += 1;
                    }
                }
            }
            eprintln!(
                "{}/{name}: {rows} rows, {exact_alloc} with exactly their allocation",
                dir.display()
            );
        }
        // Typed rows (live ones) encode to the same bytes.
        let pdb = Pdb::parse(read(&dir, "export.pdb")).unwrap();
        let tracks = read_rows(&pdb, DbKind::Export, 0).unwrap();
        let layout = DbKind::Export.table(0).unwrap().layout();
        for r in &tracks {
            let typed = Track::from_row(r);
            let mut enc =
                encode_row(DbKind::Export, r.schema, &layout, &typed.to_values()).unwrap();
            enc.set_slot(r.slot.index);
            let page = pdb.page(r.page).unwrap();
            let start = 0x28 + r.slot.offset as usize;
            for w in &enc.written {
                assert_eq!(&page[start + w.start..start + w.end], &enc.bytes[w.clone()]);
            }
        }
    }
}

/// Ladder a: read → typed rows → write (compact and export sequence) → read back = same rows.
#[test]
fn ladder_a_round_trip() {
    for dir in [rbtest(), stick()].into_iter().flatten() {
        let src = Pdb::parse(read(&dir, "export.pdb")).unwrap();
        let rows = ExportDb::read(&src).unwrap();
        let out = write_export(&rows).unwrap();
        assert_eq!(
            ExportDb::read(&Pdb::parse(out.clone()).unwrap()).unwrap(),
            rows
        );
        assert_lint_clean(&out, "compact export.pdb");

        let out = export_like_rekordbox(&rows).unwrap();
        assert_lint_clean(&out, "export-sequence export.pdb");
        let back = ExportDb::read(&Pdb::parse(out).unwrap()).unwrap();
        let mut a = rows.tracks.clone();
        let mut b = back.tracks.clone();
        a.sort_by_key(|t| t.id);
        b.sort_by_key(|t| t.id);
        assert_eq!(a, b);
        assert_eq!(back.playlist_entries.len(), rows.playlist_entries.len());
        assert_eq!(back.artwork.len(), rows.artwork.len());

        let src = Pdb::parse(read(&dir, "exportExt.pdb")).unwrap();
        let rows = ExtDb::read(&src).unwrap();
        let out = write_ext(&rows).unwrap();
        assert_eq!(
            ExtDb::read(&Pdb::parse(out.clone()).unwrap()).unwrap(),
            rows
        );
        assert_lint_clean(&out, "compact exportExt.pdb");
        let out = ext_like_rekordbox(&rows).unwrap();
        assert_lint_clean(&out, "export-sequence exportExt.pdb");
    }
}

/// Ladder b: the linter accepts rekordbox's own files.
#[test]
fn ladder_b_lint_accepts_rekordbox_files() {
    for dir in [rbtest(), stick()].into_iter().flatten() {
        for name in ["export.pdb", "exportExt.pdb"] {
            assert_lint_clean(&read(&dir, name), name);
        }
    }
}

/// The linter catches broken files.
#[test]
fn lint_flags_corruption() {
    let Some(dir) = rbtest() else { return };
    let good = read(&dir, "export.pdb");
    let rep = |b: &[u8]| lint(b, DbKind::Export);
    // Live count no longer matches the present bits (page 0x04, genres).
    let mut bad = good.clone();
    bad[4 * 4096 + 0x1a] ^= 0x20;
    assert!(rep(&bad).errors() > 0);
    // A deleted row's present bit set again on the track page 0x3c.
    let mut bad = good.clone();
    bad[0x3d * 4096 - 4] |= 0x40;
    assert!(rep(&bad).errors() > 0);
    // Page transaction id above the file's.
    let mut bad = good.clone();
    bad[0x0e * 4096 + 0x10..0x0e * 4096 + 0x14].copy_from_slice(&5000u32.to_le_bytes());
    assert!(rep(&bad).errors() > 0);
    // An empty candidate inside the file that is not zero.
    let mut bad = good.clone();
    bad[0x14 * 4096 + 100] = 1;
    assert!(rep(&bad).errors() > 0);
    // Broken chain link.
    let mut bad = good;
    bad[0x34 * 4096 + 0x0c..0x34 * 4096 + 0x10].copy_from_slice(&0x2au32.to_le_bytes());
    assert!(rep(&bad).errors() > 0);
}

/// Ladders c and d: replaying the oracle stick's export history gives rekordbox's files byte for
/// byte.
#[test]
fn ladder_c_d_oracle_replay_is_byte_exact() {
    let Some(dir) = rbtest() else { return };
    let theirs = read(&dir, "export.pdb");
    let ours = replay::oracle_export(&Pdb::parse(theirs.clone()).unwrap()).unwrap();
    let c = compare(&theirs, &ours);
    assert!(c.is_identical(), "export.pdb: {c}");
    let theirs = read(&dir, "exportExt.pdb");
    let ours = replay::oracle_ext(&Pdb::parse(theirs.clone()).unwrap()).unwrap();
    let c = compare(&theirs, &ours);
    assert!(c.is_identical(), "exportExt.pdb: {c}");
}

/// Incremental mode: continue the old stick with a new playlist and a new track.
#[test]
fn incremental_export_onto_existing_stick() {
    let Some(dir) = stick() else { return };
    let src = read(&dir, "export.pdb");
    let rows = ExportDb::read(&Pdb::parse(src.clone()).unwrap()).unwrap();
    let mut ex = Exporter::load(&src).unwrap();
    let xid_before = Pdb::parse(src.clone()).unwrap().header.xid_lo;

    let max = |it: &mut dyn Iterator<Item = u32>| it.max().unwrap_or(0);
    let mut track = rows.tracks[0].clone();
    track.id = max(&mut rows.tracks.iter().map(|t| t.id)) + 1;
    track.title = "New Track".into();
    track.folder_path = "/Contents/New Artist/UnknownAlbum/new.mp3".into();
    track.file_name = "new.mp3".into();
    let artist = Artist {
        id: max(&mut rows.artists.iter().map(|a| a.id)) + 1,
        name: "New Artist".into(),
        search_str: String::new(),
    };
    track.artist_id = artist.id;
    let art = Artwork {
        id: max(&mut rows.artwork.iter().map(|a| a.id)) + 1,
        path: "/PIONEER/Artwork/00099/a9999.jpg".into(),
    };
    track.artwork_id = art.id;
    let mut lookups = Lookups::from_export(&rows);
    lookups.artists.insert(artist.id, artist.clone());
    lookups.artwork.insert(art.id, art.clone());
    let pl = PlaylistNode(TreeNode {
        id: max(&mut rows.playlists.iter().map(|p| p.0.id)) + 1,
        seq: 0,
        name: "Incremental".into(),
        artwork_id: 0,
        attribute: 0,
        parent_id: 0,
    });
    ex.create_playlist_at_top(&pl).unwrap();
    ex.add_track(&track, &lookups).unwrap();
    ex.add_entry(&PlaylistEntry {
        playlist_id: pl.0.id,
        track_id: track.id,
        entry_index: 1,
    })
    .unwrap();
    // Re-export of an existing track with a changed title: updated in place (same ID).
    let mut old = rows.tracks[1].clone();
    old.title = format!("{} (edit)", old.title);
    ex.update_track(&old).unwrap();
    let out = ex.finish();

    assert_lint_clean(&out, "incremental export.pdb");
    let pdb = Pdb::parse(out).unwrap();
    assert!(pdb.header.xid_lo > xid_before);
    let back = ExportDb::read(&pdb).unwrap();
    assert_eq!(back.tracks.len(), rows.tracks.len() + 1);
    assert!(back.tracks.contains(&track));
    assert!(back.tracks.contains(&old));
    assert!(back.artists.contains(&artist));
    assert!(back.artwork.contains(&art));
    assert_eq!(
        back.properties[0].track_count,
        rows.properties[0].track_count + 1
    );
    let top: Vec<_> = back
        .playlists
        .iter()
        .filter(|p| p.0.seq == 0 && p.0.parent_id == 0)
        .collect();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].0.name, "Incremental");
    // Every other row is untouched.
    for t in &rows.tracks {
        if t.id != old.id {
            assert!(back.tracks.contains(t));
        }
    }
}

/// A fresh export without fixtures: defaults, three tracks with artwork, one playlist.
#[test]
fn fresh_export_from_scratch() {
    let mut rows = defaults::export_db("2026-10-12");
    for i in 1..=3u32 {
        rows.artists.push(Artist {
            id: i,
            name: format!("Artist {i}"),
            search_str: String::new(),
        });
        rows.artwork.push(Artwork {
            id: i,
            path: format!("/PIONEER/Artwork/00001/a{i}.jpg"),
        });
        rows.tracks.push(Track {
            id: i,
            folder_path: format!("/Contents/Artist {i}/UnknownAlbum/t{i}.mp3"),
            file_name: format!("t{i}.mp3"),
            file_name_short: String::new(),
            title: format!("Track {i} – Ünïcode"),
            artist_id: i,
            album_id: 0,
            genre_id: 0,
            bpm_x100: 12800,
            length_s: 300,
            track_no: 0,
            bitrate: 320,
            bit_depth: 16,
            comment: "x".repeat(200),
            file_type: 1,
            rating: 0,
            release_year: 2026,
            remixer_id: 0,
            label_id: 0,
            original_artist_id: 0,
            key_id: 0,
            stock_date: "2026-10-12".into(),
            color_id: 0,
            play_count: 0,
            artwork_id: i,
            master_db_id: 1,
            master_content_id: i,
            analysis_path: format!("/PIONEER/USBANLZ/P000/0000000{i}/ANLZ0000.DAT"),
            search_str: String::new(),
            file_size: 1000,
            disc_no: 0,
            composer_id: 0,
            subtitle: String::new(),
            sample_rate: 44100,
            disable_quantize: 0,
            analysed: 41,
            release_date: String::new(),
            date_created: "2026-10-12".into(),
            content_link: 0xc0700,
            tag: String::new(),
            modified_by_rbm: String::new(),
            hot_cue_auto_load: "ON".into(),
            delivery_control: String::new(),
            delivery_comment: String::new(),
            cue_updated: String::new(),
            analysis_updated: "1".into(),
            track_info_updated: "1".into(),
            lyricist: String::new(),
            isrc: "QZS662166224".into(),
            isrc_raw: mb_pdb::DeviceString::from_text(""),
            reserved10: String::new(),
        });
        rows.playlist_entries.push(PlaylistEntry {
            playlist_id: 1,
            track_id: i,
            entry_index: i,
        });
    }
    rows.properties[0].track_count = 3;
    rows.playlists.push(PlaylistNode(TreeNode {
        id: 1,
        seq: 0,
        name: "Fresh".into(),
        artwork_id: 0,
        attribute: 0,
        parent_id: 0,
    }));
    for out in [
        write_export(&rows).unwrap(),
        export_like_rekordbox(&rows).unwrap(),
    ] {
        assert_lint_clean(&out, "fresh export.pdb");
        let back = ExportDb::read(&Pdb::parse(out).unwrap()).unwrap();
        assert_eq!(back.tracks.len(), 3);
        assert_eq!(back.tracks[0].isrc, "QZS662166224");
        assert_eq!(back.tracks[0].title, "Track 1 – Ünïcode");
        assert_eq!(back.properties[0].track_count, 3);
        assert_eq!(back.menu_items, rows.menu_items);
    }
    let ext = write_ext(&defaults::ext_db(7)).unwrap();
    assert_lint_clean(&ext, "fresh exportExt.pdb");
}

/// Storage-model behaviour on a tiny database: reuse after delete, dirty bits, reuse list.
#[test]
fn engine_reuse_and_dirty_bits() {
    let mut db = Db::create(DbKind::Export);
    db.txn(|db| {
        for _ in 0..20 {
            db.create_table()?;
        }
        Ok(())
    })
    .unwrap();
    let genre = |id: u32, name: &str| {
        Genre(Named {
            id,
            name: name.into(),
        })
        .to_values()
    };
    let a = db.txn(|db| db.insert(1, &genre(1, "Techno"))).unwrap();
    let b = db.txn(|db| db.insert(1, &genre(2, "House"))).unwrap();
    assert_eq!((a.page, a.slot, b.page, b.slot), (4, 0, 4, 1));
    // Delete slot 0: the page joins the reuse list.
    db.txn(|db| db.delete(1, a)).unwrap();
    // A row that fits at the end of the reuse page goes there, not into the hole.
    let c = db.txn(|db| db.insert(1, &genre(3, "Trance"))).unwrap();
    assert_eq!((c.page, c.slot), (4, 2));
    let out = {
        let mut d = db.clone();
        d.close();
        d.to_bytes()
    };
    assert_lint_clean(&out, "engine");
    let pdb = Pdb::parse(out).unwrap();
    let h = pdb.header_page(pdb.header.table(1).unwrap()).unwrap();
    assert_eq!(h.reuse_pages(), vec![4]);
    let page = pdb.data_pages(pdb.header.table(1).unwrap()).unwrap()[0];
    let slots = page.slots().unwrap();
    assert_eq!(
        slots
            .iter()
            .map(|s| (s.present, s.dirty))
            .collect::<Vec<_>>(),
        [(false, false), (true, false), (true, true)]
    );
    assert_eq!(pdb.header.xid_lo, 6 + 1);
}
