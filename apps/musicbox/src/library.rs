//! Background loading of the library snapshot (read-only).

use crate::Args;
use mb_core::LibrarySnapshot;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::sync::mpsc::{Receiver, channel};

pub type LoadResult = Result<LibrarySnapshot, String>;

pub fn spawn_load(ctx: egui::Context, args: Args) -> Receiver<LoadResult> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let result = load(&args).map_err(|e| format!("{e:#}"));
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

fn load(args: &Args) -> anyhow::Result<LibrarySnapshot> {
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
    let db = MasterDb::open_read_only(&paths.db_path, &derive_passphrase(&paths.dp)?)?;
    Ok(db.load_snapshot(&paths)?)
}
