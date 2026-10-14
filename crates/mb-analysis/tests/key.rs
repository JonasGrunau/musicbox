//! Key detection (`docs/re/analysis-key.md`): synthetic checks, and the key rekordbox stored in
//! `fixtures/oracle-a` for the sampler loops (fast in debug builds; `MB_ORACLE_FULL=1` for all
//! 169 tracks), both from rekordbox's grid and from our own analysis pass. The oracle test is
//! skipped when the fixtures or the corpus are absent. `mb-library`'s `analyse_ops` covers the
//! probes.

mod common;

use mb_analysis::key;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::collections::HashMap;
use std::path::PathBuf;

/// A key buffer (rate / 8) holding the given pitches (Hz) over several octaves.
fn chord(rate: u32, secs: f32, freqs: &[f32]) -> Vec<f32> {
    let fs = rate as f32 / 8.0;
    let n = (secs * fs) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / fs;
            freqs
                .iter()
                .flat_map(|&f| [f / 2.0, f, f * 2.0])
                .map(|f| (std::f32::consts::TAU * f * t).sin() * 2000.0)
                .sum()
        })
        .collect()
}

#[test]
fn triads_give_their_key() {
    let rate = 44100;
    // C major (C E G) and A minor (A C E), no grid: the whole-track estimate decides.
    let c = chord(rate, 60.0, &[261.63, 329.63, 392.0]);
    let len = c.len() as u64 * 8;
    assert_eq!(
        key::scale_name(key::detect(&c, rate, len, 0.0, &[]) as u8),
        "C"
    );
    let am = chord(rate, 60.0, &[220.0, 261.63, 329.63]);
    assert_eq!(
        key::scale_name(key::detect(&am, rate, len, 0.0, &[]) as u8),
        "Am"
    );
}

#[test]
fn few_segments_keep_the_whole_track_key() {
    // Fewer than nine segments: the judgement keeps `key1`.
    let keys = [(5, 17); 8];
    assert_eq!(key::judge::judge(9, 21, true, &keys, 120.0), 9);
    // Segments off: the same.
    let keys = [(5, 17); 40];
    assert_eq!(key::judge::judge(9, 21, false, &keys, 120.0), 9);
}

#[test]
fn qsort3_breaks_ties_by_index() {
    let mut v = [1.0, 3.0, 3.0, 2.0, 3.0];
    let mut idx = [0, 1, 2, 3, 4];
    key::scale::qsort3(&mut v, &mut idx);
    assert_eq!(idx, [1, 2, 4, 3, 0]);
}

/// Stored key names by file path (`djmdContent.KeyID` → `djmdKey.ScaleName`).
fn stored_keys(fixture: &str) -> Option<HashMap<PathBuf, String>> {
    let root = common::fixtures().join(fixture);
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).ok()?;
    let pass = derive_passphrase(&paths.dp).ok()?;
    let tmp = std::env::temp_dir().join(format!("mb-analysis-key-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    for f in ["master.db", "master.db-wal", "master.db-shm"] {
        let src = root.join("rekordbox").join(f);
        if src.exists() {
            std::fs::copy(&src, tmp.join(f)).ok()?;
        }
    }
    let db = MasterDb::open_read_only(&tmp.join("master.db"), &pass).ok()?;
    let mut stmt = db
        .connection()
        .prepare(
            "SELECT c.FolderPath, k.ScaleName FROM djmdContent c JOIN djmdKey k ON k.ID = c.KeyID \
             WHERE c.rb_local_deleted = 0",
        )
        .ok()?;
    let map = stmt
        .query_map([], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(0)?),
                r.get::<_, String>(1)?,
            ))
        })
        .ok()?
        .filter_map(Result::ok)
        .collect();
    drop(stmt);
    drop(db);
    let _ = std::fs::remove_dir_all(&tmp);
    Some(map)
}

#[test]
fn oracle_keys() {
    let Some(tracks) = common::oracle_tracks("oracle-a") else {
        eprintln!("fixtures/oracle-a missing; skipped");
        return;
    };
    let Some(stored) = stored_keys("oracle-a") else {
        eprintln!("fixture master.db unreadable; skipped");
        return;
    };
    let full = std::env::var_os("MB_ORACLE_FULL").is_some();
    let mut n = 0;
    for t in &tracks {
        let p = t.path.to_string_lossy();
        if !(full || p.contains("/Sampler/")) {
            continue;
        }
        let Some(want) = stored.get(&t.path) else {
            continue;
        };
        // rekordbox's grid with our key buffer.
        if let Some(g) = t.dat.as_ref().and_then(|d| d.beat_grid()) {
            let (col, total) = key::collect(&t.path).expect("decode");
            let beats: Vec<(u16, u32)> =
                g.beats.iter().map(|b| (b.beat_in_bar, b.time_ms)).collect();
            let bpm = t.bpm.unwrap_or(0) as i32 as f32 * 0.01;
            let k = key::detect(col.buffer(), col.rate(), total, bpm, &beats) as u8;
            assert_eq!(key::scale_name(k), want, "{p} (rekordbox grid)");
        }
        // Our own pass.
        let opts = mb_analysis::Options {
            waveforms: false,
            loudness: false,
            beats: true,
            phrase: true,
            key: true,
            high_precision: None,
            vocal: false,
        };
        let a = mb_analysis::analyze(&t.path, opts).expect("analyze");
        assert_eq!(
            a.key.map(key::scale_name),
            Some(want.as_str()),
            "{p} (own analysis)"
        );
        n += 1;
    }
    assert!(n > 0, "no oracle tracks with a stored key");
}
