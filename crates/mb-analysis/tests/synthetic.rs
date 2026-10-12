//! Waveform properties that need no fixtures: lengths, silence, quantiser tables.

use mb_analysis::loud::{dot_class, v_devide};
use std::path::PathBuf;

/// Writes a 16-bit stereo WAV of `secs` seconds at `rate` with `f(i) -> (l, r)`.
fn wav(name: &str, rate: u32, secs: f64, f: impl Fn(usize) -> (i16, i16)) -> TempWav {
    let n = (secs * f64::from(rate)) as usize;
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let (l, r) = f(i);
        data.extend_from_slice(&l.to_le_bytes());
        data.extend_from_slice(&r.to_le_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    let dir = std::env::temp_dir().join(format!("mb-analysis-synth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, out).unwrap();
    TempWav(p)
}

/// A temporary WAV file, deleted when dropped.
struct TempWav(PathBuf);

impl std::ops::Deref for TempWav {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        // Removes the directory once the last file is gone.
        let _ = self.0.parent().map(std::fs::remove_dir);
    }
}

#[test]
fn silence_gives_empty_waveforms() {
    let p = wav("silence.wav", 44100, 3.0, |_| (0, 0));
    let w = mb_analysis::analyze_waveforms(&*p).unwrap();
    // 3 s at 150 columns per second, counted over L − 1 samples.
    assert_eq!(w.pwv3.len(), 450);
    // Height 0, whiteness 7 (no signal at all).
    assert!(w.pwv3.iter().all(|&b| b == 0xe0), "{:?}", &w.pwv3[..8]);
    assert_eq!(w.pwv5.len(), 450);
    assert_eq!(w.pwv7.len(), 450);
    assert_eq!(w.pwv4.len(), 1200);
    assert_eq!(w.pwv6.len(), 1200);
    assert_eq!(w.pwav.len(), 400);
    assert_eq!(w.pwv2.len(), 100);
    assert!(w.pwv4.iter().all(|p| p == &[0; 6]));
    assert!(w.pwv6.iter().all(|p| p == &[0; 3]));
    // No band carries energy: every band gain stays 1.0, stored as 100 %.
    assert_eq!(w.pwvc, [100, 100, 100]);
}

#[test]
fn full_scale_tone_reaches_the_top() {
    // 1 kHz at about -0.1 dBFS on both channels.
    let p = wav("tone.wav", 48000, 4.0, |i| {
        let v = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48000.0).sin() * 32400.0;
        (v as i16, v as i16)
    });
    let w = mb_analysis::analyze_waveforms(&*p).unwrap();
    assert_eq!(w.pwv3.len(), 600);
    // Normalised to the track peak, the steady part of PWV3 is at full height.
    assert!(
        w.pwv3[10..590].iter().all(|&b| b & 0x1f >= 30),
        "{:?}",
        &w.pwv3[10..20]
    );
    // A 1 kHz tone is mid band in the 3-band waveforms.
    let mid = w.pwv6[600];
    assert!(mid[1] > mid[0] && mid[1] > mid[2], "{mid:?}");
}

#[test]
fn quantiser_tables() {
    assert_eq!(v_devide(0.0), 2);
    assert_eq!(v_devide(f32::NAN), 2);
    assert_eq!(v_devide(2195.456), 3);
    assert_eq!(v_devide(4096.0), 5);
    assert_eq!(v_devide(60000.0), 25);
    assert_eq!(dot_class(0.0), 1);
    assert_eq!(dot_class(1.0), 2);
    assert_eq!(dot_class(1304.0), 3);
    assert_eq!(dot_class(20675.0), 15);
}

#[test]
fn sine_loudness() {
    // 997 Hz at -20 dBFS on both channels: about -20 + 3 - 0.691 + 0.69 (K-weighting at 1 kHz) LUFS.
    let amp = 32768.0 * 10f64.powf(-20.0 / 20.0);
    let p = wav("sine.wav", 48000, 20.0, |i| {
        let v = (2.0 * std::f64::consts::PI * 997.0 * i as f64 / 48000.0).sin() * amp;
        (v as i16, v as i16)
    });
    let opts = mb_analysis::Options {
        waveforms: false,
        loudness: true,
    };
    let l = mb_analysis::analyze(&*p, opts).unwrap().loudness.unwrap();
    assert!((l.lufs - -20.0).abs() < 0.1, "{l:?}");
    assert!((l.gain_db - (-10.0 - l.lufs)).abs() < 1e-12);
    assert!((f64::from(l.peak) - amp / 32768.0).abs() < 1e-3, "{l:?}");
    let (gh, gl, _, _) = l.mixer_param();
    assert_eq!(f32::from_bits(u32::from(gh) << 16 | u32::from(gl)), l.gain);
}
