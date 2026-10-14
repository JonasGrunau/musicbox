//! BALib v2's `UnitBeatAdjust::analyze` against `hp` harness traces (`HP_UBA`: `UBA in/out`,
//! `UI`/`UO` lists, `BAND` checksums). Usage:
//!   cargo run --release -p mb-analysis --example balib2_uba -- <audio> <uba.txt>
//!   cargo run --release -p mb-analysis --example balib2_uba -- all <tracks.tsv> <dir>   (<dir>/<id>.txt)

use mb_analysis::beats::BeatInfo;

fn list(text: &str, tag: &str) -> Vec<BeatInfo> {
    text.lines()
        .filter_map(|l| l.strip_prefix(tag))
        .filter(|l| l.contains(' '))
        .map(|l| {
            let v: Vec<&str> = l.split_whitespace().collect();
            BeatInfo {
                pos: v[0].parse().unwrap(),
                beats: v[1].parse().unwrap(),
                tempo: v[2].parse().unwrap(),
                stat: v[3].parse().unwrap(),
                band: v[4].parse().unwrap(),
                flag: v[5].parse().unwrap(),
            }
        })
        .collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "all" {
        let (mut same, mut total) = (0, 0);
        for line in std::fs::read_to_string(&a[2]).unwrap().lines() {
            let Some((id, path)) = line.split_once('\t') else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(format!("{}/{id}.txt", a[3])) else {
                continue;
            };
            if !text.contains("UBA in") {
                continue;
            }
            print!("{id}: ");
            total += 1;
            same += i32::from(one(path, &text));
        }
        println!("UnitBeatAdjust: {same}/{total} identical");
        return;
    }
    let text = std::fs::read_to_string(&a[2]).unwrap();
    one(&a[1], &text);
}

fn one(path: &str, text: &str) -> bool {
    let Ok(mut dec) = mb_audio::open(path) else {
        println!("no audio");
        return false;
    };
    let info = *dec.info();
    let frames = dec.decode_all();
    let mut an =
        mb_analysis::balib2::Analyzer::new(info.sample_rate, info.total_samples.saturating_sub(1));
    let pcm: Vec<[i16; 2]> = frames
        .iter()
        .map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])])
        .collect();
    an.put(&pcm);
    let num = |l: &str, k: &str| -> f32 {
        l.split(k)
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    };
    let bpm_in = text
        .lines()
        .find(|l| l.starts_with("UBA in"))
        .map_or(0.0, |l| num(l, "bpm "));
    let bpm_out = text
        .lines()
        .find(|l| l.starts_with("UBA out"))
        .map_or(0.0, |l| num(l, "bpm "));
    let mut bpm = bpm_in;
    let mut ours = list(text, "UI ");
    let theirs = list(text, "UO ");
    let bands = an.adjust_stage(&mut bpm, &mut ours);
    let mut ok = true;
    for (k, l) in text.lines().filter(|l| l.starts_with("BAND ")).enumerate() {
        let w: Vec<&str> = l.split_whitespace().collect();
        let t = (
            w[3].parse::<usize>().unwrap(),
            w[5].parse::<i64>().unwrap(),
            w[7].parse::<i64>().unwrap(),
        );
        if let Some(b) = bands.get(k)
            && (b.0, b.1, b.2) != t
        {
            println!("band {k} differs: ours {:?} theirs {t:?}", (b.0, b.1, b.2));
            ok = false;
        }
    }
    let conv = |b: &BeatInfo| (b.pos, b.tempo, b.stat, b.band, b.flag);
    let d = ours
        .iter()
        .zip(&theirs)
        .position(|(x, y)| conv(x) != conv(y));
    let same = d.is_none() && ours.len() == theirs.len() && bpm == bpm_out;
    println!(
        "{} bpm {bpm} vs {bpm_out}, beats {} vs {}{}",
        if same && ok { "SAME" } else { "DIFF" },
        ours.len(),
        theirs.len(),
        d.map_or(String::new(), |i| format!(
            ", first diff {i}: {:?} vs {:?}",
            conv(&ours[i]),
            conv(&theirs[i])
        ))
    );
    same && ok
}
