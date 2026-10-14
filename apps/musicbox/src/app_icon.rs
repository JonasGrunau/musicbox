//! The app icon (Dock, window, `musicbox.app`): the musicbox mark from `icons::Icon::Logo`, a blue
//! rounded tile holding three level bars, drawn on the macOS icon grid (an 824/1024 body with a
//! soft shadow). Rendered in code at any size, so no image file is kept in the repo.

/// Straight-alpha RGBA pixels of the icon, `size` × `size`.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32 / 1024.0;
    // Body on the macOS grid: 824 × 824 at (100, 100), corner radius 185.
    let body = RoundRect { cx: 512.0 * s, cy: 512.0 * s, hw: 412.0 * s, hh: 412.0 * s, r: 185.0 * s };
    // The bars of the 16-unit logo grid (x 4, 7, 10; width 2; half-heights 2, 4.5, 3; centre y 8),
    // mapped onto the body.
    let unit = 824.0 * s / 16.0;
    let bars: Vec<RoundRect> = [(4.0, 2.0), (7.0, 4.5), (10.0, 3.0)]
        .iter()
        .map(|&(x, h)| RoundRect {
            cx: 100.0 * s + (x + 1.0) * unit,
            cy: 100.0 * s + 8.0 * unit,
            hw: unit,
            hh: h * unit,
            r: unit,
        })
        .collect();
    let shadow = RoundRect { cy: body.cy + 10.0 * s, ..body };
    let blur = (24.0 * s).max(1.0);
    let top = [0x33u8, 0x8F, 0xFF].map(f32::from);
    let bottom = [0x0Eu8, 0x58, 0xCC].map(f32::from);
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Shadow below the body: fades out over `blur` pixels.
            let d = shadow.distance(px, py);
            let mut color = [0.0f32; 3];
            let mut alpha = 0.32 * (1.0 - (d / blur).clamp(0.0, 1.0)).powi(2);
            // Body with a vertical gradient.
            let t = ((py - (body.cy - body.hh)) / (2.0 * body.hh)).clamp(0.0, 1.0);
            let fill = [0, 1, 2].map(|i| top[i] + (bottom[i] - top[i]) * t);
            over(&mut color, &mut alpha, fill, coverage(body.distance(px, py)));
            // Bars, near black.
            let bar = bars.iter().map(|b| coverage(b.distance(px, py))).fold(0.0, f32::max);
            over(&mut color, &mut alpha, [11.0, 11.0, 11.0], bar * 0.94);
            let i = ((y * size + x) * 4) as usize;
            out[i..i + 3].copy_from_slice(&color.map(|c| c.round().clamp(0.0, 255.0) as u8));
            out[i + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

pub fn icon_data(size: u32) -> egui::IconData {
    egui::IconData { rgba: rgba(size), width: size, height: size }
}

#[derive(Clone, Copy)]
struct RoundRect {
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    r: f32,
}

impl RoundRect {
    /// Signed distance in pixels (negative inside).
    fn distance(&self, x: f32, y: f32) -> f32 {
        let qx = (x - self.cx).abs() - (self.hw - self.r);
        let qy = (y - self.cy).abs() - (self.hh - self.r);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) - self.r
    }
}

/// Anti-aliased coverage of a shape from its signed distance.
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Paints `src` with opacity `a` over a straight-alpha pixel.
fn over(color: &mut [f32; 3], alpha: &mut f32, src: [f32; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let out = a + *alpha * (1.0 - a);
    for i in 0..3 {
        color[i] = (src[i] * a + color[i] * *alpha * (1.0 - a)) / out;
    }
    *alpha = out;
}

#[cfg(test)]
mod tests {
    #[test]
    fn transparent_corners_opaque_centre() {
        let size = 64;
        let px = super::rgba(size);
        assert_eq!(px[3], 0, "corner is transparent");
        let centre = ((size / 2 * size + size / 2) * 4) as usize;
        assert_eq!(px[centre + 3], 255, "centre is opaque");
    }
}
