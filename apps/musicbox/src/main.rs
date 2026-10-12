const USAGE: &str = "usage: musicbox [--options <options.json>] [--db <master.db>] \
[--analysis-root <dir>] [--device <root>]... [--backup-root <dir>] [--import-folder <dir>] \
[--no-volume-scan]";

fn main() -> anyhow::Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.iter().any(|a| a == "--version") {
        println!("musicbox {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let args = musicbox::parse_args(raw)?;
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
            musicbox::install(&cc.egui_ctx);
            Ok(Box::new(musicbox::App::new(cc.egui_ctx.clone(), args)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
