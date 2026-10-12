//! Readers on files written by the test itself (no fixtures needed): PCM WAV/AIFF values and
//! layouts, the reader choice per layout, and the resampled length.

use mb_audio::{Backend, Format};
use std::path::PathBuf;

/// A temp file path, removed when the guard drops.
struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl std::ops::Deref for Tmp {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

fn tmp(name: &str) -> Tmp {
    Tmp(std::env::temp_dir().join(format!("mb-audio-synth-{}-{name}", std::process::id())))
}

/// RIFF/WAVE with one `fmt ` and one `data` chunk (optionally WAVE_FORMAT_EXTENSIBLE).
fn wav(rate: u32, channels: u16, bits: u16, tag: u16, frames: &[Vec<i32>], extensible: bool) -> Vec<u8> {
    let bps = usize::from(bits / 8);
    let mut data = Vec::new();
    for f in frames {
        for &s in f {
            data.extend_from_slice(&s.to_le_bytes()[..bps]);
        }
    }
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&(if extensible { 0xfffe } else { tag }).to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
    fmt.extend_from_slice(&(channels * bits / 8).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(&3u32.to_le_bytes());
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
    }
    let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    let len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&len.to_le_bytes());
    out
}

fn aiff(rate_ext: [u8; 10], channels: u16, bits: u16, frames: &[Vec<i32>]) -> Vec<u8> {
    let bps = usize::from(bits / 8);
    let mut ssnd = vec![0u8; 8];
    for f in frames {
        for &s in f {
            ssnd.extend_from_slice(&s.to_be_bytes()[4 - bps..]);
        }
    }
    let mut out = b"FORM\0\0\0\0AIFF".to_vec();
    out.extend_from_slice(b"COMM");
    out.extend_from_slice(&18u32.to_be_bytes());
    out.extend_from_slice(&channels.to_be_bytes());
    out.extend_from_slice(&(frames.len() as u32).to_be_bytes());
    out.extend_from_slice(&bits.to_be_bytes());
    out.extend_from_slice(&rate_ext);
    out.extend_from_slice(b"SSND");
    out.extend_from_slice(&(ssnd.len() as u32).to_be_bytes());
    out.extend_from_slice(&ssnd);
    let len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&len.to_be_bytes());
    out
}

fn ramp(n: usize, channels: usize, max: i32) -> Vec<Vec<i32>> {
    (0..n).map(|i| (0..channels).map(|c| ((i as i32 * 37 + c as i32 * 1000) % (2 * max)) - max).collect()).collect()
}

#[test]
fn wav_16_bit_stereo_is_exact() {
    let frames = ramp(5000, 2, 32768);
    let path = tmp("s16.wav");
    std::fs::write(&*path, wav(44100, 2, 16, 1, &frames, false)).unwrap();
    let mut d = mb_audio::open(&*path).unwrap();
    assert_eq!((d.info().format, d.info().backend), (Format::Wav, Backend::FileReadWav));
    assert_eq!(d.info().total_samples, 5000);
    let pcm = d.decode_all();
    for (f, s) in frames.iter().zip(&pcm) {
        assert_eq!(*s, [f[0] as f32 / 32768.0, f[1] as f32 / 32768.0]);
    }
}

#[test]
fn wav_24_bit_mono_is_duplicated() {
    let frames = ramp(3000, 1, 1 << 23);
    let path = tmp("s24m.wav");
    std::fs::write(&*path, wav(48000, 1, 24, 1, &frames, false)).unwrap();
    let mut d = mb_audio::open(&*path).unwrap();
    assert_eq!(d.info().backend, Backend::FileReadWav);
    let pcm = d.decode_all();
    for (f, s) in frames.iter().zip(&pcm) {
        let v = f[0] as f32 / 8_388_608.0;
        assert_eq!(*s, [v, v]);
    }
}

#[test]
fn extensible_and_float_wav_use_juce() {
    let frames = ramp(2000, 2, 1 << 23);
    let path = tmp("ext24.wav");
    std::fs::write(&*path, wav(48000, 2, 24, 1, &frames, true)).unwrap();
    let mut d = mb_audio::open(&*path).unwrap();
    assert_eq!(d.info().backend, Backend::Juce);
    let pcm = d.decode_all();
    assert_eq!(pcm[1234], [frames[1234][0] as f32 / 8_388_608.0, frames[1234][1] as f32 / 8_388_608.0]);

    let floats: Vec<Vec<i32>> = (0..1000).map(|i| vec![(i as f32 / 1000.0).to_bits() as i32, (-0.25f32).to_bits() as i32]).collect();
    let path = tmp("f32.wav");
    std::fs::write(&*path, wav(44100, 2, 32, 3, &floats, false)).unwrap();
    let mut d = mb_audio::open(&*path).unwrap();
    assert_eq!(d.info().backend, Backend::Juce);
    assert_eq!(d.decode_all()[500], [0.5, -0.25]);
}

#[test]
fn aiff_16_bit_is_exact() {
    let frames = ramp(4000, 2, 32768);
    let path = tmp("s16.aiff");
    std::fs::write(&*path, aiff([0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0], 2, 16, &frames)).unwrap();
    let mut d = mb_audio::open(&*path).unwrap();
    assert_eq!((d.info().format, d.info().backend), (Format::Aiff, Backend::FileReadAiff));
    assert_eq!((d.info().sample_rate, d.info().total_samples), (44100, 4000));
    let pcm = d.decode_all();
    for (f, s) in frames.iter().zip(&pcm) {
        assert_eq!(*s, [f[0] as f32 / 32768.0, f[1] as f32 / 32768.0]);
    }
}

#[test]
fn rates_outside_44_1_and_48_are_resampled() {
    let frames = ramp(10_000, 2, 30000);
    for (rate, out, len) in [(22050u32, 44100u32, 20_000u64), (96000, 48000, 5000), (32000, 48000, 15_000)] {
        let path = tmp(&format!("r{rate}.wav"));
        std::fs::write(&*path, wav(rate, 2, 16, 1, &frames, false)).unwrap();
        let d = mb_audio::open(&*path).unwrap();
        assert_eq!(d.info().file_rate, rate);
        assert_eq!(d.info().sample_rate, out);
        assert_eq!(d.info().total_samples, len, "{rate}");
    }
}

#[test]
fn analysis_rate_rule() {
    assert_eq!(mb_audio::analysis_rate(44100), 44100);
    assert_eq!(mb_audio::analysis_rate(48000), 48000);
    assert_eq!(mb_audio::analysis_rate(96000), 48000);
    assert_eq!(mb_audio::analysis_rate(32000), 48000);
    assert_eq!(mb_audio::analysis_rate(88200), 44100);
    assert_eq!(mb_audio::analysis_rate(22050), 44100);
    // 12 kHz is not a multiple of 8 kHz.
    assert_eq!(mb_audio::analysis_rate(12000), 44100);
}

#[test]
fn analysis_i16_truncates_and_clamps() {
    use mb_audio::analysis_i16;
    assert_eq!(analysis_i16(1.0), 32767);
    assert_eq!(analysis_i16(-1.5), -32768);
    assert_eq!(analysis_i16(0.5 / 32768.0 + 3.0 / 32768.0), 3);
    assert_eq!(analysis_i16(-3.9 / 32768.0), -3);
    assert_eq!(analysis_i16(f32::NAN), -32768);
}
