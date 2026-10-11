//! `rbdiff anlz-export <library_dir> <device_root>`: compares each exported track's USB ANLZ
//! files with the local analysis they were built from (docs/re/export.md, docs/re/anlz.md).
//!
//! `<library_dir>` holds `options.json` and `rekordbox/` (master.db + share/) as captured in
//! fixtures. Tracks are paired through exportLibrary.db's `masterContentId`. Scrambled PSSI/PVDI
//! are compared in plain form, so equality there also proves the unscrambling.

use anyhow::{Context, Result};
use mb_anlz::AnlzFile;
use mb_anlz::tags::TypedSection;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Tally {
    same: usize,
    differ: usize,
    only_local: usize,
    only_usb: usize,
    example: Option<String>,
}

fn tag_name(s: &TypedSection) -> String {
    s.encode().tag.to_string()
}

/// Strips the stored-form markers so plain content can be compared.
fn normalized(s: &TypedSection) -> TypedSection {
    let mut s = s.clone();
    match &mut s {
        TypedSection::SongStructure(p) => p.scrambled = false,
        TypedSection::VocalDetect(v) => v.version &= 0x7fff,
        _ => {}
    }
    s
}

fn load(path: &Path) -> Option<Vec<TypedSection>> {
    let data = std::fs::read(path).ok()?;
    Some(AnlzFile::parse(&data).ok()?.typed_sections())
}

pub fn run(library_dir: &Path, device_root: &Path) -> Result<()> {
    let mut paths = LibraryPaths::from_options_json(&library_dir.join("options.json"))?;
    paths.analysis_data_root = library_dir.join("rekordbox/share");
    let db = MasterDb::open_read_only(&library_dir.join("rekordbox/master.db"), &derive_passphrase(&paths.dp)?)?;
    let local: HashMap<String, PathBuf> = db
        .load_snapshot(&paths)?
        .tracks
        .into_iter()
        .filter_map(|t| Some((t.id.0, t.analysis_path?)))
        .collect();
    let devlib = mb_devlib::DevLibDb::open_immutable(&device_root.join("PIONEER/rekordbox/exportLibrary.db"))?;
    let tracks = devlib.tracks()?;

    let mut tally: BTreeMap<(String, String), Tally> = BTreeMap::new();
    let mut paired = 0;
    // Whole-file check of mb_anlz::usb::to_usb against rekordbox's bytes.
    let (mut bytes_same, mut bytes_differ) = (0, 0);
    for t in &tracks {
        let master_id = t.master_content_id.context("device track without masterContentId")?.to_string();
        let Some(local_dat) = local.get(&master_id) else { continue };
        paired += 1;
        let usb_dat = device_root.join(t.analysis_path.trim_start_matches('/'));
        for ext in ["DAT", "EXT", "2EX"] {
            if let (Ok(local_bytes), Ok(usb_bytes)) =
                (std::fs::read(local_dat.with_extension(ext)), std::fs::read(usb_dat.with_extension(ext)))
                && let Ok(local_file) = AnlzFile::parse(&local_bytes)
            {
                let device_path = format!("/{}", t.path.trim_start_matches('/'));
                if mb_anlz::usb::to_usb(&local_file, &device_path).to_bytes() == usb_bytes {
                    bytes_same += 1;
                } else {
                    bytes_differ += 1;
                }
            }
            let (Some(a), Some(b)) = (load(&local_dat.with_extension(ext)), load(&usb_dat.with_extension(ext))) else {
                tally.entry((ext.into(), "(file)".into())).or_default().differ += 1;
                continue;
            };
            // Pair sections by tag and occurrence (PCOB/PCO2 appear twice: hot, memory).
            let group = |v: &[TypedSection]| {
                let mut m: BTreeMap<String, Vec<TypedSection>> = BTreeMap::new();
                for s in v {
                    m.entry(tag_name(s)).or_default().push(normalized(s));
                }
                m
            };
            let (ga, gb) = (group(&a), group(&b));
            for tag in ga.keys().chain(gb.keys()).collect::<std::collections::BTreeSet<_>>() {
                let e = tally.entry((ext.into(), tag.clone())).or_default();
                match (ga.get(tag), gb.get(tag)) {
                    (Some(x), Some(y)) if x == y => e.same += 1,
                    (Some(x), Some(y)) => {
                        e.differ += 1;
                        if e.example.is_none() {
                            e.example = Some(format!("track {} ({}):\n  local {:?}\n  usb   {:?}", t.id, t.title, short(x), short(y)));
                        }
                    }
                    (Some(_), None) => e.only_local += 1,
                    (None, Some(_)) => e.only_usb += 1,
                    (None, None) => {}
                }
            }
        }
    }
    println!("{paired} of {} device tracks paired with local analysis", tracks.len());
    println!("to_usb() byte-exact: {bytes_same} files, differing: {bytes_differ}");
    println!("{:4} {:5} {:>6} {:>6} {:>10} {:>9}", "file", "tag", "same", "differ", "only-local", "only-usb");
    for ((ext, tag), t) in &tally {
        println!("{ext:4} {tag:5} {:>6} {:>6} {:>10} {:>9}", t.same, t.differ, t.only_local, t.only_usb);
    }
    for ((ext, tag), t) in &tally {
        if let Some(ex) = &t.example {
            println!("\n--- {ext} {tag} first difference, {ex}");
        }
    }
    Ok(())
}

fn short(v: &[TypedSection]) -> String {
    let s = format!("{v:?}");
    if s.len() > 400 { format!("{}…", &s[..s.char_indices().nth(400).map_or(s.len(), |(i, _)| i)]) } else { s }
}
