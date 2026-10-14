//! Seeds a library for a personal CUE tendency reference run (`docs/re/analysis-cue.md` § 4,
//! fixture `oracle-cue/T2`): adds tracks to rekordbox's [CUE Analysis Playlist] and, on some of
//! them, a memory cue on the first beat (a phrase start) and one between two beats (off the grid).
//! Refuses while rekordbox runs; backs the library up first.
//!
//! `cargo run -p mb-library --example cue_trend_seed -- <options.json> <playlist id> <ids,…> [memory-cue ids,…]`

use mb_core::{DbId, Track};
use mb_library::{Command, CueSlot, Library, NewCue};
use mb_masterdb::{LibraryPaths, MasterDb, backup_library, default_backup_root, derive_passphrase};
use std::path::PathBuf;

fn beats_ms(track: &Track) -> anyhow::Result<Vec<u32>> {
    let dat = track.analysis_path.as_ref().ok_or_else(|| anyhow::anyhow!("{} is not analysed", track.title))?;
    let file = mb_anlz::AnlzFile::parse(&std::fs::read(dat)?)?;
    let grid = file.beat_grid().ok_or_else(|| anyhow::anyhow!("{} has no grid", track.title))?;
    Ok(grid.beats.iter().map(|b| b.time_ms).collect())
}

fn ids(arg: Option<String>) -> Vec<DbId> {
    arg.map(|s| s.split(',').filter(|x| !x.is_empty()).map(|x| DbId(x.trim().to_string())).collect()).unwrap_or_default()
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let options = PathBuf::from(args.next().expect("options.json"));
    let playlist = DbId(args.next().expect("playlist id"));
    let tracks = ids(args.next());
    let memo = ids(args.next());
    let paths = LibraryPaths::from_options_json(&options)?;
    let pass = derive_passphrase(&paths.dp)?;

    let mut lib = Library::open(&paths.db_path, &pass)?; // refuses while rekordbox runs
    let backup = backup_library(&paths.db_path, &pass, &default_backup_root().expect("HOME"))?;
    lib.set_analysis_root(&paths.analysis_data_root);
    lib.set_anlz_backup(&backup);
    println!("backup: {}", backup.display());

    lib.execute(Command::AddTracks { playlist: playlist.clone(), content_ids: tracks.clone(), position: None })?;
    println!("added {} tracks to playlist {}", tracks.len(), playlist.0);

    let snap = MasterDb::open_read_only(&paths.db_path, &pass)?.load_snapshot(&paths)?;
    for id in &memo {
        let t = snap.tracks.iter().find(|t| &t.id == id).ok_or_else(|| anyhow::anyhow!("no track {}", id.0))?;
        let b = beats_ms(t)?;
        anyhow::ensure!(b.len() > 12, "{} has too few beats", t.title);
        let on = b[0];
        let off = (b[10] + b[11]) / 2;
        for ms in [on, off] {
            let cue = NewCue { slot: CueSlot::Memory, in_ms: ms, out_ms: None, color: None, color_table_index: None, comment: String::new() };
            lib.execute(Command::AddCue { content: t.id.clone(), cue })?;
        }
        println!("{} — {}: memory cues at {on} ms (first beat) and {off} ms (between beats 10 and 11)", t.artist, t.title);
    }
    Ok(())
}
