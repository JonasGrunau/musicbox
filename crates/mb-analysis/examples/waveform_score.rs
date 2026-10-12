//! Scores `mb_analysis::analyze_waveforms` against rekordbox's own analysis of the corpus
//! (`fixtures/oracle-a`). Usage:
//!   cargo run --release -p mb-analysis --example waveform_score -- [--json out.json] [--verbose] [filter]
//! Per tag: tracks byte-exact, tracks within ±1 per field, and the worst difference.

#[path = "../tests/common/mod.rs"]
mod common;

use mb_analysis::Waveforms;
use mb_anlz::Tag;
use std::sync::Mutex;

/// The tags scored, with how to split a body into fields for the ±1 test.
const TAGS: &[(&str, Tag)] = &[
    ("PWAV", Tag(*b"PWAV")),
    ("PWV2", Tag(*b"PWV2")),
    ("PWV3", Tag(*b"PWV3")),
    ("PWV4", Tag(*b"PWV4")),
    ("PWV5", Tag(*b"PWV5")),
    ("PWV6", Tag(*b"PWV6")),
    ("PWV7", Tag(*b"PWV7")),
    ("PWVC", Tag(*b"PWVC")),
];

fn ours(w: &Waveforms, name: &str) -> Option<Vec<u8>> {
    match name {
        "PWAV" => Some(w.pwav.clone()),
        "PWV2" => Some(w.pwv2.clone()),
        "PWV3" => Some(w.pwv3.clone()),
        "PWV4" => Some(w.pwv4.concat()),
        "PWV5" => Some(w.pwv5.iter().flat_map(|v| v.to_be_bytes()).collect()),
        "PWV6" => Some(w.pwv6.concat()),
        "PWV7" => Some(w.pwv7.concat()),
        "PWVC" => Some(w.pwvc.iter().flat_map(|v| v.to_be_bytes()).collect()),
        _ => None,
    }
}

/// Field values of a body: heights and colours unpacked so ±1 compares like quantities.
fn fields(name: &str, body: &[u8]) -> Vec<i32> {
    match name {
        "PWAV" | "PWV2" | "PWV3" => body
            .iter()
            .flat_map(|&b| [i32::from(b & 0x1f), i32::from(b >> 5)])
            .collect(),
        "PWV5" => body
            .chunks_exact(2)
            .flat_map(|c| {
                let v = u16::from_be_bytes([c[0], c[1]]);
                [(v >> 13) & 7, (v >> 10) & 7, (v >> 7) & 7, (v >> 2) & 0x1f].map(i32::from)
            })
            .collect(),
        "PWVC" => body
            .chunks_exact(2)
            .map(|c| i32::from(u16::from_be_bytes([c[0], c[1]])))
            .collect(),
        _ => body.iter().map(|&b| i32::from(b)).collect(),
    }
}

#[derive(Default, Clone)]
struct TagScore {
    tracks: usize,
    exact: usize,
    within1: usize,
    length_mismatch: usize,
    worst: i32,
    worst_track: String,
    diff_fields: usize,
    total_fields: usize,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args
        .iter()
        .position(|a| a == "--json")
        .map(|i| args[i + 1].clone());
    let verbose = args.iter().any(|a| a == "--verbose");
    let dump = args
        .iter()
        .position(|a| a == "--dump")
        .map(|i| std::path::PathBuf::from(&args[i + 1]));
    let filter = args
        .iter()
        .enumerate()
        .find(|(i, a)| {
            !a.starts_with("--")
                && (*i == 0 || !["--json", "--dump"].contains(&args[i - 1].as_str()))
        })
        .map(|(_, a)| a.clone());
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/oracle-a missing");
        return Ok(());
    };
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| {
            filter
                .as_ref()
                .is_none_or(|f| t.path.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    let scores = Mutex::new(vec![TagScore::default(); TAGS.len()]);
    let next = Mutex::new(0usize);
    let failures = Mutex::new(Vec::new());
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
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some(t) = tracks.get(i) else { break };
                    let w = match mb_analysis::analyze_waveforms(&t.path) {
                        Ok(w) => w,
                        Err(e) => {
                            failures.lock().unwrap().push(format!("{}: {e}", t.path.display()));
                            continue;
                        }
                    };
                    let name = t.path.file_name().unwrap().to_string_lossy().into_owned();
                    for (k, (tag_name, tag)) in TAGS.iter().enumerate() {
                        let (Some(mine), Some((_, theirs))) = (ours(&w, tag_name), t.section(*tag)) else { continue };
                        if let Some(dir) = &dump {
                            let _ = std::fs::create_dir_all(dir);
                            let _ = std::fs::write(dir.join(format!("{name}.{tag_name}.ours")), &mine);
                            let _ = std::fs::write(dir.join(format!("{name}.{tag_name}.rb")), theirs);
                        }
                        let a = fields(tag_name, &mine);
                        let b = fields(tag_name, theirs);
                        let mut sc = scores.lock().unwrap();
                        let s = &mut sc[k];
                        s.tracks += 1;
                        if a.len() != b.len() {
                            s.length_mismatch += 1;
                            if verbose {
                                println!("{tag_name} {name}: {} fields vs rekordbox {}", a.len(), b.len());
                            }
                            continue;
                        }
                        let diffs: Vec<i32> = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).collect();
                        let worst = diffs.iter().copied().max().unwrap_or(0);
                        let nd = diffs.iter().filter(|&&d| d != 0).count();
                        s.total_fields += a.len();
                        s.diff_fields += nd;
                        if worst == 0 {
                            s.exact += 1;
                        } else {
                            if worst <= 1 {
                                s.within1 += 1;
                            }
                            if verbose {
                                let first = diffs.iter().position(|&d| d != 0).unwrap();
                                println!("{tag_name} {name}: {nd} fields differ, worst {worst}, first at field {first}");
                            }
                        }
                        if worst > s.worst {
                            s.worst = worst;
                            s.worst_track = name.clone();
                        }
                    }
                }
            });
        }
    });
    let scores = scores.into_inner().unwrap();
    for f in failures.lock().unwrap().iter() {
        println!("FAILED {f}");
    }
    println!(
        "{:<5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>10}  worst track",
        "tag", "tracks", "exact", "±1", "len≠", "worst", "diff%"
    );
    let mut out = serde_json::Map::new();
    for ((name, _), s) in TAGS.iter().zip(&scores) {
        if s.tracks == 0 {
            continue;
        }
        let pct = if s.total_fields > 0 {
            100.0 * s.diff_fields as f64 / s.total_fields as f64
        } else {
            0.0
        };
        println!(
            "{name:<5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>9.5}%  {}",
            s.tracks, s.exact, s.within1, s.length_mismatch, s.worst, pct, s.worst_track
        );
        out.insert(
            (*name).to_string(),
            serde_json::json!({
                "tracks": s.tracks, "byte_exact": s.exact, "within_1": s.within1,
                "length_mismatch": s.length_mismatch, "worst_field_diff": s.worst,
                "differing_fields": s.diff_fields, "total_fields": s.total_fields,
            }),
        );
    }
    if let Some(path) = json {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::Value::Object(out))? + "\n",
        )?;
    }
    Ok(())
}
