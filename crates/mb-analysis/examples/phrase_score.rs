//! Scores the phrase analysis against rekordbox's PSSI (.EXT). Two inputs per track: our whole
//! analysis pass (grid, phrase shift, stages A–D) and rekordbox's stored grid (PQTZ and
//! `djmdContent.BPM`) as the anchor. Usage:
//!   cargo run --release -p mb-analysis --example phrase_score -- [--dump] [--rb-only] [filter]
//! (`MB_ORACLE=oracle-b` for the second fixture, `MB_HP=dynamic` for a high-precision own grid,
//! `MB_URAHAKU=0/1` to override the grid-shift flag for the stored grid, `MB_THREADS` for the
//! worker count.)

#[path = "../tests/common/mod.rs"]
mod common;

use mb_analysis::beats::{Beat, BeatGrid};
use mb_analysis::phrase::PhraseResult;
use mb_anlz::Tag;
use mb_anlz::tags::{BeatGrid as Pqtz, SongStructure};

/// The PSSI fields the analysis decides.
#[derive(Debug, Clone, PartialEq)]
struct Pssi {
    mood: u16,
    end_beat: u16,
    bank: u8,
    unk_1d: u8,
    rows: Vec<[u16; 12]>,
}

impl Pssi {
    fn from_anlz(s: &SongStructure) -> Self {
        Self {
            mood: s.mood,
            end_beat: s.end_beat,
            bank: s.bank,
            unk_1d: s.unk_1d,
            rows: s.phrases.iter().map(|p| p.words).collect(),
        }
    }
    fn from_result(r: &PhraseResult) -> Self {
        Self { mood: r.mood as u16, end_beat: r.end_beat as u16, bank: r.bank as u8, unk_1d: r.unk_14 as u8, rows: r.rows() }
    }
}

#[derive(Default)]
struct Score {
    tracks: usize,
    presence: usize,
    identical: usize,
    mood: usize,
    bank: usize,
    end_beat: usize,
    unk_1d: usize,
    count: usize,
    starts: usize,
    kinds: usize,
    fills: usize,
    phrases: usize,
    phrases_beat: usize,
    phrases_kind: usize,
    phrases_fill: usize,
    phrases_all: usize,
}

impl Score {
    fn add(&mut self, want: &Option<Pssi>, got: &Option<Pssi>) {
        self.tracks += 1;
        self.presence += (want.is_some() == got.is_some()) as usize;
        let (Some(w), Some(g)) = (want, got) else {
            self.identical += (want.is_none() && got.is_none()) as usize;
            return;
        };
        self.identical += (w == g) as usize;
        self.mood += (w.mood == g.mood) as usize;
        self.bank += (w.bank == g.bank) as usize;
        self.end_beat += (w.end_beat == g.end_beat) as usize;
        self.unk_1d += (w.unk_1d == g.unk_1d) as usize;
        self.count += (w.rows.len() == g.rows.len()) as usize;
        let col = |r: &[[u16; 12]], k: &[usize]| r.iter().map(|p| k.iter().map(|&i| p[i]).collect::<Vec<_>>()).collect::<Vec<_>>();
        self.starts += (col(&w.rows, &[1]) == col(&g.rows, &[1])) as usize;
        self.kinds += (col(&w.rows, &[1, 2]) == col(&g.rows, &[1, 2])) as usize;
        self.fills += (col(&w.rows, &[1, 10, 11]) == col(&g.rows, &[1, 10, 11])) as usize;
        self.phrases += w.rows.len();
        for (a, b) in w.rows.iter().zip(&g.rows) {
            self.phrases_beat += (a[1] == b[1]) as usize;
            self.phrases_kind += (a[1] == b[1] && a[2] == b[2]) as usize;
            self.phrases_fill += (a[1] == b[1] && a[10] == b[10] && a[11] == b[11]) as usize;
            self.phrases_all += (a == b) as usize;
        }
    }
    fn print(&self, name: &str) {
        let n = self.tracks;
        println!("{name}: {n} tracks");
        println!("  PSSI present/absent as rekordbox: {}/{n}", self.presence);
        println!("  identical PSSI: {}/{n} ({:.1} %)", self.identical, 100.0 * self.identical as f64 / n.max(1) as f64);
        println!(
            "  fields: mood {}, bank {}, end beat {}, byte 0x1d {}, count {}, starts {}, starts+kinds {}, starts+fills {}",
            self.mood, self.bank, self.end_beat, self.unk_1d, self.count, self.starts, self.kinds, self.fills
        );
        println!(
            "  phrases (by index, of {}): start {}, start+kind {}, start+fill {}, all words {}",
            self.phrases, self.phrases_beat, self.phrases_kind, self.phrases_fill, self.phrases_all
        );
    }
}

fn rb_grid(t: &common::OracleTrack) -> Option<BeatGrid> {
    let s = t.dat.as_ref()?.sections_with(Tag::PQTZ).next()?;
    let q = Pqtz::decode(s).ok()?;
    // Where high precision found no beats (oracle-b's sweeps), rekordbox kept the old grid and
    // ran the phrase pass on it with its BPM, but stored BPM 0: take beat 0's tempo then.
    let bpm = match t.bpm? as i32 {
        0 => i32::from(q.beats.first()?.tempo),
        v => v,
    };
    Some(BeatGrid {
        bpm,
        beats: q
            .beats
            .iter()
            .map(|b| Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo, time_ms: b.time_ms, sub_ms: 0 })
            .collect(),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dump = args.iter().any(|a| a == "--dump");
    let rb_only = args.iter().any(|a| a == "--rb-only");
    let filter = args.iter().find(|a| !a.starts_with("--")).cloned();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|t| filter.as_ref().is_none_or(|f| t.path.to_string_lossy().contains(f.as_str())))
        .collect();
    let hp = std::env::var("MB_HP").is_ok_and(|v| v == "dynamic").then_some(mb_analysis::hpbeat::HpMode::Dynamic);
    // oracle-b was analysed in high-precision Dynamic mode, where rekordbox does not shift the
    // grid (`phrase::run`).
    let urahaku = std::env::var("MB_URAHAKU").map_or(fixture != "oracle-b", |v| v == "1");
    let threads: usize = std::env::var("MB_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(t) = tracks.get(k) else { break };
                let want = t
                    .ext
                    .as_ref()
                    .and_then(|e| e.sections_with(Tag::PSSI).next())
                    .and_then(|s| SongStructure::decode(s).ok())
                    .map(|s| Pssi::from_anlz(&s));
                let rb = rb_grid(t)
                    .and_then(|g| mb_analysis::phrase::analyze_with_grid(&t.path, &g, urahaku))
                    .map(|r| Pssi::from_result(&r));
                let own = if rb_only {
                    None
                } else {
                    let opts = mb_analysis::Options {
                        waveforms: false,
                        loudness: false,
                        beats: true,
                        phrase: true,
                        key: false,
                        high_precision: hp,
                    };
                    mb_analysis::analyze(&t.path, opts).ok().and_then(|a| a.phrase).map(|r| Pssi::from_result(&r))
                };
                out.lock().unwrap().push((k, want, rb, own));
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    out.sort_by_key(|o| o.0);
    let (mut s_rb, mut s_own) = (Score::default(), Score::default());
    for (k, want, rb, own) in &out {
        let name = tracks[*k].path.display().to_string();
        s_rb.add(want, rb);
        if !rb_only {
            s_own.add(want, own);
        }
        let tag = |g: &Option<Pssi>| if g == want { "=" } else { "≠" };
        let head = |p: &Option<Pssi>| match p {
            None => "no PSSI".to_string(),
            Some(p) => format!("mood {} end {} bank {} 0x1d {} phrases {}", p.mood, p.end_beat, p.bank, p.unk_1d, p.rows.len()),
        };
        println!("{name}: {} | rb grid {} {} | own {} {}", head(want), tag(rb), head(rb), tag(own), head(own));
        if dump && (rb != want || (!rb_only && own != want)) {
            for (label, p) in [("want", want), ("rb", rb), ("own", own)] {
                if let Some(p) = p {
                    for r in &p.rows {
                        println!("  {label} {r:?}");
                    }
                }
            }
        }
    }
    s_rb.print(&format!("{fixture}, rekordbox grid"));
    if !rb_only {
        s_own.print(&format!("{fixture}, own analysis{}", if hp.is_some() { " (high precision, Dynamic)" } else { "" }));
    }
}
