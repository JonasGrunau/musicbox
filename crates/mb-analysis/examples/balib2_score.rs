//! BALib v2's own result (BPM, beat list, offset, hendou) against rekordbox's, from harness runs of
//! `highPrecisionBeatAnalyze` (`BALIB` and `B` lines). Usage:
//!   cargo run --release -p mb-analysis --example balib2_score -- <tracks.tsv> <dir> <mode> [verbose]
//! `tracks.tsv` = `id<TAB>path`; the reference is `<dir>/<id>.m<mode>.txt`.

use mb_analysis::balib2::Analyzer;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let verbose = a.len() > 5;
    let tracks = std::fs::read_to_string(&a[1]).unwrap();
    let (mut same, mut bpm_same, mut off_same, mut hendou_same, mut total) = (0, 0, 0, 0, 0);
    for line in tracks.lines() {
        let Some((id, path)) = line.split_once('\t') else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(format!("{}/{id}.m{}.txt", a[2], a[3])) else {
            continue;
        };
        let Some(bal) = text.lines().find(|l| l.starts_with("BALIB ")) else {
            continue;
        };
        let w: Vec<&str> = bal.split_whitespace().collect();
        let bpm: i32 = w[2].parse().unwrap();
        let offset: i32 = w[6].parse().unwrap();
        let hendou = w[8] == "1";
        let theirs: Vec<(i32, i32, i32)> = text
            .lines()
            .filter(|l| l.starts_with("B "))
            .map(|l| {
                let v: Vec<i32> = l[2..]
                    .split_whitespace()
                    .map(|x| x.parse().unwrap())
                    .collect();
                (v[0], v[1], v[2])
            })
            .collect();
        let Ok(mut dec) = mb_audio::open(path) else {
            continue;
        };
        let info = *dec.info();
        let frames = dec.decode_all();
        let total_n = info.total_samples.saturating_sub(1);
        let mut an = Analyzer::new(info.sample_rate, total_n);
        let pcm: Vec<[i16; 2]> = frames
            .iter()
            .map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])])
            .collect();
        an.put(&pcm);
        let r = an.finish();
        let (ours_bpm, ours, ours_off, ours_h) = r.map_or((0, Vec::new(), 0, false), |r| {
            (r.bpm100, r.beats, r.offset, r.hendou)
        });
        total += 1;
        bpm_same += usize::from(ours_bpm == bpm);
        off_same += usize::from(ours_off == offset);
        hendou_same += usize::from(ours_h == hendou);
        if ours_off != offset || ours_h != hendou {
            println!("{id}: offset {ours_off} vs {offset}, hendou {ours_h} vs {hendou}");
        }
        if ours == theirs && ours_bpm == bpm && ours_off == offset && ours_h == hendou {
            same += 1;
        } else {
            let first = ours.iter().zip(&theirs).position(|(x, y)| x != y);
            let name = path.rsplit('/').next().unwrap_or(path);
            println!(
                "{id} {name}: bpm {ours_bpm} vs {bpm}, beats {} vs {}, first diff {first:?} {:?} vs {:?}",
                ours.len(),
                theirs.len(),
                first.map(|i| ours[i]),
                first.map(|i| theirs[i])
            );
            if verbose && let Some(i) = first {
                for k in i.saturating_sub(2)..(i + 6).min(ours.len()).min(theirs.len()) {
                    println!("    {k}: {:?} vs {:?}", ours[k], theirs[k]);
                }
            }
        }
    }
    println!(
        "BALib v2: {same}/{total} identical, BPM {bpm_same}/{total}, offset {off_same}/{total}, hendou {hendou_same}/{total}"
    );
}
