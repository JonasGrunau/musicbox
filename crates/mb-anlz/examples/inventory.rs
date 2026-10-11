//! `cargo run -p mb-anlz --example inventory -- <dir>...`
//! Counts (file kind, tag, header length, PMAI header bytes) across all ANLZ files below the dirs.

use mb_anlz::AnlzFile;
use std::collections::BTreeMap;
use std::path::Path;

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() { walk(&p, f) } else { f(&p) }
    }
}

fn main() {
    let mut tags: BTreeMap<(String, String, usize), usize> = BTreeMap::new();
    let mut pmai: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut orders: BTreeMap<(String, String), usize> = BTreeMap::new();
    for dir in std::env::args().skip(1) {
        walk(Path::new(&dir), &mut |p| {
            let Some(ext) = p.extension().and_then(|e| e.to_str()) else { return };
            if !matches!(ext, "DAT" | "EXT" | "2EX") { return }
            let Ok(file) = AnlzFile::parse(&std::fs::read(p).unwrap()) else { return };
            *pmai.entry((ext.into(), format!("{:02x?}", file.header))).or_default() += 1;
            let order: Vec<String> = file.sections.iter().map(|s| s.tag.to_string()).collect();
            *orders.entry((ext.into(), order.join(" "))).or_default() += 1;
            for s in &file.sections {
                *tags.entry((ext.into(), s.tag.to_string(), 12 + s.header.len())).or_default() += 1;
            }
        });
    }
    println!("== PMAI header bytes"); for ((e, h), n) in &pmai { println!("{e:4} {n:5}  {h}") }
    println!("== tag order"); for ((e, o), n) in &orders { println!("{e:4} {n:5}  {o}") }
    println!("== tag header lengths"); for ((e, t, h), n) in &tags { println!("{e:4} {t} hdr={h:3} count={n}") }
}
