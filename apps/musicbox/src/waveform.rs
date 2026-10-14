//! Track overview drawn from rekordbox's own analysis files (docs/re/anlz.md):
//! PWV4 colour preview or PWV6 3-band preview from .EXT/.2EX, plus cue markers; and a zoomed
//! detail strip with the PQTZ beat grid. The strip draws the 3-band scroll wave (PWV7) when the
//! .2EX has one: PWV5 and PWV3 keep only transient peaks (a kick is a few columns, then zeros),
//! so drawn as they are they leave gaps; PWV7 keeps each band's decay. PWV5 is the fallback.

use mb_anlz::{AnlzFile, Tag};
use mb_anlz::tags::{Beat, CueKind, CueListExt, Wave3Band, WaveColorPreview, WaveColorScroll};
use std::path::Path;

pub struct TrackWaves {
    pub color: Option<WaveColorPreview>,
    pub three_band: Option<Wave3Band>,
    /// PWV5 colour scroll wave (150 columns per second).
    pub scroll: Option<WaveColorScroll>,
    /// PWV7 3-band scroll wave (150 columns per second) and its largest band value.
    pub detail_bands: Option<(Wave3Band, u8)>,
    /// PQTZ beat grid from the .DAT.
    pub grid: Vec<Beat>,
    /// Cues stored in the .EXT (device tracks; collection cues live in the database).
    pub cues: Vec<CueMark>,
}

#[derive(Clone)]
pub struct CueMark {
    pub ms: u32,
    /// Loop end.
    pub end_ms: Option<u32>,
    pub hot: Option<char>,
    pub color: egui::Color32,
}

/// Hot-cue colour codes (`ColorTableIndex`) offered in the colour menu, with the RGB rekordbox
/// shows for them (`mb_usb::cues::UI_PAD_COLORS`, `djplay::getPadColor`).
pub const HOT_CUE_COLORS: [(u8, [u8; 3]); 8] = {
    let t = mb_usb::cues::UI_PAD_COLORS;
    [(8, t[8]), (19, t[19]), (23, t[23]), (32, t[32]), (36, t[36]), (38, t[38]), (43, t[43]), (60, t[60])]
};

/// Display colour of a hot cue's colour code, as rekordbox shows it (default green without one).
pub fn hot_cue_color(code: Option<u8>) -> egui::Color32 {
    code.filter(|c| (1..=64).contains(c))
        .map_or(HOT_CUE_DEFAULT, |c| {
            let [r, g, b] = mb_usb::cues::UI_PAD_COLORS[usize::from(c)];
            egui::Color32::from_rgb(r, g, b)
        })
}

/// Memory-cue colours by `djmdCue.Color` slot, for display only (rekordbox's RGB values are open).
pub const MEMORY_CUE_COLORS: [[u8; 3]; 8] = [
    [0xF8, 0x70, 0xB4],
    [0xE6, 0x2B, 0x2B],
    [0xF0, 0x8C, 0x28],
    [0xF0, 0xDC, 0x28],
    [0x3C, 0xD2, 0x50],
    [0x28, 0xC8, 0xD2],
    [0x32, 0x6E, 0xF0],
    [0x96, 0x50, 0xE6],
];

pub fn memory_cue_color(slot: Option<u8>) -> egui::Color32 {
    slot.and_then(|s| MEMORY_CUE_COLORS.get(usize::from(s)))
        .map_or(MEMORY_CUE, |[r, g, b]| egui::Color32::from_rgb(*r, *g, *b))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Rgb,
    ThreeBand,
}

/// 3-band style colours: low, mid, high.
const BAND_COLORS: [egui::Color32; 3] =
    [egui::Color32::from_rgb(0x1A, 0x5F, 0xE8), egui::Color32::from_rgb(0xF0, 0x9A, 0x2C), egui::Color32::from_rgb(0xF4, 0xF4, 0xF4)];

/// RGB style colour of three band amplitudes (low → red, mid → green, high → blue), scaled so the
/// strongest channel is full, as the overview's PWV4 columns are coloured.
fn band_rgb(bands: [u8; 3]) -> egui::Color32 {
    let max = u16::from(*bands.iter().max().unwrap_or(&0)).max(1);
    let scale = |v: u8| (u16::from(v) * 255 / max) as u8;
    egui::Color32::from_rgb(scale(bands[0]), scale(bands[1]), scale(bands[2]))
}

const MEMORY_CUE: egui::Color32 = egui::Color32::from_rgb(0xE6, 0x2B, 0x2B);
const HOT_CUE_DEFAULT: egui::Color32 = egui::Color32::from_rgb(0x2D, 0xD7, 0x28);

fn read_sibling(dat: &Path, ext: &str) -> Option<AnlzFile> {
    let data = std::fs::read(dat.with_extension(ext)).ok()?;
    AnlzFile::parse(&data).ok()
}

/// Loads waveforms and cues for the track whose `ANLZ0000.DAT` is at `dat`.
pub fn load(dat: &Path) -> Option<TrackWaves> {
    let ext = read_sibling(dat, "EXT")?;
    let ex2 = read_sibling(dat, "2EX");
    let grid = read_sibling(dat, "DAT").and_then(|f| f.beat_grid()).map(|g| g.beats).unwrap_or_default();
    let color = ext.sections_with(Tag::PWV4).next().and_then(|s| WaveColorPreview::decode(s).ok());
    let scroll = ext.sections_with(Tag::PWV5).next().and_then(|s| WaveColorScroll::decode(s).ok());
    let three_band = ex2
        .as_ref()
        .and_then(|f| f.sections_with(Tag::PWV6).next().and_then(|s| Wave3Band::decode(s).ok()));
    let detail_bands = ex2
        .as_ref()
        .and_then(|f| f.sections_with(Tag::PWV7).next().and_then(|s| Wave3Band::decode(s).ok()))
        .filter(|w| !w.is_empty() && w.scroll.is_some_and(|(rate, _)| rate > 0))
        .map(|w| {
            let peak = w.data.iter().flat_map(|c| c.iter().copied()).max().unwrap_or(1).max(1);
            (w, peak)
        });
    let mut cues = Vec::new();
    for list in ext.cue_lists_ext() {
        push_cues(&mut cues, &list);
    }
    Some(TrackWaves { color, three_band, scroll, detail_bands, grid, cues })
}

fn push_cues(out: &mut Vec<CueMark>, list: &CueListExt) {
    for e in &list.entries {
        let hot = (list.kind == CueKind::Hot).then(|| e.hot_cue_letter()).flatten();
        let color = match (hot, e.color()) {
            (Some(_), Some((_, [r, g, b]))) => egui::Color32::from_rgb(r, g, b),
            (Some(_), None) => HOT_CUE_DEFAULT,
            (None, _) => MEMORY_CUE,
        };
        out.push(CueMark { ms: e.time_ms(), end_ms: e.loop_end_ms(), hot, color });
    }
}

/// Full-track overview with cue markers and an optional playhead; `duration_ms` maps times onto
/// the width. A click returns the clicked position in ms.
pub fn show_overview(
    ui: &mut egui::Ui,
    waves: &TrackWaves,
    cues: &[CueMark],
    duration_ms: u32,
    style: Style,
    playhead_ms: Option<f64>,
    height: f32,
) -> Option<f64> {
    let desired = egui::vec2(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, crate::theme::BG_WELL);
    let mid = rect.center().y;
    let half = rect.height() / 2.0 - 2.0;

    let column = |i: usize, n: usize, height: f32, color: egui::Color32| {
        let x0 = rect.left() + rect.width() * i as f32 / n as f32;
        let x1 = rect.left() + rect.width() * (i + 1) as f32 / n as f32;
        let h = (height * half).max(0.5);
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, mid - h), egui::pos2(x1.max(x0 + 1.0), mid + h)), 0.0, color);
    };

    match (style, &waves.three_band, &waves.color) {
        (Style::ThreeBand, Some(bands), _) => {
            let n = bands.len();
            let peak = bands.data.iter().flat_map(|c| c.iter().copied()).max().unwrap_or(1).max(1) as f32;
            // Draw low, then mid, then high on top, each scaled to its own height.
            for (band, colour) in BAND_COLORS.iter().enumerate() {
                for i in 0..n {
                    let v = bands.data[i][band] as f32 / peak;
                    column(i, n, v, *colour);
                }
            }
        }
        (_, _, Some(preview)) => {
            let n = preview.len();
            let peak = preview.max_height().max(1) as f32;
            for i in 0..n {
                if let Some(([r, g, b], h)) = preview.rgb_column(i) {
                    column(i, n, h as f32 / peak, egui::Color32::from_rgb(r, g, b));
                }
            }
        }
        _ => {}
    }

    if let (Some(ms), true) = (playhead_ms, duration_ms > 0) {
        let x = rect.left() + rect.width() * (ms as f32 / duration_ms as f32).clamp(0.0, 1.0);
        // The part already played is dimmed, as on the players.
        painter.rect_filled(egui::Rect::from_min_max(rect.min, egui::pos2(x, rect.bottom())), 0.0, egui::Color32::from_black_alpha(110));
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.5, egui::Color32::WHITE));
    }
    let clicked = response
        .clicked()
        .then(|| response.interact_pointer_pos())
        .flatten()
        .filter(|_| duration_ms > 0)
        .map(|p| ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * duration_ms as f64);

    if duration_ms > 0 {
        for cue in cues {
            let x = rect.left() + rect.width() * (cue.ms as f32 / duration_ms as f32).min(1.0);
            let top = rect.top();
            painter.line_segment([egui::pos2(x, top), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, cue.color));
            let marker = vec![egui::pos2(x - 4.0, top), egui::pos2(x + 4.0, top), egui::pos2(x, top + 6.0)];
            painter.add(egui::Shape::convex_polygon(marker, cue.color, egui::Stroke::NONE));
            if let Some(letter) = cue.hot {
                painter.text(
                    egui::pos2(x + 3.0, top + 6.0),
                    egui::Align2::LEFT_TOP,
                    letter,
                    egui::FontId::proportional(9.0),
                    egui::Color32::BLACK,
                );
            }
        }
    }
    clicked
}

/// Zoomed strip around `center_ms` (± `half_span_ms`): PWV5 columns, beat grid (downbeats
/// brighter), cue markers with loop spans, and the position line in the middle. A click returns
/// the clicked position in ms.
pub fn show_detail(
    ui: &mut egui::Ui,
    waves: &TrackWaves,
    cues: &[CueMark],
    center_ms: f64,
    half_span_ms: f64,
    height: f32,
    style: Style,
) -> Option<f64> {
    let desired = egui::vec2(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, crate::theme::BG_WELL);
    let start = center_ms - half_span_ms;
    let span = 2.0 * half_span_ms;
    let x_of = |ms: f64| rect.left() + rect.width() * ((ms - start) / span) as f32;
    let mid = rect.center().y;
    let half = rect.height() / 2.0 - 2.0;
    // Columns `first..last` of a wave with `rate` columns per second that fall into the strip.
    let visible = |rate: u16, len: usize| {
        let per_ms = f64::from(rate.max(1)) / 1000.0;
        let first = ((start * per_ms).floor().max(0.0)) as usize;
        let last = (((start + span) * per_ms).ceil().max(0.0) as usize).min(len);
        (per_ms, first, last)
    };
    let bar = |x0: f32, x1: f32, h: f32, color: egui::Color32| {
        let h = h.max(0.5);
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, mid - h), egui::pos2(x1, mid + h)), 0.0, color);
    };
    match (&waves.detail_bands, &waves.scroll) {
        (Some((bands, peak)), _) => {
            let (per_ms, first, last) = visible(bands.scroll.map_or(150, |(rate, _)| rate), bands.len());
            let peak = f32::from(*peak);
            for i in first..last {
                let c = bands.data[i];
                let x0 = x_of(i as f64 / per_ms);
                let x1 = x_of((i + 1) as f64 / per_ms).max(x0 + 1.0);
                match style {
                    Style::ThreeBand => {
                        for (band, colour) in BAND_COLORS.iter().enumerate() {
                            bar(x0, x1, f32::from(c[band]) / peak * half, *colour);
                        }
                    }
                    Style::Rgb => bar(x0, x1, f32::from(c[0].max(c[1]).max(c[2])) / peak * half, band_rgb(c)),
                }
            }
        }
        (None, Some(scroll)) => {
            let (per_ms, first, last) = visible(scroll.columns_per_second, scroll.len());
            for i in first..last {
                let Some(col) = scroll.column(i) else { continue };
                let x0 = x_of(i as f64 / per_ms);
                let x1 = x_of((i + 1) as f64 / per_ms).max(x0 + 1.0);
                let [r, g, b] = col.rgb8();
                bar(x0, x1, f32::from(col.height) / 31.0 * half, egui::Color32::from_rgb(r, g, b));
            }
        }
        (None, None) => {}
    }
    for beat in &waves.grid {
        let ms = f64::from(beat.time_ms);
        if ms < start || ms > start + span {
            continue;
        }
        let x = x_of(ms);
        let (stroke, top) = if beat.beat_in_bar == 1 {
            (egui::Stroke::new(1.5, egui::Color32::from_rgb(0xE0, 0x30, 0x30)), rect.top())
        } else {
            (egui::Stroke::new(1.0, egui::Color32::from_white_alpha(110)), rect.top() + rect.height() * 0.6)
        };
        painter.line_segment([egui::pos2(x, top), egui::pos2(x, rect.bottom())], stroke);
    }
    for cue in cues {
        let x = x_of(f64::from(cue.ms));
        if let Some(end) = cue.end_ms {
            let x1 = x_of(f64::from(end));
            painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x1, rect.bottom())), 0.0, cue.color.gamma_multiply(0.25));
        }
        if x < rect.left() || x > rect.right() {
            continue;
        }
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.5, cue.color));
        let marker = vec![egui::pos2(x - 5.0, rect.top()), egui::pos2(x + 5.0, rect.top()), egui::pos2(x, rect.top() + 7.0)];
        painter.add(egui::Shape::convex_polygon(marker, cue.color, egui::Stroke::NONE));
        if let Some(letter) = cue.hot {
            painter.text(egui::pos2(x + 4.0, rect.top() + 7.0), egui::Align2::LEFT_TOP, letter, egui::FontId::proportional(10.0), cue.color);
        }
    }
    let cx = rect.center().x;
    painter.line_segment([egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())], egui::Stroke::new(1.5, egui::Color32::WHITE));
    for (y, dir) in [(rect.top(), 1.0), (rect.bottom(), -1.0)] {
        let tip = vec![egui::pos2(cx - 4.0, y), egui::pos2(cx + 4.0, y), egui::pos2(cx, y + 4.0 * dir)];
        painter.add(egui::Shape::convex_polygon(tip, egui::Color32::WHITE, egui::Stroke::NONE));
    }
    response
        .clicked()
        .then(|| response.interact_pointer_pos())
        .flatten()
        .map(|p| (start + f64::from((p.x - rect.left()) / rect.width()) * span).max(0.0))
}
