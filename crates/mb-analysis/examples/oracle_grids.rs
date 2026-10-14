//! Lists the oracle grids for offline comparisons: one line per track rekordbox analysed,
//! `path \t BPM×100 \t PQT2 type \t beat_in_bar:tempo:ms:µs ...`.
//!   cargo run -p mb-analysis --example oracle_grids -- <oracle-a|oracle-b> > grids.tsv

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let fixture = std::env::args().nth(1).unwrap_or_else(|| "oracle-b".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    for t in tracks {
        let (Some(grid), Some(ext)) = (
            t.dat.as_ref().and_then(|d| d.beat_grid()),
            t.ext.as_ref().and_then(|e| e.ext_beat_grid()),
        ) else {
            continue;
        };
        let beats: Vec<String> = grid
            .beats
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let us = ext.entries.get(i).copied().unwrap_or(0);
                format!("{}:{}:{}:{}", b.beat_in_bar, b.tempo, b.time_ms, us)
            })
            .collect();
        println!(
            "{}\t{}\t{}\t{}",
            t.path.display(),
            t.bpm.unwrap_or(0),
            ext.analysis_type,
            beats.join(" ")
        );
    }
}
