//! Scores the high-precision (Dynamic) beat grid against rekordbox's PQTZ, PQT2 and
//! `djmdContent.BPM`. Usage:
//!   cargo run --release -p mb-analysis --example hp_score -- [--verbose] [--no-phrase] [filter]
//! (`MB_ORACLE` defaults to `oracle-b`, the high-precision fixture; `MB_THREADS` limits the
//! tracks analysed at once, 2 by default since each model run is multi-threaded.)

#[path = "../tests/common/mod.rs"]
mod common;

use mb_analysis::hpbeat::HpMode;
use std::sync::Mutex;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let phrase = !args.iter().any(|a| a == "--no-phrase");
    let filter = args.iter().find(|a| !a.starts_with("--")).cloned();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-b".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    // Only tracks rekordbox analysed itself (they carry a PQT2).
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| {
            t.ext.as_ref().and_then(|e| e.ext_beat_grid()).is_some()
                && filter.as_ref().is_none_or(|f| t.path.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    let next = Mutex::new(0usize);
    let rows = Mutex::new(Vec::new());
    let threads = std::env::var("MB_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
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
                        phrase,
                        key: false,
                        high_precision: Some(HpMode::Dynamic),
                        vocal: false,
                        ..Default::default()
                    };
                    let t0 = std::time::Instant::now();
                    let r = mb_analysis::analyze(&t.path, opts);
                    rows.lock().unwrap().push((i, r, t0.elapsed().as_millis()));
                }
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let (mut n, mut exact, mut bpm_ok, mut pqt2_ok, mut no_grid_rb_type1) = (0, 0, 0, 0, 0);
    let mut ms_total = 0u128;
    for (i, r, ms) in &rows {
        let t = &tracks[*i];
        let full = t.path.to_string_lossy();
        let name = full.split("/corpus/").last().unwrap_or(&full);
        let rb = t.dat.as_ref().and_then(|d| d.beat_grid()).unwrap();
        let x = t.ext.as_ref().and_then(|e| e.ext_beat_grid()).unwrap();
        let rb_bpm = t.bpm.unwrap_or(0);
        n += 1;
        ms_total += ms;
        let a = match r {
            Ok(a) => a,
            Err(e) => {
                println!("{name}: error {e}");
                continue;
            }
        };
        if let Some(e) = &a.high_precision_error {
            println!("{name}: high precision failed: {e}");
            continue;
        }
        let Some(g) = &a.beats else {
            // rekordbox keeps the previous grid then (its PQT2 type stays 1)
            if x.analysis_type == 1 {
                no_grid_rb_type1 += 1;
            }
            if verbose || x.analysis_type != 1 {
                println!(
                    "{name}: no high-precision grid; rekordbox: PQT2 type {}, {} beats, BPM {rb_bpm}",
                    x.analysis_type,
                    rb.beats.len()
                );
            }
            continue;
        };
        let ours: Vec<mb_anlz::tags::Beat> = g
            .beats
            .iter()
            .map(|b| mb_anlz::tags::Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo, time_ms: b.time_ms })
            .collect();
        let mut mine = mb_anlz::tags::ExtBeatGrid::from_beats(&ours, g.beats.iter().map(|b| b.sub_ms).collect());
        mine.analysis_type = a.grid_type;
        let same = ours == rb.beats;
        let b_ok = i64::from(g.bpm) == rb_bpm;
        exact += usize::from(same);
        bpm_ok += usize::from(b_ok);
        pqt2_ok += usize::from(mine == x);
        if verbose || !same || !b_ok || mine != x {
            let first_diff = ours.iter().zip(&rb.beats).position(|(a, b)| a != b);
            println!(
                "{name}: grid {same} (n {} vs {}, first diff {first_diff:?}) | bpm {} vs {rb_bpm} | PQT2 {} (type {} vs {}) | {ms} ms",
                ours.len(),
                rb.beats.len(),
                g.bpm,
                mine == x,
                mine.analysis_type,
                x.analysis_type
            );
            if let Some(k) = first_diff {
                for (j, (o, r)) in ours.iter().zip(&rb.beats).enumerate().skip(k.saturating_sub(1)).take(4) {
                    println!("    {j}: ours {o:?} rb {r:?}");
                }
            }
        }
    }
    println!(
        "{n} tracks: grids identical {exact}, BPM×100 {bpm_ok}, PQT2 identical {pqt2_ok}; no HP grid where rekordbox kept a type-1 grid {no_grid_rb_type1}; {} ms per track",
        ms_total / n.max(1) as u128
    );
}
