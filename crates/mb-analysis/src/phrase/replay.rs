//! Replay tests for the stage-D functions: each recorded call of a function (its arguments
//! before and after the call, in the call-trace layout below) is run through the port and the
//! outputs are compared exactly. The traces are not part of the repo; the tests do nothing
//! unless `MB_PHRASE_TRACE` names a directory of `*.tr` files.
//!
//! Layout: text lines `N <id> <name> <kinds>` up to `DATA`, then records `B`|`E` (u8), function
//! id (u32), call number (u32), argument count (u8) and per argument its kind (u8) and value:
//! `i u b l r c` i64; `d f q` f64; `F I U` i32 length (−1 = none) and the elements (f64, i32,
//! u8); `G` / `V` i32 rows, i32 columns and the elements (f64 / u8); `g` rows, columns and a
//! hash; `p` u64. An `E` record ends with the returned x0 (i64) and d0 (f64). `M` records carry
//! a u32 length and text.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Int(i64),
    Real(f64),
    F(Option<Vec<f64>>),
    I(Option<Vec<i32>>),
    U(Option<Vec<u8>>),
    G(i32, i32, Option<Vec<f64>>),
    V(i32, i32, Vec<u8>),
    Hash(i32, i32, u64),
    Ptr(u64),
}

impl Arg {
    pub fn int(&self) -> i32 {
        match self {
            Arg::Int(v) => *v as i32,
            a => panic!("not an int: {a:?}"),
        }
    }
    pub fn real(&self) -> f64 {
        match self {
            Arg::Real(v) => *v,
            a => panic!("not a real: {a:?}"),
        }
    }
    pub fn f(&self) -> Vec<f64> {
        match self {
            Arg::F(v) => v.clone().unwrap_or_default(),
            a => panic!("not F: {a:?}"),
        }
    }
    pub fn i(&self) -> Vec<i32> {
        match self {
            Arg::I(v) => v.clone().unwrap_or_default(),
            a => panic!("not I: {a:?}"),
        }
    }
    pub fn u(&self) -> Vec<u8> {
        match self {
            Arg::U(v) => v.clone().unwrap_or_default(),
            a => panic!("not U: {a:?}"),
        }
    }
    /// A `MatArrayFloat2` as rows.
    pub fn g(&self) -> Vec<Vec<f64>> {
        match self {
            Arg::G(r, c, Some(v)) if *r > 0 && *c > 0 => {
                v.chunks(*c as usize).map(<[f64]>::to_vec).collect()
            }
            Arg::G(..) => Vec::new(),
            a => panic!("not G: {a:?}"),
        }
    }
    /// A `MatArrayUint82` as rows.
    pub fn v(&self) -> Vec<Vec<u8>> {
        match self {
            Arg::V(r, c, v) if *r > 0 && *c > 0 => {
                v.chunks(*c as usize).map(<[u8]>::to_vec).collect()
            }
            Arg::V(..) => Vec::new(),
            a => panic!("not V: {a:?}"),
        }
    }
    pub fn has_data(&self) -> bool {
        !matches!(self, Arg::Hash(..))
    }
}

#[derive(Debug, Clone)]
pub struct Call {
    pub file: PathBuf,
    pub seq: u32,
    pub before: Vec<Arg>,
    pub after: Vec<Arg>,
    pub ret: i64,
    pub fret: f64,
}

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        s
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn i32(&mut self) -> i32 {
        self.u32() as i32
    }
    fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn f64(&mut self) -> f64 {
        f64::from_bits(self.i64() as u64)
    }
    fn arg(&mut self) -> Arg {
        match self.u8() {
            b'i' | b'u' | b'b' | b'l' | b'r' | b'c' => Arg::Int(self.i64()),
            b'd' | b'f' | b'q' => Arg::Real(self.f64()),
            b'F' => {
                let n = self.i32();
                Arg::F((n >= 0).then(|| (0..n).map(|_| self.f64()).collect()))
            }
            b'I' => {
                let n = self.i32();
                Arg::I((n >= 0).then(|| (0..n).map(|_| self.i32()).collect()))
            }
            b'U' => {
                let n = self.i32();
                Arg::U((n >= 0).then(|| self.take(n.max(0) as usize).to_vec()))
            }
            b'G' => {
                let (r, c) = (self.i32(), self.i32());
                let n = if r > 0 && c > 0 { (r * c) as usize } else { 0 };
                Arg::G(r, c, Some((0..n).map(|_| self.f64()).collect()))
            }
            b'V' => {
                let (r, c) = (self.i32(), self.i32());
                let n = if r > 0 && c > 0 { (r * c) as usize } else { 0 };
                Arg::V(r, c, self.take(n).to_vec())
            }
            b'g' => {
                let (r, c) = (self.i32(), self.i32());
                Arg::Hash(r, c, self.i64() as u64)
            }
            _ => Arg::Ptr(self.i64() as u64),
        }
    }
}

/// The calls of `name` in one trace file.
pub fn load(path: &Path, name: &str) -> Vec<Call> {
    let b = std::fs::read(path).unwrap();
    let mut names = HashMap::new();
    let mut i = 0;
    loop {
        let j = i + b[i..].iter().position(|&c| c == b'\n').unwrap();
        let line = std::str::from_utf8(&b[i..j]).unwrap();
        i = j + 1;
        if line == "DATA" {
            break;
        }
        let mut t = line.split(' ');
        t.next();
        let id: u32 = t.next().unwrap().parse().unwrap();
        names.insert(id, t.next().unwrap().to_string());
    }
    let mut r = Rd { b: &b, i };
    let mut open: HashMap<u32, Vec<Arg>> = HashMap::new();
    let mut out = Vec::new();
    while r.i < b.len() {
        let t = r.u8();
        if t == b'M' {
            let n = r.u32() as usize;
            r.take(n);
            continue;
        }
        let (id, seq, na) = (r.u32(), r.u32(), r.u8());
        let args: Vec<Arg> = (0..na).map(|_| r.arg()).collect();
        if t == b'B' {
            if names[&id] == name {
                open.insert(seq, args);
            }
        } else {
            let (ret, fret) = (r.i64(), r.f64());
            if let Some(before) = open.remove(&seq) {
                out.push(Call { file: path.to_path_buf(), seq, before, after: args, ret, fret });
            }
        }
    }
    out
}

/// Every recorded call of `name` (empty without `MB_PHRASE_TRACE`).
pub fn calls(name: &str) -> Vec<Call> {
    let Ok(dir) = std::env::var("MB_PHRASE_TRACE") else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "tr"))
        .collect();
    files.sort();
    files.iter().flat_map(|f| load(f, name)).collect()
}

/// Collects mismatches over many calls and fails once at the end with a summary.
#[derive(Default)]
pub struct Check {
    pub calls: usize,
    pub bad: usize,
    first: Vec<String>,
}

impl Check {
    pub fn call(&mut self, c: &Call, ok: Vec<(&str, Option<String>)>) {
        self.calls += 1;
        let errs: Vec<String> = ok
            .into_iter()
            .filter_map(|(what, e)| e.map(|e| format!("{what}: {e}")))
            .collect();
        if !errs.is_empty() {
            self.bad += 1;
            if self.first.len() < 12 {
                self.first.push(format!("{} #{}: {}", c.file.display(), c.seq, errs.join("; ")));
            }
        }
    }
    pub fn finish(self, name: &str) {
        eprintln!("{name}: {} calls, {} differ", self.calls, self.bad);
        for f in &self.first {
            eprintln!("  {f}");
        }
        assert_eq!(self.bad, 0, "{name}: {} of {} calls differ", self.bad, self.calls);
    }
}

/// Bit-exact comparison of float arrays (first difference).
pub fn same_f(got: &[f64], want: &[f64]) -> Option<String> {
    if got.len() != want.len() {
        return Some(format!("len {} vs {}", got.len(), want.len()));
    }
    let k = got.iter().zip(want).position(|(a, b)| a.to_bits() != b.to_bits())?;
    Some(format!("[{k}] {} vs {}", got[k], want[k]))
}

pub fn same<T: PartialEq + std::fmt::Debug>(got: &[T], want: &[T]) -> Option<String> {
    if got.len() != want.len() {
        return Some(format!("len {} vs {}", got.len(), want.len()));
    }
    let k = got.iter().zip(want).position(|(a, b)| a != b)?;
    Some(format!("[{k}] {:?} vs {:?}", got[k], want[k]))
}

pub fn same_v<T: PartialEq + std::fmt::Debug>(got: T, want: T) -> Option<String> {
    (got != want).then(|| format!("{got:?} vs {want:?}"))
}

pub fn same_r(got: f64, want: f64) -> Option<String> {
    (got.to_bits() != want.to_bits()).then(|| format!("{got} vs {want}"))
}

/// Runs `f` on every recorded call of `name` and fails on any difference.
pub fn run(name: &str, f: impl Fn(&Call) -> Vec<(&'static str, Option<String>)>) {
    let calls = calls(name);
    let mut check = Check::default();
    for c in &calls {
        let r = f(c);
        check.call(c, r);
    }
    check.finish(name);
}

mod tests {
    use super::super::{adjust, blocks, d_feat, genre, intro, melo, verse};
    use super::*;

    #[test]
    fn low_level_b() {
        run("LowLevelAnalyze_b", |c| {
            let (sums, flags) = d_feat::low_level_b(&c.before[0].f(), c.before[1].int());
            vec![("sums", same_f(&sums, &c.after[2].f())), ("flags", same(&flags, &c.after[3].u()))]
        });
    }

    #[test]
    fn middle_level_b() {
        run("MiddleLevelAnalyze_b", |c| {
            let (flags, sums) = d_feat::middle_level_b(&c.before[0].f(), c.before[1].int());
            vec![("flags", same(&flags, &c.after[2].u())), ("sums", same_f(&sums, &c.after[3].f()))]
        });
    }

    #[test]
    fn middle_peak_b() {
        run("MiddlePeakAnalyze_b", |c| {
            let mut peak = c.before[0].f();
            let means = d_feat::middle_peak_b(&mut peak, c.before[1].int(), c.before[2].int());
            vec![("peak", same_f(&peak, &c.after[0].f())), ("means", same_f(&means, &c.after[3].f()))]
        });
    }

    #[test]
    fn all_level_b() {
        run("AllLevelAnalyze_b", |c| {
            let (a, b) = d_feat::all_level_b(&c.before[0].f(), c.before[1].int());
            vec![("out0", same_f(&a, &c.after[2].f())), ("out1", same_f(&b, &c.after[3].f()))]
        });
    }

    #[test]
    fn hihat_b() {
        run("HiHatAnalyze_b", |c| {
            let flags = d_feat::hihat_b(&c.before[0].u(), c.before[1].int());
            vec![("flags", same(&flags, &c.after[2].u()))]
        });
    }

    #[test]
    fn high_peak_b() {
        run("HighPeakAnalyze32_b", |c| {
            let (p, n) = d_feat::high_peak_b(&c.before[0].f(), c.before[1].int());
            vec![("peaks", same_f(&p, &c.after[2].f())), ("count", same_v(n, c.after[3].int()))]
        });
    }

    #[test]
    fn findpeaks_ex() {
        run("findpeaks_ex", |c| {
            let p = d_feat::findpeaks_ex(&c.before[0].f());
            let ok = c.ret & 1 != 0;
            match p {
                None => vec![("ret", same_v(false, ok))],
                Some(p) => vec![
                    ("ret", same_v(true, ok)),
                    ("vals", same_f(&p.vals, &c.after[1].f())),
                    ("locs", same(&p.locs, &c.after[2].i())),
                    ("widths", same_f(&p.widths, &c.after[3].f())),
                    ("proms", same_f(&p.proms, &c.after[4].f())),
                ],
            }
        });
    }

    #[test]
    fn detect_comp() {
        run("DetectComp", |c| {
            let (a, b) = genre::detect_comp(&c.before[0].f(), &c.before[1].f(), &c.before[3].u(), c.before[4].int());
            vec![("share", same_r(a, c.after[5].real())), ("level", same_r(b, c.after[6].real()))]
        });
    }

    #[test]
    fn fade_out() {
        run("DetectFadeOut", |c| {
            let v = genre::fade_out(&c.before[0].f(), &c.before[1].f());
            vec![("flag", same_v(v, c.after[3].int()))]
        });
    }

    #[test]
    fn max_level_ratio() {
        run("CalcMaxLevelRatio", |c| {
            vec![("ratio", same_r(genre::max_level_ratio(&c.before[0].f()), c.after[1].real()))]
        });
    }

    #[test]
    fn bpm128() {
        run("DetectBPM128", |c| {
            let (a, b) = genre::bpm128(c.before[0].real());
            vec![("a", same_v(a, c.after[1].int())), ("b", same_v(b, c.after[2].int()))]
        });
    }

    #[test]
    fn tenkei() {
        run("DetectTenkeiGenre1", |c| {
            let v = genre::tenkei_genre1(c.before[0].real(), c.before[1].int() != 0);
            vec![("flag", same_v(v, c.after[2].int()))]
        });
    }

    #[test]
    fn low_power_ratio() {
        run("CalcTrackLowPowerRatio", |c| {
            let r = genre::track_low_power_ratio(&c.before[0].f(), &c.before[1].f(), c.before[2].int());
            vec![
                ("ratio", same_r(r.ratio, c.after[3].real())),
                ("bar_ratio", same_f(&r.bar_ratio, &c.after[4].f())),
                ("bar_low", same_f(&r.bar_low, &c.after[5].f())),
                ("mean_ratio", same_r(r.mean_ratio, c.after[6].real())),
                ("mean_low", same_r(r.mean_low, c.after[7].real())),
            ]
        });
    }

    #[test]
    fn slow_tempo() {
        run("DetectSlowTempo", |c| {
            let b = &c.before;
            let (s, h) = genre::slow_tempo(
                b[0].real(),
                &b[1].f(),
                b[2].real(),
                &b[3].i(),
                b[5].int(),
                b[6].int(),
                &b[7].i(),
                b[8].int(),
                b[9].int(),
                b[10].int(),
            );
            vec![("slow", same_v(s, c.after[11].int())), ("half", same_v(h, c.after[12].int()))]
        });
    }

    #[test]
    fn four_kicks() {
        run("Detect4KicksPopRockRhythm", |c| {
            let k = genre::four_kicks(&c.before[0].u(), c.before[1].real(), &c.before[2].f());
            let a = &c.after;
            vec![("p4..p7", same_v(k, (a[3].int(), a[4].int(), a[5].int(), a[6].int())))]
        });
    }

    #[test]
    fn genre_devide() {
        run("GenreDevideOverall", |c| {
            let b = &c.before;
            let g = genre::genre_devide(
                b[0].int(),
                b[1].int(),
                b[2].real(),
                (b[6].int(), b[3].int(), b[4].int(), b[5].int()),
                b[7].int(),
                b[17].real(),
                (b[12].int(), b[13].int()),
                b[14].int(),
            );
            vec![("genre", same_v(g, c.after[18].int()))]
        });
    }

    #[test]
    fn genre12_final() {
        run("Genre12FinalJudge", |c| {
            let mut g = c.before[0].int();
            genre::genre12_final(&mut g, c.before[1].int(), &c.before[2].f(), &c.before[3].f());
            vec![("genre", same_v(g, c.after[0].int()))]
        });
    }

    #[test]
    fn secure_points() {
        run("FindSecurePoint", |c| {
            let s = genre::secure_points(&c.before[0].f(), c.before[1].int());
            vec![("points", same(&s, &c.after[2].i()))]
        });
    }

    #[test]
    fn block_average() {
        run("BlockAverage", |c| {
            let b = &c.before;
            match blocks::block_average(&b[0].i(), &b[1].f(), &b[2].f(), b[3].int(), &b[5].f()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(m) => vec![
                    ("level", same_f(&m.level, &c.after[6].f())),
                    ("peak", same_f(&m.peak, &c.after[7].f())),
                    ("beat_peak", same_f(&m.beat_peak, &c.after[8].f())),
                ],
            }
        });
    }

    #[test]
    fn up_simpleloop() {
        run("UpSimpleloopAna", |c| {
            let b = &c.before;
            let mut labels = b[1].i();
            let (starts, peak) = blocks::up_simpleloop(&b[0].i(), &mut labels, &b[2].f(), &b[3].f(), &b[4].f());
            vec![
                ("labels", same(&labels, &c.after[1].i())),
                ("starts", same(&starts, &c.after[6].i())),
                ("peak", same_f(&peak, &c.after[7].f())),
            ]
        });
    }

    #[test]
    fn up_4_8_16() {
        run("Up4_8_16Ana", |c| {
            let b = &c.before;
            let o = blocks::up_4_8_16(&b[0].i(), &b[1].i(), &b[2].u());
            let a = &c.after;
            vec![
                ("labels", same(&o.labels, &a[3].i())),
                ("a", same(&o.a, &a[4].i())),
                ("b", same(&o.b, &a[5].i())),
                ("c", same(&o.c, &a[6].i())),
                ("d", same(&o.d, &a[7].i())),
            ]
        });
    }

    #[test]
    fn uta_umu() {
        run("UtaUmuAnalyze_a", |c| {
            if !c.before[0].has_data() {
                return Vec::new();
            }
            match d_feat::uta_umu(&c.before[0].g()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("lines", same_f(&v, &c.after[1].f()))],
            }
        });
    }

    #[test]
    fn intro_end_find_b() {
        run("IntroEndFind_b", |c| {
            let b = &c.before;
            if !b[2].has_data() {
                return Vec::new();
            }
            let mut cqt = b[2].g();
            let (strong, jump, level) = d_feat::intro_end_find_b(&mut cqt, b[3].int(), b[4].int(), b[5].int());
            let flat: Vec<f64> = cqt.concat();
            let want = match &c.after[2] {
                Arg::G(_, _, Some(v)) => v.clone(),
                _ => Vec::new(),
            };
            vec![
                ("cqt", same_f(&flat, &want)),
                ("strong", same(&strong, &c.after[6].u())),
                ("jump", same(&jump, &c.after[7].u())),
                ("level", same_f(&level, &c.after[8].f())),
            ]
        });
    }

    #[test]
    fn type_genre_judge() {
        run("SongStructureAna_Type_Genre_Judge", |c| {
            let b = &c.before;
            let (mut g, mut m) = (b[6].int(), b[7].int());
            let ok = genre::type_genre_judge(&b[0].f(), b[1].int(), b[2].real(), &b[4].i(), &b[5].i(), &mut g, &mut m);
            vec![
                ("ret", same_v(ok.is_some(), c.ret & 1 != 0)),
                ("genre", same_v(g, c.after[6].int())),
                ("mood", same_v(m, c.after[7].int())),
            ]
        });
    }

    #[test]
    fn verse_hook_ana_data23() {
        run("VerseHookAnaData23", |c| {
            let b = &c.before;
            let d = verse::verse_hook_ana_data23(&b[0].f(), &b[1].f(), &b[2].f(), &b[3].f(), b[4].int());
            let a = &c.after;
            let mut out = Vec::new();
            for (k, (name, band)) in [("peak", &d.peak), ("low", &d.low), ("mid_bar", &d.mid_bar), ("mid", &d.mid)].into_iter().enumerate() {
                let base = 5 + 3 * k;
                out.push((name, same_f(&band.norm, &a[base].f())));
                out.push(("spread", same_f(&band.spread, &a[base + 1].f())));
                out.push(("raw", same_f(&band.raw, &a[base + 2].f())));
            }
            out
        });
    }

    #[test]
    fn verse_hook_low_peak() {
        run("VerseHookJudge_LowPeak", |c| {
            let b = &c.before;
            match verse::verse_hook_low_peak(b[0].int(), &b[1].f(), b[2].int(), &b[3].u()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((n, list)) => vec![
                    ("count", same_v(n, c.after[4].int())),
                    ("list", same(&list, &c.after[5].i())),
                ],
            }
        });
    }

    #[test]
    fn break_find() {
        run("BreakFind", |c| {
            let b = &c.before;
            let r = verse::break_find(b[0].int(), &b[1].i(), b[2].int(), &b[3].f(), &b[4].f(), &b[5].f());
            vec![("out", same_v(r, (c.after[6].int(), c.after[7].int(), c.after[8].int())))]
        });
    }

    #[test]
    fn big_change_find() {
        run("BigChangeFind", |c| {
            let b = &c.before;
            let r = verse::big_change_find(b[0].int(), &b[1].i(), b[2].int(), &b[3].f(), &b[4].f(), &b[5].f(), &b[6].f(), &b[7].f());
            let a = &c.after;
            vec![("out", same_v(r, (a[8].int(), a[9].int(), a[10].int(), a[11].int())))]
        });
    }

    #[test]
    fn irregular_judge() {
        run("IrregularJudge", |c| {
            let b = &c.before;
            let a = verse::Irregular {
                intro: b[0].int(),
                bar: b[4].int(),
                gap: b[5].int(),
                brk: (b[6].int(), b[7].int()),
                deep: b[13].int(),
                change1: (b[8].int(), b[9].int()),
                change2: (b[10].int(), b[11].int()),
            };
            match verse::irregular_judge(a, &b[2].i(), &b[3].f(), &b[12].u()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((kind, list, last)) => vec![
                    ("kind", same_v(kind, c.after[14].int())),
                    ("list", same(&list, &c.after[15].i())),
                    ("last", same(&last, &c.after[2].i())),
                ],
            }
        });
    }

    #[test]
    fn ratio_calc_verse() {
        run("RatioCalc_Verse", |c| {
            let b = &c.before;
            let mut cv = verse::RatioCurves {
                peaks: b[1].f(),
                low: b[2].f(),
                mid: b[3].f(),
                level: b[4].f(),
                all: b[5].f(),
            };
            let r = verse::ratio_calc_verse(b[0].int(), &mut cv, &b[6].f(), b[7].real());
            let a = &c.after;
            vec![
                ("out", same_r(r, a[8].real())),
                ("peaks", same_f(&cv.peaks, &a[1].f())),
                ("low", same_f(&cv.low, &a[2].f())),
                ("mid", same_f(&cv.mid, &a[3].f())),
                ("level", same_f(&cv.level, &a[4].f())),
                ("all", same_f(&cv.all, &a[5].f())),
            ]
        });
    }

    #[test]
    fn verse_hook_judge_all() {
        run("VerseHookJudge_All", |c| {
            let b = &c.before;
            let mut cv = verse::RatioCurves {
                peaks: b[8].f(),
                low: b[9].f(),
                mid: b[10].f(),
                level: b[11].f(),
                all: b[12].f(),
            };
            match verse::verse_hook_judge_all(b[0].int(), &b[1].i(), b[3].int(), &b[4].f(), &b[5].u(), &b[6].u(), &b[7].u(), &mut cv) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("starts", same(&v, &c.after[13].i()))],
            }
        });
    }

    #[test]
    fn candidate_adjust() {
        run("CandidateAdjust", |c| {
            let b = &c.before;
            let v = verse::candidate_adjust(&b[0].i(), &b[2].f(), &b[3].f());
            vec![("out", same(&v, &c.after[4].i()))]
        });
    }

    #[test]
    fn rhythm_pattern_reject() {
        run("RhythmPatternReject", |c| {
            let b = &c.before;
            match verse::rhythm_pattern_reject(&b[0].i(), &b[1].u()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((list, mean, sims, flags)) => vec![
                    ("list", same(&list, &c.after[3].i())),
                    ("mean", same_r(mean, c.after[4].real())),
                    ("sims", same_f(&sims, &c.after[5].f())),
                    ("flags", same(&flags, &c.after[6].u())),
                ],
            }
        });
    }

    #[test]
    fn verse_hook_judge23_sub() {
        run("VerseHookJudge23Sub", |c| {
            let b = &c.before;
            let mut out = b[4].i();
            verse::verse_hook_judge23_sub(&b[0].f(), &b[1].f(), &b[2].f(), b[3].int(), &mut out);
            vec![("out", same(&out, &c.after[4].i()))]
        });
    }

    #[test]
    fn verse_hook_judge23() {
        run("VerseHookJudge23", |c| {
            let b = &c.before;
            let band = |k: usize| verse::Band { norm: b[k].f(), spread: b[k + 1].f(), raw: b[k + 2].f() };
            match verse::verse_hook_judge23(&band(0), &band(3), &band(6), &b[9].i(), b[10].int(), b[11].int(), b[12].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("starts", same(&v, &c.after[13].i()))],
            }
        });
    }

    #[test]
    fn short_break_detection() {
        run("ShortBreakDetection", |c| {
            let b = &c.before;
            let v = verse::short_break_detection(&b[0].f(), &b[1].f(), b[2].int(), b[3].int());
            vec![("out", same(&v, &c.after[5].i()))]
        });
    }

    #[test]
    fn snare_fillin_det() {
        run("SnareFillinDet", |c| {
            let b = &c.before;
            match verse::snare_fillin_det(&b[0].f(), b[2].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("out", same(&v, &c.after[3].i()))],
            }
        });
    }

    #[test]
    fn verse_hook_add_genre1() {
        run("VerseHookAdd_genre1", |c| {
            let b = &c.before;
            match verse::verse_hook_add_genre1(&b[0].f(), &b[1].f(), &b[2].f(), &b[3].f(), &b[4].f(), &b[5].f(), &b[6].i(), &b[7].i(), b[8].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("out", same(&v, &c.after[9].i()))],
            }
        });
    }

    #[test]
    fn souon_change_det() {
        run("SouonChangeDet", |c| {
            vec![("out", same(&verse::souon_change_det(&c.before[0].f()), &c.after[1].i()))]
        });
    }

    #[test]
    fn vocal_in_detection() {
        run("VocalInDetection", |c| {
            vec![("out", same(&verse::vocal_in_detection(&c.before[0].f()), &c.after[2].i()))]
        });
    }

    #[test]
    fn candidates_adjustment() {
        run("CandidatesAdjustment", |c| {
            let b = &c.before;
            let v = verse::candidates_adjustment(&b[0].i(), &b[1].i(), b[2].int(), b[3].int());
            vec![("out", same(&v, &c.after[4].i()))]
        });
    }

    #[test]
    fn verse_hook_add_genre23() {
        run("VerseHookAdd_genre23", |c| {
            let b = &c.before;
            match verse::verse_hook_add_genre23(&b[0].f(), &b[1].f(), &b[2].f(), &b[3].f(), &b[4].f(), &b[5].i(), b[6].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("out", same(&v, &c.after[7].i()))],
            }
        });
    }

    #[test]
    fn song_structure_judge() {
        run("SongStructureJudge", |c| {
            let b = &c.before;
            let (mut level, mut peaks) = (b[1].f(), b[2].f());
            let (labels, flag) = verse::song_structure_judge(&b[0].i(), &mut level, &mut peaks);
            vec![
                ("labels", same(&labels, &c.after[5].i())),
                ("flag", same_v(flag, c.after[6].int())),
                ("level", same_f(&level, &c.after[1].f())),
                ("peaks", same_f(&peaks, &c.after[2].f())),
            ]
        });
    }

    #[test]
    fn beat4_ana() {
        run("Beat4Ana", |c| {
            let b = &c.before;
            let v = verse::beat4_ana(&b[0].i(), b[1].int(), &b[2].f(), &b[3].f());
            vec![("out", same_v(v, c.after[4].int()))]
        });
    }

    #[test]
    fn outro_kickout_ana() {
        run("OutroKickoutAna", |c| {
            let b = &c.before;
            vec![("out", same(&verse::outro_kickout_ana(&b[0].i(), &b[1].i(), &b[2].u()), &c.after[3].i()))]
        });
    }

    #[test]
    fn fillin_cut_ana() {
        run("FillinCutAna", |c| {
            let b = &c.before;
            let v = verse::fillin_cut_ana(&b[0].i(), b[1].int(), &b[2].f(), &b[3].f());
            vec![("out", same_v(v, (c.after[4].int(), c.after[5].int())))]
        });
    }

    #[test]
    fn fillin_snare_ana() {
        run("FillinSnareAna", |c| {
            let b = &c.before;
            let v = verse::fillin_snare_ana(&b[0].i(), b[1].int(), &b[2].f());
            vec![("out", same_v(v, (c.after[3].int(), c.after[4].int())))]
        });
    }

    #[test]
    fn fillin_snare_ana_ex() {
        run("FillinSnareAnaEX", |c| {
            let b = &c.before;
            match verse::fillin_snare_ana_ex(&b[0].i(), &b[1].f(), b[2].int(), b[3].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("out", same_v((v, 0), (c.after[4].int(), c.after[5].int())))],
            }
        });
    }

    #[test]
    fn intro_feature_ana() {
        run("IntroFeatureAna", |c| {
            let b = &c.before;
            let (mut labels, mut fills) = (b[1].i(), b[5].i());
            match verse::intro_feature_ana(&b[0].i(), &mut labels, &b[2].f(), &b[3].f(), &b[4].f(), &mut fills, &b[6].f(), b[7].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((s, e)) => vec![
                    ("labels", same(&labels, &c.after[1].i())),
                    ("fills", same(&fills, &c.after[5].i())),
                    ("cut", same_v((s as i64, e as i64), (c.after[8].int() as i64, c.after[9].int() as i64))),
                ],
            }
        });
    }

    #[test]
    fn up_simpleloop_b() {
        run("UpSimpleloopAna_b", |c| {
            let b = &c.before;
            let mut labels = b[1].i();
            verse::up_simpleloop_b(&b[0].i(), &mut labels, &b[2].i(), &b[3].f());
            vec![("labels", same(&labels, &c.after[1].i()))]
        });
    }

    #[test]
    fn up_feature_ana() {
        run("UpFeatureAna", |c| {
            let b = &c.before;
            let (mut labels, mut fills) = (b[1].i(), b[4].i());
            match verse::up_feature_ana(&b[0].i(), &mut labels, &b[2].f(), &b[3].f(), &mut fills, &b[5].f(), b[6].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((s, e)) => vec![
                    ("labels", same(&labels, &c.after[1].i())),
                    ("fills", same(&fills, &c.after[4].i())),
                    ("starts", same(&s, &c.after[7].i())),
                    ("ends", same(&e, &c.after[8].i())),
                ],
            }
        });
    }

    #[test]
    fn hook_feature_ana() {
        run("HookFeatureAna", |c| {
            let b = &c.before;
            let (mut labels, mut fills) = (b[1].i(), b[5].i());
            match verse::hook_feature_ana(&b[0].i(), &mut labels, &b[2].f(), &b[3].f(), &b[4].f(), &mut fills, &b[6].f(), b[7].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((s, e)) => vec![
                    ("labels", same(&labels, &c.after[1].i())),
                    ("fills", same(&fills, &c.after[5].i())),
                    ("starts", same(&s, &c.after[8].i())),
                    ("ends", same(&e, &c.after[9].i())),
                ],
            }
        });
    }

    #[test]
    fn outro_feature_ana() {
        run("OutroFeatureAna", |c| {
            let b = &c.before;
            let (labels, ends) = verse::outro_feature_ana(&b[0].i(), &b[1].i(), &b[2].f(), &b[3].u(), &b[4].f(), b[5].int());
            vec![("labels", same(&labels, &c.after[6].i())), ("ends", same(&ends, &c.after[7].i()))]
        });
    }

    #[test]
    fn fill_adjust_ana() {
        run("FillAdjustAna", |c| {
            let b = &c.before;
            let (mut labels, mut fills, mut out) = (b[1].i(), b[2].i(), b[6].i());
            adjust::fill_adjust_ana(&b[0].i(), &mut labels, &mut fills, b[3].int(), &b[4].f(), &b[5].f(), &mut out);
            vec![
                ("labels", same(&labels, &c.after[1].i())),
                ("fills", same(&fills, &c.after[2].i())),
                ("out", same(&out, &c.after[6].i())),
            ]
        });
    }

    #[test]
    fn feature_last_adjust() {
        run("FeatureLastAdjust", |c| {
            let b = &c.before;
            let (mut labels, mut fills) = (b[1].i(), b[2].i());
            adjust::feature_last_adjust(&b[0].i(), &mut labels, &mut fills, &b[3].i(), b[4].int(), &b[5].f());
            vec![("labels", same(&labels, &c.after[1].i())), ("fills", same(&fills, &c.after[2].i()))]
        });
    }

    #[test]
    fn tenkaimei_analyze() {
        run("TenkaimeiAnalyze", |c| {
            let b = &c.before;
            let mut labels = b[1].i();
            let ok = melo::tenkaimei_analyze(&b[0].v(), &mut labels);
            vec![("ret", same_v(ok as i64, c.ret & 1)), ("labels", same(&labels, &c.after[1].i()))]
        });
    }

    #[test]
    fn yokuyo_analyze() {
        run("YokuyoAnalyze", |c| {
            let b = &c.before;
            let (flag, d) = melo::yokuyo_analyze(&b[0].f(), &b[1].i());
            vec![("flag", same_v(flag, c.after[3].int())), ("d", same_r(d, c.after[4].real()))]
        });
    }

    #[test]
    fn detect_slowrock() {
        run("DetectSlowrock", |c| {
            let b = &c.before;
            vec![("out", same_v(genre::detect_slowrock(&b[0].u(), &b[1].u()), c.after[3].int()))]
        });
    }

    #[test]
    fn hooksolo_analyze() {
        run("HooksoloAnalyze", |c| {
            let b = &c.before;
            let out = melo::hooksolo_analyze(&b[0].f(), &b[1].f(), &b[2].i(), &b[3].f(), b[4].int());
            vec![("out", same(&out, &c.after[5].i()))]
        });
    }

    #[test]
    fn ab_melo_analyze() {
        run("ABmeloAnalyze", |c| {
            let b = &c.before;
            if !matches!(b[0], Arg::G(..)) {
                return vec![];
            }
            match melo::ab_melo_analyze(&b[0].g(), &b[1].i(), b[2].int(), b[3].int()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some(v) => vec![("similar", same(&v, &c.after[4].v()))],
            }
        });
    }

    #[test]
    fn intro_end_judge() {
        run("IntroEndJudge", |c| {
            let b = &c.before;
            let (mut low, mut mid) = (b[1].f(), b[2].f());
            match intro::intro_end_judge(&b[0].f(), &mut low, &mut mid, b[3].int(), &b[4].f()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((list, end, flag)) => vec![
                    ("low", same_f(&low, &c.after[1].f())),
                    ("mid", same_f(&mid, &c.after[2].f())),
                    ("list", same(&list, &c.after[5].i())),
                    ("end", same_v(end, c.after[6].int())),
                    ("flag", same_v(flag, c.after[7].int())),
                ],
            }
        });
    }

    #[test]
    fn intro_end_judge_main() {
        run("IntroEndJudge_main", |c| {
            let b = &c.before;
            match intro::intro_end_judge_main(&b[0].f(), &b[1].f(), &b[2].f(), b[3].int(), &b[4].f(), &b[5].i()) {
                None => vec![("ret", same_v(0, c.ret & 1))],
                Some((list, end, found)) => vec![
                    ("list", same(&list, &c.after[6].i())),
                    ("end", same_v(end, c.after[7].int())),
                    ("found", same_v(found, c.after[8].int())),
                ],
            }
        });
    }
}

#[test]
#[ignore]
fn dump_findpeaks() {
    let Ok(f) = std::env::var("MB_DUMP_FILE") else { return };
    let seq: u32 = std::env::var("MB_DUMP_SEQ").unwrap().parse().unwrap();
    for c in load(Path::new(&f), "findpeaks_ex").iter().filter(|c| c.seq == seq) {
        let x = c.before[0].f();
        println!("x {:?}", x);
        println!("rb vals {:?}\nrb locs {:?}\nrb widths {:?}\nrb proms {:?}", c.after[1].f(), c.after[2].i(), c.after[3].f(), c.after[4].f());
        let p = super::d_feat::findpeaks_ex(&x).unwrap();
        println!("mb vals {:?}\nmb locs {:?}\nmb widths {:?}\nmb proms {:?}", p.vals, p.locs, p.widths, p.proms);
    }
}

/// Prints how often each traced function was called (development aid).
#[test]
#[ignore]
fn call_stats() {
    let Ok(dir) = std::env::var("MB_PHRASE_TRACE") else { return };
    let mut counts: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.retain(|p| p.extension().is_some_and(|e| e == "tr"));
    for f in &files {
        let b = std::fs::read(f).unwrap();
        let mut names = HashMap::new();
        let mut i = 0;
        loop {
            let j = i + b[i..].iter().position(|&c| c == b'\n').unwrap();
            let line = std::str::from_utf8(&b[i..j]).unwrap();
            i = j + 1;
            if line == "DATA" {
                break;
            }
            let t: Vec<&str> = line.split(' ').collect();
            names.insert(t[1].parse::<u32>().unwrap(), t[2].to_string());
        }
        let mut r = Rd { b: &b, i };
        let mut seen = std::collections::HashSet::new();
        while r.i < b.len() {
            let t = r.u8();
            if t == b'M' {
                let n = r.u32() as usize;
                r.take(n);
                continue;
            }
            let (id, _seq, na) = (r.u32(), r.u32(), r.u8());
            for _ in 0..na {
                r.arg();
            }
            if t == b'E' {
                r.i64();
                r.f64();
                let e = counts.entry(names[&id].clone()).or_default();
                e.0 += 1;
                if seen.insert(id) {
                    e.1 += 1;
                }
            }
        }
    }
    for (k, (n, tracks)) in counts {
        println!("{k}: {n} calls in {tracks} tracks");
    }
}

/// Every stage-D call of a trace file, per function name, in call order.
pub fn load_all(path: &Path) -> HashMap<String, std::collections::VecDeque<Call>> {
    let b = std::fs::read(path).unwrap();
    let mut names = HashMap::new();
    let mut i = 0;
    loop {
        let j = i + b[i..].iter().position(|&c| c == b'\n').unwrap();
        let line = std::str::from_utf8(&b[i..j]).unwrap();
        i = j + 1;
        if line == "DATA" {
            break;
        }
        let t: Vec<&str> = line.split(' ').collect();
        names.insert(t[1].parse::<u32>().unwrap(), t[2].to_string());
    }
    let mut r = Rd { b: &b, i };
    let mut open: HashMap<u32, (u32, Vec<Arg>)> = HashMap::new();
    let mut done: Vec<(u32, String, Call)> = Vec::new();
    while r.i < b.len() {
        let t = r.u8();
        if t == b'M' {
            let n = r.u32() as usize;
            r.take(n);
            continue;
        }
        let (id, seq, na) = (r.u32(), r.u32(), r.u8());
        let args: Vec<Arg> = (0..na).map(|_| r.arg()).collect();
        if t == b'B' {
            open.insert(seq, (id, args));
        } else {
            let (ret, fret) = (r.i64(), r.f64());
            if let Some((id, before)) = open.remove(&seq) {
                let c = Call { file: path.to_path_buf(), seq, before, after: args, ret, fret };
                done.push((seq, names[&id].clone(), c));
            }
        }
    }
    done.sort_by_key(|d| d.0);
    // Stage D starts with `IntroEndFind_b`; earlier calls (stage B's) are dropped.
    let start = done.iter().find(|d| d.1 == "IntroEndFind_b").map_or(0, |d| d.0);
    let mut out: HashMap<String, std::collections::VecDeque<Call>> = HashMap::new();
    for (_, name, c) in done.into_iter().filter(|d| d.0 >= start) {
        out.entry(name).or_default().push_back(c);
    }
    out
}

/// Stage D end to end: per `<n>.rb` (harness output: `PATH`, `INIT`, `RESULT`) in
/// `MB_PHRASE_D`, runs stages A–D on the track and compares every probed callee input with the
/// trace `<n>.tr` and the result with the harness's. Prints one line per track.
#[test]
fn stage_d_end_to_end() {
    use super::stage_d::{self, probe::Val};
    let Ok(dir) = std::env::var("MB_PHRASE_D") else { return };
    let only: Option<String> = std::env::var("MB_PHRASE_ONLY").ok();
    let mut rbs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "rb"))
        .filter(|p| only.as_ref().is_none_or(|o| p.file_stem().unwrap().to_str().unwrap() == o))
        .collect();
    rbs.sort();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let lines = std::sync::Mutex::new(Vec::<(String, bool)>::new());
    let threads: usize = std::env::var("MB_PHRASE_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(rb) = rbs.get(k) else { break };
                let line = run_one(rb);
                lines.lock().unwrap().push(line);
            });
        }
    });
    let mut lines = lines.into_inner().unwrap();
    lines.sort();
    let ok = lines.iter().filter(|l| l.1).count();
    for (l, _) in &lines {
        eprintln!("{l}");
    }
    eprintln!("stage D: {ok}/{} results identical", lines.len());

    fn run_one(rb: &Path) -> (String, bool) {
        let name = rb.file_stem().unwrap().to_str().unwrap().to_string();
        let text = std::fs::read_to_string(rb).unwrap();
        let path = text.lines().find_map(|l| l.strip_prefix("PATH ")).unwrap().to_string();
        let Some(init) = text.lines().find(|l| l.starts_with("INIT ")) else {
            return (format!("{name} no INIT"), false);
        };
        let t: Vec<&str> = init.split(' ').collect();
        let num = |k: &str| t.iter().position(|&x| x == k).map(|i| t[i + 1].parse::<i64>().unwrap()).unwrap();
        let (bpm100, fb, fbar, key) = (num("bpm") as i32, num("fb") as i32, num("fbar") as i32, num("key") as i32);
        let want: Vec<String> = text
            .lines()
            .skip_while(|l| !l.starts_with("RESULT"))
            .map(str::to_string)
            .collect();
        let mut dec = mb_audio::open(&path).unwrap();
        let info = *dec.info();
        let total = info.total_samples - 1;
        let mut p = super::PhraseAnalyzer::new(info.sample_rate, total);
        let mut frames = vec![[0.0f32; 2]; 1 << 16];
        let mut pos = 0u64;
        while pos < total {
            let n = ((total - pos) as usize).min(frames.len());
            let got = dec.read_frames(&mut frames[..n]);
            if got == 0 {
                break;
            }
            let pcm: Vec<[i16; 2]> =
                frames[..got].iter().map(|f| [mb_audio::analysis_i16(f[0]), mb_audio::analysis_i16(f[1])]).collect();
            p.put(&pcm);
            pos += got as u64;
        }
        let Some(b) = p.stage_b(bpm100, fb, fbar, true) else {
            return (format!("{name} stage B failed; want {}", want.first().cloned().unwrap_or_default()), want.is_empty());
        };
        let Some(c) = super::stage_c(Path::new(&path), bpm100, b.down) else {
            return (format!("{name} stage C failed"), false);
        };
        let din = stage_d::DIn {
            bpm: bpm100 as f64 * 0.01,
            rate: info.sample_rate as f64,
            fs8: info.sample_rate as f64 * 0.125,
            key,
            total: total as i32,
            fb0: fb,
            fb: b.down.fb_ms,
            bar: b.down.bar,
            kick6: b.kick6,
            snares: &b.snares,
            bar_pos: b.bar_pos,
        };
        let tr = rb.with_extension("tr");
        let calls = std::rc::Rc::new(std::cell::RefCell::new(if tr.exists() { load_all(&tr) } else { HashMap::new() }));
        let first: std::rc::Rc<std::cell::RefCell<Option<String>>> = Default::default();
        {
            let calls = calls.clone();
            let first = first.clone();
            stage_d::probe::HOOK.with(|h| {
                *h.borrow_mut() = Some(Box::new(move |fname: &str, args: Vec<Val>| {
                    if first.borrow().is_some() {
                        return;
                    }
                    let Some(c) = calls.borrow_mut().get_mut(fname).and_then(|q| q.pop_front()) else {
                        *first.borrow_mut() = Some(format!("{fname}: no traced call left"));
                        return;
                    };
                    for (k, v) in args.iter().enumerate() {
                        let Some(a) = c.before.get(k) else { break };
                        let e = match (v, a) {
                            (Val::I(x), Arg::I(y)) => same(x, y.as_deref().unwrap_or(&[])),
                            (Val::F(x), Arg::F(y)) => same_f(x, y.as_deref().unwrap_or(&[])),
                            (Val::U(x), Arg::U(y)) => same(x, y.as_deref().unwrap_or(&[])),
                            (Val::Int(x), Arg::Int(y)) => same_v(*x as i32, *y as i32),
                            (Val::Real(x), Arg::Real(y)) => same_r(*x, *y),
                            (Val::Skip, _) | (_, Arg::G(..) | Arg::Hash(..) | Arg::Ptr(_)) => None,
                            (x, y) => Some(format!("kind {x:?} vs {y:?}")),
                        };
                        if let Some(e) = e {
                            *first.borrow_mut() = Some(format!("{fname} #{} arg {k}: {e}", c.seq));
                            return;
                        }
                    }
                }));
            });
        }
        let res = stage_d::sub_main_d(&c, &din);
        stage_d::probe::HOOK.with(|h| *h.borrow_mut() = None);
        let got: Vec<String> = match &res {
            None => Vec::new(),
            Some(r) => {
                let mut v = vec![format!(
                    "RESULT count {} fb {} mood {} bar {} bank {} f14 {} end {}",
                    r.count, r.fb, r.mood, r.bar, r.bank, r.unk_14, r.end_beat
                )];
                for i in 0..r.count.max(0) as usize {
                    let mut s = format!("  [{}", i + 1);
                    for col in &r.cols {
                        s += &format!(", {}", col.get(i).copied().unwrap_or(-9999));
                    }
                    s += "]";
                    v.push(s);
                }
                v
            }
        };
        let ok = got == want;
        let probe = first.borrow().clone().unwrap_or_else(|| "inputs match".into());
        let mut line = format!("{name} {} | {probe}", if ok { "OK" } else { "DIFF" });
        if !ok {
            line += &format!("\n    want {:?}\n    got  {:?}", want.iter().take(4).collect::<Vec<_>>(), got.iter().take(4).collect::<Vec<_>>());
        }
        (line, ok)
    }
}
