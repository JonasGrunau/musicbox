//! Dumps rekordbox's per-track timing data from `fixtures/oracle-a` (or the fixture named as the
//! first argument): sample rate, PVBR sample count / non-zero index entries, PVB2, PWV3/PWV5/PWV7
//! column counts and the first PQTZ beat. Exploration aid for `docs/re/decode.md`.

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "oracle-a".into());
    let Some(tracks) = common::oracle_tracks(&name) else {
        eprintln!("fixture {name} or corpus missing");
        return;
    };
    println!("rel\tftype\trate\tbits\tlen_s\tpvbr_n\tpvbr_nz\tpvb2_total\tpvb2_n\tpwv3\tpwv5\tpwv7\tbeat0_ms\tbeats");
    for t in &tracks {
        let vbr = t.vbr();
        let grid = t.beat_grid();
        let pvb2 = t.seek_table();
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            t.corpus_rel().unwrap_or_else(|| t.path.display().to_string()),
            t.file_type,
            t.sample_rate,
            t.bit_depth,
            t.length,
            vbr.as_ref().and_then(|v| v.sample_count).map(|c| c.to_string()).unwrap_or_default(),
            vbr.as_ref().map(|v| v.index.iter().filter(|&&x| x != 0).count()).unwrap_or(0),
            pvb2.as_ref().map(|p| p.total_samples.to_string()).unwrap_or_default(),
            pvb2.as_ref().map(|p| p.points.len()).unwrap_or(0),
            t.wave_scroll().map(|w| w.len()).unwrap_or(0),
            t.color_scroll().map(|w| w.data.len()).unwrap_or(0),
            t.band_scroll().map(|w| w.data.len()).unwrap_or(0),
            grid.as_ref().and_then(|g| g.beats.first()).map(|b| b.time_ms.to_string()).unwrap_or_default(),
            grid.as_ref().map(|g| g.beats.len()).unwrap_or(0),
        );
    }
}
