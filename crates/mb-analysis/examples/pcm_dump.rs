//! Writes a track's decoded analysis-rate samples for offline comparison tools:
//! `b"PCM1"`, rate (u32), length L (u64), then L stereo `f32` frames (little endian).
//!
//! `cargo run --release -p mb-analysis --example pcm_dump -- <audio> <out.bin>`

use std::io::Write;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = args.as_slice() else {
        anyhow::bail!("usage: pcm_dump <audio> <out.bin>");
    };
    let mut dec = mb_audio::open(input)?;
    let info = *dec.info();
    let frames = dec.decode_all();
    let mut out = std::io::BufWriter::new(std::fs::File::create(output)?);
    out.write_all(b"PCM1")?;
    out.write_all(&info.sample_rate.to_le_bytes())?;
    out.write_all(&(frames.len() as u64).to_le_bytes())?;
    for f in &frames {
        out.write_all(&f[0].to_le_bytes())?;
        out.write_all(&f[1].to_le_bytes())?;
    }
    out.flush()?;
    eprintln!("{} frames at {} Hz (info length {})", frames.len(), info.sample_rate, info.total_samples);
    Ok(())
}
