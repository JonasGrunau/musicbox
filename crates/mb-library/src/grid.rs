//! Op: replace the beat grid of a collection track, plus the grid edits that produce the new grid.
//! Spec: `docs/re/ops/edit-grid.md` (binary-derived; no traced grid edit exists yet).
//!
//! rekordbox saves an edited grid (`djplay::BeatGridAdjustment::saveBeatPosition` @0x1010bfdc0)
//! in this order:
//! 1. if the first beat's tempo differs from `djmdContent.BPM`, the BPM column is set to it
//!    (`DatabaseIF::modifyTrackData`, field 0x1a);
//! 2. `DatabaseMediator::setBeatInfo` @0x100c5b398 → `CAnalyzerIF::setBeatInfoWithExtBeat`
//!    @0x10162d5e4 rewrites the local `.DAT` (PQTZ, adjust offset 0) and `.EXT` (PQT2);
//! 3. 500 ms later: `updateAnalysisUpdated` (`AnalysisUpdated += 1`) and the playlist timestamps
//!    (`setBeatInfo` lambda $_10 @0x100c612cc). The cloud upload request is not reimplemented.
//!
//! PQT2 rule (`setBeatInfoWithExtBeat`): when the EXT has a PQT2, it is kept only if the new grid
//! equals the old one beat for beat; otherwise it is emptied (`set_pmai_ext_ext_beat_position`
//! with 0 beats: first/last beat, count and checksum 0, no entries). An EXT without PQT2 is left
//! without one.

use crate::{Command, DbId, Effect, Error, Library, Result, xml_sync};
use mb_anlz::tags::{Beat, BeatGrid, ExtBeatGrid};
use mb_anlz::{AnlzFile, Section, Tag};
use mb_masterdb::CounterColumn;
use mb_masterdb::rusqlite::OptionalExtension;
use mb_masterdb::rusqlite::types::Value;
use std::path::{Path, PathBuf};

/// Lowest and highest tempo a grid edit may produce (BPM × 100).
// hypothesis: `BeatGridAdjustment::isBpmNotOutOfRange` @0x1010bdf84 is not decoded; this range
// covers every preset analysis range (70–180 … 128–255) with room for ×2/÷2.
pub const MIN_TEMPO: u32 = 2_000;
pub const MAX_TEMPO: u32 = 40_000;

fn interval_ms(tempo: u16) -> f64 {
    6_000_000.0 / f64::from(tempo.max(1))
}

fn next_in_bar(b: u16, step: i64) -> u16 {
    ((i64::from(b.clamp(1, 4)) - 1 + step).rem_euclid(4) + 1) as u16
}

/// Index of the beat closest to `ms`.
pub fn nearest_beat(beats: &[Beat], ms: f64) -> Option<usize> {
    (0..beats.len()).min_by(|&a, &b| {
        let da = (f64::from(beats[a].time_ms) - ms).abs();
        let db = (f64::from(beats[b].time_ms) - ms).abs();
        da.total_cmp(&db)
    })
}

/// Adds beats before the first and after the last one, at the edge beats' own tempo, so the grid
/// covers `0 ..= duration_ms`.
fn extend(mut beats: Vec<Beat>, duration_ms: u32) -> Vec<Beat> {
    let Some(&first) = beats.first() else { return beats };
    let mut front = Vec::new();
    let iv = interval_ms(first.tempo);
    let mut k = 1i64;
    loop {
        let t = (f64::from(first.time_ms) - iv * k as f64).round();
        if t < 0.0 {
            break;
        }
        front.push(Beat { beat_in_bar: next_in_bar(first.beat_in_bar, -k), tempo: first.tempo, time_ms: t as u32 });
        k += 1;
    }
    front.reverse();
    let last = *beats.last().unwrap();
    let iv = interval_ms(last.tempo);
    let mut k = 1i64;
    loop {
        let t = (f64::from(last.time_ms) + iv * k as f64).round();
        if t > f64::from(duration_ms) {
            break;
        }
        beats.push(Beat { beat_in_bar: next_in_bar(last.beat_in_bar, k), tempo: last.tempo, time_ms: t as u32 });
        k += 1;
    }
    front.extend(beats);
    front
}

/// Moves every beat by `delta_ms`; beats leaving `0 ..= duration_ms` are dropped and the gap is
/// refilled at the edge tempo (`moveBeatToLeft` / `moveBeatToRight`).
// hypothesis: rekordbox's `moveBeatPos` @0x1010ba2bc drops beats outside the track the same way;
// the refill step (`expandBeatPos`) is not decoded.
pub fn shift(beats: &[Beat], delta_ms: i32, duration_ms: u32) -> Vec<Beat> {
    let moved: Vec<Beat> = beats
        .iter()
        .filter_map(|b| {
            let t = i64::from(b.time_ms) + i64::from(delta_ms);
            (0..=i64::from(duration_ms)).contains(&t).then_some(Beat { time_ms: t as u32, ..*b })
        })
        .collect();
    extend(moved, duration_ms)
}

/// Track length for grid edits: the `.EXT` scroll wave's length (PWV3, 150 columns per second)
/// when present, else `length_seconds` × 1000. `Length` holds whole seconds, so a grid edit
/// bounded by it would drop beats in the last second that rekordbox keeps (verified against
/// rekordbox's own "move grid right", `fixtures/oracle-grid-edit`).
pub fn duration_ms(ext: Option<&AnlzFile>, length_seconds: u32) -> u32 {
    let tag = length_seconds.saturating_mul(1000);
    let wave = ext
        .and_then(|f| f.sections_with(Tag::PWV3).next())
        .and_then(|s| mb_anlz::tags::WaveScroll::decode(s).ok())
        .map(|w| (w.data.len() as u64 * 1000 / u64::from(w.columns_per_second.max(1))) as u32)
        .unwrap_or(0);
    tag.max(wave)
}

/// Makes beat `index` a downbeat (1) and renumbers every other beat around it.
pub fn set_downbeat(beats: &[Beat], index: usize) -> Vec<Beat> {
    beats
        .iter()
        .enumerate()
        .map(|(i, b)| Beat { beat_in_bar: next_in_bar(1, i as i64 - index as i64), ..*b })
        .collect()
}

/// A constant grid at `tempo` (BPM × 100) through beat `anchor`, covering the whole track; the
/// anchor keeps its time and bar position.
pub fn set_tempo(beats: &[Beat], anchor: usize, tempo: u16, duration_ms: u32) -> Result<Vec<Beat>> {
    check_tempo(u32::from(tempo))?;
    let a = *beats.get(anchor).ok_or_else(|| Error::Invalid("no beat to anchor the grid".into()))?;
    Ok(extend(vec![Beat { tempo, ..a }], duration_ms))
}

fn check_tempo(tempo: u32) -> Result<()> {
    if (MIN_TEMPO..=MAX_TEMPO).contains(&tempo) {
        Ok(())
    } else {
        Err(Error::Invalid(format!("{:.2} BPM is outside {:.0}–{:.0} BPM", tempo as f64 / 100.0, MIN_TEMPO / 100, MAX_TEMPO / 100)))
    }
}

/// Doubles the tempo: a beat between every two beats (and after the last one when it fits).
// hypothesis: mirrors `BeatGridAdjustment::doubleBpm` @0x1010bbbdc (midpoints, tempo × 2); the
// numbering continues from the first beat.
pub fn double(beats: &[Beat], duration_ms: u32) -> Result<Vec<Beat>> {
    let Some(first) = beats.first() else { return Ok(Vec::new()) };
    for b in beats {
        check_tempo(u32::from(b.tempo) * 2)?;
    }
    let mut out = Vec::with_capacity(beats.len() * 2);
    for (i, b) in beats.iter().enumerate() {
        let tempo = b.tempo * 2;
        out.push(Beat { tempo, ..*b });
        let mid = match beats.get(i + 1) {
            Some(n) => (u64::from(b.time_ms) + u64::from(n.time_ms)).div_ceil(2) as u32,
            None => (f64::from(b.time_ms) + interval_ms(b.tempo) / 2.0).round() as u32,
        };
        if beats.get(i + 1).is_some() || mid <= duration_ms {
            out.push(Beat { tempo, time_ms: mid, beat_in_bar: 0 });
        }
    }
    Ok(renumber_from(out, first.beat_in_bar))
}

/// Halves the tempo: keeps every second beat, counted from the first downbeat.
// hypothesis: mirrors `BeatGridAdjustment::halveBpm` @0x1010bc410; kept beats are renumbered so
// the first downbeat stays a downbeat.
pub fn halve(beats: &[Beat]) -> Result<Vec<Beat>> {
    for b in beats {
        check_tempo(u32::from(b.tempo) / 2)?;
    }
    let down = beats.iter().position(|b| b.beat_in_bar == 1).unwrap_or(0);
    let kept: Vec<Beat> = beats
        .iter()
        .enumerate()
        .filter(|(i, _)| (*i as i64 - down as i64).rem_euclid(2) == 0)
        .map(|(_, b)| Beat { tempo: b.tempo / 2, ..*b })
        .collect();
    // The old downbeat is the (down / 2)-th kept beat.
    let anchor = (down / 2) as i64;
    Ok(kept.iter().enumerate().map(|(i, b)| Beat { beat_in_bar: next_in_bar(1, i as i64 - anchor), ..*b }).collect())
}

fn renumber_from(beats: Vec<Beat>, first: u16) -> Vec<Beat> {
    beats.into_iter().enumerate().map(|(i, b)| Beat { beat_in_bar: next_in_bar(first, i as i64), ..b }).collect()
}

/// The local analysis files of a track: `.DAT` and `.EXT` next to it.
struct AnlzPaths {
    rel: String,
    dat: PathBuf,
    ext: PathBuf,
}

fn anlz_paths(lib: &Library, content: &DbId) -> Result<(AnlzPaths, Option<i64>)> {
    let row: Option<(Option<String>, Option<i64>)> = lib
        .connection()
        .query_row("SELECT AnalysisDataPath, BPM FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [content.as_str()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let (rel, bpm) = row.ok_or_else(|| Error::NotFound { what: "track", id: content.0.clone() })?;
    let rel = rel.filter(|p| !p.is_empty()).ok_or_else(|| Error::Invalid("track is not analysed".into()))?;
    let root = lib.analysis_root.as_ref().ok_or_else(|| Error::Invalid("analysis folder unknown".into()))?;
    let dat = root.join(rel.trim_start_matches('/'));
    let ext = dat.with_extension("EXT");
    Ok((AnlzPaths { rel, dat, ext }, bpm))
}

fn read_anlz(path: &Path) -> Result<AnlzFile> {
    let data = std::fs::read(path).map_err(|e| Error::Io { path: path.display().to_string(), source: e })?;
    AnlzFile::parse(&data).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}

/// Writes next to the file and renames it into place.
fn write_anlz(path: &Path, file: &AnlzFile) -> Result<()> {
    let tmp = path.with_extension("musicbox-tmp");
    let io = |e| Error::Io { path: path.display().to_string(), source: e };
    std::fs::write(&tmp, file.to_bytes()).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

/// Copy-on-write backup (plan, safety rule 4): the first time a session touches a file, the
/// original goes to `<backup>/anlz/<AnalysisDataPath folder>/`.
fn backup(lib: &Library, paths: &AnlzPaths) -> Result<()> {
    let Some(dir) = &lib.anlz_backup else { return Ok(()) };
    let target = dir.join("anlz").join(Path::new(paths.rel.trim_start_matches('/')).parent().unwrap_or(Path::new("")));
    for src in [&paths.dat, &paths.ext] {
        let Some(name) = src.file_name() else { continue };
        let dst = target.join(name);
        if src.exists() && !dst.exists() {
            let io = |e| Error::Io { path: dst.display().to_string(), source: e };
            std::fs::create_dir_all(&target).map_err(io)?;
            std::fs::copy(src, &dst).map_err(io)?;
        }
    }
    Ok(())
}

fn replace_section(file: &mut AnlzFile, tag: Tag, section: Option<Section>) {
    match (file.sections.iter().position(|s| s.tag == tag), section) {
        (Some(i), Some(s)) => file.sections[i] = s,
        (Some(i), None) => {
            file.sections.remove(i);
        }
        (None, Some(s)) => file.sections.push(s),
        (None, None) => {}
    }
}

/// What the grid write puts into the files.
enum NewGrid<'a> {
    /// New beats; PQT2 follows rekordbox's rule.
    Beats(&'a [Beat]),
    /// Exact sections (undo).
    Exact { pqtz: &'a Section, pqt2: Option<&'a Section> },
}

pub(crate) fn set_beat_grid(lib: &mut Library, content: &DbId, beats: &[Beat]) -> Result<(Effect, Option<Command>)> {
    if beats.is_empty() {
        return Err(Error::Invalid("a beat grid needs at least one beat".into()));
    }
    if beats.windows(2).any(|w| w[1].time_ms <= w[0].time_ms) {
        return Err(Error::Invalid("beats must be in time order".into()));
    }
    // rekordbox: "Beatgrid adjustment error. Turn off Analysis Lock."
    if crate::analysis_lock::is_locked(lib, content)? {
        return Err(Error::Invalid("the track's analysis is locked; turn off Analysis Lock to adjust the beat grid".into()));
    }
    write_grid(lib, content, NewGrid::Beats(beats), None)
}

pub(crate) fn restore_beat_grid(
    lib: &mut Library,
    content: &DbId,
    pqtz: &Section,
    pqt2: Option<&Section>,
    bpm_x100: Option<i64>,
) -> Result<(Effect, Option<Command>)> {
    write_grid(lib, content, NewGrid::Exact { pqtz, pqt2 }, bpm_x100)
}

fn write_grid(lib: &mut Library, content: &DbId, grid: NewGrid<'_>, bpm_override: Option<i64>) -> Result<(Effect, Option<Command>)> {
    let (paths, old_bpm) = anlz_paths(lib, content)?;
    let mut dat = read_anlz(&paths.dat)?;
    let old_pqtz_section = dat
        .sections_with(Tag::PQTZ)
        .next()
        .cloned()
        .ok_or_else(|| Error::Invalid("the analysis file has no beat grid".into()))?;
    let old_grid = BeatGrid::decode(&old_pqtz_section).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut ext = if paths.ext.exists() { Some(read_anlz(&paths.ext)?) } else { None };
    let old_pqt2_section = ext.as_ref().and_then(|f| f.sections_with(Tag::PQT2).next().cloned());

    let (new_pqtz, new_pqt2) = match grid {
        NewGrid::Exact { pqtz, pqt2 } => (pqtz.clone(), pqt2.cloned()),
        NewGrid::Beats(beats) => {
            let new_grid = BeatGrid { adjust_offset: 0, beats: beats.to_vec(), ..old_grid.clone() };
            let pqt2 = old_pqt2_section.as_ref().map(|s| {
                let old = ExtBeatGrid::decode(s).ok();
                let unchanged = old.as_ref().is_some_and(|o| {
                    old_grid.adjust_offset == 0
                        && old_grid.beats.as_slice() == beats
                        && o.entries.len() == beats.len()
                        && o.matches(&old_grid.beats)
                        && beats.first() == Some(&o.first)
                        && beats.last() == Some(&o.last)
                });
                match (old, unchanged) {
                    (Some(_), true) => s.clone(),
                    (Some(o), false) => ExtBeatGrid {
                        first: Beat::default(),
                        last: Beat::default(),
                        checksum: 0,
                        entries: Vec::new(),
                        unk_14: 0,
                        ..o
                    }
                    .encode(),
                    // An undecodable PQT2 is kept as it is.
                    (None, _) => s.clone(),
                }
            });
            (new_grid.encode(), pqt2)
        }
    };
    let new_beats = BeatGrid::decode(&new_pqtz).map_err(|e| Error::Invalid(e.to_string()))?.beats;
    if new_pqtz == old_pqtz_section && new_pqt2 == old_pqt2_section && bpm_override.is_none_or(|b| Some(b) == old_bpm) {
        return Ok((Effect::Unchanged, None));
    }

    backup(lib, &paths)?;
    // Step 1: BPM column from the first beat (or the saved value on undo).
    let target_bpm = bpm_override.or_else(|| new_beats.first().map(|b| i64::from(b.tempo)));
    let tx = lib.writer.transaction()?;
    if let Some(bpm) = target_bpm
        && Some(bpm) != old_bpm
    {
        // hypothesis: `modifyTrackData` takes the metadata-edit path (edit-track.md): UPDATE-COL
        // BPM, then TrackInfoUpdated += 1.
        tx.update("djmdContent", "ID", Value::Text(content.0.clone()), &[("BPM", Value::Integer(bpm))])?;
        tx.increment_counter(content.as_str(), CounterColumn::TrackInfoUpdated)?;
    }
    // Step 2: the files. The database transaction is still open, so a failed write rolls the BPM
    // change back.
    replace_section(&mut dat, Tag::PQTZ, Some(new_pqtz.clone()));
    write_anlz(&paths.dat, &dat)?;
    if let Some(ext) = ext.as_mut()
        && new_pqt2 != old_pqt2_section
    {
        replace_section(ext, Tag::PQT2, new_pqt2.clone());
        write_anlz(&paths.ext, ext)?;
    }
    // Step 3: AnalysisUpdated, then the playlist timestamps.
    tx.increment_counter(content.as_str(), CounterColumn::AnalysisUpdated)?;
    tx.commit()?;
    let playlists: Vec<String> = {
        let mut stmt = lib
            .connection()
            .prepare("SELECT DISTINCT PlaylistID FROM djmdSongPlaylist WHERE rb_local_deleted = 0 AND ContentID = ?1")?;
        stmt.query_map([content.as_str()], |r| r.get(0))?.collect::<mb_masterdb::rusqlite::Result<_>>()?
    };
    if !playlists.is_empty() {
        xml_sync::sync(lib, &playlists)?;
    }
    let inverse = Command::RestoreBeatGrid { content: content.clone(), pqtz: old_pqtz_section, pqt2: old_pqt2_section, bpm_x100: old_bpm };
    Ok((Effect::Done, Some(inverse)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(tempo: u16, n: u32, first_ms: u32) -> Vec<Beat> {
        (0..n)
            .map(|i| Beat {
                beat_in_bar: (i % 4 + 1) as u16,
                tempo,
                time_ms: first_ms + (f64::from(i) * interval_ms(tempo)).round() as u32,
            })
            .collect()
    }

    #[test]
    fn shift_moves_and_refills() {
        let g = grid(12_000, 10, 100); // 500 ms apart: 100, 600, …, 4600
        let s = shift(&g, 50, 5_000);
        assert_eq!(s[0].time_ms, 150);
        assert_eq!(s.iter().map(|b| b.time_ms).max(), Some(4_650));
        let s = shift(&g, -200, 5_000);
        // 100 − 200 < 0 is dropped and refilled as 400 − 500 < 0: nothing in front.
        assert_eq!(s[0].time_ms, 400);
        assert_eq!(s[0].beat_in_bar, 2);
        // Room at the end: 4400 + 500 = 4900 ≤ 5000 is added.
        assert_eq!(s.last().unwrap().time_ms, 4_900);
        let s = shift(&g, 450, 5_000);
        assert_eq!((s[0].time_ms, s[0].beat_in_bar), (50, 4));
    }

    #[test]
    fn downbeat_renumbers() {
        let g = set_downbeat(&grid(12_800, 8, 0), 2);
        assert_eq!(g.iter().map(|b| b.beat_in_bar).collect::<Vec<_>>(), vec![3, 4, 1, 2, 3, 4, 1, 2]);
    }

    #[test]
    fn tempo_double_halve() {
        let g = grid(12_000, 8, 0);
        let d = double(&g, 4_000).unwrap();
        assert_eq!(d.len(), 16);
        assert!(d.iter().all(|b| b.tempo == 24_000));
        assert_eq!(d[1].time_ms, 250);
        assert_eq!(d[0].beat_in_bar, 1);
        assert_eq!(d[4].beat_in_bar, 1);
        let h = halve(&g).unwrap();
        assert_eq!(h.iter().map(|b| b.time_ms).collect::<Vec<_>>(), vec![0, 1000, 2000, 3000]);
        assert!(h.iter().all(|b| b.tempo == 6_000));
        assert_eq!(h.iter().map(|b| b.beat_in_bar).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        let s = set_tempo(&g, 2, 12_800, 3_000).unwrap();
        assert!(s.iter().any(|b| b.time_ms == g[2].time_ms && b.beat_in_bar == 3));
        assert!(s.iter().all(|b| b.tempo == 12_800));
        assert!(set_tempo(&g, 0, 1_000, 3_000).is_err());
    }
}
