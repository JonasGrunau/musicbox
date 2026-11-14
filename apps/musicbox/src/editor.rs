//! Write access to the local library: one background thread owns the only write connection
//! (plan § UI ↔ DB contract). It refuses to start while rekordbox runs, backs the library up once
//! per session before the first write, and keeps the undo history.

use mb_library::{Command, Effect, History, Library};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

pub enum Request {
    Run(Command),
    /// An import from a device (`device_import`): `build` reads the device and makes the command,
    /// `after` writes the stick-side part of its outcome. Both run on this thread, so the UI never
    /// waits for the stick. A `build` error is the reply's message.
    Device { build: Box<dyn FnOnce() -> Result<Command, String> + Send>, after: Box<dyn FnOnce(&Effect) -> DeviceNote + Send> },
    Undo,
    Redo,
}

/// What the stick-side part of a device import did.
#[derive(Debug, Default)]
pub struct DeviceNote {
    /// For the status line.
    pub note: Option<String>,
    /// The device to read again (it was written).
    pub reload: Option<PathBuf>,
}

pub enum Reply {
    Ready { backup: PathBuf },
    Failed(String),
    /// A command finished; `changed` means the snapshot must be reloaded.
    Done { message: String, changed: bool },
    /// Sent before `Done` for imports from a device.
    DeviceEffect(DeviceNote),
    /// Queued commands that were not run because editing was aborted ([`Editor::abort`]).
    Skipped(usize),
}

pub struct Editor {
    /// `None` once stopped: the thread ends after the running command.
    tx: Option<Sender<Request>>,
    rx: Receiver<Reply>,
    pub ready: bool,
    pub pending: usize,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Set by `abort`: queued commands are not run, only the running one finishes.
    stopped: Arc<AtomicBool>,
}

impl Editor {
    /// `analysis_root` lets grid edits find the analysis files; they are backed up into the
    /// session backup folder before their first change.
    pub fn start(ctx: &egui::Context, db_path: &Path, passphrase: &str, backup_root: Option<PathBuf>, analysis_root: Option<PathBuf>) -> Self {
        let (tx, requests) = channel::<Request>();
        let (replies, rx) = channel::<Reply>();
        let (ctx, db_path, passphrase) = (ctx.clone(), db_path.to_path_buf(), passphrase.to_owned());
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || {
            let send = |r: Reply| {
                let _ = replies.send(r);
                ctx.request_repaint();
            };
            let backup_root = match backup_root.or_else(mb_masterdb::default_backup_root) {
                Some(root) => root,
                None => return send(Reply::Failed("HOME is not set".into())),
            };
            let mut lib = match Library::open(&db_path, &passphrase) {
                Ok(lib) => lib,
                Err(e) => return send(Reply::Failed(e.to_string())),
            };
            if let Some(root) = analysis_root {
                lib.set_analysis_root(root);
            }
            match mb_masterdb::backup_library(&db_path, &passphrase, &backup_root) {
                Ok(dir) => {
                    lib.set_anlz_backup(&dir);
                    send(Reply::Ready { backup: dir })
                }
                Err(e) => return send(Reply::Failed(format!("backup failed, editing disabled: {e}"))),
            }
            let mut history = History::default();
            for request in requests.iter() {
                if stop.load(Ordering::Acquire) {
                    // This one and everything still queued (the sender is gone, so this ends).
                    send(Reply::Skipped(1 + requests.iter().count()));
                    break;
                }
                let (result, after) = match request {
                    Request::Run(command) => (history.run(&mut lib, command).map(Some), None),
                    Request::Device { build, after } => match build() {
                        Ok(command) => (history.run(&mut lib, command).map(Some), Some(after)),
                        Err(message) => {
                            send(Reply::Done { message, changed: false });
                            continue;
                        }
                    },
                    Request::Undo => (history.undo(&mut lib), None),
                    Request::Redo => (history.redo(&mut lib), None),
                };
                if let (Ok(Some(outcome)), Some(after)) = (&result, after)
                    && matches!(outcome.effect, Effect::ImportedFromDevice(_) | Effect::HistoriesImported(_))
                {
                    send(Reply::DeviceEffect(after(&outcome.effect)));
                }
                send(match result {
                    Ok(Some(outcome)) => Reply::Done { message: describe(&outcome.effect), changed: true },
                    Ok(None) => Reply::Done { message: "Nothing to undo or redo".into(), changed: false },
                    Err(e) => Reply::Done { message: format!("Error: {e}"), changed: true },
                });
            }
        });
        Self { tx: Some(tx), rx, ready: false, pending: 0, thread: Some(thread), stopped }
    }

    pub fn send(&mut self, request: Request) {
        if let Some(tx) = &self.tx
            && tx.send(request).is_ok()
        {
            self.pending += 1;
        }
    }

    /// Ends editing once the queued commands are done: no new ones are taken, the thread runs
    /// the rest and closes the write connection. Keep polling until [`Editor::finished`].
    pub fn finish(&mut self) {
        self.tx = None;
        self.ready = false;
    }

    /// Ends editing now (rekordbox started, and nothing may write master.db while it runs): the
    /// running command finishes, the queued ones are not run and are reported as
    /// [`Reply::Skipped`].
    pub fn abort(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.finish();
    }

    /// The thread has ended (after [`Editor::finish`] or [`Editor::abort`]); its last replies
    /// are still to be polled.
    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Waits for the thread, which closes master.db: a connection closing while `exit` runs
    /// SQLCipher's cleanup crashes (seen in the UI tests).
    pub fn join(mut self) {
        self.finish();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    pub fn poll(&mut self) -> Vec<Reply> {
        let replies: Vec<Reply> = self.rx.try_iter().collect();
        for r in &replies {
            match r {
                Reply::Ready { .. } => self.ready = true,
                Reply::Done { .. } => self.pending = self.pending.saturating_sub(1),
                Reply::Skipped(n) => self.pending = self.pending.saturating_sub(*n),
                Reply::Failed(_) | Reply::DeviceEffect(_) => {}
            }
        }
        replies
    }
}

/// Watches for rekordbox processes on one background thread (reading the process list takes tens
/// of milliseconds): every 2 s while watching (editing is on), and once per [`Guard::check`].
pub struct Guard {
    tx: Sender<GuardMsg>,
    rx: Receiver<(u64, Vec<String>)>,
    last_check: u64,
    watching: bool,
}

enum GuardMsg {
    Check(u64),
    Watch(bool),
}

const GUARD_INTERVAL: Duration = Duration::from_secs(2);

impl Guard {
    /// The thread idles until asked; it ends with the `Guard`.
    pub fn start(ctx: &egui::Context) -> Self {
        let (tx, requests) = channel::<GuardMsg>();
        let (results, rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("mb-rekordbox-guard".into())
            .spawn(move || {
                let (mut watching, mut seen) = (false, 0);
                let mut last: Option<Vec<String>> = None;
                loop {
                    let msg = if watching { requests.recv_timeout(GUARD_INTERVAL) } else { requests.recv().map_err(|_| RecvTimeoutError::Disconnected) };
                    let asked = match msg {
                        Ok(GuardMsg::Check(id)) => {
                            seen = seen.max(id);
                            true
                        }
                        Ok(GuardMsg::Watch(on)) => {
                            watching = on;
                            if !on {
                                continue;
                            }
                            false
                        }
                        Err(RecvTimeoutError::Timeout) => false,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    // The watch only reports changes, so the UI is not woken every 2 s.
                    let running = mb_masterdb::running_rekordbox();
                    if !asked && last.as_ref() == Some(&running) {
                        continue;
                    }
                    last = Some(running.clone());
                    if results.send((seen, running)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn rekordbox guard");
        Self { tx, rx, last_check: 0, watching: false }
    }

    /// Asks for a fresh process list; returns its ID for [`Guard::poll`].
    pub fn check(&mut self) -> u64 {
        self.last_check += 1;
        let _ = self.tx.send(GuardMsg::Check(self.last_check));
        self.last_check
    }

    /// Turns the 2 s watch on or off.
    pub fn watch(&mut self, on: bool) {
        if on != self.watching {
            self.watching = on;
            let _ = self.tx.send(GuardMsg::Watch(on));
        }
    }

    /// The newest process list, with the ID of the last check it answers.
    pub fn poll(&self) -> Option<(u64, Vec<String>)> {
        self.rx.try_iter().last()
    }
}

/// One-line status text for a finished command.
pub fn describe(effect: &Effect) -> String {
    match effect {
        Effect::Imported(report) if report.skipped.is_empty() => format!("Imported {} file(s)", report.imported.len()),
        Effect::Imported(report) => format!("Imported {} file(s), skipped {}", report.imported.len(), report.skipped.len()),
        Effect::Removed { ids } => format!("Removed {} track(s) from the collection", ids.len()),
        Effect::PlaylistCreated { .. } => "Playlist created".into(),
        Effect::PlaylistsRestored { ids } => format!("Restored {} list(s)", ids.len()),
        Effect::EntriesAdded { entry_ids } => format!("Added {} track(s) to the playlist", entry_ids.len()),
        Effect::CueAdded { .. } => "Cue added".into(),
        Effect::ImportedFromDevice(r) => {
            let new = r.results.iter().filter(|x| matches!(x.kind, mb_library::DeviceImportKind::Imported { .. })).count();
            let mut s = format!("Imported {new} track(s) from the device ({} already in the collection)", r.results.len() - new);
            if !r.skipped.is_empty() {
                s += &format!(", skipped {}", r.skipped.len());
            }
            s
        }
        Effect::HistoriesImported(r) => format!("Imported {} history list(s), skipped {}", r.imported.len(), r.skipped.len()),
        Effect::ImportedXml(r) => {
            let count = |f: fn(&mb_library::XmlTrackResult) -> bool| r.results.iter().filter(|x| f(x)).count();
            let new = count(|x| matches!(x, mb_library::XmlTrackResult::Imported(_)));
            let existing = count(|x| matches!(x, mb_library::XmlTrackResult::InCollection(_)));
            let skipped = count(|x| matches!(x, mb_library::XmlTrackResult::Skipped(_)));
            let mut s = format!("Imported {new} track(s) from rekordbox xml ({existing} already in the collection)");
            if skipped > 0 {
                s += &format!(", skipped {skipped}");
            }
            s
        }
        Effect::Analysed { .. } => "Analysed 1 track".into(),
        Effect::Done => "Done".into(),
        Effect::Unchanged => "No change".into(),
    }
}
