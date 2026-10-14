//! Writes the app icon as a macOS icon set (`icon_16x16.png` … `icon_512x512@2x.png`) into a
//! folder, for `iconutil -c icns` (tools/bundle/bundle.sh).
//!
//! `cargo run -p musicbox --example app_icon -- <dir>.iconset`

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: app_icon <dir>.iconset"))?);
    std::fs::create_dir_all(&dir)?;
    for base in [16u32, 32, 128, 256, 512] {
        for scale in [1u32, 2] {
            let size = base * scale;
            let name = if scale == 1 { format!("icon_{base}x{base}.png") } else { format!("icon_{base}x{base}@2x.png") };
            let image = image::RgbaImage::from_raw(size, size, musicbox::app_icon::rgba(size)).expect("size matches");
            image.save(dir.join(name))?;
        }
    }
    println!("wrote {}", dir.display());
    Ok(())
}
