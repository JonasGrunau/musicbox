//! Sync Manager files `playlists3.sync` (export.pdb) and `playlists3Plus.sync`
//! (exportLibrary.db). Spec: `docs/re/export.md` § 4.4.
//!
//! Each file maps master playlist IDs to the device playlist IDs (`Dev_ID`) of its own database.
//! rekordbox's auto-sync deletes device lists by their stored `Dev_ID` when the master list is
//! gone or moved (phase 1). Device playlist IDs are `max + 1`, so a deleted list's ID can be
//! taken again by a new list; a node still holding it would then make rekordbox delete the wrong
//! list. [`forget_device_lists`] clears such `Dev_ID`s when musicbox deletes lists.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// The classic file (export.pdb IDs).
pub const CLASSIC: &str = "playlists3.sync";
/// The Device Library Plus file (exportLibrary.db IDs).
pub const PLUS: &str = "playlists3Plus.sync";

/// One `<NODE>`: a master list kept in sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncNode {
    /// Master playlist ID (printed as uppercase unpadded hex).
    pub id: u64,
    pub parent_id: u64,
    /// Master list type as a signed char: 0 playlist, 1 folder, 4 smart, −128 cloud trial list.
    pub attribute: i8,
    /// 0 rekordbox, 1 iTunes.
    pub lib_type: i32,
    /// The list's ID in this file's database (0 = none).
    pub dev_id: u32,
    /// The master list's change time in ms as of the last sync.
    pub timestamp: i64,
    pub check_type: i32,
}

/// A whole sync file (`DeviceSyncSetting::write` @0x100f9dc08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFile {
    /// The owning library's DBID, as rekordbox prints it (signed i32).
    pub dbid: i64,
    pub automatic_sync: bool,
    pub all_playlists: bool,
    pub forced_sync: bool,
    pub nodes: Vec<SyncNode>,
}

impl SyncFile {
    /// Parses the file rekordbox writes (one element per line, attributes in a fixed order; the
    /// order is not relied on here).
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut file: Option<SyncFile> = None;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("<Sync ") {
                let a = attrs(rest);
                file = Some(SyncFile {
                    dbid: get(&a, "DBID")?.parse().ok()?,
                    automatic_sync: get(&a, "AutomaticSync")? == "1",
                    all_playlists: get(&a, "AllPlaylists")? == "1",
                    forced_sync: get(&a, "ForcedSync").is_some_and(|v| v == "1"),
                    nodes: Vec::new(),
                });
            } else if let Some(rest) = line.strip_prefix("<NODE ") {
                let a = attrs(rest);
                file.as_mut()?.nodes.push(SyncNode {
                    id: u64::from_str_radix(get(&a, "Id")?, 16).ok()?,
                    parent_id: u64::from_str_radix(get(&a, "ParentId")?, 16).ok()?,
                    attribute: get(&a, "Attribute")?.parse().ok()?,
                    lib_type: get(&a, "Lib_Type")?.parse().ok()?,
                    dev_id: get(&a, "Dev_ID")?.parse().ok()?,
                    timestamp: get(&a, "Timestamp")?.parse().ok()?,
                    check_type: get(&a, "CheckType")?.parse().ok()?,
                });
            }
        }
        file
    }

    /// The bytes rekordbox writes: no BOM, CRLF endings (also after the last line), an empty
    /// line after the declaration, two-space indentation; `IncludeCue` and the root `Timestamp`
    /// are constants.
    pub fn to_bytes(&self) -> Vec<u8> {
        let b = |v: bool| if v { 1 } else { 0 };
        let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n");
        s += &format!(
            "<Sync DBID=\"{}\" AutomaticSync=\"{}\" AllPlaylists=\"{}\" IncludeCue=\"1\" ForcedSync=\"{}\" Timestamp=\"0\">\r\n",
            self.dbid,
            b(self.automatic_sync),
            b(self.all_playlists),
            b(self.forced_sync)
        );
        if self.nodes.is_empty() {
            s += "  <Playlists/>\r\n";
        } else {
            s += "  <Playlists>\r\n";
            for n in &self.nodes {
                s += &format!(
                    "    <NODE Id=\"{:X}\" ParentId=\"{:X}\" Attribute=\"{}\" Lib_Type=\"{}\" Dev_ID=\"{}\" Timestamp=\"{}\" CheckType=\"{}\"/>\r\n",
                    n.id, n.parent_id, n.attribute, n.lib_type, n.dev_id, n.timestamp, n.check_type
                );
            }
            s += "  </Playlists>\r\n";
        }
        s += "</Sync>\r\n";
        s.into_bytes()
    }
}

fn attrs(s: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(eq) = rest.find("=\"") {
        let key = rest[..eq].trim();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else { break };
        out.push((key, &after[..end]));
        rest = &after[end + 1..];
    }
    out
}

fn get<'a>(a: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    a.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The sync files of a device: `<root>/PIONEER/rekordbox`, then `<root>/.PIONEER/rekordbox`
/// (`PlaylistSyncManager::getDeviceSettingPath` @0x100fa8194).
pub fn sync_file_path(root: &Path, name: &str) -> Option<PathBuf> {
    ["PIONEER", ".PIONEER"]
        .iter()
        .map(|p| root.join(p).join("rekordbox").join(name))
        .find(|p| p.is_file())
}

/// Clears the `Dev_ID` of every node that points at a deleted device list (`classic`: export.pdb
/// IDs, `plus`: exportLibrary.db IDs), in both files, whatever library owns them. A file is only
/// rewritten when a node changed.
///
/// **Hypothesis:** with `Dev_ID` 0 rekordbox's auto-sync has no device list to delete for that
/// node; the list is exported again once the master list's timestamp changes. What rekordbox
/// itself does to the files when a device list is deleted outside Sync Manager is open
/// (`docs/re/export.md` § 4.4).
pub fn forget_device_lists(root: &Path, classic: &[u32], plus: &[u32]) -> Result<()> {
    for (name, ids) in [(CLASSIC, classic), (PLUS, plus)] {
        if ids.is_empty() {
            continue;
        }
        let Some(path) = sync_file_path(root, name) else {
            continue;
        };
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let Some(mut file) = SyncFile::parse(&bytes) else {
            continue;
        };
        let mut changed = false;
        for n in &mut file.nodes {
            if n.dev_id != 0 && ids.contains(&n.dev_id) {
                n.dev_id = 0;
                changed = true;
            }
        }
        if changed {
            crate::exec::write_replace(&path, &file.to_bytes())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<Sync DBID=\"-1966741239\" AutomaticSync=\"1\" AllPlaylists=\"0\" IncludeCue=\"1\" ForcedSync=\"0\" Timestamp=\"0\">\r\n  <Playlists>\r\n    <NODE Id=\"0\" ParentId=\"0\" Attribute=\"1\" Lib_Type=\"0\" Dev_ID=\"0\" Timestamp=\"0\" CheckType=\"1\"/>\r\n    <NODE Id=\"74447453\" ParentId=\"0\" Attribute=\"-128\" Lib_Type=\"0\" Dev_ID=\"16391\" Timestamp=\"0\" CheckType=\"1\"/>\r\n    <NODE Id=\"3C45F838\" ParentId=\"0\" Attribute=\"4\" Lib_Type=\"0\" Dev_ID=\"1\" Timestamp=\"1788290388106\" CheckType=\"1\"/>\r\n  </Playlists>\r\n</Sync>\r\n";

    #[test]
    fn round_trip() {
        let f = SyncFile::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(f.dbid, -1966741239);
        assert!(f.automatic_sync && !f.all_playlists && !f.forced_sync);
        assert_eq!(f.nodes.len(), 3);
        assert_eq!(f.nodes[1].id, 0x74447453);
        assert_eq!(f.nodes[1].attribute, -128);
        assert_eq!(f.nodes[1].dev_id, 16391);
        assert_eq!(f.nodes[2].timestamp, 1788290388106);
        assert_eq!(f.to_bytes(), SAMPLE.as_bytes());
    }

    #[test]
    fn forget_clears_only_matching_nodes() {
        let dir = std::env::temp_dir().join(format!("mb-sync-{}", std::process::id()));
        let rb = dir.join("PIONEER/rekordbox");
        std::fs::create_dir_all(&rb).unwrap();
        std::fs::write(rb.join(PLUS), SAMPLE).unwrap();
        forget_device_lists(&dir, &[1], &[16391]).unwrap();
        let f = SyncFile::parse(&std::fs::read(rb.join(PLUS)).unwrap()).unwrap();
        assert_eq!(
            f.nodes.iter().map(|n| n.dev_id).collect::<Vec<_>>(),
            [0, 0, 1]
        );
        assert!(!rb.join(CLASSIC).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
