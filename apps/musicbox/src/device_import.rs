//! "Import to Collection" and "Import Histories" for a device (`docs/re/ops/import-device.md`).
//! The commands run on the editor thread like every library write, and so does everything that
//! touches the stick: reading export.pdb to build the command, and the stick-side follow-ups
//! (new master IDs on the device rows, `ImportedHistory.xml`) afterwards, only on devices
//! musicbox may write to.

use crate::editor::{DeviceNote, Request};
use mb_library::{Command, DeviceHistory, DeviceImport, Effect, ImportedHistory, LocalTime};
use mb_pdb::{ExportDb, Pdb};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn rekordbox_dir(root: &Path) -> PathBuf {
    let hidden = root.join(".PIONEER/rekordbox/export.pdb").is_file() && !root.join("PIONEER/rekordbox/export.pdb").is_file();
    root.join(if hidden { ".PIONEER" } else { "PIONEER" }).join("rekordbox")
}

fn read_export(root: &Path) -> Result<ExportDb, String> {
    let path = rekordbox_dir(root).join("export.pdb");
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let pdb = Pdb::parse(bytes).map_err(|e| e.to_string())?;
    ExportDb::read(&pdb).map_err(|e| e.to_string())
}

/// The import command for a device playlist (name path from the top level, imported as a
/// playlist of the same name) or for single tracks (absolute paths on the device).
pub fn import_command(root: &Path, playlist: Option<&[String]>, tracks: &[PathBuf], copy_to: PathBuf) -> Result<Command, String> {
    let db = read_export(root)?;
    let all = mb_library::device_import::device_tracks(root, &db);
    let (ids, name): (Vec<u32>, Option<String>) = match playlist {
        Some(path) => {
            let mut parent = 0u32;
            let mut found = None;
            for name in path {
                found = db.playlists.iter().find(|p| p.0.parent_id == parent && &p.0.name == name).map(|p| p.0.id);
                parent = found.ok_or_else(|| format!("no list {name:?} on the device"))?;
            }
            let id = found.ok_or("empty list path")?;
            let mut entries: Vec<_> = db.playlist_entries.iter().filter(|e| e.playlist_id == id).collect();
            entries.sort_by_key(|e| e.entry_index);
            (entries.iter().map(|e| e.track_id).collect(), path.last().cloned())
        }
        None => {
            // In the order of `tracks` (first occurrence of a path).
            let mut order: HashMap<&Path, usize> = HashMap::with_capacity(tracks.len());
            for (i, p) in tracks.iter().enumerate() {
                order.entry(p.as_path()).or_insert(i);
            }
            let mut ids: Vec<(usize, u32)> =
                all.values().filter_map(|t| order.get(t.audio.as_path()).map(|&i| (i, t.device_id))).collect();
            ids.sort_by_key(|&(i, _)| i);
            (ids.into_iter().map(|(_, id)| id).collect(), None)
        }
    };
    if ids.is_empty() {
        return Err("nothing to import".into());
    }
    let tracks = ids.iter().filter_map(|id| all.get(id).cloned()).collect();
    Ok(Command::ImportFromDevice { import: DeviceImport { tracks, copy_to, playlist: name } })
}

/// The editor request for "Import to Collection" from the device at `root` (`writable`: not a
/// protected volume).
pub fn import_request(root: PathBuf, writable: bool, playlist: Option<Vec<String>>, tracks: Vec<PathBuf>, copy_to: PathBuf) -> Request {
    let device = root.clone();
    Request::Device {
        build: Box::new(move || import_command(&device, playlist.as_deref(), &tracks, copy_to).map_err(|e| format!("Import failed: {e}"))),
        after: follow_up(root, writable),
    }
}

/// The editor request for "Import Histories" from the device at `root`.
pub fn histories_request(root: PathBuf, writable: bool) -> Request {
    let device = root.clone();
    Request::Device {
        build: Box::new(move || histories_command(&device).map_err(|e| format!("History import: {e}"))),
        after: follow_up(root, writable),
    }
}

fn follow_up(root: PathBuf, writable: bool) -> Box<dyn FnOnce(&Effect) -> DeviceNote + Send> {
    Box::new(move |effect| {
        // Checked again before writing, like exports (the stick may have been swapped).
        if !writable || crate::protect::write_refusal(&root).is_some() {
            return DeviceNote { note: Some("device is protected, nothing written to it".into()), reload: None };
        }
        match apply_to_device(&root, effect) {
            Ok(Some(note)) => DeviceNote { note: Some(note), reload: Some(root) },
            Ok(None) => DeviceNote::default(),
            Err(e) => DeviceNote { note: Some(format!("device not updated: {e}")), reload: None },
        }
    })
}

/// The history import command: every export.pdb history not yet in `ImportedHistory.xml`.
pub fn histories_command(root: &Path) -> Result<Command, String> {
    let db = read_export(root)?;
    let histories: Vec<DeviceHistory> = mb_library::device_history::device_histories(&db);
    if histories.is_empty() {
        return Err("the device has no histories".into());
    }
    Ok(Command::ImportHistories { histories, already: read_imported(root), now: now() })
}

fn read_imported(root: &Path) -> ImportedHistory {
    std::fs::read_to_string(rekordbox_dir(root).join("ImportedHistory.xml"))
        .map(|t| ImportedHistory::parse(&t))
        .unwrap_or_default()
}

fn now() -> LocalTime {
    use chrono::{Datelike, Timelike};
    let t = chrono::Local::now();
    LocalTime { year: t.year(), month: t.month(), day: t.day(), hour: t.hour(), minute: t.minute(), second: t.second() }
}

/// Writes what an import changes on the stick: the master IDs of imported device rows, and the
/// imported histories in `ImportedHistory.xml`. Returns a short note for the status line.
pub fn apply_to_device(root: &Path, effect: &mb_library::Effect) -> Result<Option<String>, String> {
    match effect {
        mb_library::Effect::ImportedFromDevice(report) => {
            let stamps: Vec<(u32, u32, u32)> =
                report.results.iter().filter_map(|r| r.stamp.map(|(dbid, cid)| (r.device_id, dbid, cid))).collect();
            if stamps.is_empty() {
                return Ok(None);
            }
            let dev = mb_usb::DeviceState::load(root, None).map_err(|e| e.to_string())?;
            mb_usb::stamp::stamp_master_ids(&dev, &stamps).map_err(|e| e.to_string())?;
            Ok(Some(format!("{} device row(s) linked to the collection", stamps.len())))
        }
        mb_library::Effect::HistoriesImported(report) if !report.imported.is_empty() => {
            let mut ih = read_imported(root);
            for (id, plus, _, hash) in &report.imported {
                ih.add(*plus, *id, hash.clone());
            }
            let path = rekordbox_dir(root).join("ImportedHistory.xml");
            std::fs::write(&path, ih.to_xml()).map_err(|e| format!("{}: {e}", path.display()))?;
            mb_usb::appledouble::remove_companions(root, [path.as_path()]);
            Ok(Some("recorded in ImportedHistory.xml".into()))
        }
        _ => Ok(None),
    }
}
