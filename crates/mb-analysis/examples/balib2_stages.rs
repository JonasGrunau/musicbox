//! BALib v2 stages against `b2` harness dumps (`IN`/`OUT`/`BIA` lines). Usage:
//!   cargo run --release -p mb-analysis --example balib2_stages -- <audio> <dump.txt>
//!   cargo run --release -p mb-analysis --example balib2_stages -- all <tracks.tsv> <dir>   (<dir>/<id>.txt)

fn list(text: &str, tag: &str) -> Vec<(i32, f32, f32, i32, i32, i32)> {
    text.lines()
        .filter_map(|l| l.strip_prefix(tag).map(str::trim))
        .filter(|l| l.contains(' '))
        .map(|l| {
            let v: Vec<&str> = l.split_whitespace().collect();
            (v[0].parse().unwrap(), v[1].parse().unwrap(), v[2].parse().unwrap(), v[3].parse().unwrap(), v[4].parse().unwrap(), v[5].parse().unwrap())
        })
        .collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "all" {
        let tracks = std::fs::read_to_string(&a[2]).unwrap();
        for line in tracks.lines() {
            let Some((id, path)) = line.split_once('\t') else { continue };
            let Ok(text) = std::fs::read_to_string(format!("{}/{id}.txt", a[3])) else { continue };
            let name = path.rsplit('/').next().unwrap_or(path);
            print!("{id} {name}: ");
            one(path, &text);
        }
        return;
    }
    let text = std::fs::read_to_string(&a[2]).unwrap();
    one(&a[1], &text);
}

fn one(path: &str, text: &str) {
    let Ok(mut dec) = mb_audio::open(path) else { println!("no audio"); return };
    let info = *dec.info();
    let frames = dec.decode_all();
    let mut an = mb_analysis::balib2::Analyzer::new(info.sample_rate, info.total_samples.saturating_sub(1));
    let pcm: Vec<[i16; 2]> = frames.iter().map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])]).collect();
    an.put(&pcm);
    let (raw, out, varied) = an.stages();
    let hendou = text.lines().any(|l| l.starts_with("BIA ret") && l.contains("hendou 1"));
    let out = if hendou { varied } else { out };
    let conv = |b: &mb_analysis::beats::BeatInfo| (b.pos, b.beats, b.tempo, b.stat, b.band, i32::from(b.flag));
    let theirs_in = list(text, "IN ");
    let ours_in: Vec<_> = raw.iter().map(conv).collect();
    let d = ours_in.iter().zip(&theirs_in).position(|(x, y)| x != y);
    if d.is_some() || ours_in.len() != theirs_in.len() {
        println!("SENSOR differs: ours {} theirs {} first diff {d:?} {:?} {:?}", ours_in.len(), theirs_in.len(), d.map(|i| ours_in[i]), d.map(|i| theirs_in[i]));
        return;
    }
    let theirs_out = list(text, "OUT ");
    let ret = text.lines().find(|l| l.starts_with("BIA ret")).unwrap_or("");
    if let Some((bpm, o)) = out {
        let ours_out: Vec<_> = o.iter().map(conv).collect();
        let d = ours_out.iter().zip(&theirs_out).position(|(x, y)| x != y);
        let list_same = d.is_none() && ours_out.len() == theirs_out.len();
        println!("{} list {} (ours {} theirs {}{}) | ours bpm {bpm} | {ret}", if hendou { "VARIED" } else { "CONST" }, if list_same { "SAME" } else { "DIFF" }, ours_out.len(), theirs_out.len(),
            d.map_or(String::new(), |i| format!(", first diff {i}: {:?} vs {:?}", ours_out[i], theirs_out[i])));
    } else {
        println!("ours none | {ret} n {}", theirs_out.len());
    }
}
