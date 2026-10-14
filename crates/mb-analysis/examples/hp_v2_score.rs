//! Scores the high-precision Normal (mode 3) and Auto (mode 2) grids against rekordbox's PQTZ,
//! PQT2 and `djmdContent.BPM`, calling `hpbeat::grid_v2` directly; with `--full`, through
//! `mb_analysis::analyze_with` (the stored grid as the fallback) and PSSI too. Usage:
//!   cargo run --release -p mb-analysis --example hp_v2_score -- <normal|auto> [--full] [--verbose] [filter]
//! Normal scores `oracle-c` without the four sampler loops rekordbox crashed on, Auto scores
//! `oracle-d` (`MB_ORACLE` overrides). `MB_THREADS` limits the tracks analysed at once (2).

#[path = "../tests/common/mod.rs"]
mod common;

use mb_analysis::hpbeat::{self, HpMode};
use mb_anlz::Tag;
use mb_anlz::tags::SongStructure;
use std::sync::Mutex;

/// The PSSI fields the analysis decides.
#[derive(Debug, Clone, PartialEq)]
struct Pssi {
    mood: u16,
    end_beat: u16,
    bank: u8,
    unk_1d: u8,
    rows: Vec<[u16; 12]>,
}

/// One track's result: the high-precision grid, and the PSSI with `--full`.
struct Out {
    hp: hpbeat::HpGrid,
    pssi: Option<Option<Pssi>>,
}

/// rekordbox exits on these in mode 3; their rows were not rewritten (`fixtures/manifest.toml`).
const SKIP_NORMAL: [&str; 4] = ["/House3.wav", "/House4.wav", "/Breaks2.wav", "/Breaks3.wav"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.first().map(String::as_str) {
        Some("normal") => HpMode::Normal,
        Some("auto") => HpMode::Auto,
        _ => {
            eprintln!("usage: hp_v2_score <normal|auto> [--verbose] [filter]");
            return;
        }
    };
    let verbose = args.iter().any(|a| a == "--verbose");
    let full = args.iter().any(|a| a == "--full");
    let filter = args.iter().skip(1).find(|a| !a.starts_with("--")).cloned();
    let default = if mode == HpMode::Normal { "oracle-c" } else { "oracle-d" };
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| default.into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| {
            let p = t.path.to_string_lossy();
            t.ext.as_ref().and_then(|e| e.ext_beat_grid()).is_some()
                && !(mode == HpMode::Normal && SKIP_NORMAL.iter().any(|s| p.ends_with(s)))
                && filter.as_ref().is_none_or(|f| p.contains(f.as_str()))
        })
        .collect();
    let next = Mutex::new(0usize);
    let rows = Mutex::new(Vec::new());
    let threads = std::env::var("MB_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = {
                    let mut n = next.lock().unwrap();
                    *n += 1;
                    *n - 1
                };
                let Some(t) = tracks.get(i) else { break };
                let t0 = std::time::Instant::now();
                let r = if full { run_full(mode, t) } else { run(mode, &t.path).map(|hp| Out { hp, pssi: None }) };
                rows.lock().unwrap().push((i, r, t0.elapsed().as_millis()));
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let (mut n, mut exact, mut bpm_ok, mut pqt2_ok, mut kept, mut pssi_ok) = (0, 0, 0, 0, 0, 0);
    let mut ms_total = 0u128;
    for (i, r, ms) in &rows {
        let t = &tracks[*i];
        let full = t.path.to_string_lossy();
        let name = full.split("/corpus/").last().unwrap_or(&full);
        let rb = t.dat.as_ref().and_then(|d| d.beat_grid()).unwrap();
        let x = t.ext.as_ref().and_then(|e| e.ext_beat_grid()).unwrap();
        let rb_bpm = t.bpm.unwrap_or(0);
        n += 1;
        ms_total += ms;
        let (hp, pssi) = match r {
            Ok(o) => (&o.hp, &o.pssi),
            Err(e) => {
                println!("{name}: error {e}");
                continue;
            }
        };
        if let Some(own) = pssi {
            let want = t
                .ext
                .as_ref()
                .and_then(|e| e.sections_with(Tag::PSSI).next())
                .and_then(|s| SongStructure::decode(s).ok())
                .map(|s| Pssi { mood: s.mood, end_beat: s.end_beat, bank: s.bank, unk_1d: s.unk_1d, rows: s.phrases.iter().map(|p| p.words).collect() });
            let ok = *own == want;
            pssi_ok += usize::from(ok);
            if verbose || !ok {
                let head = |p: &Option<Pssi>| match p {
                    None => "no PSSI".to_string(),
                    Some(p) => format!("mood {} end {} bank {} 0x1d {} phrases {}", p.mood, p.end_beat, p.bank, p.unk_1d, p.rows.len()),
                };
                println!("{name}: PSSI {ok}: ours {} | rekordbox {}", head(own), head(&want));
            }
        }
        let Some(g) = &hp.grid else {
            // rekordbox keeps the previous grid and writes BPM 0
            let ok = rb_bpm == 0;
            kept += usize::from(ok);
            if verbose || !ok {
                println!("{name}: no grid; rekordbox: PQT2 type {}, {} beats, BPM {rb_bpm}", x.analysis_type, rb.beats.len());
            }
            continue;
        };
        let ours: Vec<mb_anlz::tags::Beat> = g
            .beats
            .iter()
            .map(|b| mb_anlz::tags::Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo, time_ms: b.time_ms })
            .collect();
        let mut mine = mb_anlz::tags::ExtBeatGrid::from_beats(&ours, g.beats.iter().map(|b| b.sub_ms).collect());
        mine.analysis_type = hp.grid_type;
        let same = ours == rb.beats;
        let b_ok = i64::from(g.bpm) == rb_bpm;
        exact += usize::from(same);
        bpm_ok += usize::from(b_ok);
        pqt2_ok += usize::from(mine == x);
        if verbose || !same || !b_ok || mine != x {
            let first_diff = ours.iter().zip(&rb.beats).position(|(a, b)| a != b);
            println!(
                "{name}: grid {same} (n {} vs {}, first diff {first_diff:?}) | bpm {} vs {rb_bpm} | PQT2 {} (type {} vs {}) | urahaku {} | {ms} ms",
                ours.len(),
                rb.beats.len(),
                g.bpm,
                mine == x,
                mine.analysis_type,
                x.analysis_type,
                hp.urahaku
            );
            if let Some(k) = first_diff {
                for (j, (o, r)) in ours.iter().zip(&rb.beats).enumerate().skip(k.saturating_sub(1)).take(4) {
                    println!("    {j}: ours {o:?} rb {r:?}");
                }
            }
        }
    }
    println!(
        "{fixture} ({mode:?}), {n} tracks: grids identical {exact}, BPM×100 {bpm_ok}, PQT2 identical {pqt2_ok}; no grid with rekordbox BPM 0: {kept}{}; {} ms per track",
        if full { format!("; PSSI identical {pssi_ok}") } else { String::new() },
        ms_total / n.max(1) as u128
    );
}

/// The whole analysis (beats and phrase), the stored grid as the fallback.
fn run_full(mode: HpMode, t: &common::OracleTrack) -> Result<Out, String> {
    let opts = mb_analysis::Options {
        waveforms: false,
        loudness: false,
        beats: true,
        phrase: true,
        key: false,
        high_precision: Some(mode),
        vocal: false,
        dynamic: false,
        auto_cue: false,
    };
    let a = mb_analysis::analyze_with(&t.path, opts, t.stored_grid().as_ref()).map_err(|e| e.to_string())?;
    if let Some(e) = a.high_precision_error {
        return Err(format!("high precision failed: {e}"));
    }
    let pssi = a.phrase.map(|r| Pssi { mood: r.mood as u16, end_beat: r.end_beat as u16, bank: r.bank as u8, unk_1d: r.unk_14 as u8, rows: r.rows() });
    Ok(Out { hp: hpbeat::HpGrid { grid: a.beats, grid_type: a.grid_type, urahaku: false }, pssi: Some(pssi) })
}

/// The decode pass as `mb_analysis::analyze` runs it, then the high-precision job.
fn run(mode: HpMode, path: &std::path::Path) -> Result<hpbeat::HpGrid, String> {
    let mut dec = mb_audio::open(path).map_err(|e| e.to_string())?;
    let info = *dec.info();
    let frames = dec.decode_all();
    let total = info.total_samples.saturating_sub(1).min(frames.len() as u64);
    let frames = &frames[..total as usize];
    let mut balib = mb_analysis::balib2::Analyzer::new(info.sample_rate, total);
    let pcm: Vec<[i16; 2]> = frames.iter().map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])]).collect();
    balib.put(&pcm);
    let mono: Vec<f32> = frames.iter().map(|f| (f[1] + f[0]) * 0.5).collect();
    hpbeat::grid_v2(mode, &mono, info.sample_rate, total, balib)
}
