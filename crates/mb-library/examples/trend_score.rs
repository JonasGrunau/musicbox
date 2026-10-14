//! The personal CUE tendency (`cue_personal_trend.json`, `docs/re/analysis-cue.md` § 4) against
//! rekordbox's. Usage: `cargo run --release -p mb-library --example trend_score -- <run> [playlist ID]`
//! with `<run>` a folder of `fixtures/oracle-cue` holding the library rekordbox analysed the
//! [CUE Analysis Playlist] in and the file it wrote.
//!
//! The playlist is the given ID, the one named in `<run>/browseSetting.xml` (`cueplaylistid`), or
//! 200000 (`cueplaylistdefaultid`). Its tracks' cues come from `djmdCue`, their beats and phrases
//! from the run's `.DAT`/`.EXT`. Our file is compared with rekordbox's byte for byte.

use mb_analysis::cue::{Trend, TrendTrack};
use mb_anlz::Tag;
use mb_anlz::tags::SongStructure;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::Path;

/// A private read-only copy of `<dir>/master.db` (with its WAL), so the fixture is never touched.
fn open_copy(dir: &Path, options: &Path) -> anyhow::Result<MasterDb> {
    let paths = LibraryPaths::from_options_json(options)?;
    let tmp = std::env::temp_dir().join(format!("mb-trend-score-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    for f in ["master.db", "master.db-wal", "master.db-shm"] {
        if dir.join(f).exists() {
            std::fs::copy(dir.join(f), tmp.join(f))?;
        }
    }
    Ok(MasterDb::open_read_only(&tmp.join("master.db"), &derive_passphrase(&paths.dp)?)?)
}

/// rekordbox's clean-up of a track's cue list (`DatabaseMediator::checkCueInfos` @0x101e5610c):
/// of two hot cues on one pad the later stays; no more than 10 memory cues. *Hypothesis:* of two
/// memory cues at the same in and out the earlier stays, and the list is in `rowid` order.
/// `(kind, in, out)` in list order; returns the `in`s.
fn cleaned(cues: &[(i64, i64, i64)]) -> Vec<u32> {
    let hot = |k: i64| (1..=3).contains(&k) || (5..=17).contains(&k);
    let memory = |k: i64| k == -1 || k == 0 || k == 4;
    let mut keep = vec![true; cues.len()];
    let mut pads: Vec<i64> = Vec::new();
    for i in (0..cues.len()).rev() {
        if hot(cues[i].0) {
            if pads.contains(&cues[i].0) {
                keep[i] = false;
            } else {
                pads.push(cues[i].0);
            }
        }
    }
    let mut seen: Vec<(i64, i64)> = Vec::new();
    let mut count = 0;
    for (i, c) in cues.iter().enumerate() {
        if keep[i] && memory(c.0) {
            if seen.contains(&(c.1, c.2)) || count >= 10 {
                keep[i] = false;
            } else {
                seen.push((c.1, c.2));
                count += 1;
            }
        }
    }
    cues.iter().zip(&keep).filter(|(_, k)| **k).map(|(c, _)| c.1.max(0) as u32).collect()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(run) = args.first() else { anyhow::bail!("usage: trend_score <run> [playlist ID]") };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-cue").join(run);
    let db = open_copy(&dir.join("rekordbox"), &dir.join("options.json"))?;
    let conn = db.connection();
    let from_settings = std::fs::read_to_string(dir.join("browseSetting.xml")).ok().and_then(|x| {
        x.split("name=\"cueplaylistid\" val=\"").nth(1).and_then(|r| r.split('"').next()).map(str::to_owned)
    });
    // `cueplaylistdefaultid`: rekordbox creates the playlist with ID 200000
    let playlist: String = args.get(1).cloned().or(from_settings).unwrap_or_else(|| "200000".into());
    let ids: Vec<String> = conn
        .prepare("SELECT ContentID FROM djmdSongPlaylist WHERE PlaylistID = ?1 AND rb_local_deleted = 0 ORDER BY TrackNo")?
        .query_map([&playlist], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let share = dir.join("rekordbox/share");
    let mut tracks = Vec::new();
    for id in &ids {
        let (rel, length, title, link): (Option<String>, Option<i64>, Option<String>, Option<i64>) = conn.query_row(
            "SELECT AnalysisDataPath, Length, Title, ContentLink FROM djmdContent WHERE ID = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let rel = rel.unwrap_or_default();
        let dat = share.join(rel.trim_start_matches('/'));
        let load = |ext: &str| std::fs::read(dat.with_extension(ext)).ok().and_then(|b| mb_anlz::AnlzFile::parse(&b).ok());
        let beats_ms: Vec<u32> = load("DAT").and_then(|d| d.beat_grid()).map(|g| g.beats.iter().map(|b| b.time_ms).collect()).unwrap_or_default();
        // `getSongStructInfo` @0x10149a0ac reads PSSI only for phrase-analysed tracks (RowDataTrack
        // +0x400 bit 16, *hypothesis:* `ContentLink` 0x10000; its +0x50 test, 1 or 6, is assumed met)
        let phrased = link.unwrap_or(0) & 0x10000 != 0;
        let pssi = load("EXT")
            .filter(|_| phrased)
            .and_then(|e| e.sections_with(Tag::PSSI).next().and_then(|s| SongStructure::decode(s).ok()));
        let cues: Vec<(i64, i64, i64)> = conn
            .prepare("SELECT Kind, InMsec, OutMsec FROM djmdCue WHERE ContentID = ?1 AND rb_local_deleted = 0 ORDER BY rowid")?
            .query_map([id], |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get::<_, Option<i64>>(2)?.unwrap_or(-1))))?
            .collect::<Result<_, _>>()?;
        let t = TrendTrack {
            beats_ms,
            cues_ms: cleaned(&cues),
            length: length.unwrap_or(0) as i32,
            mood: pssi.as_ref().map_or(0, |p| i32::from(p.mood)),
            starts: pssi.as_ref().map(|p| p.phrases.iter().map(|x| i32::from(x.beat())).collect()).unwrap_or_default(),
            kinds: pssi.as_ref().map(|p| p.phrases.iter().map(|x| i32::from(x.kind())).collect()).unwrap_or_default(),
        };
        println!("  {id} {}: {} cues, {} beats, mood {}, {} phrases, {} s", title.unwrap_or_default(), t.cues_ms.len(), t.beats_ms.len(), t.mood, t.starts.len(), t.length);
        tracks.push(t);
    }
    let ours = Trend::analyze(&tracks);
    let theirs_text = std::fs::read_to_string(dir.join("cue_personal_trend.json"))?;
    let ours_text = ours.to_json();
    println!("{run}: playlist {playlist}, {} tracks", ids.len());
    if ours_text == theirs_text {
        println!("  cue_personal_trend.json identical ({} bytes)", ours_text.len());
        return Ok(());
    }
    let theirs = Trend::parse(&theirs_text).map_err(anyhow::Error::msg)?;
    println!("  values {}, bytes differ", if ours == theirs { "identical" } else { "differ" });
    for (m, (a, b)) in ours.matrix.iter().zip(&theirs.matrix).enumerate() {
        for (r, (ra, rb)) in a.iter().zip(b).enumerate() {
            for (c, (x, y)) in ra.iter().zip(rb).enumerate() {
                if x != y {
                    println!("  matrix[{m}][{r}][{c}]: ours {x} vs rekordbox {y}");
                }
            }
        }
    }
    if ours.position != theirs.position {
        println!("  position: ours {:?} vs rekordbox {:?}", ours.position, theirs.position);
    }
    if let Some((i, (a, b))) = ours_text.lines().zip(theirs_text.lines()).enumerate().find(|(_, (a, b))| a != b) {
        println!("  first differing line {}: {a:?} vs {b:?}", i + 1);
    }
    Ok(())
}
