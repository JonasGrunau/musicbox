//! Oracle helpers shared by the tests and examples: rekordbox's own analysis of the corpus in
//! `fixtures/oracle-a` (master.db read through a temp copy, ANLZ files under `rekordbox/share`).
//! Everything returns `None` when the gitignored fixtures or the corpus are absent.
#![allow(dead_code)]

use mb_anlz::AnlzFile;
use mb_anlz::tags::{BeatGrid, SeekTable, Vbr, Wave3Band, WaveColorScroll, WaveScroll};
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::{Path, PathBuf};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn corpus() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join("re-work/corpus");
    dir.join("probes/probes.json").exists().then_some(dir)
}

/// One djmdContent row of the oracle library plus its local ANLZ files.
pub struct OracleTrack {
    pub id: String,
    pub path: PathBuf,
    pub file_type: i64,
    pub sample_rate: i64,
    pub bit_depth: i64,
    pub bit_rate: i64,
    /// Seconds, as rekordbox stores it.
    pub length: i64,
    pub dat: Option<AnlzFile>,
    pub ext: Option<AnlzFile>,
    pub ex2: Option<AnlzFile>,
}

impl OracleTrack {
    pub fn vbr(&self) -> Option<Vbr> {
        let s = self.dat.as_ref()?.sections_with(mb_anlz::Tag::PVBR).next()?;
        Vbr::decode(s).ok()
    }
    pub fn beat_grid(&self) -> Option<BeatGrid> {
        self.dat.as_ref()?.beat_grid()
    }
    pub fn wave_scroll(&self) -> Option<WaveScroll> {
        let s = self.ext.as_ref()?.sections_with(mb_anlz::Tag::PWV3).next()?;
        WaveScroll::decode(s).ok()
    }
    pub fn color_scroll(&self) -> Option<WaveColorScroll> {
        let s = self.ext.as_ref()?.sections_with(mb_anlz::Tag::PWV5).next()?;
        WaveColorScroll::decode(s).ok()
    }
    pub fn band_scroll(&self) -> Option<Wave3Band> {
        let s = self.ex2.as_ref()?.sections_with(mb_anlz::Tag::PWV7).next()?;
        Wave3Band::decode(s).ok()
    }
    pub fn seek_table(&self) -> Option<SeekTable> {
        let s = self.ext.as_ref()?.sections_with(mb_anlz::Tag::PVB2).next()?;
        SeekTable::decode(s).ok()
    }
    /// `<format dir>/<file name>` relative to the corpus, e.g. `probes/mp3_cbr320/x.mp3`.
    pub fn corpus_rel(&self) -> Option<String> {
        let s = self.path.to_str()?;
        s.split_once("/re-work/corpus/").map(|(_, r)| r.to_string())
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
    let tmp = std::env::temp_dir().join(format!("mb-audio-oracle-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    let db_path = tmp.join("master.db");
    std::fs::copy(&db_src, &db_path).ok()?;
    let db = MasterDb::open_read_only(&db_path, &passphrase).ok()?;
    let mut stmt = db
        .connection()
        .prepare(
            "SELECT ID, FolderPath, FileType, SampleRate, BitDepth, BitRate, Length, AnalysisDataPath \
             FROM djmdContent WHERE rb_local_deleted = 0 ORDER BY FolderPath",
        )
        .ok()?;
    let share = root.join("rekordbox/share");
    let rows: Vec<OracleTrack> = stmt
        .query_map([], |r| {
            let anlz: Option<String> = r.get(7)?;
            let load = |ext: &str| {
                let rel = anlz.as_deref()?.trim_start_matches('/').replace(".DAT", ext);
                AnlzFile::parse(&std::fs::read(share.join(rel)).ok()?).ok()
            };
            Ok(OracleTrack {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                file_type: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                sample_rate: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                bit_depth: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                bit_rate: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                length: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
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

/// `probes.json`: name → first beat in seconds (None for non-rhythmic probes).
pub fn probe_first_beats() -> Option<Vec<(String, Option<f64>)>> {
    let text = std::fs::read_to_string(corpus()?.join("probes/probes.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(
        v.as_array()?
            .iter()
            .filter_map(|p| Some((p["name"].as_str()?.to_string(), p["first_beat_seconds"].as_f64())))
            .collect(),
    )
}
