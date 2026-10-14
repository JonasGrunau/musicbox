//! Checks `hpbeat::range::rebuild` (convertToMicrosec's BPM-range rebuild) against a harness
//! dump of rekordbox's own function on random beat lists. Usage:
//!   cargo run --release -p mb-analysis --example range_check -- <cases.txt> [--verbose]
//! Each case: `C rate total bpm n`, n lines `I pos stat bpm`, `R bpm used`, used lines
//! `O bar tempo ms sub`.

use mb_analysis::beats::Beat;
use mb_analysis::hpbeat::range;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: range_check <cases.txt> [--verbose]");
        return;
    };
    let verbose = args.iter().any(|a| a == "--verbose");
    let text = std::fs::read_to_string(path).expect("cases");
    let mut lines = text.lines().peekable();
    let (mut n_cases, mut same, mut kinds) =
        (0, 0, std::collections::BTreeMap::<&str, (u32, u32)>::new());
    // outside the range, without repeated halving: by first bar and count parity
    let mut shape = std::collections::BTreeMap::<(u16, &str), (u32, u32)>::new();
    while let Some(l) = lines.next() {
        let Some(h) = l.strip_prefix("C ") else {
            continue;
        };
        let h: Vec<i64> = h.split_whitespace().map(|v| v.parse().unwrap()).collect();
        let (rate, total, bpm, n) = (h[0] as u32, h[1] as u64, h[2] as i32, h[3] as usize);
        let mut beats = Vec::with_capacity(n);
        for _ in 0..n {
            let v: Vec<i64> = lines.next().unwrap()[2..]
                .split_whitespace()
                .map(|v| v.parse().unwrap())
                .collect();
            let us = v[0] * 1_000_000 / i64::from(rate);
            let ms = us / 1000;
            beats.push(Beat {
                beat_in_bar: v[1] as u16,
                tempo: v[2] as u16,
                time_ms: ms as u32,
                sub_ms: (us - ms * 1000) as u16,
            });
        }
        let r: Vec<i64> = lines.next().unwrap()[2..]
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        let (rb_bpm, used) = (r[0] as i32, r[1] as usize);
        let mut rb = Vec::with_capacity(used);
        for _ in 0..used {
            let v: Vec<u32> = lines.next().unwrap()[2..]
                .split_whitespace()
                .map(|v| v.parse().unwrap())
                .collect();
            rb.push(Beat {
                beat_in_bar: v[0] as u16,
                tempo: v[1] as u16,
                time_ms: v[2],
                sub_ms: v[3] as u16,
            });
        }
        let first_bar = beats.first().map_or(0, |b| b.beat_in_bar);
        let first_tempo = beats.first().map_or(0, |b| i32::from(b.tempo));
        // rekordbox converts the in-range lists plainly; an empty list gives nothing
        let ours = if n == 0 {
            (Vec::new(), bpm)
        } else {
            range::rebuild(beats, bpm, rate, total)
        };
        // the BPM the range check sees
        let v = if bpm < 1 { first_tempo } else { bpm };
        let kind = match v {
            v if v < 3500 => "repeated doubling",
            v if v < 7000 => "doubling",
            v if v <= 18000 => "in range",
            v if v <= 36000 => "halving",
            _ => "repeated halving",
        };
        let ok = ours.0 == rb && ours.1 == rb_bpm;
        n_cases += 1;
        same += u32::from(ok);
        let e = kinds.entry(kind).or_default();
        e.0 += 1;
        e.1 += u32::from(ok);
        if bpm < 1 {
            let e = kinds
                .entry("(of these, BPM below 1: the first beat's tempo)")
                .or_default();
            e.0 += 1;
            e.1 += u32::from(ok);
        }
        if kind != "in range" && kind != "repeated halving" && n > 0 {
            let e = shape
                .entry((first_bar, if n % 2 == 0 { "even" } else { "odd" }))
                .or_default();
            e.0 += 1;
            e.1 += u32::from(ok);
        }
        if !ok && (verbose || kind != "repeated halving") {
            let d = ours.0.iter().zip(&rb).position(|(a, b)| a != b);
            println!(
                "case {n_cases} ({kind}, rate {rate}, n {n}, bpm {bpm}): bpm {} vs {rb_bpm}, beats {} vs {}, first diff {d:?}",
                ours.1,
                ours.0.len(),
                rb.len()
            );
            if let Some(k) = d.or(Some(ours.0.len().min(rb.len()))) {
                for j in k.saturating_sub(1)..(k + 2) {
                    println!("    {j}: ours {:?} rb {:?}", ours.0.get(j), rb.get(j));
                }
            }
        }
    }
    for (k, (n, ok)) in &kinds {
        println!("  {k}: {ok}/{n}");
    }
    for ((bar, parity), (n, ok)) in &shape {
        println!("  first bar {bar}, {parity} count: {ok}/{n}");
    }
    println!("{same}/{n_cases} identical");
}
