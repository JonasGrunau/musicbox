//! Decodes files with `mb_audio` and prints a hash of the PCM per file, to compare AudioToolbox
//! AAC decodes across runs (`docs/re/decode.md` § 6).
//!
//! `cargo run --release -p mb-audio --example aac_hash -- [--dump <dir>] <file>...`
//! `--dump` also writes each decode as raw little-endian i16 (`<dir>/<n>.pcm`, interleaved L/R).

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut dump = None;
    if args.peek().map(String::as_str) == Some("--dump") {
        args.next();
        dump = args.next().map(std::path::PathBuf::from);
    }
    for (n, p) in args.enumerate() {
        let pcm = mb_audio::open(&p).unwrap().decode_all();
        let h = pcm.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, s| {
            (h ^ u64::from(s[0].to_bits()) ^ (u64::from(s[1].to_bits()) << 32))
                .wrapping_mul(0x100_0000_01b3)
        });
        println!(
            "{h:016x} {:>9} {}",
            pcm.len(),
            p.rsplit('/').next().unwrap_or(&p)
        );
        if let Some(dir) = &dump {
            let mut out = Vec::with_capacity(pcm.len() * 4);
            for s in &pcm {
                for c in s {
                    out.extend_from_slice(&((c * 32768.0).round() as i16).to_le_bytes());
                }
            }
            std::fs::write(dir.join(format!("{n}.pcm")), out).unwrap();
        }
    }
}
