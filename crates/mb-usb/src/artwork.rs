//! Artwork images as rekordbox makes them (`db::DatabaseMediator::write_image` @0x10149e0c8,
//! `create_rescaled_image` @0x10149e3ac; `docs/re/ops/analyse-track.md` § Artwork,
//! `export.md` § 3).
//!
//! rekordbox works on JUCE images, which on macOS are CoreGraphics bitmaps (sRGB, 8-bit
//! premultiplied ARGB). The picture is decoded by ImageIO; scaling and the square canvas are
//! CoreGraphics draws; `rb::ImageHelper::ignoreAlpha` then sets every alpha byte to 255
//! without un-premultiplying; JUCE's embedded IJG libjpeg writes the file ([`jpeg`]).
//! On macOS this module makes the same calls through the system frameworks ([`cg`]) and
//! produces the same bytes; elsewhere it falls back to the `image` crate (same geometry and
//! encoder, other pixels).

#[cfg(target_os = "macos")]
mod cg;
pub mod jpeg;

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

/// A JUCE ARGB image: rows of `width` pixels, bytes B, G, R, A, colour premultiplied by alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argb {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// Decodes a picture (JPEG, PNG, …) as JUCE does (`juce_loadWithCoreImage`).
pub fn decode(bytes: &[u8]) -> Result<Argb, String> {
    #[cfg(target_os = "macos")]
    {
        cg::decode(bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        fallback::decode(bytes)
    }
}

/// `juce::Image::rescaled` with high resampling quality; the image itself at the same size.
fn rescaled(img: &Argb, width: usize, height: usize) -> Result<Argb, String> {
    if (img.width, img.height) == (width, height) {
        return Ok(img.clone());
    }
    #[cfg(target_os = "macos")]
    {
        cg::rescaled(img, width, height)
    }
    #[cfg(not(target_os = "macos"))]
    {
        fallback::rescaled(img, width, height)
    }
}

/// The image centred on an opaque black square of side max(w, h).
fn squared(img: &Argb) -> Result<Argb, String> {
    #[cfg(target_os = "macos")]
    {
        cg::squared(img)
    }
    #[cfg(not(target_os = "macos"))]
    {
        fallback::squared(img)
    }
}

/// JUCE's `roundToInt` (round half to even, through the 1.5·2⁵² trick).
fn round_to_int(v: f64) -> usize {
    ((v + 6_755_399_441_055_744.0) - 6_755_399_441_055_744.0) as usize
}

/// `create_rescaled_image(img, size, square)`. Not square: fit inside `size` (only ever
/// shrinking; the short side rounded). Square: centre a non-square image on black first, then
/// scale to `size` × `size`.
pub fn create_rescaled_image(img: &Argb, size: usize, square: bool) -> Result<Argb, String> {
    let (w, h) = (img.width, img.height);
    if square {
        return if w == h { rescaled(img, size, size) } else { rescaled(&squared(img)?, size, size) };
    }
    let (nw, nh) = if h < w {
        if size < w { (size, round_to_int(h as f64 * size as f64 / w as f64)) } else { (w, h) }
    } else if w < h {
        if size < h { (round_to_int(w as f64 * size as f64 / h as f64), size) } else { (w, h) }
    } else {
        (w.min(size), h.min(size))
    };
    rescaled(img, nw, nh)
}

/// `ignoreAlpha` + `JPEGImageFormat::writeImageToStream`: alpha forced to 255, the stored
/// (premultiplied) colour bytes written as RGB.
pub fn encode(img: &Argb) -> Vec<u8> {
    let rgb: Vec<u8> = img.data.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
    jpeg::encode(img.width, img.height, &rgb)
}

/// `write_image(img, path, size)`: size 800 keeps an image that fits in 800 × 800 as it is
/// and otherwise fits it in; other sizes make squares.
pub fn write_image(img: &Argb, size: usize) -> Result<Vec<u8>, String> {
    if size == ORIGINAL_MAX as usize {
        if img.width <= size && img.height <= size {
            return Ok(encode(img));
        }
        return Ok(encode(&create_rescaled_image(img, size, false)?));
    }
    Ok(encode(&create_rescaled_image(img, size, true)?))
}

/// A `px` × `px` JPEG of a picture file (the 80/240/320 px variants).
pub fn square_jpeg(picture: &[u8], px: u32) -> Result<Vec<u8>, String> {
    write_image(&decode(picture)?, px as usize)
}

/// `artwork.jpg` of a picture file.
pub fn original_jpeg(picture: &[u8]) -> Result<Vec<u8>, String> {
    write_image(&decode(picture)?, ORIGINAL_MAX as usize)
}

/// The local set from an embedded picture (`writeTagImage` @0x10149d90c: one decoded image,
/// written at 800, 80 and 240).
pub fn local_set(picture: &[u8]) -> Result<LocalArtworkSet, String> {
    let img = decode(picture)?;
    Ok(LocalArtworkSet { original: write_image(&img, 800)?, small: write_image(&img, 80)?, medium: write_image(&img, 240)? })
}

#[cfg(not(target_os = "macos"))]
mod fallback {
    use super::Argb;
    use image::{RgbaImage, imageops};

    fn to_rgba(img: &Argb) -> RgbaImage {
        let px = img.data.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        RgbaImage::from_raw(img.width as u32, img.height as u32, px).expect("buffer size")
    }

    fn from_rgba(img: &RgbaImage) -> Argb {
        let data = img.pixels().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        Argb { width: img.width() as usize, height: img.height() as usize, data }
    }

    pub fn decode(bytes: &[u8]) -> Result<Argb, String> {
        let mut img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
        for p in img.pixels_mut() {
            let a = u16::from(p[3]);
            for c in 0..3 {
                p[c] = ((u16::from(p[c]) * a + 127) / 255) as u8;
            }
        }
        Ok(from_rgba(&img))
    }

    pub fn rescaled(img: &Argb, width: usize, height: usize) -> Result<Argb, String> {
        Ok(from_rgba(&imageops::resize(&to_rgba(img), width as u32, height as u32, imageops::FilterType::Lanczos3)))
    }

    pub fn squared(img: &Argb) -> Result<Argb, String> {
        let side = img.width.max(img.height) as u32;
        let mut canvas = RgbaImage::from_pixel(side, side, image::Rgba([0, 0, 0, 255]));
        let x = (side - img.width as u32) / 2;
        let y = (side - img.height as u32) / 2;
        imageops::overlay(&mut canvas, &to_rgba(img), i64::from(x), i64::from(y));
        Ok(from_rgba(&canvas))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1000, 500, image::Rgb([200, 10, 10])));
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
        // the black bars of the square variants
        let s = image::load_from_memory(&set.small).unwrap().to_rgb8();
        assert!(s.get_pixel(40, 2).0.iter().all(|&c| c < 8));
        assert!(s.get_pixel(40, 40).0[0] > 180);
    }

    /// `ignoreAlpha` keeps the premultiplied colour: half-transparent pixels come out as the
    /// colour over black.
    #[test]
    fn transparency_becomes_colour_over_black() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 100, 40, 128])));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let out = image::load_from_memory(&original_jpeg(&png).unwrap()).unwrap().to_rgb8();
        let p = out.get_pixel(32, 32).0;
        for (got, want) in p.iter().zip([100, 50, 20]) {
            assert!(got.abs_diff(want) <= 3, "{p:?}");
        }
    }

    #[test]
    fn fit_rounding() {
        let img = Argb { width: 1001, height: 999, data: vec![255; 1001 * 999 * 4] };
        let out = create_rescaled_image(&img, 800, false).unwrap();
        assert_eq!((out.width, out.height), (800, 798));
        assert_eq!(round_to_int(2.5), 2);
        assert_eq!(round_to_int(3.5), 4);
    }
}
