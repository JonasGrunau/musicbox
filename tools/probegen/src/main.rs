//! Generates copyright-free probe audio with known ground truth (tempo, beat phase, key, bands),
//! then transcodes selected probes into every format rekordbox decodes differently.
//!
//! `cargo run -p probegen --release -- <out_dir>` writes `<out_dir>/<format>/<probe>.<ext>` and
//! `<out_dir>/probes.json`. Output is deterministic; ffmpeg must be on PATH for the transcodes.

mod signal;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use signal::{Synth, SR};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize)]
struct Probe {
    name: String,
    seconds: f64,
    /// Tempo segments as (start_seconds, bpm); first beat of each segment is a downbeat.
    tempo: Vec<(f64, f64)>,
    first_beat_seconds: Option<f64>,
    key: Option<String>,
    notes: String,
}

fn probes() -> Vec<(Probe, Vec<f32>)> {
    let mut out = Vec::new();
    let mut push = |name: &str, notes: &str, tempo: Vec<(f64, f64)>, first: Option<f64>, key: Option<&str>, synth: Synth| {
        let seconds = synth.seconds();
        out.push((
            Probe {
                name: name.into(),
                seconds,
                tempo,
                first_beat_seconds: first,
                key: key.map(Into::into),
                notes: notes.into(),
            },
            synth.into_samples(),
        ));
    };

    for bpm in [75.0, 90.0, 120.0, 128.0, 140.5, 174.0, 200.0] {
        let mut s = Synth::new(60.0);
        s.drums(0.0, 60.0, bpm, 0.0);
        push(&format!("click_{bpm}"), "kick on every beat, accented downbeat, offbeat hats", vec![(0.0, bpm)], Some(0.0), None, s);
    }
    {
        let mut s = Synth::new(60.0);
        s.drums(1.234, 60.0, 128.0, 0.0);
        push("click_128_offset1234", "first beat at 1.234 s after silence", vec![(1.234, 128.0)], Some(1.234), None, s);
    }
    {
        let mut s = Synth::new(64.0);
        let end_a = s.drums(0.0, 32.0, 120.0, 0.0);
        s.drums(end_a, 64.0, 126.0, 0.0);
        push("tempo_120_to_126", "120 BPM for 16 bars, then 126 BPM from the next downbeat", vec![(0.0, 120.0), (end_a, 126.0)], Some(0.0), None, s);
    }
    for (key, roots) in [
        ("Am", [(57, false), (53, true), (48, true), (55, true)]),
        ("C", [(48, true), (55, true), (57, false), (53, true)]),
        ("F#m", [(54, false), (50, true), (45, true), (52, true)]),
        ("Eb", [(51, true), (46, true), (48, false), (56, true)]),
    ] {
        let mut s = Synth::new(64.0);
        for bar in 0..32 {
            let (root, major) = roots[bar % 4];
            s.chord(bar as f64 * 2.0, 2.0, root, major);
        }
        s.drums(0.0, 64.0, 120.0, 0.4);
        push(&format!("chords_{}", key.replace('#', "s")), "i-VI-III-VII style progression, saw pads, soft drums at 120", vec![(0.0, 120.0)], Some(0.0), Some(key), s);
    }
    {
        let mut s = Synth::new(30.0);
        s.tone(0.0, 10.0, 100.0, 0.5);
        s.tone(10.0, 20.0, 1000.0, 0.5);
        s.tone(20.0, 30.0, 8000.0, 0.5);
        push("bands_100_1k_8k", "10 s each of 100 Hz, 1 kHz, 8 kHz at -6 dBFS", vec![], None, None, s);
    }
    {
        let mut s = Synth::new(30.0);
        s.sweep(0.0, 30.0, 20.0, 20000.0, 0.5);
        push("sweep_20_20k", "log sine sweep at -6 dBFS", vec![], None, None, s);
    }
    {
        let mut s = Synth::new(30.0);
        s.tone(0.0, 10.0, 1000.0, 1.0);
        s.tone(10.0, 20.0, 1000.0, 0.25);
        s.tone(20.0, 30.0, 1000.0, 0.0625);
        push("levels_0_-12_-24", "1 kHz at 0, -12, -24 dBFS for loudness/gain", vec![], None, None, s);
    }
    {
        let mut s = Synth::new(20.0);
        s.noise(0.0, 10.0, 0.25, false);
        s.noise(10.0, 20.0, 0.25, true);
        push("noise_white_pink", "white then pink noise at -12 dBFS", vec![], None, None, s);
    }
    out
}

/// (directory, extension, ffmpeg args) — every decode path rekordbox has a separate reader for.
const VARIANTS: &[(&str, &str, &[&str])] = &[
    ("aiff_44k16", "aiff", &["-c:a", "pcm_s16be"]),
    ("wav_48k24", "wav", &["-ar", "48000", "-c:a", "pcm_s24le"]),
    ("wav_96k32f", "wav", &["-ar", "96000", "-c:a", "pcm_f32le"]),
    ("wav_22k16_mono", "wav", &["-ar", "22050", "-ac", "1", "-c:a", "pcm_s16le"]),
    ("flac_44k16", "flac", &["-c:a", "flac"]),
    ("flac_48k24", "flac", &["-ar", "48000", "-c:a", "flac", "-sample_fmt", "s32"]),
    ("mp3_cbr320", "mp3", &["-c:a", "libmp3lame", "-b:a", "320k"]),
    ("mp3_vbr_v0", "mp3", &["-c:a", "libmp3lame", "-q:a", "0"]),
    ("mp3_cbr128_noxing", "mp3", &["-c:a", "libmp3lame", "-b:a", "128k", "-write_xing", "0"]),
    ("m4a_aac256", "m4a", &["-c:a", "aac_at", "-b:a", "256k"]),
    ("m4a_alac", "m4a", &["-c:a", "alac_at"]),
];

/// Probes transcoded into every variant; the rest stay 44.1 kHz/16-bit WAV only.
const TRANSCODED: &[&str] = &["click_128_offset1234", "chords_Am", "sweep_20_20k", "tempo_120_to_126"];

fn main() -> Result<()> {
    let out_dir = PathBuf::from(std::env::args().nth(1).context("usage: probegen <out_dir>")?);
    let base = out_dir.join("wav_44k16");
    std::fs::create_dir_all(&base)?;
    let mut manifest = Vec::new();
    for (probe, samples) in probes() {
        let path = base.join(format!("{}.wav", probe.name));
        write_wav(&path, &samples)?;
        if TRANSCODED.contains(&probe.name.as_str()) {
            for (dir, ext, args) in VARIANTS {
                let dst_dir = out_dir.join(dir);
                std::fs::create_dir_all(&dst_dir)?;
                transcode(&path, &dst_dir.join(format!("{}.{ext}", probe.name)), args)?;
            }
        }
        manifest.push(probe);
    }
    std::fs::write(out_dir.join("probes.json"), serde_json::to_string_pretty(&manifest)?)?;
    println!("{} probes written to {}", manifest.len(), out_dir.display());
    Ok(())
}

fn write_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec { channels: 2, sample_rate: SR, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        writer.write_sample(v)?;
        writer.write_sample(v)?;
    }
    writer.finalize()?;
    Ok(())
}

fn transcode(src: &Path, dst: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-map_metadata", "-1"])
        .args(args)
        .arg(dst)
        .status()
        .context("running ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg failed for {}", dst.display());
    }
    Ok(())
}
