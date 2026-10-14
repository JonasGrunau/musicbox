//! Dev checks for the high-precision beat port.
//!   hp_check table <out.bin>            the transition table (pointers u32, prev u32, prob f64)
//!   hp_check madmom <audio> <out.txt>   madmom beat list (hendou) for one track
//!   hp_check focus <ref.txt>            Focus on a harness reference run
//!   hp_check focusin <in.txt>           Focus on a harness `hp focus` input

use mb_analysis::hpbeat::{hmm::Hmm, madmom};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "table" => {
            let h = Hmm::get();
            let mut f = std::io::BufWriter::new(std::fs::File::create(&a[2]).unwrap());
            for v in &h.pointers { f.write_all(&v.to_le_bytes()).unwrap(); }
            for v in &h.prev { f.write_all(&v.to_le_bytes()).unwrap(); }
            for v in &h.prob { f.write_all(&v.to_le_bytes()).unwrap(); }
        }
        "madmom" => {
            let mut dec = mb_audio::open(&a[2]).unwrap();
            let rate = dec.info().sample_rate;
            let frames = dec.decode_all();
            let mono: Vec<f32> = frames.iter().map(|f| (f[1] + f[0]) * 0.5).collect();
            let total = frames.len() - 1;
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
            let model = mb_nn::Session::open(mb_nn::Model::BeatNo7, threads).unwrap();
            let t0 = std::time::Instant::now();
            let r = madmom::analyze(&model, &mono, rate, total, 0, true).unwrap();
            let mut f = std::fs::File::create(&a[3]).unwrap();
            writeln!(f, "MADMOM bpm {} beats {} ms {}", r.bpm, r.beats.len(), t0.elapsed().as_millis()).unwrap();
            for b in &r.beats { writeln!(f, "M {} {} {}", b.pos, b.num_bar, b.bpm).unwrap(); }
        }
        "dbt" => {
            // downbeat tracking on a dumped activation file (u32 rows, u32 cols, f32...)
            let b = std::fs::read(&a[2]).unwrap();
            let v: Vec<f32> = b[8..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
            let act: Vec<[f32; 2]> = v.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
            let h = Hmm::get();
            let thr: Vec<usize> = act.iter().enumerate().filter(|(_, r)| r[0] >= 0.05 || r[1] >= 0.05).map(|(i, _)| i).collect();
            let (f, l) = (thr[0], thr[thr.len() - 1] + 1);
            let (path, logp) = h.viterbi(&act[f..l]);
            let runs: Vec<(usize, u8)> = path.iter().enumerate().filter(|(i, s)| *i == 0 || h.om_pointers[**s as usize] != h.om_pointers[path[i - 1] as usize]).map(|(i, s)| (i, h.om_pointers[*s as usize])).collect();
            println!("first {f} last {l} logp {logp} runs {:?}", &runs[..runs.len().min(20)]);
            println!("{:?}", madmom::downbeat_tracking(&act));
        }
        "focus" => {
            // Focus pass on a harness reference (BALIB/MADMOM/M lines); prints F lines.
            let text = std::fs::read_to_string(&a[2]).unwrap();
            let mut rate = 0; let mut len = 0; let mut bbpm = 0; let mut bn = 0; let mut mbpm = 0;
            let mut mad = Vec::new();
            for l in text.lines() {
                let t: Vec<&str> = l.split_whitespace().collect();
                match t.first() {
                    Some(&"BALIB") => {
                        bbpm = t[2].parse().unwrap(); bn = t[4].parse().unwrap();
                        rate = t[t.iter().position(|x| *x == "rate").unwrap() + 1].parse().unwrap();
                        len = t[t.iter().position(|x| *x == "len").unwrap() + 1].parse::<i32>().unwrap();
                    }
                    Some(&"MADMOM") => mbpm = t[4].parse().unwrap(),
                    Some(&"M") => mad.push(madmom::ListBeat { pos: t[1].parse().unwrap(), num_bar: t[2].parse().unwrap(), bpm: t[3].parse().unwrap() }),
                    _ => {}
                }
            }
            let inp = mb_analysis::hpbeat::focus::FocusInput { rate, total: len - 1, varied: true, hendou: true, offset: 0 };
            let trace = std::env::var("HP_TRACE_OUT").ok();
            let Some(f) = mb_analysis::hpbeat::focus::Focus::run(inp, bn, bbpm, &mad, mbpm, trace.is_some()) else { println!("FOCUS none"); return };
            let list = f.beat_list();
            println!("FOCUS bpm {} beats {} unported {:?}", f.bpm100(), list.len(), f.unported);
            for (p, s, t) in list { println!("F {p} {s} {t}"); }
            if let Some(t) = trace { std::fs::write(t, f.trace().join("\n") + "\n").unwrap(); }
        }
        "focusin" => {
            // Focus on a harness `hp focus` input file (header line, then "pos numBar bpm").
            let text = std::fs::read_to_string(&a[2]).unwrap();
            let mut it = text.split_whitespace().map(|x| x.parse::<i32>().unwrap());
            let mut h = [0i32; 8];
            for v in &mut h { *v = it.next().unwrap(); }
            let [rate, total, varied, flag2d, offset, bpm, nb, mbpm] = h;
            let v: Vec<i32> = it.collect();
            let mad: Vec<madmom::ListBeat> = v.chunks_exact(3).map(|c| madmom::ListBeat { pos: c[0], num_bar: c[1], bpm: c[2] }).collect();
            let inp = mb_analysis::hpbeat::focus::FocusInput { rate, total: total - 1, varied: varied != 0, hendou: flag2d != 0, offset };
            let Some(f) = mb_analysis::hpbeat::focus::Focus::run(inp, nb, bpm, &mad, mbpm, false) else { println!("FOCUS none"); return };
            let list = f.beat_list();
            println!("FOCUS bpm {} beats {} unported {:?} hits {:?}", f.bpm100(), list.len(), f.unported, f.hits);
            for (p, s, t) in list { println!("F {p} {s} {t}"); }
        }
        _ => panic!("unknown"),
    }
}
