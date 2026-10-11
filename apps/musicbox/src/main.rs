mod app;
mod browser;
mod devices;
mod library;
mod theme;
mod tree;
mod waveform;

use std::path::PathBuf;

/// Command-line overrides, mainly for running against fixture copies:
/// `musicbox [--options <options.json>] [--db <master.db>] [--analysis-root <dir>] [--device <root>]...`
#[derive(Default, Clone)]
pub struct Args {
    pub options: Option<PathBuf>,
    pub db: Option<PathBuf>,
    pub analysis_root: Option<PathBuf>,
    /// Extra device roots (folders containing `PIONEER/`), e.g. fixture copies.
    pub devices: Vec<PathBuf>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("{flag} needs a value"));
        match flag.as_str() {
            "--options" => args.options = Some(value()?),
            "--db" => args.db = Some(value()?),
            "--analysis-root" => args.analysis_root = Some(value()?),
            "--device" => args.devices.push(value()?),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    Ok(args)
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("musicbox")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "musicbox",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc.egui_ctx.clone(), args)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
