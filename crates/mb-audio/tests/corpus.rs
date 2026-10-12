//! Decoding the probe corpus (`~/re-work/corpus/probes`, regenerable with `tools/probegen`):
//! timeline offsets per format against the WAV original, sample identity of the lossless paths,
//! and random access equal to sequential decoding. Skipped when the corpus or rekordbox's
//! libmpg123 are absent.

mod common;

use mb_audio::{Backend, Float2};
use std::path::PathBuf;

const PROBE: &str = "click_128_offset1234";

fn probe_path(dir: &str, ext: &str) -> Option<PathBuf> {
    let p = common::corpus()?.join("probes").join(dir).join(format!("{PROBE}.{ext}"));
    p.exists().then_some(p)
}

fn ready() -> bool {
    if common::corpus().is_none() {
        eprintln!("corpus missing, skipping");
        return false;
    }
    if !mb_audio::mpg123::available() {
        eprintln!("rekordbox libmpg123 not installed, skipping");
        return false;
    }
    true
}

fn decode(path: &PathBuf) -> (mb_audio::StreamInfo, Vec<Float2>) {
    let mut d = mb_audio::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let pcm = d.decode_all();
    (*d.info(), pcm)
}

/// Lag of `x` behind `reference` around the first onset, by cross-correlation.
fn lag(reference: &[Float2], x: &[Float2]) -> i64 {
    let m = |s: &Float2| f64::from(s[0]) + f64::from(s[1]);
    let onset = reference.iter().position(|s| m(s).abs() > 0.02).unwrap();
    let (a, b) = (onset.saturating_sub(500), onset + 6000);
    (-3000i64..=3000)
        .map(|l| {
            let s: f64 = (a..b)
                .filter_map(|i| {
                    let j = i as i64 + l;
                    (j >= 0 && (j as usize) < x.len()).then(|| m(&reference[i]) * m(&x[j as usize]))
                })
                .sum();
            (l, s)
        })
        .max_by(|p, q| p.1.total_cmp(&q.1))
        .unwrap()
        .0
}

/// Expected per format: (dir, ext, backend, rate, total samples, lag vs the WAV original at that
/// rate, lossless). The lengths are the ones rekordbox's PWV3 column counts imply.
const EXPECTED: [(&str, &str, Backend, u32, u64, i64, bool); 12] = [
    ("wav_44k16", "wav", Backend::FileReadWav, 44100, 2_646_000, 0, true),
    ("aiff_44k16", "aiff", Backend::FileReadAiff, 44100, 2_646_000, 0, true),
    ("flac_44k16", "flac", Backend::FileReadFlac, 44100, 2_646_000, 0, true),
    ("flac_48k24", "flac", Backend::FileReadFlac, 48000, 2_880_000, 0, true),
    ("wav_48k24", "wav", Backend::Juce, 48000, 2_880_000, 0, true),
    ("m4a_alac", "m4a", Backend::FileReadAlac, 44100, 2_650_112, 0, true),
    // AAC: Apple encoder priming (2112) is not trimmed.
    ("m4a_aac256", "m4a", Backend::FileReadMp4, 44100, 2_648_128, 2112, false),
    // MP3: LAME delay 576 + decoder delay 529; the ffmpeg Xing/Info frame is skipped by the scan.
    ("mp3_cbr320", "mp3", Backend::FileReadMp3, 44100, 2_647_296, 1105, false),
    ("mp3_vbr_v0", "mp3", Backend::FileReadMp3, 44100, 2_647_296, 1105, false),
    ("mp3_cbr128_noxing", "mp3", Backend::FileReadMp3, 44100, 2_647_296, 1105, false),
    // Resampled: the converter's group delay (14.5 input samples).
    ("wav_22k16_mono", "wav", Backend::FileReadWav, 44100, 2_646_000, 29, false),
    ("wav_96k32f", "wav", Backend::Juce, 48000, 2_880_000, 7, false),
];

#[test]
fn probe_timelines_per_format() {
    if !ready() {
        return;
    }
    let (_, ref44) = decode(&probe_path("wav_44k16", "wav").unwrap());
    let (_, ref48) = decode(&probe_path("wav_48k24", "wav").unwrap());
    for (dir, ext, backend, rate, total, want_lag, lossless) in EXPECTED {
        let Some(path) = probe_path(dir, ext) else { continue };
        let (info, pcm) = decode(&path);
        assert_eq!(info.backend, backend, "{dir}");
        assert_eq!(info.sample_rate, rate, "{dir}");
        assert_eq!(info.total_samples, total, "{dir}");
        assert_eq!(pcm.len() as u64, total, "{dir}");
        let reference = if rate == 44100 { &ref44 } else { &ref48 };
        assert_eq!(lag(reference, &pcm), want_lag, "{dir}");
        if lossless {
            let n = reference.len().min(pcm.len());
            assert!(reference[..n] == pcm[..n], "{dir}: lossless decode differs from the WAV original");
        }
    }
}

/// Reads at scattered positions (as a player seeking around) give the same samples as the
/// sequential decode. MP3 restarts five frames early and AAC resets the converter, the SRC
/// re-primes from 30 input samples. PCM, FLAC, ALAC and the resampler give identical samples;
/// the lossy decoders do not fully forget their state on a restart (mpg123's synthesis buffer
/// phase, Apple's AAC decoder after `AudioConverterReset`), so a few samples after a seek can
/// differ in the last bit. rekordbox's player seeks the same way; its analysis never seeks.
#[test]
fn random_access_matches_sequential() {
    if !ready() {
        return;
    }
    for (dir, ext, ..) in EXPECTED {
        let Some(path) = probe_path(dir, ext) else { continue };
        let (info, seq) = decode(&path);
        let mut d = mb_audio::open(&path).unwrap();
        let total = info.total_samples as i64;
        let tolerance = if dir.starts_with("m4a_aac") || dir.starts_with("mp3") { 4.0 / 32768.0 } else { 0.0 };
        for &pos in &[total / 2, 123_457, total - 5000, 1_000_000, 60_001, total + 10] {
            let mut buf = vec![[1.0f32; 2]; 4000];
            d.read_at(pos, &mut buf);
            for (k, s) in buf.iter().enumerate() {
                let p = pos + k as i64;
                let want = if (0..total).contains(&p) { seq[p as usize] } else { [0.0; 2] };
                let diff = (s[0] - want[0]).abs().max((s[1] - want[1]).abs());
                assert!(diff <= tolerance, "{dir}: sample {p} after seeking to {pos}: {s:?} vs {want:?}");
            }
        }
    }
}

/// The analysis quantisation of the decoded stream: 16-bit sources round-trip exactly.
#[test]
fn analysis_i16_is_identity_for_16_bit_sources() {
    if !ready() {
        return;
    }
    let (_, pcm) = decode(&probe_path("wav_44k16", "wav").unwrap());
    for s in pcm.iter().step_by(37) {
        for c in s {
            assert_eq!(f32::from(mb_audio::analysis_i16(*c)) / 32768.0, *c);
        }
    }
}
