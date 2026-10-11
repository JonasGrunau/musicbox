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
}

const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// A volume is a rekordbox device if it has `PIONEER/rekordbox/` with an export database.
pub fn is_device(root: &Path) -> bool {
    let db = root.join("PIONEER/rekordbox");
    db.join("exportLibrary.db").is_file() || db.join("export.pdb").is_file()
}

fn mounted_devices() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_device(p))
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
    if dir.join("export.pdb").is_file() {
        return mb_pdb::DeviceLibrary::open(&dir).map(|lib| lib.snapshot()).map_err(|e| e.to_string());
    }
    let lib = mb_devlib::DevLibDb::open_immutable(&dir.join("exportLibrary.db")).map_err(|e| e.to_string())?;
    lib.load_snapshot(root).map_err(|e| e.to_string())
}

impl Devices {
    pub fn new(pinned: Vec<PathBuf>, scan_volumes: bool) -> Self {
        Self { list: Vec::new(), pinned, scan_volumes, last_scan: None }
    }

    /// Rescans mounts every couple of seconds and finishes background loads.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            let mut roots = self.pinned.clone();
            if self.scan_volumes {
                roots.extend(mounted_devices());
            }
            self.list.retain(|d| roots.contains(&d.root));
            for root in roots {
                if !self.list.iter().any(|d| d.root == root) {
                    let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                    let state = DeviceState::Loading(spawn_load(ctx, root.clone()));
                    self.list.push(Device { name, root, state });
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
