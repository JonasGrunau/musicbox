//! Scores the auto-gain port against rekordbox's `djmdMixerParam` rows. Usage:
//!   cargo run --release -p mb-analysis --example loudness_score -- [--verbose] [filter]
//! (`MB_ORACLE=oracle-b` for the second fixture.)

#[path = "../tests/common/mod.rs"]
mod common;

use std::sync::Mutex;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let filter = args.iter().find(|a| !a.starts_with("--")).cloned();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| {
            t.mixer.is_some()
                && filter
                    .as_ref()
                    .is_none_or(|f| t.path.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    let next = Mutex::new(0usize);
    let rows = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
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
                        loudness: true,
                        beats: false,
                        phrase: false,
                        key: false,
                    };
                    let r = mb_analysis::analyze(&t.path, opts).map(|a| a.loudness.unwrap());
                    rows.lock().unwrap().push((t, r));
                }
            });
        }
    });
    let rows = rows.into_inner().unwrap();
    let (mut gain_exact, mut peak_exact, mut worst_db, mut n) = (0, 0, 0f64, 0);
    for (t, r) in &rows {
        let name = t.path.file_name().unwrap().to_string_lossy();
        let Ok(l) = r else {
            println!("FAILED {name}: {:?}", r.as_ref().err());
            continue;
        };
        n += 1;
        let m = t.mixer.unwrap();
        let rb_gain = f32::from_bits(((m[0] as u32) << 16) | (m[1] as u32 & 0xffff));
        let rb_peak = f32::from_bits(((m[2] as u32) << 16) | (m[3] as u32 & 0xffff));
        let ge = rb_gain.to_bits() == l.gain.to_bits();
        let pe = rb_peak.to_bits() == l.peak.to_bits();
        gain_exact += usize::from(ge);
        peak_exact += usize::from(pe);
        let db = (20.0 * f64::from(rb_gain).log10() - l.gain_db).abs();
        worst_db = worst_db.max(db);
        if verbose || !ge || !pe {
            println!(
                "{name}: LUFS {:.4} gain {} vs rb {} ({:+.6} dB) peak {} vs rb {}",
                l.lufs, l.gain, rb_gain, -db, l.peak, rb_peak
            );
        }
    }
    println!(
        "{n} tracks: gain bit-exact {gain_exact}, peak bit-exact {peak_exact}, worst gain diff {worst_db:.6} dB"
    );
}
