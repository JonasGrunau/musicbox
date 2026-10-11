fn main() -> anyhow::Result<()> {
    let args = musicbox::parse_args(std::env::args().skip(1))?;
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
