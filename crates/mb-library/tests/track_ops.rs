//! Metadata edits with undo, collection removal of tracks that sit in playlists, duplicate and
//! unreadable imports, undo of an import — on temp copies of `fixtures/oracle-a`.
//! Edits and the playlist side of removal are hypotheses (docs/re/ops/edit-track.md,
//! remove-tracks.md § Open); the tests pin the chosen pattern and the invariants.

mod common;

use common::*;
use mb_library::playlist;
use mb_library::{Command, DbId, Effect, Error, History, Library, SkipReason, TrackEdit};
use mb_masterdb::MasterPlaylists;
use mb_masterdb::rusqlite::Connection;
use std::path::PathBuf;

const SMALL: &str = "1036096362";
const ALL: &str = "2736485380";
const DEMO: &str = "200000";

fn open(dir: &TempLibrary) -> Library {
    Library::open_unguarded(&dir.db, &dir.passphrase).unwrap()
}

fn timestamp_of(dir: &TempLibrary, id: &str) -> i64 {
    let id: u64 = id.parse().unwrap();
    MasterPlaylists::read(&dir.xml_path()).unwrap().unwrap().nodes.iter().find(|n| n.id == id).unwrap().timestamp
}

fn text(c: &Connection, sql: &str, id: &str) -> Option<String> {
    c.query_row(sql, [id], |r| r.get::<_, Option<String>>(0)).unwrap()
}

/// The values an edit can change, as names.
#[derive(Debug, PartialEq)]
struct View {
    title: String,
    artist: String,
    album: String,
    album_artist_id: Option<String>,
    genre: String,
    label: String,
    key: String,
    key_id: String,
    rating: i64,
    color: String,
    comment: String,
    track_no: i64,
    year: i64,
}

fn view(c: &Connection, id: &str) -> View {
    c.query_row(
        "SELECT c.Title, ar.Name, al.Name, al.AlbumArtistID, ge.Name, la.Name, k.ScaleName, c.KeyID, c.Rating, c.ColorID, c.Commnt, c.TrackNo, c.ReleaseYear
         FROM djmdContent c LEFT JOIN djmdArtist ar ON ar.ID = c.ArtistID LEFT JOIN djmdAlbum al ON al.ID = c.AlbumID
         LEFT JOIN djmdGenre ge ON ge.ID = c.GenreID LEFT JOIN djmdLabel la ON la.ID = c.LabelID LEFT JOIN djmdKey k ON k.ID = c.KeyID
         WHERE c.ID = ?1",
        [id],
        |r| {
            Ok(View {
                title: r.get(0)?,
                artist: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                album: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                album_artist_id: r.get(3)?,
                genre: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                label: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                key: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                key_id: r.get(7)?,
                rating: r.get(8)?,
                color: r.get(9)?,
                comment: r.get(10)?,
                track_no: r.get(11)?,
                year: r.get(12)?,
            })
        },
    )
    .unwrap()
}

fn track_info_updated(c: &Connection, id: &str) -> i64 {
    text(c, "SELECT TrackInfoUpdated FROM djmdContent WHERE ID = ?1", id).map_or(0, |s| s.parse().unwrap())
}

/// Live content rows referencing artist `a` in any role, plus albums using it as album artist.
fn artist_uses(c: &Connection, a: &str) -> i64 {
    count(c, &format!(
        "SELECT (SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0 AND '{a}' IN (ArtistID, RemixerID, OrgArtistID, ComposerID)) \
         + (SELECT count(*) FROM djmdAlbum WHERE rb_local_deleted = 0 AND AlbumArtistID = '{a}')"
    ))
}

#[test]
fn metadata_edits_and_undo() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let id: String = lib
        .connection()
        .query_row(
            "SELECT ID FROM djmdContent WHERE ArtistID NOTNULL AND AlbumID NOTNULL AND GenreID NOTNULL AND LabelID NOTNULL \
             AND KeyID != '0' AND ID IN (SELECT ContentID FROM djmdSongPlaylist WHERE PlaylistID = '2736485380') ORDER BY ID LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let tid = DbId(id.clone());
    let original = view(lib.connection(), &id);
    let counter0 = track_info_updated(lib.connection(), &id);
    let all_ts = timestamp_of(&dir, ALL);
    let demo_ts = timestamp_of(&dir, DEMO);
    let mut h = History::default();
    let mut edits = 0;

    // Plain column: UPDATE-COL + TrackInfoUpdated counter.
    let out = h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: TrackEdit::Title("musicbox title".into()) }).unwrap();
    edits += 1;
    assert_eq!(out.usn_spent(), 2, "UPDATE-COL + counter");
    assert_eq!(out.inverse, Some(Command::EditTrack { id: tid.clone(), edit: TrackEdit::Title(original.title.clone()) }));
    assert_eq!(view(lib.connection(), &id).title, "musicbox title");
    assert_eq!(track_info_updated(lib.connection(), &id), counter0 + 1);
    let stored: String = lib.connection().query_row("SELECT typeof(TrackInfoUpdated) FROM djmdContent WHERE ID = ?1", [&id], |r| r.get(0)).unwrap();
    assert_eq!(stored, "text", "counters are decimal text");
    assert!(timestamp_of(&dir, ALL) > all_ts, "playlists containing the track get a new Timestamp");
    assert_eq!(timestamp_of(&dir, DEMO), demo_ts);

    // Artist: create the new name (+1), write ArtistID (+1), orphan step (+1 if the old artist is
    // now unused), counter (+1).
    let old_artist: String = text(lib.connection(), "SELECT ArtistID FROM djmdContent WHERE ID = ?1", &id).unwrap();
    let orphaned = artist_uses(lib.connection(), &old_artist) == 1;
    let out = h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: TrackEdit::Artist("musicbox artist".into()) }).unwrap();
    edits += 1;
    assert_eq!(out.usn_spent(), 3 + i64::from(orphaned));
    assert_eq!(view(lib.connection(), &id).artist, "musicbox artist");
    assert_eq!(row(lib.connection(), "djmdArtist", "ID", &old_artist).is_none(), orphaned, "old artist removed iff orphaned");
    // A second track gets the same (now existing) artist: lookup hit, no create.
    let other: String = lib.connection().query_row("SELECT ID FROM djmdContent WHERE ID != ?1 ORDER BY ID LIMIT 1", [&id], |r| r.get(0)).unwrap();
    let other_artist = text(lib.connection(), "SELECT ArtistID FROM djmdContent WHERE ID = ?1", &other);
    let other_orphaned = other_artist.as_deref().is_some_and(|a| artist_uses(lib.connection(), a) == 1);
    let out = h.run(&mut lib, Command::EditTrack { id: DbId(other.clone()), edit: TrackEdit::Artist("musicbox artist".into()) }).unwrap();
    edits += 1;
    assert_eq!(out.usn_spent(), 2 + i64::from(other_orphaned), "existing name is reused");
    assert_eq!(count(lib.connection(), "SELECT count(*) FROM djmdArtist WHERE Name = 'musicbox artist'"), 1);

    // Album keeps the album artist of the current album.
    h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: TrackEdit::Album("musicbox album".into()) }).unwrap();
    edits += 1;
    let v = view(lib.connection(), &id);
    assert_eq!((v.album.as_str(), &v.album_artist_id), ("musicbox album", &original.album_artist_id));

    // Genre, label cleared (NULL), key to an existing name and then cleared ("0").
    h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: TrackEdit::Genre("musicbox genre".into()) }).unwrap();
    h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: TrackEdit::Label(String::new()) }).unwrap();
    edits += 2;
    assert_eq!(text(lib.connection(), "SELECT LabelID FROM djmdContent WHERE ID = ?1", &id), None);
    let other_key: String = lib
        .connection()
        .query_row("SELECT ScaleName FROM djmdKey WHERE ScaleName != ?1 ORDER BY ID LIMIT 1", [&original.key], |r| r.get(0))
        .unwrap();
    let keys_before = count(lib.connection(), "SELECT count(*) FROM djmdKey");
    h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: TrackEdit::Key(other_key.clone()) }).unwrap();
    edits += 1;
    assert_eq!(view(lib.connection(), &id).key, other_key);
    assert!(count(lib.connection(), "SELECT count(*) FROM djmdKey") <= keys_before, "existing key reused");
    h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: TrackEdit::Key(String::new()) }).unwrap();
    edits += 1;
    assert_eq!(view(lib.connection(), &id).key_id, "0", "no key = KeyID \"0\"");

    // Numbers and text.
    for e in [TrackEdit::Rating(4), TrackEdit::Color(5), TrackEdit::Comment("musicbox".into()), TrackEdit::TrackNo(7), TrackEdit::ReleaseYear(1999)] {
        let out = h.run(&mut lib, Command::EditTrack { id: tid.clone(), edit: e }).unwrap();
        assert_eq!(out.usn_spent(), 2);
        edits += 1;
    }
    let v = view(lib.connection(), &id);
    assert_eq!((v.rating, v.color.as_str(), v.comment.as_str(), v.track_no, v.year), (4, "5", "musicbox", 7, 1999));
    let typ: String = lib.connection().query_row("SELECT typeof(ColorID) FROM djmdContent WHERE ID = ?1", [&id], |r| r.get(0)).unwrap();
    assert_eq!(typ, "text", "ColorID is VARCHAR");

    // No-op and invalid edits write nothing.
    let usn = lib.usn().unwrap();
    let out = lib.edit_track(&tid, TrackEdit::Rating(4)).unwrap();
    assert_eq!((out.effect, out.inverse), (Effect::Unchanged, None));
    assert!(matches!(lib.edit_track(&tid, TrackEdit::Rating(6)), Err(Error::Invalid(_))));
    assert!(matches!(lib.edit_track(&tid, TrackEdit::Color(9)), Err(Error::Invalid(_))));
    assert!(matches!(lib.edit_track(&DbId("1".into()), TrackEdit::Title("x".into())), Err(Error::NotFound { .. })));
    assert_eq!(lib.usn().unwrap(), usn);
    assert_eq!(track_info_updated(lib.connection(), &id), counter0 + edits - 1, "one counter bump per edit of this track");
    dir.verify_loads();

    // Undo everything: the values come back (lookup rows by name), with new USNs; the counter keeps counting.
    let usn = lib.usn().unwrap();
    while h.can_undo() {
        h.undo(&mut lib).unwrap();
    }
    assert!(lib.usn().unwrap() > usn, "undo is forward commands with new USNs");
    let v = view(lib.connection(), &id);
    assert_eq!(
        (&v.title, &v.artist, &v.album, &v.album_artist_id, &v.genre, &v.label, &v.key, v.rating, &v.color, &v.comment, v.track_no, v.year),
        (
            &original.title, &original.artist, &original.album, &original.album_artist_id, &original.genre, &original.label, &original.key,
            original.rating, &original.color, &original.comment, original.track_no, original.year
        )
    );
    assert_eq!(count(lib.connection(), "SELECT count(*) FROM djmdArtist WHERE Name = 'musicbox artist'"), 0, "temporary rows orphan-cleaned");
    assert_eq!(count(lib.connection(), "SELECT count(*) FROM djmdAlbum WHERE Name = 'musicbox album'"), 0);
    assert_eq!(count(lib.connection(), "SELECT count(*) FROM djmdGenre WHERE Name = 'musicbox genre'"), 0);
    // Redo works on top.
    h.redo(&mut lib).unwrap().unwrap();
    assert_eq!(view(lib.connection(), &id).title, "musicbox title");
    dir.verify_loads();
}

#[test]
fn removing_playlist_tracks_renumbers_and_cleans_up() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let small = playlist::entries(lib.connection(), SMALL).unwrap();
    let victims = vec![small[1].content_id.clone(), small[3].content_id.clone()];
    let c = lib.connection();
    let lookup_counts = |c: &Connection| -> Vec<i64> {
        ["djmdArtist", "djmdAlbum", "djmdGenre", "djmdLabel", "djmdKey"].iter().map(|t| count(c, &format!("SELECT count(*) FROM {t}"))).collect()
    };
    let lookups_before = lookup_counts(c);
    let all_len = playlist::entries(c, ALL).unwrap().len();
    let memberships = count(c, &format!("SELECT count(*) FROM djmdSongPlaylist WHERE ContentID IN ('{}', '{}')", victims[0], victims[1]));
    assert_eq!(memberships, 4, "both tracks are in oracle-small and oracle-all");
    let files = count(c, &format!("SELECT count(*) FROM contentFile WHERE ContentID IN ('{}', '{}')", victims[0], victims[1]));
    assert!(files >= 6, "ANLZ files registered");
    let (small_ts, demo_ts) = (timestamp_of(&dir, SMALL), timestamp_of(&dir, DEMO));

    let out = lib.remove_tracks(&victims).unwrap();
    let c = lib.connection();
    let orphan_tables = lookup_counts(c).iter().zip(&lookups_before).filter(|(a, b)| a < b).count() as i64;
    // 11 (remove-tracks.md) + 1 per orphan table + 1 gap-closing shift per playlist membership.
    assert_eq!(out.usn_spent(), 11 + orphan_tables + memberships);
    for (table, key) in mb_library::remove::DELETE_ROWS {
        assert_eq!(count(c, &format!("SELECT count(*) FROM {table} WHERE {key} IN ('{}', '{}')", victims[0], victims[1])), 0, "{table}");
    }
    let nos: Vec<i64> = playlist::entries(c, SMALL).unwrap().iter().map(|e| e.track_no).collect();
    assert_eq!(nos, [1, 2, 3]);
    let all: Vec<i64> = playlist::entries(c, ALL).unwrap().iter().map(|e| e.track_no).collect();
    assert_eq!(all, (1..=all_len as i64 - 2).collect::<Vec<_>>());
    assert!(timestamp_of(&dir, SMALL) > small_ts);
    assert_eq!(timestamp_of(&dir, DEMO), demo_ts);
    let snap = dir.verify_loads();
    assert_eq!(snap.tracks.len(), 167);
}

#[test]
fn import_duplicates_unreadable_and_undo() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let existing: (String, String) = lib
        .connection()
        .query_row("SELECT ID, FolderPath FROM djmdContent WHERE FolderPath LIKE '%/re-work/corpus/real/%' ORDER BY ID LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let source = PathBuf::from(&existing.1);
    if !source.exists() {
        eprintln!("corpus missing, skipping");
        return;
    }
    // Duplicate (same FolderPath) and unreadable files are skipped and cost nothing.
    let junk = dir.dir.join("not audio.mp3");
    std::fs::write(&junk, b"this is not audio").unwrap();
    let out = lib.import_files(&[source.clone(), junk.clone()]).unwrap();
    let Effect::Imported(report) = &out.effect else { panic!() };
    assert!(report.imported.is_empty());
    assert_eq!(report.skipped[0], (source.clone(), SkipReason::Duplicate(DbId(existing.0.clone()))));
    assert!(matches!(&report.skipped[1], (p, SkipReason::Unreadable(_)) if p == &junk));
    assert_eq!((out.usn_spent(), out.inverse), (0, None));

    // A copy under a new path is a new track; its tag names mostly exist already.
    let copy = dir.dir.join(format!("copy of {}", source.file_name().unwrap().to_string_lossy()));
    std::fs::copy(&source, &copy).unwrap();
    let tables = ["djmdArtist", "djmdAlbum", "djmdGenre", "djmdLabel", "djmdKey"];
    let rows = |c: &Connection| -> i64 { tables.iter().map(|t| count(c, &format!("SELECT count(*) FROM {t}"))).sum() };
    let before = rows(lib.connection());
    let mut h = History::default();
    let out = h.run(&mut lib, Command::ImportFiles { paths: vec![copy.clone()] }).unwrap();
    let created = rows(lib.connection()) - before;
    assert_eq!(out.usn_spent(), 1 + created, "one bump per created lookup row + the content row");
    let Effect::Imported(report) = &out.effect else { panic!() };
    let new_id = report.imported[0].1.clone();
    let c = lib.connection();
    let (path, inode, link, analysed): (String, String, i64, i64) = c
        .query_row("SELECT FolderPath, rb_file_id, ContentLink, Analysed FROM djmdContent WHERE ID = ?1", [new_id.as_str()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(path, copy.to_string_lossy());
    assert_eq!(inode, std::fs::metadata(&copy).unwrap().ino().to_string(), "rb_file_id = inode");
    assert_eq!((link, analysed), (14, 0));
    dir.verify_loads();

    // Undo = remove the imported track (forward, new USNs); created lookup rows are orphans again.
    let out = h.undo(&mut lib).unwrap().unwrap();
    assert!(out.usn_spent() >= 11);
    assert!(row(lib.connection(), "djmdContent", "ID", new_id.as_str()).is_none());
    assert_eq!(rows(lib.connection()), before);
    dir.verify_loads();
}
