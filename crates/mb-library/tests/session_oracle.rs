//! Replays the traced 2026-10-11 session (`fixtures/oracle-a/traces/sqltrace-37549.log`) through
//! mb-library on a temp copy of `fixtures/sandbox-initial` (the exact pre-session library) and
//! compares the result with rekordbox's own statements and after-DB (`fixtures/oracle-a`):
//! 1. remove the 14 demo tracks (L496–590),
//! 2. import the 161 corpus files in trace order (L602–3684),
//! 3. create + rename `oracle-all` and `oracle-small`, add 161 tracks to `oracle-all` in one
//!    batch and 5 single tracks to `oracle-small`.
//!
//! IDs, UUIDs and timestamps are random/time-dependent and only checked for shape. USNs are
//! checked exactly: rekordbox spent 5 extra bumps at launch (sampler fix-up, startup.md), so every
//! musicbox USN must equal rekordbox's minus 5.
//! Skipped without fixtures, trace or corpus.

mod common;

use common::*;
use mb_library::{Command, DbId, Effect, Library, NodeKind};
use mb_masterdb::MasterPlaylists;
use mb_masterdb::rusqlite::Connection;
use mb_masterdb::rusqlite::types::Value;
use std::collections::HashMap;
use std::path::PathBuf;

/// rekordbox's launch spent 506–510 before the first traced op; musicbox does not replay it.
const STARTUP_BUMPS: i64 = 5;

fn is_uuid_v4(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 36
        && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-')
        && b[14] == b'4'
        && matches!(b[19], b'8' | b'9' | b'a' | b'b')
        && s.chars().all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
}

fn typeof_col(conn: &Connection, table: &str, key_col: &str, key: &str, col: &str) -> String {
    conn.query_row(&format!("SELECT typeof({col}) FROM {table} WHERE {key_col} = ?1"), [key], |r| r.get(0)).unwrap()
}

fn lookup_name(conn: &Connection, table: &str, id: &Value) -> String {
    let Value::Text(id) = id else { return String::new() };
    let col = if table == "djmdKey" { "ScaleName" } else { "Name" };
    conn.query_row(&format!("SELECT {col} FROM {table} WHERE ID = ?1"), [id], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Stored value equality with SQLite affinity: a bare traced number lands as text in a VARCHAR
/// column and as REAL in a FLOAT column.
fn same_value(ours: &Value, traced: &Lit) -> bool {
    match (ours, traced) {
        (Value::Text(a), l) => *a == l.as_text(),
        (Value::Integer(a), Lit::Int(b)) => a == b,
        (Value::Real(a), Lit::Int(b)) => *a == *b as f64,
        (Value::Real(a), Lit::Real(b)) => a == b,
        _ => false,
    }
}

#[test]
fn replay_traced_session_on_sandbox_initial() {
    let (Some(lib_dir), Some(trace), Some(after)) = (TempLibrary::new("sandbox-initial"), trace(), open_read_only("oracle-a")) else {
        eprintln!("fixtures or trace missing, skipping");
        return;
    };
    let imports = traced_imports(&trace);
    if !imports.iter().all(|i| i.path().exists()) {
        eprintln!("corpus missing, skipping");
        return;
    }
    let after = after.connection();
    let mut lib = Library::open_unguarded(&lib_dir.db, &lib_dir.passphrase).unwrap();
    assert_eq!(lib.usn().unwrap(), 505, "sandbox-initial is the pre-session library");

    // ---- 1. remove the 14 demo tracks (remove-tracks.md) -------------------------------------
    let removal_ids: Vec<DbId> = {
        let l = trace.iter().find(|l| l.sql.starts_with("delete from djmdContent where")).unwrap();
        let list = &l.sql[l.sql.find("ID in (").unwrap() + 7..l.sql.len() - 1];
        list.split(", ").map(|s| DbId(s.to_owned())).collect()
    };
    assert_eq!(removal_ids.len(), 14);
    let out = lib.remove_tracks(&removal_ids).unwrap();
    // 10 DELETE-ROWS + VideoAssociate = 11, orphans: artist, album, genre, label = 4 (trace 511–525).
    assert_eq!(out.usn_spent(), 15, "remove: +11 in the transaction, +1 per orphan table");
    assert_eq!(out.inverse, None);
    {
        let c = lib.connection();
        for (table, key) in mb_library::remove::DELETE_ROWS {
            let n = count(c, &format!("SELECT count(*) FROM {table} WHERE {key} IN ({})", removal_ids.iter().map(|i| format!("'{i}'")).collect::<Vec<_>>().join(",")));
            assert_eq!(n, 0, "{table}.{key} rows of removed tracks");
        }
        assert_eq!(count(c, "SELECT count(*) FROM djmdContent"), 0);
        assert_eq!(count(c, "SELECT count(*) FROM contentFile"), 0, "52 contentFile rows gone");
        assert_eq!(count(c, "SELECT count(*) FROM djmdMixerParam"), 0, "14 mixer rows gone");
        assert_eq!(count(c, "SELECT count(*) FROM djmdSongSampler"), 0, "8 sampler links gone");
        assert_eq!(count(c, "SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 1"), 0, "never-synced rows are hard-deleted");
        // The orphans rekordbox deleted (trace L571–586) are exactly ours.
        for (table, ids) in [
            ("djmdArtist", vec!["4207772168", "415774920"]),
            ("djmdAlbum", vec!["267108510"]),
            ("djmdGenre", vec!["3229839016"]),
            ("djmdLabel", vec!["1103520049"]),
        ] {
            for id in ids {
                assert!(row(c, table, "ID", id).is_none(), "orphan {table} {id} removed");
            }
            assert_eq!(count(c, &format!("SELECT count(*) FROM {table}")), 0);
        }
    }
    lib_dir.verify_loads();

    // ---- 2. import the corpus in trace order (import-track.md) -------------------------------
    let paths: Vec<PathBuf> = imports.iter().map(|i| i.path()).collect();
    let usn_before_import = lib.usn().unwrap();
    let out = lib.import_files(&paths).unwrap();
    let Effect::Imported(report) = &out.effect else { panic!("{:?}", out.effect) };
    assert_eq!(report.imported.len(), 161, "skipped: {:?}", report.skipped);
    assert_eq!(out.usn_spent(), 440, "161 content + 93 artist + 14 genre + 97 album + 45 label + 30 key rows (USN 526–965)");
    assert!(matches!(&out.inverse, Some(Command::RemoveTracks { ids }) if ids.len() == 161));
    let ours_by_path: HashMap<PathBuf, DbId> = report.imported.iter().cloned().collect();
    let today = mb_masterdb::write::date_today();
    // Columns analysis rewrote in the after-DB: compared with the traced insert only.
    const ANALYSIS: &[&str] = &[
        "BPM", "Length", "BitRate", "BitDepth", "SampleRate", "Analysed", "ContentLink", "KeyID", "AnalysisDataPath", "ImagePath",
    ];
    const LOOKUPS: &[(&str, &str)] = &[
        ("ArtistID", "djmdArtist"),
        ("AlbumID", "djmdAlbum"),
        ("GenreID", "djmdGenre"),
        ("RemixerID", "djmdArtist"),
        ("LabelID", "djmdLabel"),
        ("KeyID", "djmdKey"),
    ];
    let c = lib.connection();
    let mut compared = 0usize;
    for imp in &imports {
        let id = &ours_by_path[&imp.path()];
        let ours = row(c, "djmdContent", "ID", id.as_str()).unwrap();
        let theirs_id = imp.text("ID").unwrap();
        let after_row = row(after, "djmdContent", "ID", &theirs_id).expect("oracle-a row");
        // Every column rekordbox's insert set.
        for col in &imp.columns {
            let traced = &imp.row[col];
            let v = &ours[col];
            match col.as_str() {
                "ID" | "MasterSongID" => {
                    assert_eq!(v, &Value::Text(id.0.clone()), "{col} = ID");
                    let n: u32 = id.0.parse().unwrap();
                    assert!((1..=0x0FFF_FFFF).contains(&n), "content ID in [1, 2^28-1]");
                }
                "UUID" => assert!(matches!(v, Value::Text(u) if is_uuid_v4(u))),
                "created_at" | "updated_at" => {
                    assert!(matches!(v, Value::Text(t) if t.len() == 30 && t.ends_with(" +00:00")), "{col} {v:?}");
                }
                "StockDate" | "DateCreated" => assert_eq!(v, &Value::Text(today.clone()), "{col} = import day"),
                "rb_local_usn" => {
                    let Lit::Int(t) = traced else { panic!() };
                    assert_eq!(v, &Value::Integer(t - STARTUP_BUMPS), "rb_local_usn of {}", imp.path().display());
                }
                c2 if LOOKUPS.iter().any(|(k, _)| *k == c2) => {
                    let table = LOOKUPS.iter().find(|(k, _)| *k == c2).unwrap().1;
                    if c2 == "KeyID" && traced.as_text() == "0" {
                        assert_eq!(v, &Value::Text("0".into()), "KeyID \"0\" without a key tag");
                    } else {
                        // Compare the referenced names: ours in our DB, theirs from the trace's lookup.
                        let names = traced_names(imp);
                        let want = match c2 {
                            "ArtistID" => &names[0],
                            "GenreID" => &names[1],
                            "AlbumID" => &names[3],
                            "RemixerID" => &names[4],
                            "LabelID" => &names[5],
                            _ => &names[6],
                        };
                        assert_eq!(&lookup_name(c, table, v), want, "{c2} of {}", imp.path().display());
                    }
                }
                _ => assert!(same_value(v, traced), "{col}: ours {v:?} rekordbox {traced:?} ({})", imp.path().display()),
            }
            // Storage class equals rekordbox's after-DB (affinity), except for counters and
            // analysis columns whose class analysis may change.
            if !ANALYSIS.contains(&col.as_str()) {
                assert_eq!(
                    typeof_col(c, "djmdContent", "ID", id.as_str(), col),
                    typeof_col(after, "djmdContent", "ID", &theirs_id, col),
                    "typeof {col}"
                );
            }
            // Columns analysis did not touch equal the oracle-a after-row too.
            if !ANALYSIS.contains(&col.as_str())
                && !matches!(col.as_str(), "ID" | "MasterSongID" | "UUID" | "created_at" | "updated_at" | "rb_local_usn" | "StockDate" | "DateCreated")
                && !LOOKUPS.iter().any(|(k, _)| *k == col)
            {
                assert_eq!(v, &after_row[col], "{col} vs oracle-a after-row ({})", imp.path().display());
            }
            compared += 1;
        }
        // Columns import never writes stay NULL.
        for col in [
            "OrgArtistID", "ComposerID", "SearchStr", "DisableQuantize", "Tag", "CueUpdated", "AnalysisUpdated", "TrackInfoUpdated",
            "Reserved2", "Reserved3", "Reserved4", "rb_LocalFolderPath", "SrcID", "SrcTitle", "SrcArtistName", "SrcAlbumName", "SrcLength", "usn",
        ] {
            assert_eq!(ours[col], Value::Null, "{col} stays NULL");
        }
        assert_eq!(ours["rb_local_data_status"], Value::Integer(0));
        for (k, _) in LOOKUPS {
            if !imp.row.contains_key(*k) {
                assert_eq!(ours[*k], Value::Null, "{k} omitted without a tag");
            }
        }
        // The lookup rows rekordbox created in this file's transaction exist with the same USN.
        for (table, cols) in &imp.created {
            let name_col = if table == "djmdKey" { "ScaleName" } else { "Name" };
            let name = cols[name_col].as_text();
            let Lit::Int(usn) = cols["rb_local_usn"] else { panic!() };
            let n = count(c, &format!("SELECT count(*) FROM {table} WHERE {name_col} = '{}' AND rb_local_usn = {}", name.replace('\'', "''"), usn - STARTUP_BUMPS));
            assert_eq!(n, 1, "{table} {name:?} created with USN {}", usn - STARTUP_BUMPS);
        }
    }
    eprintln!("import: {compared} traced column values compared over 161 rows");
    // Lookup tables: the same rows (by name) as rekordbox created.
    for (table, col, n) in [("djmdArtist", "Name", 93), ("djmdGenre", "Name", 14), ("djmdAlbum", "Name", 97), ("djmdLabel", "Name", 45), ("djmdKey", "ScaleName", 30)] {
        assert_eq!(count(c, &format!("SELECT count(*) FROM {table}")), n, "{table} rows");
        let mut stmt = c.prepare(&format!("SELECT {col}, UUID, rb_data_status, rb_local_deleted, rb_local_synced, usn FROM {table}")).unwrap();
        let rows: Vec<(String, String, i64, i64, i64, Option<i64>)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().map(Result::unwrap).collect();
        for (name, uuid, status, deleted, synced, usn) in rows {
            assert!(is_uuid_v4(&uuid));
            assert_eq!((status, deleted, synced, usn), (0, 0, 0, None), "{table} {name}");
        }
    }
    assert_eq!(lib.usn().unwrap(), usn_before_import + 440);
    let snap = lib_dir.verify_loads();
    assert_eq!(snap.tracks.len(), 161);

    // ---- 3. playlists (playlist-create.md, playlist-add-tracks.md) ---------------------------
    let traced_id_to_path: HashMap<String, PathBuf> = imports.iter().map(|i| (i.text("ID").unwrap(), i.path())).collect();
    let ours_of = |traced: &str| ours_by_path[&traced_id_to_path[traced]].clone();
    // Traced song-playlist inserts per playlist, grouped by batch USN, in order.
    let mut batches: Vec<(String, i64, Vec<String>)> = Vec::new();
    for l in trace.iter().filter(|l| l.db.ends_with("/master.db")) {
        if let Some((table, cols)) = parse_insert(&l.sql)
            && table == "djmdSongPlaylist"
        {
            let m: HashMap<_, _> = cols.into_iter().collect();
            let (p, Lit::Int(usn), cid) = (m["PlaylistID"].as_text(), m["rb_local_usn"].clone(), m["ContentID"].as_text()) else { panic!() };
            match batches.last_mut() {
                Some((bp, bu, list)) if *bp == p && *bu == usn => list.push(cid),
                _ => batches.push((p, usn, vec![cid])),
            }
        }
    }
    assert_eq!(batches.len(), 6, "one 161-track batch + 5 single adds");

    let start = lib.usn().unwrap();
    let out = lib.create_playlist(None, "Unbetitelte Liste", NodeKind::Playlist).unwrap();
    assert_eq!(out.usn_spent(), 4, "insert + 2 moves (new 2→1, CUE 1→2) + cloud filter row = 2476–2479");
    let Effect::PlaylistCreated { id: all_id } = out.effect.clone() else { panic!() };
    assert_eq!(out.inverse, Some(Command::DeletePlaylist { id: all_id.clone() }));
    let out = lib.execute(Command::RenamePlaylist { id: all_id.clone(), name: "oracle-all".into() }).unwrap();
    assert_eq!(out.usn_spent(), 1, "rename = 2595");
    let out = lib.create_playlist(None, "Unbetitelte Liste", NodeKind::Playlist).unwrap();
    assert_eq!(out.usn_spent(), 5, "insert + 3 moves + cloud filter row = 2758–2762");
    let Effect::PlaylistCreated { id: small_id } = out.effect.clone() else { panic!() };
    let out = lib.execute(Command::RenamePlaylist { id: small_id.clone(), name: "oracle-small".into() }).unwrap();
    assert_eq!(out.usn_spent(), 1, "rename = 2924");
    for (p, _, cids) in &batches {
        let target = if p == "2736485380" { &all_id } else { &small_id };
        let ids: Vec<DbId> = cids.iter().map(|c| ours_of(c)).collect();
        let out = lib.add_tracks(target, &ids).unwrap();
        assert_eq!(out.usn_spent(), 1, "one USN per add batch ({} rows)", ids.len());
    }
    assert_eq!(lib.usn().unwrap(), start + 4 + 1 + 5 + 1 + 6);

    let c = lib.connection();
    // Playlist rows equal oracle-a's except ID/UUID/timestamps/USN.
    for (name, ours_id) in [("oracle-all", &all_id), ("oracle-small", &small_id), ("CUE-Analyse Wiedergabeliste", &DbId("200000".into()))] {
        let ours = row(c, "djmdPlaylist", "ID", ours_id.as_str()).unwrap();
        let theirs_id: String = after.query_row("SELECT ID FROM djmdPlaylist WHERE Name = ?1", [name], |r| r.get(0)).unwrap();
        let theirs = row(after, "djmdPlaylist", "ID", &theirs_id).unwrap();
        for col in ["Seq", "Name", "ImagePath", "Attribute", "ParentID", "SmartList", "rb_data_status", "rb_local_data_status", "rb_local_deleted", "rb_local_synced", "usn"] {
            assert_eq!(ours[col], theirs[col], "{name}.{col}");
            assert_eq!(typeof_col(c, "djmdPlaylist", "ID", ours_id.as_str(), col), typeof_col(after, "djmdPlaylist", "ID", &theirs_id, col));
        }
        if name != "CUE-Analyse Wiedergabeliste" {
            assert!(matches!(&ours["UUID"], Value::Text(u) if is_uuid_v4(u)));
            let n: u64 = ours_id.0.parse().unwrap();
            assert!(n >= 1 && n < u32::MAX as u64, "playlist ID is a random u32");
        }
    }
    // Same rb_local_usn order as rekordbox: the CUE list was rewritten by both moves (2761 → ours 2761-5-…).
    let usn_of = |id: &DbId| -> i64 { c.query_row("SELECT rb_local_usn FROM djmdPlaylist WHERE ID = ?1", [id.as_str()], |r| r.get(0)).unwrap() };
    // rekordbox: oracle-all 2760 and CUE 2761 (second create's moves), oracle-small 2924 (rename).
    // Ours from `start`: create A = +1 insert, +2 A→1, +3 CUE→2, +4 filter; rename A +5;
    // create B = +6 insert, +7 B→1, +8 A→2, +9 CUE→3, +10 filter; rename B +11.
    assert_eq!(usn_of(&all_id), start + 8, "oracle-all last written by the second create's move");
    assert_eq!(usn_of(&DbId("200000".into())), start + 9, "CUE last written by the second create's move");
    assert_eq!(usn_of(&small_id), start + 11, "oracle-small last written by its rename");
    let filter_usns: Vec<i64> = {
        let mut stmt = c.prepare("SELECT rb_local_usn FROM djmdCloudFilterPlaylist ORDER BY rb_local_usn").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(filter_usns, vec![start + 4, start + 10], "cloud filter rows are the last write of each create");
    // Cloud filter rows: one per new playlist, same shape as oracle-a's.
    for id in [&all_id, &small_id] {
        let uuid: String = c.query_row("SELECT UUID FROM djmdPlaylist WHERE ID = ?1", [id.as_str()], |r| r.get(0)).unwrap();
        let (fid, seq, parent, fuuid): (String, Option<i64>, Option<String>, String) = c
            .query_row("SELECT ID, Seq, ParentID, UUID FROM djmdCloudFilterPlaylist WHERE PlaylistUUID = ?1", [&uuid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap();
        assert!(fid.parse::<u32>().is_ok() && is_uuid_v4(&fuuid));
        assert_eq!((seq, parent), (Some(0), None), "Seq default 0, ParentID NULL");
    }
    assert_eq!(count(c, "SELECT count(*) FROM djmdCloudFilterPlaylist"), count(after, "SELECT count(*) FROM djmdCloudFilterPlaylist"));
    // Entries: same content order, TrackNo 1…n, UUID row IDs, one USN per batch.
    for (name, ours_id) in [("oracle-all", &all_id), ("oracle-small", &small_id)] {
        let theirs_id: String = after.query_row("SELECT ID FROM djmdPlaylist WHERE Name = ?1", [name], |r| r.get(0)).unwrap();
        let theirs = mb_library::playlist::entries(after, &theirs_id).unwrap();
        let ours = mb_library::playlist::entries(c, ours_id.as_str()).unwrap();
        assert_eq!(ours.len(), theirs.len(), "{name} entries");
        for (o, t) in ours.iter().zip(&theirs) {
            assert_eq!(o.track_no, t.track_no);
            assert_eq!(o.content_id, ours_of(t.content_id.as_str()), "{name} TrackNo {}", o.track_no);
            assert!(is_uuid_v4(&o.id) && is_uuid_v4(&o.uuid) && o.id != o.uuid);
            for col in ["PlaylistID", "ContentID", "TrackNo", "ID", "UUID", "rb_data_status", "rb_local_data_status", "rb_local_deleted", "rb_local_synced", "usn", "rb_local_usn"] {
                assert_eq!(typeof_col(c, "djmdSongPlaylist", "ID", &o.id, col), typeof_col(after, "djmdSongPlaylist", "ID", &t.id, col), "{col}");
            }
        }
        let distinct_usns = count(c, &format!("SELECT count(DISTINCT rb_local_usn) FROM djmdSongPlaylist WHERE PlaylistID = '{ours_id}'"));
        let theirs_usns = count(after, &format!("SELECT count(DISTINCT rb_local_usn) FROM djmdSongPlaylist WHERE PlaylistID = '{theirs_id}'"));
        assert_eq!(distinct_usns, theirs_usns, "{name}: one USN per batch");
    }
    // The playlist rows are not touched by adds (updated_at/rb_local_usn stay at the rename).
    let (p_usn, p_upd): (i64, String) = c.query_row("SELECT rb_local_usn, updated_at FROM djmdPlaylist WHERE ID = ?1", [small_id.as_str()], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(p_usn, start + 4 + 1 + 5 + 1, "oracle-small keeps its rename USN");
    assert!(!p_upd.is_empty());

    // masterPlaylists6.xml: oracle-a's file with our IDs and timestamps, byte for byte.
    let ours_xml = std::fs::read_to_string(lib_dir.xml_path()).unwrap();
    let doc = MasterPlaylists::parse(&ours_xml).unwrap();
    assert_eq!(doc.nodes.len(), 3);
    let fixture_xml = std::fs::read_to_string(fixture("oracle-a").unwrap().join("rekordbox/masterPlaylists6.xml")).unwrap();
    let expected = fixture_xml
        .replace("3DC1936A", &format!("{:X}", small_id.0.parse::<u64>().unwrap()))
        .replace("A31B7404", &format!("{:X}", all_id.0.parse::<u64>().unwrap()))
        .replace("1791749159904", &doc.nodes[0].timestamp.to_string())
        .replace("1791749078209", &doc.nodes[1].timestamp.to_string());
    assert_eq!(ours_xml, expected, "masterPlaylists6.xml layout, order and the untouched 30D40 timestamp");
    assert!(doc.nodes[0].timestamp >= doc.nodes[1].timestamp, "oracle-small changed last");

    let snap = lib_dir.verify_loads();
    assert_eq!(snap.playlists.len(), 3);
    assert_eq!(snap.playlist_entries[&small_id].len(), 5);
    assert_eq!(snap.playlist_entries[&all_id].len(), 161);
    // Whole-session USN accounting: rekordbox's main app spent 505 → 965 (+ playlist ops) here.
    assert_eq!(lib.usn().unwrap(), 505 + 15 + 440 + 17);
}
