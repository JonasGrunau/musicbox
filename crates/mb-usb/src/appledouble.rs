//! AppleDouble cleanup. On FAT32 and exFAT macOS keeps a file's extended attributes in a
//! companion file `._<name>` (4 KiB, magic `00 05 16 07`). The kernel tags every file and folder
//! a provenance-tracked process creates with `com.apple.provenance`, which cannot be removed, so
//! writing "bytes only" is not enough: a stick written by such a process collects one `._` file
//! per file and folder (1107 on a test stick). rekordbox's own writes leave none, and players
//! list them in folder browsing, so every device operation removes the companions of what it
//! wrote. Only true AppleDouble files are deleted (magic checked).

use std::io::Read;
use std::path::{Path, PathBuf};

const MAGIC: [u8; 4] = [0x00, 0x05, 0x16, 0x07];

/// The companion of `path`: `._<file name>` in the same folder.
fn companion(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with("._") {
        return None;
    }
    Some(path.with_file_name(format!("._{name}")))
}

fn is_apple_double(path: &Path) -> bool {
    let mut head = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head == MAGIC)
}

fn remove_if_apple_double(path: &Path) -> bool {
    path.is_file() && is_apple_double(path) && std::fs::remove_file(path).is_ok()
}

/// Removes the companions of `paths` and of their folders up to (not including) `root`.
/// Returns how many were removed.
pub fn remove_companions<'a>(root: &Path, paths: impl IntoIterator<Item = &'a Path>) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut removed = 0;
    for path in paths {
        let mut p = Some(path);
        while let Some(cur) = p {
            if cur == root || !cur.starts_with(root) || !seen.insert(cur.to_path_buf()) {
                break;
            }
            if let Some(c) = companion(cur)
                && remove_if_apple_double(&c)
            {
                removed += 1;
            }
            p = cur.parent();
        }
    }
    removed
}

/// Removes every AppleDouble companion under `dir` (recursively) and the companion of `dir`
/// itself (up to `root`). For the device's `PIONEER` folder, which only the export writes.
pub fn clean_tree(root: &Path, dir: &Path) -> usize {
    let mut removed = remove_companions(root, [dir]);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                stack.push(path);
            } else if e.file_name().to_str().is_some_and(|n| n.starts_with("._"))
                && remove_if_apple_double(&path)
            {
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_apple_double_companions() {
        let dir = std::env::temp_dir().join(format!("mb-appledouble-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("PIONEER/rekordbox");
        std::fs::create_dir_all(&sub).unwrap();
        let mut ad = MAGIC.to_vec();
        ad.resize(4096, 0);
        std::fs::write(sub.join("export.pdb"), b"pdb").unwrap();
        std::fs::write(sub.join("._export.pdb"), &ad).unwrap();
        std::fs::write(sub.join("._notes"), b"not apple double").unwrap();
        std::fs::write(dir.join("PIONEER/._rekordbox"), &ad).unwrap();
        std::fs::write(dir.join("._PIONEER"), &ad).unwrap();
        std::fs::create_dir_all(dir.join("Contents/A")).unwrap();
        std::fs::write(dir.join("Contents/A/t.mp3"), b"mp3").unwrap();
        std::fs::write(dir.join("Contents/A/._t.mp3"), &ad).unwrap();
        std::fs::write(dir.join("Contents/._A"), &ad).unwrap();
        std::fs::write(dir.join("Contents/._other.mp3"), &ad).unwrap();
        std::fs::write(dir.join("._Contents"), &ad).unwrap();

        assert_eq!(clean_tree(&dir, &dir.join("PIONEER")), 3);
        assert_eq!(remove_companions(&dir, [dir.join("Contents/A/t.mp3").as_path()]), 3);
        assert!(sub.join("._notes").exists(), "not AppleDouble: kept");
        assert!(dir.join("Contents/._other.mp3").exists(), "not written by us: kept");
        assert!(!dir.join("._PIONEER").exists() && !dir.join("._Contents").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
