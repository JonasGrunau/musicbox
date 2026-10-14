//! Acceptance runs on the sandbox library: performs the app's own commands (the ones the UI
//! sends), so that rekordbox can then be started on the result and the before/after state
//! compared. Refuses while rekordbox runs; backs the library up first (except `survey`).
//!
//! `cargo run -p mb-library --example accept_check -- <options.json> <step> [args]`
//! - `survey [device_root] [rekordbox.xml]`: unanalysed tracks, device lists, XML lists.
//! - `analyse`: analyses every track without analysis (like auto-analysis), preferences from
//!   `rekordbox3.settings`.
//! - `xml-export <out.xml>`: musicbox's "Export Collection in xml format".
//! - `smart`: musicbox's evaluation of every intelligent playlist (`<name>\t<track IDs>`).
//! - `import <audio file>...`: imports files into the collection (the app's "Import").
//! - `xml <rekordbox.xml> <list name>`: imports one XML playlist (tracks + list).
//! - `device <device_root> <list name> [<track id to remove first>]`: imports a device
//!   playlist; optionally removes one collection track first so that it is imported anew
//!   (audio copied, analysis copied). Writes the master IDs back to the device like the app.
//! - `undo-device <track id> <playlist id>`: the app's Undo of such an import (the inverse
//!   command), with the track's copied audio, analysis and artwork folders as created files.

use mb_library::{AnalysisSettings, Command, DeviceImport, Effect, Library, analyse_track_with, stored_grid};
use mb_masterdb::{LibraryPaths, MasterDb, backup_library, default_backup_root, derive_passphrase};
use mb_pdb::{ExportDb, Pdb};
use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = PathBuf::from(&args[0]);
    let step = args.get(1).map(String::as_str).unwrap_or("survey");
    let paths = LibraryPaths::from_options_json(&options)?;
    let pass = derive_passphrase(&paths.dp)?;
    let snap = MasterDb::open_read_only(&paths.db_path, &pass)?.load_snapshot(&paths)?;
    let unanalysed: Vec<_> = snap.tracks.iter().filter(|t| t.analysed == 0 && t.analysis_path.is_none()).collect();

    if step == "xml-export" {
        let db = MasterDb::open_read_only(&paths.db_path, &pass)?;
        let opts = mb_library::xml::XmlExportOptions::from_settings(&paths.settings_root);
        let doc = mb_library::xml::export_document(db.connection(), &snap, &opts)?;
        mb_library::xml::write_document(&doc, Path::new(&args[2]))?;
        return Ok(());
    }
    if step == "smart" {
        let results = mb_core::smartlist::evaluate_all(&snap, mb_core::smartlist::Clock::utc_now());
        for p in &snap.playlists {
            if let Some(ids) = results.get(&p.id) {
                let mut ids: Vec<&str> = ids.iter().map(|i| i.0.as_str()).collect();
                ids.sort_unstable();
                println!("{}\t{}", p.name, ids.join(","));
            }
        }
        return Ok(());
    }
    if step == "survey" {
        println!("{} tracks, {} without analysis:", snap.tracks.len(), unanalysed.len());
        for t in &unanalysed {
            println!("  {} {}", t.id.0, t.path.display());
        }
        if let Some(root) = args.get(2) {
            let db = read_export(Path::new(root))?;
            println!("device {root}: {} tracks", db.tracks.len());
            for p in &db.playlists {
                let n = db.playlist_entries.iter().filter(|e| e.playlist_id == p.0.id).count();
                println!("  list {:>4} parent {:>4} {:<30} {n} tracks", p.0.id, p.0.parent_id, p.0.name);
                if n <= 10 {
                    let mut es: Vec<_> = db.playlist_entries.iter().filter(|e| e.playlist_id == p.0.id).collect();
                    es.sort_by_key(|e| e.entry_index);
                    for e in es {
                        if let Some(t) = db.tracks.iter().find(|t| t.id == e.track_id) {
                            println!("      track {:>4} master {}/{} {}", t.id, t.master_db_id, t.master_content_id, t.folder_path);
                        }
                    }
                }
            }
        }
        if let Some(xml) = args.get(3) {
            let doc = mb_library::xml::read_document(Path::new(xml))?;
            println!("xml {xml}: {} tracks", doc.collection.as_ref().map_or(0, Vec::len));
            fn walk(n: &mb_xml::PlaylistNode, depth: usize) {
                match n {
                    mb_xml::PlaylistNode::Folder { name, children, .. } => {
                        println!("  {}{name}/", "  ".repeat(depth));
                        for c in children {
                            walk(c, depth + 1);
                        }
                    }
                    mb_xml::PlaylistNode::Playlist { name, entries, .. } => {
                        println!("  {}{name} ({} entries)", "  ".repeat(depth), entries.len());
                    }
                }
            }
            for c in doc.playlists.iter().flatten() {
                walk(c, 0);
            }
        }
        return Ok(());
    }

    let mut lib = Library::open(&paths.db_path, &pass)?; // refuses while rekordbox runs
    lib.set_analysis_root(&paths.analysis_data_root);
    let backup = backup_library(&paths.db_path, &pass, &default_backup_root().expect("HOME"))?;
    println!("backup: {}", backup.display());
    match step {
        "analyse" => {
            let pref = |name| mb_usb::settings::preference(&paths.settings_root, name);
            let settings = AnalysisSettings {
                auto_gain: pref("AutoGain").is_none_or(|v| v.trim() != "0"),
                phrase: pref("trackAnalysisSettingsSongStruct").is_some_and(|v| v.trim() == "1"),
                key: pref("enableKeyAnalysis").is_none_or(|v| v.trim() != "0"),
                high_precision: AnalysisSettings::high_precision_from(
                    pref("HighPrecisionBeatAnalyze").as_deref(),
                    pref("trackHiPrecisionBeatAnalysisMode").as_deref(),
                ),
                dynamic: AnalysisSettings::dynamic_from(pref("trackAnalysisMode").as_deref()),
                vocal: pref("trackAnalysisSettingsVocalDetect").is_some_and(|v| v.trim() == "1"),
                embedding: pref("TrackAnalysisSettingsEmbedding").is_none_or(|v| v.trim() != "0"),
                cue: mb_library::CueAnalysisSettings::from_prefs(|n| mb_usb::settings::preference(&paths.settings_root, n)),
            };
            println!("settings: {settings:?}");
            for t in unanalysed {
                let name = t.path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                // The stored grid (none for unanalysed tracks) is the high-precision fallback.
                let stored = t.analysis_path.as_deref().and_then(|dat| stored_grid(dat, t.bpm_x100.map(i64::from)));
                let analysis = analyse_track_with(&t.path, name, &settings, stored.as_ref())?;
                println!("  {} {}: BPM {:?}, key {:?}, {} s", t.id.0, t.title, analysis.bpm_x100, analysis.key, analysis.length);
                lib.execute(Command::ApplyAnalysis { content: t.id.clone(), analysis: Box::new(analysis) })?;
            }
        }
        "import" => {
            let paths: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();
            let out = lib.execute(Command::ImportFiles { paths })?;
            println!("import: {:?}", out.effect);
        }
        "xml" => {
            let doc = mb_library::xml::read_document(Path::new(&args[2]))?;
            let name = &args[3];
            let node = doc.playlists.iter().flatten().find_map(|n| find_node(n, name)).ok_or_else(|| anyhow::anyhow!("no XML list {name:?}"))?;
            let import = mb_library::xml::import_for_node(&doc, node);
            let out = lib.execute(Command::ImportXml { import })?;
            let Effect::ImportedXml(report) = out.effect else { anyhow::bail!("unexpected {:?}", out.effect) };
            println!("xml import: {report:?}");
        }
        "device" => {
            let root = PathBuf::from(&args[2]);
            let name = &args[3];
            if let Some(id) = args.get(4) {
                let out = lib.execute(Command::RemoveTracks { ids: vec![mb_core::DbId(id.clone())] })?;
                println!("removed first: {:?}", out.effect);
            }
            let db = read_export(&root)?;
            let all = mb_library::device_import::device_tracks(&root, &db);
            let list = db.playlists.iter().find(|p| &p.0.name == name).ok_or_else(|| anyhow::anyhow!("no device list {name:?}"))?;
            let mut entries: Vec<_> = db.playlist_entries.iter().filter(|e| e.playlist_id == list.0.id).collect();
            entries.sort_by_key(|e| e.entry_index);
            let tracks = entries.iter().filter_map(|e| all.get(&e.track_id).cloned()).collect();
            let copy_to = mb_library::device_import::default_copy_folder().expect("HOME");
            let out = lib.execute(Command::ImportFromDevice { import: DeviceImport { tracks, copy_to, playlist: Some(name.clone()) } })?;
            let Effect::ImportedFromDevice(report) = &out.effect else { anyhow::bail!("unexpected {:?}", out.effect) };
            for r in &report.results {
                println!("  device {} → {} {:?} stamp {:?}", r.device_id, r.local_id.0, r.kind, r.stamp);
            }
            println!("  playlist {:?}, skipped {:?}", report.playlist, report.skipped);
            let stamps: Vec<(u32, u32, u32)> =
                report.results.iter().filter_map(|r| r.stamp.map(|(dbid, cid)| (r.device_id, dbid, cid))).collect();
            if !stamps.is_empty() {
                let dev = mb_usb::DeviceState::load(&root, None)?;
                mb_usb::stamp::stamp_master_ids(&dev, &stamps)?;
                println!("  {} device row(s) stamped", stamps.len());
            }
        }
        "undo-device" => {
            let id = mb_core::DbId(args[2].clone());
            let (audio, anlz, image): (String, String, String) = lib.connection().query_row(
                "SELECT FolderPath, COALESCE(AnalysisDataPath, ''), COALESCE(ImagePath, '') FROM djmdContent WHERE ID = ?1",
                [&id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let mut files = vec![PathBuf::from(audio)];
            for rel in [anlz, image] {
                if let Some(dir) = Path::new(rel.trim_start_matches('/')).parent().filter(|d| !d.as_os_str().is_empty()) {
                    files.push(paths.analysis_data_root.join(dir));
                }
            }
            println!("undo: {files:?}");
            let out = lib.execute(Command::UndoDeviceImport { ids: vec![id], playlist: Some(mb_core::DbId(args[3].clone())), files })?;
            println!("  {:?}", out.effect);
        }
        other => anyhow::bail!("unknown step {other}"),
    }
    Ok(())
}

fn read_export(root: &Path) -> anyhow::Result<ExportDb> {
    let bytes = std::fs::read(root.join("PIONEER/rekordbox/export.pdb"))?;
    Ok(ExportDb::read(&Pdb::parse(bytes)?)?)
}

fn find_node<'a>(n: &'a mb_xml::PlaylistNode, name: &str) -> Option<&'a mb_xml::PlaylistNode> {
    match n {
        mb_xml::PlaylistNode::Playlist { name: n_name, .. } if n_name == name => Some(n),
        mb_xml::PlaylistNode::Folder { children, .. } => children.iter().find_map(|c| find_node(c, name)),
        _ => None,
    }
}
