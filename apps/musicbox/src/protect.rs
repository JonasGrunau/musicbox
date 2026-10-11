//! Write protection for devices. musicbox never writes to a volume whose UUID is listed in
//! `~/Library/Application Support/musicbox/protected-volumes.txt` (one UUID per line, `#` comments).
//! The file is created on first use with the user's reference stick, which must never change.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The user's reference stick (docs/progress.md, fixtures/manifest.toml).
const DEFAULT_PROTECTED: &[(&str, &str)] = &[("2335E32F-F224-3E76-8EE8-42C79B582A15", "reference stick REKORDBOX")];

pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/musicbox/protected-volumes.txt"))
}

/// Protected UUIDs (upper-case). Creates the file with the defaults when missing.
pub fn protected_uuids() -> Vec<String> {
    let Some(path) = config_path() else {
        return DEFAULT_PROTECTED.iter().map(|(u, _)| (*u).to_owned()).collect();
    };
    if !path.exists() {
        let mut text = String::from("# Volumes musicbox must never write to (volume UUID per line).\n");
        for (uuid, note) in DEFAULT_PROTECTED {
            text.push_str(&format!("{uuid}  # {note}\n"));
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, text);
    }
    let mut uuids: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('#').next())
        .map(|l| l.trim().to_uppercase())
        .filter(|l| !l.is_empty())
        .collect();
    // The defaults stay protected even if someone edits them out of the file.
    for (uuid, _) in DEFAULT_PROTECTED {
        if !uuids.iter().any(|u| u == uuid) {
            uuids.push((*uuid).to_owned());
        }
    }
    uuids
}

/// Volume UUID of the volume mounted at `mount` (`diskutil info -plist`), upper-case.
pub fn volume_uuid(mount: &Path) -> Option<String> {
    let out = Command::new("/usr/sbin/diskutil").arg("info").arg("-plist").arg(mount).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let key = text.find("<key>VolumeUUID</key>")?;
    let rest = &text[key..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")? + start;
    Some(rest[start..end].trim().to_uppercase())
}

/// Why writing to `mount` is refused, or `None` when it is allowed. Directories that are not a
/// mounted volume (fixture copies) have no UUID and are allowed.
pub fn write_refusal(mount: &Path) -> Option<String> {
    if !mount.starts_with("/Volumes") {
        return None;
    }
    match volume_uuid(mount) {
        Some(uuid) if protected_uuids().contains(&uuid) => Some(format!("{} is write-protected (volume {uuid})", mount.display())),
        Some(_) => None,
        None => Some(format!("cannot identify volume {}; refusing to write", mount.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_stick_is_always_protected() {
        assert!(protected_uuids().contains(&"2335E32F-F224-3E76-8EE8-42C79B582A15".to_owned()));
    }

    #[test]
    fn non_volume_dirs_are_allowed() {
        assert_eq!(write_refusal(Path::new("/tmp/some-fixture-copy")), None);
    }
}
