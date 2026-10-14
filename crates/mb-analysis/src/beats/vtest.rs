//! Development check of the varied (Dynamic) path against rekordbox's own analyser: with
//! `MB_V1_DIR` naming a folder of `<n>.v1` (beat list and `PATH`), `<n>.dbg` (the analyser's
//! stage log) and `<n>.tr` (its `BeatInfoAnalyzer`/`UnitEnvelopeMaker` lists in hex floats),
//! made outside the repo, every stage is compared. Nothing runs without the variable.

use super::BeatInfo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Stage name → the lines the analyser printed (`stat bpm beats pos band`).
fn stages(dbg: &str) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut cur: Option<String> = None;
    for l in dbg.lines() {
        let l = l.trim_end_matches('\r');
        if let Some(h) = l.strip_prefix("[ ").and_then(|h| h.strip_suffix(" ]")) {
            cur = Some(h.to_string());
            out.entry(h.to_string()).or_default();
            continue;
        }
        if let Some(c) = &cur
            && l.split('\t').count() == 5
        {
            out.get_mut(c).unwrap().push(l.to_string());
        }
    }
    out
}

pub(crate) fn fmt(b: &BeatInfo) -> String {
    let bpm = f64::from(60000.0f32 / b.tempo);
    format!("{}\t{:.6}\t{:.6}\t{}\t{}", b.stat, bpm, f64::from(b.beats), b.pos, b.band)
}

/// `BIA_OUT`, `UEM_IN`, `UEM_OUT` lists of a trace file.
fn trace_lists(tr: &str) -> HashMap<String, Vec<BeatInfo>> {
    let mut out: HashMap<String, Vec<BeatInfo>> = HashMap::new();
    let mut lines = tr.lines();
    while let Some(l) = lines.next() {
        let mut t = l.split(' ');
        let (Some(tag), Some(n)) = (t.next(), t.next()) else { continue };
        if !matches!(tag, "BIA_OUT" | "UEM_IN" | "UEM_OUT") {
            continue;
        }
        let n: usize = n.parse().unwrap();
        let v = (0..n)
            .map(|_| {
                let f: Vec<&str> = lines.next().unwrap().split(' ').collect();
                BeatInfo {
                    pos: f[0].parse().unwrap(),
                    beats: hexf(f[1]) as f32,
                    tempo: hexf(f[2]) as f32,
                    stat: f[3].parse().unwrap(),
                    band: f[4].parse().unwrap(),
                    flag: f[5].parse().unwrap(),
                }
            })
            .collect();
        out.insert(tag.to_string(), v);
    }
    out
}

/// A C `%a` float.
fn hexf(s: &str) -> f64 {
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let s = s.strip_prefix("0x").unwrap();
    let (mant, exp) = s.split_once('p').unwrap();
    let (ip, fp) = mant.split_once('.').unwrap_or((mant, ""));
    let mut v = u64::from_str_radix(ip, 16).unwrap() as f64;
    let mut scale = 1.0 / 16.0;
    for c in fp.chars() {
        v += c.to_digit(16).unwrap() as f64 * scale;
        scale /= 16.0;
    }
    let v = v * 2f64.powi(exp.parse().unwrap());
    if neg { -v } else { v }
}

fn decode(path: &Path, varied: bool) -> (u32, super::BeatAnalyzer) {
    let mut dec = mb_audio::open(path).unwrap();
    let info = *dec.info();
    let total = info.total_samples - 1;
    let mut a = if varied {
        super::BeatAnalyzer::new_dynamic(info.sample_rate, total)
    } else {
        super::BeatAnalyzer::new(info.sample_rate, total)
    };
    let block = 2 * info.sample_rate as usize;
    let mut frames = vec![[0.0f32; 2]; block];
    let mut pos = 0u64;
    while pos < total {
        let n = ((total - pos) as usize).min(block);
        let got = dec.read_frames(&mut frames[..n]);
        if got == 0 {
            break;
        }
        let pcm: Vec<[i16; 2]> =
            frames[..got].iter().map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])]).collect();
        a.put(&pcm);
        pos += got as u64;
    }
    (info.sample_rate, a)
}

/// The analyser's log header of a stage.
fn header(stage: &str) -> &'static str {
    match stage {
        "raw" => "選出生データ",
        "sections" => "連続しないBPM区間削除後のデータ",
        "runs8" => "8個以上連続しない値削除後のデータ",
        "close" => "近い値調整後のデータ",
        "segments" => "アジャスト前区間情報",
        "adjusted" => "アジャスト後区間情報",
        "walk" => "区間をBPM値で分割したデータ",
        "corrected" => "位置補正後のデータ",
        _ => "",
    }
}

/// First differing line of two stage dumps.
fn diff(ours: &[String], theirs: &[String]) -> Option<String> {
    if let Some(k) = ours.iter().zip(theirs).position(|(a, b)| a != b) {
        return Some(format!("[{k}] {} vs {}", ours[k], theirs[k]));
    }
    (ours.len() != theirs.len()).then(|| format!("len {} vs {}", ours.len(), theirs.len()))
}

#[test]
fn varied_matches_rekordbox() {
    let Ok(dir) = std::env::var("MB_V1_DIR") else { return };
    let only = std::env::var("MB_V1_ONLY").ok();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "dbg"))
        .filter(|p| only.as_ref().is_none_or(|o| p.file_stem().unwrap().to_str().unwrap() == o))
        .collect();
    files.sort();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let lines = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..6 {
            s.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(f) = files.get(k) else { break };
                let l = check_one(f);
                lines.lock().unwrap().push(l);
            });
        }
    });
    let mut lines = lines.into_inner().unwrap();
    lines.sort();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for l in &lines {
        eprintln!("{l}");
        let stage = l.split(" | ").nth(1).unwrap_or("").split([':', ' ']).next().unwrap_or("").to_string();
        *counts.entry(stage).or_default() += 1;
    }
    let mut c: Vec<_> = counts.into_iter().collect();
    c.sort();
    eprintln!("{} tracks; first difference by stage: {c:?}", lines.len());
}

fn check_one(dbg: &Path) -> String {
    let name = dbg.file_stem().unwrap().to_str().unwrap().to_string();
    let v1 = std::fs::read_to_string(dbg.with_extension("v1")).unwrap();
    let path = v1.lines().find_map(|l| l.strip_prefix("PATH ")).unwrap().to_string();
    let st = stages(&std::fs::read_to_string(dbg).unwrap());
    let tr = trace_lists(&std::fs::read_to_string(dbg.with_extension("tr")).unwrap_or_default());
    let varied = std::env::var_os("MB_V1_CONST").is_none();
    let (rate, a) = decode(Path::new(&path), varied);
    let total = a.total;
    let raw = a.sensor.beats.clone();
    let r = super::info::analyze_stages(rate as i32, raw, total, varied);
    for (stage, ours) in &r.stages {
        let theirs = st.get(header(stage)).cloned().unwrap_or_default();
        let ours: Vec<String> = ours.iter().map(fmt).collect();
        if let Some(d) = diff(&ours, &theirs) {
            return format!("{name} | {stage}: {d}");
        }
    }
    if let (Some(out), Some(want)) = (&r.out, tr.get("BIA_OUT")) {
        if out != want {
            let k = out.iter().zip(want).position(|(a, b)| a != b);
            return format!("{name} | BIA_OUT: len {} vs {} first {k:?}", out.len(), want.len());
        }
    } else if r.stages.len() < super::info::STAGES.len() {
        return format!("{name} | (stages ported: {})", r.stages.len());
    }
    // The whole analyser: the final list as `BA_GetBeatList` gives it.
    let want: Vec<(i32, i32, i32)> = v1
        .lines()
        .filter_map(|l| l.strip_prefix("B "))
        .map(|l| {
            let f: Vec<i32> = l.split(' ').map(|x| x.parse().unwrap()).collect();
            (f[0], f[1], f[2])
        })
        .collect();
    let want_bpm: i32 = v1.split_whitespace().skip_while(|&w| w != "bpm").nth(1).unwrap().parse().unwrap();
    let Some((bpm, list)) = a.finish_raw() else {
        return format!("{name} | final: no grid (want {})", want.len());
    };
    let got: Vec<(i32, i32, i32)> =
        list.iter().map(|b| (b.pos, b.stat, (6_000_000.0f32 / b.tempo + 0.5) as i32)).collect();
    if bpm != want_bpm || got != want {
        let k = got.iter().zip(&want).position(|(x, y)| x != y);
        return format!("{name} | final: bpm {bpm}/{want_bpm} len {}/{} first {k:?}", got.len(), want.len());
    }
    format!("{name} | ok")
}
