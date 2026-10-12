//! The device side of an import from a device (`docs/re/ops/import-device.md` § Device rows):
//! after a track was added to the collection, rekordbox writes the new master IDs onto the
//! stick's row (`updateMasterDBInfo`, vtable +0x428 on the device controller, called from
//! `importFromDeviceThread::run` @0x1001d9618) so that the next export recognises the track
//! (`export.md` § 4.1). rekordbox writes only the database being browsed; musicbox writes the
//! row in both databases (found by path in exportLibrary.db) so they stay in lock-step.

use crate::remove::{check_unchanged, full_fsync, refuse_reference_stick};
use crate::{DeviceState, Error, Result};
use mb_devlib::DevLibWriter;
use mb_pdb::writer::Exporter;

/// Sets `masterDbId` / `masterContentId` of export.pdb tracks (by ID) and of the exportLibrary.db
/// rows with the same path. `stamps`: (export.pdb track ID, master DB ID, master content ID).
pub fn stamp_master_ids(dev: &DeviceState, stamps: &[(u32, u32, u32)]) -> Result<()> {
    refuse_reference_stick(&dev.root)?;
    if stamps.is_empty() {
        return Ok(());
    }
    let rb = dev.root.join(crate::naming::pioneer_dir(dev.hidden)).join("rekordbox");
    let Some(files) = &dev.pdb else {
        return Err(Error::Device("the device has no export.pdb".into()));
    };
    let mut ex = Exporter::load(&files.export_bytes)?;
    let mut paths = Vec::new();
    for (id, dbid, cid) in stamps {
        let Some(t) = ex.track(*id) else { continue };
        if (t.master_db_id, t.master_content_id) == (*dbid, *cid) {
            continue;
        }
        let mut t = t.clone();
        t.master_db_id = *dbid;
        t.master_content_id = *cid;
        paths.push((t.folder_path.clone(), *dbid, *cid));
        ex.update_track(&t)?;
    }
    if paths.is_empty() {
        return Ok(());
    }

    if dev.devlib.is_some() {
        let target = rb.join("exportLibrary.db");
        let tmp = rb.join(".exportLibrary.db.musicbox");
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", tmp.display()));
        }
        std::fs::copy(&target, &tmp).map_err(|e| Error::io(&target, e))?;
        let result = (|| -> Result<()> {
            let w = DevLibWriter::open(&tmp)?;
            for (path, dbid, cid) in &paths {
                w.connection().execute(
                    "UPDATE content SET masterDbId = ?1, masterContentId = ?2 WHERE path = ?3",
                    rusqlite::params![i64::from(*dbid), i64::from(*cid), path],
                )?;
            }
            w.close()?;
            Ok(())
        })();
        if let Err(e) = result {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", tmp.display()));
            }
            return Err(e);
        }
        full_fsync(&tmp)?;
        std::fs::rename(&tmp, &target).map_err(|e| Error::io(&target, e))?;
        full_fsync(&target)?;
    }

    let path = rb.join("export.pdb");
    check_unchanged(&path, &files.export_bytes)?;
    crate::exec::write_replace(&path, &ex.finish())?;
    full_fsync(&path)
}
