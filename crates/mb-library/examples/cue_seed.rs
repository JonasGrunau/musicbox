//! Seeds cues for the CUE-analysis reference runs (`docs/re/analysis-cue.md`, fixture
//! `oracle-cue/M5*`): ten analysed real tracks, by title, get existing cues so that rekordbox's
//! slot search, duplicate check and overwrite show. Refuses while rekordbox runs; backs the
//! library up first.
//!
//! 1–3: hot cue A at 0:30; 4–5: hot cues A and B at 0:30 and 0:40;
//! 6–8: a memory cue on the first downbeat (where Manual's 1.1-bar cue lands);
//! 9–10: a memory cue at 0:30.
//!
//! `cargo run -p mb-library --example cue_seed -- <options.json>`

use mb_core::Track;
use mb_library::{Command, CueSlot, Library, NewCue};
use mb_masterdb::{LibraryPaths, MasterDb, backup_library, default_backup_root, derive_passphrase};
use std::path::PathBuf;

fn first_downbeat_ms(track: &Track) -> anyhow::Result<u32> {
    let dat = track.analysis_path.as_ref().ok_or_else(|| anyhow::anyhow!("{} is not analysed", track.title))?;
    let file = mb_anlz::AnlzFile::parse(&std::fs::read(dat)?)?;
    let grid = file.beat_grid().ok_or_else(|| anyhow::anyhow!("{} has no grid", track.title))?;
    let beat = grid.beats.iter().find(|b| b.beat_in_bar == 1).ok_or_else(|| anyhow::anyhow!("{} has no downbeat", track.title))?;
    Ok(beat.time_ms)
}

fn main() -> anyhow::Result<()> {
    let options = PathBuf::from(std::env::args().nth(1).expect("options.json"));
    let paths = LibraryPaths::from_options_json(&options)?;
    let pass = derive_passphrase(&paths.dp)?;

    let mut lib = Library::open(&paths.db_path, &pass)?; // refuses while rekordbox runs
    let backup = backup_library(&paths.db_path, &pass, &default_backup_root().expect("HOME"))?;
    lib.set_analysis_root(&paths.analysis_data_root);
    lib.set_anlz_backup(&backup);
    println!("backup: {}", backup.display());

    let snap = MasterDb::open_read_only(&paths.db_path, &pass)?.load_snapshot(&paths)?;
    let mut real: Vec<&Track> = snap
        .tracks
        .iter()
        .filter(|t| !t.artist.is_empty() && t.analysis_path.as_ref().is_some_and(|p| p.exists()) && t.length_seconds.unwrap_or(0) > 60)
        .collect();
    real.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.0.cmp(&b.id.0)));
    anyhow::ensure!(real.len() >= 10, "need ten analysed tracks longer than a minute");

    let cue = |slot, in_ms| NewCue { slot, in_ms, out_ms: None, color: None, color_table_index: None, comment: String::new() };
    for (i, t) in real.iter().take(10).enumerate() {
        let cues = match i {
            0..=2 => vec![cue(CueSlot::Hot(0), 30_000)],
            3..=4 => vec![cue(CueSlot::Hot(0), 30_000), cue(CueSlot::Hot(1), 40_000)],
            5..=7 => vec![cue(CueSlot::Memory, first_downbeat_ms(t)?)],
            _ => vec![cue(CueSlot::Memory, 30_000)],
        };
        for c in cues {
            println!("{:>2}. {} — {}: {:?} at {} ms", i + 1, t.artist, t.title, c.slot, c.in_ms);
            lib.execute(Command::AddCue { content: t.id.clone(), cue: c })?;
        }
    }
    Ok(())
}
