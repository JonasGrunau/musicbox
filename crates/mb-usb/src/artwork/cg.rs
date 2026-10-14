//! The pixel side of rekordbox's artwork code on macOS, through the system frameworks: JUCE
//! decodes with ImageIO and draws with CoreGraphics (`analyse-track.md` § Artwork). Every
//! call below mirrors one JUCE makes, with the same colour space, bitmap layout, context state
//! and transforms, so CoreGraphics produces the same pixels.

use super::Argb;
use std::ffi::c_void;
use std::ptr::null;

type CFTypeRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

/// `kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`: JUCE's ARGB layout
/// (bytes B, G, R, A).
const ARGB_INFO: u32 = 0x2002;
const BLEND_NORMAL: i32 = 0;
const INTERPOLATION_HIGH: i32 = 3;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFTypeRef;
    fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
    fn CGBitmapContextCreate(data: *mut c_void, width: usize, height: usize, bpc: usize, bpr: usize, space: CFTypeRef, info: u32) -> CFTypeRef;
    fn CGContextDrawImage(c: CFTypeRef, rect: CGRect, image: CFTypeRef);
    fn CGContextFlush(c: CFTypeRef);
    fn CGContextSaveGState(c: CFTypeRef);
    fn CGContextRestoreGState(c: CFTypeRef);
    fn CGContextSetShouldSmoothFonts(c: CFTypeRef, v: bool);
    fn CGContextSetAllowsFontSmoothing(c: CFTypeRef, v: bool);
    fn CGContextSetShouldAntialias(c: CFTypeRef, v: bool);
    fn CGContextSetBlendMode(c: CFTypeRef, mode: i32);
    fn CGContextSetInterpolationQuality(c: CFTypeRef, q: i32);
    fn CGContextSetAlpha(c: CFTypeRef, a: f64);
    fn CGContextSetRGBFillColor(c: CFTypeRef, r: f64, g: f64, b: f64, a: f64);
    fn CGContextFillRect(c: CFTypeRef, rect: CGRect);
    fn CGContextConcatCTM(c: CFTypeRef, t: CGAffineTransform);
    fn CGDataProviderCreateWithCFData(data: CFTypeRef) -> CFTypeRef;
    fn CGImageCreate(w: usize, h: usize, bpc: usize, bpp: usize, bpr: usize, space: CFTypeRef, info: u32, provider: CFTypeRef, decode: *const f64, interpolate: bool, intent: i32) -> CFTypeRef;
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithDataProvider(provider: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceCreateImageAtIndex(source: CFTypeRef, index: usize, options: CFTypeRef) -> CFTypeRef;
}

/// A CoreFoundation object released on drop.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

/// A data provider over a copy of `bytes`, so that CoreGraphics may keep it as long as it likes.
fn provider(bytes: &[u8]) -> Cf {
    let data = Cf(unsafe { CFDataCreate(null(), bytes.as_ptr(), bytes.len() as isize) });
    Cf(unsafe { CGDataProviderCreateWithCFData(data.0) })
}

fn srgb() -> Cf {
    Cf(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) })
}

/// A cleared JUCE ARGB image (`CoreGraphicsPixelData`: rows of `(4w + 3) & !3` bytes, sRGB)
/// and the bitmap context drawing into it. The buffer must outlive the context.
fn canvas(width: usize, height: usize) -> Result<(Argb, Cf), String> {
    let mut img = Argb { width, height, data: vec![0; width * 4 * height] };
    let space = srgb();
    let ctx = unsafe { CGBitmapContextCreate(img.data.as_mut_ptr().cast(), width, height, 8, width * 4, space.0, ARGB_INFO) };
    if ctx.is_null() {
        return Err(format!("CGBitmapContextCreate failed for {width}×{height}"));
    }
    Ok((img, Cf(ctx)))
}

/// The state `juce::CoreGraphicsContext`'s constructor sets on a context.
fn juce_context(ctx: &Cf) {
    unsafe {
        CGContextSaveGState(ctx.0);
        CGContextSetShouldSmoothFonts(ctx.0, false);
        CGContextSetAllowsFontSmoothing(ctx.0, false);
        CGContextSetShouldAntialias(ctx.0, true);
        CGContextSetBlendMode(ctx.0, BLEND_NORMAL);
    }
}

/// A CGImage of an ARGB image's pixels (`CoreGraphicsPixelData::getCGImage`; the copy is
/// `getCachedImageRef`'s other path), sRGB.
fn cg_image(img: &Argb) -> Result<Cf, String> {
    let space = srgb();
    let provider = provider(&img.data);
    let image = unsafe { CGImageCreate(img.width, img.height, 8, 32, img.width * 4, space.0, ARGB_INFO, provider.0, null(), true, 0) };
    if image.is_null() {
        return Err("CGImageCreate failed".into());
    }
    Ok(Cf(image))
}

/// `juce::CoreGraphicsContext::drawImage` for a transform given as JUCE's
/// `[m00 m01 m02; m10 m11 m12]` (single precision, as in JUCE), into a context of `flip_height`.
fn draw_transformed(ctx: &Cf, src: &Argb, m: [f32; 6], flip_height: usize) -> Result<(), String> {
    let image = cg_image(src)?;
    let ih = src.height as f32;
    let [m00, m01, m02, m10, m11, m12] = m;
    let fill_opacity = 255.0_f32 * 0.003_921_569_f32;
    unsafe {
        CGContextSaveGState(ctx.0);
        CGContextSetAlpha(ctx.0, f64::from(fill_opacity));
        CGContextConcatCTM(ctx.0, CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: f64::from(flip_height as f32) });
        CGContextConcatCTM(
            ctx.0,
            CGAffineTransform {
                a: f64::from(m00),
                b: f64::from(m10),
                c: f64::from(-m01),
                d: f64::from(-m11),
                tx: f64::from(ih.mul_add(m01, m02)),
                ty: f64::from(ih.mul_add(m11, m12)),
            },
        );
        CGContextDrawImage(ctx.0, CGRect { x: 0.0, y: 0.0, width: src.width as f64, height: src.height as f64 }, image.0);
        CGContextRestoreGState(ctx.0);
    }
    Ok(())
}

/// `juce_loadWithCoreImage`: ImageIO decodes the file, CoreGraphics draws it 1:1 into an
/// sRGB premultiplied ARGB image.
pub fn decode(bytes: &[u8]) -> Result<Argb, String> {
    let provider = provider(bytes);
    let source = Cf(unsafe { CGImageSourceCreateWithDataProvider(provider.0, null()) });
    if source.0.is_null() {
        return Err("ImageIO cannot read the picture".into());
    }
    let image = Cf(unsafe { CGImageSourceCreateImageAtIndex(source.0, 0, null()) });
    if image.0.is_null() {
        return Err("ImageIO cannot decode the picture".into());
    }
    let (w, h) = unsafe { (CGImageGetWidth(image.0), CGImageGetHeight(image.0)) };
    let (img, ctx) = canvas(w, h)?;
    unsafe {
        CGContextDrawImage(ctx.0, CGRect { x: 0.0, y: 0.0, width: w as f64, height: h as f64 }, image.0);
        CGContextFlush(ctx.0);
    }
    drop(ctx);
    Ok(img)
}

/// `juce::Image::rescaled(w, h, highResamplingQuality)`: a new cleared ARGB image, a JUCE
/// context with high interpolation, the source drawn with a scale transform.
pub fn rescaled(src: &Argb, width: usize, height: usize) -> Result<Argb, String> {
    let (img, ctx) = canvas(width, height)?;
    juce_context(&ctx);
    unsafe { CGContextSetInterpolationQuality(ctx.0, INTERPOLATION_HIGH) };
    let sx = width as f32 / src.width as f32;
    let sy = height as f32 / src.height as f32;
    draw_transformed(&ctx, src, [sx, 0.0, 0.0, 0.0, sy, 0.0], height)?;
    drop(ctx);
    Ok(img)
}

/// The square canvas of `create_rescaled_image`: a cleared ARGB image of side max(w, h),
/// `fillAll(black)`, then `drawImageWithin(0, 0, side, side, centred)` with the context's
/// default interpolation.
pub fn squared(src: &Argb) -> Result<Argb, String> {
    let side = src.width.max(src.height);
    let (img, ctx) = canvas(side, side)?;
    juce_context(&ctx);
    let s = side as f32;
    unsafe {
        CGContextSaveGState(ctx.0);
        CGContextSetRGBFillColor(ctx.0, 0.0, 0.0, 0.0, 1.0);
        CGContextFillRect(ctx.0, CGRect { x: -1.0, y: -1.0, width: f64::from(s) + 2.0, height: f64::from(s) + 2.0 });
        CGContextRestoreGState(ctx.0);
    }
    // RectanglePlacement::getTransformToFit with xMid | yMid
    let (w, h) = (src.width as f32, src.height as f32);
    let scale = (s / h).min(s / w);
    let tx = (s - w * scale) * 0.5;
    let ty = (s - h * scale) * 0.5;
    draw_transformed(&ctx, src, [scale, 0.0, tx, 0.0, scale, ty], side)?;
    drop(ctx);
    Ok(img)
}
