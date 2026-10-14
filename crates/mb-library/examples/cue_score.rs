//! CUE analysis against rekordbox's (`fixtures/oracle-cue/<run>`, `docs/re/analysis-cue.md`).
//! Usage: `cargo run --release -p mb-library --example cue_score -- [run …]` (default: every run).
//!
//! Per track, our Manual or Auto pass runs on the grid rekordbox left in that run (its
//! `.DAT`/`.EXT`) and on the cues before it: none, or those of `<run>/seed/master.db` when the
//! run was seeded. Auto scores the phrase starts of the run's PSSI on our own decode of the
//! audio, with rekordbox's models from the install (`MB_CUE_OWN=1`: on our own grid and
//! phrases instead). The cue rows that would result are compared with the run's `djmdCue` rows
//! column by column. `MB_CUE_ONLY=<ContentID,…>` limits the tracks, `MB_CUE_DUMP=1` prints the
//! scores and the differing beats, `MB_CUE_INPUTS=<dir>` writes each track's grid and phrases as
//! text for comparison tools.

use mb_analysis::phrase::PhraseResult;
use mb_anlz::Tag;
use mb_anlz::tags::{Beat, SongStructure};
use mb_analysis::cue::{Scored, Trend};
use mb_library::cue_analysis::{CueAnalysisSettings, auto_plan, manual_plan};
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

/// Auto CUE's scores on rekordbox's grid and phrases (`mb_analysis::cue`); `None` without PSSI.
fn auto_scored(audio: &Path, beats: &[Beat], ext: &mb_anlz::AnlzFile) -> anyhow::Result<Option<Scored>> {
    let Some(pssi) = ext.sections_with(Tag::PSSI).next().and_then(|s| SongStructure::decode(s).ok()) else { return Ok(None) };
    let mut p = PhraseResult {
        count: pssi.phrases.len() as i32,
        mood: i32::from(pssi.mood),
        end_beat: i32::from(pssi.end_beat),
        ..PhraseResult::default()
    };
    p.cols[0] = pssi.phrases.iter().map(|x| i32::from(x.beat())).collect();
    p.cols[1] = pssi.phrases.iter().map(|x| i32::from(x.kind())).collect();
    let grid: Vec<mb_analysis::beats::Beat> =
        beats.iter().map(|b| mb_analysis::beats::Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo, time_ms: b.time_ms, sub_ms: 0 }).collect();
    // `MB_CUE_INPUTS=<dir>`: the grid and phrases as text, for comparison tools
    if let Some(dir) = std::env::var_os("MB_CUE_INPUTS") {
        let mut t = format!("mood {}\nbeats {}\n", p.mood, grid.len());
        t.extend(grid.iter().map(|b| format!("{}\n", b.time_ms)));
        t.push_str(&format!("phrases {}\n", p.count));
        t.extend(p.cols[0].iter().zip(&p.cols[1]).map(|(b, k)| format!("{b} {k}\n")));
        let name = audio.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        std::fs::write(Path::new(&dir).join(format!("{name}.txt")), t)?;
    }
    let (rate, pcm) = mb_analysis::cue::collect(audio)?;
    let total_samples = mb_audio::open(audio)?.info().total_samples;
    let models = mb_analysis::cue::Models::get().map_err(anyhow::Error::msg)?;
    let scores = models.scores(&pcm, rate, &grid, &p).map_err(anyhow::Error::msg)?;
    if std::env::var_os("MB_CUE_DUMP").is_some() {
        eprintln!("  {}: mood {} phrases {} scores {:?}", audio.display(), p.mood, p.count, scores);
    }
    let length = i32::from((total_samples.saturating_sub(1) / u64::from(rate.max(1))) as u16);
    Ok(Some(Scored { scores, phrases: p, beats: grid, length }))
}

fn to_anlz(v: &[mb_analysis::beats::Beat]) -> Vec<Beat> {
    v.iter().map(|b| Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo, time_ms: b.time_ms }).collect()
}

/// Auto CUE end to end: our grid, phrases and scores from our own analysis of the audio.
fn own_scored(audio: &Path) -> anyhow::Result<(Vec<Beat>, Option<Scored>)> {
    let opts = mb_analysis::Options {
        waveforms: false,
        loudness: false,
        key: false,
        auto_cue: true,
        ..mb_analysis::Options::default()
    };
    let a = mb_analysis::analyze(audio, opts)?;
    if let Some(e) = a.cue_error {
        anyhow::bail!(e);
    }
    Ok((to_anlz(&a.beats.map(|g| g.beats).unwrap_or_default()), a.cue))
}

fn main() -> anyhow::Result<()> {
    // `MB_CUE_OWN=1`: Auto runs on our own grid and phrases instead of the run's
    let own = std::env::var_os("MB_CUE_OWN").is_some();
    // Auto scores per track, shared by the Auto runs (they differ in the cue type and tendency)
    let mut auto_cache: HashMap<String, (Vec<Beat>, Option<Scored>)> = HashMap::new();
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
        // the run's personal CUE tendency (all zeros when rekordbox wrote the default)
        let trend = match std::fs::read_to_string(dir.join("cue_personal_trend.json")) {
            Ok(t) => Some(Trend::parse(&t).map_err(anyhow::Error::msg)?),
            Err(_) => None,
        };
        let db = open_copy(&dir.join("rekordbox"), &options, &run)?;
        let theirs = rows(db.connection())?;
        let seed: Option<PathBuf> = Some(dir.join("seed")).filter(|p| p.join("master.db").exists());
        let seed_db = seed.as_ref().map(|p| open_copy(p, &options, &format!("{run}-seed"))).transpose()?;
        let seed_rows = seed_db.as_ref().map(|d| rows(d.connection())).transpose()?.unwrap_or_default();
        let mut stmt = db.connection().prepare("SELECT ID, AnalysisDataPath, Title, FolderPath FROM djmdContent WHERE rb_local_deleted = 0 ORDER BY ID")?;
        let tracks: Vec<(String, Option<String>, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?.unwrap_or_default())))?
            .collect::<Result<_, _>>()?;
        let share = dir.join("rekordbox/share");
        let (mut same, mut total, mut cues_theirs, mut cues_ours) = (0, 0, 0, 0);
        let mut kinds: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
        let mut misses = Vec::new();
        // `MB_CUE_ONLY=<ContentID,…>`: only those tracks
        let only: Option<Vec<String>> = std::env::var("MB_CUE_ONLY").ok().map(|v| v.split(',').map(str::to_owned).collect());
        for (id, rel, title, audio) in &tracks {
            if only.as_ref().is_some_and(|o| !o.contains(id)) {
                continue;
            }
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
            let plan = if s.manual {
                manual_plan(&beats, offset, &before, &s)
            } else {
                if !auto_cache.contains_key(id) {
                    let c = if own { own_scored(Path::new(audio)) } else { auto_scored(Path::new(audio), &beats, &e).map(|c| (beats.clone(), c)) };
                    let c = c.unwrap_or_else(|err| {
                        eprintln!("  {id} {title}: {err}");
                        (beats.clone(), None)
                    });
                    auto_cache.insert(id.clone(), c);
                }
                let (grid, scored) = &auto_cache[id];
                let cands = scored.as_ref().map(|c| to_anlz(&c.candidates(trend.as_ref()))).unwrap_or_default();
                // our analysis stores PQT2's beat number offset as 0
                auto_plan(grid, if own { 0 } else { offset }, &cands, &before, &s)
            };
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
            if ours != rb && std::env::var_os("MB_CUE_DUMP").is_some() {
                let idx = |ms: i64| beats.iter().position(|b| i64::from(b.time_ms) == ms).map_or(-1, |i| i as i64);
                let ours_ms: Vec<(i64, i64)> = ours.iter().map(|r| (r.in_msec, idx(r.in_msec))).collect();
                let rb_ms: Vec<(i64, i64)> = rb.iter().map(|r| (r.in_msec, idx(r.in_msec))).collect();
                eprintln!("  {id}: ours (ms, beat) {ours_ms:?}\n  {id}: rekordbox     {rb_ms:?}");
            }
            if ours == rb {
                same += 1;
            } else if misses.len() < 8 {
                let first = ours.iter().zip(&rb).position(|(a, b)| a != b).unwrap_or(ours.len().min(rb.len()));
                misses.push(format!(
                    "  {id} {title} ({}): ours {} vs rekordbox {}; first difference at {first}: {:?} vs {:?}",
                    Path::new(audio).parent().and_then(Path::file_name).map_or_else(String::new, |d| d.to_string_lossy().into_owned()),
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
