//! Scores the beat-grid port against rekordbox's PQTZ and `djmdContent.BPM`. Usage:
//!   cargo run --release -p mb-analysis --example beat_score -- [--verbose] [--dump] [filter]
//! (`MB_ORACLE=oracle-b` for the second fixture, `MB_DYNAMIC=1` for Normal-mode Dynamic as in
//! oracle-e, `MB_THREADS` to limit threads.)

#[path = "../tests/common/mod.rs"]
mod common;

use std::sync::Mutex;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let dump = args.iter().any(|a| a == "--dump");
    let filter = args.iter().find(|a| !a.starts_with("--")).cloned();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    // Only tracks rekordbox analysed itself (they carry a PQT2).
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| {
            t.ext.as_ref().and_then(|e| e.ext_beat_grid()).is_some()
                && filter
                    .as_ref()
                    .is_none_or(|f| t.path.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    let next = Mutex::new(0usize);
    let rows = Mutex::new(Vec::new());
    let threads = std::env::var("MB_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let Some(t) = tracks.get(i) else { break };
                    let opts = mb_analysis::Options {
                        waveforms: false,
                        loudness: false,
                        beats: true,
                        phrase: true,
                        key: false,
                        high_precision: None,
                        vocal: false,
                        dynamic: std::env::var("MB_DYNAMIC").is_ok_and(|v| v == "1"),
                        auto_cue: false,
                    };
                    let r = mb_analysis::analyze(&t.path, opts).map(|a| (a.beats, a.grid_type));
                    rows.lock().unwrap().push((i, r));
                }
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let (mut n, mut bpm_ok, mut count_ok, mut bar_ok) = (0, 0, 0, 0);
    let (mut beats_total, mut beats_1ms, mut beats_exact) = (0usize, 0usize, 0usize);
    let (mut pqt2_ok, mut pqt2_n) = (0usize, 0usize);
    for (i, r) in &rows {
        let t = &tracks[*i];
        let full = t.path.to_string_lossy();
        let name = full.split("/corpus/").last().unwrap_or(&full);
        let rb = t.dat.as_ref().and_then(|d| d.beat_grid()).unwrap();
        let rb_bpm = t.bpm.unwrap_or(0);
        n += 1;
        let Ok((Some(g), grid_type)) = r else {
            println!("{name}: no grid ({:?})", r.as_ref().err());
            continue;
        };
        let b_ok = i64::from(g.bpm) == rb_bpm;
        bpm_ok += usize::from(b_ok);
        let c_ok = g.beats.len() == rb.beats.len();
        count_ok += usize::from(c_ok);
        let mut within = 0;
        let mut exact = 0;
        let mut bars = true;
        for (a, b) in g.beats.iter().zip(&rb.beats) {
            let d = (i64::from(a.time_ms) - i64::from(b.time_ms)).abs();
            within += usize::from(d <= 1);
            exact += usize::from(d == 0);
            bars &= a.beat_in_bar == b.beat_in_bar;
        }
        beats_total += rb.beats.len();
        beats_1ms += within;
        beats_exact += exact;
        bar_ok += usize::from(bars && c_ok);
        // PQT2: rebuilt from our grid, compared whole (header, checksum, µs entries).
        if let Some(x) = t.ext.as_ref().and_then(|e| e.ext_beat_grid()) {
            let ours: Vec<mb_anlz::tags::Beat> = g
                .beats
                .iter()
                .map(|b| mb_anlz::tags::Beat {
                    beat_in_bar: b.beat_in_bar,
                    tempo: b.tempo,
                    time_ms: b.time_ms,
                })
                .collect();
            let mut mine = mb_anlz::tags::ExtBeatGrid::from_beats(
                &ours,
                g.beats.iter().map(|b| b.sub_ms).collect(),
            );
            mine.analysis_type = *grid_type;
            pqt2_n += 1;
            if mine == x {
                pqt2_ok += 1;
            } else if verbose {
                println!(
                    "  pqt2 differs: tail {:?} vs {:?}, entries[..4] {:?} vs {:?}",
                    mine.tail,
                    x.tail,
                    &mine.entries[..mine.entries.len().min(4)],
                    &x.entries[..x.entries.len().min(4)]
                );
            }
        }
        let first = |v: &[_]| -> String { format!("{:?}", v.iter().take(2).collect::<Vec<_>>()) };
        if verbose || !b_ok || !c_ok || within != rb.beats.len() || !bars {
            let shift = match (g.beats.first(), rb.beats.first()) {
                (Some(a), Some(b)) => i64::from(b.time_ms) - i64::from(a.time_ms),
                _ => 0,
            };
            println!(
                "{name}: bpm {} vs {rb_bpm} | n {} vs {} | ±1ms {within} | bars {bars} | shift {shift} ms | first {} vs {}",
                g.bpm,
                g.beats.len(),
                rb.beats.len(),
                first(
                    &g.beats
                        .iter()
                        .map(|b| (b.beat_in_bar, b.time_ms))
                        .collect::<Vec<_>>()
                ),
                first(
                    &rb.beats
                        .iter()
                        .map(|b| (b.beat_in_bar, b.time_ms))
                        .collect::<Vec<_>>()
                ),
            );
        }
        if dump {
            for (a, b) in g.beats.iter().zip(&rb.beats).take(40) {
                println!(
                    "  {:>2}/{:>6}/{:>8}  rb {:>2}/{:>6}/{:>8}",
                    a.beat_in_bar, a.tempo, a.time_ms, b.beat_in_bar, b.tempo, b.time_ms
                );
            }
        }
    }
    println!(
        "{n} tracks: BPM×100 exact {bpm_ok}, beat count {count_ok}, bars {bar_ok}; beats ±1 ms {beats_1ms}/{beats_total}, exact {beats_exact}; PQT2 identical {pqt2_ok}/{pqt2_n}"
    );
}
