//! Lossless round trips over every PMAI file in the fixtures (skipped when fixtures are absent):
//! the raw container, the typed tag views, and fixture-level cross-checks of decoded values.

use mb_anlz::tags::{CueKind, ExtBeatGrid, TypedSection};
use mb_anlz::{AnlzFile, FileHeader, Tag, usb};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "DAT" | "EXT" | "2EX"))
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ANLZ"))
        {
            out.push(path);
        }
    }
}

fn fixture_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    files
}

#[test]
fn every_fixture_file_round_trips_byte_exact() {
    let files = fixture_files();
    if files.is_empty() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let mut failures = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match AnlzFile::parse(&data) {
            Ok(file) if file.to_bytes() == data => {}
            Ok(_) => failures.push(format!("{}: bytes differ", path.display())),
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} failed:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
    eprintln!("{} files round-tripped", files.len());
}

/// Every section of every fixture decodes into its typed view (never the raw fallback), and
/// typed decode → encode reproduces each section and the whole file byte for byte.
#[test]
fn every_fixture_file_round_trips_through_typed_views() {
    let files = fixture_files();
    if files.is_empty() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let mut failures = Vec::new();
    let mut per_tag: BTreeMap<String, usize> = BTreeMap::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let file = AnlzFile::parse(&data).unwrap();
        let Some(header) = file.file_header() else {
            failures.push(format!("{}: PMAI header not 16 bytes", path.display()));
            continue;
        };
        let mut typed = Vec::new();
        for s in &file.sections {
            match TypedSection::try_decode(s) {
                Ok(TypedSection::Unknown(_)) => {
                    failures.push(format!("{}: {} has no typed view", path.display(), s.tag))
                }
                Ok(t) if &t.encode() == s => {
                    *per_tag.entry(s.tag.to_string()).or_default() += 1;
                    typed.push(t);
                }
                Ok(_) => failures.push(format!(
                    "{}: {} re-encodes differently",
                    path.display(),
                    s.tag
                )),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        if AnlzFile::from_typed(header, &typed).to_bytes() != data {
            failures.push(format!("{}: typed file bytes differ", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!(
        "{} files typed-round-tripped; sections per tag: {per_tag:?}",
        files.len()
    );
}

/// Decoded values agree with each other across tags and files.
#[test]
fn fixture_values_are_consistent() {
    let files = fixture_files();
    if files.is_empty() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let (mut pqt2_checked, mut headers_default) = (0, 0);
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "DAT"))
    {
        let dat = AnlzFile::parse(&std::fs::read(path).unwrap()).unwrap();
        headers_default += usize::from(dat.file_header() == Some(FileHeader::default()));
        let grid = dat.beat_grid().expect("every DAT has PQTZ");
        // Constant-tempo grids (normal analysis) space beats at 60000/BPM. High-precision grids
        // (oracle-b) carry a tempo per beat that is not the gap to the next beat (anlz.md § PQTZ).
        let constant_tempo = grid.beats.windows(2).all(|w| w[0].tempo == w[1].tempo);
        for pair in grid.beats.windows(2) {
            // 8 stick grids have one beat numbered 0 where 4 is due, in the last bars (open).
            let prev = if pair[0].beat_in_bar == 0 {
                4
            } else {
                pair[0].beat_in_bar
            };
            let next = if pair[1].beat_in_bar == 0 {
                4
            } else {
                pair[1].beat_in_bar
            };
            assert_eq!(next, prev % 4 + 1, "{}", path.display());
            let expected = 60_000.0 * 100.0 / f64::from(pair[0].tempo);
            let got = f64::from(pair[1].time_ms - pair[0].time_ms);
            assert!(
                !constant_tempo || (got - expected).abs() <= 2.0,
                "{}: beat spacing {got} vs {expected}",
                path.display()
            );
        }
        let ext = AnlzFile::parse(&std::fs::read(path.with_extension("EXT")).unwrap()).unwrap();
        // A grid edit empties PQT2 (no entries, checksum 0), as rekordbox's own edit in
        // fixtures/oracle-grid-edit shows (docs/re/ops/edit-grid.md).
        if let Some(pqt2) = ext.ext_beat_grid().filter(|g| !g.entries.is_empty() || g.checksum != 0) {
            assert!(
                pqt2.matches(&grid.beats),
                "{}: PQT2 checksum",
                path.display()
            );
            assert_eq!(pqt2.checksum, ExtBeatGrid::checksum_of(&grid.beats));
            assert_eq!(
                (pqt2.first, pqt2.last),
                (grid.beats[0], *grid.beats.last().unwrap())
            );
            pqt2_checked += 1;
        }
        // DAT hot cues (A–C) reappear in the EXT PCO2 hot-cue list with the same time and type.
        let dat_hot: Vec<_> = dat
            .cue_lists()
            .into_iter()
            .filter(|l| l.kind == CueKind::Hot)
            .flat_map(|l| l.entries)
            .collect();
        let ext_hot: Vec<_> = ext
            .cue_lists_ext()
            .into_iter()
            .filter(|l| l.kind == CueKind::Hot)
            .flat_map(|l| l.entries)
            .collect();
        for c in &dat_hot {
            assert!(
                ext_hot.iter().any(|e| e.hot_cue == c.hot_cue
                    && e.time == c.time
                    && e.cue_type == c.cue_type),
                "{}: hot cue {} missing from PCO2",
                path.display(),
                c.hot_cue
            );
        }
    }
    assert!(pqt2_checked > 0);
    eprintln!("PQT2 checksums verified: {pqt2_checked}; default PMAI headers: {headers_default}");
}

/// The `USBANLZ/Pxxx/xxxxxxxx` folder of every stick set equals the hash of its PPTH path.
#[test]
fn usb_folder_hash_matches_stick() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stick/PIONEER/USBANLZ");
    let mut files = Vec::new();
    collect(&root, &mut files);
    let dats: Vec<_> = files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "DAT"))
        .collect();
    if dats.is_empty() {
        eprintln!("no stick fixtures, skipping");
        return;
    }
    let mut mismatches = Vec::new();
    let mut folders = BTreeSet::new();
    for path in &dats {
        let file = AnlzFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let ppth = file.path().expect("PPTH");
        let dir = path.parent().unwrap();
        let actual = format!(
            "{}/{}",
            dir.parent().unwrap().file_name().unwrap().to_str().unwrap(),
            dir.file_name().unwrap().to_str().unwrap()
        );
        folders.insert(actual.clone());
        if usb::usb_anlz_folder(&ppth) != actual {
            mismatches.push(format!(
                "{actual}: {ppth} -> {}",
                usb::usb_anlz_folder(&ppth)
            ));
        }
        // All three files of the set carry the same path.
        for ext in ["EXT", "2EX"] {
            let other = AnlzFile::parse(&std::fs::read(path.with_extension(ext)).unwrap()).unwrap();
            assert_eq!(other.path().as_deref(), Some(ppth.as_str()));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} sets mismatch:\n{}",
        mismatches.len(),
        dats.len(),
        mismatches.join("\n")
    );
    eprintln!(
        "folder hash matched {} of {} stick sets ({} folders)",
        dats.len(),
        dats.len(),
        folders.len()
    );
}

#[test]
fn local_fixtures_use_question_mark_paths() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sandbox-initial");
    let mut files = Vec::new();
    collect(&root, &mut files);
    for path in &files {
        let file = AnlzFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let ppth = file.path().unwrap();
        assert!(ppth.starts_with("?/"), "{}: {ppth}", path.display());
        assert!(file.sections_with(Tag::PPTH).count() == 1);
    }
}
