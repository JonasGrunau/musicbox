//! M5 acceptance check: adds cues to one track and edits the beat grids of two others, so the
//! user can confirm in rekordbox that the edits show up. Refuses while rekordbox runs; backs the
//! library up first (analysis files go into the same backup before their first change).
//!
//! `cargo run -p mb-library --example m5_check -- <options.json>`

use mb_core::Track;
use mb_library::{Command, CueEdit, CueSlot, Effect, Library, NewCue, grid};
use mb_masterdb::{LibraryPaths, MasterDb, backup_library, default_backup_root, derive_passphrase};
use std::path::PathBuf;

fn beats(track: &Track) -> anyhow::Result<Vec<mb_library::Beat>> {
    let dat = track.analysis_path.as_ref().ok_or_else(|| anyhow::anyhow!("{} is not analysed", track.title))?;
    let file = mb_anlz::AnlzFile::parse(&std::fs::read(dat)?)?;
    Ok(file.beat_grid().ok_or_else(|| anyhow::anyhow!("{} has no grid", track.title))?.beats)
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
    let real: Vec<&Track> = snap
        .tracks
        .iter()
        .filter(|t| !t.artist.is_empty() && t.analysis_path.as_ref().is_some_and(|p| p.exists()) && t.length_seconds.unwrap_or(0) > 60)
        .take(3)
        .collect();
    anyhow::ensure!(real.len() == 3, "need three analysed tracks longer than a minute");
    let (cued, shifted, doubled) = (real[0], real[1], real[2]);

    // Track 1: memory cue, hot cues A and C, a memory loop, then a comment edit.
    let add = |lib: &mut Library, cue: NewCue| -> anyhow::Result<String> {
        match lib.execute(Command::AddCue { content: cued.id.clone(), cue })?.effect {
            Effect::CueAdded { id } => Ok(id),
            other => anyhow::bail!("unexpected {other:?}"),
        }
    };
    let memory = add(&mut lib, NewCue { slot: CueSlot::Memory, in_ms: 10_000, out_ms: None, color: None, color_table_index: None, comment: String::new() })?;
    add(&mut lib, NewCue { slot: CueSlot::Hot(0), in_ms: 20_000, out_ms: None, color: None, color_table_index: Some(8), comment: "musicbox A".into() })?;
    add(&mut lib, NewCue { slot: CueSlot::Hot(2), in_ms: 30_000, out_ms: None, color: None, color_table_index: None, comment: String::new() })?;
    add(&mut lib, NewCue { slot: CueSlot::Memory, in_ms: 40_000, out_ms: Some(44_000), color: None, color_table_index: None, comment: String::new() })?;
    lib.execute(Command::EditCue { id: memory, edit: CueEdit::Comment("musicbox memo".into()) })?;

    // Track 2: grid 50 ms later, downbeat one beat later.
    let duration = |t: &Track| {
        let ext = t.analysis_path.as_ref().and_then(|p| std::fs::read(p.with_extension("EXT")).ok());
        let ext = ext.and_then(|b| mb_anlz::AnlzFile::parse(&b).ok());
        grid::duration_ms(ext.as_ref(), t.length_seconds.unwrap_or(0))
    };
    let b = beats(shifted)?;
    let moved = grid::shift(&b, 50, duration(shifted));
    let down = moved.iter().position(|x| x.beat_in_bar == 1).unwrap_or(0) + 1;
    let regrid = grid::set_downbeat(&moved, down.min(moved.len() - 1));
    lib.execute(Command::SetBeatGrid { content: shifted.id.clone(), beats: regrid })?;

    // Track 3: tempo × 2.
    let b = beats(doubled)?;
    lib.execute(Command::SetBeatGrid { content: doubled.id.clone(), beats: grid::double(&b, duration(doubled))? })?;

    let bpm = |t: &Track| t.bpm().unwrap_or(0.0);
    println!("\nExpected in rekordbox (open each track in the deck / grid edit view):");
    println!("1. \"{} — {}\":", cued.artist, cued.title);
    println!("     memory cue at 0:10.000 with comment \"musicbox memo\";");
    println!("     hot cue A at 0:20.000, blue (colour code 8), comment \"musicbox A\";");
    println!("     hot cue C at 0:30.000 (default colour);");
    println!("     memory loop 0:40.000–0:44.000.");
    println!("2. \"{} — {}\": grid 50 ms later than before, and every bar line one beat later.", shifted.artist, shifted.title);
    println!("3. \"{} — {}\": BPM {:.2} (was {:.2}), grid twice as dense.", doubled.artist, doubled.title, bpm(doubled) * 2.0, bpm(doubled));
    println!("4. No warning or repair dialog; the tracks still play and show their waveforms.");
    Ok(())
}
