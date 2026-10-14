//! musicbox's own track analysis (`docs/re/ops/analyse-track.md`), against rekordbox's analysis
//! of the corpus (`fixtures/oracle-a`). Skipped without the fixtures or the corpus audio.
//!
//! 1. `analysis_files_match_rekordbox`: the `.DAT`/`.EXT`/`.2EX` built by `analyse_track` equal
//!    rekordbox's local files section by section (tag order, headers, bodies), with phrase
//!    analysis on as rekordbox had it, except PVDI (vocal detection off here). The `djmdContent`
//!    columns, the `djmdMixerParam` row and the key equal rekordbox's. Probes by default, every
//!    track with `MB_ORACLE_FULL=1`.
//! 2. `apply_and_undo`: on a temp copy of the library, a first analysis (track reset to the
//!    imported state) and a re-analysis (rekordbox's files present) write the files, rows and
//!    `contentFile` rows the spec lists, and the key (replacing a tag key, whose row goes); undo
//!    restores everything.

mod common;
use common::*;
use mb_anlz::{AnlzFile, Tag};
use mb_library::{AnalysisSettings, Command, DbId, History, Library, analyse_track, analyse_track_with, stored_grid};
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
    tracks_in("oracle-a")
}

fn tracks_in(name: &str) -> Option<(PathBuf, Vec<Track>)> {
    let db = open_read_only(name)?;
    let share = fixture(name)?.join("rekordbox/share");
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
    let results: Vec<(String, Vec<String>)> = std::thread::scope(|s| {
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
    let failures: Vec<String> =
        results.iter().filter(|(_, p)| !p.is_empty()).map(|(name, problems)| format!("{name}: {}", problems.join("; "))).collect();
    eprintln!("{} tracks: {} identical apart from PVDI, {} failures", results.len(), results.len() - failures.len(), failures.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// (name, problems).
fn check_track(share: &Path, t: &Track) -> (String, Vec<String>) {
    let name = t.path.file_name().unwrap().to_string_lossy().into_owned();
    let mut problems = Vec::new();
    // oracle-a was analysed with phrase analysis on (the key follows the shifted grid) and
    // without the embedding (no `.3EX`, no embedding bit).
    let settings = AnalysisSettings { phrase: true, embedding: false, ..AnalysisSettings::default() };
    let a = match analyse_track(&t.path, &t.file_name, &settings) {
        Ok(a) => a,
        Err(e) => return (name, vec![e.to_string()]),
    };
    for (ext, ours) in [("DAT", &a.dat), ("EXT", &a.ext), ("2EX", &a.ex2)] {
        let theirs = load(share, &t.rel, ext).unwrap();
        if ours.header != theirs.header {
            problems.push(format!("{ext} PMAI header"));
        }
        let want: Vec<_> = theirs.sections.iter().filter(|s| s.tag != Tag::PVDI).collect();
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
    (name, problems)
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
    // With rekordbox's models installed the embedding runs too (`.3EX`, bit 0x200000).
    let embedded = a.embedding.is_some();
    assert_eq!(get(&lib, "ContentLink", &first.id), Value::Integer(0xC060E | if embedded { 0x200000 } else { 0 }));
    assert_eq!(dat.with_extension("3EX").exists(), embedded);
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
    assert_eq!(files.len(), 3 + usize::from(embedded));
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
    let again_embedded = b.embedding.is_some();
    history.run(&mut lib, Command::ApplyAnalysis { content: DbId(again.id.clone()), analysis: Box::new(b) }).unwrap();
    let ext = AnlzFile::parse(&std::fs::read(again_dat.with_extension("EXT")).unwrap()).unwrap();
    let old = AnlzFile::parse(&old_ext).unwrap();
    assert_eq!(ext.sections_with(Tag::PSSI).next(), old.sections_with(Tag::PSSI).next());
    let ex2 = AnlzFile::parse(&std::fs::read(again_dat.with_extension("2EX")).unwrap()).unwrap();
    assert!(ex2.sections_with(Tag::PVDI).next().is_some());
    let Value::Integer(old_bits) = old_link else { panic!("ContentLink") };
    assert_eq!(get(&lib, "ContentLink", &again.id), Value::Integer(old_bits | if again_embedded { 0x200000 } else { 0 }));
    assert_eq!(get(&lib, "Analysed", &again.id), Value::Integer(105));
    assert_eq!(count(lib.connection(), "SELECT count(*) FROM djmdMixerParam WHERE rb_local_deleted = 0"), mixers_before);

    // Undo both.
    history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(std::fs::read(&again_dat).unwrap(), old_dat);
    assert_eq!(std::fs::read(again_dat.with_extension("EXT")).unwrap(), old_ext);
    assert_eq!(std::fs::read(again_dat.with_extension("2EX")).unwrap(), old_2ex);
    assert!(!again_dat.with_extension("3EX").exists());
    history.undo(&mut lib).unwrap().unwrap();
    assert!(!dat.exists() && !dat.with_extension("EXT").exists() && !dat.with_extension("2EX").exists());
    assert!(!dat.with_extension("3EX").exists());
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

/// PSNR (dB) of two same-sized JPEGs, over RGB.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let (a, b) = (image::load_from_memory(a).unwrap().to_rgb8(), image::load_from_memory(b).unwrap().to_rgb8());
    assert_eq!(a.dimensions(), b.dimensions());
    let se: f64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2)).sum();
    let mse = se / a.as_raw().len() as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// First analysis of a track without artwork: the embedded picture becomes the local set at
/// rekordbox's `ImagePath`, close to rekordbox's own files; a re-analysis leaves it alone; undo
/// removes it.
#[test]
fn first_analysis_adds_artwork() {
    let Some(dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let share = fixture("oracle-a").unwrap().join("rekordbox/share");
    let db = open_read_only("oracle-a").unwrap();
    let picks: Vec<(String, PathBuf, String, String)> = db
        .connection()
        .prepare("SELECT ID, FolderPath, FileNameL, ImagePath FROM djmdContent WHERE rb_local_deleted = 0 AND ImagePath <> '' ORDER BY FolderPath")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?), r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    // One file per container that carries pictures differently (MP4 covr, ID3 APIC, FLAC).
    let mut chosen = Vec::new();
    for ext in ["m4a", "mp3", "flac"] {
        if let Some(p) = picks.iter().find(|p| p.1.exists() && p.1.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))) {
            chosen.push(p.clone());
        }
    }
    if chosen.is_empty() {
        eprintln!("skipped: corpus audio missing");
        return;
    }
    let root = dir.dir.join("share");
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(&root);
    let mut history = History::default();
    for (id, audio, file_name, image_path) in &chosen {
        // Test setup (not an op): back to the state after import.
        lib.connection()
            .execute_batch(&format!(
                "UPDATE djmdContent SET ImagePath = '', Analysed = 0, ContentLink = 14, AnalysisDataPath = NULL WHERE ID = '{id}';
                 DELETE FROM contentFile WHERE ContentID = '{id}';"
            ))
            .unwrap();
        let a = analyse_track(audio, file_name, &AnalysisSettings::default()).unwrap();
        assert!(a.artwork.is_some(), "{}: no embedded picture found", audio.display());
        history.run(&mut lib, Command::ApplyAnalysis { content: DbId(id.clone()), analysis: Box::new(a.clone()) }).unwrap();
        assert_eq!(get(&lib, "ImagePath", id), Value::Text(image_path.clone()), "{}", audio.display());
        let ours = root.join(image_path.trim_start_matches('/'));
        let theirs = share.join(image_path.trim_start_matches('/'));
        for name in ["artwork.jpg", "artwork_s.jpg", "artwork_m.jpg"] {
            let (o, t) = (std::fs::read(ours.with_file_name(name)).unwrap(), std::fs::read(theirs.with_file_name(name)).unwrap());
            let db = psnr(&o, &t);
            eprintln!("{} {name}: PSNR {db:.1} dB", audio.display());
            // artwork.jpg is the picture re-encoded (34–41 dB: two quality-85 encoders; rekordbox's
            // own file is only 35 dB from the embedded FLAC picture); the thumbnails use another
            // resampler than rekordbox's CoreGraphics draw (`analyse-track.md` § Artwork).
            let min = if name == "artwork.jpg" { 30.0 } else { 20.0 };
            assert!(db > min, "{} {name}: PSNR {db:.1} dB", audio.display());
        }
        let art_row = content_files(&lib, id).into_iter().find(|(p, ..)| p == image_path).expect("artwork contentFile row");
        assert_eq!(art_row.2, std::fs::metadata(&ours).unwrap().len() as i64);

        // A re-analysis keeps the artwork as it is.
        let before = std::fs::read(&ours).unwrap();
        std::fs::write(&ours, b"marker").unwrap();
        history.run(&mut lib, Command::ApplyAnalysis { content: DbId(id.clone()), analysis: Box::new(a.clone()) }).unwrap();
        assert_eq!(std::fs::read(&ours).unwrap(), b"marker");
        std::fs::write(&ours, &before).unwrap();

        // Undo both: the artwork goes with the first analysis.
        history.undo(&mut lib).unwrap();
        assert_eq!(get(&lib, "ImagePath", id), Value::Text(image_path.clone()));
        history.undo(&mut lib).unwrap();
        assert_eq!(get(&lib, "ImagePath", id), Value::Text(String::new()));
        assert!(!ours.exists() && !ours.with_file_name("artwork_s.jpg").exists());
        assert!(!content_files(&lib, id).iter().any(|(p, ..)| p == image_path));
    }
    dir.verify_loads();
}


/// With the phrase setting on, a re-analysis writes our own PSSI (equal to rekordbox's on a
/// probe) and sets `ContentLink`'s phrase bit, without carrying the old section over.
#[test]
fn phrase_pass_writes_pssi() {
    let Some(dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let Some((share, all)) = tracks() else { return };
    let Some(t) = all.iter().find(|t| {
        t.path.exists()
            && t.path.to_string_lossy().contains("/probes/wav_44k16/")
            && load(&share, &t.rel, "EXT").is_some_and(|f| f.sections_with(Tag::PSSI).next().is_some())
    }) else {
        eprintln!("skipped: corpus audio missing");
        return;
    };
    let root = dir.dir.join("share");
    let dat = root.join(t.rel.trim_start_matches('/'));
    std::fs::create_dir_all(dat.parent().unwrap()).unwrap();
    for ext in ["DAT", "EXT", "2EX"] {
        std::fs::copy(share.join(t.rel.trim_start_matches('/')).with_extension(ext), dat.with_extension(ext)).unwrap();
    }
    // Test setup (not an op): no PSSI and no phrase bit before, so both must come from the pass.
    let mut old = load(&share, &t.rel, "EXT").unwrap();
    old.sections.retain(|s| s.tag != Tag::PSSI);
    std::fs::write(dat.with_extension("EXT"), old.to_bytes()).unwrap();
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(&root);
    lib.connection().execute("UPDATE djmdContent SET ContentLink = ContentLink & ~65536 WHERE ID = ?1", [&t.id]).unwrap();

    let settings = AnalysisSettings { phrase: true, ..AnalysisSettings::default() };
    let a = analyse_track(&t.path, &t.file_name, &settings).unwrap();
    History::default().run(&mut lib, Command::ApplyAnalysis { content: DbId(t.id.clone()), analysis: Box::new(a) }).unwrap();
    let ext = AnlzFile::parse(&std::fs::read(dat.with_extension("EXT")).unwrap()).unwrap();
    let theirs = load(&share, &t.rel, "EXT").unwrap();
    assert_eq!(ext.sections_with(Tag::PSSI).count(), 1);
    assert_eq!(ext.sections_with(Tag::PSSI).next(), theirs.sections_with(Tag::PSSI).next());
    let Value::Integer(link) = get(&lib, "ContentLink", &t.id) else { panic!("ContentLink") };
    assert_ne!(link & 0x10000, 0, "phrase bit");
}

/// With the vocal setting on, a re-analysis writes our own PVDI (equal to rekordbox's) and sets
/// `ContentLink`'s vocal bit, without carrying the old section over. Needs the installed
/// rekordbox (its network and statistics); skipped without it.
#[test]
fn vocal_pass_writes_pvdi() {
    let Some(dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let Some((share, all)) = tracks() else { return };
    let Some(t) = all.iter().find(|t| {
        t.path.exists()
            && t.path.to_string_lossy().contains("/real/")
            && load(&share, &t.rel, "2EX").is_some_and(|f| f.sections_with(Tag::PVDI).next().is_some())
    }) else {
        eprintln!("skipped: corpus audio missing");
        return;
    };
    let settings = AnalysisSettings { vocal: true, ..AnalysisSettings::default() };
    let a = analyse_track(&t.path, &t.file_name, &settings).unwrap();
    if let Some(e) = &a.vocal_error {
        eprintln!("skipped: {e}");
        return;
    }
    let root = dir.dir.join("share");
    let dat = root.join(t.rel.trim_start_matches('/'));
    std::fs::create_dir_all(dat.parent().unwrap()).unwrap();
    for ext in ["DAT", "EXT", "2EX"] {
        std::fs::copy(share.join(t.rel.trim_start_matches('/')).with_extension(ext), dat.with_extension(ext)).unwrap();
    }
    // Test setup (not an op): no PVDI and no vocal bit before, so both must come from the pass.
    let mut old = load(&share, &t.rel, "2EX").unwrap();
    old.sections.retain(|s| s.tag != Tag::PVDI);
    std::fs::write(dat.with_extension("2EX"), old.to_bytes()).unwrap();
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(&root);
    lib.connection().execute("UPDATE djmdContent SET ContentLink = ContentLink & ~1048576 WHERE ID = ?1", [&t.id]).unwrap();
    History::default().run(&mut lib, Command::ApplyAnalysis { content: DbId(t.id.clone()), analysis: Box::new(a) }).unwrap();
    let ex2 = AnlzFile::parse(&std::fs::read(dat.with_extension("2EX")).unwrap()).unwrap();
    let theirs = load(&share, &t.rel, "2EX").unwrap();
    assert_eq!(ex2.sections_with(Tag::PVDI).count(), 1);
    assert_eq!(ex2.sections_with(Tag::PVDI).next(), theirs.sections_with(Tag::PVDI).next());
    assert_eq!(ex2.sections.iter().map(|s| s.tag).collect::<Vec<_>>(), theirs.sections.iter().map(|s| s.tag).collect::<Vec<_>>());
    let Value::Integer(link) = get(&lib, "ContentLink", &t.id) else { panic!("ContentLink") };
    assert_ne!(link & 0x100000, 0, "vocal bit");
}

/// High precision without beats (oracle-b's sweep probes): with the stored grid as input, key
/// and phrase run on it as in rekordbox, so PSSI and key equal oracle-b's, and the old grid is
/// kept.
#[test]
fn high_precision_without_beats_uses_stored_grid() {
    let Some((share, all)) = tracks_in("oracle-b") else {
        eprintln!("skipped: fixtures/oracle-b missing");
        return;
    };
    let Some(t) = all.iter().find(|t| t.path.exists() && t.path.to_string_lossy().contains("/probes/wav_44k16/sweep_20_20k")) else {
        eprintln!("skipped: corpus audio missing");
        return;
    };
    let dat = share.join(t.rel.trim_start_matches('/'));
    let stored = stored_grid(&dat, Some(t.columns[0])).expect("stored grid");
    let settings = AnalysisSettings {
        phrase: true,
        high_precision: Some(mb_analysis::hpbeat::HpMode::Dynamic),
        ..AnalysisSettings::default()
    };
    let a = analyse_track_with(&t.path, &t.file_name, &settings, Some(&stored)).unwrap();
    assert!(a.keep_old_grid);
    assert_eq!(a.key, t.key);
    let theirs = load(&share, &t.rel, "EXT").unwrap();
    assert!(theirs.sections_with(Tag::PSSI).next().is_some());
    assert_eq!(a.ext.sections_with(Tag::PSSI).next(), theirs.sections_with(Tag::PSSI).next());
    // Without the stored grid there is no PSSI.
    let b = analyse_track(&t.path, &t.file_name, &settings).unwrap();
    assert!(b.ext.sections_with(Tag::PSSI).next().is_none());
}

/// The embedding on rekordbox's fresh install (`fixtures/sandbox-initial`, embedding on, vocal
/// off): an analysis of the demo track writes a `.3EX` byte-identical to rekordbox's (no PVDI in
/// the files, so the vocal share comes from the detector run for it), its `contentFile` row
/// first, and `ContentLink`'s embedding bit; undo removes the file and the row. Needs the
/// installed rekordbox (its ONNX and vocal models); skipped without it.
#[test]
fn embedding_pass_writes_3ex() {
    let Some(dir) = TempLibrary::new("sandbox-initial") else {
        eprintln!("skipped: fixtures/sandbox-initial missing");
        return;
    };
    let Some((share, all)) = tracks_in("sandbox-initial") else { return };
    let Some(t) = all.iter().find(|t| t.path.exists() && t.file_name == "Demo Track 1.mp3") else {
        eprintln!("skipped: demo track missing");
        return;
    };
    let a = analyse_track(&t.path, &t.file_name, &AnalysisSettings::default()).unwrap();
    if let Some(e) = &a.embedding_error {
        eprintln!("skipped: {e}");
        return;
    }
    assert!(a.ex2.sections_with(Tag::PVDI).next().is_none(), "vocal off: no PVDI");
    let theirs = std::fs::read(share.join(t.rel.trim_start_matches('/')).with_extension("3EX")).unwrap();
    let root = dir.dir.join("share");
    let dat = root.join(t.rel.trim_start_matches('/'));
    std::fs::create_dir_all(dat.parent().unwrap()).unwrap();
    for ext in ["DAT", "EXT", "2EX"] {
        std::fs::copy(share.join(t.rel.trim_start_matches('/')).with_extension(ext), dat.with_extension(ext)).unwrap();
    }
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(&root);
    // Test setup (not an op): no `.3EX`, no embedding bit and no `contentFile` rows before.
    lib.connection().execute("UPDATE djmdContent SET ContentLink = ContentLink & ~2097152 WHERE ID = ?1", [&t.id]).unwrap();
    lib.connection().execute("DELETE FROM contentFile WHERE ContentID = ?1", [&t.id]).unwrap();
    let link_before = get(&lib, "ContentLink", &t.id);
    let mut history = History::default();
    history.run(&mut lib, Command::ApplyAnalysis { content: DbId(t.id.clone()), analysis: Box::new(a) }).unwrap();
    let ours = std::fs::read(dat.with_extension("3EX")).unwrap();
    assert!(ours == theirs, "{:?}", mb_analysis::embed::msgpack::decode(&ours).map(|v| v.get("embedding").cloned()));
    let Value::Integer(link) = get(&lib, "ContentLink", &t.id) else { panic!("ContentLink") };
    assert_ne!(link & 0x200000, 0, "embedding bit");
    let first_row: String = lib
        .connection()
        .query_row("SELECT Path FROM contentFile WHERE ContentID = ?1 AND rb_local_deleted = 0 ORDER BY rowid LIMIT 1", [&t.id], |r| r.get(0))
        .unwrap();
    assert!(first_row.ends_with("/ANLZ0000.3EX"), "{first_row}");
    assert_eq!(content_files(&lib, &t.id).len(), 4);

    history.undo(&mut lib).unwrap().unwrap();
    assert!(!dat.with_extension("3EX").exists());
    assert_eq!(get(&lib, "ContentLink", &t.id), link_before);
    assert!(content_files(&lib, &t.id).is_empty());
}

/// CUE analysis, Manual mode: with hot cues every 8 bars (upper limit 8) on a probe that already
/// has a hot cue on pad A, the pass sets cues on the free pads at the grid's first downbeat and
/// every 32 beats, with rekordbox's comments; with overwrite on, pads below the new count are
/// cleared first. Undo leaves only the seeded cue. Off by default.
#[test]
fn manual_cue_analysis_sets_cues() {
    let Some(dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let Some((_, all)) = tracks() else { return };
    let Some(t) = all.iter().find(|t| t.path.exists() && t.path.to_string_lossy().contains("/probes/") && t.columns[0] > 0) else {
        eprintln!("skipped: corpus audio missing");
        return;
    };
    assert!(analyse_track(&t.path, &t.file_name, &AnalysisSettings { embedding: false, ..AnalysisSettings::default() }).unwrap().cue.is_none());
    let cue = mb_library::CueAnalysisSettings { enabled: true, manual: true, loop_cue: true, max_hot: 8, ..Default::default() };
    let settings = AnalysisSettings { embedding: false, cue, ..AnalysisSettings::default() };
    let a = analyse_track(&t.path, &t.file_name, &settings).unwrap();
    let beats = a.dat.beat_grid().unwrap().beats;
    let first = beats.iter().position(|b| b.beat_in_bar == 1).unwrap();
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(dir.dir.join("share"));
    let content = DbId(t.id.clone());
    let mut history = History::default();
    // Test setup (not an op under test): only the seeded hot cue A.
    lib.connection().execute("DELETE FROM djmdCue WHERE ContentID = ?1", [&t.id]).unwrap();
    let seed = mb_library::NewCue { slot: mb_library::CueSlot::Hot(0), in_ms: 777, out_ms: None, color: None, color_table_index: None, comment: String::new() };
    history.run(&mut lib, Command::AddCue { content: content.clone(), cue: seed }).unwrap();
    let cues = |lib: &Library| {
        let mut v = mb_library::cues::load_for_content(lib.connection(), &t.id).unwrap();
        v.sort_by_key(|c| (c.slot.kind(), c.in_ms));
        v.into_iter().map(|c| (c.slot, c.in_ms, c.comment)).collect::<Vec<_>>()
    };

    history.run(&mut lib, Command::ApplyAnalysis { content: content.clone(), analysis: Box::new(a.clone()) }).unwrap();
    let want: Vec<_> = (0..8)
        .map(|k| first + 32 * k)
        .filter(|&i| i < beats.len())
        .enumerate()
        .map(|(k, i)| (mb_library::CueSlot::Hot(k as u8 + 1), beats[i].time_ms, if k == 0 { "1.1Bars".to_owned() } else { format!("1.1Bars+{}", 8 * k) }))
        .collect();
    assert!(!want.is_empty());
    let got = cues(&lib);
    assert_eq!(got[0], (mb_library::CueSlot::Hot(0), 777, String::new()));
    assert_eq!(got[1..], want[..]);

    // Overwrite: undo first, then the same pass with OverwriteCue on clears pad A.
    history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(cues(&lib), [(mb_library::CueSlot::Hot(0), 777, String::new())]);
    let mut b = a.clone();
    b.cue = Some(mb_library::CueAnalysisSettings { overwrite: true, ..cue });
    history.run(&mut lib, Command::ApplyAnalysis { content: content.clone(), analysis: Box::new(b) }).unwrap();
    let got = cues(&lib);
    assert_eq!(got.len(), want.len());
    assert_eq!(got[0], (mb_library::CueSlot::Hot(0), want[0].1, "1.1Bars".to_owned()));
    history.undo(&mut lib).unwrap().unwrap();
    assert_eq!(cues(&lib), [(mb_library::CueSlot::Hot(0), 777, String::new())]);
}

/// (Kind, InMsec, Comment) of each cue, by ContentID.
type CuesByTrack = std::collections::HashMap<String, Vec<(i64, i64, String)>>;

/// The cues of a fixture run (`fixtures/oracle-cue/<run>`), read from a private copy of its
/// `master.db` and WAL so the fixture is never touched.
fn run_cues(run: &str) -> Option<CuesByTrack> {
    let root = fixture(&format!("oracle-cue/{run}"))?;
    let paths = mb_masterdb::LibraryPaths::from_options_json(&root.join("options.json")).ok()?;
    let tmp = std::env::temp_dir().join(format!("mb-library-cues-{run}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    for f in ["master.db", "master.db-wal", "master.db-shm"] {
        if root.join("rekordbox").join(f).exists() {
            std::fs::copy(root.join("rekordbox").join(f), tmp.join(f)).ok()?;
        }
    }
    let db = mb_masterdb::MasterDb::open_read_only(&tmp.join("master.db"), &mb_masterdb::derive_passphrase(&paths.dp).ok()?).ok()?;
    let mut out = CuesByTrack::new();
    {
        let mut stmt = db.connection().prepare("SELECT ContentID, Kind, InMsec, Comment FROM djmdCue WHERE rb_local_deleted = 0").ok()?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?.unwrap_or_default())))
            .ok()?;
        for (id, kind, ms, comment) in rows.flatten() {
            out.entry(id).or_default().push((kind, ms, comment));
        }
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&tmp);
    Some(out)
}

/// CUE analysis, Auto mode, against `fixtures/oracle-cue/A1` (hot cues, upper limit 8, an
/// all-zero personal tendency): the cues our own analysis and rekordbox's cue models place on the
/// 44.1 kHz WAV probes equal rekordbox's, Kind, time and "CUE(Auto)" comment. Undo removes them.
/// Every track with `MB_ORACLE_FULL=1` (`docs/scorecards/cue-auto.json`).
#[test]
fn auto_cue_analysis_matches_oracle_cue() {
    let Some(dir) = TempLibrary::new("oracle-a") else {
        eprintln!("skipped: fixtures/oracle-a missing");
        return;
    };
    let Some(want) = run_cues("A1") else {
        eprintln!("skipped: fixtures/oracle-cue/A1 missing");
        return;
    };
    let Some((_, all)) = tracks() else { return };
    let full = std::env::var_os("MB_ORACLE_FULL").is_some();
    let selected: Vec<&Track> = all
        .iter()
        .filter(|t| t.path.exists() && t.columns[0] > 0)
        .filter(|t| full || t.path.to_string_lossy().contains("/probes/wav_44k16/"))
        .collect();
    if selected.is_empty() {
        eprintln!("skipped: corpus audio missing");
        return;
    }
    let cue = mb_library::CueAnalysisSettings { enabled: true, manual: false, max_hot: 8, ..Default::default() };
    let settings = AnalysisSettings { phrase: true, embedding: false, cue, ..AnalysisSettings::default() };
    let mut lib = Library::open_unguarded(&dir.db, &dir.passphrase).unwrap();
    lib.set_analysis_root(dir.dir.join("share"));
    let mut history = History::default();
    let (mut with_cues, mut bad) = (0, Vec::new());
    for t in &selected {
        let a = analyse_track(&t.path, &t.file_name, &settings).unwrap();
        if let Some(e) = &a.cue_error {
            eprintln!("skipped: {e}");
            return;
        }
        // Test setup (not an op under test): no cues before, as in the fixture run.
        lib.connection().execute("DELETE FROM djmdCue WHERE ContentID = ?1", [&t.id]).unwrap();
        history.run(&mut lib, Command::ApplyAnalysis { content: DbId(t.id.clone()), analysis: Box::new(a) }).unwrap();
        let mut got: Vec<(i64, i64, String)> = mb_library::cues::load_for_content(lib.connection(), &t.id)
            .unwrap()
            .into_iter()
            .map(|c| (c.slot.kind(), i64::from(c.in_ms), c.comment))
            .collect();
        got.sort();
        let mut rb = want.get(&t.id).cloned().unwrap_or_default();
        rb.sort();
        with_cues += usize::from(!rb.is_empty());
        if got != rb {
            bad.push(format!("{}: ours {got:?} vs rekordbox {rb:?}", t.path.display()));
        }
        history.undo(&mut lib).unwrap().unwrap();
        assert!(mb_library::cues::load_for_content(lib.connection(), &t.id).unwrap().is_empty(), "undo leaves no cue");
    }
    assert!(with_cues > 0, "rekordbox set cues on none of the tracks");
    assert!(bad.is_empty(), "{} of {} tracks differ:\n{}", bad.len(), selected.len(), bad.join("\n"));
}

/// Normal-mode Dynamic (`trackAnalysisMode` 1) against `fixtures/oracle-e`: PQTZ, PQT2 (type 2),
/// PSSI and `BPM` equal rekordbox's. The 44.1 kHz WAV probes by default, every track with
/// `MB_ORACLE_FULL=1` (`docs/scorecards/beats-dynamic.json`).
#[test]
fn dynamic_mode_matches_oracle_e() {
    let Some((share, all)) = tracks_in("oracle-e") else {
        eprintln!("skipped: fixtures/oracle-e missing");
        return;
    };
    let full = std::env::var_os("MB_ORACLE_FULL").is_some();
    let selected: Vec<&Track> = all
        .iter()
        .filter(|t| t.path.exists())
        .filter(|t| load(&share, &t.rel, "EXT").is_some_and(|f| f.ext_beat_grid().is_some()))
        .filter(|t| full || t.path.to_string_lossy().contains("/probes/wav_44k16/"))
        .collect();
    if selected.is_empty() {
        eprintln!("skipped: corpus audio missing");
        return;
    }
    let settings = AnalysisSettings { phrase: true, dynamic: true, embedding: false, ..AnalysisSettings::default() };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let failures: Vec<String> = std::thread::scope(|s| {
        let chunks: Vec<Vec<&Track>> = (0..threads).map(|i| selected.iter().skip(i).step_by(threads).copied().collect()).collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let share = &share;
                let settings = &settings;
                s.spawn(move || {
                    let mut bad = Vec::new();
                    for t in chunk {
                        let a = analyse_track(&t.path, &t.file_name, settings).unwrap();
                        let name = t.path.display().to_string();
                        if a.bpm_x100 != Some(t.columns[0]) {
                            bad.push(format!("{name}: BPM {:?} vs {}", a.bpm_x100, t.columns[0]));
                        }
                        for (ext, ours, tag) in [("DAT", &a.dat, Tag::PQTZ), ("EXT", &a.ext, Tag::PQT2), ("EXT", &a.ext, Tag::PSSI)] {
                            let theirs = load(share, &t.rel, ext).unwrap();
                            if ours.sections_with(tag).next() != theirs.sections_with(tag).next() {
                                bad.push(format!("{name}: {tag} differs"));
                            }
                        }
                    }
                    bad
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    eprintln!("{} tracks, {} failures", selected.len(), failures.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
