//! Vocal detection (PVDI) against rekordbox's. Usage:
//!   cargo run --release -p mb-analysis --example vocal_score -- oracle <out_dir> [filter]
//!     writes `<i>.txt` (track path + rekordbox's PVDI) for the harness comparison
//!   cargo run --release -p mb-analysis --example vocal_score -- score [filter]
//!     our vocal pass against the stored PVDI of every oracle track (MB_THREADS, default 4)
//!   cargo run --release -p mb-analysis --example vocal_score -- check <pcm.bin> <dump_dir>
//!     compares every stage with the harness dumps of the same `pcm_dump` file
//! (`MB_ORACLE` selects the fixture, default oracle-a.)

#[path = "../tests/common/mod.rs"]
mod common;

use mb_anlz::Tag;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    if args.first().map(String::as_str) == Some("check") {
        return check(std::path::Path::new(&args[1]), std::path::Path::new(&args[2]));
    }
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        anyhow::bail!("fixtures/{fixture} missing");
    };
    match args.first().map(String::as_str) {
        Some("oracle") => {
            let dir = std::path::Path::new(&args[1]);
            std::fs::create_dir_all(dir)?;
            let filter = args.get(2);
            let mut n = 0;
            for (i, t) in tracks.iter().enumerate() {
                if filter.is_some_and(|f| !t.path.to_string_lossy().contains(f.as_str())) {
                    continue;
                }
                let Some((header, body)) = t.section(Tag::PVDI) else { continue };
                let mut f = std::fs::File::create(dir.join(format!("{i:03}.txt")))?;
                writeln!(f, "PATH {}", t.path.display())?;
                writeln!(f, "HEADER {}", header.iter().map(|b| format!("{b:02x}")).collect::<String>())?;
                writeln!(f, "PVDI {}", body.len())?;
                for (k, b) in body.iter().enumerate() {
                    write!(f, "{b}{}", if k % 64 == 63 || k == body.len() - 1 { "\n" } else { " " })?;
                }
                n += 1;
            }
            println!("{n} tracks written");
        }
        Some("check") => check(std::path::Path::new(&args[1]), std::path::Path::new(&args[2]))?,
        Some("score") => score(&tracks, args.get(1)),
        _ => anyhow::bail!("usage: vocal_score oracle <out_dir> [filter] | check <pcm> <dumps>"),
    }
    Ok(())
}

/// Concatenated `(u32 count, f64…)` records of a harness dump.
fn dump(path: &std::path::Path) -> Vec<f64> {
    let b = std::fs::read(path).unwrap_or_default();
    let (mut v, mut p) = (Vec::new(), 0);
    while p + 4 <= b.len() {
        let n = u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        v.extend(b[p..p + 8 * n].chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())));
        p += 8 * n;
    }
    v
}

fn report(name: &str, ours: &[f64], rb: &[f64]) {
    let n = ours.len().min(rb.len());
    let exact = (0..n).filter(|&i| ours[i].to_bits() == rb[i].to_bits()).count();
    let first = (0..n).find(|&i| ours[i].to_bits() != rb[i].to_bits());
    let max_rel = (0..n)
        .map(|i| (ours[i] - rb[i]).abs() / rb[i].abs().max(1e-300))
        .filter(|v| v.is_finite())
        .fold(0f64, f64::max);
    println!(
        "{name:10} n {} vs {} | exact {exact}/{n} | first diff {first:?} {} | max rel {max_rel:.3e}",
        ours.len(),
        rb.len(),
        first.map_or(String::new(), |i| format!("({:e} vs {:e})", ours[i], rb[i]))
    );
}

fn check(pcm: &std::path::Path, dir: &std::path::Path) -> anyhow::Result<()> {
    use mb_analysis::vocal::{Model, VocalDetector, features};
    let b = std::fs::read(pcm)?;
    anyhow::ensure!(&b[..4] == b"PCM1", "not a pcm_dump file");
    let rate = u32::from_le_bytes(b[4..8].try_into()?);
    let len = u64::from_le_bytes(b[8..16].try_into()?) as usize;
    let st: Vec<f32> = b[16..16 + 8 * len].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    let total = len - 1;
    let mono: Vec<f32> = (0..total)
        .map(|i| (f32::from(mb_audio::analysis_i16(st[2 * i + 1])) + f32::from(mb_audio::analysis_i16(st[2 * i]))) * 0.5)
        .collect();
    let model = Model::get().map_err(anyhow::Error::msg)?;
    let mut det = VocalDetector::new(rate, model.tables());
    report("hann", &det.hann().iter().map(|&v| f64::from(v)).collect::<Vec<_>>(), &dump(&dir.join("hann.bin")));
    report("iir", &det.lowpass().coefficients().iter().map(|&v| f64::from(v)).collect::<Vec<_>>(), &dump(&dir.join("iir.bin")));
    let t = model.tables();
    report("melfilter", &t.mel, &dump(&dir.join("melfilter.bin")));
    report("mfcctab", &t.dct, &dump(&dir.join("mfcctab.bin")));
    det.debug = Some(Default::default());
    for blk in mono.chunks(2 * rate as usize) {
        det.put(blk);
    }
    let (mut f, dbg) = det.features().ok_or_else(|| anyhow::anyhow!("too short"))?;
    let dbg = dbg.unwrap_or_default();
    report("power", &dbg.power, &dump(&dir.join("power.bin")));
    report("mel", &dbg.mel, &dump(&dir.join("mel.bin")));
    report("fluct", &dbg.fluct, &dump(&dir.join("fluct.bin")));
    let fb = std::fs::read(dir.join("feat.bin"))?;
    let n = u32::from_le_bytes(fb[..4].try_into()?) as usize;
    let pre: Vec<f64> = fb[4..4 + 8 * n].chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect();
    let post: Vec<f64> = fb[4 + 8 * n..4 + 16 * n].chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect();
    report("features", &f, &pre);
    features::normalize(&mut f, model.norm());
    report("normalized", &f, &post);
    let pb = std::fs::read(dir.join("prob.bin"))?;
    let np = u32::from_le_bytes(pb[..4].try_into()?) as usize;
    let rp: Vec<f64> = pb[4..4 + 4 * np].chunks_exact(4).map(|c| f64::from(f32::from_le_bytes(c.try_into().unwrap()))).collect();
    let p = mb_analysis::vocal::infer::probabilities(model.session(), &f).map_err(|e| anyhow::anyhow!("{e}"))?;
    report("prob", &p.iter().map(|&v| f64::from(v)).collect::<Vec<_>>(), &rp);
    Ok(())
}

fn score(tracks: &[common::OracleTrack], filter: Option<&String>) {
    let tracks: Vec<_> = tracks
        .iter()
        .filter(|t| t.section(Tag::PVDI).is_some() && filter.is_none_or(|f| t.path.to_string_lossy().contains(f.as_str())))
        .collect();
    let next = std::sync::Mutex::new(0usize);
    let rows = std::sync::Mutex::new(Vec::new());
    let threads = std::env::var("MB_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = {
                    let mut n = next.lock().unwrap();
                    *n += 1;
                    *n - 1
                };
                let Some(t) = tracks.get(i) else { break };
                let opts = mb_analysis::Options {
                    waveforms: false,
                    loudness: false,
                    beats: false,
                    phrase: false,
                    key: false,
                    high_precision: None,
                    vocal: true,
                    ..Default::default()
                };
                let r = mb_analysis::analyze(&t.path, opts);
                rows.lock().unwrap().push((i, r));
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let (mut exact, mut values, mut values_ok, mut header_ok) = (0, 0usize, 0usize, 0);
    for (i, r) in &rows {
        let t = tracks[*i];
        let full = t.path.to_string_lossy();
        let name = full.split("/corpus/").last().unwrap_or(&full);
        let (header, body) = t.section(Tag::PVDI).unwrap();
        let ours = match r {
            Ok(a) => match (&a.vocal, &a.vocal_error) {
                (Some(v), _) => v.clone(),
                (None, e) => {
                    println!("{name}: no PVDI ({e:?})");
                    continue;
                }
            },
            Err(e) => {
                println!("{name}: error {e}");
                continue;
            }
        };
        // PVDI header: version 0, 1024, 22050, entry size 1, count
        let mut h = vec![0u8, 0, 0x04, 0x00, 0x56, 0x22, 0x00, 0x01];
        h.extend((ours.len() as u32).to_be_bytes());
        header_ok += usize::from(h == header);
        let same = ours == body;
        exact += usize::from(same);
        values += body.len();
        values_ok += ours.iter().zip(body).filter(|(a, b)| a == b).count();
        if !same {
            let first = ours.iter().zip(body).position(|(a, b)| a != b);
            println!("{name}: n {} vs {} | first diff {first:?}", ours.len(), body.len());
        }
    }
    println!("{} tracks: PVDI identical {exact}, header {header_ok}; values {values_ok}/{values}", rows.len());
}
