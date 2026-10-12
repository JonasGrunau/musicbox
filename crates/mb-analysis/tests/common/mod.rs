//! rekordbox's own analysis of the corpus (`fixtures/oracle-a`): the tracks of the fixture
//! master.db (read through a temp copy) and their local ANLZ files. Everything returns `None`
//! when the gitignored fixtures or the corpus are absent.
#![allow(dead_code)]

use mb_anlz::{AnlzFile, Tag};
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::{Path, PathBuf};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub struct OracleTrack {
    pub id: String,
    pub path: PathBuf,
    /// `djmdMixerParam` (GainHigh, GainLow, PeakHigh, PeakLow).
    pub mixer: Option<[i64; 4]>,
    /// `djmdContent.BPM` (BPM × 100).
    pub bpm: Option<i64>,
    pub dat: Option<AnlzFile>,
    pub ext: Option<AnlzFile>,
    pub ex2: Option<AnlzFile>,
}

impl OracleTrack {
    /// Body and header of the first section with `tag` in any of the three files.
    pub fn section(&self, tag: Tag) -> Option<(&[u8], &[u8])> {
        [&self.dat, &self.ext, &self.ex2]
            .into_iter()
            .flatten()
            .find_map(|f| f.sections_with(tag).next())
            .map(|s| (s.header.as_slice(), s.body.as_slice()))
    }
}

/// All tracks of `fixtures/<name>` whose audio file exists.
pub fn oracle_tracks(name: &str) -> Option<Vec<OracleTrack>> {
    let root = fixtures().join(name);
    let db_src = root.join("rekordbox/master.db");
    if !db_src.exists() {
        return None;
    }
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).ok()?;
    let passphrase = derive_passphrase(&paths.dp).ok()?;
    // Read a private copy so the fixture's WAL/SHM files are never touched.
    let tmp =
        std::env::temp_dir().join(format!("mb-analysis-oracle-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    let db_path = tmp.join("master.db");
    std::fs::copy(&db_src, &db_path).ok()?;
    for side in ["master.db-wal", "master.db-shm"] {
        if root.join("rekordbox").join(side).exists() {
            std::fs::copy(root.join("rekordbox").join(side), tmp.join(side)).ok()?;
        }
    }
    let db = MasterDb::open_read_only(&db_path, &passphrase).ok()?;
    let mut stmt = db
        .connection()
        .prepare(
            "SELECT c.ID, c.FolderPath, c.AnalysisDataPath, m.GainHigh, m.GainLow, m.PeakHigh, m.PeakLow, c.BPM \
             FROM djmdContent c LEFT JOIN djmdMixerParam m ON m.ContentID = c.ID AND m.rb_local_deleted = 0 \
             WHERE c.rb_local_deleted = 0 ORDER BY c.FolderPath",
        )
        .ok()?;
    let share = root.join("rekordbox/share");
    let rows: Vec<OracleTrack> = stmt
        .query_map([], |r| {
            let anlz: Option<String> = r.get(2)?;
            let load = |ext: &str| {
                let rel = anlz
                    .as_deref()?
                    .trim_start_matches('/')
                    .replace(".DAT", ext);
                AnlzFile::parse(&std::fs::read(share.join(rel)).ok()?).ok()
            };
            let m: [Option<i64>; 4] = [r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?];
            Ok(OracleTrack {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                mixer: m
                    .iter()
                    .all(Option::is_some)
                    .then(|| m.map(|v| v.unwrap_or(0))),
                bpm: r.get(7)?,
                dat: load(".DAT"),
                ext: load(".EXT"),
                ex2: load(".2EX"),
            })
        })
        .ok()?
        .filter_map(Result::ok)
        .filter(|t| t.path.exists())
        .collect();
    drop(stmt);
    drop(db);
    let _ = std::fs::remove_dir_all(&tmp);
    Some(rows)
}
