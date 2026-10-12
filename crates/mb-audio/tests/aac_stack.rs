//! Apple's AAC decoder gives slightly different samples depending on the stack alignment at
//! which `AudioConverterFillComplexBuffer` is entered (`docs/re/decode.md` § 6). `mb-audio`
//! enters it through a trampoline at a fixed alignment, so the decode must not depend on how
//! deep the caller's stack is. Skipped without the probe corpus.

mod common;

/// Decodes `path` after using `depth × 16` bytes of extra stack, and hashes the samples.
fn decode_at_depth(path: &std::path::Path, depth: usize) -> u64 {
    #[inline(never)]
    fn nest(path: &std::path::Path, depth: usize) -> u64 {
        if depth == 0 {
            let pcm = mb_audio::open(path).unwrap().decode_all();
            return pcm.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, s| {
                (h ^ u64::from(s[0].to_bits()) ^ (u64::from(s[1].to_bits()) << 32))
                    .wrapping_mul(0x100_0000_01b3)
            });
        }
        // 16 bytes of stack per level that the optimiser cannot remove.
        let pad = std::hint::black_box([depth as u64; 2]);
        let h = nest(path, depth - 1);
        std::hint::black_box(&pad);
        h
    }
    nest(path, depth)
}

#[test]
fn aac_decode_does_not_depend_on_the_callers_stack() {
    let Some(corpus) = common::corpus() else {
        eprintln!("corpus missing, skipping");
        return;
    };
    let path = corpus.join("probes/m4a_aac256/click_128_offset1234.m4a");
    if !path.exists() {
        eprintln!("{} missing, skipping", path.display());
        return;
    }
    let hashes: Vec<u64> = (0..8)
        .map(|d| {
            let path = path.clone();
            std::thread::spawn(move || decode_at_depth(&path, d))
                .join()
                .unwrap()
        })
        .collect();
    assert!(hashes.iter().all(|h| *h == hashes[0]), "{hashes:x?}");
}
