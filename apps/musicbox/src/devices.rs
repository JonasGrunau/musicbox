//! USB/SD devices carrying a rekordbox export. Discovery polls `/Volumes`; devices are only ever
//! opened read-only (exportLibrary.db with SQLite `immutable=1`, so no side files are created).

use mb_core::LibrarySnapshot;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub enum DeviceState {
    Loading(Receiver<Result<LibrarySnapshot, String>>),
    Ready(Box<LibrarySnapshot>),
    Failed(String),
}

pub struct Device {
    pub name: String,
    pub root: PathBuf,
    pub state: DeviceState,
    /// Why musicbox must not write to this device (protected volume), if so.
    pub write_refusal: Option<String>,
}

impl Device {
    pub fn snapshot(&self) -> Option<&LibrarySnapshot> {
        match &self.state {
            DeviceState::Ready(s) => Some(s.as_ref()),
            _ => None,
        }
    }
}

pub struct Devices {
    pub list: Vec<Device>,
    /// Roots passed with `--device`; always listed, never unmounted.
    pinned: Vec<PathBuf>,
    scan_volumes: bool,
    last_scan: Option<Instant>,
    removable_cache: std::collections::HashMap<PathBuf, bool>,
}

const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// A volume is a rekordbox device if it has `PIONEER/rekordbox/` with an export database.
pub fn is_device(root: &Path) -> bool {
    let db = root.join("PIONEER/rekordbox");
    db.join("exportLibrary.db").is_file() || db.join("export.pdb").is_file()
}

/// USB or SD volume, like rekordbox's `RemovableDeviceDetector` (protocol `USB` / `Secure Digital`,
/// docs/re/export.md § 6). Result per mount path is cached by the caller.
fn is_removable(root: &Path) -> bool {
    let Ok(out) = std::process::Command::new("/usr/sbin/diskutil").arg("info").arg("-plist").arg(root).output() else {
        return false;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let value = |key: &str| -> Option<String> {
        let rest = &text[text.find(&format!("<key>{key}</key>"))?..];
        let start = rest.find("<string>")? + 8;
        Some(rest[start..start + rest[start..].find("</string>")?].to_owned())
    };
    matches!(value("BusProtocol").or_else(|| value("Protocol")).as_deref(), Some("USB" | "Secure Digital"))
}

fn mounted_devices(removable_cache: &mut std::collections::HashMap<PathBuf, bool>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| !p.is_symlink())
        .filter(|p| is_device(p) || *removable_cache.entry(p.clone()).or_insert_with(|| is_removable(p)))
        .collect();
    roots.sort();
    roots
}

fn spawn_load(ctx: &egui::Context, root: PathBuf) -> Receiver<Result<LibrarySnapshot, String>> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(load(&root));
        ctx.request_repaint();
    });
    rx
}

/// rekordbox's own device browser reads export.pdb (`rekordboxDBController::startupDeviceDB`),
/// which is also the database players update (e.g. tag lists). exportLibrary.db is the fallback.
fn load(root: &Path) -> Result<LibrarySnapshot, String> {
    let dir = root.join("PIONEER/rekordbox");
    if !is_device(root) {
        // A stick without a rekordbox export yet: empty library, ready for a first export.
        return Ok(LibrarySnapshot::default());
    }
    if dir.join("export.pdb").is_file() {
        return mb_pdb::DeviceLibrary::open(&dir).map(|lib| lib.snapshot()).map_err(|e| e.to_string());
    }
    let lib = mb_devlib::DevLibDb::open_immutable(&dir.join("exportLibrary.db")).map_err(|e| e.to_string())?;
    lib.load_snapshot(root).map_err(|e| e.to_string())
}

impl Devices {
    /// Re-reads a device's library (after an export).
    pub fn reload(&mut self, ctx: &egui::Context, root: &Path) {
        if let Some(device) = self.list.iter_mut().find(|d| d.root == root) {
            device.state = DeviceState::Loading(spawn_load(ctx, root.to_path_buf()));
        }
    }

    pub fn new(pinned: Vec<PathBuf>, scan_volumes: bool) -> Self {
        Self { list: Vec::new(), pinned, scan_volumes, last_scan: None, removable_cache: Default::default() }
    }

    /// Rescans mounts every couple of seconds and finishes background loads.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            let mut roots = self.pinned.clone();
            if self.scan_volumes {
                roots.extend(mounted_devices(&mut self.removable_cache));
            }
            self.list.retain(|d| roots.contains(&d.root));
            for root in roots {
                if !self.list.iter().any(|d| d.root == root) {
                    let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                    let state = DeviceState::Loading(spawn_load(ctx, root.clone()));
                    let write_refusal = crate::protect::write_refusal(&root);
                    self.list.push(Device { name, root, state, write_refusal });
                }
            }
            ctx.request_repaint_after(SCAN_INTERVAL);
        }
        for device in &mut self.list {
            if let DeviceState::Loading(rx) = &device.state
                && let Ok(result) = rx.try_recv()
            {
                device.state = match result {
                    Ok(snapshot) => DeviceState::Ready(Box::new(snapshot)),
                    Err(e) => DeviceState::Failed(e),
                };
            }
        }
    }
}
