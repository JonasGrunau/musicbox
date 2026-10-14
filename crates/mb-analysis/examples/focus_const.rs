//! Focus's constant path against `hp` harness runs: the beats going into and out of
//! `UnitBeatAdjust::analyze` (`HP_UBA` traces: `UBA in/out`, `UI`/`UO`) and the final Focus list
//! (`F` lines of the `hp` output). Usage:
//!   cargo run --release -p mb-analysis --example focus_const -- <tracks.tsv> <dir> [id]
//! `<dir>/<id>.txt` is the trace, `<dir>/<id>.hp.txt` the `hp` output (mode 2 or 3).

use mb_analysis::hpbeat::focus::{BeatInfo, Focus, FocusInput};
use mb_analysis::hpbeat::madmom::ListBeat;

fn list(text: &str, tag: &str) -> Vec<(i32, f32, i32)> {
    text.lines()
        .filter_map(|l| l.strip_prefix(tag))
        .filter(|l| l.contains(' '))
        .map(|l| {
            let v: Vec<&str> = l.split_whitespace().collect();
            (
                v[0].parse().unwrap(),
                v[2].parse().unwrap(),
                v[3].parse().unwrap(),
            )
        })
        .collect()
}

fn field(l: &str, k: &str) -> i64 {
    l.split_whitespace()
        .skip_while(|w| *w != k)
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let only = a.get(3).cloned();
    let (mut pre_ok, mut post_ok, mut total) = (0, 0, 0);
    for line in std::fs::read_to_string(&a[1]).unwrap().lines() {
        let Some((id, path)) = line.split_once('\t') else {
            continue;
        };
        if only.as_deref().is_some_and(|o| o != id) {
            continue;
        }
        let (Ok(trace), Ok(hp)) = (
            std::fs::read_to_string(format!("{}/{id}.txt", a[2])),
            std::fs::read_to_string(format!("{}/{id}.hp.txt", a[2])),
        ) else {
            continue;
        };
        if !trace.contains("UBA in") {
            continue;
        }
        let Some(bal) = hp.lines().find(|l| l.starts_with("BALIB ")) else {
            continue;
        };
        let Some(mm) = hp.lines().find(|l| l.starts_with("MADMOM ")) else {
            continue;
        };
        let mad: Vec<ListBeat> = hp
            .lines()
            .filter_map(|l| l.strip_prefix("M "))
            .map(|l| {
                let v: Vec<i32> = l.split_whitespace().map(|x| x.parse().unwrap()).collect();
                ListBeat {
                    pos: v[0],
                    num_bar: v[1],
                    bpm: v[2],
                }
            })
            .collect();
        let Ok(mut dec) = mb_audio::open(path) else {
            continue;
        };
        let info = *dec.info();
        let frames = dec.decode_all();
        let total_n = info.total_samples.saturating_sub(1);
        let mut an = mb_analysis::balib2::Analyzer::new(info.sample_rate, total_n);
        let pcm: Vec<[i16; 2]> = frames
            .iter()
            .map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])])
            .collect();
        an.put(&pcm);
        let inp = FocusInput {
            rate: info.sample_rate as i32,
            total: total_n as i32,
            varied: false,
            hendou: field(bal, "hendou") == 1,
            offset: field(bal, "offset") as i32,
        };
        let f = Focus::run_with(
            inp,
            field(bal, "beats") as i32,
            field(bal, "bpm") as i32,
            &mad,
            field(mm, "bpm") as i32,
            false,
            Some(an.adjust_mut()),
        );
        total += 1;
        let Some(f) = f else {
            println!("{id}: no Focus result");
            continue;
        };
        let conv = |b: &BeatInfo| (b.pos, b.tempo, b.stat);
        let num = |l: &str| -> f32 {
            l.split("bpm ")
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0)
        };
        let in_bpm = trace
            .lines()
            .find(|l| l.starts_with("UBA in"))
            .map_or(0.0, num);
        let ui = list(&trace, "UI ");
        let Some(((bpm, pre), _)) = &f.adjust_io else {
            println!("{id}: no adjust stage ({:?})", f.unported);
            continue;
        };
        let ours: Vec<(i32, f32, i32)> = pre.iter().map(conv).collect();
        let d = ours.iter().zip(&ui).position(|(x, y)| x != y);
        let ok = d.is_none() && ours.len() == ui.len() && *bpm == in_bpm;
        pre_ok += i32::from(ok);
        let fl: Vec<(i32, i32, i32)> = hp
            .lines()
            .filter_map(|l| l.strip_prefix("F "))
            .map(|l| {
                let v: Vec<i32> = l.split_whitespace().map(|x| x.parse().unwrap()).collect();
                (v[0], v[1], v[2])
            })
            .collect();
        let fb = hp
            .lines()
            .find(|l| l.starts_with("FOCUS "))
            .map_or(0, |l| field(l, "bpm")) as i32;
        let ours_f = f.beat_list();
        let post = ours_f == fl && f.bpm100() == fb;
        post_ok += i32::from(post);
        println!(
            "{id}: pre {} bpm {bpm} vs {in_bpm}, beats {} vs {}{} | final {} bpm {} vs {fb}, beats {} vs {}",
            if ok { "SAME" } else { "DIFF" },
            ours.len(),
            ui.len(),
            d.map_or(String::new(), |i| format!(
                ", first diff {i}: {:?} vs {:?}",
                ours[i], ui[i]
            )),
            if post { "SAME" } else { "DIFF" },
            f.bpm100(),
            ours_f.len(),
            fl.len()
        );
    }
    println!("constant path: before UnitBeatAdjust {pre_ok}/{total}, final {post_ok}/{total}");
}
