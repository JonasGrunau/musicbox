//! Waveform parity with rekordbox's own analysis (`fixtures/oracle-b`, rekordbox 7.2.19).
//! By default the probes and the sampler loops are checked (fast in debug builds); set
//! `MB_ORACLE_FULL=1` for all 169 tracks. Skipped when the fixtures or the corpus are absent.
//!
//! Non-AAC tracks must be byte-exact in all eight tags. AAC tracks are decoded by the system's
//! AudioToolbox, whose PCM varies slightly between runs on macOS 27 (`analysis-waveforms.md`
//! § 6); they must match in length and within ±2 per field.

mod common;

use mb_anlz::Tag;

const TAGS: [&str; 8] = [
    "PWAV", "PWV2", "PWV3", "PWV4", "PWV5", "PWV6", "PWV7", "PWVC",
];

fn body(w: &mb_analysis::Waveforms, tag: &str) -> Vec<u8> {
    match tag {
        "PWAV" => w.pwav.clone(),
        "PWV2" => w.pwv2.clone(),
        "PWV3" => w.pwv3.clone(),
        "PWV4" => w.pwv4.concat(),
        "PWV5" => w.pwv5.iter().flat_map(|v| v.to_be_bytes()).collect(),
        "PWV6" => w.pwv6.concat(),
        "PWV7" => w.pwv7.concat(),
        _ => w.pwvc.iter().flat_map(|v| v.to_be_bytes()).collect(),
    }
}

/// Largest per-field difference (heights and colours unpacked where they share a byte).
fn worst(tag: &str, a: &[u8], b: &[u8]) -> i32 {
    let f = |x: &[u8]| -> Vec<i32> {
        match tag {
            "PWAV" | "PWV2" | "PWV3" => x
                .iter()
                .flat_map(|&v| [i32::from(v & 0x1f), i32::from(v >> 5)])
                .collect(),
            "PWV5" | "PWVC" => x
                .chunks_exact(2)
                .flat_map(|c| {
                    let v = u16::from_be_bytes([c[0], c[1]]);
                    if tag == "PWVC" {
                        vec![i32::from(v)]
                    } else {
                        [(v >> 13) & 7, (v >> 10) & 7, (v >> 7) & 7, (v >> 2) & 0x1f]
                            .map(i32::from)
                            .to_vec()
                    }
                })
                .collect(),
            _ => x.iter().map(|&v| i32::from(v)).collect(),
        }
    };
    f(a).iter()
        .zip(f(b))
        .map(|(x, y)| (x - y).abs())
        .max()
        .unwrap_or(0)
}

#[test]
fn waveforms_match_rekordbox() {
    let Some(tracks) = common::oracle_tracks("oracle-b") else {
        eprintln!("fixtures/oracle-b missing, skipping");
        return;
    };
    let full = std::env::var_os("MB_ORACLE_FULL").is_some();
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| {
            let p = t.path.to_string_lossy();
            full || p.contains("/re-work/corpus/probes/") || p.contains("/Sampler/")
        })
        .collect();
    if tracks.is_empty() {
        eprintln!("corpus missing, skipping");
        return;
    }
    if tracks
        .iter()
        .any(|t| t.path.extension().is_some_and(|e| e == "mp3"))
        && !mb_audio::mpg123::available()
    {
        eprintln!("rekordbox libmpg123 not installed, skipping");
        return;
    }
    let next = std::sync::Mutex::new(0usize);
    let failures = std::sync::Mutex::new(Vec::new());
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
                    let name = t.path.display().to_string();
                    let aac = mb_audio::open(&t.path)
                        .is_ok_and(|d| d.info().format == mb_audio::Format::Aac);
                    let w = match mb_analysis::analyze_waveforms(&t.path) {
                        Ok(w) => w,
                        Err(e) => {
                            failures.lock().unwrap().push(format!("{name}: {e}"));
                            continue;
                        }
                    };
                    for tag in TAGS {
                        let Some((_, theirs)) = t.section(Tag(tag.as_bytes().try_into().unwrap()))
                        else {
                            continue;
                        };
                        let mine = body(&w, tag);
                        if mine.len() != theirs.len() {
                            failures.lock().unwrap().push(format!(
                                "{name} {tag}: {} bytes vs {}",
                                mine.len(),
                                theirs.len()
                            ));
                        } else if mine != theirs {
                            let d = worst(tag, &mine, theirs);
                            if !aac || d > 2 {
                                failures
                                    .lock()
                                    .unwrap()
                                    .push(format!("{name} {tag}: worst field diff {d}"));
                            }
                        }
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    eprintln!("checked {} tracks", tracks.len());
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Auto-gain (`djmdMixerParam`): gain and sample peak as rekordbox's float bits. AAC gains may be
/// one float ulp off (`analysis-loudness.md` § 5).
#[test]
fn loudness_matches_rekordbox() {
    let Some(tracks) = common::oracle_tracks("oracle-b") else {
        eprintln!("fixtures/oracle-b missing, skipping");
        return;
    };
    let full = std::env::var_os("MB_ORACLE_FULL").is_some();
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| t.mixer.is_some())
        .filter(|t| {
            let p = t.path.to_string_lossy();
            full || p.contains("/re-work/corpus/probes/") || p.contains("/Sampler/")
        })
        .collect();
    if tracks.is_empty() || !mb_audio::mpg123::available() {
        eprintln!("corpus or libmpg123 missing, skipping");
        return;
    }
    let failures = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for chunk in tracks.chunks(tracks.len().div_ceil(threads)) {
            let failures = &failures;
            s.spawn(move || {
                for t in chunk {
                    let aac = mb_audio::open(&t.path)
                        .is_ok_and(|d| d.info().format == mb_audio::Format::Aac);
                    let opts = mb_analysis::Options {
                        waveforms: false,
                        loudness: true,
                    };
                    let l = mb_analysis::analyze(&t.path, opts)
                        .unwrap()
                        .loudness
                        .unwrap();
                    let m = t.mixer.unwrap();
                    let gain = ((m[0] as u32) << 16) | (m[1] as u32 & 0xffff);
                    let peak = ((m[2] as u32) << 16) | (m[3] as u32 & 0xffff);
                    let d = (i64::from(l.gain.to_bits()) - i64::from(gain)).abs();
                    if (d != 0 && !aac) || d > 1 || l.peak.to_bits() != peak {
                        let (g, p) = (f32::from_bits(gain), f32::from_bits(peak));
                        failures.lock().unwrap().push(format!(
                            "{}: gain {} vs {g}, peak {} vs {p}",
                            t.path.display(),
                            l.gain,
                            l.peak
                        ));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    eprintln!("checked {} tracks", tracks.len());
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
