//! musicbox's own track analysis (`docs/re/ops/analyse-track.md`), against rekordbox's analysis
//! of the corpus (`fixtures/oracle-a`). Skipped without the fixtures or the corpus audio.
//!
//! 1. `analysis_files_match_rekordbox`: the `.DAT`/`.EXT`/`.2EX` built by `analyse_track` equal
//!    rekordbox's local files section by section (tag order, headers, bodies), with phrase
//!    analysis on as rekordbox had it, except PSSI and PVDI (not computed yet) and a grid the
//!    two phrase passes place differently (same BPM and beat count, shifted; the 96 kHz click
//!    probe, `analysis-phrase.md`). The `djmdContent` columns, the `djmdMixerParam` row and the
//!    key equal rekordbox's. Probes by default, every track with `MB_ORACLE_FULL=1`.
//! 2. `apply_and_undo`: on a temp copy of the library, a first analysis (track reset to the
//!    imported state) and a re-analysis (rekordbox's files present) write the files, rows and
//!    `contentFile` rows the spec lists, and the key (replacing a tag key, whose row goes); undo
//!    restores everything.

mod common;
use common::*;
use mb_anlz::{AnlzFile, Tag};
use mb_library::{AnalysisSettings, Command, DbId, History, Library, analyse_track};
use mb_masterdb::rusqlite::types::Value;
use std::path::{Path, PathBuf};

struct Track {
    id: String,
    path: PathBuf,
    file_name: String,
    rel: String,
    columns: [i64; 5],
    mixer: Option<[i64; 4]>,
    /// `djmdKey.ScaleName` of the stored key.
    key: Option<String>,
}

fn tracks() -> Option<(PathBuf, Vec<Track>)> {
    let db = open_read_only("oracle-a")?;
    let share = fixture("oracle-a")?.join("rekordbox/share");
    let mut stmt = db
        .connection()
        .prepare(
            "SELECT c.ID, c.FolderPath, c.FileNameL, c.AnalysisDataPath, c.BPM, c.SampleRate, c.BitRate, c.BitDepth, c.Length, \
             m.GainHigh, m.GainLow, m.PeakHigh, m.PeakLow, k.ScaleName \
             FROM djmdContent c LEFT JOIN djmdMixerParam m ON m.ContentID = c.ID AND m.rb_local_deleted = 0 \
             LEFT JOIN djmdKey k ON k.ID = c.KeyID AND k.rb_local_deleted = 0 \
             WHERE c.rb_local_deleted = 0 ORDER BY c.FolderPath",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            let int = |i: usize| r.get::<_, Option<i64>>(i).map(|v| v.unwrap_or(0));
            let m: [Option<i64>; 4] = [r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?];
            Ok(Track {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                file_name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                rel: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                columns: [int(4)?, int(5)?, int(6)?, int(7)?, int(8)?],
                mixer: m.iter().all(Option::is_some).then(|| m.map(|v| v.unwrap_or(0))),
                key: r.get(13)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    Some((share, rows))
}

fn load(share: &Path, rel: &str, ext: &str) -> Option<AnlzFile> {
    let p = share.join(rel.trim_start_matches('/')).with_extension(ext);
    AnlzFile::parse(&std::fs::read(p).ok()?).ok()
}

#[test]
fn analysis_files_match_rekordbox() {
    let Some((share, all)) = tracks() else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let full = std::env::var_os("MB_ORACLE_FULL").is_some();
    // Tracks rekordbox 7.2.19 analysed itself (they carry a PQT2) whose audio is present.
    let selected: Vec<&Track> = all
        .iter()
        .filter(|t| t.path.exists())
        .filter(|t| load(&share, &t.rel, "EXT").is_some_and(|f| f.ext_beat_grid().is_some()))
        .filter(|t| full || t.path.to_string_lossy().contains("/probes/"))
        .collect();
    if selected.is_empty() {
        eprintln!("skipped: corpus audio missing");
        return;
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let results: Vec<(String, Vec<String>, bool)> = std::thread::scope(|s| {
        let chunks: Vec<Vec<&Track>> = (0..threads).map(|i| selected.iter().skip(i).step_by(threads).copied().collect()).collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let share = &share;
                s.spawn(move || chunk.into_iter().map(|t| check_track(share, t)).collect::<Vec<_>>())
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let mut failures = Vec::new();
    let mut shifted = 0;
    for (name, problems, grid_shift) in &results {
        shifted += usize::from(*grid_shift);
        if !problems.is_empty() {
            failures.push(format!("{name}: {}", problems.join("; ")));
        }
    }
    let identical = results.iter().filter(|(_, p, s)| p.is_empty() && !s).count();
    eprintln!(
        "{} tracks: {identical} identical apart from PSSI/PVDI, {shifted} with a shifted grid, {} failures",
        results.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// (name, problems, grid moved by the phrase pass).
fn check_track(share: &Path, t: &Track) -> (String, Vec<String>, bool) {
    let name = t.path.file_name().unwrap().to_string_lossy().into_owned();
    let mut problems = Vec::new();
    // oracle-a was analysed with phrase analysis on (the key follows the shifted grid).
    let settings = AnalysisSettings { phrase: true, ..AnalysisSettings::default() };
    let a = match analyse_track(&t.path, &t.file_name, &settings) {
        Ok(a) => a,
        Err(e) => return (name, vec![e.to_string()], false),
    };
    let mut grid_shift = false;
    for (ext, ours) in [("DAT", &a.dat), ("EXT", &a.ext), ("2EX", &a.ex2)] {
        let theirs = load(share, &t.rel, ext).unwrap();
        if ours.header != theirs.header {
            problems.push(format!("{ext} PMAI header"));
        }
        let want: Vec<_> = theirs.sections.iter().filter(|s| s.tag != Tag::PSSI && s.tag != Tag::PVDI).collect();
        let tags = |v: &[&mb_anlz::Section]| v.iter().map(|s| s.tag.to_string()).collect::<Vec<_>>().join(" ");
        let got: Vec<_> = ours.sections.iter().collect();
        if tags(&got) != tags(&want) {
            problems.push(format!("{ext} tags {} vs {}", tags(&got), tags(&want)));
            continue;
        }
        for (g, w) in got.iter().zip(&want) {
            if g == w {
                continue;
            }
            if g.tag == Tag::PQTZ || g.tag == Tag::PQT2 {
                // Moved by rekordbox's phrase pass: same BPM and beat count.
                let same_count = a.dat.beat_grid().map(|b| b.beats.len()) == theirs_grid_len(share, &t.rel);
                if same_count && a.bpm_x100 == Some(t.columns[0]) {
                    grid_shift = true;
                    continue;
                }
            }
            if g.tag == Tag::PVBR {
                let (x, y) = (mb_anlz::tags::Vbr::decode(g).unwrap(), mb_anlz::tags::Vbr::decode(w).unwrap());
                let first = x.index.iter().zip(&y.index).position(|(a, b)| a != b);
                problems.push(format!("PVBR count {:?} vs {:?}, first index difference {first:?}", x.sample_count, y.sample_count));
                continue;
            }
            problems.push(format!("{ext} {} differs", g.tag));
        }
    }
    let ours = [a.bpm_x100.unwrap_or(0), a.sample_rate, a.bit_rate, a.bit_depth, a.length];
    for (i, col) in ["BPM", "SampleRate", "BitRate", "BitDepth", "Length"].iter().enumerate() {
        if ours[i] != t.columns[i] {
            problems.push(format!("{col} {} vs {}", ours[i], t.columns[i]));
        }
    }
    if a.mixer != t.mixer {
        problems.push(format!("mixer {:?} vs {:?}", a.mixer, t.mixer));
    }
    if a.key != t.key {
        problems.push(format!("key {:?} vs {:?}", a.key, t.key));
    }
    (name, problems, grid_shift)
}

fn theirs_grid_len(share: &Path, rel: &str) -> Option<usize> {
    load(share, rel, "DAT")?.beat_grid().map(|b| b.beats.len())
}

fn get(lib: &Library, col: &str, id: &str) -> Value {
    lib.connection().query_row(&format!("SELECT {col} FROM djmdContent WHERE ID = ?1"), [id], |r| r.get(0)).unwrap()
}

fn content_files(lib: &Library, id: &str) -> Vec<(String, String, i64)> {
    let mut stmt =
        lib.connection().prepare("SELECT Path, Hash, Size FROM contentFile WHERE ContentID = ?1 AND rb_local_deleted = 0 ORDER BY Path").unwrap();
    stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

#[test]
fn apply_and_undo() {
    let Some(dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let Some((share, all)) = tracks() else { return };
    // Two probes rekordbox analysed: one is reset to "imported", one is re-analysed.
    let mut picks = all
        .iter()
        .filter(|t| t.path.exists() && t.path.to_string_lossy().contains("/probes/") && t.path.extension().is_some_and(|e| e == "wav"))
        .filter(|t| load(&share, &t.rel, "EXT").is_some_and(|f| f.ext_beat_grid().is_some() && f.sections_with(Tag::PSSI).next().is_some()));
    let (Some(first), Some(again)) = (picks.next(), picks.next()) else {
        eprintln!("skipped: corpus audio missing");
        return;
    };
    // A private analysis root: only the re-analysed track's files are copied in.
    let root = dir.dir.join("share");
    let again_dat = root.join(again.rel.trim_start_matches('/'));
    std::fs::create_dir_all(again_dat.parent().unwrap()).unwrap();
    for ext in ["DAT", "EXT", "2EX"] {
        std::fs::copy(share.join(again.rel.trim_start_matches('/')).with_extension(ext), again_dat.with_extension(ext)).unwrap();
    }
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(&root);
    lib.set_anlz_backup(dir.dir.join("backup"));
    // Test setup (not an op): the first track back to the state after import, with a key from
    // its tags that no other track uses.
    lib.connection()
        .execute_batch(&format!(
            "UPDATE djmdContent SET Analysed = 0, ContentLink = 14, AnalysisDataPath = NULL, KeyID = '4242424242' WHERE ID = '{0}';
             DELETE FROM djmdMixerParam WHERE ContentID = '{0}';
             DELETE FROM contentFile WHERE ContentID = '{0}';
             INSERT INTO djmdKey (ID, ScaleName, UUID, rb_data_status, rb_local_deleted, rb_local_synced, rb_local_usn, created_at, updated_at)
               VALUES ('4242424242', 'Dmin', 'test-tag-key', 0, 0, 0, 0, '2026-10-12 00:00:00.000 +00:00', '2026-10-12 00:00:00.000 +00:00');",
            first.id
        ))
        .unwrap();
    let key_name = |lib: &Library, id: &str| -> Option<String> {
        lib.connection()
            .query_row("SELECT k.ScaleName FROM djmdContent c JOIN djmdKey k ON k.ID = c.KeyID AND k.rb_local_deleted = 0 WHERE c.ID = ?1", [id], |r| r.get(0))
            .ok()
    };
    let live_keys = |lib: &Library, name: &str| {
        count(lib.connection(), &format!("SELECT count(*) FROM djmdKey WHERE rb_local_deleted = 0 AND ScaleName = '{name}'"))
    };
    let mut history = History::default();

    // First analysis.
    let a = analyse_track(&first.path, &first.file_name, &AnalysisSettings::default()).unwrap();
    let detected = a.key.clone().expect("a key");
    let keys_before = live_keys(&lib, &detected);
    let before_counter: Value = get(&lib, "AnalysisUpdated", &first.id);
    let info_before = match get(&lib, "TrackInfoUpdated", &first.id) {
        Value::Text(t) => t.parse::<i64>().unwrap(),
        _ => 0,
    };
    history.run(&mut lib, Command::ApplyAnalysis { content: DbId(first.id.clone()), analysis: Box::new(a.clone()) }).unwrap();
    let rel = format!("/PIONEER/USBANLZ/{}/ANLZ0000.DAT", mb_anlz::usb::local_anlz_folder(&match get(&lib, "UUID", &first.id) {
        Value::Text(u) => u,
        v => panic!("{v:?}"),
    }));
    let dat = root.join(rel.trim_start_matches('/'));
    assert_eq!(std::fs::read(&dat).unwrap(), a.dat.to_bytes());
    assert_eq!(std::fs::read(dat.with_extension("EXT")).unwrap(), a.ext.to_bytes());
    assert_eq!(std::fs::read(dat.with_extension("2EX")).unwrap(), a.ex2.to_bytes());
    assert_eq!(get(&lib, "AnalysisDataPath", &first.id), Value::Text(rel.clone()));
    assert_eq!(get(&lib, "Analysed", &first.id), Value::Integer(105));
    assert_eq!(get(&lib, "ContentLink", &first.id), Value::Integer(0xC060E));
    assert_eq!(get(&lib, "BPM", &first.id), Value::Integer(a.bpm_x100.unwrap()));
    assert_eq!(get(&lib, "Length", &first.id), Value::Integer(a.length));
    assert_ne!(get(&lib, "AnalysisUpdated", &first.id), before_counter);
    // The key: its row (reused or created), the tag key's row gone, two TrackInfoUpdated bumps.
    assert_eq!(key_name(&lib, &first.id), Some(detected.clone()));
    assert_eq!(live_keys(&lib, &detected), keys_before.max(1));
    assert_eq!(live_keys(&lib, "Dmin"), 0);
    assert_eq!(get(&lib, "TrackInfoUpdated", &first.id), Value::Text((info_before + 2).to_string()));
    let mixer: (i64, i64, i64, i64) = lib
        .connection()
        .query_row("SELECT GainHigh, GainLow, PeakHigh, PeakLow FROM djmdMixerParam WHERE ContentID = ?1 AND rb_local_deleted = 0", [&first.id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    let m = a.mixer.unwrap();
    assert_eq!(mixer, (m[0], m[1], m[2], m[3]));
    let files = content_files(&lib, &first.id);
    assert_eq!(files.len(), 3);
    for (path, hash, size) in &files {
        let bytes = std::fs::read(root.join(path.trim_start_matches('/'))).unwrap();
        assert_eq!(*size, bytes.len() as i64);
        use md5::Digest;
        let want: String = md5::Md5::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hash, &want);
    }

    // Re-analysis: rekordbox's PSSI/PVDI are kept, so are their ContentLink bits.
    let old_ext = std::fs::read(again_dat.with_extension("EXT")).unwrap();
    let old_2ex = std::fs::read(again_dat.with_extension("2EX")).unwrap();
    let old_dat = std::fs::read(&again_dat).unwrap();
    let old_link = get(&lib, "ContentLink", &again.id);
    let mixers_before = count(lib.connection(), "SELECT count(*) FROM djmdMixerParam WHERE rb_local_deleted = 0");
    let b = analyse_track(&again.path, &again.file_name, &AnalysisSettings::default()).unwrap();
    history.run(&mut lib, Command::ApplyAnalysis { content: DbId(again.id.clone()), analysis: Box::new(b) }).unwrap();
    let ext = AnlzFile::parse(&std::fs::read(again_dat.with_extension("EXT")).unwrap()).unwrap();
    let old = AnlzFile::parse(&old_ext).unwrap();
    assert_eq!(ext.sections_with(Tag::PSSI).next(), old.sections_with(Tag::PSSI).next());
    let ex2 = AnlzFile::parse(&std::fs::read(again_dat.with_extension("2EX")).unwrap()).unwrap();
    assert!(ex2.sections_with(Tag::PVDI).next().is_some());
    assert_eq!(get(&lib, "ContentLink", &again.id), old_link);
    assert_eq!(get(&lib, "Analysed", &again.id), Value::Integer(105));
    assert_eq!(count(lib.connection(), "SELECT count(*) FROM djmdMixerParam WHERE rb_local_deleted = 0"), mixers_before);

    // Undo both.
    history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(std::fs::read(&again_dat).unwrap(), old_dat);
    assert_eq!(std::fs::read(again_dat.with_extension("EXT")).unwrap(), old_ext);
    assert_eq!(std::fs::read(again_dat.with_extension("2EX")).unwrap(), old_2ex);
    history.undo(&mut lib).unwrap().unwrap();
    assert!(!dat.exists() && !dat.with_extension("EXT").exists() && !dat.with_extension("2EX").exists());
    assert_eq!(get(&lib, "Analysed", &first.id), Value::Integer(0));
    assert_eq!(get(&lib, "ContentLink", &first.id), Value::Integer(14));
    assert_eq!(get(&lib, "AnalysisDataPath", &first.id), Value::Null);
    // The tag key is back (a new row with its name); the analysis's key row only if it existed before.
    assert_eq!(key_name(&lib, &first.id).as_deref(), Some("Dmin"));
    assert_eq!(live_keys(&lib, &detected), keys_before);
    assert_eq!(count(lib.connection(), &format!("SELECT count(*) FROM djmdMixerParam WHERE ContentID = '{}'", first.id)), 0);
    assert!(content_files(&lib, &first.id).is_empty());
    drop(lib);
    dir.verify_loads();
}
