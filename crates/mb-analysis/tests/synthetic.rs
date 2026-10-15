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
fn zoom_view_builds_in_order() {
    // Kicks every 480 ms (125 BPM): 60 Hz bursts decaying over about 100 ms, 5.3 s long.
    let rate = 44100;
    let kick = |i: usize| {
        let t = (i % 21168) as f64 / f64::from(rate);
        let v = (2.0 * std::f64::consts::PI * 60.0 * t).sin() * (-t * 30.0).exp() * 20000.0;
        (v as i16, v as i16)
    };
    let p = wav("kicks.wav", rate, 5.3, kick);
    let mut chunks = Vec::new();
    mb_analysis::view::build_zoom_view(&*p, |ms| {
        chunks.push(ms.to_vec());
        std::ops::ControlFlow::Continue(())
    })
    .unwrap();
    // Two-second blocks: the start arrives before the whole track is read.
    assert!(chunks.len() >= 3, "{} chunks", chunks.len());
    let built: Vec<_> = chunks.concat();

    // The same as record A of the whole track, built in one go.
    let len = (5.3 * f64::from(rate)) as usize;
    let mut wave = mb_analysis::wavecreator::WaveCreator::new_view(rate, len as u64);
    let pcm: Vec<[i16; 2]> = (0..len - 1).map(|i| kick(i).into()).collect();
    for block in pcm.chunks(2 * rate as usize) {
        wave.put(block);
    }
    let whole = mb_analysis::view::ZoomView::from_record_a(wave.record_a());
    assert_eq!(built.len(), whole.ms.len());
    assert!(built == whole.ms);
    // A kick is low and loud; 400 ms later it has died away.
    let first = built[..20].iter().map(|m| m.peak).max().unwrap();
    assert!(first > 15000 && built[10].bands[0] > built[10].bands[2], "{first} {:?}", built[10]);
    assert!(built[400].peak < 200, "{:?}", built[400]);

    // Stopping early.
    let mut calls = 0;
    mb_analysis::view::build_zoom_view(&*p, |_| {
        calls += 1;
        std::ops::ControlFlow::Break(())
    })
    .unwrap();
    assert_eq!(calls, 1);
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
        beats: false,
        phrase: false,
        key: false,
        high_precision: None,
        vocal: false,
        ..Default::default()
    };
    let l = mb_analysis::analyze(&*p, opts).unwrap().loudness.unwrap();
    assert!((l.lufs - -20.0).abs() < 0.1, "{l:?}");
    assert!((l.gain_db - (-10.0 - l.lufs)).abs() < 1e-12);
    assert!((f64::from(l.peak) - amp / 32768.0).abs() < 1e-3, "{l:?}");
    let (gh, gl, _, _) = l.mixer_param();
    assert_eq!(f32::from_bits(u32::from(gh) << 16 | u32::from(gl)), l.gain);
}

/// Kicks on every beat (accented downbeats) and offbeat hats at 128 BPM after 1.234 s of
/// silence, synthesised like the `click_128_offset1234` probe: BALib's grid is 128.00 BPM, steps
/// 468.75 ms and has a beat on the first kick.
#[test]
fn click_track_grid() {
    let rate = 44100u32;
    let (first, period, secs) = (1.234, 60.0 / 128.0, 40.0);
    let n = (secs * f64::from(rate)) as usize;
    let mut buf = vec![0f64; n];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut white = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let idx = |t: f64| (t * f64::from(rate)).round() as usize;
    let mut beat = 0u64;
    loop {
        let t = first + beat as f64 * period;
        if t >= secs {
            break;
        }
        let amp = 0.6 * if beat.is_multiple_of(4) { 1.41 } else { 1.0 };
        let mut phase = 0.0;
        for i in 0..idx(0.25) {
            let x = i as f64 / f64::from(rate);
            phase += std::f64::consts::TAU * (50.0 + 100.0 * (-x * 40.0).exp()) / f64::from(rate);
            if let Some(s) = buf.get_mut(idx(t) + i) {
                *s += amp * phase.sin() * (-x * 18.0).exp();
            }
        }
        let mut prev = 0.0;
        for i in 0..idx(0.04) {
            let w = white();
            let x = i as f64 / f64::from(rate);
            if let Some(s) = buf.get_mut(idx(t + period / 2.0) + i) {
                *s += 0.15 * (w - prev) * (-x * 90.0).exp();
            }
            prev = w;
        }
        beat += 1;
    }
    let p = wav("click128.wav", rate, secs, |i| {
        let v = (buf[i] * 32767.0).clamp(-32768.0, 32767.0) as i16;
        (v, v)
    });
    let opts = mb_analysis::Options {
        waveforms: false,
        loudness: false,
        beats: true,
        phrase: true,
        key: false,
        high_precision: None,
        vocal: false,
        ..Default::default()
    };
    let g = mb_analysis::analyze(&*p, opts).unwrap().beats.unwrap();
    assert_eq!(g.bpm, 12800);
    assert!(g.beats.iter().all(|b| b.tempo == 12800));
    for w in g.beats.windows(2) {
        let d = w[1].time_ms - w[0].time_ms;
        assert!((468..=469).contains(&d), "gap {d}");
        assert_eq!(w[1].beat_in_bar, w[0].beat_in_bar % 4 + 1);
    }
    assert!(
        g.beats
            .iter()
            .any(|b| (i64::from(b.time_ms) - 1234).abs() <= 15),
        "no beat near the first kick: {:?}",
        &g.beats[..4]
    );
}
