//! Background loading of the library snapshot (read-only).

use crate::Args;
use mb_core::LibrarySnapshot;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

/// A loaded local library plus what the editor needs to open it for writing.
pub struct Loaded {
    pub snapshot: LibrarySnapshot,
    /// `djmdCue` rows by content ID.
    pub cues: std::collections::HashMap<mb_core::DbId, Vec<mb_library::Cue>>,
    pub db_path: PathBuf,
    pub passphrase: String,
    pub analysis_root: PathBuf,
    /// rekordbox's settings folder (MYSETTING*.DAT etc., copied to sticks on export).
    pub settings_root: PathBuf,
    /// rekordbox's language file (options.json `lang-path`).
    pub lang_path: Option<PathBuf>,
}

pub type LoadResult = Result<Loaded, String>;

pub fn spawn_load(ctx: egui::Context, args: Args) -> Receiver<LoadResult> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let result = load(&args).map_err(|e| format!("{e:#}"));
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

fn load(args: &Args) -> anyhow::Result<Loaded> {
    let options = args
        .options
        .clone()
        .or_else(LibraryPaths::default_options_json)
        .ok_or_else(|| anyhow::anyhow!("cannot locate rekordboxAgent options.json"))?;
    let mut paths = LibraryPaths::from_options_json(&options)?;
    if let Some(db) = &args.db {
        paths.db_path = db.clone();
    }
    if let Some(root) = &args.analysis_root {
        paths.analysis_data_root = root.clone();
    }
    let passphrase = derive_passphrase(&paths.dp)?;
    let db = MasterDb::open_read_only(&paths.db_path, &passphrase)?;
    let snapshot = db.load_snapshot(&paths)?;
    let cues = mb_library::cues::load_all(db.connection())?;
    Ok(Loaded {
        snapshot,
        cues,
        db_path: paths.db_path,
        passphrase,
        analysis_root: paths.analysis_data_root,
        settings_root: paths.settings_root,
        lang_path: paths.lang_path,
    })
}
