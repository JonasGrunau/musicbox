//! CUE analysis (Manual) against rekordbox's (`fixtures/oracle-cue/<run>`, `docs/re/analysis-cue.md`).
//! Usage: `cargo run --release -p mb-library --example cue_score -- [run …]` (default: every run).
//!
//! Per track, our Manual pass runs on the grid rekordbox left in that run (its `.DAT`/`.EXT`) and
//! on the cues before it: none, or those of `<run>/seed/master.db` when the run was seeded. The
//! cue rows that would result are compared with the run's `djmdCue` rows column by column.

use mb_library::cue_analysis::{CueAnalysisSettings, manual_plan};
use mb_library::Cue;
use mb_masterdb::rusqlite::Connection;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The compared columns of one cue row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    kind: i64,
    in_msec: i64,
    in_frame: i64,
    out_msec: i64,
    comment: String,
    color: i64,
    color_table_index: i64,
    active_loop: i64,
    beat_loop_size: i64,
    cue_microsec: i64,
    in_mpeg_frame: i64,
    in_mpeg_abs: i64,
}

fn rows(conn: &Connection) -> anyhow::Result<HashMap<String, Vec<Row>>> {
    let mut stmt = conn.prepare(
        "SELECT ContentID, Kind, InMsec, InFrame, OutMsec, Comment, Color, ColorTableIndex, ActiveLoop, BeatLoopSize, CueMicrosec, \
         InMpegFrame, InMpegAbs FROM djmdCue WHERE rb_local_deleted = 0",
    )?;
    let mut out: HashMap<String, Vec<Row>> = HashMap::new();
    let all = stmt.query_map([], |r| {
        let i = |k: usize| r.get::<_, Option<i64>>(k).map(|v| v.unwrap_or(0));
        Ok((
            r.get::<_, String>(0)?,
            Row {
                kind: i(1)?,
                in_msec: i(2)?,
                in_frame: i(3)?,
                out_msec: i(4)?,
                comment: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                color: i(6)?,
                color_table_index: i(7)?,
                active_loop: i(8)?,
                beat_loop_size: i(9)?,
                cue_microsec: i(10)?,
                in_mpeg_frame: i(11)?,
                in_mpeg_abs: i(12)?,
            },
        ))
    })?;
    for r in all {
        let (id, row) = r?;
        out.entry(id).or_default().push(row);
    }
    Ok(out)
}

/// A private read-only copy of `<dir>/master.db` (with its WAL), so the fixture is never touched.
fn open_copy(dir: &Path, options: &Path, tag: &str) -> anyhow::Result<MasterDb> {
    let paths = LibraryPaths::from_options_json(options)?;
    let tmp = std::env::temp_dir().join(format!("mb-cue-score-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    for f in ["master.db", "master.db-wal", "master.db-shm"] {
        if dir.join(f).exists() {
            std::fs::copy(dir.join(f), tmp.join(f))?;
        }
    }
    Ok(MasterDb::open_read_only(&tmp.join("master.db"), &derive_passphrase(&paths.dp)?)?)
}

/// `name="X" val="Y"` lines (`cue-settings.txt`).
fn settings(path: &Path) -> anyhow::Result<CueAnalysisSettings> {
    let text = std::fs::read_to_string(path)?;
    let mut map = HashMap::new();
    for line in text.lines() {
        let field = |key: &str| line.split(&format!("{key}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::to_owned);
        if let (Some(n), Some(v)) = (field("name"), field("val")) {
            map.insert(n, v);
        }
    }
    Ok(CueAnalysisSettings::from_prefs(|n| map.get(n).cloned()))
}

/// The row `ApplyAnalysis` writes for a planned cue (`cues::insert_cue` + `addCueOption`).
fn planned_row(c: &mb_library::NewCue) -> Row {
    let with_options = c.color.is_some() || !c.comment.is_empty() || c.color_table_index.is_some_and(|i| i != 0);
    Row {
        kind: c.slot.kind(),
        in_msec: i64::from(c.in_ms),
        in_frame: mb_library::cues::ms_to_frame(c.in_ms),
        out_msec: -1,
        comment: c.comment.clone(),
        color: if with_options { i64::from(c.color.unwrap_or(0xFF)) } else { -1 },
        color_table_index: i64::from(c.color_table_index.unwrap_or(0)),
        active_loop: 0,
        beat_loop_size: 0,
        cue_microsec: 0,
        in_mpeg_frame: 0,
        in_mpeg_abs: 0,
    }
}

fn existing_row(c: &Cue, raw: &[Row]) -> Option<Row> {
    raw.iter().find(|r| r.in_msec == i64::from(c.in_ms) && r.kind == c.slot.kind()).cloned()
}

fn main() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-cue");
    let mut runs: Vec<String> = std::env::args().skip(1).collect();
    if runs.is_empty() {
        runs = std::fs::read_dir(&root)?.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        runs.sort();
    }
    for run in runs {
        let dir = root.join(&run);
        let options = dir.join("options.json");
        let s = settings(&dir.join("cue-settings.txt"))?;
        let db = open_copy(&dir.join("rekordbox"), &options, &run)?;
        let theirs = rows(db.connection())?;
        let seed: Option<PathBuf> = Some(dir.join("seed")).filter(|p| p.join("master.db").exists());
        let seed_db = seed.as_ref().map(|p| open_copy(p, &options, &format!("{run}-seed"))).transpose()?;
        let seed_rows = seed_db.as_ref().map(|d| rows(d.connection())).transpose()?.unwrap_or_default();
        let mut stmt = db.connection().prepare("SELECT ID, AnalysisDataPath, Title FROM djmdContent WHERE rb_local_deleted = 0 ORDER BY ID")?;
        let tracks: Vec<(String, Option<String>, String)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))?.collect::<Result<_, _>>()?;
        let share = dir.join("rekordbox/share");
        let (mut same, mut total, mut cues_theirs, mut cues_ours) = (0, 0, 0, 0);
        let mut kinds: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
        let mut misses = Vec::new();
        for (id, rel, title) in &tracks {
            let Some(rel) = rel.as_deref().filter(|r| !r.is_empty()) else { continue };
            let dat = share.join(rel.trim_start_matches('/'));
            let (Ok(d), Ok(e)) = (std::fs::read(&dat), std::fs::read(dat.with_extension("EXT"))) else { continue };
            let (d, e) = (mb_anlz::AnlzFile::parse(&d)?, mb_anlz::AnlzFile::parse(&e)?);
            let beats = d.beat_grid().map(|g| g.beats).unwrap_or_default();
            let offset = e.ext_beat_grid().and_then(|g| g.tail).map_or(0, |(o, _)| o);
            let before: Vec<Cue> = match &seed_db {
                Some(sd) => mb_library::cues::load_for_content(sd.connection(), id)?,
                None => Vec::new(),
            };
            let plan = manual_plan(&beats, offset, &before, &s);
            let empty = Vec::new();
            let seed_raw = seed_rows.get(id).unwrap_or(&empty);
            let mut ours: Vec<Row> = before.iter().filter(|c| !plan.remove.contains(&c.id)).filter_map(|c| existing_row(c, seed_raw)).collect();
            ours.extend(plan.create.iter().map(planned_row));
            ours.sort();
            let mut rb = theirs.get(id).cloned().unwrap_or_default();
            rb.sort();
            total += 1;
            cues_theirs += rb.len();
            cues_ours += ours.len();
            for r in &rb {
                kinds.entry(r.kind).or_default().0 += 1;
            }
            for r in &ours {
                kinds.entry(r.kind).or_default().1 += 1;
            }
            if ours == rb {
                same += 1;
            } else if misses.len() < 8 {
                let first = ours.iter().zip(&rb).position(|(a, b)| a != b).unwrap_or(ours.len().min(rb.len()));
                misses.push(format!(
                    "  {id} {title}: ours {} vs rekordbox {}; first difference at {first}: {:?} vs {:?}",
                    ours.len(),
                    rb.len(),
                    ours.get(first),
                    rb.get(first)
                ));
            }
        }
        println!("{run}: {same}/{total} tracks identical, cues ours {cues_ours} vs rekordbox {cues_theirs}{}", if seed.is_some() { " (seeded)" } else { "" });
        println!("  per Kind (rekordbox, ours): {kinds:?}");
        for m in &misses {
            println!("{m}");
        }
    }
    Ok(())
}
