//! DevLibWriter: rekordbox's write rules on fresh databases, and the oracle-a rebuild compared
//! with the export rekordbox 7.2.19 made (skipped when the fixtures are absent).

#[path = "../examples/devlib_rebuild/rebuild.rs"]
#[allow(dead_code)]
mod rebuild;

use mb_devlib::diff::compare;
use mb_devlib::schema::{INDEXES, TABLES};
use mb_devlib::{
    CreateOptions, DevLibDb, DevLibWriter, ImageOwner, ListAttribute, LookupColumn, MyTagRow,
    NewCue, NewTrack,
};
use rebuild::TempDir;
use rusqlite::types::Value;
use std::path::{Path, PathBuf};

fn fixture(rel: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel);
    p.exists().then_some(p)
}

fn options() -> CreateOptions {
    CreateOptions {
        created_date: Some("2026-10-11".into()),
        ..CreateOptions::default()
    }
}

fn value(w: &DevLibWriter, sql: &str) -> Value {
    w.connection().query_row(sql, [], |r| r.get(0)).unwrap()
}

fn track(title: &str, artist: &str) -> NewTrack {
    NewTrack {
        title: title.into(),
        artist: artist.into(),
        path: format!("/Contents/{artist}/UnknownAlbum/{title}.mp3"),
        date_created: "2026-10-11".into(),
        date_added: "2026-10-11".into(),
        file_type: 1,
        bitrate: 320,
        bit_depth: 16,
        sample_rate: 44100,
        length: 300,
        file_size: 1000,
        master_db_id: 7,
        master_content_id: 100,
        ..NewTrack::default()
    }
}

#[test]
fn create_writes_rekordbox_schema_and_seed() {
    let tmp = TempDir::new("writer-create").unwrap();
    let path = tmp.0.join("PIONEER/rekordbox/exportLibrary.db");
    let w = DevLibWriter::create(&path, &options()).unwrap();
    assert!(
        DevLibWriter::create(&path, &options()).is_err(),
        "never overwrites"
    );
    let sql: Vec<String> = {
        let mut stmt = w
            .connection()
            .prepare("select sql from sqlite_master order by rowid")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let expected: Vec<String> = TABLES
        .iter()
        .map(|(_, d)| d.replacen("create table", "CREATE TABLE", 1))
        .chain(
            INDEXES
                .iter()
                .map(|(_, d)| d.replacen("create index", "CREATE INDEX", 1)),
        )
        .collect();
    assert_eq!(sql, expected);
    assert_eq!(value(&w, "PRAGMA journal_mode"), Value::Text("wal".into()));
    assert_eq!(value(&w, "PRAGMA synchronous"), Value::Integer(1), "NORMAL");
    // SQLCipher answers page_size as text.
    assert_eq!(value(&w, "PRAGMA page_size"), Value::Text("4096".into()));
    w.close().unwrap();
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "no -wal/-shm after close: {names:?}");

    let db = DevLibDb::open_immutable(&path).unwrap();
    let lib = db.load_library().unwrap();
    let prop = lib.property.unwrap();
    assert_eq!(prop.db_version, "1000");
    assert_eq!(prop.device_name, "");
    assert_eq!(prop.created_date, "2026-10-11");
    assert_eq!(prop.number_of_contents, 0);
    assert_eq!(lib.colors.len(), 8);
    assert_eq!(lib.menu_items.len(), 27);
    assert_eq!(lib.menu_items[26].name, "\u{FFFA}MATCHING\u{FFFB}");
    assert_eq!(lib.categories.len(), 22);
    assert_eq!(lib.sorts.len(), 17);
    assert_eq!(lib.sorts[0].id, 0, "sort_id starts at 0");
}

#[test]
fn track_insert_follows_insert_track() {
    let tmp = TempDir::new("writer-track").unwrap();
    let w = DevLibWriter::create(&tmp.0.join("e.db"), &options()).unwrap();
    let mut t = track("One", "Artist A");
    t.genre = "Techno".into();
    t.key = "Am".into();
    t.album = "LP".into();
    t.album_artist = "Album Artist".into();
    let first = w.add_track(&t).unwrap();
    assert_eq!(first, 1);
    // artist, then the album artist (inside the album lookup).
    assert_eq!(
        value(&w, "select name from artist where artist_id = 2"),
        Value::Text("Album Artist".into())
    );
    assert_eq!(value(&w, "select artist_id from album"), Value::Integer(2));
    assert_eq!(
        value(&w, "select typeof(image_id) from album"),
        Value::Text("null".into())
    );
    // Unknown optional lookups are NULL, key and lyricist always written, strings ''.
    let row = |col: &str| {
        value(
            &w,
            &format!("select {col} from content where content_id = 1"),
        )
    };
    assert_eq!(row("artist_id_remixer"), Value::Null);
    assert_eq!(row("label_id"), Value::Null);
    assert_eq!(row("image_id"), Value::Null);
    assert_eq!(row("titleForSearch"), Value::Null);
    assert_eq!(row("cueUpdateCount"), Value::Null);
    assert_eq!(row("artist_id_lyricist"), Value::Integer(0));
    assert_eq!(row("key_id"), Value::Integer(1));
    assert_eq!(row("subtitle"), Value::Text(String::new()));
    assert_eq!(row("fileName"), Value::Text("One.mp3".into()));
    assert_eq!(
        value(&w, "select numberOfContents from property"),
        Value::Integer(1)
    );

    // Same names reuse rows; a second track with no key still writes key_id 0.
    let mut t2 = track("Two", "Artist A");
    t2.master_content_id = 101;
    let second = w.add_track(&t2).unwrap();
    assert_eq!(second, 2);
    assert_eq!(value(&w, "select count(*) from artist"), Value::Integer(2));
    assert_eq!(
        value(&w, "select key_id from content where content_id = 2"),
        Value::Integer(0)
    );

    // Artwork: max+1, folder id/20+1, owner updated.
    let (img, path) = w.add_image(ImageOwner::Content(second), false).unwrap();
    assert_eq!((img, path.as_str()), (1, "/PIONEER/Artwork/00001/b1.jpg"));
    assert_eq!(
        value(&w, "select image_id from content where content_id = 2"),
        Value::Integer(1)
    );

    // Re-export lookup and change markers.
    assert_eq!(w.find_exported_track(&t2).unwrap(), Some(2));
    let mut reencoded = t2.clone();
    reencoded.file_size = 999;
    assert_eq!(w.find_exported_track(&reencoded).unwrap(), None);
    w.set_track_updated(2, "4").unwrap();
    w.set_analysis_updated(2, 105, "6", "").unwrap();
    assert_eq!(row_of(&w, 2, "informationUpdateCount"), Value::Integer(4));
    assert_eq!(row_of(&w, 2, "cueUpdateCount"), Value::Text(String::new()));
    assert_eq!(row_of(&w, 2, "analysedBits"), Value::Integer(105));
    assert_eq!(
        w.track_markers(2).unwrap(),
        ("4".to_string(), "6".to_string(), String::new())
    );

    // modify* writes 0 for an empty name and drops lookup rows nobody uses any more.
    w.modify_lookup(1, LookupColumn::Genre, "").unwrap();
    assert_eq!(row_of(&w, 1, "genre_id"), Value::Integer(0));
    assert_eq!(value(&w, "select count(*) from genre"), Value::Integer(0));
}

fn row_of(w: &DevLibWriter, id: u32, col: &str) -> Value {
    value(
        w,
        &format!("select {col} from content where content_id = {id}"),
    )
}

#[test]
fn lists_follow_create_new_list() {
    let tmp = TempDir::new("writer-lists").unwrap();
    let w = DevLibWriter::create(&tmp.0.join("e.db"), &options()).unwrap();
    // Each new list is created at the end and then moved to position 0 (USB export).
    let a = w.create_playlist("a", 0, false, Some(0)).unwrap();
    let b = w.create_playlist("b", 0, false, Some(0)).unwrap();
    let c = w.create_playlist("c", 0, false, None).unwrap();
    let folder = w.create_playlist("f", 0, true, Some(9)).unwrap();
    let seq = |id: u32| {
        value(
            &w,
            &format!("select sequenceNo from playlist where playlist_id = {id}"),
        )
    };
    assert_eq!(
        (seq(b), seq(a), seq(c), seq(folder)),
        (
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3)
        )
    );
    assert_eq!(
        value(&w, "select attribute from playlist where name = 'f'"),
        Value::Integer(1)
    );
    assert_eq!(
        value(&w, "select typeof(image_id) from playlist where name = 'a'"),
        Value::Text("null".into())
    );
    let child = w.create_playlist("child", folder, false, Some(0)).unwrap();
    assert_eq!(seq(child), Value::Integer(0));
    assert_eq!(
        w.find_playlist(0, "f").unwrap(),
        Some((folder, ListAttribute::Folder))
    );
    w.move_playlist(b, 0, 3).unwrap();
    assert_eq!(
        (seq(a), seq(c), seq(folder), seq(b)),
        (
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3)
        )
    );

    w.append_playlist_entry(a, 5).unwrap();
    w.append_playlist_entry(a, 6).unwrap();
    assert_eq!(
        value(&w, "select group_concat(sequenceNo) from playlist_content"),
        Value::Text("1,2".into())
    );
    w.clear_playlist(a).unwrap();
    assert_eq!(
        value(&w, "select count(*) from playlist_content"),
        Value::Integer(0)
    );

    // Histories: sibling sequenceNo starts at 1 (get_next_seq), entries are 1-based.
    let h1 = w.add_history("HISTORY 001", 0, 0).unwrap();
    let h2 = w.add_history("HISTORY 002", 0, 0).unwrap();
    assert_eq!(
        value(
            &w,
            &format!("select sequenceNo from history where history_id = {h1}")
        ),
        Value::Integer(1)
    );
    assert_eq!(
        value(
            &w,
            &format!("select sequenceNo from history where history_id = {h2}")
        ),
        Value::Integer(2)
    );
    w.add_tracks_to_history(h1, &[3, 4]).unwrap();
    w.add_tracks_to_history(h1, &[5]).unwrap();
    assert_eq!(
        value(&w, "select group_concat(sequenceNo) from history_content"),
        Value::Text("1,2,3".into())
    );

    // My Tags: tree rewrite + property marker.
    w.write_my_tags(
        42,
        &[
            MyTagRow {
                id: 1,
                seq: 0,
                name: "Genre".into(),
                attribute: ListAttribute::Folder,
                parent: 0,
            },
            MyTagRow {
                id: 900,
                seq: 0,
                name: "Techno".into(),
                attribute: ListAttribute::List,
                parent: 1,
            },
        ],
    )
    .unwrap();
    w.add_my_tags_to_track(1, &[900]).unwrap();
    assert_eq!(
        value(&w, "select myTagMasterDBID from property"),
        Value::Integer(42)
    );
    assert_eq!(value(&w, "select count(*) from myTag"), Value::Integer(2));
    assert_eq!(
        value(&w, "select count(*) from myTag_content"),
        Value::Integer(1)
    );

    // Cues: no out point = -1, isActiveLoop untouched unless set.
    let cue = w
        .add_cue(&NewCue {
            content_id: 1,
            kind: 1,
            in_usec: 1_500_000,
            out_usec: -1,
            ..NewCue::default()
        })
        .unwrap();
    assert_eq!(
        value(&w, &format!("select outUsec from cue where cue_id = {cue}")),
        Value::Integer(-1)
    );
    assert_eq!(
        value(
            &w,
            &format!("select typeof(isActiveLoop) from cue where cue_id = {cue}")
        ),
        Value::Text("null".into())
    );
    let bank = w.create_hot_cue_bank_list("bank", 0, false, None).unwrap();
    let bank_cue = w
        .add_hot_cue_bank_cue(
            bank,
            1,
            &NewCue {
                content_id: 1,
                kind: 2,
                ..NewCue::default()
            },
        )
        .unwrap();
    assert_eq!(
        value(&w, "select cue_id from hotCueBankList_cue"),
        Value::Integer(i64::from(bank_cue))
    );
    w.close().unwrap();
}

/// Writes into a copy of a rekordbox-made stick: the next IDs continue after rekordbox's.
#[test]
fn open_existing_and_append() {
    let Some(src) = fixture("oracle-a/rbtest/PIONEER/rekordbox/exportLibrary.db") else {
        eprintln!("no fixtures, skipping");
        return;
    };
    let tmp = TempDir::new("writer-open").unwrap();
    let path = tmp.0.join("exportLibrary.db");
    std::fs::copy(src, &path).unwrap();
    let w = DevLibWriter::open(&path).unwrap();
    let list = w.create_playlist("new", 0, false, Some(0)).unwrap();
    assert_eq!(list, 3);
    let mut t = track("Appended", "Somebody New");
    t.master_content_id = 1;
    assert_eq!(w.add_track(&t).unwrap(), 158);
    assert_eq!(
        value(
            &w,
            "select artist_id_artist from content where content_id = 158"
        ),
        Value::Integer(94)
    );
    assert_eq!(
        value(
            &w,
            "select group_concat(name) from (select name from playlist order by sequenceNo)"
        ),
        Value::Text("new,oracle-all,oracle-small".into())
    );
    w.close().unwrap();
    assert!(!PathBuf::from(format!("{}-wal", path.display())).exists());
}

/// The oracle-a rebuild: master.db + the two exports replayed, compared with rekordbox's file.
#[test]
fn oracle_a_rebuild_matches_rekordbox() {
    let (Some(library), Some(stick)) = (fixture("oracle-a"), fixture("oracle-a/rbtest")) else {
        eprintln!("no fixtures, skipping");
        return;
    };
    if !stick.join("PIONEER/rekordbox/exportLibrary.db").exists()
        || !library.join("options.json").exists()
    {
        eprintln!("oracle-a incomplete, skipping");
        return;
    }
    let tmp = TempDir::new("writer-oracle").unwrap();
    let out = tmp.0.join("rebuilt.db");
    let summary = rebuild::rebuild(&library, &stick, &out).unwrap();
    assert_eq!(
        summary.exports,
        vec![
            ("oracle-small".to_string(), 5, 0, 0),
            ("oracle-all".to_string(), 152, 5, 4)
        ]
    );
    for suffix in ["-wal", "-shm"] {
        assert!(!PathBuf::from(format!("{}{suffix}", out.display())).exists());
    }
    let theirs_path = tmp.0.join("theirs.db");
    std::fs::copy(
        stick.join("PIONEER/rekordbox/exportLibrary.db"),
        &theirs_path,
    )
    .unwrap();
    let ours = DevLibDb::open_read_only(&out).unwrap();
    let theirs = DevLibDb::open_read_only(&theirs_path).unwrap();
    let cmp = compare(ours.connection(), theirs.connection()).unwrap();
    assert!(cmp.content_identical(), "{cmp}");
    assert!(cmp.rootpages.is_empty(), "{cmp}");
    let setting = |n: &str| cmp.settings.iter().find(|s| s.name == n).unwrap().clone();
    assert_eq!(setting("page_count").left, setting("page_count").right);
    assert_eq!(setting("journal_mode").left, "wal");
}

// ---- caches ---------------------------------------------------------------------------------
// The writer answers the lookup / album / count / entry-count selects from memory; every result
// must be the one the select gives on the database as it stands.

/// `getAndAdd*ID` asked of SQLite: the first row in rowid order with that name, else max + 1.
fn expected_get_or_add(w: &DevLibWriter, table: &str, name: &str) -> u32 {
    use rusqlite::OptionalExtension;
    if name.is_empty() {
        return 0;
    }
    let found = w
        .connection()
        .query_row(
            &format!("select {table}_id from {table} where name = ?1"),
            [name],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .unwrap()
        .flatten()
        .unwrap_or(0) as u32;
    if found != 0 {
        return found;
    }
    let max: Option<i64> = w
        .connection()
        .query_row(&format!("select max({table}_id) from {table}"), [], |r| {
            r.get(0)
        })
        .unwrap();
    (max.unwrap_or(0) + 1) as u32
}

/// `getAndAddAlbumID` asked of SQLite (the album artist as `expected_get_or_add` gives it).
fn expected_album(w: &DevLibWriter, name: &str, compilation: u8, album_artist: &str) -> u32 {
    use rusqlite::OptionalExtension;
    if name.is_empty() {
        return 0;
    }
    let artist_id = expected_get_or_add(w, "artist", album_artist);
    let mut sql = String::from("select album_id from album where name = ?1");
    let mut args: Vec<Value> = vec![name.to_string().into()];
    if artist_id != 0 {
        sql.push_str(" and artist_id = ?2");
        args.push(i64::from(artist_id).into());
    }
    if compilation != 0 {
        sql.push_str(&format!(" and isComplation = ?{}", args.len() + 1));
        args.push(i64::from(compilation).into());
    }
    let found: Option<i64> = w
        .connection()
        .query_row(&sql, rusqlite::params_from_iter(args), |r| r.get(0))
        .optional()
        .unwrap()
        .flatten();
    if let Some(id) = found.filter(|&id| id != 0) {
        return id as u32;
    }
    let max: Option<i64> = w
        .connection()
        .query_row("select max(album_id) from album", [], |r| r.get(0))
        .unwrap();
    (max.unwrap_or(0) + 1) as u32
}

fn assert_count_matches(w: &DevLibWriter) {
    assert_eq!(
        value(w, "select numberOfContents from property"),
        value(w, "select count(*) from content")
    );
}

fn next_entry_seq(w: &DevLibWriter, list: u32) -> i64 {
    w.connection()
        .query_row(
            "select count(content_id) from playlist_content where playlist_id = ?1",
            [list],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        + 1
}

fn last_entry_seq(w: &DevLibWriter, list: u32) -> i64 {
    w.connection()
        .query_row(
            "select sequenceNo from playlist_content where playlist_id = ?1 order by rowid desc",
            [list],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn caches_give_what_the_selects_give() {
    use mb_devlib::Lookup;
    let tmp = TempDir::new("writer-caches").unwrap();
    let path = tmp.0.join("exportLibrary.db");
    let w = DevLibWriter::create(&path, &options()).unwrap();

    // Rows the writer did not insert: duplicate names (lowest rowid wins), case variants, NULL
    // and BLOB names, ID 0 (found, but 0 means "not found"), albums with NULL / text / real
    // artist IDs.
    w.connection()
        .execute_batch(
            "insert into artist(artist_id, name) values (10, 'Dup'), (5, 'Dup'), (7, 'dup'), \
             (8, NULL), (3, x'447570'), (0, 'Zero');
             insert into genre(genre_id, name) values (4, 'House'), (2, 'House');
             insert into album(album_id, name, artist_id, isComplation) values \
             (20, 'Alb', NULL, 0), (21, 'Alb', 5, 1), (22, 'Alb', 7, 0), (23, 'Alb', 'x7', 1), \
             (24, 'Alb', 7.5, 1), (0, 'Zero', NULL, 0), (30, 'Real', 7.0, 1.0);",
        )
        .unwrap();
    w.invalidate_caches();

    let lookups: &[(Lookup, &str, &str)] = &[
        (Lookup::Artist, "artist", "Dup"),
        (Lookup::Artist, "artist", "dup"),
        (Lookup::Artist, "artist", "DUP"),
        (Lookup::Artist, "artist", "Zero"),
        (Lookup::Artist, "artist", "Zero"),
        (Lookup::Artist, "artist", "New"),
        (Lookup::Artist, "artist", "New"),
        (Lookup::Artist, "artist", ""),
        (Lookup::Genre, "genre", "House"),
        (Lookup::Genre, "genre", "house"),
        (Lookup::Key, "key", "8A"),
        (Lookup::Label, "label", "Label"),
    ];
    for &(lookup, table, name) in lookups {
        let expected = expected_get_or_add(&w, table, name);
        assert_eq!(
            w.get_or_add(lookup, name).unwrap(),
            expected,
            "{table} {name:?}"
        );
    }
    assert_eq!(w.lookup_id(Lookup::Artist, "Dup").unwrap(), 5);
    assert_eq!(w.lookup_id(Lookup::Artist, "dUp").unwrap(), 0);

    let albums: &[(&str, u8, &str)] = &[
        ("Alb", 0, ""),
        ("Alb", 1, ""),
        ("Alb", 0, "dup"),
        ("Alb", 1, "dup"),
        ("Alb", 1, "dup"),
        ("Alb", 0, "Dup"),
        ("Alb", 1, "Other"),
        ("Zero", 0, ""),
        ("Zero", 0, ""),
        ("Real", 1, "dup"),
        ("Real", 0, "dup"),
        ("", 0, "dup"),
    ];
    for &(name, compilation, artist) in albums {
        let expected = expected_album(&w, name, compilation, artist);
        assert_eq!(
            w.get_or_add_album(name, compilation, artist).unwrap(),
            expected,
            "album {name:?} {compilation} {artist:?}"
        );
    }

    // Tracks, removals and numberOfContents; lookups after unused rows are deleted.
    let list = w.create_playlist("list", 0, false, Some(0)).unwrap();
    let mut ids = Vec::new();
    for i in 0..12 {
        let mut t = track(&format!("T{i}"), if i % 3 == 0 { "Dup" } else { "Solo" });
        t.genre = if i % 2 == 0 {
            "House".into()
        } else {
            "Techno".into()
        };
        t.album = "Alb".into();
        t.album_artist = if i % 4 == 0 {
            "dup".into()
        } else {
            String::new()
        };
        let artist = expected_get_or_add(&w, "artist", &t.artist);
        let id = w.add_track(&t).unwrap();
        assert_eq!(
            row_of(&w, id, "artist_id_artist"),
            Value::Integer(artist.into())
        );
        assert_count_matches(&w);
        let seq = next_entry_seq(&w, list);
        w.append_playlist_entry(list, id).unwrap();
        assert_eq!(last_entry_seq(&w, list), seq);
        ids.push(id);
    }
    w.remove_tracks(&ids[..5]).unwrap();
    assert_count_matches(&w);
    for &(lookup, table, name) in lookups {
        let expected = expected_get_or_add(&w, table, name);
        assert_eq!(
            w.get_or_add(lookup, name).unwrap(),
            expected,
            "{table} {name:?}"
        );
    }
    for &(name, compilation, artist) in albums {
        let expected = expected_album(&w, name, compilation, artist);
        assert_eq!(
            w.get_or_add_album(name, compilation, artist).unwrap(),
            expected
        );
    }
    // Entries after removed tracks, after an entry with NULL content_id (not counted), after
    // clearing, and in a list that reuses a deleted list's ID.
    for _ in 0..2 {
        let seq = next_entry_seq(&w, list);
        w.append_playlist_entry(list, ids[6]).unwrap();
        assert_eq!(last_entry_seq(&w, list), seq);
    }
    w.connection()
        .execute(
            "insert into playlist_content(playlist_id, content_id, sequenceNo) values (?1, NULL, 99)",
            [list],
        )
        .unwrap();
    w.invalidate_caches();
    let seq = next_entry_seq(&w, list);
    w.append_playlist_entry(list, ids[7]).unwrap();
    assert_eq!(last_entry_seq(&w, list), seq);
    w.clear_playlist(list).unwrap();
    w.append_playlist_entry(list, ids[7]).unwrap();
    assert_eq!(last_entry_seq(&w, list), 1);
    w.append_playlist_entry(list, ids[8]).unwrap();
    w.delete_playlist(list).unwrap();
    let again = w.create_playlist("again", 0, false, Some(0)).unwrap();
    assert_eq!(again, list, "max + 1 reuses the deleted ID");
    w.append_playlist_entry(again, ids[9]).unwrap();
    assert_eq!(last_entry_seq(&w, again), 1);

    // A rolled-back transaction: rows inserted in it are gone, IDs are handed out again.
    for explicit in [true, false] {
        w.begin().unwrap();
        let mut t = track("Rolled", "Rolled Artist");
        t.genre = "Rolled Genre".into();
        t.album = "Rolled Album".into();
        w.add_track(&t).unwrap();
        w.append_playlist_entry(again, ids[9]).unwrap();
        if explicit {
            w.rollback().unwrap();
        } else {
            // Behind the writer's back; noticed because the transaction is gone.
            w.connection().execute_batch("ROLLBACK").unwrap();
        }
        let artist = expected_get_or_add(&w, "artist", "Rolled Artist");
        let genre = expected_get_or_add(&w, "genre", "Rolled Genre");
        let album = expected_album(&w, "Rolled Album", 0, "");
        let seq = next_entry_seq(&w, again);
        let id = w.add_track(&t).unwrap();
        assert_eq!(
            row_of(&w, id, "artist_id_artist"),
            Value::Integer(artist.into())
        );
        assert_eq!(row_of(&w, id, "genre_id"), Value::Integer(genre.into()));
        assert_eq!(row_of(&w, id, "album_id"), Value::Integer(album.into()));
        assert_count_matches(&w);
        w.append_playlist_entry(again, id).unwrap();
        assert_eq!(last_entry_seq(&w, again), seq);
        w.remove_tracks(&[id]).unwrap();
    }
    w.close().unwrap();
}

/// A table whose DDL differs from rekordbox's (a NOCASE name, a column default) is not cached:
/// the select and the insert run as rekordbox sends them.
#[test]
fn foreign_schema_falls_back_to_plain_statements() {
    use mb_devlib::Lookup;
    let tmp = TempDir::new("writer-foreign").unwrap();
    let path = tmp.0.join("exportLibrary.db");
    let w = DevLibWriter::create(&path, &options()).unwrap();
    assert_eq!(w.get_or_add(Lookup::Artist, "Before").unwrap(), 1);
    w.connection()
        .execute_batch(
            "drop table artist;
             create table artist(artist_id integer primary key, name varchar collate nocase, nameForSearch varchar);
             insert into artist(artist_id, name) values (1, 'Abc');",
        )
        .unwrap();
    w.invalidate_caches();
    assert_eq!(w.lookup_id(Lookup::Artist, "ABC").unwrap(), 1);
    assert_eq!(w.get_or_add(Lookup::Artist, "aBc").unwrap(), 1);
    assert_eq!(w.get_or_add(Lookup::Artist, "Other").unwrap(), 2);
    assert_eq!(w.get_or_add(Lookup::Artist, "OTHER").unwrap(), 2);

    // A content table with a column default: the insert leaves out the zero lookup IDs like
    // rekordbox's statement, so the default applies (the standard DDL gets NULL).
    let standard = w.add_track(&track("Standard", "A")).unwrap();
    assert_eq!(row_of(&w, standard, "artist_id_remixer"), Value::Null);
    let ddl = TABLES[0].1.replace(
        "artist_id_remixer integer,",
        "artist_id_remixer integer default 0,",
    );
    w.connection()
        .execute_batch(&format!("drop table content; {ddl};"))
        .unwrap();
    w.invalidate_caches();
    let foreign = w.add_track(&track("Foreign", "A")).unwrap();
    assert_eq!(foreign, 1);
    assert_eq!(row_of(&w, foreign, "artist_id_remixer"), Value::Integer(0));
    assert_eq!(row_of(&w, foreign, "artist_id_composer"), Value::Null);
    w.close().unwrap();
}
