//! Differential checks between rekordbox data sources (docs/plan.md § Verification).
//!
//! `rbdiff device <device_root>` — export.pdb and exportLibrary.db on one device must describe
//! the same library: same track IDs, titles, artists, BPM, keys, playlists and playlist order.
//! `rbdiff anlz-export <library_dir> <device_root>` — USB ANLZ files vs the local analysis.

mod anlz_export;

use anyhow::{Context, Result, bail};
use mb_core::{LibrarySnapshot, PlaylistKind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("device") => {
            let root = PathBuf::from(args.next().context("usage: rbdiff device <device_root>")?);
            let problems = device(&root)?;
            if problems > 0 {
                bail!("{problems} mismatches");
            }
            Ok(())
        }
        Some("anlz-export") => {
            let lib = PathBuf::from(args.next().context("usage: rbdiff anlz-export <library_dir> <device_root>")?);
            let dev = PathBuf::from(args.next().context("usage: rbdiff anlz-export <library_dir> <device_root>")?);
            anlz_export::run(&lib, &dev)
        }
        _ => bail!("usage: rbdiff device <device_root> | anlz-export <library_dir> <device_root>"),
    }
}

fn device(root: &Path) -> Result<usize> {
    let rb = root.join("PIONEER/rekordbox");
    let pdb = mb_pdb::DeviceLibrary::open(&rb)?.snapshot();
    let devlib = mb_devlib::DevLibDb::open_immutable(&rb.join("exportLibrary.db"))?.load_snapshot(root)?;
    println!("export.pdb: {} tracks, {} playlist nodes", pdb.tracks.len(), pdb.playlists.len());
    println!("exportLibrary.db: {} tracks, {} playlist nodes", devlib.tracks.len(), devlib.playlists.len());
    let mut problems = 0;
    problems += compare_tracks(&pdb, &devlib);
    problems += compare_playlists(&pdb, &devlib);
    println!("{problems} mismatches");
    Ok(problems)
}

fn compare_tracks(a: &LibrarySnapshot, b: &LibrarySnapshot) -> usize {
    let by_id = |s: &LibrarySnapshot| -> BTreeMap<String, mb_core::Track> {
        s.tracks.iter().map(|t| (t.id.0.clone(), t.clone())).collect()
    };
    let (a, b) = (by_id(a), by_id(b));
    let mut problems = 0;
    for (id, ta) in &a {
        let Some(tb) = b.get(id) else {
            println!("track {id} only in export.pdb: {}", ta.title);
            problems += 1;
            continue;
        };
        let fields = [
            ("title", ta.title.clone(), tb.title.clone()),
            ("artist", ta.artist.clone(), tb.artist.clone()),
            ("album", ta.album.clone(), tb.album.clone()),
            ("genre", ta.genre.clone(), tb.genre.clone()),
            ("key", ta.key.clone(), tb.key.clone()),
            ("bpm", format!("{:?}", ta.bpm_x100), format!("{:?}", tb.bpm_x100)),
            ("length", format!("{:?}", ta.length_seconds), format!("{:?}", tb.length_seconds)),
            ("path", ta.path.display().to_string(), tb.path.display().to_string()),
            ("analysis", format!("{:?}", ta.analysis_path), format!("{:?}", tb.analysis_path)),
        ];
        for (name, x, y) in fields {
            if x != y {
                println!("track {id} {name}: pdb={x:?} devlib={y:?}");
                problems += 1;
            }
        }
    }
    for id in b.keys().filter(|id| !a.contains_key(*id)) {
        println!("track {id} only in exportLibrary.db");
        problems += 1;
    }
    problems
}

/// Playlists compared by their path of names, since the two databases number nodes independently.
fn compare_playlists(a: &LibrarySnapshot, b: &LibrarySnapshot) -> usize {
    fn lists(s: &LibrarySnapshot) -> BTreeMap<String, Vec<String>> {
        let by_id: BTreeMap<&mb_core::DbId, &mb_core::PlaylistNode> =
            s.playlists.iter().map(|n| (&n.id, n)).collect();
        let path = |node: &mb_core::PlaylistNode| {
            let mut parts = vec![node.name.clone()];
            let mut parent = node.parent.as_ref();
            while let Some(p) = parent.and_then(|id| by_id.get(id)) {
                parts.push(p.name.clone());
                parent = p.parent.as_ref();
            }
            parts.reverse();
            parts.join(" / ")
        };
        s.playlists
            .iter()
            .filter(|n| n.kind != PlaylistKind::Folder)
            .map(|n| {
                let entries = s.playlist_entries.get(&n.id).cloned().unwrap_or_default();
                (path(n), entries.into_iter().map(|id| id.0).collect())
            })
            .collect()
    }
    let (a, b) = (lists(a), lists(b));
    let mut problems = 0;
    for (name, ea) in &a {
        match b.get(name) {
            None => {
                println!("playlist {name:?} only in export.pdb ({} tracks)", ea.len());
                problems += 1;
            }
            Some(eb) if eb != ea => {
                println!("playlist {name:?}: {} entries in pdb vs {} in devlib", ea.len(), eb.len());
                problems += 1;
            }
            _ => {}
        }
    }
    for name in b.keys().filter(|n| !a.contains_key(*n)) {
        println!("playlist {name:?} only in exportLibrary.db");
        problems += 1;
    }
    problems
}
