//! Scores the key port against the key rekordbox stored (`djmdContent.KeyID` → `djmdKey`).
//! Two inputs per track: rekordbox's own grid (PQTZ and `djmdContent.BPM`) with our key buffer,
//! and our whole analysis pass (our grid, phrase shift, key). Usage:
//!   cargo run --release -p mb-analysis --example key_score -- [--verbose] [filter]
//! (`MB_ORACLE=oracle-b` for the second fixture, `MB_HP=dynamic` for a high-precision own grid
//! with the stored grid as fallback, `MB_THREADS` to limit threads.)

#[path = "../tests/common/mod.rs"]
mod common;

use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Stored key names by file path, read from a scratch copy of the fixture's `master.db`.
fn stored_keys(fixture: &str) -> HashMap<PathBuf, String> {
    let root = common::fixtures().join(fixture);
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    let pass = derive_passphrase(&paths.dp).unwrap();
    let tmp = std::env::temp_dir().join(format!("mb-key-score-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    for f in ["master.db", "master.db-wal", "master.db-shm"] {
        let src = root.join("rekordbox").join(f);
        if src.exists() {
            std::fs::copy(&src, tmp.join(f)).unwrap();
        }
    }
    let db = MasterDb::open_read_only(&tmp.join("master.db"), &pass).unwrap();
    let mut stmt = db
        .connection()
        .prepare(
            "SELECT c.FolderPath, k.ScaleName FROM djmdContent c JOIN djmdKey k ON k.ID = c.KeyID \
             WHERE c.rb_local_deleted = 0",
        )
        .unwrap();
    let map = stmt
        .query_map([], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(0)?),
                r.get::<_, String>(1)?,
            ))
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    drop(stmt);
    drop(db);
    let _ = std::fs::remove_dir_all(&tmp);
    map
}

struct Row {
    name: String,
    stored: String,
    /// With rekordbox's grid.
    fixture: Option<u8>,
    /// Our analysis pass.
    own: Option<u8>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let filter = args.iter().find(|a| !a.starts_with("--")).cloned();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    let keys = stored_keys(&fixture);
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| {
            keys.contains_key(&t.path)
                && filter
                    .as_ref()
                    .is_none_or(|f| t.path.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    let hp = std::env::var("MB_HP")
        .is_ok_and(|v| v == "dynamic")
        .then_some(mb_analysis::hpbeat::HpMode::Dynamic);
    let next = Mutex::new(0usize);
    let rows = Mutex::new(Vec::new());
    let threads = std::env::var("MB_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let Some(t) = tracks.get(i) else { break };
                    let fixture_key = t.dat.as_ref().and_then(|d| d.beat_grid()).and_then(|g| {
                        let (col, total) = mb_analysis::key::collect(&t.path).ok()?;
                        let beats: Vec<(u16, u32)> =
                            g.beats.iter().map(|b| (b.beat_in_bar, b.time_ms)).collect();
                        let bpm = t.bpm.unwrap_or(0) as i32 as f32 * 0.01;
                        Some(
                            mb_analysis::key::detect(col.buffer(), col.rate(), total, bpm, &beats)
                                as u8,
                        )
                    });
                    let opts = mb_analysis::Options {
                        waveforms: false,
                        loudness: false,
                        beats: true,
                        phrase: true,
                        key: true,
                        high_precision: hp,
                        vocal: false,
                        ..Default::default()
                    };
                    // With the stored grid as the fallback where high precision finds no beats.
                    let own = mb_analysis::analyze_with(&t.path, opts, t.stored_grid().as_ref())
                        .ok()
                        .and_then(|a| a.key);
                    let full = t.path.to_string_lossy();
                    let row = Row {
                        name: full.split("/corpus/").last().unwrap_or(&full).to_string(),
                        stored: keys[&t.path].clone(),
                        fixture: fixture_key,
                        own,
                    };
                    rows.lock().unwrap().push((i, row));
                }
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let name = |k: Option<u8>| k.map_or("-", mb_analysis::key::scale_name);
    let (mut n_fix, mut ok_fix, mut ok_own) = (0, 0, 0);
    for (_, r) in &rows {
        let f_ok = name(r.fixture) == r.stored;
        let o_ok = name(r.own) == r.stored;
        n_fix += usize::from(r.fixture.is_some());
        ok_fix += usize::from(f_ok);
        ok_own += usize::from(o_ok);
        if verbose || !f_ok || !o_ok {
            println!(
                "{}: stored {} | rekordbox grid {} | own grid {}",
                r.name,
                r.stored,
                name(r.fixture),
                name(r.own)
            );
        }
    }
    println!(
        "{} tracks with a stored key: rekordbox grid {ok_fix}/{n_fix}, own analysis {ok_own}/{}",
        rows.len(),
        rows.len()
    );
}
