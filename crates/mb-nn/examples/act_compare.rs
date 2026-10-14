//! Dev check: runs a beat model on a `pcm_dump` file (mono `(L + R)·0.5` over `[start, start + n)`)
//! and compares the activations with a reference dump (`u32 rows, u32 cols, f32…`).
//!   cargo run --release -p mb-nn --example act_compare -- <model7|model8> <pcm> <start> <n> [ref.act]

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let model = if a[1] == "model7" { mb_nn::Model::BeatNo7 } else { mb_nn::Model::BeatNo8 };
    let pcm = std::fs::read(&a[2]).unwrap();
    let start: usize = a[3].parse().unwrap();
    let n: usize = a[4].parse().unwrap();
    let f = |i: usize| f32::from_le_bytes(pcm[16 + i * 4..20 + i * 4].try_into().unwrap());
    let data: Vec<f32> = (start..start + n).map(|i| (f(2 * i) + f(2 * i + 1)) * 0.5).collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let s = mb_nn::Session::open(model, threads).unwrap();
    let t0 = std::time::Instant::now();
    let input = mb_nn::Tensor { dims: vec![1, n as i64], data };
    let out = s.run(&[("serving_default_fltp:0", &input)], &["StatefulPartitionedCall:1"]).unwrap();
    eprintln!("dims {:?} in {:?}", out[0].dims, t0.elapsed());
    if let Some(r) = a.get(5) {
        let b = std::fs::read(r).unwrap();
        let rows = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
        let cols = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
        let rf: Vec<f32> = b[8..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        let mine = &out[0].data;
        let (mut maxd, mut neq) = (0f32, 0usize);
        for (x, y) in mine.iter().zip(&rf) {
            if x.to_bits() != y.to_bits() { neq += 1; }
            maxd = maxd.max((x - y).abs());
        }
        eprintln!("ref {rows}x{cols} ({} values), mine {} values; differing {neq}, max |d| {maxd:e}", rf.len(), mine.len());
        for i in 0..4 { eprintln!("  {:?} vs {:?}", &mine[i * cols..(i + 1) * cols], &rf[i * cols..(i + 1) * cols]); }
    }
}
