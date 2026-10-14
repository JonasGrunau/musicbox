//! Cue and beat-grid edits with undo on temp copies of `fixtures/oracle-a` (skipped without
//! fixtures). Both ops are binary-derived (docs/re/ops/edit-cues.md, edit-grid.md); the tests pin
//! the rows, USN costs and file effects those specs describe.

mod common;

use common::*;
use mb_anlz::tags::{BeatGrid, ExtBeatGrid};
use mb_anlz::{AnlzFile, Tag};
use mb_library::{Command, CueEdit, CueSlot, DbId, Effect, History, Library, NewCue, grid};
use mb_masterdb::MasterPlaylists;
use mb_masterdb::rusqlite::Connection;
use mb_masterdb::rusqlite::types::Value;
use std::path::{Path, PathBuf};

const SMALL: &str = "1036096362";

fn open(dir: &TempLibrary) -> Library {
    Library::open_unguarded(&dir.db, &dir.passphrase).unwrap()
}

/// A track of `oracle-small` (analysed, with a PQT2).
fn small_track(c: &Connection) -> String {
    c.query_row(
        "SELECT ContentID FROM djmdSongPlaylist WHERE PlaylistID = ?1 AND rb_local_deleted = 0 ORDER BY TrackNo LIMIT 1",
        [SMALL],
        |r| r.get(0),
    )
    .unwrap()
}

fn counter(c: &Connection, col: &str, id: &str) -> Option<String> {
    c.query_row(&format!("SELECT {col} FROM djmdContent WHERE ID = ?1"), [id], |r| r.get(0)).unwrap()
}

fn cue_row(c: &Connection, id: &str) -> Vec<(String, Value)> {
    let mut stmt = c.prepare("SELECT * FROM djmdCue WHERE ID = ?1").unwrap();
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    stmt.query_row([id], |r| Ok((0..names.len()).map(|i| (names[i].clone(), r.get::<_, Value>(i).unwrap())).collect())).unwrap()
}

fn col<'a>(row: &'a [(String, Value)], name: &str) -> &'a Value {
    &row.iter().find(|(c, _)| c == name).unwrap().1
}

fn content_cue(c: &Connection, content: &str) -> (String, i64, String, Option<i64>) {
    c.query_row("SELECT ID, rb_cue_count, Cues, rb_local_usn FROM contentCue WHERE ContentID = ?1", [content], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .unwrap()
}

fn added(effect: &Effect) -> String {
    match effect {
        Effect::CueAdded { id } => id.clone(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn add_edit_delete_cues_with_undo() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let track = small_track(lib.connection());
    let content = DbId(track.clone());
    let uuid: String = lib.connection().query_row("SELECT UUID FROM djmdContent WHERE ID = ?1", [&track], |r| r.get(0)).unwrap();
    let ts_before = MasterPlaylists::read(&dir.xml_path()).unwrap().unwrap().nodes.iter().find(|n| n.id == SMALL.parse::<u64>().unwrap()).unwrap().timestamp;

    // 1. Memory cue, no options: insert + library sync (1 USN) + CueUpdated (1 USN).
    let memory = NewCue { slot: CueSlot::Memory, in_ms: 12_345, out_ms: None, color: None, color_table_index: None, comment: String::new() };
    let out = lib.execute(Command::AddCue { content: content.clone(), cue: memory }).unwrap();
    assert_eq!(out.usn_spent(), 2);
    let mem_id = added(&out.effect);
    let c = lib.connection();
    let row = cue_row(c, &mem_id);
    assert_eq!(col(&row, "Kind"), &Value::Integer(0));
    assert_eq!(col(&row, "InMsec"), &Value::Integer(12_345));
    assert_eq!(col(&row, "InFrame"), &Value::Integer(1_851));
    assert_eq!(col(&row, "OutMsec"), &Value::Integer(-1));
    assert_eq!(col(&row, "OutFrame"), &Value::Integer(0));
    assert_eq!(col(&row, "Color"), &Value::Integer(-1));
    assert_eq!(col(&row, "ColorTableIndex"), &Value::Null);
    assert_eq!(col(&row, "Comment"), &Value::Null);
    assert_eq!(col(&row, "ContentUUID"), &Value::Text(uuid.clone()));
    assert_eq!(col(&row, "rb_local_usn"), &Value::Null, "djmdCue rows carry no USN");
    assert_eq!(counter(c, "CueUpdated", &track).as_deref(), Some("1"));
    let (cc_id, count, json, cc_usn) = content_cue(c, &track);
    assert_eq!((cc_id.as_str(), count), (uuid.as_str(), 1));
    assert!(json.starts_with(&format!("[{{\"ID\":\"{mem_id}\",\"ContentID\":\"{track}\",\"ContentUUID\":\"{uuid}\",\"InMsec\":12345,")), "{json}");
    assert_eq!(cc_usn, Some(out.usn_after - 1));

    // 2. Hot cue B with colour code and comment: + addCueOption (1 more USN).
    let hot = NewCue { slot: CueSlot::Hot(1), in_ms: 30_000, out_ms: None, color: None, color_table_index: Some(8), comment: "drop".into() };
    let out = lib.execute(Command::AddCue { content: content.clone(), cue: hot }).unwrap();
    assert_eq!(out.usn_spent(), 3);
    let hot_id = added(&out.effect);
    let row = cue_row(lib.connection(), &hot_id);
    assert_eq!(col(&row, "Kind"), &Value::Integer(2));
    assert_eq!(col(&row, "Color"), &Value::Integer(255));
    assert_eq!(col(&row, "ColorTableIndex"), &Value::Integer(8));
    assert_eq!(col(&row, "Comment"), &Value::Text("drop".into()));
    assert_eq!(col(&row, "ActiveLoop"), &Value::Integer(0));
    assert_eq!(col(&row, "BeatLoopSize"), &Value::Integer(0));

    // 3. Memory loop.
    let lp = NewCue { slot: CueSlot::Memory, in_ms: 40_000, out_ms: Some(44_000), color: Some(2), color_table_index: None, comment: String::new() };
    let loop_id = added(&lib.execute(Command::AddCue { content: content.clone(), cue: lp }).unwrap().effect);
    let row = cue_row(lib.connection(), &loop_id);
    assert_eq!((col(&row, "OutMsec"), col(&row, "OutFrame")), (&Value::Integer(44_000), &Value::Integer(6_600)));
    assert_eq!(col(&row, "Color"), &Value::Integer(2));
    assert_eq!(content_cue(lib.connection(), &track).1, 3);
    let cues = mb_library::cues::load_for_content(lib.connection(), &track).unwrap();
    assert_eq!(cues.iter().map(|c| c.slot).collect::<Vec<_>>(), vec![CueSlot::Memory, CueSlot::Hot(1), CueSlot::Memory]);
    assert_eq!(cues[2].out_ms, Some(44_000));

    // 4. Edits with undo/redo through History.
    let mut history = History::default();
    let out = history.run(&mut lib, Command::EditCue { id: hot_id.clone(), edit: CueEdit::Comment("build".into()) }).unwrap();
    assert_eq!(out.usn_spent(), 2);
    assert!(content_cue(lib.connection(), &track).2.contains("\"Comment\":\"build\""));
    history.undo(&mut lib).unwrap();
    assert_eq!(col(&cue_row(lib.connection(), &hot_id), "Comment"), &Value::Text("drop".into()));
    history.redo(&mut lib).unwrap();
    assert_eq!(col(&cue_row(lib.connection(), &hot_id), "Comment"), &Value::Text("build".into()));
    let out = lib.execute(Command::EditCue { id: hot_id.clone(), edit: CueEdit::Comment("build".into()) }).unwrap();
    assert_eq!((out.usn_spent(), out.effect), (0, Effect::Unchanged));
    lib.execute(Command::EditCue { id: mem_id.clone(), edit: CueEdit::Color(Some(5)) }).unwrap();
    assert_eq!(col(&cue_row(lib.connection(), &mem_id), "Color"), &Value::Integer(5));

    // 5. Delete and restore the identical row.
    let before = cue_row(lib.connection(), &loop_id);
    history.run(&mut lib, Command::DeleteCue { id: loop_id.clone() }).unwrap();
    assert_eq!(content_cue(lib.connection(), &track).1, 2);
    history.undo(&mut lib).unwrap();
    assert_eq!(cue_row(lib.connection(), &loop_id), before, "undo puts the same row back");
    assert_eq!(content_cue(lib.connection(), &track).1, 3);

    // Every op bumped CueUpdated and the playlist timestamp.
    assert_eq!(counter(lib.connection(), "CueUpdated", &track).as_deref(), Some("9"));
    let ts_after = MasterPlaylists::read(&dir.xml_path()).unwrap().unwrap().nodes.iter().find(|n| n.id == SMALL.parse::<u64>().unwrap()).unwrap().timestamp;
    assert!(ts_after > ts_before);
    drop(lib);
    dir.verify_loads();
}

/// Copies one track's analysis folder into a private analysis root.
fn private_anlz(dir: &TempLibrary, rel: &str) -> PathBuf {
    let root = dir.dir.join("share");
    let src = dir.paths.analysis_data_root.join(rel.trim_start_matches('/'));
    let dst = root.join(rel.trim_start_matches('/'));
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    for ext in ["DAT", "EXT", "2EX"] {
        let s = src.with_extension(ext);
        if s.exists() {
            std::fs::copy(&s, dst.with_extension(ext)).unwrap();
        }
    }
    root
}

fn read(p: &Path) -> AnlzFile {
    AnlzFile::parse(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn beat_grid_edits_rewrite_anlz_and_undo_restores_bytes() {
    let Some(dir) = TempLibrary::new("oracle-a") else { return };
    let mut lib = open(&dir);
    let track = small_track(lib.connection());
    let content = DbId(track.clone());
    let (rel, bpm, secs): (String, i64, i64) = lib
        .connection()
        .query_row("SELECT AnalysisDataPath, BPM, Length FROM djmdContent WHERE ID = ?1", [&track], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    let root = private_anlz(&dir, &rel);
    lib.set_analysis_root(&root);
    let backup = dir.dir.join("backup");
    lib.set_anlz_backup(&backup);
    let dat = root.join(rel.trim_start_matches('/'));
    let ext = dat.with_extension("EXT");
    let (dat0, ext0) = (std::fs::read(&dat).unwrap(), std::fs::read(&ext).unwrap());
    let old = read(&dat).beat_grid().unwrap();
    assert!(read(&ext).ext_beat_grid().is_some_and(|g| g.matches(&old.beats)), "fixture has a full PQT2");
    let duration = (secs * 1000) as u32;

    // Shift by 10 ms: same tempo, so only the files and AnalysisUpdated change (1 USN).
    let mut history = History::default();
    let shifted = grid::shift(&old.beats, 10, duration);
    let out = history.run(&mut lib, Command::SetBeatGrid { content: content.clone(), beats: shifted.clone() }).unwrap();
    assert_eq!(out.usn_spent(), 1);
    assert_eq!(read(&dat).beat_grid().unwrap().beats, shifted);
    let pqt2 = read(&ext).ext_beat_grid().unwrap();
    assert_eq!((pqt2.entries.len(), pqt2.checksum, pqt2.first, pqt2.last), (0, 0, Default::default(), Default::default()), "PQT2 emptied");
    assert_eq!(counter(lib.connection(), "AnalysisUpdated", &track), Some((counter_value(&dir, &track) + 1).to_string()));
    // Every other section is untouched.
    let (a, b) = (AnlzFile::parse(&dat0).unwrap(), read(&dat));
    assert_eq!(a.sections.iter().filter(|s| s.tag != Tag::PQTZ).collect::<Vec<_>>(), b.sections.iter().filter(|s| s.tag != Tag::PQTZ).collect::<Vec<_>>());
    assert!(backup.join("anlz").join(Path::new(rel.trim_start_matches('/')).parent().unwrap()).join("ANLZ0000.DAT").exists());

    // New tempo: BPM column + TrackInfoUpdated (2 USN) + AnalysisUpdated.
    let tempo = (bpm as u16).saturating_add(100);
    let regrid = grid::set_tempo(&shifted, 4, tempo, duration).unwrap();
    let out = history.run(&mut lib, Command::SetBeatGrid { content: content.clone(), beats: regrid }).unwrap();
    assert_eq!(out.usn_spent(), 3);
    let new_bpm: i64 = lib.connection().query_row("SELECT BPM FROM djmdContent WHERE ID = ?1", [&track], |r| r.get(0)).unwrap();
    assert_eq!(new_bpm, i64::from(tempo));

    // Undo twice: both files are byte-identical to the originals and the BPM is back.
    history.undo(&mut lib).unwrap();
    history.undo(&mut lib).unwrap();
    assert_eq!(std::fs::read(&dat).unwrap(), dat0);
    assert_eq!(std::fs::read(&ext).unwrap(), ext0);
    let restored: i64 = lib.connection().query_row("SELECT BPM FROM djmdContent WHERE ID = ?1", [&track], |r| r.get(0)).unwrap();
    assert_eq!(restored, bpm);
    drop(lib);
    dir.verify_loads();
    // The fixture itself was never written.
    let fixture = dir.paths.analysis_data_root.join(rel.trim_start_matches('/'));
    assert_eq!(std::fs::read(fixture).unwrap(), dat0);
    let _ = (BeatGrid::new(Vec::new()), ExtBeatGrid::checksum_of(&[]));
}

fn counter_value(dir: &TempLibrary, track: &str) -> i64 {
    let fixture = common::open_read_only("oracle-a").unwrap();
    let _ = dir;
    fixture
        .connection()
        .query_row("SELECT AnalysisUpdated FROM djmdContent WHERE ID = ?1", [track], |r| r.get::<_, Option<String>>(0))
        .unwrap()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// rekordbox's own grid edit (`fixtures/oracle-grid-edit`): "move grid right" once on the
/// oracle-b analysis of one track moved every beat +1 ms, set `BPM` to the first beat's tempo,
/// bumped `AnalysisUpdated` and `TrackInfoUpdated`, and emptied PQT2. musicbox's `grid::shift`
/// + `SetBeatGrid` must write the same `.DAT`/`.EXT` bytes and counters.
#[test]
fn grid_shift_matches_rekordbox_byte_for_byte() {
    let Some(edited) = fixture_dir("oracle-grid-edit") else { return };
    let Some(dir) = TempLibrary::new("oracle-b") else { return };
    let mut lib = open(&dir);
    let track = "4059363";
    let content = DbId(track.into());
    let (rel, secs): (String, i64) = lib
        .connection()
        .query_row("SELECT AnalysisDataPath, Length FROM djmdContent WHERE ID = ?1", [track], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let root = private_anlz(&dir, &rel);
    lib.set_analysis_root(&root);
    lib.set_anlz_backup(dir.dir.join("backup"));
    let dat = root.join(rel.trim_start_matches('/'));
    let ext = dat.with_extension("EXT");
    let old = read(&dat).beat_grid().unwrap();
    let (info0, analysis0) = (counter(lib.connection(), "TrackInfoUpdated", track), counter(lib.connection(), "AnalysisUpdated", track));

    let shifted = grid::shift(&old.beats, 1, grid::duration_ms(Some(&read(&ext)), secs as u32));
    assert_eq!(shifted.len(), old.beats.len(), "rekordbox kept every beat");
    History::default().run(&mut lib, Command::SetBeatGrid { content, beats: shifted }).unwrap();

    assert!(std::fs::read(&dat).unwrap() == std::fs::read(edited.join("ANLZ0000.DAT")).unwrap(), ".DAT differs from rekordbox's");
    assert!(std::fs::read(&ext).unwrap() == std::fs::read(edited.join("ANLZ0000.EXT")).unwrap(), ".EXT differs from rekordbox's");
    let c = lib.connection();
    let bump = |v: Option<String>| (v.unwrap().parse::<i64>().unwrap() + 1).to_string();
    assert_eq!(c.query_row("SELECT BPM FROM djmdContent WHERE ID = ?1", [track], |r| r.get::<_, i64>(0)).unwrap(), 7087);
    assert_eq!(counter(c, "TrackInfoUpdated", track), Some(bump(info0)));
    assert_eq!(counter(c, "AnalysisUpdated", track), Some(bump(analysis0)));
}

fn fixture_dir(name: &str) -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
    dir.is_dir().then_some(dir)
}
