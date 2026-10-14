//! Runs the phrase stages on one file and prints intermediate results (development aid).
//!   cargo run --release -p mb-analysis --example phrase_debug -- <audio file> [--dump <out>]
//! `--dump` writes the thread's `Initialize` arguments and the int16 stream it feeds
//! (`analysis-phrase.md` § 1.4): "SSD1", rate u32, total u64, bpm100/fb/fbar/key i32, then
//! `total` frames of L, R i16 (little endian).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(args.get(1).expect("file"));
    let dump = args.iter().position(|a| a == "--dump").and_then(|i| args.get(i + 1)).cloned();
    let cout = args.iter().position(|a| a == "--cout").and_then(|i| args.get(i + 1)).cloned();
    let opts = mb_analysis::Options {
        waveforms: false,
        loudness: false,
        beats: true,
        phrase: false,
        key: false,
        high_precision: None,
        vocal: false,
        ..Default::default()
    };
    let a = mb_analysis::analyze(&path, opts).expect("analyze");
    let grid = a.beats.expect("grid");
    let info = *mb_audio::open(&path).unwrap().info();
    let total = info.total_samples - 1;
    let mut p = mb_analysis::phrase::PhraseAnalyzer::new(info.sample_rate, total);
    let (bpm100, fb, fbar) = mb_analysis::phrase::grid_anchor(&grid).unwrap();
    let mut out = dump.map(|d| {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(d).expect("dump"));
        w.write_all(b"SSD1").unwrap();
        w.write_all(&info.sample_rate.to_le_bytes()).unwrap();
        w.write_all(&total.to_le_bytes()).unwrap();
        for v in [bpm100, fb as i32, fbar, 0] {
            w.write_all(&v.to_le_bytes()).unwrap();
        }
        w
    });
    {
        let mut dec = mb_audio::open(&path).unwrap();
        let mut frames = vec![[0.0f32; 2]; 1 << 16];
        let mut pos = 0u64;
        while pos < total {
            let n = ((total - pos) as usize).min(frames.len());
            let got = dec.read_frames(&mut frames[..n]);
            if got == 0 {
                break;
            }
            let pcm: Vec<[i16; 2]> = frames[..got]
                .iter()
                .map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])])
                .collect();
            p.put(&pcm);
            if let Some(w) = &mut out {
                use std::io::Write;
                for f in &pcm {
                    w.write_all(&f[0].to_le_bytes()).unwrap();
                    w.write_all(&f[1].to_le_bytes()).unwrap();
                }
            }
            pos += got as u64;
        }
    }
    drop(out);
    let down = p.downbeat(bpm100, fb as i32, fbar);
    println!("bpm {bpm100} fb {fb} fbar {fbar} -> {down:?}");
    let Some(down) = down else { return };
    let t = std::time::Instant::now();
    let c = mb_analysis::phrase::stage_c(&path, bpm100, down);
    println!("stage C {:?}", t.elapsed());
    let Some(c) = c else {
        println!("no stage C");
        return;
    };
    if let Some(file) = cout {
        write_cout(&c, &file).expect("cout");
    }
    let s = |v: &[f64]| v.iter().sum::<f64>();
    println!(
        "n {} bar {} nonzero {} cqt {}x{} first {} beats8[..4] {:?}",
        c.n,
        c.bar,
        c.nonzero,
        c.cqt.len(),
        c.cqt_frames,
        c.cqt_first,
        &c.beats8[..4.min(c.beats8.len())]
    );
    println!(
        "low_peak {:.6} low_level {:.6} low_power {:.6} low40 {:.6} mid_level {:.6} mid_peak {:.6}",
        s(&c.low_peak),
        s(&c.low_level),
        s(&c.low_power),
        s(&c.low40),
        s(&c.mid_level),
        s(&c.mid_peak)
    );
    println!(
        "all_level {:.6} all_power {:.6} all_peak {:.6} band {:.6}/{:.6} high {:.6} flags {:.3}",
        s(&c.all_level),
        s(&c.all_power),
        s(&c.all_peak),
        s(&c.band_lo),
        s(&c.band_hi),
        s(&c.high_peak),
        s(&c.low_flags)
    );
    println!(
        "hihat {} pattern {} pattern2 {} fft1 {} fft2 {}",
        c.hihat.iter().map(|&v| v as u32).sum::<u32>(),
        c.pattern.iter().map(|&v| v as u32).sum::<u32>(),
        c.pattern2.iter().map(|&v| v as u32).sum::<u32>(),
        c.fft1.len(),
        c.fft2.len()
    );
}

/// Writes stage C's outputs in the reference harness's text+binary layout (development aid).
fn write_cout(c: &mb_analysis::phrase::stage_c::COut, file: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut w = std::io::BufWriter::new(std::fs::File::create(file)?);
    writeln!(
        w,
        "S n {} bar {} n2 {} frames {} first {} nonzero {}",
        c.n, c.bar, c.n, c.cqt_frames, c.cqt_first, c.nonzero
    )?;
    let f = |w: &mut std::io::BufWriter<std::fs::File>, name: &str, v: &[f64]| {
        writeln!(w, "F {name} {}", v.len())?;
        for x in v {
            w.write_all(&x.to_le_bytes())?;
        }
        writeln!(w)
    };
    let g = |w: &mut std::io::BufWriter<std::fs::File>, name: &str, v: &[Vec<f64>]| {
        writeln!(w, "G {name} {} {}", v.len(), v.first().map_or(0, Vec::len))?;
        for x in v.iter().flatten() {
            w.write_all(&x.to_le_bytes())?;
        }
        writeln!(w)
    };
    let u = |w: &mut std::io::BufWriter<std::fs::File>, name: &str, v: &[u8]| {
        writeln!(w, "U {name} {}", v.len())?;
        w.write_all(v)?;
        writeln!(w)
    };
    f(&mut w, "low_power", &c.low_power)?;
    f(&mut w, "all_power", &c.all_power)?;
    f(&mut w, "all_peak", &c.all_peak)?;
    f(&mut w, "low_peak", &c.low_peak)?;
    g(&mut w, "fft1", &c.fft1)?;
    f(&mut w, "mid_peak", &c.mid_peak)?;
    f(&mut w, "low_flags", &c.low_flags)?;
    u(&mut w, "hihat", &c.hihat)?;
    u(&mut w, "pattern", &c.pattern)?;
    f(&mut w, "low_level", &c.low_level)?;
    u(&mut w, "pattern2", &c.pattern2)?;
    f(&mut w, "low_peak_q", &c.low_peak_q)?;
    f(&mut w, "mid_level", &c.mid_level)?;
    f(&mut w, "band_lo", &c.band_lo)?;
    f(&mut w, "band_hi", &c.band_hi)?;
    g(&mut w, "fft2", &c.fft2)?;
    f(&mut w, "low40", &c.low40)?;
    g(&mut w, "cqt", &c.cqt)?;
    f(&mut w, "all_level", &c.all_level)?;
    f(&mut w, "high_peak", &c.high_peak)?;
    writeln!(w, "I beats8 {}", c.beats8.len())?;
    for x in &c.beats8 {
        w.write_all(&x.to_le_bytes())?;
    }
    writeln!(w)
}
