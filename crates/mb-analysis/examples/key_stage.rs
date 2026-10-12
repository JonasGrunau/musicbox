//! Development aid for the key port: runs one stage on a `key_dump` file and writes the raw
//! result, for comparison with the binary's own routine. Usage:
//!   cargo run --release -p mb-analysis --example key_stage -- cut <in.bin> <out.f32>
//!   cargo run --release -p mb-analysis --example key_stage -- est <in.bin>

use std::io::Read;

pub struct Dump {
    pub rate: u32,
    pub len: u64,
    pub bpm: f32,
    pub beats: Vec<(u16, u16, u32)>,
    pub buf: Vec<f32>,
}

fn u32le(b: &[u8], o: &mut usize) -> u32 {
    let v = u32::from_le_bytes(b[*o..*o + 4].try_into().unwrap());
    *o += 4;
    v
}

pub fn load(path: &str) -> Dump {
    let mut b = Vec::new();
    std::fs::File::open(path)
        .unwrap()
        .read_to_end(&mut b)
        .unwrap();
    let mut o = 4;
    let rate = u32le(&b, &mut o);
    let len = u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    o += 8;
    let bpm = f32::from_bits(u32le(&b, &mut o));
    let nb = u32le(&b, &mut o) as usize;
    let mut beats = Vec::with_capacity(nb);
    for _ in 0..nb {
        let bib = u16::from_le_bytes(b[o..o + 2].try_into().unwrap());
        let tempo = u16::from_le_bytes(b[o + 2..o + 4].try_into().unwrap());
        let ms = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap());
        beats.push((bib, tempo, ms));
        o += 12;
    }
    let k = u32le(&b, &mut o) as usize;
    let buf = (0..k)
        .map(|i| f32::from_le_bytes(b[o + 4 * i..o + 4 * i + 4].try_into().unwrap()))
        .collect();
    Dump {
        rate,
        len,
        bpm,
        beats,
        buf,
    }
}

fn write_f32(path: &str, v: &[f32]) {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    std::fs::write(path, bytes).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let d = if a[0] == "judgefuzz" {
        Dump {
            rate: 0,
            len: 0,
            bpm: 0.0,
            beats: vec![],
            buf: vec![],
        }
    } else {
        load(&a[1])
    };
    match a[0].as_str() {
        "cut" => {
            let k = d.buf.len();
            let mut y = vec![0f32; k];
            mb_analysis::key::cutbeats::cut_beats(&mut y, &d.buf, k, d.rate as f32 * 0.125, 8);
            write_f32(&a[2], &y);
        }
        "pitch" => {
            let k = d.buf.len();
            let mut y = vec![0f32; k];
            let rate = d.rate as f32 * 0.125;
            mb_analysis::key::cutbeats::cut_beats(&mut y, &d.buf, k, rate, 8);
            println!("{}", mb_analysis::key::pitch::pitch_detect(&y, k, rate, 8));
        }
        "kcs" => {
            use mb_analysis::key::{cqt, cutbeats, pitch};
            let k = d.buf.len();
            let mut y = vec![0f32; k];
            let rate = d.rate as f32 * 0.125;
            cutbeats::cut_beats(&mut y, &d.buf, k, rate, 8);
            let p = pitch::pitch_detect(&y, k, rate, 8);
            let mut bank = cqt::Bank::default();
            cqt::build_bank(&mut bank, k, rate, p as f32, false);
            let (mut m, frames, max) = cqt::cqt(&y, k, &bank);
            let prof = cqt::profile(&mut m, frames, max, rate, bank.hop);
            let mut out = Vec::new();
            for v in [p, bank.hop, -1, -1] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for v in prof.iter().chain(&bank.len).chain(&bank.w) {
                out.extend_from_slice(&v.to_le_bytes());
            }
            let nc = bank.len[0] as u32 * 72;
            out.extend_from_slice(&nc.to_le_bytes());
            for v in bank.cos[..nc as usize]
                .iter()
                .chain(&bank.sin[..nc as usize])
            {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&[0u8; 136]);
            std::fs::write(&a[2], out).unwrap();
        }
        "est" => {
            let beats: Vec<(u16, u32)> = d.beats.iter().map(|&(b, _, ms)| (b, ms)).collect();
            let verbose = a.get(2).is_some_and(|v| v == "-v");
            let e = mb_analysis::key::estimates_traced(
                &d.buf,
                d.rate,
                d.len,
                d.bpm,
                &beats,
                !beats.is_empty(),
                &mut |i, t| {
                    if verbose {
                        let mut v = t.profile;
                        let mut idx: [i32; 12] = std::array::from_fn(|i| i as i32);
                        mb_analysis::key::scale::qsort3(&mut v, &mut idx);
                        print!("CALL {i:?} QSORT3 12");
                        for (x, i) in v.iter().zip(&idx) {
                            print!(" {i}:{x:e}");
                        }
                        println!(" | win {:?} | votes {:?}", t.window_keys, t.votes);
                        for (w, (r, b, s)) in t.windows.iter().enumerate() {
                            println!("  W{w} r {r:?} B {b} S {s} -> {}", t.window_keys[w]);
                        }
                    }
                },
            );
            print!(
                "JUDGE {} {} {} {} {} |",
                e.key1,
                e.key2,
                e.keys.len(),
                e.segments as i32,
                d.bpm
            );
            for (a, b) in &e.keys {
                print!(" {a},{b}");
            }
            println!(
                " | {}",
                mb_analysis::key::judge::judge(e.key1, e.key2, e.segments, &e.keys, d.bpm)
            );
        }
        "judgefuzz" => {
            // Lines `k1 k2 flag bpm nseg a,b … | native` from the harness's `kfuzz`.
            let text = std::fs::read_to_string(&a[1]).unwrap();
            let (mut ok, mut n) = (0, 0);
            for l in text.lines().filter(|l| !l.starts_with("S ")) {
                let (lhs, nat) = l.split_once(" | ").unwrap();
                let f: Vec<&str> = lhs.split_whitespace().collect();
                let keys: Vec<(i32, i32)> = f[5..]
                    .iter()
                    .map(|p| {
                        let (x, y) = p.split_once(',').unwrap();
                        (x.parse().unwrap(), y.parse().unwrap())
                    })
                    .collect();
                let r = mb_analysis::key::judge::judge(
                    f[0].parse().unwrap(),
                    f[1].parse().unwrap(),
                    f[2] != "0",
                    &keys,
                    f[3].parse().unwrap(),
                );
                n += 1;
                if std::env::var_os("MB_JUDGE_STATE").is_some() {
                    let (_, st) = mb_analysis::key::judge::judge_trace(
                        f[0].parse().unwrap(),
                        f[2] != "0",
                        &keys,
                        f[3].parse().unwrap(),
                    );
                    println!("{st}");
                    continue;
                }
                if r == nat.trim().parse::<i32>().unwrap() {
                    ok += 1;
                } else if n - ok <= 5 {
                    println!("mismatch: ours {r} | {l}");
                }
            }
            println!("judge fuzz: {ok}/{n}");
            return;
        }
        s => panic!("unknown stage {s}"),
    }
    let _ = (d.len, d.bpm, &d.beats);
}
