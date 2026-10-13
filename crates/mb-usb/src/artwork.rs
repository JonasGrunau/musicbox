//! Artwork images as rekordbox makes them (`db::DatabaseMediator::write_image` @0x10149e0c8,
//! `docs/re/ops/analyse-track.md` § Artwork, `export.md` § 3). Encoding: baseline JPEG,
//! quality 85, alpha dropped (`rb::ImageHelper::ignoreAlpha`). The pixels are not
//! byte-identical to rekordbox's JUCE resampler and encoder.

use image::DynamicImage;

/// The local artwork set of a collection track: `artwork.jpg` (the image as is up to 800 px,
/// else scaled to fit 800 px), `artwork_s.jpg` (80 px) and `artwork_m.jpg` (240 px) squares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalArtworkSet {
    pub original: Vec<u8>,
    pub small: Vec<u8>,
    pub medium: Vec<u8>,
}

/// Largest side of `artwork.jpg` (`write_image` with size 800).
pub const ORIGINAL_MAX: u32 = 800;

fn jpeg(img: &image::RgbImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(img)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// A `px` × `px` JPEG: the image centred on an opaque black square, then scaled
/// (`create_rescaled_image` with the square flag, as for the 80/240/320 px variants).
pub fn square_jpeg(img: &DynamicImage, px: u32) -> Result<Vec<u8>, String> {
    let (w, h) = (img.width(), img.height());
    let side = w.max(h);
    let mut canvas = image::RgbImage::from_pixel(side, side, image::Rgb([0, 0, 0]));
    image::imageops::overlay(&mut canvas, &img.to_rgb8(), i64::from((side - w) / 2), i64::from((side - h) / 2));
    let scaled = image::imageops::resize(&canvas, px, px, image::imageops::FilterType::Lanczos3);
    jpeg(&scaled)
}

/// `artwork.jpg`: the image itself when both sides are ≤ 800 px, else scaled to fit 800 px
/// with its aspect ratio.
pub fn original_jpeg(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let rgb = if img.width() <= ORIGINAL_MAX && img.height() <= ORIGINAL_MAX {
        img.to_rgb8()
    } else {
        img.resize(ORIGINAL_MAX, ORIGINAL_MAX, image::imageops::FilterType::Lanczos3).to_rgb8()
    };
    jpeg(&rgb)
}

/// The local set from an embedded picture (JPEG or PNG bytes).
pub fn local_set(picture: &[u8]) -> Result<LocalArtworkSet, String> {
    let img = image::load_from_memory(picture).map_err(|e| e.to_string())?;
    Ok(LocalArtworkSet { original: original_jpeg(&img)?, small: square_jpeg(&img, 80)?, medium: square_jpeg(&img, 240)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1000, 500, image::Rgb([200, 10, 10])));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let set = local_set(&png).unwrap();
        let dims = |b: &[u8]| {
            let i = image::load_from_memory(b).unwrap();
            (i.width(), i.height())
        };
        assert_eq!(dims(&set.original), (800, 400));
        assert_eq!(dims(&set.small), (80, 80));
        assert_eq!(dims(&set.medium), (240, 240));
    }
}
