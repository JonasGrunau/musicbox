//! USB export jobs: one background thread per export, driving `mb-usb` (plan, then execute) with
//! progress and cancellation. Refuses protected volumes and runs only while rekordbox is closed
//! (rekordbox rewrites every stick it sees).

use crate::actions::ExportItem;
use mb_usb::source::LibraryDir;
use mb_usb::{CancelToken, DeviceState, ExportOptions, PlanOptions, Progress, SelectionItem, SourceLibrary};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

/// Everything the worker needs about the source library.
#[derive(Clone)]
pub struct Source {
    pub db_path: PathBuf,
    pub passphrase: String,
    pub analysis_root: PathBuf,
    pub settings_root: PathBuf,
    /// For the export log's language.
    pub lang_path: Option<PathBuf>,
}

enum Msg {
    Progress(Progress),
    Finished(Result<String, String>),
}

pub struct ExportJob {
    pub device_root: PathBuf,
    pub device_name: String,
    rx: Receiver<Msg>,
    cancel: CancelToken,
    pub progress: Option<Progress>,
}

/// Where musicbox writes export logs (rekordbox uses `~/Documents/rekordbox`).
fn log_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/musicbox/export-logs"))
}

impl ExportJob {
    pub fn start(ctx: &egui::Context, source: Source, item: ExportItem, device_root: PathBuf, device_name: String) -> Self {
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let (ctx, root, token) = (ctx.clone(), device_root.clone(), cancel.clone());
        let selection = vec![match item {
            ExportItem::Playlist(id) => SelectionItem::Playlist(id.0),
            ExportItem::Tracks(ids) => SelectionItem::Tracks(ids.into_iter().map(|id| id.0).collect()),
        }];
        std::thread::spawn(move || {
            let result = run(&source, selection, &root, &token, &mut |p| {
                let _ = tx.send(Msg::Progress(p.clone()));
                ctx.request_repaint();
            });
            let _ = tx.send(Msg::Finished(result));
            ctx.request_repaint();
        });
        Self { device_root, device_name, rx, cancel, progress: None }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Latest progress; `Some(result)` once the job has finished.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let mut finished = None;
        for msg in self.rx.try_iter() {
            match msg {
                Msg::Progress(p) => self.progress = Some(p),
                Msg::Finished(r) => finished = Some(r),
            }
        }
        finished
    }
}

fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))
}

fn run(
    source: &Source,
    selection: Vec<SelectionItem>,
    root: &std::path::Path,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(&Progress),
) -> Result<String, String> {
    // The stick side (the protection check runs `diskutil`; the load reads the whole stick) and
    // the library side are independent: both run at once. Nothing is written before both
    // checks have passed.
    let (refusal, device, local) = std::thread::scope(|s| {
        let refusal = s.spawn(|| crate::protect::write_refusal(root));
        let device = s.spawn(|| DeviceState::load(root, None).map_err(|e| e.to_string()));
        let local = (|| {
            let running = mb_masterdb::running_rekordbox();
            if !running.is_empty() {
                return Err(format!("Quit {} before exporting", running.join(", ")));
            }
            let library = LibraryDir::open_paths(&source.db_path, &source.passphrase, source.analysis_root.clone(), source.settings_root.clone())
                .map_err(|e| e.to_string())?;
            let src =SourceLibrary::load(library.connection(), &library.analysis_root, &selection).map_err(|e| e.to_string())?;
            Ok((library, selection, src))
        })();
        (join(refusal), join(device), local)
    });
    if let Some(reason) = refusal {
        return Err(reason);
    }
    let (library, selection, src) = local?;
    let device = device?;
    let mut plan_options = PlanOptions::default();
    if let Some(t) = mb_usb::settings::hot_cue_color_type(&library.settings_root) {
        plan_options.hot_cue_color_type = t;
    }
    let plan = mb_usb::plan_export(&src, &device, &selection, &plan_options).map_err(|e| e.to_string())?;
    let options = ExportOptions {
        settings_dir: Some(library.settings_root.clone()),
        log_dir: log_dir(),
        log_strings: mb_usb::LogStrings::from_lang_file(source.lang_path.as_deref()),
        ..ExportOptions::default()
    };
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let report = mb_usb::execute(&plan, &device, &options, cancel, &mut |p| {
        if last.elapsed().as_millis() >= 100 || p.tracks_done == p.tracks_total {
            last = std::time::Instant::now();
            progress(p);
        }
    })
    .map_err(|e| e.to_string())?;
    let skipped = report.results.iter().filter(|r| !r.is_success()).count();
    let mut summary = if report.cancelled {
        format!("Export cancelled after {} track(s)", report.completed)
    } else {
        format!("Exported {} track(s)", report.completed)
    };
    if skipped > 0 {
        summary.push_str(&format!(", {skipped} not exported (see export log)"));
    }
    if let Some(e) = report.error {
        return Err(format!("{summary}; stopped: {e}"));
    }
    Ok(summary)
}
