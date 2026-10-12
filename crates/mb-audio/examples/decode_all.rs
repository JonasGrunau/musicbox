//! Decodes every track of `fixtures/oracle-a` (or the files given as arguments) end to end and
//! prints reader, rates, length, peak and decode speed.

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let files = if args.is_empty() {
        common::oracle_tracks("oracle-a").expect("fixture").into_iter().map(|t| t.path).collect()
    } else {
        args
    };
    let (mut ok, mut failed, mut secs) = (0, 0, 0.0f64);
    let t0 = Instant::now();
    for f in &files {
        let start = Instant::now();
        match mb_audio::open(f) {
            Ok(mut d) => {
                let pcm = d.decode_all();
                let peak = pcm.iter().flat_map(|s| s.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
                let info = d.info();
                secs += info.duration_secs();
                ok += 1;
                println!(
                    "{:?}\t{}\t{}\t{}\t{:.3}\t{:.0}x\t{}",
                    info.backend,
                    info.file_rate,
                    info.sample_rate,
                    info.total_samples,
                    peak,
                    info.duration_secs() / start.elapsed().as_secs_f64(),
                    f.file_name().unwrap().to_string_lossy()
                );
            }
            Err(e) => {
                failed += 1;
                println!("ERROR\t{e}\t{}", f.display());
            }
        }
    }
    eprintln!("{ok} decoded, {failed} failed, {:.0} s of audio in {:.1} s", secs, t0.elapsed().as_secs_f64());
}
