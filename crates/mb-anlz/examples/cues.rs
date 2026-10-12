//! `cargo run -p mb-anlz --example cues -- <dir>...`
//! Prints the PCOB/PCO2 cue lists of every `ANLZ*.DAT`/`.EXT` below the given folders, one line
//! per entry, for checking layouts and ordering against rekordbox's exports.

use mb_anlz::AnlzFile;
use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "DAT" || x == "EXT") {
            out.push(p);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    for d in std::env::args().skip(1) {
        walk(Path::new(&d), &mut files);
    }
    files.sort();
    for f in files {
        let Ok(data) = std::fs::read(&f) else { continue };
        let Ok(anlz) = AnlzFile::parse(&data) else { continue };
        let lists = anlz.cue_lists();
        let ext = anlz.cue_lists_ext();
        if lists.iter().all(|l| l.entries.is_empty()) && ext.iter().all(|l| l.entries.is_empty()) {
            continue;
        }
        println!("{}", f.display());
        for l in &lists {
            println!("  PCOB {:?} n={} unk14={:#x} unk16={:#x}", l.kind, l.entries.len(), l.unk_14, l.unk_16);
            for e in &l.entries {
                println!(
                    "    hot={} type={:?} t={} end={:#x} unit={} status={} unk10={} prev={:#x} next={:#x} unk28={:x?}",
                    e.hot_cue, e.cue_type, e.time, e.loop_end, e.time_unit, e.status, e.unk_10, e.prev, e.next, e.unk_28
                );
            }
        }
        for l in &ext {
            println!("  PCO2 {:?} n={}", l.kind, l.entries.len());
            for e in &l.entries {
                println!(
                    "    hot={} type={:?} t={} end={:#x} color={} unk1d={} in_us={} out_us={} loop={}/{} comment={:?} ext={}",
                    e.hot_cue,
                    e.cue_type,
                    e.time,
                    e.loop_end,
                    e.color_index,
                    e.unk_1d,
                    e.in_us,
                    e.out_us,
                    e.loop_numerator,
                    e.loop_denominator,
                    e.comment,
                    e.ext.iter().take(4).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("") + &format!("+{}", e.ext.len().saturating_sub(4)),
                );
            }
        }
    }
}
