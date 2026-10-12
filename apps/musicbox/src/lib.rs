//! musicbox app as a library, so the headless UI tests (egui_kittest) can build the real `App`.

pub mod actions;
pub mod app;
pub mod browser;
pub mod cue_panel;
pub mod device_import;
pub mod device_remove;
pub mod devices;
pub mod editor;
pub mod export;
pub mod library;
pub mod mysettings_ui;
pub mod protect;
pub mod smartlist_ui;
pub mod theme;
pub mod tree;
pub mod waveform;
pub mod xml_bridge;

pub use app::App;

use std::path::PathBuf;

/// Command-line overrides, mainly for running against fixture copies:
/// `musicbox [--options <options.json>] [--db <master.db>] [--analysis-root <dir>] [--device <root>]...
/// [--backup-root <dir>]`
#[derive(Default, Clone)]
pub struct Args {
    pub options: Option<PathBuf>,
    pub db: Option<PathBuf>,
    pub analysis_root: Option<PathBuf>,
    /// Extra device roots (folders containing `PIONEER/`), e.g. fixture copies.
    pub devices: Vec<PathBuf>,
    /// Where session backups go (default `~/Library/Application Support/musicbox/backups`).
    pub backup_root: Option<PathBuf>,
    /// Don't list mounted volumes as devices (tests).
    pub no_volume_scan: bool,
    /// Where "Import to Collection" copies audio (default `~/Music/PioneerDJ/Imported from Device`).
    pub import_folder: Option<PathBuf>,
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<Args> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("{flag} needs a value"));
        match flag.as_str() {
            "--options" => out.options = Some(value()?),
            "--db" => out.db = Some(value()?),
            "--analysis-root" => out.analysis_root = Some(value()?),
            "--device" => out.devices.push(value()?),
            "--backup-root" => out.backup_root = Some(value()?),
            "--no-volume-scan" => out.no_volume_scan = true,
            "--import-folder" => out.import_folder = Some(value()?),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    Ok(out)
}

/// Shared setup for the native window and the test harness.
pub fn install(ctx: &egui::Context) {
    theme::apply(ctx);
    egui_extras::install_image_loaders(ctx);
}
