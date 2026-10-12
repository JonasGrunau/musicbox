//! The playback engine driven without an audio device: exact samples from a cue, pause, seek, the
//! clock, end of track and device-rate conversion. Uses a WAV written by the test.

use mb_player::{Engine, Renderer};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const RATE: u32 = 44100;
const LEN: usize = 100_000;

/// Sample value at position `i`: never zero, so silence (underrun / pause) is recognisable.
fn value(i: usize) -> i16 {
    ((i % 20_000) + 1) as i16
}

fn test_wav() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("mb-player-test-{}-{n}.wav", std::process::id()));
    let mut data = Vec::with_capacity(LEN * 4);
    for i in 0..LEN {
        data.extend_from_slice(&value(i).to_le_bytes());
        data.extend_from_slice(&(-value(i)).to_le_bytes());
    }
    let mut out = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    let len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&len.to_le_bytes());
    std::fs::write(&path, out).unwrap();
    path
}

fn engine(device_rate: u32) -> (Engine, Renderer) {
    let path = test_wav();
    let decoder = mb_audio::open(&path).unwrap();
    // The open file handle keeps the data readable.
    let _ = std::fs::remove_file(&path);
    Engine::new(decoder, device_rate)
}

/// Renders until `n` non-silent stereo frames were produced (skipping underruns).
fn collect(r: &mut Renderer, n: usize) -> Vec<[f32; 2]> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    let mut buf = vec![0.0f32; 256];
    while got.len() < n {
        assert!(Instant::now() < deadline, "timed out after {} frames", got.len());
        r.render(&mut buf, 2);
        got.extend(buf.chunks(2).filter(|f| f[0] != 0.0).map(|f| [f[0], f[1]]));
        if buf.iter().all(|&v| v == 0.0) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    got.truncate(n);
    got
}

fn expected(from: usize, n: usize) -> Vec<[f32; 2]> {
    (from..from + n).map(|i| [f32::from(value(i)) / 32768.0, -f32::from(value(i)) / 32768.0]).collect()
}

#[test]
fn play_from_cue_starts_on_the_exact_sample() {
    let (e, mut r) = engine(RATE);
    e.play_from_sample(12_345);
    assert_eq!(collect(&mut r, 3000), expected(12_345, 3000));
    // The clock covers whole render blocks (128 frames here).
    let pos = e.clock().position_samples();
    assert!((12_345 + 3000..12_345 + 3000 + 128).contains(&pos), "{pos}");
}

#[test]
fn pause_is_silent_and_stops_the_clock() {
    let (e, mut r) = engine(RATE);
    e.play();
    collect(&mut r, 1000);
    e.pause();
    let at = e.clock().position_samples();
    let mut buf = vec![1.0f32; 512];
    r.render(&mut buf, 2);
    assert!(buf.iter().all(|&v| v == 0.0));
    assert_eq!(e.clock().position_samples(), at);
    e.play();
    let next = collect(&mut r, 100);
    assert_eq!(next, expected(at as usize, 100));
}

#[test]
fn seek_jumps_clock_and_stream() {
    let (e, mut r) = engine(RATE);
    e.set_volume(0.5);
    e.play();
    collect(&mut r, 500);
    e.seek_ms(1000.0);
    assert_eq!(e.clock().position_samples(), 44_100);
    assert_eq!(e.clock().position_ms(), 1000.0);
    let got = collect(&mut r, 200);
    let want: Vec<[f32; 2]> = expected(44_100, 200).iter().map(|s| [s[0] * 0.5, s[1] * 0.5]).collect();
    assert_eq!(got, want);
}

#[test]
fn end_of_track_finishes() {
    let (e, mut r) = engine(RATE);
    e.play_from_sample(LEN as u64 - 300);
    collect(&mut r, 300);
    let mut buf = vec![0.0f32; 1024];
    for _ in 0..4 {
        r.render(&mut buf, 2);
    }
    let clock = e.clock();
    assert!(clock.finished());
    assert!(!clock.is_playing());
    assert_eq!(clock.position_samples(), LEN as u64);
}

#[test]
fn device_rate_conversion_advances_the_clock_at_track_rate() {
    let (e, mut r) = engine(48_000);
    e.play();
    collect(&mut r, 48_000);
    // 48000 device frames cover 44100 track samples (± the samples rendered in the last block).
    let pos = e.clock().position_samples() as i64;
    assert!((pos - 44_100).abs() < 300, "{pos}");
}
