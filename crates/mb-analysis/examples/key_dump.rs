//! Development aid for the key port: writes rekordbox's key-job inputs of every oracle track
//! (our key buffer, rekordbox's own PQTZ beats and BPM) to `<out>/<n>.bin`, plus `index.tsv`
//! with the stored key name. Usage:
//!   cargo run --release -p mb-analysis --example key_dump -- <out-dir> [filter]
//! (`MB_ORACLE=oracle-b` for the second fixture.)
//!
//! File layout (little endian): `MBKY`, u32 rate, u64 len (L − 1), f32 bpm, u32 beats,
//! beats × {u16 beatInBar, u16 tempo, u32 ms, u16 sub, u16 0}, u32 K, K × f32.

#[path = "../tests/common/mod.rs"]
mod common;

use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

fn keys(fixture: &str) -> HashMap<PathBuf, String> {
    let root = common::fixtures().join(fixture);
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    let pass = derive_passphrase(&paths.dp).unwrap();
    let tmp = std::env::temp_dir().join(format!("mb-key-dump-{}", std::process::id()));
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
            "SELECT c.FolderPath, k.ScaleName FROM djmdContent c LEFT JOIN djmdKey k ON k.ID = c.KeyID \
             WHERE c.rb_local_deleted = 0",
        )
        .unwrap();
    let map = stmt
        .query_map([], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(0)?),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().expect("out dir"));
    let filter = args.get(1).cloned();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    let tracks = common::oracle_tracks(&fixture).expect("fixture");
    let names = keys(&fixture);
    std::fs::create_dir_all(&out).unwrap();
    let mut index = std::fs::File::create(out.join("index.tsv")).unwrap();
    for (i, t) in tracks.iter().enumerate() {
        if filter
            .as_ref()
            .is_some_and(|f| !t.path.to_string_lossy().contains(f.as_str()))
        {
            continue;
        }
        let Some(grid) = t.dat.as_ref().and_then(|d| d.beat_grid()) else {
            continue;
        };
        let Ok((col, total)) = mb_analysis::key::collect(&t.path) else {
            eprintln!("{}: decode failed", t.path.display());
            continue;
        };
        let bpm = t.bpm.unwrap_or(0) as i32;
        let mut f = std::fs::File::create(out.join(format!("{i}.bin"))).unwrap();
        let mut b = Vec::new();
        b.extend_from_slice(b"MBKY");
        b.extend_from_slice(&col.rate().to_le_bytes());
        b.extend_from_slice(&total.to_le_bytes());
        b.extend_from_slice(&((bpm as f32) * 0.01f32).to_le_bytes());
        b.extend_from_slice(&(grid.beats.len() as u32).to_le_bytes());
        for beat in &grid.beats {
            b.extend_from_slice(&beat.beat_in_bar.to_le_bytes());
            b.extend_from_slice(&beat.tempo.to_le_bytes());
            b.extend_from_slice(&beat.time_ms.to_le_bytes());
            b.extend_from_slice(&[0u8; 4]);
        }
        let buf = col.buffer();
        b.extend_from_slice(&(buf.len() as u32).to_le_bytes());
        for x in buf {
            b.extend_from_slice(&x.to_le_bytes());
        }
        f.write_all(&b).unwrap();
        let key = names.get(&t.path).cloned().unwrap_or_default();
        writeln!(index, "{i}\t{key}\t{}", short(&t.path)).unwrap();
        eprintln!("{i} {key} {}", short(&t.path));
    }
}

fn short(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.split("/corpus/").last().unwrap_or(&s).to_string()
}
