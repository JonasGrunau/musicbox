//! Prints a hash of each file's decoded PCM (to compare AudioToolbox AAC decodes across runs).
fn main() {
    for p in std::env::args().skip(1) {
        let pcm = mb_audio::open(&p).unwrap().decode_all();
        let h = pcm.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, s| {
            (h ^ u64::from(s[0].to_bits()) ^ (u64::from(s[1].to_bits()) << 32))
                .wrapping_mul(0x100_0000_01b3)
        });
        println!("{h:016x} {}", p.rsplit('/').next().unwrap_or(&p));
    }
}
