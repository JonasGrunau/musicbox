//! Removing playlists and tracks from a device. Spec: `docs/re/export.md` § 4.3 (rekordbox's
//! row and file rules per database), § 8 (what `mb-usb` does differently).
//!
//! rekordbox deletes from the one database the user browses and looks at the other database on
//! the stick only to decide which files may go. musicbox keeps export.pdb and exportLibrary.db
//! in lock-step, so a removal runs on both:
//! 1. **Plan** ([`plan_remove`], pure): lists are found by their name path from the top level
//!    (like the export matches lists by parent and name), tracks by device path. With
//!    [`RemoveRequest::delete_orphan_tracks`] (rekordbox's `DeletePlaylistTracks`), the tracks
//!    of the deleted lists that no remaining list of either database uses are removed too. A
//!    file is deleted only when no remaining row of either database references it.
//! 2. **Execute** ([`execute_remove`]): exportLibrary.db (built next to the file and renamed
//!    into place), export.pdb, exportExt.pdb, the Sync Manager files, then the files: per track
//!    the artwork (`_m`, `_l`), the ANLZ set, the audio (only below `/Contents`).

use crate::device::DeviceState;
use crate::naming::{path_key, pioneer_dir};
use crate::{Error, Result};
use mb_pdb::writer::{Exporter, ExtEditor};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The user's reference stick (`fixtures/manifest.toml`): never written, whatever the caller
/// checked before.
const REFERENCE_STICK_UUID: &str = "2335E32F-F224-3E76-8EE8-42C79B582A15";

/// What to remove.
#[derive(Debug, Clone, Default)]
pub struct RemoveRequest {
    /// Playlists or folders by name path from the top level, e.g. `["Folder", "List"]`.
    /// Everything below a folder goes with it.
    pub playlists: Vec<Vec<String>>,
    /// Tracks by device path (`/Contents/…`), removed from the collection of both databases.
    pub tracks: Vec<String>,
    /// rekordbox's `DeletePlaylistTracks`: also remove the tracks of the deleted lists that no
    /// remaining list (in either database) uses.
    pub delete_orphan_tracks: bool,
    /// Absolute paths of files the local library still references (`delete_device_song_file`
    /// keeps on-stick audio that master.db points at). Matching audio stays.
    pub local_paths: HashSet<PathBuf>,
}

/// One file a removal deletes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveFile {
    /// An image file and its `_m` / `_l` companions (`delete_artwork_file`).
    Artwork(String),
    /// An ANLZ set: the `.DAT` path; `.EXT` and `.2EX` next to it go too.
    Anlz(String),
    /// The audio file.
    Audio(String),
}

/// Everything a removal will do.
#[derive(Debug, Clone, Default)]
pub struct RemovePlan {
    pub root: PathBuf,
    pub hidden: bool,
    /// export.pdb lists to delete (each with everything below it), in request order.
    pub pdb_lists: Vec<u32>,
    /// The same lists in exportLibrary.db.
    pub devlib_lists: Vec<u32>,
    /// Every deleted list ID, descendants included (for `playlists3*.sync`).
    pub pdb_lists_all: Vec<u32>,
    pub devlib_lists_all: Vec<u32>,
    /// Device paths of the tracks removed from the collection.
    pub track_paths: Vec<String>,
    pub pdb_tracks: Vec<u32>,
    pub devlib_tracks: Vec<u32>,
    pub files: Vec<RemoveFile>,
}

impl RemovePlan {
    pub fn is_empty(&self) -> bool {
        self.pdb_lists.is_empty()
            && self.devlib_lists.is_empty()
            && self.pdb_tracks.is_empty()
            && self.devlib_tracks.is_empty()
            && self.files.is_empty()
    }
}

/// What a removal did.
#[derive(Debug, Clone, Default)]
pub struct RemoveReport {
    pub lists: usize,
    pub tracks: usize,
    pub files_deleted: usize,
    /// Planned files that were already gone.
    pub files_missing: usize,
}

/// A list as both databases describe it: (id, parent, name, order key).
struct ListRow {
    id: u32,
    parent: u32,
    name: String,
    seq: i64,
}

fn find_list(rows: &[ListRow], path: &[String]) -> Option<u32> {
    let mut parent = 0;
    let mut found = None;
    for name in path {
        let hit = rows
            .iter()
            .filter(|r| r.parent == parent && &r.name == name)
            .min_by_key(|r| (r.seq, r.id))?;
        parent = hit.id;
        found = Some(hit.id);
    }
    found
}

fn subtree(rows: &[ListRow], top: u32, out: &mut Vec<u32>) {
    let mut kids: Vec<&ListRow> = rows
        .iter()
        .filter(|r| r.parent == top && r.id != top)
        .collect();
    kids.sort_by_key(|r| (r.seq, r.id));
    for k in kids {
        out.push(k.id);
        subtree(rows, k.id, out);
    }
}

/// Resolves the requested lists in one database: (top lists, every deleted ID).
fn resolve_lists(rows: &[ListRow], paths: &[Vec<String>]) -> (Vec<u32>, Vec<u32>) {
    let mut tops = Vec::new();
    let mut all: Vec<u32> = Vec::new();
    for p in paths {
        let Some(id) = find_list(rows, p) else {
            continue;
        };
        if all.contains(&id) {
            continue;
        }
        let mut sub = Vec::new();
        subtree(rows, id, &mut sub);
        sub.push(id);
        // A list requested earlier that lies below this one is part of this subtree now.
        tops.retain(|t| !sub.contains(t));
        tops.push(id);
        for s in sub {
            if !all.contains(&s) {
                all.push(s);
            }
        }
    }
    (tops, all)
}

/// Plans a removal (`docs/re/export.md` § 4.3; pure).
pub fn plan_remove(dev: &DeviceState, req: &RemoveRequest) -> Result<RemovePlan> {
    let pdb = dev.pdb.as_ref().map(|p| &p.export);
    let devlib = dev.devlib.as_ref();
    let pdb_rows: Vec<ListRow> = pdb
        .map(|e| {
            e.playlists
                .iter()
                .map(|n| ListRow {
                    id: n.0.id,
                    parent: n.0.parent_id,
                    name: n.0.name.clone(),
                    seq: i64::from(n.0.seq),
                })
                .collect()
        })
        .unwrap_or_default();
    let devlib_rows: Vec<ListRow> = devlib
        .map(|d| {
            d.playlists
                .iter()
                .map(|l| ListRow {
                    id: l.id,
                    parent: l.parent,
                    name: l.name.clone(),
                    seq: l.seq,
                })
                .collect()
        })
        .unwrap_or_default();
    for p in &req.playlists {
        if find_list(&pdb_rows, p).is_none() && find_list(&devlib_rows, p).is_none() {
            return Err(Error::Selection(format!(
                "playlist \"{}\" is not on the device",
                p.join(" / ")
            )));
        }
    }
    let (pdb_lists, pdb_lists_all) = resolve_lists(&pdb_rows, &req.playlists);
    let (devlib_lists, devlib_lists_all) = resolve_lists(&devlib_rows, &req.playlists);

    // Device path of every track row, per database.
    let pdb_paths: HashMap<u32, &str> = pdb
        .map(|e| {
            e.tracks
                .iter()
                .map(|t| (t.id, t.folder_path.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let devlib_paths: HashMap<u32, &str> = devlib
        .map(|d| d.tracks.iter().map(|t| (t.id, t.path.as_str())).collect())
        .unwrap_or_default();

    // Tracks to remove, by path key, in order.
    let mut remove: Vec<(String, String)> = Vec::new();
    let push = |remove: &mut Vec<(String, String)>, path: &str| {
        let key = path_key(path);
        if !remove.iter().any(|(k, _)| *k == key) {
            remove.push((key, path.to_owned()));
        }
    };
    for path in &req.tracks {
        let key = path_key(path);
        let known = pdb_paths.values().any(|p| path_key(p) == key)
            || devlib_paths.values().any(|p| path_key(p) == key);
        if !known {
            return Err(Error::Selection(format!(
                "track {path} is not on the device"
            )));
        }
        push(&mut remove, path);
    }
    if req.delete_orphan_tracks {
        // `removePlaylistTrackIfNotUsed`: the deleted lists' tracks no remaining list uses.
        let pdb_entries: Vec<(u32, u32)> = pdb
            .map(|e| {
                let mut v: Vec<_> = e.playlist_entries.iter().collect();
                v.sort_by_key(|x| (x.playlist_id, x.entry_index));
                v.into_iter().map(|x| (x.playlist_id, x.track_id)).collect()
            })
            .unwrap_or_default();
        let devlib_entries: Vec<(u32, u32)> = devlib
            .map(|d| {
                let mut lists: Vec<_> = d.entries.iter().collect();
                lists.sort_by_key(|(l, _)| **l);
                lists
                    .into_iter()
                    .flat_map(|(l, cs)| cs.iter().map(move |c| (*l, *c)))
                    .collect()
            })
            .unwrap_or_default();
        let mut used: HashSet<String> = HashSet::new();
        for (l, t) in &pdb_entries {
            if !pdb_lists_all.contains(l)
                && let Some(p) = pdb_paths.get(t)
            {
                used.insert(path_key(p));
            }
        }
        for (l, c) in &devlib_entries {
            if !devlib_lists_all.contains(l)
                && let Some(p) = devlib_paths.get(c)
            {
                used.insert(path_key(p));
            }
        }
        let mut candidates: Vec<&str> = Vec::new();
        for l in &pdb_lists_all {
            for (pl, t) in &pdb_entries {
                if pl == l
                    && let Some(p) = pdb_paths.get(t)
                {
                    candidates.push(p);
                }
            }
        }
        for l in &devlib_lists_all {
            for (pl, c) in &devlib_entries {
                if pl == l
                    && let Some(p) = devlib_paths.get(c)
                {
                    candidates.push(p);
                }
            }
        }
        for p in candidates {
            if !used.contains(&path_key(p)) {
                push(&mut remove, p);
            }
        }
    }
    let remove_keys: HashSet<&str> = remove.iter().map(|(k, _)| k.as_str()).collect();
    let mut pdb_tracks: Vec<u32> = Vec::new();
    let mut devlib_tracks: Vec<u32> = Vec::new();
    for (key, _) in &remove {
        let mut ids: Vec<u32> = pdb_paths
            .iter()
            .filter(|(_, p)| path_key(p) == *key)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        pdb_tracks.extend(ids);
        let mut ids: Vec<u32> = devlib_paths
            .iter()
            .filter(|(_, p)| path_key(p) == *key)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        devlib_tracks.extend(ids);
    }

    // What the remaining rows still reference.
    let mut keep_audio: HashSet<String> = HashSet::new();
    let mut keep_anlz: HashSet<String> = HashSet::new();
    let mut keep_images: HashSet<String> = HashSet::new();
    let pdb_image = |id: u32| -> Option<&str> {
        pdb?.artwork
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.path.as_str())
    };
    if let Some(e) = pdb {
        for t in e
            .tracks
            .iter()
            .filter(|t| !remove_keys.contains(path_key(&t.folder_path).as_str()))
        {
            keep_audio.insert(path_key(&t.folder_path));
            keep_anlz.insert(path_key(&t.analysis_path));
            if let Some(p) = pdb_image(t.artwork_id) {
                keep_images.insert(path_key(p));
            }
        }
        // Albums, lists and hot cue bank lists may share an image (conservative: all of them).
        let others = e
            .albums
            .iter()
            .map(|a| a.artwork_id)
            .chain(e.playlists.iter().map(|p| p.0.artwork_id))
            .chain(e.hot_cue_bank_lists.iter().map(|l| l.0.artwork_id));
        for id in others {
            if let Some(p) = pdb_image(id) {
                keep_images.insert(path_key(p));
            }
        }
    }
    if let Some(d) = devlib {
        for t in d
            .tracks
            .iter()
            .filter(|t| !remove_keys.contains(path_key(&t.path).as_str()))
        {
            keep_audio.insert(path_key(&t.path));
            keep_anlz.insert(path_key(&t.analysis_path));
            if let Some(p) = d.images.get(&t.image_id) {
                keep_images.insert(path_key(p));
            }
        }
    }

    // Files, per track: artwork, ANLZ, audio (`modifyRemove`'s order).
    let mut files: Vec<RemoveFile> = Vec::new();
    let add = |files: &mut Vec<RemoveFile>, f: RemoveFile| {
        if !files.contains(&f) {
            files.push(f);
        }
    };
    for (key, path) in &remove {
        let mut images = Vec::new();
        let mut anlz = Vec::new();
        if let Some(e) = pdb {
            for t in e.tracks.iter().filter(|t| path_key(&t.folder_path) == *key) {
                images.extend(pdb_image(t.artwork_id).map(str::to_owned));
                anlz.push(t.analysis_path.clone());
            }
        }
        if let Some(d) = devlib {
            for t in d.tracks.iter().filter(|t| path_key(&t.path) == *key) {
                images.extend(d.images.get(&t.image_id).cloned());
                anlz.push(t.analysis_path.clone());
            }
        }
        for img in images {
            if !img.is_empty() && !keep_images.contains(&path_key(&img)) {
                add(&mut files, RemoveFile::Artwork(img));
            }
        }
        for dat in anlz {
            if !dat.is_empty() && !keep_anlz.contains(&path_key(&dat)) {
                add(&mut files, RemoveFile::Anlz(dat));
            }
        }
        // `delete_device_song_file`: only below `/Contents`, only when no remaining row and no
        // local library file uses it.
        let under_contents = key.starts_with("/contents/");
        let local = req
            .local_paths
            .contains(&dev.root.join(path.trim_start_matches('/')));
        if under_contents && !keep_audio.contains(key) && !local && dev.contents.contains(key) {
            add(&mut files, RemoveFile::Audio(path.clone()));
        }
    }

    Ok(RemovePlan {
        root: dev.root.clone(),
        hidden: dev.hidden,
        pdb_lists,
        devlib_lists,
        pdb_lists_all,
        devlib_lists_all,
        track_paths: remove.into_iter().map(|(_, p)| p).collect(),
        pdb_tracks,
        devlib_tracks,
        files,
    })
}

/// Volume UUID of the volume mounted at `root` (`diskutil info -plist`), upper-case.
fn volume_uuid(root: &Path) -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/diskutil")
        .arg("info")
        .arg("-plist")
        .arg(root)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let rest = &text[text.find("<key>VolumeUUID</key>")?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")? + start;
    Some(rest[start..end].trim().to_uppercase())
}

/// Refuses the reference stick (callers also check the user's protected-volume list).
pub(crate) fn refuse_reference_stick(root: &Path) -> Result<()> {
    if root.starts_with("/Volumes") && volume_uuid(root).as_deref() == Some(REFERENCE_STICK_UUID) {
        return Err(Error::Device(format!(
            "{} is the reference stick; musicbox never writes to it",
            root.display()
        )));
    }
    Ok(())
}

fn fs_path(root: &Path, device_path: &str) -> PathBuf {
    root.join(device_path.trim_start_matches('/'))
}

/// Deletes one file, clearing the read-only bit first (`delete_device_song_file`). `Ok(false)`
/// when it was already gone.
fn delete_file(path: &Path) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(m) => {
            let mut perm = m.permissions();
            if perm.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(path, perm);
            }
            std::fs::remove_file(path).map_err(|e| Error::io(path, e))?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::io(path, e)),
    }
}

pub(crate) fn full_fsync(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| Error::io(path, e))
}

pub(crate) fn check_unchanged(path: &Path, expected: &[u8]) -> Result<()> {
    let now = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    if now != expected {
        return Err(Error::Device(format!(
            "{} changed since the device was read",
            path.display()
        )));
    }
    Ok(())
}

/// Runs a [`RemovePlan`]: the databases first (exportLibrary.db, export.pdb, exportExt.pdb, each
/// written next to its final name and renamed into place), the `Dev_ID`s of the deleted lists
/// in `playlists3*.sync`, then the files. `dev` must be the state the plan was made from.
pub fn execute_remove(plan: &RemovePlan, dev: &DeviceState) -> Result<RemoveReport> {
    refuse_reference_stick(&plan.root)?;
    if plan.root != dev.root {
        return Err(Error::Plan("the plan belongs to another device".into()));
    }
    let rb = plan.root.join(pioneer_dir(plan.hidden)).join("rekordbox");
    let mut report = RemoveReport::default();

    if !plan.devlib_lists.is_empty() || !plan.devlib_tracks.is_empty() {
        crate::exec::edit_devlib(&rb, |w| {
            for id in &plan.devlib_lists {
                w.delete_playlist(*id)?;
            }
            Ok(w.remove_tracks(&plan.devlib_tracks)?)
        })?;
    }

    if let Some(files) = &dev.pdb
        && (!plan.pdb_lists.is_empty() || !plan.pdb_tracks.is_empty())
    {
        let path = rb.join("export.pdb");
        check_unchanged(&path, &files.export_bytes)?;
        let mut ex = Exporter::load(&files.export_bytes)?;
        for id in &plan.pdb_lists {
            if ex.playlist(*id).is_some() {
                ex.delete_playlist(*id)?;
            }
        }
        for id in &plan.pdb_tracks {
            ex.remove_track(*id)?;
        }
        crate::exec::write_replace(&path, &ex.finish())?;
        full_fsync(&path)?;
        if let Some(ext) = &files.ext_bytes
            && !plan.pdb_tracks.is_empty()
        {
            let path = rb.join("exportExt.pdb");
            check_unchanged(&path, ext)?;
            let mut ed = ExtEditor::load(ext)?;
            for id in &plan.pdb_tracks {
                ed.remove_track(*id)?;
            }
            crate::exec::write_replace(&path, &ed.finish())?;
            full_fsync(&path)?;
        }
    }
    crate::syncfile::forget_device_lists(&plan.root, &plan.pdb_lists_all, &plan.devlib_lists_all)?;
    report.lists = plan.pdb_lists_all.len().max(plan.devlib_lists_all.len());
    report.tracks = plan.track_paths.len();

    for f in &plan.files {
        let targets: Vec<PathBuf> = match f {
            RemoveFile::Artwork(p) => {
                let base = fs_path(&plan.root, p);
                let stem = p.rsplit_once('.').map_or(p.as_str(), |(s, _)| s);
                vec![
                    base,
                    fs_path(&plan.root, &format!("{stem}_l.jpg")),
                    fs_path(&plan.root, &format!("{stem}_m.jpg")),
                ]
            }
            RemoveFile::Anlz(dat) => {
                let base = fs_path(&plan.root, dat);
                ["DAT", "EXT", "2EX"]
                    .iter()
                    .map(|ext| base.with_extension(ext))
                    .collect()
            }
            RemoveFile::Audio(p) => vec![fs_path(&plan.root, p)],
        };
        for (i, t) in targets.iter().enumerate() {
            if delete_file(t)? {
                report.files_deleted += 1;
            } else if i == 0 {
                report.files_missing += 1;
            }
        }
    }
    // Deleting a file deletes its companion; the databases and sync files are all in `rb`.
    crate::appledouble::clean_dir(&plan.root, &rb);
    let _ = std::fs::File::open(&plan.root).and_then(|f| f.sync_all());
    Ok(report)
}
