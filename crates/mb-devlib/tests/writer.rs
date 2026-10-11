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
