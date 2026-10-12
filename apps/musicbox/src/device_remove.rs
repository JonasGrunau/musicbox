//! Removing playlists and tracks from a device: one background thread per removal, driving
//! `mb-usb` (plan, then execute; `docs/re/export.md` § 4.3). Refuses protected volumes and runs
//! only while rekordbox is closed (rekordbox rewrites every stick it sees).

use mb_usb::{DeviceState, RemoveRequest};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

pub struct RemoveJob {
    pub device_root: PathBuf,
    pub device_name: String,
    rx: Receiver<Result<String, String>>,
}

impl RemoveJob {
    pub fn start(ctx: &egui::Context, device_root: PathBuf, device_name: String, request: RemoveRequest) -> Self {
        let (tx, rx) = channel();
        let (ctx, root) = (ctx.clone(), device_root.clone());
        std::thread::spawn(move || {
            let _ = tx.send(run(&root, &request));
            ctx.request_repaint();
        });
        Self { device_root, device_name, rx }
    }

    /// `Some(result)` once the job has finished.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        self.rx.try_recv().ok()
    }
}

/// Device path (`/Contents/…`) of an absolute path below `root`.
pub fn device_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(format!("/{}", rel.to_string_lossy()))
}

fn run(root: &Path, request: &RemoveRequest) -> Result<String, String> {
    if let Some(reason) = crate::protect::write_refusal(root) {
        return Err(reason);
    }
    let running = mb_masterdb::running_rekordbox();
    if !running.is_empty() {
        return Err(format!("Quit {} before changing a device", running.join(", ")));
    }
    let device = DeviceState::load(root, None).map_err(|e| e.to_string())?;
    let plan = mb_usb::plan_remove(&device, request).map_err(|e| e.to_string())?;
    let report = mb_usb::execute_remove(&plan, &device).map_err(|e| e.to_string())?;
    let mut parts = Vec::new();
    if report.lists > 0 {
        parts.push(format!("{} playlist(s)", report.lists));
    }
    if report.tracks > 0 {
        parts.push(format!("{} track(s)", report.tracks));
    }
    if parts.is_empty() {
        parts.push("nothing".into());
    }
    Ok(format!("Removed {} ({} file(s) deleted)", parts.join(" and "), report.files_deleted))
}
