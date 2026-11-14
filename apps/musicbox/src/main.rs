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
    // In musicbox.app the Dock shows the bundle's icon, drawn by the same code (`tools/bundle`);
    // an empty icon makes eframe keep it instead of its own. Only `cargo run` rasterises one.
    let icon = if bundled() { egui::IconData::default() } else { musicbox::app_icon::icon_data(512) };
    let options = eframe::NativeOptions {
        // The title bar is transparent and the content runs under it: the app's top bar is the
        // title bar (docs/design.md § Layout).
        viewport: egui::ViewportBuilder::default()
            .with_title("musicbox")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 600.0])
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_icon(std::sync::Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native(
        "musicbox",
        options,
        Box::new(move |cc| {
            musicbox::install(&cc.egui_ctx);
            let mut app = musicbox::App::new(cc.egui_ctx.clone(), args);
            app.install_menu_bar();
            #[cfg(target_os = "macos")]
            {
                if let Some(title_bar) = musicbox::window_chrome::install(cc) {
                    app.set_title_bar(title_bar);
                }
                if let Some(drag) = musicbox::window_chrome::install_drag(cc) {
                    app.set_window_drag(drag);
                }
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Running from a `.app` bundle that carries the icon (`Contents/Resources/musicbox.icns`).
fn bundled() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("Resources/musicbox.icns")))
        .is_some_and(|icns| icns.is_file())
}
