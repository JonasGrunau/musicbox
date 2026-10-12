//! Decoded lengths against rekordbox's own analysis of the corpus (`fixtures/oracle-a`):
//! scrolling-waveform column counts (PWV3, 150/s), the PVBR sample count of MP3s, the PVB2 total of
//! FLACs and the PVBR VBR index. Skipped when the fixtures, the corpus or rekordbox's libmpg123 are
//! absent.

mod common;

use mb_audio::timeline::waveform_columns;
use mb_audio::{Format, mp3};

#[test]
fn lengths_match_rekordbox_analysis() {
    let Some(tracks) = common::oracle_tracks("oracle-a") else {
        eprintln!("oracle-a fixture missing, skipping");
        return;
    };
    if !mb_audio::mpg123::available() {
        eprintln!("rekordbox libmpg123 not installed, skipping");
        return;
    }
    let mut checked = 0;
    let mut failures = Vec::new();
    for t in &tracks {
        let Some(pwv3) = t.wave_scroll().map(|w| w.len() as u64) else { continue };
        let d = match mb_audio::open(&t.path) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{}: open failed: {e}", t.path.display()));
                continue;
            }
        };
        let info = *d.info();
        checked += 1;
        if i64::from(info.sample_rate) != t.sample_rate && info.file_rate as i64 != t.sample_rate {
            failures.push(format!("{}: rate {} vs db {}", t.path.display(), info.file_rate, t.sample_rate));
        }
        let cols = waveform_columns(info.total_samples, info.sample_rate);
        if cols != pwv3 {
            failures.push(format!("{}: {} samples → {cols} columns, PWV3 has {pwv3}", t.path.display(), info.total_samples));
        }
        if info.format == Format::Mp3 {
            let count = t.vbr().and_then(|v| v.sample_count).map(u64::from);
            if count != Some(info.total_samples) {
                failures.push(format!("{}: PVBR sample count {count:?} vs {}", t.path.display(), info.total_samples));
            }
        }
        if info.format == Format::Flac {
            let total = t.seek_table().map(|s| s.total_samples);
            if total != Some(info.file_samples) {
                failures.push(format!("{}: PVB2 total {total:?} vs {}", t.path.display(), info.file_samples));
            }
        }
    }
    eprintln!("checked {checked} tracks");
    assert!(checked > 150, "only {checked} tracks checked");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

/// PVBR's 400-entry index for Xing-tagged MP3s: byte offsets of frame ⌊n·i/400⌋−8 from the first
/// audio frame.
#[test]
fn pvbr_index_matches_rekordbox() {
    let Some(tracks) = common::oracle_tracks("oracle-a") else {
        eprintln!("oracle-a fixture missing, skipping");
        return;
    };
    let mut checked = 0;
    let mut failures = Vec::new();
    for t in tracks.iter().filter(|t| t.path.extension().is_some_and(|e| e == "mp3")) {
        let Some(vbr) = t.vbr() else { continue };
        let map = mp3::scan(&std::fs::read(&t.path).unwrap()).unwrap();
        let audio: Vec<_> = map.frames.iter().copied().filter(|f| f.pos >= map.music_start as i32).collect();
        let data = std::fs::read(&t.path).unwrap();
        // Skip an ID3v2 tag, then look for a Xing header in the first frame.
        let start = if data.starts_with(b"ID3") {
            10 + data[6..10].iter().fold(0usize, |a, &b| (a << 7) | usize::from(b & 0x7f))
        } else {
            0
        };
        let xing = data[start..(start + 2048).min(data.len())].windows(4).any(|w| w == b"Xing");
        let ours = if xing { mp3::pvbr_index(&audio) } else { vec![0; 400] };
        checked += 1;
        if ours != vbr.index {
            let first = ours.iter().zip(&vbr.index).position(|(a, b)| a != b).unwrap();
            failures.push(format!(
                "{}: entry {first}: ours {} rekordbox {}",
                t.path.display(),
                ours[first],
                vbr.index[first]
            ));
        }
    }
    eprintln!("checked {checked} MP3 PVBR tables");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}
