//! Runs the phrase stages on one file and prints intermediate results (development aid).
//!   cargo run --release -p mb-analysis --example phrase_debug -- <audio file>

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).expect("file"));
    let opts = mb_analysis::Options {
        waveforms: false,
        loudness: false,
        beats: true,
        phrase: false,
        key: false,
    };
    let a = mb_analysis::analyze(&path, opts).expect("analyze");
    let grid = a.beats.expect("grid");
    let info = *mb_audio::open(&path).unwrap().info();
    let total = info.total_samples - 1;
    let mut p = mb_analysis::phrase::PhraseAnalyzer::new(info.sample_rate, total);
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
            pos += got as u64;
        }
    }
    let (bpm100, fb, fbar) = mb_analysis::phrase::grid_anchor(&grid).unwrap();
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
