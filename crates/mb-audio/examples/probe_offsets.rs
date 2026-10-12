//! Decodes every probe format of one probe (default `click_128_offset1234`) and reports, per
//! format: rekordbox reader, rates, decoded length, lag against the WAV reference at the same
//! output rate (cross-correlation around the first onset) and the residual after alignment.

use mb_audio::Float2;
use std::path::PathBuf;

fn mono(x: &[Float2]) -> Vec<f64> {
    x.iter().map(|s| f64::from(s[0]) + f64::from(s[1])).collect()
}

/// Lag (decoded − reference) maximising the correlation of `r[a..b]` with `d[a+lag..b+lag]`.
fn best_lag(r: &[f64], d: &[f64], a: usize, b: usize, max: i64) -> (i64, f64) {
    let mut best = (0, f64::MIN);
    for lag in -max..=max {
        let s: f64 = r[a..b]
            .iter()
            .enumerate()
            .filter_map(|(k, x)| usize::try_from((a + k) as i64 + lag).ok().and_then(|j| d.get(j)).map(|y| x * y))
            .sum();
        if s > best.1 {
            best = (lag, s);
        }
    }
    best
}

fn main() {
    let probe = std::env::args().nth(1).unwrap_or_else(|| "click_128_offset1234".into());
    let root = PathBuf::from(std::env::var("HOME").unwrap()).join("re-work/corpus/probes");
    let refs = [("wav_44k16", "wav"), ("wav_48k24", "wav")];
    let mut reference = std::collections::HashMap::new();
    for (dir, ext) in refs {
        let mut d = mb_audio::open(root.join(dir).join(format!("{probe}.{ext}"))).unwrap();
        let rate = d.info().sample_rate;
        reference.insert(rate, d.decode_all());
    }
    let mut dirs: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().filter(|e| e.path().is_dir()).map(|e| e.path()).collect();
    dirs.sort();
    println!("format\tbackend\tfile_rate\trate\tfile_samples\ttotal\tlag\tmax_residual");
    for dir in dirs {
        let Some(file) = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(probe.as_str())) else { continue };
        let mut d = match mb_audio::open(&file) {
            Ok(d) => d,
            Err(e) => {
                println!("{}\terror {e}", dir.file_name().unwrap().to_string_lossy());
                continue;
            }
        };
        let info = *d.info();
        let pcm = d.decode_all();
        let r = &reference[&info.sample_rate];
        let (rm, dm) = (mono(r), mono(&pcm));
        let onset = rm.iter().position(|v| v.abs() > 0.02).unwrap_or(0);
        let a = onset.saturating_sub(500);
        let b = (onset + 6000).min(rm.len());
        let (lag, _) = best_lag(&rm, &dm, a, b, 3000);
        let mut resid = 0.0f64;
        for (k, x) in r[a..b].iter().enumerate() {
            if let Some(y) = usize::try_from((a + k) as i64 + lag).ok().and_then(|j| pcm.get(j)) {
                for c in 0..2 {
                    resid = resid.max((f64::from(x[c]) - f64::from(y[c])).abs());
                }
            }
        }
        println!(
            "{}\t{:?}\t{}\t{}\t{}\t{}\t{}\t{:.3e}",
            dir.file_name().unwrap().to_string_lossy(),
            info.backend,
            info.file_rate,
            info.sample_rate,
            info.file_samples,
            info.total_samples,
            lag,
            resid
        );
    }
}
