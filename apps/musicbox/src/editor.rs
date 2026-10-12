//! Write access to the local library: one background thread owns the only write connection
//! (plan § UI ↔ DB contract). It refuses to start while rekordbox runs, backs the library up once
//! per session before the first write, and keeps the undo history.

use mb_library::{Command, History, Library};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

pub enum Request {
    Run(Command),
    Undo,
    Redo,
}

pub enum Reply {
    Ready { backup: PathBuf },
    Failed(String),
    /// A command finished; `changed` means the snapshot must be reloaded.
    Done { message: String, changed: bool },
    /// Sent before `Done` for imports from a device: the app applies the stick-side part.
    DeviceEffect(mb_library::Effect),
}

pub struct Editor {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    pub ready: bool,
    pub pending: usize,
}

impl Editor {
    /// `analysis_root` lets grid edits find the analysis files; they are backed up into the
    /// session backup folder before their first change.
    pub fn start(ctx: &egui::Context, db_path: &Path, passphrase: &str, backup_root: Option<PathBuf>, analysis_root: Option<PathBuf>) -> Self {
        let (tx, requests) = channel::<Request>();
        let (replies, rx) = channel::<Reply>();
        let (ctx, db_path, passphrase) = (ctx.clone(), db_path.to_path_buf(), passphrase.to_owned());
        std::thread::spawn(move || {
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
            for request in requests {
                let result = match request {
                    Request::Run(command) => history.run(&mut lib, command).map(Some),
                    Request::Undo => history.undo(&mut lib),
                    Request::Redo => history.redo(&mut lib),
                };
                if let Ok(Some(outcome)) = &result
                    && matches!(outcome.effect, mb_library::Effect::ImportedFromDevice(_) | mb_library::Effect::HistoriesImported(_))
                {
                    send(Reply::DeviceEffect(outcome.effect.clone()));
                }
                send(match result {
                    Ok(Some(outcome)) => Reply::Done { message: describe(&outcome.effect), changed: true },
                    Ok(None) => Reply::Done { message: "Nothing to undo or redo".into(), changed: false },
                    Err(e) => Reply::Done { message: format!("Error: {e}"), changed: true },
                });
            }
        });
        Self { tx, rx, ready: false, pending: 0 }
    }

    pub fn send(&mut self, request: Request) {
        if self.tx.send(request).is_ok() {
            self.pending += 1;
        }
    }

    pub fn poll(&mut self) -> Vec<Reply> {
        let replies: Vec<Reply> = self.rx.try_iter().collect();
        for r in &replies {
            match r {
                Reply::Ready { .. } => self.ready = true,
                Reply::Done { .. } => self.pending = self.pending.saturating_sub(1),
                Reply::Failed(_) | Reply::DeviceEffect(_) => {}
            }
        }
        replies
    }
}

/// One-line status text for a finished command.
pub fn describe(effect: &mb_library::Effect) -> String {
    use mb_library::Effect;
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
        Effect::Done => "Done".into(),
        Effect::Unchanged => "No change".into(),
    }
}
