//! USB/SD devices carrying a rekordbox export. Discovery polls `/Volumes` on a background thread
//! (`diskutil` takes 0.1–0.2 s per new volume); devices are only ever opened read-only
//! (exportLibrary.db with SQLite `immutable=1`, so no side files are created).

use mb_core::LibrarySnapshot;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

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
    /// The pinned folders were listed and the scanner started (first `poll`).
    started: bool,
    scanner: Option<Scanner>,
}

/// The mount scanner thread: sends the volumes to list, each with its write refusal, whenever
/// that list changes. Dropping it stops the thread.
struct Scanner {
    rx: Receiver<Vec<(PathBuf, Option<String>)>>,
    _stop: Sender<()>,
}

const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Volume paths need `diskutil` for their write refusal (`protect::write_refusal`); other folders
/// (fixture copies) are allowed without a lookup.
fn is_volume_path(root: &Path) -> bool {
    root.starts_with("/Volumes")
}

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

fn mounted_devices(removable_cache: &mut HashMap<PathBuf, bool>) -> Vec<PathBuf> {
    let mounted: Vec<PathBuf> =
        std::fs::read_dir("/Volumes").into_iter().flatten().flatten().map(|e| e.path()).filter(|p| !p.is_symlink()).collect();
    // A volume that went away may come back as another one under the same name.
    removable_cache.retain(|p, _| mounted.contains(p));
    let mut roots: Vec<PathBuf> = mounted
        .into_iter()
        .filter(|p| is_device(p) || *removable_cache.entry(p.clone()).or_insert_with(|| is_removable(p)))
        .collect();
    roots.sort();
    roots
}

/// The device number of the file system mounted at `root`.
fn mount_id(root: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(root).ok().map(|m| m.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        None
    }
}

/// Rescans every couple of seconds: the mounted devices (with `scan_volumes`) after the pinned
/// volume paths. A device's write refusal is looked up once, when it appears, so the app only
/// lists devices whose protection is known.
fn spawn_scanner(ctx: &egui::Context, pinned: Vec<PathBuf>, scan_volumes: bool) -> Scanner {
    let (tx, rx) = channel();
    let (stop_tx, stop) = channel::<()>();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("mb-device-scan".into())
        .spawn(move || {
            let mut removable = HashMap::new();
            // By mount path: the mounted file system (its device number) and its refusal, so a
            // stick swapped between two scans is looked up again.
            let mut refusals: HashMap<PathBuf, (Option<u64>, Option<String>)> = HashMap::new();
            let mut last = None;
            loop {
                let mut roots = pinned.clone();
                if scan_volumes {
                    roots.extend(mounted_devices(&mut removable));
                }
                refusals.retain(|p, _| roots.contains(p));
                let mut found: Vec<(PathBuf, Option<String>)> = Vec::new();
                for root in roots {
                    if !found.iter().any(|(r, _)| *r == root) {
                        let mount = mount_id(&root);
                        let cached = refusals.get(&root).filter(|(m, _)| *m == mount).map(|(_, r)| r.clone());
                        let refusal = cached.unwrap_or_else(|| {
                            let refusal = crate::protect::write_refusal(&root);
                            refusals.insert(root.clone(), (mount, refusal.clone()));
                            refusal
                        });
                        found.push((root, refusal));
                    }
                }
                if last.as_ref() != Some(&found) {
                    if tx.send(found.clone()).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    last = Some(found);
                }
                if !matches!(stop.recv_timeout(SCAN_INTERVAL), Err(RecvTimeoutError::Timeout)) {
                    return;
                }
            }
        })
        .expect("spawn device scanner");
    Scanner { rx, _stop: stop_tx }
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
        Self { list: Vec::new(), pinned, scan_volumes, started: false, scanner: None }
    }

    fn add(&mut self, ctx: &egui::Context, root: PathBuf, write_refusal: Option<String>) {
        if self.list.iter().any(|d| d.root == root) {
            return;
        }
        let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        let state = DeviceState::Loading(spawn_load(ctx, root.clone()));
        self.list.push(Device { name, root, state, write_refusal });
    }

    /// Takes the scanner's mount changes and finishes background loads. Pinned folders outside
    /// `/Volumes` are listed on the first call.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            let (volumes, folders): (Vec<PathBuf>, Vec<PathBuf>) = self.pinned.iter().cloned().partition(|r| is_volume_path(r));
            for root in folders {
                let refusal = crate::protect::write_refusal(&root);
                self.add(ctx, root, refusal);
            }
            if self.scan_volumes || !volumes.is_empty() {
                self.scanner = Some(spawn_scanner(ctx, volumes, self.scan_volumes));
            }
        }
        if let Some(found) = self.scanner.as_ref().and_then(|s| s.rx.try_iter().last()) {
            let pinned = &self.pinned;
            self.list.retain(|d| (pinned.contains(&d.root) && !is_volume_path(&d.root)) || found.iter().any(|(r, _)| *r == d.root));
            for (root, refusal) in found {
                match self.list.iter_mut().find(|d| d.root == root) {
                    Some(device) => device.write_refusal = refusal,
                    None => self.add(ctx, root, refusal),
                }
            }
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
